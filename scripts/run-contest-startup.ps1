param(
    [string]$HelperPath=(Join-Path $env:USERPROFILE '.agents\skills\dot-calendar\scripts\invoke-calendar.ps1'),
    [string]$ExpectedDataDirectory,
    [string]$StateDirectory,
    [ValidateSet('Preview','Apply')][string]$Mode='Preview',
    [ValidateSet('linkareer','dacon','mokpo','daker','aifactory')][string[]]$Sources=@('linkareer','dacon','mokpo','daker','aifactory'),
    [int]$WatchProcessId=0,
    [ValidateRange(1,1800)][int]$MaxRunSeconds=1800
)
#Requires -Version 7.0
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$OutputEncoding=[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
$activeClock=[Diagnostics.Stopwatch]::StartNew()
$deadline=[datetimeoffset]::UtcNow.AddSeconds($MaxRunSeconds)
$moduleRoot=Join-Path $PSScriptRoot 'contest-collector'
foreach($name in @('ContestPaths','ContestConnection','ContestSources','ContestExtract','ContestNormalize','ContestImport','ContestSchedule')){
    Import-Module (Join-Path $moduleRoot ($name+'.psm1')) -Force
}
Assert-ContestSafePath $HelperPath
$HelperPath=[IO.Path]::GetFullPath($HelperPath)
$connectionPath=Join-Path ([IO.Path]::GetDirectoryName([IO.Path]::GetDirectoryName($HelperPath))) 'connection.json'
Assert-ContestSafePath $connectionPath
if((Get-Item -LiteralPath $connectionPath).Length -gt 64KB){throw 'Calendar connection exceeds the configuration limit.'}
$connection=Get-Content -LiteralPath $connectionPath -Raw|ConvertFrom-Json
Assert-ContestSafePath ([string]$connection.data_directory)
Assert-ContestSafePath ([string]$connection.executable_path)
$calendarDirectory=[IO.Path]::GetFullPath([string]$connection.data_directory).TrimEnd('\')
if($ExpectedDataDirectory -and $calendarDirectory -ine [IO.Path]::GetFullPath($ExpectedDataDirectory).TrimEnd('\')){throw 'Startup collector connection differs from the running calendar; nothing was imported.'}
if(-not(Test-Path -LiteralPath $HelperPath -PathType Leaf) -or -not(Test-Path -LiteralPath $connection.executable_path -PathType Leaf)){throw 'Calendar connection is unavailable.'}
$state=Resolve-ContestCollectorStateDirectory -DataDirectory $calendarDirectory -ExplicitStateDirectory $StateDirectory
if($state.TrimEnd('\') -ieq $calendarDirectory -or $state.StartsWith($calendarDirectory+'\',[StringComparison]::OrdinalIgnoreCase) -or $calendarDirectory.StartsWith($state.TrimEnd('\')+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Collector state must be separate from the calendar directory.'}
$parent=$null
if($WatchProcessId){
    $parent=[Diagnostics.Process]::GetProcessById($WatchProcessId)
    # Open and retain its handle so PID reuse cannot attach this worker to a
    # different process. Only the connected GUI executable may own a watcher.
    $null=$parent.Handle
    if($parent.HasExited -or [IO.Path]::GetFullPath($parent.MainModule.FileName) -ine [IO.Path]::GetFullPath($connection.executable_path)){throw 'Watcher parent does not match the pinned calendar executable.'}
}
$invoke={param($Tool,$Arguments)
    if($Tool -notin @('calendar_list','calendar_deleted','calendar_create','calendar_update')){throw 'Unsupported collector operation.'}
    $json=if($null -eq $Arguments){'{}'}else{$Arguments|ConvertTo-Json -Depth 40 -Compress}
    $raw=& $HelperPath -Tool $Tool -ArgumentsJson $json
    return ($raw|ConvertFrom-Json -Depth 40)
}.GetNewClosure()
$pipeline={param($source,$runDirectory,$runId)
    foreach($name in @('ContestSources','ContestExtract','ContestNormalize','ContestImport')){Import-Module (Join-Path $moduleRoot ($name+'.psm1')) -Force}
    if($null -ne $parent -and $parent.HasExited){throw 'Calendar exited before collection.'}
    if($activeClock.Elapsed.TotalSeconds -ge $MaxRunSeconds){throw 'Active collection time budget expired.'}
    $collection=Get-ContestSourcePages -Sources @($source) -MaxDetails 150 -LatestList -DeadlineUtc $deadline
    [IO.File]::WriteAllText((Join-Path $runDirectory 'source-pages.json'),($collection|ConvertTo-Json -Depth 30),[Text.UTF8Encoding]::new($false))
    if(@($collection.sourceResults|Where-Object status -ne 'ok').Count){
        $sourceError=@($collection.sourceResults|ForEach-Object error)-join ' '
        return [pscustomobject]@{status='source_error';freshSource=$false;error=$sourceError;retryable=($sourceError -notmatch 'robots[.]txt disallows')}
    }
    $inputBatch=ConvertFrom-ContestPages -Collection $collection -RunId $runId
    $candidatePath=Join-Path $runDirectory 'candidates.json'
    [IO.File]::WriteAllText($candidatePath,($inputBatch|ConvertTo-Json -Depth 30),[Text.UTF8Encoding]::new($false))
    $candidateHash=(Get-FileHash -LiteralPath $candidatePath).Hash
    $batch=ConvertTo-ContestBatch -InputObject $inputBatch
    $preview=Invoke-ContestImport -Batch $batch -StateDirectory $state -CalendarInvoker $invoke -Mode Preview
    [IO.File]::WriteAllText((Join-Path $runDirectory 'preview.json'),($preview|ConvertTo-Json -Depth 30),[Text.UTF8Encoding]::new($false))
    if($preview.status -in @('offline','blocked') -or $preview.counts.sourceErrors -gt 0){throw 'Preview did not verify the live calendar/source. No Apply was attempted.'}
    if($Mode -ne 'Apply'){
        return [pscustomobject]@{status='preview';freshSource=$true;pending=$preview.counts.pending;error='Preview only; the daily success marker was not advanced.'}
    }
    if($activeClock.Elapsed.TotalSeconds -ge $MaxRunSeconds){throw 'Active time budget expired before Apply; preview was retained.'}
    if($null -ne $parent -and $parent.HasExited){throw 'Calendar exited before Apply.'}
    $session=[Diagnostics.Process]::GetCurrentProcess().SessionId
    $gui=@(Get-CimInstance Win32_Process -Filter "Name='dot-calendar.exe'"|Where-Object {$_.ExecutablePath -ieq $connection.executable_path -and $_.CommandLine -notmatch '(?:^|\s)--mcp(?:\s|$)'})
    if(@($gui|Where-Object SessionId -ne $session).Count){throw 'Calendar runs in another Windows session; no import was attempted.'}
    # Preserve a fresh recovery copy before every Apply, including retained
    # deletion/request IDs and importer protection/exclusion records.
    $backup=Join-Path $runDirectory 'before-apply';[void][IO.Directory]::CreateDirectory($backup)
    $items=@(Get-Item -LiteralPath (Join-Path $calendarDirectory 'calendar.json'))
    $deleted=Join-Path $calendarDirectory 'deleted';if(Test-Path -LiteralPath $deleted){$items+=@(Get-ChildItem -LiteralPath $deleted -File)}
    foreach($name in @('state.json','exclusions.json','migration.json')){$path=Join-Path $state $name;if(Test-Path -LiteralPath $path){$items+=Get-Item -LiteralPath $path}}
    $manifest=@(foreach($item in $items){
        Assert-ContestSafePath $item.FullName
        $hash=(Get-FileHash -LiteralPath $item.FullName).Hash
        $relative=if($item.FullName.StartsWith($calendarDirectory+'\',[StringComparison]::OrdinalIgnoreCase)){'calendar\'+[IO.Path]::GetRelativePath($calendarDirectory,$item.FullName)}else{'collector\'+$item.Name}
        $destination=Join-Path $backup $relative;[void][IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($destination));[IO.File]::Copy($item.FullName,$destination,$false)
        if((Get-FileHash -LiteralPath $destination).Hash -cne $hash -or (Get-FileHash -LiteralPath $item.FullName).Hash -cne $hash){throw 'A recovery source changed during copying; Apply was stopped.'}
        [pscustomobject]@{path=$relative;sha256=$hash}
    })
    [IO.File]::WriteAllText((Join-Path $backup 'manifest.json'),($manifest|ConvertTo-Json -Depth 6),[Text.UTF8Encoding]::new($false))
    if((Get-FileHash -LiteralPath $candidatePath).Hash -cne $candidateHash){throw 'Candidate batch changed after Preview.'}
    $applied=Invoke-ContestImport -Batch $batch -StateDirectory $state -CalendarInvoker $invoke -Mode Apply
    [IO.File]::WriteAllText((Join-Path $runDirectory 'apply.json'),($applied|ConvertTo-Json -Depth 30),[Text.UTF8Encoding]::new($false))
    $failures=@($applied.operations|Where-Object {$_.action -match 'error|locked' -or $_.reason -match 'conflict|failed|offline|uncertain write|response lost'})
    if($applied.status -in @('offline','blocked') -or $applied.counts.sourceErrors -gt 0 -or $failures.Count){throw 'Apply did not finish with verified writes; success day was not advanced.'}
    # Review items are a preserved result, not a failed HTTP request or a claim
    # that all candidates were added. Uncertain entries remain unwritable.
    return [pscustomobject]@{status='complete';freshSource=$true;counts=$applied.counts;candidateSha256=$candidateHash;error=$null}
}.GetNewClosure()
try{
    for($pass=0;$pass -lt 5 -and $activeClock.Elapsed.TotalSeconds -lt $MaxRunSeconds;$pass++){
        $canContinue={ $activeClock.Elapsed.TotalSeconds -lt $MaxRunSeconds -and ($null -eq $parent -or -not $parent.HasExited) }.GetNewClosure()
        $result=Invoke-ContestDailyRun -StateDirectory $state -Pipeline $pipeline -Sources $Sources -CanContinue $canContinue
        $result|ConvertTo-Json -Depth 20
        if($null -eq $parent -or $parent.HasExited -or $result.status -in @('complete','locked')){break}
        $remaining=$MaxRunSeconds-$activeClock.Elapsed.TotalSeconds
        if($result.nextCheckSeconds -ge $remaining){break}
        # Only retry an incomplete active task. Completed work never waits for
        # tomorrow. The GUI's existing date/resume/user events start a new task.
        if($parent.WaitForExit([int]$result.nextCheckSeconds*1000)){break}
    }
}finally{if($null -ne $parent){$parent.Dispose()}}
