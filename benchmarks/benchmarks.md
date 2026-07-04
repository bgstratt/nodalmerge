# Benchmark Reality Check (Actionable)

Date: 2026-05-22
Purpose: fast way to decide whether auth/guardrails are acceptable overhead or a regression.

## What this benchmark actually exercises

1. Targets: `rust-combined-server`, `dotnet-host-runtime-alias`
2. Peer counts: `2`, `10`, `20`
3. Iterations: `3` per peer count
4. Command mix per iteration: `mapOps=60`, `listOps=60`, `blobOps=20`, `blobSizeBytes=4096`
5. Transport: `ws-only`

Per row, that is 18 scenario cells (2 targets x 3 peer counts x 3 iterations).

## Alternating sample used for this note

Sequence used: baseline/auth/baseline/auth (A-B-A-B)

Artifacts:

1. `benchmarks/results/benchmark-matrix-baseline-a-20260522-135151.json`
2. `benchmarks/results/benchmark-matrix-auth-b-20260522-135151.json`
3. `benchmarks/results/benchmark-matrix-baseline-c-20260522-135151.json`
4. `benchmarks/results/benchmark-matrix-auth-d-20260522-135151.json`

## Drift-aware summary (average of two baseline vs two auth runs)

Interpretation: positive percent means auth-enforced is slower.

### Aggregate (cross-peer combined)

1. DotNet: map `677.84 -> 710.23` (`+4.78%`), list `937.19 -> 983.58` (`+4.95%`), blob `348.93 -> 376.63` (`+7.94%`)
2. Rust: map `654.51 -> 666.95` (`+1.90%`), list `889.60 -> 907.50` (`+2.01%`), blob `362.72 -> 385.57` (`+6.30%`)

### Per-peer deltas (auth vs baseline)

1. DotNet 2 peers: map `+3.68%`, list `+5.51%`, blob `+7.31%`
2. DotNet 10 peers: map `+2.96%`, list `+3.59%`, blob `+1.60%`
3. DotNet 20 peers: map `+7.61%`, list `+5.69%`, blob `+13.36%`
4. Rust 2 peers: map `+0.52%`, list `+1.63%`, blob `+15.25%`
5. Rust 10 peers: map `+2.14%`, list `+1.74%`, blob `+0.75%`
6. Rust 20 peers: map `+2.82%`, list `+2.49%`, blob `+6.51%`

## So, rocketship or push cart?

Current evidence says not a push cart.

1. Map/list overhead is modest (roughly `+2%` to `+5%` overall, depending on host).
2. Blob overhead is the most sensitive axis (roughly `+6%` to `+8%` overall; occasional higher per-peer spikes).
3. No catastrophic cliff observed under 20-peer load in this sample.

## How to run this again (same method)

```powershell
$ts = Get-Date -Format "yyyyMMdd-HHmmss"
pwsh -File .\benchmarks\Run-BenchmarkMatrix.ps1 -RowIds baseline-default-auth -OutputPath ".\benchmarks\results\benchmark-matrix-baseline-a-$ts.json"
pwsh -File .\benchmarks\Run-BenchmarkMatrix.ps1 -RowIds auth-enforced-room-lock-tokened -OutputPath ".\benchmarks\results\benchmark-matrix-auth-b-$ts.json"
pwsh -File .\benchmarks\Run-BenchmarkMatrix.ps1 -RowIds baseline-default-auth -OutputPath ".\benchmarks\results\benchmark-matrix-baseline-c-$ts.json"
pwsh -File .\benchmarks\Run-BenchmarkMatrix.ps1 -RowIds auth-enforced-room-lock-tokened -OutputPath ".\benchmarks\results\benchmark-matrix-auth-d-$ts.json"
```

## Action thresholds (quick decision guide)

Use these until we replace them with formal gates:

1. Green: map/list delta <= `+8%` and blob delta <= `+12%`
2. Yellow: map/list delta `+8..+12%` or blob delta `+12..+18%` (investigate)
3. Red: map/list delta > `+12%` or blob delta > `+18%`

Based on this run set: currently Green/Yellow, not Red.

## Important caveat

The timed map/list/blob scenario blocks measure steady-state session behavior. They do not currently break out separate timing fields for auth setup (room-key bootstrap and token provisioning). Add explicit setup timing fields if you want a full cold-path auth tax number in the same report.

## Rust trace microbench (260k ops) - signed/unsigned, batch/non-batch

Date: 2026-05-22
Source trace: `docs/rustcode.json`
Runner: `core/src/bin/text_trace_rustcode_oneshot.rs`

Command shape used for each row (with env var toggles):

```powershell
& "C:\Users\bgstr\.cargo\bin\cargo.exe" run --release --manifest-path ".\core\Cargo.toml" --bin text_trace_rustcode_oneshot
```

