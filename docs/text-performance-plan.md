# Text Throughput Improvement Plan

Date: 2026-07-02

> **Status update (2026-07-02, same day):** Phases 1-3 (as a single coherent
> rewrite of `TextProjection` around a chunked order-statistic item list) and
> Part II M1-M4 are **implemented and merged into the working tree**. Measured
> results are in `benchmarks/benchmarks.md` ("Text engine rewrite results"):
> 50k ops 5.5k → 214k ops/sec, 150k ops ~455 → 72k ops/sec, full ~980k-op trace
> from impractical (10+ hours est.) to 46.6 s with exact `endContent`
> convergence; engine-side apply cost near-flat (~2.4 → ~4.2 µs/op from 50k to
> 980k). No public API, FFI, or wire changes. Remaining open items: Phase 0
> scripted regression gates, Phase 4 S4.2/S4.4 (client range-op adoption,
> snapshot cold-start), Phase 5 dmonad-parity table, and the optional M5 blob
> transport work.
>
> **S4.3 done (2026-07-06):** the bridge per-keystroke fast path landed as an
> O(log n) Offset-anchor canonicalization in `apply_local_text_range_op`
> (projection `id_at_visible_offset` instead of full-sequence
> materialization). Browser full-trace B4: unsigned 78.83 s → 2.16 s
> (8.3 µs/op), signed 88.86 s → 9.76 s (37.6 µs/op); see
> `benchmarks/benchmarks.md` "[B4] Browser/WASM parity".
Context: benchmarks/benchmarks.md "Text throughput, wire cost, and cold-start convergence
(v1 spike)" — unbatched apply collapses from 384,947 ops/sec (5k ops) to 455 ops/sec
(150k ops); mean wire cost is ~209 bytes per character op. yjs, diamond-types, and
automerge replay the same class of editing trace at hundreds of thousands to millions of
ops/sec with amortized wire cost near 1 byte/char. This plan explains the gap
structurally (not as tuning) and phases the fixes so the FFI/host contracts and RGA
merge semantics stay unchanged throughout.

## 1. Why they are faster: the four structural differences

### 1.1 Runs/spans are their primary unit; ours is the single character

- **yjs** (`yjs/src/structs/Item.js`): one `Item` covers an entire contiguous typing
  burst (`ContentString` holding many chars). IDs are `(client, clock)` with consecutive
  clocks inside a run, so a 40k-char paste is ~1 object, split lazily only when a
  concurrent edit lands inside it.
- **diamond-types** (`src/rle`, `src/listmerge`): run-length encoding pervades every
  structure — oplog entries, causal graph, tombstones, the merge engine's spans
  (`listmerge/yjsspan.rs`).
- **automerge** (`rust/automerge/src/op_set2`, `columnar`): columnar op storage;
  sequence ops compress into runs per column.
- **nodalmerge** (`core/src/text.rs`): `entries: HashMap<CompactId, ProjectionEntry>` is
  one hash entry **per character**, `children: HashMap<CompactId, Vec<CompactId>>` one
  bucket per parent char, `visible_ids: Vec<CompactId>` one 16-byte element per char,
  `visible_pos_by_id: HashMap<CompactId, usize>` one entry per char. `TextRun` exists
  but only as a *secondary cache* layered on top of the per-char structures.

Our `OpId = (lamport, author)` with consecutive lamports inside a transaction is
*exactly* the same shape as yjs's `(client, clock)` — so run-primary storage is fully
compatible with our per-char OpId contract: any char's identity is
`(actor, run.seq_start + offset)`. Nothing in the FFI needs to change.

### 1.2 One O(log n) position structure; we maintain five parallel O(n) ones

- **diamond-types** (`src/ost/content_tree.rs`): a single order-statistic B-tree keyed
  by current visible position with subtree char counts. Position→item and item→position
  are both O(log n); insert/delete are O(log n); cache-friendly node layout.
- **yjs**: doubly-linked Item list plus *search markers* (`findMarker` in
  `src/ytype.js`) — a small cache of recent (position, item) pairs exploiting edit
  locality, so typical lookups walk a few items.
- **nodalmerge** maintains, per edit, up to five structures each with its own O(n) cost:
  1. `visible_ids: Vec` — `insert(idx, …)` is an O(n) memmove per op.
  2. `visible_string: String` — `String::insert` is an O(n) memmove **plus**
     `char_to_byte_offset` is an O(n) char scan (`text.rs:1279-1290`).
  3. `visible_pos_by_id: HashMap` — after every non-append insert/delete we iterate the
     **entire map** to shift positions (`text.rs:850-854`, `900-904`). At 150k chars
     this alone is ~150k hash-bucket visits per keystroke. This is the single biggest
     killer in the unbatched profile.
  4. `visible_runs: Vec<TextRun>` — located by **linear scan**
     (`locate_run_offset_for_insert`, `text.rs:1109`) even though…
  5. `position_index` (Fenwick) exists and supports O(log n) `select_offset`, but is
     **rebuilt from scratch** (O(runs)) on any run split/merge
     (`refresh_run_index_incremental`, `text.rs:1257`).

