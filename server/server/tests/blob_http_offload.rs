//! Slice 6.2 (blob-cas-remediation.md) — the blob HTTP surface must never
//! run its blocking work on tokio's async workers. Every `BlobPersistence`
//! method is synchronous by design (file I/O on `DirPersistence`, a
//! thread-blocking bridge on `S3BlobStore` — bounded at `op_timeout + 15s`
//! since 6.1, but still parked), and PUT additionally BLAKE3-hashes an
//! up-to-64 MiB body. Run enough of that concurrently on a small worker
//! pool and unrelated async work (WS frames, sync traffic) starves.
//!
//! The starvation probes here make that deterministic instead of load-test
//! flaky: a runtime with exactly 2 workers, a store whose ops block in
//! `std::thread::sleep(500ms)` (standing in for slow disk / a stalled S3
//! bridge), more concurrent blob requests than workers, and a trivial
//! "heartbeat" task (3 × 5ms timer sleeps, ~15-50ms wall including Windows
//! timer granularity) that must finish within 200ms while the blob ops are
//! in flight. Margins: pre-fix the heartbeat cannot even be polled until a
//! worker returns from a full 500ms store sleep (≥ 2.5x over the bound by
//! construction, typically 1.5-2s with the queue ahead of it); post-fix the
//! workers never block, so ~15-50ms against a 200ms bound leaves ≥ 4x
//! headroom for a loaded CI box. The heartbeat clock only starts once the
//! store itself reports ≥ WORKERS ops entered (`ops_entered`), so the
//! pre-fix failure does not depend on task spawn ordering.
//!
//! Also pinned: a panic inside the offloaded blocking closure must surface
//! as a logged 500 — never a hang, a torn-down connection, or a silent
//! success.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use ed25519_dalek::SigningKey;
use nodalmerge_core::Hash;
use nodalmerge_server::blob_http::{self, BlobHttpConfig};
use nodalmerge_server::room::Rooms;
use nodalmerge_server::store::{
    BlobPersistence, Composite, NoPersistence, PersistBlobError, PresignedUrl, SharedPersistence,
};
use tower::ServiceExt;

/// Deliberately fewer workers than concurrent blob ops: with 2 workers and
/// 6 requests, a handler that blocks in-line parks the whole runtime.
const WORKERS: usize = 2;
const CONCURRENT_OPS: usize = 6;
/// One store op's blocking time. Big enough that a single in-handler sleep
/// (500ms) already blows the 200ms heartbeat bound with 2.5x margin.
const STORE_DELAY: Duration = Duration::from_millis(500);
const HEARTBEAT_BOUND: Duration = Duration::from_millis(200);

/// In-memory blob store whose every op blocks the calling thread for
/// `delay` — the deterministic stand-in for slow disk / a stalled S3
/// bridge. `ops_entered` counts ops that have *entered* the store (before
/// the sleep), so tests can wait for the blocking to genuinely be in
/// flight before starting the heartbeat clock.
#[derive(Debug)]
struct SlowBlobStore {
    blobs: Mutex<HashMap<Hash, Vec<u8>>>,
    delay: Duration,
    ops_entered: Arc<AtomicUsize>,
}

