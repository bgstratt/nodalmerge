# ActiveSync .NET Host Prototype

This folder contains the PR7 prototype for a host-owned .NET runtime that calls the Rust host FFI library.

## Projects

- src/ActiveSync.DotNetHost: ASP.NET minimal host with FFI endpoints.
- tests/ActiveSync.DotNetHost.Tests: smoke-style binding tests.

## Endpoints

- GET /: host metadata.
- GET /ffi/abi-version: returns ABI version from as_host_abi_version.
- POST /ffi/submit: forwards raw binary command payload to as_host_submit_command and returns raw event bytes.
- GET /ws/ffi (WebSocket upgrade): host-owned websocket binary bridge.
- GET /ws/runtime (WebSocket upgrade): typed runtime bridge (text JSON in/out) mapped to host commands/events.
- GET /ws/{roomId} (WebSocket upgrade): compatibility alias to the runtime bridge so SDK/demo clients that connect to `/ws/<room>` can target this host without URL-shape changes.

### WebSocket Bridge Behavior

- Accepts only binary websocket messages.
- Each binary message is treated as one host-ffi command envelope payload.
- Successful submit replies with one binary websocket message containing host-ffi event envelope bytes.
- Non-binary frames receive a text error message and the socket remains open.
- Failed submit replies with a text error payload containing the mapped FFI status.

### Runtime WebSocket Behavior (Step 38)

- Accepts only text JSON websocket messages.
- Message mapping currently supports:
	- hello: maps to EnsureRoom + OpenSession + ClientHello host command sequence.
	- ensure-room: maps directly to EnsureRoom.
	- open-session: maps directly to OpenSession.
	- client-hello: maps directly to ClientHello.
	- map-set: maps directly to MapSet (`namespace`, `key`, `value`) and returns `map-set-ack`.
	- map-get: maps directly to MapGet (`namespace`, `key`) and returns `map-value`.
	- map-delete: maps directly to MapDelete (`namespace`, `key`) and returns `map-delete-ack`.
	- map-all: maps directly to MapAll (`namespace`) and returns `map-all` entries.
	- text-insert: maps directly to TextInsert (`namespace`, `key`, `after_id`, `ch`) and returns `text-insert-ack`.
	- text-delete: maps directly to TextDelete (`namespace`, `key`, `target_id`) and returns `text-delete-ack`.
	- text-get: maps directly to TextGet (`namespace`, `key`) and returns `text-value`.
	- list-push: maps directly to ListPush (`namespace`, `key`, `value`) and returns `list-push-ack`.
	- list-insert: maps directly to ListInsert (`namespace`, `key`, `index`, `value`) and returns `list-insert-ack`.
	- list-delete: maps directly to ListDelete (`namespace`, `key`, `index`) and returns `list-delete-ack`.
	- list-move: maps directly to ListMove (`namespace`, `key`, `from_index`, `to_index`) and returns `list-move-ack`.
	- list-update: maps directly to ListUpdate (`namespace`, `key`, `index`, `value`) and returns `list-update-ack`.
	- list-get: maps directly to ListGet (`namespace`, `key`) and returns `list-value`.
	- blob-set: maps directly to BlobSet (`namespace`, `hash`, `data_b64`) and returns `blob-set-ack`.
	- blob-get: maps directly to BlobGet (`namespace`, `hash`) and returns `blob-value`.
	- blob-get-many: maps directly to BlobGetMany (`namespace`, `hashes[]`) and returns `blob-values` (`entries[]`, `missing[]`).
	- request-upload: maps directly to RequestUpload (`namespace`, `hash`, `size`, optional `content_type`) and returns `upload-granted` (`url`, `expires_at_unix`) or `upload-denied`.
	- blob-request: maps directly to BlobRequest (`namespace`, `hashes[]`) and returns either `blob-redirect` (`redirects[]`) or `blob-pack` (`blobs[]`, `requested[]`).
	- presence / presence-set: maps to PresenceSet (`session_id`, `data`, optional `ttl_ms`, optional `now_unix_ms`) and returns `presence` (`from`, `data`, `joined`).
	- presence-get: maps to PresenceGetAll and returns `presence-snapshot` (`entries[]`).
	- presence-sweep: maps to PresenceSweep (`now_unix_ms`) and returns `presence-leave` for stale removals.
	- subscribe: maps to Subscribe (`session_id`, `patterns[]`) and returns `subscribe-ack`.
	- set-room-key: maps to SetRoomKey (`pubkey`) and returns `room-locked` or `set-room-key-rejected`.
	- set-policy: maps to SetPolicy (`default`, `rules[]`) and returns `policy-set` or `set-policy-rejected`.
	- pack: maps to ImportPack (`nodes`) and returns `pack-ack`.
	- request / request-server-pack: maps to RequestServerPack (`known[]`) and returns `pack` (`from`, `nodes`, `root`).
	- mst-request: maps to MstRequest (`paths[]`) and returns `mst-response` (`nodes[]`).
	- mst-done: maps to MstDone (`ids[]`) and returns `pack` (`from`, `nodes`, `root`).
	- recent-conflicts: maps to GetRecentConflicts (`since_unix_ms`) and returns `recent-conflicts` (`entries[]`).
	- webrtc-offer / webrtc-answer / webrtc-ice: map to RelayPeerSignal (`session_id`, `msg_type`, `to`, signaling payload) and relay back as peer-stamped signaling frames (`from`, `to`, payload).
	- text-insert-at: runtime helper that resolves `index` to an anchor by issuing TextGet, then dispatches TextInsert (`namespace`, `key`, `index`, `ch`) and returns `text-insert-ack`.
	- text-delete-at: runtime helper that resolves `index` to `target_id` by issuing TextGet, then dispatches TextDelete (`namespace`, `key`, `index`) and returns `text-delete-ack`.
	- noop: maps to Noop host command.
	- close-session: maps to CloseSession host command and closes the socket only when dispatch succeeds.
	- Conflict events from host-core map to `conflict` (streamed, one frame per entry) and `recent-conflicts` (snapshot list) for SDK `onConflict` and recent-history parity.