### 1.3 Concurrent-insert integration is neighborhood-local; ours walks the whole tree

- **yjs** `Item.integrate`: finds the insertion point by scanning only the conflict
  window between left and right origin — typically 0–2 items even under heavy
  concurrency.
- **diamond-types** `listmerge/merge.rs`: same idea over spans with the range tree.
- **nodalmerge**: any insert that is not a tail append calls
  `compute_visible_index_for_id` (`text.rs:1293`) — a **full pre-order DFS of the
  entire RGA forest from the roots** to find one id's visible index. O(n) per op, with
  hash lookups at every step. The batched path is no better: `apply_updates_coalesced`
  (`text.rs:677`) applies metadata then does one `ensure_materialized` **full rebuild**
  (full DFS + rebuild string + rebuild pos map + rebuild runs + rebuild Fenwick) per
  batch — O(doc) per transaction, hence still superlinear over a trace (Finding 2, and
  why batched is only 731 ops/sec at 150k).

### 1.4 Wire/DAG unit is the transaction; ours is effectively the character

- **yjs/diamond-types encoding**: var-int, run-length update encoding. A sequential
  typing run costs its content bytes plus a few bytes of header; dmonad's B4 trace
  (~260k ops) encodes to ~100-160 KB. Neither hashes nor signs per op.
- **nodalmerge**: the benchmark (and the current client pattern) emits **one `SyncNode`
  per character**: 32-byte author + 32-byte parent hash + node hash + `key` String
  cloned per op + lamport + wall_ms ⇒ ~209 bytes and one blake hash **per keystroke**;
  980k nodes for the full trace, all replayed on cold start. `InsertRange`/`DeleteRange`
  ops already exist (`op.rs`, bridge `insert_text_range`) — one node per paste — but
  (a) the batch projection path demotes them to the per-node fallback
  (`fallback_range_keys`, `graph.rs:1017-1022,1033-1036`), and (b) internally
  `apply_range_insert` still expands to per-char `apply_insert` calls.

Per-node hashing/signing is a deliberate integrity feature yjs/DT don't pay for — we
should not remove it. But at one node per *transaction* (range ops) instead of per
character, its cost amortizes to noise. That is the correct framing for "why we'll
never match diamond-types exactly, and why we don't need to."

## 2. What is *not* viable / not worth copying

1. **Dropping per-char stable identity** — our FFI/bridge contract exposes per-char
   `OpId` (`resolve_text_seq_json`, `delete_text`, `TextRangeAnchor::After(id)`).
   Fine: run-primary storage preserves it (§1.1). No contract break needed.
2. **Dropping the hash-linked DAG / signatures** — core product property (auth,
   audit, replay-to-lamport). Keep; amortize via transaction-level nodes instead.
3. **Adopting yjs's unsorted-sibling + origin-pair (YATA) semantics wholesale** — our
   RGA sorts siblings descending by OpId and ordering is part of convergence semantics
   already shipped. We keep RGA ordering; we only change *how fast* we compute it.
   `TextProjectionMode::ParityCheck` is our oracle that the rewrite is
   order-identical.
4. **automerge-style columnar storage** — optimizes memory/serialization more than
   apply latency; large lift, lower payoff than §1.2/§1.3 for our stated gap. Revisit
   only if snapshot size becomes the pain point.

## 3. Phased plan

Every slice keeps: existing public API (`resolve_text*`, `apply_local*`,
`apply_remote*`), FFI ABI, wire op enum, and ParityCheck-vs-legacy equality. Each phase
ends with the throughput test re-run at 5k/50k/150k and numbers appended to
benchmarks/benchmarks.md.

### Phase 0 — Baseline & oracle hardening (small)

- S0.1: Promote `core/tests/text_throughput_and_convergence.rs` numbers into a scripted
  baseline (5k/50k/150k, unbatched+batched, JSON out via
  `NODALMERGE_TEXT_THROUGHPUT_METRICS_PATH`) so each phase has an A/B artifact.
- S0.2: Add a randomized two-peer concurrent-edit fuzz test that asserts
  projection == legacy replay (extends ParityCheck coverage beyond sampling), so
  Phases 1–3 refactors can't silently change merge order.

