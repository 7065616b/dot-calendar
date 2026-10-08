param([string] $ExePath = (Join-Path $PSScriptRoot '..\target\release\dot-calendar.exe'))

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$ExePath = [IO.Path]::GetFullPath($ExePath)
if (-not (Test-Path -LiteralPath $ExePath -PathType Leaf)) { throw "Release executable not found: $ExePath" }

# Refuse to label a different architecture as the Windows x64 package.
$reader = [IO.BinaryReader]::new([IO.File]::OpenRead($ExePath))
try {
    if ($reader.ReadUInt16() -ne 0x5a4d) { throw 'Executable is not a Windows PE file.' }
    $reader.BaseStream.Position = 0x3c
    $reader.BaseStream.Position = $reader.ReadInt32()
    if ($reader.ReadUInt32() -ne 0x4550 -or $reader.ReadUInt16() -ne 0x8664) {
        throw 'Executable is not Windows x64.'
    }
} finally { $reader.Dispose() }

$localCargo = Join-Path $root '.tools\cargo\bin\cargo.exe'
if (Test-Path -LiteralPath $localCargo -PathType Leaf) {
    $env:CARGO_HOME = Join-Path $root '.tools\cargo'
    $env:RUSTUP_HOME = Join-Path $root '.tools\rustup'
    $cargo = $localCargo
    $rustc = Join-Path $env:CARGO_HOME 'bin\rustc.exe'
} else {
    $cargo = (Get-Command cargo -ErrorAction Stop).Source
    $rustc = (Get-Command rustc -ErrorAction Stop).Source
}
$manifest = Join-Path $root 'Cargo.toml'
$metadataJson = & $cargo metadata --locked --offline --format-version 1 --filter-platform x86_64-pc-windows-gnu --manifest-path $manifest
if ($LASTEXITCODE -ne 0) { throw 'Cargo.lock is not ready for offline packaging; run an offline locked build first.' }
$metadata = $metadataJson | ConvertFrom-Json
$app = @($metadata.packages | Where-Object { $_.source -eq $null -and $_.name -eq 'dot-calendar' })
if ($app.Count -ne 1 -or $app[0].version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?$') {
    throw 'Cannot determine the Dot Calendar package version from Cargo metadata.'
}
$version = $app[0].version
$resolved = @{}
foreach ($node in $metadata.resolve.nodes) { $resolved[[string]$node.id] = $true }
$dependencies = @($metadata.packages | Where-Object { $_.source -ne $null -and $resolved.ContainsKey([string]$_.id) } | Sort-Object name, version)

$dist = Join-Path $root 'dist'
$stage = Join-Path $dist ('package-stage-' + [guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory -Path $stage -Force)
$probe = Join-Path $stage 'version-probe'
[void](New-Item -ItemType Directory -Path $probe)
$start = [Diagnostics.ProcessStartInfo]::new()
$start.FileName = $ExePath
$start.Arguments = '--mcp'
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$start.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
$start.RedirectStandardInput = $true
$start.RedirectStandardOutput = $true
$start.RedirectStandardError = $true
$start.StandardOutputEncoding = [Text.UTF8Encoding]::new($false)
$start.StandardErrorEncoding = [Text.UTF8Encoding]::new($false)
$start.EnvironmentVariables['DOT_CALENDAR_DATA_DIR'] = $probe
$process = [Diagnostics.Process]::new()
$process.StartInfo = $start
$started = $false
try {
    if (-not $process.Start()) { throw 'Cannot start executable for version check.' }
    $started = $true
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    $request = '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"package","version":"1"}}}' + "`n"
    $bytes = [Text.Encoding]::ASCII.GetBytes($request)
    $process.StandardInput.BaseStream.Write($bytes, 0, $bytes.Length)
    $process.StandardInput.BaseStream.Close()
    if (-not $process.WaitForExit(10000)) { throw 'Executable version check timed out.' }
    $stdout = $stdoutTask.GetAwaiter().GetResult()
    $stderr = $stderrTask.GetAwaiter().GetResult()
    if ($process.ExitCode -ne 0) { throw "Executable version check failed: $stderr" }
    $frames = @($stdout -split '\r?\n' | Where-Object { $_.Trim() })
    if ($frames.Count -ne 1) { throw "Unexpected executable version response: $stdout" }
    $info = ConvertFrom-Json -InputObject $frames[0]
    if ($info.result.serverInfo.name -ne 'dot-calendar' -or $info.result.serverInfo.version -ne $version) {
        throw "Executable version $($info.result.serverInfo.version) differs from Cargo package $version. Rebuild before packaging."
    }
} finally {
    try {
        if ($started -and -not $process.HasExited) {
            $actual = [IO.Path]::GetFullPath($process.MainModule.FileName)
            if (-not [string]::Equals($actual, $ExePath, [StringComparison]::OrdinalIgnoreCase)) {
                throw "Refusing to stop unrelated process $($process.Id): $actual"
            }
            $process.Kill()
            [void]$process.WaitForExit(5000)
        }
    } finally { $process.Dispose() }
}

$payload = Join-Path $stage 'payload'
[void](New-Item -ItemType Directory -Path $payload)
Copy-Item -LiteralPath $ExePath -Destination (Join-Path $payload 'dot-calendar.exe')
$files = @(
    'README.md', 'LICENSE', 'CONTRIBUTING.md', 'THIRD_PARTY_NOTICES.md',
    'integration\README.md', 'integration\google.md',
    'integration\dot-calendar\SKILL.md',
    'integration\dot-calendar\agents\openai.yaml',
    'integration\dot-calendar\scripts\invoke-calendar.ps1',
    'scripts\install-dot-skill.ps1'
)
if (Test-Path -LiteralPath (Join-Path $root 'resources\app.manifest') -PathType Leaf) {
    $files += 'resources\app.manifest'
}
foreach ($file in $files) {
    $source = Join-Path $root $file
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw "Package source is missing: $source" }
    $target = Join-Path $payload $file
    [void](New-Item -ItemType Directory -Path (Split-Path -Parent $target) -Force)
    Copy-Item -LiteralPath $source -Destination $target
}

