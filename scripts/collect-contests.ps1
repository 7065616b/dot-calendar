param(
    [Parameter(Mandatory, ParameterSetName='Import')] [string]$CandidatesFile,
    [Parameter(Mandatory, ParameterSetName='Collect')] [switch]$CollectSources,
    [ValidateSet('Preview','Apply')] [string]$Mode='Preview',
    [string]$StateDirectory,
    [string]$ReportFile,
    [string]$HelperPath=(Join-Path $env:USERPROFILE '.agents\skills\dot-calendar\scripts\invoke-calendar.ps1'),
    [string[]]$Sources=@('linkareer','dacon','mokpo'),
    [ValidateRange(1,20)] [int]$MaxDetails=5
)
# The parent automation owns the morning schedule. This command never creates
# an OS task, a timer, an account, a network AI call or a Google Calendar write.
#Requires -Version 7.0
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$OutputEncoding=[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)

Import-Module (Join-Path $PSScriptRoot 'contest-collector\ContestConnection.psm1') -Force

$moduleRoot=Join-Path $PSScriptRoot 'contest-collector'
$report=$null
$exitCode=0
try {
    if($ReportFile) {
        $ReportFile=[IO.Path]::GetFullPath($ReportFile)
        if(Test-Path -LiteralPath $ReportFile){throw 'Refusing to overwrite an existing result log; choose a new report path.'}
    }
    if($CollectSources) {
        if($Mode -eq 'Apply'){throw 'Collection only retrieves public pages. Supply a verified candidate batch to Apply.'}
        Import-Module (Join-Path $moduleRoot 'ContestSources.psm1') -Force
        $report=Get-ContestSourcePages -Sources $Sources -MaxDetails $MaxDetails
    } else {
        $candidatePath=[IO.Path]::GetFullPath($CandidatesFile)
        if(-not (Test-Path -LiteralPath $candidatePath -PathType Leaf)){throw 'Candidate file not found.'}
        if((Get-Item -LiteralPath $candidatePath).Length -gt 4MB){throw 'Candidate input exceeds 4 MiB.'}
        $inputBatch=[IO.File]::ReadAllText($candidatePath,[Text.Encoding]::UTF8)|ConvertFrom-Json -Depth 40
        Import-Module (Join-Path $moduleRoot 'ContestNormalize.psm1') -Force
        Import-Module (Join-Path $moduleRoot 'ContestImport.psm1') -Force
        $batch=ConvertTo-ContestBatch -InputObject $inputBatch
        $HelperPath=[IO.Path]::GetFullPath($HelperPath)
        if(-not (Test-Path -LiteralPath $HelperPath -PathType Leaf)){throw 'Dot Calendar connection helper is unavailable; no calendar write occurred.'}
        $connectionFile=Join-Path ([IO.Path]::GetDirectoryName([IO.Path]::GetDirectoryName($HelperPath))) 'connection.json'
        if(-not (Test-Path -LiteralPath $connectionFile -PathType Leaf)){throw 'Dot Calendar connection configuration is unavailable.'}
        $connection=Get-Content -LiteralPath $connectionFile -Raw -Encoding utf8|ConvertFrom-Json
        $StateDirectory=Resolve-ContestCollectorStateDirectory -DataDirectory $connection.data_directory -ExplicitStateDirectory $StateDirectory
        $calendarDirectory=[IO.Path]::GetFullPath($connection.data_directory).TrimEnd('\')
        $stateBoundary=$StateDirectory.TrimEnd('\')
        if($stateBoundary -ieq $calendarDirectory -or $stateBoundary.StartsWith($calendarDirectory+'\',[StringComparison]::OrdinalIgnoreCase) -or $calendarDirectory.StartsWith($stateBoundary+'\',[StringComparison]::OrdinalIgnoreCase)){
            throw 'Collector state must be separate from the calendar data directory.'
        }
        if($Mode -eq 'Apply') {
            # Store's named mutex is session-local. Do not write against a GUI
            # owned by another interactive Windows session.
            $session=[Diagnostics.Process]::GetCurrentProcess().SessionId
            $gui=@(Get-CimInstance Win32_Process -Filter "Name='dot-calendar.exe'" | Where-Object {
                $_.ExecutablePath -ieq $connection.executable_path -and $_.CommandLine -notmatch '(?:^|\s)--mcp(?:\s|$)'
            })
            if(@($gui|Where-Object SessionId -ne $session).Count){throw 'Calendar GUI is in a different Windows session; leave existing data untouched.'}
        }
        $invoke={
            param($Tool,$Arguments)
            if($Tool -notin @('calendar_list','calendar_deleted','calendar_create','calendar_update')){throw 'Unsupported collector operation.'}
            $argFile=Join-Path ([IO.Path]::GetTempPath()) ('dot-calendar-collector-'+[guid]::NewGuid().ToString('N')+'.json')
            try {
                $json=if($null -eq $Arguments){'{}'}else{$Arguments|ConvertTo-Json -Depth 40 -Compress}
                [IO.File]::WriteAllText($argFile,$json,[Text.UTF8Encoding]::new($false))
                $result=& $HelperPath -Tool $Tool -ArgumentsFile $argFile
                return ($result|ConvertFrom-Json -Depth 40)
            } finally {
                if(Test-Path -LiteralPath $argFile){Remove-Item -LiteralPath $argFile -Force}
            }
        }.GetNewClosure()
        $report=Invoke-ContestImport -Batch $batch -StateDirectory $StateDirectory -CalendarInvoker $invoke -Mode $Mode
        if($Mode -eq 'Apply' -and -not $ReportFile){
            $ReportFile=Join-Path $StateDirectory ('logs\run-'+[datetime]::UtcNow.ToString('yyyyMMddTHHmmss')+'-'+[guid]::NewGuid().ToString('N')+'.json')
        }
    }
} catch {
    $report=[pscustomobject]@{schemaVersion=1;status='blocked';mode=$Mode;counts=[pscustomobject]@{created=0;updated=0;existing=0;pending=0;excluded=0;sourceErrors=1};error=$_.Exception.Message;calendarWriteClaimed=$false}
    $exitCode=2
}
if($ReportFile) {
    $reportPath=[IO.Path]::GetFullPath($ReportFile)
    [void][IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($reportPath))
    if(Test-Path -LiteralPath $reportPath){throw 'Refusing to overwrite an existing result log; choose a new report path.'}
    [IO.File]::WriteAllText($reportPath,($report|ConvertTo-Json -Depth 40),[Text.UTF8Encoding]::new($false))
}
$report|ConvertTo-Json -Depth 40
exit $exitCode
