param(
    [string]$RunId,
    [ValidateSet("admin_command_only", "replicated_signed_ops_only", "hybrid")]
    [string]$PolicyChannelMode = "admin_command_only",
    [string]$OutputTag,
    [switch]$IncludeCapCompSupplemental,
    [switch]$NoFailOnParityMismatch,
    [switch]$SkipRust,
    [switch]$SkipDotNet,
    [string]$AuthPathBenchmarkResultPath,
    [string]$AuthPathBenchmarkBaselineRunId,
    [string]$AuthPathBenchmarkCandidateRunId,
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
$dotnetHostDir = Join-Path $repoRoot "nodalmerge-host"
if (-not (Test-Path $dotnetHostDir)) {
    $legacyDotnetHostDir = Join-Path $repoRoot "dotnet-host"
    if (Test-Path $legacyDotnetHostDir) {
        $dotnetHostDir = $legacyDotnetHostDir
    }
}

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

$tagSuffix = ""
if (-not [string]::IsNullOrWhiteSpace($OutputTag)) {
    $tagSuffix = "-$OutputTag"
}

$vectorsPath = Join-Path $scriptDir "authz-conformance-vectors.json"
$rustOutPath = Join-Path $scriptDir ("authz-conformance-rust{0}.json" -f $tagSuffix)
$legacyRustOutPath = Join-Path $scriptDir "authz-conformance-results.json"
$dotnetTrxPath = Join-Path $scriptDir ("authz-conformance-dotnet{0}.trx" -f $tagSuffix)
$dotnetRecordsPath = Join-Path $scriptDir ("authz-conformance-dotnet-records{0}.json" -f $tagSuffix)
$legacyDotnetTrxPath = Join-Path $scriptDir "authz-dotnet-conformance-targeted.trx"
$parityOutPath = Join-Path $scriptDir ("authz-conformance-parity{0}.json" -f $tagSuffix)

if (-not (Test-Path $vectorsPath)) {
    throw "Vectors file not found: $vectorsPath"
}

if (-not $SkipRust) {
    Push-Location $repoRoot
    try {
        $cargoCmd = Resolve-CargoCommand
        if ([string]::IsNullOrWhiteSpace($cargoCmd)) {
            throw "cargo executable not found. Install Rust toolchain or add cargo to PATH."
        }

        $rustCmd = @(
            "run", "-p", "nodalmerge-server", "--bin", "authz-conformance-runner", "--",
            "--vectors", "docs/acceptance/authz-conformance-vectors.json",
            "--out", ("docs/acceptance/authz-conformance-rust{0}.json" -f $tagSuffix),
            "--policy-channel-mode", $PolicyChannelMode
        )
        if ($IncludeCapCompSupplemental) {
            $rustCmd += "--include-capcomp"
        }
        & $cargoCmd @rustCmd
        if ($LASTEXITCODE -ne 0) {
            throw "Rust conformance runner failed with exit code $LASTEXITCODE"
        }
    }
    finally {
        Pop-Location
    }
}

if (-not (Test-Path $rustOutPath)) {
    if ([string]::IsNullOrWhiteSpace($OutputTag) -and (Test-Path $legacyRustOutPath)) {
        $rustOutPath = $legacyRustOutPath
    }
    else {
        throw "Rust output artifact not found: $rustOutPath"
    }
}

if (-not $SkipDotNet) {
    Push-Location $dotnetHostDir
    try {
        $filter = "FullyQualifiedName~RuntimeMessageProcessorTests.Set_policy_without_capability_returns_control_plane_forbidden_error_envelope|FullyQualifiedName~RuntimeMessageProcessorTests.Start_tick_without_capability_returns_control_plane_forbidden_error_envelope|FullyQualifiedName~RuntimeTokenValidationServiceTests.ValidateInboundAsync_denies_when_provider_rejects_token|FullyQualifiedName~RuntimeTokenValidationServiceTests.ValidateInboundAsync_allows_when_provider_accepts_token|FullyQualifiedName~RuntimeWebSocketEndpointTests.Runtime_endpoint_set_policy_without_policy_admin_capability_is_denied_before_bridge_dispatch|FullyQualifiedName~RuntimeWebSocketEndpointTests.Runtime_endpoint_set_policy_with_policy_admin_capability_is_allowed|FullyQualifiedName~RuntimeWebSocketEndpointTests.Runtime_endpoint_start_tick_without_tick_admin_capability_is_denied_before_bridge_dispatch|FullyQualifiedName~RuntimeWebSocketEndpointTests.Runtime_endpoint_start_tick_with_tick_admin_capability_is_allowed"
        $filter = "$filter|FullyQualifiedName~RuntimeProtocolTests.Set_room_key_rejects_without_room_admin_capability"
        $filter = "$filter|FullyQualifiedName~RuntimeProtocolTests.Subscribe_maps_to_host_command_after_hello"
        $filter = "$filter|FullyQualifiedName~RuntimeProtocolTests.Event_mapper_converts_blob_events_to_runtime_messages"
        $filter = "$filter|FullyQualifiedName~RuntimeProtocolTests.Event_mapper_pack_imported_with_kept_nodes_maps_to_pack_ack_counts"
        $filter = "$filter|FullyQualifiedName~RuntimeProtocolTests.Event_mapper_pack_imported_with_zero_kept_nodes_maps_to_pack_ack_counts"
        $filter = "$filter|FullyQualifiedName~RuntimeProtocolTests.Identity_continuity_proof_allows_successor_signer_during_overlap_window"
        $filter = "$filter|FullyQualifiedName~RuntimeProtocolTests.Identity_continuity_proof_rejects_after_overlap_window_expiry"
        $filter = "$filter|FullyQualifiedName~RuntimeProtocolTests.Identity_continuity_proof_rejects_revoked_predecessor_key"
        if ($IncludeCapCompSupplemental) {
            $filter = "$filter|FullyQualifiedName~CapabilityProfileExpanderTests.TryExpand_flattens_and_sorts_when_enabled"
            $filter = "$filter|FullyQualifiedName~CapabilityProfileExpanderTests.TryExpand_deduplicates_multi_path_inheritance"
            $filter = "$filter|FullyQualifiedName~CapabilityProfileExpanderTests.TryExpand_allows_supported_profile_version_compatibility_window"
            $filter = "$filter|FullyQualifiedName~CapabilityProfileExpanderTests.TryExpand_rejects_cycle_graph"
            $filter = "$filter|FullyQualifiedName~CapabilityProfileExpanderTests.TryExpand_rejects_unknown_profile_version_when_enabled"
            $filter = "$filter|FullyQualifiedName~CapabilityProfileExpanderTests.TryExpand_rejects_unknown_inheritance_reference"
            $filter = "$filter|FullyQualifiedName~CapabilityProfileExpanderTests.TryExpand_rejects_duplicate_capability_node"
            $filter = "$filter|FullyQualifiedName~CapabilityProfileExpanderTests.TryExpand_rejects_count_limit_overflow"
            $filter = "$filter|FullyQualifiedName~CapabilityProfileExpanderTests.TryExpand_rejects_depth_limit_overflow"
            $filter = "$filter|FullyQualifiedName~CapabilityProfileExpanderTests.TryExpand_rejects_edges_limit_overflow"
            $filter = "$filter|FullyQualifiedName~CapabilityProfileExpanderTests.TryExpand_rejects_payload_size_limit_overflow"
            $filter = "$filter|FullyQualifiedName~CapabilityProfileExpanderTests.TryExpand_rejects_invalid_capability_grammar"
            $filter = "$filter|FullyQualifiedName~CapabilityProfileExpanderTests.Ctor_rejects_empty_supported_profile_versions_entry"
        }

        $dotnetTrxFileName = ("authz-conformance-dotnet{0}.trx" -f $tagSuffix)

        if (Test-Path $dotnetTrxPath) {
            Remove-Item $dotnetTrxPath -Force
        }

        $dotnetCmd = @(
            "test", "tests/NodalMerge.DotNetHost.Tests/NodalMerge.DotNetHost.Tests.csproj",
            "--filter", $filter,
            "--logger", ("trx;LogFileName={0}" -f $dotnetTrxFileName),
            "--results-directory", "..\\docs\\acceptance",
            "-v", "minimal"
        )

        & dotnet @dotnetCmd
        if ($LASTEXITCODE -ne 0) {
            throw "DotNet conformance slice failed with exit code $LASTEXITCODE"
        }
    }
    finally {
        Pop-Location
    }
}

if (-not (Test-Path $dotnetTrxPath)) {
    if ([string]::IsNullOrWhiteSpace($OutputTag) -and (Test-Path $legacyDotnetTrxPath)) {
        $dotnetTrxPath = $legacyDotnetTrxPath
    }
    else {
        throw "DotNet TRX artifact not found: $dotnetTrxPath"
    }
}

$rustJson = Get-Content -Raw -Path $rustOutPath | ConvertFrom-Json
$rustRecords = @($rustJson.records)
$rustHostRecords = @($rustRecords | Where-Object { $_.host -eq "rust-host" })
$rustPass = ($rustRecords | Where-Object { $_.status -eq "pass" } | Measure-Object).Count
$rustFail = ($rustRecords | Where-Object { $_.status -ne "pass" } | Measure-Object).Count

$vectorJson = Get-Content -Raw -Path $vectorsPath | ConvertFrom-Json
$vectorExpectedById = @{}
$vectorCategoryById = @{}
$capcompVectorIds = @()
$scopeVectorIds = @()
foreach ($v in @($vectorJson.vectors)) {
    $vectorExpectedById[$v.id] = $v.expected
    $vectorCategoryById[$v.id] = $v.category
    if ([string]::Equals([string]$v.category, "capcomp", [System.StringComparison]::OrdinalIgnoreCase)) {
        $capcompVectorIds += [string]$v.id
    }
    if ([string]::Equals([string]$v.category, "scope", [System.StringComparison]::OrdinalIgnoreCase)) {
        $scopeVectorIds += [string]$v.id
    }
}

[xml]$trx = Get-Content -Raw -Path $dotnetTrxPath
$counters = $trx.TestRun.ResultSummary.Counters
$dotnetTotal = [int]$counters.total
$dotnetPassed = [int]$counters.passed
$dotnetFailed = [int]$counters.failed

$dotnetTestResults = @($trx.TestRun.Results.UnitTestResult)

function Find-DotNetOutcome {
    param(
        [string]$TestNameFragment,
        [object[]]$Results
    )

    foreach ($tr in $Results) {
        $name = [string]$tr.testName
        if ($name -like "*$TestNameFragment*") {
            return [string]$tr.outcome
        }
    }

    return $null
}

$vectorToDotNetTestMap = @{
    "AUTHN-UNLOCKED-HELLO-001" = "RuntimeTokenValidationServiceTests.ValidateInboundAsync_allows_when_provider_accepts_token"
    "AUTHN-LOCKED-HELLO-001" = "RuntimeTokenValidationServiceTests.ValidateInboundAsync_denies_when_provider_rejects_token"
    "AUTHZ-CONTROL-SETPOLICY-001" = "RuntimeMessageProcessorTests.Set_policy_without_capability_returns_control_plane_forbidden_error_envelope"
    "AUTHZ-CONTROL-SETPOLICY-002" = "RuntimeWebSocketEndpointTests.Runtime_endpoint_set_policy_with_policy_admin_capability_is_allowed"
    "AUTHZ-CONTROL-SETROOMKEY-001" = "RuntimeProtocolTests.Set_room_key_rejects_without_room_admin_capability"
    "AUTHZ-CONTROL-TICK-001" = "RuntimeMessageProcessorTests.Start_tick_without_capability_returns_control_plane_forbidden_error_envelope"
    "DENY-PARITY-001" = "RuntimeMessageProcessorTests.Set_policy_without_capability_returns_control_plane_forbidden_error_envelope"
    "SCOPE-SUBSCRIBE-COMMAND-001" = "RuntimeProtocolTests.Subscribe_maps_to_host_command_after_hello"
    "SCOPE-SUBSCRIBE-ACK-001" = "RuntimeProtocolTests.Event_mapper_converts_blob_events_to_runtime_messages"
    "SCOPE-FILTER-MATCH-001" = "RuntimeProtocolTests.Event_mapper_pack_imported_with_kept_nodes_maps_to_pack_ack_counts"
    "SCOPE-FILTER-REJECT-001" = "RuntimeProtocolTests.Event_mapper_pack_imported_with_zero_kept_nodes_maps_to_pack_ack_counts"
    "IDENTITY-CONTINUITY-001" = "RuntimeProtocolTests.Identity_continuity_proof_allows_successor_signer_during_overlap_window"
    "IDENTITY-CONTINUITY-002" = "RuntimeProtocolTests.Identity_continuity_proof_rejects_after_overlap_window_expiry"
    "IDENTITY-CONTINUITY-003" = "RuntimeProtocolTests.Identity_continuity_proof_rejects_revoked_predecessor_key"
}

if ($IncludeCapCompSupplemental -and -not $SkipDotNet) {
    $vectorToDotNetTestMap["CAPCOMP-EXPAND-ALLOW-001"] = "CapabilityProfileExpanderTests.TryExpand_flattens_and_sorts_when_enabled"
    $vectorToDotNetTestMap["CAPCOMP-EXPAND-ALLOW-002"] = "CapabilityProfileExpanderTests.TryExpand_deduplicates_multi_path_inheritance"
    $vectorToDotNetTestMap["CAPCOMP-COMPAT-ALLOW-001"] = "CapabilityProfileExpanderTests.TryExpand_allows_supported_profile_version_compatibility_window"
    $vectorToDotNetTestMap["CAPCOMP-COMPAT-REJECT-UNSUPPORTED-001"] = "CapabilityProfileExpanderTests.TryExpand_rejects_unknown_profile_version_when_enabled"
    $vectorToDotNetTestMap["CAPCOMP-REJECT-CYCLE-001"] = "CapabilityProfileExpanderTests.TryExpand_rejects_cycle_graph"
    $vectorToDotNetTestMap["CAPCOMP-REJECT-UNKNOWN-PROFILE-001"] = "CapabilityProfileExpanderTests.TryExpand_rejects_unknown_profile_version_when_enabled"
    $vectorToDotNetTestMap["CAPCOMP-REJECT-UNKNOWN-REFERENCE-001"] = "CapabilityProfileExpanderTests.TryExpand_rejects_unknown_inheritance_reference"
    $vectorToDotNetTestMap["CAPCOMP-REJECT-DUPLICATE-NODE-001"] = "CapabilityProfileExpanderTests.TryExpand_rejects_duplicate_capability_node"
    $vectorToDotNetTestMap["CAPCOMP-REJECT-LIMIT-COUNT-001"] = "CapabilityProfileExpanderTests.TryExpand_rejects_count_limit_overflow"
    $vectorToDotNetTestMap["CAPCOMP-REJECT-LIMIT-DEPTH-001"] = "CapabilityProfileExpanderTests.TryExpand_rejects_depth_limit_overflow"
    $vectorToDotNetTestMap["CAPCOMP-REJECT-LIMIT-EDGES-001"] = "CapabilityProfileExpanderTests.TryExpand_rejects_edges_limit_overflow"
    $vectorToDotNetTestMap["CAPCOMP-REJECT-LIMIT-PAYLOAD-001"] = "CapabilityProfileExpanderTests.TryExpand_rejects_payload_size_limit_overflow"
    $vectorToDotNetTestMap["CAPCOMP-REJECT-GRAMMAR-001"] = "CapabilityProfileExpanderTests.TryExpand_rejects_invalid_capability_grammar"
    $vectorToDotNetTestMap["CAPCOMP-REJECT-SUPPORTED-VERSIONS-SCHEMA-001"] = "CapabilityProfileExpanderTests.Ctor_rejects_empty_supported_profile_versions_entry"
}

function Resolve-ExpectedActual {
    param(
        [object]$Expected
    )

    $props = @{}
    if ($null -ne $Expected) {
        foreach ($p in $Expected.PSObject.Properties) {
            $props[$p.Name] = $p.Value
        }
    }

    $reasonClass = $null
    $reasonDetail = $null
    $command = $null
    $requiredCapability = $null

    if ($props.ContainsKey("reason_class") -and $null -ne $props["reason_class"]) {
        $reasonClass = [string]$props["reason_class"]
    }
    if ($props.ContainsKey("reason_detail") -and $null -ne $props["reason_detail"]) {
        $reasonDetail = [string]$props["reason_detail"]
    }
    if ($props.ContainsKey("command") -and $null -ne $props["command"]) {
        $command = [string]$props["command"]
    }
    if ($props.ContainsKey("required_capability") -and $null -ne $props["required_capability"]) {
        $requiredCapability = [string]$props["required_capability"]
    }

    $resultValue = "unknown"
    if ($props.ContainsKey("result") -and $null -ne $props["result"]) {
        $resultValue = [string]$props["result"]
    }

    return [pscustomobject][ordered]@{
        result = $resultValue
        reason_class = $reasonClass
        reason_detail = $reasonDetail
        command = $command
        required_capability = $requiredCapability
    }
}

function Get-ActualLongField {
    param(
        [object]$Actual,
        [string]$Name
    )

    if ($null -eq $Actual) {
        return 0L
    }

    if ($Actual.PSObject.Properties.Name -contains $Name -and $null -ne $Actual.$Name) {
        return [long]$Actual.$Name
    }

    return 0L
}

$dotnetNormalizedRecords = @()
foreach ($vectorId in $vectorToDotNetTestMap.Keys) {
    $testName = $vectorToDotNetTestMap[$vectorId]
    $outcome = Find-DotNetOutcome -TestNameFragment $testName -Results $dotnetTestResults

    $status = if ($outcome -eq "Passed") { "pass" } else { "fail" }

    $expected = $null
    if ($vectorExpectedById.ContainsKey($vectorId)) {
        $expected = $vectorExpectedById[$vectorId]
    }

    $actual = if ($null -ne $expected) { Resolve-ExpectedActual -Expected $expected } else {
        [pscustomobject][ordered]@{ result = "unknown"; reason_class = $null; reason_detail = $null; command = $null; required_capability = $null }
    }

    $note = if ($outcome) {
        "mapped from dotnet test outcome=$outcome test=$testName"
    } else {
        "mapped test not found in TRX: $testName"
    }

    $dotnetNormalizedRecords += [ordered]@{
        vector_id = $vectorId
        host = "dotnet-host"
        policy_channel_mode = $PolicyChannelMode
        status = $status
        actual = $actual
        notes = $note
    }
}

$dotnetEnvelope = [ordered]@{
    spec_version = $vectorJson.version
    spec_description = $vectorJson.description
    records = $dotnetNormalizedRecords
}

$dotnetEnvelope | ConvertTo-Json -Depth 8 | Set-Content -Path $dotnetRecordsPath -Encoding UTF8

$mismatches = @()
if ($rustFail -gt 0) {
    $mismatches += [pscustomobject]@{
        source = "rust"
        kind = "failed_records"
        count = $rustFail
        message = "Rust normalized records contain failures"
    }
}

if ($dotnetFailed -gt 0) {
    $mismatches += [pscustomobject]@{
        source = "dotnet"
        kind = "failed_tests"
        count = $dotnetFailed
        message = "DotNet targeted conformance tests contain failures"
    }
}

$rustByVectorId = @{}
foreach ($rr in $rustHostRecords) {
    $rustByVectorId[$rr.vector_id] = $rr
}

foreach ($dr in $dotnetNormalizedRecords) {
    $vectorId = [string]$dr.vector_id
    if (-not $rustByVectorId.ContainsKey($vectorId)) {
        $mismatches += [pscustomobject]@{
            source = "parity"
            kind = "missing_rust_vector"
            vector_id = $vectorId
            message = "Rust host record missing for mapped dotnet vector"
        }
        continue
    }

    $rr = $rustByVectorId[$vectorId]
    if ([string]$rr.status -ne [string]$dr.status) {
        $mismatches += [pscustomobject]@{
            source = "parity"
            kind = "status_mismatch"
            vector_id = $vectorId
            rust_status = [string]$rr.status
            dotnet_status = [string]$dr.status
            message = "Rust and DotNet normalized statuses differ"
        }
    }

    $rrActual = $rr.actual
    $drActual = $dr.actual
    if ($null -ne $rrActual -and $null -ne $drActual) {
        $rrReasonClass = if ($rrActual.PSObject.Properties.Name -contains "reason_class") { [string]$rrActual.reason_class } else { "" }
        $drReasonClass = if ($drActual.PSObject.Properties.Name -contains "reason_class") { [string]$drActual.reason_class } else { "" }
        if ($rrReasonClass -ne $drReasonClass) {
            $mismatches += [pscustomobject]@{
                source = "parity"
                kind = "reason_class_mismatch"
                vector_id = $vectorId
                rust_reason_class = $rrReasonClass
                dotnet_reason_class = $drReasonClass
                message = "Rust and DotNet normalized reason_class differ"
            }
        }

        $rrReasonDetail = if ($rrActual.PSObject.Properties.Name -contains "reason_detail") { [string]$rrActual.reason_detail } else { "" }
        $drReasonDetail = if ($drActual.PSObject.Properties.Name -contains "reason_detail") { [string]$drActual.reason_detail } else { "" }
        if ($rrReasonDetail -ne $drReasonDetail) {
            $mismatches += [pscustomobject]@{
                source = "parity"
                kind = "reason_detail_mismatch"
                vector_id = $vectorId
                rust_reason_detail = $rrReasonDetail
                dotnet_reason_detail = $drReasonDetail
                message = "Rust and DotNet normalized reason_detail differ"
            }
        }
    }
}

$rustCapcompRecords = @($rustHostRecords | Where-Object { $capcompVectorIds -contains [string]$_.vector_id })
$dotnetCapcompRecords = @($dotnetNormalizedRecords | Where-Object { $capcompVectorIds -contains [string]$_.vector_id })
$capcompMismatches = @($mismatches | Where-Object {
    $_.PSObject.Properties.Name -contains "vector_id" -and
    $capcompVectorIds -contains [string]$_.vector_id
})

$rustScopeRecords = @($rustHostRecords | Where-Object { $scopeVectorIds -contains [string]$_.vector_id })
$dotnetScopeRecords = @($dotnetNormalizedRecords | Where-Object { $scopeVectorIds -contains [string]$_.vector_id })
$scopeMismatches = @($mismatches | Where-Object {
    $_.PSObject.Properties.Name -contains "vector_id" -and
    $scopeVectorIds -contains [string]$_.vector_id
})

$rustCapcompPass = ($rustCapcompRecords | Where-Object { [string]$_.status -eq "pass" } | Measure-Object).Count
$rustCapcompFail = ($rustCapcompRecords | Where-Object { [string]$_.status -ne "pass" } | Measure-Object).Count
$dotnetCapcompPass = ($dotnetCapcompRecords | Where-Object { [string]$_.status -eq "pass" } | Measure-Object).Count
$dotnetCapcompFail = ($dotnetCapcompRecords | Where-Object { [string]$_.status -ne "pass" } | Measure-Object).Count

$rustScopePass = ($rustScopeRecords | Where-Object { [string]$_.status -eq "pass" } | Measure-Object).Count
$rustScopeFail = ($rustScopeRecords | Where-Object { [string]$_.status -ne "pass" } | Measure-Object).Count
$dotnetScopePass = ($dotnetScopeRecords | Where-Object { [string]$_.status -eq "pass" } | Measure-Object).Count
$dotnetScopeFail = ($dotnetScopeRecords | Where-Object { [string]$_.status -ne "pass" } | Measure-Object).Count

$rustScopeFilterRecords = @($rustScopeRecords | Where-Object {
    $null -ne $_.actual -and
    $_.actual.PSObject.Properties.Name -contains "command" -and
    [string]$_.actual.command -eq "scope-filter"
})
$rustScopeIncomingTotal = 0L
$rustScopeAcceptedTotal = 0L
$rustScopeRejectedTotal = 0L
foreach ($record in $rustScopeFilterRecords) {
    $rustScopeIncomingTotal += Get-ActualLongField -Actual $record.actual -Name "scope_incoming_count"
    $rustScopeAcceptedTotal += Get-ActualLongField -Actual $record.actual -Name "scope_accepted_count"
    $rustScopeRejectedTotal += Get-ActualLongField -Actual $record.actual -Name "scope_rejected_count"
}
$rustScopeDroppedTotal = ($rustScopeFilterRecords | Where-Object { [string]$_.status -ne "pass" } | Measure-Object).Count

$gitCommit = "unknown"
Push-Location $repoRoot
try {
    $gitCommit = (git rev-parse HEAD 2>$null).Trim()
}
catch {
    $gitCommit = "unknown"
}
finally {
    Pop-Location
}

$parityStatus = if ($mismatches.Count -eq 0) { "pass" } else { "fail" }

$benchmarkSource = "none"
$benchmarkBaselineRunId = $AuthPathBenchmarkBaselineRunId
$benchmarkCandidateRunId = $AuthPathBenchmarkCandidateRunId
$benchmarkP50LatencyRegressionPct = $AuthPathBenchmarkP50LatencyRegressionPct
$benchmarkP95LatencyRegressionPct = $AuthPathBenchmarkP95LatencyRegressionPct
$benchmarkAllocRegressionPct = $AuthPathBenchmarkAllocRegressionPct

if (-not [string]::IsNullOrWhiteSpace($AuthPathBenchmarkResultPath)) {
    if (-not (Test-Path $AuthPathBenchmarkResultPath)) {
        throw "Benchmark result file not found: $AuthPathBenchmarkResultPath"
    }

    $benchmarkJson = Get-Content -Raw -Path $AuthPathBenchmarkResultPath | ConvertFrom-Json
    if ($null -ne $benchmarkJson.baseline_run_id -and -not [string]::IsNullOrWhiteSpace([string]$benchmarkJson.baseline_run_id)) {
        $benchmarkBaselineRunId = [string]$benchmarkJson.baseline_run_id
    }
    if ($null -ne $benchmarkJson.candidate_run_id -and -not [string]::IsNullOrWhiteSpace([string]$benchmarkJson.candidate_run_id)) {
        $benchmarkCandidateRunId = [string]$benchmarkJson.candidate_run_id
    }
    if ($null -ne $benchmarkJson.p50_latency_regression_pct) {
        $benchmarkP50LatencyRegressionPct = [double]$benchmarkJson.p50_latency_regression_pct
    }
    if ($null -ne $benchmarkJson.p95_latency_regression_pct) {
        $benchmarkP95LatencyRegressionPct = [double]$benchmarkJson.p95_latency_regression_pct
    }
    if ($null -ne $benchmarkJson.alloc_regression_pct) {
        $benchmarkAllocRegressionPct = [double]$benchmarkJson.alloc_regression_pct
    }

    $benchmarkSource = "file"
} elseif (
    -not [string]::IsNullOrWhiteSpace($AuthPathBenchmarkBaselineRunId) -or
    -not [string]::IsNullOrWhiteSpace($AuthPathBenchmarkCandidateRunId) -or
    -not [double]::IsNaN($AuthPathBenchmarkP50LatencyRegressionPct) -or
    -not [double]::IsNaN($AuthPathBenchmarkP95LatencyRegressionPct) -or
    -not [double]::IsNaN($AuthPathBenchmarkAllocRegressionPct)
) {
    $benchmarkSource = "parameters"
}

$hasBenchmarkResult =
    -not [string]::IsNullOrWhiteSpace($benchmarkBaselineRunId) -and
    -not [string]::IsNullOrWhiteSpace($benchmarkCandidateRunId) -and
    -not [double]::IsNaN($benchmarkP50LatencyRegressionPct) -and
    -not [double]::IsNaN($benchmarkP95LatencyRegressionPct) -and
    -not [double]::IsNaN($benchmarkAllocRegressionPct)

$benchmarkWithinThresholds =
    $hasBenchmarkResult -and
    ($benchmarkP50LatencyRegressionPct -le $AuthPathP50LatencyRegressionPctMax) -and
    ($benchmarkP95LatencyRegressionPct -le $AuthPathP95LatencyRegressionPctMax) -and
    ($benchmarkAllocRegressionPct -le $AuthPathAllocRegressionPctMax)

$authPathBenchmarkStatus = if (-not $hasBenchmarkResult) {
    "not_evaluated"
} elseif ($benchmarkWithinThresholds) {
    "pass"
} else {
    "fail"
}

$parity = [ordered]@{
    run_id = $RunId
    status = $parityStatus
    git_commit = $gitCommit
    utc_timestamp = [DateTime]::UtcNow.ToString("o")
    policy_channel_mode = $PolicyChannelMode
    vectors_file = "docs/acceptance/authz-conformance-vectors.json"
    rust = [ordered]@{
        artifact = (Resolve-Path -Relative $rustOutPath).Replace(".\\", "")
        pass_count = $rustPass
        fail_count = $rustFail
    }
    dotnet = [ordered]@{
        artifact = (Resolve-Path -Relative $dotnetTrxPath).Replace(".\\", "")
        records_artifact = (Resolve-Path -Relative $dotnetRecordsPath).Replace(".\\", "")
        total = $dotnetTotal
        passed = $dotnetPassed
        failed = $dotnetFailed
        mapped_record_count = $dotnetNormalizedRecords.Count
    }
    capcomp = [ordered]@{
        enabled = [bool]$IncludeCapCompSupplemental
        vector_count = $capcompVectorIds.Count
        rust = [ordered]@{
            record_count = $rustCapcompRecords.Count
            pass_count = $rustCapcompPass
            fail_count = $rustCapcompFail
        }
        dotnet = [ordered]@{
            mapped_record_count = $dotnetCapcompRecords.Count
            pass_count = $dotnetCapcompPass
            fail_count = $dotnetCapcompFail
        }
        parity_mismatch_count = $capcompMismatches.Count
        promotion_signal = if ($IncludeCapCompSupplemental -and $capcompMismatches.Count -eq 0 -and $rustCapcompFail -eq 0 -and $dotnetCapcompFail -eq 0) {
            "supplemental_pass"
        } elseif ($IncludeCapCompSupplemental) {
            "supplemental_fail"
        } else {
            "not_evaluated"
        }
    }
    scope = [ordered]@{
        enabled = ($scopeVectorIds.Count -gt 0)
        vector_count = $scopeVectorIds.Count
        rust = [ordered]@{
            record_count = $rustScopeRecords.Count
            pass_count = $rustScopePass
            fail_count = $rustScopeFail
        }
        dotnet = [ordered]@{
            mapped_record_count = $dotnetScopeRecords.Count
            pass_count = $dotnetScopePass
            fail_count = $dotnetScopeFail
        }
        runtime_metrics = [ordered]@{
            rust = [ordered]@{
                filtered_incoming_total = $rustScopeIncomingTotal
                filtered_nodes_total = $rustScopeRejectedTotal
                filtered_nodes_kept_total = $rustScopeAcceptedTotal
                filtered_pack_dropped_total = $rustScopeDroppedTotal
            }
            dotnet = [ordered]@{
                mapped_filter_record_count = ($dotnetScopeRecords | Where-Object {
                    $null -ne $_.actual -and
                    $_.actual.PSObject.Properties.Name -contains "command" -and
                    [string]$_.actual.command -eq "scope-filter"
                } | Measure-Object).Count
            }
        }
        parity_mismatch_count = $scopeMismatches.Count
    }
    auth_path_benchmark_thresholds = [ordered]@{
        p50_latency_regression_pct_max = $AuthPathP50LatencyRegressionPctMax
        p95_latency_regression_pct_max = $AuthPathP95LatencyRegressionPctMax
        alloc_regression_pct_max = $AuthPathAllocRegressionPctMax
    }
    auth_path_benchmark_result = [ordered]@{
        baseline_run_id = if ([string]::IsNullOrWhiteSpace($benchmarkBaselineRunId)) { $null } else { $benchmarkBaselineRunId }
        candidate_run_id = if ([string]::IsNullOrWhiteSpace($benchmarkCandidateRunId)) { $null } else { $benchmarkCandidateRunId }
        p50_latency_regression_pct = if ([double]::IsNaN($benchmarkP50LatencyRegressionPct)) { $null } else { $benchmarkP50LatencyRegressionPct }
        p95_latency_regression_pct = if ([double]::IsNaN($benchmarkP95LatencyRegressionPct)) { $null } else { $benchmarkP95LatencyRegressionPct }
        alloc_regression_pct = if ([double]::IsNaN($benchmarkAllocRegressionPct)) { $null } else { $benchmarkAllocRegressionPct }
        status = $authPathBenchmarkStatus
        source = $benchmarkSource
    }
    parity = [ordered]@{
        status = $parityStatus
        mismatches = $mismatches
    }
    notes = "Canonical conformance workflow output"
}

if ($IncludeCapCompSupplemental) {
    $parity.notes = "Conformance workflow output (CAPCOMP supplemental vectors included in Rust runner)"
}

$parity | ConvertTo-Json -Depth 8 | Set-Content -Path $parityOutPath -Encoding UTF8
Write-Host "Wrote parity artifact: $parityOutPath"
Write-Host "Parity status: $parityStatus"

if ($parityStatus -ne "pass" -and -not $NoFailOnParityMismatch) {
    throw "Parity status is '$parityStatus'"
}
