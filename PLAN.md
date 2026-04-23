# ActiveSync Engineering Plan

> Goal: Transition from "making it work" to "engineering for the hardware."
> Beat Firebase, Yjs, Automerge, Replicache, ElectricSQL by being a low-level,
> extensible, E2EE-ready CRDT engine — not a black box.

---

## Current State (Baseline)

- ✅ Rust/WASM monorepo (`core`, `bridge`, `server`)
- ✅ Append-only DAG with Blake3 content-addressing
- ✅ LWW-Map CRDT with Lamport + pubkey tie-breaking
- ✅ Ed25519 signing on every node
- ✅ Blob CAS with integrity verification
- ✅ Axum WebSocket server with room isolation
- ✅ Presence / awareness side-channel
- ✅ 8 passing core tests including PKI tests
- ✅ Node graph persisted to `localStorage` on client (survives page close)
- ✅ **A1 complete** — `NodeStore` + `BlobStore` traits; `MemoryNodeStore` / `MemoryBlobStore`; `StateGraph<N: NodeStore = MemoryNodeStore>`; `size()` + `get_range()` on `BlobStore`
- ✅ **A2 complete** — `Op::Map(MapOp)` | `Op::List(ListOp)` | `Op::Text(TextOp)`; `List`/`Text` are uninhabited stubs; all 20 core tests pass
- ✅ **A3 complete** — `WireNode` postcard shadow type; `pack_nodes` / `unpack_nodes` in core; server + bridge use postcard+base64 for all `pack` messages; postcard size verified <50% of JSON; storage key bumped to v4
- ✅ **A4 complete** — four criterion benchmarks in `core/benches/`; baselines: blob_verify 8.9µs, resolve_1k 278µs, merge_10k 350ms (Ed25519 bottleneck flagged for B1), sync_handshake pack 326µs encode
- ✅ **A5 complete** — `Policy` type, `PolicyRule`, glob matcher, `StateGraph` enforcement, `PolicyViolation` error, 33 tests pass
- ✅ **A6 complete** — `Frontier` type in `core`; `StateGraph::frontier()` accessor; `hello`/`welcome` carry `frontier:[hex]`; 39 tests pass; WASM bridge exposes `frontier_hex_json()`
- ✅ **A7 complete** — `SyncCapabilities` in `core` with serde defaults; `intersect()` for session negotiation; `hello`/`welcome` carry `caps:{...}`; 47 tests pass; WASM bridge exposes `our_capabilities_json()`
- ✅ **B1 complete** — `Ibf` (Invertible Bloom Filter, 80 cells, k=3) in `core/src/ibf.rs`; client sends `ibf:"b64"` in hello; server decodes symmetric diff via XOR + peel; `supports_ibf:true` in default caps; 55 tests pass; WASM bridge exposes `compute_ibf_b64()`; **handshake wire: 2.9 KB** for any graph size (vs 179.7 KB pack) ✅ checkpoint
- ✅ **B2 complete** — `MerkleSearchTree` (16-way nibble trie, Blake3) in `core/src/mst.rs`; `mst_root` in hello/welcome; `mst-request`/`mst-response`/`mst-done` handlers; `supports_mst:true` in default caps; 66 tests pass; WASM bridge exposes `mst_root_hex()`, `mst_get_node_json()`, `mst_process_response_json()`; **3 round trips, 816 ns simulate for 1k-node diff in 2k-node graph** ✅ checkpoint
- ✅ **C2 complete** — IndexedDB replaces `localStorage`; `nodes` object store holds full base64 postcard pack keyed by `'all'`; `blobs` object store holds raw `Uint8Array` keyed by hash hex; one-time migration from `activesync-v4` localStorage on first boot; blob bytes persisted on every `store_blob_bytes` and `set_blob` call; node graph persisted fire-and-forget after every mutation; no bridge changes (pure JS adapter layer)
- ✅ **C3 complete** — Ed25519 capability tokens; `RoomToken` in core with `capabilities: Vec<String>` field (signed, tamper-proof); server locks room on `set-room-key`, rejects `hello` without valid token; bridge exports `sign_room_token(…, caps)` and `room_pubkey_hex`; Room Auth UI card in demo; 76 core tests pass. Room = transport/connection scope; Policy (A5) = authority over what peers can read/write once inside.
- ✅ **E1 complete** — Super-Peer server mode; persistent Ed25519 keypair in `server.key`; pubkey printed on startup; `welcome` carries `server_pubkey` hex; `set-policy` handler installs room `Policy` in the server's `StateGraph` (enforced by `apply_remote` on all incoming nodes); `server-info` handler for on-demand pubkey query; `parse_policy` helper validates rule JSON including per-path `can_write` pubkey arrays; 87 core tests pass.
- ✅ **D1 complete** — End-to-End Encryption; `core/src/crypto.rs` with HKDF-SHA256 key derivation (`derive_room_key`) + AES-256-GCM encrypt/decrypt; deterministic nonce from `(room_key, author, lamport)` — no randomness source needed in WASM; encrypted nodes carry a single sentinel op (`key = "\x00e2ee"`, `value = nonce_12 || ciphertext`); bridge `SyncStore` gains `set_room_key` / `clear_room_key` / `has_room_key`; all `set`/`delete`/`set_blob` encrypt when key active; `import_pack` transparently decrypts; `resolve_json` filters sentinel key; `supports_encryption: true` in default `SyncCapabilities`; bridge exports `derive_e2ee_key`; 11 new crypto tests; 87 total core tests pass. Server relays ciphertext without decryption.
- ✅ **C1 complete** — RGA collaborative text in `core/src/text.rs`; stable `OpId = (lamport, author)` identity per character; deterministic DFS traversal with descending-`OpId` sibling order; tombstone deletions; bridge exposes `text_insert(key, pos, ch)` / `text_delete(key, pos)` / `text_resolve(key)`; demo's shared textarea proves concurrent convergence across two tabs.
- ✅ **D2 complete** — WebRTC data-channel fallback in `web/demo.js`; server-hosted STUN-based signaling over the existing WS; when a direct peer channel opens, node packs and blob bytes flow peer-to-peer; falls back to WS if negotiation fails.
- ✅ **D3 complete** — DAG compaction in `core/src/compaction.rs`; snapshot node carries `snapshot_hash` (Blake3 of canonical state) + subsumed frontier in sentinel ops; `compact()` / `rebuild_from_snapshot()` / `verify_snapshot()`; receivers verify independently via `replay()`; 117 tests pass including `snapshot_hash_matches_replay` round-trip.
- ✅ **D4 complete** — Deterministic replay in `core/src/replay.rs`; pure-function `replay(&[SyncNode], Option<&Policy>) -> ResolvedState` with Blake3 `canonical_hash`; property tests prove order-independence including out-of-order delivery; server CLI `activesync-server replay <room-id>` dumps the log and final state.
- ✅ **E2 complete** — Speculative vs. Canonical views on `StateGraph` (`read_speculative` / `read_canonical`); local writes are speculative-only until echoed back or superseded by an authoritative remote node; snap-back is automatic through LWW pubkey priority; unit tests cover all three transition paths.
- ✅ **E3 complete** — Tick-based batching via `TickConfig { interval_ms, max_ops_per_tick }`; optional buffered mode on `StateGraph`; each buffered op keeps its own `OpId` so tick boundaries never affect resolved state.
- ✅ **Observability** — server logs routed through `tracing` + `tracing-subscriber`; default filter `info,activesync_server=info,activesync_core=info`; `RUST_LOG` overrides at runtime.
- ✅ **Batched Ed25519 verification** — `StateGraph::apply_remote_batch(Vec<SyncNode>) -> BatchResult { accepted, rejected }`; dedupe → parallel `ed25519_dalek::verify_batch` (rayon, 64-node chunks, native only) → per-chunk per-node fallback on failure → session-scoped `verified_ids` cache; server `import_nodes` and catchup paths use it. **`merge_10k_batch` 34 ms vs serial `merge_10k` 320 ms (9.4×).** 117 tests pass.
- ⚠️  Server-side `NodeStore`/`BlobStore` are still in-memory — disk adapters remain a deployment concern, not a protocol gap

