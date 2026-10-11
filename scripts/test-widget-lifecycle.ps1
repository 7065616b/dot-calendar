param(
    [Parameter(Mandatory)] [string] $DataDir,
    [string] $ExePath = (Join-Path $PSScriptRoot '..\dist\dot-calendar.exe'),
    [ValidateRange(1, 10)] [int] $Restarts = 3,
    [ValidateRange(3000, 30000)] [int] $TimeoutMs = 15000,
    [switch] $Desktop,
    [switch] $SimulateMissedNotification
)

# Run only against a new directory, for example:
# ./scripts/test-widget-lifecycle.ps1 -DataDir (Join-Path $PWD ('qa-data\lifecycle-' + [guid]::NewGuid().ToString('N')))
# Captures are limited to the test window bounds, but overlapping windows can
# appear on an interactive desktop. Keep the evidence local. Every message is PID checked.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$workspace = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$qaBoundary = [IO.Path]::GetFullPath((Join-Path $workspace 'qa-data')).TrimEnd('\') + '\'
$DataDir = [IO.Path]::GetFullPath($DataDir)
$ExePath = [IO.Path]::GetFullPath($ExePath)
if (-not $DataDir.StartsWith($qaBoundary, [StringComparison]::OrdinalIgnoreCase) -or
    (Test-Path -LiteralPath $DataDir)) {
    throw 'DataDir must be a new, unique directory beneath the workspace qa-data directory.'
}
if (-not $ExePath.StartsWith($workspace.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase) -or
    -not (Test-Path -LiteralPath $ExePath -PathType Leaf)) {
    throw 'ExePath must be an existing executable inside the workspace.'
}
# Do not let a junction turn an apparently isolated fixture into a personal directory.
$ancestor = [IO.DirectoryInfo]::new($DataDir).Parent
while ($null -ne $ancestor) {
    if ($ancestor.Exists -and ($ancestor.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw "DataDir has a reparse-point ancestor: $($ancestor.FullName)"
    }
    $ancestor = $ancestor.Parent
}
[void](New-Item -ItemType Directory -Path $DataDir)
$utf8 = [Text.UTF8Encoding]::new($false)
$calendarPath = Join-Path $DataDir 'calendar.json'
$reportPath = Join-Path $DataDir 'lifecycle-report.json'

if (-not ('CalendarLifecycleNative' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class CalendarLifecycleNative {
    [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)] struct BitmapInfo {
        public uint Size; public int Width, Height; public ushort Planes, Bits;
        public uint Compression, ImageSize; public int XPels, YPels; public uint Used, Important;
    }
    public sealed class Capture {
        public int Width, Height; public byte[] Pixels; public bool Succeeded;
    }
    delegate bool EnumProc(IntPtr hwnd, IntPtr data);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc callback, IntPtr data);
    [DllImport("user32.dll")] static extern bool EnumChildWindows(IntPtr parent, EnumProc callback, IntPtr data);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr hwnd, StringBuilder name, int count);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool IsWindowEnabled(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr hwnd, out Rect rect);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr hwnd);
    [DllImport("user32.dll", SetLastError=true)] public static extern bool SetWindowPos(IntPtr hwnd, IntPtr after, int x, int y, int width, int height, uint flags);
    [DllImport("user32.dll", SetLastError=true)] public static extern IntPtr SendMessageTimeout(IntPtr hwnd, uint message, IntPtr wp, IntPtr lp, uint flags, uint timeout, out IntPtr result);
    [DllImport("user32.dll")] public static extern bool InvalidateRect(IntPtr hwnd, ref Rect rect, bool erase);
    [DllImport("user32.dll")] public static extern bool UpdateWindow(IntPtr hwnd);
    [DllImport("user32.dll")] static extern IntPtr GetDC(IntPtr hwnd);
    [DllImport("user32.dll")] static extern int ReleaseDC(IntPtr hwnd, IntPtr dc);
    [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr hwnd, IntPtr dc, uint flags);
    [DllImport("gdi32.dll")] static extern IntPtr CreateCompatibleDC(IntPtr dc);
    [DllImport("gdi32.dll")] static extern IntPtr CreateCompatibleBitmap(IntPtr dc, int width, int height);
    [DllImport("gdi32.dll")] static extern IntPtr SelectObject(IntPtr dc, IntPtr obj);
    [DllImport("gdi32.dll")] static extern bool DeleteObject(IntPtr obj);
    [DllImport("gdi32.dll")] static extern bool DeleteDC(IntPtr dc);
    [DllImport("gdi32.dll")] static extern bool BitBlt(IntPtr target, int x, int y, int width, int height, IntPtr source, int sx, int sy, uint op);
    [DllImport("gdi32.dll")] static extern int GetDIBits(IntPtr dc, IntPtr bitmap, uint first, uint lines, byte[] bits, ref BitmapInfo info, uint usage);
    public static IntPtr FindWidget(uint pid) {
        IntPtr found = IntPtr.Zero;
        EnumProc match = (hwnd, data) => {
            uint owner; GetWindowThreadProcessId(hwnd, out owner);
            var name = new StringBuilder(64); GetClassName(hwnd, name, name.Capacity);
            if (owner == pid && name.ToString() == "DotCalendarWidget") { found = hwnd; return false; }
            return true;
        };
        EnumWindows((hwnd, data) => {
            if (!match(hwnd, data)) return false;
            // A desktop widget belongs to an Explorer host, rather than EnumWindows.
            EnumChildWindows(hwnd, match, IntPtr.Zero);
            return found == IntPtr.Zero;
        }, IntPtr.Zero);
        return found;
    }
    public static int CountColor(byte[] pixels, byte red, byte green, byte blue) {
        int count = 0;
        for (int i = 0; i < pixels.Length; i += 4)
            if (pixels[i] == blue && pixels[i + 1] == green && pixels[i + 2] == red) count++;
        return count;
    }
    public static Capture CaptureClient(IntPtr hwnd, bool print, bool layered) {
        Rect rect;
        if (!GetClientRect(hwnd, out rect)) throw new InvalidOperationException("Cannot read test client rectangle.");
        int width = rect.Right - rect.Left, height = rect.Bottom - rect.Top;
        if (width <= 0 || height <= 0 || (long)width * height > 16000000) throw new InvalidOperationException("Invalid test window dimensions.");
        IntPtr source = GetDC(hwnd), memory = IntPtr.Zero, bitmap = IntPtr.Zero, old = IntPtr.Zero;
        try {
            memory = CreateCompatibleDC(source); bitmap = CreateCompatibleBitmap(source, width, height);
            if (source == IntPtr.Zero || memory == IntPtr.Zero || bitmap == IntPtr.Zero) throw new InvalidOperationException("Cannot allocate test capture.");
            old = SelectObject(memory, bitmap);
            // Read the actual surface before PrintWindow can request another paint.
            uint operation = layered ? 0x40CC0020U : 0x00CC0020U; // SRCCOPY with optional CAPTUREBLT.
            bool copied = print ? PrintWindow(hwnd, memory, 3) : BitBlt(memory, 0, 0, width, height, source, 0, 0, operation);
            SelectObject(memory, old); old = IntPtr.Zero;
            var info = new BitmapInfo { Size = 40, Width = width, Height = -height, Planes = 1, Bits = 32 };
            byte[] pixels = new byte[checked(width * height * 4)];
            bool read = GetDIBits(memory, bitmap, 0, (uint)height, pixels, ref info, 0) == height;
            return new Capture { Width = width, Height = height, Pixels = pixels, Succeeded = copied && read };
        } finally {
            if (old != IntPtr.Zero) SelectObject(memory, old);
            if (bitmap != IntPtr.Zero) DeleteObject(bitmap);
            if (memory != IntPtr.Zero) DeleteDC(memory);
            if (source != IntPtr.Zero) ReleaseDC(hwnd, source);
        }
    }
}
'@
}

