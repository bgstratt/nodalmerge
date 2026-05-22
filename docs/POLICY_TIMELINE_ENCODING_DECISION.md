# Policy Timeline Encoding Decision (P2 Task 1)

Date: 2026-05-21
Status: Approved for implementation
Owners: Core runtime maintainers
Related tracker: [AUTHORIZATION_CORE_HOST_EXECUTION_TRACKER.md](AUTHORIZATION_CORE_HOST_EXECUTION_TRACKER.md)

## Decision

Use a hybrid policy timeline model:

1. Canonical replay input (Phase P2) is an out-of-band, lamport-versioned timeline:
   - sequence of entries: `(effective_lamport, policy)`
   - policy at node apply time is the latest entry where `effective_lamport <= node.lamport`
2. Keep existing static-policy replay API as a compatibility wrapper:
   - static policy is represented as one timeline entry at lamport `0`
3. Reserve in-graph policy transition encoding for a later phase when signed policy-history replication is required across pack/compaction boundaries.

## Why this model

1. Minimal surface change to land deterministic policy-at-time semantics now.
2. Avoids immediate wire-format and compaction format churn while P1/P1.5 hardening is still stabilizing.
3. Keeps an explicit migration path to signed policy transitions without invalidating replay invariants.

## Replay invariants

1. Timeline is evaluated only by lamport cutover, not wall clock.
2. For equal lamport values, cutover is inclusive (`>= effective_lamport`).
3. If no timeline entry applies, replay uses `Policy::default()`.
4. Timeline ordering is normalized by replay (`effective_lamport` ascending) to preserve deterministic behavior.

## Migration strategy

Phase A (current):
1. Add timeline-aware replay API in core.
2. Keep existing `replay(nodes, policy)` signature via wrapper.
3. Add unit tests for cutover allow/deny semantics.

Phase B:
1. Define compaction compatibility metadata for policy timeline provenance.
2. Add replay golden fixtures that include policy transitions.

Phase C:
1. Add signed in-graph policy transition op(s) and pack/compaction carriage rules.
2. Optionally materialize out-of-band timeline from in-graph transitions at ingest.

## Implementation notes

1. This decision does not yet define the final in-graph policy op encoding.
2. This decision does not change control-plane capability naming.
3. This decision keeps room-lock auth semantics unchanged (still opt-in by `set-room-key`).
