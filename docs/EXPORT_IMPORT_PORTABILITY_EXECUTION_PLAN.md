# Export and Import Portability Execution Plan

Owner: Core/runtime
Status: InProgress (Phase D closed for this slice; post-closeout monitoring active)
Last updated: 2026-05-27

## 1. Why this plan exists

Replay branching introduces lineage-aware payloads, but platform portability needs a first-class archive contract that is deterministic, versioned, and host-agnostic.

Without this plan, export/import behavior tends to fragment across products and host adapters, which causes:

1. non-portable backups and migration workflows
2. weak audit and reproducibility guarantees
3. drift in compatibility handling across runtimes

## 2. Scope and non-goals

In scope:

1. portable archive manifest and payload schema
2. full-room clone and checkpoint-fork export semantics
3. deterministic import verification and compatibility checks
4. host-core and websocket parity for export/import operations
5. conformance vectors for roundtrip parity and failure classes
6. Next checkpoint: maintain post-closeout monitoring cadence and record threshold recalibration updates when triggered
Out of scope (v1):

1. cloud vendor-specific backup orchestration
2. incremental remote sync protocol redesign
3. multi-authority federation handoff semantics

## 3. Core invariants

1. Deterministic export: same room state and cut produces identical manifest metadata and digest set.
2. Deterministic import: importing an archive into a compatible runtime yields canonical hash parity at declared checkpoint.
3. Integrity-first: import refuses payloads with digest, signature, or schema mismatch.
4. Compatibility-explicit: archive format and feature versions are validated against declared support windows.
5. Auditability: every export/import includes stable provenance metadata for traceability.

## 4. Proposed archive primitives (v1)

1. `ArchiveFormatVersion`
2. `ArchiveManifestId`
3. `ArchiveKind` (`full_clone`, `checkpoint_fork`)
4. `ArchiveCheckpoint` (frontier + canonical hash + optional policy timeline hash)
5. `ArchivePayloadDigestSet`
6. `ArchiveCompatibilityWindow`
7. `ArchiveProvenance` (source room, exported_at, tool/runtime version)

Minimal conceptual operations:

1. `ExportRoomArchive`
2. `ExportCheckpointArchive`
3. `ValidateArchive`
4. `ImportArchive`
5. `DescribeArchive`

## 5. Phased implementation

6. Core/server Phase A parity vectors now include archive envelope and deterministic rejection taxonomy coverage:
   - `archive_describe_001_envelope_contains_manifest_checkpoint_and_digest_metadata`
   - `archive_validate_reject_001_manifest_invalid_uses_deterministic_reason_class`
   - `archive_import_reject_001_digest_mismatch_uses_deterministic_reason_class`
   - server mirrors: `*_server_path` variants in `server/tests/archive_portability_vectors.rs`
   - acceptance artifacts:
     - `docs/acceptance/archive-phasea-parity-core.json`
     - `docs/acceptance/archive-phasea-parity-server.json`
     - `docs/acceptance/archive-phasea-contract-types.json`
     - `docs/acceptance/archive-phasea-runtime-adapter.json`
     - `docs/acceptance/archive-phasea-runtime-processors.json`
     - `docs/acceptance/archive-phasea-negative-runtime.json`
   - `docs/acceptance/archive-phaseb-export-runtime-roundtrip.json`
   - `docs/acceptance/archive-phaseb-closeout.json`
   - `docs/acceptance/archive-phasec-scope-open.json`
   - `docs/acceptance/archive-phasec-policy-timeline-parity.json`
   - `docs/acceptance/archive-phasec-range-cutover-parity.json`
   - `docs/acceptance/archive-phasec-transition-progression.json`
   - `docs/acceptance/archive-phasec-mixed-range-boundaries.json`
   - `docs/acceptance/archive-phasec-nonzero-transition-progression.json`
   - `docs/acceptance/archive-phasec-closeout.json`
   - `docs/acceptance/archive-phased-scope-open.json`
   - `docs/acceptance/archive-phased-drill-run01.json`
   - `docs/acceptance/archive-phased-drill-run01-003-004.json`
   - `docs/acceptance/archive-phased-benchmark-baseline-run01.json`
