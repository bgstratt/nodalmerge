# ActiveSync Operations Inventory

Purpose: single-source inventory of currently exposed operations across SDK, wire protocol, server runtime, core engine, auth bridge, and GC contracts, with a gap-analysis view.

Status legend:

- Implemented: present in runtime code today
- Documented contract: defined in docs, not yet implemented as shared runtime surface
- Planned: tracked in plans, not yet finalized

## 0) Overall Architecture Assessment

This project is in stabilization-and-extraction stage, not early architecture stage.

| Area | Status |
|---|---|
| Core replication engine | Excellent |
| Sync primitives | Excellent |
| Wire protocol | Mature |
| Persistence abstraction | Good |
| Storage abstraction | Good |
| Runtime/server isolation | Pretty good |
| WASM embedding proof | Excellent |
| GC architecture | Partially mature |
| Host-neutral orchestration | Missing |
| FFI surface | Missing |

Primary remaining risk is lifecycle ownership semantics (especially blobs/GC), not missing CRDT functionality.

## 1) Public SDK operations (web/sdk.js, web/sdk.d.ts)

Source: web/sdk.d.ts, docs/sdk.md

### 1.1 Document lifecycle

- createDoc(options) -> Promise<Doc) (Implemented)
- ready() -> Promise (Implemented)
- attachServer(doc, serverUrl, options) (Implemented)

Doc instance:

- connect(), disconnect(), close() (Implemented)
- onConnect(cb), onDisconnect(cb), onError(cb), onChange(cb) (Implemented)
- peers() (Implemented)
- send(msg) raw escape hatch (Implemented)

### 1.2 Data model handles

Map:

- map(namespace) -> MapHandle (Implemented)
- set(key, value), get(key), delete(key), all(), onChange(cb) (Implemented)
- setBlob(key, bytes, options), getBlob(hashOrKey) (Implemented)

Text:

- text(key) -> TextHandle (Implemented)
- insert(pos, str), delete(pos, len), toString(), onChange(cb) (Implemented)

List:

- list(key) -> ListHandle (Implemented)
- ids(), get(id), toArray(), length (Implemented)
- push, insert, insertAfter, insertBefore, move, delete, update (Implemented)
- onChange, onReorder (Implemented)
- gestures: dropOnto, dropBetween, swap, dropBefore, dropAfter (Implemented)

Presence:

- presence.set(patch), clear(), me(), others(), get(pubkey) (Implemented)
- onJoin, onUpdate, onLeave (Implemented)

### 1.3 Conflict and undo

- onConflict(cb), recentConflicts(sinceMs) (Implemented)
- undoManager({scope, captureTimeout, maxItems}) -> undo/redo API (Implemented)

### 1.4 Subscription and filtering

- subscription patterns (read-only view)
- subscribe(patterns), isSubscribed(path), onSubscriptionChange(cb) (Implemented)

### 1.5 Auth/token options

- roomSeed + tokenCaps + tokenExpirySecs (Implemented)
- tokenProvider(ctx) async mint hook (Implemented)

### 1.6 Transport options

- transport: auto | ws-only (Implemented)
- iceServers override (Implemented)

## 2) Wire protocol operations (server websocket)

Source: server/src/ws_handler.rs

### 2.1 Client -> server message types

Core sync:

- hello
- pack
- request
- subscribe
- mst-request
- mst-done

Blob/data channel:

- blob-upload
- blob-request
- request-upload
- blob-uploaded

Presence:

- presence

Room/admin/authority:

- set-room-key
- set-policy
- server-info
- start-tick
- stop-tick
- compact-room

WebRTC signaling relay:

- webrtc-offer
- webrtc-answer
- webrtc-ice

### 2.2 Server -> client message types

Core sync:

- welcome
- pack
- error
- subscribe-ack
- mst-response

Peer/session:

- peer-joined
- peer-left

Blob/data channel:

- blob-available
- blob-pack
- blob-redirect
- upload-granted
- upload-denied
- upload-rejected

Presence:

- presence

Room/admin/authority:

- room-locked
- policy-set
- server-info
- tick-started
- tick-already-running
- tick-stopped
- snapshot-pack
- compact-ack

## 3) Server runtime operations (CLI and background workers)

Source: server/src/main.rs, docs/operator.md, docs/quickstart.md

### 3.1 CLI flags

