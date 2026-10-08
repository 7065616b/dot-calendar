param([string] $ExePath = (Join-Path $PSScriptRoot '..\..\..\dist\dot-calendar.exe'))

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$helper = Join-Path $PSScriptRoot 'invoke-calendar.ps1'
$dataDir = Join-Path $PSScriptRoot ('..\..\..\qa-data\skill-test-' + [guid]::NewGuid().ToString('N'))
$dataDir = [IO.Path]::GetFullPath($dataDir)
[void](New-Item -ItemType Directory -Path $dataDir -Force)

function Call-Calendar([string] $Tool, [hashtable] $Arguments) {
    $json = ConvertTo-Json -InputObject $Arguments -Compress -Depth 20
    return (& $helper -Tool $Tool -ArgumentsJson $json -ExePath $ExePath -DataDirectory $dataDir)
}

function Assert-True([bool] $Condition, [string] $Message) {
    if (-not $Condition) { throw $Message }
}

function Assert-Rejected([string] $Tool, [hashtable] $Arguments, [string] $ExpectedText) {
    $failure = ''
    try { [void](Call-Calendar $Tool $Arguments) }
    catch { $failure = $_.Exception.Message }
    Assert-True ($failure.Contains($ExpectedText)) "$Tool should reject this request with '$ExpectedText'; got '$failure'."
}

