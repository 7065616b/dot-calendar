param(
    [string]$ExePath = (Join-Path $PSScriptRoot '..\target\release\dot-calendar.exe'),
    [string]$CompilerPath
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$ExePath = [IO.Path]::GetFullPath($ExePath)
if (-not (Test-Path -LiteralPath $ExePath -PathType Leaf)) {
    throw "Release executable not found: $ExePath"
}

$manifest = Get-Content -LiteralPath (Join-Path $root 'Cargo.toml') -Raw
$package = [regex]::Match($manifest, '(?ms)^\[package\]\s*(.*?)(?=^\[|\z)').Groups[1].Value
$versionMatch = [regex]::Match($package, '(?m)^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"\s*$')
if (-not $versionMatch.Success) { throw 'Cannot determine the application version from Cargo.toml.' }
$version = $versionMatch.Groups[1].Value

if (-not $CompilerPath) {
    $command = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    if ($command) { $CompilerPath = $command.Source }
}
if (-not $CompilerPath) {
    foreach ($candidate in @(
        (Join-Path $root '.tools\inno\ISCC.exe'),
        'C:\Program Files (x86)\Inno Setup 7\ISCC.exe',
        'C:\Program Files\Inno Setup 7\ISCC.exe',
        'C:\Program Files (x86)\Inno Setup 6\ISCC.exe',
        'C:\Program Files\Inno Setup 6\ISCC.exe'
    )) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            $CompilerPath = $candidate
            break
        }
    }
}
if (-not $CompilerPath -or -not (Test-Path -LiteralPath $CompilerPath -PathType Leaf)) {
    throw 'Inno Setup compiler (ISCC.exe) not found. Install Inno Setup 6.3+ or pass -CompilerPath.'
}

# The packager verifies the executable's architecture and embedded MCP version,
# then includes only maintained public files and dependency license notices.
& (Join-Path $PSScriptRoot 'package.ps1') -ExePath $ExePath
if ($LASTEXITCODE -ne 0) { throw 'Portable package creation failed.' }
$dist = Join-Path $root 'dist'
$archive = Join-Path $dist "dot-calendar-$version-windows-x64-preview.zip"
if (-not (Test-Path -LiteralPath $archive -PathType Leaf)) { throw "Portable package missing: $archive" }

$stage = Join-Path $dist ('installer-stage-' + [guid]::NewGuid().ToString('N'))
Expand-Archive -LiteralPath $archive -DestinationPath $stage
if (-not (Test-Path -LiteralPath (Join-Path $stage 'dot-calendar.exe') -PathType Leaf)) {
    throw 'Installer staging did not contain dot-calendar.exe.'
}
if ((Get-FileHash -LiteralPath (Join-Path $stage 'dot-calendar.exe') -Algorithm SHA256).Hash -ne
    (Get-FileHash -LiteralPath $ExePath -Algorithm SHA256).Hash) {
    throw 'Installer staging executable differs from the verified release executable.'
}

$script = Join-Path $root 'installer\dot-calendar.iss'
& $CompilerPath "-dAppVersion=$version" "-dSourceDir=$stage" "-o$dist" $script
if ($LASTEXITCODE -ne 0) { throw "Inno Setup compilation failed ($LASTEXITCODE)." }

$installer = Join-Path $dist "dot-calendar-$version-windows-x64-setup.exe"
if (-not (Test-Path -LiteralPath $installer -PathType Leaf)) {
    throw "Inno Setup did not produce the expected installer: $installer"
}
$hash = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash
Write-Output "Installer: $installer"
Write-Output "SHA256: $hash"
