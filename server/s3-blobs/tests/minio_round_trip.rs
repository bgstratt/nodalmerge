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
//! blob-cas-remediation.md slice 1.3 adds the two GC gates that the S3
//! backend's real (bucket-touching) two-phase grace rests on — see
//! `s3_blob_gc_two_phase_honors_grace_and_tombstones_first` and
//! `s3_min_physical_grace_floor_is_effective_end_to_end` below. Those cannot
//! be delegated to `NonHydratingBackend` (server/stores/blob-conformance):
//! that fake is a port of this backend's logic, so it gates a *copy*, never
//! the bucket-side tombstone objects themselves.
//!
//! Requires Docker. Skipped locally if container start fails; CI sets
//! `NODALMERGE_REQUIRE_DOCKER=1`, which turns that skip into a panic (see
//! `start_minio`).

use std::time::Duration;

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
const BUCKET: &str = "nodalmerge-test";

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
            // Graceful skip (the caller's `else` branch below) is correct for
            // a dev laptop that may not have Docker running — but the
            // `eprintln!` + `return` it does reports as a PASS to the test
            // harness. In CI that is catastrophic: if the
            // Docker/testcontainers setup ever breaks, the job goes green
            // while testing nothing, silently losing all coverage of
            // `S3BlobStore` — including the two-phase blob GC grace that
            // blob-cas-remediation.md slice 1.3 added, which is the *only*
            // thing standing between an unreferenced-for-one-tick blob and
            // permanent deletion on the S3 path. `NODALMERGE_REQUIRE_DOCKER=1`
            // (set at workflow level in .github/workflows/blob-s3-gc-minio.yml)
            // converts the skip into a hard failure; local/dev runs without
            // the var keep skipping gracefully. Same pattern, same reasoning
            // as server/stores/postgres/tests/postgres_round_trip.rs.
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

/// Every key under `prefix` in the test bucket. Used to assert on the
/// tombstone objects `blob_gc_sweep` writes (there is no public API for
/// them — they are the backend's own bookkeeping, exactly like
/// `DirPersistence`'s `blobs/.tombstones/blake3/` directory, which
/// `server/server/tests/blob_gc.rs` asserts on the same way).
async fn list_keys(endpoint: &str, prefix: &str) -> Vec<String> {
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
    let out = client
        .list_objects_v2()
        .bucket(BUCKET)
        .prefix(prefix)
        .send()
        .await
        .expect("list_objects_v2");
    out.contents()
        .iter()
        .filter_map(|o| o.key().map(|k| k.to_string()))
        .collect()
}

/// Boot MinIO + create the bucket, returning `(container, endpoint, runtime)`.
/// `None` ⇒ Docker unavailable and `NODALMERGE_REQUIRE_DOCKER` is not set.
fn minio_fixture() -> Option<(
    testcontainers::Container<GenericImage>,
    String,
    tokio::runtime::Runtime,
)> {
    let _ = tracing_subscriber::fmt::try_init();
    let container = start_minio()?;
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
    Some((container, endpoint, rt))
}

