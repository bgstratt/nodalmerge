# Text Update Plan

Goal: Improve text runtime performance by moving from replay-per-resolve to incrementally maintained projection and canonical persisted range ops, while preserving existing RGA convergence semantics.

Framing: This is a pre-deploy enhancement/iteration path. We are not maintaining backward compatibility constraints between old and new text engines.

## Scope and Impact Matrix

| Area | Phase 1 | Phase 1.5 | Phase 2 | Phase 3 |
|---|---|---|---|---|
| core | required | required | required | required |
| bridge (WASM Rust API surface) | likely small | optional (debug toggles) | required (range APIs) | optional |
| JS SDK and web bindings | likely small | optional | required | optional |
| host integrations (dotnet-host, server) | none expected | none expected | only if adopting range APIs immediately | none expected |

Interpretation:
- Most implementation work is in core.
- Host/server can stay mostly untouched through Phase 1 and 1.5.
- Bridge and SDK changes are mainly API ergonomics and optional toggles until range ops land.

## Architectural Model (Explicit)

Text runtime is modeled as layered materialization:

OpLog
  -> TextProjection
      -> PositionIndex

- OpLog:
  - immutable source of truth
  - convergence semantics
  - replay and repair
- TextProjection:
  - canonical runtime materialization layer
  - visible text + tombstone-aware metadata
  - incrementally maintained from accepted ops
- PositionIndex:
  - optional accelerator for offset/anchor mapping
  - pluggable and replaceable
  - benchmarked independently from projection correctness

Design rule: Keep these layers decoupled so indexing acceleration does not contaminate CRDT or projection semantics.

## Phase 1 - TextProjection in StateGraph

Status: [x] complete

Execution checklist:
- [x] Add TextProjection data model in core/src/text.rs or core/src/text_projection.rs with per-key state.
- [x] Add text_projections: HashMap<String, TextProjection> to StateGraph.
- [x] Update apply_local success path to incrementally apply text ops into projection.
- [x] Update apply_remote success path to incrementally apply text ops into projection.
- [x] Update apply_remote_batch success path to incrementally apply accepted nodes in finalized graph insertion order (not transport order).
- [x] Route resolve_text and resolve_text_seq through projection as canonical runtime path.
- [x] Keep replay resolver only as debug/parity/repair infrastructure.
- [x] Separate projection internals into two structures:
  - [x] CRDT metadata graph (anchors, ordering, tombstones)
  - [x] dense visible sequence (hot-path reads)
- [x] Ensure tombstones do not participate in hot-path visible iteration.

Acceptance criteria:
- [x] Existing text tests in core/src/text.rs pass unchanged.
- [x] No replay or convergence regressions in concurrent insert/delete scenarios.
- [x] Runtime text reads are served by projection path by default.

## Phase 1.5 - Safety Fallback and Parity Checks

Status: [x] complete

Execution note:
- Proceeding with Phase 2+ delivery is allowed even if parity hardening remains open; this track is kept for rollback confidence, diagnostics, and pre-rollout verification.

Execution checklist:
- [x] Add compile-time flag for projection components (example: text_projection).
- [x] Add runtime mode switch:
  - [x] Disabled
  - [x] Enabled
  - [x] ParityCheck
- [x] Add parity-check mode that compares projection results against replay resolver on deterministic corpora.
- [x] Add mismatch diagnostics (key, node id, op id context).
- [x] Add projection self-healing path: on mismatch, discard projection, rebuild from oplog, continue safely.
- [x] Add a benchmark mode that reports projection vs legacy resolve timing.
- [x] Add parity sampling controls (`sample_every`, selected key allow-list) and parity-run counters.

Acceptance criteria:
- [x] Zero parity mismatches across deterministic test corpus.
- [x] Projection path shows measurable speedup on text trace workloads.
- [x] Replay resolver remains available for validation/repair but is not a long-term hot path.

## Phase 2 - Range Ops and Op Compression

Status: [x] complete

Execution checklist:
- [x] Add phase-2 scaffolding module for range-op operation shape + canonical lowering rules.
- [x] Extend TextOp with range operations (insert/delete spans).
- [x] Define canonical lowering rules so char and range ops compose deterministically.
- [x] Add first projection updater support for lowered range streams (range lowered to deterministic one-char stream).
- [x] Update projection updater to apply both op forms.
- [x] Expose both offset and anchor APIs for range insertion/deletion.
- [x] Add SDK convenience helpers to prefer range ops for paste/replace flows.
- [x] Establish deterministic per-character identity derivation scaffold for persisted ranges.
- [x] Persist range ops canonically without runtime-only lowering as the long-term write-path model.
- [x] Replace transitional char-expansion code paths with canonical range-aware replay/projection application.
- [x] Optimize for single-char ops but do not enforce one-insert-per-tx invariant.

Acceptance criteria:
- [x] Cross-peer convergence is identical for equivalent canonical range edit traces.
- [x] Metadata and allocation overhead reduced for large contiguous edits.

## Phase 2.5 - Partial Materialization and Windowed Reads

Status: [x] complete

Execution checklist:
- [x] Add windowed read APIs (example: resolve_text_range(key, start, len)).
- [x] Add partial/lazy flattening to avoid full-document string materialization for viewport reads.
- [x] Track dirty ranges (not only dirty boolean) to support incremental UTF-8 buffer updates.
- [x] Add benchmark scenarios for large docs with viewport-like read patterns.
- [x] Add large-document acceptance harness for windowed reads + projection allocation/rebuild behavior.
- [x] Add CI artifact output for large-doc harness metrics.

Acceptance criteria:
- [x] Viewport/windowed reads avoid full flatten for large documents.
- [x] Large-document memory and allocation behavior improves under repeated small edits.

## Recent Benchmark Snapshot (2026-05-23)

Configuration:
- `ACTIVESYNC_TEXT_TRACE_MAX_OPS=2000`
- `ACTIVESYNC_TEXT_TRACE_RANGE_LEN=256`
- `ACTIVESYNC_TEXT_TRACE_RANGE_STRIDE=512`
- `ACTIVESYNC_TEXT_TRACE_RANGE_WINDOWS=8`
- Criterion flags (stabilized matrix): `--sample-size 20 --measurement-time 5 --warm-up-time 2`

Stabilized timing table (95% CI, bounded trace):

| Mode | Full replay+resolve | Range replay+window-resolve |
|---|---:|---:|
| disabled | 300.37 ms [277.07, 326.12] | 290.51 ms [284.97, 296.31] |
| enabled | 257.94 ms [250.05, 267.02] | 279.54 ms [273.41, 285.79] |
| parity (`sample_every=16`) | 376.87 ms [357.70, 400.87] | 369.83 ms [339.16, 406.96] |

Delta view from stabilized matrix:
- Enabled vs disabled (full): ~14.1% faster median.
- Enabled vs disabled (range mixed): ~3.8% faster median.
- Parity(16) vs enabled (full): ~46.1% slower median.
- Parity(16) vs `sample_every=1` historical runs: materially faster (sub-second vs ~1.7s class).

Split-benchmark smoke snapshot (`--sample-size 10 --measurement-time 1 --warm-up-time 1`, enabled mode):
- `text_trace_rustcode_apply_only_unsigned_enabled`: 257.32 ms [248.50, 267.80]
- `text_trace_rustcode_read_only_full_unsigned_enabled`: 104.62 ns [96.61, 115.31]
- `text_trace_rustcode_read_only_range_unsigned_enabled`: 4.731 us [4.338, 5.162]

Cursor mapping ROI snapshot (`--sample-size 20 --measurement-time 5 --warm-up-time 2`):
- `text_trace_rustcode_cursor_mapping_unsigned_enabled`: 1.3756 us [1.3639, 1.3875]
- `text_trace_rustcode_cursor_mapping_unsigned_disabled`: 1.1722 s [1.1633, 1.1825]
- Enabled vs disabled cursor mapping: ~852k times lower median latency in this benchmark profile.

Cursor mapping size-sweep snapshot (`text_cursor_mapping_size_sweep`, enabled mode):
- size 1k: 6.0899 us [6.0058, 6.1689]
- size 2k: 6.2632 us [5.9944, 6.5437]
- size 4k: 6.5031 us [6.3564, 6.7019]
- size 8k: 7.4491 us [7.2309, 7.6217]
- size 16k: 8.1073 us [7.9156, 8.2924]
- 16x document growth produced ~1.33x median mapping cost, consistent with sublinear/log-like scaling in this profile.

Append fast-path snapshot (`--sample-size 10 --measurement-time 1 --warm-up-time 1`, apply-only):
- `text_trace_rustcode_apply_only_unsigned_enabled`: 14.201 ms [13.951, 14.605]
- `text_trace_rustcode_apply_only_unsigned_disabled`: 14.359 ms [13.956, 14.882]
- Enabled median is ~1.1% lower in this profile after introducing append fast-path placement.

Projection-local cache refinement snapshot (`--sample-size 10 --measurement-time 1 --warm-up-time 1`, read-only range):
- `text_trace_rustcode_read_only_range_unsigned_enabled`: 4.5572 us [4.4639, 4.6208]
- `text_trace_rustcode_read_only_range_unsigned_disabled`: 80.591 ms [78.189, 83.306]
- Enabled median remains orders of magnitude lower after routing windowed reads through projection-local cached string state.

Allocation reuse snapshot (`--sample-size 10 --measurement-time 1 --warm-up-time 1`, apply-only):
- `text_trace_rustcode_apply_only_unsigned_enabled`: 24.454 ms [21.440, 27.064]
- `text_trace_rustcode_apply_only_unsigned_disabled`: 30.330 ms [29.375, 31.295]
- Absolute medians drifted versus earlier microprofile runs, but enabled remained ~19.4% lower median than disabled in this run.
- Optimization implemented: reuse of `visible_seq` capacity during materialization rebuilds plus scratch buffer reuse for range-delete target collection.

