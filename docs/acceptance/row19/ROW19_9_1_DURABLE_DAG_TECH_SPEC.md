# Row 19 - 9.1 Durable DAG Persistence Technical Spec

Status: Slice A, Slice B, and conservative Slice C implemented
Owner: host-runtime migration stream
Scope: Native accepted-node durability + per-room snapshot/compaction lifecycle for hosted runtime.

## 1. Goals

1. Make accepted-node persistence the correctness path for restart/recovery.
2. Keep snapshot as acceleration layer, not independent correctness source.
3. Preserve incremental-update economics (no full-state rewrite per mutation).
4. Support deterministic fallback recovery when snapshot is missing/corrupt.

## 2. Non-goals (9.1)

1. Final tombstone compaction policy (completed in 9.4).
2. Cross-room/global snapshots.
3. Forcing hard startup failure on snapshot load errors.

## 3. Locked decisions

1. Mongo is the primary backend for 9.1 validation.
2. Per-room snapshot scope only.
3. Hybrid snapshot trigger; not per-change snapshots.
4. Retention window is configurable; initial default is 7 days.
5. Fallback behavior is required; startup must recover.

## 4. Persistence contract

Accepted-node record fields:

1. node_id_hex: stable identity for dedupe/upsert.
2. payload: incremental payload bytes (pack for current runtime slice).
3. payload_kind: discriminator (initially "pack").
4. causal_parent_node_ids: optional parent links for future native-node hydration.
5. frontier_hash_hex: optional frontier marker.
6. applied: whether node was applied in hydration pipeline.
7. is_tombstone: tombstone marker (conservative handling in 9.1).
8. accepted_at_utc: acceptance timestamp.
9. eligible_for_compaction_at_utc: earliest compaction-eligible timestamp.

Snapshot fields (current/next slice):

1. room_id
2. snapshot_payload (latest full room state at boundary)
3. created_at_utc
4. boundary metadata (frontier/hash/version; added in follow-up 9.1 slice)

## 5. Hydration algorithm

1. Ensure room exists in runtime.
2. Load latest valid room snapshot.
3. If snapshot exists and validates, apply snapshot state.
4. Load accepted-node records after snapshot boundary.
5. Apply incremental records in deterministic order.
6. Mark applied records as needed.
7. If snapshot is missing/corrupt, skip snapshot path and hydrate from accepted-node records.
8. Emit diagnostics with room id, source path, counts, and duration.

## 6. Compaction/snapshot lifecycle

1. Snapshot trigger policy (hybrid):
- primary: accepted-node distance from last snapshot
- secondary: time/size safety thresholds
2. Keep one active snapshot per room (atomic replace).
3. Compaction eligibility:
- record eligible when accepted_at_utc >= retention window
- never compact unsafe tombstone lineage in 9.1
4. Compaction removes only records proven replay-safe beyond snapshot boundary.

## 7. Failure and fallback behavior

1. Snapshot decode/validation error:
- log warning with scenario trace id and room id
- increment fallback metric
- continue with accepted-node hydration
2. Accepted-node store read failure:
- surface error telemetry
- preserve process liveness
- room remains unavailable until data path recovers

## 8. Observability requirements

1. room_hydrate_duration_ms
2. room_hydrate_source (snapshot_plus_delta, accepted_nodes_only, empty)
3. room_hydrate_records_loaded
4. room_snapshot_load_failures_total
5. room_compaction_actions_total
6. room_compaction_records_removed_total

## 9. Acceptance mapping (9.1)

1. Cold restart frontier/hash equality before and after restart.
2. No duplicate logical applies during hydration.
3. Compaction + snapshot boundary preserves convergence and entity integrity.

Current status against acceptance mapping:

1. Cold restart frontier/hash equality before and after restart:
- Implemented via restart acceptance tests in provider-backed runtime harness.
- Current evidence: restart pre/post `ServerPackPrepared.root_hex` equality assertions for snapshot-hydration path.
2. No duplicate logical applies during hydration:
- Implemented and test-covered.
- Current evidence: snapshot-first + delta ordering, boundary-node ordering precedence, and fallback behavior tests in RuntimeDagPersistenceServiceTests.
3. Compaction + snapshot boundary preserves convergence and entity integrity:
- Implemented with pruning-enabled restart convergence assertions.
- Current evidence: pruning mode compaction test verifies root-hash convergence after restart and reduced persisted accepted-node count.

## 10. Implementation slices

Slice A (this change):

1. Expand accepted-node contract schema.
2. Persist metadata in providers.
3. Populate metadata in runtime pack persistence path.

Slice B:

1. Snapshot boundary metadata.
2. Native hydration path scaffold (snapshot + delta ordering).
3. Fallback path instrumentation.

