# Public contest HTML only: no calendar writes, credentials, cookies, browser,
# API discovery, candidate classification, or scheduling.
Set-StrictMode -Version Latest

$script:CollectorUserAgent = 'DotCalendarContestCollector/1.0'
$script:Profiles = @{
    linkareer = @{ Url = 'https://linkareer.com/list/contest?filterType=CATEGORY&orderBy_direction=DESC&orderBy_field=CREATED_AT&page=1'; Hosts = @('linkareer.com','www.linkareer.com'); Detail = '^/activity/([0-9]+)/?$' }
    dacon = @{ Url = 'https://dacon.io/competitions'; Hosts = @('dacon.io','www.dacon.io'); Detail = '^/competitions/official/([0-9]+)(?:/overview(?:/(?:description|schedule|rules))?)?/?$' }
    mokpo = @{ Url = 'https://www.mokpo.ac.kr/www/308/subview.do'; Hosts = @('www.mokpo.ac.kr','mokpo.ac.kr'); Detail = '^/bbs/www/148/([0-9]+)/artclView[.]do$' }
    daker = @{ Url = 'https://daker.ai/public/hackathons'; Hosts = @('daker.ai','www.daker.ai'); Detail = '^/public/hackathons/([a-zA-Z0-9][a-zA-Z0-9_-]*)/?$' }
    aifactory = @{ Url = 'https://aifactory.space/ko/competition'; Hosts = @('aifactory.space','www.aifactory.space'); Detail = '^/ko/competitions/([0-9]+)/?$' }
}

function Assert-ApprovedContestUri {
    param([uri]$Uri, [hashtable]$Profile)
    if (-not $Uri.IsAbsoluteUri -or $Uri.Scheme -ne 'https' -or
        $Uri.Port -ne 443 -or $Uri.UserInfo -or
        $Uri.DnsSafeHost.ToLowerInvariant() -notin $Profile.Hosts) {
        throw "Unapproved public URL: $Uri"
    }
    if ($Uri.AbsolutePath -match '(?i)(?:^|/)(?:login|sign[-_]?in|auth)(?:/|$)') {
        throw "Login URL is not a public source: $Uri"
    }
}

function Get-PublicContestHttp {
    param([uri]$Uri, [hashtable]$Profile, [object]$RobotsPolicy = $null)
    Add-Type -AssemblyName System.Net.Http -ErrorAction Stop
    $handler = [System.Net.Http.HttpClientHandler]::new()
    $handler.AllowAutoRedirect = $false
    $handler.UseCookies = $false
    $handler.UseDefaultCredentials = $false
    $handler.Credentials = $null
    $handler.PreAuthenticate = $false
    $client = [System.Net.Http.HttpClient]::new($handler)
    $client.Timeout = [TimeSpan]::FromSeconds(15)
    $client.MaxResponseContentBufferSize = 5MB
    $client.DefaultRequestHeaders.UserAgent.ParseAdd($script:CollectorUserAgent)
    try {
        $current = $Uri
        for ($redirect = 0; $redirect -le 3; $redirect++) {
            Assert-ApprovedContestUri $current $Profile
            if ($current.DnsSafeHost -ne $Uri.DnsSafeHost) {
                throw "Redirected host requires separate robots validation: $current"
            }
            if ($null -ne $RobotsPolicy -and -not (Test-ContestRobotsAllowed ([string]$RobotsPolicy) $current)) {
                throw "robots.txt disallows redirected URL: $current"
            }
            $cancellation=[Threading.CancellationTokenSource]::new()
            try{
                if($Profile.ContainsKey('DeadlineUtc')){
                    $remaining=($Profile.DeadlineUtc-[datetimeoffset]::UtcNow).TotalMilliseconds
                    if($remaining -le 0){throw 'Active source time budget expired.'}
                    $cancellation.CancelAfter([int][Math]::Min(15000,$remaining))
                }
                $response = $client.GetAsync($current,$cancellation.Token).GetAwaiter().GetResult()
            }finally{$cancellation.Dispose()}
            try {
                $status = [int]$response.StatusCode
                $content = $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
                if ($content.Length -gt 5MB) { throw 'Public HTML exceeds the 5 MiB collection limit.' }
                if ($status -in @(301,302,303,307,308)) {
                    if ($redirect -eq 3) { throw 'Public source redirect limit exceeded.' }
                    if ($null -eq $response.Headers.Location) { throw 'Redirect response has no Location.' }
                    $current = [uri]::new($current, $response.Headers.Location)
                    continue
                }
                return @{ StatusCode = $status; Content = $content; Url = $current.AbsoluteUri }
            }
            finally { $response.Dispose() }
        }
    }
    finally { $client.Dispose() }
}

