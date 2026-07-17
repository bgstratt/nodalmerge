//! S2.1b — the blob HTTP origin surface: `GET`/`HEAD`/`PUT /blobs/{hash}`.
//!
//! Frozen contract: `docs/BLOB_HTTP_SURFACE.md`, golden vectors
//! `engine/commands/blob-http-surface-vectors.v1.json`. The .NET mirror is
//! `hosts/dotnet/tests/NodalMerge.DotNetHost.Tests/BlobHttpSurfaceTests.cs`
//! (and its production wiring). If you change behavior here, the doc, the
//! vectors, and both harnesses must move together.
//!
//! This is deliberately **not** the existing WebSocket blob flow
//! (`blob-request`/`blob-pack`/`blob-upload` in `ws_handler.rs`), which is
//! unchanged and remains the in-room transfer path. This module adds a
//! well-known, room-agnostic origin that peers' chained blob providers can
//! fetch from / push to, backed by the same global CAS
//! (`docs/BLOB_STORAGE_LAYOUT.md`) via `Rooms::persistence`.
//!
//! HEAD used to be served by axum's built-in GET→HEAD behavior
//! (`MethodRouter` routing a `HEAD` request to the `get(...)` handler and
//! stripping the response body afterward) — that stopped being correct once
//! S4.2 added S3-backed stores: `S3BlobStore::get_blob` never hydrates
//! bytes (`None` always, by design — see `store::BlobPersistence::get_blob`'s
//! doc), so reusing `GET`'s logic for `HEAD` would report every blob
//! missing even when the bucket object exists. `HEAD` now has its own
//! `.head(...)` handler (`head_blob`, below) that answers via
//! `BlobPersistence::has_blob` — a cheap existence check (a bucket `HEAD`
//! for S3-like backends) — and manually strips the body at the end, since
//! an explicit handler doesn't get axum's automatic GET→HEAD stripping.
//!
//! S4.2 also adds the two "blob URL resolution" endpoints from
//! `docs/BLOB_HTTP_SURFACE.md`: `GET /blobs/{hash}/url` and `POST
//! /blobs/{hash}/uploaded`. Both are entirely backend-agnostic — they only
//! ever call through the `BlobPersistence` trait object on `rooms.persistence`
//! — so no S3-specific code lives in this crate; `nodalmerge-s3-blobs`
//! plugs in underneath via whatever composes `rooms.persistence` at startup
//! (see `server/server-s3`, the composition binary S4.2 added, since
//! `nodalmerge-server` itself can't depend on `nodalmerge-s3-blobs` without
//! a cyclic package dependency — `nodalmerge-s3-blobs` already depends on
//! `nodalmerge-server` for the `BlobPersistence` trait).
//!
//! The 413 (payload too large) response for oversize `PUT` bodies is not
//! hand-rolled: `DefaultBodyLimit::max(cfg.max_blob_bytes)` is layered onto
//! these routes only (via `route_layer`, so `/ws/:room_id` is unaffected),
//! and axum's `Bytes` extractor itself returns a `413 Payload Too Large`
//! rejection when the (possibly chunked, possibly `Content-Length`-less)
//! body stream exceeds that limit while buffering.
//!
//! S6.2 (blob-cas-remediation.md): every handler splits into a cheap, pure
//! validation prefix that stays on the async worker and a store-touching
//! tail that runs on the blocking pool via [`off_worker`] — see its doc for
//! why. Scheduling only: statuses, bodies, headers, and the order of
//! side effects within a request (hash → inventory mark → store) are
//! unchanged.

use std::sync::Arc;

