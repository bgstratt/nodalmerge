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
  (both stores do, as of blob-cas-remediation 3.2); a corrupt stored blob is served as
  404, never as wrong bytes. **Scope: this server-side verify applies to identity
  responses.** The zstd pass-through response is contractually exempt — see "Content
  encoding" below; there, "never wrong bytes" is an end-to-end guarantee the client
  completes after decompress, not a server-side one.

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
- **Encoded pass-through exemption (contractual, decided 2026-07-16 —
  blob-cas-remediation 3.2, both runtimes).** A zstd response is the stored at-rest
  frame served **byte-for-byte** (`get_blob_encoded`, and the .NET encoded branch),
  not decode-then-recode. The server structurally cannot verify it: the hash is of the
  **plaintext**, so checking the frame means fully decompressing — the exact work the
  pass-through exists to avoid. Integrity on this path is completed by the client
  ("Clients MUST decompress before verifying/caching", above; the reference readers do —
  `HttpRemoteBlobStoreProvider` since Phase 3, the web SDK since blob-cas-remediation
  3.3). Accepted consequence: the same corrupt stored blob answers **404** on an
  identity GET (verify-on-read fails) and **200 + the corrupt frame** on an encoded GET;
  a client that decompresses and verifies rejects it there, so end-to-end "never wrong
  bytes" holds. Deliberately **no** verify-on-encoded-GET option exists: it would
  silently cost a full decompress per GET to buy a property the client already
  provides.

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

## Blob URL resolution (optional capability)

Status: **contract frozen 2026-07-15** (slice S4.1 of the CAS distribution plan,
`nodalmerge-studio/plans/cas-distribution-and-storage.md` Phase 4). Driven by
`engine/commands/blob-url-resolution-vectors.v1.json`.

This section adds the presigned-URL seam (plan D5, `IBlobUrlResolverProvider` /
`nodalmerge-s3-blobs`) to the blob HTTP surface. **Decision (recorded in the plan,
2026-07-15): URL resolution is an explicit endpoint, not a 307 redirect on
`GET/PUT /blobs/{hash}`.** Redirect-with-body semantics for PUT are fragile across HTTP
clients (auth headers stripped cross-origin, bodies not reliably replayed on 307), and a
chained provider's s3-direct link needs the URL as a plain value anyway. The relay
endpoints above are **unchanged** by this section; a server with no delegated backend
answers 501 here and clients fall back to the relay path, which keeps relay-only
deployments (Phase 2) conformant with zero code change.

### `GET /blobs/{hash}/url`

```
GET /blobs/{hash}/url?op=get|put[&size=<bytes>&contentType=<mime>]
```

- `{hash}` MUST be exactly 64 lowercase hex characters — same rule as the relay
  endpoints, checked first. Malformed → **400** `{"error":"non-canonical hash"}`.
- `op` is required and MUST be exactly `get` or `put`; anything else → **400**
  `{"error":"op must be 'get' or 'put'"}`.
- `size` and `contentType` are meaningful only for `op=put`: they flow through to the
  presign delegate as metadata (the `size`/`content_type` fields of the delegate presign
  protocol v1, `BLOB_STORAGE_LAYOUT.md` §7) so an app-side presign endpoint can, e.g.,
  enforce a size cap or set the object's content type. `op=put` without a positive
  integer `size` → **400**.
- Success → **200** `{"url": "<presigned url>", "expiresAtUtc": "<ISO-8601 UTC
  timestamp>"}`. There is no `expiresAt`-as-unix-seconds variant; every implementation
  of this endpoint emits `expiresAtUtc` as an ISO-8601 string.
- No presign-capable backend behind the server (relay-only deployment, or a backend that
  declined to presign) → **501**. A server MAY collapse "no resolver/backend configured
  at all" and "a configured backend declined to presign this request" into the same 501
  — this section does not require distinguishing them, and the .NET host does not
  distinguish them (see below).
- This endpoint carries no room/namespace semantics, same as the relay endpoints — the
  CAS is global and content-addressed. An implementation MAY require the same bearer
  auth as the relay endpoints when a token is configured.

### `POST /blobs/{hash}/uploaded`

```
POST /blobs/{hash}/uploaded
```

Upload confirmation: the client calls this once it has PUT bytes directly to a
presigned URL obtained above, so the server can flip the asset's lifecycle state from
`Uploading` to `Active` per `docs/delegated-storage-gc.md`.

- `{hash}` validation is identical to every other endpoint on this surface: malformed →
  **400** `{"error":"non-canonical hash"}`.
- A server **with** bucket visibility SHOULD verify the upload (HEAD the object at the
  canonical key, §"Bucket object layout" below) before flipping the asset to `Active`.
  Verification failure (object missing, or a size mismatch against what was declared at
  presign time) → **409**.
- A server **without** bucket visibility MAY accept without verification — this mirrors
  the Rust delegate auth mode's `BlobPersistence::verify_uploaded` default of `Ok(())`
  (Delegate mode has no bucket credentials to HEAD with; the app's own presign endpoint
  can verify independently by the time the room asks for the same hash on a read). The
  .NET host takes this branch today: it has no HEAD/verify hook behind
  `IBlobUrlResolverProvider`, so it accepts (**200**) once a resolver is registered.
- No presign-capable backend / resolver registered at all → **501**.

### Bucket object layout

Per `docs/BLOB_STORAGE_LAYOUT.md` §3 and §8 (v3 at-rest encoding), the bucket object a
resolved URL ultimately points at lives at:

```
{Prefix}blake3/<64-lowercase-hex>          identity encoding
{Prefix}blake3/<64-lowercase-hex>.zst      zstd encoding — hash is always of the
                                            UNCOMPRESSED bytes (the v3 invariant)
```

`{Prefix}` is deployment configuration (`path_prefix` in `nodalmerge-s3-blobs`, an
app-chosen prefix for a delegated backend) — never part of the hash's identity. No room
id or namespace is ever encoded into the key. Client-side compression before a presigned
PUT (upload `.zst` bytes + a `contentEncoding` object-metadata entry) is a Phase 4
seam — see `BLOB_STORAGE_LAYOUT.md` §8's S3 note; the .NET client-side link that produces
`.zst` uploads is slice 4.3, not this one.

### Status and what's deferred

- **This doc section is the frozen contract.** The .NET host
  (`WebApplicationExtensions.HandleBlobUrlResolveAsync` /
  `HandleBlobUploadedConfirmAsync`, routed through `IBlobUrlResolverProvider`) conforms
  to it as of slice S4.1.
- **The Rust server's HTTP implementation of this section is slice 4.2** — today the
  Rust server has no route for either endpoint; `nodalmerge-s3-blobs`'s
  `resolve_get_url`/`resolve_put_url`/`verify_uploaded` hooks exist and are consumed by
  the WS blob flow (`blob-redirect`/`blob-uploaded`) but not yet by an HTTP route. A
  disk-backed Rust server will answer neither route until 4.2 lands (whatever the
  framework's default for an unmapped route is — not necessarily 501 — since the route
  doesn't exist yet).
- **The .NET host's legacy `/sync/blob-url` route (and its `/api/sync/blob-url`
  mirror) remain as an alias of `GET /blobs/{hash}/url`**: same handler, same response
  shape (`{"url", "expiresAtUtc"}`), same status codes (400 malformed hash, 501 no
  backend). The only difference is the query-parameter surface: the legacy route also
  accepts `room`/`namespace` query parameters (defaulting to `"default"`/`"assets"` when
  absent) for backward compatibility with pre-S4.1 callers; the new route carries no
  room/namespace concept at all, consistent with this surface being room-agnostic.