Implementation evidence for Slice B:

1. Runtime hydration tests: nodalmerge-host/tests/ActiveSync.DotNetHost.Tests/RuntimeDagPersistenceServiceTests.cs
2. Provider durability tests: nodalmerge-host/tests/ActiveSync.DotNetHost.Tests/ProviderDurabilityTests.cs
3. Restart durability tests: nodalmerge-host/tests/ActiveSync.DotNetHost.Tests/ProviderHostRestartDurabilityIntegrationTests.cs

Slice C:

1. Compaction executor using eligibility window.
2. Lifecycle metrics and acceptance tests.
3. Snapshot payload materialization from host server-pack path (with safe fallback).

Implementation evidence for Slice C:

1. Compaction executor + metrics:
- nodalmerge-host/src/ActiveSync.DotNetHost/Runtime/RuntimeDagPersistenceService.cs
2. Node pruning contract support:
- nodalmerge-host/src/ActiveSync.Host.Abstractions/Providers/INodeStoreProvider.cs
- nodalmerge-host/src/ActiveSync.Host.Composition/MongoNodeStoreProvider.cs
- nodalmerge-host/src/ActiveSync.Host.Composition/SqliteNodeStoreProvider.cs
3. Runtime compaction tests:
- nodalmerge-host/tests/ActiveSync.DotNetHost.Tests/RuntimeDagPersistenceServiceTests.cs
4. Provider restart metadata durability test:
- nodalmerge-host/tests/ActiveSync.DotNetHost.Tests/ProviderDurabilityTests.cs
5. Restart root-hash equivalence + pruning convergence acceptance tests:
- nodalmerge-host/tests/ActiveSync.DotNetHost.Tests/ProviderHostRestartDurabilityIntegrationTests.cs

Conservative safety mode (current):

1. Compaction snapshot boundaries are generated from eligible accepted-pack records.
2. Compaction snapshots now prefer host-generated full-room pack payloads (`RequestServerPack` with `known_ids=[]`) when available; boundary payload fallback is retained for resilience.
3. Pruning remains disabled by default unless explicitly enabled via runtime configuration.
4. Lifecycle metrics (`room_compaction_actions_total`, `room_compaction_records_removed_total`, `room_compaction_duration_ms`) are emitted for operational validation.

## 11. Open items

1. Precise accepted-node ordering key when non-pack payload kinds are introduced.
2. Final tombstone retention and pruning constraints (deferred to 9.4).
3. Concrete performance targets after baseline measurements.

9.2 kickoff progress (next stream):

1. Recovery audit markers (`hydrate-start`, `hydrate-complete`) are now emitted with source and reconciliation counters.
2. Hydration now deduplicates repeated accepted-node IDs within a recovery run to tighten exactly-once reconciliation behavior.
3. Deterministic entity-count reconciliation report coverage is added in restart integration tests for board/button/position/text/list namespaces.

9.2 evidence (initial):

1. Recovery marker + reconcile-count logging:
- nodalmerge-host/src/ActiveSync.DotNetHost/Runtime/RuntimeDagPersistenceService.cs
2. Hydration dedupe guard test:
- nodalmerge-host/tests/ActiveSync.DotNetHost.Tests/RuntimeDagPersistenceServiceTests.cs
3. Restart reconciliation count parity test:
- nodalmerge-host/tests/ActiveSync.DotNetHost.Tests/ProviderHostRestartDurabilityIntegrationTests.cs
4. Replay lifecycle markers:
- `replay-start` and `replay-complete` markers emitted during hydration in RuntimeDagPersistenceService.
5. Multi-stage restart scenarios:
- Warm restart cycle root-hash/entity-count parity test in ProviderHostRestartDurabilityIntegrationTests.
- Restart-during-traffic convergence parity test in ProviderHostRestartDurabilityIntegrationTests.
6. Regression/soak restart validation:
- Periodic restart soak cycle test with root-hash and reconciliation-count drift assertions in ProviderHostRestartDurabilityIntegrationTests.

9.4 kickoff progress (delete/tombstone stream):

1. Restart/hydration delete terminality scenario added:
- `Delete_remains_terminal_after_restart_and_hydration` in ProviderHostRestartDurabilityIntegrationTests.
2. Pruning-compaction delete anti-resurrection scenario added:
- `Delete_state_survives_pruning_compaction_without_resurrection` in ProviderHostRestartDurabilityIntegrationTests.
3. Fake runtime bridge harness now supports `MapDelete` command path for deterministic delete lifecycle assertions.
4. Cross-device multi-room delete propagation sweep added:
- `Cross_device_multi_room_delete_propagation_converges_without_resurrection` in ProviderHostRestartDurabilityIntegrationTests.