- --store <path>
- --metrics-addr <ip:port>
- --idle-timeout <secs>
- --broadcast-capacity <N>
- --peer-rate-nodes <N>
- --peer-rate-bytes <MiB>
- --blob-gc-interval <secs>
- --blob-gc-grace <secs>
- --snapshot-interval <N>
- --snapshot-max-chain <K>

### 3.2 Background workers

- idle room sweeper (Implemented)
- blob GC sweeper (Implemented)
- snapshot sweeper (Implemented)
- authoritative tick loop per room (Implemented)

## 4) Persistence and blob lifecycle methods (runtime traits)

Source: server/src/store.rs

### 4.1 NodePersistence

- load_room_nodes(room_id)
- persist_node(room_id, node)
- persist_nodes(room_id, nodes)
- nodes_durable()

### 4.2 BlobPersistence

- load_room_blobs(room_id)
- persist_blob(room_id, hash, bytes)
- blob_gc_sweep(room_id, live, grace)
- resolve_get_url(room_id, hash, size_hint)
- resolve_put_url(room_id, hash, size, content_type)
- verify_uploaded(room_id, hash)
- blobs_durable()

### 4.3 ServerPersistence

- is_durable() combined durability helper

### 4.4 Implementations present

- NoPersistence (in-memory)
- DirPersistence (SQLite + file blobs + tombstone-based GC)
- Composite<Node, Blob>
- S3BlobStore crate implementing BlobPersistence with direct/delegate modes

## 5) Core engine exports (activesync-core)

Source: core/src/lib.rs

### 5.1 Data and sync primitives

- StateGraph, SyncNode, NodeId, Transaction
- Op, MapOp, TextOp, ListOp
- pack_nodes, unpack_nodes
- Frontier
- SyncCapabilities
- Ibf, MerkleSearchTree

### 5.2 Determinism/replay/compaction

- replay, canonical_hash
- compact, compact_incremental
- verify_snapshot
- rebuild_from_snapshot
- pack_snapshot_pack, unpack_snapshot_pack

### 5.3 Security/crypto/auth

- RoomToken, TokenError
- derive_room_key, encrypt_ops, decrypt_ops, wrap_encrypted_ops
- E2EE sentinel helpers

### 5.4 Storage primitives

- NodeStore, BlobStore
- MemoryNodeStore, MemoryBlobStore

## 6) JWT bridge operations (activesync-jwt-bridge)

Source: jwt-bridge/src/lib.rs

- JwtVerifier::hs256(secret)
- JwtVerifier::rs256_from_pem(pem)
- JwtVerifier::es256_from_pem(pem)
- mint_room_token(cfg, jwt) -> RoomToken

Bridge config surfaces:

- verifier
- room_key
- allowed_issuers
- allowed_audiences

## 7) GC operations inventory

### 7.1 Implemented runtime GC operations

- BlobPersistence::blob_gc_sweep(room_id, live, grace)
- Rooms::sweep_blobs(grace)
- spawn_blob_gc_sweeper(interval, grace)
- server CLI: --blob-gc-interval, --blob-gc-grace

Current runtime shape:

1. Server computes live hash set from room graph state.
2. Server calls storage adapter sweep (`blob_gc_sweep(room_id, live, grace)`).

This is effective for standalone topology, but may over-couple GC semantics to room/runtime ownership in embedded and multi-host futures.

### 7.2 Documented GC contracts (not yet shared runtime package)

Source: docs/delegated-storage-gc.md

- LiveHashSource::collect_live_hashes
- AssetInventoryStore::*
- GcRunStore::{start_run, apply_delta, finish_run}
- AdminPinStore::is_pinned
- BlobObjectStore::{head, delete}
- ReferenceDeltaSink::apply_delta (optional fast path)
- HTTP fallback: POST /internal/gc/live-hashes

These contracts point toward a system-reference-centric GC model, but are not yet consolidated as a runtime subsystem crate.

### 7.3 GC ownership and semantics gaps

Frozen decisions (A1):

1. Authoritative liveness = reachable in authoritative state within GC domain.
2. Mark pass is source of truth; delta path is optimization-only.
3. Deletion scope is domain-scoped (tenant/bucket/prefix), not room-scoped.
4. Shared assets are protected by any live reference in the same domain.
5. Pin/lease protection overrides deletion eligibility.

Remaining work items:

1. encode these semantics in `activesync-gc` contracts/coordinator code
2. validate via conformance tests and adapter boundary review

### 7.4 Recommended blob lifecycle states (contract level)

Recommended explicit state machine for documentation and conformance tests:

1. Uploading: bytes staged, integrity/ownership not finalized.
2. Live: actively referenced by authoritative reachability model.
3. Grace: recently dereferenced, within retention window.
4. Pinned: protected from deletion by policy/admin/system pin.
5. SweepCandidate: eligible after grace + safety checks.
6. Deleted: physically removed from backing store.

Note: state naming can vary by implementation; semantics should not.

### 7.5 Recommended method split (long-term)

Current `blob_gc_sweep(...)` is intentionally pragmatic but combines policy, orchestration, and execution.

Long-term split should be:

1. GC subsystem decides eligibility (`eligible_for_delete(hash, domain, now)`).
2. Storage adapter executes object operations (`head/delete`).
3. Run ledger/inventory track transitions and auditability.

This reduces host/runtime coupling and improves embedding consistency.

## 8) Gap analysis matrix (high-level)

### 8.1 Exposed and usable today

- Full SDK document API (Map/Text/List/Presence/Undo/Conflicts/Subs)
- Full websocket protocol with advanced admin/tick/compaction operations
- Runtime persistence traits and GC hooks
- Direct blob IO redirect/grant/verify flow
- JWT -> RoomToken bridge

### 8.2 Defined but not consolidated as one runtime surface

- Product-neutral GC coordinator package and trait implementations
- Inventory/run ledger/admin pins as first-class runtime modules
- Uniform command/event API for host-owned runtime extraction
- Thin C ABI host runtime surface (planned in hostedMigrationPlan.md)
- Canonical reference ownership model for blob liveness and delete domains
- Explicit blob lifecycle state machine and transition invariants
- GC policy/orchestration split from storage execution semantics

### 8.3 Must-stabilize before host extraction

1. Blob lifecycle state model and transition invariants encoded in executable contracts.
2. Reference ownership model for shared/admin/template assets validated by tests.
3. Host command/event API formalization for orchestration extraction.

### 8.4 Strongly recommended next

1. Split GC policy/orchestration from storage execution APIs.
2. Define room-scoped vs global/domain-scoped asset model explicitly.
3. Add typed protocol command layer for host-core and FFI usage.

### 8.5 Can follow after stabilization

1. Full C ABI breadth expansion.
2. Full .NET embedding implementation.
3. Non-websocket transport adapters.

### 8.6 Documentation gaps to close next

1. Add a dedicated operation-reference section in docs/operator.md for websocket admin operations (set-policy, compact-room, tick controls).
2. Add a protocol message catalog doc with request/response examples (could be docs/protocol.md).
3. Add a "method status" appendix mapping implemented vs proposed contracts for GC and host-core APIs.
4. Add quick links to this inventory from quickstart.md and operator.md.
5. Add a GC semantics appendix: liveness definition, delete domain, lifecycle states, and conformance invariants.

## 9) Proposed subsystem boundary for GC

Suggested target shape:

- `activesync-gc` (new shared subsystem crate)

Responsibilities:

1. liveness computation contract integration
2. inventory reconciliation
3. pin/lease policy hooks
4. retention and delete eligibility decisions
5. sweep coordination and run ledger updates
6. delta reconciliation with mark authority

Non-responsibilities:

1. S3/filesystem/vendor-specific execution details
2. websocket/runtime/session concerns
3. CRDT merge semantics

## 9.1 Immediate implementation start (PR-01 to PR-03)

1. PR-01 (current): semantic freeze and gate status update in docs.
2. PR-02: scaffold `activesync-gc` crate with frozen contract types/traits.
3. PR-03: implement coordinator with `DryRun` and `MarkOnly` first.

## 10) Suggested documentation structure

Use quickstart for onboarding only, and keep complete operation detail in dedicated references:

- quickstart.md: basic run/connect flow + critical flags only
- sdk.md: client API methods
- operator.md: server CLI + operational/admin commands
- delegated-storage-gc.md + GC_IMPLEMENTATION_PLAN.md: GC contracts and rollout
- operations-inventory.md (this file): cross-surface index + gap analysis anchor

Execution reference:

- PRE_HOST_EXTRACTION_IMPLEMENTATION_PLAN.md: concrete phased implementation plan and extraction kickoff gates
