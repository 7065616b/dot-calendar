Set-StrictMode -Version 3.0

$script:ContestAutoFields = @('date', 'time', 'title', 'notes')
$script:ContestEventFields = @('id', 'date', 'time', 'title', 'notes', 'request_id', 'completed', 'color', 'recurrence', 'reminder_minutes')

function Get-ContestProperty {
    param($Object, [string] $Name, $Default = $null)
    if ($null -eq $Object) { return $Default }
    if ($Object -is [System.Collections.IDictionary]) {
        if ($Object.Contains($Name)) { return $Object[$Name] }
        return $Default
    }
    $property = $Object.PSObject.Properties[$Name]
    if ($null -ne $property) { return $property.Value }
    return $Default
}

function Copy-ContestObject {
    param($Value)
    return ($Value | ConvertTo-Json -Depth 60 -Compress | ConvertFrom-Json)
}

function Test-ContestValueEqual {
    param($Left, $Right)
    if ($null -eq $Left -or $null -eq $Right) { return ($null -eq $Left -and $null -eq $Right) }
    return [string]::Equals([string]$Left, [string]$Right, [StringComparison]::Ordinal)
}

function Get-ContestProjection {
    param($Event)
    return [pscustomobject][ordered]@{
        date = Get-ContestProperty $Event 'date'
        time = Get-ContestProperty $Event 'time'
        title = Get-ContestProperty $Event 'title'
        notes = Get-ContestProperty $Event 'notes' ''
    }
}

function Test-ContestProjectionEqual {
    param($Left, $Right)
    foreach ($field in $script:ContestAutoFields) {
        if (-not (Test-ContestValueEqual (Get-ContestProperty $Left $field) (Get-ContestProperty $Right $field))) { return $false }
    }
    return $true
}

function Get-ContestName {
    param([string] $Value)
    return ([regex]::Replace($Value.ToLowerInvariant(), '[^\p{L}\p{N}]', ''))
}

function Get-ContestCoreName {
    param([string]$Value)
    # Only a conservative duplicate guard, never proof of identity/ownership.
    # Portal republication commonly adds a publisher prefix and explicit year.
    $plain=$Value -replace '^\s*(?:\[[^\]]+\]\s*)+',''
    $plain=$plain -replace '^\s*20\d{2}\s*년?\s*',''
    return Get-ContestName $plain
}

function Get-ContestEdition {
    param($Entry)
    # Do not derive identity from a deadline year: a correction may cross a year.
    $edition = [string](Get-ContestProperty $Entry 'edition')
    if ($edition) { return $edition }
    $explicit = [regex]::Match([string](Get-ContestProperty $Entry 'contestTitle'), '(?<!\d)(20\d{2})(?!\d)')
    if ($explicit.Success) { return $explicit.Groups[1].Value }
    return ''
}

function Get-ContestUrl {
    param([string] $Value)
    if ([string]::IsNullOrWhiteSpace($Value)) { return '' }
    $uri = $null
    if (-not [Uri]::TryCreate($Value.Trim(), [UriKind]::Absolute, [ref]$uri) -or $uri.Scheme -notin @('https', 'http')) { return '' }
    $builder = [UriBuilder]::new($uri)
    $builder.Host = $uri.DnsSafeHost.ToLowerInvariant()
    if ($builder.Host.StartsWith('www.')) { $builder.Host = $builder.Host.Substring(4) }
    $builder.Fragment = ''
    $builder.Path = [regex]::Replace($uri.AbsolutePath, '(?i);jsessionid=[^/;]*', '')
    $parts = @($builder.Query.TrimStart('?') -split '&' | Where-Object {
        if (-not $_) { return $false }
        $name = [Uri]::UnescapeDataString(($_ -split '=',2)[0].Replace('+',' ')).ToLowerInvariant()
        return ($name -notmatch '^utm_' -and $name -notin @('fbclid','gclid','jsessionid'))
    })
    $builder.Query = ($parts | Sort-Object) -join '&'
    return $builder.Uri.AbsoluteUri.TrimEnd('/')
}

function Get-ContestStage {
    param([string] $Value)
    $value = $Value.ToLowerInvariant()
    if ($value -match 'submit|submission|pdf|제출|기획서|제안서|보고서|작품|자료') { return 'submission' }
    if ($value -match 'result|announcement|(^|\W)selection(\W|$)|결과|진출팀\s*발표') { return 'result' }
    if ($value -match 'award|ceremony|시상') { return 'award' }
    if ($value -match 'education|training|mentoring|교육|멘토링') { return 'training' }
    if ($value -match 'orientation|briefing|설명회|(^|\W)ot(\W|$)') { return 'briefing' }
    if ($value -match 'final|onsite|본선|현장|발표평가') { return 'final' }
    if ($value -match 'screening|judging|preliminary|심사|예선') { return 'judging' }
    if ($value -match 'apply|application|registration|recruit|접수|신청|모집') { return 'application' }
    return ''
}

function Test-ContestSourceError {
    param($SourceResult)
    $status = ([string](Get-ContestProperty $SourceResult 'status')).ToLowerInvariant()
    return ($status -in @('failed','error','offline','unavailable','partial','blocked','unsupported','not_run','notrun','denied','timeout','timed_out','auth_required','connection_required'))
}

function Get-ContestNewExclusion {
    param($Context, $Entry)
    foreach ($field in @('candidateStatus','recruitmentStatus','registrationStatus','contestStatus')) {
        $status = ([string](Get-ContestProperty $Entry $field)).ToLowerInvariant()
        if ($status -in @('closed','ended','expired','cancelled','canceled','completed','practice','test','종료','마감','취소')) {
            return ('Source status excludes new creation: ' + $status)
        }
    }
    if ((Get-ContestProperty $Entry 'newCreationBlocked' $false) -eq $true) { return 'Source explicitly blocks new creation.' }
    $date = [DateTime]::ParseExact([string]$Entry.date, 'yyyy-MM-dd', [Globalization.CultureInfo]::InvariantCulture)
    if ($date -lt $Context.NowKst.Date) { return 'Past-date candidate: retained for matching, excluded from new creation.' }
    $isDeadline = [string]$Entry.stage -match 'deadline|마감' -or [string]$Entry.title -match '마감'
    if ($date -eq $Context.NowKst.Date -and $isDeadline -and $null -ne $Entry.time) {
        $end = [DateTimeOffset]::ParseExact(([string]$Entry.date + 'T' + [string]$Entry.time + ':00+09:00'), 'yyyy-MM-ddTHH:mm:sszzz', [Globalization.CultureInfo]::InvariantCulture)
        if ($end -le $Context.NowKst) { return 'Confirmed deadline has already passed in Asia/Seoul; no new event.' }
    }
    return ''
}

function Test-ContestNewExpired {
    param($Context, $Entry)
    return -not [string]::IsNullOrEmpty((Get-ContestNewExclusion $Context $Entry))
}

