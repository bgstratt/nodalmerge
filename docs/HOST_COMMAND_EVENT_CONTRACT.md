# Host Command/Event Contract

Status: Draft freeze candidate (Track B / PR-07)

Purpose: define the host-owned runtime boundary for NodalMerge orchestration using typed commands and events, independent of transport/runtime choice.

## 1. Design constraints

1. Transport-agnostic: no WebSocket framing assumptions in contract payloads.
2. Runtime-agnostic: no Tokio task handles, async runtime types, or scheduler ownership in contract types.
3. Deterministic core ownership: CRDT merge/resolve semantics remain in `nodalmerge-core`.
4. Host ownership: socket lifecycle, auth issuance/validation policy, persistence wiring, timers, and process model are host responsibilities.
5. Compatibility-first migration: `nodalmerge-server` remains a host adapter and reference implementation.

## 2. Core model

The contract is modeled as:

- `HostCommand`: host -> engine intent
- `HostEvent`: engine -> host side effect or outbound message intent
- `CommandResult`: synchronous outcome for direct host handling (accepted/rejected + metadata)

This can be implemented as Rust enums first, then mapped to C ABI tagged structs.

## 3. Command catalog (host -> engine)

### 3.1 Session and room lifecycle

1. `EnsureRoom { room_id }`
2. `OpenSession { room_id, session_id, peer_pubkey, caps, token }`
3. `CloseSession { room_id, session_id, reason }`
4. `SetSubscription { room_id, session_id, patterns }`

### 3.2 Replication flow

1. `ClientHello { room_id, session_id, hello }`
2. `ClientPack { room_id, session_id, packed_nodes }`
3. `ClientRequest { room_id, session_id, known_frontier }`
4. `ClientMstRequest { room_id, session_id, request }`
5. `ClientMstDone { room_id, session_id }`

### 3.3 Blob flow

1. `ClientBlobUpload { room_id, session_id, blobs }`
2. `ClientBlobRequest { room_id, session_id, hashes }`
3. `ClientRequestUpload { room_id, session_id, hash, size, content_type }`
4. `ClientBlobUploaded { room_id, session_id, hash }`

### 3.4 Presence and admin flow

1. `ClientPresence { room_id, session_id, payload }`
2. `SetRoomKey { room_id, verifying_key_hex }`
3. `SetPolicy { room_id, policy }`
4. `StartTick { room_id, interval_ms, intent_prefix }`
5. `StopTick { room_id }`
6. `CompactRoom { room_id }`
7. `GetServerInfo { room_id }`

## 4. Event catalog (engine -> host)

### 4.1 Outbound transport intents

1. `SendToSession { room_id, session_id, envelope }`
2. `BroadcastRoom { room_id, envelope, exclude_session_id }`
3. `DisconnectSession { room_id, session_id, code, reason }`

### 4.2 Persistence/IO intents

1. `PersistNode { room_id, node }`
2. `PersistNodes { room_id, nodes }`
3. `PersistBlob { room_id, hash, bytes }`
4. `ResolveBlobGetUrl { room_id, hash, size_hint }`
5. `ResolveBlobPutUrl { room_id, hash, size, content_type }`
6. `VerifyUploadedBlob { room_id, hash }`

### 4.3 Runtime/ops intents

1. `EmitMetric { name, tags, value }`
2. `ScheduleTick { room_id, interval_ms }`
3. `CancelTick { room_id }`
4. `LogEvent { level, message, fields }`

## 5. Error taxonomy

`CommandResult::Rejected` should include category so hosts map behavior consistently.

1. `Auth`: token invalid/expired, room locked, key mismatch
2. `Protocol`: malformed or out-of-order message
3. `RateLimited`: peer throttled by node/byte limits
4. `Validation`: invalid payload, unsupported capability, bad policy
5. `Conflict`: session/room state conflict (already running/already closed)
6. `Internal`: unexpected runtime/storage failures

Retry guidance:

1. `Auth`: no retry until token/key changes
2. `Protocol` and `Validation`: no retry without payload fix
3. `RateLimited`: retry with backoff
4. `Conflict`: host may retry after state refresh
5. `Internal`: retry with backoff and alert if sustained

## 6. Wire mapping (current server parity)

Host command/event names intentionally map to current websocket message types in `server/src/ws_handler.rs`:

1. hello <-> `ClientHello` / welcome
2. pack <-> `ClientPack` / pack
3. request <-> `ClientRequest` / pack
4. subscribe <-> `SetSubscription` / subscribe-ack
5. blob-request <-> `ClientBlobRequest` / blob-pack or blob-redirect
6. request-upload <-> `ClientRequestUpload` / upload-granted or upload-denied
7. blob-uploaded <-> `ClientBlobUploaded` / blob-available or upload-rejected
8. presence <-> `ClientPresence` / presence
9. set-room-key <-> `SetRoomKey` / room-locked
10. set-policy <-> `SetPolicy` / policy-set
11. server-info <-> `GetServerInfo` / server-info
12. start-tick <-> `StartTick` / tick-started or tick-already-running
13. stop-tick <-> `StopTick` / tick-stopped
14. compact-room <-> `CompactRoom` / snapshot-pack + compact-ack

## 7. Contract invariants

1. Engine does not open sockets or spawn host-owned network tasks.
2. Engine may emit intents; host performs side effects and returns outcomes via subsequent commands.
3. All payloads crossing host boundary are typed/binary capable; JSON is adapter-level only.
4. Session IDs are host-generated opaque handles.
5. Room IDs and peer IDs are value data, not transport handles.

## 8. Versioning and compatibility

1. Add `contract_version` to hello/server-info payloads once host-core is introduced.
2. New command/event variants must be additive when possible.
3. Breaking variant shape changes require major version bump and migration notes.

## 9. Acceptance criteria for freeze

1. All current server admin and sync operations map to command/event entries.
2. No command/event type references runtime-specific concrete types (Tokio/Axum/WebSocket).
3. At least one adapter (server) compiles against the contract surface via compatibility shims.
4. C ABI design in Track C can encode this surface without JSON as the primary interface.
