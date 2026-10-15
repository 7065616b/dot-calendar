param([string]$OutputDirectory=(Join-Path $PSScriptRoot ('..\qa-data\extract-tests-'+[guid]::NewGuid().ToString('N'))))
$ErrorActionPreference='Stop';Set-StrictMode -Version Latest
Import-Module (Join-Path $PSScriptRoot 'contest-collector\ContestExtract.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'contest-collector\ContestNormalize.psm1') -Force
[void][IO.Directory]::CreateDirectory($OutputDirectory)
$tests=[Collections.Generic.List[object]]::new()
function Assert($x,$message){if(-not $x){throw $message}}
function Test($name,$body){try{& $body;$tests.Add(@{name=$name;status='passed'})}catch{$tests.Add(@{name=$name;status='failed';error=$_.Exception.Message})}}
function Page([string]$Text,[string]$Target='대상 제한 없음',[string]$Close='2031-11-01',[switch]$NoQualification,[switch]$NoOrganizer){
 $prefix=$(if(-not $NoQualification){'<p>참가자격: 제한 없음</p>'})+$(if(-not $NoOrganizer){'<p>주최: 가상 연구소</p>'})
 $cache=@{'Activity:900001'=@{detailText=@{__ref='ActivityText:1'}};'ActivityText:1'=@{text=($prefix+$Text)}}
 $activity=@{id='900001';title='가상 AI 작품 경진대회';organizationName='가상 연구소';categories=@();targets=@(@{name=$Target});homepageURL='https://example.org/qa';recruitCloseAt=([datetimeoffset]($Close+'T14:59:59Z')).ToUnixTimeMilliseconds()}
 $embedded=@{props=@{pageProps=@{data=@{activityData=@{activity=$activity}};__APOLLO_STATE__=$cache}}}|ConvertTo-Json -Depth 15 -Compress
 [pscustomobject]@{source='linkareer';sourceId='900001';sourceUrl='https://linkareer.com/activity/900001';title='가상 AI 작품 경진대회';text='';html=('<script id="__NEXT_DATA__">'+$embedded+'</script>')}
}
function Extract($page){$c=ConvertFrom-ContestPages -Collection ([pscustomobject]@{sourceResults=@([pscustomobject]@{source='linkareer';status='ok'});pages=@($page)}) -Now ([datetimeoffset]'2031-10-01T00:00:00Z');return $c}
Test 'Explicit application date and absent time stay all-day' {
 $c=Extract (Page '<p>참가 신청 마감: 2031년 11월 1일</p>');$m=$c.candidates[0].milestones[0]
 Assert ($m.confidence -eq 'confirmed' -and $m.date -eq '2031-11-01' -and $null -eq $m.time) 'Date/time changed.'
}
Test 'Prohibited AI is excluded even when the title contains AI' {
 $c=Extract (Page '<p>AI 기능 사용 금지</p><p>참가 신청 마감: 2031.11.1</p>');$b=ConvertTo-ContestBatch $c
 Assert ($b.entries.Count -eq 0 -and $b.excluded[0].reason -eq 'ai_forbidden') 'Prohibition became permission.'
}
Test 'Qualifier on the line after a heading overrides broad portal eligibility' {
 $c=Extract (Page '<p>참가자격</p><p>특정 지역 거주 재학생만 참가 가능</p><p>참가 신청 마감: 2031.11.1</p>');$b=ConvertTo-ContestBatch $c
 Assert ($b.entries.Count -eq 0 -and $c.candidates[0].eligibility -eq 'unknown') 'Unverified qualification passed.'
}
Test 'Printed date conflicts remain pending' {
 $c=Extract (Page '<p>참가 신청 마감: 2031.11.2</p>');$b=ConvertTo-ContestBatch $c
 Assert ($b.entries.Count -eq 0 -and $c.candidates[0].milestones[0].conflict) 'Conflicting metadata was ignored.'
}
Test 'Anyone inside a selected cohort is not unrestricted eligibility' {
 $c=Extract (Page '<p>참가자격</p><p>LG Aimers 9기 교육생이라면 누구나 참가 가능</p><p>참가 신청 마감: 2031.11.1</p>');$b=ConvertTo-ContestBatch $c
 Assert ($b.entries.Count -eq 0 -and $c.candidates[0].eligibility -eq 'unknown') 'Conditional anyone became eligible.'
}
Test 'An age reference date is not an application deadline' {
 $c=Extract (Page '<p>일반 성인 : 참가 신청일(2031.11.01) 기준 만 19세 이상</p>')
 Assert ($c.candidates[0].milestones.Count -eq 0) 'Age reference date became a deadline.'
}
Test 'Same-line restricted qualification overrides broad portal tags' {
 $c=Extract (Page '<p>참가자격 교육생만 참가 가능</p><p>참가 신청 마감: 2031.11.1</p>');$b=ConvertTo-ContestBatch $c
 Assert ($b.entries.Count -eq 0) 'Unrecognized inline condition ignored.'
}
Test 'A paragraph mentioning an application is not a schedule heading' {
 $c=Extract (Page '<p>참가 자격: 제한 없음</p><p>자세한 안내에서 참가 신청 마감: 2031.11.1을 확인하세요</p>')
 Assert ($c.candidates[0].milestones.Count -eq 0) 'Prose was promoted to confirmed schedule.'
}
Test 'No year is not inferred from now or metadata' {
 $c=Extract (Page '<p>참가 신청 마감: 11월 1일</p>');Assert ($c.candidates[0].milestones.Count -eq 0) 'Year inferred.'
}
Test 'Short range endpoint inherits only its printed year' {
 $d=Get-ContestDateEvidence '제출 기간: 2031.10.1 ~ 11.1 18:00'
 Assert ($d.start -eq '2031-10-01' -and $d.end -eq '2031-11-01' -and $d.time -eq '18:00') 'Explicit range lost.'
 Assert ($null -eq (Get-ContestDateEvidence '2031.12.1 ~ 1.1')) 'Cross-year endpoint invented.'
}
Test 'Korean clock time outside strict parser is held rather than erased' {
 $c=Extract (Page '<p>참가 신청 마감: 2031.11.1 16시까지</p>');$b=ConvertTo-ContestBatch $c
 Assert ($b.entries.Count -eq 0 -and $c.candidates[0].milestones[0].confidence -eq 'uncertain') 'Printed time became all-day.'
}
Test 'Reception-only prose does not guess application versus submission' {
 $c=Extract (Page '<p>접수 기간: 2031.10.1 ~ 11.1</p>');$b=ConvertTo-ContestBatch $c
 Assert ($b.entries.Count -eq 0) 'Ambiguous reception classified as an application.'
}
Test 'A period produces one reference and results do not imply selection' {
 $c=Extract (Page '<p>대회 기간: 2031.10.1 ~ 11.1</p><p>결과 발표: 2031.11.5</p>');$b=ConvertTo-ContestBatch $c
 Assert ($b.entries.Count -eq 2 -and $c.candidates[0].milestones[0].endDate -eq '2031-11-01' -and -not $c.candidates[0].milestones[1].conditional) 'Period split or wrong condition.'
}
Test 'Missing embedded schema remains reviewable, never a new candidate' {
 $p=Page '<p>참가 신청 마감: 2031.11.1</p>';$p.html='<html>Changed public schema</html>';$c=Extract $p;$b=ConvertTo-ContestBatch $c
 Assert ($b.entries.Count -eq 0 -and $b.pending.Count -gt 0) 'Unknown schema silently accepted/excluded.'
}
Test 'Broad portal tags alone cannot prove actual eligibility' {
 $c=Extract (Page '<p>참가 신청 마감: 2031.11.1</p>' -NoQualification);$b=ConvertTo-ContestBatch $c
 Assert ($b.entries.Count -eq 0 -and $c.candidates[0].eligibility -eq 'unknown') 'Portal target tag was treated as eligibility proof.'
}
Test 'Publishing agency is not silently presented as the organizer' {
 $c=Extract (Page '<p>참가 신청 마감: 2031.11.1</p>' -NoOrganizer);$b=ConvertTo-ContestBatch $c
 Assert ($b.entries.Count -eq 0 -and $c.candidates[0].reviewReason -match 'Organizer') 'Publisher was promoted to organizer.'
}
Test 'AM PM and subminute times are held without silent conversion' {
 foreach($clock in @('오후 5:00','5:00 PM','18:00:30')){
  $c=Extract (Page ('<p>참가 신청 마감: 2031.11.1 '+$clock+'</p>'));$b=ConvertTo-ContestBatch $c
  Assert ($b.entries.Count -eq 0) ('Unsupported time was accepted: '+$clock)
 }
}
Test 'Possible duplicate identities stay pending without merging or creating both' {
 $c=Extract (Page '<p>결과 발표: 2031.11.5</p>')
 $c.candidates[0].officialUrl=$null
 $copy=$c.candidates[0]|ConvertTo-Json -Depth 15|ConvertFrom-Json -Depth 15
 $copy.sourceId='900002';$copy.sourceUrl='https://linkareer.com/activity/900002';$copy.title='[게시기관] '+$copy.title
 $c.candidates+=@($copy);$b=ConvertTo-ContestBatch $c
 Assert ($b.entries.Count -eq 0 -and @($b.pending|Where-Object reason -eq 'possible_duplicate_identity').Count -eq 2) 'Potential duplicated contest was auto-created.'
}
Test 'An explicitly printed application URL is retained without inventing or fetching a URL' {
 $p=Page '<p>신청 및 제출: https://daker.ai/public/hackathons/qa-public-fixture</p><p>참가 신청 마감: 2031.11.1</p>'
 $p.html=$p.html.Replace('https://example.org/qa','')
 $c=Extract $p
 Assert ($c.candidates[0].officialUrl -ceq 'https://daker.ai/public/hackathons/qa-public-fixture') 'Printed application identity was lost.'
}
$tests|ConvertTo-Json -Depth 6|Set-Content -LiteralPath (Join-Path $OutputDirectory 'extraction-tests.json') -Encoding utf8
$tests|ForEach-Object {Write-Output ($_.status+': '+$_.name+$(if($_.ContainsKey('error')){' — '+$_.error}else{''}))}
if(@($tests|Where-Object status -eq failed).Count){throw 'Extraction regression failed.'}