function New-ContestRequestId {
    param([string] $Key)
    # UUID v5, URL namespace. The immutable source/ID/stage key is the name.
    $namespace = [byte[]]@(0x6b,0xa7,0xb8,0x11,0x9d,0xad,0x11,0xd1,0x80,0xb4,0x00,0xc0,0x4f,0xd4,0x30,0xc8)
    $name = [Text.Encoding]::UTF8.GetBytes('dot-calendar-collector/v1/' + $Key)
    $inputBytes = [byte[]]::new($namespace.Length + $name.Length)
    [Array]::Copy($namespace, 0, $inputBytes, 0, $namespace.Length)
    [Array]::Copy($name, 0, $inputBytes, $namespace.Length, $name.Length)
    $sha = [Security.Cryptography.SHA1]::Create()
    try { $bytes = $sha.ComputeHash($inputBytes) } finally { $sha.Dispose() }
    $bytes[6] = [byte](($bytes[6] -band 0x0f) -bor 0x50)
    $bytes[8] = [byte](($bytes[8] -band 0x3f) -bor 0x80)
    $hex = -join ($bytes[0..15] | ForEach-Object { $_.ToString('x2') })
    return ('{0}-{1}-{2}-{3}-{4}' -f $hex.Substring(0,8),$hex.Substring(8,4),$hex.Substring(12,4),$hex.Substring(16,4),$hex.Substring(20,12))
}

function Invoke-ContestCalendar {
    param($Context, [string] $Tool, $Arguments)
    if ($Tool -notin @('calendar_list', 'calendar_deleted', 'calendar_create', 'calendar_update')) { throw 'Unsupported collector tool.' }
    if ($Context.Mode -eq 'Preview' -and $Tool -notin @('calendar_list', 'calendar_deleted')) { throw 'Preview cannot write the calendar.' }
    $raw = @(& $Context.Invoker $Tool $Arguments)
    $value = $raw
    if ($raw.Count -eq 1) {
        $value = $raw[0]
        if ($value -is [string]) {
            if ($value.Trim() -eq '[]') { $value = [object[]]@() }
            else { $value = $value | ConvertFrom-Json -ErrorAction Stop }
        }
    }
    if ($null -ne $value -and (Get-ContestProperty $value 'isError' $false)) { throw 'Calendar invoker returned an error result.' }
    return [pscustomobject]@{ Value = $value }
}

function Get-ContestCalendarRows {
    param($Context, [string] $Tool)
    $response = Invoke-ContestCalendar $Context $Tool @{}
    $rows = @($response.Value)
    $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($row in $rows) {
        if ($null -eq $row) { throw 'Calendar returned a null event.' }
        foreach ($field in $script:ContestEventFields) {
            if ($null -eq $row.PSObject.Properties[$field] -and -not ($row -is [Collections.IDictionary] -and $row.Contains($field))) {
                throw ('Calendar event is missing snapshot field: ' + $field)
            }
        }
        $id = [string](Get-ContestProperty $row 'id')
        if (-not $id -or -not $seen.Add($id)) { throw 'Calendar returned missing or duplicate event IDs.' }
    }
    return [pscustomobject]@{ Rows = $rows }
}

function Add-ContestOperation {
    param($Context, [string] $Action, [string] $Key, [string] $Reason, $EventId = $null, $Fields = @())
    [void]$Context.Operations.Add([pscustomobject][ordered]@{
        action = $Action; key = $Key; eventId = $EventId; reason = $Reason; fields = @($Fields)
    })
}

function Add-ContestPending {
    param($Context, [string] $Key, [string] $Reason, $EventId = $null, $Fields = @())
    $Context.Report.counts.pending++
    Add-ContestOperation $Context 'pending' $Key $Reason $EventId $Fields
}

function Assert-ContestProjection {
    param($Value)
    if ($null -eq $Value) { throw 'Collector state has a missing field baseline.' }
    foreach ($field in $script:ContestAutoFields) {
        if ($null -eq $Value.PSObject.Properties[$field] -and -not ($Value -is [Collections.IDictionary] -and $Value.Contains($field))) { throw ('Collector state baseline is incomplete: ' + $field) }
    }
    [void][DateTime]::ParseExact([string](Get-ContestProperty $Value 'date'), 'yyyy-MM-dd', [Globalization.CultureInfo]::InvariantCulture)
    $time = Get-ContestProperty $Value 'time'
    if ($null -ne $time -and ($time -isnot [string] -or $time -notmatch '^([01][0-9]|2[0-3]):[0-5][0-9]$')) { throw 'Collector state baseline has an invalid time.' }
    $title = Get-ContestProperty $Value 'title'
    $notes = Get-ContestProperty $Value 'notes'
    if ($title -isnot [string] -or [string]::IsNullOrWhiteSpace($title) -or $title.Length -gt 200 -or $notes -isnot [string] -or $notes.Length -gt 4096) { throw 'Collector state baseline has invalid title/notes.' }
}

function Read-ContestState {
    param([string] $Path)
    if (-not [IO.File]::Exists($Path)) {
        return [pscustomobject][ordered]@{ schemaVersion = 1; owner = 'parent'; entries = @(); updatedAt = $null; lastRunId = $null }
    }
    try {
        $utf8 = [Text.UTF8Encoding]::new($false, $true)
        $state = $utf8.GetString([IO.File]::ReadAllBytes($Path)) | ConvertFrom-Json -ErrorAction Stop
    } catch { throw ('Collector state is invalid; it was not replaced: ' + $_.Exception.Message) }
    if ((Get-ContestProperty $state 'schemaVersion') -ne 1 -or (Get-ContestProperty $state 'owner') -ne 'parent' -or $null -eq $state.PSObject.Properties['entries']) {
        throw 'Unsupported collector state; it was not replaced.'
    }
    $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($mapping in @($state.entries)) {
        foreach ($field in @('key','identityKey','aliases','source','sourceId','sourceUrl','officialUrl','contestTitle','organizer','edition','stage','requestId','eventId','owned','status','lastWritten','protectedFields','intent','pendingReason','lastSeenAt')) {
            if ($null -eq $mapping.PSObject.Properties[$field]) { throw ('Collector state mapping is incomplete: ' + $field) }
        }
        if (-not $mapping.key -or -not $seen.Add([string]$mapping.key)) { throw 'Collector state has duplicate or missing keys.' }
        if ($mapping.owned -isnot [bool] -or ($mapping.owned -and -not $mapping.requestId)) { throw 'Collector state has invalid ownership.' }
        if ($mapping.owned -and $mapping.requestId -cne (New-ContestRequestId ([string]$mapping.key))) { throw 'Collector state request UUID does not match its immutable key.' }
        if ($mapping.owned -and $mapping.status -ne 'suppressed' -and $null -eq $mapping.lastWritten -and $null -eq $mapping.intent) { throw 'Collector state has no ownership baseline.' }
        if ($mapping.status -notin @('active','pending_create','pending_update','pending','suppressed')) { throw 'Collector state has an invalid status.' }
        if (-not $mapping.identityKey -or -not $mapping.stage -or $null -eq $mapping.aliases -or $null -eq $mapping.protectedFields) { throw 'Collector state has incomplete identity/protection metadata.' }
        foreach ($field in @($mapping.protectedFields)) { if ($field -notin $script:ContestAutoFields) { throw 'Collector state has an unknown protected field.' } }
        if ($null -ne $mapping.lastWritten) { Assert-ContestProjection $mapping.lastWritten }
        if ($null -ne $mapping.intent) {
            if (-not $mapping.owned -or (Get-ContestProperty $mapping.intent 'kind') -notin @('create','update')) { throw 'Collector state has an invalid intent.' }
            foreach ($field in @('kind','requestId','payload','desired','before')) {
                if ($null -eq $mapping.intent.PSObject.Properties[$field]) { throw ('Collector state intent is incomplete: ' + $field) }
            }
            if (-not (Test-ContestValueEqual $mapping.requestId $mapping.intent.requestId)) { throw 'Collector state intent request UUID differs from ownership.' }
            Assert-ContestProjection $mapping.intent.desired
            if (-not (Test-ContestProjectionEqual $mapping.intent.desired (Get-ContestProjection $mapping.intent.payload))) { throw 'Collector state intent payload differs from its baseline.' }
            if ($mapping.intent.kind -eq 'create') {
                if (-not (Test-ContestValueEqual $mapping.requestId (Get-ContestProperty $mapping.intent.payload 'request_id')) -or (Get-ContestProperty $mapping.intent.payload 'completed') -ne $false -or $null -ne $mapping.intent.before) { throw 'Collector state create payload has invalid ownership fields.' }
                $allowed = @('date','time','title','notes','request_id','completed')
                $required = @('date','title','notes','request_id','completed')
            } else {
                $before = $mapping.intent.before
                Assert-ContestProjection $before
                foreach ($field in $script:ContestEventFields) { if ($null -eq $before.PSObject.Properties[$field]) { throw 'Collector state update snapshot is incomplete.' } }
                if (-not (Test-ContestValueEqual (Get-ContestProperty $before 'id') $mapping.eventId) -or -not (Test-ContestValueEqual (Get-ContestProperty $before 'request_id') $mapping.requestId) -or
                    -not (Test-ContestValueEqual (Get-ContestProperty $mapping.intent.payload 'id') $mapping.eventId) -or
                    ($before | ConvertTo-Json -Depth 30 -Compress) -cne ((Get-ContestProperty $mapping.intent.payload 'expected') | ConvertTo-Json -Depth 30 -Compress)) { throw 'Collector state update intent has inconsistent full snapshot.' }
                $allowed = @('id','date','time','title','notes','expected')
                $required = @('id','date','title','notes','expected')
            }
            foreach ($field in $required) { if ($null -eq $mapping.intent.payload.PSObject.Properties[$field]) { throw ('Collector state intent payload is incomplete: ' + $field) } }
            foreach ($property in $mapping.intent.payload.PSObject.Properties) { if ($property.Name -notin $allowed) { throw 'Collector state intent has an unsupported payload field.' } }
        }
    }
    return $state
}

