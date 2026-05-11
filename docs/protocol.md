# ActiveSync Protocol Catalog

Status: Draft baseline (Track B / PR-08)

Purpose: practical request/response catalog for current websocket protocol used by `activesync-server`, aligned with host command/event mapping.

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

## 8. Notes for host extraction

1. This catalog is transport-level documentation only.
2. Host command/event interfaces in `HOST_COMMAND_EVENT_CONTRACT.md` are the host-neutral control plane.
3. JSON is adapter-level; future C ABI should prefer typed or binary payload transport.