- `session_id` behavior: if `hello`, `open-session`, `client-hello`, or `close-session` provides `session_id`, the runtime mapper persists that value in connection state and reuses it for subsequent commands when omitted.
- `hello.token` is forwarded into the host `ClientHello` payload when provided (`peer_pubkey`, `expiry`, `caps[]`, `sig`). Missing required token fields are rejected by the mapper.
- Connection affinity is enforced after initialization: explicit `room`/`pubkey` values in follow-on commands cannot switch away from the initialized connection identity.
- Host events are translated back to typed runtime responses (welcome, session-opened, session-closed, noop-ack).
- Unsupported message types return an error message (keeps connection open).
- Runtime error frames are serialized through typed JSON envelope builders so error text/status values are JSON-safe (no malformed frames from quote/newline content).
- Offline persistence boundary: this runtime host does not own browser IndexedDB hydrate/save behavior. Local offline persistence remains an SDK/browser concern; runtime host responsibilities stop at sync command/event translation and transport orchestration.
- Undo boundary: this runtime host does not implement `undoManager` policy/state machines. Undo/redo remains SDK/app-layer compensating-op behavior over host runtime command/event primitives.
- Transport boundary: reconnect/backoff policy and `transport: auto | ws-only` selection remain SDK transport concerns. This runtime host is WS-first authoritative and exposes optional signaling relay verbs used by SDK WebRTC mesh flows.
- Metrics boundary: app-facing `onMetric` callbacks are SDK-owned and server operational telemetry export remains adapter-owned (for example Prometheus endpoint wiring). This runtime host does not define a separate metrics wire command/event stream.
- Runtime command dispatch orchestration is centralized in `RuntimeMessageProcessor` so inbound mapping, bridge submission, host-event translation, and close-policy decisions are exercised by unit tests without websocket harnessing.
- Runtime websocket frame processing is centralized in `RuntimeFrameProcessor` so message-type gating (`text` required) and frame-to-runtime dispatch behavior can be tested independently of the socket receive loop.
- Runtime websocket receive/send/close loop orchestration is centralized in `RuntimeWebSocketLoopRunner`, enabling integration-style loop tests over fake websocket transport (frame ingress, outbound error/message emission, and server/client close semantics).
- Host app composition and endpoint mapping are centralized in `HostApplication.Build(...)`, allowing in-memory TestServer websocket endpoint tests for `/ws/runtime` with service overrides (for example, replacing `IRuntimeCommandBridge` in tests).
- `/ws/ffi` binary command dispatch now resolves through `IFfiBinaryBridge` (implemented by `FfiBridgeProcessor`) so endpoint tests can override binary bridge behavior without loading native FFI handles.
- In-memory `/ws/ffi` endpoint tests now cover non-websocket 400 rejection, text-frame rejection (`binary messages required`), binary submit status-error text mapping, and binary success payload echo path.
- HTTP FFI endpoints (`/ffi/abi-version`, `/ffi/submit`) now resolve through `IFfiHttpBridge` so TestServer integration tests can validate response shape/status behavior without native engine allocation.
- In-memory HTTP endpoint tests now cover abi-version payload shape and submit success/error behavior (bad-request status envelope + binary body return path).
- Boundary/resilience coverage now includes empty `/ffi/submit` body forwarding, `/ws/ffi` invalid-frame-type recovery (text error then follow-on binary success), `/ws/ffi` empty-binary submit handling, malformed runtime JSON keep-open behavior, and duplicate `hello` rejection while preserving session continuity.
- Runtime endpoint-level resilience coverage now also asserts `/ws/runtime` binary-frame rejection recovery (error + continued command processing) and failed `close-session` status-error recovery (connection remains open and follow-on `noop` succeeds).
- Runtime websocket receive-loop hardening now enforces a bounded inbound message size (`64 KiB`) on `/ws/runtime`; oversized fragmented messages emit a typed `message too large` error frame and keep the connection alive for subsequent valid commands.
- `/ws/ffi` websocket receive-loop now also enforces a bounded inbound message size (`64 KiB`) for fragmented binary command payloads; oversized messages emit a typed `message too large` error frame while preserving connection continuity for subsequent valid binary commands.
- Runtime and FFI websocket loops now use guarded send/close behavior (state-aware writes + swallowed transport/disposing races) so shutdown and transient disconnect edges do not surface as unhandled host exceptions.
- Both websocket endpoints now propagate request-abort cancellation tokens into their loop runners, and loop-level cancellation (`OperationCanceledException`) is treated as a clean shutdown path rather than an unhandled failure.
- `/ws/ffi` loop orchestration is now extracted into `FfiWebSocketLoopRunner` (mirroring runtime-loop architecture) with focused unit coverage for invalid-type recovery, oversized fragmented payload handling, send failure, close failure, and cancellation-path clean shutdown.
- Runtime and FFI loop runners now also treat receive-side transport/dispose failures (`WebSocketException` / `ObjectDisposedException`) as clean disconnect paths, with deterministic unit coverage for receive-fault handling in both loop test suites.
- Loop test scaffolding now injects receive fault mode by exception type, and both runtime and FFI suites include explicit `ObjectDisposedException` receive-path tests to lock in clean-disconnect behavior for disposed transport edges.
- Endpoint-level TestServer coverage now includes abrupt client-abort scenarios for both `/ws/runtime` and `/ws/ffi`; after a client aborts while the loop is awaiting receive, a fresh websocket connection still succeeds and processes follow-on commands, guarding against late-fault regressions in real endpoint wiring.
- Endpoint-level TestServer coverage now also includes client half-close (`CloseOutputAsync`) scenarios for both `/ws/runtime` and `/ws/ffi`, asserting server-side normal close handshake completion on close-frame receive paths without invoking native bridge dependencies.
- Multi-pass close-path hardening coverage now additionally includes partial-fragment-then-half-close endpoint scenarios (runtime text and FFI binary) plus loop-level partial-fragment-then-close frame tests for both runners, asserting no outbound error/message leakage and clean normal close handshake behavior even when close frames carry non-normal peer status values.
- Additional bundled resilience passes now cover: (1) endpoint-level non-normal client half-close status handling (`PolicyViolation`) for both `/ws/runtime` and `/ws/ffi` with expected server normal-close completion, (2) loop-level direct close-frame handling with non-normal peer status for both runners, and (3) zero-length text-frame behavior at endpoint level (`/ws/runtime` invalid-JSON recovery path and `/ws/ffi` binary-required recovery path).
- Additional bundled resilience passes now also cover: (1) `/ws/runtime` zero-length binary-frame rejection/recovery (`text messages required` then successful follow-on `hello -> noop-ack`), (2) loop-level direct close-frame handling with non-normal status and null close reason for both runtime/FFI runners, and (3) fake transport close-frame fidelity updates that transition receive state to `CloseReceived` before close-handshake completion.
- Additional bundled resilience passes now also cover: (1) error-send race behavior on both runners for invalid-type and oversized-message paths (error-frame send failure exits cleanly without unhandled exceptions), (2) endpoint-level fragmented invalid-frame recovery (`/ws/runtime` fragmented binary -> text-required error -> recovery, `/ws/ffi` fragmented text -> binary-required error -> recovery), and (3) fake close-frame receive state progression fidelity retained across these branches.
- Additional bundled resilience passes now also cover: (1) loop-level outbound send-failure races after successfully assembled fragmented messages (`/ws/runtime` fragmented `noop` and `/ws/ffi` fragmented binary command) exiting cleanly without unhandled exceptions, and (2) endpoint-level fragmented valid-message happy paths for both adapters (`/ws/runtime` fragmented `hello` + fragmented `noop` -> `noop-ack`, `/ws/ffi` fragmented binary command -> binary success payload).
- Additional bundled resilience passes now also cover: (1) fragmented `close-session` success semantics for runtime at both loop and endpoint levels (assembled fragmented command still closes normally with expected close frame behavior), and (2) fragmented binary status-failure semantics for FFI at both loop and endpoint levels (assembled fragmented command returns status-error text and endpoint path remains recoverable for follow-on binary success).
- Additional bundled resilience passes now also cover: (1) fragmented `close-session` failure-path send-race hardening in runtime loop processing (bridge status failure + error-send failure exits cleanly), (2) fragmented binary status-failure send-race hardening in FFI loop processing (error-send failure exits cleanly), and (3) endpoint interleaving resilience where fragmented failure paths recover to follow-on valid frames (`/ws/runtime` fragmented close-session failure -> status error -> `noop-ack`; `/ws/ffi` fragmented text error -> fragmented binary success).
- Additional bundled resilience passes now also cover: (1) loop-level mid-message receive-fault interleavings for both runners (partial fragment received, then transport/dispose receive failure on subsequent receive call) with clean termination and no outbound leakage, and (2) endpoint-level partial-fragment-then-abort resilience for both adapters where a new websocket connection still succeeds (`/ws/runtime` follow-on `hello -> noop-ack`, `/ws/ffi` follow-on binary submit success).
- Additional bundled resilience passes now also cover: (1) loop-level outbound-then-peer-close race handling for both runners (successful outbound response followed by inbound close frame completes cleanly with normal close semantics), and (2) endpoint-level reconnect soak stability for both adapters across repeated short-lived websocket sessions (`/ws/runtime` repeated `hello -> noop-ack`, `/ws/ffi` repeated binary submit success) without lifecycle drift.
- Additional bundled resilience passes now also cover: (1) endpoint-level websocket status-matrix coverage for both adapters across all non-OK FFI statuses (`InvalidArg`, `NotFound`, `Auth`, `Policy`, `Protocol`, `Internal`) with explicit error-envelope assertions and follow-on recovery, and (2) a single end-to-end demo smoke test (`DemoReadinessSmokeTests`) that exercises runtime and FFI success/failure/recovery flows in one run.
- Additional bundled resilience passes now also cover parallel connection-isolation behavior under mixed outcomes for both adapters: runtime endpoint parallel dispatch isolation (`Runtime_endpoint_parallel_connections_mixed_dispatch_outcomes_are_isolated`) and FFI endpoint parallel dispatch isolation (`Ffi_endpoint_parallel_connections_mixed_outcomes_are_isolated`), asserting one connection can fail while another succeeds concurrently without cross-connection state bleed and with independent recovery on the failing socket.
- Additional bundled resilience passes now also cover parallel churn isolation under repeated mixed outcomes for both adapters: runtime endpoint alternating-failure churn isolation (`Runtime_endpoint_parallel_churn_alternating_failures_remain_isolated_and_recover`) and FFI endpoint alternating-failure churn isolation (`Ffi_endpoint_parallel_churn_alternating_failures_remain_isolated_and_recover`), asserting one connection can repeatedly fail/recover while another continues to succeed across rounds with no cross-connection bleed.
- Additional bundled resilience passes now also cover mixed fragmented-frame parallel churn under higher round counts for both adapters: runtime endpoint fragmented/text-mixed churn isolation (`Runtime_endpoint_parallel_fragmented_churn_mixed_frames_remain_isolated_and_recover`) and FFI endpoint fragmented-binary churn isolation (`Ffi_endpoint_parallel_fragmented_churn_mixed_frames_remain_isolated_and_recover`), asserting repeated concurrent fragmented command processing keeps failures isolated and each failing socket remains independently recoverable.
- Additional bundled resilience passes now also cover oversized fragmented-message parallel interleaving and send-race failure paths for both adapters: runtime and FFI endpoint isolation tests assert one socket can process oversized fragmented payload rejection while a concurrent socket continues successful fragmented command processing with independent recovery, and loop-runner tests assert oversized-message error-send failures exit cleanly. Runtime endpoint oversized interleaving coverage now uses a deterministic noop-ack test bridge to prevent intermittent long-running hangs while preserving isolation semantics.
- Additional bundled resilience passes now also cover parallel half-close interleaving isolation for both adapters: when one socket issues client half-close (`CloseOutputAsync`) concurrently with active command traffic on a second socket, the closing socket completes normal server close handshake while the active socket continues successful command processing and follow-on recovery without cross-connection lifecycle bleed.
- Additional bundled resilience passes now also cover parallel half-close interleaving with active error/recovery and oversized/recovery paths for both adapters: while one socket closes-output, a concurrent socket can independently traverse invalid-frame error/recovery or oversized-message error/recovery flows without cross-connection lifecycle bleed.
- Additional bundled resilience passes now also cover parallel half-close interleaving with fragmented active paths for both adapters: while one socket closes-output, a concurrent socket can independently process fragmented active success traffic or fragmented invalid-frame error/recovery traffic without cross-connection lifecycle bleed.
- Additional bundled resilience passes now also cover dual half-close parallel interleavings for both adapters: while two sockets close-output concurrently, a third socket can independently traverse invalid-frame error/recovery and continue successful follow-on command processing without cross-connection lifecycle bleed.
- Runtime mapper test coverage now includes negative-path guardrails (invalid JSON, missing type, non-array event payloads, unsupported message types) and full supported event-variant mapping assertions.