### Phase 1 — Kill the O(n)-per-op hot spots in place (no structural redesign)

Target: unbatched 150k from ~455 → tens of thousands ops/sec. All changes inside
`TextProjection`; no signature changes.

- S1.1: Delete the `visible_pos_by_id` full-map shift loops (`text.rs:850-854`,
  `900-904`). Replace position lookup (`index_of_visible_id`, `offset_for_anchor`) with:
  per-actor sorted interval map `(lamport_start, len) → run_idx` + Fenwick prefix sum.
  O(log n), no per-op maintenance of a per-char map. Then delete `visible_pos_by_id`.
- S1.2: Use the Fenwick `select_offset` in `locate_run_offset_for_insert/existing`
  instead of the linear run scan.
- S1.3: Make Fenwick maintenance incremental for run split/insert/remove (shift-tail
  point updates or a rebuild-threshold hybrid) so `refresh_run_index_incremental` stops
  being O(runs) per structure change.
- S1.4: Stop eagerly maintaining `visible_string` (O(n) memmove + O(n) char scan per
  op). Make reads materialize from `visible_runs` on demand with a dirty flag;
  `resolve_string_cached` becomes "rebuild if dirty, else cached". Range reads
  (`resolve_string_range`) read directly from runs via the index instead of
  `.chars().skip(start)`.
- S1.5: Route `apply_updates_coalesced` through the incremental `apply_insert`/
  `apply_delete` path instead of metadata + full `ensure_materialized` rebuild; keep the
  rebuild only as the correctness fallback. This removes the O(doc)-per-transaction
  rebuild that makes even the batched path superlinear, and fixes the "batching is
  slower at 50k" anomaly (Finding 1) at the same time.

### Phase 2 — Runs become the primary store (memory + constant factors)

Target: ~10-20x memory reduction on large docs; big constant-factor apply win;
`visible_ids` per-char Vec goes cold.

- S2.1: Replace per-char `entries`/`children` with run-level metadata: an insert run is
  `(actor_idx, lamport_start, text, parent_anchor)`; split lazily when a concurrent
  edit lands inside a run (the yjs `Item.splice` move). Sibling order stays
  "descending OpId of run head" — identical RGA order, proven by S0.2/ParityCheck.
- S2.2: Tombstones stay range-based (`TombstoneStore.spans_by_actor` already is); drop
  the per-char `deleted_ids: HashSet` in favor of span binary-search `contains`.
- S2.3: Derive `visible_ids`/`resolve_seq` on demand from runs (only the JSON/debug
  paths need per-char pairs); remove the last per-char hot-path structure.

### Phase 3 — O(log n) integration and position index

Target: flat per-op cost at any doc size; unbatched and batched both in the
hundreds-of-thousands ops/sec range at 150k+; full 980k trace replay in seconds.

- S3.1: Replace `Vec<TextRun>` + Fenwick with an order-statistic B-tree over runs
  (subtree char counts; position↔run O(log n), split/insert O(log n)). Pattern:
  diamond-types `src/ost/content_tree.rs`. Consider vendoring a minimal version rather
  than depending on their crate.
- S3.2: Replace `compute_visible_index_for_id`'s full-forest DFS with local
  integration: locate the `after` parent via the per-actor run map O(log n), then scan
  only the parent's sibling window in descending-OpId order to place the new run.
  Delete the `mark_dirty → ensure_materialized` fallback from the insert path (keep it
  only for `from_nodes` cold builds).
- S3.3 (optional, evaluate after S3.1): store run text in a rope (jumprope/ropey) if
  string materialization for huge docs still shows up in profiles; likely unnecessary
  once reads are on-demand (S1.4).

### Phase 4 — Wire, DAG, and cold-start efficiency

Target: bytes/op from ~209 → ~content-size amortized; cold start bounded by snapshot
size, not history length. No wire-enum changes — the ops already exist.

- S4.1: Make range ops first-class in the batch projection path: translate
  `InsertRange`/`DeleteRange` into run-level projection updates instead of the
  per-node `fallback_range_keys` slow path (`graph.rs:1031-1044`), and make
  `apply_range_insert` insert one run instead of looping per-char `apply_insert`.
- S4.2: Client/SDK/benchmark pattern: one transaction (one SyncNode) per editor
  transaction using range ops — bridge `insert_text_range`/`delete_text_range` already
  exist; sdk-js and the benchmark runners should default to them. Re-measure
  bytes/op — expect ~two orders of magnitude reduction for typing bursts.
- S4.3: Local-edit fast path in the bridge: `insert_text` currently calls
  `resolve_text_seq_with_chars` (full per-char Vec build) per keystroke to compute
  `after`; use the projection's `anchor_for_offset` / run index instead.