Phase 3 completion evidence:
- Cursor mapping ROI gate benchmark is implemented and exercised in CI (`core-cursor-mapping-roi`).
- Cursor mapping median threshold is enforced via `ACTIVESYNC_CURSOR_MAPPING_MAX_MEDIAN_NS` parsing criterion `estimates.json`.
- Randomized cursor mapping/convergence coverage is in place (`phase3_cursor_mapping_randomized_trace_roundtrips`).
- Latest local validation run: `cargo test -p activesync-core` passed (178 unit tests, plus fixture/integration suites).
- Projection-enabled resolve paths no longer fall back to replay on projection misses; they materialize transient projection state instead.
- CI parity soak gate runs deterministic zero-mismatch corpus for `ACTIVESYNC_PARITY_SOAK_RUNS` iterations (default `20`).

Tuning decision:
- Set default parity sampling cadence to `sample_every=16` to make parity mode practical for routine validation while preserving the ability to force strict every-update parity (`sample_every=1`) when needed.
- Added benchmark knob `ACTIVESYNC_TEXT_TRACE_PARITY_SAMPLE_EVERY` (default `16`) for matrix-driven tuning.

## Perf Delta Summary (Phase-by-Phase)

Method notes:
- Different snapshots used different benchmark slices and stabilization settings; treat cross-snapshot comparisons as directional unless they share the same benchmark id + flags.
- All medians below are copied from the benchmark snapshots in this document.

Phase 1 / projection runtime directionality:
- Historical bounded trace (`ACTIVESYNC_TEXT_TRACE_MAX_OPS=2000`) moved from:
  - disabled: ~231.99 ms
  - enabled: ~202.39 ms
  - delta: ~12.8% faster enabled median.

Phase 1.5 / parity practicality:
- Stabilized matrix (`sample_every=16`) full trace:
  - enabled: 257.94 ms [250.05, 267.02]
  - parity: 376.87 ms [357.70, 400.87]
  - parity overhead vs enabled: ~46.1%.
- Historical parity (`sample_every=1`) reference remained in ~1.7 s class; `sample_every=16` is materially faster for routine validation.

Phase 2 + 2.5 / range + windowed-read behavior:
- Stabilized matrix range replay+window-resolve:
  - disabled: 290.51 ms [284.97, 296.31]
  - enabled: 279.54 ms [273.41, 285.79]
  - delta: ~3.8% faster enabled median.

Phase 3 / index-backed cursor mapping ROI:
- Cursor mapping benchmark:
  - enabled: 1.3756 us [1.3639, 1.3875]
  - disabled: 1.1722 s [1.1633, 1.1825]
  - delta: ~852k times lower enabled median latency.
- Cursor mapping size sweep (enabled) stayed sublinear in profile:
  - 1k: 6.0899 us -> 16k: 8.1073 us (~1.33x cost for 16x size growth).

Post-Phase optimization deltas:
- Append fast path (apply-only microprofile):
  - enabled: 14.201 ms [13.951, 14.605]
  - disabled: 14.359 ms [13.956, 14.882]
  - enabled median ~1.1% lower.
- Projection-local range read refinement (read-only range microprofile):
  - enabled: 4.5572 us [4.4639, 4.6208]
  - disabled: 80.591 ms [78.189, 83.306]
  - enabled remains orders of magnitude lower.
- Sibling fanout distribution (rustcode trace replay):
  - buckets: 516430; max: 18; avg: 1.0119; p95: 1
  - >1 fanout buckets: 4462 (~0.86%); tail >= 8 fanout buckets: 6 total
  - decision: keep `Vec` representation; no `smallvec` migration needed at current distribution.
- Sibling ordering maintenance:
  - projection insert path keeps siblings ordered incrementally via binary-search insertion (`partition_point`) at apply time.
  - no resolve-time sibling re-sort is used in projection traversal paths.
- Per-key dirty-bit fast-read behavior:
  - projection keeps a per-key materialization dirty flag and only rebuilds when invalidated.
  - repeated read validation (`text_projection_repeated_reads_without_writes_do_not_rebuild`) confirms no additional rebuilds under read-only loops.

Stable-profile follow-up snapshot (2026-05-23, `--sample-size 10 --measurement-time 1 --warm-up-time 1`, `ACTIVESYNC_TEXT_TRACE_MAX_OPS=2000`):
- apply-only:
  - enabled: 14.935 ms [14.458, 15.611]
  - disabled: 13.779 ms [13.431, 14.296]
  - directional note: near-parity class with disabled slightly lower in this short run.
- read-only range:
  - enabled: 2.3010 us [2.1874, 2.4667]
  - disabled: 37.493 ms [36.564, 38.644]
  - enabled remains ~16k times lower median latency.
- cursor mapping:
  - enabled: 315.44 ns [310.19, 320.02]
  - disabled: 76.736 ms [75.163, 78.457]
  - enabled remains ~243k times lower median latency.

Follow-up perf enhancement evaluation outcome:
- Completed implemented set (cache refinement, append fast path, allocation reuse, sibling incremental ordering, dirty-bit read fast path) is sufficient for current rollout goals.
- No additional blocker-level optimizations are required; retain benchmark monitoring and only add new structural changes if stable-profile regressions persist.

## Implementation Status (Current)

- Core projection runtime (Phase 1): complete and running as default path.
- Safety/parity layer (Phase 1.5): in progress; parity sampling/filters/self-heal, compile-time feature gating, and CI parity profile are in place.
- Windowed reads and dirty-range groundwork (Phase 2.5): complete; APIs, benchmark coverage, acceptance harness, and CI metrics artifact output are in place.
- Range-op model (Phase 2): complete; canonical persisted range semantics are in place, transitional expansion paths were removed, and range apply path allocation churn was reduced.
- Counted position index (Phase 3): complete; counted-index-backed mapping internals, randomized cursor stress tests, sibling fanout instrumentation, and ROI gate are in place.

## Phase 3 - Counted Position Index (log n mapping)

Status: [x] complete

Execution checklist:
- [x] Introduce counted index structure for visible length accounting (Fenwick-based scaffold; tree/chunk upgrade optional).
- [x] Implement offset to anchor and anchor to offset APIs with scaffold semantics (linear now, index-backed later).
- [x] Integrate index maintenance into incremental projection updates.
- [x] Add stress tests for large docs and random cursor movement/edit patterns.
- [x] Instrument sibling fanout distribution before introducing heavier sibling data structures.
- [x] Use the cursor mapping benchmark as an ROI gate in CI.

Acceptance criteria:
- [x] Position mapping complexity scales as O(log n) in benchmarked workloads.
- [x] Cursor-heavy operations avoid full-sequence scans.
- [x] No convergence or ordering regressions.

## Test and Validation Plan

- [x] Keep existing unit tests as baseline guardrails.
- [x] Add projection-specific unit tests:
  - [x] out-of-order remote insert application
  - [x] tombstone interactions with descendants
  - [x] sibling ordering under identical after
  - [x] multi-key isolation
- [x] Add parity tests (projection output equals legacy output for randomized traces).
- [x] Add convergence fixtures for equivalent canonical range traces.
- [x] Extend text_trace_rustcode benchmarking with projection on/off variants.
- [x] Split benchmark reporting into apply cost and read cost.
- [x] Record perf deltas after each phase.
- [x] Treat deterministic parity mismatch as correctness failure that blocks rollout (optional for dev velocity; enforce before production rollout).
- [x] Add large-document acceptance harness test for repeated small edits + viewport reads.
- [x] Publish large-document acceptance metrics as CI artifact.
- [x] Enforce cursor mapping median threshold via criterion estimates in CI.

Phase 2.5 acceptance gate (CI-enforced):
- `ACTIVESYNC_TEXT_LARGE_DOC_MAX_FULL_REBUILDS=16`
- `ACTIVESYNC_TEXT_LARGE_DOC_MAX_CAPACITY_RATIO=5.0`
- `ACTIVESYNC_TEXT_LARGE_DOC_MAX_DURATION_MS=20000`

## Remaining Work to Ship

Release blockers (minimum to declare rollout-complete):
- [x] Demonstrate zero parity mismatches across deterministic corpus in CI for the agreed soak window.
- [x] Sunset replay-per-resolve from runtime hot path once parity confidence gate is met.

Release blocker summary: none remaining.

Post-rollout hardening and optimization backlog:
- [x] Record perf deltas after each phase with a stable benchmark profile.
- [x] Resolve pending design docs (memory budget target, offset semantics boundary).
- [x] Evaluate additional projection performance enhancements as follow-up work.

## Phase 4 - Data-Driven Runtime Lifecycle and Write-Path Efficiency

Status: [ ] in progress

Decision policy:
- [ ] Any optimization proposal must include stable-profile before/after benchmark evidence.
- [ ] No optimization lands on benchmark noise alone; require either:
  - [ ] non-overlapping confidence intervals in stable profile, or
  - [ ] repeated directional improvement across at least 3 runs.
- [ ] If an optimization improves one metric but regresses another, keep it only when net impact matches product priorities (editor latency first, throughput second, memory third).

Execution checklist:
- [x] Add write-path focused benchmarks (single-user rapid typing, paste bursts, remote burst apply, mixed local+remote contention).
- [x] Add apply-path counters and manual snapshot export for:
  - [x] projection update time
  - [x] index maintenance time
  - [x] invalidation/rebuild counts
- [x] Add batched mutation apply path with coalesced invalidation/finalize.
- [x] Add benchmark comparison for batch vs per-op apply in burst workloads.
- [x] Extend dirty tracking from single range to coalesced multi-range where data shows benefit.
- [x] Add memory telemetry profiles for long-lived docs:
  - [x] bytes per visible char
  - [x] bytes per tombstone
  - [x] index overhead
  - [x] projection residency by key
- [x] Define runtime temperature tiers and transitions:
  - [x] Cold: oplog only
  - [x] Warm: projection materialized
  - [x] Hot: projection + index + cached flattening
- [x] Add projection demotion/eviction policy and rebuild latency benchmark.
- [x] Add canonical range-op hardening suite:
  - [x] mixed char/range equivalence fixtures
  - [x] interleaved peer traces
  - [x] randomized lowering regression corpus