/// Direct-mode config against `endpoint`, always presigning.
fn test_cfg(endpoint: &str) -> S3BlobStoreConfig {
    S3BlobStoreConfig {
        bucket: BUCKET.into(),
        region: "us-east-1".into(),
        endpoint: Some(endpoint.to_string()),
        path_prefix: "blobs/".into(),
        require_https: false,
        direct_upload_threshold: 0, // always presign for the test
        auth: S3Auth::direct_explicit(ACCESS_KEY, SECRET_KEY),
        ..Default::default()
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
    let Some((container, endpoint, rt)) = minio_fixture() else {
        eprintln!("skipping: Docker / MinIO container unavailable");
        return;
    };
    let store = S3BlobStore::new(test_cfg(&endpoint)).expect("build S3BlobStore");

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

    // 2b. S4.2: `has_blob` is a real bucket HEAD in Direct mode (used by the
    // blob HTTP origin's `HEAD /blobs/{hash}`), independent of `get_blob`
    // (which always returns `None` for this backend).
    assert!(store.has_blob(&hash), "has_blob should find the object after upload");
    assert!(store.get_blob(&hash).is_none(), "S3BlobStore never hydrates bytes via get_blob");

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
    let deleted = store.blob_gc_sweep(&live, Duration::from_secs(0));
    assert!(deleted >= 1, "expected at least 1 deleted, got {deleted}");

    // 5. After GC, verify_uploaded reports missing.
    let post_gc = store.verify_uploaded(room_id, &hash);
    assert!(post_gc.is_err(), "object should be gone after GC: {post_gc:?}");
    assert!(!store.has_blob(&hash), "has_blob should report false after GC deletes the object");

    drop(container);
}

// ─── blob-cas-remediation.md slice 1.3 (finding #3) ──────────────────────────

#[test]
fn s3_blob_gc_two_phase_honors_grace_and_tombstones_first() {
    // Slice 1.3 — the real gate. `S3BlobStore::blob_gc_sweep` used to bind
    // `_grace` and never read it: a single-pass, immediate delete of every
    // object not in the live set, no tombstone, no grace window. The
    // documented premise was that "S3 object versions act as their own grace
    // period, and operators who want hard-delete can disable versioning" —
    // i.e. the only thing standing in for grace was an optional bucket
    // feature the same comment invited operators to turn off (and which this
    // MinIO bucket, like any default bucket, does NOT have enabled).
    //
    // This asserts the two-phase protocol `DirPersistence::blob_gc_sweep`
    // has always had, against a real bucket:
    //   1. first sighting of an unreferenced object only tombstones,
    //   2. a second sighting *within* grace still does not delete (this is
    //      the step that proves `grace` is genuinely read, rather than the
    //      sweep merely deleting on any second sighting),
    //   3. a sighting once the tombstone is older than grace deletes,
    //   4. a live object is never touched, and never tombstoned.
    let Some((container, endpoint, rt)) = minio_fixture() else {
        eprintln!("skipping: Docker / MinIO container unavailable");
        return;
    };
    let store = S3BlobStore::new(test_cfg(&endpoint)).expect("build S3BlobStore");

    let live_bytes = b"referenced-by-a-setblob-op".to_vec();
    let live_hash = Hash::of(&live_bytes);
    let orphan_bytes = b"uploaded-but-never-referenced".to_vec();
    let orphan_hash = Hash::of(&orphan_bytes);
    store.persist_blob(&live_hash, &live_bytes);
    store.persist_blob(&orphan_hash, &orphan_bytes);
    assert!(store.has_blob(&live_hash), "sanity: live object uploaded");
    assert!(store.has_blob(&orphan_hash), "sanity: orphan object uploaded");

    let live: std::collections::HashSet<Hash> = [live_hash].into_iter().collect();
    let grace = Duration::from_millis(50);

    // --- Phase 1: tombstone only. ------------------------------------------
    let deleted = store.blob_gc_sweep(&live, grace);
    assert_eq!(deleted, 0, "first sighting must only tombstone, never delete");
    assert!(store.has_blob(&orphan_hash), "orphan object must still be in the bucket");
    assert!(store.has_blob(&live_hash), "live object must survive");

    let tombs = rt.block_on(list_keys(&endpoint, "blobs/.tombstones/"));
    assert!(
        tombs.iter().any(|k| k.contains(&orphan_hash.to_hex())),
        "orphan must have a tombstone object after its first sighting; tombstone keys: {tombs:?}"
    );
    assert!(
        !tombs.iter().any(|k| k.contains(&live_hash.to_hex())),
        "a live object must never be tombstoned; tombstone keys: {tombs:?}"
    );

    // --- Phase 2: still inside grace ⇒ still no delete. ---------------------
    // This is the assertion a "delete on the second sighting" implementation
    // fails: the tombstone exists, but it is not yet `grace` old.
    let deleted = store.blob_gc_sweep(&live, Duration::from_secs(3600));
    assert_eq!(
        deleted, 0,
        "an existing tombstone younger than grace must not be deleted (grace must be read)"
    );
    assert!(store.has_blob(&orphan_hash), "orphan must survive a within-grace sweep");

    // --- Phase 3: tombstone ages past grace ⇒ delete. -----------------------
    std::thread::sleep(Duration::from_millis(120));
    let deleted = store.blob_gc_sweep(&live, grace);
    assert_eq!(deleted, 1, "exactly the orphan is deleted once its tombstone ages out");
    assert!(!store.has_blob(&orphan_hash), "orphan must be gone");
    assert!(store.has_blob(&live_hash), "live object must still survive");

    let tombs = rt.block_on(list_keys(&endpoint, "blobs/.tombstones/"));
    assert!(
        tombs.is_empty(),
        "the orphan's tombstone must be cleaned up with it; leftover: {tombs:?}"
    );

    // --- Phase 4: idempotent. ----------------------------------------------
    let deleted = store.blob_gc_sweep(&live, grace);
    assert_eq!(deleted, 0, "a repeat sweep is a no-op");

    drop(container);
}

#[test]
fn s3_blob_gc_clears_tombstone_when_object_becomes_live_again() {
    // The `DirPersistence` counterpart is
    // `blob_gc_clears_tombstone_when_blob_becomes_live_again`
    // (server/server/tests/blob_gc.rs). A blob that is unreferenced for one
    // tick and referenced on the next (a `SetBlob` op arriving between
    // sweeps) must have its tombstone cleared, not merely be spared this
    // round — otherwise the *next* sweep after it goes unreferenced again
    // would find an already-aged tombstone and delete with no grace at all.
    let Some((container, endpoint, rt)) = minio_fixture() else {
        eprintln!("skipping: Docker / MinIO container unavailable");
        return;
    };
    let store = S3BlobStore::new(test_cfg(&endpoint)).expect("build S3BlobStore");

    let bytes = b"unreferenced-then-referenced".to_vec();
    let hash = Hash::of(&bytes);
    store.persist_blob(&hash, &bytes);

    // Not live yet → tombstoned.
    let empty = std::collections::HashSet::new();
    assert_eq!(store.blob_gc_sweep(&empty, Duration::from_secs(3600)), 0);
    let tombs = rt.block_on(list_keys(&endpoint, "blobs/.tombstones/"));
    assert!(
        tombs.iter().any(|k| k.contains(&hash.to_hex())),
        "sanity: object must be tombstoned while unreferenced; keys: {tombs:?}"
    );

    // Now live → tombstone cleared, object untouched, even at grace == 0.
    let live: std::collections::HashSet<Hash> = [hash].into_iter().collect();
    let deleted = store.blob_gc_sweep(&live, Duration::ZERO);
    assert_eq!(deleted, 0, "a live object is never deleted, whatever the grace");
    assert!(store.has_blob(&hash), "live object must survive");
    let tombs = rt.block_on(list_keys(&endpoint, "blobs/.tombstones/"));
    assert!(
        tombs.is_empty(),
        "tombstone must be cleared once the object is live again; leftover: {tombs:?}"
    );

    drop(container);
}

#[test]
fn s3_min_physical_grace_floor_is_effective_end_to_end() {
    // blob-cas-remediation.md slice 1.4 (finding #5), bug 2 ↔ slice 1.3.
    //
    // 1.4 floored the grace `Rooms::sweep_blobs` hands the backend to
    // `MIN_PHYSICAL_GRACE` (1ms, server/server/src/room.rs) so that an
    // unreferenced blob's *first sighting* can only ever tombstone —
    // physical deletion always waits for a later, separate sweep call. That
    // closes the write-through window in which a blob's bytes land before
    // the node referencing them.
    //
    // On the S3 path that floor was a total no-op until slice 1.3: the
    // backend ignored `grace` entirely, so `--blob-gc-grace 0` still meant
    // "delete on first sighting" no matter what `sweep_blobs` passed down.
    // 1.4's regression pin (`blob_gc_zero_grace_never_deletes_on_first_sighting`)
    // only ever exercised `DirPersistence`. This is the same invariant, same
    // shape, driven through the real production wiring — `Rooms::sweep_blobs`
    // → `Composite<DirPersistence, S3BlobStore>` (exactly how `server-s3`
    // composes a node store with a bucket) → a real MinIO bucket.
    let Some((container, endpoint, rt)) = minio_fixture() else {
        eprintln!("skipping: Docker / MinIO container unavailable");
        return;
    };

    use ed25519_dalek::SigningKey;
    use nodalmerge_server::room::Rooms;
    use nodalmerge_server::store::{Composite, DirPersistence, SharedPersistence};
    use std::sync::Arc;

    let dir = tempfile::tempdir().expect("tempdir");
    let nodes = DirPersistence::open(dir.path()).expect("open DirPersistence");
    let blobs = S3BlobStore::new(test_cfg(&endpoint)).expect("build S3BlobStore");

    // An orphan: bytes in the bucket, referenced by no node in any room.
    let orphan_bytes = b"orphan-through-the-real-s3-wiring".to_vec();
    let orphan_hash = Hash::of(&orphan_bytes);
    blobs.persist_blob(&orphan_hash, &orphan_bytes);

    let persistence: SharedPersistence = Arc::new(Composite::new(nodes, blobs));
    let rooms = Rooms::new(
        SigningKey::from_bytes(&[0x13u8; 32]),
        Arc::clone(&persistence),
        512,
        0,
        0,
    );
    let probe = S3BlobStore::new(test_cfg(&endpoint)).expect("probe store");
    assert!(probe.has_blob(&orphan_hash), "sanity: orphan is in the bucket");

    rt.block_on(async {
        let _room = rooms.get_or_create("s3-floor-room").await;

        // --- First sighting with grace == 0: MUST only tombstone. -----------
        let deleted = rooms.sweep_blobs(Duration::ZERO).await;
        assert_eq!(
            deleted, 0,
            "with --blob-gc-grace 0, the S3 path must still never delete on first sighting"
        );
        assert!(
            probe.has_blob(&orphan_hash),
            "orphan object must still be in the bucket after its first sighting"
        );
        let tombs = list_keys(&endpoint, "blobs/.tombstones/").await;
        assert!(
            tombs.iter().any(|k| k.contains(&orphan_hash.to_hex())),
            "orphan must be tombstoned on its first sighting; keys: {tombs:?}"
        );

        // Age the tombstone past MIN_PHYSICAL_GRACE (1ms) — this ages an
        // already-existing tombstone, it does not race a window.
        tokio::time::sleep(Duration::from_millis(30)).await;

        // --- Second, separate sweep: now it may go. -------------------------
        let deleted = rooms.sweep_blobs(Duration::ZERO).await;
        assert_eq!(deleted, 1, "the second, separate sweep deletes the aged tombstone");
        assert!(!probe.has_blob(&orphan_hash), "orphan must be gone after the second sweep");
    });

    drop(container);
}

// ─── blob-cas-remediation.md slice 2.2 (finding #10) ─────────────────────────

/// Slice 2.2 — **the real gate for the hydrating tree read.**
///
/// `tree_walk::walk_tree` fetches tree objects through `BlobPersistence`.
/// Before 2.2 it used `get_blob`, which this backend hardwires to `None` by
/// policy — so every studio GC run over a cas-tree snapshot failed closed
/// forever on any S3-backed server (finding #10).
///
/// The fix routes the walk through `BlobPersistence::hydrate_blob`, which
/// `S3BlobStore` overrides with a real Direct-mode `GET`. **That policy is
/// not being violated:** `walk_tree` only ever fetches *tree* objects (v1
/// entries and v2 `"f"` entries are terminal — inserted into the live set,
/// never fetched), which are small JSON metadata the server must parse to do
/// its job. `get_blob`'s non-hydrating contract — about large *file* payloads
/// — is untouched, and this test asserts that explicitly below.
///
/// This test cannot be delegated to `NonHydratingBackend`
/// (server/stores/blob-conformance): that fake is a *port* of this backend,
/// so a green test there gates a copy of the logic, never the real bucket
/// `GET`. Same reasoning as the 1.3 GC gates above.
#[test]
fn s3_direct_tree_walk_resolves_tree_objects_from_the_real_bucket() {
    let Some((container, endpoint, _rt)) = minio_fixture() else {
        eprintln!("skipping: Docker / MinIO container unavailable");
        return;
    };
    let store = S3BlobStore::new(test_cfg(&endpoint)).expect("build S3BlobStore");

    // A file blob referenced by the tree. Never fetched by the walk — the
    // walk only needs its hash — but uploaded anyway so the fixture is a
    // faithful repo snapshot rather than a dangling reference.
    let file_bytes = b"a real repo file, living only in the bucket".to_vec();
    let file_hash = Hash::of(&file_bytes);
    store.persist_blob(&file_hash, &file_bytes);

    // The tree object is *also* an ordinary CAS blob (TREE_OBJECT_FORMAT.md).
    let tree_bytes = serde_json::to_vec(&serde_json::json!({
        "nodalmerge": "tree",
        "version": 2,
        "entries": [{"n": "a.txt", "k": "f", "h": file_hash.to_hex()}],
    }))
    .unwrap();
    let tree_hash = Hash::of(&tree_bytes);
    store.persist_blob(&tree_hash, &tree_bytes);

    assert!(store.has_blob(&tree_hash), "sanity: the tree object really is in the bucket");
    assert!(
        store.get_blob(&tree_hash).is_none(),
        "the non-hydrating policy on get_blob is deliberately UNCHANGED by slice 2.2 — \
         if this ever starts returning bytes, 2.2 broke the contract it was told not to touch"
    );

    let live = nodalmerge_server::tree_walk::walk_tree(&store, &tree_hash)
        .expect("finding #10: the tree walk must resolve tree objects on a real S3 bucket");

    assert!(live.contains(&tree_hash), "the root tree object itself must be live");
    assert!(live.contains(&file_hash), "the file the tree names must be live");
    assert_eq!(live.len(), 2, "root + file, nothing else: {live:?}");

    drop(container);
}

/// Slice 2.2, the nested half: a `"d"` entry is the *only* kind the walk
/// recurses into, so a multi-level tree is what actually proves the S3
/// hydrating read is reachable more than once per walk (a single-level tree
/// would pass even if only the root were resolvable).
#[test]
fn s3_direct_tree_walk_recurses_into_nested_directories_from_the_real_bucket() {
    let Some((container, endpoint, _rt)) = minio_fixture() else {
        eprintln!("skipping: Docker / MinIO container unavailable");
        return;
    };
    let store = S3BlobStore::new(test_cfg(&endpoint)).expect("build S3BlobStore");

    let leaf_bytes = b"nested file".to_vec();
    let leaf_hash = Hash::of(&leaf_bytes);
    store.persist_blob(&leaf_hash, &leaf_bytes);

    let subtree_bytes = serde_json::to_vec(&serde_json::json!({
        "nodalmerge": "tree", "version": 2,
        "entries": [{"n": "b.txt", "k": "f", "h": leaf_hash.to_hex()}],
    }))
    .unwrap();
    let subtree_hash = Hash::of(&subtree_bytes);
    store.persist_blob(&subtree_hash, &subtree_bytes);

    let root_bytes = serde_json::to_vec(&serde_json::json!({
        "nodalmerge": "tree", "version": 2,
        "entries": [{"n": "dir", "k": "d", "h": subtree_hash.to_hex()}],
    }))
    .unwrap();
    let root_hash = Hash::of(&root_bytes);
    store.persist_blob(&root_hash, &root_bytes);

    let live = nodalmerge_server::tree_walk::walk_tree(&store, &root_hash)
        .expect("nested tree walk must resolve every directory level from the bucket");

    assert!(live.contains(&root_hash), "root tree live");
    assert!(live.contains(&subtree_hash), "subtree (fetched via a second bucket GET) live");
    assert!(live.contains(&leaf_hash), "the nested file live");
    assert_eq!(live.len(), 3, "root + subtree + file: {live:?}");

    drop(container);
}

/// Slice 2.2 — a tree object that genuinely isn't in the bucket must still
/// read as **Missing**, not as the backend-can't-hydrate error. Without this
/// the fix could "pass" by reporting everything unresolvable, which would be
/// just as fail-closed as the bug (and would make the Delegate-mode gate
/// meaningless).
#[test]
fn s3_direct_tree_walk_reports_a_genuinely_absent_tree_as_missing() {
    let Some((container, endpoint, _rt)) = minio_fixture() else {
        eprintln!("skipping: Docker / MinIO container unavailable");
        return;
    };
    let store = S3BlobStore::new(test_cfg(&endpoint)).expect("build S3BlobStore");

    let phantom = Hash::of(b"a tree object that was never uploaded");
    assert!(!store.has_blob(&phantom), "sanity: it really isn't there");

    let err = nodalmerge_server::tree_walk::walk_tree(&store, &phantom)
        .expect_err("an absent root must fail the walk");
    assert!(
        matches!(err, nodalmerge_server::tree_walk::TreeWalkError::MissingBlob(_)),
        "an absent object is Missing, NOT Unresolvable — Unresolvable means \
         'this backend can never hydrate', which Direct mode plainly can: {err:?}"
    );

    drop(container);
}
