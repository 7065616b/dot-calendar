Set-StrictMode -Version Latest

$script:ContestSourceHosts = @{
    linkareer = @('linkareer.com', 'www.linkareer.com')
    dacon = @('dacon.io', 'www.dacon.io')
    mokpo = @('mokpo.ac.kr', 'www.mokpo.ac.kr')
    daker = @('daker.ai', 'www.daker.ai')
    aifactory = @('aifactory.space', 'www.aifactory.space')
}

function Get-ContestField {
    param($Object, [string]$Name, $Default = $null)
    if ($null -eq $Object) { return ,$Default }
    if ($Object -is [System.Collections.IDictionary]) {
        if ($Object.Contains($Name)) { return ,$Object[$Name] }
        return ,$Default
    }
    $property = $Object.PSObject.Properties[$Name]
    if ($null -ne $property) { return ,$property.Value }
    return ,$Default
}

function Get-ContestText {
    param($Value)
    if ($Value -isnot [string]) { return '' }
    return $Value.Trim()
}

function Get-ContestHash {
    param([string]$Text)
    $hash = [System.Security.Cryptography.SHA256]::Create()
    try { return ([BitConverter]::ToString($hash.ComputeHash([Text.Encoding]::UTF8.GetBytes($Text)))).Replace('-', '').ToLowerInvariant() }
    finally { $hash.Dispose() }
}

function Limit-ContestText {
    param([string]$Text, [int]$Limit)
    if ($Text.Length -le $Limit) { return $Text }
    $length = $Limit - 1
    if ([char]::IsHighSurrogate($Text[$length - 1])) { $length-- }
    return $Text.Substring(0, $length) + [char]0x2026
}

function ConvertTo-ContestUrl {
    param($Value, [string]$Source = '', [switch]$Primary)
    $text = Get-ContestText $Value
    $uri = $null
    if (-not [Uri]::TryCreate($text, [UriKind]::Absolute, [ref]$uri)) { throw 'invalid_url' }
    if ($uri.Scheme -ne 'https' -or $uri.UserInfo -or $uri.IsLoopback) { throw 'unsafe_url' }
    $address = $null
    if ([Net.IPAddress]::TryParse($uri.DnsSafeHost, [ref]$address)) { throw 'ip_literal_url' }
    if ($uri.DnsSafeHost -notmatch '\.' -or $uri.DnsSafeHost.EndsWith('.localhost')) { throw 'local_url' }
    if ($Primary -and ($uri.DnsSafeHost.ToLowerInvariant() -notin $script:ContestSourceHosts[$Source])) { throw 'source_host_mismatch' }
    $decodedPath = [Uri]::UnescapeDataString($uri.AbsolutePath)
    if ($decodedPath -match '(?i)/(login|signin|sign-in|sign_in|auth|oauth|oauth2|authorize|authentication|myprofile|my-profile|account)([;/]|\.(php|jsp|aspx?|do|html?)($|/)|$)') { throw 'authentication_url' }
    $pairs = New-Object 'System.Collections.Generic.List[string]'
    foreach ($pair in $uri.Query.TrimStart('?').Split('&')) {
        if (-not $pair) { continue }
        $parts = $pair.Split(@('='), 2)
        $name = [Uri]::UnescapeDataString($parts[0].Replace('+', ' ')).ToLowerInvariant()
        if ($name -match '^utm_' -or $name -eq 'jsessionid') { continue }
        if ($name -match '^(access_token|refresh_token|id_token|token|auth|authorization|api_key|apikey|secret|password|passwd|jwt|session|sessionid|sid|credential|signature|sig)$' -or $name -match '^x-amz-') { throw 'credential_url' }
        [void]$pairs.Add($pair)
    }
    $builder = New-Object UriBuilder($uri)
    $builder.Host = $uri.DnsSafeHost.ToLowerInvariant()
    if ($builder.Host.StartsWith('www.')) { $builder.Host = $builder.Host.Substring(4) }
    $builder.Fragment = ''
    $builder.Path = [regex]::Replace($uri.AbsolutePath, '(?i);jsessionid=[^/;]*', '')
    if ($builder.Path.Length -gt 1) { $builder.Path = $builder.Path.TrimEnd('/') }
    $builder.Query = (@($pairs | Sort-Object) -join '&')
    return $builder.Uri.AbsoluteUri
}