function Read-ContestResponse {
    param([uri]$Uri, [hashtable]$Profile, [scriptblock]$Fetcher, [object]$RobotsPolicy = $null)
    Assert-ApprovedContestUri $Uri $Profile
    try {
        if($Profile.ContainsKey('DeadlineUtc') -and [datetimeoffset]::UtcNow -ge $Profile.DeadlineUtc){throw 'Active source time budget expired.'}
        $raw = if ($Fetcher) { & $Fetcher $Uri } else { Get-PublicContestHttp $Uri $Profile $RobotsPolicy }
        if ($raw -is [string]) {
            $result = @{ StatusCode = 200; Content = $raw; Url = $Uri.AbsoluteUri }
        }
        elseif ($raw -is [System.Collections.IDictionary]) {
            if (-not $raw.Contains('StatusCode') -or -not $raw.Contains('Content')) {
                throw 'Fetcher must return StatusCode and Content.'
            }
            $result = @{ StatusCode = [int]$raw.StatusCode; Content = [string]$raw.Content; Url = $Uri.AbsoluteUri }
            if ($raw.Contains('Url') -and $raw.Url) { $result.Url = [string]$raw.Url }
        }
        else {
            if ($null -eq $raw -or -not $raw.PSObject.Properties['StatusCode'] -or -not $raw.PSObject.Properties['Content']) {
                throw 'Fetcher must return HTML or an object with StatusCode and Content.'
            }
            $result = @{ StatusCode = [int]$raw.StatusCode; Content = [string]$raw.Content; Url = $Uri.AbsoluteUri }
            if ($raw.PSObject.Properties['Url'] -and $raw.Url) { $result.Url = [string]$raw.Url }
        }
        Assert-ApprovedContestUri ([uri]$result.Url) $Profile
        if (([uri]$result.Url).DnsSafeHost -ne $Uri.DnsSafeHost) {
            throw "Redirected host requires separate robots validation: $($result.Url)"
        }
        if ($null -ne $RobotsPolicy -and -not (Test-ContestRobotsAllowed ([string]$RobotsPolicy) ([uri]$result.Url))) {
            throw "robots.txt disallows redirected URL: $($result.Url)"
        }
        if ($result.Content.Length -gt 5MB) { throw 'Public HTML exceeds the 5 MiB collection limit.' }
        $result.Error = $null
        $result.Blocked = $false
        if ($result.StatusCode -in @(401,403,429)) {
            $result.Blocked = $true
            $result.Error = "Public source access blocked: HTTP $($result.StatusCode)."
        }
        elseif ($result.StatusCode -ge 400 -or $result.StatusCode -lt 200 -or $result.StatusCode -ge 300) {
            $result.Error = "Public source returned HTTP $($result.StatusCode)."
        }
        elseif ($result.Content -match '(?is)<title[^>]*>\s*(?:Just a moment|Attention Required|Access denied|Login|Sign in|로그인|CAPTCHA|Security verification)\b' -or
                $result.Content -match '(?i)(?:id=["'']challenge-form|cf-chl-|/cdn-cgi/challenge-platform/|class=["''][^"'']*\b(?:g-recaptcha|h-captcha)\b)') {
            $result.Blocked = $true
            $result.Error = 'Public source returned a login or CAPTCHA/challenge page.'
        }
        return $result
    }
    catch {
        $blocked = $_.Exception.Message -match 'Unapproved public URL|Login URL|robots[.]txt disallows|requires separate robots validation'
        return @{ StatusCode = 0; Content = ''; Url = $Uri.AbsoluteUri; Blocked = $blocked; Error = $_.Exception.Message }
    }
}

