# Authorization Core-Host Execution Tracker

Status: Active
Owner: Core + host runtime streams
Last Updated: 2026-05-22
Companion Plan: [AUTHORIZATION_CORE_HOST_SEPARATION_PLAN.md](AUTHORIZATION_CORE_HOST_SEPARATION_PLAN.md)
Companion Execution Plan: [PLATFORM_FOUNDATION_EXECUTION_PLAN.md](PLATFORM_FOUNDATION_EXECUTION_PLAN.md)

## 1. Program Snapshot

Objective:
1. Ship deterministic core authorization semantics with host-owned identity integration.
2. Preserve offline-first and transport-agnostic convergence.
3. Deliver a repeatable pattern for Rust crate/NuGet/npm adapters and future host stacks.

Current phase:
1. P0 complete.
2. P1 complete.
3. P1.5 complete.
4. P2 complete.
5. P4 complete.
6. P5 in progress (capability inheritance/composition host-side rollout).

## 2. Owners

| Area | Primary Owner | Supporting Owner | Notes |
| ---- | ------------- | ---------------- | ----- |
| Core policy semantics | Core runtime maintainers | Security reviewer | activesync-core enforcement and replay semantics |
| Rust host control-plane enforcement | Rust host maintainers | Core runtime maintainers | ws_handler and room policy controls |
| DotNetHost parity | DotNetHost maintainers | API/platform maintainers | provider composition and control-plane guardrails |
| SDK ergonomics (npm) | SDK maintainers | Host maintainers | intent/state helpers and rejection surfaces |
| Multi-host conformance | Architecture group | Language host owners | includes future host contract patterns |

## 3. Milestones

| Milestone | Target Date | Phase Gate | Owner |
| --------- | ----------- | ---------- | ----- |
| M1: Baseline + taxonomy locked | 2026-05-28 | P0 exit | Core runtime maintainers |
| M2: Control-plane hardening merged | 2026-06-18 | P1 exit | Rust host maintainers + DotNetHost maintainers |
| M3: Policy timeline replay correctness | 2026-07-16 | P2 exit | Core runtime maintainers |
| M4: SDK auth ergonomics baseline | 2026-08-06 | P3 exit | SDK maintainers |
| M5: Multi-host conformance kit v1 | 2026-09-03 | P4 exit | Architecture group |

## Qualified-Run Milestone Checklist

Use this checklist to decide when a full qualified run is mandatory (in addition to nightly drift sentinel runs).

Trigger categories requiring a qualified run:
1. Identity/auth handling:
   - continuity proof validation, overlap/revocation logic, signer identity normalization, token provider contract or parser changes.
2. CAPCOMP and authorization profile expansion:
   - profile DAG semantics, limits/validation behavior, version compatibility windows, flattening payload changes.
3. Replay/restore and compaction behavior:
   - snapshot restore path, replay ordering/timeline cutover behavior, checkpoint/restore hash logic.
4. Runtime websocket command path:
   - hello/bootstrap flow, subscribe/scope behavior, control-plane command authorization/dispatch, reject reason envelope shape.
5. Conformance/promotion gating plumbing:
   - vector definitions, Rust↔DotNet mapping tables, benchmark gate ingestion/threshold policy, promotion summary/history logic.

Required evidence bundle for milestone-triggered runs:
1. Canonical parity pass artifact present.
2. CAPCOMP supplemental parity pass artifact present.
3. Benchmark evidence present and benchmark status evaluated (not `not_evaluated`).
4. Promotion readiness summary + history updated with evaluable gate verdict fields.

## 4. Phase Checklist

## Decision Log (2026-05-21)

1. Tick commands are treated as host-runtime control-plane features (commonly used by engine/simulation hosts), not core data-plane policy features.
2. Canonical capability source is a shared contract doc at [AUTHORIZATION_CONTROL_PLANE_CAPABILITIES.md](AUTHORIZATION_CONTROL_PLANE_CAPABILITIES.md); language-specific code constants may mirror it.
3. DotNetHost server-peer bypass parity is implemented; runtime mapper now derives `IsServerPeer` from configured trusted server peer pubkey (`ActiveSync:Runtime:ServerPeerPubkeyHex`) on handshake paths (`hello`, `open-session`, `client-hello`).
4. Denial wire format remains message-string based for now, with deterministic `reject.control_plane_forbidden` prefix.
5. Initial metrics contract is required in P1 for control-plane denies:
   - metric name: `runtime_control_plane_denied_total`
   - labels: `host`, `command`, `required_capability`, `reason_class`
   - cardinality policy: command/capability/reason labels must come from fixed enums only.
