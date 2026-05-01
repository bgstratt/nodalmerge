//! F6 integration test — round-trip a blob through a real MinIO container.
//!
//! Direct-mode flow end-to-end:
//!  1. Construct `S3BlobStore` against a MinIO endpoint.
//!  2. Mint a presigned PUT URL via `resolve_put_url`.
//!  3. PUT bytes to that URL with a plain HTTP client.
//!  4. `verify_uploaded` succeeds (HEAD object).
//!  5. Mint a presigned GET URL via `resolve_get_url`.
//!  6. GET bytes back and confirm round-trip.
//!  7. `blob_gc_sweep` deletes objects not in the live set.
//!
//! Requires Docker. Skipped if container start fails.

use std::time::Duration;

use activesync_core::Hash;
use activesync_s3_blobs::{S3Auth, S3BlobStore, S3BlobStoreConfig};
use activesync_server::store::BlobPersistence;
use testcontainers::{
    core::{IntoContainerPort, WaitFor},
    runners::SyncRunner,
    GenericImage, ImageExt,
};

const ACCESS_KEY: &str = "minioadmin";
const SECRET_KEY: &str = "minioadmin";
const BUCKET: &str = "activesync-test";

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

/// Create the test bucket using the AWS SDK (dev-dep). Idempotent.
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
            // BucketAlreadyOwnedByYou / BucketAlreadyExists are fine.
            let s = format!("{e:?}");
            if s.contains("BucketAlreadyOwnedByYou") || s.contains("BucketAlreadyExists") {
                Ok(())
            } else {
                Err(format!("CreateBucket: {s}").into())
            }
        }
    }
}

#[test]
fn s3_blob_round_trip_via_minio() {
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

    let cfg = S3BlobStoreConfig {
        bucket: BUCKET.into(),
        region: "us-east-1".into(),
        endpoint: Some(endpoint.clone()),
        path_prefix: "blobs/".into(),
        require_https: false,
        direct_upload_threshold: 0, // always presign for the test
        auth: S3Auth::direct_explicit(ACCESS_KEY, SECRET_KEY),
        ..Default::default()
    };
    let store = S3BlobStore::new(cfg).expect("build S3BlobStore");

    let payload = b"hello F6 from MinIO".to_vec();
    let hash = Hash::of(&payload);
    let room_id = "room-A";

    // 1. Mint presigned PUT and upload via plain HTTP.
    let put = store
        .resolve_put_url(room_id, &hash, payload.len() as u64, None)
        .expect("presign PUT");
    let put_status = rt.block_on(async {
        reqwest::Client::new()
            .put(&put.url)
            .header("Content-Type", "application/octet-stream")
            .body(payload.clone())
            .send()
            .await
            .expect("PUT")
            .status()
    });
    assert!(put_status.is_success(), "PUT to MinIO failed: {put_status}");

    // 2. Verify object exists.
    store
        .verify_uploaded(room_id, &hash)
        .expect("HEAD should find the object");

    // 3. Mint presigned GET, fetch, confirm round-trip.
    let get = store
        .resolve_get_url(room_id, &hash, Some(payload.len() as u64))
        .expect("presign GET");
    let body = rt
        .block_on(async {
            let r = reqwest::Client::new().get(&get.url).send().await?;
            assert!(r.status().is_success(), "GET status: {}", r.status());
            r.bytes().await
        })
        .expect("GET");
    assert_eq!(&body[..], &payload[..], "round-trip bytes mismatch");

    // 4. GC sweep: empty live set ⇒ object is deleted.
    let live = std::collections::HashSet::new();
    let deleted = store.blob_gc_sweep(room_id, &live, Duration::from_secs(0));
    assert!(deleted >= 1, "expected at least 1 deleted, got {deleted}");

    // 5. After GC, verify_uploaded reports missing.
    let post_gc = store.verify_uploaded(room_id, &hash);
    assert!(post_gc.is_err(), "object should be gone after GC: {post_gc:?}");

    drop(container);
}
