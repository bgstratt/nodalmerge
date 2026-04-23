# ActiveSync Architecture

> A Rust/WASM CRDT engine with cryptographic identity, content-addressed
> storage, and a transport stack that scales from "two browser tabs" to
> "10⁶-node graphs over a thin WebSocket pipe."

---

## 1. Components

```
┌────────────────────────────────────────────────────────────────────────┐
│  Browser Tab (any number)                                              │
│                                                                        │
│  activesync_bridge.wasm   (Rust → WASM, wasm-bindgen)                 │
│  ├── SyncStore                                                         │
│  │   ├── StateGraph<MemoryNodeStore>   — append-only DAG              │
│  │   ├── BlobStore (in-memory)         — CAS for large bytes          │
│  │   ├── Ed25519 SigningKey            — peer identity                │
│  │   ├── E2EE key (optional)           — AES-256-GCM room key         │
│  │   ├── Frontier                      — set of DAG tips              │
│  │   ├── IBF / MerkleSearchTree        — sync indices                 │
│  │   └── RGA text resolver             — collaborative text           │
│  │                                                                     │
│  demo.js                                                               │
│  ├── WebSocket client + reconnect                                      │
│  ├── WebRTC peer channels (STUN signaled over WS)                      │
│  ├── IndexedDB persistence (debounced 250 ms)                          │
│  └── Delta broadcast (export_nodes_missing_from)                       │
└────────────────────┬───────────────────────────────────────────────────┘
                     │  ws[s]://host/ws/<room_id>     (WebRTC for P2P)
┌────────────────────▼───────────────────────────────────────────────────┐
│  activesync-server  (Rust / Tokio multi-thread / Axum 0.7)            │
│  ├── Rooms registry  — one Room per room_id                           │
│  │   ├── StateGraph        — server's own peer view of the DAG        │
│  │   ├── BlobStore         — in-memory CAS                            │
│  │   ├── Policy (optional) — A5 capability rules                      │
│  │   ├── RoomToken pubkey  — locked vs. open rooms                    │
│  │   └── broadcast::Sender — fan-out to every connected peer          │
│  ├── ws_handler           — one async task per connection             │
│  ├── Ed25519 server keypair (server.key, persisted)                   │
│  ├── tracing + EnvFilter (RUST_LOG)                                   │
│  └── replay CLI            — deterministic state reconstruction       │
└────────────────────────────────────────────────────────────────────────┘
```

### Crate map

| Crate | Role |
|---|---|
| `activesync-core` | Pure Rust. DAG, CRDT resolution, Blake3, Ed25519, AES-GCM, IBF, MST, RGA text, replay, compaction, policy, tokens. No I/O. |
| `activesync-bridge` | `wasm-bindgen` cdylib. Thin facade over `core`; adds JS-friendly JSON/base64 serialization, signing, and IndexedDB-friendly accessors. |
| `activesync-server` | Axum WebSocket server. Rooms, fan-out, IBF/MST handlers, Super-Peer policy enforcement, `replay` CLI. |

---

## 2. The DAG

Every mutation becomes a `Transaction` wrapped in a `SyncNode`:

```rust
SyncNode {
    id:          Blake3(Transaction)        // content-addressable id
    transaction: {
        author:    [u8; 32]                 // Ed25519 verifying key
        lamport:   u64                      // logical clock
        wall_ms:   u64                      // wall time (informational)
        ops:       Vec<Op>                  // Op::Map | Op::Text | Op::List
        parents:   Vec<Hash>                // Merkle links to previous frontier
    }
    signature:   [u8; 64]                   // Ed25519 over id.as_bytes()
}
```

* **Immutable / append-only.** Tampering breaks either the Blake3 id or the
  Ed25519 signature.
* **`Op` is extensible.** `Op::Map(MapOp)` covers LWW key/value and blob refs;
  `Op::Text(TextOp)` carries RGA insert/delete; `Op::List` is reserved.