6. P1 integration coverage must include allowed and denied paths for `set-policy` and `set-room-key` on both hosts; tick commands are covered where implemented/supported.
7. Room data policy remains namespace/path based (`can_write`) and is not expanded in P1 to per-item room ACL models.
8. Policy update channel contract is finalized as host-configurable (`admin_command_only`, `replicated_signed_ops_only`, `hybrid`) with profile-based defaults (auth-enabled hosted authority defaults to `admin_command_only`; decentralized/distributed may default to `hybrid`).
9. Canonical conformance release-gating baseline runs in `policy_channel_mode=admin_command_only`; non-canonical profile runs are optional supplemental evidence.
10. Canonical P4 execution workflow and artifact contract are defined in [AUTHORIZATION_CONFORMANCE_SPEC.md](AUTHORIZATION_CONFORMANCE_SPEC.md) and should be used for release-gating evidence.
11. Capability inheritance/composition remains host-side only and must flatten capabilities before token signing; core evaluator remains unchanged.
12. Capability composition model is additive explicit-DAG edges only; deny semantics and wildcard inheritance are deferred.
13. Unknown capability profile versions must deterministically reject (no fallback behavior).
14. Promotion streak accounting is run-based (consecutive qualified evidence runs), not calendar-day based; nightly remains a drift sentinel.

## P0: Consolidate and document baseline

Status:
1. Complete.

Acceptance criteria:
1. Single source of truth document accepted.
2. Ingress point inventory complete.
3. Rejection reason taxonomy documented.

Tasks:
1. [x] Publish core-host separation architecture plan.
2. [x] Produce ingress point inventory (Rust server, DotNetHost runtime path, FFI-backed paths).
3. [x] Define canonical rejection reason taxonomy and label policy.
4. [x] Add link references in deployment/architecture docs to the canonical plan.

Evidence:
1. [AUTHORIZATION_CORE_HOST_SEPARATION_PLAN.md](AUTHORIZATION_CORE_HOST_SEPARATION_PLAN.md)
2. [AUTHORIZATION_INGRESS_INVENTORY.md](AUTHORIZATION_INGRESS_INVENTORY.md)
3. [AUTHORIZATION_REJECTION_TAXONOMY.md](AUTHORIZATION_REJECTION_TAXONOMY.md)
2. Tracker updates in this file.

## P1: Control-plane hardening

Status:
1. Complete.

Acceptance criteria:
1. set-policy, set-room-key, start-tick, stop-tick are authorization-gated.
2. Unauthorized control-plane mutation is blocked in tests.
3. Deterministic denial reasons are emitted.

Tasks:
1. [x] Define control-plane capability keys: policy.admin, room.admin, tick.admin.
2. [x] Implement Rust host gating for control-plane commands.
3. [x] Implement DotNetHost equivalent control-plane gating (set-policy, set-room-key).
4. [x] Add integration tests for allowed/denied control-plane commands (set-policy, set-room-key).
5. [x] Add observability counters for control-plane denies by reason class and command.
6. [x] DotNetHost tick command parity (`start-tick` / `stop-tick`) with `tick.admin` authorization enforcement and runtime command mapping.

Progress notes:
1. Rust `ws_handler` now enforces capability-gated control-plane access:
   - `set-room-key` requires `room.admin`.
   - `set-policy` requires `policy.admin`.
   - `start-tick` and `stop-tick` require `tick.admin`.
2. Server peer remains an explicit bypass path for control-plane operations.
3. Deterministic deny codes now use `reject.control_plane_forbidden` message prefix.
4. Unit tests added for control-plane authorization decision helper.
5. DotNetHost runtime mapper now enforces control-plane capability checks with deterministic deny reason prefix:
   - `set-room-key` requires `room.admin`.
   - `set-policy` requires `policy.admin`.
6. DotNetHost focused coverage now includes mapper, message processor, and websocket endpoint tests for allowed vs denied control-plane paths.
7. Cargo PATH was restored on Windows developer environment and verified with `cargo --version` / `rustc --version`.
8. Rust control-plane auth test execution is unblocked after aligning server test destructuring with current `import_nodes` return shape in related server test modules.
9. DotNetHost runtime now emits `runtime_control_plane_denied_total` with fixed labels (`host`, `command`, `required_capability`, `reason_class`) on `reject.control_plane_forbidden` deny paths.
10. Focused DotNetHost tests validate deny metric emission and existing control-plane allow/deny behavior.
11. DotNetHost FFI websocket ingress now emits the same `runtime_control_plane_denied_total` counter on policy-status bridge denies with fixed labels (`host=dotnet-host`, `command=ffi`, `required_capability=unknown`, `reason_class=reject.control_plane_forbidden`).
12. Host-ffi now exposes optional `_ex` ABI submit calls that return deny metadata JSON bytes (`reason_class`, `command`, `required_capability`, optional `deny_message`) without breaking legacy ABI calls.
13. DotNetHost `HostFfiClient` and bridge paths now consume `_ex` metadata when available and automatically fall back to legacy submit calls when `_ex` exports are unavailable.
14. DotNetHost FFI policy deny metrics now emit concrete labels from deny metadata when present, with existing `ffi/unknown` fallback retained for older host-ffi builds.
15. DotNetHost runtime mapper now enforces `tick.admin` for `start-tick` and `stop-tick`, mapping authorized requests to `StartTick`/`StopTick` host commands.
16. DotNetHost focused tests now cover `start-tick`/`stop-tick` allow + deny flows across mapper, processor metrics, and websocket endpoint dispatch behavior.

