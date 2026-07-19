//! Golden-vector conformance test for the blob HTTP origin surface
//! (`GET`/`HEAD`/`PUT /blobs/{hash}`, S2.1b). Drives every vector in
//! `engine/commands/blob-http-surface-vectors.v1.json` against the real
//! `blob_http::blob_routes` router (via `tower::ServiceExt::oneshot`,
//! backed by a `DirPersistence` store in a tempdir) so this test and the
//! .NET mirror (`hosts/dotnet/tests/NodalMerge.DotNetHost.Tests/
//! BlobHttpSurfaceTests.cs`) stay in lockstep with
//! `docs/BLOB_HTTP_SURFACE.md`.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request};
use axum::Router;
use ed25519_dalek::SigningKey;
use nodalmerge_core::Hash;
use nodalmerge_server::blob_http::{self, BlobHttpConfig};
use nodalmerge_server::room::Rooms;
use nodalmerge_server::store::{BlobCompressionConfig, DirPersistence, SharedPersistence};
use serde::Deserialize;
use tower::ServiceExt;

const VECTORS_JSON: &str = include_str!("../../../engine/commands/blob-http-surface-vectors.v1.json");

#[derive(Debug, Deserialize)]
struct VectorsFile {
    seed_blob: SeedBlob,
    max_blob_bytes_for_tests: usize,
    auth_token_for_tests: String,
    vectors: Vec<Vector>,
}

#[derive(Debug, Deserialize)]
struct SeedBlob {
    seed_content: String,
}

#[derive(Debug, Deserialize)]
struct Vector {
    id: String,
    method: String,
    hash: String,
    #[serde(default)]
    body_kind: Option<String>,
    #[serde(default)]
    body_content: Option<String>,
    auth: String,
    server_token_configured: bool,
    expect_status: u16,
    #[serde(default)]
    expect_headers: Option<std::collections::HashMap<String, String>>,
    #[serde(default)]
    expect_body: Option<String>,
    #[serde(default)]
    expect_stored: Option<bool>,
}

fn tmpdir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!("nodalmerge-blob-http-{tag}-{nanos}"));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Compute the request body bytes for a vector, given the seeded blob's
/// bytes and the configured oversize threshold.
fn body_bytes_for(vector: &Vector, seed_bytes: &[u8], max_bytes: usize) -> Vec<u8> {
    match vector.body_kind.as_deref() {
        None => Vec::new(),
        Some("seeded") => seed_bytes.to_vec(),
        Some("new-content") | Some("wrong-bytes") => vector
            .body_content
            .clone()
            .expect("body_content required for this body_kind")
            .into_bytes(),
        Some("oversize") => vec![0u8; max_bytes + 1],
        Some(other) => panic!("vector `{}`: unknown body_kind `{other}`", vector.id),
    }
}

/// Resolve the `{hash}` path segment for a vector: `"seed"` substitutes the
/// seeded blob's hash, `"computed-from-body"` hashes the request body about
/// to be sent, and anything else (including the deliberately malformed
/// non-canonical fixtures) is used verbatim.
fn hash_segment_for(vector: &Vector, seed_hash_hex: &str, body: &[u8]) -> String {
    match vector.hash.as_str() {
        "seed" => seed_hash_hex.to_string(),
        "computed-from-body" => Hash::of(body).to_hex(),
        literal => literal.to_string(),
    }
}

fn auth_header_for(vector: &Vector, correct_token: &str) -> Option<String> {
    match vector.auth.as_str() {
        "none" => None,
        "bearer-correct" => Some(format!("Bearer {correct_token}")),
        "bearer-wrong" => Some("Bearer this-is-not-the-configured-token".to_string()),
        other => panic!("vector `{}`: unknown auth kind `{other}`", vector.id),
    }
}

