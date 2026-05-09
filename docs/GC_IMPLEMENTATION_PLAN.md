# ActiveSync GC Implementation Plan (Delegated CAS + DAG)

This document is the implementation plan for reusable ActiveSync garbage
collection of externally stored CAS blobs referenced from DAG-visible state.

It complements `docs/delegated-storage-gc.md`:

1. `delegated-storage-gc.md` defines contracts/interfaces.
2. This document defines execution order, rollout, validation, and guardrails.

## 0. Scope and non-goals

Scope:

1. Reclaim externally stored CAS blobs (S3/R2/MinIO/filesystem blob stores) when no longer live.
2. Keep ActiveSync core product-agnostic and backend-agnostic.
3. Support optional datastore adapters via separate modules/crates.

Non-goals:

1. Do not delete immutable DAG history/nodes.
2. Do not hard-code product selectors, room naming, tenancy semantics, or admin policy in core.
3. Do not require `ListBucket` for daily safe GC.

Important model:

1. Logical removal happens by new DAG operations that remove/replace blob references.
2. Physical blob deletion happens later via GC after liveness and safety checks.

## 1. Architecture and ownership model

1. ActiveSync provides reusable GC contracts and orchestration.
2. Products provide policy configuration and selector mapping through adapters/config.
3. Datastore/object-store details are isolated in optional adapters.

Recommended deployment options:

1. Generic ActiveSync GC worker process using product-neutral contracts.
2. External scheduler calling ActiveSync internal GC APIs/contracts.

Portability rules:

1. Core coordinator depends only on interfaces.
2. Adapters own backend specifics (Mongo/Postgres/MySQL/MSSQL/Oracle/KV).
3. Policy hooks remain injectable per product.

## 2. Data model (logical contracts)

Logical records used by GC (storage-neutral):

1. `cas_blob_inventory`
2. `cas_gc_runs`
3. `hash_reference_state`
4. `cas_delete_queue`
5. Optional `cas_admin_pins`

Reference: concrete trait and type contracts are specified in
`docs/delegated-storage-gc.md`.

## 3. Liveness and deletion semantics

Primary liveness model:

1. Incremental reference deltas are primary signal (`oldHashes` vs `newHashes`).
2. Mark/sweep is repair/audit and drift correction.
3. Deletion decisions are made over full delete domain, not per-room in isolation.

Deletion domain rule:

1. One GC domain per `(tenantId, bucket, keyPrefix)` where applicable.
2. Any live reference in the domain keeps hash live.

Hard-delete preconditions:

1. Hash is pending and older than grace window.
2. Hash is not pinned.
3. Ref-state still indicates zero.
4. Authoritative live check still reports not live.
5. Optional HEAD check passes policy gate.

## 4. Sweep algorithm

Incremental path:

1. Compute `deltaAdded = newHashes - oldHashes`.
2. Compute `deltaRemoved = oldHashes - newHashes`.
3. For `deltaAdded`: increment refcount, update lastReferencedAt, dequeue pending delete.
4. For `deltaRemoved`: decrement (floor at zero); enqueue delete when zero and not pinned.

Repair path:

1. Mark: collect live hashes from `LiveHashSource`.
2. Soft sweep: move unmarked non-pinned entries to pending.
3. Hard sweep: delete pending entries past grace after safety rechecks.
4. Reconcile drift between ref-state and authoritative live set.

## 5. Safety controls and rollout

Feature flags / modes:

1. `DryRun`
2. `MarkOnly`
3. `SweepSoft`
4. `SweepHard`

Operational controls:

1. `graceWindow`: default 24h prod / 1h staging.
2. `maxDeletesPerRun` cap.
3. `requireHeadBeforeDelete` rollout gate.
4. Circuit breaker / kill switch for hard delete.

Rollout phases:

1. Phase 0: inventory + run ledger + DryRun validation.
2. Phase 0.5: incremental reference tracking + queue (no hard deletes).
3. Phase 1: soft sweep only.
4. Phase 2: limited hard delete (small cap).
5. Phase 3: full hard delete with monitoring.

## 6. Implementation backlog (ActiveSync-first)

Execution order:

1. B0 - Contract freeze and package boundaries.
2. B1 - Coordinator skeleton (no deletes).
3. B2 - Delta engine and queue semantics with idempotency.
4. B3 - Adapter implementation (Mongo first, contract-neutral shape).
5. B4 - Hard delete path + safety controls.
6. B5 - Conformance + fault-injection tests.
7. B6 - Demo validation + operational readiness.

B0 deliverables:

1. Product-neutral API surface finalized.
2. Package map documented:
   1. core contracts/coordinator package,
   2. optional adapter packages.

B3 deliverables:

1. First adapter (Mongo) passes conformance suite.
2. No backend types leak into shared contracts.

B6 deliverables:

1. Demo validation report.
2. Baseline dashboards/alerts.
3. Go/no-go decision record.

## 7. Conformance and validation gates

Required behavior tests:

1. Refcount floor at zero.
2. Idempotency under replay/retry.
3. Grace window enforcement.
4. Pinned hash protection.
5. Drift repair correctness.
6. Queue retry/backoff and dead-letter handling.

Track-B gate (must pass before product integration work):

1. Demo shows shared hash survives until last reference removed.
2. Zero-ref hash is queued and deleted only after grace.
3. Live-again hash cancels pending delete.
4. No false deletes under retry/replay scenarios.
5. Coupling review passes (no product-specific leakage into core).

## 8. Coupling guardrails (ActiveSync)

Forbidden in ActiveSync core/shared crates:

1. Product selector defaults (for example specific field-name constants).
2. Product-specific room naming or owner semantics.
3. Backend-specific types in generic interfaces.
4. Compile-time dependency on any product SDK/app packages.

Required in ActiveSync core/shared crates:

1. Product-neutral naming and contracts.
2. Config-driven selector/policy hooks.
3. Adapter-based backend portability.

## 9. Product integration boundary

Product-side responsibilities (outside ActiveSync core):

1. Selector mapping and policy values.
2. Scheduler wiring in product deployment.
3. Product migrations/backfills and operational runbooks.

ActiveSync responsibilities:

1. Reusable GC contracts, coordinator, and optional adapters.
2. Stable integration surface for products to plug in policy/config.

## 10. Go / no-go criteria

Go:

1. Seven consecutive DryRun/Soft runs with no false-positive protected deletes.
2. Stable mark coverage and low drift mismatch variance.
3. Canary hard-delete runs with zero restore incidents.
4. Dashboards and alerts active.

No-go:

1. Protected/pinned hash reaches pending delete.
2. Authoritative live-hash source unavailable.
3. Error rate exceeds threshold during canary.
4. Missing deletion audit trail.

## 11. Initial execution checklist

1. Confirm defaults for grace window, caps, and run modes.
2. Freeze contract package boundaries.
3. Implement idempotent delta hook before any hard-delete enablement.
4. Enable queue consumer in observe-only mode first.
5. Complete coupling review before merging core changes.