function Assert-True([bool] $Condition, [string] $Message) {
    if (-not $Condition) { throw $Message }
}

function New-TestStartInfo([string[]] $Arguments) {
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $ExePath
    $start.WorkingDirectory = $DataDir
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.Environment['DOT_CALENDAR_DATA_DIR'] = $DataDir
    if ($Desktop) { [void]$start.Environment.Remove('DOT_CALENDAR_WINDOWED') }
    else { $start.Environment['DOT_CALENDAR_WINDOWED'] = '1' }
    foreach ($argument in $Arguments) { [void]$start.ArgumentList.Add($argument) }
    return $start
}

function Get-TestExitMessage($Process) {
    $stderrProperty = $Process.PSObject.Properties['CalendarTestStderrTask']
    $errorText = ''
    if ($null -ne $stderrProperty) {
        try {
            if (-not $stderrProperty.Value.IsCompleted) { [void]$stderrProperty.Value.Wait(1000) }
            if ($stderrProperty.Value.IsCompleted) { $errorText = $stderrProperty.Value.GetAwaiter().GetResult().Trim() }
            else { $errorText = '(stderr capture has not completed)' }
        } catch { $errorText = "(stderr capture failed: $($_.Exception.Message))" }
    }
    return "Spawned test PID $($Process.Id) exited unexpectedly (exit $($Process.ExitCode)). stderr: $errorText"
}

