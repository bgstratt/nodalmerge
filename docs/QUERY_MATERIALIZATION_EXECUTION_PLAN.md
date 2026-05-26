# Query and Materialization Execution Plan

Owner: Core/runtime
Status: Planned
Last updated: 2026-05-25

## 1. Why this plan exists

After fork-lineage and speculative-vs-authoritative semantics, the next platform-level capability is deterministic query/materialization.

Without a first-class runtime contract, each product builds ad-hoc indexing and projection logic, which causes:

1. inconsistent correctness semantics across hosts
2. replay mismatch risk between live and reconstructed views
3. duplicated complexity in SDKs and adapters

## 2. Scope and non-goals

In scope:

1. deterministic query contract over authoritative/canonical state
2. materialized projection lifecycle and invalidation semantics
3. replay-compatible query results and checkpoint behavior
4. host-core and websocket parity requirements
5. conformance vectors and acceptance harness additions

Out of scope (v1):

1. full SQL layer
2. distributed/federated query planner
3. product-specific ranking/relevance policies

## 3. Core invariants

1. Query determinism: same canonical input state and same query spec always produce identical results.
2. Replay determinism: replaying to checkpoint C and querying at C matches live query result recorded at C.
3. Projection isolation: query/materialization must not mutate DAG state.
4. Lane discipline: authoritative/canonical lane is source of truth for v1 query semantics.
5. Compatibility discipline: query spec changes are versioned and backward-compatible within declared windows.

## 4. Proposed runtime primitives (v1)

1. `QuerySpecId`
2. `QuerySpecVersion`
3. `ProjectionId`
4. `ProjectionCheckpoint` (frontier + canonical hash)
5. `ProjectionDigest` (stable summary for parity checks)
6. `ProjectionInvalidationReason`

Minimal conceptual operations:

1. `RegisterQuerySpec`
2. `BuildProjection`
3. `ReadProjection`
4. `InvalidateProjection`
5. `ListProjections`

## 5. Phased implementation

### Phase A - Contract freeze

Deliverables:

1. host-core query/materialization command/event contract draft
2. websocket parity mapping table
3. versioning and compatibility notes

Acceptance criteria:

1. contract types compile in host-core without runtime-owned scheduler types
2. parity matrix covers success + deny/error + invalidation paths

### Phase B - Canonical execution path

Deliverables:

1. canonical query evaluator wired to authoritative state
2. projection build/read APIs in runtime adapter
3. stable projection digest generation

Acceptance criteria:

1. identical result and digest under repeated execution at same checkpoint
2. no DAG mutation side effects

### Phase C - Replay compatibility

Deliverables:

1. replay-time query hooks at explicit checkpoint cut
2. snapshot/pack metadata required for projection checkpoint verification
3. replay parity tests

Acceptance criteria:

1. live-vs-replay equivalence vectors pass
2. mismatch diagnostics include checkpoint and digest metadata

### Phase D - SDK/operator surface

Deliverables:

1. SDK projection/query read APIs for canonical lane
2. operator docs and examples for build/read/invalidate flow
3. metrics and rejection taxonomy updates

Acceptance criteria:

1. SDK parity test suite passes across wasm/web and host paths
2. operator runbook includes failure and recovery guidance

### Phase E - Performance hardening

Deliverables:

1. projection cache policy and bounded memory behavior
2. indexing strategy guardrails and benchmark baselines
3. backpressure behavior for long-running projection builds

Acceptance criteria:

1. benchmark targets and memory ceilings are documented and reproducible
2. projection rebuild under pressure remains deterministic

## 6. Conformance vector additions

Add vectors under a new family:

1. `QUERY-DET-001`: deterministic result ordering and payload equality
2. `QUERY-DET-002`: deterministic digest equality
3. `QUERY-REPLAY-001`: live vs replay parity at checkpoint
4. `QUERY-INVAL-001`: invalidation reason propagation parity (host-core/ws)
5. `QUERY-COMPAT-001`: older `QuerySpecVersion` acceptance within compatibility window
6. `QUERY-COMPAT-REJECT-001`: unsupported version deterministic rejection

## 7. Risks and mitigations

1. Risk: Hidden app-specific assumptions leak into core query contract.
   Mitigation: keep v1 contract minimal and schema-agnostic.
2. Risk: replay/query parity drift.
   Mitigation: mandatory checkpoint digest validation vectors.
3. Risk: projection memory growth.
   Mitigation: explicit cache policy and operator limits in Phase E.

## 8. Recommended sequencing

1. Complete speculative/authoritative Phase A-B.
2. Complete replay branching Phase A-C.
3. Execute this query/materialization plan Phase A-C.
4. Run Phase D-E in parallel with broader future-state hardening.
