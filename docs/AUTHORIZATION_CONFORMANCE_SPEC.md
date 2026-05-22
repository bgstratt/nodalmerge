# Authorization Conformance Spec (P4)

Status: Draft v1
Owner: Architecture group + host owners
Companion tracker: [AUTHORIZATION_CORE_HOST_EXECUTION_TRACKER.md](AUTHORIZATION_CORE_HOST_EXECUTION_TRACKER.md)

## Purpose

Define a host-independent conformance suite for authorization/authentication behavior across Rust host and DotNetHost.

## Scope

This suite validates:

1. Authn admission behavior for room-locked and unlocked flows.
2. Authz control-plane gating parity (`set-policy`, `set-room-key`, `start-tick`, `stop-tick`).
3. Replay policy timeline semantics parity for policy-at-time transitions.
4. Deterministic rejection reason classification and label semantics.
5. Policy update channel mode behavior and parity.
6. Capability composition/flattening parity (host-side only) for profile-versioned capability graphs.

Out of scope:

1. UI-facing SDK behavior details (covered in SDK docs/examples).
2. Host-specific packaging/deployment details.

## Required Inputs

1. Canonical capability contract: [AUTHORIZATION_CONTROL_PLANE_CAPABILITIES.md](AUTHORIZATION_CONTROL_PLANE_CAPABILITIES.md)
2. Canonical rejection taxonomy: [AUTHORIZATION_REJECTION_TAXONOMY.md](AUTHORIZATION_REJECTION_TAXONOMY.md)
3. Policy timeline decision: [POLICY_TIMELINE_ENCODING_DECISION.md](POLICY_TIMELINE_ENCODING_DECISION.md)
4. Reusable vector file: [acceptance/authz-conformance-vectors.json](acceptance/authz-conformance-vectors.json)
5. Capability composition architecture contract: [AUTHORIZATION_CORE_HOST_SEPARATION_PLAN.md](AUTHORIZATION_CORE_HOST_SEPARATION_PLAN.md)
6. Capability profile schema contract: [AUTHORIZATION_CAPABILITY_PROFILE_SCHEMA.md](AUTHORIZATION_CAPABILITY_PROFILE_SCHEMA.md)

## Test Categories

## C1. Authn admission

1. Unlocked room allows `ClientHello` without token.
2. Locked room rejects missing/invalid token with auth rejection class.
3. Locked room accepts valid token for matching room and peer.

## C2. Control-plane authz gating

1. `set-policy` requires `policy.admin`.
2. `set-room-key` requires `room.admin`.
3. `start-tick` and `stop-tick` require `tick.admin`.
4. Allowed paths succeed when capability present.

## C3. Deny surface parity

1. Denied control-plane operations emit `reject.control_plane_forbidden` class.
2. When structured deny metadata exists, `command` and `required_capability` are concrete and stable.
3. Metric label values for deny paths are in fixed, expected enums.

## C4. Replay policy timeline

1. Policy cutover at lamport boundary is inclusive.
2. Timeline ordering normalization yields deterministic outcomes.
3. Replay + compaction compatibility metadata validates against expected timeline fingerprint.

## C5. Transport/path consistency

1. Equivalent deny semantics across runtime ingress and FFI ingress paths.
2. Equivalent behavior between Rust host and DotNetHost for the same vectors.

## C6. Policy update channel contract

Final decision:

1. Hosts expose a configurable `policy_channel_mode` with three allowed values:
	- `admin_command_only`
	- `replicated_signed_ops_only`
	- `hybrid`
2. Default mode is profile-based:
	- auth-enabled hosted authority profile: `admin_command_only`
	- decentralized/distributed profile: `hybrid` (or `replicated_signed_ops_only` where required)
3. Canonical release-gating conformance baseline runs with `policy_channel_mode=admin_command_only`.
4. In hybrid mode, policy updates may be accepted from admin control-plane commands and replicated signed policy ops, but both paths must converge to the same deterministic policy timeline semantics.
5. Host/provider selection (NuGet/crate/adapter consumers) may change mode, but mode must be explicit and test-reported.

