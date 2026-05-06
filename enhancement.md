# ActiveSync Enhancement Tracker

Tracks planned and in-progress improvements beyond the shipped G1–G7 operational floor.
Each entry records the motivation, design decision, implementation surface, and status.

---

## E1 — S3-Backed Op-Log Replication *(in-progress)*

### What is it?

`activesync-s3-blobs` (F6) already ships a `BlobPersistence` impl backed by any S3-compatible
object store (AWS, Cloudflare R2, MinIO, GCS S3-compat, Azure via gateway). It handles **blob
bytes** only — audio, images, binary assets.

The gap: **node packs (the DAG op-log itself) are not replicated to object storage**. The
`DirPersistence` SQLite file is the only durable node backend. If the server crashes, SQLite
is recovered — but for multi-region or serverless deployments, SQLite-on-disk is not viable.

### What Fireproof does (inspiration)

Fireproof writes every mutation as a content-addressed "car file" (Merkle DAG block) to S3/R2.
The current "head" for each peer is a well-known S3 key (`heads/<room>/<pubkey>`). Other clients
poll that key to merge changes — the WS server is an optimization, not a correctness requirement.

### Our design

Extend the existing `NodePersistence` trait split (F6) with an `S3NodePersistence` impl in
`activesync-s3-blobs`:

```rust
// activesync-s3-blobs/src/node.rs
pub struct S3NodePersistence {
    store: Arc<AmazonS3>,
    config: S3NodeConfig,
}

// S3 key layout:
//   nodes/<room_id>/<node_id_hex>        — individual node blobs (postcard-encoded SyncNode)
//   heads/<room_id>/<pubkey_hex>         — latest frontier for each writer (postcard Vec<Hash>)
```

**Write path:** `persist_node` → PUT `nodes/<room>/<id>` (content-addressed, idempotent via
existing Blake3/Ed25519 guarantee) + update `heads/<room>/<pubkey>` with the new frontier.

**Read/hydrate path:** `load_room_nodes` → list `nodes/<room>/` (paginated S3 ListObjects) →
GET each → deserialize → return in insertion order (sort by node Lamport or fetch index).

**Offline-first consequence:** A new server instance can cold-start with zero local state and
hydrate entirely from S3. The WS server becomes stateless between restarts.

### Crates involved

All via the `object_store` 0.11 crate (already a dependency of `activesync-s3-blobs`):

| Provider | `object_store` feature | Cargo feature flag |
|---|---|---|
| AWS S3 | `aws` (already on) | default |
| Cloudflare R2 | `aws` (R2 is S3-compat, set endpoint) | `r2` alias |
| Google Cloud Storage | `gcp` | `gcs` |
| Azure Blob Storage | `azure` | `azure` |
| MinIO / local dev | `aws` (set endpoint to localhost) | — |

No additional crates needed. Flip on `object_store` feature flags in `Cargo.toml`:

```toml
# activesync-s3-blobs/Cargo.toml
[features]
default = ["aws"]
aws = ["object_store/aws"]
gcs = ["object_store/gcp"]
azure = ["object_store/azure"]
```

### Server CLI wiring (new flags)

```
--s3-nodes-bucket <bucket>     Enable S3 NodePersistence
--s3-nodes-prefix <prefix>     Key prefix (default "nodes/")
--s3-heads-prefix <prefix>     Heads prefix (default "heads/")
--s3-region <region>           Region (default "us-east-1")
--s3-endpoint <url>            Override endpoint (R2/MinIO/GCS)
```

When `--s3-nodes-bucket` is set, compose `(S3NodePersistence, S3BlobStore)` into
`Arc<(N, B)>` — the blanket `ServerPersistence` impl on tuple `(N: NodePersistence, B: BlobPersistence)`
already handles this.

### Head polling (optional SDK path)

For true serverless operation, expose a `GET /heads/<room>/<pubkey>` HTTP endpoint (no WS)
that returns the frontier pack. SDK can poll this at 30 s intervals when WS is unavailable,
merge the downloaded packs, and stay consistent without a live connection.