7. Shared archive contract types are now promoted in `nodalmerge-core` (`core/src/archive_contracts.rs`) and consumed by core/server archive parity vectors, including deterministic reason-class enums and ws request/response envelopes.
8. Runtime adapter consumption is now active in server ingress paths: `server/src/adapter_context.rs` routes `archive.describe|archive.validate|archive.import|archive.export`, `server/src/ws_handler.rs` emits shared `ArchiveWsResponse` envelopes, and `host-core/src/protocol.rs` provides shared envelope serialization helpers.
9. Persistence-backed runtime processors are now active in `server/src/archive_adapter.rs`: archive describe/validate/import load persisted room data (`room://` refs), compute deterministic digest/checkpoint metadata, enforce expected checkpoint verification on import, and are covered by host migration parity fixture `archive_runtime_adapter_room_ref`.
10. External non-room-local archive providers and signature verification are now active: `file://` and `object://` manifest sources are supported with deterministic rejection classes for unsupported format, signature invalid, and checkpoint not found, covered by host migration parity fixtures.
11. Phase B export runtime integration is active: deterministic builders now back `archive.export` responses (`archive.export.result`/`archive.export.rejected`) and generated file/object manifests are consumed by roundtrip parity vectors asserting deterministic import canonical-hash parity.
12. Phase B conformance hardening is complete: export manifest output now includes compatibility-window metadata and payload digest policy declarations, and both server vectors + websocket-facing parity fixtures pin deterministic rejection mappings for unsupported compatibility windows and unsupported payload digest policies.
13. Phase C initial implementation is active: export manifest output and `archive.export.result` envelopes now carry `policy_timeline_hash` metadata, external manifest signatures bind that field, and runtime validate/import deterministically reject policy timeline mismatches.
14. Phase C conformance vectors now include policy timeline mismatch lanes in server vectors and websocket-facing parity harness fixtures.
15. Phase C compatibility semantics are broadened: runtime now enforces explicit compatibility-window range overlap behavior (instead of fixed-window equality assumptions) for external manifest acceptance/rejection.
16. Phase C policy timeline parity metadata now includes both `policy_timeline_hash` and `policy_timeline_cutover_lamport` in signed manifests and export envelopes, with deterministic mismatch vectors in core/server/ws lanes.
17. Phase C transition progression parity is now active: signed manifests and export envelopes include `policy_timeline_transition_cutovers`, runtime validate/import enforce deterministic progression-shape checks (`non-empty`, `strictly increasing`, `ends_with(policy_timeline_cutover_lamport)`), and server/core/host lanes include deterministic conformance coverage.
18. Phase C mixed-range migration boundary vectors are now active in server + websocket parity suites, pinning deterministic outcomes for no-overlap reject and edge-overlap accept compatibility-window behavior.
19. Phase C positive non-zero transition progression vectors are now active: room runtime tracks monotonic policy cutover history, and server/websocket vectors verify non-zero `policy_timeline_cutover_lamport` + `policy_timeline_transition_cutovers` parity on export and validate flows.
20. Phase C closeout rerun is complete: full archive conformance suites in core/server/websocket lanes are green and recorded in `docs/acceptance/archive-phasec-closeout.json`.
21. Phase D scope is now open for operationalization: migration drill matrix and baseline benchmark gates are defined in this plan and recorded in `docs/acceptance/archive-phased-scope-open.json`.
22. Phase D drill run-01 for ARCHIVE-DRILL-001/002 is complete with passing server/ws conformance lanes and recorded in `docs/acceptance/archive-phased-drill-run01.json`.
23. Phase D drill run-01 for ARCHIVE-DRILL-003/004 is complete with passing server/ws conformance lanes and recorded in `docs/acceptance/archive-phased-drill-run01-003-004.json`.
24. Phase D baseline benchmark profile run-01 for DRILL-001/002 is recorded in `docs/acceptance/archive-phased-benchmark-baseline-run01.json`; latency gates are currently above target while memory stays within target.
25. Phase D baseline benchmark profile run-02 is recorded in `docs/acceptance/archive-phased-benchmark-baseline-run02.json` with runtime-aligned in-process measurements, all latency gates passing at p95, and memory gate still passing.
26. Phase D benchmark profile run-03 is recorded in `docs/acceptance/archive-phased-benchmark-baseline-run03.json`: second-slice manifest metadata cache optimization further reduces validate/import p95 while retaining gate pass across latency and memory.
27. Cache-hit telemetry is now instrumented for external manifest metadata loads (`nodalmerge_archive_manifest_cache_lookup_total`, `nodalmerge_archive_manifest_cache_lookup_seconds`) with focused cache miss-hit-invalidation test coverage.
28. Phase D object-manifest parity benchmark run-04 is recorded in `docs/acceptance/archive-phased-benchmark-baseline-run04.json`; file/object p95 deltas are within the 5 ms threshold while latency and memory gates remain green for both lanes.
29. Operator alert thresholds are now defined and recorded in `docs/acceptance/archive-phased-alert-thresholds-run01.json` for cache miss ratio, object/file parity drift, and absolute latency safety rails.
30. Operator runbook/dashboard wiring is complete and recorded in `docs/acceptance/archive-phased-operator-alert-runbook-run01.json`, including panel contract, on-call ownership, and escalation flow.
31. Alert-route tabletop drill run-01 is complete and recorded in `docs/acceptance/archive-phased-alert-route-tabletop-run01.json` with warn and critical paths meeting acknowledgement/escalation timing targets.
32. Alert template publication checkpoint is complete and recorded in `docs/acceptance/archive-phased-alert-template-publication-run01.json`.
33. Live dashboard annotation + incident ticket dry-run run-01 is complete and recorded in `docs/acceptance/archive-phased-alert-dryrun-run01.json`.
34. Critical-route live dry-run run-02 is complete and recorded in `docs/acceptance/archive-phased-alert-dryrun-run02.json` with L3 freeze/rollback decision logging evidence.
35. Warn + critical dry-run evidence is consolidated and Phase D operational closeout recommendation is recorded in `docs/acceptance/archive-phased-operational-closeout-recommendation-run01.json`.
36. Runtime-owner signoff is complete and Phase D closeout is approved in `docs/acceptance/archive-phased-operational-closeout-signoff-run01.json`.

