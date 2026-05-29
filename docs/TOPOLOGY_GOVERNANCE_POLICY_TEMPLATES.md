# Topology Governance Policy Templates (FSE-10.B)

Use these templates for manager/worker room families where child-room outcomes may be promoted into parent canonical state.

Companion docs:

1. `docs/MANAGER_WORKER_TOPOLOGY_PLAYBOOK.md`
2. `docs/AUTHORITY_AND_ROOM_TOPOLOGY_EXECUTION_PLAN.md`
3. `docs/operator.md` (runtime triage and metrics)

---

## Template A: Strict controlled promotion

Use when parent room is high-sensitivity and only explicitly approved promotions are allowed.

Policy intent:

1. `promotion_policy_id = "promotion-based"`
2. require `topology.admin` for propose/validate/apply
3. require explicit child lineage binding to parent checkpoint
4. reject any parent drift (`reject.promotion_stale_parent`) and force re-validation

Operational defaults:

1. low `NODALMERGE_TOPOLOGY_PROMOTION_MAX_INFLIGHT` (1-2)
2. bounded queue (`NODALMERGE_TOPOLOGY_PROMOTION_MAX_QUEUE`) with active monitoring
3. promotion proposal ids must be captured in audit trail

Best for:

1. financial/audit-critical state
2. release gating workflows
3. compliance-heavy domains

---

## Template B: Balanced throughput policy

Use when deterministic convergence is required but moderate concurrency is acceptable.

Policy intent:

1. `promotion_policy_id = "promotion-based"`
2. strict lineage validation + child checkpoint verification
3. allow concurrent proposals; expect single-winner behavior per parent checkpoint under apply contention

Operational defaults:

1. medium inflight (for example 2-4)
2. medium queue (for example 32-128)
3. alert on stale-parent reject spikes (often indicates parent churn vs worker cadence mismatch)

Best for:

1. multi-team planning/ops workflows
2. high-volume but deterministic aggregation lanes

---

## Template C: Reference-only topology

Use when parent needs child awareness/indexing, not deterministic child-state convergence.

Policy intent:

1. `promotion_policy_id = "reference-only"`
2. allow lineage metadata and child listing
3. reject promotion propose/apply by policy (`reject.promotion_policy_denied`)

Operational defaults:

1. low topology promotion queue needs
2. focus monitoring on lineage cardinality and child lifecycle health

Best for:

1. discovery/index catalogs
2. independent worker rooms where parent only tracks references

---

## Selection guidance

Pick template by risk and operational model:

1. strict control and low tolerance for drift -> Template A
2. controlled throughput with deterministic outcomes -> Template B
3. relationship-only topology without convergence -> Template C

Migration path:

1. start Template C during pilot
2. move to Template B for production aggregation
3. move to Template A where governance/audit pressure requires it