Phase 4 evidence (in-progress):
- Added write-path benchmark harness at `core/benches/text_write_path.rs` with workloads for:
  - rapid typing append stream
  - paste bursts via range insert
  - remote burst apply (`apply_remote_batch`)
  - mixed local+remote interleaving
- Initial smoke snapshot (2026-05-23, enabled mode, reduced workload knobs, `--sample-size 10 --measurement-time 1 --warm-up-time 1`):
  - `text_write_rapid_typing_enabled/append_single_char`: 4.2349 ms to 4.5877 ms
  - `text_write_paste_bursts_enabled/insert_range_end`: 184.73 us to 198.74 us
  - `text_write_remote_burst_enabled/apply_remote_batch`: 3.6050 ms to 3.9755 ms
  - `text_write_mixed_local_remote_enabled/interleaved_local_remote`: 63.549 ms to 65.932 ms
- Initial smoke snapshot (2026-05-23, disabled mode, same knobs/flags):
  - `text_write_rapid_typing_disabled/append_single_char`: 6.8162 ms to 7.0247 ms
  - `text_write_paste_bursts_disabled/insert_range_end`: 292.44 us to 371.67 us
  - `text_write_remote_burst_disabled/apply_remote_batch`: 5.9475 ms to 6.0808 ms
  - `text_write_mixed_local_remote_disabled/interleaved_local_remote`: 108.16 ms to 111.45 ms
- Initial smoke snapshot (2026-05-23, parity mode, same knobs/flags):
  - `text_write_rapid_typing_parity/append_single_char`: 57.411 ms to 60.037 ms
  - `text_write_paste_bursts_parity/insert_range_end`: 416.50 us to 436.15 us
  - `text_write_remote_burst_parity/apply_remote_batch`: 56.543 ms to 58.073 ms
  - `text_write_mixed_local_remote_parity/interleaved_local_remote`: 1.3409 s to 1.3703 s
- Directional interpretation (smoke profile only):
  - enabled is faster than disabled across all four write-path workloads in this run profile
  - parity mode is substantially slower than enabled in write-heavy paths, consistent with validation overhead expectations
- Apply-path counter instrumentation is available via:
  - `StateGraph::text_apply_runtime_counters()`
  - `StateGraph::reset_text_apply_runtime_counters()`
  - per-key projection rollups from `TextProjectionDebugStats` (`invalidation_count`, `index_update_time_ns`, `index_rebuild_time_ns`)
- Manual snapshot emission is available in `core/benches/text_write_path.rs` via:
  - `ACTIVESYNC_TEXT_WRITE_EMIT_SNAPSHOT=1`
- Snapshot example (enabled mode, same reduced smoke knobs):
  - `rapid_typing`: `projection_update_calls=1000`, `projection_update_total_ns=2852100`, `index_maintenance_total_ns=1978800`
  - `paste_bursts`: `projection_update_calls=64`, `projection_update_total_ns=110900`, `index_maintenance_total_ns=16700`
  - `remote_burst`: `projection_update_calls=1000`, `projection_update_total_ns=3171300`, `index_maintenance_total_ns=2135600`
  - `mixed_local_remote`: `projection_update_calls=1334`, `projection_update_total_ns=42074500`, `index_maintenance_total_ns=3629000`
- Coalesced batch projection update path:
  - `apply_remote_batch` now applies verified nodes without per-node projection updates, then runs one coalesced projection update pass across accepted nodes.
  - Coalesced pass groups `Insert`/`Delete` updates per key, applies metadata updates, and performs one finalize materialization per touched key.
  - Range-op keys retain correctness-first fallback to per-node projection application.
- Batch vs per-op benchmark evidence (2026-05-23, enabled mode, same reduced knobs, `--sample-size 10 --measurement-time 1 --warm-up-time 1`):
  - `text_write_remote_burst_enabled/apply_remote_batch`: 1.8160 ms to 1.9843 ms
  - `text_write_remote_burst_enabled/apply_remote_per_op`: 4.0045 ms to 4.0927 ms
  - Directional delta: batch path is ~2.1x to 2.2x faster than per-op in this profile.
- Dirty tracking extension evidence:
  - projection now maintains coalesced multi-range dirty spans (`dirty_ranges`) in addition to `last_dirty_range`.
  - apply/runtime rollups now expose:
    - `dirty_range_count_total`
    - `dirty_span_chars_total`
    - `dirty_range_merge_count_total`
  - snapshot emission includes these fields when `ACTIVESYNC_TEXT_WRITE_EMIT_SNAPSHOT=1`.
  - enabled smoke snapshot sample (same reduced knobs):
    - `rapid_typing`: `dirty_range_count_total=1`, `dirty_span_chars_total=1000`, `dirty_range_merge_count_total=999`
    - `remote_burst`: `dirty_range_count_total=1`, `dirty_span_chars_total=1000`, `dirty_range_merge_count_total=0`
    - `mixed_local_remote`: `dirty_range_count_total=333`, `dirty_span_chars_total=55948`, `dirty_range_merge_count_total=1001`
  - benchmark effect after coalescing fast-path tuning (enabled mode, same reduced knobs):
    - no significant regression detected across rapid typing, remote burst batch, and mixed local+remote workloads in this run
    - paste burst workload showed directional improvement in this run profile
- Memory telemetry profile evidence (2026-05-24):
  - `core/tests/text_large_doc_acceptance.rs` now emits per-key and global memory telemetry via:
    - `ACTIVESYNC_TEXT_LARGE_DOC_METRICS_PATH`
    - `ACTIVESYNC_TEXT_MEMORY_TELEMETRY_PATH`
  - Large-doc acceptance artifact (`large_doc_windowed_reads_and_allocation_behavior`):
    - `bytes_per_visible_char_lower_bound=105.55`
    - `bytes_per_tombstone_lower_bound=1065.39` (small-tombstone denominator case)
    - `index_bytes_lower_bound=47,150`
    - `projection_bytes_lower_bound=552,871`
    - `tombstone_count=231`, `visible_len=5,238`
  - Long-lived multi-key profile artifact (`long_lived_projection_memory_telemetry_profile`):
    - global: `bytes_per_visible_char_lower_bound=120.12`, `bytes_per_tombstone_lower_bound=323.62`, `index_bytes_lower_bound=39,282`
    - per-key residency:
      - `doc`: `metadata_entries=2,027`, `visible_len=1,486`, `tombstone_count=541`
      - `title`: `metadata_entries=1,979`, `visible_len=1,442`, `tombstone_count=537`
      - `notes`: `metadata_entries=1,975`, `visible_len=1,434`, `tombstone_count=541`
  - Interpretation:
    - telemetry now reports required signals (bytes/visible, bytes/tombstone, index overhead, projection residency by key).
    - measured bytes/visible lower-bound currently sits above the previously documented steady-state target band and should be treated as a follow-up optimization signal.
- Runtime temperature tier evidence (2026-05-24):
  - Added tier model in `core/src/graph.rs`:
    - `TextRuntimeTemperature::{Cold, Warm, Hot}`
    - `TextRuntimeTemperatureThresholds { hot_read_calls, hot_write_calls }`
  - Added lifecycle APIs:
    - `set_text_runtime_temperature_thresholds(...)`
    - `text_runtime_temperature_thresholds()`
    - `text_runtime_temperature_for_key(key)`
    - `text_runtime_temperature_snapshot()`
  - Transition semantics implemented:
    - `Cold`: key has no resident projection entry (`text_projections` miss).
    - `Warm`: key has resident projection but has not crossed hot thresholds.
    - `Hot`: key is resident and crosses either read-call or write-call threshold (derived from `TextProjectionDebugStats` counters).
  - Validation coverage:
    - `text_runtime_temperature_tiers_transition`
    - `text_runtime_temperature_snapshot_reports_resident_keys`
- Projection demotion/eviction + rebuild latency evidence (2026-05-24):
  - Added residency policy model in `core/src/graph.rs`:
    - `TextProjectionResidencyPolicy { max_resident_keys }`
    - key-touch tracking with LRU-style eviction preference for non-hot keys
  - Added lifecycle APIs:
    - `set_text_projection_residency_policy(...)`
    - `text_projection_residency_policy()`
    - `text_projection_resident_key_count()`
    - `demote_text_projection_key(key)`
    - `ensure_text_projection_resident(key)`
  - Eviction policy behavior:
    - enforces `max_resident_keys` after projection update/rebuild paths
    - evicts oldest warm key first; falls back to oldest key when all candidates are hot
  - Added rebuild latency benchmark in `core/benches/text_write_path.rs`:
    - group: `text_projection_rebuild_latency_<mode>`
    - case: `evict_then_rebuild_resident`
    - profile run (`enabled`, `--sample-size 10 --measurement-time 1 --warm-up-time 1`):
      - `text_projection_rebuild_latency_enabled/evict_then_rebuild_resident`: 83.330 ms to 86.993 ms
  - Validation coverage:
    - `text_projection_residency_policy_evicts_oldest_warm_key`
    - `text_projection_demote_and_rebuild_api_roundtrip`
- Canonical range-op hardening suite evidence (2026-05-24):
  - Expanded `core/tests/text_range_convergence.rs` with:
    - `mixed_char_range_equivalence_fixtures`
    - `interleaved_peer_traces_char_and_range_converge`
    - `randomized_lowering_regression_corpus_matches_char_semantics`
  - Coverage intent:
    - fixed fixtures for mixed char/range sequencing equivalence
    - interleaved multi-peer traces validating canonical convergence (`resolve_text_canonical`)
    - deterministic randomized corpus for lowering regression detection
  - Validation run:
    - `cargo test -p activesync-core --test text_range_convergence` => 5 passed, 0 failed

Acceptance criteria:
- [x] Benchmarks show measurable write-path gain on burst scenarios without regressing cursor and viewport latency gates.
- [ ] Memory telemetry remains within documented budget bands on long-lived workloads.
- [x] Hot/warm/cold transitions are validated by benchmarked rebuild and interaction latency.
- [x] Range-op hardening corpus shows zero deterministic divergence.

