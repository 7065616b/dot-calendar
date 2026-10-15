param([string]$OutputDirectory = (Join-Path $PSScriptRoot ('..\qa-data\daily-tests-' + [guid]::NewGuid().ToString('N'))))
#Requires -Version 7.0
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Import-Module (Join-Path $PSScriptRoot 'contest-collector\ContestSchedule.psm1') -Force
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
[void][IO.Directory]::CreateDirectory($OutputDirectory)
$results = [Collections.Generic.List[object]]::new()
function Assert($Value,[string]$Message) { if(-not $Value){throw $Message} }
function Test([string]$Name,[scriptblock]$Body) {
    try { & $Body; $results.Add(@{name=$Name;status='passed'}) }
    catch { $results.Add(@{name=$Name;status='failed';error=$_.Exception.Message}) }
}
function StatePath([string]$Name) { Join-Path $OutputDirectory $Name }
$success = {param($source,$dir,$id) [pscustomobject]@{status='complete';freshSource=$true;error=$null}}
$failure = {param($source,$dir,$id) [pscustomobject]@{status='error';freshSource=$false;error='offline fixture'}}
$now = [datetimeoffset]'2031-10-14T23:30:00Z'
Test 'Korean date changes at UTC 15:00, independent of machine locale' {
    Assert ((Get-ContestKoreanDay ([datetimeoffset]'2031-10-14T14:59:59Z')) -eq '2031-10-14') 'Before boundary.'
    Assert ((Get-ContestKoreanDay ([datetimeoffset]'2031-10-14T15:00:00Z')) -eq '2031-10-15') 'After boundary.'
}
Test 'A completed day does not call the pipeline again after restart' {
    $p=StatePath 'once';$a=Invoke-ContestDailyRun $p $success -Sources linkareer -Now $now
    $b=Invoke-ContestDailyRun $p {throw 'Duplicate source call'} -Sources linkareer -Now $now.AddHours(3)
    Assert ($a.status -eq 'complete' -and $b.status -eq 'complete' -and $b.sourceResults.Count -eq 0) 'Repeated daily collection.'
    Assert ($b.lastSuccessKstDate -eq '2031-10-15') 'Success date wrong.'
}
Test 'Failure stays a failure, persisted backoff survives restart, recovery succeeds' {
    $p=StatePath 'retry';$a=Invoke-ContestDailyRun $p $failure -Sources dacon -Now $now
    $b=Invoke-ContestDailyRun $p {throw 'Retried too early'} -Sources dacon -Now $now.AddSeconds(299)
    $c=Invoke-ContestDailyRun $p $success -Sources dacon -Now $now.AddSeconds(300)
    Assert ($a.status -eq 'partial' -and $null -eq $a.lastSuccessKstDate) 'Failure marked success.'
    Assert ($b.sourceResults.Count -eq 0 -and $c.status -eq 'complete') 'Backoff or recovery failed.'
}
Test 'Only failed sources retry after partial success' {
    $p=StatePath 'partial';$pipe={param($s,$d,$id) if($s -eq 'dacon'){throw 'network fixture'};[pscustomobject]@{status='complete';freshSource=$true}}
    $a=Invoke-ContestDailyRun $p $pipe -Sources @('linkareer','dacon') -Now $now
    $b=Invoke-ContestDailyRun $p $success -Sources @('linkareer','dacon') -Now $now.AddMinutes(5)
    Assert ($a.status -eq 'partial' -and $b.sourceResults.Count -eq 1 -and $b.sourceResults[0].source -eq 'dacon') 'Successful source repeated.'
}
Test 'Old candidate replay cannot advance the success day' {
    $a=Invoke-ContestDailyRun (StatePath 'stale') {param($s,$d,$id) [pscustomobject]@{status='complete';freshSource=$false}} -Sources mokpo -Now $now
    Assert ($a.status -eq 'partial' -and $null -eq $a.lastSuccessKstDate) 'Stale data marked fresh.'
}
Test 'Maximum five attempts per Korean date, new day resets attempts' {
    $p=StatePath 'cap';$t=$now
    for($i=0;$i -lt 5;$i++){ $a=Invoke-ContestDailyRun $p $failure -Sources daker -Now $t; $t=$t.AddSeconds(@(300,900,3600,14400,14400)[$i]) }
    $b=Invoke-ContestDailyRun $p {throw 'Exceeded cap'} -Sources daker -Now $t
    Assert ($b.sourceResults.Count -eq 0) 'Retry cap failed.'
    $c=Invoke-ContestDailyRun $p $success -Sources daker -Now $now.AddDays(1)
    Assert ($c.status -eq 'complete' -and $c.sourceResults.Count -eq 1) 'Next day did not run.'
}
Test 'Concurrent daily lock prevents all collection and import calls' {
    $p=StatePath 'lock';[void][IO.Directory]::CreateDirectory($p)
    $lock=[IO.FileStream]::new((Join-Path $p 'daily.lock'),[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
    try{$a=Invoke-ContestDailyRun $p {throw 'Lock bypassed'} -Now $now;Assert ($a.status -eq 'locked') 'Lock bypassed.'}finally{$lock.Dispose()}
}
Test 'Corrupt daily state is preserved without executing pipeline' {
    $p=StatePath 'corrupt';[void][IO.Directory]::CreateDirectory($p);$f=Join-Path $p 'daily.json';[IO.File]::WriteAllText($f,'{invalid')
    $failed=$false;try{$null=Invoke-ContestDailyRun $p {throw 'Pipeline ran'} -Now $now}catch{$failed=$_.Exception.Message -like 'Daily state is unreadable*'}
    Assert ($failed -and [IO.File]::ReadAllText($f) -ceq '{invalid') 'Corruption reset or ignored.'
}
Test 'Backward clock does not recollect an already completed day' {
    $p=StatePath 'clock';$null=Invoke-ContestDailyRun $p $success -Sources aifactory -Now $now
    $a=Invoke-ContestDailyRun $p {throw 'Clock caused replay'} -Sources aifactory -Now $now.AddDays(-1)
    Assert ($a.sourceResults.Count -eq 0) 'Clock replay.'
}
Test 'Attempt intent is durable before source work begins' {
    $p=StatePath 'intent';$pipe={param($s,$d,$id) $state=Get-Content -LiteralPath (Join-Path $p 'daily.json') -Raw|ConvertFrom-Json;if(-not($state.sources.dacon.status -eq 'running' -and $state.sources.dacon.attempts -eq 1 -and $state.sources.dacon.nextAttemptAt)){throw 'Intent absent'};throw 'Crash fixture'}.GetNewClosure()
    $a=Invoke-ContestDailyRun $p $pipe -Sources dacon -Now $now
    Assert ($a.status -eq 'partial' -and $a.sourceResults[0].result.error -eq 'Crash fixture') 'Intent was not written first.'
}
Test 'Expired active task does not attempt or mark any source successful' {
    $p=StatePath 'deadline';$a=Invoke-ContestDailyRun $p {throw 'Expired task executed'} -Sources @('linkareer','dacon') -Now $now -CanContinue {$false}
    Assert ($a.status -eq 'partial' -and $a.sourceResults.Count -eq 0 -and $null -eq $a.lastSuccessKstDate) 'Deadline claimed successful work.'
}
Test 'Robots refusal is retained without repeated attempts on the same day' {
    $p=StatePath 'robots';$blocked={param($s,$d,$id) [pscustomobject]@{status='blocked';freshSource=$false;retryable=$false;error='robots.txt disallows'}}
    $a=Invoke-ContestDailyRun $p $blocked -Sources daker -Now $now
    $b=Invoke-ContestDailyRun $p {throw 'Refusal retried'} -Sources daker -Now $now.AddHours(1)
    Assert ($a.status -eq 'partial' -and $b.sourceResults.Count -eq 0 -and $null -eq $b.lastSuccessKstDate) 'Refusal was retried or marked successful.'
}
Test 'A reparse state directory is rejected before any pipeline call or file creation' {
    $target=StatePath 'reparse-target';[void][IO.Directory]::CreateDirectory($target)
    $link=StatePath 'reparse-state';$null=New-Item -ItemType Junction -Path $link -Target $target
    $caught=$false
    try{$null=Invoke-ContestDailyRun $link {throw 'Pipeline must not run'} -Sources dacon -Now $now}catch{$caught=$_.Exception.Message -match 'reparse'}
    Assert ($caught -and @(Get-ChildItem -LiteralPath $target -Force).Count -eq 0) 'Reparse state created files or ran pipeline.'
}
Test 'A reparse runs ancestor is rejected without creating a run in its target' {
    $p=StatePath 'reparse-runs';[void][IO.Directory]::CreateDirectory($p)
    $target=StatePath 'reparse-runs-target';[void][IO.Directory]::CreateDirectory($target)
    $null=New-Item -ItemType Junction -Path (Join-Path $p 'runs') -Target $target
    $caught=$false
    try{$null=Invoke-ContestDailyRun $p {throw 'Pipeline must not run'} -Sources dacon -Now $now}catch{$caught=$_.Exception.Message -match 'reparse'}
    Assert ($caught -and @(Get-ChildItem -LiteralPath $target -Force).Count -eq 0) 'Run was created through a reparse ancestor.'
}
$results | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'schedule-tests.json') -Encoding utf8
$results | ForEach-Object { Write-Output ($_.status + ': ' + $_.name + $(if($_.ContainsKey('error')){' — '+$_.error}else{''})) }
if(@($results|Where-Object status -eq failed).Count){throw 'Daily scheduling regression failed.'}