P1 focused test-case IDs:
1. `RuntimeProtocolTests.Set_room_key_rejects_without_room_admin_capability`
2. `RuntimeProtocolTests.Set_policy_rejects_without_policy_admin_capability`
3. `RuntimeMessageProcessorTests.Set_policy_without_capability_returns_control_plane_forbidden_error_envelope`
4. `RuntimeWebSocketEndpointTests.Runtime_endpoint_set_policy_without_policy_admin_capability_is_denied_before_bridge_dispatch`
5. `RuntimeWebSocketEndpointTests.Runtime_endpoint_set_policy_with_policy_admin_capability_is_allowed`
6. `FfiWebSocketLoopRunnerTests.Policy_status_failure_emits_control_plane_deny_metric_with_fixed_labels`
7. `FfiWebSocketLoopRunnerTests.Policy_status_failure_with_deny_metadata_emits_concrete_metric_labels`
8. `host-ffi/tests/abi.rs` suite passes with `_ex` coverage (`cargo test -p activesync-host-ffi --test abi`).
9. `cargo test -p activesync-server control_plane_auth_tests` passes (3 passed, 0 failed).
10. Focused DotNetHost tick parity test slice passes (9 passed, 0 failed).

Dependencies:
1. P0 rejection taxonomy.
2. Canonical capability contract doc: [AUTHORIZATION_CONTROL_PLANE_CAPABILITIES.md](AUTHORIZATION_CONTROL_PLANE_CAPABILITIES.md).

## P1.5: Hardening and compatibility polish

Status:
1. Complete.

Acceptance criteria:
1. FFI deny metrics remain compatible with legacy host-ffi binaries.
2. Concrete deny labels are emitted when enriched metadata is available.
3. Tracker captures sequencing so hardening does not block P1 closure.

Tasks:
1. [x] Add optional host-ffi `_ex` submit APIs with deny metadata output and legacy ABI compatibility.
2. [x] Add DotNetHost `_ex` consume path with fallback to legacy entry points.
3. [x] Emit concrete FFI deny metric labels when metadata is present with `ffi/unknown` fallback.
4. [x] Expand host-core deny reason surfacing so more denied paths can emit concrete metadata labels.

Dependencies:
1. P1 control-plane deny semantics.
2. FFI deny metadata decision set in [FFI_DENY_METADATA_REVIEW_LIST.md](FFI_DENY_METADATA_REVIEW_LIST.md).

P1.5 progress notes:
1. Host-core now exposes explicit `AuthViolation` and `PolicyViolation` error variants (in addition to existing protocol/status errors).
2. Locked-room `ClientHello` token failures are now surfaced as `AuthViolation` instead of generic protocol errors.
3. Host-ffi `_ex` deny metadata now emits concrete metadata for auth and protocol denies (`reason_class`, `command`, `required_capability`, `deny_message`) instead of policy-only metadata.
4. Focused validation passed:
   - `cargo test -p activesync-host-core client_hello_requires_valid_token_when_room_is_locked` (1 passed)
   - `cargo test -p activesync-host-ffi --test abi` (20 passed)
5. DotNetHost runtime status error envelopes now surface bridge deny metadata diagnostics when available (`msg`, `reason_class`, `command`, `required_capability`) while preserving existing `status` semantics.
6. Additional focused validation passed:
   - `dotnet test ... RuntimeMessageProcessorTests.Bridge_failure_with_deny_metadata_surfaces_diagnostics_in_error_envelope` (2/2 focused tests passed)
   - `cargo test -p activesync-host-ffi --test abi submit_command_ex_client_hello_peer_mismatch_returns_protocol_deny_metadata_json` (1 passed)
   - `cargo test -p activesync-host-ffi --test abi submit_command_ex_locked_room_missing_token_returns_auth_status_and_deny_metadata_json` (1 passed)
7. Auth enforcement remains opt-in at room scope: token checks are enforced only when a room is locked via `set-room-key`; unlocked rooms continue without token validation.
8. Host-ffi deny metadata now emits concrete command labels for a broad command set (not only control-plane/auth samples), reducing `command=unknown` fallback on protocol/auth deny paths.
9. Additional protocol-label validation passed:
   - `cargo test -p activesync-host-ffi --test abi submit_command_ex_subscribe_room_mismatch_returns_protocol_deny_metadata_with_subscribe_label` (1 passed)

## P2: Policy replication and replay correctness

Status:
1. Complete.
2. Task 1 complete: hybrid timeline model selected and documented in [POLICY_TIMELINE_ENCODING_DECISION.md](POLICY_TIMELINE_ENCODING_DECISION.md).
3. Tasks 2-4 complete in `activesync-core` (timeline replay order, compaction compatibility metadata, and fixture-backed transition tests).
4. Exit criteria met with replay + compaction + fixture validation and server replay CLI integration.