use axum::{
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use nodalmerge_core::Hash;
use nodalmerge_gc::contracts::AssetInventoryStore;
use serde::Deserialize;
use serde_json::json;

use crate::room::Rooms;
use crate::store::{hash_from_hex, is_canonical_blob_name};

/// S4.2 — the `room_id`/namespace placeholder passed to
/// `BlobPersistence::resolve_get_url`/`resolve_put_url`/`verify_uploaded`
/// from this room-agnostic HTTP surface. Those trait methods take a
/// `room_id` because it flows through as metadata to delegate protocols
/// (`docs/BLOB_STORAGE_LAYOUT.md` §7) — it plays no role in where/how a
/// blob's bytes are stored (the CAS is global). `"_global"` mirrors the
/// existing placeholder `room.rs`'s `sweep_blobs` already uses for the same
/// "this isn't really any one room" situation when calling
/// `gc_adapter::run_mark_only_preflight`.
const GLOBAL_ROOM_PLACEHOLDER: &str = "_global";

/// Runtime configuration for the blob HTTP origin routes.
#[derive(Clone)]
pub struct BlobHttpConfig {
    /// Static bearer token. `None` = anonymous (matches the rest of the
    /// current HTTP surface). `Some(token)` requires every blob request to
    /// carry `Authorization: Bearer <token>`.
    pub auth_token: Option<String>,
    /// Max accepted `PUT` body size, in bytes. Enforced via
    /// `DefaultBodyLimit` on the whole body stream (not just
    /// `Content-Length`), so chunked bodies are capped while buffering too.
    pub max_blob_bytes: usize,
    /// S5.3 — the GC coordinator's asset inventory, if the server has one
    /// configured (`--store` + `--blob-gc-interval`). When present, a
    /// successful local `PUT` (identity write) or a successful
    /// `POST /blobs/{hash}/uploaded` confirmation (S4.2 presign path) both
    /// upsert the hash as `Active` in the inventory immediately — the
    /// `Uploading -> Active` transition `docs/delegated-storage-gc.md`
    /// calls normative, rather than waiting for the next scheduled mark
    /// pass to notice the reference. `None` (the default) is a no-op —
    /// every existing deployment without the new coordinator enabled is
    /// unaffected.
    pub gc_inventory: Option<Arc<dyn AssetInventoryStore>>,
}

impl Default for BlobHttpConfig {
    fn default() -> Self {
        Self {
            auth_token: None,
            max_blob_bytes: 64 * 1024 * 1024, // 64 MiB
            gc_inventory: None,
        }
    }
}

impl std::fmt::Debug for BlobHttpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlobHttpConfig")
            .field("auth_token", &self.auth_token.as_ref().map(|_| "<redacted>"))
            .field("max_blob_bytes", &self.max_blob_bytes)
            .field("gc_inventory", &self.gc_inventory.is_some())
            .finish()
    }
}

/// S5.3 — sentinel `run_id` used when upserting an inventory row outside a
/// real `GcCoordinator::run_once` call (i.e. from the upload-confirm path
/// below, not a mark pass). Never matched against a real run id; it exists
/// purely so the row has *some* `last_marked_run_id` and reads clearly in
/// the ledger.
const UPLOAD_MARK_SENTINEL: &str = "upload";

