# Export and Import Portability Execution Plan

Owner: Core/runtime
Status: Planned
Last updated: 2026-05-25

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

### Phase A - Contract freeze

Deliverables:

1. archive manifest schema and versioning policy
2. host-core export/import command-event contract draft
3. websocket/admin parity mapping and error taxonomy

Acceptance criteria:

1. contract compiles without runtime-specific scheduler ownership types
2. explicit compatibility-window behavior documented for unsupported versions

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
