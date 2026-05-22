param(
    [string]$RunId,
    [string]$OutputTag,
    [switch]$SkipSupplementalProfiles,
    [switch]$SkipCapCompSupplemental,
    [switch]$SkipSnapshotDrill,
    [switch]$SkipPromotionSummary,
    [string]$AuthPathBenchmarkResultPath,
    [string]$AuthPathBenchmarkBaselineRunId,
    [string]$AuthPathBenchmarkCandidateRunId,
    [string]$AuthPathBenchmarkBaselineJsonPath,
    [string]$AuthPathBenchmarkCandidateJsonPath,
    [string]$AuthPathBenchmarkTarget = "dotnet-host-runtime",
    [switch]$AuthPathBenchmarkAssumeZeroAllocWhenMissing,
    [double]$AuthPathBenchmarkP50LatencyRegressionPct = [double]::NaN,
    [double]$AuthPathBenchmarkP95LatencyRegressionPct = [double]::NaN,
    [double]$AuthPathBenchmarkAllocRegressionPct = [double]::NaN,
    [double]$AuthPathP50LatencyRegressionPctMax = 5,
    [double]$AuthPathP95LatencyRegressionPctMax = 10,
    [double]$AuthPathAllocRegressionPctMax = 10
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Resolve-Path (Join-Path $scriptDir "..\..")

if ([string]::IsNullOrWhiteSpace($RunId)) {
    $RunId = "local-nightly-" + [DateTime]::UtcNow.ToString("yyyyMMdd-HHmmss")
}

if ([string]::IsNullOrWhiteSpace($OutputTag)) {
    $OutputTag = [DateTime]::UtcNow.ToString("yyyyMMdd-HHmmss")
}

$summaryPath = Join-Path $scriptDir ("local-nightly-equivalent-{0}.json" -f $OutputTag)
$snapshotOutPath = Join-Path $scriptDir ("snapshot-restore-drill-local-{0}.json" -f $OutputTag)
$promotionOutPath = Join-Path $scriptDir ("promotion-readiness-local-{0}.json" -f $OutputTag)
$benchmarkOutPath = Join-Path $scriptDir ("authz-auth-path-benchmark-result-local-{0}.json" -f $OutputTag)
$historyDir = Join-Path $scriptDir "_history"
$historyPath = Join-Path $historyDir "promotion-readiness-history.jsonl"
$historySummaryPath = Join-Path $historyDir "promotion-readiness-history-summary.json"

$steps = New-Object System.Collections.Generic.List[object]
$effectiveBenchmarkResultPath = $AuthPathBenchmarkResultPath

function Invoke-Step {
    param(
        [string]$Name,
        [scriptblock]$Body
    )

    Write-Host "==> $Name"
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    try {
        & $Body
        $sw.Stop()
        $steps.Add([ordered]@{
            name = $Name
            status = "pass"
            duration_seconds = [math]::Round($sw.Elapsed.TotalSeconds, 3)
        })
    }
    catch {
        $sw.Stop()
        $steps.Add([ordered]@{
            name = $Name
            status = "fail"
            duration_seconds = [math]::Round($sw.Elapsed.TotalSeconds, 3)
            error = $_.Exception.Message
        })
        throw
    }
}

function Build-AuthzPwshParams {
    param(
        [string]$StepRunId,
        [string]$PolicyChannelMode,
        [string]$StepOutputTag,
        [switch]$IncludeCapCompSupplemental,
        [switch]$NoFailOnParityMismatch
    )

    $authzParams = @(
        "-File", ".\docs\acceptance\Run-AuthzConformance.ps1",
        "-RunId", $StepRunId,
        "-PolicyChannelMode", $PolicyChannelMode
    )

    if (-not [string]::IsNullOrWhiteSpace($StepOutputTag)) {
        $authzParams += @("-OutputTag", $StepOutputTag)
    }

    if ($IncludeCapCompSupplemental) {
        $authzParams += "-IncludeCapCompSupplemental"
    }

    if ($NoFailOnParityMismatch) {
        $authzParams += "-NoFailOnParityMismatch"
    }

    if (-not [string]::IsNullOrWhiteSpace($effectiveBenchmarkResultPath)) {
        $authzParams += @("-AuthPathBenchmarkResultPath", $effectiveBenchmarkResultPath)
    }

    if (-not [string]::IsNullOrWhiteSpace($AuthPathBenchmarkBaselineRunId)) {
        $authzParams += @("-AuthPathBenchmarkBaselineRunId", $AuthPathBenchmarkBaselineRunId)
    }

    if (-not [string]::IsNullOrWhiteSpace($AuthPathBenchmarkCandidateRunId)) {
        $authzParams += @("-AuthPathBenchmarkCandidateRunId", $AuthPathBenchmarkCandidateRunId)
    }

    if (-not [double]::IsNaN($AuthPathBenchmarkP50LatencyRegressionPct)) {
        $authzParams += @("-AuthPathBenchmarkP50LatencyRegressionPct", [string]$AuthPathBenchmarkP50LatencyRegressionPct)
    }

    if (-not [double]::IsNaN($AuthPathBenchmarkP95LatencyRegressionPct)) {
        $authzParams += @("-AuthPathBenchmarkP95LatencyRegressionPct", [string]$AuthPathBenchmarkP95LatencyRegressionPct)
    }

    if (-not [double]::IsNaN($AuthPathBenchmarkAllocRegressionPct)) {
        $authzParams += @("-AuthPathBenchmarkAllocRegressionPct", [string]$AuthPathBenchmarkAllocRegressionPct)
    }

    $authzParams += @("-AuthPathP50LatencyRegressionPctMax", [string]$AuthPathP50LatencyRegressionPctMax)
    $authzParams += @("-AuthPathP95LatencyRegressionPctMax", [string]$AuthPathP95LatencyRegressionPctMax)
    $authzParams += @("-AuthPathAllocRegressionPctMax", [string]$AuthPathAllocRegressionPctMax)

    return ,$authzParams
}

Push-Location $repoRoot
try {
    $canGenerateBenchmarkEvidence =
        (-not [string]::IsNullOrWhiteSpace($AuthPathBenchmarkBaselineRunId)) -and
        (-not [string]::IsNullOrWhiteSpace($AuthPathBenchmarkCandidateRunId)) -and
        (
            ((-not [string]::IsNullOrWhiteSpace($AuthPathBenchmarkBaselineJsonPath)) -and (-not [string]::IsNullOrWhiteSpace($AuthPathBenchmarkCandidateJsonPath))) -or
            ((-not [double]::IsNaN($AuthPathBenchmarkP50LatencyRegressionPct)) -and (-not [double]::IsNaN($AuthPathBenchmarkP95LatencyRegressionPct)) -and (-not [double]::IsNaN($AuthPathBenchmarkAllocRegressionPct)))
        )

    if ([string]::IsNullOrWhiteSpace($effectiveBenchmarkResultPath) -and $canGenerateBenchmarkEvidence) {
        Invoke-Step -Name "benchmark evidence generation" -Body {
            $benchmarkParams = @(
                "-File", ".\docs\acceptance\Write-AuthPathBenchmarkResult.ps1",
                "-OutPath", ("docs/acceptance/{0}" -f (Split-Path -Leaf $benchmarkOutPath)),
                "-BaselineRunId", $AuthPathBenchmarkBaselineRunId,
                "-CandidateRunId", $AuthPathBenchmarkCandidateRunId,
                "-BenchmarkTarget", $AuthPathBenchmarkTarget
            )

            if (-not [string]::IsNullOrWhiteSpace($AuthPathBenchmarkBaselineJsonPath)) {
                $benchmarkParams += @("-BaselineBenchmarkJsonPath", $AuthPathBenchmarkBaselineJsonPath)
            }
            if (-not [string]::IsNullOrWhiteSpace($AuthPathBenchmarkCandidateJsonPath)) {
                $benchmarkParams += @("-CandidateBenchmarkJsonPath", $AuthPathBenchmarkCandidateJsonPath)
            }
            if ($AuthPathBenchmarkAssumeZeroAllocWhenMissing) {
                $benchmarkParams += "-AssumeZeroAllocWhenMissing"
            }
            if (-not [double]::IsNaN($AuthPathBenchmarkP50LatencyRegressionPct)) {
                $benchmarkParams += @("-P50LatencyRegressionPct", [string]$AuthPathBenchmarkP50LatencyRegressionPct)
            }
            if (-not [double]::IsNaN($AuthPathBenchmarkP95LatencyRegressionPct)) {
                $benchmarkParams += @("-P95LatencyRegressionPct", [string]$AuthPathBenchmarkP95LatencyRegressionPct)
            }
            if (-not [double]::IsNaN($AuthPathBenchmarkAllocRegressionPct)) {
                $benchmarkParams += @("-AllocRegressionPct", [string]$AuthPathBenchmarkAllocRegressionPct)
            }

            & pwsh @benchmarkParams
            if ($LASTEXITCODE -ne 0) {
                throw "Write-AuthPathBenchmarkResult.ps1 failed with exit code $LASTEXITCODE"
            }
            $script:effectiveBenchmarkResultPath = "docs/acceptance/{0}" -f (Split-Path -Leaf $benchmarkOutPath)
        }
    }

    Invoke-Step -Name "canonical admin_command_only" -Body {
        $authzParams = Build-AuthzPwshParams -StepRunId ("{0}-admin" -f $RunId) -PolicyChannelMode "admin_command_only"
        & pwsh @authzParams
        if ($LASTEXITCODE -ne 0) {
            throw "Run-AuthzConformance.ps1 failed for canonical run with exit code $LASTEXITCODE"
        }
    }

    if (-not $SkipSnapshotDrill) {
        Invoke-Step -Name "snapshot restore drill" -Body {
            & pwsh -File .\docs\acceptance\Run-SnapshotRestoreDrill.ps1 `
                -RunId ("{0}-snapshot" -f $RunId) `
                -OutPath ("docs/acceptance/{0}" -f (Split-Path -Leaf $snapshotOutPath))
            if ($LASTEXITCODE -ne 0) {
                throw "Run-SnapshotRestoreDrill.ps1 failed with exit code $LASTEXITCODE"
            }
        }
    }

    if (-not $SkipSupplementalProfiles) {
        Invoke-Step -Name "supplemental hybrid" -Body {
            $authzParams = Build-AuthzPwshParams -StepRunId ("{0}-supp-hybrid" -f $RunId) -PolicyChannelMode "hybrid" -StepOutputTag ("supp-hybrid-{0}" -f $OutputTag) -NoFailOnParityMismatch
            & pwsh @authzParams
            if ($LASTEXITCODE -ne 0) {
                throw "Run-AuthzConformance.ps1 failed for supplemental hybrid with exit code $LASTEXITCODE"
            }
        }

        Invoke-Step -Name "supplemental replicated_signed_ops_only" -Body {
            $authzParams = Build-AuthzPwshParams -StepRunId ("{0}-supp-repl" -f $RunId) -PolicyChannelMode "replicated_signed_ops_only" -StepOutputTag ("supp-repl-{0}" -f $OutputTag) -NoFailOnParityMismatch
            & pwsh @authzParams
            if ($LASTEXITCODE -ne 0) {
                throw "Run-AuthzConformance.ps1 failed for supplemental replicated_signed_ops_only with exit code $LASTEXITCODE"
            }
        }
    }

    if (-not $SkipCapCompSupplemental) {
        Invoke-Step -Name "supplemental capcomp admin_command_only" -Body {
            $authzParams = Build-AuthzPwshParams -StepRunId ("{0}-supp-capcomp" -f $RunId) -PolicyChannelMode "admin_command_only" -StepOutputTag ("supp-capcomp-{0}" -f $OutputTag) -IncludeCapCompSupplemental -NoFailOnParityMismatch
            & pwsh @authzParams
            if ($LASTEXITCODE -ne 0) {
                throw "Run-AuthzConformance.ps1 failed for supplemental capcomp with exit code $LASTEXITCODE"
            }
        }
    }

    if (-not $SkipPromotionSummary) {
        Invoke-Step -Name "promotion readiness summary" -Body {
            $canonicalParityPath = ".\docs\acceptance\authz-conformance-parity.json"
            $capcompParityPath = ".\docs\acceptance\authz-conformance-parity-supp-capcomp-{0}.json" -f $OutputTag
            if ($SkipCapCompSupplemental) {
                throw "promotion summary requires capcomp supplemental parity output; rerun without -SkipCapCompSupplemental"
            }

            & pwsh -File .\docs\acceptance\Summarize-AuthzPromotionReadiness.ps1 `
                -CanonicalParityPath $canonicalParityPath `
                -CapcompParityPath $capcompParityPath `
                -OutPath ("docs/acceptance/{0}" -f (Split-Path -Leaf $promotionOutPath))
            if ($LASTEXITCODE -ne 0) {
                throw "Summarize-AuthzPromotionReadiness.ps1 failed with exit code $LASTEXITCODE"
            }
        }

        Invoke-Step -Name "promotion readiness history append" -Body {
            New-Item -ItemType Directory -Path $historyDir -Force | Out-Null
            $workflowRunNumber = 1
            if (Test-Path $historyPath) {
                $existing = @(Get-Content -Path $historyPath | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
                $workflowRunNumber = $existing.Count + 1
            }

            & pwsh -File .\docs\acceptance\Append-AuthzPromotionReadinessHistory.ps1 `
                -SnapshotPath ("docs/acceptance/{0}" -f (Split-Path -Leaf $promotionOutPath)) `
                -HistoryPath ("docs/acceptance/_history/{0}" -f (Split-Path -Leaf $historyPath)) `
                -SummaryPath ("docs/acceptance/_history/{0}" -f (Split-Path -Leaf $historySummaryPath)) `
                -WorkflowRunId $RunId `
                -WorkflowRunNumber $workflowRunNumber
            if ($LASTEXITCODE -ne 0) {
                throw "Append-AuthzPromotionReadinessHistory.ps1 failed with exit code $LASTEXITCODE"
            }
        }
    }
}
finally {
    Pop-Location
}

$failedCount = @($steps | Where-Object { $_.status -ne "pass" }).Count
$summary = [ordered]@{
    run_id = $RunId
    output_tag = $OutputTag
    utc_timestamp = [DateTime]::UtcNow.ToString("o")
    status = if ($failedCount -eq 0) { "pass" } else { "fail" }
    failed_step_count = $failedCount
    artifacts = [ordered]@{
        canonical_parity = "docs/acceptance/authz-conformance-parity.json"
        benchmark_evidence = if ([string]::IsNullOrWhiteSpace($effectiveBenchmarkResultPath)) { $null } else { $effectiveBenchmarkResultPath }
        snapshot_drill = if ($SkipSnapshotDrill) { $null } else { "docs/acceptance/{0}" -f (Split-Path -Leaf $snapshotOutPath) }
        promotion_readiness = if ($SkipPromotionSummary) { $null } else { "docs/acceptance/{0}" -f (Split-Path -Leaf $promotionOutPath) }
        history_jsonl = if ($SkipPromotionSummary) { $null } else { "docs/acceptance/_history/{0}" -f (Split-Path -Leaf $historyPath) }
        history_summary = if ($SkipPromotionSummary) { $null } else { "docs/acceptance/_history/{0}" -f (Split-Path -Leaf $historySummaryPath) }
    }
    steps = $steps
}

$summary | ConvertTo-Json -Depth 6 | Set-Content -Path $summaryPath -Encoding UTF8
Write-Host "Local nightly-equivalent summary written: $summaryPath"

if ($failedCount -gt 0) {
    throw "local nightly-equivalent run failed ($failedCount step(s) failed)"
}
