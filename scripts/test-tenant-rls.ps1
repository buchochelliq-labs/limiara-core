$ErrorActionPreference = "Stop"

$repositoryRoot = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $repositoryRoot

$git = Get-Command git -ErrorAction SilentlyContinue
$gitBash = if ($null -ne $git) {
    Join-Path (Split-Path -Parent (Split-Path -Parent $git.Source)) "bin\bash.exe"
}
else {
    $null
}

if ($null -ne $gitBash -and (Test-Path -LiteralPath $gitBash)) {
    & $gitBash scripts/test-tenant-rls.sh
}
else {
    & bash scripts/test-tenant-rls.sh
}
if ($LASTEXITCODE -ne 0) {
    throw "Tenant RLS harness failed with exit code $LASTEXITCODE"
}
