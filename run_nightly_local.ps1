param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [object[]]$ForwardedArgs
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$runnerPath = Join-Path $PSScriptRoot "docs\acceptance\Run-LocalNightlyEquivalent.ps1"
if (-not (Test-Path $runnerPath)) {
    throw "Run-LocalNightlyEquivalent.ps1 not found at '$runnerPath'."
}

& pwsh -File $runnerPath @ForwardedArgs
exit $LASTEXITCODE
