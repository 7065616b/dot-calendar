# Conservative, deterministic extraction. Public text is evidence, never code.
Set-StrictMode -Version Latest

function ConvertFrom-ContestMarkup([string]$Html) {
    $options=[Text.RegularExpressions.RegexOptions]::IgnoreCase -bor [Text.RegularExpressions.RegexOptions]::Singleline
    $timeout=[timespan]::FromSeconds(2)
    $s=[regex]::Replace($Html,'<(script|style|svg)\b[^>]*>.*?</\1\s*>','',$options,$timeout)
    $s=[regex]::Replace($s,'</?(?:p|div|br|li|h[1-6]|tr|section)\b[^>]*>',"`n",$options,$timeout)
    $s=[regex]::Replace($s,'<[^>]+>',' ',$options,$timeout)
    ([Net.WebUtility]::HtmlDecode($s) -replace '[\t \u00a0]+',' ' -replace ' *\r?\n *',"`n").Trim()
}

function Get-ContestStageFromLabel([string]$Label) {
    # Only a printed schedule heading, not a keyword inside eligibility/prose.
    $heading=($Label -replace '^\s*(?:[-•■●○□▶※]|[0-9]+[.)])\s*','').Trim()
    switch -Regex ($heading) {
        '^팀\s*병합\s*(?:마감|기간|일정)?\s*[:：\-\[(0-9]' { return 'team_merge_deadline' }
        '^코드(?:\s*(?:및|와|/|·)\s*설명서)?\s*(?:설명|검증|제출)(?:\s*(?:마감|기간|일정))?\s*[:：\-\[(0-9]' { return 'code_submission_deadline' }
        '^리더보드\s*(?:제출|마감)(?:\s*(?:마감|기간|일정))?\s*[:：\-\[(0-9]' { return 'leaderboard_submission_deadline' }
        '^(?:본선\s*(?:평가|발표)|발표\s*평가)(?:\s*(?:일정|일시|기간))?\s*[:：\-\[(0-9]' { return 'final_presentation' }
        '^(?:수상(?:자|작)?|결과)\s*발표(?:\s*(?:일정|일시|예정))?\s*[:：\-\[(0-9]' { return 'results' }
        '^(?:(?:작품|결과물|제안서|서류|영상)\s*제출|제출)(?:\s*(?:마감|기간|일정|일시))?\s*[:：\-\[(0-9]' { return 'submission_deadline' }
        '^(?:(?:참가|참여)\s*신청|신청|접수|참가자?\s*모집)(?:\s*(?:마감|기간|일정|일시))?\s*[:：\-\[(0-9]' { return 'application_deadline' }
        '^(?:대회\s*(?:기간|일정)|공모\s*기간)\s*[:：\-\[(0-9]' { return 'competition_period' }
        default { return $null }
    }
}