Benchmark profile contract (for decision-making):
- [x] Use fixed trace/workload definitions and fixed Criterion flags for all comparisons.
- [ ] Record median and 95% CI for each metric, and keep raw artifacts in CI.
- [ ] Track a small set of release-significant KPIs:
  - [x] cursor mapping latency (ns/us class)
  - [x] viewport read latency (us class)
  - [x] apply throughput under bursts
  - [x] parity mismatch count (must stay zero in deterministic corpus)
  - [x] projection memory ratio vs visible bytes

Phase 4 canonical profile run snapshot (2026-05-23, local):
- Fixed profile knobs used:
  - `ACTIVESYNC_TEXT_TRACE_MAX_OPS=2000`
  - `ACTIVESYNC_TEXT_TRACE_RANGE_LEN=256`
  - `ACTIVESYNC_TEXT_TRACE_RANGE_STRIDE=512`
  - `ACTIVESYNC_TEXT_TRACE_RANGE_WINDOWS=8`
  - `ACTIVESYNC_TEXT_TRACE_PARITY_SAMPLE_EVERY=16`
  - Criterion flags: `--sample-size 20 --measurement-time 5 --warm-up-time 2`
- Write-path burst comparison (`core/benches/text_write_path.rs`):
  - `rapid_typing`: enabled `3.2006 ms` vs disabled `5.0474 ms` (~1.58x faster)
  - `paste_bursts`: enabled `156.64 us` vs disabled `224.11 us` (~1.43x faster)
  - `remote_burst apply_remote_batch`: enabled `1.2373 ms` vs disabled `3.1147 ms` (~2.52x faster)
  - `mixed_local_remote`: enabled `44.179 ms` vs disabled `75.412 ms` (~1.71x faster)
- Cursor + viewport latency comparison (`core/benches/text_trace_rustcode.rs`):
  - `read_only_range`: enabled `2.8816 us` vs disabled `48.273 ms` (enabled orders of magnitude lower)
  - `cursor_mapping`: enabled `388.37 ns` vs disabled `98.654 ms` (enabled orders of magnitude lower)
- Rebuild interaction evidence:
  - `text_projection_rebuild_latency_enabled/evict_then_rebuild_resident`: `42.295 ms`
- Deterministic correctness checks:
  - `cargo test -p activesync-core --test text_range_convergence` => `5 passed, 0 failed`
  - `cargo test -p activesync-core text_projection_parity_deterministic_corpus_has_zero_mismatches` => `1 passed, 0 failed`
- Memory telemetry artifacts (local paths):
  - `artifacts/phase4-closure/large-doc-metrics.json`
  - `artifacts/phase4-closure/memory-telemetry.json`
  - key results retained above target band:
    - large-doc `bytes_per_visible_char_lower_bound=105.55`
    - long-lived global `bytes_per_visible_char_lower_bound=120.12`
  - budget status remains open versus documented target (`~24 to 64` bytes/visible char)

Phase 4 closure runbook (to mark acceptance + contract items complete):
- Canonical profile (fixed for all comparisons):
  - benchmark flags: --sample-size 20 --measurement-time 5 --warm-up-time 2
  - trace knobs:
    - ACTIVESYNC_TEXT_TRACE_MAX_OPS=2000
    - ACTIVESYNC_TEXT_TRACE_RANGE_LEN=256
    - ACTIVESYNC_TEXT_TRACE_RANGE_STRIDE=512
    - ACTIVESYNC_TEXT_TRACE_RANGE_WINDOWS=8
    - ACTIVESYNC_TEXT_TRACE_PARITY_SAMPLE_EVERY=16
- Required command set (single profile pass):
  - cargo bench -p activesync-core --bench text_write_path -- --sample-size 20 --measurement-time 5 --warm-up-time 2
  - cargo bench -p activesync-core --bench text_trace_rustcode -- --sample-size 20 --measurement-time 5 --warm-up-time 2
  - cargo test -p activesync-core --test text_range_convergence
  - cargo test -p activesync-core --test text_large_doc_acceptance -- --nocapture
- Memory telemetry artifact emission:
  - set ACTIVESYNC_TEXT_LARGE_DOC_METRICS_PATH and ACTIVESYNC_TEXT_MEMORY_TELEMETRY_PATH to CI artifact paths before running text_large_doc_acceptance
  - evaluate bytes_per_visible_char_lower_bound against documented target band (~24 to 64 bytes/visible char)
  - evaluate projection memory ratio versus visible bytes against documented ceiling (~2x to 4x raw UTF-8 bytes)
  - current recorded profile values (~105.55 and ~120.12 bytes/visible char) remain above target and keep the memory acceptance item open until optimized or target is revised
- Evidence quality rule for optimization decisions:
  - accept only when either stable-profile 95% CIs do not overlap or at least 3 repeated runs show the same directional improvement
- Checkbox completion mapping:
  - measurable write-path gain without cursor/viewport regression: check only after canonical profile shows enabled write-path gains while cursor mapping and viewport read KPIs remain within prior gate bands
  - memory telemetry budget: check only after long-lived profile satisfies documented memory target/ceiling bands
  - hot/warm/cold validation: check only after transition tests plus rebuild-latency benchmark evidence are captured in canonical profile artifacts
  - range-op hardening divergence: check when deterministic corpus remains zero-divergence in text_range_convergence runs

## Phase 5 - Projection Storage Densification (Run/Span Engine)

Status: [ ] in progress

Execution stage status (2026-05-23):
- [x] Level 1 complete: physical compaction baseline (run/contiguous layout, run-level indexing, tombstone intervals, visible-cache densification).
- [ ] Level 2 active: semantic metadata amortization without changing CRDT convergence semantics.
- [ ] Level 3 deferred: aggressive semantic/history compression remains out of scope for now.

Level 2 preconditions (must hold before additional densification slices):
- [x] Projection rebuild determinism from oplog is validated.
- [x] Canonical convergence guardrails remain green in deterministic corpora.
- [x] Run-split/merge correctness corpus exists and is passing.
- [x] Add run-split-heavy benchmark traces (viewport/cursor + write churn) to close remaining observability gaps before claiming Level 2 closure.

Planning decision:
- Proceed with Level 2 implementation now; no additional blocker-level prework is required beyond the remaining benchmark coverage/checklist items already tracked below.

Objective:
- Reduce projection memory footprint from current ~32 to 47 bytes/visible-char lower-bound toward the documented target band (~24 to 64 bytes/visible-char), while preserving existing convergence and ordering semantics.

Non-negotiable invariants:
- CRDT/oplog semantics remain unchanged.
- Deterministic ordering and anchor precision remain unchanged.
- Mid-range insertion/deletion behavior remains correct via split-at-boundary run operations.
- `resolve_text_canonical` output remains identical for equivalent traces.
- Do not weaken replayability from oplog history.
- Do not weaken rebuildability/determinism for projection rehydration.
- Do not canonicalize away per-character logical identity (`OpId`) at semantic boundaries.
- Do not aggressively squash history as a side effect of densification.
- Do not permanently merge semantic tombstone/history spans; physical compression must stay lossless and reversible via replay.

Execution checklist:
- [x] Introduce contiguous projection storage primitives in `core/src/text.rs`:
  - [x] `TextRun` representation for visible content and metadata sharing
  - [x] arena/index-based addressing (no per-char heap objects on hot path)
  - [x] run split/merge helpers for insert/delete-at-offset behavior
- [x] Move projection metadata from per-char duplication to amortized per-run fields:
  - [x] actor/sequence range metadata on run header
  - [x] compact local actor table indirection for projection internals
- [x] Replace per-char tombstone materialization with tombstone span compression:
  - [x] interval/range set representation for deleted spans
  - [x] deterministic mapping from logical char targets to run offsets
- [x] Upgrade counted index integration from char-level references to run-level references:
  - [x] run-length weights in index maintenance
  - [x] offset->anchor and anchor->offset correctness under split/merge updates
- [x] Keep per-char logical identity in semantic layer while changing only physical projection layout:
  - [x] projection rebuild path from oplog remains available and deterministic
  - [x] parity/canonical validation paths continue to compare against same logical outputs
- [x] Add densification-specific tests:
  - [x] mid-run insert splits and preserves deterministic ordering
  - [x] range deletes over mixed visible/tombstoned spans converge
  - [x] randomized run split/merge corpus matches canonical char semantics
  - [x] projection rebuild from oplog reproduces identical run materialization outputs
- [x] Add densification benchmark and telemetry profile coverage:
  - [x] run-level memory profile fields (run_count, avg_run_len, tombstone_span_count)
  - [x] viewport/read latency and cursor mapping latency under run-split heavy traces
  - [x] write-path traces emphasizing mid-range edits and run churn

Acceptance criteria:
- [x] Long-lived profile `bytes_per_visible_char_lower_bound` is at or below 64 in canonical profile runs.
- [ ] Stretch target: long-lived profile reaches <= 40 bytes/visible-char lower-bound without cursor/viewport regression.
- [x] Cursor mapping and viewport read KPIs remain in current order-of-magnitude class vs disabled mode.
- [x] Deterministic parity corpus remains zero-mismatch.
- [x] Canonical range-op hardening corpus remains zero-divergence.

Decision gates:
- [x] No densification change lands without before/after canonical-profile evidence.
- [x] Evidence must satisfy either non-overlapping 95% CI or repeated directional improvement across at least 3 runs.
- [x] If memory improves but latency regresses, keep only when editor-latency gates remain within accepted class and product priority policy is satisfied.

Implementation order (risk-managed):
1. contiguous storage primitives
2. metadata amortization
3. tombstone span compression
4. run-level indexing
5. optional cold-history compaction

