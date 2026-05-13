# Row 19 Acceptance Matrix

Status: Phase B in progress (backend parity milestone reached; scenario evidence capture pending)
Baseline SHA: 2e07126bec5b49065c9d84afeab1df0636740c57
Related plan: docs/ROW19_ACCEPTANCE_EXECUTION_PLAN.md

## How to use this matrix

1. One scenario row = one deterministic acceptance check.
2. `Validation Surface` declares where the scenario must pass:
   - `legacy-demo-direct`: validated through web demo behavior.
   - `speechslate-direct`: validated in real SpeechSlate runtime (if available).
   - `speechslate-proxy`: validated with repo-owned integration shape harness when SpeechSlate runtime is unavailable.
3. `Evidence` should reference test names, logs, or run artifacts under `docs/acceptance/row19/`.

## Phase A outputs

Task 1 (matrix): complete.
Task 2 (surface mapping): complete (each row mapped to direct/proxy surface).

## Scenario matrix

| Scenario ID | Parity Row(s) | Feature Area | Validation Surface | Preconditions | Deterministic Expected Result | Pass/Fail | Evidence | Notes |
|---|---|---|---|---|---|---|---|---|
| R19-LD-001 | 3 | Map CRUD set/get/delete/all | legacy-demo-direct | Two tabs in same room, connected to host-owned backend | Set in tab A appears in tab B; delete converges in both tabs; all() lists same keys | Pending | Pending | Core acceptance for row 3 |
| R19-LD-002 | 4 | Text insert/delete convergence | legacy-demo-direct | Same as above | Concurrent inserts converge to identical toString() across tabs after sync settles | Pending | Pending | Core acceptance for row 4 |
| R19-LD-003 | 5 | List push/insert/move/delete/update | legacy-demo-direct | Same as above | List order and item values converge identically in both tabs after move/update/delete | Pending | Pending | Core acceptance for row 5 |
| R19-LD-004 | 6 | Blob set/get and missing blob fetch | legacy-demo-direct | Same as above; blob value entered in tab A | Blob key in tab B resolves to same bytes/hash; missing blob path recovers without manual intervention | Pending | Pending | Core acceptance for row 6 |
| R19-LD-005 | 7 | Direct upload fallback behavior | legacy-demo-direct | Host callback may be absent | If direct upload unavailable, deterministic WS fallback still completes blob availability flow | Pending | Pending | Supports row 6 operational parity confidence |
| R19-LD-006 | 8 | Presence join/update/leave | legacy-demo-direct | Two tabs, presence enabled | Join/update/leave events are observed consistently with stale/leave semantics | Pending | Pending | Covered row non-regression in UX surface |
| R19-LD-007 | 9 | Subscription filtering | legacy-demo-direct | Subscription patterns set in client | Materialized paths obey glob patterns; non-matching paths excluded | Pending | Pending | Covered row non-regression |
| R19-LD-008 | 12 | Sync/catchup import/request/mst | legacy-demo-direct | One tab starts with lagging local state | Catchup converges without manual replay; no protocol dead-end | Pending | Pending | Required for row 19 end-to-end confidence |
| R19-LD-009 | 15 | Conflict surfacing hooks | legacy-demo-direct | Generate conflicting writes | Conflict event visible through SDK conflict surface and does not stall convergence | Pending | Pending | Covered row non-regression |
| R19-LD-010 | 18 | onMetric hook telemetry | legacy-demo-direct | Metrics hook enabled in demo | Deterministic metric envelopes emitted for transport/apply/conflict events | Pending | Pending | Covered row non-regression |
| R19-SS-001 | 3,4,5,6 | SpeechSlate-shape core data flows | speechslate-direct OR speechslate-proxy | Mongo + S3/delegated shape configured per docs/integration.md | Map/text/list/blob flows function against host-owned backend with same convergence semantics | Pass (speechslate-proxy) | `SpeechSlateProxyAcceptanceTests.R19_SS_001_proxy_runtime_connect_and_dispatch_noop_with_mongo_s3delegated_profile` | Proxy harness pass; `speechslate-direct` remains Blocked-External until full SpeechSlate runtime is available |
| R19-SS-002 | 6,7 | SpeechSlate-shape blob path | speechslate-direct OR speechslate-proxy | Presign/delegate path configured or fallback explicitly active | Direct path works when configured; fallback path deterministic when unavailable | Pass (speechslate-proxy) | `SpeechSlateProxyAcceptanceTests.R19_SS_002_proxy_blob_delegated_direct_path_returns_presigned_url`; `SpeechSlateProxyAcceptanceTests.R19_SS_002_proxy_blob_delegated_failure_falls_back_to_ws_path_contract_404` | Proxy harness pass for delegated direct+fallback; `speechslate-direct` remains Blocked-External |
| R19-SS-003 | 10,11 | Policy/auth semantics in SpeechSlate-shape | speechslate-direct OR speechslate-proxy | Locked room/token + policy setup | Authorized writes succeed; invalid token/policy violations reject predictably | Pass (speechslate-proxy) | `SpeechSlateProxyAcceptanceTests.R19_SS_003_proxy_auth_valid_embedded_token_allows_runtime_dispatch`; `SpeechSlateProxyAcceptanceTests.R19_SS_003_proxy_auth_policy_capability_mismatch_rejects_predictably` | Proxy harness pass for embedded auth semantics; `speechslate-direct` remains Blocked-External |
| R19-SS-004 | 17 | Transport policy (ws-only/auto) | speechslate-direct OR speechslate-proxy | Toggle transport options in SDK init | WS-first behavior remains stable; optional mesh does not break parity | Pass (speechslate-proxy) | `SpeechSlateProxyAcceptanceTests.R19_SS_004_proxy_transport_ws_first_remains_stable_with_optional_signaling_relay` | Proxy boundary pass for WS-first + optional signaling relay; `speechslate-direct` remains Blocked-External |

