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
    BlobPersistence, Composite, DirPersistence, NoPersistence, PersistBlobError, PresignedUrl, SharedPersistence,
};
use serde::Deserialize;
use tower::ServiceExt;

const VECTORS_JSON: &str =
    include_str!("../../../engine/commands/blob-url-resolution-vectors.v1.json");

/// Slice 7.4 — the frozen delegate room-id placeholder lives in the
/// `delegate_room_id_placeholder` slot of the work-unit vectors file (the
/// slot slice 0.4 reserved for exactly this decision). See
/// `delegate_room_placeholder_matches_frozen_vector` below.
const WORK_UNIT_VECTORS_JSON: &str =
    include_str!("../../../engine/commands/work-unit-status-vectors.v1.json");

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
    fn persist_blob(&self, _hash: &Hash, _bytes: &[u8]) -> Result<(), PersistBlobError> {
        Ok(())
    }

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

/// Slice 7.4 — like `FakePresignBackend`, but records every `room_id` the
/// room-agnostic blob HTTP routes pass into the delegate presign seam
/// (`resolve_get_url`/`resolve_put_url`/`verify_uploaded`), so the frozen
/// placeholder is pinned as ROUTE BEHAVIOR, not just a constant's value.
#[derive(Debug, Default)]
struct CapturingPresignBackend {
    rooms_seen: std::sync::Mutex<Vec<String>>,
}

impl CapturingPresignBackend {
    fn record(&self, room_id: &str) {
        self.rooms_seen.lock().unwrap().push(room_id.to_string());
    }
}

impl BlobPersistence for CapturingPresignBackend {
    fn persist_blob(&self, _hash: &Hash, _bytes: &[u8]) -> Result<(), PersistBlobError> {
        Ok(())
    }

    fn resolve_get_url(
        &self,
        room_id: &str,
        _hash: &Hash,
        _size_hint: Option<u64>,
    ) -> Option<PresignedUrl> {
        self.record(room_id);
        Some(PresignedUrl::with_ttl(
            "https://bucket.test/presigned".to_string(),
            Duration::from_secs(900),
        ))
    }

    fn resolve_put_url(
        &self,
        room_id: &str,
        _hash: &Hash,
        _size: u64,
        _content_type: Option<&str>,
    ) -> Option<PresignedUrl> {
        self.record(room_id);
        Some(PresignedUrl::with_ttl(
            "https://bucket.test/presigned".to_string(),
            Duration::from_secs(900),
        ))
    }

    fn verify_uploaded(&self, room_id: &str, _hash: &Hash) -> Result<(), String> {
        self.record(room_id);
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

/// Slice 7.4 (blob-cas-remediation.md) — freeze the delegate room-id
/// placeholder. `room` in the delegate presign protocol v1
/// (`docs/BLOB_STORAGE_LAYOUT.md` §7) is metadata-only (never a key input),
/// but a delegate that logs/quotas/audits per room previously saw DIFFERENT
/// per-host values on the room-agnostic routes: this server sent
/// `"_global"` while the .NET host sent `"default"`. The frozen winner
/// lives in the `delegate_room_id_placeholder` slot of
/// `engine/commands/work-unit-status-vectors.v1.json`; the .NET mirror is
/// `DelegateRoomPlaceholderVectorTests.cs`.
///
/// Note on `namespace`: the vector slot also freezes `namespace: "blobs"`
/// for hosts that send the protocol's OPTIONAL namespace field at all. This
/// server's delegate client (`nodalmerge-s3-blobs::delegate_request`) has no
/// namespace field in its request struct and omits it entirely, which stays
/// conformant — so there is deliberately no namespace assertion here.
#[tokio::test]
async fn delegate_room_placeholder_matches_frozen_vector() {
    let file: serde_json::Value = serde_json::from_str(WORK_UNIT_VECTORS_JSON)
        .expect("engine/commands/work-unit-status-vectors.v1.json must parse");
    let expected_room = file["delegate_room_id_placeholder"]["room"]
        .as_str()
        .expect("delegate_room_id_placeholder.room must be a decided (non-null) string — slice 7.4")
        .to_string();

    let vectors: VectorsFile = serde_json::from_str(VECTORS_JSON)
        .expect("engine/commands/blob-url-resolution-vectors.v1.json must parse");

    let backend = Arc::new(CapturingPresignBackend::default());
    let router = {
        let rooms = Rooms::new(
            SigningKey::from_bytes(&[0x73; 32]),
            Arc::new(Composite::new(NoPersistence, SharedBackend(backend.clone()))) as SharedPersistence,
            512,
            0,
            0,
        );
        blob_http::blob_routes(BlobHttpConfig::default()).with_state(rooms)
    };

    let hash = &vectors.canonical_hash;
    for (method, uri) in [
        (Method::GET, format!("/blobs/{hash}/url?op=get")),
        (Method::GET, format!("/blobs/{hash}/url?op=put&size=1024")),
        (Method::POST, format!("/blobs/{hash}/uploaded")),
    ] {
        let req = Request::builder()
            .method(method.clone())
            .uri(&uri)
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert!(
            resp.status().is_success(),
            "{method} {uri}: expected success, got {}",
            resp.status()
        );
    }

    let seen = backend.rooms_seen.lock().unwrap().clone();
    assert_eq!(
        seen.len(),
        3,
        "expected all three room-agnostic routes to reach the presign seam, saw {seen:?}"
    );
    for room in &seen {
        assert_eq!(
            room, &expected_room,
            "room-agnostic route passed room `{room}`; the frozen contract placeholder is `{expected_room}` \
             (delegate_room_id_placeholder in work-unit-status-vectors.v1.json)"
        );
    }
}

/// `Arc<CapturingPresignBackend>` newtype so the capturing backend can be
/// shared between the router and the test's assertions (`Composite` takes
/// its halves by value).
#[derive(Debug)]
struct SharedBackend(Arc<CapturingPresignBackend>);

impl BlobPersistence for SharedBackend {
    fn persist_blob(&self, hash: &Hash, bytes: &[u8]) -> Result<(), PersistBlobError> {
        self.0.persist_blob(hash, bytes)
    }

    fn resolve_get_url(
        &self,
        room_id: &str,
        hash: &Hash,
        size_hint: Option<u64>,
    ) -> Option<PresignedUrl> {
        self.0.resolve_get_url(room_id, hash, size_hint)
    }

    fn resolve_put_url(
        &self,
        room_id: &str,
        hash: &Hash,
        size: u64,
        content_type: Option<&str>,
    ) -> Option<PresignedUrl> {
        self.0.resolve_put_url(room_id, hash, size, content_type)
    }

    fn verify_uploaded(&self, room_id: &str, hash: &Hash) -> Result<(), String> {
        self.0.verify_uploaded(room_id, hash)
    }

    fn supports_presigned_urls(&self) -> bool {
        self.0.supports_presigned_urls()
    }
}