All rows below use `ACTIVESYNC_TEXT_TRACE_MAX_OPS=260000`.

### Environment comparability note

Treat cross-run numbers as comparable only when hardware/runtime are aligned.
Recent reruns were performed on a laptop-class host:

1. Manufacturer/Model: ASUSTeK COMPUTER INC. ProArt P16 H7606WP_H7606WP
2. RAM: 33,413,771,264 bytes (~31.1 GiB)
3. CPU: AMD Ryzen AI 9 HX 370 (12 cores / 24 logical)
4. OS: Windows 11 Home (10.0.26200)

If earlier numbers were collected on a different desktop host (for example higher RAM class, different sustained thermals/boost behavior, different power policy), use same-host reruns before attributing deltas to code changes.

| Variant | signed | use_batch | batch_size | applied_ops | apply_ms | ops_per_sec |
|---|---:|---:|---:|---:|---:|---:|
| unsigned non-batch | 0 | 0 | n/a | 260000 | 335 | 774955 |
| unsigned batch | 0 | 1 | default | 260000 | 382 | 679002 |
| signed non-batch | 1 | 0 | n/a | 260000 | 8500 | 30587 |
| signed batch | 1 | 1 | default | 260000 | 680 | 382220 |
| signed batch (50k) | 1 | 1 | 50000 | 260000 | 722 | 360029 |

Interpretation notes:

1. `apply_ms` and `ops_per_sec` are apply-phase only (pre-translated nodes), not JSON parse/adapter translation time.
2. Signed mode includes Ed25519 verification cost during apply; batch mode uses batched verify in `apply_remote_batch`.
3. For this run, batching helps significantly in signed mode, while unsigned mode is slightly faster without batch.

## Text throughput, wire cost, and cold-start convergence (v1 spike)

Date: 2026-07-01
Source trace: `docs/rustcode.json` (real editing trace, restored from git history — was
deleted by the "cleanup stale docs" commit along with the other benchmark result
artifacts on this page)
Runner: `core/tests/text_throughput_and_convergence.rs` (`cargo test -p nodalmerge-core
--release --features text_projection --test text_throughput_and_convergence -- --nocapture`)

This adds numbers the earlier percentage-based projection/replay summaries
didn't have: char op throughput (ops/sec, both unbatched and batched), wire
cost (bytes/op), and cold-start convergence (a fresh peer bulk-catching-up on
an existing room via `apply_remote_batch`), loosely modeled on
[dmonad/crdt-benchmarks](https://github.com/dmonad/crdt-benchmarks)'
real-world-trace-replay methodology. `TextProjectionMode::Enabled` throughout.
"Unbatched" = one `apply_remote` call per character (what both this test and
the existing `text_trace_rustcode.rs` bench have always done). "Batched" = one
`apply_remote_batch` call per trace transaction (the realistic client-SDK
pattern — a paste or keystroke burst submitted as one transaction).

| Ops applied | unbatched ops/sec | batched ops/sec | batched speedup | mean bytes/op | cold-start batch_apply_ms |
|---:|---:|---:|---:|---:|---:|
| 5,000 | 384,947 | — | — | 208.9 | 9.8 |
| 50,000 | 4,603–5,332 | 3,371 | **0.73x (slower)** | 210.8 | 128–201 |
| 150,000 | 455 | 731 | **1.60x (faster)** | 210.8 | 469 |

### Finding 1: batching helps or hurts depending on edit pattern — it is not a free win here

At 50k ops, batching is *slower*. At 150k ops, batching is *faster*. The
difference is what's actually in the trace: `docs/rustcode.json`'s first
~42,493 ops are one single sequential append (a paste into an empty doc,
`docs/rustcode.json` txn 0). `core/src/text.rs`'s `apply_insert` (the unbatched
per-op path, via `apply_node`) has a `can_fast_append` shortcut specifically
for exactly this case — appending at the current end of the document — which
the batched path's `apply_updates_coalesced` (all updates queued, then one
`ensure_materialized()` call at the end) doesn't get. So the 50k-op window
lands almost entirely inside the regime the *unbatched* path already handles
well, and batching just adds coalescing overhead for no benefit. Once the
trace moves past that first paste into the scattered/non-append edits that
make up the bulk of a real editing session (confirmed at 150k ops, well past
the first two large pastes at op offsets 0 and ~70,194), batching wins clearly
(1.60x), matching the `merge_10k` → `merge_10k_batch` ~10x win documented
above in the "Rust trace microbench" table for the merge/verify path.

Net: batching is the right default recommendation for client SDKs handling
real multi-op edits (the scattered-edit case dominates real sessions), but
it's not universally faster — sequential append-heavy bursts (initial doc
load, large pastes) are already well-served by the existing fast-append path
and don't need it.