### Phase A - Contract freeze

Deliverables:

1. archive manifest schema and versioning policy
2. host-core export/import command-event contract draft
3. websocket/admin parity mapping and error taxonomy

Acceptance criteria:

1. contract compiles without runtime-specific scheduler ownership types
2. explicit compatibility-window behavior documented for unsupported versions

Phase A kickoff record:
1. Kickoff date (UTC): 2026-05-27
2. Owner: Brad
3. Current focus: Phase C execution for explicit compatibility range semantics and policy timeline cutover parity metadata across lanes.
4. Mixed-range migration boundary checkpoint complete: server + websocket conformance vectors now pin no-overlap reject and edge-overlap accept semantics.
5. Positive non-zero transition progression checkpoint complete: runtime policy timeline history now yields multi-cutover parity metadata and vectors for deterministic export/validate behavior.
6. Transition progression checkpoint complete: `policy_timeline_transition_cutovers` is now signed and validated with deterministic rejection vectors.
7. Phase C closeout evidence bundle is recorded in `docs/acceptance/archive-phasec-closeout.json`.
8. Phase D scope open evidence is recorded in `docs/acceptance/archive-phased-scope-open.json`.
9. ARCHIVE-DRILL-001 and ARCHIVE-DRILL-002 run-01 evidence is recorded in `docs/acceptance/archive-phased-drill-run01.json`.
10. ARCHIVE-DRILL-003 and ARCHIVE-DRILL-004 run-01 evidence is recorded in `docs/acceptance/archive-phased-drill-run01-003-004.json`.
11. Baseline benchmark profile run-01 for DRILL-001/002 is recorded in `docs/acceptance/archive-phased-benchmark-baseline-run01.json`.
12. Baseline benchmark profile run-02 for DRILL-001/002 is recorded in `docs/acceptance/archive-phased-benchmark-baseline-run02.json`.
13. Baseline benchmark profile run-03 for DRILL-001/002 is recorded in `docs/acceptance/archive-phased-benchmark-baseline-run03.json`.
14. Cache-hit telemetry checkpoint is complete for external manifest metadata loads.
15. Object-manifest parity benchmark run-04 is recorded in `docs/acceptance/archive-phased-benchmark-baseline-run04.json`.
16. Operator alert thresholds checkpoint is complete and recorded in `docs/acceptance/archive-phased-alert-thresholds-run01.json`.
17. Operator runbook and dashboard wiring checkpoint is complete and recorded in `docs/acceptance/archive-phased-operator-alert-runbook-run01.json`.
18. Alert-route tabletop drill run-01 is complete and recorded in `docs/acceptance/archive-phased-alert-route-tabletop-run01.json`.
19. Dashboard annotation and incident ticket templates are published and recorded in `docs/acceptance/archive-phased-alert-template-publication-run01.json`.
20. Live dashboard annotation + incident ticket dry-run run-01 is complete and recorded in `docs/acceptance/archive-phased-alert-dryrun-run01.json`.
21. Critical-route live dry-run run-02 is complete and recorded in `docs/acceptance/archive-phased-alert-dryrun-run02.json` with L3 freeze/rollback decision logging evidence.
22. Warn + critical dry-run evidence is consolidated and Phase D operational closeout recommendation is recorded in `docs/acceptance/archive-phased-operational-closeout-recommendation-run01.json`.
23. Runtime-owner signoff is complete and Phase D closeout is approved in `docs/acceptance/archive-phased-operational-closeout-signoff-run01.json`.
24. Next checkpoint: maintain post-closeout monitoring cadence and record threshold recalibration updates when triggered.

