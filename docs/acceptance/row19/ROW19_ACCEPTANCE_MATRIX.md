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
| R19-SS-001 | 3,4,5,6 | SpeechSlate-shape core data flows | speechslate-direct OR speechslate-proxy | Mongo + S3/delegated shape configured per docs/integration.md | Map/text/list/blob flows function against host-owned backend with same convergence semantics | Pending | Pending | Primary row 19 SpeechSlate-shape proof |
| R19-SS-002 | 6,7 | SpeechSlate-shape blob path | speechslate-direct OR speechslate-proxy | Presign/delegate path configured or fallback explicitly active | Direct path works when configured; fallback path deterministic when unavailable | Pending | Pending | Must capture chosen mode in evidence |
| R19-SS-003 | 10,11 | Policy/auth semantics in SpeechSlate-shape | speechslate-direct OR speechslate-proxy | Locked room/token + policy setup | Authorized writes succeed; invalid token/policy violations reject predictably | Pending | Pending | Confirms enterprise path behavior |
| R19-SS-004 | 17 | Transport policy (ws-only/auto) | speechslate-direct OR speechslate-proxy | Toggle transport options in SDK init | WS-first behavior remains stable; optional mesh does not break parity | Pending | Pending | Boundary regression check |

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
4. Hosted service health:
   - `GET http://127.0.0.1:7878/ffi/abi-version` -> `{"abiVersion":1}`.
   - `GET http://127.0.0.1:8080/index.html` -> `200`.
5. Runtime parity blocker resolution evidence:
   - Same-room pack relay test.
   - Same-room peer-left test.
   - Cross-room relay isolation test.

Evidence pointers (code/tests):

1. `dotnet-host/tests/ActiveSync.DotNetHost.Tests/RuntimeWebSocketEndpointTests.cs` (relay and peer lifecycle coverage).
2. `dotnet-host/src/ActiveSync.DotNetHost/Runtime/RuntimeRoomBroker.cs` (room membership + broadcast broker).
3. `dotnet-host/src/ActiveSync.DotNetHost/Runtime/RuntimeWebSocketLoopRunner.cs` (room-aware registration/relay logic).

## Execution note

When full SpeechSlate runtime is unavailable in this workspace, mark `speechslate-direct` scenarios as `Blocked-External` and execute `speechslate-proxy` equivalents with explicit boundary notes.