### Finding 2: still-open — absolute apply-time scaling looks superlinear regardless of batching

Both paths get dramatically slower per-op as the document grows: unbatched
goes from 384,947 ops/sec (5k ops) to 4,603–5,332 ops/sec (50k ops) to 455
ops/sec (150k ops) — nowhere near flat, in either mode. This is present in the
**existing, unmodified** `core/benches/text_trace_rustcode.rs` bench too —
independently confirmed at 50k ops: 9.36s / 5,341 elem/s via `cargo bench`,
matching this test's unbatched numbers. Extrapolating, the full ~980k-op trace
would take well over an hour, not the ~258ms implied by the earlier "Enabled
projection: 257.94 ms median" summary on this page — that number was almost
certainly computed against a much smaller bounded subset of the trace, not the
full trace (which itself turns out to be ~980k ops, not the "260k" the old
table's env var name implied — that 260000 was itself already a truncation).

This is unverified against the `unsigned non-batch: 774,955 ops/sec` row in
the "Rust trace microbench" table above — that row's runner
(`core/src/bin/text_trace_rustcode_oneshot.rs`) no longer exists in the repo or
its history, and (based on the ops/sec gap) almost certainly did not run with
`TextProjectionMode::Enabled`. Not an apples-to-apples comparison; flagging
rather than reconciling.

**Not yet root-caused**: whether the remaining superlinear-with-document-size
trend (present in *both* the fast-append and batched-coalesced paths) is a
real issue in `ensure_materialized()`/the underlying tree structure, or
expected/acceptable given Enabled mode's read-side win (read-only range ~1.5µs
vs ~6.2ms disabled, documented elsewhere on this page). Worth a profiling
follow-up before trusting Enabled-mode apply-time numbers at large document
sizes — this is a separate question from the batching finding above.

### Test scope note

The test defaults to a 50k-op bound (`ACTIVESYNC_TEXT_THROUGHPUT_MAX_OPS=150000`
or `=full` to see the regime where batching wins, `=<N>` for anything else)
because the full trace is impractical for a normal `cargo test` run given
Finding 2. Note the 50k default lands almost entirely inside the append-heavy
first paste (Finding 1) — it's a fast smoke test, not representative of
batching's real-world benefit.

## Text engine rewrite results (docs/text-performance-plan.md Phases 1-3)

Date: 2026-07-02
Host: same laptop-class host as above (ProArt P16, Ryzen AI 9 HX 370).
Same runner/trace as the v1 spike (`core/tests/text_throughput_and_convergence.rs`,
`docs/rustcode.json`, `TextProjectionMode::Enabled`). Baseline re-measured same-day,
same host, immediately before the change (`benchmarks/results/text-throughput-baseline-50k.json`).

The projection was rewritten from five parallel per-character structures (per-char
HashMap entries/children/positions, O(n) shift loops, full-DFS integration, full
rebuilds per batch) to a chunked order-statistic item list: all items including
tombstones in 128-char chunks, two Fenwick indexes (visible chars / visible bytes),
classic flat-RGA scan integration from the `after` anchor, orphan buffering for
out-of-causal-order delivery. Merge semantics (descending-OpId sibling order),
per-char OpId identity, and the whole public API are unchanged — verified by the
existing randomized projection-vs-legacy parity test, all 200+ core tests, and the
full-trace `endContent` equality assert.

| Ops | unbatched ops/sec (before → after) | batched ops/sec (before → after) | cold-start batch_apply_ms (before → after) |
|---:|---|---|---|
| 50,000 | 5,540 → **213,690** (38.6x) | 4,747 → **204,556** (43x) | 115.8 → 119.5 |
| 150,000 | ~455 → **72,367** (~159x) | 731 → **73,560** (~100x) | 469 → 441.0 |
| 979,844 (full) | impractical (est. 10+ hours) → **46.6 s** (21,042 ops/sec) | same | — → **4,161.6** (~235k ops/sec engine-side) |

Notes:

1. Full-trace replay now completes and the final text **matches the trace's
   `endContent` exactly** — Finding 2's superlinear blowup is resolved on the engine
   side. The remaining decay in the *replay* ops/sec columns (214k → 72k → 21k) is
   dominated by the test harness's own `Vec::insert/remove` position bookkeeping
   (O(n) memmove per op on a growing `Vec<OpId>`), not the engine: the cold-start
   column — pure engine work over the same nodes — holds ~2.4 µs/op at 50k and
   ~4.2 µs/op at 980k (near-flat, log-ish).
2. Finding 1 (batching slower at 50k) is also resolved: batched ≈ unbatched at every
   size, because the batch path now applies incrementally instead of doing an
   O(document) rebuild per transaction.
