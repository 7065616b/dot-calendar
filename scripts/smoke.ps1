param(
    [string] $ExePath = (Join-Path $PSScriptRoot '..\dist\dot-calendar.exe'),
    [int] $TimeoutMs = 15000
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ($TimeoutMs -lt 1000) { throw 'TimeoutMs must be at least 1000.' }
$ExePath = [System.IO.Path]::GetFullPath($ExePath)
if (-not (Test-Path -LiteralPath $ExePath -PathType Leaf)) {
    throw "Executable does not exist: $ExePath"
}

$workspace = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$dataDir = Join-Path $workspace ('qa-data\smoke-' + [guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory -Path $dataDir -Force)

function Assert-True([bool] $Condition, [string] $Message) {
    if (-not $Condition) { throw $Message }
}

function Start-TestProcess([string[]] $Arguments) {
    $start = [System.Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $ExePath
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardInput = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.Environment['DOT_CALENDAR_DATA_DIR'] = $dataDir
    foreach ($argument in $Arguments) { [void]$start.ArgumentList.Add($argument) }

    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $start
    try {
        if (-not $process.Start()) { throw "Could not start $ExePath" }
        return [pscustomobject]@{
            Process = $process
            StdoutTask = $process.StandardOutput.ReadToEndAsync()
            StderrTask = $process.StandardError.ReadToEndAsync()
        }
    } catch {
        $process.Dispose()
        throw
    }
}

function Finish-TestProcess($Run, [string] $InputText = '') {
    $process = $Run.Process
    try {
        if ($InputText.Length -gt 0) { $process.StandardInput.Write($InputText) }
        $process.StandardInput.Close()
        if (-not $process.WaitForExit($TimeoutMs)) {
            try { $process.Kill($true) } catch { try { $process.Kill() } catch {} }
            throw "Process timed out after $TimeoutMs ms: $($process.StartInfo.FileName) $($process.StartInfo.Arguments)"
        }
        $stdout = $Run.StdoutTask.GetAwaiter().GetResult()
        $stderr = $Run.StderrTask.GetAwaiter().GetResult()
        return [pscustomobject]@{ ExitCode = $process.ExitCode; Stdout = $stdout; Stderr = $stderr }
    } finally {
        $process.Dispose()
    }
}

function Invoke-Calendar([string[]] $Arguments) {
    $run = Start-TestProcess $Arguments
    return Finish-TestProcess $run
}

function Expect-Success($Result, [string] $Label) {
    Assert-True ($Result.ExitCode -eq 0) "$Label failed (exit $($Result.ExitCode)): $($Result.Stderr)"
    Assert-True (-not [string]::IsNullOrWhiteSpace($Result.Stdout)) "$Label returned empty stdout"
    $parsed = ConvertFrom-Json -InputObject $Result.Stdout -Depth 30 -NoEnumerate
    return ,$parsed
}

try {
    $empty = Expect-Success (Invoke-Calendar @('list', '--date', '2026-10-07')) 'initial list'
    Assert-True (@($empty).Count -eq 0) 'Initial calendar is not empty.'

    $created = Expect-Success (Invoke-Calendar @('add', '--date', '2026-10-07', '--time', '15:00', '--title', 'Smoke meeting', '--notes', 'Initial', '--request-id', 'smoke-one', '--color', '#12ABEF', '--recurrence', 'weekly', '--reminder-minutes', '15')) 'add'
    Assert-True ($created.id -and $created.time -eq '15:00' -and $created.recurrence -eq 'weekly') 'Created event is incomplete.'
    $retry = Expect-Success (Invoke-Calendar @('add', '--date', '2026-10-07', '--time', '15:00', '--title', 'Smoke meeting', '--notes', 'Initial', '--request-id', 'smoke-one', '--color', '#12ABEF', '--recurrence', 'weekly', '--reminder-minutes', '15')) 'idempotent add retry'
    Assert-True ($retry.id -eq $created.id) 'The same request-id created a second event.'
    $occurring = Expect-Success (Invoke-Calendar @('occurrences', '--from', '2026-10-07', '--to', '2026-10-21')) 'weekly occurrences'
    Assert-True (@($occurring).Count -eq 3) 'Weekly recurrence did not produce three occurrences.'
    $details = Expect-Success (Invoke-Calendar @('details', '--id', [string]$created.id, '--completed', 'true', '--color', 'none')) 'details patch'
    Assert-True ($details.completed -and $null -eq $details.color -and $details.recurrence -eq 'weekly') 'Details patch changed an omitted field or failed to clear color.'

    # Each command is a fresh process. This list also checks persistence after reopening.
    $reopened = Expect-Success (Invoke-Calendar @('list', '--date', '2026-10-07')) 'reopen/list'
    Assert-True (@($reopened).Count -eq 1) 'Reopened process did not see exactly one event.'

    $updated = Expect-Success (Invoke-Calendar @('update', '--id', [string]$created.id, '--date', '2026-10-08', '--title', 'Smoke moved')) 'update'
    Assert-True ($updated.date -eq '2026-10-08' -and $null -eq $updated.time -and $updated.notes -eq '' -and $updated.completed -and $updated.recurrence -eq 'weekly') 'Update failed to replace text or preserve details.'
    $oldDay = Expect-Success (Invoke-Calendar @('list', '--date', '2026-10-07')) 'list old day'
    Assert-True (@($oldDay).Count -eq 0) 'Updated event remains on old day.'

    $deleted = Expect-Success (Invoke-Calendar @('delete', '--id', [string]$created.id)) 'delete'
    Assert-True ($deleted.id -eq $created.id) 'Delete returned wrong event.'
    $retired = Invoke-Calendar @('add', '--date', '2026-10-07', '--time', '15:00', '--title', 'Smoke meeting', '--notes', 'Initial', '--request-id', 'smoke-one')
    Assert-True ($retired.ExitCode -ne 0) 'Deleted request-id was reused; tombstone missing.'

    $invalid = Invoke-Calendar @('add', '--date', '2026-02-30', '--title', 'Invalid')
    Assert-True ($invalid.ExitCode -ne 0) 'Invalid calendar date was accepted.'
    Write-Output 'PASS CLI CRUD, reopen, retry, tombstone, invalid input'

    $count = 16
    $runs = @()
    for ($i = 0; $i -lt $count; $i++) {
        $runs += Start-TestProcess @('add', '--date', '2027-01-14', '--title', "Concurrent $i", '--request-id', "concurrent-$i")
    }
    foreach ($run in $runs) {
        $result = Finish-TestProcess $run
        [void](Expect-Success $result 'concurrent add')
    }
    $concurrent = Expect-Success (Invoke-Calendar @('list', '--date', '2027-01-14')) 'concurrent list'
    Assert-True (@($concurrent).Count -eq $count) "Lost update: expected $count events, found $(@($concurrent).Count)."
    $uniqueIds = @($concurrent | ForEach-Object { $_.id } | Sort-Object -Unique)
    Assert-True ($uniqueIds.Count -eq $count) 'Concurrent event IDs are not unique.'
    Write-Output "PASS $count concurrent inserts without lost updates"

    $backupPath = Join-Path $dataDir 'backup.json'
    [void](Expect-Success (Invoke-Calendar @('backup-export', '--file', $backupPath)) 'backup export')
    Assert-True ((Test-Path -LiteralPath $backupPath) -and (Get-Content -LiteralPath $backupPath -Raw).Contains('"events"')) 'Backup export did not write event data.'
    [void](Expect-Success (Invoke-Calendar @('delete', '--id', [string]$concurrent[0].id)) 'delete before restore')
    [void](Expect-Success (Invoke-Calendar @('backup-restore', '--file', $backupPath)) 'backup restore')
    $restored = Expect-Success (Invoke-Calendar @('list', '--date', '2027-01-14')) 'restored list'
    Assert-True (@($restored).Count -eq $count) 'Backup restore did not recover all events.'
    Write-Output 'PASS full JSON backup export and restore'

    $messages = @(
        @{ jsonrpc = '2.0'; id = 1; method = 'initialize'; params = @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'smoke'; version = '1' } } }
        @{ jsonrpc = '2.0'; method = 'notifications/initialized' }
        @{ jsonrpc = '2.0'; id = 2; method = 'tools/list'; params = @{} }
        @{ jsonrpc = '2.0'; id = 3; method = 'tools/call'; params = @{ name = 'calendar_create'; arguments = @{ date = '2099-12-31'; time = '09:30'; title = 'MCP smoke'; request_id = 'mcp-smoke'; recurrence = 'daily'; reminder_minutes = 5 } } }
        @{ jsonrpc = '2.0'; id = 4; method = 'tools/call'; params = @{ name = 'calendar_list'; arguments = @{ date = '2099-12-31' } } }
        @{ jsonrpc = '2.0'; id = 5; method = 'tools/call'; params = @{ name = 'calendar_update'; arguments = @{ id = '<replace>'; date = '2099-12-31'; title = 'MCP changed' } } }
        @{ jsonrpc = '2.0'; id = 6; method = 'tools/call'; params = @{ name = 'calendar_delete'; arguments = @{ id = '<replace>' } } }
        @{ jsonrpc = '2.0'; id = 7; method = 'tools/call'; params = @{ name = 'calendar_create'; arguments = @{ date = 123; title = 'bad'; request_id = 'bad' } } }
        @{ jsonrpc = '1.0'; id = 8; method = 'ping' }
    )

    # First MCP process discovers the event id; a second process tests update/delete.
    $firstLines = ($messages[0..4] | ForEach-Object { ConvertTo-Json -InputObject $_ -Compress -Depth 20 }) -join "`n"
    $first = Finish-TestProcess (Start-TestProcess @('--mcp')) ($firstLines + "`n")
    Assert-True ($first.ExitCode -eq 0) "MCP first process failed: $($first.Stderr)"
    $responses = @($first.Stdout -split '\r?\n' | Where-Object { $_.Trim().Length -gt 0 } | ForEach-Object { ConvertFrom-Json -InputObject $_ -Depth 30 })
    Assert-True ($responses.Count -eq 4) "MCP expected four responses, found $($responses.Count)."
    Assert-True ($responses[0].result.protocolVersion -eq '2025-06-18') 'MCP initialization version mismatch.'
    $toolNames = @($responses[1].result.tools | ForEach-Object { $_.name })
    Assert-True ($toolNames.Count -eq 9) "MCP expected nine tools, found $($toolNames.Count)."
    foreach ($name in @('calendar_today', 'calendar_list', 'calendar_occurrences', 'calendar_create', 'calendar_update', 'calendar_set_details', 'calendar_delete', 'calendar_deleted', 'calendar_restore')) {
        Assert-True ($toolNames -contains $name) "MCP tool absent: $name"
    }
    Assert-True (-not $responses[2].result.isError) 'MCP create returned a tool error.'
    $createdMcp = ConvertFrom-Json -InputObject $responses[2].result.content[0].text -Depth 30
    Assert-True (-not [string]::IsNullOrEmpty($createdMcp.id)) 'MCP create did not return an event id.'
    $listedMcp = ConvertFrom-Json -InputObject $responses[3].result.content[0].text -Depth 30 -NoEnumerate
    Assert-True (@($listedMcp).Count -eq 1 -and $listedMcp[0].id -eq $createdMcp.id) 'MCP list did not return the created event.'

    $messages[5].params.arguments.id = [string]$createdMcp.id
    $messages[6].params.arguments.id = [string]$createdMcp.id
    $setDetails = @{ jsonrpc = '2.0'; id = 9; method = 'tools/call'; params = @{ name = 'calendar_set_details'; arguments = @{ id = [string]$createdMcp.id; completed = $true; color = '#334455' } } }
    $listOccurrences = @{ jsonrpc = '2.0'; id = 10; method = 'tools/call'; params = @{ name = 'calendar_occurrences'; arguments = @{ from = '2099-12-30'; to = '2100-01-02' } } }
    $second = @($messages[0], $messages[1], $messages[5], $setDetails, $listOccurrences, $messages[6], $messages[7], $messages[8])
    $secondLines = ($second | ForEach-Object { ConvertTo-Json -InputObject $_ -Compress -Depth 20 }) -join "`n"
    $finish = Finish-TestProcess (Start-TestProcess @('--mcp')) ($secondLines + "`n")
    Assert-True ($finish.ExitCode -eq 0) "MCP second process failed: $($finish.Stderr)"
    $responses = @($finish.Stdout -split '\r?\n' | Where-Object { $_.Trim().Length -gt 0 } | ForEach-Object { ConvertFrom-Json -InputObject $_ -Depth 30 })
    Assert-True ($responses.Count -eq 7) "MCP expected seven responses, found $($responses.Count)."
    Assert-True (-not $responses[1].result.isError -and -not $responses[2].result.isError -and -not $responses[3].result.isError -and -not $responses[4].result.isError) 'MCP update/details/occurrences/delete returned an error.'
    $updatedMcp = ConvertFrom-Json -InputObject $responses[1].result.content[0].text -Depth 30
    Assert-True ($updatedMcp.title -eq 'MCP changed') 'MCP update did not change title.'
    $detailsMcp = ConvertFrom-Json -InputObject $responses[2].result.content[0].text -Depth 30
    Assert-True ($detailsMcp.completed -and $detailsMcp.color -eq '#334455') 'MCP details did not persist.'
    $occurrencesMcp = ConvertFrom-Json -InputObject $responses[3].result.content[0].text -Depth 30 -NoEnumerate
    Assert-True (@($occurrencesMcp).Count -eq 3) 'MCP daily recurrence did not expand the date range.'
    Assert-True ($responses[5].result.isError -eq $true) 'MCP invalid tool arguments did not set isError.'
    Assert-True ($responses[6].error.code -eq -32600) 'MCP invalid protocol did not return -32600.'
    $afterDelete = Expect-Success (Invoke-Calendar @('list', '--date', '2099-12-31')) 'MCP delete verification'
    Assert-True (@($afterDelete).Count -eq 0) 'MCP delete did not persist.'
    Write-Output 'PASS MCP initialize, tools/list, CRUD, invalid arguments and protocol'
    Write-Output "PASS smoke fixtures retained at $dataDir"
} catch {
    throw "Smoke test failed. Fixtures retained at $dataDir. $($_.Exception.Message)"
}
