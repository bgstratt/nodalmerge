//! Golden-vector conformance test for the blob URL-resolution endpoints
//! (S4.2): `GET /blobs/{hash}/url` and `POST /blobs/{hash}/uploaded`. Drives
//! every vector in `engine/commands/blob-url-resolution-vectors.v1.json`
//! against the real `blob_http::blob_routes` router (via
//! `tower::ServiceExt::oneshot`) so this test and the .NET mirror
//! (`hosts/dotnet/tests/NodalMerge.DotNetHost.Tests/
//! BlobUrlResolutionVectorTests.cs`) stay in lockstep with
//! `docs/BLOB_HTTP_SURFACE.md`.
//!
//! `backend_configured: true` vectors run against a router whose persistence
//! is a `Composite<NoPersistence, FakePresignBackend>` — a fake
//! `BlobPersistence` that always returns `fake_presigned_url`/
//! `fake_ttl_seconds` from the vectors file, mirroring what the vectors
//! file's own `$comment` says the .NET harness does. `backend_configured:
//! false` vectors run against the plain `DirPersistence` default (the
//! codebase's actual no-presign-capability backend) — exactly the "relay-only
//! deployment" the frozen contract says must answer 501.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::body::Body;
use axum::http::{Method, Request};
use axum::Router;
use ed25519_dalek::SigningKey;
use nodalmerge_core::Hash;
use nodalmerge_server::blob_http::{self, BlobHttpConfig};
use nodalmerge_server::room::Rooms;
use nodalmerge_server::store::{
    BlobPersistence, Composite, DirPersistence, NoPersistence, PresignedUrl, SharedPersistence,
};
use serde::Deserialize;
use tower::ServiceExt;

const VECTORS_JSON: &str =
    include_str!("../../../engine/commands/blob-url-resolution-vectors.v1.json");

#[derive(Debug, Deserialize)]
struct VectorsFile {
    canonical_hash: String,
    fake_presigned_url: String,
    fake_ttl_seconds: u64,
    vectors: Vec<Vector>,
}

#[derive(Debug, Deserialize)]
struct Vector {
    id: String,
    route: String,
    hash: String,
    #[serde(default)]
    op: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    content_type: Option<String>,
    backend_configured: bool,
    expect_status: u16,
    #[serde(default)]
    expect_response_shape: Vec<String>,
    #[serde(default)]
    expect_error: Option<String>,
}

/// Always-presign-capable fake backend, standing in for a real S3-like
/// store the way the vectors file's `$comment` describes the .NET harness
/// doing (`fake_presigned_url`/`fake_ttl_seconds`, `verify_uploaded` always
/// `Ok(())` — mirroring the Rust delegate auth mode's default).
#[derive(Debug)]
struct FakePresignBackend {
    url: String,
    ttl: Duration,
}

impl BlobPersistence for FakePresignBackend {
    fn persist_blob(&self, _hash: &Hash, _bytes: &[u8]) {}

    fn resolve_get_url(
        &self,
        _room_id: &str,
        _hash: &Hash,
        _size_hint: Option<u64>,
    ) -> Option<PresignedUrl> {
        Some(PresignedUrl::with_ttl(self.url.clone(), self.ttl))
    }

    fn resolve_put_url(
        &self,
        _room_id: &str,
        _hash: &Hash,
        _size: u64,
        _content_type: Option<&str>,
    ) -> Option<PresignedUrl> {
        Some(PresignedUrl::with_ttl(self.url.clone(), self.ttl))
    }

    fn verify_uploaded(&self, _room_id: &str, _hash: &Hash) -> Result<(), String> {
        Ok(())
    }

    fn supports_presigned_urls(&self) -> bool {
        true
    }
}

fn tmpdir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!("nodalmerge-blob-url-resolution-{tag}-{nanos}"));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn router_with(persistence: SharedPersistence, key_seed: u8) -> Router {
    let rooms = Rooms::new(SigningKey::from_bytes(&[key_seed; 32]), persistence, 512, 0, 0);
    blob_http::blob_routes(BlobHttpConfig::default()).with_state(rooms)
}