3. Wire cost is unchanged (~210 bytes per per-char op) — that axis is addressed by
   using the existing `InsertRange`/`DeleteRange` ops (one node per editor
   transaction), which is a client/SDK pattern change, not an engine change.

## [B4] Real-world editing dataset (dmonad/crdt-benchmarks parity)

Date: 2026-07-02
Runner: `core/tests/b4_editing_trace.rs`
Trace: `benchmarks/data/b4-editing-trace.json` — extracted 1:1 from
`crdt-benchmarks/js-lib/b4-editing-trace.js` (the automerge-perf edit-by-index
trace: 182,315 single-char insertions + 77,463 deletions = 259,778 ops,
104,852-char final LaTeX document). Applied the same way the B4 drivers do:
one transaction per edit, position-based ops (`InsertRange`/`DeleteRange`,
`Offset` anchors), single client, then extract content and compare to
`finalText`. Every edit is a full unsigned hash-linked DAG node (blake3 id +
parents + postcard) — integrity work the JS CRDTs don't do at all.

| Metric | nodalmerge (Rust core, projection Enabled) |
|---|---:|
| [B4] time (apply 259,778 edits + content check) | **882 ms** (294,521 ops/sec) |
| [B4] convergence | exact `finalText` match |
| [B4] total update bytes (per-node postcard) | 46,542,513 (179.2 bytes/edit) |
| [B4] cold-start: fresh peer applies full history + content check | 947 ms |

Cross-system context (their README numbers, **different hardware** — desktop
i5-8400/Node 20 vs this laptop Ryzen AI 9 HX 370 — so treat as order-of-
magnitude context, not a controlled comparison): yjs 5,714 ms, ywasm
28,675 ms, loro 3,089 ms, automerge 14,326 ms for the same B4 "time" row.
Our per-edit wire cost (179 bytes) is dominated by DAG node framing (author,
parent hash, node hash) around single-char ops; multi-char edits amortize it
(that's the range-op client pattern), and yjs's ~29 bytes/edit has no
integrity metadata to carry. Their "parseTime" decodes a compact document
snapshot; our closest analog today replays the op history (947 ms) — snapshot
cold-start is the planned S4.4 follow-up.

## Map/list/blob incremental caches (docs/text-performance-plan.md Part II, M1-M4)

Date: 2026-07-02

`StateGraph` now maintains incremental materialized views updated O(1) per op at
apply time, replacing full-DAG replays at read time:

1. `resolve` / `resolve_canonical` / `read_speculative` / `read_canonical` /
   `resolve_with_meta`: LWW winner caches (speculative + canonical) — O(keys) per
   read instead of O(history); per-key reads O(1).
2. `resolve_list`: per-key incremental list projection (LWW position + absorbing
   tombstones + ordered visible set) — O(items) per read instead of O(history).
3. `referenced_blob_hashes`: incremental set.
4. Host ingest (`ImportPack` in host-core): the per-batch full-history
   `detect_conflicts()` rescan is replaced by an incremental conflict stream
   (`drain_pending_conflicts`) — O(batch) per import instead of O(history).
   `detect_conflicts()` itself is unchanged for on-demand callers (bridge).
   Note the stream emits each conflict pairing once at the moment it happens; the
   old rescan re-paired every historical loser against the current winner on every
   call (e.g. one winning write against a key with 1,000 same-author overwrites
   emitted 1,000 events) — downstream fingerprint dedup made most of that noise,
   and the host-visible history is now strictly cleaner.

Parity: randomized cache-vs-replay tests (`map_cache_matches_replay_*`,
`list_cache_matches_replay_*`) assert the caches equal the retained full-replay
oracles across mixed local/remote interleavings.

Measured: `core/benches/resolve_1k.rs` (1,000 keys, one write each — the *minimum*
possible history-to-key ratio, so this is the floor of the win): **-66%**
(criterion vs stored pre-change baseline, ~350 µs → 118 µs). The gap grows with
room age: replay was O(total ops ever), the cache is O(live keys), so a
long-lived room with overwrites sees orders of magnitude, not 3x. Text
throughput re-measured after the cache hook landed: 213,897 ops/sec at 50k —
no regression (cache update is a no-op match arm for text ops).

## Full ecosystem benchmarks retained (.NET host vs Rust host)

This microbench section is additive and does not replace host-level apples-to-apples measurements.

Existing ecosystem benchmark coverage remains in place:

1. `benchmarks/Run-SdkScenarioBenchmarks.mjs` (semantic map/list/blob scenarios across targets)
2. `benchmarks/Run-BenchmarkMatrix.ps1` with `benchmarks/benchmark-matrix.v1.json` (auth/security/guardrail profile rows)
3. target comparison includes `rust-combined-server` and `dotnet-host-runtime-alias` as documented above in this file
