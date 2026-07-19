//! Slice 4.2 (blob-cas-remediation.md, finding #8) — `PUT /blobs/{hash}`
//! must not lie about durability.
//!
//! Pre-4.2 the handler (1) upserted the hash `Active` in the GC inventory
//! BEFORE the write, (2) called a `persist_blob` that was infallible by
//! signature — Dir and S3 both logged-and-swallowed their failures — and
//! (3) returned `201 Created` unconditionally. The client then dropped its
//! only copy of the bytes. The inventory row is the data-loss *multiplier*:
//! the 1.3 upload-window union reads recent `Active` rows into the live
//! set, so GC spent the window protecting a hash the store never held, and
//! when the window lapsed there was nothing to reclaim — the loss surfaced
//! only when a peer asked for the blob.
//!
//! Post-4.2 contract, pinned here end-to-end:
//!   * a failed persist returns 5xx — `503` when the backend never answered
//!     (timeout/unreachable; retry is right and idempotent), `500` when the
//!     write was attempted and reported failure (disk full, permissions,
//!     bucket error reply);
//!   * NO `Active` inventory row exists after a failed PUT;
//!   * a later GET/HEAD agrees (404) — the store and GC state never
//!     disagree about a hash.
//!
//! Backends covered: an injectable failing fake (both error classes), a
//! real `DirPersistence` against a genuinely broken filesystem target, and
//! a real `S3BlobStore` against a stalled TCP listener (bridge_timeouts'
//! adversary: accepts the connection, never says another byte). The S3
//! variant is Docker-free on purpose — same placement reasoning as
//! `bridge_timeouts.rs` (6.1): the stalled listener needs no container, so
//! it lives in the fast blob-layout-parity workflow rather than the MinIO
//! one; a stopped-MinIO variant would gate the same class at container
//! cost.
//!
//! RED protocol note: these tests assert post-fix behavior. The pre-fix
//! reproduction is `git checkout HEAD~ -- src/blob_http.rs`-style reverts
//! of the handler only (the trait change keeps the tree compiling — the
//! old handler statement-drops the new `Result`, which is exactly the
//! swallow being fixed), under which the failure-path tests here go red
//! with `201` + an `Active` row.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use ed25519_dalek::SigningKey;
use nodalmerge_core::Hash;
use nodalmerge_gc::contracts::AssetInventoryStore;
use nodalmerge_gc::types::AssetState;
use nodalmerge_s3_blobs::{S3Auth, S3BlobStore, S3BlobStoreConfig};
use nodalmerge_server::blob_http::{self, BlobHttpConfig};
use nodalmerge_server::gc_store::{local_key_scheme, SqliteGcStore};
use nodalmerge_server::room::Rooms;
use nodalmerge_server::store::{
    BlobPersistence, Composite, DirPersistence, NoPersistence, PersistBlobError,
    SharedPersistence,
};
use tower::ServiceExt;

fn tmpdir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!("nodalmerge-put-durability-{tag}-{nanos}"));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// A store whose every persist fails with a configured class and which
/// honestly holds nothing (`has_blob` via the trait default: `get_blob`
/// `None` → `false`) — the injectable "write failed" backend.
#[derive(Debug)]
struct FailingBlobStore {
    error: PersistBlobError,
}

impl BlobPersistence for FailingBlobStore {
    fn persist_blob(&self, _hash: &Hash, _bytes: &[u8]) -> Result<(), PersistBlobError> {
        Err(self.error.clone())
    }
}

/// Router + the inventory handle, wired the way `main.rs` wires them: one
/// `SqliteGcStore` shared between `BlobHttpConfig::gc_inventory` and (in
/// production) `Rooms::with_gc_inventory`.
fn router_with_inventory(
    persistence: SharedPersistence,
    inventory_dir: &std::path::Path,
    key_seed: u8,
) -> (Router, Arc<SqliteGcStore>) {
    let inventory = Arc::new(SqliteGcStore::open(inventory_dir, local_key_scheme()).unwrap());
    let rooms = Rooms::new(
        SigningKey::from_bytes(&[key_seed; 32]),
        persistence,
        512,
        0,
        0,
    );
    let cfg = BlobHttpConfig {
        gc_inventory: Some(Arc::clone(&inventory) as Arc<dyn AssetInventoryStore>),
        ..BlobHttpConfig::default()
    };
    (blob_http::blob_routes(cfg).with_state(rooms), inventory)
}

