Set-StrictMode -Version Latest
Import-Module (Join-Path $PSScriptRoot 'ContestPaths.psm1')

function Resolve-ContestCollectorStateDirectory {
    param(
        [Parameter(Mandatory)] [string]$DataDirectory,
        [string]$ExplicitStateDirectory,
        [string]$UserProfile=$env:USERPROFILE,
        [string]$LocalAppData=$env:LOCALAPPDATA,
        [string[]]$LegacyRoots
    )
    # An explicit fixture/operator state path retains its existing contract.
    Assert-ContestSafePath $DataDirectory
    if($ExplicitStateDirectory){Assert-ContestSafePath $ExplicitStateDirectory;return [IO.Path]::GetFullPath($ExplicitStateDirectory)}
    if([string]::IsNullOrWhiteSpace($UserProfile)){throw 'Collector state migration_required: USERPROFILE is unavailable.'}
    $profile=[IO.Path]::GetFullPath($UserProfile).TrimEnd('\')
    $data=[IO.Path]::GetFullPath($DataDirectory).TrimEnd('\')
    $algorithm=[Security.Cryptography.SHA256]::Create()
    try{$hash=$algorithm.ComputeHash([Text.Encoding]::UTF8.GetBytes($data.ToLowerInvariant()))}finally{$algorithm.Dispose()}
    $suffix=[BitConverter]::ToString($hash).Replace('-','').Substring(0,16).ToLowerInvariant()
    $stateDirectory=Join-Path $profile ('.dot-calendar-collector\'+$suffix)
    Assert-ContestSafePath $stateDirectory
    if(-not $PSBoundParameters.ContainsKey('LegacyRoots')){
        $roots=[Collections.Generic.List[string]]::new()
        if($LocalAppData){$roots.Add((Join-Path $LocalAppData 'DotCalendarCollector'))}
        $roots.Add((Join-Path $profile 'AppData\Local\DotCalendarCollector'))
        # Query only the collector target-folder metadata in package caches;
        # package identity and unrelated application contents are not assumed.
        $packages=Join-Path $profile 'AppData\Local\Packages'
        if(Test-Path -LiteralPath $packages -PathType Container){
            $packageDirectories=@(Get-ChildItem -LiteralPath $packages -Directory)
            if($packageDirectories.Count -gt 256){throw 'Collector state migration_required: package-cache target metadata exceeds the bounded probe.'}
            foreach($package in $packageDirectories){
                $target=Join-Path $package.FullName 'LocalCache\Local\DotCalendarCollector'
                if(Test-Path -LiteralPath $target -PathType Container){$roots.Add($target)}
            }
        }
        $LegacyRoots=@($roots.ToArray())
    }
    $groups=@{}
    foreach($legacyRoot in @($LegacyRoots|Where-Object {$_}|Sort-Object -Unique)){
        if(-not(Test-Path -LiteralPath $legacyRoot)){continue}
        if(-not(Test-Path -LiteralPath $legacyRoot -PathType Container)){throw 'Collector state migration_required: legacy state root is not a directory.'}
        foreach($bucket in @(Get-ChildItem -LiteralPath $legacyRoot -Directory)){
            if($bucket.Name -notmatch '^[a-fA-F0-9]{16}$'){throw 'Collector state migration_required: unrecognized legacy bucket; no empty state was created.'}
            $files=@(foreach($name in @('state.json','exclusions.json')){
                $path=Join-Path $bucket.FullName $name
                if(Test-Path -LiteralPath $path){
                    if(-not(Test-Path -LiteralPath $path -PathType Leaf) -or (Get-Item -LiteralPath $path).Length -gt 4MB){throw 'Collector state migration_required: legacy state cannot be verified.'}
                    [pscustomobject]@{name=$name;sha256=(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash}
                }
            })
            if(-not $files.Count){continue}
            # Logical and package-cache aliases of identical files are one source.
            $key=$bucket.Name.ToLowerInvariant()+'|'+(($files|ForEach-Object {$_.name+':'+$_.sha256}) -join '|')
            if(-not $groups.ContainsKey($key)){$groups[$key]=[pscustomobject]@{directories=[Collections.Generic.List[string]]::new();files=$files}}
            $groups[$key].directories.Add([IO.Path]::GetFullPath($bucket.FullName).TrimEnd('\'))
        }
    }
    $migrationPath=Join-Path $stateDirectory 'migration.json'
    if(-not $groups.Count -and -not(Test-Path -LiteralPath $migrationPath)){return $stateDirectory}
    if(-not(Test-Path -LiteralPath $migrationPath -PathType Leaf)){
        throw 'Collector state migration_required: legacy state/exclusions exist; prepare and verify the shared bucket before any calendar call.'
    }
    if((Get-Item -LiteralPath $migrationPath).Length -gt 1MB){throw 'Collector state migration_required: migration record exceeds1MiB.'}
    try{
        $migration=[IO.File]::ReadAllText($migrationPath,[Text.Encoding]::UTF8)|ConvertFrom-Json -Depth 30 -AsHashtable
        if($migration.schemaVersion -ne 1 -or [IO.Path]::GetFullPath([string]$migration.dataDirectory).TrimEnd('\') -ine $data -or -not $migration.legacySources){throw 'Migration identity is incomplete or belongs to another calendar directory.'}
        # A recorded source disappearing is a stale migration, including when
        # all legacy files disappear. Do not reduce the required file set to
        # whatever is still discoverable today.
        foreach($source in $migration.legacySources){
            $directory=[IO.Path]::GetFullPath([string]$source.directory).TrimEnd('\')
            $allowed=@($LegacyRoots|Where-Object {
                $_ -and [IO.Path]::GetDirectoryName($directory) -ieq [IO.Path]::GetFullPath($_).TrimEnd('\')
            })
            if(-not $allowed.Count -or [IO.Path]::GetFileName($directory) -notmatch '^[a-fA-F0-9]{16}$' -or -not $source.files){throw 'Recorded legacy source is outside the bounded bucket roots or has no file list.'}
            $seen=[Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
            foreach($file in $source.files){
                if($file.name -cnotin @('state.json','exclusions.json') -or -not $seen.Add([string]$file.name) -or [string]$file.sha256 -cnotmatch '^[A-F0-9]{64}$'){throw 'Recorded legacy file identity is invalid.'}
                $original=Join-Path $directory $file.name
                $target=Join-Path $stateDirectory $file.name
                if(-not(Test-Path -LiteralPath $original -PathType Leaf) -or (Get-FileHash -LiteralPath $original -Algorithm SHA256).Hash -cne $file.sha256){throw 'A recorded legacy source disappeared or changed.'}
                if(-not(Test-Path -LiteralPath $target -PathType Leaf) -or (Get-Item -LiteralPath $target).Length -eq 0){throw 'A recorded required target document disappeared or is empty.'}
            }
        }
        foreach($group in $groups.Values){
            $matched=@($migration.legacySources|Where-Object {
                $_ -is [Collections.IDictionary] -and $_.directory -and
                [IO.Path]::GetFullPath([string]$_.directory).TrimEnd('\') -in @($group.directories.ToArray())
            })
            if($matched.Count -ne 1){throw 'A legacy bucket has no unique reviewed mapping to the new calendar directory.'}
            foreach($file in $group.files){
                $recorded=@($matched[0].files|Where-Object {$_.name -ceq $file.name})
                if($recorded.Count -ne 1 -or [string]$recorded[0].sha256 -cne $file.sha256){throw 'Legacy state changed or its preservation hash is missing.'}
                $target=Join-Path $stateDirectory $file.name
                if(-not(Test-Path -LiteralPath $target -PathType Leaf) -or (Get-Item -LiteralPath $target).Length -eq 0){throw 'A preserved state/exclusion target is missing or empty.'}
                # The reviewed marker identifies the preserved bootstrap.
                # Target state/exclusions may later change through user actions;
                # source hashes and required target documents still must exist.
                $legacy=[IO.File]::ReadAllText((Join-Path $matched[0].directory $file.name),[Text.Encoding]::UTF8)|ConvertFrom-Json -Depth 60 -AsHashtable
                $current=[IO.File]::ReadAllText($target,[Text.Encoding]::UTF8)|ConvertFrom-Json -Depth 60 -AsHashtable
                if($legacy.schemaVersion -ne 1 -or $current.schemaVersion -ne 1 -or $legacy.owner -cne 'parent' -or $current.owner -cne 'parent'){throw 'Preserved state/exclusion schema is invalid.'}
                $array=if($file.name -ceq 'exclusions.json'){'contests'}else{'entries'}
                if(-not $legacy.Contains($array) -or -not $current.Contains($array) -or $legacy[$array] -isnot [Collections.IList] -or $current[$array] -isnot [Collections.IList]){throw 'Preserved state/exclusion arrays are missing or invalid.'}
            }
        }
    }catch{throw ('Collector state migration_required: '+$_.Exception.Message)}
    return $stateDirectory
}


Export-ModuleMember -Function Resolve-ContestCollectorStateDirectory