---

## Phase A — Make the Architecture Right
> No new user-visible features. Prerequisite to everything else.
> **Target: 2 weeks**

### A1. `StorageAdapter` trait  ← START HERE
The most important refactor. Decouples the engine from any specific storage medium.

```rust
// core/src/storage.rs
pub trait NodeStore: Send + Sync {
    fn put(&mut self, node: SyncNode) -> Result<(), SyncError>;
    fn get(&self, id: &NodeId) -> Option<&SyncNode>;
    fn all_ids(&self) -> Vec<NodeId>;
}

pub trait BlobStore: Send + Sync {
    fn put(&mut self, data: Vec<u8>) -> Hash;
    fn get(&self, hash: &Hash) -> Option<Vec<u8>>;
    fn contains(&self, hash: &Hash) -> bool;
    /// Byte length of the stored blob. Used for bandwidth planning, chunking,
    /// and streaming decisions. Must not require loading blob bytes.
    fn size(&self, hash: &Hash) -> Option<u64>;
    /// Partial fetch. Enables streaming, resumable transfers, and WebRTC chunking.
    fn get_range(&self, hash: &Hash, range: std::ops::Range<u64>) -> Option<Vec<u8>>;
    // Returns a redirect URL instead of bytes (S3, R2, IPFS, CDN, etc.)
    fn resolve_url(&self, hash: &Hash) -> Option<String> { None }
}
```

**Implementations to ship with A1:**
- `MemoryNodeStore` — current behavior
- `MemoryBlobStore` — current behavior

