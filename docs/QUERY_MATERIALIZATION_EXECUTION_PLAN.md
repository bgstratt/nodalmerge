# Query and Materialization Execution Plan

Owner: Core/runtime
Status: InProgress (Phase D kickoff; Phase C parity complete across host/core/server/ws)
Last updated: 2026-05-27

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

Phase A kickoff record:
1. Kickoff date (UTC): 2026-05-26
2. Owner: Brad
3. Current focus: command/event contract draft outline plus websocket parity mapping table skeleton
4. Next checkpoint: submit initial contract draft and parity matrix headings for review

Phase A working draft - host command/event contract (v1):

| Operation | Host command (request) | Host event/response (result) | Determinism requirement | Notes |
|---|---|---|---|---|
| Register query spec | `RegisterQuerySpec { query_spec_id, version, descriptor, options }` | `QuerySpecRegistered { query_spec_id, version, canonical_hash, accepted }` or `QuerySpecRejected { query_spec_id, version, reason_class, reason_message }` | Same `(query_spec_id, version, descriptor)` input yields stable acceptance/rejection outcome at same policy+capability state. | Descriptor is schema-agnostic in v1 and cannot imply DAG mutation.
| Build projection | `BuildProjection { projection_id, query_spec_id, target_checkpoint }` | `ProjectionBuildCompleted { projection_id, checkpoint, digest }` or `ProjectionBuildRejected { projection_id, reason_class, reason_message }` | Build result at identical checkpoint is digest-stable. | `target_checkpoint` resolves to canonical-only cut.
| Read projection | `ReadProjection { projection_id, page_token, limit }` | `ProjectionReadResult { projection_id, checkpoint, rows, digest, next_page_token }` | Row order and payload are deterministic for same checkpoint and query version. | Pagination tokens must be replay-stable.
| Invalidate projection | `InvalidateProjection { projection_id, reason }` | `ProjectionInvalidated { projection_id, reason, invalidated_at_hlc }` | Invalidation reason class is stable and bounded-cardinality. | Invalidations are metadata events; no DAG mutation.
| List projections | `ListProjections { query_spec_id?, state_filter? }` | `ProjectionListResult { items, cursor? }` | Item ordering is deterministic under same filter/checkpoint metadata. | Supports operator inspection and audit workflows.

Phase A working draft - websocket parity matrix headings:

| Capability path | Host command/event path | WS request shape | WS response/event shape | Parity status | Open questions |
|---|---|---|---|---|---|
| Success path - register spec | `RegisterQuerySpec` -> `QuerySpecRegistered` | `query.register` | `query.registered` | Draft | Finalize descriptor envelope fields.
| Success path - build projection | `BuildProjection` -> `ProjectionBuildCompleted` | `projection.build` | `projection.build.completed` | Draft | Confirm checkpoint selector encoding.
| Success path - read projection | `ReadProjection` -> `ProjectionReadResult` | `projection.read` | `projection.read.result` | Draft | Confirm page token stability contract.
| Success path - list projections | `ListProjections` -> `ProjectionListResult` | `projection.list` | `projection.list.result` | Draft | Confirm default ordering and cursor shape.
| Deny/error path - capability reject | `<any command>` -> `*Rejected` | `<same request type>` | `error` with reason taxonomy | Draft | Map capability names to reason classes.
| Invalidation path | `InvalidateProjection` -> `ProjectionInvalidated` | `projection.invalidate` or server-side invalidation trigger | `projection.invalidated` | Draft | Decide explicit client invalidate support in v1.

Phase A versioning and compatibility notes (initial):
1. `QuerySpecVersion` is required on registration and immutable once accepted.
2. Host must reject unsupported versions with deterministic `reason_class` (`reject.query_unsupported_version`).
3. Backward compatibility window is explicit and host-configurable; defaults are documented per release.
4. WS envelopes must carry version fields needed for parity diagnostics.

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
7. `QUERY-REPLAY-002`: mismatch diagnostics include checkpoint + digest metadata
8. `QUERY-WS-PARITY-001`: ws mapping parity for query/projection rejection taxonomy and metadata

