param(
    [string]$SnapshotPath = "docs/acceptance/promotion-readiness.json",
    [string]$HistoryPath = "docs/acceptance/promotion-readiness-history.jsonl",
    [string]$SummaryPath = "docs/acceptance/promotion-readiness-history-summary.json",
    [string]$WorkflowRunId,
    [string]$WorkflowRunNumber
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not (Test-Path $SnapshotPath)) {
    throw "Promotion readiness snapshot not found: $SnapshotPath"
}

$snapshot = Get-Content -Raw -Path $SnapshotPath | ConvertFrom-Json

$record = [ordered]@{
    captured_utc = [DateTime]::UtcNow.ToString("o")
    workflow_run_id = if ([string]::IsNullOrWhiteSpace($WorkflowRunId)) { $null } else { $WorkflowRunId }
    workflow_run_number = if ([string]::IsNullOrWhiteSpace($WorkflowRunNumber)) { $null } else { $WorkflowRunNumber }
    overall_signal = if ($null -ne $snapshot.streak) { [string]$snapshot.streak.overall_signal } else { "not_evaluated" }
    capcomp_streak_event = if ($null -ne $snapshot.capcomp) { [string]$snapshot.capcomp.streak_event } else { "not_evaluated" }
    benchmark_streak_event = if ($null -ne $snapshot.benchmark) { [string]$snapshot.benchmark.streak_event } else { "not_evaluated" }
    capcomp_promotion_signal = if ($null -ne $snapshot.capcomp) { [string]$snapshot.capcomp.promotion_signal } else { "not_evaluated" }
    benchmark_status = if ($null -ne $snapshot.benchmark) { [string]$snapshot.benchmark.status } else { "not_evaluated" }
    gate_verdict = if ($null -ne $snapshot.streak -and $null -ne $snapshot.streak.gate_verdict) { [string]$snapshot.streak.gate_verdict } else { "NOT_EVALUATED" }
    canonical_run_id = if ($null -ne $snapshot.run) { [string]$snapshot.run.canonical_run_id } else { $null }
    capcomp_run_id = if ($null -ne $snapshot.run) { [string]$snapshot.run.capcomp_run_id } else { $null }
}

$historyDir = Split-Path -Parent $HistoryPath
if (-not [string]::IsNullOrWhiteSpace($historyDir) -and -not (Test-Path $historyDir)) {
    New-Item -ItemType Directory -Path $historyDir -Force | Out-Null
}

$existingRecords = @()
if (Test-Path $HistoryPath) {
    $lines = Get-Content -Path $HistoryPath | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    foreach ($line in $lines) {
        $existingRecords += ($line | ConvertFrom-Json)
    }
}

$dedupeByRunId = $null
if (-not [string]::IsNullOrWhiteSpace($WorkflowRunId)) {
    $dedupeByRunId = $WorkflowRunId
}

$alreadyPresent = $false
if ($null -ne $dedupeByRunId) {
    foreach ($r in $existingRecords) {
        if ($null -ne $r.workflow_run_id -and [string]$r.workflow_run_id -eq $dedupeByRunId) {
            $alreadyPresent = $true
            break
        }
    }
}

if (-not $alreadyPresent) {
    ($record | ConvertTo-Json -Compress) | Add-Content -Path $HistoryPath -Encoding UTF8
    $existingRecords += [pscustomobject]$record
}

$capcompConsecutive = 0
$benchmarkConsecutive = 0
$combinedConsecutive = 0
$gateVerdictConsecutive = 0

for ($i = $existingRecords.Count - 1; $i -ge 0; $i--) {
    $row = $existingRecords[$i]

    if ([string]$row.capcomp_streak_event -eq "pass") {
        $capcompConsecutive++
    } else {
        break
    }
}

for ($i = $existingRecords.Count - 1; $i -ge 0; $i--) {
    $row = $existingRecords[$i]

    if ([string]$row.benchmark_streak_event -eq "pass") {
        $benchmarkConsecutive++
    } else {
        break
    }
}

for ($i = $existingRecords.Count - 1; $i -ge 0; $i--) {
    $row = $existingRecords[$i]

    if ([string]$row.capcomp_streak_event -eq "pass" -and [string]$row.benchmark_streak_event -eq "pass") {
        $combinedConsecutive++
    } else {
        break
    }
}

for ($i = $existingRecords.Count - 1; $i -ge 0; $i--) {
    $row = $existingRecords[$i]

    if ([string]$row.gate_verdict -eq "PASS_FOR_STREAK") {
        $gateVerdictConsecutive++
    } else {
        break
    }
}

$summary = [ordered]@{
    generated_utc = [DateTime]::UtcNow.ToString("o")
    history_path = $HistoryPath
    total_records = $existingRecords.Count
    latest = if ($existingRecords.Count -gt 0) { $existingRecords[-1] } else { $null }
    streaks = [ordered]@{
        required_consecutive_runs = 14
        capcomp_consecutive_passes = $capcompConsecutive
        benchmark_consecutive_passes = $benchmarkConsecutive
        combined_consecutive_passes = $combinedConsecutive
        capcomp_ready = ($capcompConsecutive -ge 14)
        benchmark_ready = ($benchmarkConsecutive -ge 14)
        combined_ready = ($combinedConsecutive -ge 14)
        gate_verdict_consecutive_passes = $gateVerdictConsecutive
        gate_verdict_ready = ($gateVerdictConsecutive -ge 14)
    }
}

$summaryDir = Split-Path -Parent $SummaryPath
if (-not [string]::IsNullOrWhiteSpace($summaryDir) -and -not (Test-Path $summaryDir)) {
    New-Item -ItemType Directory -Path $summaryDir -Force | Out-Null
}

$summary | ConvertTo-Json -Depth 12 | Set-Content -Path $SummaryPath -Encoding UTF8

Write-Host "Updated promotion readiness history: $HistoryPath"
Write-Host "Updated promotion readiness history summary: $SummaryPath"