function Read-ContestExclusions {
    param([string] $Path)
    try { $bytes = [IO.File]::ReadAllBytes($Path) }
    catch {
        $failure = $_.Exception
        while ($null -ne $failure.InnerException) { $failure = $failure.InnerException }
        if ($failure -is [IO.FileNotFoundException] -or $failure -is [IO.DirectoryNotFoundException]) { return [pscustomobject]@{ Contests = @() } }
        throw ('Collector exclusions cannot be read; no calendar calls are permitted: ' + $failure.Message)
    }
    try {
        $utf8 = [Text.UTF8Encoding]::new($false, $true)
        $document = $utf8.GetString($bytes) | ConvertFrom-Json -ErrorAction Stop
        $schema = Get-ContestProperty $document 'schemaVersion'
        if (($schema -isnot [int] -and $schema -isnot [long]) -or $schema -ne 1 -or (Get-ContestProperty $document 'owner') -cne 'parent') { throw 'Unsupported exclusions schema or owner.' }
        if ($null -eq $document.PSObject.Properties['contests'] -or $null -eq $document.contests -or $document.contests -is [string] -or $document.contests -is [Collections.IDictionary] -or $document.contests -isnot [Collections.IEnumerable]) { throw 'Exclusions contests must be an array.' }
        $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        foreach ($contest in @($document.contests)) {
            foreach ($field in @('source','sourceId','sourceUrl','officialUrl','contestTitle','edition','reason','recordedAt','deletedEventIds')) {
                if ($null -eq $contest -or $null -eq $contest.PSObject.Properties[$field]) { throw ('Exclusion is missing: ' + $field) }
            }
            foreach ($field in @('source','sourceId','sourceUrl','contestTitle','reason')) {
                $value = Get-ContestProperty $contest $field
                if ($value -isnot [string] -or [string]::IsNullOrWhiteSpace($value) -or $value -match '[\x00-\x1f]') { throw ('Exclusion has invalid: ' + $field) }
            }
            if (-not $seen.Add($contest.source.ToLowerInvariant() + '|' + $contest.sourceId)) { throw 'Exclusions have duplicate source IDs.' }
            if (-not (Get-ContestUrl $contest.sourceUrl)) { throw 'Exclusion sourceUrl must be an absolute HTTP URL.' }
            if ($null -ne $contest.officialUrl -and ($contest.officialUrl -isnot [string] -or -not (Get-ContestUrl $contest.officialUrl))) { throw 'Exclusion officialUrl must be null or an absolute HTTP URL.' }
            if ($null -ne $contest.edition -and ($contest.edition -isnot [string] -or $contest.edition -match '[\x00-\x1f]')) { throw 'Exclusion edition must be null or explicit text.' }
            # PowerShell 7 may deserialize an ISO timestamp as DateTime automatically.
            if ($contest.recordedAt -is [DateTime] -and $contest.recordedAt.Kind -eq [DateTimeKind]::Unspecified) { throw 'Exclusion recordedAt must have an explicit time zone.' }
            if ($contest.recordedAt -isnot [DateTime] -and $contest.recordedAt -isnot [DateTimeOffset]) {
                if ($contest.recordedAt -isnot [string] -or $contest.recordedAt -notmatch '^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,7})?(?:Z|[+-]\d{2}:\d{2})$') { throw 'Exclusion recordedAt must be an ISO timestamp with a time zone.' }
                [void][DateTimeOffset]::Parse($contest.recordedAt, [Globalization.CultureInfo]::InvariantCulture)
            }
            if ($null -eq $contest.deletedEventIds -or $contest.deletedEventIds -is [string] -or $contest.deletedEventIds -is [Collections.IDictionary] -or $contest.deletedEventIds -isnot [Collections.IEnumerable]) { throw 'Exclusion deletedEventIds must be an array.' }
            foreach ($eventId in @($contest.deletedEventIds)) { if ($eventId -isnot [string] -or [string]::IsNullOrWhiteSpace($eventId)) { throw 'Exclusion deletedEventIds contains an invalid ID.' } }
        }
        return [pscustomobject]@{ Contests = @($document.contests) }
    } catch { throw ('Collector exclusions are invalid; no calendar calls are permitted and the file was not replaced: ' + $_.Exception.Message) }
}

