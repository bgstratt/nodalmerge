param(
    [string]$OutPath = "docs/acceptance/authz-auth-path-benchmark-result.json",
    [string]$BaselineRunId,
    [string]$CandidateRunId,
    [string]$BaselineBenchmarkJsonPath = "",
    [string]$CandidateBenchmarkJsonPath = "",
    [string]$BenchmarkTarget = "dotnet-host-runtime",
    [switch]$AssumeZeroAllocWhenMissing,

    [double]$P50LatencyRegressionPct = [double]::NaN,
    [double]$P95LatencyRegressionPct = [double]::NaN,
    [double]$AllocRegressionPct = [double]::NaN,

    [double]$BaselineP50LatencyMs = [double]::NaN,
    [double]$CandidateP50LatencyMs = [double]::NaN,
    [double]$BaselineP95LatencyMs = [double]::NaN,
    [double]$CandidateP95LatencyMs = [double]::NaN,
    [double]$BaselineAllocBytes = [double]::NaN,
    [double]$CandidateAllocBytes = [double]::NaN
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function GetRegressionPercent {
    param(
        [double]$Baseline,
        [double]$Candidate,
        [string]$Name
    )

    if ([double]::IsNaN($Baseline) -or [double]::IsNaN($Candidate)) {
        return [double]::NaN
    }

    if ($Baseline -eq 0) {
        if ($Candidate -eq 0) {
            return 0.0
        }
        throw "Cannot compute regression percentage for '$Name' when baseline is zero."
    }

    return [math]::Round((($Candidate - $Baseline) / $Baseline) * 100, 4)
}

function Resolve-BenchmarkTargetMetrics {
    param(
        [string]$Path,
        [string]$Target,
        [switch]$AllowMissingAlloc
    )

    if ([string]::IsNullOrWhiteSpace($Path)) {
        return $null
    }

    if (-not (Test-Path $Path)) {
        throw "Benchmark JSON file not found: $Path"
    }

    $parsed = Get-Content -Raw -Path $Path | ConvertFrom-Json
    $records = @()

    if ($parsed -is [System.Array]) {
        $records = @($parsed)
    }
    elseif ($null -ne $parsed.results) {
        $records = @($parsed.results)
    }
    else {
        throw "Unsupported benchmark JSON format in '$Path'."
    }

    $entry = $null
    foreach ($record in $records) {
        if ([string]::Equals([string]$record.target, $Target, [System.StringComparison]::OrdinalIgnoreCase)) {
            if ([string]::Equals([string]$record.status, "ok", [System.StringComparison]::OrdinalIgnoreCase)) {
                $entry = $record
                break
            }
        }
    }

    if ($null -eq $entry) {
        throw "No status=ok benchmark entry found for target '$Target' in '$Path'."
    }

    $p50 = [double]::NaN
    $p95 = [double]::NaN
    $alloc = [double]::NaN

    if ($null -ne $entry.p50_ms) {
        $p50 = [double]$entry.p50_ms
    }
    if ($null -ne $entry.p95_ms) {
        $p95 = [double]$entry.p95_ms
    }

    if ($entry.PSObject.Properties.Name -contains "alloc_bytes" -and $null -ne $entry.alloc_bytes) {
        $alloc = [double]$entry.alloc_bytes
    }
    elseif ($entry.PSObject.Properties.Name -contains "alloc_avg_bytes" -and $null -ne $entry.alloc_avg_bytes) {
        $alloc = [double]$entry.alloc_avg_bytes
    }
    elseif ($AllowMissingAlloc) {
        $alloc = 0.0
    }

    if ([double]::IsNaN($p50) -or [double]::IsNaN($p95)) {
        throw "Target '$Target' in '$Path' is missing required p50_ms/p95_ms metrics."
    }

    return [ordered]@{
        p50_ms = $p50
        p95_ms = $p95
        alloc_bytes = $alloc
    }
}

if ([string]::IsNullOrWhiteSpace($BaselineRunId)) {
    throw "BaselineRunId is required."
}

if ([string]::IsNullOrWhiteSpace($CandidateRunId)) {
    throw "CandidateRunId is required."
}

$baselineMetrics = Resolve-BenchmarkTargetMetrics -Path $BaselineBenchmarkJsonPath -Target $BenchmarkTarget -AllowMissingAlloc:$AssumeZeroAllocWhenMissing
$candidateMetrics = Resolve-BenchmarkTargetMetrics -Path $CandidateBenchmarkJsonPath -Target $BenchmarkTarget -AllowMissingAlloc:$AssumeZeroAllocWhenMissing

if ($null -ne $baselineMetrics) {
    if ([double]::IsNaN($BaselineP50LatencyMs)) {
        $BaselineP50LatencyMs = [double]$baselineMetrics.p50_ms
    }
    if ([double]::IsNaN($BaselineP95LatencyMs)) {
        $BaselineP95LatencyMs = [double]$baselineMetrics.p95_ms
    }
    if ([double]::IsNaN($BaselineAllocBytes)) {
        $BaselineAllocBytes = [double]$baselineMetrics.alloc_bytes
    }
}

if ($null -ne $candidateMetrics) {
    if ([double]::IsNaN($CandidateP50LatencyMs)) {
        $CandidateP50LatencyMs = [double]$candidateMetrics.p50_ms
    }
    if ([double]::IsNaN($CandidateP95LatencyMs)) {
        $CandidateP95LatencyMs = [double]$candidateMetrics.p95_ms
    }
    if ([double]::IsNaN($CandidateAllocBytes)) {
        $CandidateAllocBytes = [double]$candidateMetrics.alloc_bytes
    }
}

$computedP50 = GetRegressionPercent -Baseline $BaselineP50LatencyMs -Candidate $CandidateP50LatencyMs -Name "p50_latency"
$computedP95 = GetRegressionPercent -Baseline $BaselineP95LatencyMs -Candidate $CandidateP95LatencyMs -Name "p95_latency"
$computedAlloc = GetRegressionPercent -Baseline $BaselineAllocBytes -Candidate $CandidateAllocBytes -Name "alloc"

if ([double]::IsNaN($P50LatencyRegressionPct) -and (-not [double]::IsNaN($computedP50))) {
    $P50LatencyRegressionPct = $computedP50
}
if ([double]::IsNaN($P95LatencyRegressionPct) -and (-not [double]::IsNaN($computedP95))) {
    $P95LatencyRegressionPct = $computedP95
}
if ([double]::IsNaN($AllocRegressionPct) -and (-not [double]::IsNaN($computedAlloc))) {
    $AllocRegressionPct = $computedAlloc
}

if ([double]::IsNaN($P50LatencyRegressionPct) -or [double]::IsNaN($P95LatencyRegressionPct) -or [double]::IsNaN($AllocRegressionPct)) {
    throw "Provide either explicit regression percentages or baseline/candidate metric pairs for all benchmark dimensions (p50, p95, alloc)."
}

$mode = if ((-not [double]::IsNaN($computedP50)) -or (-not [double]::IsNaN($computedP95)) -or (-not [double]::IsNaN($computedAlloc))) {
    "computed"
} else {
    "explicit"
}

$result = [ordered]@{
    baseline_run_id = $BaselineRunId
    candidate_run_id = $CandidateRunId
    p50_latency_regression_pct = [math]::Round($P50LatencyRegressionPct, 4)
    p95_latency_regression_pct = [math]::Round($P95LatencyRegressionPct, 4)
    alloc_regression_pct = [math]::Round($AllocRegressionPct, 4)
    metric_mode = $mode
    benchmark_target = $BenchmarkTarget
    benchmark_source = if (($null -ne $baselineMetrics) -or ($null -ne $candidateMetrics)) { "json" } else { "manual" }
    utc_timestamp = [DateTime]::UtcNow.ToString("o")
}

$outDir = Split-Path -Parent $OutPath
if (-not [string]::IsNullOrWhiteSpace($outDir) -and -not (Test-Path $outDir)) {
    New-Item -ItemType Directory -Path $outDir -Force | Out-Null
}

$result | ConvertTo-Json -Depth 8 | Set-Content -Path $OutPath -Encoding UTF8
Write-Host "Wrote benchmark evidence file: $OutPath"
