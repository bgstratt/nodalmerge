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
signing (C3) is wired through `roomSeed` + `tokenCaps`. `doc.list()`
deliberately throws — List CRDT is tracked post-F. `doc.store` is the escape
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
  `features = ["http-listener"]`) in ctivesync-server.
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

**Next (Phase G).** G1 — backpressure (send timeout, 4001 lagged close,
`--broadcast-capacity`). G3 — rate limit (governor token buckets,
`--peer-rate-nodes` / `--peer-rate-bytes`, 4008 close).