Phase 5 evidence (in-progress):
- Baseline memory artifacts before densification:
  - `artifacts/phase4-closure/large-doc-metrics.json` (`bytes_per_visible_char_lower_bound=105.55`)
  - `artifacts/phase4-closure/memory-telemetry.json` (`bytes_per_visible_char_lower_bound=120.12` global)
- Item 1 implementation slice (2026-05-23):
  - Added run primitives and split/merge helpers in `core/src/text.rs`:
    - `TextRun` (run header + contiguous `text`)
    - run insertion/removal helpers (`insert_into_runs`, `remove_from_runs`)
    - boundary-safe run locator helpers for insert/delete
  - Wired run helper updates into projection incremental paths:
    - `apply_insert` updates run structure in both fast-append and mid-sequence insert paths
    - `apply_delete` updates run structure on visible removal paths
    - `ensure_materialized` now rebuilds run layout from visible sequence
  - Added first mid-run tests:
    - `text_projection_mid_run_insert_splits_and_delete_merges_runs`
    - `text_projection_run_helpers_preserve_mid_insert_ordering`
  - Validation:
    - `cargo test -p activesync-core text_projection_` => pass
    - `cargo test -p activesync-core` => pass (187 unit tests + integration suites)
- Item 2 implementation slice (2026-05-23):
  - Replaced run-local per-char id vectors with amortized run headers in `core/src/text.rs`:
    - `TextRun { actor_idx, seq_start, len_chars, text }`
    - contiguous id derivation by `(actor_idx, seq_start + offset)` for run-local addressing
  - Added projection-local actor table indirection:
    - `ProjectionActorTable` interns authors once and maps to compact local indices (`u32`)
    - run merge/split/append decisions now operate on run-header metadata instead of per-char id buffers
  - Added validation test:
    - `text_projection_actor_table_indirection_is_deduplicated`
  - Validation:
    - `cargo test -p activesync-core text_projection_` => pass
    - `cargo test -p activesync-core` => pass (187 unit tests + integration suites)
- First before/after telemetry delta (2026-05-23):
  - New artifacts:
    - `artifacts/phase5-item1/large-doc-metrics.json`
    - `artifacts/phase5-item1/memory-telemetry.json`
  - Large-doc bytes/visible lower-bound: `105.55 -> 105.55` (no change)
  - Long-lived global bytes/visible lower-bound: `120.12 -> 120.12` (no change)
  - Interpretation: item 1 established run-primitive plumbing and correctness guardrails; measurable memory reduction is expected in later phases (metadata amortization + tombstone span compression + run-level indexing).
- Item 2 before/after telemetry delta (2026-05-23):
  - New artifacts:
    - `artifacts/phase5-item2/large-doc-metrics.json`
    - `artifacts/phase5-item2/memory-telemetry.json`
  - Large-doc bytes/visible lower-bound: `105.55 -> 105.55` (no change)
  - Long-lived global bytes/visible lower-bound: `120.12 -> 120.12` (no change)
  - Interpretation: run-header metadata amortization and actor-table indirection landed correctly, but current telemetry estimator still tracks dominant `visible_seq + entries` lower-bound components; larger memory delta is expected after tombstone span compression and run-level index transition.
- Item 3 implementation slice (2026-05-23):
  - Replaced per-entry tombstone boolean materialization with compressed tombstone span storage in `core/src/text.rs`:
    - `TombstoneStore { deleted_ids, spans_by_author }`
    - merged interval/range spans per author (`(start_lamport, end_lamport)`) with incremental coalescing
  - Projection behavior updates:
    - visibility checks now route through tombstone membership (`tombstones.contains(id)`)
    - delete paths (`apply_delete`, `apply_delete_metadata`) mark compressed spans via `mark_deleted`
    - replay/materialization and placement logic now use tombstone store state rather than per-entry deleted flags
  - Added compression regression test:
    - `text_projection_tombstone_spans_compress_contiguous_deletes`
  - Validation:
    - `cargo test -p activesync-core text_projection_tombstone_spans_compress_contiguous_deletes` => pass
    - `cargo test -p activesync-core` => pass (188 unit tests + integration suites)
- Item 3 telemetry snapshot (2026-05-23):
  - New artifacts:
    - `artifacts/phase5-item3/large-doc-metrics.json`
    - `artifacts/phase5-item3/memory-telemetry.json`
  - Added tombstone compression profile fields:
    - `tombstone_span_count`
    - `tombstone_author_bucket_count`
  - Snapshot values:
    - large-doc `bytes_per_visible_char_lower_bound=97.77`, `tombstone_span_count=222`
    - long-lived global `bytes_per_visible_char_lower_bound=122.34`
  - Interpretation:
    - tombstone span compression is now represented in runtime state and telemetry output.
    - absolute bytes/visible remains above target band; next high-impact step is run-level index/sequence storage reduction so telemetry is no longer dominated by per-char visible/index structures.
- Item 4 implementation slice (2026-05-23):
  - Upgraded counted index integration from char-level references to run-level references in `core/src/text.rs`:
    - `CountedPositionIndex` now stores run weights (`weights`) and Fenwick totals over run lengths
    - offset mapping now resolves through run selection (`select_offset`) then derives id via run header metadata (`id_for_visible_offset`)
    - anchor mapping now computes offsets from run-prefix totals + in-run offset
  - Projection update paths now maintain index as run-length structure:
    - removed char-level incremental index updates from `apply_insert`/`apply_delete`
    - run mutation helpers (`insert_into_runs`, `remove_from_runs`) now refresh run-weight index incrementally
    - materialization rebuild path now initializes index from run lengths (`new_from_weights`)
  - Validation:
    - `cargo test -p activesync-core text_projection_` => pass
    - `cargo test -p activesync-core` => pass (188 unit tests + integration suites)
- Item 4 telemetry snapshot (2026-05-23):
  - New artifacts:
    - `artifacts/phase5-item4/large-doc-metrics.json`
    - `artifacts/phase5-item4/memory-telemetry.json`
  - Large-doc bytes/visible lower-bound: `97.77` (vs item 3 `97.77`; equivalent within current estimator)
  - Large-doc duration: `1707ms -> 1678ms` (minor improvement)
  - Long-lived global bytes/visible lower-bound: `122.34` (vs item 3 `122.34`; equivalent)
  - Interpretation:
    - run-level index migration completed with parity/acceptance stability.
    - memory telemetry remained flat across item 3 -> item 4 snapshots in current profile; additional reductions likely require visible-sequence storage densification beyond index representation.
- Item 5 implementation slice (2026-05-23):
  - Added optional projection cold-history compaction controls:
    - `TextProjection::compact_cold_storage()` in `core/src/text.rs` shrinks over-allocated runtime buffers/maps while preserving projection semantics.
    - `StateGraph::compact_text_projection_key(key)` in `core/src/graph.rs` exposes compaction for resident keys without dropping metadata.
  - Added compaction regression test:
    - `text_projection_cold_compaction_preserves_semantics`
    - validates text output stability across compaction and checks capacity non-increase.
  - Validation:
    - `cargo test -p activesync-core text_projection_` => pass
    - `cargo test -p activesync-core` => pass (189 unit tests + integration suites)
- Item 5 telemetry snapshot (2026-05-23):
  - New artifacts:
    - `artifacts/phase5-item5/large-doc-metrics.json`
    - `artifacts/phase5-item5/memory-telemetry.json`
  - Large-doc bytes/visible lower-bound: `97.77` (vs item 4 `97.77`; equivalent)
  - Large-doc duration: `1676ms -> 1675ms` (noise-level change)
  - Long-lived global bytes/visible lower-bound: `122.34` (vs item 4 `122.34`; equivalent)
  - Interpretation:
    - optional cold compaction API landed for explicit lifecycle control.
    - canonical acceptance telemetry remains flat in default runtime flow because compaction is an opt-in cold-key maintenance action, not an always-on mutation in hot paths.
- Densification test-hardening slice (2026-05-23):
  - Added convergence test for mixed visible+tombstoned delete windows in `core/tests/text_range_convergence.rs`:
    - `mixed_visible_and_tombstoned_range_delete_converges`
  - Added randomized split/merge churn corpus against canonical semantics in `core/tests/text_range_convergence.rs`:
    - `randomized_run_split_merge_corpus_matches_canonical_semantics`
  - Validation:
    - `cargo test -p activesync-core --test text_range_convergence` => pass
    - `cargo test -p activesync-core` => pass (189 unit tests + integration suites)
  - Post-hardening acceptance/telemetry rerun artifacts:
    - `artifacts/phase5-hardening/large-doc-metrics.json`
    - `artifacts/phase5-hardening/memory-telemetry.json`
  - Post-hardening telemetry deltas vs item 5:
    - large-doc `bytes_per_visible_char_lower_bound`: `97.77 -> 97.77` (equivalent)
    - long-lived global `bytes_per_visible_char_lower_bound`: `122.34 -> 122.34` (equivalent)
    - large-doc duration: `1773ms -> 1684ms` (directional improvement, same threshold class)
- Highest-ROI visible-cache densification slice (2026-05-23):
  - Replaced duplicated per-char tuple cache storage with densified visible-id storage in `core/src/text.rs`:
    - projection now stores visible ids (`Vec<OpId>`) plus contiguous UTF-8 payload (`visible_string`) rather than `Vec<(OpId, char)>` tuple duplication.
    - `resolve_seq`/anchor/range helpers were rewired to derive pair views from the densified cache while preserving deterministic semantics.
  - Telemetry estimator alignment in `core/tests/text_large_doc_acceptance.rs`:
    - visible cache lower-bound now models `visible_ids + visible_string` layout.
  - Validation:
    - `cargo test -p activesync-core text_projection_` => pass
    - `cargo test -p activesync-core` => pass (189 unit tests + integration suites)
  - New artifacts:
    - `artifacts/phase5-arena/large-doc-metrics.json`
    - `artifacts/phase5-arena/memory-telemetry.json`
  - Telemetry deltas vs item 5 baseline:
    - large-doc `bytes_per_visible_char_lower_bound`: `97.77 -> 89.77` (improved)
    - long-lived global `bytes_per_visible_char_lower_bound`: `122.34 -> 114.34` (improved)
    - large-doc duration: `1675ms -> 1678ms` (noise-level)