Acceptance criteria:
1. Policy timeline semantics defined and implemented.
2. Replay and compaction honor policy-at-time behavior.
3. Deterministic replay tests pass across policy transitions.

Tasks:
1. [x] Select policy timeline encoding model (in-graph, out-of-band versioned, or hybrid).
2. [x] Implement policy timeline application order in replay pipeline.
3. [x] Add compaction compatibility rules for policy history.
4. [x] Add replay golden tests for policy transitions.

Validation evidence:
1. `cargo test -p activesync-core replay::tests -- --nocapture` (13 passed).
2. `cargo test -p activesync-core compaction::tests -- --nocapture` (12 passed).
3. `cargo test -p activesync-core --test policy_timeline_fixtures -- --nocapture` (3 passed).
4. Replay timeline helpers exported: `replay_with_policy_timeline`, `PolicyTimelineEntry`, timeline hash/cutover helpers in `core/src/replay.rs` and `core/src/lib.rs`.
5. Compaction policy-history compatibility metadata implemented (`SNAP_POLICY_TIMELINE_HASH_KEY`, `SNAP_POLICY_CUTOVER_LAMPORT_KEY`) with verification compatibility helper in `core/src/compaction.rs`.
6. Fixture-backed policy transition coverage added under `core/tests/fixtures/policy_timeline/` and `core/tests/policy_timeline_fixtures.rs`.
7. Server operational replay now accepts optional policy timeline payload (`--policy-timeline <file>` or `--policy-timeline-json '<json>'`) and dispatches to `replay_with_policy_timeline` when provided (`server/src/main.rs`).
8. Server replay CLI parser coverage: `cargo test -p activesync-server --bin activesync-server -- --nocapture` (3 passed).

Dependencies:
1. P1 control-plane gating.

## P3: SDK ergonomics and developer APIs

Status:
1. Complete.
2. Task 1 complete: SDK namespace capability helper APIs landed (`capability`, `namespaceCapabilities`) with `tokenCaps` object-form support.
3. Tasks 2-4 complete: typed rejection surfaces plus intent/canonical and local-first refinement examples landed in SDK docs/API.
4. Stabilization pass complete: typed rejection parsing now supports canonical and legacy deny-message shapes.

Acceptance criteria:
1. Developers can configure common policy classes without low-level internals.
2. Intent/state split is represented in SDK API.
3. Rejection reasons are surfaced to app code consistently.

Tasks:
1. [x] Add SDK helper APIs for namespace protections.
2. [x] Add intent-vs-canonical examples and docs.
3. [x] Add rejection reason mapping and typed error surfaces.
4. [x] Add local-first examples with authoritative refinement.

Validation evidence:
1. SDK helper exports added in `web/sdk.js` and typed in `web/sdk.d.ts`.
2. SDK docs updated with helper usage and `tokenCaps` ergonomic object form in `docs/sdk.md`.
3. Typed rejection surfaces added to SDK runtime and types:
   - `doc.onRejection(cb)` and `doc.recentRejections(sinceMs?)`
   - `CreateDocOptions.onRejection`
   - `ActiveSyncRejectionError` (`err.rejection`) and `RejectionEvent` typings
4. SDK docs now include rejection handling examples tied to typed events, plus intent-vs-canonical and local-first authoritative refinement examples in `docs/sdk.md`.

Dependencies:
1. P1 and P2 semantics stable.

## P4: Multi-host conformance kit

Status:
1. Complete.
2. Initial conformance artifacts published (spec + reusable vectors).
3. Baseline executable conformance runner is implemented and generating normalized result artifacts from shared vectors.
4. Task 3 execution evidence is now complete for both Rust host and DotNetHost.
5. Operational hardening is the current focus (runner packaging/automation and periodic parity checks).
6. DotNetHost websocket endpoint stability hardening for auth-adjacent conformance tests has been applied and revalidated.
7. P4 implementation tasks are complete.
8. Canonical full-run evidence is green on current baseline artifacts.

Acceptance criteria:
1. Conformance suite runs against Rust host and DotNetHost.
2. Conformance runner packaging and automation workflow are documented and repeatable.
3. Core semantics are host-independent and verifiable.

Tasks:
1. [x] Define conformance test spec (authn/authz, replay, deny semantics).
2. [x] Implement reusable test vectors and expected outcomes.
3. [x] Execute and record results for Rust host and DotNetHost.
4. [x] Package conformance runner workflow for repeatable local/CI execution.
5. [x] Add periodic host parity checks (Rust + DotNetHost + npm SDK rejection surface consistency).
6. [x] Stabilize DotNetHost runtime websocket auth-adjacent tests used by targeted conformance slices.

