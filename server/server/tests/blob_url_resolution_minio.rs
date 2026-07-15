//! S4.2 — MinIO-gated end-to-end integration test for the blob
//! URL-resolution HTTP endpoints, driven through the real axum router
//! (unlike `server/s3-blobs/tests/minio_round_trip.rs`, which exercises
//! `S3BlobStore` directly without any HTTP layer). Covers the full external
//! contract: `GET /blobs/{hash}/url?op=put` -> PUT bytes to the returned
//! URL -> `POST /blobs/{hash}/uploaded` -> `HEAD /blobs/{hash}` -> `GET
//! /blobs/{hash}/url?op=get` -> GET bytes from the returned URL ->
//! byte-identical. Also confirms the relay `GET /blobs/{hash}` 404s (S3
//! backends never hydrate bytes into the server process — see
//! `blob_relay_nonhydrating_backend.rs` for the fast in-process version of
//! that same assertion).
//!
//! Requires Docker. Skipped (not failed) if the MinIO container can't
//! start — same gating pattern as `server/s3-blobs/tests/minio_round_trip.rs`.
//!
//! ## Running locally
//! ```text
//! cargo test -p nodalmerge-server --test blob_url_resolution_minio -- --nocapture
//! ```
//! Requires a working Docker daemon reachable the way `testcontainers`
//! expects (Docker Desktop on Windows/macOS, or a local `dockerd` on
//! Linux). No manual MinIO setup needed — the test starts and tears down
//! its own container.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use ed25519_dalek::SigningKey;
use nodalmerge_core::Hash;
use nodalmerge_s3_blobs::{S3Auth, S3BlobStore, S3BlobStoreConfig};
use nodalmerge_server::blob_http::{self, BlobHttpConfig};
use nodalmerge_server::room::Rooms;
use nodalmerge_server::store::{Composite, NoPersistence, SharedPersistence};
use testcontainers::{
    core::{IntoContainerPort, WaitFor},
    runners::SyncRunner,
    GenericImage, ImageExt,
};
use tower::ServiceExt;

const ACCESS_KEY: &str = "minioadmin";
const SECRET_KEY: &str = "minioadmin";
const BUCKET: &str = "nodalmerge-url-resolution-test";

fn start_minio() -> Option<testcontainers::Container<GenericImage>> {
    let result = GenericImage::new("minio/minio", "latest")
        .with_exposed_port(9000.tcp())
        .with_wait_for(WaitFor::message_on_stderr("API:"))
        .with_env_var("MINIO_ROOT_USER", ACCESS_KEY)
        .with_env_var("MINIO_ROOT_PASSWORD", SECRET_KEY)
        .with_cmd(vec!["server", "/data"])
        .start();
    match result {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("MinIO start failed: {e:?}");
            None
        }
    }
}

async fn ensure_bucket(endpoint: &str) -> Result<(), Box<dyn std::error::Error>> {
    use aws_credential_types::Credentials;
    let creds = Credentials::new(ACCESS_KEY, SECRET_KEY, None, None, "test");
    let cfg = aws_sdk_s3::Config::builder()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new("us-east-1"))
        .endpoint_url(endpoint)
        .force_path_style(true)
        .credentials_provider(creds)
        .build();
    let client = aws_sdk_s3::Client::from_conf(cfg);
    match client.create_bucket().bucket(BUCKET).send().await {
        Ok(_) => Ok(()),
        Err(e) => {
            let s = format!("{e:?}");
            if s.contains("BucketAlreadyOwnedByYou") || s.contains("BucketAlreadyExists") {
                Ok(())
            } else {
                Err(format!("CreateBucket: {s}").into())
            }
        }
    }
}

fn json_field<'a>(v: &'a serde_json::Value, field: &str) -> &'a str {
    v.get(field)
        .and_then(|x| x.as_str())
        .unwrap_or_else(|| panic!("response missing string field `{field}`: {v}"))
}

