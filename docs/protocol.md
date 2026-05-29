# NodalMerge Protocol Catalog

Status: Active runtime catalog

Purpose: practical request/response catalog for current websocket protocol used by `nodalmerge-server`, aligned with host command/event mapping.

## 1. Envelope conventions

All messages are JSON objects with `type`.

1. Client -> server messages are requests, notifications, or data pushes.
2. Server -> client messages are acknowledgements, data pushes, or errors.
3. Binary node payloads are encoded in base64 fields (`nodes`, blob payloads).

## 2. Core sync flow

### 2.1 Hello

Client:

```json
{
  "type": "hello",
  "room": "room-1",
  "pubkey": "<hex>",
  "frontier": ["<node_id_hex>"],
  "caps": { "supports_ibf": true },
  "subscribe": ["**"]
}
```

Server:

```json
{
  "type": "welcome",
  "room": "room-1",
  "root": "<node_id_hex>",
  "missing": ["<node_id_hex>"],
  "caps": { "supports_ibf": true }
}
```

### 2.2 Pack push

Client or server:

```json
{
  "type": "pack",
  "from": "server",
  "nodes": "<base64_packed_nodes>"
}
```

### 2.3 Frontier request

Client:

```json
{
  "type": "request",
  "known": ["<node_id_hex>"]
}
```

Server responds with one or more `pack` messages.

### 2.4 Subscription update

Client:

```json
{
  "type": "subscribe",
  "patterns": ["world/**", "presence/**"]
}
```

Server:

```json
{
  "type": "subscribe-ack"
}
```

## 3. Blob flow

### 3.1 Inline upload

Client:

```json
{
  "type": "blob-upload",
  "blobs": [
    { "hash": "<blake3_hex>", "data_b64": "<base64>" }
  ]
}
```

Server announces availability:

```json
{
  "type": "blob-available",
  "hash": "<blake3_hex>",
  "from": "<peer_pubkey>"
}
```

### 3.2 Blob fetch

Client:

```json
{
  "type": "blob-request",
  "hashes": ["<blake3_hex>"]
}
```

Server returns direct bytes or redirect:

```json
{
  "type": "blob-pack",
  "blobs": [
    { "hash": "<blake3_hex>", "data_b64": "<base64>" }
  ]
}
```

or

```json
{
  "type": "blob-redirect",
  "redirects": [
    {
      "hash": "<blake3_hex>",
      "url": "https://...",
      "expires_at_unix": 1760000000
    }
  ]
}
```

### 3.3 Delegated upload

Client requests upload URL:

```json
{
  "type": "request-upload",
  "hash": "<blake3_hex>",
  "size": 12345,
  "content_type": "audio/ogg"
}
```

Server grants/denies:

```json
{
  "type": "upload-granted",
  "hash": "<blake3_hex>",
  "url": "https://...",
  "expires_at_unix": 1760000000
}
```

or

```json
{
  "type": "upload-denied",
  "hash": "<blake3_hex>",
  "reason": "..."
}
```

Client confirms upload completion:

```json
{
  "type": "blob-uploaded",
  "hash": "<blake3_hex>"
}
```

Server accepts/rejects:

```json
{ "type": "blob-available", "hash": "<blake3_hex>" }
```

or

```json
{ "type": "upload-rejected", "hash": "<blake3_hex>", "reason": "..." }
```

## 4. Presence flow

Client:

```json
{
  "type": "presence",
  "data": { "cursor": [120, 45], "status": "editing" }
}
```

Server broadcasts:

```json
{
  "type": "presence",
  "from": "<peer_pubkey>",
  "data": { "cursor": [120, 45], "status": "editing" }
}
```

Contract notes:

1. `presence` is ephemeral and never persisted as DAG state.
2. A peer's first `presence` update in a session is a `join`; later updates are `update`.
3. Session close emits `leave`; TTL expiry emits `stale`.
4. Lease fields are optional but must be supplied as a valid pair:
   - `ttl_ms` + `now_unix_ms` together, or neither.
5. Stable malformed-lease rejects:
   - `reject.presence_lease_invalid:ttl_ms_requires_now_unix_ms`
   - `reject.presence_lease_invalid:now_unix_ms_requires_ttl_ms`
   - `reject.presence_lease_invalid:ttl_ms_must_be_positive_u64`
   - `reject.presence_lease_invalid:now_unix_ms_must_be_u64`

## 5. Admin and room-control flow

### 5.1 Room auth key

Client:

```json
{
  "type": "set-room-key",
  "pubkey": "<ed25519_hex>"
}
```

Server:

```json
{ "type": "room-locked" }
```

### 5.2 Policy

Client:

```json
{
  "type": "set-policy",
  "default": "deny",
  "rules": [
    {
      "path_glob": "world/**",
      "allow": ["<peer_pubkey_hex>"]
    }
  ]
}
```

Server:

```json
{ "type": "policy-set" }
```

### 5.3 Server info

Client:

```json
{ "type": "server-info" }
```

Server:

```json
{
  "type": "server-info",
  "version": "<semver_or_build>",
  "caps": { "supports_ibf": true }
}
```

### 5.4 Tick control

Client:

```json
{
  "type": "start-tick",
  "interval_ms": 100,
  "intent_prefix": "intent/"
}
```

Server:

```json
{
  "type": "tick-started",
  "interval_ms": 100,
  "intent_prefix": "intent/"
}
```

or

```json
{
  "type": "tick-already-running",
  "interval_ms": 100,
  "intent_prefix": "intent/"
}
```

Stop:

```json
{ "type": "stop-tick" }
```

Server:

```json
{ "type": "tick-stopped" }
```

### 5.5 Compaction

Client:

```json
{ "type": "compact-room" }
```

Server:

```json
{
  "type": "snapshot-pack",
  "snapshot": "<base64_snapshot>",
  "frontier": ["<node_id_hex>"]
}
```

then

```json
{ "type": "compact-ack" }
```

## 6. Error envelope

Server error:

```json
{
  "type": "error",
  "msg": "human-readable reason"
}
```

Suggested client behavior:

1. Treat protocol/auth errors as non-retry until input changes.
2. Retry transient errors with bounded exponential backoff.
3. Log unknown message types for telemetry and forward compatibility.

## 7. Peer lifecycle events

Server emits:

```json
{ "type": "peer-joined", "peer": "<pubkey_prefix_or_hex>" }
```

and

```json
{ "type": "peer-left", "peer": "<pubkey_prefix_or_hex>" }
```

Lifecycle invariants:

1. `peer-joined` is emitted once when a peer pubkey transitions from zero to one active sessions.
2. `peer-left` is emitted once when that pubkey transitions from one to zero active sessions.
3. Reconnect overlap (two sockets temporarily open for the same pubkey) must not emit duplicate `peer-joined` or early `peer-left`.

## 8. Query, archive, and topology control plane

These message families are implemented on the Rust websocket ingress and mapped through host-core/FFI for native host adapters.

### 8.1 Query / projection

Requests:

```json
{ "type": "query.register", "query_spec_id": "q.rooms", "version": "v1", "descriptor": { "prefix": "world/" } }
```

```json
{ "type": "projection.build", "projection_id": "p.rooms", "query_spec_id": "q.rooms", "target_checkpoint": { "selector": "latest" } }
```

```json
{ "type": "projection.read", "projection_id": "p.rooms", "limit": 50, "page_token": "offset:0" }
```

```json
{ "type": "projection.invalidate", "projection_id": "p.rooms", "reason": "manual" }
```

```json
{ "type": "projection.list", "query_spec_id": "q.rooms", "state_filter": "active" }
```

```json
{ "type": "replay.read-range", "key_prefix": "world/", "from_lamport": 0, "limit": 100, "cursor": "offset:0" }
```

Primary responses:

1. `query.registered` / `query.register.rejected`
2. `projection.build.completed` / `projection.build.rejected`
3. `projection.read.result` / `projection.read.rejected`
4. `projection.invalidated` / `projection.invalidate.rejected`
5. `projection.list.result` / `projection.list.rejected`
6. `replay.read-range.result` / `replay.read-range.rejected`

### 8.2 Archive control plane

Requests:

```json
{ "type": "archive.describe", "archive_ref": "room://parent-room" }
```

```json
{ "type": "archive.validate", "archive_ref": "room://parent-room", "mode": "full_integrity" }
```

```json
{ "type": "archive.export", "source_room": "parent-room", "archive_ref": "file://C:/tmp/room.nmarchive" }
```

```json
{ "type": "archive.import", "archive_ref": "file://C:/tmp/room.nmarchive", "import_mode": "full_apply" }
```

Responses:

1. `archive.describe.result`
2. `archive.validate.result` / `archive.validate.rejected`
3. `archive.export.result` / `archive.export.rejected`
4. `archive.import.completed` / `archive.import.rejected`

Stable archive reject classes:

1. `reject.archive_unsupported_format`
2. `reject.archive_manifest_invalid`
3. `reject.archive_digest_mismatch`
4. `reject.archive_signature_invalid`
5. `reject.archive_checkpoint_not_found`
6. `reject.archive_policy_timeline_mismatch`

### 8.3 Topology control plane

Requests:

```json
{ "type": "topology.create-child", "parent_room_id": "parent-room", "child_room_id": "child-room-a", "child_purpose": "worker-task", "promotion_policy_id": "promotion-based", "parent_checkpoint": { "frontier": ["seq:1"], "canonical_hash": "<hex>" } }
```

```json
{ "type": "topology.describe-lineage", "room_id": "child-room-a" }
```

```json
{ "type": "topology.list-children", "parent_room_id": "parent-room" }
```

```json
{ "type": "topology.propose-promotion", "parent_room_id": "parent-room", "child_room_id": "child-room-a", "child_checkpoint_hash": "<hex>", "payload_ref": "room://child-room-a" }
```

```json
{ "type": "topology.validate-promotion", "proposal_id": "prop-1" }
```

```json
{ "type": "topology.apply-promotion", "proposal_id": "prop-1" }
```

Responses:

1. `topology.create-child.completed`
2. `topology.describe-lineage.result`
3. `topology.list-children.result`
4. `topology.propose-promotion.completed`
5. `topology.validate-promotion.completed`
6. `topology.apply-promotion.completed`

## 9. Notes for host extraction

1. This catalog is transport-level documentation only.
2. Host command/event interfaces in `HOST_COMMAND_EVENT_CONTRACT.md` are the host-neutral control plane.
3. JSON is adapter-level; future C ABI should prefer typed or binary payload transport.