function Assert-TestProcess($Process, [switch] $AllowStarting) {
    $Process.Refresh()
    if ($Process.HasExited) { throw (Get-TestExitMessage $Process) }
    Assert-True ($Process.StartInfo.Environment['DOT_CALENDAR_DATA_DIR'] -eq $DataDir) 'Test process has the wrong data directory.'
    $module = $null
    $moduleError = 'MainModule is not available yet.'
    try { $module = $Process.MainModule } catch { $moduleError = $_.Exception.Message }
    if ($null -eq $module -or [string]::IsNullOrWhiteSpace($module.FileName)) {
        $Process.Refresh()
        if ($Process.HasExited) { throw (Get-TestExitMessage $Process) }
        $Process | Add-Member -NotePropertyName CalendarModuleError -NotePropertyValue $moduleError -Force
        if ($AllowStarting) { return $false }
        throw "Refusing to operate live test PID $($Process.Id): executable path is unavailable. $moduleError"
    }
    $actual = [IO.Path]::GetFullPath($module.FileName)
    Assert-True ([string]::Equals($actual, $ExePath, [StringComparison]::OrdinalIgnoreCase)) "Refusing to operate PID $($Process.Id): executable changed."
    return $true
}

function Assert-TestWindow($Process, [IntPtr] $Window) {
    [void](Assert-TestProcess $Process)
    [uint32]$owner = 0
    [void][CalendarLifecycleNative]::GetWindowThreadProcessId($Window, [ref]$owner)
    $className = [Text.StringBuilder]::new(64)
    [void][CalendarLifecycleNative]::GetClassName($Window, $className, 64)
    Assert-True ([CalendarLifecycleNative]::IsWindow($Window) -and $owner -eq $Process.Id -and $className.ToString() -eq 'DotCalendarWidget') 'Refusing a message or capture for a window outside the spawned test PID.'
}

function Invoke-TestCommand([string[]] $Arguments) {
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = New-TestStartInfo $Arguments
    try {
        if (-not $process.Start()) { throw 'Could not start isolated command.' }
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        $process | Add-Member -NotePropertyName CalendarTestStderrTask -NotePropertyValue $stderr
        if (-not $process.WaitForExit($TimeoutMs)) {
            [void](Assert-TestProcess $process)
            $process.Kill()
            [void]$process.WaitForExit(5000)
            throw "Isolated command timed out: $($Arguments -join ' ')"
        }
        $output = $stdout.GetAwaiter().GetResult()
        $errorText = $stderr.GetAwaiter().GetResult()
        Assert-True ($process.ExitCode -eq 0) "Isolated command failed: $($Arguments -join ' '); $errorText"
        return $output
    } finally { $process.Dispose() }
}

