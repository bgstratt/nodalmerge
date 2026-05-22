param(
    [string]$RunId,
    [string]$OutPath = "docs/acceptance/snapshot-restore-drill.json"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Resolve-CargoCommand {
    $cargo = Get-Command cargo -ErrorAction SilentlyContinue
    if ($cargo) {
        return $cargo.Source
    }

    $userCargo = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"
    if (Test-Path $userCargo) {
        return $userCargo
    }

    return $null
}

if ([string]::IsNullOrWhiteSpace($RunId)) {
    $RunId = [DateTime]::UtcNow.ToString("yyyyMMdd-HHmmss")
}

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$cargoCmd = Resolve-CargoCommand
if ([string]::IsNullOrWhiteSpace($cargoCmd)) {
    throw "cargo executable not found. Install Rust toolchain or add cargo to PATH."
}

$checks = @(
    [ordered]@{
        name = "snapshot_restore_rebuild"
        description = "rebuild_from_snapshot restores state and supports forward replay delta"
        test_filter = "compaction::tests::rebuild_restores_state"
    },
    [ordered]@{
        name = "snapshot_hash_equality"
        description = "snapshot hash equals replay hash over the same logical history"
        test_filter = "compaction::tests::snapshot_hash_matches_replay"
    }
)

$results = @()

Push-Location $repoRoot
try {
    foreach ($check in $checks) {
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $output = & $cargoCmd test -p activesync-core $check.test_filter -- --exact --nocapture 2>&1 | Out-String
        $exit = $LASTEXITCODE
        $sw.Stop()

        $results += [ordered]@{
            name = [string]$check.name
            description = [string]$check.description
            test_filter = [string]$check.test_filter
            status = if ($exit -eq 0) { "pass" } else { "fail" }
            duration_seconds = [math]::Round($sw.Elapsed.TotalSeconds, 3)
            output_excerpt = (($output -split "`r?`n") | Select-Object -First 30) -join "`n"
        }

        if ($exit -ne 0) {
            throw "Snapshot restore drill check failed: $($check.name)"
        }
    }
}
finally {
    Pop-Location
}

$allPassed = @($results | Where-Object { [string]$_.status -ne "pass" }).Count -eq 0

$artifact = [ordered]@{
    run_id = $RunId
    status = if ($allPassed) { "pass" } else { "fail" }
    utc_timestamp = [DateTime]::UtcNow.ToString("o")
    checks = $results
    assertions = [ordered]@{
        snapshot_restore_forward_replay = [bool]($results | Where-Object { $_.name -eq "snapshot_restore_rebuild" -and $_.status -eq "pass" })
        snapshot_hash_equality = [bool]($results | Where-Object { $_.name -eq "snapshot_hash_equality" -and $_.status -eq "pass" })
    }
    notes = "Phase B deterministic snapshot restore drill artifact"
}

$outDir = Split-Path -Parent $OutPath
if (-not [string]::IsNullOrWhiteSpace($outDir) -and -not (Test-Path $outDir)) {
    New-Item -ItemType Directory -Path $outDir -Force | Out-Null
}

$artifact | ConvertTo-Json -Depth 8 | Set-Content -Path $OutPath -Encoding UTF8
Write-Host "Wrote snapshot restore drill artifact: $OutPath"

if (-not $allPassed) {
    throw "Snapshot restore drill failed"
}