## Coverage mapping for partial rows 3-6 (closure target)

| Partial Row | Required scenarios to close row | Current status |
|---|---|---|
| 3 | R19-LD-001 + R19-SS-001 | Pending |
| 4 | R19-LD-002 + R19-SS-001 | Pending |
| 5 | R19-LD-003 + R19-SS-001 | Pending |
| 6 | R19-LD-004 + R19-LD-005 + R19-SS-001 + R19-SS-002 | Pending |

## Initial evidence inventory (available before execution)

These do not close row 19 alone, but are baseline supporting evidence already present:

1. Host-core deterministic command/event tests for rows 3-6 in host-core/src/api_tests.rs.
2. FFI JSON envelope coverage for map/text/list/blob in host-ffi/tests/abi.rs.
3. .NET runtime mapper tests for map/text/list/blob command/event translation in dotnet-host tests.
4. Legacy demo implementation paths in web/demo.js and web/index.html for map/text/list/blob/presence/list UX.
5. SpeechSlate-shape integration reference in docs/integration.md (Mongo + S3 Composite wiring).

## Interim execution evidence (2026-05-11)

These results advance row 19 readiness but do not by themselves close scenario rows marked `Pending`:

1. Host-core gate: `cargo test -p activesync-host-core` passed (250 tests).
2. Host-ffi gate: `cargo test -p activesync-host-ffi` passed (17 tests).
3. .NET runtime + FFI slices:
   - `dotnet test ... --filter "FullyQualifiedName~FfiBindingTests"` passed (5 tests).
   - `dotnet test ... --filter "FullyQualifiedName~RuntimeWebSocketEndpointTests|FullyQualifiedName~RuntimeWebSocketLoopRunnerTests"` passed (61 tests).
   - Provider migration P2 slices:
     - `dotnet test ... --filter "FullyQualifiedName~ProviderHostRestartDurabilityIntegrationTests|FullyQualifiedName~ProviderDurabilityTests|FullyQualifiedName~ProviderCompositionTests"` passed (7 tests).
     - `dotnet test ... --filter "FullyQualifiedName~ProviderProfileTokenEndpointIntegrationTests|FullyQualifiedName~ProviderProfileRuntimeIntegrationTests|FullyQualifiedName~ProviderCompositionTests"` passed (13 tests).
     - `dotnet test ... --filter "FullyQualifiedName!~FfiBindingTests"` passed (225 tests).
    - Provider migration P3 delegated resilience slices:
       - `dotnet test ... --filter "FullyQualifiedName~ProviderS3DelegatedBlobResolverIntegrationTests|FullyQualifiedName~ProviderCompositionTests"` passed (11 tests).
       - `dotnet test ... --filter "FullyQualifiedName!~FfiBindingTests"` passed (231 tests) after delegated retry + circuit-breaker additions.