$licenseRoot = Join-Path $payload 'licenses'
[void](New-Item -ItemType Directory -Path $licenseRoot)
$index = [Collections.Generic.List[string]]::new()
$index.Add('# Resolved Rust dependency licenses')
$index.Add('')
$index.Add('Generated from `cargo metadata --locked --offline --filter-platform x86_64-pc-windows-gnu`. This index does not declare a license for the application itself.')
$index.Add('')
$index.Add('| Crate | Version | Declared license | Bundled texts |')
$index.Add('| --- | --- | --- | --- |')
foreach ($package in $dependencies) {
    $crateDir = Split-Path -Parent $package.manifest_path
    if (-not (Test-Path -LiteralPath $crateDir -PathType Container)) { throw "Crate source is missing: $($package.name) $($package.version)" }
    $crateTarget = Join-Path $licenseRoot ("$($package.name)-$($package.version)")
    $candidates = @(Get-ChildItem -LiteralPath $crateDir -File -Recurse | Where-Object { $_.Name -match '^(LICENSE|NOTICE|COPYING)(?:$|[._-])' })
    if ($package.license_file) {
        $declared = Join-Path $crateDir $package.license_file
        if (Test-Path -LiteralPath $declared -PathType Leaf) { $candidates += Get-Item -LiteralPath $declared }
    }
    $candidates = @($candidates | Sort-Object FullName -Unique)
    $included = [Collections.Generic.List[string]]::new()
    foreach ($file in $candidates) {
        $full = [IO.Path]::GetFullPath($file.FullName)
        if (-not $full.StartsWith($crateDir.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)) {
            throw "License file escaped crate source: $full"
        }
        $bytes = [IO.File]::ReadAllBytes($full)
        if ([Array]::IndexOf($bytes, [byte]0) -ge 0) { continue }
        $relative = $full.Substring($crateDir.TrimEnd('\').Length).TrimStart('\')
        $target = Join-Path $crateTarget $relative
        [void](New-Item -ItemType Directory -Path (Split-Path -Parent $target) -Force)
        [IO.File]::WriteAllBytes($target, $bytes)
        $included.Add($relative.Replace('\', '/'))
    }
    if ($included.Count -eq 0) { throw "No text license or notice found for $($package.name) $($package.version)" }
    $declaredLicense = if ($package.license) { [string]$package.license } elseif ($package.license_file) { 'license-file' } else { 'not declared' }
    $index.Add("| $($package.name) | $($package.version) | $($declaredLicense.Replace('|', '\|')) | $($included -join ', ') |")
}
$sysroot = (& $rustc --print sysroot).Trim()
if ($LASTEXITCODE -ne 0) { throw 'Cannot locate the Rust toolchain sysroot.' }
$rustVersion = (& $rustc --version).Trim()
if ($LASTEXITCODE -ne 0) { throw 'Cannot determine the Rust toolchain version.' }
$rustDocs = Join-Path $sysroot 'share\doc\rust'
foreach ($file in @('COPYRIGHT.html', 'COPYRIGHT-library.html')) {
    if (-not (Test-Path -LiteralPath (Join-Path $rustDocs $file) -PathType Leaf)) {
        throw "Rust standard library notice is missing: $file"
    }
}
if (-not (Test-Path -LiteralPath (Join-Path $rustDocs 'licenses') -PathType Container)) {
    throw 'Rust standard library license texts are missing.'
}
$rustTarget = Join-Path $licenseRoot 'rust-stdlib'
[void](New-Item -ItemType Directory -Path $rustTarget)
foreach ($file in @('COPYRIGHT.html', 'COPYRIGHT-library.html')) {
    Copy-Item -LiteralPath (Join-Path $rustDocs $file) -Destination (Join-Path $rustTarget $file)
}
Copy-Item -LiteralPath (Join-Path $rustDocs 'licenses') -Destination $rustTarget -Recurse
$index.Add('')
$index.Add("Rust standard library notices and license texts from $rustVersion are bundled in rust-stdlib/.")
[IO.File]::WriteAllLines((Join-Path $licenseRoot 'INDEX.md'), $index, [Text.UTF8Encoding]::new($false))

$quickstart = @'
Dot Calendar portable preview

1. Extract this entire ZIP, then run dot-calendar.exe to open the desktop widget.
2. Click Dot in the calendar. Local integration is prepared automatically.
3. Copy the one-time introduction from that panel and send it to Your dot.
   After that, use short requests such as "Add a meeting tomorrow at 3 PM."

The setup.exe download handles installation and connection preparation together.
Your PC must be connected to Your dot and online with the ChatGPT app open.

Google Calendar is optional and requires your own Desktop OAuth client ID in
the widget settings. This ZIP does not contain account credentials or events.
'@
[IO.File]::WriteAllText((Join-Path $payload 'QUICKSTART.txt'), $quickstart, [Text.UTF8Encoding]::new($false))

$zip = Join-Path $dist "dot-calendar-$version-windows-x64-preview.zip"
Compress-Archive -Path (Join-Path $payload '*') -DestinationPath $zip -Force
Write-Output "Packaged $zip ($($dependencies.Count) resolved Rust dependencies; staging retained at $stage)"