try {
    $date = '2099-12-01'
    $title = '한글 "따옴표" ''작업'' & | ; $() ` \ 끝'
    $notes = '줄1 & <태그> ; $HOME $(실행금지) ` "인용"'
    $requestId = 'skill-test-' + [guid]::NewGuid().ToString('N')
    $createArgs = @{ date = $date; time = '09:30'; title = $title; notes = $notes; request_id = $requestId; recurrence = 'weekly' }
    $created = Call-Calendar 'calendar_create' $createArgs | ConvertFrom-Json -AsHashtable
    Assert-True ($created.id -and $created.title -eq $title -and $created.notes -eq $notes) 'Create damaged Unicode or quoted arguments.'

    $listed = Call-Calendar 'calendar_list' @{ date = $date } | ConvertFrom-Json -AsHashtable -NoEnumerate
    Assert-True ($listed.Count -eq 1 -and $listed[0].id -eq $created.id) 'Created event was not readable.'

    $retried = Call-Calendar 'calendar_create' $createArgs | ConvertFrom-Json -AsHashtable
    Assert-True ($retried.id -eq $created.id) 'Request-id retry created a duplicate.'

    $today = (Get-Date).ToString('yyyy-MM-dd')
    $lastWeek = (Get-Date).AddDays(-7).ToString('yyyy-MM-dd')
    $once = Call-Calendar 'calendar_create' @{ date = $today; title = '오늘 단일 작업'; request_id = [guid]::NewGuid().ToString('N') } | ConvertFrom-Json -AsHashtable
    $weekly = Call-Calendar 'calendar_create' @{ date = $lastWeek; title = '오늘 반복 작업'; recurrence = 'weekly'; request_id = [guid]::NewGuid().ToString('N') } | ConvertFrom-Json -AsHashtable
    $finished = Call-Calendar 'calendar_create' @{ date = $today; title = '완료한 작업'; completed = $true; request_id = [guid]::NewGuid().ToString('N') } | ConvertFrom-Json -AsHashtable
    $googleId = 'google-' + ('a' * 64)
    $google = Call-Calendar 'calendar_create' @{ date = $today; title = '구글 가져온 작업'; request_id = $googleId } | ConvertFrom-Json -AsHashtable
    $agenda = Call-Calendar 'calendar_today' @{} | ConvertFrom-Json -AsHashtable
    Assert-True ($agenda.date -eq $today -and $agenda.events.Count -eq 3) 'Today agenda omitted a recurring or visible item, or included a completed item.'
    $onceRow = @($agenda.events | Where-Object { $_.event.id -eq $once.id })[0]
    $weeklyRow = @($agenda.events | Where-Object { $_.event.id -eq $weekly.id })[0]
    $googleRow = @($agenda.events | Where-Object { $_.event.id -eq $google.id })[0]
    Assert-True ($onceRow.source -eq 'local' -and $weeklyRow.event.date -eq $lastWeek -and $googleRow.source -eq 'google') 'Today agenda lost original snapshots or source tags.'
    $withFinished = Call-Calendar 'calendar_today' @{ include_completed = $true } | ConvertFrom-Json -AsHashtable
    Assert-True ($withFinished.events.Count -eq 4 -and @($withFinished.events | Where-Object { $_.event.id -eq $finished.id }).Count -eq 1) 'Completed agenda switch failed.'
    [IO.File]::WriteAllText((Join-Path $dataDir 'widget.json'), '{"show_google":false}', [Text.UTF8Encoding]::new($false))
    $hidden = Call-Calendar 'calendar_today' @{} | ConvertFrom-Json -AsHashtable
    Assert-True ($hidden.events.Count -eq 2 -and @($hidden.events | Where-Object { $_.source -eq 'google' }).Count -eq 0) 'Google visibility switch failed.'

    $newTitle = '수정 "따옴표" ''완료'' & | ; $() ` \ 끝'
    $updateFile = Join-Path $dataDir 'update-arguments.json'
    $updateJson = @{ id = $created.id; date = $date; title = $newTitle; notes = $notes } | ConvertTo-Json -Compress -Depth 20
    [IO.File]::WriteAllText($updateFile, $updateJson, [Text.UTF8Encoding]::new($false))
    $updated = & $helper -Tool calendar_update -ArgumentsFile $updateFile -ExePath $ExePath -DataDirectory $dataDir |
        ConvertFrom-Json -AsHashtable
    Assert-True ($updated.title -eq $newTitle -and $updated.notes -eq $notes) 'Update file damaged quoted arguments.'

    $deleted = Call-Calendar 'calendar_delete' @{ id = $created.id } | ConvertFrom-Json -AsHashtable
    Assert-True ($deleted.id -eq $created.id) 'Delete returned the wrong event.'
    $after = Call-Calendar 'calendar_list' @{ date = $date } | ConvertFrom-Json -AsHashtable -NoEnumerate
    Assert-True ($after.Count -eq 0) 'Deleted event remains readable.'
    $archived = Call-Calendar 'calendar_deleted' @{} | ConvertFrom-Json -AsHashtable -NoEnumerate
    Assert-True ($archived.Count -eq 1 -and $archived[0].id -eq $deleted.id -and $archived[0].notes -eq $notes) 'Deleted event was not durably archived with its notes.'
    Assert-True (Test-Path -LiteralPath (Join-Path $dataDir 'deleted') -PathType Container) 'Deleted-event archive directory is missing.'
    $restored = Call-Calendar 'calendar_restore' @{ id = $deleted.id } | ConvertFrom-Json -AsHashtable
    Assert-True ($restored.id -eq $deleted.id -and $restored.title -eq $newTitle -and $restored.notes -eq $notes -and $restored.request_id -eq $requestId -and $restored.recurrence -eq 'weekly' -and $null -eq $restored.time) 'Restore lost the original event ID or fields.'
    $afterRestore = Call-Calendar 'calendar_list' @{ date = $date } | ConvertFrom-Json -AsHashtable -NoEnumerate
    Assert-True ($afterRestore.Count -eq 1 -and $afterRestore[0].id -eq $created.id) 'Restored event did not reappear in a fresh process.'
    $archiveAfterRestore = Call-Calendar 'calendar_deleted' @{} | ConvertFrom-Json -AsHashtable -NoEnumerate
    Assert-True ($archiveAfterRestore.Count -eq 0) 'Restored event remains in recoverable deletion list.'

    $exactDate = '2099-12-15'
    $exact = Call-Calendar 'calendar_create' @{ date = $exactDate; title = '날짜 제목 바로 삭제'; notes = '복구할 상세 메모'; request_id = [guid]::NewGuid().ToString('N') } | ConvertFrom-Json -AsHashtable
    $exactDeleted = Call-Calendar 'calendar_delete' @{ date = $exactDate; title = '날짜 제목 바로 삭제' } | ConvertFrom-Json -AsHashtable
    Assert-True ($exactDeleted.id -eq $exact.id) 'Exact date/title selector deleted the wrong event.'
    $exactGone = Call-Calendar 'calendar_list' @{ date = $exactDate } | ConvertFrom-Json -AsHashtable -NoEnumerate
    Assert-True ($exactGone.Count -eq 0) 'Exact date/title deletion did not persist.'
    $exactRecovered = Call-Calendar 'calendar_restore' @{ id = $exact.id } | ConvertFrom-Json -AsHashtable
    Assert-True ($exactRecovered.id -eq $exact.id -and $exactRecovered.notes -eq '복구할 상세 메모' -and $exactRecovered.request_id -eq $exact.request_id) 'Exact-match deleted event did not restore faithfully.'

    $ambiguousDate = '2099-12-16'
    $first = Call-Calendar 'calendar_create' @{ date = $ambiguousDate; time = '09:00'; title = '중복 제목'; request_id = [guid]::NewGuid().ToString('N') } | ConvertFrom-Json -AsHashtable
    $second = Call-Calendar 'calendar_create' @{ date = $ambiguousDate; time = '10:00'; title = '중복 제목'; request_id = [guid]::NewGuid().ToString('N') } | ConvertFrom-Json -AsHashtable
    Assert-Rejected 'calendar_delete' @{ date = $ambiguousDate; title = '중복 제목' } 'multiple events match'
    $stillThere = Call-Calendar 'calendar_list' @{ date = $ambiguousDate } | ConvertFrom-Json -AsHashtable -NoEnumerate
    Assert-True ($stillThere.Count -eq 2) 'Ambiguous selector removed an event.'
    $timedDelete = Call-Calendar 'calendar_delete' @{ date = $ambiguousDate; title = '중복 제목'; time = '09:00' } | ConvertFrom-Json -AsHashtable
    Assert-True ($timedDelete.id -eq $first.id) 'Time disambiguation deleted the wrong event.'
    $oneLeft = Call-Calendar 'calendar_list' @{ date = $ambiguousDate } | ConvertFrom-Json -AsHashtable -NoEnumerate
    Assert-True ($oneLeft.Count -eq 1 -and $oneLeft[0].id -eq $second.id) 'Time disambiguation failed to preserve the other event.'

    $occurrenceDate = '2099-12-08'
    Assert-Rejected 'calendar_delete' @{ date = $occurrenceDate; title = $newTitle } 'requires series: true'
    $seriesStillThere = Call-Calendar 'calendar_list' @{ date = $date } | ConvertFrom-Json -AsHashtable -NoEnumerate
    Assert-True ($seriesStillThere.Count -eq 1 -and $seriesStillThere[0].id -eq $created.id) 'Recurring guard removed the series.'
    $seriesDeleted = Call-Calendar 'calendar_delete' @{ date = $occurrenceDate; title = $newTitle; series = $true } | ConvertFrom-Json -AsHashtable
    Assert-True ($seriesDeleted.id -eq $created.id -and $seriesDeleted.date -eq $date) 'Recurring selector did not delete the original series.'
    $seriesRestored = Call-Calendar 'calendar_restore' @{ id = $created.id } | ConvertFrom-Json -AsHashtable
    Assert-True ($seriesRestored.id -eq $created.id -and $seriesRestored.recurrence -eq 'weekly') 'Recurring series restore lost recurrence.'

    $staleDate = '2099-12-17'
    $original = Call-Calendar 'calendar_create' @{ date = $staleDate; title = '동시 편집 전'; request_id = [guid]::NewGuid().ToString('N') } | ConvertFrom-Json -AsHashtable
    $changed = Call-Calendar 'calendar_update' @{ id = $original.id; date = $staleDate; title = '동시 편집 후' } | ConvertFrom-Json -AsHashtable
    Assert-Rejected 'calendar_delete' @{ id = $original.id; expected = $original } 'event changed since it was read'
    Assert-Rejected 'calendar_delete' @{ id = $original.id; date = $staleDate; title = '동시 편집 후' } 'Unexpected field: date'
    $unchanged = Call-Calendar 'calendar_list' @{ date = $staleDate } | ConvertFrom-Json -AsHashtable -NoEnumerate
    Assert-True ($unchanged.Count -eq 1 -and $unchanged[0].id -eq $changed.id -and $unchanged[0].title -eq '동시 편집 후') 'Rejected stale or mixed-argument deletion changed data.'
    Write-Output 'PASS exact delete, ambiguity/time guards, series guard, stale snapshot, and durable undo'

    $rejected = $false
    try {
        [void](Call-Calendar 'calendar_create' @{ date = $date; title = 'missing request-id' })
    } catch {
        $rejected = $_.Exception.Message -like '*Missing required field: request_id*'
    }
    Assert-True $rejected 'MCP tool errors were not returned as failures.'
    Write-Output "PASS isolated MCP skill helper; fixture retained at $dataDir"
} catch {
    throw "Skill helper test failed; fixture retained at $dataDir. $($_.Exception.Message)"
}
