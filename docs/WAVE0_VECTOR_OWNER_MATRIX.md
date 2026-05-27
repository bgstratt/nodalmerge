# Wave 0 Vector Owner Matrix

Owner: Platform/runtime
Status: InProgress (initial mapping)
Last updated: 2026-05-26

## 1. Purpose

Provide a concrete owner and test-harness target for each required Wave 0 vector so implementation can proceed without coordination ambiguity.

Source plans:

1. docs/SPECULATIVE_AUTHORITATIVE_EXECUTION_PLAN.md (SPEC-AUTH-001..006)
2. docs/REPLAY_BRANCHING_EXECUTION_PLAN.md (BRANCH-FORK-001..006)

## 2. Owner Streams

1. Core stream: deterministic replay, hash equivalence, lineage invariants.
2. Host runtime stream: command/event behavior, authorization and rejection metadata.
3. SDK stream: app-facing parity and reconnect/order safety signals.
4. QA/Acceptance stream: cross-host parity orchestration and artifact production.

## 3. Vector Mapping

| Vector ID | Primary owner | Secondary owner | Proposed executable stub target | Acceptance artifact target | Status |
|---|---|---|---|---|---|
| SPEC-AUTH-001 | SDK stream | Host runtime stream | server/tests/spec_auth_vectors.rs::spec_auth_001_optimistic_visible_before_authority | docs/acceptance/spec-auth-001.json | Passing |
| SPEC-AUTH-002 | Core stream | QA/Acceptance stream | server/tests/spec_auth_vectors.rs::spec_auth_002_accepted_converges | docs/acceptance/spec-auth-002.json | Passing |
| SPEC-AUTH-003 | Host runtime stream | SDK stream | nodalmerge-host/tests/NodalMerge.DotNetHost.Tests/SpecAuthVectorsTests.cs::SpecAuth003RejectedIntentRollsBack | docs/acceptance/spec-auth-003.json | Passing |
| SPEC-AUTH-004 | Core stream | QA/Acceptance stream | core/tests/spec_auth_replay_vectors.rs::spec_auth_004_canonical_hash_stable | docs/acceptance/spec-auth-004.json | Passing |
| SPEC-AUTH-005 | SDK stream | Core stream | server/tests/spec_auth_vectors.rs::spec_auth_005_disposition_idempotent_reconnect_safe | docs/acceptance/spec-auth-005.json | Passing |
| SPEC-AUTH-006 | Host runtime stream | QA/Acceptance stream | nodalmerge-host/tests/NodalMerge.DotNetHost.Tests/SpecAuthVectorsTests.cs::SpecAuth006CapabilityGatedIntentNamespaceReject | docs/acceptance/spec-auth-006.json | Passing |
| BRANCH-FORK-001 | Core stream | QA/Acceptance stream | core/tests/branch_fork_vectors.rs::branch_fork_001_frontier_hash_equal | docs/acceptance/branch-fork-001.json | Passing |
| BRANCH-FORK-002 | Core stream | QA/Acceptance stream | core/tests/branch_fork_vectors.rs::branch_fork_002_snapshot_hash_equal | docs/acceptance/branch-fork-002.json | Passing |
| BRANCH-FORK-003 | Host runtime stream | Core stream | server/tests/branch_fork_vectors.rs::branch_fork_003_target_writes_isolated | docs/acceptance/branch-fork-003.json | Passing |
| BRANCH-FORK-004 | Host runtime stream | Core stream | server/tests/branch_fork_vectors.rs::branch_fork_004_source_writes_isolated | docs/acceptance/branch-fork-004.json | Passing |
| BRANCH-FORK-005 | Core stream | Host runtime stream | core/tests/branch_fork_vectors.rs::branch_fork_005_compaction_preserves_metadata | docs/acceptance/branch-fork-005.json | Passing |
| BRANCH-FORK-006 | Host runtime stream | QA/Acceptance stream | nodalmerge-host/tests/NodalMerge.DotNetHost.Tests/BranchForkVectorsTests.cs::BranchFork006UnauthorizedRejected | docs/acceptance/branch-fork-006.json | Passing |

## 4. Execution Order (Wave 0)

1. Land executable stubs for SPEC-AUTH-004, BRANCH-FORK-001, BRANCH-FORK-002 first (determinism core).
2. Land host authorization and rejection stubs for SPEC-AUTH-006 and BRANCH-FORK-006.
3. Land convergence and reconnect-order safety stubs for SPEC-AUTH-002 and SPEC-AUTH-005.
4. Land cross-room fork isolation stubs for BRANCH-FORK-003 and BRANCH-FORK-004.
5. Wire acceptance artifact writer for all vectors under docs/acceptance/.

## 5. Definition of Done for this Matrix

1. Every vector has a primary owner and executable stub location.
2. Every vector has an acceptance artifact target path.
3. Any changes to vector IDs in source plans are reflected in this file in the same PR.
4. Status moves from Planned -> Stubbed -> Executing -> Passing as work lands.
