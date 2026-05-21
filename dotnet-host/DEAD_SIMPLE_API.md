# Dead-Simple Runtime API Surface

This is the package-level surface we treat as the baseline for adoption.

## 1. room

- ensure-room
- open-session
- client-hello
- close-session

## 2. sync

- map-set, map-get, map-delete, map-all
- text-insert, text-delete, text-get
- list-push, list-insert, list-delete, list-move, list-update, list-get

## 3. replay

- pack, request-server-pack
- mst-request, mst-done
- recent-conflicts

## 4. offline

- transport/session reconnect flows over ws runtime endpoint
- queued operations and catch-up through request/pack and mst paths

## 5. CAS

- blob-set, blob-get, blob-get-many
- request-upload
- blob-request (redirect or blob-pack)

## 6. topology

- replication/topology endpoint in host
- runtime events and operation stream diagnostics

## 7. API transport

- WebSocket runtime endpoint: /ws/runtime
- Optional compatibility alias: /ws/{roomId}
- HTTP host health: /api/host/health

Notes:

- This is intentionally low-level and deterministic.
- Higher-level sdk ergonomics can wrap this transport/runtime surface.