- Densification rebuild-equivalence test slice (2026-05-23):
  - Added graph-level rebuild determinism test in `core/src/graph.rs`:
    - `text_projection_rebuild_from_oplog_reproduces_identical_run_materialization_outputs`
  - Fixed projection rebuild/transient node ordering in `core/src/graph.rs`:
    - `all_nodes_for_text` now uses deterministic logical transaction order (`lamport`, `author`, `node id`) rather than hash-id ordering.
  - Coverage intent:
    - construct deterministic mid-run split/merge + tombstone churn trace
    - demote resident projection, rebuild from oplog (`ensure_text_projection_resident`)
    - verify rebuilt outputs match pre-rebuild materialization (`resolve_text`, `resolve_text_seq_with_chars`)
    - verify run/index materialization invariants remain identical (`visible_len`, `index_weights_len`, `index_fenwick_len`, `metadata_entries`, `tombstone_count`, `tombstone_span_count`)
  - Validation:
    - `cargo test -p activesync-core text_projection_rebuild_from_oplog_reproduces_identical_run_materialization_outputs` => pass
    - `cargo test -p activesync-core text_projection_` => pass (20 tests)
    - `cargo test -p activesync-core --test text_range_convergence` => pass (7 tests)
    - `cargo test -p activesync-core` => pass (190 unit tests + integration suites)
    - `ACTIVESYNC_TEXT_WRITE_PROJECTION_MODE=enabled cargo bench -p activesync-core --bench text_write_path -- --sample-size 10 --measurement-time 1 --warm-up-time 1`
    - `ACTIVESYNC_TEXT_TRACE_PROJECTION_MODE=enabled cargo bench -p activesync-core --bench text_trace_rustcode -- --sample-size 10 --measurement-time 1 --warm-up-time 1`
  - Benchmark sanity snapshot (same smoke flags):
    - write-path rapid typing: disabled `~2.25ms` vs enabled `~1.79ms`
    - write-path remote burst batch: disabled `~3.30ms` vs enabled `~2.55ms`
    - read-only range trace: disabled `~6.66ms` vs enabled `~2.72us`
    - cursor mapping trace: disabled `~13.20ms` vs enabled `~295ns`
  - Post-slice acceptance/telemetry rerun artifacts:
    - `artifacts/phase5-rebuild/large-doc-metrics.json`
    - `artifacts/phase5-rebuild/memory-telemetry.json`
  - Telemetry deltas vs phase5-arena:
    - large-doc `bytes_per_visible_char_lower_bound`: `89.77 -> 89.77` (equivalent)
    - long-lived global `bytes_per_visible_char_lower_bound`: `114.34 -> 114.34` (equivalent)
    - large-doc duration: `1678ms -> 1630ms` (noise-level directional improvement)
- Densification benchmark coverage closure slice (2026-05-23):
  - Added write churn benchmark in `core/benches/text_write_path.rs`:
    - group: `text_write_run_churn_<mode>`
    - case: `mid_range_insert_delete_churn`
    - workload shape: repeated mid-document inserts followed by mid-document deletes to force run split/merge churn.
  - Added run-split-heavy read/cursor benchmarks in `core/benches/text_trace_rustcode.rs`:
    - `text_trace_rustcode_read_only_range_run_split_unsigned_<mode>`
    - `text_trace_rustcode_cursor_mapping_run_split_unsigned_<mode>`
    - workload shape: pre-built split/merge churned document state, then read-only viewport and cursor mapping probes.
  - Validation:
    - `cargo test -p activesync-core --test text_range_convergence` => pass (7 passed, 0 failed)
  - Smoke benchmark profile (`--sample-size 10 --measurement-time 1 --warm-up-time 1`):
    - write churn (`ACTIVESYNC_TEXT_WRITE_CHURN_BASE_LEN=1024`, `ACTIVESYNC_TEXT_WRITE_CHURN_STEPS=600`, `ACTIVESYNC_TEXT_WRITE_CHURN_INSERT_LEN=4`, `ACTIVESYNC_TEXT_WRITE_CHURN_DELETE_SPAN=3`):
      - `text_write_run_churn_enabled/mid_range_insert_delete_churn`: `286.06 ms` to `289.82 ms`
      - `text_write_run_churn_disabled/mid_range_insert_delete_churn`: `566.70 ms` to `570.93 ms`
      - directional delta: enabled median is ~2x lower in this profile.
    - run-split read/cursor (`ACTIVESYNC_TEXT_TRACE_RUN_SPLIT_BASE_LEN=2048`, `ACTIVESYNC_TEXT_TRACE_RUN_SPLIT_CHURN_STEPS=200`, `ACTIVESYNC_TEXT_TRACE_RUN_SPLIT_INSERT_LEN=4`, `ACTIVESYNC_TEXT_TRACE_RUN_SPLIT_DELETE_SPAN=3`, `ACTIVESYNC_TEXT_TRACE_RUN_SPLIT_WINDOWS=8`):
      - `text_trace_rustcode_read_only_range_run_split_unsigned_enabled`: `5.2272 us` to `5.9980 us`
      - `text_trace_rustcode_read_only_range_run_split_unsigned_disabled`: `628.15 ms` to `629.00 ms`
      - `text_trace_rustcode_cursor_mapping_run_split_unsigned_enabled`: `768.22 ns` to `935.77 ns`
      - `text_trace_rustcode_cursor_mapping_run_split_unsigned_disabled`: `1.2673 s` to `1.2683 s`
      - directional delta: enabled remains orders-of-magnitude lower for run-split-heavy viewport and cursor probes.
- Densification memory refresh slice (2026-05-23):
  - Refreshed memory telemetry artifacts:
    - `artifacts/phase5-memory-refresh/large-doc-metrics.json`
    - `artifacts/phase5-memory-refresh/memory-telemetry.json`
  - Refreshed memory values:
    - large-doc `bytes_per_visible_char_lower_bound=89.77`
    - long-lived global `bytes_per_visible_char_lower_bound=114.34`
    - long-lived global `projection_bytes_lower_bound=498,744`, `visible_chars=4,362`
  - Interpretation:
    - memory remains above the documented target band (`~24 to 64` bytes/visible char); acceptance remains open.
    - latest refresh confirms benchmark-coverage additions did not materially change memory profile.
- Item 6 implementation slice (2026-05-23):
  - Densified run-level counted index storage in `core/src/text.rs`:
    - `CountedPositionIndex.weights`: `Vec<usize> -> Vec<u32>`
    - `CountedPositionIndex.fenwick`: `Vec<usize> -> Vec<u32>`
    - saturating conversion helpers preserve deterministic behavior while reducing index payload width.
  - Aligned memory telemetry estimator in `core/tests/text_large_doc_acceptance.rs` to u32 run-index accounting.
  - Validation:
    - `cargo test -p activesync-core text_projection_` => pass (20 passed, 0 failed)
    - `cargo test -p activesync-core --test text_range_convergence` => pass (7 passed, 0 failed)
  - New artifacts:
    - `artifacts/phase5-item6/large-doc-metrics.json`
    - `artifacts/phase5-item6/memory-telemetry.json`
  - Telemetry deltas vs `phase5-memory-refresh`:
    - large-doc `bytes_per_visible_char_lower_bound`: `89.77 -> 89.60` (improved)
    - long-lived global `bytes_per_visible_char_lower_bound`: `114.34 -> 113.43` (improved)
    - large-doc `index_bytes_lower_bound`: `8,297 -> 7,372` (improved)
    - long-lived global `index_bytes_lower_bound`: `35,484 -> 31,532` (improved)
  - Interpretation:
    - slice produced a measurable directional memory reduction, but acceptance remains open vs target band (`~24 to 64`).
- Item 7 implementation slice (2026-05-23):
  - Densified visible-id cache representation in `core/src/text.rs`:
    - `TextProjection.visible_ids`: `Vec<OpId> -> Vec<CompactId>`
    - `TextProjection.visible_pos_by_id`: `HashMap<OpId, usize> -> HashMap<CompactId, usize>`
    - added internal compact id mapping (`CompactId { actor_idx, lamport }`) backed by existing projection actor-table indirection.
  - Behavior contract preserved:
    - external APIs still accept/return canonical `OpId`; compact ids are projection-internal only.
    - range-anchor mapping and delete-window targeting still resolve against canonical ids at API boundaries.
  - Telemetry estimator alignment in `core/tests/text_large_doc_acceptance.rs`:
    - visible cache lower-bound now models compact id footprint (`u32 actor_idx + u64 lamport`) rather than full `OpId` storage.
  - Validation:
    - `cargo test -p activesync-core text_projection_` => pass (20 passed, 0 failed)
    - `cargo test -p activesync-core --test text_range_convergence` => pass (7 passed, 0 failed)
    - `cargo test -p activesync-core --test text_large_doc_acceptance -- --nocapture` => pass (2 passed, 0 failed)
  - New artifacts:
    - `artifacts/phase5-item7/large-doc-metrics.json`
    - `artifacts/phase5-item7/memory-telemetry.json`
  - Telemetry deltas vs `phase5-item6`:
    - large-doc `bytes_per_visible_char_lower_bound`: `89.60 -> 61.60` (improved)
    - long-lived global `bytes_per_visible_char_lower_bound`: `113.43 -> 85.43` (improved)
    - large-doc `projection_bytes_lower_bound`: `469,304 -> 322,640` (improved)
    - long-lived global `projection_bytes_lower_bound`: `494,792 -> 372,656` (improved)
  - Interpretation:
    - large-doc profile now lands inside target band (`<=64`), but long-lived global profile remains above target; acceptance remains open.