function Test-ContestRobotsAllowed {
    param([string]$Robots, [uri]$Uri)
    $groups = [System.Collections.Generic.List[object]]::new()
    $agents = [System.Collections.Generic.List[string]]::new()
    $rules = [System.Collections.Generic.List[object]]::new()
    $hadDirective = $false
    $Robots = $Robots -replace '^\uFEFF',''
    foreach ($rawLine in ($Robots -split "\r?\n")) {
        $line = ($rawLine -replace '#.*$', '').Trim()
        if (-not $line) {
            if ($agents.Count -gt 0 -and $hadDirective) {
                $groups.Add(@{ Agents = $agents.ToArray(); Rules = $rules.ToArray() })
                $agents.Clear(); $rules.Clear(); $hadDirective = $false
            }
            continue
        }
        if ($line -notmatch '^([^:]+):\s*(.*)$') { continue }
        $key = $Matches[1].Trim().ToLowerInvariant()
        $value = $Matches[2].Trim()
        if ($key -eq 'user-agent') {
            if ($hadDirective -and $agents.Count -gt 0) {
                $groups.Add(@{ Agents = $agents.ToArray(); Rules = $rules.ToArray() })
                $agents.Clear(); $rules.Clear(); $hadDirective = $false
            }
            $agents.Add($value.ToLowerInvariant())
        }
        elseif ($agents.Count -gt 0) {
            $hadDirective = $true
            if ($key -in @('allow','disallow') -and $value) {
                $rules.Add(@{ Allow = ($key -eq 'allow'); Path = $value })
            }
        }
    }
    if ($agents.Count -gt 0) { $groups.Add(@{ Agents = $agents.ToArray(); Rules = $rules.ToArray() }) }
    $selected = [System.Collections.Generic.List[object]]::new()
    $bestAgent = -1
    foreach ($group in $groups) {
        $score = -1
        foreach ($agent in $group.Agents) {
            $candidate = if ($agent -eq '*') { 0 } elseif ($script:CollectorUserAgent.ToLowerInvariant().StartsWith($agent)) { $agent.Length } else { -1 }
            if ($candidate -gt $score) { $score = $candidate }
        }
        if ($score -gt $bestAgent) { $selected.Clear(); $bestAgent = $score }
        if ($score -eq $bestAgent -and $score -ge 0) { $selected.Add($group) }
    }
    $bestRule = -1
    $allowed = $true
    foreach ($group in $selected) {
        foreach ($rule in $group.Rules) {
            $path = [string]$rule.Path
            $terminal = $path.EndsWith('$')
            if ($terminal) { $path = $path.Substring(0, $path.Length - 1) }
            $pattern = '^' + [regex]::Escape($path).Replace('\*','.*')
            if ($terminal) { $pattern += '$' }
            if ([regex]::IsMatch($Uri.PathAndQuery, $pattern, [System.Text.RegularExpressions.RegexOptions]::None, [TimeSpan]::FromSeconds(2))) {
                $score = ($path -replace '\*','').Length
                if ($score -gt $bestRule -or ($score -eq $bestRule -and $rule.Allow)) {
                    $bestRule = $score
                    $allowed = [bool]$rule.Allow
                }
            }
        }
    }
    return $allowed
}

function ConvertFrom-ContestHtml {
    param([string]$Html)
    $options = [System.Text.RegularExpressions.RegexOptions]::IgnoreCase -bor [System.Text.RegularExpressions.RegexOptions]::Singleline
    $timeout = [TimeSpan]::FromSeconds(2)
    $newline = [string][char]10
    $value = [regex]::Replace($Html, '<!--.*?-->|<(script|style|svg)\b[^>]*>.*?</\1\s*>', '', $options, $timeout)
    $value = [regex]::Replace($value, '</?(?:p|div|br|li|h[1-6]|tr|section|article|header|footer)\b[^>]*>', $newline, $options, $timeout)
    $value = [regex]::Replace($value, '<[^>]+>', ' ', $options, $timeout)
    $value = [System.Net.WebUtility]::HtmlDecode($value)
    $value = $value -replace '[\t \u00a0]+',' ' -replace ' *\r?\n *',$newline -replace '\n{3,}',($newline + $newline)
    return $value.Trim()
}