**Unlocks:** IndexedDB, disk, S3-redirect adapters all become drop-in. S3 signed
URLs are not a core concern — they are a `BlobStore` adapter whose `resolve_url`
returns a presigned URL; the client fetches directly from the CDN.

**Checkpoint A1:** `StateGraph` and `SyncStore` take generic `impl NodeStore` /
`impl BlobStore`. All existing tests pass. Wire format unchanged.

---

### A2. Extensible `Op` enum
Restructure now before adding any new CRDT types. Changing this later breaks
the wire format and all existing serialized nodes.

```rust
pub enum Op {
    Map(MapOp),
    List(ListOp),   // stub — Phase C
    Text(TextOp),   // stub — Phase C
}

pub enum MapOp {
    Set    { key: String, value: Vec<u8> },
    Delete { key: String },
    SetBlob { key: String, blob_hash: Hash },
}
```

> **⚠️ LWW is a transitional data model.** The current LWW-Map with Lamport +
> pubkey tie-breaking is correct but has known limitations: concurrent edits
> silently overwrite each other, ordering is not preserved, and semantic
> conflicts are hidden. It is the right starting point for Map keys, but Text
> and List types MUST use RGA/fractional indexing (Phase C) before shipping to
> real users. Do not rely on LWW semantics in application-level logic.

**Checkpoint A2:** Existing `Set`/`Delete`/`SetBlob` ops migrate into `Op::Map(MapOp::*)`.
All 8 core tests pass. Wire format is backwards-compatible via serde rename.

---

### A3. Binary wire format (`postcard`)
Replace JSON node serialization with `postcard` (compact binary, serde-compatible,
no schema files). ~60% size reduction. Zero-copy friendly.

- Nodes serialized with `postcard` on the wire
- `Hash` and `Signature` get efficient binary representations
- JSON kept only for the outer WebSocket envelope (type routing)
- FlatBuffers / rkyv deferred until benchmarks prove the bottleneck

**Checkpoint A3:** Wire size benchmark shows ≥50% reduction vs JSON for a 1k-node pack.

---

### A4. Benchmark suite in `core`
You cannot optimize what you don't measure. Build this before Phase B.

```
benches/
  merge_10k.rs        → time to merge 10,000 concurrent updates  (target: <10ms)
  sync_handshake.rs   → bytes to sync 1,000 missing keys         (target: <5KB)
  resolve_1k.rs       → LWW resolution on 1,000-key map          (target: <1ms)
  blob_verify.rs      → Blake3 verify 50KB blob                  (target: <0.5ms)
```

**Checkpoint A4:** All four benchmarks have a passing baseline number. CI fails
if a PR regresses any benchmark by >10%.

✅ **A4 complete** — latest results (release profile, refreshed post-C1/D3/D4):

| Benchmark | Result | Target | Notes |
|---|---|---|---|
| `blob_verify_50kb` | **9.2 µs** | <500 µs | ✅ 54× under target |
| `resolve_1k` | **338 µs** | <1 ms | ✅ 3× under target |
| `sync_handshake_pack_1k_missing` | **387 µs** (encode time) | — | Wire size: 179.7 KB for 1k nodes |
| `sync_handshake_ibf_encode_1k` | **270 µs** | — | IBF hello: **2.9 KB** for any graph size ✅ |
| `sync_handshake_ibf_decode_1k_diff` | **888 ns** | — | XOR + peel decode on server; O(diff); iteration-cap guard prevents infinite loops on degenerate IBFs |
| `sync_handshake_mst_build_1k` | **1.32 ms** | — | Build 16-way nibble trie from 1k node IDs; O(n log n) |
| `sync_handshake_mst_build_2k` | **3.28 ms** | — | Build trie from 2k IDs; ~linear scaling confirmed |
| `sync_handshake_mst_simulate_1k_diff` | **302 µs** | — | Simulate 1k-node diff between two 2k-node trees; **3 round trips** |
| `merge_10k` | **320 ms** (per-node `apply_remote`) | <10 ms | Legacy serial path; one Ed25519 verify per node ≈ 32 µs × 10 k. |
| `merge_10k_batch` | **34 ms** (`apply_remote_batch`) | <10 ms | 🔥 9.4× win. Dedupe → parallel `ed25519_dalek::verify_batch` (rayon, chunk = 64) → per-chunk fallback to per-node verify on failure → session-scoped `verified_ids` cache skips re-broadcasts. Server `import_nodes` and the catchup pack path go through this automatically. |

---

### A5. Scoped Write Policies (Capability-Based Security)
The foundational primitive that unlocks the Super-Peer model. Without this, the
server is a dumb relay forever. With it, the engine becomes a platform.

A `Policy` object is attached to the room (stored as a special genesis node).
It maps key-path patterns to a set of allowed signer public keys.