/// Build the `/blobs/:hash` routes (`GET`, `HEAD`, `PUT`) plus the S4.2
/// blob-URL-resolution routes (`GET /blobs/:hash/url`, `POST
/// /blobs/:hash/uploaded`), scoped to `Router<Rooms>` so callers can
/// `.merge()` this into their existing room router. `cfg` is captured by
/// the route closures — no `Extension` layer needed.
pub fn blob_routes(cfg: BlobHttpConfig) -> Router<Rooms> {
    let max_bytes = cfg.max_blob_bytes;
    let get_cfg = cfg.clone();
    let head_cfg = cfg.clone();
    let put_cfg = cfg.clone();
    let url_cfg = cfg.clone();
    let uploaded_cfg = cfg;

    Router::new()
        .route(
            "/blobs/:hash",
            get({
                let cfg = get_cfg;
                move |state: State<Rooms>, path: Path<String>, headers: HeaderMap| {
                    let cfg = cfg.clone();
                    async move { get_blob(state, path, headers, cfg).await }
                }
            })
            .head({
                let cfg = head_cfg;
                move |state: State<Rooms>, path: Path<String>, headers: HeaderMap| {
                    let cfg = cfg.clone();
                    async move { head_blob(state, path, headers, cfg).await }
                }
            })
            .put({
                let cfg = put_cfg;
                move |state: State<Rooms>,
                      path: Path<String>,
                      headers: HeaderMap,
                      body: Bytes| {
                    let cfg = cfg.clone();
                    async move { put_blob(state, path, headers, body, cfg).await }
                }
            }),
        )
        .route(
            "/blobs/:hash/url",
            get({
                let cfg = url_cfg;
                move |state: State<Rooms>,
                      path: Path<String>,
                      headers: HeaderMap,
                      query: Query<BlobUrlQuery>| {
                    let cfg = cfg.clone();
                    async move { resolve_blob_url(state, path, headers, query, cfg).await }
                }
            }),
        )
        .route(
            "/blobs/:hash/uploaded",
            post({
                let cfg = uploaded_cfg;
                move |state: State<Rooms>, path: Path<String>, headers: HeaderMap| {
                    let cfg = cfg.clone();
                    async move { confirm_blob_uploaded(state, path, headers, cfg).await }
                }
            }),
        )
        // Scoped to just these routes: `/ws/:room_id` keeps axum's default
        // (2 MiB) body limit untouched.
        .route_layer(DefaultBodyLimit::max(max_bytes))
}

/// S6.2 — run a store-touching closure on tokio's blocking pool and await
/// it, keeping the async worker free to serve WS/sync traffic.
///
/// Every `BlobPersistence` method is synchronous by design (the trait is
/// implemented sync by Dir/S3/Composite and the test fakes): file I/O plus
/// BLAKE3 verify/zstd decode on `DirPersistence`, and on `S3BlobStore` a
/// bridge that *blocks the calling thread* for up to `op_timeout + 15s`
/// (bounded since 6.1, but still parked). PUT additionally BLAKE3-hashes
/// an up-to-64 MiB body. Enough of that running in-line on handlers
/// starves everything else sharing the runtime — hence this boundary at
/// the handler layer rather than an async-trait conversion (the sync trait
/// is load-bearing across four implementations).
///
/// `Err` is the `JoinError` case: the closure panicked, or the runtime is
/// shutting down. Either way it must surface as a logged 500 — never a
/// hang, a torn-down connection, or a silent success.
async fn off_worker<T, F>(f: F) -> Result<T, Response>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(f).await.map_err(|join_err| {
        tracing::error!(?join_err, "blob HTTP blocking task failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "internal error"})),
        )
            .into_response()
    })
}

async fn get_blob(
    State(rooms): State<Rooms>,
    Path(hash_hex): Path<String>,
    headers: HeaderMap,
    cfg: BlobHttpConfig,
) -> Response {
    // a. Canonical name check first, before auth or any store access.
    if !is_canonical_blob_name(&hash_hex) {
        return non_canonical_response();
    }
    // b. Auth.
    if let Some(resp) = check_auth(&headers, &cfg) {
        return resp;
    }
    // c. GET (and, via axum's built-in body-stripping, HEAD too).
    let hash = match hash_from_hex(&hash_hex) {
        Some(h) => h,
        None => return non_canonical_response(), // unreachable: canonical checked above
    };

    let wants_zstd = accepts_zstd(&headers);
    let persistence = Arc::clone(&rooms.persistence);
    off_worker(move || {
        // S3.1b (docs/BLOB_HTTP_SURFACE.md "Content encoding"): if the client
        // advertises `Accept-Encoding: zstd` and the store holds the stored
        // zstd form, serve those bytes as-is — no recompress-on-serve. Identity
        // requests (no header, or the store has no `.zst` form) are unchanged.
        if wants_zstd {
            if let Some((bytes, encoding)) = persistence.get_blob_encoded(&hash) {
                return (
                    StatusCode::OK,
                    [
                        (header::CONTENT_TYPE, "application/octet-stream".to_string()),
                        (header::CONTENT_ENCODING, encoding.to_string()),
                        (header::ETAG, format!("\"{hash_hex}\"")),
                    ],
                    bytes,
                )
                    .into_response();
            }
        }

        match persistence.get_blob(&hash) {
            Some(bytes) => (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, "application/octet-stream".to_string()),
                    (header::ETAG, format!("\"{hash_hex}\"")),
                ],
                bytes,
            )
                .into_response(),
            None => not_found_response(),
        }
    })
    .await
    .unwrap_or_else(|resp| resp)
}