function Get-ContestDateEvidence([string]$Text) {
    # A year must be printed. Only the second endpoint of one explicit range
    # may inherit that same year; cross-year ranges require both printed years.
    $full='(?<y>20[0-9]{2})\s*(?:[.\-/]|년)\s*(?<m>[0-9]{1,2})\s*(?:[.\-/]|월)\s*(?<d>[0-9]{1,2})\s*(?:[.]|일)?(?:\s*\([^)]{1,4}\))?(?:\s*(?<h>[0-9]{1,2}):(?<n>[0-9]{2}))?'
    $m=[regex]::Match($Text,$full,[Text.RegularExpressions.RegexOptions]::None,[timespan]::FromSeconds(2))
    if(-not $m.Success){return $null}
    try{$date=[datetime]::new([int]$m.Groups['y'].Value,[int]$m.Groups['m'].Value,[int]$m.Groups['d'].Value)}catch{return $null}
    $time=if($m.Groups['h'].Success){'{0:00}:{1:00}' -f [int]$m.Groups['h'].Value,[int]$m.Groups['n'].Value}else{$null}
    $start=$date.ToString('yyyy-MM-dd');$end=$null
    $tail=$Text.Substring($m.Index+$m.Length)
    if($tail -match '^\s*(?:~|～|∼|–|부터)'){
        $next=[regex]::Match($tail,$full,[Text.RegularExpressions.RegexOptions]::None,[timespan]::FromSeconds(2))
        if(-not $next.Success){
            $short=[regex]::Match($tail,'^\s*(?:~|～|∼|–|부터)\s*(?<m>[0-9]{1,2})\s*(?:[.\-/]|월)\s*(?<d>[0-9]{1,2})\s*(?:[.]|일)?(?:\s*\([^)]{1,4}\))?(?:\s*(?<h>[0-9]{1,2}):(?<n>[0-9]{2}))?')
            if(-not $short.Success){return $null}
            try{$finish=[datetime]::new($date.Year,[int]$short.Groups['m'].Value,[int]$short.Groups['d'].Value)}catch{return $null}
            if($finish -lt $date){return $null}
            $time=if($short.Groups['h'].Success){'{0:00}:{1:00}' -f [int]$short.Groups['h'].Value,[int]$short.Groups['n'].Value}else{$null}
        }else{
            try{$finish=[datetime]::new([int]$next.Groups['y'].Value,[int]$next.Groups['m'].Value,[int]$next.Groups['d'].Value)}catch{return $null}
            $time=if($next.Groups['h'].Success){'{0:00}:{1:00}' -f [int]$next.Groups['h'].Value,[int]$next.Groups['n'].Value}else{$null}
        }
        if($finish -lt $date){return $null};$end=$finish.ToString('yyyy-MM-dd')
    }
    if($time -and $time -notmatch '^(?:[01][0-9]|2[0-3]):[0-5][0-9]$|^24:00$'){return $null}
    [pscustomobject]@{start=$start;end=$end;time=$time}
}