## C7. Capability composition and flattening contract

1. Core evaluation contract remains flattened capability matching only; no inheritance semantics are evaluated in core.
2. Hosts that enable capability composition must expand via explicit additive DAG edges only.
3. Expanded capabilities must be canonicalized before signing:
	- trim
	- lowercase
	- charset/length validation
	- deduplicate
	- lexicographic ascending sort
4. Unknown capability profile versions must deterministically reject (no fallback to another profile).
5. Invalid graphs (cycles, malformed edges, overflow of configured limits) must deterministically reject and report stable reason-class metadata.
6. Runtime token payload remains flattened; inheritance graph/provenance is not required in runtime token payload.
7. Compatibility windows are explicit and bounded via host profile support lists; no implicit compatibility inference is allowed.

Initial capability-composition vectors (to be added and gated in phased rollout):

1. `CAPCOMP-EXPAND-ALLOW-001`: direct and inherited capabilities flatten to expected canonical set.
2. `CAPCOMP-EXPAND-ALLOW-002`: multiple inheritance paths deduplicate to one canonical capability instance.
3. `CAPCOMP-COMPAT-ALLOW-001`: explicitly supported prior profile version is accepted in compatibility window.
4. `CAPCOMP-COMPAT-REJECT-UNSUPPORTED-001`: unsupported profile version is rejected even when compatibility window is present.
5. `CAPCOMP-REJECT-CYCLE-001`: cycle in capability graph is rejected.
6. `CAPCOMP-REJECT-UNKNOWN-PROFILE-001`: unknown `capability_profile_version` is rejected.
7. `CAPCOMP-REJECT-LIMIT-COUNT-001`: flattened capability count overflow is rejected.
8. `CAPCOMP-REJECT-LIMIT-DEPTH-001`: inheritance depth overflow is rejected.
9. `CAPCOMP-REJECT-UNKNOWN-REFERENCE-001`: unknown inheritance reference is rejected.
10. `CAPCOMP-REJECT-DUPLICATE-NODE-001`: duplicate capability node declaration is rejected.
11. `CAPCOMP-REJECT-LIMIT-EDGES-001`: per-node inheritance edge overflow is rejected.
12. `CAPCOMP-REJECT-LIMIT-PAYLOAD-001`: flattened capability payload-byte overflow is rejected.
13. `CAPCOMP-REJECT-SUPPORTED-VERSIONS-SCHEMA-001`: invalid supported-profile-version schema entry is rejected.
14. `CAPCOMP-REJECT-GRAMMAR-001`: invalid capability token grammar is rejected.

## C8. Scoped replication and subscription contract (Phase A)

1. Runtime subscribe command accepts explicit path-pattern scopes and maps deterministically to host subscribe command payload.
2. Runtime subscription-updated host events map deterministically to `subscribe-ack` client-visible envelopes.
3. Scope vectors in this phase validate mapping parity; deeper catch-up budget and selective materialization vectors are added in subsequent phases.

Initial scope vectors:

1. `SCOPE-SUBSCRIBE-COMMAND-001`: subscribe command with explicit patterns maps to host subscribe command.
2. `SCOPE-SUBSCRIBE-ACK-001`: host subscription update event maps to `subscribe-ack` runtime message.
3. `SCOPE-FILTER-MATCH-001`: scope filtering keeps at least one node when path patterns match.
4. `SCOPE-FILTER-REJECT-001`: scope filtering drops nodes when no path patterns match.

## C9. Identity continuity and rotation contract (Phase C)

1. Identity continuity-v1 follows the contract in [IDENTITY_CONTINUITY_V1_CONTRACT.md](IDENTITY_CONTINUITY_V1_CONTRACT.md).
2. Successor signer admission is allowed only inside declared overlap window.
3. Successor signer admission is rejected when predecessor key is revoked.

Initial continuity vectors:

1. `IDENTITY-CONTINUITY-001`: allow successor signer during overlap window.
2. `IDENTITY-CONTINUITY-002`: reject successor signer after overlap window expiry.
3. `IDENTITY-CONTINUITY-003`: reject successor signer when predecessor key is revoked.

