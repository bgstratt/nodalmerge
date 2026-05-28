# NodalMerge Repo Boundary and Parity Matrix

Owner: Platform/runtime  
Status: Draft (for adoption)  
Last updated: 2026-05-27

## 1. Purpose

Define what stays in the `nodalmerge` core repo, what moves to adjacent repos, and which surfaces require strict parity for forward development.

This document is the decision and execution reference for:

1. Core-vs-tools-vs-product repo boundaries
2. Runtime host parity expectations
3. SDK/headless semantic parity expectations
4. CI and acceptance evidence gates

## 2. Boundary decisions

### 2.1 Keep in `nodalmerge` (core repo)

1. Engine and deterministic semantics:
   - `core`
   - replay/branching/query/materialization contracts
2. Runtime authority and host surfaces:
   - Rust host/core authority paths
   - `.NET` host runtime authority paths
   - native/FFI runtime bindings (`host-ffi`, `runtime-local-ffi`)
3. Compatibility runtime:
   - legacy Rust integrated server (retained for compatibility and migration lanes)
4. Integrator-facing SDK/runtime baseline:
   - npm/sdk + bridge/WASM
   - small demo app for light validation
5. Operator/integrator contracts:
   - metrics schema + label policy
   - runbooks and PromQL snippets
   - acceptance vectors and parity artifacts

### 2.2 Move out of `nodalmerge` (adjacent repos)

1. Product implementations and heavy showcases:
   - tactical showcase repo
   - AI workspace/tutorials/hosted implementation
2. Hosted service implementation concerns:
   - tenant dashboards, on-call routes, product SLO policy packs
3. Optional tools repo (`nodalmerge-tools`) scope:
   - `headless`
   - `cli`
   - tool-centric packaging/release docs

## 3. Parity policy

### 3.1 Tier definitions

1. **Tier 1 (Required parity):** Rust host runtime <-> `.NET` host runtime
2. **Tier 2 (Required semantic parity):** SDK <-> headless (same protocol/persistence semantics, different form factor)
3. **Tier 3 (Compatibility parity):** Legacy Rust integrated server <-> primary host surfaces (best-effort and migration scoped)

### 3.2 Forward-path principle

New runtime features are host-first (Tier 1), then projected into SDK/headless semantics (Tier 2).  
Legacy server support is added only when required by compatibility/migration gates (Tier 3).

## 4. Parity matrix (required behavior)

| Capability area | Rust host runtime | `.NET` host runtime | Legacy Rust server | SDK | Headless | Tier / gate |
|---|---|---|---|---|---|---|
| Deterministic core replay/hash semantics | Required | Required | Compatibility | Required consumer behavior | Required consumer behavior | Tier 1 + Tier 2 |
| Authority topology (`create/list/propose/validate/apply`) | Required | Required | Compatibility | Optional client wrapper | Optional worker consumer | Tier 1 |
| Archive control-plane envelopes/rejections | Required | Required | Compatibility | Optional helper surface | Optional CLI path | Tier 1 + Tier 3 |
| Query/projection control-plane envelopes/rejections | Required | Required | Compatibility lane decision per release | SDK runtime support | CLI/headless compatibility as needed | Tier 1 + Tier 3 |
| Peer-local persistence FFI/native runtime | Required | Required | N/A | IndexedDB adapter contract | runtime-local adapters | Tier 1 + Tier 2 |
| Session/token capability enforcement semantics | Required | Required | Compatibility | Required for locked-room use | Required for locked-room use | Tier 1 + Tier 2 |
| Metrics schema names/labels | Required | Required | Compatibility | N/A | Required producer parity where implemented | Tier 1 + Tier 3 |

## 5. CI and evidence gates

### 5.1 Required CI classes

1. Tier 1 host parity smoke (Rust host vs `.NET` host)
2. Tier 2 SDK/headless semantic parity smoke
3. Tier 3 legacy compatibility smoke (narrow, migration-focused)

### 5.2 Acceptance artifact policy

Each parity-impacting change must update or add acceptance evidence under `docs/acceptance/` with:

1. run id/date
2. surface(s) touched
3. parity tier affected
4. pass/fail and known bounded deltas

## 6. Execution plan (phased)

### Phase A: Boundary freeze

1. Adopt this document in roadmap/checklist
2. Mark Tier 1/Tier 2 required gates
3. Mark Tier 3 compatibility-only scope

### Phase B: Tooling split preparation (`nodalmerge-tools`)

1. Define dependency boundary for `headless` + `cli`
2. Add release/CI contract between core and tools repos
3. Preserve protocol/vector tests in core until tools split stabilizes

### Phase C: Runtime parity hardening

1. Close remaining non-blockers:
   - headless Prometheus exporter
   - token mint helper CLI
   - query control-plane parity decision and implementation path across Rust host and `.NET` host, with explicit legacy-server compatibility gate
2. Add/maintain cross-host parity acceptance artifacts

### Phase D: Observability split

1. Keep metrics schema + PromQL snippets in core
2. Keep hosted dashboards in product/hosted repo
3. Optionally publish reference dashboard JSON outside core release-critical path

## 7. Open decisions to resolve explicitly

1. Query control-plane authoritative runtime path for Rust host and legacy server compatibility
2. Final migration date/trigger for moving `headless` + `cli` to `nodalmerge-tools`
3. Minimum Tier 3 compatibility gate set after tools split

## 8. Definition of done for this plan

1. Boundary and tier policy is referenced by `docs/roadmap.md` and `docs/EXECUTION_CHECKLIST.md`
2. Tiered parity CI lanes are documented and active
3. First post-adoption acceptance artifact records parity status using this matrix