function Wait-TestWindow($Process, [bool] $Visible = $true) {
    $clock = [Diagnostics.Stopwatch]::StartNew()
    while ($clock.ElapsedMilliseconds -lt $TimeoutMs) {
        if (-not (Assert-TestProcess $Process -AllowStarting)) {
            Start-Sleep -Milliseconds 100
            continue
        }
        $window = [CalendarLifecycleNative]::FindWidget([uint32]$Process.Id)
        if ($window -ne [IntPtr]::Zero -and [CalendarLifecycleNative]::IsWindowVisible($window) -eq $Visible) {
            Assert-TestWindow $Process $window
            return $window
        }
        Start-Sleep -Milliseconds 100
    }
    [void](Assert-TestProcess $Process)
    throw "Test widget did not reach visible=$Visible for live PID $($Process.Id)."
}

function Get-CalendarState([string] $ExpectedHash) {
    $saved = [IO.File]::ReadAllText($calendarPath) | ConvertFrom-Json -Depth 20
    Assert-True ($saved.version -eq 1 -and @($saved.events).Count -eq 89) 'Calendar must retain all 89 fictional events.'
    Assert-True (@($saved.retired_request_ids).Count -eq 0) 'Lifecycle operations unexpectedly retired a request ID.'
    $byId = @{}
    foreach ($event in $saved.events) {
        Assert-True (-not $byId.ContainsKey($event.id)) 'Calendar contains a duplicate event ID.'
        $byId[$event.id] = $event
    }
    foreach ($expected in $expectedEvents) {
        Assert-True ($byId.ContainsKey($expected.id)) "Missing fictional event ID $($expected.id)."
        foreach ($field in @('id', 'date', 'time', 'title', 'notes', 'request_id', 'completed', 'color', 'recurrence', 'reminder_minutes')) {
            $actualValue = $byId[$expected.id].PSObject.Properties[$field].Value
            $expectedValue = $expected.PSObject.Properties[$field].Value
            Assert-True ($actualValue -ceq $expectedValue) "Unexpected change to $($expected.id).$field."
        }
    }
    $hash = (Get-FileHash -LiteralPath $calendarPath -Algorithm SHA256).Hash
    if ($ExpectedHash) { Assert-True ($hash -eq $ExpectedHash) 'Read/show/close/restart changed calendar bytes.' }
    return [ordered]@{ EventCount = @($saved.events).Count; Sha256 = $hash; Bytes = (Get-Item -LiteralPath $calendarPath).Length }
}

function Get-EmptyCalendarState([string] $ExpectedHash) {
    $saved = [IO.File]::ReadAllText($calendarPath) | ConvertFrom-Json -Depth 20
    Assert-True ($saved.version -eq 1 -and @($saved.events).Count -eq 0 -and @($saved.retired_request_ids).Count -eq 0) 'Empty-cache setup must contain zero fictional events and retired IDs.'
    $hash = (Get-FileHash -LiteralPath $calendarPath -Algorithm SHA256).Hash
    Assert-True ($hash -eq $ExpectedHash) 'Starting or hiding the empty widget changed calendar bytes.'
    return [ordered]@{ EventCount = 0; Sha256 = $hash }
}

function Replace-TestCalendarWithoutSignal([byte[]] $Bytes) {
    $temp = Join-Path $DataDir ('calendar-no-signal-' + [guid]::NewGuid().ToString('N') + '.tmp')
    $backup = Join-Path $DataDir ('calendar-before-no-signal-' + [guid]::NewGuid().ToString('N') + '.json')
    [IO.File]::WriteAllBytes($temp, $Bytes)
    [IO.File]::Copy($calendarPath, $backup)
    [IO.File]::Move($temp, $calendarPath, $true)
    return $backup
}

function Save-Capture($Capture, [string] $Path) {
    $writer = [IO.BinaryWriter]::new([IO.File]::Create($Path))
    try {
        $writer.Write([byte[]]@(66, 77))
        $writer.Write([uint32](54 + $Capture.Pixels.Length))
        $writer.Write([uint32]0); $writer.Write([uint32]54); $writer.Write([uint32]40)
        $writer.Write([int32]$Capture.Width); $writer.Write([int32](-$Capture.Height))
        $writer.Write([uint16]1); $writer.Write([uint16]32)
        $writer.Write([uint32]0); $writer.Write([uint32]$Capture.Pixels.Length)
        $writer.Write([int32]0); $writer.Write([int32]0); $writer.Write([uint32]0); $writer.Write([uint32]0)
        $writer.Write([byte[]]$Capture.Pixels)
    } finally { $writer.Dispose() }
}