### Phase D scope opening record (operationalization matrix)

Phase D kickoff record:
1. Scope-open timestamp: 2026-05-26T23:33:20.7491872-05:00
2. Evidence artifact: `docs/acceptance/archive-phased-scope-open.json`
3. Focus: operator-facing migration drills with deterministic pass/fail gates and benchmark ceilings.

Migration drill matrix:

| Drill ID | Lane | Scenario | Deterministic pass criteria | Deterministic fail criteria |
|---|---|---|---|---|
| ARCHIVE-DRILL-001 | File manifest migration | Validate -> import full clone from `file://` manifest | `archive.validate.result.accepted=true`; `archive.import.completed` canonical hash parity; rollback checkpoint unchanged on retry path | Any `*.rejected` reason class outside bounded taxonomy; canonical hash drift after completed import |
| ARCHIVE-DRILL-002 | Object manifest migration | Validate -> import full clone from `object://` manifest | Same parity criteria as DRILL-001 with object root wiring confirmed | `reject.archive_checkpoint_not_found` when manifest exists and source room is present; non-deterministic reason class mapping |
| ARCHIVE-DRILL-003 | Compatibility boundary rehearsal | Edge-overlap accept and no-overlap reject windows | Edge-overlap lanes accepted; no-overlap lanes rejected as `reject.archive_unsupported_format` | Any inverse boundary outcome or unstable reason class |
| ARCHIVE-DRILL-004 | Policy timeline progression rehearsal | Non-zero progression parity + malformed progression reject | Non-zero progression emits expected cutover list; malformed progression rejects as `reject.archive_manifest_invalid` | Missing cutover progression metadata or accept of malformed progression |

Baseline benchmark target gates:

| Metric | Target (p95 unless noted) | Enforcement point |
|---|---|---|
| Export manifest build latency | <= 100 ms | DRILL-001, DRILL-002 pre-import step |
| Validate full integrity latency | <= 200 ms | DRILL-001, DRILL-002 validate stage |
| Import full apply latency | <= 400 ms | DRILL-001, DRILL-002 import stage |
| Import peak memory ceiling | <= 512 MB (max) | DRILL-001, DRILL-002 run profile capture |

Phase B closeout record:
1. Runtime export command path is active (`archive.export`) with deterministic result/rejected envelopes.
2. Generated file/object manifests are consumed by deterministic roundtrip import vectors with canonical hash parity.
3. Export manifest output now carries compatibility-window metadata and payload digest policy; rejection mappings are pinned by server + websocket conformance fixtures.

Phase C scope opening record:
1. Expand compatibility-window semantics beyond fixed `1..1` for controlled forward/backward compatibility windows.
2. Add policy timeline parity metadata and deterministic import validation for timeline mismatches.
3. Lock websocket conformance vectors that pin Phase C semantics and reason-class mapping stability.

Phase C execution record (initial):
1. Export manifests now include signed `policy_timeline_hash` metadata and archive export envelopes expose the same field.
2. Runtime validate/import paths now enforce deterministic policy timeline mismatch rejections for external manifests.
3. Server vectors and websocket parity fixtures include policy timeline mismatch conformance lanes.

Phase C execution record (range + cutover):
1. Runtime compatibility checks now use explicit version-range overlap semantics for manifest `compatibility_window` validation.
2. Signed export manifests and `archive.export.result` envelopes now include `policy_timeline_cutover_lamport` alongside `policy_timeline_hash`.
3. Core/server/websocket conformance vectors include compatibility-range acceptance plus policy timeline cutover mismatch rejection lanes.

Phase A closeout record:
1. Contracts stabilized across core/server/host-core with shared archive request/response and reason taxonomy types.
2. Runtime processors now cover room-local and external manifest refs (`room://`, `file://`, `object://`) with signature verification path.
3. Negative runtime parity is recorded for unsupported format, signature invalid, and checkpoint-not-found external refs.

Phase A working draft - host/ws parity matrix headings:

| Capability path | Host command/event path | WS/admin request shape | WS/admin response/event shape | Parity status | Open questions |
|---|---|---|---|---|---|
| Success path - describe archive | `DescribeArchive` -> `ArchiveDescribed` | `archive.describe` | `archive.describe.result` | Draft | Confirm minimal manifest summary fields for v1.
| Success path - validate archive | `ValidateArchive` -> `ArchiveValidated` | `archive.validate` | `archive.validate.result` | Draft | Confirm policy timeline hash optionality.
| Success path - import archive | `ImportArchive` -> `ArchiveImported` | `archive.import` | `archive.import.completed` | Draft | Confirm progress event granularity for long imports.
| Deny/error path - compatibility reject | `<any archive command>` -> `Archive*Rejected` | `<same request type>` | `error` or command-specific rejected event | Draft | Decide command-specific reject envelope versus shared error envelope.
| Integrity reject path | `ImportArchive` -> `ArchiveImportRejected` | `archive.import` | `archive.import.rejected` | Draft | Confirm digest/signature mismatch reason class split.

Phase A deterministic rejection taxonomy draft:

1. `reject.archive_unsupported_format`: archive format/version outside declared compatibility window.
2. `reject.archive_manifest_invalid`: manifest schema/required field validation failure.
3. `reject.archive_digest_mismatch`: payload digest set does not match manifest declarations.
4. `reject.archive_signature_invalid`: signature verification fails for signed payloads/manifests.
5. `reject.archive_checkpoint_not_found`: referenced checkpoint cannot be resolved in target runtime.
6. `reject.archive_policy_timeline_mismatch`: declared policy timeline hash conflicts with expected state.

Draft guidance:

1. Keep `reason_class` bounded and machine-stable; allow `reason_message` to vary for diagnostics.
2. Unsupported but well-formed archives should reject with compatibility classes, not generic protocol classes.
3. Integrity failures must be deterministic and non-retriable until artifact input changes.

Phase A concrete host/ws envelope draft:

| Operation | Host command (request) | Host event/response (result) | WS/admin request shape | WS/admin response/event shape | Notes |
|---|---|---|---|---|---|
| Describe archive | `DescribeArchive { room_id, archive_ref }` | `ArchiveDescribed { room_id, archive_ref, manifest_id, format_version, archive_kind, checkpoint, payload_digest_set, compatibility_window, provenance }` | `archive.describe` | `archive.describe.result` | Read-only metadata probe; no DAG mutation.
| Validate archive | `ValidateArchive { room_id, archive_ref, mode }` | `ArchiveValidated { room_id, archive_ref, accepted, compatibility_window, checks }` or `ArchiveValidationRejected { room_id, archive_ref, reason_class, reason_message }` | `archive.validate` | `archive.validate.result` or `archive.validate.rejected` | `mode` draft values: `metadata_only`, `full_integrity`.
| Import archive | `ImportArchive { room_id, archive_ref, import_mode, expected_checkpoint? }` | `ArchiveImported { room_id, archive_ref, canonical_hash, checkpoint, imported_nodes, imported_blobs }` or `ArchiveImportRejected { room_id, archive_ref, reason_class, reason_message }` | `archive.import` | `archive.import.completed` or `archive.import.rejected` | Import result must include canonical checkpoint/hash for deterministic parity evidence.

Phase A ws/admin parity examples (draft envelopes):

Describe request:

```json
{
   "type": "archive.describe",
   "room": "room-a",
   "archive_ref": "s3://nm-archive/room-a/full-2026-05-27.nmar"
}
```

Describe response:

```json
{
   "type": "archive.describe.result",
   "room": "room-a",
   "archive_ref": "s3://nm-archive/room-a/full-2026-05-27.nmar",
   "manifest_id": "m.room-a.20260527.0001",
   "format_version": "1",
   "archive_kind": "full_clone",
   "checkpoint": {
      "frontier": ["seq:120"],
      "canonical_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
   },
   "payload_digest_set": {
      "nodes": "sha256:1111",
      "blobs": "sha256:2222"
   },
   "compatibility_window": {
      "min_supported": "1",
      "max_supported": "1"
   }
}
```

Validate rejected response:

```json
{
   "type": "archive.validate.rejected",
   "room": "room-a",
   "archive_ref": "s3://nm-archive/room-a/full-2026-05-27.nmar",
   "reason_class": "reject.archive_manifest_invalid",
   "reason_message": "manifest missing required field: checkpoint.canonical_hash"
}
```

Import completed response:

```json
{
   "type": "archive.import.completed",
   "room": "room-a",
   "archive_ref": "s3://nm-archive/room-a/full-2026-05-27.nmar",
   "canonical_hash": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
   "checkpoint": {
      "frontier": ["seq:120"],
      "canonical_hash": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
   },
   "imported_nodes": 120,
   "imported_blobs": 15
}
```

Import rejected response:

```json
{
   "type": "archive.import.rejected",
   "room": "room-a",
   "archive_ref": "s3://nm-archive/room-a/full-2026-05-27.nmar",
   "reason_class": "reject.archive_digest_mismatch",
   "reason_message": "payload digest mismatch for nodes payload"
}
```

### Phase B - Deterministic export

Deliverables:

1. export builders for full-clone and checkpoint-fork kinds
2. deterministic manifest generation and payload digest computation
3. policy timeline metadata inclusion rules

Acceptance criteria:

1. repeated export at same checkpoint produces identical digest set
2. manifest contains required provenance and compatibility fields

### Phase C - Verified import

Deliverables:

1. validation pipeline (schema, digest, signatures, compatibility window)
2. import pipeline with canonical checkpoint verification
3. deterministic error classes for rejection paths

Acceptance criteria:

1. import success confirms canonical hash parity at checkpoint
2. malformed/unsupported archives fail with stable reason codes

### Phase D - SDK/operator surface and runbooks

Deliverables:

1. operator documentation for backup, restore, and migration workflows
2. SDK and host-adapter helper APIs for archive describe/validate/import
3. troubleshooting guide for compatibility and integrity failures

Acceptance criteria:

1. operator runbook has recovery procedures for failed import scenarios
2. parity tests pass across wasm/web, server, and host-core paths

### Phase E - Hardening and ecosystem packaging

Deliverables:

1. artifact signing and optional encryption at rest guidance
2. large-room performance baselines for export/import operations
3. archive retention and compliance metadata recommendations

Acceptance criteria:

1. reproducible benchmark targets and operational ceilings documented
2. archive lifecycle policy guidance is actionable in production runbooks

## 6. Conformance vector additions

Add vectors under a new family:

1. `ARCHIVE-ROUNDTRIP-001`: full-clone export/import canonical hash parity
2. `ARCHIVE-ROUNDTRIP-002`: checkpoint-fork export/import canonical hash parity
3. `ARCHIVE-DET-001`: deterministic manifest digest equality at fixed checkpoint
4. `ARCHIVE-COMPAT-ALLOW-001`: supported older format accepted in compatibility window
5. `ARCHIVE-COMPAT-REJECT-001`: unsupported format rejected deterministically
6. `ARCHIVE-INTEGRITY-REJECT-001`: payload digest mismatch deterministic rejection
7. `ARCHIVE-POLICY-001`: policy timeline metadata parity preserved across roundtrip

Wave 1 parallel kickoff evidence (2026-05-27):
1. Core portability vectors are now executable in `core/tests/archive_portability_vectors.rs`:
   - `archive_det_001_manifest_digest_equality_at_fixed_checkpoint`
   - `archive_roundtrip_001_full_clone_canonical_hash_parity`
   - `archive_compat_reject_001_unsupported_format_rejected_deterministically`
2. Server portability vectors are now executable in `server/tests/archive_portability_vectors.rs`:
   - `archive_det_001_manifest_digest_equality_at_fixed_checkpoint_server_path`
   - `archive_roundtrip_001_full_clone_canonical_hash_parity_server_path`
   - `archive_compat_reject_001_unsupported_format_rejected_deterministically_server_path`
3. Acceptance artifacts recorded:
   - `docs/acceptance/archive-roundtrip-001-core.json`
   - `docs/acceptance/archive-roundtrip-001-server.json`
4. Phase A contract-freeze artifacts recorded:
   - `docs/acceptance/archive-phasea-envelope-draft.json`
   - `docs/acceptance/archive-phasea-ws-parity-draft.json`
5. Host Phase A stub parity implementation is now executable:
   - inbound mapper support for `archive.describe`, `archive.validate`, `archive.import` in `nodalmerge-host/src/NodalMerge.DotNetHost/Runtime/RuntimeProtocolMapper.cs`
   - outbound ws event mapping for `archive.describe.result`, `archive.validate.result`, `archive.validate.rejected`, `archive.import.completed`, `archive.import.rejected`
   - runtime stub handling and deterministic rejection class mapping in `nodalmerge-host/src/NodalMerge.DotNetHost/Runtime/RuntimeMessageProcessor.cs`
   - test evidence in `RuntimeProtocolTests` and `RuntimeMessageProcessorTests`
   - acceptance artifact: `docs/acceptance/archive-phasea-host-stub.json`

## 7. Risks and mitigations

1. Risk: archive shape churn causes ecosystem fragmentation.
   Mitigation: strict versioning contract and compatibility-window policy in Phase A.
2. Risk: import-time drift from replay/branch semantics.
   Mitigation: mandatory canonical-checkpoint parity vectors.
3. Risk: large archive runtime pressure.
   Mitigation: benchmark gates and bounded resource guidance in Phase E.

## 8. Recommended sequencing

1. Complete speculative/authoritative Phase A-B.
2. Complete replay branching Phase A-C.
3. Execute query/materialization Phase A-C.
4. Execute this export/import portability plan Phase A-C.
5. Run Phase D-E in parallel with scheduler/backpressure hardening.