- Item 8 implementation slice (2026-05-24):
  - Densified projection metadata key storage in `core/src/text.rs`:
    - `TextProjection.entries`: `HashMap<OpId, ProjectionEntry> -> HashMap<CompactId, ProjectionEntry>`
    - `TextProjection.pending_deletes`: `HashSet<OpId> -> HashSet<CompactId>`
    - added compact-id boundary helpers (`compact_id_for`, `compact_id_existing`, `entry_for_id`) to preserve canonical `OpId` behavior at external boundaries.
  - Telemetry estimator alignment in `core/tests/text_large_doc_acceptance.rs`:
    - metadata-entry lower-bound now models compact id key footprint (`u32 actor_idx + u64 lamport`) rather than full `OpId` key storage.
  - Validation:
    - `cargo test -p activesync-core text_projection_` => pass (20 passed, 0 failed)
    - `cargo test -p activesync-core --test text_range_convergence` => pass (7 passed, 0 failed)
    - `cargo test -p activesync-core --test text_large_doc_acceptance -- --nocapture` => pass (2 passed, 0 failed)
  - New artifacts:
    - `artifacts/phase5-item8/large-doc-metrics.json`
    - `artifacts/phase5-item8/memory-telemetry.json`
  - Telemetry deltas vs `phase5-item7`:
    - large-doc `bytes_per_visible_char_lower_bound`: `61.60 -> 32.36` (improved)
    - long-lived global `bytes_per_visible_char_lower_bound`: `85.43 -> 47.04` (improved)
    - large-doc `projection_bytes_lower_bound`: `322,640 -> 169,508` (improved)
    - long-lived global `projection_bytes_lower_bound`: `372,656 -> 205,188` (improved)
  - Interpretation:
    - both large-doc and long-lived profiles now sit inside the documented target band (`~24 to 64`) in this profile run.
    - acceptance checkbox remains gated by canonical-profile evidence policy before final closure.
- Item 9 correctness + latency guard slice (2026-05-24):
  - Added projection correctness hardening tests in `core/src/text.rs`:
    - `text_projection_anchor_offset_roundtrip_under_split_merge_churn`
      - validates offset->anchor->offset roundtrip across split/merge churn and tombstone interactions.
    - `text_projection_tombstone_accounting_is_exact_and_idempotent`
      - validates tombstone counts are exact and duplicate deletes are idempotent.
  - Validation:
    - `cargo test -p activesync-core text_projection_` => pass (22 passed, 0 failed)
    - `cargo test -p activesync-core --test text_range_convergence` => pass (7 passed, 0 failed)
    - `cargo test -p activesync-core text_projection_parity_deterministic_corpus_has_zero_mismatches` => pass (1 passed, 0 failed)
  - Latency smoke checks (`--sample-size 10 --measurement-time 1 --warm-up-time 1`, bounded run-split knobs):
    - write-path (`core/benches/text_write_path.rs`):
      - rapid typing: enabled `~3.69 ms` vs disabled `~4.59 ms`
      - paste bursts: enabled `~368 us` vs disabled `~957 us`
      - remote burst batch: enabled `~5.32 ms` vs disabled `~7.17 ms`
      - mixed local+remote: enabled `~323 ms` vs disabled `~591 ms`
      - run churn: enabled `~638 ms` vs disabled `~1.27 s`
      - rebuild latency enabled: `~20.3 ms`
    - trace read/cursor (`core/benches/text_trace_rustcode.rs`):
      - read-only range: enabled `~2.70 us` vs disabled `~6.53 ms`
      - cursor mapping: enabled `~366 ns` vs disabled `~12.86 ms`
      - run-split read-only range: enabled `~5.11 us` vs disabled `~1.07 s`
      - run-split cursor mapping: enabled `~946 ns` vs disabled `~1.73 s`
      - apply-only: enabled `~7.12 ms` vs disabled `~6.87 ms` (near-parity class in this short profile)
  - Interpretation:
    - correctness guardrails for index/anchor placement and tombstone accounting remain green after densification.
    - latency class remains strong for read/cursor and read-heavy run-split scenarios; write-path enabled remains lower than disabled across burst/churn workloads in this profile.
- Canonical profile contract closure (2026-05-24):
  - Canonical contract validation commands:
    - `cargo test -p activesync-core --test text_range_convergence` => pass (7 passed, 0 failed)
    - `ACTIVESYNC_TEXT_LARGE_DOC_METRICS_PATH=artifacts/phase5-canonical-2026-05-24/large-doc-metrics.json ACTIVESYNC_TEXT_MEMORY_TELEMETRY_PATH=artifacts/phase5-canonical-2026-05-24/memory-telemetry.json cargo test -p activesync-core --test text_large_doc_acceptance -- --nocapture` => pass (2 passed, 0 failed)
  - Canonical artifact outputs:
    - `artifacts/phase5-canonical-2026-05-24/large-doc-metrics.json`
    - `artifacts/phase5-canonical-2026-05-24/memory-telemetry.json`
  - Canonical memory telemetry:
    - large-doc `bytes_per_visible_char_lower_bound=32.36`
    - long-lived global `bytes_per_visible_char_lower_bound=47.04`
    - canonical target-band verdict (`<=64`): pass
    - stretch target verdict (`<=40`): open
  - Canonical latency medians (Criterion `new/estimates.json`):
    - rapid typing: enabled `6.877 ms` vs disabled `4.588 ms` (disabled faster in this case)
    - paste bursts: enabled `647.088 us` vs disabled `959.686 us`
    - remote burst batch: enabled `5.419 ms` vs disabled `7.177 ms`
    - remote burst per-op: enabled `3.139 ms` vs disabled `8.044 ms`
    - mixed local+remote: enabled `181.483 ms` vs disabled `598.781 ms`
    - run churn: enabled `353.898 ms` vs disabled `1.230 s`
    - read-only range: enabled `1.527 us` vs disabled `6.242 ms`
    - cursor mapping: enabled `340.621 ns` vs disabled `12.387 ms`
    - run-split read-only range: enabled `6.466 us` vs disabled `1.086 s`
    - run-split cursor mapping: enabled `934.573 ns` vs disabled `2.189 s`
    - apply-only: enabled `3.591 ms` vs disabled `3.562 ms` (near parity)
    - rebuild latency (enabled): `10.638 ms`
  - Canonical closure decision:
    - core acceptance gates are satisfied for memory target band, parity/range convergence, and read/cursor latency class.
    - no mandatory additional densification slices are required to close this phase.
    - optional follow-up remains available for the stretch target (`<=40` bytes/visible-char global) and rapid-typing write-path optimization.
- Canonical guardrail re-verification pass (2026-05-24, post-invariant hardening):
  - Explicit guardrail tests:
    - `cargo test -p activesync-core text_projection_cold_compaction_preserves_semantics` => pass (1 passed, 0 failed)
    - `cargo test -p activesync-core text_projection_rebuild_from_oplog_reproduces_identical_run_materialization_outputs` => pass (1 passed, 0 failed)
  - Canonical correctness contract:
    - `cargo test -p activesync-core --test text_range_convergence` => pass (7 passed, 0 failed)
    - `cargo test -p activesync-core --test text_large_doc_acceptance -- --nocapture` => pass (2 passed, 0 failed)
  - Fresh artifact outputs:
    - `artifacts/phase5-canonical-2026-05-24-guardrails/large-doc-metrics.json`
    - `artifacts/phase5-canonical-2026-05-24-guardrails/memory-telemetry.json`
  - Guardrail-pass memory telemetry:
    - large-doc `bytes_per_visible_char_lower_bound=32.3559`
    - long-lived global `bytes_per_visible_char_lower_bound=47.0206`
    - canonical target-band verdict (`<=64`): pass
    - stretch target verdict (`<=40`): open
  - Current canonical KPI medians from Criterion `new/estimates.json`:
    - rapid typing: enabled `6.877 ms` vs disabled `4.588 ms` (disabled faster in this case)
    - paste bursts: enabled `647.088 us` vs disabled `959.686 us`
    - remote burst batch: enabled `5.419 ms` vs disabled `7.177 ms`
    - remote burst per-op: enabled `3.139 ms` vs disabled `8.044 ms`
    - mixed local+remote: enabled `181.483 ms` vs disabled `598.781 ms`
    - run churn: enabled `353.898 ms` vs disabled `1.230 s`
    - read-only range: enabled `1.527 us` vs disabled `5.622 ms`
    - cursor mapping: enabled `340.621 ns` vs disabled `11.118 ms`
    - run-split read-only range: enabled `6.466 us` vs disabled `525.806 ms`
    - run-split cursor mapping: enabled `934.573 ns` vs disabled `1.038 s`
    - apply-only: enabled `3.591 ms` vs disabled `7.088 ms`
    - rebuild latency (enabled): `10.638 ms`
  - Re-verification interpretation:
    - replayability/rebuildability/identity-preservation guardrails are now explicitly exercised and green in addition to canonical convergence checks.
    - physical densification remains constrained to lossless, replay-reversible representation changes.
- Rapid-typing split benchmark evidence (Option C, 2026-05-24):
  - Benchmark changes in `core/benches/text_write_path.rs`:
    - retained existing mixed case: `append_single_char` (apply stream + one final read)
    - added write-only case: `append_single_char_apply_only`
    - added interactive case: `append_single_char_periodic_read` (read every `ACTIVESYNC_TEXT_WRITE_TYPING_READ_EVERY`, default `32`)
  - Profile used for split run:
    - `ACTIVESYNC_TEXT_WRITE_TYPING_OPS=2000`
    - `ACTIVESYNC_TEXT_WRITE_TYPING_READ_EVERY=32`
    - Criterion flags: `--sample-size 10 --measurement-time 1 --warm-up-time 1`
  - Criterion median snapshot (`new/estimates.json`):
    - mixed final-read case:
      - enabled `4.225 ms`
      - disabled `5.066 ms`
    - apply-only case:
      - enabled `4.145 ms`
      - disabled `4.242 ms` (near parity)
    - periodic-read case (every 32 ops):
      - enabled `3.923 ms`
      - disabled `29.724 ms`
  - Interpretation:
    - write-only rapid typing is effectively near-parity between modes in this profile.
    - as soon as reads are interleaved at realistic cadence, enabled remains materially lower latency than disabled.
    - this split confirms the rapid-typing anomaly is largely workload-shape amortization (deferred disabled cost) rather than a broad enabled write-path regression.
