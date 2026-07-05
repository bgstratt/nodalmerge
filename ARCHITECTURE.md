# NodalMerge Architecture

> A Rust CRDT engine with cryptographic identity and content-addressed
> storage, exposed three ways: a standalone WebSocket server, an embeddable
> host runtime behind a C ABI (consumed by the .NET host), and a WASM bridge
> for browsers.

This is the top-level map. Details live in [docs/](docs/); each section
points at the authoritative document.

---

## 1. Repository layout

```
core/
  crdt/            nodalmerge-core        — deterministic CRDT runtime: DAG,
                                            resolution, Blake3, Ed25519,
                                            AES-GCM, IBF, MST, RGA text,
                                            replay, compaction, policy,
                                            RoomTokens. No I/O.
  gc/              nodalmerge-gc          — delegated-storage GC (mark/sweep
                                            contracts for S3/R2/MinIO).
engine/
  host-core/       nodalmerge-host-core   — embeddable HostEngine: typed
                                            HostCommand/HostEvent runtime on
                                            top of core. In-memory, no
                                            transport.
  host-ffi/        nodalmerge-host-ffi    — C ABI over host-core
                                            (`nm_host_*`, `nm_room_token_*`).
  commands/        nodalmerge-command-registry — shared command registry both
                                            parity test suites are driven by.
server/
  server/          nodalmerge-server      — the mature standalone server:
                                            Axum WebSocket, rooms, fan-out,
                                            IBF/MST sync, policy enforcement,
                                            persistence, metrics, replay CLI.
  axum-embed/      nodalmerge-host-axum   — thin Axum adapter for embedding
                                            the host runtime.
  dev-server/      nodalmerge-dev-server  — integrated hosted server (Mongo-
                                            backed) used in benchmarks/dev.
  jwt-bridge/      nodalmerge-jwt-bridge  — verifies third-party JWTs
                                            (HS/RS/ES256), mints RoomTokens.
  s3-blobs/        nodalmerge-s3-blobs    — S3/R2/MinIO blob store.
  stores/          mongo, postgres, conformance — node stores + shared
                                            conformance suite.
peer/
  runtime-local/     nodalmerge-runtime-local     — peer-local durable
                                            persistence (SQLite-backed).
  runtime-local-ffi/ nodalmerge-runtime-local-ffi — C ABI (`nm_local_*`).
  headless/          nodalmerge-headless  — headless peer for load/soak runs.
  cli/               nodalmerge-cli       — command-line peer.
hosts/
  dotnet/          NodalMerge.DotNetHost + NodalMerge.Host.* — ASP.NET host
                   over host-ffi/runtime-local-ffi; ships native libs in
                   NuGet (`NodalMerge.DotNetHost.Native.<rid>`).
clients/
  bridge-wasm/     nodalmerge-bridge      — wasm-bindgen facade over core.
  sdk-js/          nodalmerge-sdk-js      — JS SDK (`createDoc`), IndexedDB
                   peer-local persistence (`nodalmerge-peer-local`).
  web/             demo web client + static server (`serve.py`).
```

## 2. The two engines

The repo intentionally contains **two engine implementations** (see
[docs/RESTRUCTURE_AND_PARITY_PLAN.md](docs/RESTRUCTURE_AND_PARITY_PLAN.md)):

- **`nodalmerge-server`** — the original, mature path. Owns its own room
  loop, persistence, and wire handling. Persistent, battle-tested,
  operationally documented in [docs/deployment.md](docs/deployment.md) and
  [docs/operator.md](docs/operator.md).
- **`host-core::HostEngine`** — the embeddable runtime consumed through
  `host-ffi` by the .NET host (and `axum-embed`). In-memory; the .NET host
  layers durability on top through its provider interfaces (Mongo, SQLite,
  file/S3 blobs).

Both speak the same typed command/event contract
([docs/HOST_COMMAND_EVENT_CONTRACT.md](docs/HOST_COMMAND_EVENT_CONTRACT.md));
`engine/commands` is the machine-readable registry of every command with its
support status per surface, and the Rust and .NET parity test suites are
generated from it. `graph.*` and `replay.read-range` commands route through
shared "brain" functions in `host-core` on both engines.

