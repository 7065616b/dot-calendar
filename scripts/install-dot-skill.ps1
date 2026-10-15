param(
    [string]$ExePath,
    [string]$DataDirectory,
    [string]$SkillsDirectory
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Get-AbsoluteDirectory([string]$Value, [string]$Name) {
    if ([string]::IsNullOrEmpty($Value)) {
        throw "$Name is unavailable; specify an absolute DataDirectory or DOT_CALENDAR_DATA_DIR."
    }
    $root = [IO.Path]::GetPathRoot($Value)
    # IsPathRooted alone also accepts C:relative and \rooted-without-drive.
    $driveRoot = $root.Length -ge 3 -and $root[1] -eq ':' -and $root[2] -in @('\', '/')
    $uncRoot = $root.StartsWith('\\', [StringComparison]::Ordinal)
    if (-not ($driveRoot -or $uncRoot)) { throw "$Name must be an absolute path." }
    return [IO.Path]::GetFullPath($Value)
}

function Get-CalendarPathKind([string]$Path) {
    try { $attributes = [IO.File]::GetAttributes($Path) }
    catch [IO.FileNotFoundException] { return 'Missing' }
    catch [IO.DirectoryNotFoundException] { return 'Missing' }
    if ($attributes -band [IO.FileAttributes]::ReparsePoint) { return 'Other' }
    if ($attributes -band [IO.FileAttributes]::Directory) { return 'Directory' }
    return 'File'
}

function Assert-RegularDirectory([string]$Path) {
    $kind = Get-CalendarPathKind $Path
    if ($kind -notin @('Missing', 'Directory')) {
        throw "Calendar data directory is not a regular directory: $Path"
    }
    return $kind
}

function Test-StoreTrace([string]$Directory) {
    foreach ($name in @('calendar.json', 'calendar.initialized', 'deleted')) {
        if ((Get-CalendarPathKind (Join-Path $Directory $name)) -ne 'Missing') { return $true }
    }
    return $false
}

function Test-LegacySetting([string]$Path) {
    switch (Get-CalendarPathKind $Path) {
        'Missing' { return $false }
        'File' { return $true }
        default { throw "Legacy calendar settings path is not a regular file: $Path" }
    }
}

function Resolve-CalendarDataDirectory([string]$ExplicitDirectory) {
    if (-not [string]::IsNullOrEmpty($ExplicitDirectory)) {
        return Get-AbsoluteDirectory $ExplicitDirectory 'DataDirectory'
    }
    if (-not [string]::IsNullOrEmpty($env:DOT_CALENDAR_DATA_DIR)) {
        return Get-AbsoluteDirectory $env:DOT_CALENDAR_DATA_DIR 'DOT_CALENDAR_DATA_DIR'
    }
    $profileDirectory = Get-AbsoluteDirectory $env:USERPROFILE 'USERPROFILE'
    $shared = Join-Path $profileDirectory '.dot-calendar'
    [void](Assert-RegularDirectory $shared)
    if (Test-StoreTrace $shared) { return $shared }

    $legacy = Join-Path (Get-AbsoluteDirectory $env:LOCALAPPDATA 'LOCALAPPDATA') 'DotCalendar'
    $legacyKind = Assert-RegularDirectory $legacy
    $legacyPresent = Test-StoreTrace $legacy
    if (-not $legacyPresent) {
        foreach ($name in @('widget.json', 'window.json', 'google-config.json', 'google-tokens.dpapi')) {
            if (Test-LegacySetting (Join-Path $legacy $name)) { $legacyPresent = $true; break }
        }
    }
    if (-not $legacyPresent -and $legacyKind -eq 'Directory') {
        foreach ($entry in [IO.Directory]::EnumerateFileSystemEntries($legacy)) {
            $name = [IO.Path]::GetFileName($entry)
            if ($name -like 'google-sync-*.json' -and (Test-LegacySetting $entry)) {
                $legacyPresent = $true
                break
            }
        }
    }
    if ($legacyPresent) {
        throw "Calendar migration required: existing data or settings at $legacy must be reviewed and explicitly migrated to $shared; no data was copied or initialized."
    }
    return $shared
}

function Assert-NoReparsePath([string]$Path) {
    $part = [IO.Path]::GetFullPath($Path)
    while ($null -ne $part) {
        if ((Get-CalendarPathKind $part) -eq 'Other') {
            throw "Refusing a redirected connection path: $part"
        }
        $parent = [IO.DirectoryInfo]::new($part).Parent
        $part = if ($null -ne $parent) { $parent.FullName } else { $null }
    }
}

function Write-PreservedFile($Plan) {
    Assert-NoReparsePath $Plan.Target
    $currentKind = Get-CalendarPathKind $Plan.Target
    if ($Plan.HasExisting) {
        if ($currentKind -ne 'File') { throw "Connection target changed since preflight: $($Plan.Target)" }
        $current = [IO.File]::ReadAllBytes($Plan.Target)
        if ([Convert]::ToBase64String($current) -cne [Convert]::ToBase64String($Plan.Existing)) {
            throw "Connection target changed since preflight; preserved current bytes: $($Plan.Target)"
        }
    } elseif ($currentKind -ne 'Missing') {
        throw "Connection target appeared since preflight; preserved current file: $($Plan.Target)"
    }
    if ($Plan.HasExisting -and [Convert]::ToBase64String($Plan.Existing) -ceq [Convert]::ToBase64String($Plan.Bytes)) { return }
    $parent = Split-Path -Parent $Plan.Target
    [void][IO.Directory]::CreateDirectory($parent)
    Assert-NoReparsePath $Plan.Target
    $nonce = [guid]::NewGuid().ToString('N')
    if ($Plan.HasExisting) {
        $backup = $Plan.Target + '.' + $nonce + '.backup'
        $stream = [IO.File]::Open($backup, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
        try { $stream.Write($Plan.Existing, 0, $Plan.Existing.Length); $stream.Flush($true) }
        finally { $stream.Dispose() }
    }
    $temporary = Join-Path $parent ('.dot-calendar-' + $nonce + '.tmp')
    try {
        $stream = [IO.File]::Open($temporary, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
        try { $stream.Write($Plan.Bytes, 0, $Plan.Bytes.Length); $stream.Flush($true) }
        finally { $stream.Dispose() }
        if ($Plan.HasExisting) { [IO.File]::Replace($temporary, $Plan.Target, $null) }
        else { [IO.File]::Move($temporary, $Plan.Target) }
    } finally {
        if ((Get-CalendarPathKind $temporary) -ne 'Missing') { [IO.File]::Delete($temporary) }
    }
}

# Resolve the storage path before creating skills, backups or connection files.
# The default resolver probes legacy traces but never copies or initializes data.
$DataDirectory = Resolve-CalendarDataDirectory $DataDirectory
$packageRoot = Split-Path -Parent $PSScriptRoot
$source = Join-Path $packageRoot 'integration\dot-calendar'
if (-not $ExePath) {
    $ExePath = Join-Path $packageRoot 'dot-calendar.exe'
    if (-not (Test-Path -LiteralPath $ExePath -PathType Leaf)) {
        $ExePath = Join-Path $packageRoot 'dist\dot-calendar.exe'
    }
}
$ExePath = (Resolve-Path -LiteralPath $ExePath).Path
Assert-NoReparsePath $ExePath
if ((Get-CalendarPathKind $ExePath) -ne 'File') { throw 'ExePath must be an existing regular executable file.' }
if (-not $SkillsDirectory) {
    $SkillsDirectory = Join-Path (Get-AbsoluteDirectory $env:USERPROFILE 'USERPROFILE') '.agents\skills'
}
$destination = Join-Path (Get-AbsoluteDirectory $SkillsDirectory 'SkillsDirectory') 'dot-calendar'
$utf8 = [Text.UTF8Encoding]::new($false)
$plans = @()
foreach ($file in @('SKILL.md', 'agents\openai.yaml', 'scripts\invoke-calendar.ps1', 'connection.json')) {
    $target = Join-Path $destination $file
    Assert-NoReparsePath $target
    if ($file -eq 'connection.json') {
        $config = [ordered]@{ executable_path = $ExePath; data_directory = $DataDirectory } | ConvertTo-Json
        $bytes = $utf8.GetBytes($config)
    } else {
        $sourcePath = Join-Path $source $file
        Assert-NoReparsePath $sourcePath
        if ((Get-CalendarPathKind $sourcePath) -ne 'File') { throw "Missing regular skill source: $file" }
        $bytes = [IO.File]::ReadAllBytes($sourcePath)
    }
    $kind = Get-CalendarPathKind $target
    if ($kind -notin @('Missing', 'File')) { throw "Connection target is not a regular file: $target" }
    $existing = if ($kind -eq 'File') { ,([IO.File]::ReadAllBytes($target)) } else { $null }
    $plans += [pscustomobject]@{ Target=$target; Bytes=$bytes; Existing=$existing; HasExisting=($kind -eq 'File') }
}
# Publish only the three maintained files and connection, preserving changed bytes.
foreach ($plan in $plans) { Write-PreservedFile $plan }
Write-Output "Installed Dot Calendar skill: $destination"
Write-Output "Calendar data: $DataDirectory"
Write-Output 'Keep the executable at this location; rerun this installer after moving it.'
Write-Output 'In Your dot, ask to use the connected PC and dot-calendar skill to read today first.'
