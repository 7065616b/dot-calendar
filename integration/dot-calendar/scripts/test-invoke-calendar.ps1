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