function ConvertFrom-ContestPage {
    param([Parameter(Mandatory)]$Page,[datetimeoffset]$Now=[datetimeoffset]::UtcNow)
    $text=[string]$Page.text
    $title=[string]$Page.title
    $organizer='';$category='';$target='';$official=$null;$metadataEnd=$null;$status='unknown'
    $source=[string]$Page.source;$id=[string]$Page.sourceId
    $reason=$null
    if($source -eq 'linkareer'){
        try{
            $match=[regex]::Match($Page.html,'(?is)<script\b[^>]*\bid=["'']__NEXT_DATA__["''][^>]*>(.*?)</script\s*>',[Text.RegularExpressions.RegexOptions]::None,[timespan]::FromSeconds(2))
            if(-not $match.Success){throw 'Missing Linkareer public embedded data.'}
            $data=($match.Groups[1].Value|ConvertFrom-Json -AsHashtable).props.pageProps
            $activity=$data.data.activityData.activity
            if([string]$activity.id -cne $id){throw 'Detail identity differs from observed list.'}
            $title=[string]$activity.title
            # organizationName identifies the publishing account, which may be
            # an agency. It is not proof of the contest's stated organizer.
            $category=@($activity.categories|ForEach-Object {[string]$_.name}) -join ', '
            $target=@($activity.targets|ForEach-Object {[string]$_.name}) -join ', '
            $cache=$data.__APOLLO_STATE__; $record=$cache['Activity:'+$id]
            $body=$record.detailText
            if($body -is [Collections.IDictionary] -and $body.Contains('__ref')){$body=$cache[[string]$body.__ref]}
            if($body -is [Collections.IDictionary]){
                $bodyText=@('text','html','content')|Where-Object {$body.Contains($_) -and $body[$_] -is [string]}|Select-Object -First 1
                if(-not $bodyText){throw 'Unrecognized Linkareer body schema.'}
                $text=ConvertFrom-ContestMarkup ([string]$body[$bodyText])
            }else{throw 'Missing Linkareer authoritative description.'}
            if($activity.homepageURL -is [string] -and $activity.homepageURL -match '^https://'){$official=$activity.homepageURL}
            $metadataEnd=[datetimeoffset]::FromUnixTimeMilliseconds([long]$activity.recruitCloseAt).ToOffset([timespan]::FromHours(9)).ToString('yyyy-MM-dd')
            $status=if($metadataEnd -clt $Now.ToOffset([timespan]::FromHours(9)).ToString('yyyy-MM-dd')){'closed'}else{'open'}
        }catch{$reason=$_.Exception.Message;$status='unknown';$text=''}
    }
    $eligibility='unknown'
    $bodyLines=@($text -split '\r?\n'|Where-Object {$_.Trim()})
    $qualification=@(for($q=0;$q -lt $bodyLines.Count;$q++){
        if($bodyLines[$q] -match '^\s*(?:[-•■●○□▶※]|[0-9]+[.)])?\s*\[?(?:(?:참가|참여|지원|응모)\s*자격|(?:참여|참가|응모|지원|공모)?\s*대상)\]?(?:\s|[:：]|$)'){
            $bodyLines[$q]
            for($next=$q+1;$next -lt [Math]::Min($bodyLines.Count,$q+6);$next++){
                $following=$bodyLines[$next].Trim()
                if($following -match '^(?:[-•■●○□▶※]\s*)?(?:주최|주관|문의|신청|접수|제출|일정|공모\s*기간|시상|선발)\s*[:：]' -or (Get-ContestStageFromLabel $following)){break}
                $following
            }
        }
    }) -join ' '
    # Broad portal tags do not override any unexplained body qualification.
    $unrestricted=$qualification -match '(?:대상|참가|참여)?\s*제한\s*없음|누구나(?:[.。,，\s]|$)'
    $restricted=$qualification -match '보유|재학|졸업|지역|이내|이상|이하|미취업|전공|자격증|단체|기업|거주|연령|[0-9]\s*세|대한민국|교육생|수강생|수료|선발|회원|참가자(?:만|\s)|참여자(?:만|\s)|참가팀|등록자|이라면|라면|한정|에\s*한해|만\s*(?:참가|참여|가능)'
    if($qualification -and $unrestricted -and -not $restricted){$eligibility='eligible'}
    if(-not $organizer){$line=@(($text -split '\r?\n')|Where-Object {$_ -match '^\s*(?:[-•■]\s*)?주최\s*[:：]'})|Select-Object -First 1;if($line){$organizer=($line -replace '^.*?주최\s*[:：]\s*','' -replace '\s*/\s*(?:운영|주관)\s*[:：].*$','').Trim()}}
    if(-not $organizer){$organizer='원문 주최자 확인 필요';$reason='Organizer requires review.'}
    if(-not $official){
        $printedLinks=@(foreach($line in $bodyLines){
            $m=[regex]::Match($line,'^\s*(?:신청\s*및\s*제출|공식\s*(?:신청|접수)|신청|접수)\s*[:：]\s*(https://[^\s<>]+)')
            if($m.Success){$m.Groups[1].Value.TrimEnd('.',',',')',']',';')}
        })
        $officialLinks=@($printedLinks|Select-Object -Unique)
        if($officialLinks.Count -eq 1){$official=$officialLinks[0]}
        elseif($officialLinks.Count -gt 1){$reason='Conflicting official application URLs require review.'}
    }
    $kind='none';$aiText=''
    if($category -match '과학|공학'){$kind='category';$aiText=$category}
    elseif(($title+"`n"+$text) -match '(?i)\bAI\b|인공지능|머신러닝|딥러닝'){
        $aiLines=@((($title+"`n"+$text) -split '\r?\n')|Where-Object {$_ -match '(?i)\bAI\b|인공지능|머신러닝|딥러닝'})
        $aiText=@($aiLines|Select-Object -First 3) -join ' '
        $kind=if($title -match '(?i)\bAI\b|인공지능|머신러닝|딥러닝' -or $aiText -match '(?i)(?:\bAI\b|인공지능).{0,35}(?:사용|활용|제작).{0,20}(?:허용|가능|권장)'){'explicit'}else{'context'}
        if($aiText.Length -gt 500){$aiText=$aiText.Substring(0,500)}
    }
    # A keyword in a prohibition is not evidence of permission or relevance.
    $prohibition=@($bodyLines|Where-Object {$_ -match '(?i)(?:\bAI\b|인공지능).{0,60}(?:금지|불가|제외|불허)'}|Select-Object -First 1)
    if($prohibition.Count){$kind='forbidden';$aiText=$prohibition[0]}
    if($title -match '(?i)(?:연습|practice|테스트|test\s*(대회|competition))'){$status='practice'}
    $milestones=[Collections.Generic.List[object]]::new()
    $lines=@($text -split '\r?\n'|Where-Object {$_.Trim()})
    for($i=0;$i -lt $lines.Count;$i++){
        $line=$lines[$i].Trim()
        # Join an isolated heading with its immediately following printed date.
        if($line -notmatch '20[0-9]{2}' -and $i+1 -lt $lines.Count -and $lines[$i+1] -match '^20[0-9]{2}'){$line+=': '+$lines[$i+1].Trim()}
        $stage=Get-ContestStageFromLabel $line
        if(-not $stage -or $line.Length -gt 600){continue}
        $evidence=$line
        # A heading followed immediately by a date is one printed statement.
        if($evidence -notmatch '20[0-9]{2}' -and $i+1 -lt $lines.Count -and $lines[$i+1] -match '^20[0-9]{2}'){$evidence+=' '+$lines[$i+1].Trim()}
        $date=Get-ContestDateEvidence $evidence
        if($null -eq $date){continue}
        $isRange=$stage -eq 'competition_period'
        $day=if($date.end -and -not $isRange){$date.end}else{$date.start}
        $conflict=$stage -eq 'application_deadline' -and $metadataEnd -and $day -cne $metadataEnd
        $ambiguousStage=$stage -eq 'application_deadline' -and $line -notmatch '(참가|참여)\s*신청|신청\s*(기간|마감)'
        # Printed Korean/AM-PM/midnight times outside the strict HH:MM parser
        # are held for review rather than silently converted into all-day items.
        $unparsedTime=($evidence -match '오전|오후|자정|정오|(?i)\b(?:AM|PM)\b|[0-9]:[0-9]{2}:[0-9]{2}' -or
            ($null -eq $date.time -and $evidence -match '[0-9]\s*시'))
        if($isRange -and $null -ne $date.time){$unparsedTime=$true}
        $milestones.Add([pscustomobject]@{stage=$stage;date=$day;time=$(if($isRange){$null}else{$date.time});endDate=$(if($isRange){$date.end}else{$null});confidence=$(if($conflict -or $reason -or $ambiguousStage -or $unparsedTime){'uncertain'}else{'confirmed'});conditional=($stage -in @('final_presentation','code_submission_deadline'));evidence=$evidence;conflict=[bool]$conflict})
    }
    # Unknown prose is retained for review; never invent a year/time from today,
    # the list publication time, URL, server defaults, or a previous run.
    if($source -ne 'linkareer' -and $milestones.Count){
        $latest=@($milestones|ForEach-Object {if($_.endDate){$_.endDate}else{$_.date}}|Sort-Object)[-1]
        $status=if($latest -clt $Now.ToOffset([timespan]::FromHours(9)).ToString('yyyy-MM-dd')){'closed'}else{'open'}
    }
    if($reason){$eligibility='unknown';if($kind -eq 'none'){$kind='context';$aiText='Extraction requires review; no reliable classification was made.'}}
    [pscustomobject]@{source=$source;sourceId=$id;sourceUrl=[string]$Page.sourceUrl;title=$title;organizer=$organizer;officialUrl=$official;status=$status;eligibility=$eligibility;aiEvidence=[pscustomobject]@{kind=$kind;text=$aiText};milestones=$milestones.ToArray();reviewReason=$reason;qualificationEvidence=$qualification}
}

function ConvertFrom-ContestPages {
    param([Parameter(Mandatory)]$Collection,[string]$RunId=([guid]::NewGuid().ToString('N')),[datetimeoffset]$Now=[datetimeoffset]::UtcNow)
    $candidates=@(foreach($page in @($Collection.pages)){ConvertFrom-ContestPage $page $Now})
    [pscustomobject]@{schemaVersion=1;runId=$RunId;collectedAt=$Now.ToString('o');sourceResults=@($Collection.sourceResults);candidates=$candidates}
}

Export-ModuleMember -Function ConvertFrom-ContestPages, Get-ContestDateEvidence