/// S4.2 — `HEAD /blobs/{hash}`. Deliberately *not* a thin wrapper around
/// `get_blob`: it answers via `has_blob` (a cheap existence check — a real
/// bucket `HEAD` for S3-like backends) rather than `get_blob`/
/// `get_blob_encoded` (which never hydrate bytes for those backends). Builds
/// the same status/headers a `GET` would, then strips the body — matching
/// what axum's automatic GET→HEAD conversion used to do before this
/// explicit handler existed, so `head-found`/`head-missing`
/// (`blob-http-surface-vectors.v1.json`) stay green unchanged.
async fn head_blob(
    State(rooms): State<Rooms>,
    Path(hash_hex): Path<String>,
    headers: HeaderMap,
    cfg: BlobHttpConfig,
) -> Response {
    let resp = async {
        if !is_canonical_blob_name(&hash_hex) {
            return non_canonical_response();
        }
        if let Some(resp) = check_auth(&headers, &cfg) {
            return resp;
        }
        let hash = match hash_from_hex(&hash_hex) {
            Some(h) => h,
            None => return non_canonical_response(), // unreachable: canonical checked above
        };
        // S6.2 — `has_blob` is a file stat locally but a real bucket HEAD
        // (thread-blocking bridge) on S3 backends; off the async worker.
        let persistence = Arc::clone(&rooms.persistence);
        off_worker(move || {
            if persistence.has_blob(&hash) {
                (
                    StatusCode::OK,
                    [
                        (header::CONTENT_TYPE, "application/octet-stream".to_string()),
                        (header::ETAG, format!("\"{hash_hex}\"")),
                    ],
                )
                    .into_response()
            } else {
                not_found_response()
            }
        })
        .await
        .unwrap_or_else(|resp| resp)
    }
    .await;
    let (parts, _body) = resp.into_parts();
    Response::from_parts(parts, Body::empty())
}

/// S4.2 — query params for `GET /blobs/{hash}/url`
/// (`docs/BLOB_HTTP_SURFACE.md` "Blob URL resolution"). Fields are all
/// `Option<String>` (not `Option<u64>`/typed) so a malformed `size` (e.g.
/// non-numeric) is a 400 we control the message for, rather than an axum
/// query-deserialize rejection with a different shape.
#[derive(Debug, Deserialize)]
struct BlobUrlQuery {
    op: Option<String>,
    size: Option<String>,
    #[serde(rename = "contentType")]
    content_type: Option<String>,
}