function ConvertTo-ContestDate {
    param($Value)
    $text = Get-ContestText $Value
    if ($text -notmatch '^\d{4}-\d{2}-\d{2}$') { throw 'date_requires_explicit_year_month_day' }
    $date = [datetime]::MinValue
    if (-not [datetime]::TryParseExact($text, 'yyyy-MM-dd', [Globalization.CultureInfo]::InvariantCulture, [Globalization.DateTimeStyles]::None, [ref]$date)) { throw 'invalid_calendar_date' }
    return $date
}

function Get-ContestEdition {
    param($Candidate, [string]$Title)
    $explicit = Get-ContestText (Get-ContestField $Candidate 'edition')
    if ($explicit) {
        if ($explicit.Length -gt 80 -or $explicit -match '[\x00-\x1f]') { throw 'invalid_edition' }
        return [regex]::Replace($explicit, '\s+', ' ').ToLowerInvariant()
    }
    $year = [regex]::Match($Title, '(?<!\d)(20\d{2})(?!\d)')
    if ($year.Success) { return $year.Groups[1].Value }
    $round = [regex]::Match($Title, '제\s*(\d+)\s*회')
    if ($round.Success) { return $round.Groups[1].Value + '회' }
    return ''
}

function Get-ContestStageLabel {
    param([string]$Stage)
    switch ($Stage) {
        { $_ -in @('application', 'apply', 'registration', 'application_deadline', 'registration_deadline') } { return '참가 신청 마감' }
        { $_ -in @('submission', 'submission_deadline', 'proposal_submission_deadline', 'proposal_deadline') } { return '제출 마감' }
        { $_ -in @('final_presentation_pdf_deadline', 'finalist_presentation_pdf_deadline', 'presentation_submission_deadline') } { return '발표 자료 제출 마감' }
        'team_merge_deadline' { return '팀 병합 마감' }
        'leaderboard_submission_deadline' { return '리더보드 제출 마감' }
        { $_ -in @('orientation', 'ot') } { return 'OT' }
        { $_ -in @('training', 'education') } { return '교육' }
        'training_online' { return '온라인 교육' }
        'training_offline' { return '현장 교육' }
        'mentoring' { return '멘토링' }
        { $_ -in @('final', 'finals', 'final_presentation', 'final_competition') } { return '본선' }
        { $_ -in @('announcement', 'result', 'results', 'result_announcement', 'finalist_announcement', 'finalists_announcement', 'winner_announcement') } { return '결과 발표' }
        { $_ -in @('judging', 'review', 'preliminary_code_verification') } { return '심사 참고 일정' }
        'award_ceremony' { return '시상식' }
        'competition_end' { return '대회 종료' }
        'competition_period' { return '대회 기간' }
        'code_submission_deadline' { return '코드 제출 마감' }
        default { return $Stage.Replace('_', ' ') }
    }
}

function Test-ContestConditionalStage {
    param([string]$Stage)
    if ($Stage -match 'announcement|result|발표.*결과|결과.*발표|진출.*발표') { return $false }
    return ($Stage -match '(^|_)(orientation|ot|final|finals|training|education)(_|$)|본선|교육|오리엔테이션|시상식' -or $Stage -eq 'award_ceremony')
}

function Get-ContestEvidence {
    param($Evidence)
    if ($Evidence -is [string]) { return $Evidence.Trim() }
    return Get-ContestText (Get-ContestField $Evidence 'text')
}

function New-ContestDecision {
    param([string]$Reason, [string]$Source, [string]$SourceId, [string]$Title, [string]$Stage = '', [string]$Key = '')
    return [pscustomobject][ordered]@{ reason = $Reason; source = $Source; sourceId = $SourceId; contestTitle = $Title; stage = $Stage; key = $Key }
}

