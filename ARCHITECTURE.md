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
│  web/sdk.js    createDoc({ room, serverUrl, … })                       │
│  ├── MapHandle / TextHandle / PresenceHandle  — F1a document API       │
│  ├── Subscription filter (glob `world/**`)    — F3a client-side        │
│  ├── WebSocket client + exp-backoff reconnect                          │
│  ├── makePeerMesh (F1b) — per-peer RTCDataChannels, WS signaling       │
│  │   sync channel (JSON) + blobs channel (binary [hash][bytes])        │
│  └── onChange({ transport: 'ws' | 'webrtc' })                          │
│                                                                        │
│  activesync_bridge.wasm   (Rust → WASM, wasm-bindgen)                  │
│  └── SyncStore (facade over core)                                      │
│      ├── StateGraph<MemoryNodeStore>   — append-only DAG               │
│      ├── MemoryBlobStore               — CAS for large bytes           │
│      ├── Ed25519 SigningKey            — peer identity                 │
│      ├── E2EE key (optional)           — AES-256-GCM room key          │
│      ├── Frontier                      — set of DAG tips               │
│      ├── IBF / MerkleSearchTree        — sync indices                  │
│      └── RGA text resolver             — collaborative text            │
│                                                                        │
│  IndexedDB (activesync-v5): nodes (postcard pack) + blobs (Uint8Array),│
│  trailing-debounced 250 ms.                                            │
└────────────────────┬───────────────────────────────────────────────────┘
                     │  ws[s]://host/ws/<room_id>     (WebRTC P2P per peer)
┌────────────────────▼───────────────────────────────────────────────────┐
│  activesync-server  (Rust / Tokio multi-thread / Axum 0.7)             │
│  ├── Rooms registry  — one Room per room_id                            │
│  │   ├── StateGraph        — server's own peer view of the DAG         │
│  │   ├── MemoryBlobStore   — in-memory CAS (hydrated from disk)        │
│  │   ├── Policy (optional) — A5 capability rules                       │
│  │   ├── RoomToken pubkey  — locked vs. open rooms                     │
│  │   ├── connected_peers + idle_since  — F4-follow-up eviction clock   │
│  │   ├── tick_abort        — E1 authoritative tick loop handle         │
│  │   └── broadcast::Sender — fan-out to every connected peer           │
│  ├── ws_handler    — one async task per connection;                    │
│  │                   F3b subscription filter on catch-up + broadcast   │
│  ├── ServerPersistence (F4)  — optional `--store <dir>`                │
│  │   └── DirPersistence: SQLite WAL (nodes) + file-per-blob            │
│  │     Hydrate on get_or_create; write-through on accept.              │
│  ├── spawn_idle_sweeper (60 s) — evicts idle rooms when durable        │
│  ├── Ed25519 server keypair (~/.activesync/server.key, persistent)     │
│  ├── tracing + EnvFilter (RUST_LOG)                                    │
│  └── replay CLI            — deterministic state reconstruction        │
└────────────────────────────────────────────────────────────────────────┘