fn put(router: &Router, hash_hex: &str, body: Vec<u8>) -> Request<Body> {
    let _ = router;
    Request::builder()
        .method(Method::PUT)
        .uri(format!("/blobs/{hash_hex}"))
        .body(Body::from(body))
        .unwrap()
}

async fn send(router: &Router, req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let resp = router.clone().oneshot(req).await.expect("infallible");
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read body");
    (status, body.to_vec())
}

/// Both halves of the failed-PUT assertion, shared by every backend
/// variant below. The inventory half matters as much as the status: the
/// `Active` row is the data-loss vector (GC protects a hash the store
/// never persisted; the window lapses; nothing was ever there).
async fn assert_failed_put_left_no_trace(
    router: &Router,
    inventory: &SqliteGcStore,
    hash_hex: &str,
) {
    assert_eq!(
        inventory.asset_state(hash_hex),
        None,
        "a failed PUT must NOT leave an Active inventory row — that row is what \
         the 1.3 upload-window union feeds into the GC live set"
    );
    let (get_status, _) = send(
        router,
        Request::builder()
            .method(Method::GET)
            .uri(format!("/blobs/{hash_hex}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        get_status,
        StatusCode::NOT_FOUND,
        "a later GET must be consistent with the failure: the bytes are not there"
    );
    let (head_status, _) = send(
        router,
        Request::builder()
            .method(Method::HEAD)
            .uri(format!("/blobs/{hash_hex}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(head_status, StatusCode::NOT_FOUND, "HEAD agrees with GET");
}

// ─── Injectable failing backend: the two error classes → the two statuses ──

/// Core RED (Dir-class). Pre-fix (handler reverted): `201` + an `Active`
/// inventory row for bytes that were never stored. Post-fix: `500`, no
/// row, GET/HEAD 404.
#[tokio::test]
async fn put_on_a_backend_that_reports_write_failure_returns_500_and_no_active_row() {
    let dir = tmpdir("failing-backend-500");
    let persistence: SharedPersistence = Arc::new(Composite::new(
        NoPersistence,
        FailingBlobStore {
            error: PersistBlobError::Backend("disk full (injected)".into()),
        },
    ));
    let (router, inventory) = router_with_inventory(persistence, &dir, 0x42);

    let body = b"bytes the store will refuse".to_vec();
    let hash_hex = Hash::of(&body).to_hex();
    let (status, resp_body) = send(&router, put(&router, &hash_hex, body)).await;

    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "a reported write failure is a broken origin: 500, never 201"
    );
    let json: serde_json::Value = serde_json::from_slice(&resp_body).expect("error body is JSON");
    assert_eq!(json, serde_json::json!({"error": "blob write failed"}));
    assert_failed_put_left_no_trace(&router, &inventory, &hash_hex).await;

    let _ = std::fs::remove_dir_all(&dir);
}

/// Same shape, `Unavailable` class: the backend never answered, so the
/// client's right move is retry-later — `503`.
#[tokio::test]
async fn put_on_an_unavailable_backend_returns_503_and_no_active_row() {
    let dir = tmpdir("failing-backend-503");
    let persistence: SharedPersistence = Arc::new(Composite::new(
        NoPersistence,
        FailingBlobStore {
            error: PersistBlobError::Unavailable("op timed out (injected)".into()),
        },
    ));
    let (router, inventory) = router_with_inventory(persistence, &dir, 0x43);

    let body = b"bytes the backend never acknowledged".to_vec();
    let hash_hex = Hash::of(&body).to_hex();
    let (status, _) = send(&router, put(&router, &hash_hex, body)).await;

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "an unanswered write is retryable: 503, never 201"
    );
    assert_failed_put_left_no_trace(&router, &inventory, &hash_hex).await;

    let _ = std::fs::remove_dir_all(&dir);
}

// ─── Real DirPersistence against a genuinely broken filesystem target ──────

/// Core RED (Dir, real fs). The `blobs/blake3` directory is replaced by a
/// plain FILE, so `persist_blob`'s `create_dir_all` genuinely fails —
/// cross-platform and deterministic, unlike read-only-dir tricks. Pre-fix
/// `DirPersistence` warn!-swallowed exactly this and the handler said 201.
#[tokio::test]
async fn put_on_a_dir_store_whose_write_fails_returns_500_and_no_active_row() {
    let dir = tmpdir("dir-blocked-blake3");
    let persistence: SharedPersistence = Arc::new(DirPersistence::open(&dir).unwrap());
    // Sabotage the CAS directory AFTER open (which pre-creates it empty).
    let blake3_dir = dir.join("blobs").join("blake3");
    std::fs::remove_dir(&blake3_dir).expect("fresh blake3 dir is empty and removable");
    std::fs::write(&blake3_dir, b"a file squatting where the CAS dir must be").unwrap();

    let (router, inventory) = router_with_inventory(persistence, &dir, 0x44);

    let body = b"bytes with nowhere to land".to_vec();
    let hash_hex = Hash::of(&body).to_hex();
    let (status, _) = send(&router, put(&router, &hash_hex, body)).await;

    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "a local fs write failure must surface as 500 (pre-4.2: warn-only + 201)"
    );
    assert_failed_put_left_no_trace(&router, &inventory, &hash_hex).await;

    let _ = std::fs::remove_dir_all(&dir);
}

/// Unit-level pin for the same failure, without the HTTP layer: the Dir
/// backend itself must report `Err(Backend)` and must not report the hash
/// present afterwards (the conformance crate pins the same
/// failure↔presence coupling as a trait-shape scenario).
#[test]
fn dir_persistence_persist_blob_reports_backend_error_and_no_phantom_presence() {
    let dir = tmpdir("dir-unit-err");
    let store = DirPersistence::open(&dir).unwrap();
    let blake3_dir = dir.join("blobs").join("blake3");
    std::fs::remove_dir(&blake3_dir).unwrap();
    std::fs::write(&blake3_dir, b"squatter").unwrap();

    let bytes = b"unpersistable".to_vec();
    let hash = Hash::of(&bytes);
    let err = store
        .persist_blob(&hash, &bytes)
        .expect_err("create_dir_all over a file must fail");
    assert!(
        matches!(err, PersistBlobError::Backend(_)),
        "fs failures are Backend (500-class), got: {err:?}"
    );
    assert!(
        !store.has_blob(&hash),
        "a failed persist must not leave a phantom presence"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ─── Real S3BlobStore against a stalled endpoint (bridge_timeouts' shape) ──

/// Accepts connections, never responds — the hung-bucket adversary from
/// `s3-blobs/tests/bridge_timeouts.rs`, reused verbatim.
fn stalled_endpoint() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind stalled endpoint");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for conn in listener.incoming() {
            match conn {
                Ok(stream) => held.push(stream),
                Err(_) => break,
            }
        }
    });
    format!("http://{addr}")
}

fn stalled_direct_store() -> S3BlobStore {
    let mut cfg = S3BlobStoreConfig::default();
    cfg.bucket = "stalled-bucket".into();
    cfg.endpoint = Some(stalled_endpoint());
    cfg.require_https = false;
    cfg.auth = S3Auth::direct_explicit("ak", "sk");
    cfg.op_timeout = Duration::from_millis(750);
    cfg.connect_timeout = Duration::from_millis(500);
    S3BlobStore::new(cfg).expect("store construction is local")
}

/// Store-level classification: a persist against a hung bucket must return
/// `Err(Unavailable)` — bounded by the 6.1 bridge, naming the timeout —
/// never `Ok`. (The pre-fix signature could not express this at all; the
/// swallow was verified by sabotage — see the slice report.)
#[test]
fn stalled_bucket_persist_blob_reports_unavailable_within_the_timeout() {
    let store = stalled_direct_store();
    let bytes = b"bytes the bucket will never acknowledge".to_vec();
    let hash = Hash::of(&bytes);

    // bridge_timeouts' watchdog pattern: the op must COMPLETE well within
    // this bound; pre-6.1 it never returned at all.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(store.persist_blob(&hash, &bytes));
    });
    let res = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("persist_blob against a stalled bucket must fail bounded, not hang");

    let err = res.expect_err("a hung bucket must be an error, never a confirmed write");
    match &err {
        PersistBlobError::Unavailable(msg) => {
            assert!(
                msg.to_lowercase().contains("timed out") || msg.to_lowercase().contains("timeout"),
                "the Unavailable message should name the timeout, got: {msg}"
            );
        }
        PersistBlobError::Backend(_) => panic!(
            "a bridge timeout classified as Backend(500) — it must be Unavailable(503), \
             the write was never answered: {err:?}"
        ),
    }
}