function ConvertTo-ContestBatch {
    [CmdletBinding()]
    param([Parameter(Mandatory = $true)]$InputObject, [datetimeoffset]$Now = [datetimeoffset]::UtcNow)
    if ($InputObject -is [string]) { $InputObject = $InputObject | ConvertFrom-Json }
    $version = Get-ContestField $InputObject 'schemaVersion'
    if ($version -is [bool] -or $version -is [string] -or $null -eq $version -or $version -ne 1) { throw 'schemaVersion must be numeric 1.' }
    $runId = Get-ContestText (Get-ContestField $InputObject 'runId')
    if (-not $runId -or $runId.Length -gt 128 -or $runId -match '[\x00-\x1f]') { throw 'runId must be a nonempty string without control characters.' }
    $collectedAtValue = Get-ContestField $InputObject 'collectedAt'
    $collectedAt = [datetimeoffset]::MinValue
    if ($collectedAtValue -is [datetimeoffset]) { $collectedAt = $collectedAtValue }
    elseif ($collectedAtValue -is [datetime]) {
        if ($collectedAtValue.Kind -eq [DateTimeKind]::Unspecified) { throw 'collectedAt DateTime must have UTC or Local kind.' }
        $collectedAt = [datetimeoffset]$collectedAtValue.ToUniversalTime()
    } else {
        $collectedAtText = Get-ContestText $collectedAtValue
        if ($collectedAtText -notmatch '^\d{4}-\d{2}-\d{2}T' -or $collectedAtText -notmatch '(Z|[+-]\d{2}:\d{2})$' -or -not [datetimeoffset]::TryParse($collectedAtText, [Globalization.CultureInfo]::InvariantCulture, [Globalization.DateTimeStyles]::None, [ref]$collectedAt)) { throw 'collectedAt must be an explicit ISO datetime with timezone.' }
    }
    $candidatesValue = Get-ContestField $InputObject 'candidates'
    if ($null -eq $candidatesValue -or $candidatesValue -is [string] -or $candidatesValue -is [System.Collections.IDictionary] -or $candidatesValue -isnot [System.Collections.IEnumerable]) { throw 'candidates must be an array.' }
    $sourceResultsValue = Get-ContestField $InputObject 'sourceResults'
    if ($null -eq $sourceResultsValue -or $sourceResultsValue -is [string] -or $sourceResultsValue -is [System.Collections.IDictionary] -or $sourceResultsValue -isnot [System.Collections.IEnumerable]) { throw 'sourceResults must be an array, preserving source errors.' }
    $entries = New-Object 'System.Collections.Generic.List[object]'
    $pending = New-Object 'System.Collections.Generic.List[object]'
    $excluded = New-Object 'System.Collections.Generic.List[object]'
    foreach ($candidate in @($candidatesValue)) {
        $source = (Get-ContestText (Get-ContestField $candidate 'source')).ToLowerInvariant()
        $sourceId = ([string](Get-ContestField $candidate 'sourceId')).Trim()
        $title = Get-ContestText (Get-ContestField $candidate 'title')
        $organizer = Get-ContestText (Get-ContestField $candidate 'organizer')
        try {
            if (-not $script:ContestSourceHosts.ContainsKey($source)) { throw 'unsupported_source' }
            if ($source -eq 'daker') { if ($sourceId -cnotmatch '^[a-z0-9]+(?:-[a-z0-9]+)*$' -or $sourceId.Length -gt 128) { throw 'invalid_source_id' } }
            elseif ($sourceId -notmatch '^\d{1,20}$') { throw 'invalid_source_id' }
            if (-not $title -or $title.Length -gt 200 -or -not $organizer -or $organizer.Length -gt 200) { throw 'missing_or_invalid_title_organizer' }
            $sourceUrl = ConvertTo-ContestUrl (Get-ContestField $candidate 'sourceUrl') -Source $source -Primary
            $officialUrl = $null
            $officialValue = Get-ContestField $candidate 'officialUrl'
            if ($null -ne $officialValue -and $officialValue -isnot [string]) { throw 'invalid_official_url_type' }
            if ($null -ne $officialValue -and (Get-ContestText $officialValue)) { $officialUrl = ConvertTo-ContestUrl $officialValue }
            $edition = Get-ContestEdition $candidate $title
        } catch {
            [void]$pending.Add((New-ContestDecision $_.Exception.Message $source $sourceId $title))
            continue
        }
        $status = (Get-ContestText (Get-ContestField $candidate 'status')).ToLowerInvariant()
        $eligibility = (Get-ContestText (Get-ContestField $candidate 'eligibility')).ToLowerInvariant()
        $ai = Get-ContestField $candidate 'aiEvidence'
        $aiKind = (Get-ContestText (Get-ContestField $ai 'kind')).ToLowerInvariant()
        $aiText = Get-ContestText (Get-ContestField $ai 'text')
        if ($status -in @('closed', 'cancelled', 'practice', 'test')) {
            $decision = New-ContestDecision ('status_' + $status) $source $sourceId $title
            if ($status -in @('closed', 'cancelled')) {
                # Preserve lookup provenance only. Closed/cancelled candidates never become writable entries.
                $stageLookup = New-Object 'System.Collections.Generic.List[object]'
                $stageSeen = New-Object 'System.Collections.Generic.HashSet[string]'
                $lookupMilestones = Get-ContestField $candidate 'milestones'
                foreach ($item in @($lookupMilestones)) {
                    $itemStage = (Get-ContestText (Get-ContestField $item 'stage')).ToLowerInvariant()
                    if (-not $itemStage -or $itemStage.Length -gt 80 -or $itemStage -match '[\x00-\x1f]' -or -not $stageSeen.Add($itemStage)) { continue }
                    $itemKey = '{0}:{1}:{2}' -f $source, $sourceId, $itemStage
                    $itemIdentity = $itemKey
                    if ($officialUrl) { $itemIdentity = 'official:' + (Get-ContestHash ($officialUrl + '|' + $edition)) + ':' + $itemStage }
                    [void]$stageLookup.Add([pscustomobject]@{ stage = $itemStage; key = $itemKey; identityKey = $itemIdentity })
                }
                $metadata = [ordered]@{ status = $status; sourceUrl = $sourceUrl; officialUrl = $officialUrl; organizer = $organizer; edition = if ($edition) { $edition } else { $null }; stages = @($stageLookup.ToArray()) }
                foreach ($name in $metadata.Keys) { Add-Member -InputObject $decision -NotePropertyName $name -NotePropertyValue $metadata[$name] }
            }
            [void]$excluded.Add($decision); continue
        }
        if ($eligibility -eq 'ineligible' -or $aiKind -in @('none', 'forbidden')) {
            $reason = if ($eligibility -eq 'ineligible') { 'ineligible' } else { 'ai_' + $aiKind }
            [void]$excluded.Add((New-ContestDecision $reason $source $sourceId $title)); continue
        }
        $hold = ''
        if ($status -ne 'open') { $hold = 'status_not_confirmed_open' }
        elseif ($eligibility -ne 'eligible') { $hold = 'eligibility_not_confirmed' }
        elseif ($aiKind -notin @('explicit', 'category') -or -not $aiText) { $hold = 'ai_evidence_not_explicit' }
        elseif ($aiKind -eq 'category' -and $aiText -notmatch '과학\s*[/·,및&]?\s*공학|과학|공학|(?i)science|engineering') { $hold = 'category_not_science_engineering' }
        elseif ($aiKind -eq 'category' -and $aiText -match '(?i)(과학|공학|science|engineering).{0,20}(제외|아님|아니|금지|불가|비해당|except|exclud|not\b)') { $hold = 'category_evidence_negated' }
        if ($hold) { [void]$pending.Add((New-ContestDecision $hold $source $sourceId $title)); continue }
        $milestones = Get-ContestField $candidate 'milestones'
        if ($null -eq $milestones -or $milestones -is [string] -or $milestones -is [System.Collections.IDictionary] -or $milestones -isnot [System.Collections.IEnumerable] -or @($milestones).Count -eq 0) {
            [void]$pending.Add((New-ContestDecision 'no_milestones' $source $sourceId $title)); continue
        }
        foreach ($milestone in @($milestones)) {
            $stage = (Get-ContestText (Get-ContestField $milestone 'stage')).ToLowerInvariant()
            $key = ''
            try {
                if (-not $stage -or $stage.Length -gt 80 -or $stage -match '[\x00-\x1f]') { throw 'invalid_stage' }
                $key = '{0}:{1}:{2}' -f $source, $sourceId, $stage
                $identityKey = $key
                if ($officialUrl) { $identityKey = 'official:' + (Get-ContestHash ($officialUrl + '|' + $edition)) + ':' + $stage }
                $confidence = (Get-ContestText (Get-ContestField $milestone 'confidence')).ToLowerInvariant()
                $evidenceValue = Get-ContestField $milestone 'evidence'
                $evidence = Get-ContestEvidence $evidenceValue
                $conflict = Get-ContestField $milestone 'conflict' $false
                $evidenceConflict = Get-ContestField $evidenceValue 'conflict' $false
                $candidateConflicts = Get-ContestField $candidate 'conflicts'
                $affectedConflict = $false
                foreach ($item in @($candidateConflicts)) {
                    if ($null -eq $item) { continue }
                    $itemStage = Get-ContestText (Get-ContestField $item 'stage')
                    $itemField = Get-ContestText (Get-ContestField $item 'field')
                    $itemDate = Get-ContestText (Get-ContestField $item 'date')
                    if ($itemStage) { if ($itemStage.ToLowerInvariant() -eq $stage) { $affectedConflict = $true }; continue }
                    if ($itemField.Contains('.')) { if ($itemField.Split('.')[0].ToLowerInvariant() -eq $stage) { $affectedConflict = $true }; continue }
                    if ($itemDate) { if ($itemDate -eq (Get-ContestText (Get-ContestField $milestone 'date'))) { $affectedConflict = $true }; continue }
                    $affectedConflict = $true
                }
                if ($confidence -ne 'confirmed') { throw 'date_confidence_not_confirmed' }
                if ($conflict -eq $true -or $evidenceConflict -eq $true -or $affectedConflict) { throw 'conflicting_source_evidence' }
                if (-not $evidence) { throw 'missing_date_evidence' }
                $date = ConvertTo-ContestDate (Get-ContestField $milestone 'date')
                $originalDate = $date.ToString('yyyy-MM-dd')
                $time = Get-ContestField $milestone 'time'
                if ($null -ne $time -and $time -isnot [string]) { throw 'invalid_time_type' }
                if ($time -is [string]) { $time = $time.Trim(); if (-not $time) { $time = $null } }
                if ($null -ne $time -and $time -notmatch '^(?:[01]\d|2[0-3]):[0-5]\d$|^24:00$') { throw 'invalid_time' }
                $endDateText = Get-ContestField $milestone 'endDate'
                $endDate = $null
                if ($null -ne $endDateText -and $endDateText -isnot [string]) { throw 'invalid_end_date_type' }
                if ($null -ne $endDateText -and (Get-ContestText $endDateText)) {
                    $endDate = ConvertTo-ContestDate $endDateText
                    if ($endDate -lt $date) { throw 'invalid_date_range' }
                }
                $rollover = $time -eq '24:00'
                if ($rollover) {
                    try { $date = $date.AddDays(1) } catch { throw '24_hour_rollover_out_of_range' }
                    $time = '00:00'
                    if ($null -ne $endDate -and $endDate -lt $date) { throw 'ambiguous_24_hour_range' }
                }
                $conditionalValue = Get-ContestField $milestone 'conditional' $false
                if ($conditionalValue -isnot [bool]) { throw 'invalid_conditional_type' }
                $conditional = $conditionalValue -or (Test-ContestConditionalStage $stage)
                $label = Get-ContestStageLabel $stage
                if ($conditional) { $label = '선정 시 ' + $label }
                $titleAffix = ' · ' + $label
                if ($null -ne $endDate) { $titleAffix += ' (기간 참고)' }
                if ($rollover) { $titleAffix += ' (' + $originalDate + ' 24:00 마감)' }
                # Keep condition, range and the original midnight deadline visible even with a long contest name.
                $entryTitle = (Limit-ContestText $title (200 - $titleAffix.Length)) + $titleAffix
                $noteLines = New-Object 'System.Collections.Generic.List[string]'
                [void]$noteLines.Add('공모전: ' + $title)
                [void]$noteLines.Add('주최: ' + $organizer)
                [void]$noteLines.Add('공개 출처: ' + $sourceUrl)
                if ($officialUrl) { [void]$noteLines.Add('공식 신청·접수 정보: ' + $officialUrl) }
                if ($edition) { [void]$noteLines.Add('명시 연도·회차: ' + $edition) }
                [void]$noteLines.Add('단계: ' + $label)
                [void]$noteLines.Add('날짜 근거: ' + (Limit-ContestText $evidence 1800))
                [void]$noteLines.Add('AI 근거: ' + (Limit-ContestText $aiText 500))
                if ($conditional) { [void]$noteLines.Add('선정·진출 조건이 있는 일정이며 사용자 선정 여부는 미확인.') }
                if ($null -eq $time) { [void]$noteLines.Add('공고 시각: 미공개(종일).') }
                if ($rollover) { [void]$noteLines.Add('원문 마감: ' + $originalDate + ' 24:00 (Asia/Seoul). 실제 시각: ' + $date.ToString('yyyy-MM-dd') + ' 00:00.') }
                if ($null -ne $endDate) { [void]$noteLines.Add('전체 기간: ' + $originalDate + '~' + $endDate.ToString('yyyy-MM-dd') + '. 기간 안내용 참고 일정.') }
                [void]$entries.Add([pscustomobject][ordered]@{
                    key = $key; identityKey = $identityKey; source = $source; sourceId = $sourceId; sourceUrl = $sourceUrl
                    contestTitle = $title; organizer = $organizer; officialUrl = $officialUrl; edition = if ($edition) { $edition } else { $null }
                    stage = $stage; date = $date.ToString('yyyy-MM-dd'); time = $time
                    title = (Limit-ContestText $entryTitle 200); notes = (Limit-ContestText ($noteLines -join "`n") 4096); conditional = [bool]$conditional
                })
            } catch { [void]$pending.Add((New-ContestDecision $_.Exception.Message $source $sourceId $title $stage $key)) }
        }
    }
    # A repeated key/alias with different facts is a conflict, not a second deadline.
    $removed = New-Object 'System.Collections.Generic.HashSet[string]'
    foreach ($field in @('key', 'identityKey')) {
        foreach ($group in @($entries | Group-Object -Property $field)) {
            if ($group.Count -lt 2) { continue }
            $facts = @($group.Group | ForEach-Object { '{0}|{1}|{2}|{3}' -f $_.date, $_.time, $_.conditional, $_.notes } | Select-Object -Unique)
            # Notes vary between publishers. Dates, times and the explicit condition decide alias agreement.
            if ($field -eq 'identityKey') { $facts = @($group.Group | ForEach-Object { '{0}|{1}|{2}' -f $_.date, $_.time, $_.conditional } | Select-Object -Unique) }
            if ($facts.Count -gt 1) {
                foreach ($entry in $group.Group) {
                    if ($removed.Add($entry.key)) { [void]$pending.Add((New-ContestDecision 'duplicate_stage_conflict' $entry.source $entry.sourceId $entry.contestTitle $entry.stage $entry.key)) }
                }
            }
        }
    }
    $seen = New-Object 'System.Collections.Generic.HashSet[string]'
    # Similar duplicate postings without a shared official identity are held,
    # never silently merged or emitted as two copies of the same deadline.
    foreach($group in @($entries|Group-Object -Property {
        (($_.contestTitle -replace '^\s*\[[^\]]+\]\s*','' -replace '\s+','').ToLowerInvariant())+'|'+$_.edition+'|'+$_.stage
    })){
        $identities=@($group.Group|ForEach-Object identityKey|Select-Object -Unique)
        if($identities.Count -gt 1){foreach($entry in $group.Group){
            if($removed.Add($entry.key)){[void]$pending.Add((New-ContestDecision 'possible_duplicate_identity' $entry.source $entry.sourceId $entry.contestTitle $entry.stage $entry.key))}
        }}
    }
    $uniqueEntries = New-Object 'System.Collections.Generic.List[object]'
    foreach ($entry in $entries) {
        if ($removed.Contains($entry.key)) { continue }
        if (-not $seen.Add($entry.key)) { [void]$excluded.Add((New-ContestDecision 'duplicate_identical_stage' $entry.source $entry.sourceId $entry.contestTitle $entry.stage $entry.key)); continue }
        [void]$uniqueEntries.Add($entry)
    }
    return [pscustomobject][ordered]@{
        schemaVersion = 1; runId = $runId; collectedAt = $collectedAt.ToString('o'); evaluatedAt = $Now.ToString('o')
        entries = @($uniqueEntries.ToArray()); pending = @($pending.ToArray()); excluded = @($excluded.ToArray()); sourceResults = @($sourceResultsValue)
    }
}

Export-ModuleMember -Function ConvertTo-ContestBatch