┌──── Out-of-band (F5) ──────────────────────────────────────────────────┐
│  activesync-jwt-bridge: verify JWT (HS/RS/ES256) → mint RoomToken      │
│  Docker: multi-stage image, non-root, --store /data, :7878             │
└────────────────────────────────────────────────────────────────────────┘
```

### Crate map

| Crate | Role |
|---|---|
| `activesync-core` | Pure Rust. DAG, CRDT resolution, Blake3, Ed25519, AES-GCM, IBF, MST, RGA text, replay, compaction, policy, tokens. No I/O. |
| `activesync-bridge` | `wasm-bindgen` cdylib. Thin facade over `core`; adds JS-friendly JSON/base64 serialization, signing, and IndexedDB-friendly accessors. |
| `activesync-server` | Axum WebSocket server. Rooms, fan-out, IBF/MST handlers, Super-Peer policy enforcement, F3b subscription filter, optional `DirPersistence` (SQLite + file blobs), idle-room eviction, `replay` CLI. |
| `activesync-jwt-bridge` | Verifies a third-party JWT (HS256 / RS256 / ES256) and mints a `RoomToken`. Lets Clerk / Supabase / Auth0 / custom auth issue room credentials without owning identity. |
| `web/sdk.js` (+ `sdk.d.ts`) | High-level document API on top of the bridge: `createDoc`, `MapHandle`, `TextHandle`, `PresenceHandle`, WS transport, WebRTC peer mesh, glob subscriptions, reconnect. |

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

### Subscriptions (F3)

A room is flat. Peers carve it internally by subscribing to key-path globs
(`world/**`, `chat/**`, `intents/player1/*`). Default is `**` (zero-overhead
fast path).

* **Client-side (F3a).** The SDK's `createDoc({ subscribe: [...] })` compiles
  globs and filters `MapHandle` / `TextHandle` materialization. Pure JS,
  no wire changes.
* **Server-side (F3b).** `hello.subscribe` (and a runtime `subscribe`
  message) re-scopes the WS relay. `filter_pack_for_subscriber` unpacks each
  outbound pack, drops non-matching nodes, and suppresses the send entirely
  when nothing remains. Applied to both catch-up and steady-state broadcast.
  Sentinel keys (`\x00…` — E2EE envelopes, snapshot meta) always pass
  through. Writes are **not** filtered — authority is Policy's job (A5).

Nested rooms / sub-rooms are deliberately not a thing. Hard isolation = new
top-level room; authority = Policy; bandwidth slicing = subscriptions.

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
| **Third-party auth (F5)** | `activesync-jwt-bridge` verifies a JWT signed by your auth provider (HS256 / RS256 / ES256 via `jsonwebtoken 9`), matches the claim shape `{ room, pubkey, exp, caps[] }` plus standard `exp/nbf/iss/aud`, and mints a `RoomToken` using the room's Ed25519 key. Optional `allowed_issuers` / `allowed_audiences` allow-lists. ActiveSync never owns identity. |

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
| **Server graph** | `RwLock<StateGraph>` in memory; optional durable mirror via `DirPersistence` | In-memory build: lost on restart. With `--store`: survives restarts and idle eviction. |
| **Server blobs** | `RwLock<MemoryBlobStore>` in memory; optional `blobs/<room>/<hash>` files | Same as node graph. |
| **Server identity** | `~/.activesync/server.key` (32-byte seed file) | Persistent across restarts |

IndexedDB writes are trailing-debounced at 250 ms so high-frequency typing
doesn't hammer the disk. Blob bytes are persisted eagerly on `store_blob_bytes`
and `set_blob`.

Rooms are isolated at the server: each room has its own `StateGraph`, `BlobStore`,
optional `Policy`, optional `RoomToken` pubkey, and broadcast channel.

### Server-side persistence (F4)

Opt-in via `--store <path>`. Absent, the server stays fully in-memory (good
for dev, demos, and ephemeral deployments). With `--store`:

* **Nodes.** SQLite (`<path>/activesync.db`, `journal_mode=WAL`,
  `synchronous=NORMAL`, `INSERT OR IGNORE` for idempotency). One row per
  `SyncNode`.
* **Blobs.** File-per-blob at `<path>/blobs/<room>/<hash>`, written via
  tmp-file + rename. Hydrate re-verifies every blob's Blake3 and drops any
  tampered file.
* **Write-through.** `Room` accepts a node → verify signature → enforce
  policy → merge → persist, all before broadcast. Tick nodes signed by the
  server are persisted on the same path.
* **Hydrate.** `Room::new` reads nodes and blobs from disk *before* the
  room is visible to the registry. Bench: 10 000 nodes rebuild in well under
  the 5 s CI ceiling.
* **Room-id sanitization.** Non-`[A-Za-z0-9_-]` bytes are escaped as `_HH` so
  any UTF-8 room id maps to a safe filesystem path.
* **`ServerPersistence::is_durable()`.** Distinguishes `DirPersistence`
  (`true`) from the default `NoPersistence` (`false`). Consumed by the idle
  sweeper so in-memory rooms are never evicted.

### Idle-room eviction (F4 follow-up)

`--idle-timeout <seconds>` (default 300, `0` disables) runs a 60 s background
sweeper that drops rooms with zero connected peers past the timeout. Three
safety gates — all required — must all hold to evict:

1. `persistence.is_durable()` — in-memory rooms are never touched.
2. `Arc::strong_count(&Room) == 1` — nobody else holds a handle (prevents
   eviction mid-handshake).
3. `connected_peers.is_empty()` re-checked under the peers lock.

`Room` tracks an `idle_since: Mutex<Option<Instant>>`. `register_peer` clears
it; `deregister_peer` stamps `Instant::now()` when the last peer leaves.
Eviction aborts the tick loop and drops the broadcast sender; lagging
subscribers see `RecvError::Closed` which the SDK already handles as a
reconnect trigger. The next `get_or_create` rebuilds the room from disk —
identical path to a cold restart.

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
| `merge_10k_batch` | **31 ms** | <10 ms | `apply_remote_batch`: dedupe + parallel `verify_batch` (rayon, 64-node chunks) + session `verified_ids` cache. **~10× over the per-node path.** Canonical `Transaction::hash` is `postcard`-encoded (509 ns; was 1.07 µs with `serde_json`). Server `import_nodes` ingests packs through this path. |

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
| F | F0 WS token enforcement, F1a SDK `createDoc`, F1b WebRTC peer mesh in SDK, F2 Presence API, F3a client-side subscriptions, F3b server-side subscription filter, F4 `DirPersistence` (SQLite + file blobs), F4-follow-up idle-room eviction, F5 JWT bridge + Docker self-host | ✅ |
| Ops | tracing + RUST_LOG, batch verify (`verify_batch`, chunk=256) | ✅ |
| Future | CDN-backed `BlobStore::resolve_url` adapter; List CRDT (fractional index); RGA run compression; Peritext rich-text | open |

Everything described in the protocol stack is on disk and exercised by tests
and the live demo. Remaining open items are product-specific CRDT extensions
and CDN-offloaded blobs, not protocol-level gaps.

---

## 10. SDK surface (F1/F2/F3a)

`web/sdk.js` is the only API product teams are expected to touch. The
bridge `SyncStore` stays as the low-level escape hatch (`doc.store`).

```js
import { createDoc } from "./sdk.js";

const doc = await createDoc({
    room:        "room-1",
    serverUrl:   "wss://host/ws",
    subscribe:   ["world/**", "chat/**"],   // F3a glob filter
    transport:   "auto",                    // "auto" | "ws-only"
    iceServers:  [{ urls: "stun:stun.l.google.com:19302" }],
    roomSeed,    tokenCaps,                 // C3 / F5 auth
});

const world  = doc.map("world");
const intents = doc.map("intents");
const notes  = doc.text("notes/welcome");

world.set("player1", { x: 10, y: 20 });
notes.insert(0, "Hello");

doc.onChange(ev => {
    // ev.transport: 'ws' | 'webrtc'
    // ev.keys: Set<string>  — keys touched by the merged nodes
});

doc.presence.set({ name: "Brad", color: "#f43" });
doc.presence.others().forEach(peer => { /* ... */ });
doc.presence.onJoin(cb); doc.presence.onLeave(cb);