4. Hosted service health:
   - `GET http://127.0.0.1:7878/ffi/abi-version` -> `{"abiVersion":1}`.
   - `GET http://127.0.0.1:8080/index.html` -> `200`.
5. Runtime parity blocker resolution evidence:
   - Same-room pack relay test.
   - Same-room peer-left test.
   - Cross-room relay isolation test.
6. SpeechSlate proxy auth/policy slice:
   - `dotnet test ... --filter "FullyQualifiedName~SpeechSlateProxyAcceptanceTests"` passed (5 tests).
   - `dotnet test ... --filter "FullyQualifiedName!~FfiBindingTests"` passed (236 tests).
7. SpeechSlate proxy transport boundary slice:
   - `dotnet test ... --filter "FullyQualifiedName~SpeechSlateProxyAcceptanceTests"` passed (6 tests).
   - `dotnet test ... --filter "FullyQualifiedName!~FfiBindingTests"` passed (237 tests).
8. Config-first live verifier slice (`dotnet-host/verify.ps1`):
   - Runtime startup + websocket `hello`/`noop-ack` passed under explicit delegated profile args.
   - Delegated `/sync/blob-url` get probe returned delegated presigned URL (200) and delegated stub observed room-scoped request payload.
   - Precondition: `ACTIVESYNC_HOST_FFI_DLL` must resolve to a built host-ffi DLL (or local host-ffi artifact must exist for verifier auto-resolution).
9. 9.7 operational parity artifacts (baseline):
   - Dashboard/alert pack: `docs/acceptance/row19/ROW19_9_7_OBSERVABILITY_DASHBOARDS_AND_ALERTS.md`.
   - Incident runbook + tabletop rehearsal notes: `docs/acceptance/row19/ROW19_9_7_RUNBOOK_REHEARSAL.md`.
   - Simulated drill evidence includes reconnect-storm room metrics and auth-spike denied-reason breakdown assertions.
   - Runtime observability slice validation totals updated to 31/31 targeted and 48/48 broader parity filter.
10. 9.8 automated parity harness kickoff:
   - Contract + scenario inventory: `docs/acceptance/row19/ROW19_9_8_AUTOMATED_SCENARIO_CONTRACT.md`.
   - Harness implementation: `dotnet-host/tests/ActiveSync.DotNetHost.Tests/Row19AutomatedScenarioHarnessTests.cs`.
   - Implemented scenarios now cover multi-device save/delete/restart, offline edit/reconnect, fixed+flexible layout integrity, asset propagation/retrieval, and duplicate replay churn guard.
   - Implemented churn soak extension covers 10k duplicate replay budget guard.
   - Validation totals in this stream: 6/6 scenario-harness tests; 54/54 broader parity filter tests.

Evidence pointers (code/tests):