P4 validation evidence:
1. Conformance spec published: `docs/AUTHORIZATION_CONFORMANCE_SPEC.md`.
2. Reusable vectors published: `docs/acceptance/authz-conformance-vectors.json`.
3. Baseline runner implemented: `cargo run -p activesync-server --bin authz-conformance-runner -- --vectors docs/acceptance/authz-conformance-vectors.json --out docs/acceptance/authz-conformance-results.json`.
4. Baseline result artifact produced: `docs/acceptance/authz-conformance-results.json` (16/16 pass).
5. Runner now executes Rust-host control-plane vectors via server `ws_handler` authorization helper and replay vectors via core `replay_with_policy_timeline`.
6. DotNetHost runtime/ffi adapter-backed authorization execution validated via targeted runtime tests with recorded TRX artifact:
   - Command: `dotnet test tests/ActiveSync.DotNetHost.Tests/ActiveSync.DotNetHost.Tests.csproj --filter "FullyQualifiedName~RuntimeMessageProcessorTests.Set_policy_without_capability_returns_control_plane_forbidden_error_envelope|FullyQualifiedName~RuntimeMessageProcessorTests.Start_tick_without_capability_returns_control_plane_forbidden_error_envelope|FullyQualifiedName~RuntimeTokenValidationServiceTests.ValidateInboundAsync_denies_when_provider_rejects_token|FullyQualifiedName~RuntimeTokenValidationServiceTests.ValidateInboundAsync_allows_when_provider_accepts_token|FullyQualifiedName~RuntimeWebSocketEndpointTests.Runtime_endpoint_set_policy_without_policy_admin_capability_is_denied_before_bridge_dispatch|FullyQualifiedName~RuntimeWebSocketEndpointTests.Runtime_endpoint_set_policy_with_policy_admin_capability_is_allowed|FullyQualifiedName~RuntimeWebSocketEndpointTests.Runtime_endpoint_start_tick_without_tick_admin_capability_is_denied_before_bridge_dispatch|FullyQualifiedName~RuntimeWebSocketEndpointTests.Runtime_endpoint_start_tick_with_tick_admin_capability_is_allowed" --logger "trx;LogFileName=authz-dotnet-conformance-targeted.trx" --results-directory "..\\docs\\acceptance" -v minimal`
   - Result counters: total=8 executed=8 passed=8 failed=0 notExecuted=0
   - Artifact: `docs/acceptance/authz-dotnet-conformance-targeted.trx`
7. Baseline refresh completed after handshake test hardening; targeted TRX counters remain stable (8/8 passing).
8. Guardrail protocol test added to pin hello bootstrap ordering/shape and prevent endpoint-test drift:
   - `RuntimeProtocolTests.Hello_handshake_guardrail_keeps_bootstrap_command_order_and_shapes`
9. Runtime websocket endpoint stability hardening validated:
   - `RuntimeWebSocketEndpointTests` file run: 48/48 passing
   - Focused repeat validation (5x each):
     - `Runtime_endpoint_duplicate_hello_returns_error_without_forcing_close`
     - `Runtime_endpoint_binary_frame_error_does_not_break_follow_on_noop`
     - `Runtime_endpoint_client_abort_during_receive_allows_follow_on_connection`
     - `Runtime_endpoint_partial_fragment_then_abort_allows_follow_on_connection`
     - Result: 20/20 passes, 0 failures
10. Canonical workflow helper script implemented at `docs/acceptance/Run-AuthzConformance.ps1`:
   - Executes Rust runner + DotNet targeted conformance slice
   - Emits canonical parity artifact `docs/acceptance/authz-conformance-parity.json`
   - Supports migration fallback to legacy artifact names during transition
11. Script validation completed (artifact-only mode):
   - `pwsh -File docs/acceptance/Run-AuthzConformance.ps1 -PolicyChannelMode admin_command_only -SkipRust -SkipDotNet`
   - Parity artifact emitted with `parity.status=pass`
12. Nightly CI workflow added: `.github/workflows/authz-conformance-nightly.yml`.
   - Blocking canonical job: `policy_channel_mode=admin_command_only`
   - Non-blocking supplemental matrix jobs: `hybrid`, `replicated_signed_ops_only`
   - All jobs publish conformance artifacts.
   - Canonical job now optionally emits and ingests `docs/acceptance/authz-auth-path-benchmark-result.json` from workflow-dispatch benchmark inputs.
   - Dedicated non-blocking CAPCOMP supplemental nightly job now emits `authz-conformance-parity-supp-capcomp.json` and related artifacts for promotion-streak evidence.
13. Nightly workflow now includes blocking npm SDK rejection-surface parity validation and artifact publication:
   - Script: `docs/acceptance/Check-SdkRejectionParity.mjs`
   - Artifact: `docs/acceptance/authz-conformance-sdk-parity.json`
14. Nightly workflow now includes a post-run promotion readiness summary job:
   - Script: `docs/acceptance/Summarize-AuthzPromotionReadiness.ps1`
   - Input artifacts: `authz-conformance-parity.json`, `authz-conformance-parity-supp-capcomp.json`
   - Output artifact: `docs/acceptance/promotion-readiness.json`