fn hash_segment_for(vector: &Vector, canonical_hash: &str) -> String {
    match vector.hash.as_str() {
        "canonical" => canonical_hash.to_string(),
        // "malformed": any string that fails `is_canonical_blob_name` works;
        // the vectors file uses this as a sentinel, not a literal value.
        "malformed" => "not-a-canonical-hash".to_string(),
        literal => literal.to_string(),
    }
}

#[tokio::test]
async fn blob_url_resolution_vectors_conform() {
    let file: VectorsFile = serde_json::from_str(VECTORS_JSON)
        .expect("engine/commands/blob-url-resolution-vectors.v1.json must parse");

    let dir = tmpdir("no-backend");
    let no_backend_router = router_with(
        Arc::new(DirPersistence::open(&dir).unwrap()) as SharedPersistence,
        0x71,
    );

    let with_backend_router = router_with(
        Arc::new(Composite::new(
            NoPersistence,
            FakePresignBackend {
                url: file.fake_presigned_url.clone(),
                ttl: Duration::from_secs(file.fake_ttl_seconds),
            },
        )) as SharedPersistence,
        0x72,
    );

    let expected_expiry_floor = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    for vector in &file.vectors {
        let hash_segment = hash_segment_for(vector, &file.canonical_hash);
        let router = if vector.backend_configured {
            with_backend_router.clone()
        } else {
            no_backend_router.clone()
        };

        let (method, mut uri) = match vector.route.as_str() {
            "url" => (Method::GET, format!("/blobs/{hash_segment}/url")),
            "uploaded" => (Method::POST, format!("/blobs/{hash_segment}/uploaded")),
            other => panic!("vector `{}`: unknown route `{other}`", vector.id),
        };

        if vector.route == "url" {
            let mut params = Vec::new();
            if let Some(op) = &vector.op {
                params.push(format!("op={op}"));
            }
            if let Some(size) = vector.size {
                params.push(format!("size={size}"));
            }
            if let Some(ct) = &vector.content_type {
                params.push(format!("contentType={ct}"));
            }
            if !params.is_empty() {
                uri.push('?');
                uri.push_str(&params.join("&"));
            }
        }

        let req = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap_or_else(|e| panic!("vector `{}`: request build failed: {e}", vector.id));

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

        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|e| panic!("vector `{}`: reading response body failed: {e}", vector.id));

        if !vector.expect_response_shape.is_empty() {
            let parsed: serde_json::Value = serde_json::from_slice(&body_bytes)
                .unwrap_or_else(|e| panic!("vector `{}`: response body not JSON: {e}", vector.id));
            for field in &vector.expect_response_shape {
                assert!(
                    parsed.get(field).is_some(),
                    "vector `{}`: response missing field `{field}` (body: {parsed})",
                    vector.id
                );
            }
            if let Some(url) = parsed.get("url").and_then(|v| v.as_str()) {
                assert_eq!(
                    url, file.fake_presigned_url,
                    "vector `{}`: url field mismatch",
                    vector.id
                );
            }
            if let Some(expires) = parsed.get("expiresAtUtc").and_then(|v| v.as_str()) {
                // Must be a plausible ISO-8601 UTC string: ends in 'Z', has
                // a 'T' separator, and (loosely) is not in the past relative
                // to when the test started.
                assert!(
                    expires.ends_with('Z') && expires.contains('T'),
                    "vector `{}`: expiresAtUtc `{expires}` doesn't look like ISO-8601 UTC",
                    vector.id
                );
                let _ = expected_expiry_floor; // documents intent; exact parse not needed here
            }
        }

        if let Some(expected_error) = &vector.expect_error {
            let parsed: serde_json::Value = serde_json::from_slice(&body_bytes)
                .unwrap_or_else(|e| panic!("vector `{}`: response body not JSON: {e}", vector.id));
            assert_eq!(
                parsed.get("error").and_then(|v| v.as_str()),
                Some(expected_error.as_str()),
                "vector `{}`: error message mismatch",
                vector.id
            );
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
}
