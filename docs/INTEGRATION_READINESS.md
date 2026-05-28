# Integration readiness (pre–AI workspace)

Owner: Platform/runtime  
Last updated: 2026-05-27

This document answers: **what can you run today** to integrate headless workers, branching/topology, query/materialization, export/import, and the CLI — before building realtime AI workspace / shared agent memory workflows.

## Executive summary

| Lane | Runnable? | Declared slice | What to run |
|------|-----------|----------------|-------------|
| **Headless peer worker** | Yes | Closed | `nodalmerge-headless`, Docker image, `--health`, `--report-json` |
| **Browser demo + SDK** | Yes | Closed | `web/` demo with SDK `persistence`; `sdk-js` npm package |
| **Topology / promotion CLI** | Yes | Closed | `nodalmerge topology …` over WebSocket |
| **Operator CLI (run / archive / query)** | Yes | New | `nodalmerge run`, `archive`, `query` subcommands |
| **Query / materialization** | Yes | Closed | Server WS + host vectors; SDK `query.*` on `nodalmerge-sdk-js` |
| **Export / import (archive)** | Yes | Closed | Server archive adapters; post-closeout monitoring on calendar |
| **Spec / auth / branching** | Partial | Wave 2–3 closed for declared scope | Topology admin command group now routes through native host-core/FFI in `.NET`; sign-offs OOB |
| **Observability dashboards** | Deferred | — | PromQL snippets in `docs/operator.md` only; no hosted Grafana commitment |

## 1. Headless peer worker

**Binary:** `nodalmerge-headless` (`headless/`)

```bash
# Health probe (no server required)
nodalmerge-headless --health

# Short sync session with durable peer-local store
export NODALMERGE_HEADLESS_SERVER_URL=ws://127.0.0.1:7878/ws/my-room
export NODALMERGE_HEADLESS_ROOM=my-room
export NODALMERGE_HEADLESS_BACKEND=embedded   # alias for file/SQLite
export NODALMERGE_HEADLESS_DATA_DIR=./peer-data
nodalmerge-headless --report-json ./session.json
```

**Evidence:** `docs/acceptance/headless-run-phased-worker-run01.json`, `headless-run-phased-depth-run01.json`, `headless-run-phased-phasee-run01.json`, `headless-persistence-phasec-sdk-run01.json`, `headless-persistence-phasee-closeout.json`

**Deferred (non-blocking for integration):** Prometheus `/metrics` exporter on headless; formal Criterion baselines per backend.

## 2. Browser + SDK persistence

**Demo:** `web/demo.js` uses `createDoc({ persistence: { enabled: true } })` — no app-managed IndexedDB.

**Package:** `sdk-js/` — `createNodalMergeSdk({ persistence: { enabled: true, adapter: "indexeddb" } })`.

**Evidence:** `docs/acceptance/demo-persistence-cutover-run01.json`, `docs/acceptance/headless-persistence-phasec-sdk-run01.json`

## 3. Topology / promotion CLI

**Binary:** `nodalmerge` (`cli/`)

```bash
export NODALMERGE_SERVER_URL=ws://127.0.0.1:7878/ws/parent-room
nodalmerge topology list-children --parent-room parent-room
nodalmerge topology propose-promotion --parent-room P --child-room C ...
```

Requires server with topology commands; locked rooms need `NODALMERGE_TOKEN_JSON` with `topology.admin`.

**Evidence:** `docs/acceptance/authority-topology-phased-cli-run01.json`, `authority-topology-wave3-closeout.json`

## 4. Query / materialization

**Surface:** WebSocket `query.register`, `projection.build`, `projection.read`, … on server; SDK helpers on `nodalmerge-sdk-js` and `web/sdk.js` (runtime-message based).

**Smoke:**

```bash
cargo test -p nodalmerge-core query_ --test query_materialization_vectors
cargo test -p nodalmerge-server server_query_ --test query_materialization_vectors
```

**Evidence:** `docs/acceptance/query-phasee-closeout.json`, `docs/acceptance/query-rust-server-control-plane-smoke-run01.json`

## 5. Export / import (archive portability)

**Surface:** Server archive adapter + host migration fixtures; operational monitoring cadence.

**Next calendar item:** weekly post-closeout review **2026-06-03** (`docs/acceptance/archive-phased-post-closeout-monitoring-run01.json`).

**Evidence:** `docs/acceptance/archive-phasec-closeout.json`

## 6. Speculative / authoritative + replay branching

Execution plans (contract + vectors largely in-tree):

- `docs/SPECULATIVE_AUTHORITATIVE_EXECUTION_PLAN.md`
- `docs/REPLAY_BRANCHING_EXECUTION_PLAN.md`
- `docs/AUTHORITY_AND_ROOM_TOPOLOGY_EXECUTION_PLAN.md`

Use these when wiring **parent/child rooms**, promotion, and checkpoint-aware replay — the same server + CLI paths above.

## 7. Suggested integration order (before AI workspace)