/// S4.2 — `GET /blobs/{hash}/url?op=get|put[&size=&contentType=]`. Frozen
/// contract: `docs/BLOB_HTTP_SURFACE.md` "Blob URL resolution", golden
/// vectors `engine/commands/blob-url-resolution-vectors.v1.json`.
async fn resolve_blob_url(
    State(rooms): State<Rooms>,
    Path(hash_hex): Path<String>,
    headers: HeaderMap,
    Query(query): Query<BlobUrlQuery>,
    cfg: BlobHttpConfig,
) -> Response {
    // a. Canonical name check first, same rule/order as the relay endpoints.
    if !is_canonical_blob_name(&hash_hex) {
        return non_canonical_response();
    }
    // b. Auth (optional — same static bearer token as the relay endpoints).
    if let Some(resp) = check_auth(&headers, &cfg) {
        return resp;
    }
    let hash = match hash_from_hex(&hash_hex) {
        Some(h) => h,
        None => return non_canonical_response(), // unreachable: canonical checked above
    };

    // S6.2 — presign resolution looks pure but is not: in S3 Delegate mode
    // it is a full HTTP round trip through the thread-blocking bridge; off
    // the async worker. Query validation (op/size) stays on the worker —
    // its 400s never touch the store.
    let persistence = Arc::clone(&rooms.persistence);
    let presigned = match query.op.as_deref() {
        Some("get") => {
            // `size` is meaningful only for `op=put` per the doc; if a
            // caller sends it on `op=get` anyway, pass it through as a
            // best-effort size hint rather than erroring.
            let size_hint = query.size.as_deref().and_then(|s| s.parse::<u64>().ok());
            off_worker(move || {
                persistence.resolve_get_url(GLOBAL_ROOM_PLACEHOLDER, &hash, size_hint)
            })
            .await
        }
        Some("put") => {
            let size = match query.size.as_deref().map(str::parse::<u64>) {
                Some(Ok(n)) if n > 0 => n,
                _ => {
                    return bad_request_response(
                        "size is required and must be a positive integer for op=put",
                    )
                }
            };
            let content_type = query.content_type.clone();
            off_worker(move || {
                persistence.resolve_put_url(
                    GLOBAL_ROOM_PLACEHOLDER,
                    &hash,
                    size,
                    content_type.as_deref(),
                )
            })
            .await
        }
        _ => return bad_request_response("op must be 'get' or 'put'"),
    };
    let presigned = match presigned {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match presigned {
        Some(p) => (
            StatusCode::OK,
            Json(json!({
                "url": p.url,
                "expiresAtUtc": format_iso8601_utc(p.expires_at_unix),
            })),
        )
            .into_response(),
        // No presign-capable backend, or a configured backend declined —
        // the doc allows collapsing both into 501 (relay-only deployments
        // stay conformant with zero code change).
        None => StatusCode::NOT_IMPLEMENTED.into_response(),
    }
}

/// S4.2 — `POST /blobs/{hash}/uploaded`. Frozen contract:
/// `docs/BLOB_HTTP_SURFACE.md` "Blob URL resolution", golden vectors
/// `engine/commands/blob-url-resolution-vectors.v1.json`.
async fn confirm_blob_uploaded(
    State(rooms): State<Rooms>,
    Path(hash_hex): Path<String>,
    headers: HeaderMap,
    cfg: BlobHttpConfig,
) -> Response {
    if !is_canonical_blob_name(&hash_hex) {
        return non_canonical_response();
    }
    if let Some(resp) = check_auth(&headers, &cfg) {
        return resp;
    }
    let hash = match hash_from_hex(&hash_hex) {
        Some(h) => h,
        None => return non_canonical_response(), // unreachable: canonical checked above
    };

    // S6.2 — `verify_uploaded` is a bucket HEAD through the thread-blocking
    // bridge in S3 Direct mode, and the inventory upsert is a synchronous
    // SQLite write; both off the async worker. `supports_presigned_urls`
    // rides along (pure, but it belongs with the store logic it gates).
    let persistence = Arc::clone(&rooms.persistence);
    let gc_inventory = cfg.gc_inventory.clone();
    off_worker(move || {
        // Distinguish "no presign-capable backend at all" (501) from "a real
        // backend accepted without verification" (Delegate mode, 200) — see
        // `store::BlobPersistence::supports_presigned_urls`'s doc for why
        // `verify_uploaded`'s own `Ok(())` can't be used for this on its own.
        if !persistence.supports_presigned_urls() {
            return StatusCode::NOT_IMPLEMENTED.into_response();
        }

        match persistence.verify_uploaded(GLOBAL_ROOM_PLACEHOLDER, &hash) {
            Ok(()) => {
                // S5.3: Uploading -> Active per docs/delegated-storage-gc.md's
                // normative lifecycle guidance, immediately rather than waiting
                // for the next scheduled mark pass.
                if let Some(inv) = &gc_inventory {
                    if let Err(e) = inv.upsert_active_seen(UPLOAD_MARK_SENTINEL, &hash_hex, std::time::SystemTime::now()) {
                        tracing::warn!(?e, hash = %hash_hex, "gc inventory upsert after verify_uploaded failed");
                    }
                }
                StatusCode::OK.into_response()
            }
            Err(msg) => (StatusCode::CONFLICT, Json(json!({"error": msg}))).into_response(),
        }
    })
    .await
    .unwrap_or_else(|resp| resp)
}

/// Format a Unix-epoch second count as an ISO-8601 UTC timestamp
/// (`YYYY-MM-DDTHH:MM:SSZ`). Hand-rolled rather than pulling in `chrono`/
/// `time` — no crate in this workspace currently depends on either, and
/// `main.rs` already hand-rolls its own base64 codec for the same "don't
/// add a dependency for one small pure function" reason.
fn format_iso8601_utc(unix_secs: u64) -> String {
    // Clamp rather than overflow on the (never-produced-in-this-codebase)
    // `u64::MAX` "unknown expiry" sentinel some `PresignedUrl` constructors
    // could in principle return.
    let unix_secs = unix_secs.min(253_402_300_799); // 9999-12-31T23:59:59Z
    let days = (unix_secs / 86_400) as i64;
    let secs_of_day = unix_secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Howard Hinnant's `civil_from_days`: days-since-epoch (1970-01-01) →
/// (year, month, day), proleptic Gregorian. Pure integer arithmetic, no
/// external date/time dependency.
/// <http://howardhinnant.github.io/date_algorithms.html#civil_from_days>
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

/// Whether the request's `Accept-Encoding` header lists `zstd` (case-
/// insensitive; a comma-separated list per RFC 9110) AND does not refuse it
/// via `q=0` (S6.4/3.4, blob-cas-remediation.md finding "Accept-Encoding:
/// zstd;q=0 treated as accept"). This honors only the `q=0` refusal case —
/// a nonzero `q` or no `q` at all keeps the pre-3.4 accept-if-mentioned
/// behavior; this is a MAY-serve optimization, not full qvalue-preference
/// ordering (a client sending `zstd;q=0.1, gzip;q=0.9` still gets zstd here
/// if the store has it — that ordering question is out of scope).
fn accepts_zstd(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(',').any(coding_accepts_zstd))
        .unwrap_or(false)
}

/// Whether one `Accept-Encoding` list element (e.g. `"zstd;q=0"`, `" gzip "`)
/// names `zstd` and is not refused by that same element's own `q=0`
/// parameter. Parameters are per-coding — `"gzip;q=0, zstd"` must still
/// accept zstd; `gzip`'s `q=0` must not leak onto the next token.
fn coding_accepts_zstd(token: &str) -> bool {
    let mut parts = token.split(';');
    let name = parts.next().unwrap_or("").trim();
    if !name.eq_ignore_ascii_case("zstd") {
        return false;
    }
    !parts.any(|param| {
        let param = param.trim();
        let mut kv = param.splitn(2, '=');
        let key = kv.next().unwrap_or("").trim();
        let value = kv.next().unwrap_or("").trim();
        key.eq_ignore_ascii_case("q") && is_qvalue_zero(value)
    })
}

/// RFC 9110 qvalue syntax is `( "0" [ "." 0*3DIGIT ] ) / ( "1" [ "." 0*3("0") ] )`.
/// This recognizes every all-zero spelling of the zero branch — `0`, `0.`,
/// `0.0`, `0.00`, `0.000` — and nothing else (a malformed or >3-decimal
/// value is treated as "not q=0", i.e. accepted; this function only needs to
/// catch refusal, not validate the header).
fn is_qvalue_zero(raw: &str) -> bool {
    if raw == "0" {
        return true;
    }
    match raw.strip_prefix("0.") {
        Some(rest) => rest.len() <= 3 && rest.bytes().all(|b| b == b'0'),
        None => false,
    }
}

async fn put_blob(
    State(rooms): State<Rooms>,
    Path(hash_hex): Path<String>,
    headers: HeaderMap,
    body: Bytes,
    cfg: BlobHttpConfig,
) -> Response {
    // a. Canonical name check first — before auth, before touching the
    // body/store. Matches the vector `put-rejects-non-canonical-before-auth-or-body`.
    if !is_canonical_blob_name(&hash_hex) {
        return non_canonical_response();
    }
    // b. Auth.
    if let Some(resp) = check_auth(&headers, &cfg) {
        return resp;
    }
    // c. Content-Encoding on PUT is reserved (docs/BLOB_HTTP_SURFACE.md
    // "Content encoding"): reject with 415 until pre-compressed uploads are
    // implemented, rather than silently persisting compressed bytes under
    // the identity hash.
    if headers.contains_key(header::CONTENT_ENCODING) {
        return unsupported_media_type_response();
    }
    let path_hash = match hash_from_hex(&hash_hex) {
        Some(h) => h,
        None => return non_canonical_response(), // unreachable: canonical checked above
    };

    // d. (Oversize bodies never reach here — `DefaultBodyLimit` + the
    // `Bytes` extractor reject them with 413 before the handler runs.)
    //
    // S6.2 — everything from here down runs off the async worker: the
    // BLAKE3 hash of an up-to-64 MiB body, the SQLite inventory upsert,
    // and the store round trip (file I/O, or a bucket HEAD + PUT through
    // the thread-blocking bridge on S3 backends). Order within the request
    // is unchanged: hash → 422 check → inventory mark → dedupe → persist.
    let persistence = Arc::clone(&rooms.persistence);
    let gc_inventory = cfg.gc_inventory.clone();
    off_worker(move || {
        let computed = Hash::of(&body);
        if computed != path_hash {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({"error": "hash mismatch"})),
            )
                .into_response();
        }

        // S5.3: either branch below means the bytes now genuinely exist under
        // `hash_hex` — mark it Active immediately (see `gc_inventory`'s doc).
        if let Some(inv) = &gc_inventory {
            if let Err(e) = inv.upsert_active_seen(UPLOAD_MARK_SENTINEL, &hash_hex, std::time::SystemTime::now()) {
                tracing::warn!(?e, hash = %hash_hex, "gc inventory upsert after PUT failed");
            }
        }

        if persistence.has_blob(&path_hash) {
            return StatusCode::OK.into_response();
        }

        persistence.persist_blob(&path_hash, &body);
        StatusCode::CREATED.into_response()
    })
    .await
    .unwrap_or_else(|resp| resp)
}