* **Frontier.** `Frontier { heads: Vec<NodeId> }` tracks DAG tips. New nodes
  parent off the current frontier; merging advances it.
* **Lamport clock.** Each new node uses `max(local_lamport, wall_ms) + 1`.
  Causality survives reconnects without server help.

### CRDT semantics

| Op family | Strategy | Tie-break |
|---|---|---|
| `Op::Map` (LWW) | Last-Write-Wins on `(lamport, key)` | Author Ed25519 pubkey, lex order |
| `Op::Text` (RGA) | Per-character `OpId = (lamport, author)` | Higher `OpId` appears leftward at the same anchor; deletes are tombstones |

Both strategies are pure functions of the DAG. Two peers with the same node set
*always* converge to byte-identical state — verified by deterministic `replay()`
(see §6).

### Speculative vs. canonical state (E2)

`StateGraph` exposes two views:

* **`read_speculative(key)`** — includes locally authored, not-yet-confirmed
  writes. UI reads this for instant feedback.
* **`read_canonical(key)`** — only includes nodes the server (or any
  authoritative signer) has merged.

Snap-back is automatic: when a higher-priority canonical node arrives that
contradicts a speculative write, LWW pubkey priority chooses the canonical
value. No application logic required.

### Tick batching (E3)

`StateGraph::set_tick_config(TickConfig { interval_ms, max_ops_per_tick })`
buffers ops into a single signed `SyncNode` per tick. Each op keeps its own
`OpId`, so merge results are independent of how peers grouped them.

---

## 3. Sync Protocol

The handshake stack went from "send every node id" to "send a 2.9 KB invertible
filter and walk a 16-way Merkle Search Tree if needed."

### Handshake (capability-negotiated)

```
Tab connects:
  → hello   {
      frontier: [hex],                  // A6 — DAG tips, not full id list
      ibf:      "b64",                  // B1 — 80-cell, k=3 IBF (~2.9 KB)
      mst_root: "hex",                  // B2 — Merkle Search Tree root
      caps:     {                       // A7 — feature negotiation
        supports_ibf, supports_mst,
        supports_postcard, supports_encryption,
        supports_webrtc, max_tick_interval_ms
      },
      pubkey:    "hex",
      auth_key:  "<RoomToken>?"          // C3 — required if room is locked
    }

  ← welcome {
      root, frontier, missing:[ids server wants],
      caps, server_pubkey, peers, mst_root
    }

  → pack { nodes: postcard+b64 }         // A3 — binary wire
  → blob-request / blob-pack             // CAS sync, on demand
```

### Reconciliation modes

| Mode | When chosen | Cost |
|---|---|---|
| **Pack only** | Both peers small or empty | 1 RTT, full pack |
| **IBF (B1)** | `supports_ibf` on both sides | 1 RTT, ~2.9 KB regardless of graph size, decode `O(diff)` |
| **MST (B2)** | `supports_mst` and IBF cannot decode | ≤ a few RTTs, descends only diverging branches |

The IBF decoder has a hard iteration cap (`IBF_CELLS * 4`) so a degenerate
filter (e.g. populated client vs. empty server) cannot stall the handler.

### Delta broadcast

After the welcome, peers stop sending full graphs. The bridge tracks
`sentToServer` and emits `export_nodes_missing_from(known_ids)` on every local
mutation. A 1k-keystroke session sends 1k tiny packs instead of 1k full-graph
exports.

### Blob CAS

Blobs are content-addressed by Blake3. The protocol keeps blob bytes off the
node graph entirely:

* `blob-upload` / `blob-available` (broadcast) / `blob-request` / `blob-pack`.
* On every connect a tab re-uploads its locally cached blobs (the server's
  in-memory `BlobStore` may have restarted).
* Missing blobs are requested immediately on `welcome`, with a 10-second
  retry loop as a fallback.

### Transports

* **WebSocket** — primary; multi-thread Tokio runtime; Axum 0.7 router; CORS
  open in dev.