### Status: 🔲 Not started

---

## E2 — Fractional-Index List CRDT *(SHIPPED — architecture correction)*

> **Note:** The architecture doc (§9 Future, §10 `doc.list()`) states this is unimplemented.
> **It is fully shipped as F8.** This entry corrects the record.

### What shipped (F8)

- **Core:** `activesync-core/src/list.rs` — `FracIdx` (base-62 position strings), `between()`
  generator, `ListStore` resolver, `Op::List(ListOp)` with `Insert`, `Move`, `Delete`.
- **Bridge:** `list_insert_at`, `list_move_to`, `list_delete`, `list_length`, `list_ids_json`
  exposed on `SyncStore` via `wasm-bindgen`.
- **SDK:** `doc.list(key)` returns a `ListHandle` with `.insert(index, id, content)`,
  `.move(id, index)`, `.delete(id)`, `.items()`, `.length`, `.onChange(cb)`. **Does not throw.**
- **Concurrency:** `Move` is LWW on `(lamport, author)` — not delete+insert — so a concurrent
  content edit on the sidecar Map composes cleanly with a concurrent position change.
- **Rebalance:** `REBALANCE_THRESHOLD = 128` bytes. SDK is responsible for emitting rebalance
  ops; the core ships only the primitive.

### What is NOT done (gap vs. Loro's movable-tree)

Loro's "forest CRDT" handles **concurrent moves in a tree structure** (parent/child, not just
flat position), preventing cycles when two peers move a node under each other. Our `ListOp::Move`
is flat-list only. For button-grid reordering this is sufficient. For a nested folder/tree UI
it would not be.

### Status: ✅ Shipped (F8). Architecture doc updated (§9, §10, §12 Future list).

---

## E3 — App-Layer Undo Manager with Compensating Ops

### What is it?