function Measure-Capture($Capture, [string] $Color, [string] $Path) {
    Save-Capture $Capture $Path
    $red = [Convert]::ToByte($Color.Substring(1, 2), 16)
    $green = [Convert]::ToByte($Color.Substring(3, 2), 16)
    $blue = [Convert]::ToByte($Color.Substring(5, 2), 16)
    $matching = [CalendarLifecycleNative]::CountColor($Capture.Pixels, $red, $green, $blue)
    $cellPixels = [CalendarLifecycleNative]::CountColor($Capture.Pixels, 0x2D, 0x37, 0x3E)
    return [ordered]@{ Succeeded = $Capture.Succeeded; Width = $Capture.Width; Height = $Capture.Height; EventColorPixels = $matching; CalendarCellPixels = $cellPixels; Bitmap = $Path; Sha256 = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash }
}

function Get-RenderState($Process, [IntPtr] $Window, [string] $Stage, [string] $Color, [switch] $ExpectEmpty) {
    Assert-TestWindow $Process $Window
    $parent = [CalendarLifecycleNative]::GetParent($Window)
    if ($Desktop) { Assert-True ($parent -ne [IntPtr]::Zero) 'Desktop attachment fell back to a normal window; desktop rendering was not exercised.' }
    # Raise only the test HWND: above sibling icons when attached, or topmost when windowed.
    $after = if ($parent -eq [IntPtr]::Zero) { [IntPtr](-1) } else { [IntPtr]::Zero }
    Assert-True ([CalendarLifecycleNative]::SetWindowPos($Window, $after, 0, 0, 0, 0, 0x53)) 'Cannot expose the isolated test window for client capture.'
    [void][CalendarLifecycleNative]::UpdateWindow($Window)
    $client = [CalendarLifecycleNative]::CaptureClient($Window, $false, $false)
    $clientStats = Measure-Capture $client $Color (Join-Path $DataDir "$Stage-client.bmp")
    Assert-TestWindow $Process $Window
    $layered = [CalendarLifecycleNative]::CaptureClient($Window, $false, $true)
    $layeredStats = Measure-Capture $layered $Color (Join-Path $DataDir "$Stage-layered.bmp")
    Assert-TestWindow $Process $Window
    $printed = [CalendarLifecycleNative]::CaptureClient($Window, $true, $false)
    $printStats = Measure-Capture $printed $Color (Join-Path $DataDir "$Stage-print.bmp")
    $state = [ordered]@{ Stage = $Stage; Pid = $Process.Id; ParentHwnd = $parent.ToInt64(); DesktopAttached = $parent -ne [IntPtr]::Zero; ExpectedEmpty = $ExpectEmpty.IsPresent; Client = $clientStats; CaptureBlt = $layeredStats; PrintWindow = $printStats }
    $report.Captures.Add($state)
    # PrintWindow can be unsupported by a custom layered canvas. Its black result
    # alone is not a failure; the actual visible client must contain the event block.
    if ($ExpectEmpty) {
        $hasEmptyGrid = ($clientStats.Succeeded -and $clientStats.EventColorPixels -eq 0 -and $clientStats.CalendarCellPixels -ge 10) -or ($layeredStats.Succeeded -and $layeredStats.EventColorPixels -eq 0 -and $layeredStats.CalendarCellPixels -ge 10)
        Assert-True $hasEmptyGrid "Empty-cache setup did not render an empty calendar grid at $Stage."
    } else {
        $hasEventPixels = ($clientStats.Succeeded -and $clientStats.EventColorPixels -ge 10) -or ($layeredStats.Succeeded -and $layeredStats.EventColorPixels -ge 10)
        Assert-True $hasEventPixels "Blank or stale visible calendar at $Stage. Captures retained in $DataDir."
    }
    return $state
}

