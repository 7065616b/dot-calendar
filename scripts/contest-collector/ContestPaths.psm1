Set-StrictMode -Version Latest

function Assert-ContestSafePath {
    param([Parameter(Mandatory)][string]$Path)
    if(-not [IO.Path]::IsPathFullyQualified($Path)){throw 'Collector path must be an explicit absolute path.'}
    $part=[IO.Path]::GetFullPath($Path)
    while($part){
        try{
            $item=Get-Item -LiteralPath $part -Force -ErrorAction Stop
            if($item.Attributes -band [IO.FileAttributes]::ReparsePoint){throw ('Collector reparse path refused: '+$part)}
        }catch [System.Management.Automation.ItemNotFoundException]{}
        $part=[IO.Path]::GetDirectoryName($part)
    }
}

Export-ModuleMember -Function Assert-ContestSafePath
