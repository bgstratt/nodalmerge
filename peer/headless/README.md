# nodalmerge-headless

Headless NodalMerge peer worker: WebSocket sync to a reflector with peer-local persistence (`memory` or `file`).

## Container

```bash
docker build -f headless/Dockerfile -t nodalmerge-headless .
docker run --rm \
  -e NODALMERGE_HEADLESS_SERVER_URL=ws://host.docker.internal:8080/ws/my-room \
  -e NODALMERGE_HEADLESS_ROOM=my-room \
  -v nodalmerge-peer-data:/data \
  nodalmerge-headless
```

Default image settings: `BACKEND=file`, `DATA_DIR=/data`, IBF/MST negotiation on.

## Environment

| Variable | Purpose |
|----------|---------|
| `NODALMERGE_HEADLESS_SERVER_URL` | WebSocket URL (`ws://host:port/ws/room-id`) |
| `NODALMERGE_HEADLESS_ROOM` | Room id |
| `NODALMERGE_HEADLESS_BACKEND` | `memory`, `file`, `embedded`, `sqlite`, or `composite`; `registered:<name>` for registry pilots |
| `NODALMERGE_HEADLESS_DATA_DIR` | Peer-local directory (required for `file`) |
| `NODALMERGE_HEADLESS_RUN_SECS` | Catch-up window after hello (default `10`) |
| `NODALMERGE_HEADLESS_NEGOTIATE_IBF` | `0`/`false` to disable IBF in hello |
| `NODALMERGE_HEADLESS_NEGOTIATE_MST` | `0`/`false` to disable MST descent after welcome |
| `NODALMERGE_HEADLESS_REPORT_JSON` | Write session report JSON (`-` for stdout) |

Sync uses IBF set-reconciliation when the local log is non-empty, then MST descent when roots differ, then applies server `pack` messages during the run window.

## Health probe

```bash
nodalmerge-headless --health
# {"artifact":"nodalmerge-headless-health","backend":"memory","durable":false}
```

## Session report

```bash
nodalmerge-headless --report-json ./session.json ...
```

Emits `timings_ms`, `backend`, `durable`, sync counters, and `canonical_hash_hex` for operator dashboards. See `docs/operator.md` (Headless peer worker section).
