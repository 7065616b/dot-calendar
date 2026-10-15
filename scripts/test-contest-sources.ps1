param([string]$OutputDirectory=(Join-Path $PSScriptRoot ('..\qa-data\source-tests-'+[guid]::NewGuid().ToString('N'))))
#Requires -Version 7.0
$ErrorActionPreference='Stop';Set-StrictMode -Version Latest
Import-Module (Join-Path $PSScriptRoot 'contest-collector\ContestSources.psm1') -Force
[void][IO.Directory]::CreateDirectory($OutputDirectory)
$tests=[Collections.Generic.List[object]]::new()
function Assert($value,$message){if(-not $value){throw $message}}
function Test($name,$body){try{& $body;$tests.Add(@{name=$name;status='passed'})}catch{$tests.Add(@{name=$name;status='failed';error=$_.Exception.Message})}}
function DaconFixture($Blocked,$Missing){
 $calls=[Collections.Generic.List[string]]::new()
 $fetch={param($uri)
  $calls.Add($uri.AbsolutePath)
  switch($uri.AbsolutePath){
   '/robots.txt' { if($Blocked){return "User-agent: *`nDisallow: /competitions/official/999001/overview/rules"};return 'User-agent: *' }
   '/competitions' {return '<a href="/competitions/official/999001/overview/description">Fixture</a>'}
   '/competitions/official/999001/overview/description' {
    if($Missing){return '<h1>AI fixture</h1><p>주최: 가상 연구소</p>'}
    return '<h1>AI fixture</h1><p>주최: 가상 연구소</p><a href="/competitions/official/999001/overview/schedule">일정</a><a href="/competitions/official/999001/overview/rules">규칙</a>'
   }
   '/competitions/official/999001/overview/schedule' {return '<p>참가 신청 마감: 2031.11.01</p>'}
   '/competitions/official/999001/overview/rules' {return '<p>참가자격: 제한 없음</p>'}
   default {throw 'Unexpected fixture URL'}
  }
 }.GetNewClosure()
 @{calls=$calls;fetch=$fetch}
}
function LinkareerFixture([string]$Variant='valid'){
 $calls=[Collections.Generic.List[string]]::new()
 $fetch={param($uri)
  $calls.Add($uri.PathAndQuery)
  if($uri.AbsolutePath -eq '/robots.txt'){return 'User-agent: *'}
  if($uri.AbsolutePath -eq '/list/contest'){
   if($Variant -eq 'empty'){return '<html>Schema changed without links</html>'}
   if($Variant -eq 'unknown'){return '<a href="/activity/999999">Banner only</a>'}
   $isAi=$uri.Query.Contains('filterBy_q=AI')
   $filter=if($isAi){@{q='AI'}}else{@{categoryIDs=@('35')}}
   if($Variant -eq 'wrong-filter'){$filter=@{categoryIDs=@('99')}}
   $query=@{filterBy=$filter;orderBy=@{field='CREATED_AT';direction='DESC'};pagination=@{page=1}}|ConvertTo-Json -Depth 8 -Compress
   $ids=if($isAi){@('900002','900003')}else{@('900001','900002')}
   $items=@($ids|ForEach-Object {@{url=('/activity/'+$_)}})
   if($Variant -eq 'missing-link'){$items+=@{url='/activity/900004'}}
   $rootQuery=@{};$rootQuery['activities('+$query+')']=@{}
   $embedded=@{props=@{pageProps=@{activityItems=$items;__APOLLO_STATE__=@{ROOT_QUERY=$rootQuery}}}}|ConvertTo-Json -Depth 15 -Compress
   return '<a href="/activity/999999">Banner</a>'+(@($ids|ForEach-Object {'<a href="/activity/'+$_+'">Observed detail</a>'}) -join '')+'<script id="__NEXT_DATA__">'+$embedded+'</script>'
  }
  if($uri.AbsolutePath -match '^/activity/90000[1-3]$'){return '<h1>Public fixture</h1>'}
  throw ('Unexpected fixture URL '+$uri.PathAndQuery)
 }.GetNewClosure()
 @{calls=$calls;fetch=$fetch}
}
Test 'Observed DACON schedule and rules are retained with their original URLs' {
 $f=DaconFixture $false $false;$r=Get-ContestSourcePages -Sources dacon -LatestList -Fetcher $f.fetch
 Assert ($r.sourceResults[0].status -eq 'ok' -and $r.pages.Count -eq 1) 'Expected one complete detail.'
 Assert ($r.pages[0].evidencePages.Count -eq 3 -and $r.pages[0].text.Contains('2031.11.01') -and $r.pages[0].text.Contains('제한 없음')) 'Missing tab evidence.'
 Assert ($f.calls.Count -eq 5) 'Unexpected network-equivalent requests.'
}
Test 'A missing required DACON tab is incomplete, not zero success' {
 $f=DaconFixture $false $true;$r=Get-ContestSourcePages -Sources dacon -LatestList -Fetcher $f.fetch
 Assert ($r.sourceResults[0].status -eq 'partial') 'Missing tab accepted.'
}
Test 'Robots-disallowed DACON tab is never fetched' {
 $f=DaconFixture $true $false;$r=Get-ContestSourcePages -Sources dacon -LatestList -Fetcher $f.fetch
 Assert ($r.sourceResults[0].status -ne 'ok' -and -not $f.calls.Contains('/competitions/official/999001/overview/rules')) 'Blocked URL fetched or accepted.'
}
Test 'Expired deadline performs no request and does not report zero new contests' {
 $r=Get-ContestSourcePages -Sources dacon -LatestList -DeadlineUtc ([datetimeoffset]::UtcNow.AddSeconds(-1)) -Fetcher {throw 'Fetcher must not be called'}
 Assert ($r.sourceResults[0].status -eq 'error' -and $r.pages.Count -eq 0 -and $r.sourceResults[0].error -match 'budget') 'Expired deadline ignored.'
}
Test 'DAKER robots restriction is preserved without API or login calls' {
 $calls=[Collections.Generic.List[string]]::new()
 $fetch={param($uri) $calls.Add($uri.AbsolutePath);if($uri.AbsolutePath -eq '/robots.txt'){return "User-agent: *`nDisallow: /api/"};throw 'Disallowed request'}.GetNewClosure()
 $r=Get-ContestSourcePages -Sources daker -LatestList -Fetcher $fetch
 Assert ($r.sourceResults[0].status -eq 'blocked' -and $calls.Count -eq 1) 'DAKER restriction bypassed.'
}
Test 'Linkareer unions verified science and AI latest lists, deduplicating IDs and excluding banners' {
 $f=LinkareerFixture;$r=Get-ContestSourcePages -Sources linkareer -LatestList -Fetcher $f.fetch
 Assert ($r.sourceResults[0].status -eq 'ok' -and $r.pages.Count -eq 3) 'Verified list union incomplete.'
 Assert (@($r.pages|Where-Object sourceId -eq '900002').Count -eq 1 -and -not ($f.calls -match '/activity/999999')) 'Duplicate or advertising detail fetched.'
 Assert ($r.sourceResults[0].scopeComplete -eq $false -and $r.sourceResults[0].evidenceListings.Count -eq 2) 'First-page bounds were overstated.'
}
Test 'Linkareer requires server evidence of the requested filters and ordering' {
 $f=LinkareerFixture 'wrong-filter';$r=Get-ContestSourcePages -Sources linkareer -LatestList -Fetcher $f.fetch
 Assert ($r.sourceResults[0].status -ne 'ok' -and $r.pages.Count -eq 0) 'A different category was silently collected.'
}
Test 'Linkareer missing schema and unobserved detail URLs are errors, not successful zero counts' {
 foreach($variant in @('empty','unknown','missing-link')){
  $f=LinkareerFixture $variant;$r=Get-ContestSourcePages -Sources linkareer -LatestList -Fetcher $f.fetch
  Assert ($r.sourceResults[0].status -ne 'ok' -and $r.pages.Count -eq 0) ('Unexpected success for '+$variant)
 }
}
$tests|ConvertTo-Json -Depth 6|Set-Content -LiteralPath (Join-Path $OutputDirectory 'source-tests.json') -Encoding utf8
$tests|ForEach-Object {Write-Output ($_.status+': '+$_.name+$(if($_.ContainsKey('error')){' — '+$_.error}else{''}))}
if(@($tests|Where-Object status -eq failed).Count){throw 'Source regression failed.'}