CRDT-native undo: instead of rewinding the DAG (which would invalidate remote peers' causality),
an undo manager generates **new forward ops that invert the effect** of a previous local op.

This is how Yjs `Y.UndoManager` works and is the only approach compatible with a convergent DAG.

### Design

Lives entirely in `web/sdk.js` — zero engine changes.

```js
// Proposed API
const undo = doc.undoManager({
    scope: ['boards/**'],     // only track ops under these key prefixes
    captureTimeout: 500,      // ops within 500ms → single undo item (debounce)
    maxItems: 100,            // ring buffer
});

undo.undo();   // generates compensating Set/Delete ops for the most recent item
undo.redo();   // re-applies the inverted ops
undo.clear();  // flush history (e.g. on page navigate)

doc.onChange(ev => {
    // ev.origin: string | undefined — set by UndoManager to avoid tracking its own ops
});
```

**Origin tagging:** Each compensating op is committed with `origin: 'undo-manager'`. The
`onChange` handler checks `ev.origin` to skip adding undo-of-undo to the stack.

**What gets tracked:** Only ops authored by the local peer (`ev.author === doc.pubkey()`).
Remote ops from other peers are *not* undone — this is intentional and matches Yjs semantics.

**Map key undo:** For `Set(key, newVal)`, the undo item stores `(key, previousVal | TOMBSTONE)`.
Reading `previousVal` requires calling `read_canonical(key)` *before* committing the op — the
same moment-before-write snapshot already used by G9 conflict reporting.

**List op undo:** For `list_insert_at(key, idx, id)`, undo is `list_delete(key, id)`.
For `list_delete`, undo is `list_insert_at` at the previously-recorded position. For
`list_move_to`, undo is `list_move_to` back to the previous position.

**Blob ops:** Blob CAS references (`SetBlob`) are not reversed — the bytes stay in S3/IndexedDB.
Only the Map key pointing at the hash is reset. This is correct: the blob may still be
referenced by another key.

### Integration point (PWASoundboard)

The board editor (add/remove/move buttons, rename board) is the primary use case.
In `web-react/src/sync/activesync/boardRoom.ts`, wrap mutations with undo tracking:

```ts
const undo = boardDoc.undoManager({ scope: [`board/${boardId}/**`] });
// Then expose undo/redo via keyboard (Ctrl+Z / Ctrl+Y) in EditBoardPage
```

### Status: ✅ Shipped

**Implementation:** `makeUndoManager()` in `web/sdk.js`. Added `preMutationE` emitter
at `createDoc` scope; each mutation method fires it with the before-state needed for
compensation. The manager builds a ring buffer of undo groups (debounced by `captureTimeout`).
Exposed as `doc.undoManager({ scope, captureTimeout, maxItems })` returning an object with
`.undo()`, `.redo()`, `.commit()`, `.clear()`, `.destroy()`, `.undoDepth`, `.redoDepth`.

**What is tracked:** `map.set`, `map.delete`, `list.push`, `list.insert`, `list.move`,
`text.insert`. `list.delete` and `text.delete` are intentionally not reversible (CRDT tombstone
limitations). Multiple independent managers can coexist per `createDoc` instance.

---

## E4 — Incremental Snapshots (Configurable Checkpoint Depth)

### Problem

Current compaction (D3) always produces a **single monolithic snapshot** that subsumes the full
pre-compaction DAG. Cold-start hydration is then `snapshot_node + recent_delta`. This is fine
until the "recent delta" itself grows large (long-running rooms with no compaction). There is
currently no way to compact incrementally — only all-at-once.

### Design: chained checkpoints

Extend the compaction snapshot op to carry an optional `base_snapshot_id`:

```
"\x00snap:hash"      32 bytes   canonical hash of state at this checkpoint
"\x00snap:front"     frontier IDs subsumed by this checkpoint
"\x00snap:base"      (optional) NodeId of the prior checkpoint this one builds on
```

Cold-start hydration chain:
```
base_snap_0  →  incremental_snap_1  →  incremental_snap_2  →  recent_delta
```

Each incremental snapshot only covers the nodes *since the previous checkpoint*. A peer that
has `snap_1` locally only needs to fetch `snap_2 + recent_delta`, not replay from genesis.

### Configurable depth (the "N" you asked about)

Server CLI flag:
```
--snapshot-interval <N>      Compact after N new nodes (0 = disabled, default 0)
--snapshot-max-chain <K>     Collapse chain when it reaches K increments (default 10)
```

When the chain reaches `K`, the server emits a **full consolidating snapshot** (same as
current D3) that subsumes all prior incremental checkpoints, resetting depth to 0.

Clients can also request compaction explicitly via a signed `\x00compact-request` op, which
the Super-Peer validates before acting.

### What the client sees

`StateGraph` hydration order:
1. Check for any snapshot node with `\x00snap:base` that chains back to local state → only
   fetch and apply the missing incremental segments.
2. Fall back to current behavior (full snapshot + delta) if no matching chain found.

### No user-facing knob needed on the SDK side

The SDK hydrates whatever the server sends — checkpoint chaining is transparent. The only
user-visible knob is `--snapshot-interval N` on the server.

### Status: ✅ Shipped

**Implementation:**
- `activesync-core/src/compaction.rs`: Added `SNAP_BASE_KEY` constant, `compact_incremental()`
  function (full snapshot + `\x00snap:base` chain pointer op), updated `SnapshotMeta` to
  include `base_id: Option<NodeId>`, updated `verify_snapshot()` to extract the optional chain
  pointer. Re-exported new symbols from `lib.rs`.
- `activesync-server/src/room.rs`: Added `spawn_snapshot_sweeper()` — wakes every 30 s,
  checks node-count delta per room, emits incremental or full snapshots based on `max_chain_depth`,
  broadcasts via the room's broadcast channel, and persists via `room.persistence.persist_node()`.
  Tracks `activesync_snapshot_total{kind=incremental|full,room}` counter.
- `activesync-server/src/main.rs`: Added `--snapshot-interval <N>` and `--snapshot-max-chain <K>`
  CLI flags (both default 0 / 10). Added `parse_usize_flag()` helper. Added `Arc` import.

---

## E5 — Conflict Visibility in `onChange` (G9, product polish)

*(Already designed in §12 G9 of ARCHITECTURE.md — reproduced here for tracking.)*

**Plan:** `ev.overwrote: Map<key, { prevAuthor, prevLamport, prevValue }>` in `onChange`.
Opt-in via `createDoc({ conflictReporting: true })`.

**PWASoundboard use case:** Surface a toast "Your change was overwritten by another device"
when a concurrent board edit wins by LWW.

### Status: 🔲 Not started

---

## E6 — SDK Metrics Hook (G8, product polish)

*(Already designed in §12 G8 of ARCHITECTURE.md — reproduced here for tracking.)*

**Plan:** `createDoc({ onMetric: (m) => … })` — synchronous callback, zero deps.

**PWASoundboard use case:** Feed `merge_ms` and `reconnect` events to PostHog or a custom
dashboard to detect poor network conditions on caregiver devices.

### Status: 🔲 Not started

---

## E7 — Schema Migration Guide (G10, docs-only)

*(Already designed in §12 G10 of ARCHITECTURE.md — reproduced here for tracking.)*

One-page `docs/schema-migrations.md` explaining additive-only changes, tombstone semantics,
and the version-escape-hatch pattern.

### Status: 🔲 Not started

---

## Deferred / Not pursuing

| Item | Reason |
|---|---|
| **Loro-style movable tree CRDT** | Flat `ListOp::Move` sufficient for button grid; tree cycles not a current problem |
| **BLE / WiFi Direct mesh (Ditto-style)** | Platform native APIs required; WS+WebRTC covers the use cases |
| **CAR file export / IPFS** | IPFS-specific; CAS blobs in S3 serve the same offline-first goal without the IPFS stack |
| **Peritext rich-text marks** | No collaborative rich text in AAC soundboard; RGA per-char text already overkill |
| **Relative positions / cursor sync** | Presence API already handles cursor position as presence state; no richer protocol needed |
| **Sub-documents / nested rooms** | Room-references (rooms that reference other rooms) already handled by the room isolation model |
| **G2 WebRTC mesh cap** | Trivial if a >24-peer room ever appears; skip until then |
| **G11 Field-level projection** | Path nesting (`board/<id>/buttons/<n>`) already works as a workaround |

---

## What CAR files / IPFS are (for reference)

**IPFS** (InterPlanetary File System) is a peer-to-peer content-addressed storage network.
Data is identified by its CID (Content Identifier = multihash of the bytes), not its location.

**CAR files** (Content Addressable aRchives) are the transport format for IPFS blocks — a binary
container of `{ CID → bytes }` pairs, used to batch-export or import IPFS DAG nodes. Fireproof
uses them to serialize its Merkle CRDT state to S3/R2 in a format that IPFS tools can read.

**Why we don't need them:** ActiveSync already uses Blake3 CAS for blobs and Ed25519-signed
`SyncNode` packs (postcard-encoded) for DAG state. This gives the same content-addressable
correctness guarantee as IPFS CIDs, without the IPFS stack, CID multiformats, libp2p, or
UnixFS overhead. The `activesync-s3-blobs` crate already achieves "write to S3, verify on
read" — the same correctness property CAR exports give to Fireproof users.

---

## Priority sequencing

| Priority | Item | Why |
|---|---|---|
| **P1** | E1 — S3 op-log replication | Unblocks serverless deploy; SQLite-on-disk is the main scaling constraint |
| **P2** | E3 — Undo manager | High caregiver value; zero engine changes; `web/sdk.js` only |
| **P3** | E4 — Incremental snapshots | Caps cold-start cost as room history grows; pure server-side |
| **P4** | E5 — Conflict visibility | Low effort; feeds directly into PWASoundboard board-edit UX |
| **P5** | E6 — SDK metrics hook | Small; enables operational dashboards |
| **P6** | E7 — Schema migration docs | No code; just write it |
| **Skip** | E2 (already shipped) | Architecture doc corrected |
