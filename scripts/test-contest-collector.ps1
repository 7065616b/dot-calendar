param([string]$OutputDirectory)
#Requires -Version 7.0
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$OutputEncoding=[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
$workspace=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if(-not $OutputDirectory){$OutputDirectory=Join-Path $workspace ('qa-data\collector-tests-'+[guid]::NewGuid().ToString('N'))}
$OutputDirectory=[IO.Path]::GetFullPath($OutputDirectory)
if(-not $OutputDirectory.StartsWith((Join-Path $workspace 'qa-data')+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Tests must stay in workspace qa-data.'}
[void][IO.Directory]::CreateDirectory($OutputDirectory)
Import-Module (Join-Path $PSScriptRoot 'contest-collector\ContestNormalize.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'contest-collector\ContestImport.psm1') -Force
$results=[Collections.Generic.List[object]]::new()
function Assert([bool]$Condition,[string]$Message){if(-not $Condition){throw $Message}}
function Clone($Object){return ($Object|ConvertTo-Json -Depth 40|ConvertFrom-Json -Depth 40)}
function Test([string]$Name,[scriptblock]$Body){try{& $Body;$results.Add(@{name=$Name;status='passed'})}catch{$results.Add(@{name=$Name;status='failed';error=$_.Exception.Message})}}
function Input-Batch([string]$Source='linkareer',[string]$SourceId='900001'){
 $url=if($Source -eq 'daker'){'https://daker.ai/public/hackathons/'+$SourceId}else{'https://linkareer.com/activity/'+$SourceId}
 return [pscustomobject]@{schemaVersion=1;runId=[guid]::NewGuid().ToString();collectedAt='2026-10-13T13:00:00Z';sourceResults=@([pscustomobject]@{source=$Source;status='ok';url=$url;error=$null});candidates=@([pscustomobject]@{source=$Source;sourceId=$SourceId;sourceUrl=$url;title='2031 AI 품질검증 공모전';organizer='QA 가상기관';officialUrl='https://example.org/contests/qa2031';status='open';eligibility='eligible';aiEvidence=[pscustomobject]@{kind='explicit';text='가상 fixture: AI 품질검증 공모전'};milestones=@([pscustomobject]@{stage='application_deadline';date='2031-11-01';time='18:00';endDate=$null;confidence='confirmed';conditional=$false;evidence='가상 fixture의 2031-11-01 18:00 신청 마감'})})}
}
function Normalize(${FixtureBatch}){return ConvertTo-ContestBatch -InputObject ${FixtureBatch} -Now ([datetimeoffset]'2026-10-13T13:00:00Z')}
function Mock-Calendar {
 $mock=[pscustomobject]@{events=[Collections.Generic.List[object]]::new();deleted=[Collections.Generic.List[object]]::new();creates=0;updates=0;loseCreate=$false;offline=$false;conflict=$false}
 $callback={param($Tool,$Arguments)
  if($mock.offline){throw 'offline fixture'}
  switch($Tool){
   'calendar_list'{return @(($mock.events.ToArray()|ConvertTo-Json -Depth 40|ConvertFrom-Json -Depth 40))}
   'calendar_deleted'{return @(($mock.deleted.ToArray()|ConvertTo-Json -Depth 40|ConvertFrom-Json -Depth 40))}
   'calendar_create'{
    $prior=@($mock.events|Where-Object request_id -eq $Arguments.request_id)
    if($prior.Count){return ($prior[0]|ConvertTo-Json -Depth 40|ConvertFrom-Json -Depth 40)}
    $hasTime=if($Arguments -is [Collections.IDictionary]){$Arguments.Contains('time')}else{$null -ne $Arguments.PSObject.Properties['time']}
    $time=if($hasTime){$Arguments.time}else{$null}
    $event=[pscustomobject]@{id='fixture-'+[guid]::NewGuid().ToString('N');request_id=$Arguments.request_id;date=$Arguments.date;time=$time;title=$Arguments.title;notes=$Arguments.notes;completed=$false;color=$null;recurrence=$null;reminder_minutes=$null}
    $mock.events.Add($event);$mock.creates++
    if($mock.loseCreate){$mock.loseCreate=$false;throw 'response lost after durable creation'}
    return ($event|ConvertTo-Json -Depth 40|ConvertFrom-Json -Depth 40)
   }
   'calendar_update'{
    $current=@($mock.events|Where-Object id -eq $Arguments.id)[0]
    if($mock.conflict){$mock.conflict=$false;$current.notes+=' concurrent user edit'}
    foreach($field in @('id','request_id','date','time','title','notes','completed','color','recurrence','reminder_minutes')){
     if(($current.$field|ConvertTo-Json -Compress) -cne ($Arguments.expected.$field|ConvertTo-Json -Compress)){throw 'expected conflict fixture'}
    }
    $current.date=$Arguments.date;$current.title=$Arguments.title;$current.notes=$Arguments.notes
    $hasTime=if($Arguments -is [Collections.IDictionary]){$Arguments.Contains('time')}else{$null -ne $Arguments.PSObject.Properties['time']}
    $current.time=if($hasTime){$Arguments.time}else{$null};$mock.updates++
    return ($current|ConvertTo-Json -Depth 40|ConvertFrom-Json -Depth 40)
   }
   default{throw 'An unsupported calendar operation was requested.'}
  }
 }.GetNewClosure()
 return [pscustomobject]@{data=$mock;invoke=$callback}
}
Test 'Preview leaves calendar and new sidecar untouched' {
 $mock=Mock-Calendar;$state=Join-Path $OutputDirectory 'preview';$batch=Normalize (Input-Batch)
 $null=Invoke-ContestImport -Batch $batch -StateDirectory $state -CalendarInvoker $mock.invoke -Mode Preview
 Assert ($mock.data.events.Count -eq 0 -and $mock.data.creates -eq 0) 'Preview wrote an event.'
 Assert (-not (Test-Path -LiteralPath $state)) 'Preview created state files.'
}
Test 'Repeated input and verified extension retain one event and ID' {
 $mock=Mock-Calendar;$state=Join-Path $OutputDirectory 'idempotent';${FixtureBatch}=Input-Batch;$batch=Normalize ${FixtureBatch}
 $null=Invoke-ContestImport -Batch $batch -StateDirectory $state -CalendarInvoker $mock.invoke -Mode Apply
 $id=$mock.data.events[0].id
 $null=Invoke-ContestImport -Batch $batch -StateDirectory $state -CalendarInvoker $mock.invoke -Mode Apply
 Assert ($mock.data.events.Count -eq 1 -and $mock.data.creates -eq 1) 'Retry duplicated creation.'
 ${FixtureBatch}.candidates[0].milestones[0].date='2031-11-08'
 $null=Invoke-ContestImport -Batch (Normalize ${FixtureBatch}) -StateDirectory $state -CalendarInvoker $mock.invoke -Mode Apply
 Assert ($mock.data.events.Count -eq 1 -and $mock.data.events[0].id -eq $id -and $mock.data.events[0].date -eq '2031-11-08') 'Extension did not retain the event ID.'
}
Test 'Cross-portal alias uses the same official URL and event' {
 $mock=Mock-Calendar;$state=Join-Path $OutputDirectory 'aliases'
 $null=Invoke-ContestImport -Batch (Normalize (Input-Batch)) -StateDirectory $state -CalendarInvoker $mock.invoke -Mode Apply
 $null=Invoke-ContestImport -Batch (Normalize (Input-Batch 'daker' 'qa-quality-2031')) -StateDirectory $state -CalendarInvoker $mock.invoke -Mode Apply
 Assert ($mock.data.events.Count -eq 1 -and $mock.data.creates -eq 1) 'Cross-portal identity duplicated an event.'
}
Test 'User title notes completion color and moved date survive collection' {
 $mock=Mock-Calendar;$state=Join-Path $OutputDirectory 'user-edit';${FixtureBatch}=Input-Batch
 $null=Invoke-ContestImport -Batch (Normalize ${FixtureBatch}) -StateDirectory $state -CalendarInvoker $mock.invoke -Mode Apply
 $event=$mock.data.events[0];$event.date='2031-11-03';$event.time='09:15';$event.title='내 일정 이름';$event.notes='내 메모';$event.completed=$true;$event.color='#AA33CC'
 $before=$event|ConvertTo-Json -Depth 20 -Compress
 ${FixtureBatch}.candidates[0].milestones[0].date='2031-11-08'
 $null=Invoke-ContestImport -Batch (Normalize ${FixtureBatch}) -StateDirectory $state -CalendarInvoker $mock.invoke -Mode Apply
 Assert (($mock.data.events[0]|ConvertTo-Json -Depth 20 -Compress) -ceq $before) 'User edits were overwritten.'
}
Test 'Lost creation response is reconciled without another UUID or event' {
 $mock=Mock-Calendar;$state=Join-Path $OutputDirectory 'lost';$mock.data.loseCreate=$true;$batch=Normalize (Input-Batch)
 try{$null=Invoke-ContestImport -Batch $batch -StateDirectory $state -CalendarInvoker $mock.invoke -Mode Apply}catch{}
 $null=Invoke-ContestImport -Batch $batch -StateDirectory $state -CalendarInvoker $mock.invoke -Mode Apply
 Assert ($mock.data.events.Count -eq 1 -and $mock.data.creates -eq 1) 'Response loss duplicated creation.'
}
Test 'User deletion is suppressed and never restored or recreated' {
 $mock=Mock-Calendar;$state=Join-Path $OutputDirectory 'deleted';$batch=Normalize (Input-Batch)
 $null=Invoke-ContestImport -Batch $batch -StateDirectory $state -CalendarInvoker $mock.invoke -Mode Apply
 $old=Clone $mock.data.events[0];$mock.data.deleted.Add($old);$mock.data.events.Clear()
 $null=Invoke-ContestImport -Batch $batch -StateDirectory $state -CalendarInvoker $mock.invoke -Mode Apply
 Assert ($mock.data.events.Count -eq 0 -and $mock.data.creates -eq 1) 'Deleted event was recreated.'
}
Test 'Concurrent user edit rejects guarded update and remains intact' {
 $mock=Mock-Calendar;$state=Join-Path $OutputDirectory 'conflict';${FixtureBatch}=Input-Batch
 $null=Invoke-ContestImport -Batch (Normalize ${FixtureBatch}) -StateDirectory $state -CalendarInvoker $mock.invoke -Mode Apply
 $mock.data.conflict=$true;${FixtureBatch}.candidates[0].milestones[0].date='2031-11-08'
 try{$null=Invoke-ContestImport -Batch (Normalize ${FixtureBatch}) -StateDirectory $state -CalendarInvoker $mock.invoke -Mode Apply}catch{}
 Assert ($mock.data.events[0].date -eq '2031-11-01' -and $mock.data.events[0].notes.EndsWith(' concurrent user edit')) 'Concurrent edit was overwritten.'
}
Test 'Offline calendar and corrupt sidecar never create an event' {
 $mock=Mock-Calendar;$mock.data.offline=$true;$state=Join-Path $OutputDirectory 'offline';$batch=Normalize (Input-Batch)
 try{$null=Invoke-ContestImport -Batch $batch -StateDirectory $state -CalendarInvoker $mock.invoke -Mode Apply}catch{}
 Assert ($mock.data.creates -eq 0) 'Offline calendar was reported as created.'
 $mock.data.offline=$false;$corrupt=Join-Path $OutputDirectory 'corrupt';[void][IO.Directory]::CreateDirectory($corrupt);[IO.File]::WriteAllText((Join-Path $corrupt 'state.json'),'{broken')
 try{$null=Invoke-ContestImport -Batch $batch -StateDirectory $corrupt -CalendarInvoker $mock.invoke -Mode Apply}catch{}
 Assert ($mock.data.creates -eq 0) 'Corrupt state was ignored before creation.'
}
Test 'Period is one conditional reference and midnight keeps original deadline' {
 ${FixtureBatch}=Input-Batch;$step=${FixtureBatch}.candidates[0].milestones[0];$step.stage='training';$step.date='2031-11-02';$step.time=$null;$step.endDate='2031-11-25'
 $batch=Normalize ${FixtureBatch}
 Assert (@($batch.entries).Count -eq 1 -and $batch.entries[0].conditional) 'Period expanded or lost its condition.'
 Assert ($batch.entries[0].notes -match '2031-11-25') 'End date missing from period reference.'
 ${FixtureBatch}=Input-Batch;${FixtureBatch}.candidates[0].milestones[0].time='24:00';$batch=Normalize ${FixtureBatch}
 Assert ($batch.entries[0].date -eq '2031-11-02' -and $batch.entries[0].time -eq '00:00' -and $batch.entries[0].title -match '11[/.-]0?1|11월\s*1') '24:00 was not converted while retaining the original date.'
}
Test 'Unknown eligibility context AI and uncertain dates stay pending' {
 foreach($type in @('eligibility','context','date')){
  ${FixtureBatch}=Input-Batch
  if($type -eq 'eligibility'){${FixtureBatch}.candidates[0].eligibility='unknown'}
  if($type -eq 'context'){${FixtureBatch}.candidates[0].aiEvidence.kind='context'}
  if($type -eq 'date'){${FixtureBatch}.candidates[0].milestones[0].confidence='uncertain'}
  $batch=Normalize ${FixtureBatch}
  Assert (@($batch.entries).Count -eq 0 -and @($batch.pending).Count -gt 0) "Uncertain $type was accepted automatically."
 }
}
Test 'Closed practice test ineligible and forbidden sources are excluded' {
 foreach($type in @('closed','practice','test','ineligible','forbidden')){
  ${FixtureBatch}=Input-Batch
  if($type -in @('closed','practice','test')){${FixtureBatch}.candidates[0].status=$type}
  if($type -eq 'ineligible'){${FixtureBatch}.candidates[0].eligibility='ineligible'}
  if($type -eq 'forbidden'){${FixtureBatch}.candidates[0].aiEvidence.kind='forbidden'}
  $batch=Normalize ${FixtureBatch}
  Assert (@($batch.entries).Count -eq 0) "Excluded $type became a new event."
 }
}
Test 'Exclusive runner lock prevents a second writer' {
 $mock=Mock-Calendar;$state=Join-Path $OutputDirectory 'locked';[void][IO.Directory]::CreateDirectory($state)
 $handle=[IO.File]::Open((Join-Path $state 'collector.lock'),[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
 try{try{$null=Invoke-ContestImport -Batch (Normalize (Input-Batch)) -StateDirectory $state -CalendarInvoker $mock.invoke -Mode Apply}catch{}}finally{$handle.Dispose()}
 Assert ($mock.data.creates -eq 0) 'A concurrent runner acquired the active lock.'
}
Test 'Republished notice with a publisher/year prefix cannot duplicate an existing user deadline' {
 $mock=Mock-Calendar;$state=Join-Path $OutputDirectory 'republished'
 $null=& $mock.invoke 'calendar_create' @{date='2031-11-01';time='18:00';title='AI 품질검증 공모전 참가 신청 마감';notes='User text; source https://linkareer.com/activity/800001';request_id='old-user-row'}
 $before=$mock.data.events[0]|ConvertTo-Json -Depth 10 -Compress
 $caseInput=Input-Batch;$caseInput.candidates[0].title='[다른 게시기관] 2031 AI 품질검증 공모전';$caseInput.candidates[0].officialUrl=$null
 $r=Invoke-ContestImport -Batch (Normalize $caseInput) -StateDirectory $state -CalendarInvoker $mock.invoke -Mode Apply
 Assert ($mock.data.creates -eq 1 -and $r.counts.pending -ge 1 -and ($mock.data.events[0]|ConvertTo-Json -Depth 10 -Compress) -ceq $before) 'Reposted existing deadline was duplicated or modified.'
}
Test 'A renamed-source match in deleted history cannot be recreated' {
 $mock=Mock-Calendar;$state=Join-Path $OutputDirectory 'republished-deleted'
 $null=& $mock.invoke 'calendar_create' @{date='2031-11-01';time='18:00';title='AI 품질검증 공모전 참가 신청 마감';notes='Previous source https://linkareer.com/activity/800001';request_id='deleted-user-row'}
 $mock.data.deleted.Add((Clone $mock.data.events[0]));$mock.data.events.Clear()
 $caseInput=Input-Batch;$caseInput.candidates[0].title='[새 게시기관] 2031 AI 품질검증 공모전';$caseInput.candidates[0].officialUrl=$null
 $r=Invoke-ContestImport -Batch (Normalize $caseInput) -StateDirectory $state -CalendarInvoker $mock.invoke -Mode Apply
 Assert ($mock.data.events.Count -eq 0 -and $mock.data.creates -eq 1 -and $r.counts.pending -ge 1) 'Deleted republished deadline was recreated.'
}
Test 'A different confirmed stage is not treated as the existing application deadline' {
 $mock=Mock-Calendar;$state=Join-Path $OutputDirectory 'different-stage'
 $null=& $mock.invoke 'calendar_create' @{date='2031-11-01';time='18:00';title='AI 품질검증 공모전 참가 신청 마감';notes='Previous source https://linkareer.com/activity/800001';request_id='existing-application'}
 $caseInput=Input-Batch;$caseInput.candidates[0].title='[게시기관] 2031 AI 품질검증 공모전';$caseInput.candidates[0].officialUrl=$null;$caseInput.candidates[0].milestones[0].stage='submission_deadline'
 $null=Invoke-ContestImport -Batch (Normalize $caseInput) -StateDirectory $state -CalendarInvoker $mock.invoke -Mode Apply
 Assert ($mock.data.creates -eq 2 -and $mock.data.events.Count -eq 2) 'Separate submission stage was incorrectly merged with application.'
}
Test 'An excluded contest remains protected after republication without a proven official URL' {
 $mock=Mock-Calendar;$state=Join-Path $OutputDirectory 'republished-exclusion';[void][IO.Directory]::CreateDirectory($state)
 $path=Join-Path $state 'exclusions.json'
 @{schemaVersion=1;owner='parent';contests=@(@{source='linkareer';sourceId='800001';sourceUrl='https://linkareer.com/activity/800001';officialUrl=$null;contestTitle='2031 AI 품질검증 공모전';edition='2031';reason='User excluded this fixture';recordedAt='2026-10-15T00:00:00Z';deletedEventIds=@()})}|ConvertTo-Json -Depth 10|Set-Content -LiteralPath $path -Encoding utf8
 $hash=(Get-FileHash -LiteralPath $path).Hash
 $caseInput=Input-Batch;$caseInput.candidates[0].title='[게시기관] 2031 AI 품질검증 공모전';$caseInput.candidates[0].officialUrl=$null
 $r=Invoke-ContestImport -Batch (Normalize $caseInput) -StateDirectory $state -CalendarInvoker $mock.invoke -Mode Apply
 Assert ($mock.data.creates -eq 0 -and $r.counts.pending -ge 1 -and (Get-FileHash -LiteralPath $path).Hash -ceq $hash) 'Republished exclusion was bypassed or rewritten.'
}
$failed=@($results|Where-Object status -eq 'failed')
$report=[pscustomobject]@{passed=$results.Count-$failed.Count;failed=$failed.Count;results=$results.ToArray();outputDirectory=$OutputDirectory;personalCalendarUsed=$false;guiUsed=$false}
$report|ConvertTo-Json -Depth 10|Set-Content -LiteralPath (Join-Path $OutputDirectory 'test-report.json') -Encoding utf8
$report|ConvertTo-Json -Depth 10
if($failed.Count){exit 1}