## Policy Channel Profile Matrix

| Profile | Typical deployment | Default `policy_channel_mode` | Release-gating required | Notes |
| ------- | ------------------ | ----------------------------- | ----------------------- | ----- |
| Hosted Secure | Auth-enabled server/super-peer authority | `admin_command_only` | Yes (canonical) | Best default for centralized policy governance and least privilege. |
| Hybrid Authority | Hosted authority with delegated/distributed policy writes | `hybrid` | Optional supplemental | Must prove deterministic convergence between admin-command and signed-op policy updates. |
| Fully Distributed | Peer mesh or authority-light deployments | `replicated_signed_ops_only` (or `hybrid`) | Optional supplemental | Prioritizes decentralized control; requires stronger key/signature hygiene. |

## Pass Criteria

1. Every required vector passes on Rust host and DotNetHost.
2. No reason-class drift between hosts for equivalent vector IDs.
3. No capability-name drift from canonical contract.
4. Every run reports `policy_channel_mode` and host metadata in artifacts.
5. Canonical release gate requires admin-command-only parity pass on both hosts.
6. For capability-composition profile runs, expanded capability outputs and reject classes must match for equivalent vectors.
7. Unknown-profile and invalid-graph vectors must yield deterministic, host-parity rejection classes.
8. CAPCOMP reject vectors that declare `expected.reason_detail` must match deterministic `actual.reason_detail` values across hosts.
9. Identity continuity reject vectors that declare `expected.reason_detail` must match deterministic `actual.reason_detail` values.

## Reporting Format

Each host publishes results as:

1. `vector_id`
2. `status` (`pass` or `fail`)
3. `actual.reason_class`
4. `actual.reason_detail` (nullable; required for CAPCOMP detail vectors)
5. `actual.command`
6. `actual.required_capability`
7. `policy_channel_mode`
8. `notes`
9. `capability_profile_version` (when capability composition vectors are enabled)

Run-level metadata required for every artifact set:

1. `run_id`
2. `git_commit`
3. `utc_timestamp`
4. `host`
5. `harness`
6. `policy_channel_mode`
7. `vectors_file`
8. `capability_profile_version` (nullable)

## Execution

1. Load vectors from [acceptance/authz-conformance-vectors.json](acceptance/authz-conformance-vectors.json).
2. Execute vectors in host-specific harness using explicit `policy_channel_mode` (canonical: `admin_command_only`).
3. Emit normalized result records and host-native artifacts.
5. Compare outputs between hosts for parity.

Capability composition execution mode (phase-gated):

1. Canonical release gate remains `policy_channel_mode=admin_command_only` without composition vectors until promoted.
2. Composition vectors run as supplemental profile evidence until tracker promotes them to release-gating.
3. Supplemental composition runs must set explicit profile metadata and publish the same parity artifact schema.

Parity comparison rule:

1. Rust host uses normalized runner records for full vector coverage.
2. DotNetHost parity uses normalized records mapped from targeted TRX outcomes (`authz-conformance-dotnet-records*.json`).
3. Record-level parity is required for all mapped vectors in canonical mode.

## Canonical execution workflow (single path)

Use this workflow as the only release-gating path for P4:

1. Set a run identifier (`RUN_ID`) and use fixed artifact names under `docs/acceptance/`.
2. Execute SDK rejection-surface parity check and emit SDK parity artifact.
3. Execute Rust conformance runner and emit normalized records.
4. Execute DotNetHost targeted authz slice and emit TRX.
5. Emit a host-agnostic parity report comparing Rust normalized results + DotNetHost mapped outcomes.
6. Record `policy_channel_mode=admin_command_only` in all run metadata.

Canonical commands:

```bash
cargo run -p activesync-server --bin authz-conformance-runner -- \
  --vectors docs/acceptance/authz-conformance-vectors.json \
  --out docs/acceptance/authz-conformance-rust.json
```