Wave 1 initial stub evidence (2026-05-27):
1. `QUERY-DET-001` executable stub is passing in `core/tests/query_materialization_vectors.rs`.
2. `QUERY-REPLAY-001` executable stub is passing in `core/tests/query_materialization_vectors.rs`.
3. `QUERY-DET-002` executable host stub is passing in `nodalmerge-host/tests/NodalMerge.DotNetHost.Tests/QueryMaterializationVectorsTests.cs`.
4. `QUERY-INVAL-001` executable host stub is passing in `nodalmerge-host/tests/NodalMerge.DotNetHost.Tests/QueryMaterializationVectorsTests.cs`.
5. Projection pagination window semantics (`limit` + `page_token`) are implemented in runtime stub read path with deterministic digest continuity across page windows.
6. Acceptance artifacts recorded:
   - `docs/acceptance/query-det-001.json`
   - `docs/acceptance/query-replay-001.json`
   - `docs/acceptance/query-det-002.json`
   - `docs/acceptance/query-inval-001.json`
   - `docs/acceptance/query-replay-001-host.json`
   - `docs/acceptance/query-replay-002-host.json`
   - `docs/acceptance/query-compat-reject-001-host.json`
   - `docs/acceptance/query-replay-002-core.json`
   - `docs/acceptance/query-replay-002-server.json`
   - `docs/acceptance/query-compat-reject-001-core.json`
   - `docs/acceptance/query-compat-reject-001-server.json`
   - `docs/acceptance/query-ws-parity-001-host.json`
7. Phase B host stub lift has replaced build-time synthetic projection rows with canonical runtime map-state row generation (including `map-set`/`map-delete` mutation tracking) and is covered by `Projection_build_uses_canonical_map_state_and_reflects_map_mutations` in `nodalmerge-host/tests/NodalMerge.DotNetHost.Tests/RuntimeMessageProcessorTests.cs`.
8. Phase C host replay compatibility stub is now executable via explicit checkpoint selection (`target_checkpoint.selector=seq`, `canonical_seq`) and covered by `QueryReplay001LiveVsReplayParityAtExplicitCheckpoint` plus deterministic rejection coverage in `Projection_build_rejects_unknown_checkpoint_sequence`.
9. Phase C host replay selector expansion now supports equivalent checkpoint addressing by sequence (`selector=seq`), canonical hash (`selector=hash`), and frontier token (`selector=frontier`) with digest parity coverage in `QueryReplaySelectorEquivalenceSeqHashAndFrontier` and deterministic unknown-hash rejection in `Projection_build_rejects_unknown_checkpoint_hash`.
10. Compatibility lane for selector payload validation is now active with bounded rejection taxonomy assertions: malformed frontier tokens, mixed selector fields, and canonical hash format violations reject as `reject.checkpoint_selector_invalid`, while well-formed but unknown checkpoints reject as `reject.checkpoint_not_found` (`QueryCompatReject001SelectorPayloadValidationBoundedTaxonomy`, `Projection_build_selector_payload_validation_uses_bounded_rejection_taxonomy`).
11. Phase C parity is complete across host/core/server/ws lanes: core/server vectors now mirror selector-validation taxonomy and mismatch diagnostics (`query_compat_reject_001_selector_payload_validation_bounded_taxonomy`, `server_query_compat_reject_001_selector_payload_validation_bounded_taxonomy`, `query_replay_002_mismatch_diagnostics_include_checkpoint_and_digest_metadata`, `server_query_replay_002_mismatch_diagnostics_include_checkpoint_and_digest_metadata`) and ws mapping parity now asserts rejection reason class/message plus checkpoint/digest metadata surfaces (`Event_mapper_converts_query_projection_events_to_runtime_messages`).
12. With Phase C parity complete for this slice, execution focus is moved to Phase D deliverables (SDK/operator surface).
13. Phase D SDK surface implementation has started in `sdk-js`: canonical-lane query/projection helpers (`registerSpec`, `buildProjection`, `readProjection`, `invalidateProjection`, `listProjections`) now emit parity request envelopes and await typed runtime responses, with unit coverage in `sdk-js/index.test.js` for checkpoint selector validation and projection build/read response matching.
14. Phase D parity coverage now includes deterministic rejected-path SDK tests for register/build/read/invalidate/list (`query.register.rejected`, `projection.build.rejected`, `projection.read.rejected`, `projection.invalidate.rejected`, `projection.list.rejected`) and operator runbook lifecycle guidance for register/build/read/invalidate/list including failure triage keyed by bounded `reason_class` taxonomy.
15. Phase D pagination parity vectors now include deterministic multi-page ordering and digest continuity checks in both core/server suites (`query_det_003_pagination_multi_page_order_is_deterministic`, `query_det_004_pagination_digest_continuity_matches_full_projection`, `server_query_det_003_pagination_multi_page_order_is_deterministic`, `server_query_det_004_pagination_digest_continuity_matches_full_projection`).

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