* **WebRTC data channel (D2)** — STUN-based, signaled over the same WS. When a
  channel opens, node packs and blob bytes flow peer-to-peer; the server is
  bypassed for that pair. Falls back to WS on negotiation failure.

---

## 4. Security & Trust Model

| Concern | Mechanism |
|---|---|
| **Node authenticity** | Every node is Ed25519-signed by its author. `apply_remote` verifies before accepting. |
| **Content integrity** | `node.id = Blake3(Transaction)`. Tampered nodes fail the hash check. |
| **Blob integrity** | `Blake3(bytes) == expected_hash` enforced on store. Corrupt bytes are rejected client-side. |
| **Identity** | Each tab generates a 32-byte random seed → Ed25519 keypair (sessionStorage). Public key is the peer identity. |
| **Deterministic LWW tie-break** | Author pubkey (lex). The server cannot fabricate a winner without a private key. |
| **Room auth (C3)** | `RoomToken` = `Ed25519_sign(room_id ‖ peer_pubkey ‖ expiry ‖ caps)` by the room key. Server verifies on `hello`. Empty token = open room. |
| **Write policies (A5)** | `Policy { rules: [PolicyRule { path_glob, can_write, can_read, can_derive }], default }`. Enforced inside `apply_remote` on every peer — server *and* client. Violating nodes are silently dropped, never re-broadcast. |
| **Super-Peer (E1)** | Server holds a persistent Ed25519 keypair (`server.key`) and joins rooms as a regular peer. Authoritative mode = policy lists `server_pubkey` as the only `can_write` for protected paths. Cooperative mode = no policy, server is just a well-connected relay. |
| **End-to-End Encryption (D1)** | HKDF-SHA256 derives a room key; AES-256-GCM encrypts the op set; deterministic nonce from `(room_key, author, lamport)` (no WASM RNG required). Ciphertext rides as a single sentinel op (`key = "\x00e2ee"`); the outer Ed25519 signature is preserved so the server still verifies authenticity without decrypting. |
| **Transport** | `ws://` in dev, `wss://` behind any TLS terminator in prod. |

The server is *cryptographically incapable* of:

* signing nodes as any peer (it has no peer private key);
* mutating an existing node (Blake3 + Ed25519 detect any change);
* injecting an LWW winner (no peer keypair, no pubkey priority);
* reading op contents in an E2EE room (no room key).

It *can* refuse to relay or store specific nodes, which is why Super-Peer mode
makes that authority explicit via signed `Policy` nodes.

---

## 5. Persistence & Data Segregation

| Layer | Storage | Lifetime |
|---|---|---|
| **Node graph (browser)** | IndexedDB `activesync-v5/nodes` (postcard pack, key `'all'`) | Survives refresh / page close |
| **Blob bytes (browser)** | IndexedDB `activesync-v5/blobs` (`Uint8Array` keyed by hex hash) | Survives refresh; refilled from server on miss |
| **Identity seed** | `sessionStorage["activesync-identity-v3"]` | Survives refresh, cleared on tab close |
| **Server graph** | In-memory `RwLock<StateGraph>` per room | Lost on restart; clients repopulate via handshake |
| **Server blobs** | In-memory `RwLock<BlobStore>` per room | Lost on restart; clients re-upload on connect |
| **Server identity** | `server.key` (32-byte seed file) | Persistent across restarts |

IndexedDB writes are trailing-debounced at 250 ms so high-frequency typing
doesn't hammer the disk. Blob bytes are persisted eagerly on `store_blob_bytes`
and `set_blob`.

Rooms are isolated at the server: each room has its own `StateGraph`, `BlobStore`,
optional `Policy`, optional `RoomToken` pubkey, and broadcast channel.

---

## 6. Determinism & Replay

`activesync_core::replay::replay(&[SyncNode], Option<&Policy>) -> ResolvedState`
is a pure function. Same input → same `ResolvedState { map, hash }` on every
platform, every time. Property tests (`replay_is_deterministic_with_out_of_order_delivery`)
shuffle delivery order and assert the canonical hash is invariant.