1. Run **server** + **browser demo** or **headless worker** against the same room — confirm canonical hash parity after restart (`LOCAL-PERSIST-001` / demo refresh).
2. Exercise **topology CLI** (create-child → propose → validate → apply) on a test parent/child pair.
3. Register a **query spec** and build/read a **projection** at a fixed checkpoint (query Phase E vectors).
4. Run an **archive export/import** roundtrip on a room ref (export/import closeout).
5. Only then layer **agent memory / branching workflows** on top of stable room + persistence + promotion contracts.

## 8. Integration smoke (automated vectors, 2026-05-27)

| Scenario | Command / artifact | Result |
|----------|-------------------|--------|
| Peer-local persistence | `cargo test -p nodalmerge-runtime-local --test local_persist_vectors` | 9/9 pass |
| Headless worker | `cargo test -p nodalmerge-headless --test headless_run_vectors` | 4/4 pass |
| Topology CLI | `cargo test -p nodalmerge-cli --test topology_cli_vectors` | 2/2 pass |
| Archive CLI | `cargo test -p nodalmerge-cli --test archive_cli_vectors` | 1/1 pass |
| Peer-local FFI ABI | `cargo test -p nodalmerge-runtime-local-ffi --test abi` | 4/4 pass |
| Query materialization | `cargo test -p nodalmerge-core query_ --test query_materialization_vectors` | 7/7 pass |
| Query (server) | `cargo test -p nodalmerge-server server_query_ --test query_materialization_vectors` | 6/6 pass |
| Archive portability | `cargo test -p nodalmerge-server archive_ --test archive_portability_vectors` | 19/19 pass |
| Headless health | `cargo run -p nodalmerge-headless -- --health` | JSON ok |

**Live E2E:** `.\scripts\integration-smoke.ps1` (vectors + `nodalmerge run` against `nodalmerge-server` on `ws://127.0.0.1:7878`). Evidence: `docs/acceptance/integration-smoke-live-run01.json`. Archive/query control-plane steps need capability tokens on locked rooms.

## 9. Native / hosted runtime embedding (.NET and modular hosts)

Today there are **three peer embeddings**, not one monolith:

| Embedding | What runs in-process | What syncs over WS | Peer-local disk |
|-----------|---------------------|------------------|-----------------|
| **Browser** | WASM `SyncStore` via `web/sdk.js` | Yes | IndexedDB (`persistence` adapter) |
| **.NET host** (`NodalMerge.DotNetHost` + `host-ffi` + `runtime-local-ffi`) | Engine + control plane + optional `LocalPersistFfiClient` | Optional (host app decides) | Yes via `nodalmerge-runtime-local-ffi` (`memory` / `embedded`) |
| **Headless worker** (`nodalmerge-headless` / `nodalmerge run`) | Rust engine + `runtime-local` | Yes | `memory` / `file` / `embedded` |

**You have Rust-integrated headless** for pods and workstations. **You have .NET in-process engine** via `host-ffi` for authoritative hosted runtime (topology, archive, query events mapped in `RuntimeProtocolMapper`).

**Embedding options:**

1. **Sidecar:** `nodalmerge run` / `nodalmerge-headless` — process boundary, full WS sync + peer-local disk.
2. **In-process (.NET):** `LocalPersistFfiClient` over `nodalmerge-runtime-local-ffi` — hydrate/append/flush/recover without a sidecar; host still owns WS if needed.
3. **In-process (Rust):** `nodalmerge-headless` / `runtime-local` as rlib (`nodalmerge run` pattern).

Engine semantics are shared (`nodalmerge-core`); packaging is what differs.

## 10. Functional gaps (honest checklist)

| Area | In place | Still missing / deferred |
|------|----------|---------------------------|
| Headless sync + persist | Yes | Criterion backend baselines |
| .NET peer-local FFI + topology admin bridge | Yes (`runtime-local-ffi`, `LocalPersistFfiClient`, `RuntimePeerLocalPersistenceService`, topology admin via host-core/FFI) | Production enablement + `cargo build -p nodalmerge-runtime-local-ffi --release` before NuGet pack |
| CLI topology / archive / run | Yes | — |
| CLI query | Yes (commands wired) | Cursor/token ergonomics polish only (functional lane now runs on Rust WS + .NET host runtime) |
| Query/materialization | Yes (core/server vectors + SDK) | Extended replay/load/perf hardening in Wave 3 |
| Export/import | Yes (server + CLI import/describe/validate/export) | — |
| Durable promotion lineage on server restart | Yes (promotion records + lineage metadata durable) | Phase E lineage index optimization and large-room-family scale baselines |
| Hosted dashboards | — | Deferred by product choice |

**Pre–AI workspace closeout:** `docs/acceptance/pre-ai-workspace-integration-closeout.json`

## 11. Out of scope for this readiness doc

- Realtime AI workspace product UX
- Shared agent memory schema
- Hosted observability vendor selection (revisit when ready)
