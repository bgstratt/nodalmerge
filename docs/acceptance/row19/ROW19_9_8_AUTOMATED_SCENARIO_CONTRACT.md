# Row 19 - 9.8 Automated Scenario Contract

Status: Harness expanded and locked for this stream, including high-volume churn soak guard
Owner: host-runtime migration stream
Date: 2026-05-12

## 1. Purpose

Define deterministic acceptance scenarios for row-19 parity and make them executable as a repeatable test harness.

## 2. Harness shape (current)

Implementation file:
1. dotnet-host/tests/ActiveSync.DotNetHost.Tests/Row19AutomatedScenarioHarnessTests.cs

Determinism controls:
1. Scenario seed per test case.
2. Stable scenario trace id (`<scenario>-seed-<seed>`).
3. Isolated temp sqlite store per scenario run.
4. Root-hash equality checks across restart boundaries.

## 3. Implemented scenarios (slice 1 + slice 2)

1. R19_98_multi_device_save_delete_restart_converges_without_resurrection
   - Simulates device A save, device B delete, persistence, and restart hydration.
   - Pass contract:
     - post-restart root hash equals pre-restart root hash.
     - deleted key does not resurrect.

2. R19_98_offline_edit_then_reconnect_applies_buffered_ops_deterministically
   - Simulates buffered offline edits replayed on reconnect.
   - Pass contract:
     - post-restart root hash equals pre-restart root hash.
     - final materialized value equals last buffered offline operation.
     - trace id persisted in scenario payload.

  3. R19_98_layout_position_integrity_preserves_fixed_and_flexible_coordinates_after_restart
     - Simulates fixed-board and flexible-board layouts in one deterministic room stream.
     - Pass contract:
    - fixed layout coordinates survive restart exactly.
    - flexible layout coordinates (including negative/large values) survive restart without clamping.

  4. R19_98_asset_propagation_and_retrieval_survives_restart
     - Simulates device A asset write and device B retrieval via file blob provider.
     - Pass contract:
    - written bytes are retrievable before restart.
    - bytes remain retrievable after provider restart.

  5. R19_98_duplicate_apply_churn_guard_suppresses_repeated_pack_replay
     - Replays identical pack payload repeatedly.
     - Pass contract:
    - duplicate pack replay is suppressed and persisted accepted-node set does not grow unbounded.
    - room snapshot retains a single accepted node for repeated identical pack input.

  6. R19_98_duplicate_apply_churn_soak_10k_replays_stays_within_budget
     - Replays an identical pack 10,000 times.
     - Pass contract:
       - persisted accepted-node count remains within explicit budget threshold.
       - replay volume does not cause unbounded node growth.

## 4. Pending required scenarios

  1. None in this stream's scoped 9.8 plan.

## 5. Validation command

1. dotnet test dotnet-host/tests/ActiveSync.DotNetHost.Tests/ActiveSync.DotNetHost.Tests.csproj --filter "FullyQualifiedName~Row19AutomatedScenarioHarnessTests"

Observed result in this stream:
1. 6 passed, 0 failed.

## 6. Evidence linkage

1. This contract file is referenced by docs/ROW19_ACCEPTANCE_EXECUTION_PLAN.md section 9.8.
2. Matrix updates should include each scenario id and result as the 9.8 suite expands.