- S4.4: Cold-start via snapshot: wire the existing compaction/D3 snapshot-node
  machinery (`core/src/compaction.rs`) into the text path so a fresh peer applies
  checkpoint + tail instead of 980k nodes. This is the answer to cold-start
  convergence, not faster full replay.

### Phase 5 — Benchmark parity & regression gates

- S5.1: Port the dmonad B4 methodology properly (same trace semantics, same metrics)
  and publish a nodalmerge vs yjs vs diamond-types vs automerge table with the
  signed/unsigned and per-char/per-txn axes called out — our integrity features are a
  legitimate column, not an excuse.
- S5.2: Turn S0.1 numbers into CI thresholds (e.g. 150k-op batched replay must stay
  under N seconds; bytes/op under M for the range-op path).

## 4. Expected end-state honestly stated

- Phases 1–3 remove every O(doc)-per-op term; apply cost becomes O(log n) with small
  constants. Same-order-of-magnitude throughput as diamond-types is *not* the target
  (they've spent years on cache layout and have no DAG/policy/hash work per node);
  100k–500k+ ops/sec sustained at large doc sizes, flat in doc size, is realistic.
- Phase 4 fixes the 209 bytes/op and cold-start story, which for a sync product
  matters more than raw local apply speed.
- Merge semantics (RGA descending-OpId sibling order), per-char OpId identity, FFI ABI,
  wire op enum, and host contracts are unchanged throughout; ParityCheck + the S0.2
  fuzz test are the guardrails proving it.

---

# Part II — Maps, Lists, Blobs, and Host Ingest Paths

Date: 2026-07-02
Same disease as text, simpler cure: every map/list/blob read (and one hot write path)
replays the **entire DAG** instead of consulting an incrementally maintained view. LWW
and fractional-index semantics are monotone (winners only improve, deletes are
absorbing, nodes are never removed outside compaction), so exact incremental caches are
straightforward — unlike text, there is no ordering/integration problem to solve. None
of this needs contract changes.

## 5. Findings

### 5.1 Map reads are O(total history), every time

`resolve_inner` (`core/src/graph.rs:1515`) walks **every node and every op in the
graph** per call, cloning each op's key `String` and the winning values.

- `read_speculative(key)` / `read_canonical(key)` (`graph.rs:1504-1511`) build the
  **entire** resolved map — clone every winning value in the room — then `.remove(key)`
  to return one value.
- The server tick loop (`process_tick`, `server/src/room.rs:283`) calls `resolve()`
  **per tick** to collect intent keys.
- `resolve_with_meta` (bridge `resolve_json_with_meta`) additionally base64-encodes
  every blob's full bytes on every state read.

So per-op cost for map workloads grows linearly with room history — the same
superlinear session curve text has, just with a smaller constant. The benchmark matrix
(60 map ops × 3 iterations × N peers per row) partially hides this because rooms are
short-lived; long-lived rooms will not be.

### 5.2 Host ingest runs a full-history conflict scan per write batch

`HostCommand::ImportPack` (`host-core/src/engine.rs:1731`) calls
`graph.detect_conflicts()` after **every** imported pack. `detect_conflicts`
(`core/src/conflicts.rs:101`) does two full passes over all nodes (map pass + list
pass) with a per-op `key.clone()` and winner-value clones, then sorts. That is
O(history) work on the hottest ingest path of the .NET host — the
`dotnet-host-runtime-alias` benchmark target pays this on every batch, and it grows
with room age. This is very likely a measurable slice of the DotNet-vs-Rust gap in the
existing matrix rows and will dominate as sessions lengthen.

### 5.3 List reads replay all nodes, several times per bridge operation

`resolve_list` (`graph.rs:1864`) collects all node ids + refs and replays every list op
for one key. The bridge calls it up to twice per user-visible list operation (compute
neighbors for `between`, then re-read after commit; `bridge/src/lib.rs:648-717`), plus
`list_len`/`list_ids` each doing their own full replay.

### 5.4 Blob-adjacent scans

- `referenced_blob_hashes` (`graph.rs:1605`) — full DAG scan per call
  (`bridge/src/lib.rs:863`, used to plan blob fetches).
- Blob *data* transfer itself (4 KB payloads at ~350-390 ops/sec in the matrix) is
  transport-dominated (base64 + JSON envelope + ws frame + persist); the DAG only
  carries the 32-byte hash, which is the right design. The earlier auth-overhead
  sensitivity on blobs (+6-8%) is a separate axis (per-op token checks), not storage.

### 5.5 Not problems (checked, fine as-is)

`insert_node` is O(parents) and cheap; `merkle_root` is O(leaves·log); `missing_hashes`
is per-handshake not per-op; MST/IBF sync structures are off the per-op path.

## 6. Plan — Phase M (can run independently of text Phases 1-4)

All slices are internal to `StateGraph`/host-core; public signatures, FFI ABI, and wire
formats unchanged. Compaction rebuilds caches from the rebuilt graph (it already
replays), so D3 snapshots stay correct.

- **M1 — Incremental LWW map view.** Maintain two per-key winner maps
  (speculative and canonical: `key → (lamport, author, value-or-tombstone, is_blob)`)
  updated in the apply paths: `apply_local` updates speculative only; `apply_remote*`
  updates both. Monotonicity argument: `(lamport, author)` is a total order, winners
  only improve, `local_node_ids` (`graph.rs:246,763`) only grows, nodes are only
  removed by compaction (which rebuilds). `resolve/read_*` become map lookups;
  `resolve_with_meta` reads the same cache. O(1) per op, O(keys) memory (values are
  already small or 32-byte blob hashes). This also makes the server tick loop
  O(intent keys) instead of O(history).
  *Guardrail:* keep `resolve_inner` as the oracle; add a sampled parity mode like
  text's `ParityCheck`, plus a fuzz test (random local/remote interleavings, cache ==
  replay).
