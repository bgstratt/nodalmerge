# nodalmerge-cli

Operator CLI for NodalMerge — no hand-written WebSocket clients required.

| Command group | Purpose |
|---------------|---------|
| `nodalmerge run` | Headless peer worker (sync + peer-local persistence) |
| `nodalmerge topology` | Parent/child rooms, lineage, promotion |
| `nodalmerge archive` | Describe, validate, export, import archives |
| `nodalmerge query` | Register specs, build/read/list/invalidate projections |
| `nodalmerge token` | Mint `NODALMERGE_TOKEN_JSON` for locked-room workflows |

Connects to a running `nodalmerge-server` WebSocket endpoint. Locked rooms require capability tokens (`topology.admin`, `archive.admin`, etc.) or use an open room for local development.

## Environment

| Variable | Purpose |
|----------|---------|
| `NODALMERGE_SERVER_URL` | WebSocket URL (e.g. `ws://127.0.0.1:7878/ws/my-room`) |
| `NODALMERGE_ROOM` | Session room id (path segment if omitted from URL) |
| `NODALMERGE_TOKEN_JSON` | Optional `hello.token` JSON for locked rooms |

## Examples

```bash
# Headless worker (same engine path as nodalmerge-headless binary)
export NODALMERGE_HEADLESS_SERVER_URL=ws://127.0.0.1:7878
export NODALMERGE_HEADLESS_ROOM=my-room
export NODALMERGE_HEADLESS_BACKEND=embedded
export NODALMERGE_HEADLESS_DATA_DIR=./peer-data
nodalmerge run --report-json session.json

export NODALMERGE_SERVER_URL=ws://127.0.0.1:7878/ws/parent-room
nodalmerge topology list-children --parent-room parent-room

nodalmerge archive describe --archive-ref room://parent-room --room parent-room
nodalmerge query list-projections --room my-room
nodalmerge query replay-read-range --room my-room --key-prefix world/ --from-lamport 0 --limit 50
nodalmerge token mint \
  --room parent-room \
  --room-key-seed-hex <64-hex-secret> \
  --peer-seed-hex <64-hex-peer-seed> \
  --caps topology.admin,archive.read \
  --ttl-secs 3600
nodalmerge topology show-lineage --room child-room
nodalmerge topology create-child \
  --parent-room parent-room \
  --child-room child-a \
  --purpose worker-task \
  --policy promotion-based \
  --parent-checkpoint-file checkpoint.json
```
