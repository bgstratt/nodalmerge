# Runtime and Integration Guide

This guide gives you a single operational picture for:

1. headless worker flows
2. web/SDK + WASM flows
3. server and native/FFI host flows
4. current implementation status (done vs partial/deferred)

## 1) Current implementation status

Implemented now:

1. Rust server WS control plane for topology, archive, and query commands.
2. Query build backpressure slices including fair bounded queue (`NODALMERGE_QUERY_BUILD_MAX_QUEUE`).
3. Promotion durability across restart (`topology-promotions.db`).
4. Lineage metadata durability across restart (`topology-lineage.db`).
5. Headless worker + CLI run path with peer-local persistence backends.
6. JS SDK + WASM bridge package path (`nodalmerge-sdk-js` / `nodalmerge-bridge` wrappers).
7. .NET host runtime bridge and peer-local FFI embedding path.

Partially implemented / deferred:

1. Topology Phase E scale items (lineage index optimization/retention, large family baselines).
2. Hosted dashboard productization remains deferred.
3. Operator automated drill runner remains deferred (manual runbook path is present).
4. Large-room-family lineage retention/scale hardening remains in-progress (operational tuning and baselines).

Parity status note:

1. `.NET` topology admin flows (`create-child`, `describe-lineage`, `list-children`, `propose/validate/apply-promotion`) execute through native host-core/FFI boundary (no local runtime stubs for this command group).

## 1.1) CLI command reference (man page style)

Command root:

```text
nodalmerge <run|topology|archive|query|token> [subcommand] [flags]
```

### `nodalmerge run`

Headless peer worker wrapper (`nodalmerge-headless`).

```text
nodalmerge run \
  --server-url <ws/http base> \
  --room <room-id> \
  --backend <memory|file|embedded|sqlite|composite> \
  [--data-dir <path>] \
  [--run-secs <u64>] \
  [--negotiate-ibf <bool>] \
  [--negotiate-mst <bool>] \
  [--report-json <path>]
```

Env aliases: `NODALMERGE_HEADLESS_SERVER_URL`, `NODALMERGE_HEADLESS_ROOM`, `NODALMERGE_HEADLESS_BACKEND`, `NODALMERGE_HEADLESS_DATA_DIR`, `NODALMERGE_HEADLESS_RUN_SECS`, `NODALMERGE_HEADLESS_NEGOTIATE_IBF`, `NODALMERGE_HEADLESS_NEGOTIATE_MST`, `NODALMERGE_HEADLESS_REPORT_JSON`.

### `nodalmerge topology`

Common globals (all topology subcommands): `--server`, `--room`, `--token-json`, `--timeout-secs` with env aliases `NODALMERGE_SERVER_URL`, `NODALMERGE_ROOM`, `NODALMERGE_TOKEN_JSON`.

```text
nodalmerge topology create-child \
  --parent-room <id> \
  --child-room <id> \
  --purpose <string> \
  --policy <promotion-policy-id> \
  [--created-by <actor>] \
  --parent-checkpoint-file <json>
```

```text
nodalmerge topology list-children --parent-room <id>
```

```text
nodalmerge topology show-lineage --room <id>
```

```text
nodalmerge topology propose-promotion \
  --parent-room <id> \
  --child-room <id> \
  --child-checkpoint <canonical-hash> \
  --payload-ref <uri> \
  [--idempotency-key <key>]
```

```text
nodalmerge topology validate-promotion --proposal-id <id>
nodalmerge topology apply-promotion --proposal-id <id>
```

### `nodalmerge archive`

Common globals: same as topology (`--server`, `--room`, `--token-json`, `--timeout-secs`).

```text
nodalmerge archive describe --archive-ref <room://...|file://...>
nodalmerge archive validate --archive-ref <ref> [--mode <full_integrity|...>]
nodalmerge archive export --source-room <id> --archive-ref <ref>
nodalmerge archive import --archive-ref <ref> [--import-mode <full_apply|...>] [--expected-checkpoint-file <path>]
```

### `nodalmerge query`

Common globals: same as topology (`--server`, `--room`, `--token-json`, `--timeout-secs`).

```text
nodalmerge query register-spec --query-spec-id <id> --version <v> --descriptor-file <path>
nodalmerge query build-projection --projection-id <id> --query-spec-id <id> [--selector <latest|seq|hash|frontier>] [--canonical-seq <u64>]
nodalmerge query list-projections [--query-spec-id <id>]
nodalmerge query read-projection --projection-id <id> [--limit <u64>] [--page-token <token>]
nodalmerge query invalidate-projection --projection-id <id> [--reason <manual|...>]
```

