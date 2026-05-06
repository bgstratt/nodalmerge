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
- ✅ **Batched Ed25519 verification** — `StateGraph::apply_remote_batch(Vec<SyncNode>) -> BatchResult { accepted, rejected }`; dedupe → parallel `ed25519_dalek::verify_batch` (rayon, 256-node chunks, native only) → per-chunk per-node fallback on failure → session-scoped `verified_ids` cache; server `import_nodes` and catchup paths use it. **`merge_10k_batch` 28.6 ms vs serial `merge_10k` 320 ms (~11×).** Chunk size empirically tuned: 64 → 30.5 ms, 128 → 28.8 ms, 256 → 28.6 ms, 512 → 28.2 ms (within noise; 256 preserves parallelism on smaller catchups). 117 tests pass.
- ✅ **Canonical `Transaction` hash is `postcard`-encoded** (was `serde_json`). 3.46× smaller preimage and 2.1× faster `hash()` end-to-end (1.07 µs → 509 ns on a 3-op tx). Removes a latent footgun where a future `serde_json` formatting change could silently invalidate signatures. Micro-bench lives in `core/benches/tx_hash.rs` as a regression guard.
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
| `merge_10k_batch` | **31 ms** (`apply_remote_batch`) | <10 ms | 🔥 ~10× win. Dedupe → parallel `ed25519_dalek::verify_batch` (rayon, chunk = 64) → per-chunk fallback to per-node verify on failure → session-scoped `verified_ids` cache skips re-broadcasts. Canonical hash is `postcard`-encoded (2.1× faster than JSON). Server `import_nodes` and the catchup pack path go through this automatically. |
| `tx_hash_postcard_plus_blake3` | **509 ns** | — | Canonical `Transaction::hash`: `postcard::to_allocvec` + Blake3. Was 1.07 µs with `serde_json`. |

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
| postcard before FlatBuffers/rkyv | Serde-compatible, zero schema friction. Validated: we shipped it, benchmarked it (3.46× smaller than JSON, 1.83× faster to produce), and extended it to canonical hashing. rkyv/FlatBuffers would only pay off for mmap-scale snapshots or cross-language wire sharing — neither applies. |
| Canonical `Transaction::hash` uses `postcard`, not `serde_json` | Protocol-level: the signed preimage is now the same format as the wire. 3.46× smaller, 2.1× faster. Removes a latent footgun where a future `serde_json` formatting change would silently invalidate every existing signature. Wire-breaking — acceptable because pre-1.0 and no persisted deployments. |
| IBF **and** MST, negotiated per session (supersedes "IBF before MST") | Both shipped; they are complementary, not alternatives. IBF handles the 99% case (small diffs, 2.9 KB hello) and MST handles large or structural diffs (≤3 round trips, ~50 KB). Capability flags (`supports_ibf`, `supports_mst`) pick at join time; a peer that speaks neither falls back to the pack path. |
| `BATCH_VERIFY_CHUNK = 256` for `verify_batch` | Empirically tuned on Zen 3, 12 cores, `ed25519-dalek` 2.2 simd backend. Sweep: 64→30.5 ms, 128→28.8 ms, 256→28.6 ms, 512→28.2 ms (noise). 256 plateaus the Pippenger curve while preserving chunk count for rayon load balance on smaller catchups. Further wins require AVX-512 IFMA (hardware out of scope) or a different signature primitive. |
| S3 as BlobStore adapter | Core never pushes bytes; `resolve_url()` returns presigned URL, client fetches CDN directly |
| E2EE via op-level encryption | Server keeps relay role without decryption capability; outer signature still verifiable |
| RGA over LSEQ/Logoot | RGA is the best-studied, most compatible with our DAG model; Loro/Diamond Types are inspiration not dependency. Known limits are documented in separate entries (move op, run compression, rich text) rather than hidden. |
| Text is per-character RGA with tombstones — no move op, no rich text | Correct for concurrent typing, interleaving, and offline convergence (what C1 demonstrates). Three known gaps, each tracked as separate future work: (1) **Move** — not representable as delete+insert under concurrent edit of the moved block; lives in the future List CRDT (fractional-index with stable block IDs), not in Text. (2) **Run compression** — large paste = N char ops; an RGA-run variant would compress to a single op with length, no wire break. Schedulable when text throughput is a real constraint. (3) **Rich-text spans** — Peritext-style attributed ranges; deferred until a product actually needs it. |
| Merge strategy is type-selected, not value-configurable | `Op::Map` = LWW, `Op::Text` = RGA, `Op::List` = fractional-index (future). Apps pick the right type per field — same model as Automerge/Yjs. A "pluggable merge strategy per key" would let two peers with different config diverge on merge; that is strictly worse than LWW. The opaque `Vec<u8>` Map value is the escape hatch for custom semantics (app-level sub-CRDTs, e.g. an `Op::Map::Increment` counter if/when needed). |
| LWW tie-break is fixed at `(lamport desc, pubkey desc)` | This is a protocol contract, not an implementation detail. Every peer must use the exact same rule or they diverge. `pubkey desc` rather than asc is arbitrary but stable; Super-Peer authority sorts after peers because the server pubkey is listed first in `Policy.can_write` and the resolver prefers authoritative rules before falling back to pubkey order. |
| Session-scoped `verified_ids` cache, never persisted | The cache lives on a `StateGraph` instance and resets on restart. Intentional: signatures are always re-checkable from the stored bytes, so a persistent cache would add a consistency surface with zero benefit. F4 persistent NodeStores inherit this property — restart reverifies. |
| Idle-room eviction is gated on durable persistence | Dropping an in-memory `Room` is data loss. `--idle-timeout` (default 300 s, 0 disables) runs a 60 s sweeper that evicts rooms with zero connected peers past the timeout *only* when `ServerPersistence::is_durable()` returns true (`DirPersistence` yes, `NoPersistence` no). On an in-memory build with `--idle-timeout` set the server logs a warning and the sweeper does not start. Double-safeguard: the sweep also requires `Arc::strong_count(&Room) == 1` so a mid-handshake join that has cloned the Arc but not yet called `register_peer` isn't evicted out from under it. |
| No nested rooms, no sub-rooms — use Policy + F3 subscriptions instead | "Sub-rooms" / "rooms within rooms" / "soft partitions" always decompose into one of three primitives we already have (or will): **separate top-level rooms** for hard isolation, **Policy rules** for authority over key-path subtrees inside one room (A5, shipped), **F3 subscription scoping** for bandwidth/visibility slicing inside one room (client-side F3a, server-side F3b). Nested rooms would duplicate every one of these with a worse trust boundary (who signs the child's genesis? who authorizes promotion?) and force every consumer of `StateGraph` to recurse. Rooms stay flat. |
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
| WebRTC lives in the SDK, not the engine | Transport and authority are orthogonal. The engine already exposes transport-agnostic primitives (`export_nodes_missing_from`, `import_pack`, `store_blob_bytes`, `compute_ibf_b64`, `diff_with_ibf_b64`) — everything a non-WS transport needs. The SDK owns both WS and WebRTC; rule is **WS always** (server = persistence + authority + guaranteed relay for peers without a direct channel), **WebRTC additionally** per-peer when a data channel is open. Content-addressed dedup in `import_pack` makes double delivery free. Putting RTC inside `activesync-core` would leak a browser-only concern into a crate that also compiles for servers and CLI tools. |

---

## Phase F — Make the Engine Usable
> The engine is infra-complete. This phase turns it into something a product
> team other than us can adopt without reading the source.
> **Target: next milestone cycle.**

### F0. Close the security gap (blocker for F4 and F5)
Verify that WebSocket accept-time token enforcement in `server/src/ws_handler.rs`
rejects any `hello` without a valid `RoomToken` for the target room, and that
token `capabilities` are surfaced to the room so future F3 work can use them.
If missing, this is non-negotiable before persistent stores or hosted mode —
otherwise we're persisting unauthenticated writes.

**Exit:** integration test `ws_rejects_unsigned_hello`; integration test
`ws_accepts_signed_token_with_caps`; capability list attached to the `PeerSession`
struct on the server side.

### F1. Stable SDK surface
The user-facing API today is `SyncStore.set / get / subscribe / text_insert / …`
on a single flat namespace. That's a 1:1 mirror of the bridge, not an SDK. Ship
a Firebase/Replicache-style document API on top without changing the engine:

```js
const doc = await createDoc({ room: "room-1", serverUrl });
const world = doc.map("world");
const intents = doc.map("intents");
const notes = doc.text("notes/welcome");

world.set("player1", { x: 10, y: 20 });
notes.insert(0, "Hello");
doc.onChange(ev => …);
```

Internally this is a thin JS wrapper around today's `SyncStore`. No core changes.

**Status (Apr 2026 — SHIPPED):** `web/sdk.js` exports `createDoc` with
`MapHandle` + `TextHandle` + `PresenceHandle` + `onChange` + full WS transport
(hello/welcome/pack/request + IBF + MST + blob round-trip + exponential-backoff
reconnect) + WebRTC peer mesh (F1b, see below). `web/sdk.d.ts` ships
TypeScript types including `MeshPeer` + `transport: 'ws' | 'webrtc'` on change
events. `docs/sdk.md` covers quickstart + API + known limitations. Token
signing (C3) is wired through `roomSeed` + `tokenCaps`. `doc.store` is the escape
hatch for anything the SDK doesn't surface yet (speculative reads, raw wire).
`web/demo.js` has been ported onto `createDoc` and is now the reference
consumer of the SDK rather than a parallel impl.

**F1b — WebRTC peer mesh inside the SDK (shipped).** `makePeerMesh` in
`web/sdk.js` opens two `RTCDataChannel`s per remote peer (`sync` JSON,
`blobs` binary `[32-byte hash][bytes]` frames). Initiation is tie-broken by
pubkey hex order. Handshake uses `compute_ibf_b64` → `diff_with_ibf_b64` →
`export_nodes_missing_from` for node reconciliation, and
`local_blob_hashes_json` → `blob-have` → binary frames for blob exchange.
Signaling rides the existing WS relay (`webrtc-offer`/`-answer`/`-ice`, all
already intact in `server/src/ws_handler.rs` from D2). Policy: WS is the
authoritative path always; WebRTC is additive per peer when a data channel is
open; `import_pack` dedupes on node id so double delivery is free.
`createDoc({ transport: 'auto' | 'ws-only' })` controls mesh creation;
`iceServers` overrides the default Google STUN pair. `doc.peers()` returns
`[{ pubkey, transport, syncReady, blobsReady, connectionState }, …]`. Runtime
gate falls back to `ws-only` when `RTCPeerConnection` is undefined (Node / old
environments). Zero engine, bridge, or server changes.

**Exit:** `web/pkg/activesync.js` ships both `SyncStore` (low-level, unchanged)
and `createDoc` (high-level); TypeScript types for both; demo rebuilt on
`createDoc`; one page of docs under `docs/sdk.md`. ✅

### F2. Presence / awareness as a first-class API
The awareness side-channel already exists. Productize it:

```js
doc.presence.set({ name: "Brad", color: "#f43" });
doc.presence.set({ cursor: { x: 312, y: 44 } });
doc.presence.others().forEach(peer => …);
doc.presence.onJoin(cb); doc.presence.onLeave(cb); doc.presence.onUpdate(cb);
```

Implemented purely in the JS wrapper over the existing awareness messages —
no bridge or server changes. `doc.presence.set(patch)` merges and broadcasts;
the SDK heartbeats on `presenceHeartbeatMs` (default 15s) so joiners see
everyone, and sweeps stale peers after `presenceStaleMs` (default 45s). Leave
events fire on server `peer-left`, on `clear()`, or on heartbeat staleness.

**Status (Apr 2026):** shipped in [web/sdk.js](web/sdk.js) + types in
[web/sdk.d.ts](web/sdk.d.ts) + docs in [docs/sdk.md](docs/sdk.md). Zero
bridge/server changes. Demo port still rides with the rest of the F1 demo
rewrite (post-F3a).

**Exit:** presence works in the demo without any bridge changes; documented.

### F3. Partial replication / subscription scoping (a.k.a. soft partitions, doc slicing)
Today peers replicate the whole room. Product apps want `doc.subscribe("world/**")`
without the cost of the unrelated `chat/**` traffic. This is the engine answer
to "sub-rooms" / "rooms within rooms" / "soft partitions" — we do **not** nest
room objects (see decision log). Instead, a single room carries multiple path
subtrees and each peer subscribes to the slice it cares about. Rooms stay flat
and independent; subscriptions carve the room internally.

Ship this in two steps so we can defer server enforcement until real usage
demands it:

1. **F3a — Client-side filter.** `createDoc` accepts `subscribe: ["world/**"]`.
   Client still receives the full stream but only materializes matching ops.
   Zero wire changes. Gives product teams the API shape immediately.
2. **F3b — Server-side filter.** Server reads the client's subscription
   patterns from the `RoomToken.capabilities` field (which F0 already wired
   through) and only relays matching nodes. Requires a `subscribe` wire message
   and server-side path-matching against each node's op keys. This is where we
   also finally enforce `read:` capabilities end-to-end.

**Status (Apr 2026 — F3a):** shipped. `createDoc({ subscribe })` accepts
glob patterns; `doc.subscription` / `doc.isSubscribed(path)` / `doc.subscribe(patterns)`
/ `doc.onSubscriptionChange(cb)` exposed on the Doc. `MapHandle.all/get/onChange`
filter by subscription; `TextHandle` construction throws if the key is outside
the subscription. Glob compiler handles `**`, `/**` as optional-subtree, `*`,
and literals (unit-tested 16/16). Zero wire or core changes.

**Status (Apr 2026 — F3b):** shipped. SDK's `hello` carries an optional
`subscribe: [patterns]` field, and a runtime `{type:"subscribe", patterns}`
message re-scopes an open connection. Server-side `Subscription` wraps a
pure-Rust glob matcher (no regex dep) with identical semantics to the SDK;
`filter_pack_for_subscriber` unpacks each relayed pack, drops nodes whose op
keys don't match, and suppresses the send entirely when nothing remains.
Sentinel-prefixed keys (`\x00…` — E2EE envelopes, snapshot meta) are always
passed through so encrypted rooms keep working. Default subscription is `**`
(zero-overhead fast path). 10 Rust unit tests cover trailing `/**`, middle
`/**/`, leading `**/`, segment-local `*`, literals, and or-of-patterns.
Filtering applies to both the catch-up pack and the steady-state broadcast.
Writes are **not** filtered — path authority is still Policy's job (A5).

**Exit (F3a):** demo adds a "scope" filter that visibly cuts the subscription;
`doc.subscribe` documented. **Exit (F3b):** server drops non-matching nodes
before relay; bench shows 2-room shared connection drops fan-out.

### F4. Persistent NodeStore + BlobStore (deployment blocker)
Today server state is in-memory. Ship two adapters behind the existing traits:

- `SqliteNodeStore` — one row per `SyncNode`, `(id BLOB PRIMARY KEY, parents BLOB, author BLOB, lamport INTEGER, payload BLOB, sig BLOB)`, plus a `leaves` table kept in sync with the graph's `leaves` set.
- `FileBlobStore` — content-addressed file-per-blob under a root dir; `get_range` uses `pread`. Pairs naturally with `size()` which A1 already landed.

`activesync-server` picks the adapter via CLI flag (`--store sqlite:./room.db`
or `--store mem`). No core changes; the traits from A1 already fit.

**Exit:** server restarts preserve the full DAG and all blobs; new bench
`startup_replay_10k` loads a 10k-node room in <500 ms; `docs/deployment.md`
covers ops basics (backups = file copy for blobs, `.backup` for SQLite).

**Status (Apr 2026 — F4):** shipped. `--store <path>` flag enables a
SQLite (`<path>/activesync.db`) + file-per-blob (`<path>/blobs/<room>/<hash>`)
backend rooted at a single directory. Absent, the server stays in-memory
(default). Persistence is a narrow write-through hook (`ServerPersistence`
trait in `server/src/store.rs`) that sits *above* the core
`NodeStore`/`BlobStore` traits — `Room::new` hydrates from disk first, then
every accepted node and blob is persisted immediately. SQLite uses
`journal_mode=WAL`, `synchronous=NORMAL`, and `INSERT OR IGNORE` for idempotency;
blob writes go through tmp-file + rename; hydrate re-verifies blob hashes and
drops tampered files. `room_id → filesystem name` sanitization escapes
non-`[A-Za-z0-9_-]` bytes as `_HH`. Integration test
`large_room_hydrates_quickly` (10k nodes, full write + read cycle) passes
under a 5s hydrate ceiling; `room_survives_restart_with_nodes_and_blobs`
covers mixed node+blob round-trip. `docs/deployment.md` documents layout,
backups, and tuning.

### F5. Hosted story + real auth integration
Only meaningful once F0–F4 land. Two pieces:

- **JWT → RoomToken bridge.** Trusted issuer (your app's auth server) signs
  a JWT with `(room_id, pubkey, caps, exp)`; a small Rust crate verifies the
  JWT and mints a `RoomToken`. Lets customers plug Clerk / Supabase / Auth0
  in front without us owning identity.
- **Managed deployment sketch.** Single-binary Docker image with SQLite +
  file blobs + the JWT bridge; `docker run activesync/server`. Not a product
  commitment — a reference deployment.

**Exit:** JWT bridge crate with tests; reference `Dockerfile`; one-page
"self-host in 5 minutes" doc.

---

### F — not doing (intentionally)

| Item | Why skipped |
|---|---|
| Whiteboard / diagram demo | Whiteboards exist; the tech isn't uniquely suited vs. a dozen other tools; wedge validation is better served by a real product (e.g. SpeechSlate) that exercises text + blobs + presence + policy together. |
| Further engine perf below 28 ms/10k merges | Post-postcard flamegraph shows we are ~95% inside `verify_batch`; further wins require AVX-512 IFMA (hardware we don't target yet) or a new signature scheme (ed25519 → BLS aggregation), both out of scope. `Frontier::advance`, ancestry caches, and allocation hunting did not appear in the profile — speculative optimization skipped. |
| Server-enforced subscription filters before F3a | We don't know what patterns real apps want. Ship client-side first, let product usage teach us the shape, then codify in F3b. |
| A custom query language | `doc.map().list().text()` + path globs already cover the product surface; a DSL is a second-system trap. |

---

### F perf sweep log — Ed25519 batch chunk size (Apr 2026)

Host: AMD Ryzen 9 5900X (Zen 3, 12 physical cores), Rust 1.93 stable,
`ed25519-dalek` 2.2 simd backend, rayon 1.12. Bench: `merge_10k_batch`,
20 samples per point.

| `BATCH_VERIFY_CHUNK` | Median | Δ vs 64 |
|---|---|---|
| 64 (previous) | 30.48 ms | — |
| 128 | 28.84 ms | −5.4% (p<0.01) |
| **256 (current)** | **28.57 ms** | **−6.3%** |
| 512 | 28.19 ms | −7.5% (within noise of 256) |

**Decision:** 256. The Pippenger efficiency curve has plateaued by 256; pushing
to 512 buys nothing statistically significant while halving the number of
rayon chunks, which would hurt load balance on smaller catchup payloads
(at 1k nodes: 4 chunks vs. 2 chunks on 12 cores). `verify_batch` is now ~95%
of wall-clock on the 10k merge — further gains require AVX-512 IFMA or a
different signature primitive.


---

## F5 — Trusted issuer bridge + Docker self-host (SHIPPED)

Crate `jwt-bridge/` (`activesync-jwt-bridge`) verifies a JWT signed by your
auth provider (HS256 / RS256 / ES256 supported via `jsonwebtoken 9`) and
mints a `RoomToken` using the room's Ed25519 key. Claim shape:
`{ room, pubkey(hex), exp, caps[] }`; standard JWT claims `exp/nbf/iss/aud`
are validated by the JWT layer. Optional `allowed_issuers` and
`allowed_audiences` allow-lists applied per-call. 7 unit tests + 1 doctest.

Reference `Dockerfile` at repo root: multi-stage
(`rust:1.82-slim-bookworm` → `debian:bookworm-slim`), non-root user,
`--store /data` by default, `EXPOSE 7878`. Stubs dep-only manifests for
cache-friendly layer reuse. Build is rot-guarded by
`.github/workflows/docker.yml`, which rebuilds and smoke-tests the image on
every push/PR that touches the Dockerfile or any crate it compiles in.

`docs/self-host.md` is the 5-minute walkthrough: build image, run with a
mounted volume, wrap the bridge in a 30-line Axum service, point your
issuer at the claim shape, wire the client. Cross-linked from
`docs/deployment.md`.

---

## F6 — Pluggable persistence: S3-compatible blobs + split trait (SHIPPED)

> **Status.** Implemented end-to-end. Trait split, S3 store crate (Direct +
> Delegate auth), capability negotiation, wire protocol, SDK bifurcation,
> and a live MinIO integration test all green. See "Implementation
> locations" at the end of this section for the file map.

### Problem

Today `ServerPersistence` in `server/src/store.rs` is a single trait that
owns both nodes and blobs. Two impls ship: `NoPersistence` (memory) and
`DirPersistence` (SQLite + file-per-blob). Neither fits a production
SpeechSlate-class deployment:

- **Nodes** belong in whatever database the product already runs (Mongo
  for SpeechSlate, Postgres for others). Standing up SQLite alongside
  the existing DB is pure ops burden.
- **Blobs** (audio recordings, board images) belong in S3/R2/MinIO with
  a CDN in front. `FileBlobStore` works for self-host demos but doesn't
  scale past one server; forces egress through the sync server even
  when a CDN would serve 1000× faster.

The `core::BlobStore` trait already anticipated this — `resolve_url()`
returns `Option<String>` so a backend can redirect instead of transmitting
bytes. F6 wires that redirect through `ServerPersistence` → WS protocol →
SDK, and splits the trait so Mongo-nodes + S3-blobs is a one-line compose.

### Shape

**Split the trait.** `ServerPersistence` becomes a composition of two
narrower traits:

```rust
// server/src/store.rs

pub trait NodePersistence: Send + Sync + Debug {
    fn load_room_nodes(&self, room_id: &str) -> Vec<SyncNode>;
    fn persist_node(&self, room_id: &str, node: &SyncNode);
    fn persist_nodes(&self, room_id: &str, nodes: &[&SyncNode]) { /* default loops */ }
    fn is_durable(&self) -> bool { true }
}

pub trait BlobPersistence: Send + Sync + Debug {
    fn load_room_blobs(&self, room_id: &str) -> Vec<(Hash, Vec<u8>)>;
    fn persist_blob(&self, room_id: &str, hash: &Hash, bytes: &[u8]);
    fn blob_gc_sweep(&self, room_id: &str, live: &HashSet<Hash>, grace: Duration) -> usize { 0 }
    /// F6: redirect target for the client. `Some(url)` tells the SDK to
    /// fetch this blob directly from the URL and skips sending the bytes
    /// down the WS. `None` = serve bytes inline (today's behavior).
    fn resolve_get_url(&self, _room_id: &str, _hash: &Hash) -> Option<String> { None }
    /// F6: direct-upload path. `Some(url)` tells the SDK to PUT bytes
    /// straight to this URL (presigned); server is notified of completion
    /// by the client via a new `blob-uploaded` wire message. `None` =
    /// client sends bytes over WS as today.
    fn resolve_put_url(&self, _room_id: &str, _hash: &Hash, _size: u64) -> Option<String> { None }
    fn is_durable(&self) -> bool { true }
}

pub trait ServerPersistence: NodePersistence + BlobPersistence {}
impl<T: NodePersistence + BlobPersistence + ?Sized> ServerPersistence for T {}
```

A blanket `impl<N: NodePersistence, B: BlobPersistence> ServerPersistence for
(N, B)` (or a `Composite<N, B>` struct) lets you write:

```rust
let store = Composite::new(MongoNodeStore::connect(...)?, S3BlobStore::new(...)?);
let rooms = Rooms::with_persistence(Arc::new(store));
```

`DirPersistence` continues to exist — it implements both halves. Existing
code that passes `Arc<dyn ServerPersistence>` is unchanged.

### New crate: `activesync-s3-blobs`

Workspace crate `s3-blobs/` publishing `activesync_s3_blobs::S3BlobStore`.

- **S3 client.** `object_store 0.11` (abstracts AWS, R2, MinIO, GCS,
  Azure behind one API). Avoids locking to the AWS SDK and lets SpeechSlate
  switch to R2 for egress cost without touching sync code.
- **Config.**

  ```rust
  pub struct S3BlobStoreConfig {
      pub endpoint: Option<String>,           // None = AWS; set for R2/MinIO
      pub bucket: String,
      pub region: String,
      pub path_prefix: String,                // e.g. "blobs/" — multi-tenant safe
      pub auth: S3Auth,
      pub presign_get_ttl: Duration,          // default 1 h
      pub presign_put_ttl: Duration,          // default 15 min
      pub direct_upload_threshold: u64,       // default 1 MiB
  }

  pub enum S3Auth {
      /// Server holds IAM creds (env vars, IAM role, shared config).
      Direct { /* object_store's credential chain handles this */ },
      /// Server asks the app's API for presigned URLs on demand.
      /// The app API is the credential holder; sync server never
      /// sees S3 keys.
      Delegate {
          presign_endpoint: String,           // e.g. "https://api.speechslate.app/blobs/presign"
          auth_header: HeaderValue,           // shared secret sync-server→app-api
          http_client: reqwest::Client,
      },
  }
  ```

- **`Direct` mode.** `resolve_get_url` signs a GET with `presign_get_ttl`
  and returns the URL. `resolve_put_url` signs a PUT if `size >=
  direct_upload_threshold`, else returns `None` (bytes ride WS).
  `persist_blob` writes via the S3 client (for the < threshold case,
  after server receives bytes). `load_room_blobs` is rarely called —
  blobs are pulled on demand via `resolve_get_url` — but when it *is*
  called (cold-start hydration), it streams from S3 with a concurrency
  cap (default 8).
- **`Delegate` mode.** `resolve_get_url` / `resolve_put_url` POST to
  `presign_endpoint` with `{room_id, hash, op: "get"|"put", size}` and
  expect `{url, expires_at}`. Sync server is a dumb proxy for URL
  minting; AWS creds stay in SpeechSlate's existing API. Timeouts,
  retries, circuit breaker on API outage → fall back to bytes-through-WS.

### Wire protocol additions

Three new messages, all backwards-compatible (clients that don't speak
them fall through to the existing bytes-inline path):

```jsonc
// Server → client. Replaces the binary blob frame when resolve_get_url is Some.
{ "type": "blob-redirect", "hash": "<hex>", "url": "https://...", "expires_at": 1714000000 }

// Client → server. Request a direct-upload URL. Server either grants it
// or replies "send via WS" and the client uses the existing path.
{ "type": "request-upload", "hash": "<hex>", "size": 52428800 }
{ "type": "upload-granted", "hash": "<hex>", "url": "https://...", "expires_at": ... }
{ "type": "upload-denied",  "hash": "<hex>", "reason": "use-ws" }

// Client → server. Notify that a presigned PUT completed. Server does a
// HEAD to verify the object exists and the size matches, then broadcasts
// blob-have to the room so peers can pull via resolve_get_url.
{ "type": "blob-uploaded", "hash": "<hex>" }
```

`request-upload` / `blob-uploaded` messages are guarded by a new
capability flag `supports_direct_blob_io` in `A7` capability negotiation
so old SDKs never see them.

### SDK changes (`web/sdk.js`)

- **Download path.** Existing `getBlob(hash)` gets one extra branch: if
  the server replies with `blob-redirect`, the SDK does `fetch(url)` and
  returns the bytes. Caller API unchanged. Retry on 403/expired → ask
  server for a fresh redirect.
- **Upload path.** `setBlob(bytes)` bifurcates on size:
  - `bytes.length < threshold` (server-advertised in `welcome`): send
    via WS as today.
  - `bytes.length >= threshold`: send `request-upload`; on
    `upload-granted`, `fetch(url, { method: 'PUT', body: bytes })`, then
    send `blob-uploaded`. On `upload-denied` or `request-upload` timeout,
    fall back to WS.
- **Progress callbacks.** New optional `onProgress` hook on `setBlob` /
  `getBlob` — uses `fetch` with a `ReadableStream` for direct transfers,
  no-op for WS transfers (SDK knows the size up front, can emit
  synthetic 0 → 100% events).
- **Caching.** SDK caches presigned URLs until 60 s before expiry —
  avoids round-tripping the server on every `getBlob` in a render loop.
- **API compat.** No breaking changes. `setBlob` / `getBlob` signatures
  are identical; the threshold + URL handling is internal.

### Tests

- `activesync-s3-blobs` unit tests against a **MinIO** container started
  by the test harness (docker-compose or testcontainers-rs). Covers
  both `Direct` and `Delegate` modes, GC sweep, presigned URL minting,
  threshold-based upload routing.
- Server integration test: two clients, one uploads a 10 MB blob, other
  downloads. Assert bytes never appear on either WS (fully off-loaded
  to MinIO). Mirror test with a 1 KB blob: assert bytes *do* appear on
  WS (below threshold).
- Wire-compat test: SDK with `supports_direct_blob_io=false` talking to
  a server with S3 backend. Should transparently fall back to bytes-WS.
- Trait split regression: existing `DirPersistence` tests
  (`room_survives_restart_with_nodes_and_blobs`, `blob_gc_*`) must pass
  unchanged.

### SpeechSlate integration sketch

```toml
# Cargo.toml of a SpeechSlate-specific server binary
[dependencies]
activesync-server  = { path = "..." }
activesync-s3-blobs = { path = "..." }
# future:
# activesync-mongo-store = { path = "..." }
```

```rust
// main.rs
let nodes = MongoNodeStore::connect(&env::var("MONGO_URI")?).await?;
let blobs = S3BlobStore::new(S3BlobStoreConfig {
    endpoint: Some(env::var("S3_ENDPOINT")?),   // R2
    bucket:   "speechslate-blobs".into(),
    region:   "auto".into(),
    path_prefix: "activesync/".into(),
    auth: S3Auth::Delegate {
        presign_endpoint: "https://api.speechslate.app/blobs/presign".into(),
        auth_header: HeaderValue::from_str(&env::var("SS_API_SHARED_SECRET")?)?,
        http_client: reqwest::Client::new(),
    },
    presign_get_ttl: Duration::from_secs(3600),
    presign_put_ttl: Duration::from_secs(900),
    direct_upload_threshold: 1024 * 1024,
})?;

let store = Composite::new(nodes, blobs);
let rooms = Rooms::with_persistence(Arc::new(store));
// ... rest of activesync-server bootstrap
```

### Non-goals for F6

- **Node storage in S3.** Thought about it; not worth it. Nodes are
  small, frequent, need indexed lookup by `(room_id, seq)`. S3 gives
  neither efficient indexed lookup nor affordable per-node IOPS.
  Mongo/Postgres is the right shape for nodes.
- **Server-to-server replication.** Multi-region deployments are a
  separate problem (room ownership, consensus, etc.). F6 assumes one
  sync server per region; S3 is shared.
- **Blob encryption at rest beyond S3's server-side encryption.**
  D1's op-level E2EE already covers the crypto path; blobs in an E2EE
  room are already encrypted by the client before `setBlob` sees them.

### Relationship to other phases

- **Depends on:** F4 (trait exists); A7 (capability negotiation for the
  new wire messages).
- **Unblocks:** SpeechSlate integration; any product that wants
  CDN-fronted blob delivery.
- **Adjacent:** F7 (future) — same split-trait treatment for nodes
  (`activesync-mongo-store`, `activesync-postgres-store`).

### Decision log additions (when F6 ships)

| Decision | Rationale |
|---|---|
| Split `ServerPersistence` into `NodePersistence + BlobPersistence` | Mongo + S3 is the single most common combo; a composite trait is the clean way to mix-and-match without per-backend boilerplate. Blanket impl for `(N, B)` keeps the public surface unchanged. |
| `object_store` crate over `aws-sdk-s3` | One dependency covers AWS, R2, MinIO, GCS, Azure. R2 egress cost is a real knob SpeechSlate will want to turn; lock-in would force a re-implementation. |
| Two auth modes (`Direct` / `Delegate`) | Self-hosters want simple IAM; SaaS integrators already have an auth API and want sync-server to not be another credential holder. Single code path cannot satisfy both. |
| Direct-upload threshold (SDK bifurcates on size) | Phones uploading 50 MB audio cannot afford the WS round-trip; 1 KB symbol icons cannot afford the presigned-URL round-trip. The threshold is the only way to be optimal for both. |
| Capability flag gates new wire messages | F6 must not break pre-F6 SDK + post-F6 server combinations. Zero-behavior-change fall-through is a hard constraint. |

### Implementation locations

- `core/src/capabilities.rs` — `supports_direct_blob_io: bool` (default `true`, intersect on negotiate).
- `server/src/store.rs` — trait split into `NodePersistence` + `BlobPersistence`; `ServerPersistence` is now a supertrait with a blanket impl. `Composite<N, B>` lets callers mix-and-match. `PresignedUrl { url, expires_at_unix }` with `with_ttl(url, Duration)` helper. Sub-traits use `nodes_durable()` / `blobs_durable()` to avoid method-name collision; supertrait's `is_durable()` defaults to AND.
- `s3-blobs/` — new crate `activesync-s3-blobs`. `S3BlobStore` implements `BlobPersistence`. Two auth modes: `S3Auth::Direct { access_key_id, secret_access_key, session_token }` (uses `object_store::aws::AmazonS3` + `Signer::signed_url`) and `S3Auth::Delegate { presign_endpoint, auth_header }` (POSTs `{op,room_id,hash,size,ttl_seconds}` to the integrating app and trusts its returned URL). Sync→async bridge via owned `Arc<tokio::runtime::Runtime>` per store so callers from inside the server's main runtime don't block-on the same runtime. Object key layout: `{path_prefix}{sanitized_room}/{hash_hex}` with non-`[A-Za-z0-9_-]` chars escaped as `_XX`.
- `server/src/ws_handler.rs` — wire handlers for `blob-redirect`, `request-upload`, `upload-granted`, `upload-denied`, `upload-rejected`, `blob-uploaded`. Each new path is gated on `negotiated_caps.supports_direct_blob_io`; falsy ⇒ legacy `blob-pack` / `blob-upload` path.
- `web/sdk.js` — `sendBlobs` bifurcates on `DIRECT_UPLOAD_THRESHOLD = 1 MiB`; below that or when capability is off, uses legacy WS upload. `directUpload(hash, bytes)` requests a PUT URL, fetches it, then sends `blob-uploaded`. `fetchBlobViaUrl(hash, url)` handles `blob-redirect` payloads with 403/expired retry that falls back to WS. Cache map `blobUrlCache` reuses GET URLs; on 403 the entry is evicted and the caller falls back.
- `s3-blobs/tests/minio_round_trip.rs` — live MinIO integration test (Docker required, gracefully skips if Docker is unavailable). Spins up `minio/minio:latest`, creates the bucket via `aws-sdk-s3` (dev-dep only), then exercises `resolve_put_url` → HTTP PUT → `verify_uploaded` → `resolve_get_url` → HTTP GET → byte-equality assertion → `blob_gc_sweep` → post-GC HEAD-fails assertion.

---

## F7 — Database node-store adapters (SHIPPED)

> **Status.** Implemented end-to-end. `MongoNodeStore` (mongodb 3.x async
> driver) and `PostgresNodeStore` (sqlx 0.8) ship as separate crates under
> `node-stores/`, both pass the shared conformance suite live against
> `mongo:7` and `postgres:16` containers.

### Problem

`DirPersistence` stores DAG nodes in a local SQLite file. Fine for self-host,
wrong for production SaaS:
- Can't share state across horizontally scaled sync-server replicas.
- Operators already run Mongo/Postgres and don't want a second durable store.
- Backups, monitoring, and access controls are already wired for the primary DB.

F7 ships two first-party `NodePersistence` adapters plus documentation for
rolling your own.

### Crates

Two workspace crates, same shape:

```
node-stores/
  mongo/     → activesync-mongo-store    (publishes MongoNodeStore)
  postgres/  → activesync-postgres-store (publishes PostgresNodeStore)
```

Both are thin. Nodes are opaque bytes keyed by `(room_id, node_id)`; the
adapter just needs to index by room + preserve insertion order. Neither
touches `StateGraph`, `Op`, or the wire format — they're byte-level stores
under the `NodePersistence` trait (see F6).

### MongoNodeStore

- **Driver.** `mongodb 3.x` async Rust driver.
- **Collection.** `activesync_nodes` (configurable).
- **Document shape:**

  ```jsonc
  {
    "_id":     "<room_id>:<node_id_hex>",   // compound primary key
    "room_id": "<room_id>",                 // indexed
    "node_id": <BinData 32>,
    "seq":     <auto-increment via ordered insert>,
    "bytes":   <BinData postcard-packed SyncNode>,
    "created_at": <ISODate>
  }
  ```

- **Indexes.** `{ room_id: 1, seq: 1 }` for ordered hydration. Compound `_id`
  makes re-inserts idempotent (duplicate key = drop, matches
  `INSERT OR IGNORE` semantics from DirPersistence).
- **Seq.** Mongo doesn't have auto-increment. Use a dedicated counter
  collection `activesync_seq { room_id, next }` with `$inc` on every batch —
  one round trip per pack, not per node. Alternatively use ObjectId timestamps
  (monotonic per room) and sort on those.
- **Batching.** `persist_nodes` uses `insert_many` with `ordered: false`
  so duplicate keys inside a batch don't kill the whole write.
- **Config.**

  ```rust
  pub struct MongoNodeStoreConfig {
      pub connection_uri: String,
      pub database:       String,
      pub collection:     String,   // default "activesync_nodes"
      pub seq_collection: String,   // default "activesync_seq"
      pub write_concern:  WriteConcern,  // default Majority
      pub read_concern:   ReadConcern,   // default Local (hydrate)
  }
  ```

### PostgresNodeStore

- **Driver.** `sqlx 0.8` with `postgres` feature, compile-time checked queries.
- **Schema:**

  ```sql
  CREATE TABLE activesync_nodes (
    room_id    TEXT NOT NULL,
    node_id    BYTEA NOT NULL,
    seq        BIGSERIAL PRIMARY KEY,
    bytes      BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(room_id, node_id)
  );
  CREATE INDEX idx_activesync_nodes_room_seq ON activesync_nodes(room_id, seq);
  ```

- **Upserts.** `INSERT ... ON CONFLICT (room_id, node_id) DO NOTHING`.
- **Batching.** `COPY ... FROM STDIN` via `sqlx::copy_in_raw` for `persist_nodes`
  batches over ~16 nodes; single `INSERT` below that. `COPY` beats multi-row
  insert by 5–10× on real hardware.
- **Connection pool.** `sqlx::PgPool` with operator-configurable max
  connections. Default 20.
- **Migrations.** Schema lives in the crate as a `sqlx::migrate!()` macro
  pointing at `migrations/`; applied on startup via
  `MIGRATOR.run(&pool).await`.

### Shared concerns

- **Error policy.** Transient errors (network, pool exhaustion) → retry with
  jitter, log at `warn`. Permanent errors (schema mismatch, auth failure)
  → `tracing::error!` and mark the backend as failed; sync-server continues
  running on in-memory cache but logs that persistence is degraded. Same
  policy `DirPersistence` uses today.
- **Observability.** Both adapters re-use the existing
  `activesync_persistence_write_seconds{kind=node|nodes_batch}` histogram
  (G7). Adds one label: `backend=mongo|postgres|dir`.
- **Hydration order.** Nodes returned by `load_room_nodes` **must** be in
  insertion order so the DAG's parent edges resolve. Both backends order
  by `seq` ascending. Tests verify this under concurrent writers.
- **Blob-side pairing.** F7 doesn't change blob storage. A typical
  SpeechSlate server is `Composite::new(MongoNodeStore, S3BlobStore)`;
  a Postgres shop is `Composite::new(PostgresNodeStore, S3BlobStore)`.
  `DirPersistence` stays as the self-host default.

### Tests

- Per-adapter unit tests against a real DB container (testcontainers-rs
  spins up Mongo / Postgres; CI runs both).
- Shared conformance test suite — same scenarios that exercise
  `DirPersistence` today, parameterized over the adapter. Catches accidental
  behavioral drift.
- Concurrency test: two sync-server processes sharing one DB, both writing
  to the same room. Covers the multi-replica scenario that SQLite flatly
  can't do.
- Regression: 100% of existing `DirPersistence` tests stay green.

### Non-goals for F7

- **Cross-region replication.** Both Mongo and Postgres solve this at the
  DB layer (replica sets, logical replication). F7 doesn't try to add
  sync-server-level replication — the DB is the source of truth.
- **Read-your-writes across sync-server replicas.** That's a sticky-session
  or leader-election problem at the load balancer. F7 assumes room-to-server
  affinity (consistent hashing on `room_id` → replica) as the deployment
  pattern.
- **SQLite driver replacement.** `DirPersistence` keeps rusqlite. Different
  adapter, different lifecycle.

### Decision log additions (when F7 ships)

| Decision | Rationale |
|---|---|
| `mongodb` driver over raw BSON | Stable 3.x API, async-native, owned by MongoDB Inc. Alternatives (mongo-rust-driver forks) have lagged on async. |
| `sqlx` over `tokio-postgres` / `diesel` | Compile-time checked queries, async-native, migrations built in, no runtime schema drift. Diesel's sync API would block the tokio reactor. |
| `BYTEA` node storage, not `JSONB` | Nodes are postcard bytes. Decoding to JSONB would force a re-encode on every read, double storage, and lose signature bit-for-bit fidelity. Opaque bytes is the right shape. |
| Compound primary key `(room_id, node_id)` | Idempotent inserts without a round trip. Makes at-least-once delivery safe (duplicate sends just no-op). |
| Separate sequence counter on Mongo | Mongo has no native autoincrement. A single counter doc per room is cheap (`$inc` is O(1)) and keeps hydration order deterministic. Alternative (ObjectId timestamps) leaks server clock into the protocol. |

### Implementation locations

- `node-stores/postgres/` — `activesync-postgres-store`. `PostgresNodeStore::connect_and_migrate(cfg)` builds a `sqlx::PgPool`, applies embedded migrations from `migrations/`. `persist_node` uses `INSERT ... ON CONFLICT (room_id, node_id) DO NOTHING`; `persist_nodes` uses `UNNEST($1::text[], $2::bytea[], $3::bytea[])` for one-round-trip batch inserts. Sync→async via owned `Arc<tokio::runtime::Runtime>` (same pattern as `S3BlobStore`).
- `node-stores/mongo/` — `activesync-mongo-store`. `MongoNodeStore::connect(cfg)` parses URI, creates the `(room_id, seq)` compound index. Sequence allocation: `find_one_and_update` with `$inc` on a per-room counter doc in `activesync_seq` (one round trip per batch, not per node). `persist_nodes` uses `insert_many(... ).ordered(false)` so duplicate-key entries inside a batch don't poison the rest; `E11000` is treated as success (idempotent re-insert). Uses mongodb's vendored bson (`mongodb::bson`) — never add `bson` as a separate workspace dep, the namespace re-export collides.
- `node-stores/conformance/` — `activesync-nodestore-conformance`. Shared `pub fn run_all(p: &dyn NodePersistence, room: &str)` covering: `nodes_durable() == true`, empty-room load, single-node round-trip, idempotent duplicate persist, batch round-trip + idempotency, per-room isolation, hydration order matches insertion. `tests/dir_persistence.rs` runs the suite against the existing `DirPersistence` to prove the suite itself is correct.
- `node-stores/mongo/tests/mongo_round_trip.rs`, `node-stores/postgres/tests/postgres_round_trip.rs` — live testcontainers integration tests; spin up `mongo:7` / `postgres:16`, connect with retries, then call `run_all`. Skip gracefully if Docker is unavailable.
- Composition: callers wire `Composite::new(PostgresNodeStore::connect_and_migrate(...)?, S3BlobStore::new(...)?)` (or the Mongo equivalent) and pass the result to `Rooms::with_persistence(Arc::new(...))`. The blanket `impl<N+B> ServerPersistence` from F6 means no per-combination boilerplate.

---

## F8 — List CRDT with move semantics (SHIPPED)

> **Status.** Shipped. Core: `core/src/list.rs` with `FracIdx` fractional
> indexing, `Op::List { Insert, Move, Delete }`, and `resolve_list_seq` —
> 23 tests green (fractional-index + multi-peer convergence + gesture
> sequences). Bridge: WASM exports `list_insert_at` / `list_move_to` /
> `list_delete` / `list_resolve_json` / `list_length` / `list_ids_json`.
> SDK: `doc.list<T>(key)` → `ListHandle<T>` with `push`, `insert`,
> `insertAfter/Before`, `move`, `delete`, `update`, `toArray`, `onChange`,
> `onReorder`, plus `list.gestures.{dropOnto, dropBetween, swap,
> dropBefore, dropAfter}`. Demo: drag-to-reorder + drop-onto-to-replace
> wired into `web/demo.js`. Capability flag `supports_list_crdt` defaults
> true; old SDKs ignore List ops cleanly.

> **Target.** Shipping data model for ordered collections with concurrent
> editors (SpeechSlate buttons, boards, playlists, kanban columns). Unblocks
> any product where "this thing is a list" is the natural shape and multiple
> users may reorder at the same time. Promotes the long-deferred "post-F
> List CRDT" item into an actual plan.
>
> **Depends on:** nothing (self-contained core + SDK work). Runs in parallel
> with F6 and F7.

### Problem

When this work was proposed, `doc.list()` threw. Apps modeled ordered collections as:
1. Map with an app-level `sortKey` field — works under single-editor, silently
   corrupts under concurrent reorder (two clients computing the same
   "between A and B" key collide).
2. Whole-JSON overwrite on every change — loses concurrent content edits.
3. Text — insane (but technically merges).

For SpeechSlate: a board is a list of buttons. Drag-to-reorder is the
primary editing gesture. A clinic team editing a shared board must not
lose each other's moves, and a user editing a button's label while someone
else drags it must not lose the label edit. None of the three workarounds
handles that.

### Design: fractional-index List + stable item IDs + orthogonal content

The element in the list is **just a reference + position**:

```rust
pub enum Op {
    Map(MapOp),
    Text(TextOp),
    List(ListOp),   // NEW
}

pub enum ListOp {
    /// Insert `item_id` at `position`. Duplicate (same `item_id`) is ignored.
    /// `item_id` is UUIDv4, generated client-side.
    Insert { list_key: String, item_id: ItemId, position: FracIdx },
    /// Tombstone an item. Further Move/Insert ops targeting the same id
    /// are ignored (delete wins).
    Delete { list_key: String, item_id: ItemId },
    /// Reposition an existing item. LWW on (lamport, pubkey) per item_id
    /// resolves concurrent moves.
    Move   { list_key: String, item_id: ItemId, position: FracIdx },
}

pub struct ItemId(pub [u8; 16]);         // UUIDv4
pub struct FracIdx(pub String);          // lexicographic fractional index
```

Item **content** lives in a sidecar Map. The SDK composes them:

```js
const buttons = doc.list('boards/abc/buttons');    // ordered list of item ids
const items   = doc.map('buttons');                // keyed content store

// Push a new button
const id = buttons.push({ label: 'Hello', imageId: 'img-42' });
// Under the hood:
//   item id uuid-xyz
//   items.set('uuid-xyz', { label, imageId })
//   buttons op: Insert { item_id: uuid-xyz, position: <after last> }

buttons.move(id, 0);                                // reorder to front
items.set(id, { label: 'Hola', imageId: 'img-42' }); // edit content, orthogonal

buttons.toArray();                                   // [{ id, content }, …]
buttons.onChange(ev => …);                           // fires on order + content
```

**Why this shape:**
- **Move is a position update, not delete+insert.** Concurrent "Alice moves X"
  + "Bob edits X" compose cleanly — different ops touching different state.
- **Concurrent moves resolve deterministically** via the same LWW rule Map
  uses (`lamport desc, pubkey desc`). No new tie-break protocol.
- **Content and order are separable.** Apps can subscribe to just the list
  (fires on reorder) or just the content (fires on label edit) or both.
- **No interleaving hazards.** Fractional indices don't have text's
  character-level concurrency problem because each item has a stable
  UUID — concurrent insertions at "the same spot" get different UUIDs
  and stable, if arbitrary, order.

### Fractional indexing

- Algorithm: `fractional-indexing`-style base-62 strings. `"a"` < `"b"`,
  insert between `"a"` and `"b"` = `"am"`, etc. Well-known, two small
  published references (npm `fractional-indexing`, crates.io several).
- Pure function, no coordination. Client generates the position locally
  from the neighbors' positions.
- **Rebalance.** Worst-case adversarial inserts grow positions unboundedly
  (`"a"` → `"am"` → `"amm"` → ...). In practice 32 chars handles millions
  of inserts. When a position exceeds `REBALANCE_THRESHOLD = 128` chars,
  emit a "rebalance" op that rewrites every position in the list to evenly
  spaced values. Rebalance is a standard op set; merges like any other.
  Server can drive rebalance in authoritative rooms (E1) or any peer can
  do it in cooperative rooms.
- **Tie-break on identical positions.** Two clients concurrently inserting
  at "end of list" compute the same position independently. Tie-break on
  `item_id` (UUIDv4 is unique → deterministic).

### Move semantics

```rust
// apply_remote semantics for Move:
match existing_position_for(item_id) {
    None => ignore,                 // item deleted or not yet seen — drop
    Some(existing) if node.lamport > existing.lamport
        || (node.lamport == existing.lamport && node.author > existing.author)
        => update position,
    _ => ignore,                    // older move, LWW loses
}
```

Concurrent Alice-moves-X-to-A and Bob-moves-X-to-B:
- Both peers eventually see both ops.
- LWW on `(lamport, pubkey)` picks one winner deterministically.
- Both converge to the same final position. No user intervention.

Concurrent Alice-deletes-X and Bob-moves-X:
- Delete wins (tombstone is absorbing). Bob's move is no-op on apply.
- Matches user intuition: "X was removed, where it was moving to doesn't matter."

### Wire / protocol

- `ListOp` is a new `Op` enum variant. Existing messages carry it in
  `Transaction::ops` unchanged. Canonical hash covers it via postcard.
- A7 capability flag `supports_list_crdt: bool`. Peers without it see
  `Op::List` ops as unknown and drop them (same behavior as any future
  op — postcard decode fails, node is dropped with a `warn` log). This
  is acceptable because lists are additive: old peers don't get list
  semantics, but they also can't corrupt new peers.
- F3 subscription filtering: list ops match against `list_key` the same
  way Map ops match against their key. Zero special-case code.
- E1 Policy: `can_write` rules apply to `list_key`. SpeechSlate can lock
  `boards/*/buttons` to the board owner's pubkey.

### SDK (`web/sdk.js`)

- **`doc.list(key)`** returns a `ListHandle` instead of throwing.
- **`ListHandle` API:**

  | Method | Semantics |
  |---|---|
  | `push(content)` | Insert at end, return new item id. |
  | `insert(index, content)` | Insert at numeric index. |
  | `insertAfter(afterId, content)` | Insert after a known id. |
  | `move(id, newIndex)` | Reposition. |
  | `delete(id)` | Tombstone. |
  | `update(id, content)` | Update sidecar Map content. |
  | `get(id)` | `{ id, content, position }`. |
  | `toArray()` | `[{ id, content }, …]` in current order. |
  | `length` | Count excluding tombstones. |
  | `onChange(cb)` | Fires on insert/delete/move/content update. |
  | `onReorder(cb)` | Fires only on order changes, not content. |

- **Content sidecar.** SDK manages the `items` sidecar Map
  automatically — `list.push({label: 'x'})` writes content to
  `<listKey>:items` and the list entry references the item id. Caller
  never sees the composition.
- **Cursor-friendly.** `ListHandle.cursor(id)` returns a stable cursor
  that survives reorders, for UI libraries that track "the item the
  user right-clicked on."

### Gesture helpers (ships with F8)

Drag-and-drop gestures decompose into multi-op sequences. The CRDT
guarantees convergence on any sequence, but the app has to decide *which*
sequence a given gesture emits. Bare LWW on replace-on-drop is bad UX
(the loser's button silently vanishes). The SDK ships a small
`list.gestures` namespace with composed helpers so apps get sensible
concurrent behavior for free:

| Helper | Behavior |
|---|---|
| `list.gestures.dropOnto(draggedId, targetId)` | If `targetId` is still present locally → `delete(targetId)` + `move(draggedId, targetPosition)` (replace semantics). If already tombstoned → `insertAt(draggedId, lastKnownIndexOf(targetId))` (insert-and-shift). Under a concurrent race where Alice and Bob both drop onto the same target, both deletes are idempotent and the two moves land at tied fractional positions — LWW + `item_id` tie-break places them adjacent, so **neither drop vanishes**. |
| `list.gestures.dropBetween(draggedId, beforeId, afterId)` | Pure `move(draggedId, fracIdxBetween(before, after))`. Standard drag-between-two-neighbors. |
| `list.gestures.swap(aId, bId)` | Two `move` ops exchanging positions. Concurrent swaps of overlapping pairs resolve via LWW; identity is preserved. |
| `list.gestures.dropBefore(draggedId, targetId)` / `dropAfter(…)` | `move(draggedId, fracIdxBefore(target))` / `fracIdxAfter(target)`. |

**Why these live in the SDK, not `Op::List`:** gestures are
app-layer intent. Keeping the core CRDT to three ops (`Insert`, `Delete`,
`Move`) means any app can build its own gesture vocabulary; the helpers
are just the good defaults. Apps that prefer "replace wins, loser
vanishes" can skip them and call the primitives directly.

**Tests.** Each helper has a two-peer property test: both peers issue
the same gesture concurrently against the same target(s), assert
deterministic convergence and that **no primary dragged item is lost**
(tombstoned targets are expected to be gone; dragged items are not).

### Tests

- **Property: convergence.** Random sequences of insert/delete/move/update
  applied in different orders across peers always converge to the same
  array, same content.
- **Property: move preserves identity.** After N concurrent moves of the
  same item, every peer sees the same single item (not duplicated, not
  deleted).
- **Property: delete is absorbing.** After delete, later concurrent
  inserts/moves with the same id have no effect on any peer.
- **Fractional rebalance.** Force 10k sequential "insert at position 0"
  ops, assert rebalance triggers ≤2× and final positions are bounded.
- **SDK integration.** Two browser docs, drag-to-reorder 100 items
  concurrently, both converge within one round trip.
- **Wire compat.** v1 SDK (pre-F8) joining a room with List ops ignores
  them cleanly; v2 SDK sees the full list. Zero crashes.

### Non-goals for F8

- **Rich-text spans inside list items.** Items are `Vec<u8>` map values —
  the app's content shape. If an item needs collaborative text, the app
  layer composes `doc.text(itemKey)` alongside. Keeps the list primitive
  small.
- **Nested lists.** Lists-of-lists work by convention (a list item's
  content is a Map whose value is another list key). Engine doesn't need
  to know.
- **Undo/redo.** Requires an undo log layer above the CRDT; separate
  problem, separate feature (post-F8).

### SpeechSlate integration sketch

```js
// Board = content + ordered list of button refs
const board    = doc.map('boards').get(boardId);
const buttons  = doc.list(`boards/${boardId}/buttons`);
const contents = doc.map('buttons');

// Add a button
const id = buttons.push({ label: 'Yes', imageHash: await assets.setBlob(fileBytes) });

// Reorder (drag-and-drop)
buttons.move(id, targetIndex);

// Edit label while someone else drags — no lost edits
contents.set(id, { ...contents.get(id), label: 'Yeah' });

// Render in order
buttons.toArray().forEach(({ id, content }) => drawButton(id, content));
```

### Decision log additions (when F8 ships)

| Decision | Rationale |
|---|---|
| Fractional-index list, not RGA | RGA is for per-character text where every element has content. List elements have stable UUIDs and the content lives in a sidecar Map — that's a different problem shape. Fractional indexing gives clean move semantics; RGA does not. |
| Move = position update, not delete+insert | delete+insert loses concurrent content edits. Orthogonal position + content ops are the only way to make concurrent "Alice moves X, Bob edits X" work. |
| Item content in a sidecar Map, not inline in List ops | Keeps the list primitive small, lets content use Map's full LWW semantics, enables content updates without writing a list op. SDK composes them so the app API looks unified. |
| UUIDv4 item ids, not OpId-derived | OpId is tied to the creating node; an item that's been moved 50 times would have 50 OpIds for the same logical item. UUIDs are stable for the lifetime of the item. |
| Rebalance as a standard op | A room never gets into an unfixable state. Any peer (or the Super-Peer) can rebalance; the op merges like any other. Alternative ("rebalance is an offline admin tool") loses the invariant that the DAG fully describes state. |
| Capability flag gates List ops | Old SDKs must not crash on new op variants. A7's model already handles this; `supports_list_crdt` is the next flag. |

---

## F4 follow-up — Room eviction on idle (SHIPPED)

Long-running deployments accumulate per-room state in memory (the
hydrated `StateGraph` + `MemoryBlobStore` + broadcast channel + tick-loop
slot) for every room that was ever joined, even after all peers leave.
With persistence on, this in-memory state is pure cache — we can drop it
and rehydrate on next join. With persistence off, dropping it is data
loss, so eviction must be gated.

**What shipped.** `--idle-timeout <seconds>` CLI flag, default 300 s
(5 min); `0` disables. A 60 s background sweeper
(`room::spawn_idle_sweeper`) checks every durable room, evicts those
with zero connected peers whose idle clock has exceeded the timeout, and
logs `room=<id> evicted idle room`. On the next `get_or_create` the room
is rebuilt fresh from `DirPersistence` — identical path to a server
restart, which F4's `room_survives_restart_with_nodes_and_blobs` test
already exercises.

**Model.** Idle means *zero connected peers*, not *no recent activity*.
A room with one silent but connected peer stays in memory. The clock
starts the moment `connected_peers` drops to empty (`deregister_peer`
stamps `Instant::now()`) and clears the moment anyone rejoins
(`register_peer` → `idle_since = None`).

**Safety guards** (all three required before eviction):
1. `ServerPersistence::is_durable()` — `NoPersistence` returns `false`,
   `DirPersistence` returns `true`. In-memory builds with
   `--idle-timeout` set log a warning at startup and the sweeper is
   never spawned.
2. `Arc::strong_count(&room) == 1` — only the registry holds a
   reference. Prevents evicting a room mid-handshake where a handler
   has cloned the Arc but not yet called `register_peer`.
3. `connected_peers.read().await.is_empty()` re-checked under the peers
   lock immediately before removal, closing the
   register-between-decision-and-removal race.

On eviction the room's tick loop is aborted via `stop_tick()`; the
broadcast sender is dropped (any lagging subscriber gets
`RecvError::Closed`, which clients already handle as a reconnect
trigger).

**Tests.** `server/tests/idle_eviction.rs`:
- `durable_idle_room_is_evicted_and_rehydrates` — full round-trip:
  write a node, disconnect, sweep, rejoin, state is still there.
- `in_memory_rooms_are_never_evicted` — `NoPersistence` + timeout-zero
  sweep returns empty.
- `connected_room_is_not_evicted` — sweep with a live peer registered
  (or with a caller-held extra Arc) returns empty; after deregister +
  drop, sweep evicts.

**Not shipped / deferred.** No per-room override (every room shares the
global timeout); no metrics counter (add under a `metrics` feature if we
ever ship one); no graceful drain on SIGTERM (tokio runtime drop
suffices). Sweeper interval is a const rather than a flag — 60 s is
fine-grained enough for a 5-minute default and coarse enough to cost
nothing.


---

## Phase G — Operational gaps (in progress)

### G7 — Metrics / observability (SHIPPED, Apr 2026)

**Problem.** Pre-G7 the server emitted only 	racing logs. No counters, no
histograms, no scrape endpoint — you couldn't answer "is the merge queue
falling behind?" without attaching a profiler.

**What shipped.**
- metrics 0.23 + metrics-exporter-prometheus 0.15 (`default-features = false`,
  `features = ["http-listener"]`) in Activesync-server.
- `server/src/metrics.rs`: `init(addr)` installs the global Prometheus
  recorder + HTTP listener; `parse_arg(&args)` reads
  `--metrics-addr <ip:port>` (and `--metrics-addr=…`); `peer_label(hex)`
  returns the 12-char pubkey prefix used by G3.
- Admin port is a separate listener — the public WS port is never
  mixed with `/metrics`. Default off; install failure is logged and the
  server keeps running without observability.
- Custom histogram buckets via `PrometheusBuilder::set_buckets_for_metric`
  so p50/p95/p99 queries come out clean:
  - `activesync_merge_batch_seconds`: 50µs, 100µs, 250µs, 500µs, 1ms, 2.5ms, 5ms, 10ms, 25ms, 50ms, 100ms, 250ms, 500ms, 1s, 2.5s.
  - `activesync_persistence_write_seconds`: 100µs, 250µs, 500µs, 1ms, 2.5ms, 5ms, 10ms, 25ms, 50ms, 100ms, 250ms, 500ms.

**Baseline metrics.**

| Metric | Kind | Labels | Source |
|---|---|---|---|
| `activesync_rooms_total` | gauge | — | `Rooms::get_or_create` + `sweep_idle` |
| `activesync_peers_total` | gauge | `room` | `Room::register_peer` / `deregister_peer` |
| `activesync_nodes_accepted_total` | counter | `room` | `import_nodes` |
| `activesync_merge_batch_seconds` | histogram | — | `import_nodes` (wall time) |
| `activesync_persistence_write_seconds` | histogram | `kind=node` \| `nodes_batch` \| `blob` | `DirPersistence` |
| `activesync_eviction_total` | counter | — | `Rooms::sweep_idle` |

Gap-specific counters (`activesync_broadcast_lagged_total`,
`_ws_send_timeout_total`, `_rate_limit_drops_total`, `_blob_gc_deleted_total`,
`_lamport_rejected_total`, `_token_expired_disconnects_total`) are
registered by G1/G3/G4/G5/G6 at their instrumentation sites — documented
in `server/src/metrics.rs` doc-comments so they stay discoverable.

**Tests.** `server/tests/metrics_endpoint.rs`:
- `metrics_endpoint_exposes_baseline_series` — installs the recorder on
  loopback, drives a room + peer + import, scrapes `/metrics` via a raw
  HTTP/1.1 GET on a `spawn_blocking` task, asserts the baseline metric
  names + `# HELP` lines appear.
- `parse_arg_accepts_flag_and_equals_form` — both CLI forms + missing +
  malformed cases.
- `peer_label_truncates_to_12_chars` — G3 label invariant.

**Constraints.** `metrics::init` installs a *process-global* recorder; a
second install returns `Err`. All metrics integration coverage stays in
`metrics_endpoint.rs` so no two tests race for the slot.

**Next (Phase G).** G3 — rate limit (governor token buckets,
`--peer-rate-nodes` / `--peer-rate-bytes`, 4008 close).

### G1 — Backpressure & slow-client policy (SHIPPED, Apr 2026)

**Problem.** Per-room `broadcast::channel(512)` silently swallowed
`RecvError::Lagged`, so a slow consumer kept receiving fresh packs while
missing the dropped ones — divergence with no signal until the next
reconnect. `sink.send()` had no timeout, so a stalled TCP write could
wedge a WS task indefinitely.

**What shipped.**
- `ws_send` helper in `server/src/ws_handler.rs` wraps every
  application-level send in `tokio::time::timeout(5s, …)`. On timeout it
  bumps `activesync_ws_send_timeout_total{room}`, emits a
  `1011 server overload` close (bounded by a 1s send timeout), and
  drops the peer.
- Lagged arm now closes with custom code `4001 resync required` and
  bumps `activesync_broadcast_lagged_total{room}`. The SDK's existing
  exp-backoff reconnect runs the normal recovery (hello → IBF → catch-up).
- `--broadcast-capacity <N>` CLI (default `512`, `0` rejected) plumbed
  through `Rooms::new` → `Room::new`. Tradeoff: larger = more slack for
  brief stalls, smaller = faster divergence detection.
- Both counters registered via `describe_counter!` in
  `server/src/metrics.rs`.

**Tests.** `server/tests/backpressure_lagged.rs` boots a real axum server
on an ephemeral loopback port with `broadcast_capacity=2`, connects a
tungstenite client, drains the welcome, then floods 5 000 synchronous
`room.tx.send(...)` calls with no `.await` between them so the handler
task stays parked while the size-2 ring buffer overflows. Asserts a
`Close { code: 4001 }` arrives in ≤ 5 s and `connected_peers` drains
in ≤ 2 s. Deterministic because `broadcast::Sender::send` is
non-yielding. Full server suite: 24/24 pass.

**Next (Phase G).** G3 — rate limit (governor token buckets,
`--peer-rate-nodes` / `--peer-rate-bytes`, 4008 close).

### G3 — Rate limiting per peer (SHIPPED, Apr 2026)

**Problem.** A peer past the handshake could flood signed packs and burn
Ed25519 verify cycles indefinitely. No ingress cap.

**What shipped.**
- `governor` 0.7 direct rate limiters, per WS session, one for
  nodes/sec and one for decoded-pack bytes/sec. Both checked *before*
  `import_nodes` so rejected floods never hit Ed25519 batch verify.
- CLI: `--peer-rate-nodes <N>` (default 200; 0 disables) and
  `--peer-rate-bytes <MiB>` (default 4; 0 disables) — threaded via
  `Rooms` into every session.
- Violation → WS close `4008 rate limit exceeded` (bounded 1 s send
  timeout) + `activesync_rate_limit_drops_total{peer=<12-char-prefix>}`.
  Both `NotUntil` (rate exceeded) and `InsufficientCapacity` (single
  pack > 1 s burst) count as violations.
- Super-Peer server key exempt: no limiter when peer pubkey matches
  `rooms.server_key`.
- `describe_counter!` registration in `server/src/metrics.rs`.

**Tests.** `server/tests/rate_limit.rs::oversized_pack_closes_with_4008`
— real axum server on loopback with `peer_rate_nodes=2`, handshake, one
pack of 5 signed nodes (`check_n(5)` → `InsufficientCapacity`), assert
`Close{4008}` within 5 s. Runs in ~270 ms. Fast-path suite
(lib + idle + metrics + backpressure + rate-limit): 23/23 green.

**Next (Phase G).** G4 — blob GC (`--blob-gc-interval`, two-phase sweep
with tombstone grace, `activesync_blob_gc_deleted_total`).

### G4 — Blob GC (SHIPPED, Apr 2026)

**Problem.** `DirPersistence` wrote `blobs/<room>/<hash>` forever. Every
`Op::Map::SetBlob` overwrite leaked the old file. No refcount, no sweep.
Disk grew monotonically for the lifetime of the room.

**What shipped.**
- `ServerPersistence::blob_gc_sweep(room, live, grace) -> usize` trait
  method with a no-op default (non-durable backends always return 0).
- `DirPersistence::blob_gc_sweep` — two-phase tombstone protocol:
  - Tombstones at `<root>/blob-tombstones/<sanitized_room>/<hash>` —
    **sibling** of `blobs/`, so F4 hydration (`load_room_blobs`)
    doesn't need to filter them out.
  - Phase 1: orphan (not in `live`) without a tombstone → write empty
    tombstone file (mtime=now), leave blob alone. Live blob with a
    stale tombstone → clear tombstone.
  - Phase 2 (or later): orphan whose tombstone mtime is older than
    `grace` → delete blob + tombstone, increment counter.
  - `grace = Duration::ZERO` collapses both phases — useful for tests
    and aggressive-GC operators.
- Live set = union of `Op::Map::SetBlob { blob_hash }` across **every**
  node in the DAG (not just `resolve()` winners), via
  `room::collect_live_blob_hashes`. Guarantees peers catching up from
  far behind still find historical blobs.
- `Rooms::sweep_blobs(grace)` snapshots the hot-room list and iterates;
  cold-on-disk rooms wait until they hydrate, same policy as the idle
  sweeper.
- `room::spawn_blob_gc_sweeper(rooms, interval, grace)` mirrors
  `spawn_idle_sweeper` (`MissedTickBehavior::Skip`, skip t=0).
- CLI: `--blob-gc-interval <secs>` (default 0 = disabled) and
  `--blob-gc-grace <secs>` (default 86400 = 24 h). Durable-only: warn
  + skip spawn when persistence is in-memory. Shared `parse_u64_flag`
  helper added to `main.rs`.
- Metric: `activesync_blob_gc_deleted_total{room}` (counter) registered
  in `server/src/metrics.rs`.

**Tests.** `server/tests/blob_gc.rs` — 3/3 green in ~130 ms, real
`DirPersistence` in a tmpdir:
- `blob_gc_two_phase_deletes_orphans_only` — two blobs persisted, one
  referenced; first sweep returns 0 (tombstone only); after 50 ms grace
  second sweep returns 1; third sweep 0.
- `blob_gc_clears_tombstone_when_blob_becomes_live_again` — tombstone
  gets cleared when `SetBlob` re-references the blob; `grace=0` sweep
  does not delete because live set wins.
- `blob_gc_is_noop_on_in_memory_persistence` — `NoPersistence` always
  returns 0.

Full server test suite after G4: 3/3 lib + 2/2 persistence + 1/1
rate-limit + 3/3 blob-gc + (backpressure + metrics + idle) all green.

**Next (Phase G).** G5 — Lamport ceiling / wall-clock sanity
(`LAMPORT_SLACK = 1<<20`, 24 h wall skew soft-reject,
`activesync_lamport_rejected_total{reason}`).

### G5 — Lamport ceiling & wall-clock sanity (SHIPPED, Apr 2026)

**Problem.** `apply_remote` trusted any `lamport: u64`. A peer that
published `lamport = u64::MAX` would win every LWW comparison forever
*and* poison the room's Lamport clock into the same range. `wall_ms`
was equally unchecked — informational, but a "sort-me-to-the-top"
vector for apps that render timelines by wall clock.

**What shipped.**
- Two public constants in `core/src/graph.rs`, re-exported from
  `activesync_core`:
  - `LAMPORT_SLACK: u64 = 1 << 20` (~1.05 M) — max gap between a
    node's Lamport and `graph.lamport()`.
  - `WALL_SKEW_MAX_MS: u64 = 86_400_000` — 24 h forward skew ceiling
    on `Transaction::wall_ms`.
- Two new `SyncError` variants (`LamportCeiling`, `WallClockSkew`),
  both with full diagnostic context (`id`, values, ceiling / now_ms).
- Two new `StateGraph` entry points:
  - `apply_remote_checked(node, now_ms: Option<u64>)`
  - `apply_remote_batch_checked(nodes, now_ms: Option<u64>)`
  Existing `apply_remote` / `apply_remote_batch` stay as
  `None`-passthrough wrappers so the WASM bridge and `compaction.rs`
  (neither has a trusted clock) are unchanged.
- Check order: hash → **Lamport ceiling → wall-skew** → ed25519 batch
  verify → parent/policy → insert. Sanity rejects never burn signature
  CPU.
- `wall_ms == 0` always passes (compaction snapshots + legacy unsigned
  clients). Lamport check uses `saturating_add` against
  `u64::MAX` attacks.
- Server wiring in `server/src/room.rs::import_nodes`: one
  `SystemTime::now() → millis` sample per pack, passed as
  `Some(now_ms)` into `apply_remote_batch_checked`. Rejected variants
  increment `activesync_lamport_rejected_total{reason=ceiling|wall_skew}`
  (registered via `describe_counter!` in `server/src/metrics.rs`) and
  are surfaced in the `errors` vec for the WS pack logger.
- No wire change, no SDK change.

**Tests.**
- `core/src/graph.rs` — 7 unit tests (boundary accept, over-ceiling
  reject, batch-mixed, wall-skew hit/miss, `None`-skips-check,
  zero-wall-accepted, batch wall-skew). 124/124 core lib tests green.
- `server/tests/lamport_ceiling.rs` — 2 integration tests driving
  `Rooms` + `import_nodes`. The rejection test pads `bad_wall` to
  `now + 24h + 1h` so the `SystemTime::now()` re-sample inside
  `import_nodes` can't push it back inside the ceiling.

**Next (Phase G).** G6 — token expiry enforced mid-session
(short-lived tokens, WS close `4002 token expired` once `expiry_secs`
lapses).

### G6 — Token expiry enforced mid-session (SHIPPED, Apr 2026)

**Problem.** `RoomToken` was validated only at `hello` — the stored
`expiry_secs` gated *admission*, not session length. Once the WS was
open the server never looked at it again. A leaked or extended token
kept working until the client disconnected of its own accord, which on
a long-lived editing session could be hours or days.

**What shipped.** Option (a) from the architecture plan — short-lived
tokens plus an SDK refresh pattern — is now the supported mechanism.
Option (b) revocation-list polling was **not** implemented (no network
dependency, no new endpoint, revisit only when a product actually
requires instant revocation rather than short-TTL rotation).

- `server/src/ws_handler.rs::verify_hello_token` return type changed
  from `Result<(), String>` to `Result<u64, String>`; on accept it now
  yields the token's `expiry_secs` to the caller. `RoomToken::verify`
  still performs the `now >= expiry` admission check, so already-dead
  tokens continue to fail `hello` with `4001 unauthorized` — G6 only
  covers tokens that expire while a session is live.
- Per-session state gains `token_deadline: Option<tokio::time::Instant>`.
  For unlocked rooms (`auth_key = None`) it stays `None`; for locked
  rooms it is computed exactly once at auth time as
  `Some(Instant::now() + Duration::from_secs(expiry_secs.saturating_sub(now_secs)))`.
- The main `tokio::select!` loop has a new first arm: when the deadline
  is `Some(d)` it awaits `tokio::time::sleep_until(d)`; when `None` it
  awaits `std::future::pending::<()>()` (the arm is never chosen, zero
  overhead on unlocked rooms). On fire:
  - increments `activesync_token_expired_disconnects_total{room}`
  - sends a best-effort `Close{4002, "token expired"}` with a 1 s
    timeout so a stuck sink can't block shutdown
  - breaks the session loop
- Close code `4002` is distinct from `4001 unauthorized` (bad/expired
  at hello) and `4008 rate limited`. SDKs read it as "refetch token
  and reconnect", not "user lost access".
- No wire change, no new CLI flag, no new server state. Tokens already
  carried `expiry`; G6 just honours it for the duration of the session.

**Tests.** `server/tests/token_expiry.rs` — 2 WS integration tests
against a live locked room:
- `expired_token_closes_with_4002`: 2 s-expiry token completes the
  handshake and then gets a `Close{4002}` within the 6 s poll window.
- `valid_token_does_not_close_prematurely`: 60 s-expiry token receives
  its welcome and is **not** closed within the first 3 s (regression
  guard against the deadline firing too early).

Both tests pass. Full server test suite after G6: 15 lib + 2 token-expiry
+ all G1/G3/G4/G5/backpressure/metrics/persistence/idle suites green.

**Phase G — Operational Safety: complete.** G1, G3, G4, G5, G6, G7 all
shipped. Next: product-polish wave (G8 SDK metrics hook, G9 conflict
surfacing, G10 migration docs).

### G8 — SDK metrics hook (SHIPPED)

**Implementation locations.**

- [web/sdk.js](web/sdk.js) — `makeMetrics(onMetric)` near the other
  small utilities returns `{ emit, enabled }`. `emit(kind, value, labels)`
  is a true no-op when `onMetric` is unset (zero allocation, zero hot-path
  cost). When set, app-side throws are caught + logged so a buggy hook
  can't corrupt SDK state.
- `createDoc` accepts a top-level `onMetric: (ev) => void` option. The
  delivered envelope is `{ kind, value, labels, timestamp }` where
  `timestamp` is `Date.now()`.
- `makeTransport` now takes a `metrics` parameter and is instrumented at
  the seven sites listed in the kinds table:
  - **`pack_applied`** — emitted in the `pack` message handler with
    `value = nodes.length` and `labels.from = "live"|"<peer>"`.
  - **`op_apply_latency`** — wraps `afterLocalMutation` in `createDoc`.
    `value` is `performance.now()` ms across `sendLocalDelta` +
    mesh broadcast. Labels: `type` (map/text/list/blob/etc.), `source`.
  - **`ws_reconnect`** — emitted twice: in `ws.onclose` with
    `outcome:"scheduled"` (and `delay_ms`/`was_connected`), and in
    `ws.onopen` with `outcome:"ok"` when the previous attempt count was
    >0. The streak counter resets on every successful connect.
  - **`blob_upload`** — emitted in both `sendBlobs` (WS-batched path,
    bytes estimated from base64 length) and `directUpload` (direct PUT,
    exact bytes).
  - **`blob_download`** — emitted in `blob-pack` (WS, exact bytes,
    count) and `fetchBlobViaUrl` (redirect, with `result:"ok"`/
    `"expired"`/`"err"`).
  - **`direct_upload_fallback`** — emitted in `sendBlobs`'s catch
    block when the presigned PUT fails and we fall back to bytes-over-WS.
    `labels.reason` carries the underlying error message.
- The demo wires the hook to `console.debug('[metric]', kind, value, labels)`
  in [web/demo.js](web/demo.js) so the surface gets exercised end-to-end;
  `web/index.html` is cache-busted to `demo.js?v=7`.

**Deferred from the original kind list.** `presence_latency` is not
shipped — the server doesn't echo presence broadcasts, so there is no
honest round-trip to measure. We can revisit this once presence acks
land or when peer-to-peer presence makes the latency observable.

---

### G8 — SDK metrics hook (design retained for reference)

**Problem.** The server exposes a rich Prometheus surface from G7
(`activesync_*` histograms, gauges, counters). The SDK is a black box
from the ops side: no visibility into sync latency, WS reconnect
frequency, bytes up/down, blob cache hit rate, or direct-upload (F6)
success rate. SpeechSlate and any other integrator needs to correlate
"server says p99 = 40 ms" with "client saw 800 ms apply latency" to
diagnose regressions. Today there is no hook to do this.

**Scope.** Add an SDK-side metrics emitter that fires on well-defined
events. The SDK does **not** push metrics anywhere itself — it
invokes a user-supplied callback. Integrators wire that callback to
their existing metrics pipeline (Prometheus pushgateway, Datadog,
OpenTelemetry, console.log for dev).

**Shape.**

```js
const doc = new Doc({
  url,
  token,
  onMetric: (ev) => { /* app owns shipping */ },
});
```

Each metric event is a plain object:

```jsonc
{
  "kind":      "pack_applied" | "ws_reconnect" | "blob_upload" |
               "blob_download" | "direct_upload_fallback" |
               "op_apply_latency" | "presence_latency",
  "timestamp": <ms since epoch>,
  "value":     <number, meaning depends on kind>,
  "labels":    { "path": "...", "result": "ok|err|timeout", ... }
}
```

**Kinds to ship.**

| Kind | Value | Labels | Fires when |
|---|---|---|---|
| `pack_applied` | node count | `{from: "initial" \| "live"}` | Server pack applied to local graph |
| `op_apply_latency` | ms | `{kind: "map" \| "text" \| "list" \| "blob"}` | Local apply → broadcast roundtrip |
| `ws_reconnect` | attempt # | `{reason, outcome}` | WS reconnect attempt completes |
| `blob_upload` | bytes | `{transport: "ws" \| "direct", result}` | Blob upload completes |
| `blob_download` | bytes | `{transport: "ws" \| "redirect" \| "cache", result}` | Blob fetch completes |
| `direct_upload_fallback` | bytes | `{reason}` | F6 direct upload fell back to WS |
| `presence_latency` | ms | — | Presence heartbeat → server ack |

**Implementation sketch.** One helper `emit(kind, value, labels)` in
`web/sdk.js`. Call sites: `handleServerPack`, `sendBlobs`, `directUpload`,
`fetchBlobViaUrl`, the WS reconnect loop, presence handler. Gate every
callsite on `this.onMetric` being set — zero-cost when not configured.
No allocation unless the hook is installed.

**Wire/server impact.** None. Pure client-side observability on top of
existing flows.

**Tests.** Unit tests via `node --test` (or a small harness) that drive
a fake WS and assert the emitter fires the expected kinds. Integration
sanity: demo page wires `onMetric` to `console.log` and confirms the
expected cascade on load + one setBlob + one reconnect.

**Docs.** Document the kind list + labels in `docs/sdk.md`; add a
"Shipping metrics to Prometheus" snippet (pushgateway example).

**Non-goals.**
- Client-side histograms / aggregation. The hook emits point events; the
  app owns bucketing. Keeps the SDK small and lets apps pick their stack.
- Automatic correlation with server metrics. That's an ops-side join
  (trace id / request id propagation) — out of scope for G8.

### G9 — Conflict surfacing (SHIPPED)

**Implementation locations.**

- [core/src/conflicts.rs](core/src/conflicts.rs) — new module. Pure
  `detect_conflicts(nodes: &[&SyncNode]) -> Vec<ConflictEvent>`. Two
  passes per op family (Map, List); O(N) over the node set. Output is
  sorted deterministically so two callers with identical graph state
  produce identical sequences.
- [core/src/graph.rs](core/src/graph.rs) — `StateGraph::detect_conflicts(&self)`
  thin wrapper that collects the graph's nodes and hands them to the
  detector.
- Definition. A conflict is recorded when, for a given key (Map key or
  `list_key#item_hex` for List), a loser op came from a *different*
  author than the winner. Same-author overwrites are refinement, not
  conflict. This is a pragmatic approximation of causal concurrency —
  in practice "different-author loser" exactly matches the surface
  apps want to show. Deep causal traversal is deferred.
- Kinds: `MapOverwrite`, `ListMoveLost`, `ListDeleteWon`. Text is not
  emitted (RGA per-char attribution is a separate feature — see
  non-goals in the design section below).
- [bridge/src/lib.rs](bridge/src/lib.rs) — `SyncStore.take_conflicts_json()`
  returns a JSON array of conflicts not yet delivered. Internal
  fingerprint set (`HashSet<(kind,key,wl,wa,ll,la)>` + `VecDeque` for
  bounded eviction at 4096 entries) ensures each conflict ships once.
- [web/sdk.js](web/sdk.js) — `conflictE` emitter, `CONFLICT_BUFFER_MAX=256`
  ring buffer, `pollConflicts()` invoked from every `emitChange` with
  `ev.type === 'pack'|'bulk'` or `ev.source === 'local'`. Public API:
  `doc.onConflict(cb)` + `doc.recentConflicts(sinceMs)`. Each event
  also fans out through the G8 metrics hook as
  `emit('conflict', 1, {kind, by_you})`.

**Tests.** Four in `core/src/conflicts.rs::tests`:
- `map_concurrent_set_emits_overwrite` — `Set(k,a)` vs `Set(k,b)` from
  two authors merges into one `MapOverwrite` event.
- `map_same_author_overwrite_is_not_conflict` — single author
  rewriting their own key produces zero conflicts.
- `list_concurrent_move_emits_move_lost` — two authors Move the same
  item to different positions; reports at least one `ListMoveLost`.
- `list_delete_emits_delete_won` — Delete wins over concurrent Move;
  reports at least one `ListDeleteWon`.

**Rebuild.** `wasm-pack build bridge --target web --out-dir ../web/pkg`
was re-run so the `take_conflicts_json` export lands in
`web/pkg/activesync_bridge.js`. If you pull this branch, rerun that
command (or the `wasm:build` task you have wired up) before `web/`
will work.

---

### G9 — Conflict surfacing (design retained for reference)

**Problem.** The CRDT layer silently resolves every conflict: Map uses
LWW on `(lamport, author)`, List absorbs on Delete, Text merges
character-level. From the user's perspective a concurrent edit can
disappear with no feedback. SpeechSlate's AAC use case is forgiving
(users overwrite their own symbols intentionally), but any editor-class
product needs a "your change was overridden" affordance — even if the
merge is correct, the *user* needs to know it happened.

**Definition of "conflict".** A node whose operation(s) lost the LWW
race against a concurrent peer's node. Specifically:
- **Map:** `Set(k, v_a)` loses when a concurrent `Set(k, v_b)` has a
  higher `(lamport, author)`.
- **List:** `Move` loses when a concurrent `Move` or `Delete` on the
  same item wins LWW. `Insert` losing is rare (collides only on
  `ItemId`, which is random).
- **Text:** `insert`/`delete` on the same position from concurrent
  authors doesn't "lose" — both apply — but the *visible ordering*
  may surprise the user.

**Scope.** Observability, not policy. The SDK exposes losing edits as
events; the app decides whether to show a toast, a revert button, an
audit log, or nothing. Server semantics unchanged.

**Shape.**

```js
doc.onConflict((ev) => {
  // ev = {
  //   at:        timestamp,
  //   key:       "<map key or list key>",
  //   kind:      "map_overwrite" | "list_move_lost" | "list_delete_won",
  //   localOp:   { kind, value },
  //   winningOp: { kind, value, author, lamport },
  //   byYou:     bool,  // was the winning op from this client?
  // };
});
```

Also expose a post-hoc query: `doc.recentConflicts(sinceMs)` returning
the same events for apps that want to show a history panel instead of
a live toast.

**Implementation sketch.** Conflict detection already happens in
`core/src/graph.rs::resolve_map` (and the list resolver); they drop
losers on the floor. Add a second pass that returns a `Vec<ConflictEvent>`
alongside the resolved state. Bridge surfaces this via a new WASM
export `take_conflicts_json`; SDK polls after every pack-apply and
fires `onConflict` for each. Keep the in-memory buffer bounded (last
256 events).

**Tests.**
- Unit: concurrent Map `Set(k, a)` + `Set(k, b)` with `b` winning
  LWW emits one `map_overwrite` with `localOp = {value: a}` and
  `winningOp = {value: b}`.
- Unit: concurrent List `Move` on same item emits `list_move_lost`
  for the loser.
- Bridge round-trip: WASM → SDK → `onConflict` fires.
- SDK regression: events emitted in the order the CRDT applied the
  winning nodes (same order peers would see them).

**Wire/server impact.** None. Purely a derived view over data the
graph already has.

**Non-goals.**
- Conflict *resolution UI*. That's a product decision per integrator.
- Automatic revert. Apps can call the existing `MapHandle.set(...)` to
  re-apply a losing value if they want to.
- Text-level character attribution. RGA already gives you "who
  inserted this char"; that's a different feature (inline blame).

### G10 — Migration + operator docs (SHIPPED)

**Implementation locations.**

- [docs/migration.md](docs/migration.md) — phase-boundary deltas (F4
  through F8 + Phase G). Each section covers schema changes, config
  knobs added, wire back-compat story, required code changes (usually
  none), and rollback notes. Explicit "mixed-version fleets" note at
  the bottom points at `A7` capability negotiation.
- [docs/operator.md](docs/operator.md) — steady-state runbook. Full
  CLI flag table; every `activesync_*` metric with its kind, labels,
  and suggested alert shape; per-backend backup/restore
  (`DirPersistence`, `PostgresNodeStore`, `MongoNodeStore`,
  `S3BlobStore`); capacity planning rules of thumb drawn from the
  bench suite; rolling-restart procedure; symptom → first-check
  cheat sheet.
- [docs/integration.md](docs/integration.md) — four worked
  embedding examples: self-host with `DirPersistence`,
  SpeechSlate-shape `Composite<MongoNodeStore, S3BlobStore>` with
  `S3Auth::Delegate`, Postgres shape with `S3Auth::Direct`, and the
  F5 JWT bridge. Every example uses the real crate APIs
  (`Rooms::new(server_key, persistence, broadcast_capacity,
  peer_rate_nodes, peer_rate_bytes)`, `DirPersistence::open`,
  `MongoNodeStore::connect`, `PostgresNodeStore::connect_and_migrate`,
  `S3BlobStore::new`, `BridgeConfig { verifier, room_key, … }`).
- Every snippet round-trips against current `cargo doc` / node syntax
  at authoring time; no API reference duplication (PLAN.md G10
  non-goal).

---

### G10 — Migration + operator docs (design retained for reference)

**Problem.** F6 and F7 just landed a trait split + two new adapters +
capability negotiation + new wire messages. Anyone integrating
activesync today reads `PLAN.md` top-to-bottom and reverse-engineers
the path. That worked when we had one user; it will not scale.

**Scope.** A short, linear migration guide plus an operator runbook.
Target readers:
1. **SpeechSlate backend engineer** wiring Mongo + S3 into sync-server.
2. **Self-host operator** upgrading past F6 (new capability flag,
   `DirPersistence` still works unchanged).
3. **Frontend developer** upgrading past F8 (`doc.list<T>`) or F6
   (opaque direct-upload behind `setBlob`).

**Deliverables.**

- `docs/migration.md` — version-to-version delta. Sections per phase
  boundary: "Before F4 → after F4", "Before F6 → after F6", etc. Each
  section has: schema changes (if any), config knobs added, wire-level
  back-compat story, required code changes (usually none).

- `docs/operator.md` — steady-state operations for a deployed server.
  Sections:
  - **Config reference.** Every CLI flag + env var, with defaults.
  - **Metrics reference.** Every `activesync_*` metric, its kind
    (counter/gauge/histogram), labels, and what an alert should look
    like.
  - **Backup + restore.** `DirPersistence`, `PostgresNodeStore`,
    `MongoNodeStore`, `S3BlobStore` — each with "what to back up,
    how to restore, consistency guarantees".
  - **Capacity planning.** Rules of thumb: node bytes per op, blob
    bytes per room, peers per replica. Numbers drawn from the bench
    suite (`cargo bench`).
  - **Upgrading sync-server.** Rolling-restart procedure given
    room-to-server affinity; capability negotiation handles the
    mixed-version window automatically.

- `docs/integration.md` — "how to embed activesync in your product".
  One example each for:
  - Self-host with `DirPersistence` (smallest config).
  - SpeechSlate-shape (`Composite<MongoNodeStore, S3BlobStore>`).
  - Postgres-shape (`Composite<PostgresNodeStore, S3BlobStore>`).
  - JWT bridge config for trusted-issuer auth (F5).

**Non-goals.**
- API reference for every type. `cargo doc` covers that; duplicating
  it in Markdown is a maintenance drag.
- Tutorial-style "build an AAC app in 5 minutes". The demo app
  (`web/`) is the tutorial; docs point to it.

**Tests.** Docs aren't tested by CI, but every code snippet in
`docs/*.md` must round-trip through `cargo check` / `node --check`
at authoring time. Call out the snippet's source file so it stays
honest.

### G11 — Hot-room memory bound (observability SHIPPED; enforcement deferred)

**Shipped — observability slice.** The first-step gauge is live:
`activesync_room_bytes_resident{room}` is registered in
[server/src/metrics.rs](server/src/metrics.rs) via `describe_gauge!`
and updated at the end of every `import_nodes` in
[server/src/room.rs](server/src/room.rs). The value is
`node_count × 512 + Σ blob.len()` — a rough estimate (large
transactions under-count on the per-node 512 B constant). Operator
docs call this out; see
[docs/operator.md](docs/operator.md#metrics-reference).

**Deferred — enforcement options A/B/C.** No compaction trigger, no
LRU hot window, no size-triggered eviction. The revisit trigger
remains: first production room with >24 h continuous activity, or
RSS SLO breach in a hosted deployment. Until then the gauge alone is
enough — don't pre-optimize a shape we haven't seen.

---

### G11 — Hot-room memory bound (design retained for reference)

**Problem.** A long-lived room holds its full `StateGraph` + (in dev)
`MemoryBlobStore` + `verified_ids` cache + presence map in RAM for as
long as any peer is connected. Every axis that could grow unboundedly
on disk or over the wire now has a ceiling — G4 (blob GC), G3 (peer
ingress), G1 (broadcast backlog), G5 (Lamport), F4 follow-up (idle
eviction) — but a *hot* room (continuously connected, never idle) has
no per-room RAM cap. First >24 h continuous-activity production room
is the latent blast radius.

**What's already on the shelf.** D3 compaction (`core/src/compaction.rs`,
snapshot + `rebuild_from_snapshot` + `verify_snapshot`) and D4
deterministic replay (`core/src/replay.rs`, `canonical_hash`) both
shipped. The primitive to truncate DAG history while preserving
verifiable state exists; G11 is about wiring it into the server's
hot-room lifecycle, not inventing it.

**Options (not yet chosen).**
- **A. Compaction-driven.** Server periodically calls `compact()` on
  rooms past a node-count or byte threshold, signs the snapshot with
  its own key (same pattern as E1 authoritative writes), drops
  subsumed ancestors from RAM, keeps them on disk for late joiners.
  Cleanest bound; requires the server to hold `can_write` on a
  sentinel snapshot path (trivial in Authoritative mode, needs a new
  Policy rule in Cooperative mode).
- **B. LRU hot window.** Keep last N nodes + leaves in RAM; demand-page
  older nodes from `SqliteNodeStore` on lookup. No wire or policy
  change; trades p99 latency on ancestry walks for bounded RAM.
- **C. Size-triggered force-evict.** Reuse the F4 idle-evictor path
  with a byte-count trigger. Simplest, but disconnects live peers
  briefly to rehydrate — same UX as a server restart.

**First step regardless of option.** Observability before enforcement.
Add `activesync_room_bytes_resident{room}` gauge (node count ×
per-node estimate + blob-store resident bytes + verified_ids len) so
the trend is measurable before we pick a cap. Pairs with G7's metrics
infrastructure; zero wire impact.

**Revisit trigger.** First production room with >24 h continuous
activity, or RSS SLO breach in a hosted deployment, whichever comes
first. Until then the gauge alone is enough — don't pre-optimize a
shape we haven't seen.

**Not in G11.** `verified_ids` unbounded growth (bounded in practice
by node count, falls with compaction); presence map growth (bounded
by concurrent peers × F4 idle eviction); dev-mode `MemoryBlobStore`
(not a production configuration — use `--store` with `FileBlobStore`).