15. Nightly workflow now includes a promotion readiness history accumulator job:
   - Script: `docs/acceptance/Append-AuthzPromotionReadinessHistory.ps1`
   - Input artifact: `docs/acceptance/promotion-readiness.json`
   - Output artifacts: `_history/promotion-readiness-history.jsonl`, `_history/promotion-readiness-history-summary.json`
   - Historical state persists across nightly runs via workflow cache for streak proofing.
16. Promotion readiness summary now emits explicit gate verdict labels (`PASS_FOR_STREAK`, `FAIL`, `NOT_EVALUATED`) and history tracks gate-verdict streak counters.
17. CAPCOMP depth-overflow conformance vector and DotNet parity mapping/test coverage are now wired (`CAPCOMP-REJECT-LIMIT-DEPTH-001`).
18. Conformance vectors extended for:
   - policy channel mode behavior (`policy-channel` category)
   - actor/signer normalization contract checks (`identity` category)
19. Rust conformance runner now supports:
   - `--policy-channel-mode` flag
   - `policy-channel` and `identity` vector categories
   - per-record `policy_channel_mode` output metadata
20. Canonical conformance script now emits normalized DotNetHost records mapped from targeted TRX outcomes (`authz-conformance-dotnet-records*.json`) and performs record-level Rust-vs-DotNet parity checks for mapped vectors.
21. Full canonical workflow validation passed (`policy_channel_mode=admin_command_only`):
   - Rust runner: vectors=14, records=28, passed=28, failed=0 (`docs/acceptance/authz-conformance-rust.json`)
   - DotNet targeted slice: total=9, failed=0, passed=9 (`docs/acceptance/authz-conformance-dotnet.trx`)
   - DotNet normalized mapping artifact emitted (`docs/acceptance/authz-conformance-dotnet-records.json`, mapped_record_count=7)
   - Parity artifact: `status=pass`, `parity.status=pass`, `mismatches=none` (`docs/acceptance/authz-conformance-parity.json`)

Dependencies:
1. P0-P3 complete.

## P5: Capability inheritance/composition rollout (host-side)

Status:
1. In progress.
2. Architecture contract and separation-plan decisions are documented.
3. Conformance spec now includes composition parity contract and CAPCOMP supplemental vector IDs.
4. Rust host scaffold for capability profile loading and deterministic flattening is landed in server module + conformance runner.
5. Rust runtime admission + jwt-bridge minting paths now support profile-backed flattening when composition is enabled.
6. DotNetHost runtime/admission and provider mint/validate paths now mirror capability profile loader + flattening contract.
7. CAPCOMP DotNet test mappings are wired into supplemental parity records.
8. CAPCOMP parity artifact summaries now emit promotion-signal metadata (`not_evaluated`, `supplemental_pass`, `supplemental_fail`).
9. Auth-path benchmark threshold/result fields are now emitted in parity artifacts, with file-based benchmark evidence ingestion supported.
10. Compatibility-window profile-version checks are now explicit and testable via `supported_profile_versions` CAPCOMP vectors.
11. CAPCOMP negative vector coverage now includes unknown-reference, duplicate-node, edge-limit, payload-limit, and supported-version-schema failures.
12. Runtime-path compatibility-window behavior is covered in host runtime tests (Rust `ws_handler` admission path + `jwt-bridge` mint expansion path).
13. Benchmark evidence generation now supports direct hosted-benchmark JSON ingestion and computed regression fields.
14. Nightly workflow now emits gate verdict summary fields into logs and job summary (`$GITHUB_STEP_SUMMARY`).
15. Benchmark-backed qualified run evidence evaluates as streak-eligible (`PASS_FOR_STREAK`) with benchmark + CAPCOMP pass signals at `2/14`:
   - run summaries: `docs/acceptance/local-nightly-equivalent-qualified-20260522-03.json`, `docs/acceptance/local-nightly-equivalent-qualified-20260522-04.json`
   - promotion readiness: `docs/acceptance/promotion-readiness-local-qualified-20260522-03.json`, `docs/acceptance/promotion-readiness-local-qualified-20260522-04.json`
   - history summary: `docs/acceptance/_history/promotion-readiness-history-summary.json` (`capcomp_consecutive_passes=2`, `benchmark_consecutive_passes=2`, `combined_consecutive_passes=2`)
16. Cross-host benchmark matrix foundation is now present for optional auth/security/guardrail profile comparisons using shared SDK workloads:
   - matrix definition: `benchmarks/benchmark-matrix.v1.json`
   - runner: `benchmarks/Run-BenchmarkMatrix.ps1`
   - result schema: `benchmarks/results/benchmark-matrix-result.schema.v1.json`

Acceptance criteria:
1. Host capability profile schema and DAG validation rules are implemented and versioned.
2. Host expansion pipeline deterministically canonicalizes flattened capability sets before signing.
3. Unknown profile/cycle/overflow/grammar failures are deterministic and parity-tested across Rust host and DotNetHost.
4. Core evaluator remains unchanged and perf regression checks stay within defined benchmark thresholds.

