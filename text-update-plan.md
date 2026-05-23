# Text Update Plan

Goal: Improve text runtime performance by moving from replay-per-resolve to incrementally maintained projection, while preserving existing RGA convergence semantics.

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

## Phase 1 - TextProjection in StateGraph

Status: [ ] not started

Execution checklist:
- [ ] Add TextProjection data model in core/src/text.rs or core/src/text_projection.rs with per-key state.
- [ ] Add text_projections: HashMap<String, TextProjection> to StateGraph.
- [ ] Update apply_local success path to incrementally apply text ops into projection.
- [ ] Update apply_remote success path to incrementally apply text ops into projection.
- [ ] Update apply_remote_batch success path to incrementally apply accepted nodes in batch order.
- [ ] Keep legacy resolver intact as fallback path.
- [ ] Route resolve_text and resolve_text_seq through projection when enabled.

Acceptance criteria:
- [ ] Existing text tests in core/src/text.rs pass unchanged.
- [ ] Existing bridge text behavior remains API-compatible (text_insert, text_delete, text_resolve).
- [ ] No replay or convergence regressions in concurrent insert/delete scenarios.

## Phase 1.5 - Safety Fallback and Parity Checks

Status: [ ] not started

Execution checklist:
- [ ] Add feature flag for projection read path (example: text_projection).
- [ ] Add optional parity-check mode that compares projection results against legacy resolver for sampled keys or calls.
- [ ] Add mismatch diagnostics (key, node id, op id context).
- [ ] Add a benchmark mode that reports projection vs legacy resolve timing.

Acceptance criteria:
- [ ] Zero parity mismatches across deterministic test corpus.
- [ ] Projection path shows measurable speedup on text trace workloads.
- [ ] Easy rollback to legacy path via feature flag.

## Phase 2 - Range Ops with Char Compatibility

Status: [ ] not started

Execution checklist:
- [ ] Extend TextOp with range operations (insert/delete spans) while retaining existing char ops.
- [ ] Define canonical lowering rules so char and range ops compose deterministically.
- [ ] Update projection updater to apply both op forms.
- [ ] Expose bridge methods for range insertion/deletion while keeping existing methods stable.
- [ ] Add SDK convenience helpers to prefer range ops for paste/replace flows.

Acceptance criteria:
- [ ] Backward compatibility: old char-only peers interoperate correctly.
- [ ] Cross-peer convergence is identical for equivalent char-vs-range edit traces.
- [ ] Metadata and allocation overhead reduced for large contiguous edits.

## Phase 3 - Counted Position Index (log n mapping)

Status: [ ] not started

Execution checklist:
- [ ] Introduce counted index structure for visible length accounting (tree/rope/B-tree variant).
- [ ] Implement offset to anchor and anchor to offset APIs using subtree counts.
- [ ] Integrate index maintenance into incremental projection updates.
- [ ] Add stress tests for large docs and random cursor movement/edit patterns.

Acceptance criteria:
- [ ] Position mapping complexity scales as O(log n) in benchmarked workloads.
- [ ] Cursor-heavy operations avoid full-sequence scans.
- [ ] No convergence or ordering regressions.

## Test and Validation Plan

- [ ] Keep existing unit tests as baseline guardrails.
- [ ] Add projection-specific unit tests:
  - [ ] out-of-order remote insert application
  - [ ] tombstone interactions with descendants
  - [ ] sibling ordering under identical after
  - [ ] multi-key isolation
- [ ] Add parity tests (projection output equals legacy output for randomized traces).
- [ ] Extend text_trace_rustcode benchmarking with projection on/off variants.
- [ ] Record perf deltas after each phase.

## Rollout Sequence

- [ ] Land Phase 1 behind default-off feature flag.
- [ ] Enable parity mode in local/dev CI.
- [ ] Flip default to projection after parity confidence threshold is met.
- [ ] Land Phase 2 API additions (non-breaking).
- [ ] Land Phase 3 index internals (no external API break required).

## Explicit Non-Goals

- [ ] Rich text schema/marks/blocks are out of scope for this migration.
- [ ] Server authority/policy model changes are out of scope.
- [ ] Snapshot wire format changes are out of scope unless needed later for projection persistence.

## Working Notes

- This plan optimizes text runtime representation, not CRDT convergence rules.
- Host applications can remain on older bridge/NuGet while this evolves.
- Bridge and SDK updates are expected but can be staged and mostly backward compatible.
