# Federation Preconditions Checklist (FSE-10.C)

Purpose: define readiness gates that must be met before enabling multi-authority federation work (FSE-08).

Scope:

1. non-federated readiness only (no trust-domain runtime implementation in this slice)
2. aligns authority/topology hardening with future federation entry criteria

Companion docs:

1. `docs/AUTHORITY_AND_ROOM_TOPOLOGY_EXECUTION_PLAN.md`
2. `docs/MANAGER_WORKER_TOPOLOGY_PLAYBOOK.md`
3. `docs/future-state-enhancements.md`

---

## Gate group A: Identity continuity

1. Device/key continuity policy is documented and enforced for long-lived operators/services.
2. Rotation overlap and revocation semantics are tested in mixed-client lanes.
3. Reject classes for continuity failures are stable and operator-routable.

Entry evidence:

1. continuity contract doc reference
2. acceptance artifact covering rotation overlap and revoke behavior

---

## Gate group B: Lineage and promotion determinism

1. Parent/child lineage binding to explicit parent checkpoint is mandatory.
2. Promotion path has deterministic outcomes under:
   - stale-parent drift
   - concurrent apply contention (single-winner behavior)
3. Replay and audit artifacts are reconstructable without child full replay by default.

Entry evidence:

1. topology vectors for happy path + stale/reject + contention
2. audit key/hash parity evidence bundle

---

## Gate group C: Governance and operator maturity

1. Governance policy templates exist for strict/balanced/reference-only modes.
2. Operator drill runbook includes stale-parent recovery and queue contention drills.
3. Reject taxonomy is documented with action-oriented remediation paths.

Entry evidence:

1. policy template doc
2. drill runbook + at least one rehearsal artifact

---

## Gate group D: Persistence and portability hygiene

1. Peer-local persistence hardening covers read-only, version-skew, corruption, and composite fallback paths.
2. Archive/import compatibility-window guidance is in place for versioned rollouts.
3. Migration cookbook/checklist is used in rollout PR gates.

Entry evidence:

1. `LOCAL-PERSIST-*` vector bundle
2. migration cookbook/checklist references in operator + SDK docs

---

## Gate group E: Observability baseline

1. Promotion metrics (stage/outcome/reason + latency) are consumed in operations.
2. Queue pressure signals are available and tied to runbook actions.
3. Incident templates include proposal id, reason class, and parent hash transitions.

Entry evidence:

1. metric-to-runbook mapping documented
2. operator incident artifact showing full triage fields

---

## Exit rule for FSE-10.C

FSE-10.C is complete when:

1. all gate groups have explicit references/evidence owners
2. unresolved gaps are listed as blockers for FSE-08 entry
3. tracker status explicitly keeps federation implementation deferred until these gates are met