Tasks:
1. [x] Define and publish host capability profile schema (nodes, edges, version, limits, validation errors) in [AUTHORIZATION_CAPABILITY_PROFILE_SCHEMA.md](AUTHORIZATION_CAPABILITY_PROFILE_SCHEMA.md).
2. [x] Land Rust host scaffold for profile loader + DAG expansion + canonical flattening pipeline (`server/src/capability_profile.rs`, conformance runner integration).
3. [x] Implement DotNetHost equivalent profile loader + DAG expansion + canonical flattening pipeline.
4. [x] Add capability composition vectors (`CAPCOMP-*`) to `docs/acceptance/authz-conformance-vectors.json`.
5. [x] Extend conformance harnesses/scripts to execute composition supplemental profile runs and parity checks.
6. [x] Add deterministic failure mapping for unknown-profile/cycle/depth/count/grammar violations (host-level reason detail parity beyond current stable class mapping).
7. [ ] Add stable auth-path benchmark comparison before/after host expansion rollout and collect 14-run qualified gate evidence streak (field emission + benchmark evidence generator + nightly ingestion wiring are complete; current progress: 2/14).
8. [x] Wire Rust host runtime token-minting path to use profile-backed flattening when capability composition is enabled.
9. [x] Add CAPCOMP parity summary fields and promotion signal metadata to parity artifacts.
10. [x] Add compatibility-window CAPCOMP vectors and parity mappings for supported/unsupported profile-version behavior.
11. [x] Add missing CAPCOMP negative vectors and host parity tests for unknown-reference, duplicate-node, edge-limit, payload-limit, and supported-version-schema failure classes.
12. [x] Add runtime-path compatibility-window tests for Rust ws admission and jwt-bridge mint expansion paths.
13. [x] Automate benchmark comparison ingestion from benchmark harness JSON outputs.
14. [x] Add nightly gate-summary emission in workflow logs/job summary.

Promotion gates (supplemental -> release-gating):
1. CAPCOMP supplemental parity passes for Rust host and DotNetHost with zero mapped status mismatches across all `CAPCOMP-*` vectors for 14 consecutive qualified runs.
2. CAPCOMP supplemental parity artifacts include CAPCOMP summary metadata (`vector_count`, mapped/pass/fail counts, mismatch count, promotion signal).
3. Deterministic failure mapping for unknown-profile/cycle/depth/count/grammar is stabilized and equivalent across hosts.
4. Auth-path benchmark gate is satisfied for 14 consecutive qualified runs with explicit evidence fields:
   - `auth_path_benchmark_thresholds.p50_latency_regression_pct_max = 5`
   - `auth_path_benchmark_thresholds.p95_latency_regression_pct_max = 10`
   - `auth_path_benchmark_thresholds.alloc_regression_pct_max = 10`
   - `auth_path_benchmark_result.p50_latency_regression_pct <= p50_latency_regression_pct_max`
   - `auth_path_benchmark_result.p95_latency_regression_pct <= p95_latency_regression_pct_max`
   - `auth_path_benchmark_result.alloc_regression_pct <= alloc_regression_pct_max`
   - `auth_path_benchmark_result.baseline_run_id` and `auth_path_benchmark_result.candidate_run_id` are non-empty and traceable to stored benchmark artifacts.
   - `auth_path_benchmark_result.status = pass`.
5. No CAPCOMP-related websocket/runtime instability regressions in targeted host test slices.

Dependencies:
1. P4 canonical workflow remains green.
2. Separation-plan capability composition contract in [AUTHORIZATION_CORE_HOST_SEPARATION_PLAN.md](AUTHORIZATION_CORE_HOST_SEPARATION_PLAN.md).

## 5. Remaining Backlog (Execution-Focused)

Priority 0:
1. Keep canonical conformance workflow and artifacts healthy in CI (regression maintenance).
2. Monitor nightly parity signals across Rust host, DotNetHost, and npm SDK rejection surface.

Priority 1:
1. Produce and retain 14-run CAPCOMP + benchmark gate streak evidence in history artifacts (qualified runs; nightly + integration-triggered).
2. Confirm benchmark evidence sourcing from hosted harness artifacts in nightly dispatches and tune defaults as needed.

Priority 2:
1. Extend conformance vectors for intent-vs-canonical authority flow and protected namespace enforcement.
2. Keep websocket/abort-path stability guardrails in targeted conformance slices to prevent false-negative parity runs.
3. Evaluate promotion criteria for composition vectors from supplemental to release-gating.

## 6. Risk Register

1. Risk: control-plane commands remain writable by non-admin actors.
   Mitigation: P1 gating + integration tests + deny counters.
2. Risk: policy timeline drift across replay/compaction.
   Mitigation: replay golden tests and explicit policy version ordering.
3. Risk: host-specific behavior divergence.
   Mitigation: conformance suite and shared test vectors.
