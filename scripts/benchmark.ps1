param(
    [string] $ExePath = (Join-Path $PSScriptRoot '..\dist\dot-calendar.exe'),
    [ValidatePattern('^[A-Za-z0-9_-]{1,40}$')] [string] $Label = 'current',
    [ValidateRange(1, 20)] [int] $Iterations = 3,
    [ValidateRange(1, 60)] [int] $IdleSeconds = 5,
    [ValidateRange(100, 255)] [int] $Opacity = 255,
    [ValidateRange(840, 7680)] [int] $WindowWidth = 1100,
    [ValidateRange(530, 4320)] [int] $WindowHeight = 730,
    [switch] $Desktop,
    [switch] $Dark
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$workspace = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$ExePath = [IO.Path]::GetFullPath($ExePath)
$boundary = $workspace.TrimEnd('\') + '\'
if (-not $ExePath.StartsWith($boundary, [StringComparison]::OrdinalIgnoreCase) -or
    -not (Test-Path -LiteralPath $ExePath -PathType Leaf)) {
    throw "Executable must exist inside the workspace: $ExePath"
}

$qa = Join-Path $workspace 'qa-data'
$runRoot = Join-Path $qa ('benchmark-' + $Label + '-' + [guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory -Path $runRoot -Force)
$baseDate = [datetime]::Today.AddDays(1 - [datetime]::Today.Day - 70)
$dateCulture = [Globalization.CultureInfo]::InvariantCulture
$utf8 = [Text.UTF8Encoding]::new($false)
if (-not ('CalendarBenchCounters' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class CalendarBenchCounters {
    [StructLayout(LayoutKind.Sequential)]
    public struct Io {
        public ulong ReadOperations, WriteOperations, OtherOperations;
        public ulong ReadBytes, WriteBytes, OtherBytes;
    }
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern bool GetProcessIoCounters(IntPtr process, out Io counters);
    [DllImport("user32.dll")]
    public static extern uint GetGuiResources(IntPtr process, uint flags);
}
'@
}
$prefs = [ordered]@{
    position = @(80, 80, $WindowWidth, $WindowHeight); opacity = $Opacity; dark = $Dark.IsPresent
    desktop = $Desktop.IsPresent; locked = $false; lunar = $true; holidays = $true
    auto_start = $false; sync_minutes = 5; day_colors = @{}
} | ConvertTo-Json -Depth 5 -Compress

function New-Events([string] $CaseName) {
    $count = switch ($CaseName) { 'empty' { 0 } 'typical100' { 100 } 'stress500daily' { 500 } }
    $events = [Collections.Generic.List[object]]::new()
    for ($i = 0; $i -lt $count; $i++) {
        $daily = $CaseName -eq 'stress500daily'
        $weekly = $CaseName -eq 'typical100' -and $i % 2 -eq 1
        $dayOffset = if ($daily) { $i % 14 } elseif ($weekly) { $i % 28 } else { $i % 84 }
        $events.Add([ordered]@{
            id = 'bench-{0:d4}' -f $i
            date = $baseDate.AddDays($dayOffset).ToString('yyyy-MM-dd', $dateCulture)
            time = '{0:d2}:00' -f (8 + $i % 10)
            title = 'Benchmark event {0:d4}' -f $i
            notes = if ($daily) { 'n' * 4096 } else { 'Short benchmark note' }
            request_id = $null
            completed = $false
            color = $null
            recurrence = if ($daily) { 'daily' } elseif ($weekly) { 'weekly' } else { $null }
            reminder_minutes = $null
        })
    }
    return $events.ToArray()
}

function Get-Median([double[]] $Values) {
    $sorted = @($Values | Sort-Object)
    $mid = [int][Math]::Floor($sorted.Count / 2)
    if ($sorted.Count % 2) { return [double]$sorted[$mid] }
    return ([double]$sorted[$mid - 1] + [double]$sorted[$mid]) / 2
}

function Measure-Run([string] $DataDir) {
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $ExePath
    $start.WorkingDirectory = $workspace
    $start.UseShellExecute = $false
    $start.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
    $start.Environment['DOT_CALENDAR_DATA_DIR'] = $DataDir
    if ($Desktop) { [void]$start.Environment.Remove('DOT_CALENDAR_WINDOWED') }
    else { $start.Environment['DOT_CALENDAR_WINDOWED'] = '1' }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $start
    $clock = [Diagnostics.Stopwatch]::StartNew()
    try {
        if (-not $process.Start()) { throw "Could not start $ExePath" }
        if (-not $process.WaitForInputIdle(15000)) { throw "UI did not become input-idle within 15 seconds (PID $($process.Id))" }
        $readyMs = $clock.Elapsed.TotalMilliseconds
        Start-Sleep -Seconds 2
        $process.Refresh()
        if ($process.HasExited) { throw "UI exited before sampling (PID $($process.Id), exit $($process.ExitCode))" }
        $cpuStart = $process.TotalProcessorTime.TotalMilliseconds
        $ioStart = [CalendarBenchCounters+Io]::new()
        if (-not [CalendarBenchCounters]::GetProcessIoCounters($process.Handle, [ref]$ioStart)) { throw 'Cannot read initial I/O counters' }
        Start-Sleep -Seconds $IdleSeconds
        $process.Refresh()
        if ($process.HasExited) { throw "UI exited during idle sampling (PID $($process.Id), exit $($process.ExitCode))" }
        $ioEnd = [CalendarBenchCounters+Io]::new()
        if (-not [CalendarBenchCounters]::GetProcessIoCounters($process.Handle, [ref]$ioEnd)) { throw 'Cannot read final I/O counters' }
        return [ordered]@{
            InputIdleMs = [Math]::Round($readyMs, 1)
            StartupCpuMs = [Math]::Round($cpuStart, 1)
            PrivateMiB = [Math]::Round($process.PrivateMemorySize64 / 1MB, 2)
            WorkingSetMiB = [Math]::Round($process.WorkingSet64 / 1MB, 2)
            PeakWorkingSetMiB = [Math]::Round($process.PeakWorkingSet64 / 1MB, 2)
            CpuDeltaMs = [Math]::Round($process.TotalProcessorTime.TotalMilliseconds - $cpuStart, 1)
            IdleReadBytes = $ioEnd.ReadBytes - $ioStart.ReadBytes
            IdleWriteBytes = $ioEnd.WriteBytes - $ioStart.WriteBytes
            IdleOtherOperations = $ioEnd.OtherOperations - $ioStart.OtherOperations
            Handles = $process.HandleCount
            Threads = $process.Threads.Count
            GdiObjects = [CalendarBenchCounters]::GetGuiResources($process.Handle, 0)
            UserObjects = [CalendarBenchCounters]::GetGuiResources($process.Handle, 1)
        }
    } finally {
        try {
            if (-not $process.HasExited) {
                $actual = [IO.Path]::GetFullPath($process.MainModule.FileName)
                if (-not [string]::Equals($actual, $ExePath, [StringComparison]::OrdinalIgnoreCase)) {
                    throw "Refusing to stop PID $($process.Id): executable path changed to $actual"
                }
                $process.Kill()
                if (-not $process.WaitForExit(5000)) { throw "Could not stop spawned PID $($process.Id)" }
            }
        } finally { $process.Dispose() }
    }
}

$caseResults = @()
foreach ($caseName in @('empty', 'typical100', 'stress500daily')) {
    $events = @(New-Events $caseName)
    $database = [ordered]@{ version = 1; events = $events; retired_request_ids = @() } |
        ConvertTo-Json -Depth 5 -Compress
    $bytes = [Text.Encoding]::UTF8.GetBytes($database)
    if ($bytes.Length -ge 4MB) { throw "$caseName fixture exceeds the 4 MiB database limit" }
    $samples = @()
    for ($iteration = 1; $iteration -le $Iterations; $iteration++) {
        $dataDir = Join-Path $runRoot "$caseName-$iteration"
        [void](New-Item -ItemType Directory -Path $dataDir)
        [IO.File]::WriteAllBytes((Join-Path $dataDir 'calendar.json'), $bytes)
        [IO.File]::WriteAllText((Join-Path $dataDir 'widget.json'), $prefs, $utf8)
        $samples += Measure-Run $dataDir
    }
    $median = [ordered]@{}
    foreach ($metric in @('InputIdleMs', 'StartupCpuMs', 'PrivateMiB', 'WorkingSetMiB', 'PeakWorkingSetMiB', 'CpuDeltaMs', 'IdleReadBytes', 'IdleWriteBytes', 'IdleOtherOperations', 'Handles', 'Threads', 'GdiObjects', 'UserObjects')) {
        $median[$metric] = [Math]::Round((Get-Median -Values ([double[]]@($samples | ForEach-Object { [double]$_[$metric] }))), 2)
    }
    $caseResults += [ordered]@{
        Name = $caseName; EventCount = $events.Count; DatabaseBytes = $bytes.Length
        Median = $median; Samples = $samples
    }
    Write-Output ("{0}: private {1:N2} MiB, working {2:N2} MiB, peak {3:N2} MiB, idle CPU {4:N1} ms/{5}s, input-idle {6:N1} ms" -f
        $caseName, $median.PrivateMiB, $median.WorkingSetMiB, $median.PeakWorkingSetMiB,
        $median.CpuDeltaMs, $IdleSeconds, $median.InputIdleMs)
}

$result = [ordered]@{
    Label = $Label
    Executable = $ExePath
    ExecutableSha256 = (Get-FileHash -LiteralPath $ExePath -Algorithm SHA256).Hash
    FixtureStartDate = $baseDate.ToString('yyyy-MM-dd', $dateCulture)
    Window = "desktop=$($Desktop.IsPresent) ${WindowWidth}x${WindowHeight}; dark=$($Dark.IsPresent); opacity $Opacity/255; lunar/holidays on; Google off"
    Iterations = $Iterations
    IdleSeconds = $IdleSeconds
    DataRoot = $runRoot
    Cases = $caseResults
}
$outputPath = Join-Path $qa "performance-$Label.json"
[IO.File]::WriteAllText($outputPath, ($result | ConvertTo-Json -Depth 8), $utf8)
Write-Output "Saved $outputPath"