/// `None` = authorized (or anonymous access, when no token is configured).
/// `Some(response)` = the 401 to return immediately.
fn check_auth(headers: &HeaderMap, cfg: &BlobHttpConfig) -> Option<Response> {
    let expected = cfg.auth_token.as_ref()?;
    let provided_bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    match provided_bearer {
        Some(token) if constant_time_eq(token.as_bytes(), expected.as_bytes()) => None,
        _ => Some(unauthorized_response()),
    }
}

/// Constant-time-ish byte comparison: always walks `max(a.len(), b.len())`
/// bytes and folds every difference (including the length mismatch itself)
/// into one accumulator, rather than returning early on the first length or
/// byte mismatch. Good enough for a static bearer token (no `subtle`
/// dependency in this crate); not a substitute for a real crypto-grade
/// constant-time comparison if this ever guards something higher-value.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = (a.len() ^ b.len()) as u8;
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= x ^ y;
    }
    diff == 0
}

fn non_canonical_response() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error": "non-canonical hash"})),
    )
        .into_response()
}

fn unauthorized_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": "unauthorized"})),
    )
        .into_response()
}

fn not_found_response() -> Response {
    (StatusCode::NOT_FOUND, Json(json!({"error": "not found"}))).into_response()
}

fn unsupported_media_type_response() -> Response {
    (
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        Json(json!({"error": "Content-Encoding not supported on PUT"})),
    )
        .into_response()
}

