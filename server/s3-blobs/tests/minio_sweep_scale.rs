//! blob-cas-remediation.md slice 6.3 — bulk S3 sweep I/O, against real MinIO.
//!
//! `S3BlobStore::blob_gc_sweep` used to await one delete per object (N+1);
//! 6.3 classifies during the LIST and then runs per-object action chains
//! `SWEEP_IO_CONCURRENCY` (16) at a time. The two-phase tombstone protocol
//! itself is pinned by `minio_round_trip.rs` (untouched by this slice —
//! those tests must stay green as-is); this file adds:
//!
//! * a plain (Docker-gated, not `#[ignore]`d) **behavior-equality smoke** at
//!   small N: a mixed live/orphan population must tombstone-first,
//!   delete-after-grace, and clean up, exactly as the serial sweep did —
//!   i.e. concurrency changed the wall time, never the protocol;
//! * an `#[ignore]`d **bench** at ~10k objects with a loose sanity bound
//!   (the concurrent sweep must beat a serial single-op estimate), whose
//!   measured numbers are recorded in the slice report. Timing bounds are
//!   deliberately loose — CI runs only the smoke.
//!
//! Same Docker posture as minio_round_trip.rs: skips gracefully on a dev
//! laptop, `NODALMERGE_REQUIRE_DOCKER=1` (set by blob-s3-gc-minio.yml)
//! turns the skip into a hard failure.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use nodalmerge_core::Hash;
use nodalmerge_s3_blobs::{S3Auth, S3BlobStore, S3BlobStoreConfig};
use nodalmerge_server::store::BlobPersistence;
use testcontainers::{
    core::{IntoContainerPort, WaitFor},
    runners::SyncRunner,
    GenericImage, ImageExt,
};

const ACCESS_KEY: &str = "minioadmin";
const SECRET_KEY: &str = "minioadmin";
const BUCKET: &str = "nodalmerge-sweep-scale";

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
            // Fail-open-into-PASS is catastrophic in CI — same reasoning,
            // same env gate as minio_round_trip.rs's start_minio.
            if std::env::var("NODALMERGE_REQUIRE_DOCKER").as_deref() == Ok("1") {
                panic!(
                    "NODALMERGE_REQUIRE_DOCKER=1 but Docker / MinIO container is \
                     unavailable ({e:?}) — this suite must not silently pass in CI"
                );
            }
            eprintln!("MinIO start failed: {e:?}");
            None
        }
    }
}

fn s3_client(endpoint: &str) -> aws_sdk_s3::Client {
    use aws_credential_types::Credentials;
    let creds = Credentials::new(ACCESS_KEY, SECRET_KEY, None, None, "test");
    let cfg = aws_sdk_s3::Config::builder()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new("us-east-1"))
        .endpoint_url(endpoint)
        .force_path_style(true)
        .credentials_provider(creds)
        .build();
    aws_sdk_s3::Client::from_conf(cfg)
}