## Native Library Resolution

The host resolves the native library name activesync_host_ffi.

Before running the host, ensure the Rust FFI library is built:

- `cargo build -p activesync-host-ffi`

Runtime resolution order:

1. `ACTIVESYNC_HOST_FFI_DLL` environment variable (full path to compiled native library).
2. Common local build paths (for example `target/debug/activesync_host_ffi.dll` on Windows).
3. Platform default native loader search via library name.

If loading still fails, set `ACTIVESYNC_HOST_FFI_DLL` explicitly to remove path ambiguity.

## Local NuGet Packaging (Pre-Publish)

Use this flow to validate managed/native packaging locally before publishing.

1. Build and pack local packages:
- `cd dotnet-host`
- `pwsh -File .\pack-local-nuget.ps1 -Version 0.1.0-local`

This writes packages to `artifacts/nuget-local`.

2. Restore host in package-consumer mode:
- `dotnet restore .\ActiveSync.DotNetHost.slnx --configfile .\NuGet.Local.config -p:ActiveSyncUseNuGetPackages=true -p:ActiveSyncPackageVersion=0.1.0-local`

3. Run host against local packages:
- `dotnet run --project .\src\ActiveSync.DotNetHost\ActiveSync.DotNetHost.csproj --no-launch-profile -p:ActiveSyncUseNuGetPackages=true -p:ActiveSyncPackageVersion=0.1.0-local`

