param(
    [string]$CanonicalParityPath = "docs/acceptance/authz-conformance-parity.json",
    [string]$CapcompParityPath = "docs/acceptance/authz-conformance-parity-supp-capcomp.json",
    [string]$OutPath = "docs/acceptance/promotion-readiness.json"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Read-JsonFileOrNull {
    param([string]$Path)

    if ([string]::IsNullOrWhiteSpace($Path)) {
        return $null
    }

    if (-not (Test-Path $Path)) {
        return $null
    }

    return (Get-Content -Raw -Path $Path | ConvertFrom-Json)
}

function Get-StringOrNull {
    param([object]$Value)

    if ($null -eq $Value) {
        return $null
    }

    $s = [string]$Value
    if ([string]::IsNullOrWhiteSpace($s)) {
        return $null
    }

    return $s
}

$canonical = Read-JsonFileOrNull -Path $CanonicalParityPath
$capcomp = Read-JsonFileOrNull -Path $CapcompParityPath

$canonicalBenchmarkStatus = "not_evaluated"
if ($null -ne $canonical -and $null -ne $canonical.auth_path_benchmark_result -and $null -ne $canonical.auth_path_benchmark_result.status) {
    $canonicalBenchmarkStatus = [string]$canonical.auth_path_benchmark_result.status
}

$capcompPromotionSignal = "not_evaluated"
if ($null -ne $capcomp -and $null -ne $capcomp.capcomp -and $null -ne $capcomp.capcomp.promotion_signal) {
    $capcompPromotionSignal = [string]$capcomp.capcomp.promotion_signal
}

$capcompStreakEvent = if ($capcompPromotionSignal -eq "supplemental_pass") {
    "pass"
} elseif ($capcompPromotionSignal -eq "supplemental_fail") {
    "fail"
} else {
    "not_evaluated"
}

$benchmarkStreakEvent = if ($canonicalBenchmarkStatus -eq "pass") {
    "pass"
} elseif ($canonicalBenchmarkStatus -eq "fail") {
    "fail"
} else {
    "not_evaluated"
}

$overallSignal = if ($capcompStreakEvent -eq "pass" -and $benchmarkStreakEvent -eq "pass") {
    "ready_for_streak_increment"
} elseif ($capcompStreakEvent -eq "not_evaluated" -and $benchmarkStreakEvent -eq "not_evaluated") {
    "not_evaluated"
} else {
    "blocked"
}

$gateVerdict = if ($capcompStreakEvent -eq "pass" -and $benchmarkStreakEvent -eq "pass") {
    "PASS_FOR_STREAK"
} elseif ($capcompStreakEvent -eq "fail" -or $benchmarkStreakEvent -eq "fail") {
    "FAIL"
} else {
    "NOT_EVALUATED"
}

$result = [ordered]@{
    generated_utc = [DateTime]::UtcNow.ToString("o")
    sources = [ordered]@{
        canonical_parity_path = $CanonicalParityPath
        canonical_parity_found = ($null -ne $canonical)
        capcomp_parity_path = $CapcompParityPath
        capcomp_parity_found = ($null -ne $capcomp)
    }
    run = [ordered]@{
        canonical_run_id = if ($null -ne $canonical) { Get-StringOrNull -Value $canonical.run_id } else { $null }
        capcomp_run_id = if ($null -ne $capcomp) { Get-StringOrNull -Value $capcomp.run_id } else { $null }
        canonical_git_commit = if ($null -ne $canonical) { Get-StringOrNull -Value $canonical.git_commit } else { $null }
        capcomp_git_commit = if ($null -ne $capcomp) { Get-StringOrNull -Value $capcomp.git_commit } else { $null }
    }
    capcomp = [ordered]@{
        promotion_signal = $capcompPromotionSignal
        streak_event = $capcompStreakEvent
        parity_status = if ($null -ne $capcomp -and $null -ne $capcomp.parity) { Get-StringOrNull -Value $capcomp.parity.status } else { $null }
        vector_count = if ($null -ne $capcomp -and $null -ne $capcomp.capcomp) { $capcomp.capcomp.vector_count } else { $null }
        parity_mismatch_count = if ($null -ne $capcomp -and $null -ne $capcomp.capcomp) { $capcomp.capcomp.parity_mismatch_count } else { $null }
        rust_fail_count = if ($null -ne $capcomp -and $null -ne $capcomp.capcomp -and $null -ne $capcomp.capcomp.rust) { $capcomp.capcomp.rust.fail_count } else { $null }
        dotnet_fail_count = if ($null -ne $capcomp -and $null -ne $capcomp.capcomp -and $null -ne $capcomp.capcomp.dotnet) { $capcomp.capcomp.dotnet.fail_count } else { $null }
    }
    benchmark = [ordered]@{
        status = $canonicalBenchmarkStatus
        streak_event = $benchmarkStreakEvent
        source = if ($null -ne $canonical -and $null -ne $canonical.auth_path_benchmark_result) { Get-StringOrNull -Value $canonical.auth_path_benchmark_result.source } else { $null }
        baseline_run_id = if ($null -ne $canonical -and $null -ne $canonical.auth_path_benchmark_result) { Get-StringOrNull -Value $canonical.auth_path_benchmark_result.baseline_run_id } else { $null }
        candidate_run_id = if ($null -ne $canonical -and $null -ne $canonical.auth_path_benchmark_result) { Get-StringOrNull -Value $canonical.auth_path_benchmark_result.candidate_run_id } else { $null }
        p50_latency_regression_pct = if ($null -ne $canonical -and $null -ne $canonical.auth_path_benchmark_result) { $canonical.auth_path_benchmark_result.p50_latency_regression_pct } else { $null }
        p95_latency_regression_pct = if ($null -ne $canonical -and $null -ne $canonical.auth_path_benchmark_result) { $canonical.auth_path_benchmark_result.p95_latency_regression_pct } else { $null }
        alloc_regression_pct = if ($null -ne $canonical -and $null -ne $canonical.auth_path_benchmark_result) { $canonical.auth_path_benchmark_result.alloc_regression_pct } else { $null }
        thresholds = [ordered]@{
            p50_latency_regression_pct_max = if ($null -ne $canonical -and $null -ne $canonical.auth_path_benchmark_thresholds) { $canonical.auth_path_benchmark_thresholds.p50_latency_regression_pct_max } else { $null }
            p95_latency_regression_pct_max = if ($null -ne $canonical -and $null -ne $canonical.auth_path_benchmark_thresholds) { $canonical.auth_path_benchmark_thresholds.p95_latency_regression_pct_max } else { $null }
            alloc_regression_pct_max = if ($null -ne $canonical -and $null -ne $canonical.auth_path_benchmark_thresholds) { $canonical.auth_path_benchmark_thresholds.alloc_regression_pct_max } else { $null }
        }
    }
    streak = [ordered]@{
        capcomp_increment_eligible = ($capcompStreakEvent -eq "pass")
        benchmark_increment_eligible = ($benchmarkStreakEvent -eq "pass")
        overall_signal = $overallSignal
        gate_verdict = $gateVerdict
        required_consecutive_runs = 14
        next_action = if ($gateVerdict -eq "PASS_FOR_STREAK") {
            "increment_capcomp_and_benchmark_streak_counters"
        } elseif ($gateVerdict -eq "FAIL") {
            "inspect_capcomp_or_benchmark_failures"
        } else {
            "collect_missing_evidence_inputs"
        }
    }
}

$outDir = Split-Path -Parent $OutPath
if (-not [string]::IsNullOrWhiteSpace($outDir) -and -not (Test-Path $outDir)) {
    New-Item -ItemType Directory -Path $outDir -Force | Out-Null
}

$result | ConvertTo-Json -Depth 12 | Set-Content -Path $OutPath -Encoding UTF8
Write-Host "Wrote promotion readiness artifact: $OutPath"
