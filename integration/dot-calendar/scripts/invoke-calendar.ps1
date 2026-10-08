param(
    [Parameter(Mandatory)]
    [ValidateSet('calendar_today', 'calendar_list', 'calendar_occurrences', 'calendar_create', 'calendar_update', 'calendar_set_details', 'calendar_delete', 'calendar_deleted', 'calendar_restore')]
    [string] $Tool,
    [string] $ArgumentsJson = '{}',
    [string] $ArgumentsFile,
    [string] $ExePath,
    [string] $DataDirectory,
    [ValidateRange(1000, 60000)] [int] $TimeoutMs = 15000
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ($PSBoundParameters.ContainsKey('ArgumentsJson') -and $PSBoundParameters.ContainsKey('ArgumentsFile')) {
    throw 'Use either -ArgumentsJson or -ArgumentsFile.'
}
if ($ArgumentsFile) { $ArgumentsJson = [IO.File]::ReadAllText((Resolve-Path -LiteralPath $ArgumentsFile).Path, [Text.Encoding]::UTF8) }
try { $arguments = ConvertFrom-Json -InputObject $ArgumentsJson }
catch { throw "Arguments must be a JSON object: $($_.Exception.Message)" }
if ($arguments -isnot [pscustomobject]) { throw 'Arguments must be a JSON object.' }

if (-not $ExePath -or -not $DataDirectory) {
    $connectionPath = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\connection.json'))
    if (-not (Test-Path -LiteralPath $connectionPath -PathType Leaf)) {
        throw "Connection settings not found at $connectionPath. Run the local skill installer or supply -ExePath and -DataDirectory."
    }
    $connection = Get-Content -LiteralPath $connectionPath -Raw -Encoding UTF8 | ConvertFrom-Json
    if (-not $connection.PSObject.Properties['executable_path'] -or
        -not $connection.PSObject.Properties['data_directory']) {
        throw "Connection settings must contain executable_path and data_directory: $connectionPath"
    }
    if (-not $ExePath) { $ExePath = $connection.executable_path }
    if (-not $DataDirectory) { $DataDirectory = $connection.data_directory }
}
if ([string]::IsNullOrWhiteSpace($ExePath) -or
    -not [IO.Path]::GetPathRoot($ExePath).EndsWith('\') -or [IO.Path]::GetPathRoot($ExePath) -eq '\') {
    throw 'Executable path must be absolute.'
}
if ([string]::IsNullOrWhiteSpace($DataDirectory) -or
    -not [IO.Path]::GetPathRoot($DataDirectory).EndsWith('\') -or [IO.Path]::GetPathRoot($DataDirectory) -eq '\') {
    throw 'Calendar data directory must be absolute.'
}
$ExePath = [IO.Path]::GetFullPath($ExePath)
$DataDirectory = [IO.Path]::GetFullPath($DataDirectory)
if (-not (Test-Path -LiteralPath $ExePath -PathType Leaf)) { throw "Calendar executable not found: $ExePath" }

$init = [ordered]@{
    jsonrpc = '2.0'; id = 1; method = 'initialize'
    params = @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'dot-calendar-skill'; version = '1' } }
} | ConvertTo-Json -Compress -Depth 20
$initialized = '{"jsonrpc":"2.0","method":"notifications/initialized"}'
$call = [ordered]@{
    jsonrpc = '2.0'; id = 2; method = 'tools/call'
    params = @{ name = $Tool; arguments = $arguments }
} | ConvertTo-Json -Compress -Depth 30
foreach ($line in @($init, $initialized, $call)) {
    if ([Text.Encoding]::UTF8.GetByteCount($line) + 1 -gt 1MB) { throw 'MCP request exceeds the server 1 MiB line limit.' }
}

$start = [Diagnostics.ProcessStartInfo]::new()
$start.FileName = $ExePath
$start.WorkingDirectory = [IO.Path]::GetDirectoryName($ExePath)
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$start.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
$start.RedirectStandardInput = $true
$start.RedirectStandardOutput = $true
$start.RedirectStandardError = $true
$start.StandardOutputEncoding = [Text.UTF8Encoding]::new($false)
$start.StandardErrorEncoding = [Text.UTF8Encoding]::new($false)
$start.EnvironmentVariables['DOT_CALENDAR_DATA_DIR'] = $DataDirectory
$start.Arguments = '--mcp'

$process = [Diagnostics.Process]::new()
$process.StartInfo = $start
$started = $false
try {
    if (-not $process.Start()) { throw "Could not start calendar executable: $ExePath" }
    $started = $true
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    $inputBytes = [Text.Encoding]::UTF8.GetBytes((@($init, $initialized, $call) -join "`n") + "`n")
    $process.StandardInput.BaseStream.Write($inputBytes, 0, $inputBytes.Length)
    $process.StandardInput.BaseStream.Close()
    if (-not $process.WaitForExit($TimeoutMs)) { throw "Calendar MCP call timed out after $TimeoutMs ms." }
    $stdout = $stdoutTask.GetAwaiter().GetResult()
    $stderr = $stderrTask.GetAwaiter().GetResult()
    if ($process.ExitCode -ne 0) { throw "Calendar MCP process exited $($process.ExitCode): $stderr" }
    $frames = @($stdout -split '\r?\n' | Where-Object { $_.Trim().Length -gt 0 } | ForEach-Object {
        ConvertFrom-Json -InputObject $_
    })
    if ($frames.Count -ne 2 -or $frames[0].id -ne 1 -or $frames[1].id -ne 2) {
        throw "Unexpected Calendar MCP response: $stdout"
    }
    if ($frames[0].PSObject.Properties['error']) { throw "Calendar MCP initialization failed: $($frames[0].error.message)" }
    if ($frames[0].result.protocolVersion -ne '2025-06-18') { throw 'Calendar MCP protocol version mismatch.' }
    if ($frames[1].PSObject.Properties['error']) { throw "Calendar MCP tool call failed: $($frames[1].error.message)" }
    $result = $frames[1].result
    $content = $result.content[0].text
    if ($result.isError) { throw "Calendar $Tool failed: $content" }
    if (-not $content) { throw 'Calendar MCP returned empty tool content.' }
    Write-Output $content
} finally {
    try {
        if ($started -and -not $process.HasExited) {
            $actual = [IO.Path]::GetFullPath($process.MainModule.FileName)
            if (-not [string]::Equals($actual, $ExePath, [StringComparison]::OrdinalIgnoreCase)) {
                throw "Refusing to stop PID $($process.Id): executable path is $actual"
            }
            $process.Kill()
            [void]$process.WaitForExit(5000)
        }
    } finally { $process.Dispose() }
}