function Get-ContestHtmlTitle {
    param([string]$Html)
    foreach ($tag in @('h1','title')) {
        $match = [regex]::Match($Html, "<$tag\b[^>]*>(.*?)</$tag\s*>",
            [System.Text.RegularExpressions.RegexOptions]::IgnoreCase -bor [System.Text.RegularExpressions.RegexOptions]::Singleline,
            [TimeSpan]::FromSeconds(2))
        if ($match.Success) {
            $title = ConvertFrom-ContestHtml $match.Groups[1].Value
            if ($title) { return ($title -replace '\s+',' ').Trim() }
        }
    }
    return ''
}

function Get-ObservedContestLinks {
    param([string]$Html, [uri]$BaseUri, [hashtable]$Profile)
    $seen = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    $pattern = '\bhref\s*=\s*(?:"([^"]*)"|''([^'']*)''|([^\s"''<>]+))'
    foreach ($match in [regex]::Matches($Html, $pattern, [System.Text.RegularExpressions.RegexOptions]::IgnoreCase, [TimeSpan]::FromSeconds(2))) {
        $href = ($match.Groups[1].Value, $match.Groups[2].Value, $match.Groups[3].Value | Where-Object { $_ } | Select-Object -First 1)
        if (-not $href) { continue }
        $href = [System.Net.WebUtility]::HtmlDecode([string]$href)
        if ($href -match '(?i)^(?:javascript|data|mailto|tel):') { continue }
        try {
            $uri = [uri]::new($BaseUri, $href)
            Assert-ApprovedContestUri $uri $Profile
            $path = $uri.AbsolutePath -replace '(?i);jsessionid=[^/?#]*',''
            $detail = [regex]::Match($path, $Profile.Detail)
            if (-not $detail.Success) { continue }
            $builder = [UriBuilder]::new($uri)
            $builder.Path = $path
            $builder.Query = ''; $builder.Fragment = ''
            $url = $builder.Uri.AbsoluteUri
            if ($seen.Add($url)) { [pscustomobject]@{ Id = $detail.Groups[1].Value; Url = $url } }
        }
        catch { continue }
    }
}

function Get-ListingContestLinks {
    param([string]$Source, [string]$Html, [uri]$BaseUri, [hashtable]$Profile, [switch]$LatestList)
    $links = @(Get-ObservedContestLinks $Html $BaseUri $Profile)
    if ($Source -ne 'linkareer') { return $links }
    if ($links.Count -eq 0) { if($LatestList){throw 'No observed URLs in the latest public listing.'};return $links }
    # The observed public Next.js activityItems array is the requested list. Banner
    # recommendations precede it in HTML. Reorder observed URLs only; never infer IDs
    # into URLs, call GraphQL/APIs, or decide whether an item is new from this ordering.
    try {
        $next = [regex]::Match($Html, '<script\b[^>]*\bid=["'']__NEXT_DATA__["''][^>]*>(.*?)</script\s*>',
            [System.Text.RegularExpressions.RegexOptions]::IgnoreCase -bor [System.Text.RegularExpressions.RegexOptions]::Singleline,
            [TimeSpan]::FromSeconds(2))
        if (-not $next.Success) { if($LatestList){throw 'Missing latest-list embedded data.'};return $links }
        $props = ($next.Groups[1].Value | ConvertFrom-Json -ErrorAction Stop).props.pageProps
        if (-not $props.PSObject.Properties['activityItems']) { if($LatestList){throw 'Missing latest-list items.'};return $links }
        if($LatestList -and ($Profile.ContainsKey('ExpectedCategory') -or $Profile.ContainsKey('ExpectedQuery'))){
            $keys=@($props.__APOLLO_STATE__.ROOT_QUERY.PSObject.Properties.Name|Where-Object {$_ -like 'activities(*'})
            $verified=$false
            foreach($key in $keys){
                $query=$key.Substring(11,$key.Length-12)|ConvertFrom-Json -AsHashtable
                if($query.orderBy.field -ne 'CREATED_AT' -or $query.orderBy.direction -ne 'DESC' -or $query.pagination.page -ne 1){continue}
                if($Profile.ContainsKey('ExpectedCategory') -and $query.filterBy.ContainsKey('categoryIDs') -and @($query.filterBy.categoryIDs) -contains $Profile.ExpectedCategory){$verified=$true}
                if($Profile.ContainsKey('ExpectedQuery') -and $query.filterBy.ContainsKey('q') -and $query.filterBy.q -ceq $Profile.ExpectedQuery){$verified=$true}
            }
            if(-not $verified){throw 'Public listing did not confirm the requested category/search and latest ordering.'}
        }
        $ordered = [System.Collections.Generic.List[object]]::new()
        $seen = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        foreach ($item in @($props.activityItems)) {
            if (-not $item.PSObject.Properties['url'] -or -not $item.url) { if($LatestList){throw 'Unrecognized latest-list item URL.'};continue }
            $uri = [uri]::new($BaseUri, [string]$item.url)
            try { Assert-ApprovedContestUri $uri $Profile } catch { if($LatestList){throw};continue }
            $identity = [regex]::Match($uri.AbsolutePath, $Profile.Detail)
            if (-not $identity.Success) { if($LatestList){throw 'Unrecognized latest-list detail identity.'};continue }
            $observed=$false
            foreach ($link in $links) {
                if ($link.Id -eq $identity.Groups[1].Value) { $observed=$true;if($seen.Add($link.Url)) { $ordered.Add($link) } }
            }
            if($LatestList -and -not $observed){throw 'Latest-list entry has no observed public detail link.'}
        }
        # Daily checks cover the requested latest list, not advertising banners.
        # A missing/unrecognized list remains an error rather than an empty result.
        if ($LatestList -and $ordered.Count -eq 0) { throw 'No observed URLs matched the public latest-list data.' }
        if (-not $LatestList) { foreach ($link in $links) { if ($seen.Add($link.Url)) { $ordered.Add($link) } } }
        return $ordered.ToArray()
    }
    catch { if($LatestList){throw}; return $links }
}