## 3. FFI surfaces

Two independent cdylibs, both shipped in the .NET NuGet:

| Library | Prefix | Consumed by |
|---|---|---|
| `nodalmerge_host_ffi` | `nm_host_*`, `nm_room_token_*`, `nm_bytes_*` | `NativeMethods.cs`, `RoomTokenEmbeddedAuth` |
| `nodalmerge_runtime_local_ffi` | `nm_local_*`, `nm_bytes_*` | `LocalNativeMethods.cs` |

C header: [engine/host-ffi/include/nodalmerge_host.h](engine/host-ffi/include/nodalmerge_host.h).
Managed and native always version together in the NuGet, so the ABI has no
cross-version compatibility aliases.

## 4. Identity & auth

- Peers are Ed25519 keypairs; every node in the DAG is signed and
  content-addressed (Blake3).
- Room access is a `RoomToken`: `{ peer_pubkey, expiry, caps[] }` signed by
  the room's Ed25519 key. Capability globs (`read:**`, `write:world/**`)
  gate commands; control-plane commands require admin capabilities.
- `nodalmerge-jwt-bridge` lets Clerk/Supabase/Auth0/custom auth issue
  RoomTokens by verifying their JWT (HS256/RS256/ES256) — NodalMerge never
  owns identity.
- The .NET host mints/validates RoomTokens natively via
  `nm_room_token_mint_json` / `nm_room_token_validate_json` (the
  `RoomTokenEmbedded` auth provider), so both runtimes share one crypto
  implementation.

## 5. Persistence

| Surface | Store | Notes |
|---|---|---|
| `nodalmerge-server` | `DirPersistence`: SQLite `<root>/nodalmerge.db` (WAL) + file blobs, or Mongo/Postgres/S3 via store crates | server key at `./server.key` |
| .NET host | Provider interfaces: Mongo (`accepted_nodes`), SQLite, file/S3 blobs | canonical cross-runtime schema in [docs/PERSISTENCE_SCHEMA.md](docs/PERSISTENCE_SCHEMA.md) |
| Postgres | `nodalmerge_nodes` table, sqlx migrations | conformance-tested |
| Browser peer | IndexedDB `nodalmerge-peer-local` via sdk-js | opt-out |
| Native peer | `runtime-local` SQLite via `nm_local_*` FFI | hydrate/recover/flush/append |

The Mongo schema is deliberately identical between Rust and .NET (compound
`_id = "<room>:<node_hex>"`, `$setOnInsert` upsert) so both runtimes can
share one database. Blob file/S3 layout parity is still unaudited — do not
assume cross-runtime blob sharing works until that audit closes.

## 6. Observability

All metrics are `nodalmerge_*` (Prometheus via `metrics-exporter-prometheus`
on the Rust server; `System.Diagnostics.Metrics` meters
`NodalMerge.DotNetHost.*` on the .NET host). The metric tables and the
guardrails they feed (backpressure, rate limiting, blob GC, Lamport
rejection, token expiry) are documented in
[docs/deployment.md](docs/deployment.md) and
[docs/operator.md](docs/operator.md). Legacy `activesync_*` names are gone
as of the 2026-07-05 purge; nothing dual-emits.

## 7. Further reading

- [docs/quickstart.md](docs/quickstart.md) — five-minute local run
- [docs/self-host.md](docs/self-host.md) — Docker + JWT walkthrough
- [docs/integration.md](docs/integration.md) — embedding and store wiring
- [docs/sdk.md](docs/sdk.md) — JS SDK API
- [docs/protocol.md](docs/protocol.md) — wire protocol
- [docs/delegated-storage-gc.md](docs/delegated-storage-gc.md) — blob GC
- [docs/schema-migrations.md](docs/schema-migrations.md) — store migrations
- [benchmarks/README.md](benchmarks/README.md) — benchmark matrix