```bash
node docs/acceptance/Check-SdkRejectionParity.mjs
```

```powershell
dotnet test tests/ActiveSync.DotNetHost.Tests/ActiveSync.DotNetHost.Tests.csproj \
  --filter "FullyQualifiedName~RuntimeMessageProcessorTests.Set_policy_without_capability_returns_control_plane_forbidden_error_envelope|FullyQualifiedName~RuntimeMessageProcessorTests.Start_tick_without_capability_returns_control_plane_forbidden_error_envelope|FullyQualifiedName~RuntimeTokenValidationServiceTests.ValidateInboundAsync_denies_when_provider_rejects_token|FullyQualifiedName~RuntimeTokenValidationServiceTests.ValidateInboundAsync_allows_when_provider_accepts_token|FullyQualifiedName~RuntimeWebSocketEndpointTests.Runtime_endpoint_set_policy_without_policy_admin_capability_is_denied_before_bridge_dispatch|FullyQualifiedName~RuntimeWebSocketEndpointTests.Runtime_endpoint_set_policy_with_policy_admin_capability_is_allowed|FullyQualifiedName~RuntimeWebSocketEndpointTests.Runtime_endpoint_start_tick_without_tick_admin_capability_is_denied_before_bridge_dispatch|FullyQualifiedName~RuntimeWebSocketEndpointTests.Runtime_endpoint_start_tick_with_tick_admin_capability_is_allowed" \
  --logger "trx;LogFileName=authz-conformance-dotnet.trx" \
  --results-directory "..\\docs\\acceptance" -v minimal
```

Canonical workflow helper script:

1. [acceptance/Run-AuthzConformance.ps1](acceptance/Run-AuthzConformance.ps1)
2. Example:

```powershell
pwsh -File docs/acceptance/Run-AuthzConformance.ps1 -PolicyChannelMode admin_command_only
```

Local nightly-equivalent helper script:

1. [acceptance/Run-LocalNightlyEquivalent.ps1](acceptance/Run-LocalNightlyEquivalent.ps1)
2. Runs canonical + snapshot drill + supplemental profiles + CAPCOMP summary/history in one command.
3. Writes a machine-readable run summary artifact at `docs/acceptance/local-nightly-equivalent-<timestamp>.json`.
4. Can auto-generate benchmark evidence for canonical parity when benchmark inputs are provided (run IDs + either benchmark JSON files or explicit regression percentages).
4. Example:

```powershell
pwsh -File docs/acceptance/Run-LocalNightlyEquivalent.ps1
```

Example with benchmark JSON ingestion:

```powershell
pwsh -File docs/acceptance/Run-LocalNightlyEquivalent.ps1 `
	-AuthPathBenchmarkBaselineRunId baseline-001 `
	-AuthPathBenchmarkCandidateRunId candidate-001 `
	-AuthPathBenchmarkBaselineJsonPath docs/acceptance/bench-baseline.json `
	-AuthPathBenchmarkCandidateJsonPath docs/acceptance/bench-candidate.json `
	-AuthPathBenchmarkTarget dotnet-host-runtime `
	-AuthPathBenchmarkAssumeZeroAllocWhenMissing
```

Supplemental non-canonical evidence runs:

```powershell
pwsh -File docs/acceptance/Run-AuthzConformance.ps1 -PolicyChannelMode hybrid -OutputTag supp-hybrid -NoFailOnParityMismatch
```

```powershell
pwsh -File docs/acceptance/Run-AuthzConformance.ps1 -PolicyChannelMode replicated_signed_ops_only -OutputTag supp-replicated_signed_ops_only -NoFailOnParityMismatch
```

Supplemental CAPCOMP evidence run:

```powershell
pwsh -File docs/acceptance/Run-AuthzConformance.ps1 -PolicyChannelMode admin_command_only -IncludeCapCompSupplemental -OutputTag supp-capcomp -NoFailOnParityMismatch
```

Auth-path benchmark evidence helper (for file-based parity ingestion):

