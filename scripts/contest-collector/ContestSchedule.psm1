# Daily execution state is separate from importer ownership/exclusions. Only a
# successful fresh-source pipeline may advance a source's Korean calendar day.
Set-StrictMode -Version Latest
Import-Module (Join-Path $PSScriptRoot 'ContestPaths.psm1')

function Get-ContestKoreanDay {
    param([datetimeoffset]$Now = [datetimeoffset]::UtcNow)
    $Now.ToOffset([timespan]::FromHours(9)).ToString('yyyy-MM-dd')
}

function Save-ContestSchedule {
    param([string]$Path, [Collections.IDictionary]$State)
    Assert-ContestSafePath $Path
    $temp = $Path + '.' + [guid]::NewGuid().ToString('N') + '.tmp'
    $bytes = [Text.UTF8Encoding]::new($false).GetBytes(($State | ConvertTo-Json -Depth 20))
    try {
        $stream = [IO.FileStream]::new($temp, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
        try { $stream.Write($bytes, 0, $bytes.Length); $stream.Flush($true) } finally { $stream.Dispose() }
        if (Test-Path -LiteralPath $Path) { [IO.File]::Replace($temp, $Path, [NullString]::Value) }
        else { [IO.File]::Move($temp, $Path) }
    } finally { if (Test-Path -LiteralPath $temp) { [IO.File]::Delete($temp) } }
}

function Read-ContestSchedule {
    param([string]$Path)
    Assert-ContestSafePath $Path
    if (-not (Test-Path -LiteralPath $Path)) {
        return @{ schemaVersion = 1; lastSuccessKstDate = $null; sources = @{} }
    }
    if ((Get-Item -LiteralPath $Path).Length -gt 1MB) { throw 'Daily state exceeds 1 MiB; it was not reset.' }
    try {
        $jsonOptions = @{ AsHashtable = $true }
        if ((Get-Command ConvertFrom-Json).Parameters.ContainsKey('DateKind')) { $jsonOptions.DateKind = 'String' }
        $state = [IO.File]::ReadAllText($Path) | ConvertFrom-Json @jsonOptions
        if ($state.schemaVersion -ne 1 -or $state.sources -isnot [Collections.IDictionary] -or
            -not $state.Contains('lastSuccessKstDate')) { throw 'Invalid daily state schema.' }
        foreach ($value in $state.sources.Values) {
            foreach ($name in @('attemptDate','attempts','lastSuccessKstDate','nextAttemptAt','status','error')) {
                if (-not $value.Contains($name)) { throw "Missing daily source field: $name" }
            }
            if ($value.attempts -isnot [long] -and $value.attempts -isnot [int]) { throw 'Invalid attempt count.' }
            if ($value.attempts -lt 0 -or $value.attempts -gt 5) { throw 'Invalid attempt count.' }
            foreach ($name in @('attemptDate','lastSuccessKstDate')) {
                if ($value[$name]) { [void][datetime]::ParseExact($value[$name], 'yyyy-MM-dd', [Globalization.CultureInfo]::InvariantCulture) }
            }
            if ($value.nextAttemptAt) { [void][datetimeoffset]::Parse($value.nextAttemptAt, [Globalization.CultureInfo]::InvariantCulture) }
        }
        return $state
    } catch { throw ('Daily state is unreadable; it was not reset: ' + $_.Exception.Message) }
}

function Invoke-ContestDailyRun {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$StateDirectory,
        [Parameter(Mandatory)][scriptblock]$Pipeline,
        [ValidateSet('linkareer','dacon','mokpo','daker','aifactory')]
        [string[]]$Sources = @('linkareer','dacon','mokpo','daker','aifactory'),
        [datetimeoffset]$Now = [datetimeoffset]::UtcNow,
        [scriptblock]$CanContinue = { $true }
    )
    if (-not [IO.Path]::IsPathFullyQualified($StateDirectory)) { throw 'Daily state path must be absolute.' }
    Assert-ContestSafePath $StateDirectory
    [void][IO.Directory]::CreateDirectory($StateDirectory)
    $lock = $null
    try {
        Assert-ContestSafePath (Join-Path $StateDirectory 'daily.lock')
        try { $lock = [IO.FileStream]::new((Join-Path $StateDirectory 'daily.lock'), [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None) }
        catch { return [pscustomobject]@{ status = 'locked'; nextCheckSeconds = 300; sourceResults = @(); error = 'Another daily collector owns the lock; no collection or import attempted.' } }
        $path = Join-Path $StateDirectory 'daily.json'
        $state = Read-ContestSchedule $path
        $day = Get-ContestKoreanDay $Now
        $results = [Collections.Generic.List[object]]::new()
        $sourcesUnique = @($Sources | Select-Object -Unique)
        foreach ($source in $sourcesUnique) {
            if (-not $state.sources.Contains($source)) {
                $state.sources[$source] = @{ attemptDate = $day; attempts = 0; lastSuccessKstDate = $null; nextAttemptAt = $null; status = 'new'; error = $null }
            }
            $s = $state.sources[$source]
            if(-not (& $CanContinue)){continue}
            # A backward wall-clock change must not cause another successful day
            # to be collected again. No time is derived from the OS locale.
            if ($s.lastSuccessKstDate -and [string]$s.lastSuccessKstDate -cge $day) { continue }
            if ($s.attemptDate -cgt $day) { continue }
            if ($s.attemptDate -cne $day) { $s.attemptDate = $day; $s.attempts = 0; $s.nextAttemptAt = $null }
            if ($s.attempts -ge 5) { continue }
            if ($s.nextAttemptAt -and [datetimeoffset]::Parse($s.nextAttemptAt) -gt $Now) { continue }
            $s.attempts++
            $s.status = 'running'
            $s.error = 'A previous attempt may have ended before confirmation; importer request IDs reconcile any completed writes.'
            # Commit intent/backoff before doing work. A crash or restart cannot
            # defeat the retry limit or immediately run the same source again.
            $delay = @(300,900,3600,14400,14400)[$s.attempts - 1]
            $s.nextAttemptAt = $Now.AddSeconds($delay).ToString('o')
            Save-ContestSchedule $path $state
            $runId = $Now.UtcDateTime.ToString('yyyyMMddTHHmmss') + '-' + $source + '-' + [guid]::NewGuid().ToString('N')
            $runDirectory = Join-Path $StateDirectory ('runs\' + $runId)
            Assert-ContestSafePath $runDirectory
            [void][IO.Directory]::CreateDirectory($runDirectory)
            try {
                $result = & $Pipeline $source $runDirectory $runId
                if ($null -eq $result -or -not $result.PSObject.Properties['status'] -or
                    -not $result.PSObject.Properties['freshSource'] -or $result.freshSource -isnot [bool]) {
                    throw 'Pipeline did not return a verified fresh-source result.'
                }
                $s.status = [string]$result.status
                $s.error = if ($result.PSObject.Properties['error']) { $result.error } else { $null }
                if ($result.status -eq 'complete' -and $result.freshSource -eq $true) {
                    $s.lastSuccessKstDate = $day; $s.nextAttemptAt = $null; $s.error = $null
                } elseif (-not $s.error) { $s.error = 'Source or import is incomplete; success day was not advanced.' }
                if($result.PSObject.Properties['retryable'] -and $result.retryable -eq $false){$s.attempts=5;$s.nextAttemptAt=$null}
                $results.Add([pscustomobject]@{ source = $source; result = $result; runDirectory = $runDirectory })
            } catch {
                $s.status = 'error'; $s.error = $_.Exception.Message
                $results.Add([pscustomobject]@{ source = $source; result = [pscustomobject]@{status='error';freshSource=$false;error=$s.error}; runDirectory = $runDirectory })
            }
            $s.lastRunDirectory = $runDirectory
            Save-ContestSchedule $path $state
        }
        $complete = @($sourcesUnique | Where-Object { -not $state.sources[$_].lastSuccessKstDate -or [string]$state.sources[$_].lastSuccessKstDate -clt $day }).Count -eq 0
        if ($complete -and (-not $state.lastSuccessKstDate -or $state.lastSuccessKstDate -clt $day)) {
            $state.lastSuccessKstDate = $day; Save-ContestSchedule $path $state
        }
        # This is scheduling metadata, not an instruction to keep a watcher alive.
        $kst = $Now.ToOffset([timespan]::FromHours(9))
        $morning = [datetimeoffset]::new($kst.Year,$kst.Month,$kst.Day,6,0,0,$kst.Offset).AddDays(1)
        $next = $morning
        foreach ($source in $sourcesUnique) {
            $s = $state.sources[$source]
            if ($s.lastSuccessKstDate -eq $day -or $s.attempts -ge 5 -or -not $s.nextAttemptAt) { continue }
            $due = [datetimeoffset]::Parse($s.nextAttemptAt)
            if ($due -lt $next) { $next = $due }
        }
        return [pscustomobject]@{ status = $(if($complete){'complete'}else{'partial'}); dateKst=$day; lastSuccessKstDate=$state.lastSuccessKstDate; sourceResults=$results.ToArray(); nextCheckSeconds=[Math]::Max(30,[int][Math]::Ceiling(($next-$Now).TotalSeconds)) }
    } finally { if ($null -ne $lock) { $lock.Dispose() } }
}

Export-ModuleMember -Function Invoke-ContestDailyRun, Get-ContestKoreanDay