// Plain `#[test]`, not `#[tokio::test]` — `testcontainers`'s blocking
// `SyncRunner::start()` (used by `start_minio`) creates its own Tokio
// runtime internally, which panics ("Cannot start a runtime from within a
// runtime") if called from a thread that's already driving one. This
// mirrors `server/s3-blobs/tests/minio_round_trip.rs`'s exact structure: a
// synchronous outer test, with a single `tokio::runtime::Runtime` created
// after the container is up to drive every `async` call below.
#[test]
fn blob_url_resolution_round_trip_via_minio() {
    let _ = tracing_subscriber::fmt::try_init();
    let Some(container) = start_minio() else {
        eprintln!("skipping: Docker / MinIO container unavailable");
        return;
    };
    let host_port = container.get_host_port_ipv4(9000).expect("host port");
    let endpoint = format!("http://127.0.0.1:{host_port}");
    eprintln!("MinIO listening at {endpoint}");

    let rt = tokio::runtime::Runtime::new().unwrap();

    let bucket_ok = rt.block_on(async {
        for attempt in 0..20 {
            match ensure_bucket(&endpoint).await {
                Ok(()) => return true,
                Err(e) => {
                    eprintln!("ensure_bucket attempt {attempt} failed: {e}");
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
            }
        }
        false
    });
    assert!(bucket_ok, "could not create test bucket");

    let s3_cfg = S3BlobStoreConfig {
        bucket: BUCKET.into(),
        region: "us-east-1".into(),
        endpoint: Some(endpoint.clone()),
        path_prefix: "blobs/".into(),
        require_https: false,
        direct_upload_threshold: 0, // always presign for the test
        auth: S3Auth::direct_explicit(ACCESS_KEY, SECRET_KEY),
        ..Default::default()
    };
    let blobs = S3BlobStore::new(s3_cfg).expect("build S3BlobStore");
    let persistence: SharedPersistence = Arc::new(Composite::new(NoPersistence, blobs));
    let rooms = Rooms::new(SigningKey::from_bytes(&[0x91; 32]), persistence, 512, 0, 0);
    let app: Router = blob_http::blob_routes(BlobHttpConfig::default()).with_state(rooms);

    let payload = b"hello S4.2 blob url resolution via MinIO".to_vec();
    let hash = Hash::of(&payload);
    let hash_hex = hash.to_hex();

    rt.block_on(async {
        // 1. GET /blobs/{hash}/url?op=put&size=N -> 200 {url, expiresAtUtc}
        let req = Request::builder()
            .method(Method::GET)
            .uri(format!("/blobs/{hash_hex}/url?op=put&size={}", payload.len()))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let put_url = json_field(&parsed, "url").to_string();
        let _ = json_field(&parsed, "expiresAtUtc");

        // 2. PUT bytes to the returned URL with a plain HTTP client.
        let put_status = reqwest::Client::new()
            .put(&put_url)
            .header("Content-Type", "application/octet-stream")
            .body(payload.clone())
            .send()
            .await
            .expect("PUT to presigned URL")
            .status();
        assert!(put_status.is_success(), "PUT to MinIO failed: {put_status}");

        // 3. POST /blobs/{hash}/uploaded -> 200
        let req = Request::builder()
            .method(Method::POST)
            .uri(format!("/blobs/{hash_hex}/uploaded"))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "upload confirmation should verify via bucket HEAD (Direct auth mode)"
        );

        // 4. HEAD /blobs/{hash} -> 200 (via has_blob, a real bucket HEAD).
        let req = Request::builder()
            .method(Method::HEAD)
            .uri(format!("/blobs/{hash_hex}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // 5. GET /blobs/{hash} (the relay/escape-hatch path) -> 404: even
        // though the object genuinely exists (step 4 just confirmed it via
        // HEAD), S3BlobStore::get_blob never hydrates bytes into the server
        // process.
        let req = Request::builder()
            .method(Method::GET)
            .uri(format!("/blobs/{hash_hex}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "relay GET must not proxy bytes for a backend that doesn't hydrate them"
        );

        // 6. GET /blobs/{hash}/url?op=get -> 200 {url, expiresAtUtc}
        let req = Request::builder()
            .method(Method::GET)
            .uri(format!("/blobs/{hash_hex}/url?op=get"))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let get_url = json_field(&parsed, "url").to_string();

        // 7. GET the returned URL and confirm byte-identical round-trip.
        let fetched = reqwest::Client::new()
            .get(&get_url)
            .send()
            .await
            .expect("GET presigned URL")
            .bytes()
            .await
            .expect("read GET body");
        assert_eq!(&fetched[..], &payload[..], "round-trip bytes mismatch");
    });

    drop(container);
}
