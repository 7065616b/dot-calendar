; Built by scripts/build-installer.ps1 from the version-checked portable payload.
#ifndef AppVersion
  #error AppVersion must be set by the build script.
#endif
#ifndef SourceDir
  #error SourceDir must be set by the build script.
#endif

[Setup]
AppId=DotCalendar.7065616b
AppName=Dot Calendar
AppVersion={#AppVersion}
AppVerName=Dot Calendar {#AppVersion}
AppPublisher=Dot Calendar contributors
AppPublisherURL=https://github.com/7065616b/dot-calendar
AppSupportURL=https://github.com/7065616b/dot-calendar/issues
AppUpdatesURL=https://github.com/7065616b/dot-calendar/releases
DefaultDirName={localappdata}\Programs\DotCalendar
DefaultGroupName=Dot Calendar
UninstallDisplayIcon={app}\dot-calendar.exe
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableReadyPage=yes
WizardStyle=modern
ShowLanguageDialog=auto
CloseApplications=yes
RestartApplications=no
Compression=lzma2
SolidCompression=yes
OutputBaseFilename=dot-calendar-{#AppVersion}-windows-x64-setup
OutputDir=..\dist

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "ko"; MessagesFile: "compiler:Languages\Korean.isl"

[CustomMessages]
en.CloseForUpdate=Dot Calendar could not close for this update. Close it and retry.
ko.CloseForUpdate=업데이트를 위해 Dot Calendar를 종료할 수 없습니다. 앱을 닫고 다시 시도해 주세요.
en.CloseForUninstall=Dot Calendar could not close. Finish editing in the calendar and retry uninstalling.
ko.CloseForUninstall=Dot Calendar를 종료하지 못했습니다. 달력에서 편집을 마친 뒤 제거를 다시 시도해 주세요.
en.ConnectFailed=Dot Calendar was installed, but Your dot could not be connected automatically. Open the app and choose Dot to retry.
ko.ConnectFailed=Dot Calendar 설치는 완료됐지만 닷 연결은 자동으로 끝내지 못했습니다. 앱의 Dot 버튼에서 다시 시도해 주세요.

[Files]
; The portable archive is assembled from an explicit allowlist. The installer
; reuses the same verified executable, notices, and Your dot integration.
; Listing the executable first also lets Setup extract it to {tmp} before
; installation, so it can gracefully close even an older portable version.
Source: "{#SourceDir}\dot-calendar.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\*"; Excludes: "dot-calendar.exe,QUICKSTART.txt"; DestDir: "{app}"; Flags: recursesubdirs createallsubdirs ignoreversion

[Icons]
Name: "{userprograms}\Dot Calendar"; Filename: "{app}\dot-calendar.exe"; WorkingDir: "{app}"

[Run]
Filename: "{app}\dot-calendar.exe"; Parameters: "--dot-onboarding"; Description: "{cm:LaunchProgram,Dot Calendar}"; Flags: postinstall nowait skipifsilent

[UninstallRun]
; The app removes its own skill only if that skill still points at this install.
Filename: "{app}\dot-calendar.exe"; Parameters: "--disconnect-dot"; Flags: runhidden skipifdoesntexist; RunOnceId: "disconnect-dot-calendar"

[Code]
var
  DotConnectFailed: Boolean;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ShutdownExe: String;
  ExitCode: Integer;
begin
  Result := '';
  ExtractTemporaryFile('dot-calendar.exe');
  ShutdownExe := ExpandConstant('{tmp}\dot-calendar.exe');
  ExitCode := -1;
  if not Exec(ShutdownExe, '--shutdown', ExpandConstant('{tmp}'),
    SW_HIDE, ewWaitUntilTerminated, ExitCode) or (ExitCode <> 0) then
    Result := CustomMessage('CloseForUpdate');
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  ConnectExe: String;
  ExitCode: Integer;
begin
  if CurStep <> ssPostInstall then
    Exit;
  ConnectExe := ExpandConstant('{app}\dot-calendar.exe');
  ExitCode := -1;
  if not Exec(ConnectExe, '--connect-dot', ExpandConstant('{app}'),
    SW_HIDE, ewWaitUntilTerminated, ExitCode) or (ExitCode <> 0) then
  begin
    DotConnectFailed := True;
    Log('Dot Calendar connection setup failed, exit code ' + IntToStr(ExitCode));
    SuppressibleMsgBox(CustomMessage('ConnectFailed'), mbError, MB_OK, IDOK);
  end;
end;

function GetCustomSetupExitCode: Integer;
begin
  Result := 0;
  if DotConnectFailed then
    Result := 20;
end;

function InitializeUninstall: Boolean;
var
  InstalledExe: String;
  ExitCode: Integer;
begin
  Result := True;
  InstalledExe := ExpandConstant('{app}\dot-calendar.exe');
  if not FileExists(InstalledExe) then
    Exit;
  ExitCode := -1;
  if not Exec(InstalledExe, '--shutdown', ExpandConstant('{app}'),
    SW_HIDE, ewWaitUntilTerminated, ExitCode) or (ExitCode <> 0) then
  begin
    SuppressibleMsgBox(CustomMessage('CloseForUninstall'), mbError, MB_OK, IDOK);
    Result := False;
  end;
end;