```powershell
pwsh -File docs/acceptance/Write-AuthPathBenchmarkResult.ps1 \
	-BaselineRunId baseline-20260521 \
	-CandidateRunId candidate-20260521 \
	-BaselineP50LatencyMs 1.21 -CandidateP50LatencyMs 1.24 \
	-BaselineP95LatencyMs 3.82 -CandidateP95LatencyMs 3.95 \
	-BaselineAllocBytes 18432 -CandidateAllocBytes 19008 \
	-OutPath docs/acceptance/authz-auth-path-benchmark-result.json
```

Auth-path benchmark evidence helper (JSON ingestion from benchmark harness output):

```powershell
pwsh -File docs/acceptance/Write-AuthPathBenchmarkResult.ps1 \
	-BaselineRunId bench-baseline-20260521 \
	-CandidateRunId bench-candidate-20260522 \
	-BaselineBenchmarkJsonPath benchmarks/results/hosted-baseline.json \
	-CandidateBenchmarkJsonPath benchmarks/results/hosted-candidate.json \
	-BenchmarkTarget dotnet-host-runtime \
	-AssumeZeroAllocWhenMissing \
	-OutPath docs/acceptance/authz-auth-path-benchmark-result.json
```

Then provide benchmark evidence to parity emission:

```powershell
pwsh -File docs/acceptance/Run-AuthzConformance.ps1 \
	-PolicyChannelMode admin_command_only \
	-AuthPathBenchmarkResultPath docs/acceptance/authz-auth-path-benchmark-result.json
```

Nightly workflow-dispatch examples:

Manual regression percentages:

```bash
gh workflow run authz-conformance-nightly.yml \
	-f auth_path_benchmark_baseline_run_id=baseline-20260521 \
	-f auth_path_benchmark_candidate_run_id=candidate-20260522 \
	-f auth_path_benchmark_p50_regression_pct=1.4 \
	-f auth_path_benchmark_p95_regression_pct=2.1 \
	-f auth_path_benchmark_alloc_regression_pct=0.0
```

JSON-driven benchmark comparison:

```bash
gh workflow run authz-conformance-nightly.yml \
	-f auth_path_benchmark_baseline_run_id=baseline-20260521 \
	-f auth_path_benchmark_candidate_run_id=candidate-20260522 \
	-f auth_path_benchmark_baseline_json_path=benchmarks/results/hosted-baseline.json \
	-f auth_path_benchmark_candidate_json_path=benchmarks/results/hosted-candidate.json \
	-f auth_path_benchmark_target=dotnet-host-runtime
```

## Canonical artifact contract

Required artifacts per run:

1. `docs/acceptance/authz-conformance-vectors.json` (input vectors snapshot)
2. `docs/acceptance/authz-conformance-sdk-parity.json` (npm SDK rejection-surface parity artifact)
3. `docs/acceptance/authz-conformance-rust.json` (normalized Rust host results)
4. `docs/acceptance/authz-conformance-dotnet.trx` (DotNetHost execution artifact)
5. `docs/acceptance/authz-conformance-dotnet-records.json` (normalized DotNetHost records mapped from targeted conformance tests)
6. `docs/acceptance/authz-conformance-parity.json` (cross-host parity report)

Nightly governance artifacts (promotion readiness/streak tracking):

1. `docs/acceptance/promotion-readiness.json` (single-run gate evidence summary)
2. `_history/promotion-readiness-history.jsonl` (append-only per-run snapshot log)
3. `_history/promotion-readiness-history-summary.json` (derived streak counters and readiness flags)

Supplemental run artifact naming:

1. Tagged runs append `-<OutputTag>` before extension (for example `authz-conformance-parity-supp-hybrid.json`).

`authz-conformance-parity.json` minimum schema:

1. `run_id`
2. `policy_channel_mode`
3. `rust`:
	- `artifact`
	- `pass_count`
	- `fail_count`
4. `dotnet`:
	- `artifact`
	- `total`
	- `passed`
	- `failed`
5. `parity`:
	- `status` (`pass` or `fail`)
	- `mismatches` (array)
6. `notes`

