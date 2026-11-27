$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

$compiler = (Get-Command ISCC.exe -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1).Source
if (-not $compiler) {
    $compiler = @(
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
        "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
    ) | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
}
if (-not $compiler) {
    throw 'Install Inno Setup 6 with "winget install --exact --id JRSoftware.InnoSetup --scope user", or add ISCC.exe to PATH.'
}

$metadata = cargo metadata --locked --no-deps --format-version 1
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$shell = ($metadata | ConvertFrom-Json).packages | Where-Object name -eq 'winsh'
& $compiler "/DAppVersion=$($shell.version)" 'windows\installer.iss'
exit $LASTEXITCODE
