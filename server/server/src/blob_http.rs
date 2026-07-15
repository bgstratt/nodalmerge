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
//! HEAD is served by axum's built-in GET→HEAD behavior: `MethodRouter`
//! routes a `HEAD` request to the `get(...)` handler and strips the
//! response body afterward (see `axum::routing::method_routing`), so the
//! status code and headers are identical to `GET` for free. No separate
//! `.head(...)` handler is registered.
//!
//! The 413 (payload too large) response for oversize `PUT` bodies is not
//! hand-rolled: `DefaultBodyLimit::max(cfg.max_blob_bytes)` is layered onto
//! these routes only (via `route_layer`, so `/ws/:room_id` is unaffected),
//! and axum's `Bytes` extractor itself returns a `413 Payload Too Large`
//! rejection when the (possibly chunked, possibly `Content-Length`-less)
//! body stream exceeds that limit while buffering.

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use nodalmerge_core::Hash;
use serde_json::json;

use crate::room::Rooms;
use crate::store::{hash_from_hex, is_canonical_blob_name};

/// Runtime configuration for the blob HTTP origin routes.
#[derive(Debug, Clone)]
pub struct BlobHttpConfig {
    /// Static bearer token. `None` = anonymous (matches the rest of the
    /// current HTTP surface). `Some(token)` requires every blob request to
    /// carry `Authorization: Bearer <token>`.
    pub auth_token: Option<String>,
    /// Max accepted `PUT` body size, in bytes. Enforced via
    /// `DefaultBodyLimit` on the whole body stream (not just
    /// `Content-Length`), so chunked bodies are capped while buffering too.
    pub max_blob_bytes: usize,
}

impl Default for BlobHttpConfig {
    fn default() -> Self {
        Self {
            auth_token: None,
            max_blob_bytes: 64 * 1024 * 1024, // 64 MiB
        }
    }
}

/// Build the `/blobs/:hash` routes (`GET`, `HEAD` via axum's built-in
/// GET→HEAD, `PUT`), scoped to `Router<Rooms>` so callers can `.merge()`
/// this into their existing room router. `cfg` is captured by the route
/// closures — no `Extension` layer needed.
pub fn blob_routes(cfg: BlobHttpConfig) -> Router<Rooms> {
    let max_bytes = cfg.max_blob_bytes;
    let get_cfg = cfg.clone();
    let put_cfg = cfg;

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
        // Scoped to just these routes: `/ws/:room_id` keeps axum's default
        // (2 MiB) body limit untouched.
        .route_layer(DefaultBodyLimit::max(max_bytes))
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
    match rooms.persistence.get_blob(&hash) {
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
    let path_hash = match hash_from_hex(&hash_hex) {
        Some(h) => h,
        None => return non_canonical_response(), // unreachable: canonical checked above
    };

    // d. (Oversize bodies never reach here — `DefaultBodyLimit` + the
    // `Bytes` extractor reject them with 413 before the handler runs.)
    let computed = Hash::of(&body);
    if computed != path_hash {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "hash mismatch"})),
        )
            .into_response();
    }

    if rooms.persistence.has_blob(&path_hash) {
        return StatusCode::OK.into_response();
    }

    rooms.persistence.persist_blob(&path_hash, &body);
    StatusCode::CREATED.into_response()
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