async fn ensure_bucket(endpoint: &str) -> Result<(), Box<dyn std::error::Error>> {
    match s3_client(endpoint).create_bucket().bucket(BUCKET).send().await {
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

async fn list_keys(endpoint: &str, prefix: &str) -> Vec<String> {
    let client = s3_client(endpoint);
    let mut out = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut req = client.list_objects_v2().bucket(BUCKET).prefix(prefix);
        if let Some(t) = &token {
            req = req.continuation_token(t);
        }
        let resp = req.send().await.expect("list_objects_v2");
        out.extend(
            resp.contents()
                .iter()
                .filter_map(|o| o.key().map(|k| k.to_string())),
        );
        match resp.next_continuation_token() {
            Some(t) => token = Some(t.to_string()),
            None => break,
        }
    }
    out
}

/// Upload `n` distinct tiny blobs concurrently via the AWS SDK (setup only —
/// the code under test is the sweep, not the upload path). Returns hashes.
async fn seed_objects(endpoint: &str, n: usize, tag: &str) -> Vec<Hash> {
    use futures_util::StreamExt;
    let client = s3_client(endpoint);
    let hashes: Vec<(Hash, Vec<u8>)> = (0..n)
        .map(|i| {
            let bytes = format!("{tag}-{i}").into_bytes();
            (Hash::of(&bytes), bytes)
        })
        .collect();
    let jobs: Vec<(String, Vec<u8>)> = hashes
        .iter()
        .map(|(h, bytes)| (format!("blobs/blake3/{}", h.to_hex()), bytes.clone()))
        .collect();
    let mut uploads = futures_util::stream::iter(jobs.into_iter().map(|(key, body)| {
        let client = client.clone();
        async move {
            client
                .put_object()
                .bucket(BUCKET)
                .key(key)
                .body(body.into())
                .send()
                .await
                .expect("seed PUT");
        }
    }))
    .buffer_unordered(64);
    while uploads.next().await.is_some() {}
    drop(uploads);
    hashes.into_iter().map(|(h, _)| h).collect()
}

fn minio_fixture() -> Option<(
    testcontainers::Container<GenericImage>,
    String,
    tokio::runtime::Runtime,
)> {
    let _ = tracing_subscriber::fmt::try_init();
    let container = start_minio()?;
    let host_port = container.get_host_port_ipv4(9000).expect("host port");
    let endpoint = format!("http://127.0.0.1:{host_port}");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let bucket_ok = rt.block_on(async {
        for _ in 0..20 {
            if ensure_bucket(&endpoint).await.is_ok() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        false
    });
    assert!(bucket_ok, "could not create test bucket");
    Some((container, endpoint, rt))
}

fn test_cfg(endpoint: &str) -> S3BlobStoreConfig {
    S3BlobStoreConfig {
        bucket: BUCKET.into(),
        region: "us-east-1".into(),
        endpoint: Some(endpoint.to_string()),
        path_prefix: "blobs/".into(),
        require_https: false,
        direct_upload_threshold: 0,
        auth: S3Auth::direct_explicit(ACCESS_KEY, SECRET_KEY),
        ..Default::default()
    }
}

/// Behavior-equality smoke at small N: the concurrent sweep must produce
/// exactly the serial sweep's observable end states across a mixed
/// population — live objects untouched and never tombstoned, orphans
/// tombstoned first, deleted (with tombstone cleanup) only after grace.
#[test]
fn concurrent_sweep_small_scale_keeps_two_phase_semantics_exactly() {
    let Some((container, endpoint, rt)) = minio_fixture() else {
        eprintln!("skipping: Docker / MinIO container unavailable");
        return;
    };
    let store = S3BlobStore::new(test_cfg(&endpoint)).expect("build S3BlobStore");

    // 40 orphans + 10 live — enough that buffer_unordered(16) actually
    // overlaps actions, small enough for a smoke.
    let orphans = rt.block_on(seed_objects(&endpoint, 40, "orphan"));
    let live_hashes = rt.block_on(seed_objects(&endpoint, 10, "live"));
    let live: HashSet<Hash> = live_hashes.iter().copied().collect();
    let grace = Duration::from_millis(50);

    // Phase 1: tombstones for every orphan, zero deletes, zero live tombstones.
    let deleted = store.blob_gc_sweep(&live, grace);
    assert_eq!(deleted, 0, "first sighting must only tombstone, never delete");
    let tombs = rt.block_on(list_keys(&endpoint, "blobs/.tombstones/"));
    assert_eq!(tombs.len(), 40, "exactly one tombstone per orphan: {}", tombs.len());
    for h in &live_hashes {
        assert!(
            !tombs.iter().any(|k| k.contains(&h.to_hex())),
            "a live object must never be tombstoned"
        );
    }

    // Phase 2: aged past grace — all orphans deleted, tombstones cleaned,
    // live objects intact.
    std::thread::sleep(Duration::from_millis(120));
    let deleted = store.blob_gc_sweep(&live, grace);
    assert_eq!(deleted, 40, "every aged orphan deletes in one sweep");
    let remaining = rt.block_on(list_keys(&endpoint, "blobs/blake3/"));
    assert_eq!(remaining.len(), 10, "exactly the live objects remain");
    for h in &orphans {
        assert!(!remaining.iter().any(|k| k.contains(&h.to_hex())));
    }
    let tombs = rt.block_on(list_keys(&endpoint, "blobs/.tombstones/"));
    assert!(tombs.is_empty(), "tombstones cleaned up with their objects: {tombs:?}");

    // Phase 3: idempotent.
    assert_eq!(store.blob_gc_sweep(&live, grace), 0);

    drop(container);
}

/// Bench (not CI-gating): sweep-delete cost at ~10k objects. The loose
/// sanity bound compares the concurrent sweep against a serial estimate
/// measured in-test (mean single-op latency x object count / 3) — the
/// pre-6.3 sweep was exactly one awaited op per object, so that estimate
/// is a faithful floor for the old shape. Real before/after numbers are
/// measured by checking out the pre-6.3 lib.rs and recorded in the report.
#[test]
#[ignore = "bench: run by hand with --ignored --nocapture (slice 6.3 perf validation; needs Docker)"]
fn bench_sweep_delete_10k_objects() {
    const N: usize = 10_000;
    let Some((container, endpoint, rt)) = minio_fixture() else {
        eprintln!("skipping: Docker / MinIO container unavailable");
        return;
    };
    let store = S3BlobStore::new(test_cfg(&endpoint)).expect("build S3BlobStore");

    let t0 = Instant::now();
    let _hashes = rt.block_on(seed_objects(&endpoint, N, "bench"));
    eprintln!("seeded {N} objects in {:?}", t0.elapsed());

    // Serial single-op estimate: mean of 50 sequential DELETEs against probe
    // objects outside `blobs/blake3/` (the sweep never lists them). DELETEs,
    // not HEADs, deliberately: on MinIO a delete costs several times a
    // metadata read, and the pre-6.3 sweep paid exactly one awaited delete
    // per op — an estimate from HEAD latency flatters the serial baseline.
    let client = s3_client(&endpoint);
    rt.block_on(async {
        for i in 0..50 {
            client
                .put_object()
                .bucket(BUCKET)
                .key(format!("bench-probe/{i}"))
                .body(Vec::from(b"p" as &[u8]).into())
                .send()
                .await
                .expect("probe PUT");
        }
    });
    let t0 = Instant::now();
    for i in 0..50 {
        store.delete_key(&format!("bench-probe/{i}")).expect("probe DELETE");
    }
    let single_op = t0.elapsed() / 50;

    let live = HashSet::new();
    let t0 = Instant::now();
    let deleted_phase1 = store.blob_gc_sweep(&live, Duration::from_millis(1));
    let tombstone_pass = t0.elapsed();
    assert_eq!(deleted_phase1, 0, "phase 1 only tombstones");

    std::thread::sleep(Duration::from_millis(50));
    let t0 = Instant::now();
    let deleted = store.blob_gc_sweep(&live, Duration::from_millis(1));
    let delete_pass = t0.elapsed();
    assert_eq!(deleted, N, "phase 2 deletes everything");

    // Phase 2 pays two deletes per object (blob + its tombstone), exactly
    // as the old serial loop did.
    let serial_estimate = single_op * (2 * N as u32);
    println!(
        "sweep over {N} objects: tombstone pass {tombstone_pass:?}, delete pass {delete_pass:?} \
         ({:.0} deletes/s); measured single-delete latency {single_op:?} -> serial estimate \
         {serial_estimate:?} (2 ops/object), speedup vs serial estimate {:.1}x",
        N as f64 / delete_pass.as_secs_f64(),
        serial_estimate.as_secs_f64() / delete_pass.as_secs_f64(),
    );
    assert!(
        delete_pass < serial_estimate / 3,
        "concurrent sweep should beat the serial single-delete estimate by at least ~3x \
         (loose sanity bound, not a CI gate): {delete_pass:?} vs {serial_estimate:?}"
    );

    drop(container);
}