#[tokio::test]
async fn blob_http_surface_vectors_conform() {
    let file: VectorsFile = serde_json::from_str(VECTORS_JSON)
        .expect("engine/commands/blob-http-surface-vectors.v1.json must parse");

    let dir = tmpdir("surface");
    let persistence: SharedPersistence = Arc::new(DirPersistence::open(&dir).unwrap());
    let rooms = Rooms::new(
        SigningKey::from_bytes(&[0x51u8; 32]),
        Arc::clone(&persistence),
        512,
        0,
        0,
    );

    // Seed the store once — vectors marked `seeded: true` expect this blob
    // to already be present.
    let seed_bytes = file.seed_blob.seed_content.as_bytes().to_vec();
    let seed_hash = Hash::of(&seed_bytes);
    let seed_hash_hex = seed_hash.to_hex();
    persistence.persist_blob(&seed_hash, &seed_bytes).unwrap();

    // Two router variants sharing the same underlying store: one anonymous,
    // one gated behind `auth_token_for_tests`. Vectors pick between them via
    // `server_token_configured`.
    let no_auth_router: Router = blob_http::blob_routes(BlobHttpConfig {
        auth_token: None,
        max_blob_bytes: file.max_blob_bytes_for_tests,
        gc_inventory: None,
    })
    .with_state(rooms.clone());
    let auth_router: Router = blob_http::blob_routes(BlobHttpConfig {
        auth_token: Some(file.auth_token_for_tests.clone()),
        max_blob_bytes: file.max_blob_bytes_for_tests,
        gc_inventory: None,
    })
    .with_state(rooms.clone());

    for vector in &file.vectors {
        let body = body_bytes_for(vector, &seed_bytes, file.max_blob_bytes_for_tests);
        let hash_segment = hash_segment_for(vector, &seed_hash_hex, &body);
        let method = Method::from_bytes(vector.method.as_bytes())
            .unwrap_or_else(|_| panic!("vector `{}`: invalid method `{}`", vector.id, vector.method));

        let mut builder = Request::builder()
            .method(method)
            .uri(format!("/blobs/{hash_segment}"));
        if let Some(auth) = auth_header_for(vector, &file.auth_token_for_tests) {
            builder = builder.header(header::AUTHORIZATION, auth);
        }
        let req = builder
            .body(Body::from(body.clone()))
            .unwrap_or_else(|e| panic!("vector `{}`: request build failed: {e}", vector.id));

        let router = if vector.server_token_configured {
            auth_router.clone()
        } else {
            no_auth_router.clone()
        };
        let resp = router
            .oneshot(req)
            .await
            .unwrap_or_else(|e| panic!("vector `{}`: request failed: {e:?}", vector.id));

        assert_eq!(
            resp.status().as_u16(),
            vector.expect_status,
            "vector `{}`: unexpected status (got {}, want {})",
            vector.id,
            resp.status(),
            vector.expect_status
        );

        if let Some(expect_headers) = &vector.expect_headers {
            for (name, expected_raw) in expect_headers {
                let expected = expected_raw.replace("<seed-hash>", &seed_hash_hex);
                let actual = resp
                    .headers()
                    .get(name.as_str())
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_else(|| panic!("vector `{}`: missing header `{name}`", vector.id));
                assert_eq!(
                    actual, expected,
                    "vector `{}`: header `{name}` mismatch",
                    vector.id
                );
            }
        }

        let expect_body = vector.expect_body.clone();
        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|e| panic!("vector `{}`: reading response body failed: {e}", vector.id));
        match expect_body.as_deref() {
            Some("seed-bytes") => assert_eq!(
                body_bytes.as_ref(),
                seed_bytes.as_slice(),
                "vector `{}`: body mismatch",
                vector.id
            ),
            Some("empty") => assert!(
                body_bytes.is_empty(),
                "vector `{}`: expected empty body, got {} bytes",
                vector.id,
                body_bytes.len()
            ),
            Some(other) => panic!("vector `{}`: unknown expect_body `{other}`", vector.id),
            None => {}
        }

        if let Some(expect_stored) = vector.expect_stored {
            let on_disk_path = dir.join("blobs").join("blake3").join(&hash_segment);
            assert_eq!(
                on_disk_path.is_file(),
                expect_stored,
                "vector `{}`: expect_stored={expect_stored} but on-disk presence was {}",
                vector.id,
                on_disk_path.is_file()
            );
            if expect_stored {
                // Layout interchangeability: the HTTP PUT's stored bytes
                // must be byte-identical to the body that was sent, at
                // exactly the path the shared CAS layout dictates
                // (docs/BLOB_STORAGE_LAYOUT.md).
                let on_disk = std::fs::read(&on_disk_path).unwrap_or_else(|e| {
                    panic!("vector `{}`: reading stored blob failed: {e}", vector.id)
                });
                assert_eq!(
                    on_disk, body,
                    "vector `{}`: stored bytes differ from the PUT body",
                    vector.id
                );
            }
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
}

// ─── S3.1b: content-encoding negotiation (docs/BLOB_HTTP_SURFACE.md ─────────
// "Content encoding (reserved v1.1 — Phase 3)"). Hand-written rather than
// vector-driven: the frozen vectors file (engine/commands/) has no
// Accept-Encoding-aware vectors yet, and none of the existing 15 vectors
// send that header, so they must stay green unchanged (asserted above).
//
// Slice 3.4 (blob-cas-remediation.md): the `q=0`-refusal tests below are
// hand-written for the same reason — the vector schema has no request-header
// slot at all (no field carries a request Accept-Encoding/Content-Encoding),
// so extending it would mean teaching both this file's and the .NET
// consumer's struct a field neither the doc nor the fixture reads elsewhere.
// Paired native tests here + BlobHttpEncodingNegotiationTests.cs instead.

fn compressible_payload() -> Vec<u8> {
    b"the quick brown fox jumps over the lazy dog. ".repeat(500)
}

#[tokio::test]
async fn get_with_accept_encoding_zstd_serves_stored_zstd_bytes() {
    let dir = tmpdir("content-encoding-get");
    let persistence: SharedPersistence = Arc::new(
        DirPersistence::open_with_compression(
            &dir,
            BlobCompressionConfig {
                enabled: true,
                level: 3,
                min_bytes: 16,
            },
        )
        .unwrap(),
    );
    let rooms = Rooms::new(
        SigningKey::from_bytes(&[0x61u8; 32]),
        Arc::clone(&persistence),
        512,
        0,
        0,
    );
    let router: Router = blob_http::blob_routes(BlobHttpConfig::default()).with_state(rooms.clone());

    let payload = compressible_payload();
    let hash = Hash::of(&payload);
    let hash_hex = hash.to_hex();
    persistence.persist_blob(&hash, &payload).unwrap();
    // Sanity: the compression-on store really did write the .zst form.
    let encoded_path = dir.join("blobs").join("blake3").join(format!("{hash_hex}.zst"));
    assert!(encoded_path.is_file(), "test setup expected a .zst write");

    // GET with Accept-Encoding: zstd -> Content-Encoding: zstd, and the
    // body zstd-decodes back to the original bytes.
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/blobs/{hash_hex}"))
        .header(header::ACCEPT_ENCODING, "zstd")
        .body(Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CONTENT_ENCODING).and_then(|v| v.to_str().ok()),
        Some("zstd")
    );
    assert_eq!(
        resp.headers().get(header::ETAG).and_then(|v| v.to_str().ok()),
        Some(format!("\"{hash_hex}\"").as_str())
    );
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let decoded = zstd::stream::decode_all(&body[..]).expect("response body must be a valid zstd frame");
    assert_eq!(decoded, payload);

    // GET without Accept-Encoding -> identity bytes, no Content-Encoding.
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/blobs/{hash_hex}"))
        .body(Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    assert!(
        resp.headers().get(header::CONTENT_ENCODING).is_none(),
        "identity response must not carry Content-Encoding"
    );
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body.as_ref(), payload.as_slice());

    let _ = std::fs::remove_dir_all(&dir);
}

