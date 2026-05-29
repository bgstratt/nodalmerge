# Peer-Local Production Hardening Checklist (FSE-09)

Use this checklist to close a peer-local persistence rollout for current scope.

Companion docs:

1. `docs/HEADLESS_RUNTIME_PERSISTENCE_EXECUTION_PLAN.md`
2. `docs/operator.md`
3. `docs/future-state-enhancements.md`

---

## Gate A: Backend contract and determinism

1. `memory`, `file`, and `composite` backends implement `PeerLocalPersistence` contract.
2. Canonical hash parity holds across restart and backend switch vectors.
3. Tail conflict behavior is deterministic and uses stable reject classes.

Evidence:

1. `LOCAL-PERSIST-001`, `LOCAL-PERSIST-002`, `LOCAL-PERSIST-004`

---

## Gate B: Durable failure classification

1. Read-only writes reject with `reject.local_persist_readonly`.
2. Schema skew rejects with `reject.local_persist_version_skew`.
3. Malformed persisted bytes classify as `reject.local_persist_corruption`.
4. Composite drift/failover paths are covered (stale-tail + durable blob fallback).

Evidence:

1. `LOCAL-PERSIST-005`, `LOCAL-PERSIST-006`, `LOCAL-PERSIST-007`, `LOCAL-PERSIST-008`, `LOCAL-PERSIST-009`

---

## Gate C: Operational integration

1. Headless runbook contains backend selection, rejection triage, and restart guidance.
2. Session report/metrics paths are documented and exercised in smoke.
3. `.NET` in-process peer-local wiring path is documented for host operators.

Evidence:

1. `docs/operator.md` headless section + acceptance artifacts for phased headless runs

---

## Gate D: Packaging and rollout hygiene

1. Runtime-local and headless packaging paths are reproducible in CI/local smoke.
2. Migration notes for SDK persistence path are documented.
3. No server/store path confusion (`peer-local` path is separate from reflector `--store`).

Evidence:

1. runtime smoke workflow + `sdk-js/PERSISTENCE_MIGRATION.md` + operator notes

---

## Exit rule for FSE-09 current scope

FSE-09 is complete for current scope when:

1. Gate A/B/C/D are satisfied with linked evidence,
2. unresolved follow-ons are explicitly moved to next-cycle scope (new adapters, deeper plugin ecosystem),
3. tracker status is reconciled to `Complete (current scope)`.