This is the trust anchor for:

* **Compaction (D3).** A snapshot is a normal signed `SyncNode` whose two
  sentinel ops carry `snapshot_hash = canonical_hash(...)` and the subsumed
  frontier. Receivers verify the snapshot by replaying the pre-compaction log
  and comparing hashes — no trust in the compactor required beyond their
  signature.
* **Server `replay <room_id>` CLI.** Reconstructs full state from the
  in-memory log; useful for audit, debugging, and validating proposed snapshots.
* **Snapshot acceptance.** Peers joining post-compaction get
  `snapshot + recent_delta` and start contributing immediately.

---

## 7. Observability

The server uses `tracing` + `tracing-subscriber` with `EnvFilter`. Default
filter: `info,activesync_server=info,activesync_core=info`. Override at runtime:

```powershell
$env:RUST_LOG = "debug,activesync_server::ws_handler=trace"
cargo run -p activesync-server
```

Per-connection logs include structured fields (`peer`, `room`); pack accepts
log at `info`, lifecycle (cleanup, exit) at `debug`. The earlier `eprintln!`
A1–A9a debug stream has been removed.

---

## 8. Benchmarks

Release profile, latest run (`cargo bench -p activesync-core`):

| Benchmark | Result | Target | Notes |
|---|---|---|---|
| `blob_verify_50kb` | **9.2 µs** | <500 µs | 54× under target |
| `resolve_1k` | **338 µs** | <1 ms | LWW resolution, 1 000-key map |
| `sync_handshake_pack_1k_missing` | **387 µs** encode | — | Wire size: 179.7 KB for 1 k nodes |
| `sync_handshake_ibf_encode_1k` | **270 µs** | — | Hello payload: **2.9 KB** for any graph size |
| `sync_handshake_ibf_decode_1k_diff` | **888 ns** | — | XOR + peel; `O(diff)`; iteration-cap guard |
| `sync_handshake_mst_build_1k` | **1.32 ms** | — | 16-way nibble trie from 1 k node IDs |
| `sync_handshake_mst_build_2k` | **3.28 ms** | — | Linear scaling confirmed |
| `sync_handshake_mst_simulate_1k_diff` | **302 µs** | — | 1 k-node diff in 2 k-node tree, **3 round trips** |
| `merge_10k` | **320 ms** | <10 ms | Per-node `apply_remote`; one Ed25519 verify per node. |
| `merge_10k_batch` | **34 ms** | <10 ms | `apply_remote_batch`: dedupe + parallel `verify_batch` (rayon, 64-node chunks) + session `verified_ids` cache. **9.4× over the per-node path.** Server `import_nodes` ingests packs through this path. |

Test suite: **117 / 117** passing in `activesync-core` (`cargo test -p activesync-core`).

---

## 9. Phase status (see `PLAN.md` for full detail)

| Phase | Item | Status |
|---|---|---|
| A | A1 Storage trait, A2 Op enum, A3 postcard wire, A4 benches, A5 Policies, A6 Frontier, A7 Capabilities | ✅ |
| B | B1 IBF, B2 MST | ✅ |
| C | C1 RGA text, C2 IndexedDB, C3 Room tokens | ✅ |
| D | D1 E2EE, D2 WebRTC P2P, D3 Compaction, D4 Replay | ✅ |
| E | E1 Super-Peer, E2 Speculative/canonical views, E3 Tick batching | ✅ |
| Ops | tracing + RUST_LOG | ✅ |
| Future | Server-side disk `NodeStore` / `BlobStore` adapters; batch-verify; CDN `BlobStore::resolve_url` adapter | open |

Everything described in the protocol stack is on disk and exercised by tests
and the live demo. The remaining gaps are deployment-ergonomic (durable server
storage, batched signature verification, CDN-backed blobs) rather than
protocol-level.