Notes:
- Default build uses project references; package mode is opt-in via `ActiveSyncUseNuGetPackages=true`.
- Package mode includes native runtime package references (`win-x64`, `linux-x64`) so runtime assets resolve via NuGet instead of local cargo outputs.

Dead-simple surface reference:

- `DEAD_SIMPLE_API.md`

NuGet package readme source used in package metadata:

- `NUGET_README.md`

Optional override:

- Set ACTIVESYNC_HOST_FFI_DLL to an absolute path for the native library file.

Examples:

- Windows: ACTIVESYNC_HOST_FFI_DLL=C:\path\to\activesync_host_ffi.dll
- Linux: ACTIVESYNC_HOST_FFI_DLL=/path/to/libactivesync_host_ffi.so
- macOS: ACTIVESYNC_HOST_FFI_DLL=/path/to/libactivesync_host_ffi.dylib

## Build and Test

- dotnet build dotnet-host/ActiveSync.DotNetHost.slnx
- dotnet test dotnet-host/ActiveSync.DotNetHost.slnx
- dotnet test dotnet-host/ActiveSync.DotNetHost.slnx --filter "FullyQualifiedName~DemoReadinessSmokeTests"

## SpeechSlate-Shape Local Smoke

Use this to validate hosted AS service readiness before wiring full SpeechSlate web-react + API runs.

