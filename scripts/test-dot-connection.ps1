param([string]$ExePath = (Join-Path $PSScriptRoot '..\target\release\dot-calendar.exe'))
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$ExePath = (Resolve-Path -LiteralPath $ExePath).Path
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ('..\qa-data\connection-' + [guid]::NewGuid().ToString('N'))))
$testProfile = Join-Path $root '사용자 profile'
$testData = Join-Path $root '일정 data'
$skill = Join-Path $testProfile '.agents\skills\dot-calendar'
[void](New-Item -ItemType Directory -Path $testProfile,$testData -Force)

function Invoke-Connection([string]$Command) {
    $start = [Diagnostics.ProcessStartInfo]::new($ExePath, $Command)
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.EnvironmentVariables['USERPROFILE'] = $testProfile
    $start.EnvironmentVariables['DOT_CALENDAR_DATA_DIR'] = $testData
    $process = [Diagnostics.Process]::Start($start)
    try {
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(15000)) { throw "Connection command timed out: $Command" }
        [void]$stdout.GetAwaiter().GetResult()
        $errorText = $stderr.GetAwaiter().GetResult()
        if ($process.ExitCode -ne 0) { throw "$Command failed: $errorText" }
    } finally { $process.Dispose() }
}

Invoke-Connection '--connect-dot'
$configPath = Join-Path $skill 'connection.json'
$config = Get-Content -LiteralPath $configPath -Raw -Encoding utf8 | ConvertFrom-Json
if ($config.executable_path -ne $ExePath -or $config.data_directory -ne $testData) { throw 'Installed connection has wrong paths.' }
$helper = Join-Path $skill 'scripts\invoke-calendar.ps1'
$json = @{ date='2099-10-08'; title='연결 검사'; notes='본문 그대로'; request_id=[guid]::NewGuid().ToString() } | ConvertTo-Json -Compress
$created = & $helper -Tool calendar_create -ArgumentsJson $json | ConvertFrom-Json
$listed = & $helper -Tool calendar_list | ConvertFrom-Json -NoEnumerate
if ($listed.Count -ne 1 -or $listed[0].id -ne $created.id -or $listed[0].notes -ne '본문 그대로') { throw 'Installed helper did not round-trip the event.' }
$calendar = Join-Path $testData 'calendar.json'
$before = (Get-FileHash -LiteralPath $calendar).Hash
$skillPath = Join-Path $skill 'SKILL.md'
[IO.File]::WriteAllText($skillPath,'custom original')
Invoke-Connection '--connect-dot'
$backups = @(Get-ChildItem -LiteralPath $skill -Filter '*.backup' | Where-Object { [IO.File]::ReadAllText($_.FullName) -eq 'custom original' })
if ($backups.Count -ne 1) { throw 'Customized connection was not preserved before update.' }
$extra = Join-Path $skill 'user-note.txt'
[IO.File]::WriteAllText($extra,'keep')
Invoke-Connection '--disconnect-dot'
if ((Test-Path -LiteralPath $configPath) -or -not (Test-Path -LiteralPath $extra)) { throw 'Disconnect removed unrelated data or left owned connection active.' }
if ((Get-FileHash -LiteralPath $calendar).Hash -ne $before) { throw 'Connection changes modified calendar data.' }
Write-Output "PASS auto connection, installed helper, refresh backup, disconnect/data preservation; fixtures: $root"