/// S4.2 — 400 with a caller-supplied message, for the `/url` endpoint's
/// `op`/`size` validation errors (`non_canonical_response`'s message is
/// fixed by the frozen contract; these two aren't).
fn bad_request_response(message: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": message}))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso8601_epoch_zero() {
        assert_eq!(format_iso8601_utc(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn iso8601_known_timestamp() {
        // Cross-checked against `date -u -d @<secs>`.
        let secs = 1_784_118_896u64;
        assert_eq!(format_iso8601_utc(secs), "2026-07-15T12:34:56Z");
    }

    #[test]
    fn iso8601_end_of_year_boundary() {
        // 2025-12-31T23:59:59Z -> 2026-01-01T00:00:00Z one second later.
        assert_eq!(format_iso8601_utc(1_767_225_599), "2025-12-31T23:59:59Z");
        assert_eq!(format_iso8601_utc(1_767_225_600), "2026-01-01T00:00:00Z");
    }

    #[test]
    fn iso8601_leap_day() {
        // 2024-02-29T00:00:00Z (2024 is a leap year).
        assert_eq!(format_iso8601_utc(1_709_164_800), "2024-02-29T00:00:00Z");
    }

    // ─── slice 3.4: q=0 refusal (unit-level, below the HTTP-router tests in
    // tests/blob_http_surface.rs) ───────────────────────────────────────────

    #[test]
    fn is_qvalue_zero_recognizes_every_all_zero_spelling() {
        for z in ["0", "0.", "0.0", "0.00", "0.000"] {
            assert!(is_qvalue_zero(z), "{z} must be recognized as q=0");
        }
    }

    #[test]
    fn is_qvalue_zero_rejects_nonzero_and_malformed() {
        for nz in ["1", "0.5", "0.0001", "1.0", "", "not-a-number", "00"] {
            assert!(!is_qvalue_zero(nz), "{nz} must NOT be recognized as q=0");
        }
    }

    #[test]
    fn coding_accepts_zstd_bare_name() {
        assert!(coding_accepts_zstd("zstd"));
        assert!(coding_accepts_zstd(" zstd "));
        assert!(coding_accepts_zstd("ZSTD"));
    }

    #[test]
    fn coding_accepts_zstd_refuses_on_q0() {
        assert!(!coding_accepts_zstd("zstd;q=0"));
        assert!(!coding_accepts_zstd("zstd ; q=0.000"));
        assert!(!coding_accepts_zstd("ZSTD;Q=0"));
    }

    #[test]
    fn coding_accepts_zstd_keeps_nonzero_q() {
        assert!(coding_accepts_zstd("zstd;q=0.5"));
        assert!(coding_accepts_zstd("zstd;q=1"));
    }

    #[test]
    fn coding_accepts_zstd_rejects_other_codings() {
        assert!(!coding_accepts_zstd("gzip"));
        assert!(!coding_accepts_zstd("gzip;q=0"));
        assert!(!coding_accepts_zstd(""));
    }

    #[test]
    fn accepts_zstd_honors_q0_across_a_list() {
        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT_ENCODING, "zstd;q=0, gzip".parse().unwrap());
        assert!(!accepts_zstd(&headers));

        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT_ENCODING, "gzip;q=0, zstd".parse().unwrap());
        assert!(accepts_zstd(&headers));

        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT_ENCODING, "zstd".parse().unwrap());
        assert!(accepts_zstd(&headers));

        assert!(!accepts_zstd(&HeaderMap::new()));
    }
}