function Get-ContestSourcePages {
    <#
    .SYNOPSIS
    Collect observed public HTML detail links without deciding which contests are new.
    .PARAMETER Fetcher
    Offline fixture hook, called with one System.Uri. Return an HTML string (HTTP 200), or
    @{ StatusCode = 200; Content = '<html>...'; Url = 'https://...' }.
    Without a hook: anonymous HTTPS, no cookies/credentials, 15-second request timeout.
    .PARAMETER MaxDetails
    Maximum detail URLs fetched per source. The observed requested Linkareer SSR list
    takes precedence over banner links. Ordering is only a collection bound; recommended
    or first entries are never assumed to be new. No early stop at known IDs.
    #>
    [CmdletBinding()]
    param(
        [ValidateNotNullOrEmpty()]
        [ValidateSet('linkareer','dacon','mokpo','daker','aifactory')]
        [string[]]$Sources = @('linkareer','dacon','mokpo'),
        [ValidateRange(1,150)][int]$MaxDetails = 5,
        [switch]$LatestList,
        [scriptblock]$Fetcher,
        [datetimeoffset]$DeadlineUtc = [datetimeoffset]::MaxValue
    )
    $results = [System.Collections.Generic.List[object]]::new()
    $pages = [System.Collections.Generic.List[object]]::new()
    $uniqueSources = @($Sources | ForEach-Object { $_.ToLowerInvariant() } | Select-Object -Unique)
    foreach ($source in $uniqueSources) {
        $profile = $script:Profiles[$source].Clone()
        if($LatestList -and $source -eq 'linkareer'){
            # Category 35 = 과학/공학 in the public detail schema; the public
            # client serializes filterBy.categoryIDs as filterBy_categoryIDs.
            $profile.Url='https://linkareer.com/list/contest?filterBy_categoryIDs=35&filterType=CATEGORY&orderBy_direction=DESC&orderBy_field=CREATED_AT&page=1'
            $profile.ExpectedCategory='35'
        }
        if($LatestList){$profile.DeadlineUtc=[datetimeoffset]::UtcNow.AddSeconds(120)}
        if($DeadlineUtc -lt [datetimeoffset]::MaxValue -and (-not $profile.ContainsKey('DeadlineUtc') -or $DeadlineUtc -lt $profile.DeadlineUtc)){$profile.DeadlineUtc=$DeadlineUtc}
        $sourceClock = [Diagnostics.Stopwatch]::StartNew()
        $listUri = [uri]$profile.Url
        $result = [pscustomobject]@{ source = $source; status = 'error'; url = $profile.Url; error = $null }
        $robotsUri = [uri]($listUri.GetLeftPart([UriPartial]::Authority) + '/robots.txt')
        $sourcePageStart = $pages.Count
        try {
        $robots = Read-ContestResponse $robotsUri $profile $Fetcher
        if ($robots.Blocked) {
            $result.status = 'blocked'; $result.error = $robots.Error
            $results.Add($result); continue
        }
        if ($robots.Error -and $robots.StatusCode -ne 404) {
            $result.error = "robots.txt check failed: $($robots.Error)"
            $results.Add($result); continue
        }
        $robotsText = if ($robots.StatusCode -eq 404) { '' } else { $robots.Content }
        if ($LatestList -and $source -eq 'daker') {
            # Observed in the public client on 2026-10-15. Do not bypass robots,
            # log in, manufacture credentials, or call account-specific endpoints.
            $api = [uri]'https://daker.ai/api/hackathons/public-list?limit=9'
            $result.status = if(Test-ContestRobotsAllowed $robotsText $api){'partial'}else{'blocked'}
            $result.error = if($result.status -eq 'blocked'){'robots.txt disallows the public list API; the HTML is a JavaScript shell. Automatic collection is unavailable.'}else{'The public API adapter requires schema review; no guessed API data was imported.'}
            $results.Add($result); continue
        }
        if (-not (Test-ContestRobotsAllowed $robotsText $listUri)) {
            $result.status = 'blocked'; $result.error = 'robots.txt disallows the approved listing URL.'
            $results.Add($result); continue
        }
        $listing = Read-ContestResponse $listUri $profile $Fetcher $robotsText
        if ($listing.Error) {
            $result.status = if ($listing.Blocked) { 'blocked' } else { 'error' }
            $result.error = $listing.Error; $results.Add($result); continue
        }
        if ($LatestList -and $source -eq 'mokpo') {
            # This RSS link is printed on the requested /www/308 listing. Its
            # item descriptions are official source text; no image OCR is guessed.
            $rssLink = [regex]::Match($listing.Content,'href=["''](/bbs/www/148/rssList[.]do\?row=50)["'']')
            if(-not $rssLink.Success){throw 'Mokpo listing no longer exposes the reviewed RSS link.'}
            $rssUri=[uri]::new([uri]$listing.Url,[Net.WebUtility]::HtmlDecode($rssLink.Groups[1].Value))
            if(-not(Test-ContestRobotsAllowed $robotsText $rssUri)){throw 'robots.txt disallows the official RSS.'}
            $rss=Read-ContestResponse $rssUri $profile $Fetcher $robotsText
            if($rss.Error){throw $rss.Error}
            $settings=[Xml.XmlReaderSettings]::new();$settings.DtdProcessing=[Xml.DtdProcessing]::Prohibit;$settings.XmlResolver=$null
            $reader=[Xml.XmlReader]::Create([IO.StringReader]::new($rss.Content),$settings)
            try{$xml=[Xml.XmlDocument]::new();$xml.XmlResolver=$null;$xml.Load($reader)}finally{$reader.Dispose()}
            $items=@($xml.SelectNodes('/rss/channel/item'))
            if(-not $items.Count -or $items.Count -gt 50){throw 'Mokpo RSS item count/schema requires review.'}
            foreach($item in $items){
                $title=$item.SelectSingleNode('title').InnerText
                if($title -notmatch '공모|경진|해커톤|챌린지'){continue}
                $uri=[uri]::new($rssUri,$item.SelectSingleNode('link').InnerText);Assert-ApprovedContestUri $uri $profile
                $idMatch=[regex]::Match($uri.AbsolutePath,$profile.Detail)
                if(-not $idMatch.Success){throw 'Mokpo RSS detail identity changed.'}
                $body=$item.SelectSingleNode('description').InnerText
                $pages.Add([pscustomobject]@{source=$source;sourceId=$idMatch.Groups[1].Value;sourceUrl=$uri.GetLeftPart([UriPartial]::Path);title=$title;html=$body;text=(ConvertFrom-ContestHtml $body);evidenceUrl=$rssUri.AbsoluteUri})
            }
            $result.status='ok';$result.error=$null
            $result|Add-Member -NotePropertyName scope -NotePropertyValue 'Official latest RSS items (at most 50); older notices were not exhaustively scanned.'
            $result|Add-Member -NotePropertyName scopeComplete -NotePropertyValue $false
            $results.Add($result);continue
        }
        $links = @(Get-ListingContestLinks $source $listing.Content ([uri]$listing.Url) $profile -LatestList:$LatestList)
        if($LatestList -and $source -eq 'linkareer'){
            $aiProfile=$profile.Clone();$aiProfile.Remove('ExpectedCategory');$aiProfile.ExpectedQuery='AI'
            $aiUri=[uri]'https://linkareer.com/list/contest?filterBy_q=AI&filterType=CATEGORY&orderBy_direction=DESC&orderBy_field=CREATED_AT&page=1'
            if(-not(Test-ContestRobotsAllowed $robotsText $aiUri)){throw 'robots.txt disallows the AI latest list.'}
            $aiList=Read-ContestResponse $aiUri $aiProfile $Fetcher $robotsText
            if($aiList.Error){throw $aiList.Error}
            $aiLinks=@(Get-ListingContestLinks $source $aiList.Content ([uri]$aiList.Url) $aiProfile -LatestList)
            $ids=[Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
            $links=@(@($links)+@($aiLinks)|Where-Object {$ids.Add($_.Id)})
            $result|Add-Member -NotePropertyName scope -NotePropertyValue 'Union of the first 20 latest science/engineering notices and first 20 AI search notices; banner recommendations excluded.'
            $result|Add-Member -NotePropertyName scopeComplete -NotePropertyValue $false
            $result|Add-Member -NotePropertyName evidenceListings -NotePropertyValue @($listing.Url,$aiList.Url)
        }
        if($LatestList -and $source -eq 'aifactory'){
            $cards=[regex]::Matches($listing.Content,'(?is)<a\b[^>]*href="/ko/competitions/([0-9]+)"[^>]*>(.*?)</a>',[Text.RegularExpressions.RegexOptions]::None,[timespan]::FromSeconds(2))
            if(-not $cards.Count){throw 'AIFactory public card schema requires review.'}
            $closed=[Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
            foreach($card in $cards){if((ConvertFrom-ContestHtml $card.Groups[2].Value) -match '(?m)^종료$'){[void]$closed.Add($card.Groups[1].Value)}}
            $result|Add-Member -NotePropertyName excludedClosedIds -NotePropertyValue @($closed|Sort-Object)
            $result|Add-Member -NotePropertyName scopeComplete -NotePropertyValue $false
            $links=@($links|Where-Object {-not $closed.Contains($_.Id)})
            if(-not $links.Count -and $closed.Count -gt 0){
                $result.status='ok';$result.error=$null
                $result|Add-Member -NotePropertyName scope -NotePropertyValue 'Every observed public card is explicitly marked ended; no ended detail was reimported.'
                $results.Add($result);continue
            }
        }
        if ($links.Count -eq 0) {
            $result.status = 'partial'
            $result.error = 'No approved detail links exposed by public HTML (possibly a JavaScript shell). Unavailable collection is not evidence of zero new contests.'
            $results.Add($result); continue
        }
        $errors = [System.Collections.Generic.List[string]]::new()
        $succeeded = 0
        $blocked = $false
        foreach ($link in ($links | Select-Object -First $MaxDetails)) {
            if ($LatestList -and $sourceClock.Elapsed.TotalSeconds -ge 120) {
                $errors.Add('The two-minute source budget was reached; remaining details were not claimed as checked.')
                break
            }
            $detailUri = [uri]$link.Url
            if ($detailUri.DnsSafeHost -ne ([uri]$robots.Url).DnsSafeHost) {
                $errors.Add("Separate robots validation is required for detail host $($detailUri.DnsSafeHost)."); $blocked = $true
                continue
            }
            if (-not (Test-ContestRobotsAllowed $robotsText $detailUri)) {
                $errors.Add("robots.txt disallows detail $($link.Url)."); $blocked = $true
                continue
            }
            $detail = Read-ContestResponse $detailUri $profile $Fetcher $robotsText
            if ($detail.Error) {
                $errors.Add("$($link.Url): $($detail.Error)")
                if ($detail.Blocked) { $blocked = $true; break }
                continue
            }
            $finalPath = ([uri]$detail.Url).AbsolutePath -replace '(?i);jsessionid=[^/?#]*',''
            $finalMatch = [regex]::Match($finalPath, $profile.Detail)
            if (-not $finalMatch.Success -or $finalMatch.Groups[1].Value -ne $link.Id) {
                $errors.Add("$($link.Url): Redirected detail does not match its observed source ID.")
                continue
            }
            $detailParts=[Collections.Generic.List[object]]::new()
            $detailParts.Add($detail)
            if($LatestList -and $source -eq 'dacon'){
                $tabs=@(Get-ObservedContestLinks $detail.Content ([uri]$detail.Url) $profile |
                    Where-Object {$_.Id -eq $link.Id -and ([uri]$_.Url).AbsolutePath -match '/overview/(schedule|rules)/?$'})
                foreach($tabName in @('schedule','rules')){
                    $tab=@($tabs|Where-Object {([uri]$_.Url).AbsolutePath -match ('/overview/'+$tabName+'/?$')})|Select-Object -First 1
                    if(-not $tab){$errors.Add("$($link.Url): Required $tabName link is not present in public HTML.");continue}
                    $tabUri=[uri]$tab.Url
                    if($tabUri.DnsSafeHost -ne ([uri]$robots.Url).DnsSafeHost -or -not(Test-ContestRobotsAllowed $robotsText $tabUri)){$errors.Add('robots.txt disallows or does not cover an observed DACON tab.');$blocked=$true;continue}
                    $tabPage=Read-ContestResponse $tabUri $profile $Fetcher $robotsText
                    if($tabPage.Error){$errors.Add("$($tab.Url): $($tabPage.Error)");if($tabPage.Blocked){$blocked=$true};continue}
                    if(([uri]$tabPage.Url).AbsolutePath -cne $tabUri.AbsolutePath){$errors.Add('DACON tab redirected to a different page.');continue}
                    $detailParts.Add($tabPage)
                }
            }
            $texts=[Collections.Generic.List[string]]::new()
            foreach($part in $detailParts){
            $body = [regex]::Match($part.Content, '<body\b[^>]*>(.*?)</body\s*>',
                [System.Text.RegularExpressions.RegexOptions]::IgnoreCase -bor [System.Text.RegularExpressions.RegexOptions]::Singleline,
                [TimeSpan]::FromSeconds(2))
            $texts.Add((ConvertFrom-ContestHtml $(if ($body.Success) { $body.Groups[1].Value } else { $part.Content })))
            }
            $text=$texts -join "`n"
            if (-not $text) { $errors.Add("$($link.Url): No readable SSR text."); continue }
            $pages.Add([pscustomobject]@{
                source = $source; sourceId = $link.Id; sourceUrl = $link.Url
                title = (Get-ContestHtmlTitle $detail.Content); html = $detail.Content; text = $text
                evidencePages = @($detailParts|ForEach-Object {[pscustomobject]@{url=$_.Url;html=$_.Content}})
            })
            $succeeded++
        }
        if ($errors.Count -gt 0) {
            $result.status = if ($succeeded -gt 0) { 'partial' } elseif ($blocked) { 'blocked' } else { 'error' }
            $result.error = $errors -join ' '
        }
        elseif ($succeeded -gt 0) {
            if ($links.Count -gt $MaxDetails) {
                $result.status = 'partial'
                $result.error = "Collection limited to $MaxDetails of $($links.Count) observed detail URLs; this is not an exhaustive new-contest check."
            }
            else { $result.status = 'ok' }
        }
        else { $result.error = 'No detail body was collected; collection is unavailable.' }
        $results.Add($result)
        }
        catch {
            $result.status = if ($pages.Count -gt $sourcePageStart) { 'partial' } else { 'error' }
            $result.error = "Public HTML processing failed: $($_.Exception.Message)"
            $results.Add($result)
        }
    }
    return [pscustomobject]@{ sourceResults = $results.ToArray(); pages = $pages.ToArray() }
}

Export-ModuleMember -Function Get-ContestSourcePages

