$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $projectRoot
if (Test-Path -LiteralPath "$projectRoot\.tools\cargo\bin\cargo.exe") {
    $env:CARGO_HOME = "$projectRoot\.tools\cargo"
    $env:RUSTUP_HOME = "$projectRoot\.tools\rustup"
    $env:PATH = "$env:CARGO_HOME\bin;$env:PATH"
    $llvmBin = "$projectRoot\.tools\llvm-mingw-20260922-msvcrt-x86_64\bin"
    if (Test-Path -LiteralPath "$llvmBin\llvm-dlltool.exe") {
        $env:PATH = "$llvmBin;$env:PATH"
        $env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = "$llvmBin\x86_64-w64-mingw32-gcc.exe"
        $env:RUSTFLAGS = '-C link-self-contained=yes'
    }
}