function Get-ContestExclusionMatch {
    param($Exclusions, $Entry)
    foreach ($contest in @($Exclusions)) {
        $directId = [string]::Equals([string]$contest.source, [string](Get-ContestProperty $Entry 'source'), [StringComparison]::OrdinalIgnoreCase) -and
            (Test-ContestValueEqual $contest.sourceId (Get-ContestProperty $Entry 'sourceId'))
        $prefix = $contest.source + ':' + $contest.sourceId + ':'
        $knownAlias = @((Get-ContestProperty $Entry 'aliases') | Where-Object { $_ -is [string] -and $_.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase) }).Count -gt 0
        if ($directId -or $knownAlias) {
            return [pscustomobject]@{ Kind = 'excluded'; Reason = 'User contest exclusion (source + ID, all stages): ' + $contest.reason }
        }
    }
    $url = Get-ContestUrl ([string](Get-ContestProperty $Entry 'officialUrl'))
    $edition = Get-ContestEdition $Entry
    $uncertain = $null
    $core = Get-ContestCoreName ([string](Get-ContestProperty $Entry 'contestTitle'))
    foreach ($contest in @($Exclusions)) {
        $excludedEdition = Get-ContestEdition $contest
        if($core.Length -ge 8 -and $core -eq (Get-ContestCoreName ([string]$contest.contestTitle)) -and
            (-not $edition -or -not $excludedEdition -or $edition -eq $excludedEdition)) {
            $uncertain='Possible republication of a user-excluded contest; source/edition identity requires review, no calendar write.'
        }
        if (-not $url -or -not (Test-ContestValueEqual $url (Get-ContestUrl ([string]$contest.officialUrl)))) { continue }
        $excludedEdition = Get-ContestEdition $contest
        if ($edition -and $excludedEdition -and (Get-ContestName $edition) -eq (Get-ContestName $excludedEdition)) {
            return [pscustomobject]@{ Kind = 'excluded'; Reason = 'User contest exclusion (official URL + explicit edition alias): ' + $contest.reason }
        }
        if (-not $edition -or -not $excludedEdition) { $uncertain = 'Official URL matches a user-excluded contest but the edition is unconfirmed; no calendar write.' }
        # A different explicit edition is a different contest cycle.
    }
    if ($uncertain) { return [pscustomobject]@{ Kind = 'pending'; Reason = $uncertain } }
    return [pscustomobject]@{ Kind = ''; Reason = '' }
}

function Stop-ContestExcludedEntry {
    param($Context, $Entry, $EventId = $null)
    $match = Get-ContestExclusionMatch $Context.Exclusions $Entry
    if ($match.Kind -eq 'excluded') {
        $Context.Report.counts.excluded++
        Add-ContestOperation $Context 'excluded' ([string](Get-ContestProperty $Entry 'key')) $match.Reason $EventId
        return $true
    }
    if ($match.Kind -eq 'pending') {
        Add-ContestPending $Context ([string](Get-ContestProperty $Entry 'key')) $match.Reason $EventId
        return $true
    }
    return $false
}