- **M2 — Incremental conflict detection.** Only ops in the incoming batch can create
  new conflicts. Given M1's winner cache: for each batch op, compare against the
  cached winner — a losing op from a different author emits a conflict; a new winner
  demotes the old winner into a conflict event. O(batch) per ImportPack instead of
  O(history). Host-core's fingerprint dedup (`engine.rs:1742`) already makes delivery
  semantics identical. Keep full `detect_conflicts()` as the on-demand/debug API.
- **M3 — Incremental list projection.** Per list key:
  `HashMap<ItemId, (FracIdx, lamport, author)>` + absorbing tombstone set + a sorted
  visible structure (`BTreeSet<(FracIdx, ItemId)>` — fractional indexing means there is
  no integration problem, just ordered-set maintenance, O(log n) per op). Bridge list
  ops stop paying O(history)×2; `list_len`/`list_ids` become O(1)/O(n) reads. Same
  parity-oracle guardrail against `resolve_list_seq`.
- **M4 — Incremental `referenced_blob_hashes`** — a `HashSet<Hash>` maintained on
  apply. Trivial once M1 exists (SetBlob already flows through it).
- **M5 — Blob transport (optional, measure first).** If blob ops/sec matters beyond
  the matrix numbers: binary ws frames for blob payloads instead of base64-in-JSON
  (~33% wire cut + encode/decode CPU), and skip re-encoding blob bytes in
  `resolve_json_with_meta` unless explicitly requested. This is the only Part II item
  that touches a wire surface, and it's additive (new frame type, old path retained).
- **M6 — Bound history in RAM/cold-start (shared with text S4.4).** Long-lived rooms
  keep every per-op node resident; the compaction/D3 snapshot machinery already exists
  — wiring it into host residency + cold-start applies to map/list/blob rooms
  identically.

## 7. Where breaking contracts *would* buy real gains (offered, not assumed)

1. **Multi-op transactions as the default SDK write unit** — not actually a contract
   break (`Transaction.ops` is already a Vec), but a client-pattern change: batching N
   map sets per node amortizes hash/sign/ws-frame cost N×. The benchmark matrix
   currently measures one-op-per-node clients.
2. **Confirmation semantics for canonical reads** — today a locally-authored node is
   *never* canonical on its own peer (`local_node_ids` never shrinks; authoritative
   servers re-sign under a new id). If peers are ever expected to see their own writes
   confirmed by echo of the *same* node id, that's a semantic change worth making
   deliberately — it would also simplify the canonical cache to one map + a diff set.
3. **Dropping per-node `wall_ms`/parents duplication in packs** — batch frames with
   delta-encoded parents/author (text S4.2 adjacent). Saves wire, needs a pack-format
   version bump; only worth it after range ops land, since range ops already remove
   the dominant per-char node overhead.

## 8. Expected outcome

M1-M4 convert every map/list/blob read and the host ingest path from O(history) to
O(op)/O(log n), flattening per-op cost in room age — for the benchmark matrix this
mostly protects the *long-session* regime the current short rows don't exercise, and
removes the .NET host's per-batch conflict-scan tax immediately. Combined with Part I,
the only remaining O(history) work anywhere in the hot paths is cold-start replay,
which M6/S4.4 bounds by snapshot size.