4. Risk: overgrowth into RBAC complexity.
   Mitigation: keep core model capability and namespace oriented.

## 7. Weekly Update Template

Week of:
1. Completed:
2. In progress:
3. Blockers:
4. Next week plan:
5. Risks changed:

## 8. Immediate Next Actions (This Sprint)

1. Kick off Platform Foundation execution cycle from [PLATFORM_FOUNDATION_EXECUTION_PLAN.md](PLATFORM_FOUNDATION_EXECUTION_PLAN.md).
2. Phase A (Scoped Replication v1):
   - [x] Add scope conformance vectors and runner support.
   - [x] Add scoped filtering metrics + parity artifact fields.
   - [x] Add bounded catch-up behavior checks for large-room joins.
3. Phase B (Snapshot/Replay Hardening v1):
   - [x] Add deterministic snapshot restore drill script and CI step.
   - [x] Publish compaction cadence and replay truncation operator guidance.
4. Phase C (Identity Continuity/Rotation v1):
   - [x] Publish continuity contract and overlap-window semantics.
   - [x] Add host validation seam + conformance vectors for continuity success/failure.
5. Phase D (Conformance + CAPCOMP stabilization):
   - Maintain canonical nightly gate health.
   - Complete CAPCOMP + benchmark 14-run streak evidence accumulation.

## 9. Platform Foundation Cycle Status (2026-05-22 Kickoff)

1. Plan published: [PLATFORM_FOUNDATION_EXECUTION_PLAN.md](PLATFORM_FOUNDATION_EXECUTION_PLAN.md).
2. Execution mode: workspace/product progress in parallel with foundation-only platform increments.
3. Current active implementation tranche:
   - CAPCOMP/benchmark promotion streak evidence tracking (Phase D)
   - Phase B restore drill CI evidence accrual (first nightly proof pending)
   - Phase C continuity v1 admission/conformance hardening (slice 2 complete)
4. Phase C slice 1 completion evidence (2026-05-22):
   - Published continuity contract doc `docs/IDENTITY_CONTINUITY_V1_CONTRACT.md` (token continuity shape, overlap semantics, revocation behavior).
   - Added optional host admission seam in `server/src/ws_handler.rs` to validate continuity-v1 metadata when present.
   - Added conformance vectors `IDENTITY-CONTINUITY-001..003` in `docs/acceptance/authz-conformance-vectors.json`.
   - Extended Rust conformance runner identity evaluator with deterministic continuity outcomes for overlap/expiry/revocation cases.
5. Phase C slice 2 completion evidence (2026-05-22):
   - Added DotNet runtime continuity validation + passthrough for `hello.token.continuity` in `nodalmerge-host/src/ActiveSync.DotNetHost/Runtime/RuntimeProtocolMapper.cs`.
   - Added DotNet targeted tests for continuity allow/expiry/revoked cases in `nodalmerge-host/tests/ActiveSync.DotNetHost.Tests/RuntimeProtocolTests.cs`.
   - Wired explicit continuity vector mappings in `docs/acceptance/Run-AuthzConformance.ps1` for:
     - `IDENTITY-CONTINUITY-001`
     - `IDENTITY-CONTINUITY-002`
     - `IDENTITY-CONTINUITY-003`
   - Added concrete SDK + host migration flow docs for device switch and key rotation in `docs/sdk.md` and `nodalmerge-host/README.md`.
   - Local nightly-equivalent rerun passed with continuity vectors present in DotNet mapped records (`authz-conformance-dotnet-records.json`) and canonical parity `status=pass`.
6. Phase B slice 1 completion evidence (2026-05-22):
   - Added deterministic drill script `docs/acceptance/Run-SnapshotRestoreDrill.ps1` producing `snapshot-restore-drill.json`.
   - Canonical nightly workflow now runs snapshot restore drill and uploads artifact in `authz-conformance-nightly.yml`.
   - Deployment runbook now includes compaction cadence, replay truncation watermark semantics, and rollback flow in `docs/deployment.md`.
   - Local drill validation passed with both required assertions (`snapshot_restore_forward_replay=true`, `snapshot_hash_equality=true`).
7. Phase A completion evidence (2026-05-22):
   - Scope parity slice is green with full Rust + DotNet mapped coverage (`scope.vector_count=4`, `scope.dotnet.mapped_record_count=4`, `scope.parity_mismatch_count=0`).
   - Rust runtime scoped filtering counters are instrumented (`activesync_filtered_nodes_total`, `activesync_filtered_bytes_total`, `activesync_filtered_pack_dropped_total`).
   - Canonical parity artifact emits scoped runtime metric rollups under `scope.runtime_metrics.rust.*`.
   - Large-room catch-up budget checks are enforced with deterministic drop fallback via `ACTIVESYNC_SCOPE_MAX_FILTERED_CATCHUP_NODES` and `ACTIVESYNC_SCOPE_MAX_FILTERED_CATCHUP_BYTES`.