impl SlowBlobStore {
    fn new(delay: Duration) -> Self {
        Self {
            blobs: Mutex::new(HashMap::new()),
            delay,
            ops_entered: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Seed without the artificial delay (test setup, not a timed op).
    fn seed(&self, bytes: &[u8]) -> Hash {
        let hash = Hash::of(bytes);
        self.blobs.lock().unwrap().insert(hash, bytes.to_vec());
        hash
    }

    fn enter(&self) {
        self.ops_entered.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(self.delay);
    }
}

impl BlobPersistence for SlowBlobStore {
    fn get_blob(&self, hash: &Hash) -> Option<Vec<u8>> {
        self.enter();
        self.blobs.lock().unwrap().get(hash).cloned()
    }

    fn has_blob(&self, hash: &Hash) -> bool {
        self.enter();
        self.blobs.lock().unwrap().contains_key(hash)
    }

    fn persist_blob(&self, hash: &Hash, bytes: &[u8]) -> Result<(), PersistBlobError> {
        self.enter();
        self.blobs.lock().unwrap().insert(*hash, bytes.to_vec());
        Ok(())
    }
}

/// Presign-capable fake whose resolution/verification ops block for
/// `delay` — the shape of the 6.1 S3 bridge residue (presign in Delegate
/// mode is a real HTTP round trip; `verify_uploaded` is a bucket HEAD).
#[derive(Debug)]
struct SlowPresignStore {
    delay: Duration,
    ops_entered: Arc<AtomicUsize>,
}

impl SlowPresignStore {
    fn new(delay: Duration) -> Self {
        Self {
            delay,
            ops_entered: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn enter(&self) {
        self.ops_entered.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(self.delay);
    }
}

impl BlobPersistence for SlowPresignStore {
    fn persist_blob(&self, _hash: &Hash, _bytes: &[u8]) -> Result<(), PersistBlobError> {
        Ok(())
    }

    fn resolve_get_url(
        &self,
        _room_id: &str,
        _hash: &Hash,
        _size_hint: Option<u64>,
    ) -> Option<PresignedUrl> {
        self.enter();
        Some(PresignedUrl::with_ttl(
            "https://example.invalid/presigned",
            Duration::from_secs(60),
        ))
    }

    fn verify_uploaded(&self, _room_id: &str, _hash: &Hash) -> Result<(), String> {
        self.enter();
        Ok(())
    }

    fn supports_presigned_urls(&self) -> bool {
        true
    }
}

/// A store whose read path panics — drives the "panic in the blocking task
/// must map to 500, not a hang or silent success" pin.
#[derive(Debug)]
struct PanickingBlobStore;

impl BlobPersistence for PanickingBlobStore {
    fn get_blob(&self, _hash: &Hash) -> Option<Vec<u8>> {
        panic!("deliberate test panic in the blocking store op");
    }

    fn persist_blob(&self, _hash: &Hash, _bytes: &[u8]) -> Result<(), PersistBlobError> {
        Ok(())
    }
}

fn router_with(blob_half: impl BlobPersistence + 'static, key_seed: u8) -> Router {
    let persistence: SharedPersistence = Arc::new(Composite::new(NoPersistence, blob_half));
    let rooms = Rooms::new(
        SigningKey::from_bytes(&[key_seed; 32]),
        persistence,
        512,
        0,
        0,
    );
    blob_http::blob_routes(BlobHttpConfig::default()).with_state(rooms)
}

/// Shared starvation probe. Spawns `CONCURRENT_OPS` requests on a
/// `WORKERS`-thread runtime, waits (spinning on the `block_on` thread,
/// which is not a runtime worker) until the store reports at least
/// `WORKERS` ops blocking in flight, then races the heartbeat and asserts
/// it beats `HEARTBEAT_BOUND`. Finally drains every request and asserts
/// its status, so the probe doubles as a contract check under the new
/// scheduling.
fn run_starvation_probe(
    router: Router,
    make_req: impl Fn(usize) -> Request<Body>,
    ops_entered: Arc<AtomicUsize>,
    expect_status: StatusCode,
    probe_name: &str,
) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(WORKERS)
        .enable_all()
        .build()
        .expect("test runtime");
    rt.block_on(async {
        let mut ops = Vec::new();
        for i in 0..CONCURRENT_OPS {
            let router = router.clone();
            let req = make_req(i);
            ops.push(tokio::spawn(async move {
                router.oneshot(req).await.expect("infallible").status()
            }));
        }

        // Starting gun: don't measure until the blocking is genuinely in
        // flight, so the pre-fix failure never depends on spawn ordering.
        let spin_start = Instant::now();
        while ops_entered.load(Ordering::SeqCst) < WORKERS {
            assert!(
                spin_start.elapsed() < Duration::from_secs(10),
                "{probe_name}: blob ops never reached the store"
            );
            std::thread::sleep(Duration::from_millis(1));
        }

        let t0 = Instant::now();
        let heartbeat = tokio::spawn(async {
            for _ in 0..3 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });
        heartbeat.await.expect("heartbeat task panicked");
        let heartbeat_elapsed = t0.elapsed();

        for op in ops {
            assert_eq!(
                op.await.expect("blob op task panicked"),
                expect_status,
                "{probe_name}: blob op status changed under concurrency"
            );
        }

        assert!(
            heartbeat_elapsed <= HEARTBEAT_BOUND,
            "{probe_name}: heartbeat took {heartbeat_elapsed:?} (bound {HEARTBEAT_BOUND:?}) — \
             blob ops are parking the async workers"
        );
    });
}

#[test]
fn concurrent_gets_on_a_slow_store_do_not_starve_the_runtime() {
    let store = SlowBlobStore::new(STORE_DELAY);
    let hash_hex = store.seed(b"offload-probe-blob").to_hex();
    let ops_entered = Arc::clone(&store.ops_entered);
    let router = router_with(store, 0x62);
    run_starvation_probe(
        router,
        |_| {
            Request::builder()
                .method(Method::GET)
                .uri(format!("/blobs/{hash_hex}"))
                .body(Body::empty())
                .unwrap()
        },
        ops_entered,
        StatusCode::OK,
        "GET",
    );
}

#[test]
fn concurrent_puts_do_not_starve_the_runtime() {
    let store = SlowBlobStore::new(STORE_DELAY);
    let ops_entered = Arc::clone(&store.ops_entered);
    let router = router_with(store, 0x63);
    run_starvation_probe(
        router,
        |i| {
            // Unique bodies: every PUT takes the full has_blob(false) →
            // persist_blob path (two blocking store ops each).
            let body = format!("put-probe-{i}").into_bytes();
            let hash_hex = Hash::of(&body).to_hex();
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/blobs/{hash_hex}"))
                .body(Body::from(body))
                .unwrap()
        },
        ops_entered,
        StatusCode::CREATED,
        "PUT",
    );
}

#[test]
fn concurrent_url_resolutions_do_not_starve_the_runtime() {
    let store = SlowPresignStore::new(STORE_DELAY);
    let ops_entered = Arc::clone(&store.ops_entered);
    let router = router_with(store, 0x64);
    let hash_hex = Hash::of(b"url-probe").to_hex();
    run_starvation_probe(
        router,
        |_| {
            Request::builder()
                .method(Method::GET)
                .uri(format!("/blobs/{hash_hex}/url?op=get"))
                .body(Body::empty())
                .unwrap()
        },
        ops_entered,
        StatusCode::OK,
        "URL-RESOLVE",
    );
}

/// A panic inside the offloaded store op must come back as a 500 with the
/// module's JSON error shape — pre-fix the panic unwound straight through
/// the handler poll (tearing down whatever was driving the connection);
/// post-fix `spawn_blocking` contains it and the JoinError maps to 500.
#[tokio::test]
async fn panic_in_the_blocking_store_op_maps_to_500() {
    let router = router_with(PanickingBlobStore, 0x65);
    let hash_hex = Hash::of(b"panic-probe").to_hex();
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/blobs/{hash_hex}"))
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.expect("infallible");
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("500 body is JSON");
    assert_eq!(json, serde_json::json!({"error": "internal error"}));
}

/// Green-side regression guard (passes pre-fix too — honest: this one is
/// NOT a RED): upload-confirm still answers 200 and the /uploaded contract
/// survives being moved onto the blocking pool.
#[tokio::test]
async fn upload_confirm_still_succeeds_through_a_slow_backend() {
    let router = router_with(SlowPresignStore::new(Duration::from_millis(50)), 0x66);
    let hash_hex = Hash::of(b"uploaded-probe").to_hex();
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/blobs/{hash_hex}/uploaded"))
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.expect("infallible");
    assert_eq!(resp.status(), StatusCode::OK);
}
