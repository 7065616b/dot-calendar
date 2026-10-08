param(
    [string]$ExePath,
    [string]$DataDirectory,
    [string]$SkillsDirectory = (Join-Path $env:USERPROFILE '.agents\skills')
)
$ErrorActionPreference = 'Stop'
$packageRoot = Split-Path -Parent $PSScriptRoot
$source = Join-Path $packageRoot 'integration\dot-calendar'
if (-not $ExePath) {
    $ExePath = Join-Path $packageRoot 'dot-calendar.exe'
    if (-not (Test-Path -LiteralPath $ExePath -PathType Leaf)) {
        $ExePath = Join-Path $packageRoot 'dist\dot-calendar.exe'
    }
}
$ExePath = (Resolve-Path -LiteralPath $ExePath).Path
if (-not $DataDirectory) {
    $DataDirectory = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'DotCalendar'
}
$DataDirectory = [IO.Path]::GetFullPath($DataDirectory)
$destination = Join-Path ([IO.Path]::GetFullPath($SkillsDirectory)) 'dot-calendar'
foreach ($file in @('SKILL.md', 'agents\openai.yaml', 'scripts\invoke-calendar.ps1')) {
    if (-not (Test-Path -LiteralPath (Join-Path $source $file) -PathType Leaf)) {
        throw "Missing skill source: $file"
    }
}
# Only this app's three maintained files and connection are installed. Other skills stay intact.
foreach ($file in @('SKILL.md', 'agents\openai.yaml', 'scripts\invoke-calendar.ps1')) {
    $target = Join-Path $destination $file
    New-Item -ItemType Directory -Path (Split-Path -Parent $target) -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $source $file) -Destination $target -Force
}
$config = @{ executable_path = $ExePath; data_directory = $DataDirectory } | ConvertTo-Json
[IO.File]::WriteAllText((Join-Path $destination 'connection.json'), $config, [Text.UTF8Encoding]::new($false))
Write-Output "Installed Dot Calendar skill: $destination"
Write-Output "Calendar data: $DataDirectory"
Write-Output 'Keep the executable at this location; rerun this installer after moving it.'
Write-Output 'In Your dot, ask to use the connected PC and dot-calendar skill to read today first.'
