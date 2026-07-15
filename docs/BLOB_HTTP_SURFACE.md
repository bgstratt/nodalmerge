# Blob HTTP surface (frozen contract)

Status: **v1 frozen 2026-07-15** (slice S2.0 of the CAS distribution plan,
`nodalmerge-studio/plans/cas-distribution-and-storage.md` Phase 2). Served in parity by
the .NET host (`NodalMerge.DotNetHost`) and the Rust server; both are driven by the same
golden vectors: `engine/commands/blob-http-surface-vectors.v1.json`. If you change the
surface, this doc, the vectors, and both implementations must move together.

This is the **server-relay origin** surface (plan D5): a well-known blob origin that
peers' chained blob providers fetch from and push to. It deliberately carries no room or
namespace semantics — the CAS is global, content-addressed, and self-verifying. Per-repo
*access* control is a token/capability concern layered on the endpoint (Phase 6); the
existing WebSocket blob flow (`blob-request`/`blob-pack`/`blob-upload`) is unchanged and
remains the in-room transfer path.

## Routes

```
GET  /blobs/{hash}
HEAD /blobs/{hash}
PUT  /blobs/{hash}
```

The .NET host additionally mirrors each route at `/api/blobs/{hash}` (host convention,
cf. `/sync/blob-url` + `/api/sync/blob-url`). The Rust server serves only `/blobs/…`.

`{hash}` MUST be exactly 64 lowercase hex characters (the canonical blob name,
`BLOB_STORAGE_LAYOUT.md` §3). Anything else → **400** with body
`{"error":"non-canonical hash"}`. No normalization: uppercase is 400, not lowercased.

## Semantics

### GET
- Found → **200**, body = the blob's raw bytes, headers `Content-Type:
  application/octet-stream` (always — stores do not persist content types; type is a
  hint, never identity) and `ETag: "<hash>"` (quoted).
- Missing → **404**, body `{"error":"not found"}`.
- Servers verify stored bytes against the hash on read where their store supports it
  (the Rust store already does); a corrupt stored blob is served as 404, never as wrong
  bytes.

### HEAD
- Same status codes as GET, no body. Exists for cheap existence checks (the reconcile
  sweep). Headers on 200 match GET's (minus body-dependent ones).

### PUT
- Request body = the blob's **raw, uncompressed** bytes. A `Content-Type` request
  header is accepted and may be ignored.
- The server computes BLAKE3 of the received body:
  - mismatch with `{hash}` → **422**, body `{"error":"hash mismatch"}`, nothing stored;
  - blob already present → **200** (conditional-PUT no-op — idempotent by construction);
  - newly stored → **201**.
- Body larger than the configured limit → **413**. The limit applies to the actual
  received size (chunked bodies without `Content-Length` are capped while buffering).
  Default limit: **64 MiB**.
- Durability: PUT against a server without a durable blob store (e.g. Rust
  `NoPersistence`) is not meaningful; an origin deployment requires a durable store
  (Rust: `--store <path>`; .NET: `Providers:BlobStorage = File` or better).

### Auth (v1)
- Optional static bearer token. When the server has **no token configured**, the blob
  endpoints are anonymous (matches the rest of the current HTTP surface). When a token
  is configured, every blob request must carry `Authorization: Bearer <token>`;
  wrong/missing → **401** `{"error":"unauthorized"}`.
- Config: .NET `NodalMerge:BlobHttp:AuthToken`; Rust `--blob-token` /
  `NODALMERGE_BLOB_TOKEN`.
- Upgrade path (recorded, not built): Phase 6 replaces the static token with
  workgroup/repo-room capability tokens at this same endpoint (plan D1/D5) — per-repo
  access control is enforced here, while the store stays one global CAS.

### Content encoding (reserved v1.1 — Phase 3)
- GET: a client MAY send `Accept-Encoding: zstd`; the server MAY respond
  `Content-Encoding: zstd` with a single zstd frame whose **decompressed** bytes hash to
  `{hash}`. Clients MUST decompress before verifying/caching. Servers that don't
  implement this simply serve identity.
- PUT with `Content-Encoding` is reserved; servers MAY reject it with **415** until
  implemented.
- The hash is always BLAKE3 of the uncompressed bytes (layout invariant).

## Storage

The store behind these endpoints MUST use the shared CAS layout
(`docs/BLOB_STORAGE_LAYOUT.md`), so a store directory written through one
implementation is readable by the other (and by the WS blob flow, GC, and
`nodalmerge-s3-blobs`).

## Client behavior (normative for `HttpRemoteBlobStoreProvider` / chained providers)

- Verify BLAKE3 of fetched (and, where applicable, decompressed) bytes against the
  requested hash **before** caching or returning; a mismatch is a fetch failure and the
  payload must never be written to a local store.
- Treat 5xx/429/timeouts as retryable; 400/401/404/413/422 are not.
- A PUT answered 200 or 201 is success; there is no need to distinguish them.
