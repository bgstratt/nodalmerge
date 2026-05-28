# Wave 2 Phase A — sign-off checklist

Owner: Platform/runtime (process); approvers below (people)
Status: **Draft contracts implemented in-repo; formal multi-stream sign-off pending**
Last updated: 2026-05-28

Wave 2 **implementation** (lineage, promotion, headless worker, CLI, composite pilot) does not require these sign-offs to merge engineering work. Phase A sign-off **closes the contract freeze gate** for program accounting and cross-team alignment.

---

## 1. Headless / peer-local persistence (Phase A)

**Plan:** `docs/HEADLESS_RUNTIME_PERSISTENCE_EXECUTION_PLAN.md` Phase A  
**Evidence:** `docs/acceptance/headless-persistence-phasea-contract-freeze-run01.json`

### What was frozen

1. `PeerLocalPersistence` lifecycle: hydrate → append → flush → checkpoint → recover  
2. Bounded `LocalPersistReason` taxonomy (unavailable, corruption, tail_conflict, readonly, …)  
3. Additive compatibility: no silent cross-backend migration in v1  

### Approvals required

| Stream | Approver role (fill name in issue/PR) | Review focus | Sign-off criterion |
|--------|----------------------------------------|--------------|-------------------|
| **SDK** | SDK / client maintainer | Browser path can implement same trait; `createDoc` stays additive | No SDK-breaking change required for v1 headless |
| **Runtime** | Core/runtime maintainer | Deterministic replay hash parity across adapters | Accepts `LOCAL-PERSIST-*` vectors as conformance bar |
| **Host** | Host-core / FFI maintainer | .NET embedding via `Arc<dyn PeerLocalPersistence>` or sidecar | No host-ffi semantic drift vs peer-local contract |

**Suggested action:** one comment or issue per stream on the Phase A artifact (or this doc) with **Approved** or **Approved with edits** + link to edits.

---

## 2. Authority / room topology (Phase A)

**Plan:** `docs/AUTHORITY_AND_ROOM_TOPOLOGY_EXECUTION_PLAN.md` Phase A  
**Evidence:** `docs/acceptance/authority-topology-phasea-contract-freeze-run01.json`

### What was frozen

1. Authority role matrix (server / owner / worker × room types)  
2. Room lineage metadata fields (`parent_checkpoint`, `promotion_policy_id`, …)  
3. Promotion command/event names (create child, describe, propose, validate, apply)  

### Approvals required

| Stream | Approver role | Review focus | Sign-off criterion |
|--------|---------------|--------------|-------------------|
| **Runtime** | Server/core maintainer | `parent_checkpoint` binding at child create; promotion audit on parent | Matches implemented WS + `AUTH-ROOM-*` vectors |
| **Host** | Host-core maintainer | Future host-core envelope parity for topology commands | No conflict with existing control-plane caps |
| **Operator** | Operator / SRE representative | `nodalmerge topology` CLI map + playbook drills | Operator can run child/promotion flows without browser |

**Note:** Phases B–D are **implemented** with acceptance artifacts; Phase A sign-off confirms the **frozen vocabulary** was correct, not that promotion is incomplete.

---

## 3. Manager/worker CLI plan (Phase A — playbook)

**Plan:** `docs/MANAGER_WORKER_TOPOLOGY_PLAYBOOK.md` §7a  
**Evidence:** `docs/acceptance/manager-worker-cli-workflow-plan-run01.json`

| Stream | Approver role | Sign-off criterion |
|--------|---------------|-------------------|
| **Operator** | Operator maintainer | Command names and flags match implemented `nodalmerge topology` |
| **Runtime** | Runtime maintainer | CLI maps to same WS types as server tests |

Executable CLI is **done** (`docs/acceptance/authority-topology-phased-cli-run01.json`); this sign-off only ratifies the **freeze** against implementation.

---

## 4. What is *not* gated on Phase A sign-off

- Headless depth (IBF/MST, Dockerfile, session report JSON) — shipped  
- Authority Phases B–D + topology CLI — shipped  
- Composite / registry pilot — shipped (`docs/acceptance/headless-persistence-phaseb-composite-pilot-run01.json`)  
- Export/import post-closeout monitoring cadence — ops process (`docs/acceptance/archive-phased-post-closeout-monitoring-run01.json`)  

---

## 5. Suggested sign-off workflow (lightweight)

1. Open **one tracking issue** per stream pair (e.g. “Wave 2 Phase A — SDK + headless contract”) or a single **Wave 2 Phase A sign-off** issue with a checklist.  
2. Link this doc + both Phase A JSON artifacts.  
3. Each maintainer replies: `Approved` | `Approved with edits: <link>` | `Blocked: <reason>`.  
4. When all rows are Approved, update artifact JSON `completion_gate.*_signoff` fields to `recorded` and close the issue.

---

## 6. After Phase A sign-off — recommended engineering order

1. **Wave 3 topology hardening** (from `docs/roadmap.md`): durable promotion store, host-core parity, promotion metrics, multi-peer parity vectors.  
2. **Additional peer-local backends** (§4b): Mongo/Redis pilots via `register_backend`.  
3. **Headless Phase E remainder**: Prometheus exporter, Criterion baselines (optional).