- Rapid-typing low-risk optimization evidence (Option B slice 1, 2026-05-24):
  - Code change in `core/src/text.rs`:
    - optimized fast-append eligibility check to use compact-id comparisons (`CompactId`) instead of repeated `OpId` conversion/lookups in `apply_insert`/`can_fast_append` hot path.
    - preserved ordering/tombstone/rebuild semantics; no behavior-contract changes.
  - Correctness validation:
    - `cargo test -p activesync-core text_projection_` => pass (22 passed, 0 failed)
  - Re-benchmark method:
    - isolated per-case runs (one benchmark id per process) to avoid group-order and parallel contention noise.
    - knobs: `ACTIVESYNC_TEXT_WRITE_TYPING_OPS=2000`, `ACTIVESYNC_TEXT_WRITE_TYPING_READ_EVERY=32`
    - flags: `--sample-size 10 --measurement-time 1 --warm-up-time 1`
  - Criterion median snapshot (`new/estimates.json`, isolated runs):
    - `append_single_char_apply_only`:
      - enabled `3.955 ms` (was `4.145 ms` in Option C split baseline)
      - disabled `3.990 ms` (was `4.242 ms`)
    - `append_single_char_periodic_read`:
      - enabled `3.957 ms` (was `3.923 ms`, same latency class)
      - disabled `29.676 ms` (was `29.724 ms`, same latency class)
  - Interpretation:
    - enabled apply-only path improved directionally in this profile while preserving periodic-read behavior.
    - enabled retains strong advantage under periodic reads (orders of magnitude class separation maintained vs disabled).
    - this low-risk slice is safe to keep; additional write-path gains likely require deeper changes than fast-append eligibility checks alone.
- Rapid-typing stable-profile repetition pass (2026-05-24, 3x isolated repetitions):
  - Objective:
    - satisfy the evidence-quality rule (repeated directional stability) before closing this optimization pass.
  - Method:
    - repeated each split case in isolated process runs (no parallel benchmark contention):
      - `append_single_char_apply_only`
      - `append_single_char_periodic_read`
    - modes: enabled + disabled
    - knobs: `ACTIVESYNC_TEXT_WRITE_TYPING_OPS=2000`, `ACTIVESYNC_TEXT_WRITE_TYPING_READ_EVERY=32`
    - Criterion flags: `--sample-size 20 --measurement-time 2 --warm-up-time 1`
  - Repetition medians (ms):
    - apply-only:
      - enabled: `4.162`, `4.053`, `4.110` (avg `4.108`)
      - disabled: `3.971`, `4.072`, `4.389` (avg `4.144`)
    - periodic-read:
      - enabled: `4.147`, `4.131`, `4.169` (avg `4.149`)
      - disabled: `30.225`, `29.099`, `29.574` (avg `29.633`)
  - Repetition interpretation:
    - apply-only remains near-parity class with a slight enabled average advantage in this pass.
    - periodic-read remains consistently and materially faster in enabled mode (~7x class delta).
    - optimization pass is considered stable enough to keep and defer deeper write-path work.
- Run-split disabled-profile practicality note (2026-05-23):
  - High-churn disabled run (`ACTIVESYNC_TEXT_TRACE_RUN_SPLIT_BASE_LEN=4096`, `ACTIVESYNC_TEXT_TRACE_RUN_SPLIT_CHURN_STEPS=1000`) is operationally expensive for smoke profiling:
    - `text_trace_rustcode_read_only_range_run_split_unsigned_disabled`: `18.515 s` to `24.632 s`
    - `text_trace_rustcode_cursor_mapping_run_split_unsigned_disabled`: estimated multi-minute sample collection (`~367 s` class), unsuitable for routine quick iterations.
  - Recommended bounded comparability knobs for disabled-vs-enabled smoke profiles:
    - `ACTIVESYNC_TEXT_TRACE_RUN_SPLIT_BASE_LEN=2048`
    - `ACTIVESYNC_TEXT_TRACE_RUN_SPLIT_CHURN_STEPS=200`
    - `ACTIVESYNC_TEXT_TRACE_RUN_SPLIT_INSERT_LEN=4`
    - `ACTIVESYNC_TEXT_TRACE_RUN_SPLIT_DELETE_SPAN=3`
    - `ACTIVESYNC_TEXT_TRACE_RUN_SPLIT_WINDOWS=8`

## Rollout Sequence

- [x] Land Phase 1 behind default-off feature flag (historical milestone).
- [x] Enable parity mode in local/dev CI.
- [x] Flip default to projection after parity confidence threshold is met.
- [x] Land Phase 2 API additions (non-breaking).
- [x] Land Phase 2.5 windowing APIs.
- [x] Land Phase 3 index internals (no external API break required).
- [x] Sunset replay-per-resolve from runtime hot path once parity confidence is reached.

## Explicit Non-Goals

- [ ] Rich text schema/marks/blocks are out of scope for this migration.
- [ ] Server authority/policy model changes are out of scope.
- [ ] Serializing projection/index state into snapshot wire format is out of scope initially.

## Resolved Design Decisions

- [x] Projection is the canonical runtime materialization layer, not an optional long-term optimization.
- [x] Keep both compile-time and runtime controls for projection behavior.
- [x] Text remains speculative-first for runtime/UI reads.
- [x] Add resolve_text_canonical for persistence/export/audit use cases.
- [x] Bootstrap strategy is hybrid lazy-with-heat (lazy by default, eager rebuild for hottest keys only).
- [x] Do not enforce one-insert-per-tx invariant; allow future batching/range flows.
- [x] Expose both offset and anchor APIs; normalize offsets to anchors as early as possible.
- [x] Recompute projection from snapshot + delta; do not persist projection format initially.
- [x] Any deterministic parity mismatch blocks rollout.
- [x] Set explicit projection memory budget target (steady state):
  - [x] target ~24 to 64 bytes per visible character (projection-only structures; excludes oplog/node storage)
  - [x] ceiling ~2x to 4x raw UTF-8 bytes (warn above target band; investigate/mitigate above ceiling)
- [x] Decide and document offset semantics boundary:
  - [x] core internals use anchor-based addressing for correctness/convergence; offset helpers are normalized to anchors at API boundaries
  - [x] bridge/editor adapters perform UTF-16 conversion where needed; core text APIs remain UTF-8/Unicode-scalar based

## Additional Performance Enhancements (Beyond Current Checklist)

- [x] Add a projection-local cache for both `(OpId, char)` sequence and flattened `String`; update incrementally and avoid repeated `collect()` on every resolve.
- [x] Keep sibling lists incrementally ordered on insert (binary-search insertion) instead of global re-sort during resolve.
- [x] Track a per-key "dirty" bit so repeated reads with no writes are O(1) returns.
- [x] Add fast path for common append (`after == current tail`) to avoid unnecessary index traversal.
- [x] Reuse allocation buffers (`Vec` capacity reuse) in projection update paths to reduce allocator churn under keystroke workloads.
- [x] Add lightweight counters/telemetry in core (resolve calls, projection hits, fallback hits, parity mismatches, projection rebuilds).
- [x] Split benchmark reporting into apply cost vs read cost (today trace bench blends replay and resolve).
- [x] Add targeted cursor benchmark for repeated index-to-anchor and anchor-to-index lookups to validate Phase 3 index ROI.
- [x] Keep sibling representation simple initially (Vec/smallvec) and upgrade only if measured fanout requires it.

## Implementation Readiness Checklist (Phase 0)

Status: [x] complete

- [x] Confirm module placement and ownership (`core/src/text.rs` vs `core/src/text_projection.rs`) and wire `core/src/lib.rs` exports if needed.
- [x] Add `StateGraph` field(s) for projection state and feature toggles without changing public constructor behavior.
- [x] Define projection update hooks in exactly three write paths:
  - [x] `apply_local`
  - [x] `apply_remote`
  - [x] `apply_remote_batch` (accepted nodes only, finalized graph insertion order)
- [x] Implement projection internals with explicit separation of metadata graph and visible sequence.
- [x] Implement incremental UTF-8 buffer maintenance with dirty ranges.
- [x] Define mismatch handling contract: parity mismatch triggers self-heal rebuild + diagnostics.
- [x] Add deterministic parity test harness that runs same trace through projection and legacy and compares full sequence + string outputs.
- [x] Extend `core/benches/text_trace_rustcode.rs` with projection on/off mode switch and emit both latency and speedup ratio.
- [x] Add benchmark mode for windowed range reads.
- [x] Add rollout docs snippet (how to enable flag, how to enable parity mode, how to force legacy mode).
- [x] Pre-merge validation command set (documented and repeatable):
  - [x] `cargo test -p activesync-core`
  - [x] `cargo bench -p activesync-core --bench text_trace_rustcode`
  - [x] smoke checks for primary bridge text flows (`insert_text` / `delete_text` / `resolve_text`)

Phase 0 evidence (local run):
- `cargo test -p activesync-core` => pass (178 unit tests + fixture tests green).
- Bounded benchmark (`ACTIVESYNC_TEXT_TRACE_MAX_OPS=2000`):
  - disabled median: ~231.99 ms
  - enabled median: ~202.39 ms
  - parity median: ~1.6587 s

Parity overhead explanation:
- Current parity mode verifies correctness by comparing projection output to replay output after text updates.
- Replay comparison is intentionally expensive because it re-derives sequence from oplog for touched keys and may trigger self-heal rebuilds on mismatch.
- This safety cost is expected in validation mode and is not intended for steady-state production hot path.

## Working Notes

- This plan optimizes runtime materialization and indexing, not CRDT convergence semantics.
- Replay-per-resolve remains validation/repair infrastructure during migration and is removed from hot path once confidence is met.
- Projection/index persistence is intentionally deferred until runtime format and instrumentation stabilize.
