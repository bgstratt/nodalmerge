//! S4.2 — relay-endpoint behavior against a backend that never hydrates
//! blob bytes into the server process (the shape `S3BlobStore` has:
//! `get_blob` always `None`, `has_blob` a real existence check).
//!
//! This is the "server never proxies bytes" design point from
//! `docs/BLOB_STORAGE_LAYOUT.md`'s CAS distribution plan: `HEAD
//! /blobs/{hash}` must still answer truthfully via `has_blob` (a cheap
//! existence check — a bucket `HEAD` for a real S3 backend), while `GET
//! /blobs/{hash}` — the escape hatch, not the fast path — reports 404
//! because there are no bytes to proxy. Callers needing the actual bytes are
//! expected to use `GET /blobs/{hash}/url` instead once a real presign
//! backend is wired in (S4.1/S4.2's URL-resolution endpoints).
//!
//! Exercised here with an in-process fake rather than real S3/MinIO (that
//! coverage is `server/s3-blobs/tests/minio_round_trip.rs` plus the
//! MinIO-gated `blob_url_resolution_minio.rs`) so this specific relay
//! contract is covered by a fast, always-on test.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use ed25519_dalek::SigningKey;
use nodalmerge_core::Hash;
use nodalmerge_server::blob_http::{self, BlobHttpConfig};
use nodalmerge_server::room::Rooms;
use nodalmerge_server::store::{BlobPersistence, Composite, NoPersistence, SharedPersistence};
use tower::ServiceExt;

/// Minimal stand-in for `S3BlobStore`'s shape: `get_blob` never hydrates
/// bytes; `has_blob` is a real (in this fake, in-memory) existence check.
#[derive(Debug, Default)]
struct NonHydratingBackend {
    present: Mutex<HashSet<Hash>>,
}

impl BlobPersistence for NonHydratingBackend {
    fn get_blob(&self, _hash: &Hash) -> Option<Vec<u8>> {
        None
    }
    fn has_blob(&self, hash: &Hash) -> bool {
        self.present.lock().unwrap().contains(hash)
    }
    fn persist_blob(&self, hash: &Hash, _bytes: &[u8]) {
        self.present.lock().unwrap().insert(*hash);
    }
}

fn router(persistence: SharedPersistence) -> Router {
    let rooms = Rooms::new(SigningKey::from_bytes(&[0x81; 32]), persistence, 512, 0, 0);
    blob_http::blob_routes(BlobHttpConfig::default()).with_state(rooms)
}

#[tokio::test]
async fn head_reports_present_via_has_blob_even_though_get_blob_never_hydrates() {
    let backend = NonHydratingBackend::default();
    let hash = Hash::of(b"never actually hydrated");
    backend.present.lock().unwrap().insert(hash);
    let hash_hex = hash.to_hex();

    // `SharedPersistence = Arc<dyn ServerPersistence>` needs both halves;
    // `NonHydratingBackend` only stands in for the blob half, so pair it
    // with `NoPersistence`'s node half via `Composite` (nodes are unused by
    // every request this test sends).
    let persistence: SharedPersistence = Arc::new(Composite::new(NoPersistence, backend));
    let app = router(persistence);

    let head_req = Request::builder()
        .method(Method::HEAD)
        .uri(format!("/blobs/{hash_hex}"))
        .body(Body::empty())
        .unwrap();
    let head_resp = app.clone().oneshot(head_req).await.unwrap();
    assert_eq!(
        head_resp.status(),
        StatusCode::OK,
        "HEAD must answer via has_blob, independent of get_blob"
    );
    let head_body = axum::body::to_bytes(head_resp.into_body(), usize::MAX).await.unwrap();
    assert!(head_body.is_empty(), "HEAD must never carry a body");

    let get_req = Request::builder()
        .method(Method::GET)
        .uri(format!("/blobs/{hash_hex}"))
        .body(Body::empty())
        .unwrap();
    let get_resp = app.oneshot(get_req).await.unwrap();
    assert_eq!(
        get_resp.status(),
        StatusCode::NOT_FOUND,
        "GET (the escape hatch, not the fast path) must 404 when get_blob returns None, \
         even though the blob genuinely exists per has_blob"
    );
}

#[tokio::test]
async fn head_reports_missing_when_not_present() {
    let backend = NonHydratingBackend::default();
    let hash_hex = Hash::of(b"never uploaded").to_hex();
    let persistence: SharedPersistence = Arc::new(Composite::new(NoPersistence, backend));
    let app = router(persistence);

    let req = Request::builder()
        .method(Method::HEAD)
        .uri(format!("/blobs/{hash_hex}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