doc.peers();               // [{ pubkey, transport, syncReady, blobsReady, connectionState }]
doc.subscribe(["world/**"]); // re-scope; server is notified via subscribe msg (F3b)
```

Behaviors worth flagging:

* **WS is always the authoritative path.** WebRTC is additive per peer when a
  data channel is open; `import_pack` dedupes on node id so double delivery
  is free. Falls back to `ws-only` when `RTCPeerConnection` is undefined
  (Node, old browsers).
* **Reconnect.** Exponential backoff; session identity stable via
  `sessionStorage` seed; IndexedDB means zero-bytes rehydrate is possible.
* **Subscriptions affect materialization and the wire.** F3a filters what
  `MapHandle` exposes; F3b tells the server to stop relaying non-matching
  nodes. `TextHandle` construction throws if the key is outside the
  subscription.
* **Presence.** Side-channel, not in the DAG. Heartbeats every 15 s; peers
  swept after 45 s silence; leave events fire on `peer-left`, `clear()`, or
  staleness.
* **`doc.list()` deliberately throws.** List CRDT (fractional index) is
  scheduled post-F.

`activesync-jwt-bridge` (Rust) is the companion piece for hosted
deployments: your auth server mints a JWT with
`{ room, pubkey, exp, caps[] }`; the bridge verifies and hands back a signed
`RoomToken` that the SDK passes in `hello`.

---

## 11. Scaling & complexity

The engine was designed around "what's the `O(...)` of the hot path?"
Every user-visible operation has a sub-linear or amortized answer.

| Hot path | Complexity | Measured |
|---|---|---|
| Handshake wire size | **O(1)** in graph size (IBF is 2.9 KB fixed) | B1 |
| IBF decode | **O(diff)** with iteration cap | 888 ns / 1k diff |
| MST sync rounds | **O(depth of diff)**, branch-prune | 3 RTT / 1k diff in 2k tree |
| Local write | **O(1)** sign + `apply_local` | \~µs |
| Batch merge | **O(N / chunk)** parallel verify (rayon), `verified_ids` dedupe | 31 ms / 10k nodes |
| LWW resolve | **O(keys)** | 338 µs / 1k keys |
| Blob integrity | **O(bytes)** Blake3 | 9.2 µs / 50 KB |
| Delta broadcast | **O(new nodes since `sentToServer`)**, not O(graph) | \~µs per keystroke |
| Room hydrate (F4) | **O(nodes + blobs)** one-time at `get_or_create` | 10k nodes in < 5 s |
| Idle sweeper | **O(rooms)** every 60 s, read-locked | negligible |
| Subscription filter (F3b) | **O(ops in pack × patterns)** per broadcast | — |

What does **not** scale with graph size:

* Hello payload (IBF is a fixed 2.9 KB; the frontier is `O(tips)`, not
  `O(nodes)`).
* Per-keystroke bandwidth after handshake (delta broadcast, not full
  export).
* Memory footprint of an idle room with `--store` on (evicted; rehydrates
  lazily).

What scales linearly but cheaply:

* `merge_N_batch` is \~3 µs/node amortized (Ed25519 batch verify, SIMD).
* `replay()` re-verifies from bytes — same 3 µs/node floor.
* Blob persistence is one `rename` per blob (durable, idempotent).

What scales with the *set of writers* rather than graph size:

* Frontier size is O(concurrent writers). A 10k-node graph with 2 writers
  has frontier ≤ 2.
* Policy check is O(rules × ops in node).
* Presence is O(connected peers) and expires after 45 s silence.

Known `O(n)` costs we have not yet optimized:

* Per-node `apply_remote` is the legacy path (320 ms / 10k nodes). Every
  ingress path — `import_nodes`, catch-up pack, WS/RTC pack — uses
  `apply_remote_batch` instead; the serial path is kept only for tests.
* Ed25519 `verify_batch` is ~95% of `merge_10k_batch` wall-clock. Further
  gains require AVX-512 IFMA or a different signature primitive, both out
  of scope.

Bottom line: the engine handles a million-node graph because the
handshake is constant, the merge is parallel-batched, and the expensive
paths (full replay, Ed25519 verify) only run when you actually need them.

---

## 12. Known operational gaps

The protocol stack is feature-complete; these are the operational safeguards
and lifecycle pieces that remain open. Listed in the order we intend to ship
them. Each entry names the concrete surface change so the gap stays
falsifiable.

### G1 — Backpressure & slow-client policy *(SHIPPED)*

**Problem.** Pre-G1 the per-room `broadcast::channel(512)` silently dropped
lagged consumers (`RecvError::Lagged` was swallowed) and `sink.send()` had
no timeout — a stalled TCP write could wedge a WS task until the OS gave
up. Slow clients diverged silently until the next reconnect/IBF cycle.

**What shipped.**
- `ws_send` helper in [server/src/ws_handler.rs](server/src/ws_handler.rs)
  wraps every application-level `sink.send` in
  `tokio::time::timeout(Duration::from_secs(5), …)`; on timeout it
  increments `activesync_ws_send_timeout_total{room}`, emits a
  `1011 server overload` close frame (bounded by a 1s send timeout), and
  signals the caller to drop the peer.
- `RecvError::Lagged` arm now closes the WS with a `4001 resync required`
  custom close code and increments
  `activesync_broadcast_lagged_total{room}`. The SDK's exp-backoff
  reconnect path runs the normal recovery (hello → IBF diff → catch-up).
- `--broadcast-capacity <N>` CLI flag (default `512`); `0` is rejected
  (would panic `broadcast::channel`). Plumbed through `Rooms::new` →
  `Room::new` so every per-room channel uses the configured capacity.
  Tradeoff: larger = more slack for brief stalls; smaller = faster
  divergence detection.
- Metrics registered via `describe_counter!` in
  [server/src/metrics.rs](server/src/metrics.rs).

**Tests.** `server/tests/backpressure_lagged.rs` — boots the real axum
server on an ephemeral loopback port with `broadcast_capacity=2`,
connects a tungstenite client, drains the welcome, then floods 5 000
synchronous `room.tx.send(...)` calls with no `.await` between them so
the handler task stays parked while the size-2 ring buffer overflows.
Asserts the client receives a `Close { code: 4001 }` frame within 5s and
that `room.connected_peers` empties within 2s of the close. Deterministic
because `broadcast::Sender::send` is non-yielding.

### G2 — WebRTC mesh cap

**Where it lives today.** `makePeerMesh` in `web/sdk.js` opens two
`RTCDataChannel`s per remote peer, unconditionally. A 50-peer room =
O(N²) ≈ 2 450 channels per peer.

**Plan.**
- `createDoc({ maxMeshPeers: 24 })` option (default 24; `0` = WS-only).
- Mesh selection: sort peers by pubkey, keep the closest-hash K; everyone else rides WS. Deterministic and symmetric (both sides pick the same K).
- `doc.peers()` already surfaces `transport`; surface the reason (`'mesh-cap'` vs `'ws-only'` vs `'negotiating'`) too.
- Zero server change. Falls out of existing D2 fallback path.

### G3 — Rate limiting per peer

**Where it lives today.** Nothing. A well-formed malicious peer can spam
signed nodes and force Ed25519 batch work.

**Plan.**
- Token bucket per `PeerSession` over (a) accepted nodes/sec, (b) accepted bytes/sec. Use `governor` (already in the dep tree via axum).
- CLI: `--peer-rate-nodes <n/s>` (default 200), `--peer-rate-bytes <mib/s>` (default 4).
- Violation → close WS with `4008 rate limit exceeded`; metric increment `activesync_rate_limit_drops_total{peer}` (short-hash label).
- Super-Peer server key is exempt.

### G4 — Blob GC

**Where it lives today.** `DirPersistence` writes `blobs/<room>/<hash>`
forever. No refcount, no sweep. `Op::Map::SetBlob` can overwrite a previous
blob reference; the old file survives indefinitely.

**Plan.**
- Per-room sweep: walk current `resolve()` output for `SetBlob { blob_hash }` + walk every retained snapshot frontier → live set.
- Two-phase deletion: mark (write `blobs/<room>/.tombstones/<hash>` with timestamp) on first sweep; delete on next sweep if still orphaned *and* tombstone age > `blob_grace_secs` (default 24 h). Grace window absorbs concurrent uploads by offline peers.
- Opt-in: `--blob-gc-interval <secs>` (default 0 = disabled). Runs inside the idle sweeper's task budget so it inherits the durable-only gate.
- Safe because blobs are re-uploadable from any peer that still has the bytes (the same guarantee F4 already relies on).
- Metric: `activesync_blob_gc_deleted_total{room}`.

### G5 — Lamport ceiling & wall-clock sanity

**Where it lives today.** `apply_remote` accepts any `lamport: u64`. A
malicious peer publishing `lamport = u64::MAX` would win every LWW forever
and poison the room's Lamport clock to boot.

**Plan.**
- Reject nodes where `lamport > graph.lamport() + LAMPORT_SLACK` (const `1 << 20`). Legitimate concurrent-writer fanout stays well under this; anything larger is tampering.
- Soft-reject nodes with `wall_ms` more than 24 h past server `wall_ms` (log + drop). Doesn't affect correctness (wall_ms is informational), but closes a "sort me to the top of the display timeline" attack for apps that render by wall clock.
- Surfaces as `activesync_lamport_rejected_total{reason}`. No wire change; both checks live in `StateGraph::apply_remote` / `apply_remote_batch`.

### G6 — Token revocation / re-validation

**Where it lives today.** `RoomToken` is validated once on `hello`; the WS
stays open until `exp` or disconnect. A leaked token is valid for its full
expiry window (could be days).

**Plan, prefer (a) first:**
- **(a) Short-lived tokens + SDK refresh hook.** `createDoc({ getToken: async () => "…" })` refetches before expiry (10-minute default lifetime). Server disconnects at `exp` with `4002 token expired`. Zero protocol change, zero new state.
- **(b) Revocation list, if (a) is insufficient.** `ws_handler` periodically (`--revocation-poll <secs>`) checks an app-supplied endpoint; hits disconnect the WS. Adds a network dependency — only ship if a product actually needs it.

Metric: `activesync_token_expired_disconnects_total`.

### G7 — Metrics / observability (server) *(SHIPPED)*

**Where it lived before.** `tracing` only. No counters, no histograms, no
scrape endpoint.

**Shipped.**
- `metrics` 0.23 + `metrics-exporter-prometheus` 0.15 in `activesync-server`.
- Admin server on a **separate** port: `--metrics-addr 127.0.0.1:9090` (default off) exposes `/metrics`. Public WS port is untouched — metrics must not be internet-reachable by default.
- Global recorder installs exactly once per process; install failure is logged and the server keeps running without observability.
- Baseline set (cardinality-conscious: `room` label where it helps, `peer` label is a 12-char pubkey prefix):
  - `activesync_rooms_total` (gauge)
  - `activesync_peers_total{room}` (gauge)
  - `activesync_nodes_accepted_total{room}` (counter)
  - `activesync_merge_batch_seconds` (histogram, custom buckets 50µs–2.5s)
  - `activesync_persistence_write_seconds{kind=node|nodes_batch|blob}` (histogram, custom buckets 100µs–500ms)
  - `activesync_eviction_total` (counter)
- Histogram buckets set via `PrometheusBuilder::set_buckets_for_metric(Matcher::Full(...), &buckets)` so Grafana p50/p95/p99 queries work out-of-the-box.
- Remaining counters below are registered by their own gap at instrumentation time:
  - `activesync_broadcast_lagged_total{room}` — feeds G1
  - `activesync_ws_send_timeout_total{room}` — feeds G1
  - `activesync_rate_limit_drops_total{peer}` — feeds G3
  - `activesync_blob_gc_deleted_total{room}` — feeds G4
  - `activesync_lamport_rejected_total{reason}` — feeds G5
  - `activesync_token_expired_disconnects_total` — feeds G6

Integration test: `server/tests/metrics_endpoint.rs` installs the recorder on a loopback port, exercises room + peer + import paths, and scrapes `/metrics` via a raw HTTP/1.1 GET to assert the baseline series are present.

### G8 — Metrics surface in the SDK (thin, BYO backend)

**Rationale.** Adding a metrics exporter inside the WASM bridge pays a
bundle-size tax in every browser session and forces a backend choice the
engine has no business making. Host apps already ship analytics (Sentry,
Datadog RUM, PostHog, plain `performance.mark`); double-instrumenting is
waste.

**Plan.**
- `createDoc({ onMetric: (m) => … })` — pure JS callback, synchronous,
  zero deps.
- Emitted events (object shape stable, name string enumerated):
  - `{ name: 'merge_ms', value, room, peer, nodes }`
  - `{ name: 'lagged_broadcast', count, room }`
  - `{ name: 'rtc_channel_open' | 'rtc_channel_close', peer, channel }`
  - `{ name: 'blob_fetch_ms', value, hash, via: 'ws' | 'webrtc' }`
  - `{ name: 'reconnect', attempt, backoff_ms }`
- Tree-shaker drops the whole path when `onMetric` is absent.
- Mirrors server counter names where the concept overlaps (`lagged_broadcast` ↔ `activesync_broadcast_lagged_total`) so end-to-end dashboards are trivial.

### G9 — Conflict visibility (SDK)

**Where it lives today.** LWW is silent: caregiver A overwrites caregiver B
with no user-facing signal.

**Plan.**
- Extend `doc.onChange(ev)` with `ev.overwrote: Map<key, { prevAuthor, prevLamport, prevValue }>` — populated only when an accepted op displaced a non-tombstoned value written by a different author within the last `conflictWindowMs` (default 30 s).
- Pure SDK-side; reads `read_canonical(key)` before merge to capture the displaced value. No wire change.
- Opt-in via `createDoc({ conflictReporting: true })` to avoid the extra resolve cost for apps that don't need it.

### G10 — Schema migration guidance (docs-only)

**Where it lives today.** The engine is schema-free (`Op::Map` values are
opaque `Vec<u8>`). App authors have no written rules of the road.

**Plan.** One-page `docs/schema-migrations.md`:
- **Ops are forever.** Old nodes survive compaction only as state, not as replayable ops; old op *shapes* survive forever in pre-compaction history.
- **Additive changes only.** New optional fields in value JSON. Renames = `Set(new_key, value)` + `Set(old_key, tombstone)`.
- **Versioning escape hatch.** App-layer `version` field inside the value bytes. SDK does not interpret; app chooses forward/backward compat.
- **Deletion = tombstone, not absence.** Backed by LWW semantics.

No code change. This is the single piece of adoption friction product teams
will hit first.

### G11 — Field-level projection (deferred)

**Where it lives today.** Subscriptions filter by key path, not by fields
inside a value. A board with 100 buttons + 10 visible still syncs all 100
when the glob matches.

**Plan.** Do not ship until a real product hits the cap. Two forward paths,
both behind the existing glob layer:

1. **Path nesting (zero engine change).** Apps encode buttons as
   `board/<id>/buttons/<n>` and subscribe to the visible subset. Already works.
2. **Typed projection (engine change).** Requires a value schema + op-level
   sub-path support. Large. Schedule only when (1) is demonstrably
   insufficient.

### Sequencing summary

| Wave | Items | Why this bundle |
|---|---|---|
| **Operational safety** | G1, G7, G3 | Backpressure + metrics + rate limiting are load-bearing on every other operational claim. Can't diagnose G4/G5/G6 without G7. |
| **Before real users** | G4, G5 | Disk-bloat and Lamport-skew both produce garbage you can't clean up after the fact. |
| **Product polish** | G6, G8, G9, G10 | Revocation, SDK metrics hook, conflict surfacing, migration docs. All small, all app-adjacent. |
| **Never unless asked** | G2 (cap only), G11 | G2 is trivial if mesh scale ever bites; G11 is expensive and speculative. |