`authz-conformance-parity.json` CAPCOMP supplemental fields (present for all runs, meaningful when CAPCOMP is enabled):

1. `capcomp`:
	- `enabled` (bool)
	- `vector_count` (count of `CAPCOMP-*` vectors in current vector set)
	- `rust`:
		- `record_count`
		- `pass_count`
		- `fail_count`
	- `dotnet`:
		- `mapped_record_count`
		- `pass_count`
		- `fail_count`
	- `parity_mismatch_count`
	- `promotion_signal` (`not_evaluated`, `supplemental_pass`, `supplemental_fail`)

Promotion-signal interpretation:

1. `not_evaluated`: CAPCOMP supplemental mode not enabled for this run.
2. `supplemental_pass`: CAPCOMP supplemental mode enabled and CAPCOMP records/mappings passed with zero CAPCOMP parity mismatches.
3. `supplemental_fail`: CAPCOMP supplemental mode enabled and at least one CAPCOMP failure or CAPCOMP mismatch occurred.

`authz-conformance-parity.json` scope fields (present when scope vectors exist in the vector set):

1. `scope`:
	- `enabled` (bool)
	- `vector_count`
	- `rust`:
		- `record_count`
		- `pass_count`
		- `fail_count`
	- `dotnet`:
		- `mapped_record_count`
		- `pass_count`
		- `fail_count`
	- `parity_mismatch_count`

`authz-conformance-parity.json` auth-path benchmark gate fields (machine-produced for every run):

1. `auth_path_benchmark_thresholds`:
	- `p50_latency_regression_pct_max`
	- `p95_latency_regression_pct_max`
	- `alloc_regression_pct_max`
2. `auth_path_benchmark_result`:
	- `baseline_run_id` (nullable)
	- `candidate_run_id` (nullable)
	- `p50_latency_regression_pct` (nullable)
	- `p95_latency_regression_pct` (nullable)
	- `alloc_regression_pct` (nullable)
	- `status` (`not_evaluated`, `pass`, `fail`)
	- `source` (`none`, `parameters`, `file`)

Benchmark status interpretation:

1. `not_evaluated`: benchmark evidence fields are incomplete for this run.
2. `pass`: all benchmark evidence fields are present and each regression percentage is less than or equal to its configured threshold.
3. `fail`: benchmark evidence fields are present but one or more regression percentages exceed configured thresholds.

## Reference runner (baseline)

A minimal executable baseline runner is available in the `activesync-server`
crate:

1. Binary: `authz-conformance-runner`
2. Command:

```bash
cargo run -p activesync-server --bin authz-conformance-runner -- \
	--vectors docs/acceptance/authz-conformance-vectors.json \
	--out docs/acceptance/authz-conformance-results.json
```

Current baseline artifact:

1. [acceptance/authz-conformance-results.json](acceptance/authz-conformance-results.json)

Execution notes:

1. Rust-host control-plane vectors execute through the server ingress helper used by `ws_handler` capability checks.
2. Replay vectors execute through core `replay_with_policy_timeline` with timeline-driven policy cutover evaluation.
3. DotNetHost runtime/ffi adapter-backed authorization behavior is validated via targeted runtime tests, with result artifact `docs/acceptance/authz-dotnet-conformance-targeted.trx` (8/8 passing for authn/authz/deny coverage).
4. DotNetHost records in the minimal Rust runner remain contract-mapped, but Task 3 execution evidence is now covered by adapter-backed DotNetHost runtime test execution.
5. During migration to canonical naming, historical artifacts may retain prior names (`authz-conformance-results.json`, `authz-dotnet-conformance-targeted.trx`); new runs should emit canonical artifact names.
6. Current canonical baseline run is green in `policy_channel_mode=admin_command_only` with artifacts:
	- `docs/acceptance/authz-conformance-rust.json`
	- `docs/acceptance/authz-conformance-dotnet.trx`
	- `docs/acceptance/authz-conformance-dotnet-records.json`
	- `docs/acceptance/authz-conformance-parity.json` (`status=pass`, `mismatches=none`)
