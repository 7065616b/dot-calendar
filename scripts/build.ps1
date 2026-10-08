param([switch]$Debug)
. "$PSScriptRoot\dev-env.ps1"
$cargoArgs = @('build','--locked')
if (-not $Debug) { $cargoArgs += '--release' }
& cargo @cargoArgs
if ($LASTEXITCODE -ne 0) { throw 'Rust build failed.' }
if (-not $Debug) {
    New-Item -ItemType Directory -Path "$projectRoot\dist" -Force | Out-Null
    Copy-Item -LiteralPath "$projectRoot\target\release\dot-calendar.exe" -Destination "$projectRoot\dist\dot-calendar.exe"
    Write-Output "Built $projectRoot\dist\dot-calendar.exe"
}