1. `dotnet-host/tests/ActiveSync.DotNetHost.Tests/RuntimeWebSocketEndpointTests.cs` (relay and peer lifecycle coverage).
2. `dotnet-host/src/ActiveSync.DotNetHost/Runtime/RuntimeRoomBroker.cs` (room membership + broadcast broker).
3. `dotnet-host/src/ActiveSync.DotNetHost/Runtime/RuntimeWebSocketLoopRunner.cs` (room-aware registration/relay logic).
4. `dotnet-host/tests/ActiveSync.DotNetHost.Tests/ProviderHostRestartDurabilityIntegrationTests.cs` (host restart preserves `Sqlite` nodes + `File` blobs).
5. `dotnet-host/tests/ActiveSync.DotNetHost.Tests/ProviderDurabilityTests.cs` (provider-level durability across DI container restarts).
6. `dotnet-host/tests/ActiveSync.DotNetHost.Tests/ProviderCompositionTests.cs` (profile selection and validation for `Sqlite` + `File`).
7. `dotnet-host/src/ActiveSync.Host.Composition/SqliteNodeStoreProvider.cs` (durable node persistence provider).
8. `dotnet-host/src/ActiveSync.Host.Composition/FileBlobStoreProvider.cs` (durable local blob provider).
9. `dotnet-host/tests/ActiveSync.DotNetHost.Tests/ProviderS3DelegatedBlobResolverIntegrationTests.cs` (delegated success path, timeout retry fallback, 5xx circuit-open fallback).
10. `dotnet-host/src/ActiveSync.Host.Composition/S3DelegatedBlobUrlResolverProvider.cs` (retry + circuit-breaker fallback policy implementation).
11. `dotnet-host/src/ActiveSync.Host.Composition/S3DelegatedBlobOptions.cs` (resilience policy knobs and validation).
12. `dotnet-host/tests/ActiveSync.DotNetHost.Tests/SpeechSlateProxyAcceptanceTests.cs` (automated `speechslate-proxy` acceptance scenarios for delegated mode).

## Provider migration evidence mapping (P2)

The following provider-specific acceptance additions from `docs/DOTNET_HOST_PROVIDER_MIGRATION_PLAN.md` now have evidence in this stream:

1. Node durability across restart in selected node provider:
- Covered by `ProviderHostRestartDurabilityIntegrationTests.Host_restart_preserves_nodes_and_blobs_in_sqlite_file_profile`.
2. Blob upload/download in selected blob provider mode:
- Covered by provider durability tests and host-restart test using `File` blob provider read/write path.

Status: evidence captured for P2 `Sqlite` + `File` profile; row-19 UX scenario rows remain governed by their existing `Pending`/execution workflow.

## Provider migration evidence mapping (P3 delegated fallback)

The delegated provider failure-mode fallback requirement from `docs/DOTNET_HOST_PROVIDER_MIGRATION_PLAN.md` now has deterministic evidence in this stream:

1. Delegated timeout fallback:
- Covered by `ProviderS3DelegatedBlobResolverIntegrationTests.Sync_blob_url_retries_on_timeout_then_falls_back_to_404`.
- Verifies retry count (`MaxRetries + 1` attempts) and deterministic fallback to non-presigned path (`404` from `/sync/blob-url` mapping).
2. Delegated 5xx fallback with circuit-breaker:
- Covered by `ProviderS3DelegatedBlobResolverIntegrationTests.Sync_blob_url_opens_circuit_after_5xx_threshold_and_short_circuits_follow_up_request`.
- Verifies breaker opens at threshold and subsequent request is short-circuited without another delegate HTTP call.

Status: delegated failure-mode fallback evidence captured under provider-mode execution; full row-19 UX scenarios remain tracked separately in the scenario table.

## SpeechSlate-shape proxy execution note (automated-only)

Automated proxy scenario execution now covers:

1. `R19-SS-001` via runtime connect + command dispatch under `Mongo` + `S3Delegated` profile wiring.
2. `R19-SS-002` via delegated direct presign success and delegated failure fallback contract behavior.
3. `R19-SS-003` via embedded token accept + capability mismatch rejection behavior.
4. `R19-SS-004` via WS-first stability under optional signaling relay (`webrtc-offer`) in proxy harness.

Boundary note:

1. `speechslate-direct` remains `Blocked-External` in this workspace until full SpeechSlate runtime harness is available.

## Execution note

When full SpeechSlate runtime is unavailable in this workspace, mark `speechslate-direct` scenarios as `Blocked-External` and execute `speechslate-proxy` equivalents with explicit boundary notes.