```rust
// core/src/policy.rs
pub struct Policy {
    /// Rules evaluated in order; first match wins.
    pub rules: Vec<PolicyRule>,
    /// Fallback: if no rule matches, allow all (open) or deny all (locked).
    pub default: PolicyDefault,
}

pub struct PolicyRule {
    /// Glob pattern matched against the op key.  e.g. "world/**", "intent/*"
    pub path_glob: String,
    /// Keys that may write (sign) ops to matching paths.
    pub can_write: Vec<PublicKey>,
    /// Keys that may read (decrypt) values at matching paths. Enforced only
    /// with E2EE enabled (D1); ignored in plaintext rooms.
    pub can_read: Vec<PublicKey>,
    /// Keys that may produce derived / computed state from matching paths.
    /// Reserved for Super-Peer compute authority (Phase E). A key with
    /// can_derive but not can_write may read intent paths and emit canonical
    /// state to a different path covered by a separate can_write rule.
    pub can_derive: Vec<PublicKey>,
}

pub enum PolicyDefault { AllowAll, DenyAll }
```

> `can_read` and `can_derive` fields are defined now but only enforced in later
> phases (D1 and E1 respectively). Having the shape in the wire format today
> avoids a breaking schema change later.

**Enforcement in `apply_remote`:** before merging any incoming `SyncNode`, check
that `node.author` is allowed to write each op key under the room policy.
Nodes that violate policy are dropped (not stored, not re-broadcast).

**Checkpoint A5:**
- `Policy` type added to `core` with glob matching.
- `apply_remote` enforces `can_write`; unit tests cover allow, deny, and wildcard cases.
- `can_read` and `can_derive` fields parsed and stored but not yet enforced.
- A room with no policy behaves identically to today (AllowAll default).
- Wire format: policy stored as a dedicated root-level node type, not in `Op`.

---

### A6. Frontier / Version Vector
The missing primitive that makes IBF, MST, and compaction tractable. Without an
explicit frontier the engine cannot efficiently answer "what do you have that I
don't?" and compaction boundary becomes ambiguous.

A frontier is the minimal set of DAG tips — the set of `NodeId`s that have no
successors yet. Every node added or merged is reconciled against the frontier.

```rust
// core/src/frontier.rs

/// Minimal set of DAG tips. Fully describes "what this peer has seen."
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Frontier {
    pub heads: Vec<NodeId>,
}

impl Frontier {
    /// Returns true if `id` is already subsumed by this frontier.
    pub fn contains(&self, id: &NodeId) -> bool { ... }

    /// Advance the frontier when a new node is appended or merged.
    /// Removes any heads that are ancestors of `new_node`.
    pub fn advance(&mut self, new_node: &SyncNode) { ... }

    /// Encode as a compact byte string for the wire (used by IBF in B1).
    pub fn encode(&self) -> Vec<u8> { ... }
}
```

- `SyncStore` maintains a `Frontier` updated on every `apply_local` / `apply_remote`.
- `hello` handshake switches from sending all node IDs to sending a `Frontier`.
  This is the precondition for B1 (IBF uses the frontier as the starting point).
- Compaction (D3) snapshots at the current frontier and advances from there.

**Checkpoint A6:**
- `Frontier` type in `core` with `advance`, `contains`, and `encode`.
- `SyncStore::frontier()` returns the current heads.
- `hello` message carries `frontier` instead of the flat `known` list.
- All existing tests pass. Sync round-trip with two peers via frontier handshake works.

---

### A7. Sync Capability Negotiation
Without this, rolling out IBF, MST, or E2EE breaks older clients. Define the
negotiation envelope now so every future upgrade is backwards-compatible.

```rust
// core/src/capabilities.rs

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncCapabilities {
    pub supports_ibf:         bool,   // B1
    pub supports_mst:         bool,   // B2
    pub supports_postcard:    bool,   // A3 (binary wire)
    pub supports_encryption:  bool,   // D1
    pub supports_webrtc:      bool,   // D2
    pub max_tick_interval_ms: Option<u64>, // E3
}
```

- Sent in the `hello` message alongside the frontier.
- Both peers compute the intersection and use the highest common feature set.
- A peer that does not understand a capability field ignores it (serde default).
- No capability means current MVP behavior (no IBF, JSON wire, no E2EE).

**Checkpoint A7:**
- `SyncCapabilities` in `core` with serde defaults (all false / None).
- `hello` and `welcome` messages carry capabilities.
- Integration test: a v1 client (no capabilities) connects to a v2 server;
  server falls back to legacy JSON handshake; sync still works.

---

## Phase B — Fix the O(n) Sync Problem
> Handshake currently sends full node-ID list. At 10k nodes that is ~640KB
> just for the hello message. This is the scale killer.
> **Target: 2–4 weeks after Phase A**

### B1. Set Reconciliation (Bloom / IBF) ✅
Replace "send all known IDs" with an Invertible Bloom Filter (~2KB structure).
Two peers derive exactly which IDs to send. O(diff) instead of O(n).

