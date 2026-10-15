param(
    [Parameter(Mandatory)][string]$ExePath,
    [Parameter(Mandatory)][string]$Output,
    [string]$UnitTestExe,
    [switch]$Smoke,
    [switch]$Release,
    [string]$SeedCalendar,
    [string]$SeedDeletedDirectory
)
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$workspace=[IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$boundary=$workspace.TrimEnd('\')+'\qa-data\'
$Output=[IO.Path]::GetFullPath($Output);$ExePath=[IO.Path]::GetFullPath($ExePath)
if(-not $Output.StartsWith($boundary,[StringComparison]::OrdinalIgnoreCase) -or (Test-Path -LiteralPath $Output)){throw 'A new QA output directory is required.'}
if(-not $ExePath.StartsWith($boundary,[StringComparison]::OrdinalIgnoreCase) -or -not(Test-Path -LiteralPath $ExePath)){throw 'A copied QA executable is required.'}
foreach($target in @($Output,$ExePath)){
    $current=$target
    while($current){if(Test-Path -LiteralPath $current){if((Get-Item -LiteralPath $current -Force).Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Reparse path refused.'}};$current=[IO.Path]::GetDirectoryName($current)}
}
$principal=[Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)){throw 'Administrator context refused.'}
$self=Get-CimInstance Win32_Process -Filter ('ProcessId='+$PID)
$parent=Get-CimInstance Win32_Process -Filter ('ProcessId='+$self.ParentProcessId)
if($parent.ExecutablePath -ine 'C:\Windows\explorer.exe'){throw 'Run this isolated UI test from the existing Explorer desktop.'}
[void][IO.Directory]::CreateDirectory($Output)
$report=[ordered]@{status='running';startedUtc=[datetime]::UtcNow.ToString('o');forcedTermination=$false;administrator=$false;helperParent=$parent.ProcessId;exeSha256=(Get-FileHash -LiteralPath $ExePath).Hash;checks=@();keyboardModes=@()}
$reportPath=Join-Path $Output 'result.json'
function Save-Report{$report|ConvertTo-Json -Depth 15|Set-Content -LiteralPath $reportPath -Encoding utf8}
function Check([bool]$pass,[string]$name){$report.checks+=@{name=$name;pass=$pass};Save-Report;if(-not $pass){throw $name}}
Save-Report
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type -Path 'C:\Windows\Microsoft.NET\Framework64\v4.0.30319\WPF\UIAutomationTypes.dll'
Add-Type -Path 'C:\Windows\Microsoft.NET\Framework64\v4.0.30319\WPF\UIAutomationClient.dll'
Add-Type -TypeDefinition @'
using System;using System.Collections.Generic;using System.Text;using System.Runtime.InteropServices;
public static class NativeUiQA {
 public delegate bool EnumProc(IntPtr h,IntPtr p);
 [StructLayout(LayoutKind.Sequential)] public struct Rect{public int Left,Top,Right,Bottom;}
 [StructLayout(LayoutKind.Sequential)] public struct Point{public int X,Y;}
 [StructLayout(LayoutKind.Sequential)] public struct Io{public ulong ReadOperations,WriteOperations,OtherOperations,ReadBytes,WriteBytes,OtherBytes;}
 [DllImport("kernel32.dll")] public static extern bool GetProcessIoCounters(IntPtr process,out Io counters);
 [DllImport("user32.dll")] public static extern bool GetCursorPos(out Point p);
 [DllImport("user32.dll")] public static extern bool SetCursorPos(int x,int y);
 [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(Point p);
 [DllImport("user32.dll")] public static extern void mouse_event(uint flags,uint x,uint y,uint data,UIntPtr extra);
 [StructLayout(LayoutKind.Sequential)] public struct GuiInfo{public int size,flags;public IntPtr active,focus,capture,menu,move,caret;public Rect rect;}
 [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f,IntPtr p);
 [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h,EnumProc f,IntPtr p);
 [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h,out uint pid);
 [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
 [DllImport("user32.dll")] public static extern bool IsWindowEnabled(IntPtr h);
 [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
 [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h,out Rect r);
 [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h,out Rect r);
 [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h,ref Point p);
 [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h,IntPtr after,int x,int y,int w,int hgt,uint flags);
 [DllImport("dwmapi.dll")] public static extern int DwmFlush();
 [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
 [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
 [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
 [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
 [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
 [DllImport("user32.dll")] public static extern uint GetGuiResources(IntPtr h,uint type);
 [DllImport("user32.dll")] public static extern uint GetSysColor(int index);
 [DllImport("gdi32.dll")] public static extern uint GetPixel(IntPtr dc,int x,int y);
 [DllImport("user32.dll")] public static extern bool GetGUIThreadInfo(uint thread,ref GuiInfo info);
 [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a,uint b,bool attach);
 [DllImport("user32.dll")] static extern bool GetKeyboardState(byte[] state);
 [DllImport("user32.dll")] static extern bool SetKeyboardState(byte[] state);
 [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
 [DllImport("user32.dll")] public static extern IntPtr SetFocus(IntPtr h);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr h,StringBuilder b,int n);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr h,StringBuilder b,int n);
 [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern IntPtr SendMessageTimeout(IntPtr h,uint m,IntPtr w,IntPtr l,uint flags,uint timeout,out IntPtr result);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern IntPtr SendMessageTimeout(IntPtr h,uint m,IntPtr w,string l,uint flags,uint timeout,out IntPtr result);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern IntPtr SendMessageTimeout(IntPtr h,uint m,IntPtr w,StringBuilder l,uint flags,uint timeout,out IntPtr result);
 [DllImport("user32.dll")] public static extern IntPtr GetDC(IntPtr h);
 [DllImport("user32.dll")] public static extern int ReleaseDC(IntPtr h,IntPtr dc);
 [DllImport("gdi32.dll")] public static extern bool BitBlt(IntPtr dst,int x,int y,int w,int h,IntPtr src,int sx,int sy,uint op);
 [DllImport("gdi32.dll")] static extern IntPtr SelectObject(IntPtr dc,IntPtr obj);
 [DllImport("gdi32.dll",CharSet=CharSet.Unicode)] static extern bool GetTextExtentPoint32(IntPtr dc,string text,int count,out Point size);
 public static int[] DigitWidths(IntPtr h){var dc=GetDC(h);var font=new IntPtr(Send(h,0x31));var old=SelectObject(dc,font);try{var widths=new int[10];for(int i=0;i<10;i++){Point size;if(!GetTextExtentPoint32(dc,i.ToString(),1,out size))throw new Exception("Text measurement failed");widths[i]=size.X;}return widths;}finally{SelectObject(dc,old);ReleaseDC(h,dc);}}
 public static uint Owner(IntPtr h){uint p;GetWindowThreadProcessId(h,out p);return p;}
 public static string Class(IntPtr h){var b=new StringBuilder(256);GetClassName(h,b,b.Capacity);return b.ToString();}
 public static string Text(IntPtr h){var b=new StringBuilder(16384);IntPtr r;if(SendMessageTimeout(h,0xD,new IntPtr(b.Capacity),b,2,2000,out r)==IntPtr.Zero)throw new Exception("Text read timed out");return b.ToString();}
 public static long Send(IntPtr h,uint m,long w=0,long l=0){IntPtr result;if(SendMessageTimeout(h,m,new IntPtr(w),new IntPtr(l),2,4000,out result)==IntPtr.Zero)throw new Exception("Message timed out: "+m);return result.ToInt64();}
 public static void SetText(IntPtr h,string s){IntPtr r;if(SendMessageTimeout(h,0xC,IntPtr.Zero,s,2,3000,out r)==IntPtr.Zero)throw new Exception("Text update timed out");}
 public static IntPtr[] Windows(uint pid){var list=new List<IntPtr>();EnumProc visit=(h,p)=>{if(Owner(h)==pid&&!list.Contains(h))list.Add(h);return true;};EnumWindows((h,p)=>{visit(h,p);EnumChildWindows(h,visit,IntPtr.Zero);return true;},IntPtr.Zero);return list.ToArray();}
 public static IntPtr Focused(IntPtr h){uint p;uint t=GetWindowThreadProcessId(h,out p);var info=new GuiInfo{size=Marshal.SizeOf(typeof(GuiInfo))};if(!GetGUIThreadInfo(t,ref info))throw new Exception("Cannot read GUI focus");return info.focus;}
 public static void Focus(IntPtr h){uint p;uint t=GetWindowThreadProcessId(h,out p), current=GetCurrentThreadId();bool attach=t!=current;try{if(attach&&!AttachThreadInput(current,t,true))throw new Exception("Cannot focus test control");SetFocus(h);}finally{if(attach)AttachThreadInput(current,t,false);}}
 public static void Activate(IntPtr h){uint p;uint foreground=GetWindowThreadProcessId(GetForegroundWindow(),out p),current=GetCurrentThreadId();bool attach=foreground!=0&&foreground!=current;try{if(attach)AttachThreadInput(current,foreground,true);SetForegroundWindow(h);}finally{if(attach)AttachThreadInput(current,foreground,false);}}
 public static void ControlK(IntPtr h,bool repeat){uint pid;uint target=GetWindowThreadProcessId(h,out pid),current=GetCurrentThreadId();if(!AttachThreadInput(current,target,true))throw new Exception("Cannot attach candidate input queue");var saved=new byte[256];try{if(!GetKeyboardState(saved))throw new Exception("Cannot read candidate keyboard state");var state=(byte[])saved.Clone();state[0x11]=0x80;state[0xA2]=0x80;if(!SetKeyboardState(state))throw new Exception("Cannot set candidate keyboard state");PostMessage(h,0x100,new IntPtr(0x4B),new IntPtr(repeat?0x40000001:1));System.Threading.Thread.Sleep(160);PostMessage(h,0x101,new IntPtr(0x4B),new IntPtr(unchecked((int)0xC0000001)));}finally{SetKeyboardState(saved);AttachThreadInput(current,target,false);}}
}
// Use the OS COM client. The managed Framework Win32 proxy factory on this
// host throws inside LoadDefaultProxies and reports every HWND as a Pane.
public static class NativeUiaQA {
 [DllImport("ole32.dll")] static extern int CoCreateInstance(ref Guid cls,IntPtr outer,uint context,ref Guid iid,out IntPtr obj);
 [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int FromHandle(IntPtr self,IntPtr hwnd,out IntPtr element);
 [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int GetInt(IntPtr self,out int value);
 [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int GetString(IntPtr self,out IntPtr value);
 [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int GetPattern(IntPtr self,int id,out IntPtr pattern);
 [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int Action(IntPtr self);
 static T Method<T>(IntPtr obj,int slot) where T:Delegate {return Marshal.GetDelegateForFunctionPointer<T>(Marshal.ReadIntPtr(Marshal.ReadIntPtr(obj),slot*IntPtr.Size));}
 static int Number(IntPtr obj,int slot){int value;Marshal.ThrowExceptionForHR(Method<GetInt>(obj,slot)(obj,out value));return value;}
 static string Text(IntPtr obj,int slot){IntPtr value;Marshal.ThrowExceptionForHR(Method<GetString>(obj,slot)(obj,out value));try{return Marshal.PtrToStringBSTR(value);}finally{Marshal.FreeBSTR(value);}}
 public sealed class Info{public string name,role,provider;public bool invokable;public bool? selected;}
 public static Info Read(IntPtr hwnd,bool invoke,bool focus){
  var cls=new Guid("ff48dba4-60ef-4201-aa87-54103eef594e");var iid=new Guid("30cbe57d-d9d0-452a-ab13-7ac5ac4825ee");IntPtr client=IntPtr.Zero,element=IntPtr.Zero,pattern=IntPtr.Zero;
  try{
   Marshal.ThrowExceptionForHR(CoCreateInstance(ref cls,IntPtr.Zero,1,ref iid,out client));
   Marshal.ThrowExceptionForHR(Method<FromHandle>(client,6)(client,hwnd,out element));
   int type=Number(element,21);string role=type==50000?"Button":type==50004?"Edit":type==50008?"List":type==50013?"RadioButton":type==50033?"Pane":type.ToString();
   var info=new Info{name=Text(element,23),role="ControlType."+role,provider=Text(element,51)};
   int hr=Method<GetPattern>(element,16)(element,10000,out pattern);info.invokable=hr>=0&&pattern!=IntPtr.Zero;
   if(invoke){if(!info.invokable)throw new Exception("UIA Invoke unavailable");Marshal.ThrowExceptionForHR(Method<Action>(pattern,3)(pattern));}
   if(pattern!=IntPtr.Zero){Marshal.Release(pattern);pattern=IntPtr.Zero;}
   hr=Method<GetPattern>(element,16)(element,10010,out pattern);if(hr>=0&&pattern!=IntPtr.Zero)info.selected=Number(pattern,6)!=0;
   if(focus)Marshal.ThrowExceptionForHR(Method<Action>(element,3)(element));
   return info;
  }finally{if(pattern!=IntPtr.Zero)Marshal.Release(pattern);if(element!=IntPtr.Zero)Marshal.Release(element);if(client!=IntPtr.Zero)Marshal.Release(client);}
 }
}
'@
[void][NativeUiQA]::SetThreadDpiAwarenessContext([IntPtr](-4))
$data=Join-Path $Output 'data';[void][IO.Directory]::CreateDirectory($data)
$utf8=[Text.UTF8Encoding]::new($false)
$today=[datetime]::Now.ToString('yyyy-MM-dd')
$events=@(
    @{id='qa-a';date=$today;time='09:00';title='디자인 리뷰';notes="첫 번째 메모`n둘째 줄 · 유니코드 ✓";request_id='qa-request-a';completed=$false;recurrence=$null;color=$null;reminder_minutes=$null},
    @{id='qa-b';date=$today;time='14:00';title='완료된 일정';notes='보존할 완료 메모';request_id='qa-request-b';completed=$true;recurrence=$null;color=$null;reminder_minutes=$null},
    @{id='qa-c';date=$today;time=$null;title='주간 확인';notes='반복 일정';request_id=$null;completed=$false;recurrence='weekly';color=$null;reminder_minutes=$null}
)
$calendar=Join-Path $data 'calendar.json'
[IO.File]::WriteAllText($calendar,(@{version=1;events=$events;retired_request_ids=@('qa-retired')}|ConvertTo-Json -Depth 10),$utf8)
if($SeedCalendar){
    $seed=[IO.Path]::GetFullPath($SeedCalendar)
    if(-not $seed.StartsWith($boundary,[StringComparison]::OrdinalIgnoreCase)){throw 'Seed must be a preserved QA copy.'}
    [IO.File]::WriteAllBytes($calendar,[IO.File]::ReadAllBytes($seed))
    if($SeedDeletedDirectory){
        $deleted=[IO.Path]::GetFullPath($SeedDeletedDirectory)
        if(-not $deleted.StartsWith($boundary,[StringComparison]::OrdinalIgnoreCase)){throw 'Deleted-event seed must be a QA copy.'}
        [void][IO.Directory]::CreateDirectory((Join-Path $data 'deleted'))
        foreach($file in Get-ChildItem -LiteralPath $deleted -File){if($file.Attributes-band [IO.FileAttributes]::ReparsePoint){throw 'Reparse seed refused.'};[IO.File]::Copy($file.FullName,(Join-Path $data ('deleted\'+$file.Name)),$false)}
    }
}
$fixtureHash=(Get-FileHash -LiteralPath $calendar).Hash
[IO.File]::WriteAllText((Join-Path $data 'widget.json'),(@{position=@(100,100,1060,700);opacity=255;dark=$true;desktop=$false;locked=$false;lunar=$true;holidays=$true;show_google=$true;auto_start=$false;sync_minutes=5;day_colors=@{}}|ConvertTo-Json -Depth 5),$utf8)
$process=$null;$stderr=$null;$widget=[IntPtr]::Zero
function Start-App {
    $start=[Diagnostics.ProcessStartInfo]::new($ExePath);$start.WorkingDirectory=$data;$start.UseShellExecute=$false;$start.CreateNoWindow=$true;$start.WindowStyle=[Diagnostics.ProcessWindowStyle]::Hidden;$start.RedirectStandardError=$true
    $start.EnvironmentVariables['DOT_CALENDAR_DATA_DIR']=$data;$start.EnvironmentVariables['DOT_CALENDAR_WINDOWED']='1'
    $script:process=[Diagnostics.Process]::Start($start);$script:stderr=$script:process.StandardError.ReadToEndAsync()
    $watch=[Diagnostics.Stopwatch]::StartNew()
    do {
        if($script:process.HasExited){throw ('Application exited: '+$script:stderr.GetAwaiter().GetResult())}
        $found=@([NativeUiQA]::Windows($script:process.Id)|Where-Object{[NativeUiQA]::Class($_)-eq 'DotCalendarWidget' -and [NativeUiQA]::IsWindowVisible($_)})
        if($found.Count -eq 1){$script:widget=$found[0];break}
        Start-Sleep -Milliseconds 100
    }while($watch.ElapsedMilliseconds -lt 15000)
    Check ($found.Count -eq 1) 'One visible candidate widget'
    Check ($script:process.MainModule.FileName -ieq $ExePath) 'Candidate process identity'
    Check ([NativeUiQA]::Text($script:widget)-eq 'AI 캘린더') 'Native window exposes the approved AI calendar name'
    Check ((Get-Item -LiteralPath $ExePath).VersionInfo.ProductName-eq 'AI 캘린더') 'Executable product metadata exposes the approved name'
    [NativeUiQA]::Activate($script:widget);Start-Sleep -Milliseconds 350
}
function Windows { return @([NativeUiQA]::Windows($process.Id)) }
function Toasts { return @(Windows|Where-Object{[NativeUiQA]::Class($_)-eq 'DotCalendarSurface' -and [NativeUiQA]::Text($_)}) }
function Palette {
    $watch=[Diagnostics.Stopwatch]::StartNew()
    do{$found=@(Windows|Where-Object{[NativeUiQA]::Class($_)-eq 'DotCalendarPalette' -and [NativeUiQA]::GetParent($_)-eq $widget});if($found.Count-eq 1){Start-Sleep -Milliseconds 400;return $found[0]};Start-Sleep -Milliseconds 50}while($watch.ElapsedMilliseconds-lt 10000)
    throw 'Palette did not become ready.'
}
function Rect-Of([IntPtr]$h){$r=[NativeUiQA+Rect]::new();[void][NativeUiQA]::GetWindowRect($h,[ref]$r);return @($r.Left,$r.Top,$r.Right,$r.Bottom)-join ','}
function Metric([int]$id){return [NativeUiQA]::Send($widget,0x8018,$id,0)}
function Control([int]$id){$found=@(Windows|Where-Object{[NativeUiQA]::GetDlgCtrlID($_)-eq $id -and [NativeUiQA]::IsWindowVisible($_)});if($found.Count-ne 1){throw "Expected one visible control $id; found $($found.Count)"};return $found[0]}
function Command([int]$id){Check ([NativeUiQA]::Owner($widget)-eq $process.Id) 'Command PID guard';[void][NativeUiQA]::PostMessage($widget,0x111,[IntPtr]$id,[IntPtr]::Zero);Start-Sleep -Milliseconds 360}
function Keys([string]$keys){
    $foreground=[NativeUiQA]::GetForegroundWindow()
    if([NativeUiQA]::Owner($foreground)-ne $process.Id){
        $modal=@(Windows|Where-Object{[NativeUiQA]::Class($_)-eq 'DotCalendarPalette' -and [NativeUiQA]::GetParent($_)-eq $widget})
        [NativeUiQA]::Activate($(if($modal.Count){$modal[0]}else{$widget}));Start-Sleep -Milliseconds 120
        $foreground=[NativeUiQA]::GetForegroundWindow()
    }
    if([NativeUiQA]::Owner($foreground)-eq $process.Id){
        $report.keyboardModes+=@{keys=$keys;mode='physical SendKeys; foreground PID checked'}
        [System.Windows.Forms.SendKeys]::SendWait($keys)
    }else{
        # Do not inject global keys into another app. Queued native key messages
        # still test the actual app loop, but cannot establish physical Ctrl+K.
        $report.keyboardModes+=@{keys=$keys;mode='targeted native message; physical input unavailable';foregroundPid=[NativeUiQA]::Owner($foreground)}
        $modal=@(Windows|Where-Object{[NativeUiQA]::Class($_)-eq 'DotCalendarPalette' -and [NativeUiQA]::GetParent($_)-eq $widget})
        if($keys-eq '^k'){
            $target=[NativeUiQA]::Focused($widget);if($target-eq [IntPtr]::Zero){$target=$widget}
            Check ([NativeUiQA]::Owner($target)-eq $process.Id) 'Shortcut input queue PID guard'
            [NativeUiQA]::ControlK($target,$false)
        }else{
            $key=@{'{ENTER}'=13;'{ESC}'=27;'{UP}'=38;'{DOWN}'=40;'{RIGHT}'=39;'{LEFT}'=37;'{TAB}'=9}[$keys]
            if($null-eq $key){throw ('No targeted fallback for '+$keys)}
            $target=[NativeUiQA]::Focused($widget);if($target-eq [IntPtr]::Zero){$target=$(if($modal.Count){$modal[0]}else{$widget})}
            Check ([NativeUiQA]::Owner($target)-eq $process.Id) 'Targeted keyboard PID guard'
            [void][NativeUiQA]::PostMessage($target,0x100,[IntPtr]$key,[IntPtr]::Zero)
            [void][NativeUiQA]::PostMessage($target,0x101,[IntPtr]$key,[IntPtr]::Zero)
        }
    }
    Start-Sleep -Milliseconds 300
}
function Capture([string]$name,[IntPtr]$handle=$widget){
    Check ([NativeUiQA]::Owner($handle)-eq $process.Id) 'Capture PID guard'
    $r=[NativeUiQA+Rect]::new();[void][NativeUiQA]::GetClientRect($handle,[ref]$r)
    $bitmap=[Drawing.Bitmap]::new($r.Right,$r.Bottom);$g=[Drawing.Graphics]::FromImage($bitmap);$source=[NativeUiQA]::GetDC($handle);$dc=$g.GetHdc()
    try{Check ([NativeUiQA]::BitBlt($dc,0,0,$r.Right,$r.Bottom,$source,0,0,0x40CC0020)) 'Native client capture';$g.ReleaseHdc($dc);$dc=[IntPtr]::Zero;$bitmap.Save((Join-Path $Output ($name+'.png')),[Drawing.Imaging.ImageFormat]::Png)}finally{if($dc-ne [IntPtr]::Zero){$g.ReleaseHdc($dc)};[void][NativeUiQA]::ReleaseDC($handle,$source);$g.Dispose();$bitmap.Dispose()}
}
function Capture-Screen([string]$name,[IntPtr]$handle=$widget){
    Check ([NativeUiQA]::Owner($handle)-eq $process.Id) 'Composited screen capture PID guard'
    $r=[NativeUiQA+Rect]::new();$origin=[NativeUiQA+Point]::new()
    [void][NativeUiQA]::GetClientRect($handle,[ref]$r);[void][NativeUiQA]::ClientToScreen($handle,[ref]$origin)
    $screen=[Windows.Forms.SystemInformation]::VirtualScreen
    Check ($origin.X-ge $screen.Left -and $origin.Y-ge $screen.Top -and ($origin.X+$r.Right)-le $screen.Right -and ($origin.Y+$r.Bottom)-le $screen.Bottom) 'Capture is confined to the visible candidate client'
    [void][NativeUiQA]::SetWindowPos($handle,[IntPtr](-1),0,0,0,0,0x13)
    try{
        [void][NativeUiQA]::DwmFlush();Start-Sleep -Milliseconds 60
        $bitmap=[Drawing.Bitmap]::new($r.Right,$r.Bottom);$graphics=[Drawing.Graphics]::FromImage($bitmap)
        try{$graphics.CopyFromScreen($origin.X,$origin.Y,0,0,$bitmap.Size);$bitmap.Save((Join-Path $Output ($name+'.png')),[Drawing.Imaging.ImageFormat]::Png)}finally{$graphics.Dispose();$bitmap.Dispose()}
    }finally{[void][NativeUiQA]::SetWindowPos($handle,[IntPtr](-2),0,0,0,0,0x13)}
}
function Accessible([IntPtr]$handle,[switch]$Invoke,[switch]$Focus){
    Check ([NativeUiQA]::Owner($handle)-eq $process.Id) 'Accessibility PID guard'
    return [NativeUiaQA]::Read($handle,$Invoke.IsPresent,$Focus.IsPresent)
}
function Stop-App {
    if($null -eq $process -or $process.HasExited){return}
    foreach($h in Windows|Where-Object{[NativeUiQA]::Class($_)-eq 'DotCalendarPalette' -and [NativeUiQA]::GetParent($_)-eq $widget}){[void][NativeUiQA]::PostMessage($h,0x10,[IntPtr]::Zero,[IntPtr]::Zero)}
    [void][NativeUiQA]::PostMessage($widget,0x111,[IntPtr]30,[IntPtr]::Zero)
    Check ($process.WaitForExit(8000)) 'Normal application exit'
    Check ($process.ExitCode -eq 0) 'Clean application exit code'
}
try {
    if($UnitTestExe){
        $si=[Diagnostics.ProcessStartInfo]::new($UnitTestExe);$si.UseShellExecute=$false;$si.CreateNoWindow=$true;$si.WindowStyle=[Diagnostics.ProcessWindowStyle]::Hidden;$si.RedirectStandardOutput=$true;$si.RedirectStandardError=$true
        $unit=[Diagnostics.Process]::Start($si);$uo=$unit.StandardOutput.ReadToEndAsync();$ue=$unit.StandardError.ReadToEndAsync();Check ($unit.WaitForExit(60000)) 'Unit process completed';[IO.File]::WriteAllText((Join-Path $Output 'unit-tests.log'),$uo.GetAwaiter().GetResult()+$ue.GetAwaiter().GetResult(),$utf8);Check ($unit.ExitCode -eq 0) 'Rust regression suite';$unit.Dispose()
    }
    Start-App
    $report.dpi=[NativeUiQA]::GetDpiForWindow($widget)
    $report.candidatePid=$process.Id
    if(-not $SeedCalendar){try{[void](Accessible $widget -Focus);$report.foregroundAfterUia=[NativeUiQA]::Owner([NativeUiQA]::GetForegroundWindow())}catch{$report.activationLimitation=$_.Exception.Message}}
    if($SeedCalendar){
        $beforeSeed=Get-Content -LiteralPath $calendar -Raw -Encoding UTF8|ConvertFrom-Json
        $deletedHashes=@(Get-ChildItem -LiteralPath (Join-Path $data 'deleted') -File|ForEach-Object{@{path=$_.FullName;hash=(Get-FileHash -LiteralPath $_.FullName).Hash}})
        for($cycle=0;$cycle-lt 3;$cycle++){
            Command 54;Command 53;Command 52
            Check ((Get-FileHash -LiteralPath $calendar).Hash-eq $fixtureHash) 'Latest full calendar bytes retained while browsing'
            Stop-App
            if($cycle-lt 2){Start-App}
        }
        foreach($entry in $deletedHashes){Check ((Get-FileHash -LiteralPath $entry.path).Hash-eq $entry.hash) 'Deleted archive retained after restarts'}
        $report.preservedEvents=@($beforeSeed.events).Count;$report.preservedRetired=@($beforeSeed.retired_request_ids).Count;$report.preservedDeleted=@($deletedHashes|Where-Object path -Like '*.json').Count;$report.preservedArchiveFiles=$deletedHashes.Count
        $report.fixtureHash=$fixtureHash;$report.status='PASS';return
    }
    Capture 'calendar'
    Command 55
    Capture 'native-showcase'
    Capture-Screen 'screen-native-showcase'
    $accessible=Accessible (Control 56)
    $report.primaryAccessible=$accessible
    Check ($accessible.name-eq 'Primary' -and $accessible.role-eq 'ControlType.Button') 'UIA primary button exposes its name and role'
    $report.digitWidths=[NativeUiQA]::DigitWidths((Control 56))
    Check (@($report.digitWidths|Select-Object -Unique).Count-eq 1) 'Native UI font uses equal digit advances'
    Check $accessible.invokable 'UIA button supports Invoke'
    [void](Accessible (Control 56) -Invoke);Start-Sleep -Milliseconds 350
    Check (@(Toasts).Count-eq 1) 'UIA Invoke runs the native button action'
    Keys '{ESC}'
    Command 56;Command 57;Command 59
    Capture 'stacked-toasts'
    $report.composition=@{submitted=[NativeUiQA]::Send($widget,0x8018,1,0);completed=[NativeUiQA]::Send($widget,0x8018,2,0);failures=[NativeUiQA]::Send($widget,0x8018,3,0);layers=[NativeUiQA]::Send($widget,0x8018,0,0);timer=[NativeUiQA]::Send($widget,0x8018,4,0)}
    if(-not $Release){
        Check ($report.composition.submitted-ge 4) 'Actual composition animations submitted'
        Check ($report.composition.failures-eq 0) 'No compositor errors'
        Check ($report.composition.layers-eq 0 -and $report.composition.timer-eq 0) 'Animation layers and timer released'
    }
    $trigger=Control 51;[NativeUiQA]::Focus($trigger)
    Keys '^k'
    $wait=[Diagnostics.Stopwatch]::StartNew()
    do{$palettes=@(Windows|Where-Object{[NativeUiQA]::Class($_)-eq 'DotCalendarPalette'});if($palettes.Count-eq 3){break};Start-Sleep -Milliseconds 100}while($wait.ElapsedMilliseconds-lt 10000 -and -not $process.HasExited)
    Check ($palettes.Count-eq 3) 'Palette root, backdrop and panel'
    $root=@($palettes|Where-Object{[NativeUiQA]::GetParent($_)-eq $widget})[0]
    Start-Sleep -Milliseconds 650
    Capture 'command-palette' $root
    Capture-Screen 'screen-command-palette' $root
    $search=Control 101;$list=Control 102
    [NativeUiQA]::ControlK($search,$true);Start-Sleep -Milliseconds 150
    Check ([NativeUiQA]::IsWindow($root)) 'Held Ctrl+K repeat does not toggle the palette'
    Keys '^k';Check (-not [NativeUiQA]::IsWindow($root)) 'Ctrl+K key path toggles the open palette closed'
    Keys '^k';$root=Palette;$search=Control 101;$list=Control 102
    $accessible=Accessible $search
    $report.searchAccessibleName=$accessible.name
    Check (-not [string]::IsNullOrWhiteSpace($accessible.name) -and $accessible.role-eq 'ControlType.Edit') 'UIA search exposes an accessible name and edit role'
    Check ((Accessible $list).role-eq 'ControlType.List') 'UIA command results expose list semantics'
    [NativeUiQA]::SetText($search,'not-a-command-검색');Start-Sleep -Milliseconds 150
    Check ([NativeUiQA]::Send($list,0x18B)-eq 0) 'Empty search state'
    Keys '{ENTER}';Check ([NativeUiQA]::IsWindow($root)) 'Empty Enter keeps palette open'
    [NativeUiQA]::SetText($search,'');Start-Sleep -Milliseconds 150
    $count=[NativeUiQA]::Send($list,0x18B)
    Keys '{UP}';Check ([NativeUiQA]::Send($list,0x188)-eq $count-1) 'Up wraps to last command'
    Keys '{DOWN}';Check ([NativeUiQA]::Send($list,0x188)-eq 0) 'Down wraps to first command'
    Keys '{ESC}';Check (-not [NativeUiQA]::IsWindow($root)) 'Escape closes palette';Check ([NativeUiQA]::Focused($widget)-eq $trigger) 'Exact trigger focus restored'
    Command 53;Check ([NativeUiQA]::Send((Control 60),0x18B)-eq 3) 'Day view retains three fixture events'
    Command 54;Check ([NativeUiQA]::Send((Control 60),0x18B)-eq 1) 'Completed view filters without mutation';Capture 'completed-view'
    Check ((Get-FileHash -LiteralPath $calendar).Hash-eq $fixtureHash) 'UI exploration leaves original fixture bytes unchanged'
    if(-not $Smoke){
        # The native radio state is what assistive technologies consume.
        Check ([NativeUiQA]::Send((Control 54),0xF0)-eq 1) 'Completed segment exposes checked state'
        $accessible=Accessible (Control 54)
        Check ($accessible.role-eq 'ControlType.RadioButton') 'UIA segment exposes radio semantics'
        Check $accessible.selected 'UIA segment exposes selected state'
        Check ([NativeUiQA]::Send((Control 53),0xF0)-eq 0) 'Other segment exposes unchecked state'
        [NativeUiQA]::Focus((Control 54));Keys '{LEFT}'
        Check ([NativeUiQA]::Send((Control 53),0xF0)-eq 1) 'Segment arrow navigation updates checked state'
        Command 52;Command 13
        $editor=Control 70;$draft="보존 테스트 메모`r`n한글과 줄바꿈을 보존합니다."
        [NativeUiQA]::SetText($editor,$draft);[NativeUiQA]::Focus($editor)
        Command 51;$modal=Palette;Keys '{ESC}'
        Check ([NativeUiQA]::IsWindow($editor)) 'Cancel palette retains dirty editor'
        Check ([NativeUiQA]::Text($editor)-ceq $draft) 'Cancel palette retains exact draft text'
        Check ([NativeUiQA]::Focused($widget)-eq $editor) 'Cancel palette restores editor focus'
        Check ((Get-FileHash -LiteralPath $calendar).Hash-eq $fixtureHash) 'Palette does not commit dirty draft'
        Command 71
        $saved=Get-Content -LiteralPath $calendar -Raw -Encoding UTF8|ConvertFrom-Json
        Check (@($saved.events).Count-eq 4) 'Explicit save creates one local note'
        Check (@($saved.events|Where-Object title -CEQ '보존 테스트 메모').Count-eq 1) 'Saved note verified'
        foreach($original in $events){$found=@($saved.events|Where-Object id -CEQ $original.id);Check ($found.Count-eq 1 -and $found[0].notes-ceq $original.notes -and $found[0].completed-eq $original.completed) 'Existing note and completion preserved'}
        $fixtureHash=(Get-FileHash -LiteralPath $calendar).Hash
        Command 55
        $button=Control 56;[NativeUiQA]::Focus($button);$bounds=Rect-Of $button
        [void][NativeUiQA]::PostMessage($button,0x100,[IntPtr]32,[IntPtr]::Zero);Start-Sleep -Milliseconds 210
        Check ((Rect-Of $button)-eq $bounds) 'Press animation does not move button hit geometry'
        if(-not $Release){Check ((Metric 0)-ge 1 -and (Metric 4)-eq 0) 'Held button has settled composition with no frame timer'}
        Capture-Screen 'screen-button-pressed'
        [void][NativeUiQA]::PostMessage($button,0x101,[IntPtr]32,[IntPtr]::Zero);Start-Sleep -Milliseconds 350
        if(-not $Release){Check ((Metric 0)-eq 0 -and (Metric 4)-eq 0) 'Release removes press layer and completion timer'}
        Capture-Screen 'screen-button-released'
        $originalCursor=[NativeUiQA+Point]::new();[void][NativeUiQA]::GetCursorPos([ref]$originalCursor)
        $buttonRect=[NativeUiQA+Rect]::new();[void][NativeUiQA]::GetWindowRect($button,[ref]$buttonRect)
        $point=[NativeUiQA+Point]::new();$point.X=[int](($buttonRect.Left+$buttonRect.Right)/2);$point.Y=[int](($buttonRect.Top+$buttonRect.Bottom)/2)
        [void](Accessible $widget -Focus);[void][NativeUiQA]::SetCursorPos($point.X,$point.Y);Start-Sleep -Milliseconds 180
        try{
            Check ([NativeUiQA]::Owner([NativeUiQA]::WindowFromPoint($point))-eq $process.Id -and [NativeUiQA]::Owner([NativeUiQA]::GetForegroundWindow())-eq $process.Id) 'Physical mouse target and foreground PID guard'
            [NativeUiQA]::mouse_event(2,0,0,0,[UIntPtr]::Zero);Start-Sleep -Milliseconds 210
            Check ((Rect-Of $button)-eq $bounds) 'Physical mouse press preserves the hit target bounds'
        }finally{[NativeUiQA]::mouse_event(4,0,0,0,[UIntPtr]::Zero);[void][NativeUiQA]::SetCursorPos($originalCursor.X,$originalCursor.Y)}
        Start-Sleep -Milliseconds 350
        for($n=0;$n-lt 4;$n++){Command 59}
        Check (@(Toasts).Count-eq 3) 'Toast stack is bounded at three'
        Keys '{ESC}';Check (@(Toasts).Count-eq 2) 'Escape removes newest toast'
        Keys '{ESC}';Keys '{ESC}'
        Check (@(Toasts).Count-eq 0) 'Toast dismissals release all windows'
        Command 59
        $toast=@(Toasts)[0]
        $close=@(Windows|Where-Object{[NativeUiQA]::GetParent($_)-eq $toast -and [NativeUiQA]::GetDlgCtrlID($_)-eq 1})[0]
        [NativeUiQA]::Focus($close)
        [void][NativeUiQA]::PostMessage($close,0x100,[IntPtr]32,[IntPtr]::Zero);Start-Sleep -Milliseconds 210
        Keys '{ESC}';Start-Sleep -Milliseconds 250
        Check (-not [NativeUiQA]::IsWindow($toast) -and -not [NativeUiQA]::IsWindow($close)) 'Dismiss removes a toast while its close button is held'
        if(-not $Release){Check ((Metric 0)-eq 0 -and (Metric 4)-eq 0 -and (Metric 3)-eq 0) 'Destroyed pressed control releases its composition layer'}
        Command 59;$toast=@(Toasts)[0]
        $close=@(Windows|Where-Object{[NativeUiQA]::GetParent($_)-eq $toast -and [NativeUiQA]::GetDlgCtrlID($_)-eq 1})[0]
        [NativeUiQA]::Focus($close);[void][NativeUiQA]::PostMessage($close,0x100,[IntPtr]32,[IntPtr]::Zero);Start-Sleep -Milliseconds 210
        Command 59;Command 59;Command 59
        Check (-not [NativeUiQA]::IsWindow($close)) 'Stack overflow removes the oldest pressed close control'
        if(-not $Release){Check ((Metric 0)-eq 0 -and (Metric 4)-eq 0) 'Stack overflow does not leak a held composition surface'}
        Keys '{ESC}';Keys '{ESC}';Keys '{ESC}'
        if(-not $Release){
            [void][NativeUiQA]::Send($widget,0x8018,10,1);Start-Sleep -Milliseconds 100
            $submitted=Metric 1;Command 59;Command 54
            Check ((Metric 1)-eq $submitted -and (Metric 0)-eq 0) 'Reduced motion applies final state without GPU animation'
            Capture 'reduced-motion'
            [void][NativeUiQA]::Send($widget,0x8018,10,2);Start-Sleep -Milliseconds 100;Command 55;Capture 'high-contrast-simulation'
            Capture-Screen 'screen-high-contrast-simulation'
            Check ((Metric 0)-eq 0 -and (Metric 4)-eq 0) 'High contrast has no transient animation layers'
            $toast=@(Toasts)[0];$dc=[NativeUiQA]::GetDC($toast)
            try{Check ([NativeUiQA]::GetPixel($dc,20,20)-eq [NativeUiQA]::GetSysColor(15)) 'Existing toast adopts the current system contrast colors'}finally{[void][NativeUiQA]::ReleaseDC($toast,$dc)}
            Command 52;Capture 'high-contrast-calendar'
            Command 51;$modal=Palette;Capture 'high-contrast-palette' $modal;Keys '{ESC}'
            [void][NativeUiQA]::Send($widget,0x8018,10,1);Start-Sleep -Milliseconds 100
            Command 55;Command 59;$dpiToast=@(Toasts)[0]
            $originalBounds=[NativeUiQA+Rect]::new();[void][NativeUiQA]::GetWindowRect($widget,[ref]$originalBounds)
            $report.simulatedDpi=@()
            foreach($dpi in @(96,144,192)){
                [void][NativeUiQA]::Send($widget,0x8018,20,$dpi)
                [void][NativeUiQA]::SetWindowPos($widget,[IntPtr]::Zero,$originalBounds.Left,$originalBounds.Top,[int](840*$dpi/96),[int](530*$dpi/96),0x14)
                Start-Sleep -Milliseconds 150
                $buttonBounds=[NativeUiQA+Rect]::new();[void][NativeUiQA]::GetWindowRect((Control 51),[ref]$buttonBounds)
                Check (($buttonBounds.Right-$buttonBounds.Left)-eq [int](150*$dpi/96)) 'Simulated DPI scales the existing command trigger'
                $close=@(Windows|Where-Object{[NativeUiQA]::GetParent($_)-eq $dpiToast -and [NativeUiQA]::GetDlgCtrlID($_)-eq 1})[0]
                [void][NativeUiQA]::GetWindowRect($close,[ref]$buttonBounds)
                Check (($buttonBounds.Right-$buttonBounds.Left)-eq [int](38*$dpi/96)) 'Simulated DPI updates the existing toast close target'
                Capture ('dpi-simulation-'+$dpi);Capture-Screen ('screen-dpi-simulation-'+$dpi);$report.simulatedDpi+=@{dpi=$dpi;physicalMonitorTransition=$false}
            }
            [void][NativeUiQA]::Send($widget,0x8018,20,$report.dpi)
            [void][NativeUiQA]::SetWindowPos($widget,[IntPtr]::Zero,$originalBounds.Left,$originalBounds.Top,$originalBounds.Right-$originalBounds.Left,$originalBounds.Bottom-$originalBounds.Top,0x14)
            [void][NativeUiQA]::Send($widget,0x8018,10,0);Start-Sleep -Milliseconds 150
        }
        Command 51;$modal=Palette;[NativeUiQA]::SetText((Control 101),'하루');Start-Sleep -Milliseconds 100;Keys '{ENTER}'
        Check (-not [NativeUiQA]::IsWindow($modal)) 'Enter executes matching palette command'
        Check ([NativeUiQA]::Send((Control 60),0x18B)-eq 4) 'Palette command opens real updated day view'
        Check (@(Toasts).Count-ge 1) 'Accepted palette command shows a toast'
        Command 55
        for($n=0;$n-lt 8;$n++){Command $(if($n%2){55}else{54});Command 59}
        $expiry=[Diagnostics.Stopwatch]::StartNew()
        do{Start-Sleep -Milliseconds 100}while(@(Toasts).Count-gt 0 -and $expiry.ElapsedMilliseconds-lt 10000)
        Check (@(Toasts).Count-eq 0) 'Automatic toast expiry completes before idle measurement'
        Start-Sleep -Milliseconds 300
        $process.Refresh();$cpu=$process.TotalProcessorTime.TotalMilliseconds;$gdi=[NativeUiQA]::GetGuiResources($process.Handle,0);$user=[NativeUiQA]::GetGuiResources($process.Handle,1)
        $ioBefore=[NativeUiQA+Io]::new();[void][NativeUiQA]::GetProcessIoCounters($process.Handle,[ref]$ioBefore)
        Start-Sleep -Milliseconds 3000;$process.Refresh()
        $ioAfter=[NativeUiQA+Io]::new();[void][NativeUiQA]::GetProcessIoCounters($process.Handle,[ref]$ioAfter)
        $report.idle=@{seconds=3;cpuMs=$process.TotalProcessorTime.TotalMilliseconds-$cpu;gdi=$gdi;user=$user;gdiAfter=[NativeUiQA]::GetGuiResources($process.Handle,0);userAfter=[NativeUiQA]::GetGuiResources($process.Handle,1);readBytes=$ioAfter.ReadBytes-$ioBefore.ReadBytes;writeBytes=$ioAfter.WriteBytes-$ioBefore.WriteBytes;privateBytes=$process.PrivateMemorySize64;workingSetBytes=$process.WorkingSet64}
        Check ($report.idle.cpuMs-lt 100) 'Idle CPU remains below 100ms over three seconds'
        Check ($report.idle.gdiAfter-eq $gdi -and $report.idle.userAfter-eq $user) 'Idle GDI and USER resources remain stable'
        Check ($report.idle.readBytes-eq 0 -and $report.idle.writeBytes-eq 0) 'Idle process performs no file reads or writes'
        if(-not $Release){Check ((Metric 0)-eq 0 -and (Metric 4)-eq 0 -and (Metric 3)-eq 0) 'Repeated interactions leave no animation layers, timers or errors'}
        if(-not $Release){
            [void][NativeUiQA]::Send($widget,0x8018,10,4);Start-Sleep -Milliseconds 100;Command 59
            Check (@(Toasts).Count-eq 1) 'Composition failure retains the toast in static rendering'
            Capture 'static-fallback';Keys '{ESC}'
            $report.injectedFallbackFailures=Metric 3
            Check ($report.injectedFallbackFailures-ge 2 -and (Metric 0)-eq 0 -and (Metric 4)-eq 0) 'Simulated composition failure releases resources and dismisses toast'
            [void][NativeUiQA]::Send($widget,0x8018,10,0)
        }
    }
    Stop-App
    for($n=0;$n-lt 2;$n++){Start-App;Check ((Get-FileHash -LiteralPath $calendar).Hash-eq $fixtureHash) 'Restart preserves fixture bytes';Stop-App}
    if(-not $Smoke){
        $prefsFile=Join-Path $data 'widget.json';$prefs=Get-Content -LiteralPath $prefsFile -Raw|ConvertFrom-Json;$prefs.dark=$false;[IO.File]::WriteAllText($prefsFile,($prefs|ConvertTo-Json -Depth 6),$utf8)
        Start-App;Command 55;Capture 'light-theme';Command 51;$modal=Palette;Capture 'light-palette' $modal;Keys '{ESC}';Stop-App
        Check ((Get-FileHash -LiteralPath $calendar).Hash-eq $fixtureHash) 'Light theme and modal preserve saved calendar'
    }
    $report.status='PASS'
} catch {
    $report.status='FAIL';$report.error=$_.Exception.Message;$_|Out-String|Set-Content -LiteralPath (Join-Path $Output 'error.txt') -Encoding utf8
} finally {
    try{Stop-App}catch{$report.cleanupError=$_.Exception.Message}
    if($stderr -and $stderr.IsCompleted){[IO.File]::WriteAllText((Join-Path $Output 'app-stderr.log'),$stderr.GetAwaiter().GetResult(),$utf8)}
    $report.finishedUtc=[datetime]::UtcNow.ToString('o');Save-Report
}