function Save-ContestState {
    param($Context)
    if ($Context.Mode -ne 'Apply') { return }
    if ($Context.StateWriteFailed) { throw 'State persistence failed earlier; no further writes are allowed.' }
    $Context.State.entries = @($Context.Mappings.ToArray())
    $Context.State.updatedAt = [DateTime]::UtcNow.ToString('o')
    $Context.State.lastRunId = $Context.Report.runId
    $bytes = [Text.UTF8Encoding]::new($false).GetBytes(($Context.State | ConvertTo-Json -Depth 60))
    $temp = Join-Path $Context.Directory ('.state-' + [Guid]::NewGuid().ToString('N') + '.tmp')
    $stream = $null
    try {
        $stream = [IO.FileStream]::new($temp, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
        $stream.Write($bytes, 0, $bytes.Length)
        $stream.Flush($true)
        $stream.Dispose(); $stream = $null
        if ([IO.File]::Exists($Context.StatePath)) { [IO.File]::Replace($temp, $Context.StatePath, ($Context.StatePath + '.previous'), $true) }
        else { [IO.File]::Move($temp, $Context.StatePath) }
    } catch {
        $Context.StateWriteFailed = $true
        throw ('Collector state was not committed; saved calendar data must be reconciled using the previous intent: ' + $_.Exception.Message)
    } finally {
        if ($null -ne $stream) { $stream.Dispose() }
        if ([IO.File]::Exists($temp)) { [IO.File]::Delete($temp) }
    }
}

function New-ContestMapping {
    param($Entry, $Event = $null, [bool] $Owned = $false)
    return [pscustomobject][ordered]@{
        key = [string]$Entry.key; identityKey = [string]$Entry.identityKey; aliases = @([string]$Entry.key)
        source = [string]$Entry.source; sourceId = [string]$Entry.sourceId; sourceUrl = [string]$Entry.sourceUrl
        officialUrl = [string]$Entry.officialUrl; contestTitle = [string]$Entry.contestTitle; organizer = [string]$Entry.organizer
        edition = Get-ContestEdition $Entry; stage = [string]$Entry.stage
        requestId = if ($Owned) { New-ContestRequestId ([string]$Entry.key) } else { Get-ContestProperty $Event 'request_id' }
        eventId = Get-ContestProperty $Event 'id'; owned = $Owned; status = if ($null -ne $Event) { 'active' } else { 'pending_create' }
        lastWritten = if ($null -ne $Event) { Get-ContestProjection $Event } else { $null }
        protectedFields = @(); intent = $null; pendingReason = $null; lastSeenAt = $null
    }
}

function Find-ContestMappings {
    param($Context, $Entry)
    $matches = [Collections.Generic.List[object]]::new()
    $possible = [Collections.Generic.List[object]]::new()
    foreach ($mapping in $Context.Mappings) {
        $sameKey = Test-ContestValueEqual $mapping.key $Entry.key
        $aliasKey = @($mapping.aliases | Where-Object { Test-ContestValueEqual $_ $Entry.key }).Count -gt 0
        $sameStage = Test-ContestValueEqual $mapping.stage $Entry.stage
        $sameIdentity = $sameStage -and (Test-ContestValueEqual $mapping.identityKey $Entry.identityKey)
        $edition = Get-ContestEdition $Entry
        $sameName = $sameStage -and $edition -and $edition -eq $mapping.edition -and
            (Get-ContestName $mapping.contestTitle) -eq (Get-ContestName $Entry.contestTitle) -and
            (Get-ContestName $mapping.organizer) -and (Get-ContestName $mapping.organizer) -eq (Get-ContestName $Entry.organizer)
        $oldUrl = Get-ContestUrl ([string]$mapping.officialUrl)
        $newUrl = Get-ContestUrl ([string]$Entry.officialUrl)
        $urlConflict = $oldUrl -and $newUrl -and -not (Test-ContestValueEqual $oldUrl $newUrl)
        if ($sameKey -or $aliasKey) { [void]$matches.Add($mapping) }
        elseif (($sameIdentity -or $sameName) -and $urlConflict) { [void]$possible.Add($mapping) }
        elseif ($sameIdentity -and (-not $edition -or -not $mapping.edition -or $edition -ne $mapping.edition)) {
            # A generic annual application URL is not proof of the same source cycle.
            [void]$possible.Add($mapping)
        }
        elseif ($sameIdentity -or $sameName) { [void]$matches.Add($mapping) }
    }
    return [pscustomobject]@{ Rows = @($matches.ToArray()); Possible = @($possible.ToArray()) }
}

function Test-ContestLegacyMatch {
    param($Entry, $Event)
    $title = [string](Get-ContestProperty $Event 'title')
    $notes = [string](Get-ContestProperty $Event 'notes')
    $base = Get-ContestName ([string]$Entry.contestTitle)
    $eventName = Get-ContestName $title
    $exactTitle = (Get-ContestName ([string]$Entry.title)) -eq $eventName
    $baseMatch = $base.Length -ge 4 -and $eventName.Contains($base)
    $sourceUrls = @((Get-ContestUrl ([string]$Entry.sourceUrl)), (Get-ContestUrl ([string]$Entry.officialUrl))) | Where-Object { $_ }
    $urlMatch = $false
    foreach ($match in [regex]::Matches($notes, 'https?://[^\s<>"\x27]+')) {
        if ((Get-ContestUrl $match.Value.TrimEnd('.', ',', ')', ']', ';')) -in $sourceUrls) { $urlMatch = $true; break }
    }
    $stage = Get-ContestStage ([string]$Entry.stage)
    if (-not $stage) { $stage = Get-ContestStage ([string]$Entry.title) }
    $eventStage = Get-ContestStage $title
    $sameStage = $stage -and $eventStage -and $stage -eq $eventStage
    $sameYear = ([string](Get-ContestProperty $Event 'date')).Substring(0,4) -eq ([string]$Entry.date).Substring(0,4)
    if ($exactTitle -and ($urlMatch -or $sameYear)) { return 'match' }
    if ($urlMatch -and $baseMatch -and $sameStage) { return 'match' }
    # A source-linked user-renamed/date-edited row is a reason to hold, not to create another row.
    if ($urlMatch -and ($baseMatch -or $exactTitle) -and (-not $stage -or -not $eventStage)) { return 'possible' }
    if ($urlMatch -and (-not $baseMatch -or -not $stage -or -not $eventStage)) { return 'possible' }
    if ($baseMatch -and $sameStage -and $sameYear) { return 'possible' }
    $core = Get-ContestCoreName ([string]$Entry.contestTitle)
    $titleYear=[regex]::Match($title,'(?<!\d)(20\d{2})(?!\d)')
    $edition=Get-ContestEdition $Entry
    $differentPrintedYear=$titleYear.Success -and $edition -match '^20\d{2}$' -and $edition -ne $titleYear.Groups[1].Value
    if($core.Length -ge 8 -and $eventName.Contains($core) -and $sameStage -and -not $differentPrintedYear){return 'possible'}
    return ''
}

function Find-ContestLegacy {
    param($Context, $Entry, $Rows = $null)
    $matches = [Collections.Generic.List[object]]::new()
    $possible = [Collections.Generic.List[object]]::new()
    if ($null -eq $Rows) { $Rows = $Context.Active }
    foreach ($event in @($Rows)) {
        $kind = Test-ContestLegacyMatch $Entry $event
        if ($kind -eq 'match') { [void]$matches.Add($event) }
        elseif ($kind -eq 'possible') { [void]$possible.Add($event) }
    }
    return [pscustomobject]@{ Matches = @($matches.ToArray()); Possible = @($possible.ToArray()) }
}

function Set-ContestSuppressed {
    param($Context, $Mapping, [string] $Reason)
    $Mapping.status = 'suppressed'; $Mapping.pendingReason = $Reason; $Mapping.intent = $null
    Save-ContestState $Context
    Add-ContestPending $Context $Mapping.key $Reason $Mapping.eventId
}

function Get-ContestMappedEvent {
    param($Context, $Mapping)
    $matches = @($Context.Active | Where-Object {
        ($Mapping.eventId -and (Test-ContestValueEqual (Get-ContestProperty $_ 'id') $Mapping.eventId)) -or
        ($Mapping.owned -and $Mapping.requestId -and (Test-ContestValueEqual (Get-ContestProperty $_ 'request_id') $Mapping.requestId))
    })
    if ($matches.Count -gt 1) { throw 'Mapping resolved to multiple calendar events; no write is safe.' }
    if ($matches.Count -eq 1 -and $Mapping.owned -and -not (Test-ContestValueEqual (Get-ContestProperty $matches[0] 'request_id') $Mapping.requestId)) {
        throw 'Owned event request UUID no longer matches state; no write is safe.'
    }
    return [pscustomobject]@{ Event = if ($matches.Count -eq 1) { $matches[0] } else { $null } }
}

function Test-ContestDeletedMapping {
    param($Context, $Mapping)
    return (@($Context.Deleted | Where-Object {
        ($Mapping.eventId -and (Test-ContestValueEqual (Get-ContestProperty $_ 'id') $Mapping.eventId)) -or
        ($Mapping.requestId -and (Test-ContestValueEqual (Get-ContestProperty $_ 'request_id') $Mapping.requestId))
    }).Count -gt 0)
}

function New-ContestCreatePayload {
    param($Entry, [string] $RequestId)
    $payload = [ordered]@{ date = [string]$Entry.date; title = [string]$Entry.title; notes = [string]$Entry.notes; request_id = $RequestId; completed = $false }
    if ($null -ne $Entry.time) { $payload.time = [string]$Entry.time }
    return $payload
}

function New-ContestUpdatePayload {
    param($Event, $Desired)
    $payload = [ordered]@{
        id = Get-ContestProperty $Event 'id'; date = $Desired.date; title = $Desired.title; notes = $Desired.notes
        expected = Copy-ContestObject $Event
    }
    # Omission clears time in this API. Here it is intentional only when the desired value is null.
    if ($null -ne $Desired.time) { $payload.time = $Desired.time }
    return $payload
}

function Complete-ContestIntent {
    param($Context, $Mapping, $Event, $Baseline)
    $Mapping.eventId = Get-ContestProperty $Event 'id'
    $Mapping.lastWritten = Get-ContestProjection $Baseline
    $Mapping.status = 'active'; $Mapping.intent = $null; $Mapping.pendingReason = $null
    Save-ContestState $Context
}

function Invoke-ContestWriteIntent {
    param($Context, $Mapping, $Entry)
    # Re-read immediately before a write so a newly recorded exclusion also stops a retained intent.
    $Context.Exclusions = (Read-ContestExclusions (Join-Path $Context.Directory 'exclusions.json')).Contests
    if ($null -ne $Entry -and (Stop-ContestExcludedEntry $Context $Entry)) { return }
    if (Stop-ContestExcludedEntry $Context $Mapping $Mapping.eventId) { return }
    $intent = $Mapping.intent
    $tool = if ($intent.kind -eq 'create') { 'calendar_create' } else { 'calendar_update' }
    # The caller has already committed this exact payload and request UUID to state.
    $callError = $null
    try { [void](Invoke-ContestCalendar $Context $tool $intent.payload) }
    catch { $callError = $_.Exception.Message }
    try { $Context.Active = (Get-ContestCalendarRows $Context 'calendar_list').Rows }
    catch {
        $Context.Report.counts.sourceErrors++
        Add-ContestPending $Context $Mapping.key 'Write outcome is unknown; intent is retained until a successful calendar_list.' $Mapping.eventId
        Add-ContestOperation $Context 'calendar_error' $Mapping.key $_.Exception.Message $Mapping.eventId
        $Context.CalendarOffline = $true
        return
    }
    $event = (Get-ContestMappedEvent $Context $Mapping).Event
    if ($null -ne $event -and (Test-ContestProjectionEqual (Get-ContestProjection $event) $intent.desired)) {
        $counter = if ($intent.kind -eq 'create') { 'created' } else { 'updated' }
        $Context.Report.counts.$counter++
        Add-ContestOperation $Context $counter $Mapping.key $(if ($callError) { 'Saved data confirmed after an uncertain API response.' } else { 'Saved data confirmed by calendar_list.' }) $Mapping.eventId
        Complete-ContestIntent $Context $Mapping $event $intent.desired
        return
    }
    if ($null -ne $event) {
        if ($intent.kind -eq 'update' -and (Test-ContestProjectionEqual (Get-ContestProjection $event) (Get-ContestProjection $intent.before))) {
            # The requested change did not happen. Do not call old values user edits.
            $Mapping.status = 'pending_update'
            $Mapping.pendingReason = if ($callError) { $callError } else { 'Update did not become visible; retry only with a fresh full snapshot.' }
            Save-ContestState $Context
            $Context.Report.counts.sourceErrors++
            Add-ContestPending $Context $Mapping.key $Mapping.pendingReason $Mapping.eventId
            return
        }
        $Mapping.eventId = Get-ContestProperty $event 'id'
        if ($intent.kind -eq 'create' -and $null -eq $Mapping.lastWritten) { $Mapping.lastWritten = Get-ContestProjection $intent.desired }
        $Mapping.status = 'pending'; $Mapping.pendingReason = 'Saved event differs from the intended fields; user changes are preserved.'
        foreach ($field in $script:ContestAutoFields) {
            if (-not (Test-ContestValueEqual (Get-ContestProperty $event $field) (Get-ContestProperty $intent.desired $field))) {
                if ($field -notin @($Mapping.protectedFields)) { $Mapping.protectedFields = @($Mapping.protectedFields) + $field }
            }
        }
        $Mapping.intent = $null
        Save-ContestState $Context
        Add-ContestPending $Context $Mapping.key $Mapping.pendingReason $Mapping.eventId $Mapping.protectedFields
        return
    }
    $Context.Deleted = (Get-ContestCalendarRows $Context 'calendar_deleted').Rows
    if ((Test-ContestDeletedMapping $Context $Mapping) -or ($callError -and $callError.Contains('request-id belongs to a deleted event'))) {
        Set-ContestSuppressed $Context $Mapping 'User-deleted event; no restore, replacement UUID, or recreation is permitted.'
        return
    }
    $Mapping.pendingReason = if ($callError) { $callError } else { 'Write returned without a matching saved event.' }
    Save-ContestState $Context
    $Context.Report.counts.sourceErrors++
    Add-ContestPending $Context $Mapping.key $Mapping.pendingReason $Mapping.eventId
}

function Invoke-ContestEntry {
    param($Context, $Entry)
    if (Stop-ContestExcludedEntry $Context $Entry) { return }
    $sourceFailures = @($Context.SourceResults | Where-Object {
        (Test-ContestSourceError $_) -and (Test-ContestValueEqual (Get-ContestProperty $_ 'source') $Entry.source)
    })
    if ($sourceFailures.Count -gt 0) { Add-ContestPending $Context $Entry.key 'Source adapter did not complete successfully; existing data is preserved.'; return }
    $matchResult = Find-ContestMappings $Context $Entry
    $found = $matchResult.Rows
    if ($found.Count -gt 1 -or $matchResult.Possible.Count -gt 0) { Add-ContestPending $Context $Entry.key 'Ambiguous state identity/stage or conflicting official URLs.'; return }
    $mapping = if ($found.Count -eq 1) { $found[0] } else { $null }
    if ($null -ne $mapping -and (Stop-ContestExcludedEntry $Context $mapping $mapping.eventId)) { return }
    if ($null -eq $mapping) {
        $legacy = Find-ContestLegacy $Context $Entry
        if ($legacy.Matches.Count -gt 1 -or $legacy.Possible.Count -gt 0) { Add-ContestPending $Context $Entry.key 'Ambiguous legacy match; existing dates and text are preserved.'; return }
        if ($legacy.Matches.Count -eq 1) {
            $mapping = New-ContestMapping $Entry $legacy.Matches[0] $false
            [void]$Context.Mappings.Add($mapping)
        } else {
            $deletedLegacy = Find-ContestLegacy $Context $Entry $Context.Deleted
            if ($deletedLegacy.Matches.Count -gt 1 -or $deletedLegacy.Possible.Count -gt 0) {
                Add-ContestPending $Context $Entry.key 'Possible user-deleted legacy contest; no new event is permitted.'
                return
            }
            if ($deletedLegacy.Matches.Count -eq 1) {
                $mapping = New-ContestMapping $Entry $deletedLegacy.Matches[0] $false
                [void]$Context.Mappings.Add($mapping)
                Set-ContestSuppressed $Context $mapping 'User-deleted legacy event; read-only suppression, never restore or recreate.'
                return
            }
            # Deterministic request_id also finds a completed create after sidecar loss.
            $requestId = New-ContestRequestId ([string]$Entry.key)
            $orphans = @($Context.Active | Where-Object { Test-ContestValueEqual (Get-ContestProperty $_ 'request_id') $requestId })
            if ($orphans.Count -gt 1) { Add-ContestPending $Context $Entry.key 'Multiple events share the request UUID.'; return }
            if ($orphans.Count -eq 1) {
                # Ownership baseline was lost: read-only adoption is safer than claiming fields.
                $mapping = New-ContestMapping $Entry $orphans[0] $false
                [void]$Context.Mappings.Add($mapping)
                $mapping.pendingReason = 'Recovered request UUID without ownership state; read-only adoption.'
            } else {
                $mapping = New-ContestMapping $Entry $null $true
                if (Test-ContestDeletedMapping $Context $mapping) {
                    [void]$Context.Mappings.Add($mapping)
                    Set-ContestSuppressed $Context $mapping 'Deleted request UUID found in archive; creation is prohibited.'
                    return
                }
                if (Test-ContestNewExpired $Context $Entry) {
                    $Context.Report.counts.excluded++
                    Add-ContestOperation $Context 'excluded' $Entry.key (Get-ContestNewExclusion $Context $Entry)
                    return
                }
                if ($Context.Mode -eq 'Preview') { Add-ContestOperation $Context 'would_create' $Entry.key 'Verified new candidate; Preview makes no writes.'; return }
                [void]$Context.Mappings.Add($mapping)
                $payload = New-ContestCreatePayload $Entry $mapping.requestId
                $mapping.intent = [pscustomobject]@{ kind = 'create'; requestId = $mapping.requestId; payload = $payload; desired = Get-ContestProjection $Entry; before = $null }
                Save-ContestState $Context
                Invoke-ContestWriteIntent $Context $mapping $Entry
                return
            }
        }
    }
    if ($Entry.key -notin @($mapping.aliases)) { $mapping.aliases = @($mapping.aliases) + [string]$Entry.key }
    $mapping.lastSeenAt = $Context.CollectedAt
    if ($mapping.status -eq 'suppressed') { Add-ContestPending $Context $Entry.key 'Previously suppressed/user-deleted event; no recreation.' $mapping.eventId; return }
    $event = (Get-ContestMappedEvent $Context $mapping).Event
    if ($null -eq $event) {
        if ($mapping.eventId -or (Test-ContestDeletedMapping $Context $mapping)) {
            Set-ContestSuppressed $Context $mapping 'Mapped event is absent/deleted; no recreation or restore.'
            return
        }
        if ($null -eq $mapping.intent -or $mapping.intent.kind -ne 'create') { Add-ContestPending $Context $Entry.key 'Ownership or create intent is missing; no write.'; return }
        if (Test-ContestNewExpired $Context $Entry) {
            $Context.Report.counts.excluded++; Add-ContestOperation $Context 'excluded' $Entry.key 'Expired pending create will not be retried.'; return
        }
        if (-not (Test-ContestProjectionEqual $mapping.intent.desired (Get-ContestProjection $Entry))) {
            Add-ContestPending $Context $Entry.key 'Pending-create payload changed; retain the same UUID and original intent for review.'
            return
        }
        if ($Context.Mode -eq 'Preview') { Add-ContestOperation $Context 'would_retry_create' $Entry.key 'Same UUID/payload only; Preview makes no writes.'; return }
        Save-ContestState $Context
        Invoke-ContestWriteIntent $Context $mapping $Entry
        return
    }
    if (-not $mapping.owned) {
        $Context.Report.counts.existing++
        if (-not (Test-ContestProjectionEqual (Get-ContestProjection $event) (Get-ContestProjection $Entry)) -or $mapping.pendingReason) {
            Add-ContestPending $Context $Entry.key 'Legacy/read-only adopted event; user fields cannot be overwritten.' (Get-ContestProperty $event 'id')
        } else { Add-ContestOperation $Context 'existing' $Entry.key 'Read-only legacy adoption; no calendar write.' (Get-ContestProperty $event 'id') }
        Save-ContestState $Context
        return
    }
    if ($null -ne $mapping.intent) {
        if (Test-ContestProjectionEqual (Get-ContestProjection $event) $mapping.intent.desired) {
            Complete-ContestIntent $Context $mapping $event $mapping.intent.desired
            Add-ContestOperation $Context 'recovered' $Entry.key 'Previous durable write confirmed; ownership baseline recovered.' $mapping.eventId
        } elseif ($mapping.intent.kind -eq 'create') {
            $mapping.lastWritten = Get-ContestProjection $mapping.intent.desired
            $mapping.intent = $null
        } elseif (-not (Test-ContestProjectionEqual (Get-ContestProjection $event) (Get-ContestProjection $mapping.intent.before))) {
            $mapping.intent = $null
            $mapping.pendingReason = 'Event changed during pending update; preserve all user changes.'
        } else {
            # Re-plan below against a fresh full Event; never reuse stale expected.
            $mapping.intent = $null
        }
    }
    if ($null -eq $mapping.lastWritten) { Add-ContestPending $Context $Entry.key 'Ownership baseline missing; no update.' $mapping.eventId; return }
    $desired = Get-ContestProjection $event
    $changed = [Collections.Generic.List[string]]::new()
    $held = [Collections.Generic.List[string]]::new()
    foreach ($field in $script:ContestAutoFields) {
        $currentValue = Get-ContestProperty $event $field
        if ($field -notin @($mapping.protectedFields) -and -not (Test-ContestValueEqual $currentValue (Get-ContestProperty $mapping.lastWritten $field))) {
            $mapping.protectedFields = @($mapping.protectedFields) + $field
        }
        if ($field -in @($mapping.protectedFields)) { [void]$held.Add($field); continue }
        $incoming = Get-ContestProperty $Entry $field
        if ($field -eq 'time' -and $null -eq $incoming -and $null -ne $currentValue) { [void]$held.Add('time:unconfirmed'); continue }
        if (-not (Test-ContestValueEqual $currentValue $incoming)) { $desired.$field = $incoming; [void]$changed.Add($field) }
    }
    if ($held.Count -gt 0) { Add-ContestPending $Context $Entry.key 'Edited/protected or unconfirmed fields are preserved.' $mapping.eventId $held.ToArray() }
    if ($changed.Count -eq 0) {
        $Context.Report.counts.existing++
        Add-ContestOperation $Context 'existing' $Entry.key 'No eligible owned-field changes.' $mapping.eventId
        Save-ContestState $Context
        return
    }
    if ($Context.Mode -eq 'Preview') { Add-ContestOperation $Context 'would_update' $Entry.key 'Only unchanged owned fields are eligible; Preview makes no writes.' $mapping.eventId $changed.ToArray(); return }
    # Refresh immediately before CAS, then re-check fields instead of dropping expected on conflict.
    $Context.Active = (Get-ContestCalendarRows $Context 'calendar_list').Rows
    $fresh = (Get-ContestMappedEvent $Context $mapping).Event
    if ($null -eq $fresh) { Set-ContestSuppressed $Context $mapping 'Event disappeared before update; no recreation.'; return }
    if (-not (Test-ContestProjectionEqual (Get-ContestProjection $fresh) (Get-ContestProjection $event))) {
        Add-ContestPending $Context $Entry.key 'Calendar fields changed during planning; re-read on a later run, do not overwrite.' $mapping.eventId
        Save-ContestState $Context
        return
    }
    $payload = New-ContestUpdatePayload $fresh $desired
    $mapping.status = 'pending_update'
    $mapping.intent = [pscustomobject]@{ kind = 'update'; requestId = $mapping.requestId; payload = $payload; desired = $desired; before = Copy-ContestObject $fresh }
    Save-ContestState $Context
    Invoke-ContestWriteIntent $Context $mapping $Entry
}

function Review-ContestMissingMappings {
    param($Context, $Batch)
    foreach ($mapping in $Context.Mappings) {
        if ($mapping.status -eq 'suppressed') { continue }
        $present = @($Batch.entries | Where-Object { $_.key -eq $mapping.key -or $_.key -in @($mapping.aliases) })
        if ($present.Count -gt 0) { continue }
        $decisions = @(@($Batch.pending) + @($Batch.excluded) | Where-Object {
            $key = [string](Get-ContestProperty $_ 'key')
            $stageKeys = @((Get-ContestProperty $_ 'stages') | ForEach-Object { [string](Get-ContestProperty $_ 'key') } | Where-Object { $_ })
            $stageAlias = @($stageKeys | Where-Object { $_ -eq $mapping.key -or $_ -in @($mapping.aliases) }).Count -gt 0
            ($key -and ($key -eq $mapping.key -or $key -in @($mapping.aliases))) -or
            $stageAlias -or
            ((Test-ContestValueEqual (Get-ContestProperty $_ 'source') $mapping.source) -and (Test-ContestValueEqual (Get-ContestProperty $_ 'sourceId') $mapping.sourceId))
        })
        if ($decisions.Count -gt 0) {
            $mapping.pendingReason = 'Source candidate is excluded or needs review; existing calendar data is preserved.'
            Add-ContestPending $Context $mapping.key $mapping.pendingReason $mapping.eventId
            continue
        }
        $completeSource = @($Context.SourceResults | Where-Object {
            (Test-ContestValueEqual (Get-ContestProperty $_ 'source') $mapping.source) -and
            ([string](Get-ContestProperty $_ 'status')).ToLowerInvariant() -in @('ok','success','complete','verified') -and
            (Get-ContestProperty $_ 'scopeComplete' $false) -eq $true
        })
        # Feed/pagination omissions are not evidence that an announcement was cancelled.
        if ($completeSource.Count -gt 0) {
            $mapping.pendingReason = 'Candidate absent from a confirmed complete source scan; review only, never delete.'
            Add-ContestPending $Context $mapping.key $mapping.pendingReason $mapping.eventId
        }
    }
}

function Assert-ContestBatch {
    param($Batch)
    if ((Get-ContestProperty $Batch 'schemaVersion') -ne 1) { throw 'Batch schemaVersion must be 1.' }
    foreach ($field in @('runId','collectedAt','entries','pending','excluded','sourceResults')) {
        if ($null -eq $Batch.PSObject.Properties[$field] -and -not ($Batch -is [Collections.IDictionary] -and $Batch.Contains($field))) { throw ('Batch is missing: ' + $field) }
    }
    $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($entry in @($Batch.entries)) {
        foreach ($field in @('key','identityKey','source','sourceId','sourceUrl','contestTitle','organizer','officialUrl','stage','date','time','title','notes','conditional')) {
            if ($null -eq $entry.PSObject.Properties[$field] -and -not ($entry -is [Collections.IDictionary] -and $entry.Contains($field))) { throw ('Entry is missing: ' + $field) }
        }
        if (-not $entry.key -or -not $entry.identityKey -or -not $entry.stage -or -not $seen.Add([string]$entry.key)) { throw 'Batch keys/stages must be nonempty and unique.' }
        [void][DateTime]::ParseExact([string]$entry.date, 'yyyy-MM-dd', [Globalization.CultureInfo]::InvariantCulture)
        if ($null -ne $entry.time -and $entry.time -notmatch '^([01][0-9]|2[0-3]):[0-5][0-9]$') { throw 'Entry time must be null or HH:MM.' }
        if ($entry.title -isnot [string] -or $entry.notes -isnot [string] -or [string]::IsNullOrWhiteSpace($entry.title) -or $entry.title.Length -gt 200 -or $entry.notes.Length -gt 4096) { throw 'Entry title/notes exceeds the calendar contract.' }
        if ($entry.conditional -isnot [bool]) { throw 'Entry conditional must be Boolean.' }
    }
}

function Invoke-ContestImport {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)] $Batch,
        [Parameter(Mandatory)] [string] $StateDirectory,
        [Parameter(Mandatory)] [scriptblock] $CalendarInvoker,
        [ValidateSet('Preview','Apply')] [string] $Mode = 'Preview',
        [DateTimeOffset] $Now = [DateTimeOffset]::UtcNow
    )
    Assert-ContestBatch $Batch
    if (-not [IO.Path]::IsPathRooted($StateDirectory)) { throw 'StateDirectory must be absolute.' }
    $directory = [IO.Path]::GetFullPath($StateDirectory)
    $collected = [DateTimeOffset]::Parse([string]$Batch.collectedAt, [Globalization.CultureInfo]::InvariantCulture)
    $report = [pscustomobject][ordered]@{
        schemaVersion = 1; owner = 'parent'; mode = $Mode; runId = [string]$Batch.runId; collectedAt = [string]$Batch.collectedAt
        status = 'complete'; counts = [pscustomobject][ordered]@{ created = 0; updated = 0; existing = 0; pending = @($Batch.pending).Count; excluded = @($Batch.excluded).Count; sourceErrors = 0 }
        operations = @()
    }
    $operations = [Collections.Generic.List[object]]::new()
    $lock = $null
    $context = $null
    try {
        if ($Mode -eq 'Apply') {
            [void][IO.Directory]::CreateDirectory($directory)
            try { $lock = [IO.FileStream]::new((Join-Path $directory 'collector.lock'), [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None) }
            catch { $report.status = 'partial'; $report.counts.pending += @($Batch.entries).Count; $report.operations = @([pscustomobject]@{ action = 'locked'; key = $null; eventId = $null; reason = 'Another collector owns the state lock; no calendar calls or writes.'; fields = @() }); return $report }
        }
        $statePath = Join-Path $directory 'state.json'
        # A malformed suppression file must fail before even calendar_list/calendar_deleted.
        $exclusions = (Read-ContestExclusions (Join-Path $directory 'exclusions.json')).Contests
        $state = Read-ContestState $statePath
        $mappings = [Collections.Generic.List[object]]::new()
        foreach ($mapping in @($state.entries)) { [void]$mappings.Add($mapping) }
        $context = [pscustomobject]@{
            Mode = $Mode; Invoker = $CalendarInvoker; Directory = $directory; StatePath = $statePath; State = $state; Mappings = $mappings
            Report = $report; Operations = $operations; Active = @(); Deleted = @(); CalendarOffline = $false; StateWriteFailed = $false
            CollectedAt = [string]$Batch.collectedAt; NowKst = $Now.ToOffset([TimeSpan]::FromHours(9)); SourceResults = @($Batch.sourceResults); Exclusions = $exclusions
        }
        foreach ($sourceResult in @($Batch.sourceResults)) {
            if (Test-ContestSourceError $sourceResult) { $report.counts.sourceErrors++ }
        }
        foreach ($item in @($Batch.pending)) { Add-ContestOperation $context 'pending_source' ([string](Get-ContestProperty $item 'key')) ([string](Get-ContestProperty $item 'reason' 'Source needs review.')) }
        foreach ($item in @($Batch.excluded)) { Add-ContestOperation $context 'excluded_source' ([string](Get-ContestProperty $item 'key')) ([string](Get-ContestProperty $item 'reason' 'Excluded by normalization.')) }
        try {
            $context.Active = (Get-ContestCalendarRows $context 'calendar_list').Rows
            $context.Deleted = (Get-ContestCalendarRows $context 'calendar_deleted').Rows
        } catch {
            $report.status = 'offline'; $report.counts.sourceErrors++; $report.counts.pending += @($Batch.entries).Count
            Add-ContestOperation $context 'calendar_error' '' $_.Exception.Message
            $context.CalendarOffline = $true
        }
        if (-not $context.CalendarOffline) {
            $index = 0
            foreach ($entry in @($Batch.entries)) {
                if ($context.CalendarOffline) { $report.counts.pending += @($Batch.entries).Count - $index; break }
                Invoke-ContestEntry $context $entry
                $index++
            }
            if (-not $context.CalendarOffline -and -not $context.StateWriteFailed) { Review-ContestMissingMappings $context $Batch }
            if (-not $context.StateWriteFailed) { Save-ContestState $context }
        }
        if ($context.CalendarOffline) { $report.status = if ($report.counts.created + $report.counts.updated -gt 0) { 'partial' } else { 'offline' } }
        elseif ($report.counts.pending -gt 0 -or $report.counts.sourceErrors -gt 0) { $report.status = 'partial' }
    } catch {
        if ($null -eq $context) { throw }
        $report.status = 'partial'; $report.counts.sourceErrors++
        Add-ContestOperation $context 'import_error' '' $_.Exception.Message
        # A failed sidecar commit must leave its preceding intent intact on disk.
        Add-ContestPending $context '' $(if ($context.StateWriteFailed) { 'Sidecar commit failed; no further calendar writes. Recover from the previously committed intent.' } else { 'Import stopped; previous calendar events are preserved.' })
    } finally {
        if ($null -ne $context) { $report.operations = @($operations.ToArray()) }
        if ($Mode -eq 'Apply' -and $null -ne $context -and -not $context.StateWriteFailed) {
            try {
                $tag = [regex]::Replace([string]$Batch.runId, '[^A-Za-z0-9_.-]', '_')
                if ($tag.Length -gt 80) { $tag = $tag.Substring(0,80) }
                $log = Join-Path $directory ('report-' + $tag + '-' + [Guid]::NewGuid().ToString('N') + '.json')
                [IO.File]::WriteAllText($log, ($report | ConvertTo-Json -Depth 30), [Text.UTF8Encoding]::new($false))
            } catch { $report.status = 'partial'; $report.counts.sourceErrors++; $report.operations = @($report.operations) + [pscustomobject]@{ action = 'report_error'; key = $null; eventId = $null; reason = $_.Exception.Message; fields = @() } }
        }
        if ($null -ne $lock) { $lock.Dispose() }
    }
    return $report
}

Export-ModuleMember -Function Invoke-ContestImport