/// Shared setup for the `q=0` negotiation tests below: a zstd-compression-on
/// store seeded with a real `.zst` sibling — same shape as
/// `get_with_accept_encoding_zstd_serves_stored_zstd_bytes`'s setup, factored
/// out because five tests need it instead of one.
fn seed_zstd_router(tag: &str) -> (Router, PathBuf, Vec<u8>, String) {
    let dir = tmpdir(tag);
    let persistence: SharedPersistence = Arc::new(
        DirPersistence::open_with_compression(
            &dir,
            BlobCompressionConfig {
                enabled: true,
                level: 3,
                min_bytes: 16,
            },
        )
        .unwrap(),
    );
    let rooms = Rooms::new(
        SigningKey::from_bytes(&[0x63u8; 32]),
        Arc::clone(&persistence),
        512,
        0,
        0,
    );
    let router: Router = blob_http::blob_routes(BlobHttpConfig::default()).with_state(rooms.clone());

    let payload = compressible_payload();
    let hash = Hash::of(&payload);
    let hash_hex = hash.to_hex();
    persistence.persist_blob(&hash, &payload).unwrap();
    // Sanity: the compression-on store really did write the .zst form.
    let encoded_path = dir.join("blobs").join("blake3").join(format!("{hash_hex}.zst"));
    assert!(encoded_path.is_file(), "test setup expected a .zst write");

    (router, dir, payload, hash_hex)
}