/// Delegate mode's `Ok` is deliberate and pinned: it is a documented
/// structural no-op (no bucket credentials; clients use request-upload),
/// not a failed write — see `PersistBlobError`'s doc for the rule.
#[test]
fn delegate_mode_persist_blob_is_a_structural_no_op_ok() {
    let mut cfg = S3BlobStoreConfig::default();
    cfg.bucket = "delegate-bucket".into();
    cfg.auth = S3Auth::delegate("http://127.0.0.1:9/never-called", None);
    let store = S3BlobStore::new(cfg).expect("store construction is local");

    store
        .persist_blob(&Hash::of(b"delegate"), b"delegate")
        .expect("Delegate mode never writes through this path, by design — Ok, not Err");
}

/// Core RED (S3). This is 6.1's filed feeder verbatim: "PUT on a
/// timing-out backend: has_blob→false, persist_blob times out warn-only,
/// handler returns 201 with nothing durably stored." Pre-fix (handler
/// reverted): 201 + Active row. Post-fix: 503, no row.
#[tokio::test]
async fn put_against_a_stalled_bucket_returns_503_and_no_active_row() {
    let dir = tmpdir("s3-stalled-503");
    let persistence: SharedPersistence =
        Arc::new(Composite::new(NoPersistence, stalled_direct_store()));
    let (router, inventory) = router_with_inventory(persistence, &dir, 0x45);

    let body = b"bytes the hung bucket never stores".to_vec();
    let hash_hex = Hash::of(&body).to_hex();
    let (status, _) = send(&router, put(&router, &hash_hex, body)).await;

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "a timing-out backend must answer 503 — pre-4.2 this was a 201 with \
         nothing durably stored (6.1's filed feeder)"
    );
    assert_eq!(
        inventory.asset_state(&hash_hex),
        None,
        "no Active row for an unconfirmed write"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ─── Green-side ordering guards (honest: these pass pre-fix too) ───────────

/// NOT a RED — the success path already answered 201 and marked Active
/// pre-fix (in the other order). This pins the post-4.2 ordering's
/// success half: 201 AND the row exists, i.e. reordering the mark to
/// after the write didn't lose it.
#[tokio::test]
async fn successful_put_answers_201_and_marks_active() {
    let dir = tmpdir("success-201");
    let persistence: SharedPersistence = Arc::new(DirPersistence::open(&dir).unwrap());
    let (router, inventory) = router_with_inventory(persistence, &dir, 0x46);

    let body = b"genuinely stored bytes".to_vec();
    let hash_hex = Hash::of(&body).to_hex();
    let (status, _) = send(&router, put(&router, &hash_hex, body.clone())).await;

    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        inventory.asset_state(&hash_hex),
        Some(AssetState::Active),
        "a confirmed write still marks Active — the reorder must not drop the mark"
    );
    let (get_status, get_body) = send(
        &router,
        Request::builder()
            .method(Method::GET)
            .uri(format!("/blobs/{hash_hex}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(get_status, StatusCode::OK);
    assert_eq!(get_body, body);

    let _ = std::fs::remove_dir_all(&dir);
}

/// NOT a RED. The dedupe branch (bytes already present → 200) also marks
/// Active — those bytes genuinely exist, and pre-4.2 the mark covered this
/// branch by running before the dedupe check.
#[tokio::test]
async fn dedupe_200_still_marks_active() {
    let dir = tmpdir("dedupe-200");
    let store = Arc::new(DirPersistence::open(&dir).unwrap());
    let body = b"already present".to_vec();
    let hash = Hash::of(&body);
    store.persist_blob(&hash, &body).unwrap();

    let persistence: SharedPersistence = store;
    let (router, inventory) = router_with_inventory(persistence, &dir, 0x47);

    let (status, _) = send(&router, put(&router, &hash.to_hex(), body)).await;
    assert_eq!(status, StatusCode::OK, "idempotent re-PUT");
    assert_eq!(
        inventory.asset_state(&hash.to_hex()),
        Some(AssetState::Active),
        "the dedupe branch marks Active too — the bytes exist"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
