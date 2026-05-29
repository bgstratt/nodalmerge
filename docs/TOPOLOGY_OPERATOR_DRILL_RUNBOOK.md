# Topology Operator Drill Runbook (FSE-10.B)

Structured drills for manager/worker topology operations and promotion recovery paths.

Prerequisites:

1. reachable `nodalmerge-server` with topology control plane enabled
2. valid `topology.admin` capability for locked rooms
3. baseline room family created via `nodalmerge topology create-child`

---

## Drill 1: Stale-parent recovery

Goal:

1. validate operators can recover from `reject.promotion_stale_parent` without manual state surgery

Steps:

1. Propose + validate promotion for child A.
2. Mutate parent canonical state before apply (simulate concurrent parent progress).
3. Apply proposal and confirm reject class is stale-parent.
4. Re-snapshot parent/child lineage, re-propose, re-validate, re-apply.
5. Record proposal ids and final parent canonical hash.

Pass criteria:

1. stale-parent reject observed and correctly classified
2. follow-up promotion succeeds deterministically
3. audit key exists for successful apply

---

## Drill 2: Queue pressure and fairness

Goal:

1. verify bounded behavior under concurrent promotion requests

Steps:

1. Set bounded queue knobs:
   - `NODALMERGE_TOPOLOGY_PROMOTION_MAX_INFLIGHT`
   - `NODALMERGE_TOPOLOGY_PROMOTION_MAX_QUEUE`
2. Submit concurrent propose/validate/apply requests from multiple children.
3. Observe:
   - admitted requests eventually complete or reject deterministically
   - overflow requests reject with bounded queue behavior

Pass criteria:

1. no deadlock/starvation
2. reject classes and outcomes are deterministic per request class
3. promotion queue metrics reflect contention profile

---

## Drill 3: Single-winner apply contention

Goal:

1. confirm concurrent applies against same parent checkpoint produce one winner and stale-parent losers

Steps:

1. Validate two promotion proposals against same parent baseline.
2. Trigger apply calls concurrently.
3. Confirm one apply succeeds and the loser rejects stale-parent.

Pass criteria:

1. exactly one successful apply per shared parent checkpoint window
2. losing apply rejects with stale-parent class
3. parent canonical hash and audit artifacts are consistent

---

## Evidence bundle template

Capture per drill:

1. timestamp, room ids, child ids, proposal ids
2. request/response excerpts (including reject classes)
3. resulting parent canonical hash
4. relevant metric snapshots:
   - `nodalmerge_topology_promotion_total`
   - `nodalmerge_topology_promotion_seconds`
   - queue/inflight gauges when contention drill runs

Store artifacts under `docs/acceptance/` with run id and drill number.