### `nodalmerge token`

```text
nodalmerge token mint \
  --room <id> \
  --room-key-seed-hex <64-hex> \
  --peer-seed-hex <64-hex> \
  [--caps <csv>] \
  [--ttl-secs <u64>]
```

## 1.2) SDK command reference (both surfaces)

There are two actively used SDK surfaces in this repo:

1. `web/sdk.js` high-level doc API (`createDoc`) used by browser/demo integrations.
2. `nodalmerge-sdk-js` package API (`createNodalMergeSdk`) used by wrapper integrations and newer runtime-oriented clients.

### A) `createDoc` API (`web/sdk.js`)

Bootstrap:

```ts
import { createDoc } from "./sdk.js";

const doc = await createDoc({
  serverUrl: "ws://127.0.0.1:7878",
  room: "demo-room",
  autoConnect: false,
});
```

Lifecycle (requested by you):

```ts
doc.connect();
doc.disconnect();
doc.close();
```

Map ops (requested by you):

```ts
doc.map("world").set("username", "alice");
const user = doc.map("world").get("username");
const blobHash = doc.map("assets").setBlob("avatar", bytes, { contentType: "image/png" });
const blobBytes = doc.map("assets").getBlob(blobHash);
```

List ops (requested by you):

```ts
const taskId = doc.list("tasks").push({ title: "ship vertical" });
const allTasks = doc.list("tasks").toArray();
```

Additional commonly used handles:

```ts
doc.text("note").insert(0, "hello");
doc.text("note").delete(0, 1);
doc.send({ type: "request" }); // escape hatch
```

### B) `createNodalMergeSdk` API (`nodalmerge-sdk-js`)

Bootstrap:

```ts
import { createNodalMergeSdk } from "nodalmerge-sdk-js";

const sdk = await createNodalMergeSdk({
  wsUrl: "ws://127.0.0.1:7878/ws/runtime",
  roomId: "demo-room",
  persistence: { enabled: true, adapter: "indexeddb" },
});

await sdk.initialize();
await sdk.room.connect();
```

Lifecycle:

```ts
await sdk.room.connect();
sdk.room.disconnect();
```

Map/list/text-ish sync plane:

```ts
sdk.sync.set("username", "alice");
const user = sdk.sync.get("username");
sdk.sync.del("username");
sdk.sync.insertTextRange("doc:title", { kind: "end" }, " world");
sdk.sync.deleteTextRange("doc:title", { kind: "offset", pos: 5 }, 1);
sdk.sync.push();
sdk.sync.pull();
```

CAS blobs:

```ts
const hash = sdk.cas.setBlob("avatar", bytes);
const restored = sdk.cas.getBlob(hash);
sdk.cas.requestMissingBlobs();
```

Query control-plane:

```ts
await sdk.query.registerSpec({ querySpecId: "q.rooms", version: "v1", descriptor: { source: "rooms" } });
await sdk.query.buildProjection({ projectionId: "p.rooms", querySpecId: "q.rooms", targetCheckpoint: { selector: "latest" } });
await sdk.query.readProjection({ projectionId: "p.rooms", limit: 50 });
await sdk.query.listProjections({ querySpecId: "q.rooms" });
await sdk.query.invalidateProjection({ projectionId: "p.rooms", reason: "manual" });
```

Presence/signaling/offline/persistence:

```ts
sdk.presence.set({ cursor: { x: 120, y: 220 } }, { ttlMs: 5000 });
sdk.presence.getAll();
sdk.presence.sweep(Date.now());
sdk.signaling.offer("peer-b", "v=0...");
sdk.offline.flush();
await sdk.persistence.flush();
```

## 2) Headless guide

Primary binaries:

1. `nodalmerge-headless` crate binary.
2. `nodalmerge run` (CLI wrapper over the same headless engine path).

### Quick start

```powershell
$env:NODALMERGE_HEADLESS_SERVER_URL = "ws://127.0.0.1:7878"
$env:NODALMERGE_HEADLESS_ROOM = "my-room"
$env:NODALMERGE_HEADLESS_BACKEND = "embedded"
$env:NODALMERGE_HEADLESS_DATA_DIR = ".\peer-data"
cargo run -p nodalmerge-cli -- run --run-secs 10 --report-json .\session.json
```

What each key env var does:

1. `NODALMERGE_HEADLESS_SERVER_URL`: server base URL.
2. `NODALMERGE_HEADLESS_ROOM`: room id target.
3. `NODALMERGE_HEADLESS_BACKEND`: `memory`, `file`, `embedded`, `sqlite`, `composite`.
4. `NODALMERGE_HEADLESS_DATA_DIR`: local durable data directory (when backend needs disk).

Health check:

```powershell
cargo run -p nodalmerge-headless -- --health
```

Runs vector suite:

```powershell
cargo test -p nodalmerge-headless --test headless_run_vectors
```

## 3) Web/SDK + WASM guide

Key packages:

1. `nodalmerge-sdk-js` (high-level SDK, currently compatibility wrapper during rename window).
2. `nodalmerge-bridge` (WASM bridge wrapper).
3. Underlying runtime is the bridge/core stack exposed to JS.

### Install and connect

```bash
npm install nodalmerge-sdk-js
```

```ts
import { createNodalMergeSdk } from "nodalmerge-sdk-js";

const sdk = await createNodalMergeSdk({
  wsUrl: "ws://127.0.0.1:7878/ws/runtime",
  roomId: "demo-room",
  persistence: { enabled: true, adapter: "indexeddb", dbName: "nodalmerge-peer-local" },
});

await sdk.room.connect();
sdk.sync.set("username", "alice");
sdk.sync.push();
```

Query control plane from SDK:

```ts
await sdk.query.registerSpec({
  querySpecId: "q.rooms",
  version: "v1",
  descriptor: { source: "rooms" }
});
await sdk.query.buildProjection({
  projectionId: "p.rooms",
  querySpecId: "q.rooms",
  targetCheckpoint: { selector: "latest" }
});
```

Browser smoke:

```powershell
Set-Location web/smoke
npm install
npm run smoke
```

## 4) Server guide (Rust WS reflector + APIs)

Start server:

```powershell
cargo run -p nodalmerge-server -- --store .\data --metrics-addr 127.0.0.1:9090
```

Main endpoints:

1. `GET /ws/:room_id` WebSocket runtime/control-plane entry.
2. Metrics endpoint when enabled (`http://127.0.0.1:9090/metrics`).

Common server flags:

1. `--store <path>`: enable durable node/blob/topology sidecars.
2. `--idle-timeout <secs>`: idle room eviction.
3. `--blob-gc-interval <secs>` / `--blob-gc-grace <secs>`.
4. `--snapshot-interval <N>` / `--snapshot-max-chain <K>`.
5. `--peer-rate-nodes <N>` / `--peer-rate-bytes <MiB>`.
6. `--broadcast-capacity <N>`.
7. `NODALMERGE_TOPOLOGY_PROMOTION_MAX_INFLIGHT` / `NODALMERGE_TOPOLOGY_PROMOTION_MAX_QUEUE`: bounded fair queue for topology promotion operations.
8. `NODALMERGE_LINEAGE_CHILDREN_INDEX_MAX`: optional retention cap for in-memory parent→children index.

Topology durability sidecars under `--store`:

1. `nodalmerge.db` (room nodes)
2. `topology-promotions.db` (promotion records)
3. `topology-lineage.db` (child lineage metadata)

## 5) Native/FFI guide

### Runtime-local FFI (peer-local persistence embedding)

Build:

```powershell
cargo build -p nodalmerge-runtime-local-ffi --release
```

Used by `.NET` `LocalPersistFfiClient` and `RuntimePeerLocalPersistenceService`.

### .NET runtime host

The host exposes:

1. `/ws/runtime` typed JSON runtime bridge
2. `/ws/{roomId}` compatibility alias
3. `/ws/ffi` binary FFI bridge
4. `/ffi/abi-version`, `/ffi/submit` HTTP FFI diagnostics path

Build/test host:

```powershell
dotnet build nodalmerge-host/NodalMerge.DotNetHost.slnx
dotnet test nodalmerge-host/NodalMerge.DotNetHost.slnx
```

## 6) End-to-end integration action list

Use this as the practical default:

1. Start `nodalmerge-server` with `--store`.
2. Run `scripts/integration-smoke.ps1` for build/vector/run/report checks.
3. Run `cargo test -p nodalmerge-server auth_room_006 -- --nocapture` for restart durability vectors.
4. Run web smoke (`web/smoke`) when touching SDK/bridge/browser behavior.
5. Run `nodalmerge-host` tests when touching FFI/.NET runtime mapping.