async fn assert_identity_response(router: &Router, hash_hex: &str, accept_encoding: &str, payload: &[u8]) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/blobs/{hash_hex}"))
        .header(header::ACCEPT_ENCODING, accept_encoding)
        .body(Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    assert!(
        resp.headers().get(header::CONTENT_ENCODING).is_none(),
        "Accept-Encoding: {accept_encoding} must be treated as a zstd refusal (identity response), got Content-Encoding: {:?}",
        resp.headers().get(header::CONTENT_ENCODING)
    );
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body.as_ref(), payload);
}

async fn assert_zstd_response(router: &Router, hash_hex: &str, accept_encoding: &str, payload: &[u8]) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/blobs/{hash_hex}"))
        .header(header::ACCEPT_ENCODING, accept_encoding)
        .body(Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CONTENT_ENCODING).and_then(|v| v.to_str().ok()),
        Some("zstd"),
        "Accept-Encoding: {accept_encoding} must still be served the stored zstd form"
    );
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let decoded = zstd::stream::decode_all(&body[..]).expect("response body must be a valid zstd frame");
    assert_eq!(decoded, payload);
}

/// RFC 9110 `q=0` means "do not send this coding" — bare `zstd;q=0`.
#[tokio::test]
async fn get_with_accept_encoding_zstd_q0_serves_identity_bytes() {
    let (router, dir, payload, hash_hex) = seed_zstd_router("content-encoding-q0-bare");
    assert_identity_response(&router, &hash_hex, "zstd;q=0", &payload).await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// Every qvalue-zero spelling RFC 9110 allows (up to three fractional
/// digits), plus whitespace around the `;`/`=` the RFC also allows, must
/// refuse zstd identically.
#[tokio::test]
async fn get_with_accept_encoding_zstd_q0_syntax_variants_all_refuse() {
    let (router, dir, payload, hash_hex) = seed_zstd_router("content-encoding-q0-variants");
    for variant in ["zstd;q=0", "zstd;q=0.0", "zstd;q=0.00", "zstd;q=0.000", "zstd ; q=0", "ZSTD;Q=0"] {
        assert_identity_response(&router, &hash_hex, variant, &payload).await;
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// `zstd;q=0, gzip` — zstd is refused by its own per-coding `q=0`; the
/// absence of a stored gzip form means identity either way, but this
/// specifically exercises that the comma-separated list is still walked and
/// zstd's parameter doesn't leak onto the next token.
#[tokio::test]
async fn get_with_accept_encoding_zstd_q0_then_gzip_serves_identity_bytes() {
    let (router, dir, payload, hash_hex) = seed_zstd_router("content-encoding-q0-then-gzip");
    assert_identity_response(&router, &hash_hex, "zstd;q=0, gzip", &payload).await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// `gzip;q=0, zstd` — the opposite order: gzip's `q=0` must NOT leak onto
/// zstd's (unparameterized, so accepted) token. Parameters are per-coding.
#[tokio::test]
async fn get_with_accept_encoding_gzip_q0_then_zstd_still_serves_zstd_bytes() {
    let (router, dir, payload, hash_hex) = seed_zstd_router("content-encoding-gzip-q0-then-zstd");
    assert_zstd_response(&router, &hash_hex, "gzip;q=0, zstd", &payload).await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// A nonzero (or fractional non-zero) `q` is NOT a refusal — this slice only
/// honors `q=0`, it does not build qvalue-preference ordering.
#[tokio::test]
async fn get_with_accept_encoding_zstd_nonzero_q_still_serves_zstd_bytes() {
    let (router, dir, payload, hash_hex) = seed_zstd_router("content-encoding-q-nonzero");
    assert_zstd_response(&router, &hash_hex, "zstd;q=0.5", &payload).await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn put_with_content_encoding_header_is_rejected_415() {
    let dir = tmpdir("content-encoding-put-415");
    let persistence: SharedPersistence = Arc::new(DirPersistence::open(&dir).unwrap());
    let rooms = Rooms::new(
        SigningKey::from_bytes(&[0x62u8; 32]),
        Arc::clone(&persistence),
        512,
        0,
        0,
    );
    let router: Router = blob_http::blob_routes(BlobHttpConfig::default()).with_state(rooms.clone());

    let body = b"whatever bytes, never persisted".to_vec();
    let hash_hex = Hash::of(&body).to_hex();

    let req = Request::builder()
        .method(Method::PUT)
        .uri(format!("/blobs/{hash_hex}"))
        .header(header::CONTENT_ENCODING, "zstd")
        .body(Body::from(body))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let on_disk = dir.join("blobs").join("blake3").join(&hash_hex);
    assert!(!on_disk.is_file(), "rejected PUT must never persist bytes");

    let _ = std::fs::remove_dir_all(&dir);
}
