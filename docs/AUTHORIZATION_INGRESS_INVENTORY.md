# Authorization Ingress Inventory

Status: Draft (P0 deliverable)
Owner: Core + host runtime streams
Last Updated: 2026-05-21
Companion Tracker: [AUTHORIZATION_CORE_HOST_EXECUTION_TRACKER.md](AUTHORIZATION_CORE_HOST_EXECUTION_TRACKER.md)

## 1. Purpose

Enumerate all mutation ingress points and confirm authorization enforcement path before canonical graph merge.

## 2. Required Invariant

For every ingress point that can mutate graph state:

1. Integrity checks run.
2. Signature checks run.
3. Policy/capability checks run.
4. Unauthorized ops are rejected before merge.

## 3. Ingress Surfaces

## 3.1 Rust host WebSocket command ingress

Primary location:
1. [server/src/ws_handler.rs](server/src/ws_handler.rs)

Mutation-bearing client commands:
1. pack
2. request-upload and blob-uploaded pathways (metadata/control flow)
3. set-policy
4. set-room-key
5. start-tick / stop-tick

Current merge path:
1. ws_handler pack command routes to room import helper.
2. room helper delegates to core apply_remote_batch path.
3. core apply path enforces policy can_write.

Notes:
1. Control-plane commands require explicit separate authorization gating (P1).

## 3.2 Rust room helper / graph import ingress

Primary location:
1. [server/src/room.rs](server/src/room.rs)

Mutation-bearing functions:
1. import_nodes
2. process_tick (authoritative writes)
3. set_policy

Current merge path:
1. import_nodes -> core apply_remote_batch_checked.
2. process_tick -> core apply_local (server-signed nodes).

## 3.3 Core graph ingress (canonical enforcement point)

Primary location:
1. [core/src/graph.rs](core/src/graph.rs)

Mutation-bearing entry points:
1. apply_remote
2. apply_remote_checked
3. apply_remote_batch
4. apply_remote_batch_checked

Current checks before merge:
1. hash integrity check.
2. lamport and optional wall-skew checks.
3. signature verify (with verified-id cache optimization).
4. parent existence checks.
5. policy can_write checks per op key.

## 3.4 DotNetHost runtime ingress

Primary locations:
1. [nodalmerge-host/src/ActiveSync.DotNetHost/Runtime](nodalmerge-host/src/ActiveSync.DotNetHost/Runtime)
2. [nodalmerge-host/src/ActiveSync.DotNetHost/HostApplication.cs](nodalmerge-host/src/ActiveSync.DotNetHost/HostApplication.cs)

Mutation-bearing surfaces:
1. /ws/runtime message dispatch to host command bridge.
2. /ws/ffi binary bridge path to native host commands.

Required parity rule:
1. Any mutation that reaches graph state must ultimately traverse host-core/native enforcement path equivalent to Rust host policy checks.
2. Control-plane capability mapping must follow [AUTHORIZATION_CONTROL_PLANE_CAPABILITIES.md](AUTHORIZATION_CONTROL_PLANE_CAPABILITIES.md).

## 3.5 FFI/host-core command ingress

Primary locations:
1. [host-core](host-core)
2. [host-ffi](host-ffi)

Mutation-bearing surfaces:
1. command submissions that map to graph operations.

Required parity rule:
1. No side path may bypass policy evaluation semantics required by core.

## 4. Ingress Checklist

1. [x] Rust ws_handler control-plane authorization gate exists for set-policy, set-room-key, start-tick, stop-tick.
2. [x] Core apply_remote/apply_remote_batch enforce policy can_write.
3. [x] DotNetHost runtime command bridge explicitly rejects unauthorized control-plane commands with deterministic reasons.
4. [x] FFI command paths are verified to share equivalent denial semantics.
5. [x] Focused Rust/DotNetHost control-plane ingress tests now assert deterministic deny semantics for allowed vs denied paths.
6. [x] DotNetHost tick control-plane parity validation (`start-tick` / `stop-tick`) is covered with focused mapper, processor, and websocket endpoint tests.

## 5. Evidence Pointers

1. [core/src/policy.rs](core/src/policy.rs)
2. [core/src/graph.rs](core/src/graph.rs)
3. [server/src/ws_handler.rs](server/src/ws_handler.rs)
4. [server/src/room.rs](server/src/room.rs)
5. [nodalmerge-host/src/ActiveSync.Host.Composition/ServiceCollectionExtensions.cs](nodalmerge-host/src/ActiveSync.Host.Composition/ServiceCollectionExtensions.cs)
6. [nodalmerge-host/src/ActiveSync.DotNetHost/Ffi/FfiWebSocketLoopRunner.cs](nodalmerge-host/src/ActiveSync.DotNetHost/Ffi/FfiWebSocketLoopRunner.cs)
7. [nodalmerge-host/tests/ActiveSync.DotNetHost.Tests/FfiWebSocketLoopRunnerTests.cs](nodalmerge-host/tests/ActiveSync.DotNetHost.Tests/FfiWebSocketLoopRunnerTests.cs)

## 6.1 P1 Test-Case Mapping

1. Checklist #3 ->
	- `RuntimeProtocolTests.Set_room_key_rejects_without_room_admin_capability`
	- `RuntimeProtocolTests.Set_policy_rejects_without_policy_admin_capability`
	- `RuntimeMessageProcessorTests.Set_policy_without_capability_returns_control_plane_forbidden_error_envelope`
	- `RuntimeWebSocketEndpointTests.Runtime_endpoint_set_policy_without_policy_admin_capability_is_denied_before_bridge_dispatch`
	- `RuntimeWebSocketEndpointTests.Runtime_endpoint_set_policy_with_policy_admin_capability_is_allowed`
2. Checklist #4 ->
	- `FfiWebSocketLoopRunnerTests.Policy_status_failure_emits_control_plane_deny_metric_with_fixed_labels`

## 7. Next Actions

1. Complete control-plane ingress gating in Rust host (P1).
2. Confirm whether FFI can surface command/capability labels beyond `ffi`/`unknown` without protocol changes.
3. Expand host-core deny reason surfacing so additional FFI denied paths emit concrete metadata labels.