function Hide-TestWindow($Process, [IntPtr] $Window) {
    Assert-TestWindow $Process $Window
    $messageResult = [IntPtr]::Zero
    Assert-True ([CalendarLifecycleNative]::SendMessageTimeout($Window, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero, 2, 3000, [ref]$messageResult) -ne [IntPtr]::Zero) 'Isolated WM_CLOSE timed out.'
    [void](Wait-TestWindow $Process $false)
}

$date = [datetime]::Today.ToString('yyyy-MM-dd', [Globalization.CultureInfo]::InvariantCulture)
$month = [datetime]::Today.AddDays(1 - [datetime]::Today.Day)
$fixture = for ($index = 0; $index -lt 89; $index++) {
    [ordered]@{
        id = 'lifecycle-{0:d3}' -f $index
        date = if ($index -eq 0) { $date } else { $month.AddDays($index % 28).ToString('yyyy-MM-dd', [Globalization.CultureInfo]::InvariantCulture) }
        time = if ($index -eq 0) { '00:01' } else { '{0:d2}:00' -f (10 + $index % 8) }
        title = if ($index -eq 0) { 'Fictional lifecycle target' } else { 'Fictional event {0:d3}' -f $index }
        notes = 'Isolated regression fixture; no personal calendar data.'
        request_id = 'lifecycle-request-{0:d3}' -f $index
        completed = $false
        color = if ($index -eq 0) { '#42E66A' } else { $null }
        recurrence = $null
        reminder_minutes = $null
    }
}
$database = [ordered]@{ version = 1; events = @($fixture); retired_request_ids = @() } | ConvertTo-Json -Depth 5
[IO.File]::WriteAllText($calendarPath, $database, $utf8)
$expectedEvents = @((ConvertFrom-Json -InputObject $database -Depth 20).events)
$prefs = [ordered]@{ position = @(80, 80, 1100, 730); opacity = 255; dark = $true; desktop = $Desktop.IsPresent; locked = $false; lunar = $false; holidays = $false; show_google = $true; auto_start = $false; sync_minutes = 5; day_colors = @{} }
[IO.File]::WriteAllText((Join-Path $DataDir 'widget.json'), ($prefs | ConvertTo-Json -Depth 5), $utf8)
$report = [ordered]@{
    Status = 'running'; Executable = $ExePath; ExecutableSha256 = (Get-FileHash -LiteralPath $ExePath -Algorithm SHA256).Hash
    DataDir = $DataDir; FixtureDate = $date; Restarts = $Restarts; Desktop = $Desktop.IsPresent; SimulateMissedNotification = $SimulateMissedNotification.IsPresent
    Captures = [Collections.Generic.List[object]]::new(); Cycles = [Collections.Generic.List[object]]::new(); MissedNotificationChecks = [Collections.Generic.List[object]]::new()
    Limitations = @('PrintWindow support is reported separately; assertions use client DC with SRCCOPY or CAPTUREBLT.', 'Requires an interactive Windows desktop with room for the test window.', 'Overlapping windows can appear in captures of the test window bounds; keep evidence local and inspect before sharing.', 'The optional missed notification case models a stale cache; it does not establish the cause of a personal app symptom.')
}
$ui = $null
$expectedHash = (Get-CalendarState '').Sha256
$fullFixtureBytes = [IO.File]::ReadAllBytes($calendarPath)
try {
    for ($cycle = 0; $cycle -le $Restarts; $cycle++) {
        if ($SimulateMissedNotification -and $cycle -eq 0) {
            $emptyDatabase = [ordered]@{ version = 1; events = @(); retired_request_ids = @() } | ConvertTo-Json -Depth 5
            [IO.File]::WriteAllText($calendarPath, $emptyDatabase, $utf8)
            $emptyHash = (Get-FileHash -LiteralPath $calendarPath -Algorithm SHA256).Hash
            $report.EmptyCacheRecovery = [ordered]@{ Status = 'running'; ExpectedInitialEventCount = 0; EmptyCalendarSha256 = $emptyHash; ExpectedRestoredCount = 89; ExpectedRestoredSha256 = $expectedHash }
        }
        $ui = [Diagnostics.Process]::new()
        $ui.StartInfo = New-TestStartInfo @()
        if (-not $ui.Start()) { throw 'Cannot start isolated GUI.' }
        $uiOutput = $ui.StandardOutput.ReadToEndAsync()
        $uiError = $ui.StandardError.ReadToEndAsync()
        $ui | Add-Member -NotePropertyName CalendarTestStderrTask -NotePropertyValue $uiError
        $window = Wait-TestWindow $ui
        $target = $expectedEvents[0]
        if ($SimulateMissedNotification -and $cycle -eq 0) {
            $emptyState = Get-EmptyCalendarState $emptyHash
            $report.EmptyCacheRecovery.AfterStartupEventCount = $emptyState.EventCount
            $report.EmptyCacheRecovery.AfterStartupSha256 = $emptyState.Sha256
            [void](Get-RenderState $ui $window 'empty-cache-startup' $target.color -ExpectEmpty)
            Hide-TestWindow $ui $window
            $emptyState = Get-EmptyCalendarState $emptyHash
            $report.EmptyCacheRecovery.BeforeRestoreEventCount = $emptyState.EventCount
            $report.EmptyCacheRecovery.BeforeRestoreSha256 = $emptyState.Sha256
            Assert-TestWindow $ui $window
            $report.EmptyCacheRecovery.EmptyCalendarBackup = Replace-TestCalendarWithoutSignal $fullFixtureBytes
            [void](Get-CalendarState $expectedHash)
            [void](Invoke-TestCommand @())
            $window = Wait-TestWindow $ui
            [void](Get-RenderState $ui $window 'empty-cache-to-89-show' $target.color)
            $restored = Get-CalendarState $expectedHash
            $report.EmptyCacheRecovery.RestoredEventCount = $restored.EventCount
            $report.EmptyCacheRecovery.RestoredSha256 = $restored.Sha256
            $report.EmptyCacheRecovery.Status = 'passed'
        }
        [void](Get-RenderState $ui $window "cycle-$cycle-startup" $target.color)
        $state = Get-CalendarState $expectedHash

        # Exercise a nonzero paint origin after startup, which catches partial
        # invalidation regressions in the bitmap viewport/clipping calculations.
        Assert-TestWindow $ui $window
        $clientRect = [CalendarLifecycleNative+Rect]::new()
        [void][CalendarLifecycleNative]::GetClientRect($window, [ref]$clientRect)
        $dpi = [Math]::Max(96, [CalendarLifecycleNative]::GetDpiForWindow($window))
        $partial = [CalendarLifecycleNative+Rect]::new()
        $partial.Left = [int](16 * $dpi / 96); $partial.Top = [int](84 * $dpi / 96)
        $partial.Right = $clientRect.Right - $partial.Left; $partial.Bottom = $clientRect.Bottom - [int](32 * $dpi / 96)
        Assert-True ([CalendarLifecycleNative]::InvalidateRect($window, [ref]$partial, $false)) 'Could not invalidate the isolated calendar grid.'
        [void][CalendarLifecycleNative]::UpdateWindow($window)
        [void](Get-RenderState $ui $window "cycle-$cycle-partial" $target.color)

        # WM_CLOSE hides the widget. A second invocation hands SHOW to the same PID.
        Hide-TestWindow $ui $window
        [void](Get-CalendarState $expectedHash)
        if ($cycle -lt $Restarts) {
            $newColor = @('#EB34B7', '#13CFE1', '#F1B62C')[$cycle % 3]
            $newTitle = "Fictional saved cycle $cycle"
            $updated = Invoke-TestCommand @('update', '--id', $target.id, '--date', $target.date, '--time', $target.time, '--title', $newTitle, '--notes', "Fictional external save $cycle", '--completed', 'true', '--color', $newColor) | ConvertFrom-Json -Depth 20
            Assert-True ($updated.id -eq $target.id -and $updated.color -eq $newColor) 'External CLI save returned the wrong fictional event.'
            $target.title = $newTitle; $target.notes = "Fictional external save $cycle"; $target.completed = $true; $target.color = $newColor
            $expectedHash = (Get-CalendarState '').Sha256
        }
        [void](Invoke-TestCommand @())
        $window = Wait-TestWindow $ui
        [void](Get-RenderState $ui $window "cycle-$cycle-show" $target.color)
        [void](Get-CalendarState $expectedHash)
        if ($SimulateMissedNotification) {
            # The normal CLI edit has already appeared in the UI, proving its signal
            # was consumed. Now replace only this fictional file without a signal.
            Hide-TestWindow $ui $window
            [void](Get-CalendarState $expectedHash)
            Assert-TestWindow $ui $window
            $oldColor = $target.color
            $target.color = @('#A35DF0', '#F07828', '#6CF03B', '#ED417B')[$cycle % 4]
            $target.title = "Fictional missed notification $cycle"
            $target.notes = "Atomic isolated fixture replacement without a store notification, cycle $cycle."
            $replacement = [ordered]@{ version = 1; events = $expectedEvents; retired_request_ids = @() } | ConvertTo-Json -Depth 5
            $beforeReplacement = Replace-TestCalendarWithoutSignal ($utf8.GetBytes($replacement))
            $check = [ordered]@{ Cycle = $cycle; BeforeSha256 = $expectedHash; BeforeReplacementBackup = $beforeReplacement; OldColor = $oldColor; NewColor = $target.color; ShowReloadPassed = $false }
            $expectedHash = (Get-CalendarState '').Sha256
            $check.AfterSha256 = $expectedHash
            $report.MissedNotificationChecks.Add($check)
            [void](Invoke-TestCommand @())
            $window = Wait-TestWindow $ui
            [void](Get-RenderState $ui $window "cycle-$cycle-missed-notification-show" $target.color)
            [void](Get-CalendarState $expectedHash)
            $check.ShowReloadPassed = $true
        }
        $listed = @(Invoke-TestCommand @('list') | ConvertFrom-Json -Depth 20 -NoEnumerate)
        Assert-True (@($listed[0]).Count -eq 89) 'Fresh CLI process did not see all 89 fictional events.'

        # The new data directory makes --shutdown's controller key unique.
        Assert-TestWindow $ui $window
        Assert-True ([CalendarLifecycleNative]::IsWindowEnabled($window)) 'Isolated GUI has an unexpected modal dialog.'
        [void](Invoke-TestCommand @('--shutdown'))
        Assert-True ($ui.WaitForExit($TimeoutMs)) 'Isolated GUI did not exit after its scoped --shutdown.'
        Assert-True ($ui.ExitCode -eq 0) "Isolated GUI exited with code $($ui.ExitCode): $($uiError.GetAwaiter().GetResult())"
        [void]$uiOutput.GetAwaiter().GetResult()
        $state = Get-CalendarState $expectedHash
        $report.Cycles.Add([ordered]@{ Cycle = $cycle; Pid = $ui.Id; EventCount = $state.EventCount; CalendarSha256 = $state.Sha256; Bytes = $state.Bytes; GracefulExit = $true })
        $ui.Dispose(); $ui = $null
        Write-Output "PASS cycle ${cycle}: 89 IDs/content preserved, startup/partial/show pixels, external save, graceful exit"
    }
    $report.Status = 'passed'
} catch {
    $report.Status = 'failed'
    $report.Error = $_.Exception.Message
    if ($report.Contains('EmptyCacheRecovery') -and $report.EmptyCacheRecovery.Status -eq 'running') { $report.EmptyCacheRecovery.Status = 'failed' }
    throw
} finally {
    try {
        if ($null -ne $ui) {
            try {
                if (-not $ui.HasExited) {
                    [void](Assert-TestProcess $ui)
                    try { [void](Invoke-TestCommand @('--shutdown')) } catch { $report.CleanupError = $_.Exception.Message }
                    if (-not $ui.WaitForExit(3000)) {
                        [void](Assert-TestProcess $ui)
                        $ui.Kill()
                        [void]$ui.WaitForExit(5000)
                        $report.ForcedTestProcessCleanup = $true
                    }
                }
            } finally { $ui.Dispose() }
        }
    } catch {
        $report.CleanupError = $_.Exception.Message
    } finally {
        [IO.File]::WriteAllText($reportPath, ($report | ConvertTo-Json -Depth 12), $utf8)
        Write-Output "Lifecycle evidence retained at $reportPath"
    }
}