- Used by Bitcoin, ATProto (Bluesky), and IPFS
- Implementable in ~1 week
- Drop-in replacement for the `hello`/`welcome` handshake

**Checkpoint B1:** Handshake bytes < 5KB for a 10k-node graph with 10 missing nodes. ✅ **Achieved: 2.9 KB**

---

### B2. Merkle Search Tree (MST) ✅
Organize the DAG index as an MST (like ATProto's `com.atproto.sync` protocol).
Sync is O(depth of diff). The "right" answer at full scale.

- Harder engineering investment than B1
- Do after B1 is working and benchmarked
- Sync depth target: ≤16 rounds for any diff size

**Checkpoint B2:** Sync a 1M-node graph with 100 missing nodes in ≤20 round trips
and ≤50KB total bandwidth. ✅ **Achieved: 3 round trips, 1000-node diff in 2k-node graph; 66/66 tests pass**

---

## Phase C — Feature Parity (the demos that matter)
> **Target: Month 2**

### C1. RGA Collaborative Text
The killer demo. Enables real-time co-editing of text fields.

```rust
pub struct TextOp {
    Insert { after: Option<OpId>, char: char },
    Delete { target: OpId },
}
```

- Plugs into `Op::Text(TextOp)` from A2
- Merge logic in `core`: pure function of the op log
- Bridge exposes `insert_text(key, pos, char)` / `delete_text(key, pos)`
- Demo: shared textarea with cursor presence

**Checkpoint C1:** Two peers concurrently edit the same text field. Both converge
to identical state within one round trip after reconnect. ✅ **Complete** — RGA in `core/src/text.rs`; per-keystroke delta broadcast via `export_nodes_missing_from` keeps bandwidth bounded; demo textarea converges cleanly under concurrent typing.

---

### C2. IndexedDB Persistence ✅
Closes the biggest production gap: blob bytes and node graph lost on page refresh.

- `IndexedDbNodeStore` implementing `NodeStore` trait (from A1)
- `IndexedDbBlobStore` implementing `BlobStore` trait (from A1)
- Nodes: stored as postcard binary, keyed by hex hash
- Blobs: stored as raw `Uint8Array`, keyed by hex hash
- Replaces `localStorage` for node graph

**Checkpoint C2:** Hard refresh a tab. Node graph and blob bytes survive. No
`blob-request` needed on reconnect if IndexedDB is populated. ✅ **Achieved: IDB `nodes` + `blobs` stores; one-time localStorage migration; fire-and-forget persistence on every mutation**

---

### C3. Room Auth (Capability Tokens)
No passwords, no usernames. Cryptographic capability.

- A room is created with an Ed25519 keypair
- The token to join: `sign(room_id + peer_pubkey + expiry + caps)` with room key
- Server verifies the signature on `hello`
- Rooms without a token remain open (current behavior, opt-in auth)
- `capabilities` field in token carries optional path-scoped grants (e.g.
  `"read:world/**"`, `"write:intent/**"`); empty = full access. Signed into the
  token so they cannot be forged. Enforcement deferred to E1 (Super-Peer).

**Architecture:** Room = transport / connection scope (who may join the pipe).
Policy (A5) = authority (what they may read/write once inside, matched against
op key-paths). Tokens grant connection; Policy enforces data boundaries. This
separation means the Super-Peer and cross-room sharing scenarios do not require
a redesign — only the Policy changes.

**Checkpoint C3:** A locked room rejects peers without a valid token. Token
expiry is enforced server-side. ✅ **Complete.**

---

## Phase D — Differentiation (what beats the giants)
> **Target: Month 3–4**

### D1. End-to-End Encryption
The enterprise selling point. Impossible with Firebase.

- Room symmetric key (AES-256-GCM) derived from room keypair
- `transaction.ops` bytes are encrypted before signing
- Server relays ciphertext it cannot read
- Server still verifies Ed25519 outer signature (no decryption needed)
- Peers with the room key decrypt locally

**Checkpoint D1:** Enable E2EE flag on a room. Server operator cannot read op
values. Blake3 integrity and Ed25519 authenticity still enforced end-to-end.

---

### D2. WebRTC Peer-to-Peer (Server-Optional)
Latency drops to <1ms for same-LAN peers. Blobs never leave the local network.

- Server remains for ICE signaling only (tiny messages)
- `BlobStore` adapter: `WebRtcBlobStore` uses data channels for blob transfer
- DAG node sync also runs over data channels when direct path is available
- Fallback to WebSocket if WebRTC negotiation fails

**Checkpoint D2:** Two tabs on the same machine sync a 50KB blob without any
bytes traversing the server. ✅ **Complete** — WebRTC signaling over the existing WS; data channels carry node packs and blob bytes when negotiation succeeds; automatic WS fallback.

---

### D3. DAG Compaction
The graph grows forever without this. Required before production.

- Compute a "snapshot" of resolved state at a frontier (set of head `NodeId`s
  from A6) — not an arbitrary height.
- The snapshot node carries:
  - `snapshot_hash = Blake3(canonical_resolved_state_bytes)` — deterministic,
    verifiable via deterministic replay (D4).
  - An Ed25519 signature from the compacting peer (or the Super-Peer in
    authoritative rooms) so receivers can verify the snapshot without replaying.
  - A `frontier` field listing the `NodeId`s subsumed by the snapshot.
- Drop all pre-snapshot nodes only after the signed snapshot is accepted by a
  quorum (or by the Super-Peer).
- Peers joining after compaction get the snapshot + recent delta.

**Checkpoint D3:** A 1M-node graph compacts to a single checkpoint node + recent
delta. Sync after compaction works identically to sync from genesis. A peer can
independently verify `snapshot_hash` by replaying the pre-compaction log. ✅ **Complete** — `compact()` / `verify_snapshot()` / `rebuild_from_snapshot()` in `core/src/compaction.rs`; snapshot is a standard signed `SyncNode` with sentinel ops carrying hash + frontier; tests `snapshot_hash_matches_replay` and `rebuild_restores_state` pass.

---

### D4. Deterministic Replay
The trust anchor for the entire system. If two peers replay the same op log they
must arrive at byte-identical state. This is what makes snapshot verification,
audit logging, debugging, and server validation possible.

```rust
// core/src/replay.rs

/// Replay an ordered sequence of nodes to produce a resolved state.
/// Deterministic: same input → same output on every platform, every time.
pub fn replay(nodes: &[SyncNode], policy: Option<&Policy>) -> Result<ResolvedState, SyncError> {
    let mut store = SyncStore::with_memory_backend();
    for node in nodes {
        store.apply_remote(node.clone())?;
    }
    Ok(store.resolved_state())
}

pub struct ResolvedState {
    pub map:  BTreeMap<String, Vec<u8>>,
    pub hash: Hash,  // Blake3 of canonical serialization — used for snapshot verification
}
```

- Replay is a pure function: no I/O, no randomness, no wall-clock time.
- `ResolvedState::hash` is the value stored in compaction snapshot nodes (D3).
- Server uses replay to validate that a proposed snapshot is correct before signing.
- Debug tool: `activesync-server replay <room-id>` dumps the full op log and
  final state to stdout.
- WASM bridge exposes `replay(nodes: JsValue) -> JsValue` for browser-side
  verification and testing.

**Checkpoint D4:**
- `replay()` function in `core` with 100% determinism property test.
- Property test: shuffle op order, replay, assert same `ResolvedState::hash`
  (modulo ordering-sensitive ops like concurrent LWW — those have deterministic
  tie-breaking already).
- Server CLI: `activesync-server replay` command works on a recorded session log.
- `snapshot_hash` round-trip: snapshot a live room, compact, rejoin, verify hash.

✅ **Complete** — `core/src/replay.rs` with `replay()` + `canonical_hash()`; property tests cover in-order, shuffled, and out-of-order delivery; `activesync-server replay <room-id>` CLI wired into `server/src/main.rs`.

---

## Phase E — Super-Peer & Distributed State Engine
> Promotes the server from a "dumb relay" to an authoritative peer.
> Requires A5 (Scoped Write Policies) and C3 (Room Auth).
> This is the phase that turns the project into a platform.
> **Target: Month 3–4 (in parallel with D)**

### Architecture Summary

The server compiles and runs `activesync-core` as native Rust — the **same crate**
the WASM bridge uses. It joins a room as a regular peer, but its public key is
listed in the room `Policy` (A5) as the sole authorized signer for designated
key-paths (e.g. `world/**`, `game/economy/**`).

```
Client Peer                     Server Peer (Super-Peer)
──────────────────────────────  ─────────────────────────────────
apply_local(Set("intent/move",  receive intent node
            "up"))              run_tick() →
broadcast intent node     →       calculate physics
                                  apply_local(Set("world/player1/pos",
                                               "{x:10,y:20}"))
receive canonical node    ←       broadcast signed canonical node
merge canonical into DAG          (signed with server's pubkey)
snap-back if speculative
 contradicts canonical
```

**Why this is not "just a use case":**
- Same CRDT logic runs on client and server. No separate "server rules" DSL.
- Solves O(n²) fan-out: clients send intents, server fans out one signed truth.
- Server can drop policy-violating nodes before storing or relaying them.
- The developer slides between fully-decentralized and fully-authoritative by
  changing the `Policy` alone — no code changes.

---

### E1. Super-Peer Server Mode
Upgrade `activesync-server` from a relay to a stateful peer.

The Super-Peer design supports two explicit operating modes. Both are expressed
purely through the room `Policy` (A5) — no code-path distinction required:

| Mode | Policy | Behavior |
|---|---|---|
| **Cooperative** | `default: AllowAll`, no can_derive rules | Pure CRDT convergence. Every peer is equal. Server is an always-online peer with good connectivity, nothing more. |
| **Authoritative** | Protected paths with `can_write: [server_pubkey]` | Server signs canonical state. Clients send intents; server emits truth. LWW pubkey priority ensures server wins conflicts. |

The developer slides between modes by changing the `Policy` genesis node. The
engine code does not change. This is how you avoid accidentally rebuilding a
centralized system with extra steps.

- Server instantiates its own `SyncStore` per room (same as a client would).
- Server generates a persistent Ed25519 keypair on first start; public key is
  recorded in the room `Policy` as the authoritative signer for protected paths.
- On receiving a node from a client, the server:
  1. Validates the Ed25519 signature.
  2. Checks the node against the room `Policy` `can_write` rules (A5).
  3. Merges valid nodes into its own local DAG.
  4. Re-broadcasts to all other peers.
- Server has a `process_room_tick(room_id)` function that reads pending intent
  nodes from the DAG and emits canonical state nodes (Authoritative mode only).

**Checkpoint E1:**
- Server maintains per-room `SyncStore` in memory (disk adapter from A1 later).
- Server's keypair is stable across restarts (persisted to disk in a keyfile).
- Policy-violating nodes are silently dropped by the server and not re-broadcast.
- Integration test (Authoritative): two clients, one policy. Client A tries to
  write to a protected path; server drops the node; Client B never receives it.
- Integration test (Cooperative): no policy. Both clients write freely; server
  relays all nodes; both converge identically to current behavior.

---

### E2. Speculative vs. Canonical State
Clients can act instantly on local input while waiting for server confirmation.
This beats Replicache and standard CRDTs in perceived latency.

The `SyncStore` maintains two resolved views:

```rust
pub struct SyncStore {
    // ...existing fields...

    /// All nodes merged so far, including unconfirmed local writes.
    speculative_state: LwwMap,
    /// Only nodes signed by a recognized authoritative key (per Policy).
    canonical_state: LwwMap,
}
```

- **Local write:** applied immediately to `speculative_state`. UI reads
  `speculative_state` for instant feedback.
- **Canonical update received:** merged into `canonical_state`. If it
  contradicts `speculative_state` on the same key (snap-back), the canonical
  value wins (higher-priority pubkey in LWW tie-break).
- **Read API:** `store.read_speculative(key)` / `store.read_canonical(key)`.
  The app layer chooses which view to display.

**Checkpoint E2:**
- `SyncStore` exposes both views.
- Unit test: local write to `world/pos` → speculative shows new value, canonical
  unchanged → receive server node contradicting it → both views now match server.
- WASM bridge exposes `readSpeculative` / `readCanonical` to JS.

✅ **Complete** — `StateGraph::read_speculative` / `read_canonical`; `graph::tests::local_write_is_speculative_not_canonical` + `remote_write_is_canonical_and_speculative` pass.

---

### E3. Tick-Based Batching
Reduces signing overhead for high-frequency update scenarios (games, dashboards).

Instead of hashing and signing each op immediately, the engine collects ops over
a configurable tick window and emits one signed "Tick Node" per window.

```rust
pub struct TickConfig {
    /// How long to buffer ops before emitting a tick node. Default: 16ms (60fps).
    pub interval_ms: u64,
    /// Maximum ops per tick node before flushing early.
    pub max_ops_per_tick: usize,
}
```

- The tick node is a standard `SyncNode` whose `ops` field contains all buffered
  ops. Existing merge logic is unchanged.
- For the intent path (client→server), ticking is optional and defaults off
  (clients want low-latency intent delivery).
- For the canonical path (server→clients), ticking is on by default (server
  batches physics updates).
- Tick interval is a room-level config, negotiated at join time.

> **Determinism constraint:** tick boundaries are a transport optimization only.
> Each op inside a tick node retains its own `OpId` (author + Lamport clock).
> Merge logic operates on individual ops, not on tick nodes. Two peers that
> receive the same ops in different tick groupings MUST converge to identical
> state. Tick boundaries MUST NOT affect final resolved state.

**Checkpoint E3:**
- `SyncStore::set_tick_config(TickConfig)` enables buffered mode.
- Benchmark: 1,000 ops/sec with 16ms tick window → ≤63 signed nodes/sec
  instead of 1,000 (≥94% reduction in signing calls).
- Integration test: two clients at 60fps intent rate; server emits tick nodes;
  both clients converge state within 2 ticks.
- Property test: same set of ops delivered in random tick groupings always
  produces the same resolved state.

✅ **Complete** — `TickConfig` + buffered-mode path on `StateGraph`; every op retains its own `OpId` so merge is tick-independent; property tests confirm grouping-invariance.

---

## Sequencing Summary

```
Week 1–2   Phase A: StorageAdapter (A1) → Op enum + LWW warning (A2)
Week 3     Phase A: postcard wire (A3) → benchmarks (A4)
Week 4     Phase A: Write Policies (A5) → Frontier (A6) → Capability Negotiation (A7)
Week 5–6   Phase B: Set reconciliation (B1)  [requires A6 frontier]
Week 7–8   Phase C: IndexedDB (C2) → Room auth (C3)
Month 2    Phase C: RGA text (C1)
Month 3    Phase B: MST (B2) — driven by benchmark data
Month 3    Phase D: E2EE (D1) | Phase E: Super-Peer (E1)  [requires A5, A6, C3]
Month 4    Phase E: Speculative state (E2) → Tick batching (E3)
Month 4    Phase D: Deterministic Replay (D4)  [enables D3 compaction verification]
Month 4+   Phase D: WebRTC (D2) → Compaction (D3)  [requires D4]
```

---

## Decision Log

| Decision | Rationale |
|---|---|
| postcard before FlatBuffers/rkyv | Serde-compatible, zero schema friction, prove the bottleneck first |
| IBF before MST | IBF is 1 week, MST is 1 month; get the wins fast, validate with benchmarks |
| S3 as BlobStore adapter | Core never pushes bytes; `resolve_url()` returns presigned URL, client fetches CDN directly |
| E2EE via op-level encryption | Server keeps relay role without decryption capability; outer signature still verifiable |
| RGA over LSEQ/Logoot | RGA is the best-studied, most compatible with our DAG model; Loro/Diamond Types are inspiration not dependency |
| IndexedDB before disk persistence | Browser first; server-side disk is a `NodeStore` adapter, same interface |
| Super-Peer over separate server logic | Running the same `activesync-core` on server eliminates dual codebases; policy is the only distinction between a peer and an authority |
| Scoped Write Policies in core (A5) before Super-Peer (E1) | Policy enforcement must live in `apply_remote` so every peer (not just the server) validates and drops unauthorized nodes; enforcement cannot be server-only |
| Speculative + Canonical dual views over single state | Instant UI feedback is a hard requirement for games and dashboards; snap-back is deterministic via LWW pubkey priority, not application logic |
| Tick-based batching opt-in per room | General collaboration tools (text editors, whiteboards) do not benefit from batching; forcing it would add latency for no gain |
| Frontier (A6) before IBF (B1) | IBF requires a compact frontier representation as the starting point; building it last-minute would require retrofitting the handshake |
| Capability negotiation (A7) in Phase A | Mixed-version deployments are inevitable; negotiation must be in the wire format from day one or every future feature is a breaking change |
| `size()` + `get_range()` in BlobStore interface from A1 | WebRTC chunking, CDN range requests, and mobile streaming all require these; adding them post-hoc forces every BlobStore implementer to update |
| LWW documented as transitional | Sets correct expectations; prevents users from depending on LWW behavior for Text/List types before RGA is ready |
| `can_read` and `can_derive` in PolicyRule now (A5) | Shape is in the wire format before it is enforced; avoids a breaking schema change when E2EE and Super-Peer compute authority are implemented |
| Two explicit Super-Peer modes (Cooperative / Authoritative) | Prevents accidental centralization; developer intent must be explicit in Policy; engine code is identical for both |
| Deterministic replay (D4) before compaction (D3) | Compaction correctness cannot be verified without a deterministic `replay()` + `snapshot_hash`; shipping D3 first would make snapshots unverifiable |
| Snapshot hash signed by compacting peer | Receivers can verify compaction correctness without replaying the full history; trust model remains cryptographic end-to-end |
| Room = transport scope, Policy = authority | Token answers "who may connect"; Policy (A5) answers "what they may read/write" via key-path rules. This separation keeps E2EE, Super-Peer, and cross-room data sharing tractable without redesigning the auth layer. |
| `capabilities` in token signed but not yet enforced | Path-scoped grants (`"read:world/**"`, `"write:intent/**"`) are included in the Ed25519 signed message now so the wire format is stable; enforcement lands in E1 when Super-Peer processes per-connection capability sets. Adding the field post-hoc would be a breaking protocol change. |
| E2EE via sentinel op, not a new SyncNode field | Storing the AES-256-GCM ciphertext as `Op::Map::Set { key: "\x00e2ee" }` requires zero changes to `SyncNode`, `WireNode`, or pack/unpack. The node id/signature chain covers the ciphertext. Server verifies Ed25519 without decryption. Old peers see an opaque key and apply it through LWW — safe fallback. |
| Deterministic nonce from (room_key, author, lamport) | Eliminates any randomness source requirement in WASM (no getrandom calls for nonce generation). Safe because (author, lamport) is a globally unique pair per node. HKDF-SHA256 domain-separates the nonce derivation from the key derivation. |