1. From repo root, build host FFI:
- `cargo build -p activesync-host-ffi`
2. Run verifier from `dotnet-host` folder:
- `cd dotnet-host`
- `pwsh -File .\verify.ps1`

Expected success markers in output:

- `Server ready. Verifying delegated blob-url route...`
- `Delegated blob-url check passed:`
- `Received: {"type":"noop-ack"}`
- `Verification SUCCESS (delegated blob-url + runtime websocket).`

If `verify.ps1` is run outside `dotnet-host`, use:

- `Set-Location <repo>\dotnet-host; .\verify.ps1`

Package-mode smoke (local NuGet feed):

- `Set-Location <repo>\dotnet-host; .\verify.ps1 -UseNuGetPackages -ActiveSyncPackageVersion 0.1.0-local`

## Auth Profile Mode

The host auth provider is selected through `ActiveSync:Providers:Auth`:

- `Default`: pass-through validation semantics, `/sync/token` returns `501` (no mint support).
- `JwtBridgeEmbedded`: in-host JWT mint + validate (no extra process required).
- `JwtBridgeSidecar`: optional external auth bridge mode (opt-in only).

Embedded mode options:

- `ActiveSync:Auth:JwtBridgeEmbedded:Issuer`
- `ActiveSync:Auth:JwtBridgeEmbedded:Audience`
- `ActiveSync:Auth:JwtBridgeEmbedded:SigningKey` (minimum 32 chars for HS256)

Development defaults in `appsettings.Development.json` are configured for `JwtBridgeEmbedded` so the host can run standalone without requiring a sidecar.

## Notes

This slice focuses on host runtime ownership and P/Invoke lifecycle wiring.
Binary command/event schema decoding in .NET is intentionally deferred to follow-on PR7 slices.
