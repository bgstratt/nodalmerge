//! blob-cas-remediation.md slice 6.1 (finding #11) — bounded waits on every
//! sync→async bridge in `S3BlobStore`, and timeout-classified-as-backend-error
//! (never "missing") on every surface GC can reach.
//!
//! The adversary here is an endpoint that **accepts the TCP connection and
//! then never says another byte** — a hung bucket, a wedged delegate presign
//! service, a half-dead NAT. Pre-6.1 every S3 op blocked the calling thread
//! on a timeout-less `mpsc::recv()`, so that adversary pinned a tokio worker
//! forever; 2.2/2.3 made the traffic per-blob-per-GC-sweep and
//! per-file-per-export, so one such endpoint wedged the whole server.
//!
//! Every test wraps the op in its own watchdog thread + `recv_timeout` so the
//! *test* stays bounded even when the op under test hangs — pre-fix, that
//! watchdog firing IS the failure ("op still running"), which makes these
//! genuine behavioral REDs, not structure checks.
//!
//! No Docker, no MinIO: the stalled endpoint is a plain in-test
//! `std::net::TcpListener`. CI: blob-layout-parity.yml.

use std::net::TcpListener;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use nodalmerge_core::Hash;
use nodalmerge_s3_blobs::{S3Auth, S3BlobObjectStore, S3BlobStore, S3BlobStoreConfig};
use nodalmerge_server::store::{BlobPersistence, HydrateError};

/// How long the watchdog gives the op before declaring it hung. Must sit
/// above the store's configured op timeout (plus the bridge's recv margin)
/// so the op's own, better-labelled timeout always fires first — the same
/// layering rule the production bridge itself follows.
const TEST_BOUND: Duration = Duration::from_secs(10);

/// Short per-op timeout for these tests, well under `TEST_BOUND`. Long
/// enough that connect + request dispatch to a localhost listener is never
/// the thing that times out; short enough the suite stays fast.
const OP_TIMEOUT: Duration = Duration::from_millis(750);

/// Bind a listener that accepts connections and then holds them open
/// forever without responding — the "hung bucket / hung delegate" shape.
/// Sockets are parked in a leaked thread; the test process's exit reaps it.
fn stalled_endpoint() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stalled endpoint");
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

fn direct_cfg_against(endpoint: &str) -> S3BlobStoreConfig {
    let mut cfg = S3BlobStoreConfig::default();
    cfg.bucket = "stalled-bucket".into();
    cfg.endpoint = Some(endpoint.to_string());
    cfg.require_https = false;
    cfg.auth = S3Auth::direct_explicit("ak", "sk");
    cfg.op_timeout = OP_TIMEOUT;
    cfg.connect_timeout = Duration::from_millis(500);
    // The whole-sweep bound; still comfortably under TEST_BOUND so the
    // sweep test exercises the production timeout, not the watchdog.
    cfg.sweep_timeout = Duration::from_secs(3);
    cfg
}

fn delegate_cfg_against(endpoint: &str) -> S3BlobStoreConfig {
    let mut cfg = S3BlobStoreConfig::default();
    cfg.bucket = "stalled-bucket".into();
    cfg.auth = S3Auth::delegate(format!("{endpoint}/presign"), None);
    cfg.op_timeout = OP_TIMEOUT;
    cfg.connect_timeout = Duration::from_millis(500);
    cfg
}

/// Run `op` on its own thread and insist it *completes* within
/// [`TEST_BOUND`]. Pre-6.1 the ops under test never complete at all, so
/// this returning `Err` is exactly finding #11 reproduced.
fn bounded<T: Send + 'static>(op: impl FnOnce() -> T + Send + 'static) -> Result<(T, Duration), ()> {
    let (tx, rx) = mpsc::channel();
    let started = Instant::now();
    std::thread::spawn(move || {
        let _ = tx.send(op());
    });
    rx.recv_timeout(TEST_BOUND).map(|v| (v, started.elapsed())).map_err(|_| ())
}

/// The delegate presign path against a wedged endpoint must fail the op and
/// fall back to WS within the configured timeout — never pin the caller.
/// Pre-6.1: `reqwest::Client::new()` (no timeout) + `mpsc::recv()` (no
/// timeout) — the op below simply never returned and this test failed on
/// the watchdog.
#[test]
fn stalled_delegate_endpoint_fails_the_op_within_the_timeout() {
    let endpoint = stalled_endpoint();
    let store = S3BlobStore::new(delegate_cfg_against(&endpoint)).unwrap();

    let out = bounded(move || store.resolve_get_url("room-1", &Hash::of(b"blob"), None));
    let (url, took) = out.unwrap_or_else(|_| {
        panic!(
            "finding #11: resolve_get_url against a stalled delegate endpoint was still \
             running after {TEST_BOUND:?} — the sync→async bridge / delegate HTTP client \
             has no bounded wait"
        )
    });
    assert!(url.is_none(), "a timed-out presign must fall back to WS (None), got {url:?}");
    assert!(
        took >= OP_TIMEOUT,
        "op returned in {took:?}, faster than the {OP_TIMEOUT:?} op timeout — the endpoint \
         wasn't actually stalled, so this test proved nothing"
    );
}

/// Same adversary on the Direct PUT-presign path (presigning is local key
/// signing for real S3, but `resolve_put_url`'s Delegate arm does a network
/// roundtrip — cover it too).
#[test]
fn stalled_delegate_endpoint_fails_put_presign_within_the_timeout() {
    let endpoint = stalled_endpoint();
    let store = S3BlobStore::new(delegate_cfg_against(&endpoint)).unwrap();

    let out = bounded(move || {
        store.resolve_put_url("room-1", &Hash::of(b"blob"), 64 * 1024 * 1024, None)
    });
    let (url, _) = out.unwrap_or_else(|_| {
        panic!(
            "finding #11: resolve_put_url against a stalled delegate endpoint was still \
             running after {TEST_BOUND:?}"
        )
    });
    assert!(url.is_none(), "a timed-out presign must fall back to WS (None), got {url:?}");
}

/// **The load-bearing classification test.** `hydrate_blob` is the seam GC's
/// tree walk reads through (slice 2.2). A hung bucket must surface as
/// `HydrateError::Backend` — retryable, "says nothing about whether the
/// object exists" — and categorically never as `Missing`, which GC treats
/// as a fact about the data.
#[test]
fn stalled_direct_endpoint_hydrate_blob_is_backend_error_never_missing() {
    let endpoint = stalled_endpoint();
    let store = S3BlobStore::new(direct_cfg_against(&endpoint)).unwrap();

    let out = bounded(move || store.hydrate_blob(&Hash::of(b"tree object")));
    let (res, _) = out.unwrap_or_else(|_| {
        panic!(
            "finding #11: hydrate_blob against a stalled bucket was still running after \
             {TEST_BOUND:?} — a hung bucket wedges the GC tree walk"
        )
    });
    match res {
        Err(HydrateError::Backend(msg)) => {
            assert!(
                msg.to_lowercase().contains("timed out") || msg.to_lowercase().contains("timeout"),
                "backend error should say it was a timeout, got: {msg}"
            );
        }
        Err(HydrateError::Missing) => panic!(
            "a bucket timeout surfaced as HydrateError::Missing — GC would treat a hung \
             bucket as lost data. This is the exact misclassification slice 6.1 forbids."
        ),
        other => panic!("expected HydrateError::Backend, got {other:?}"),
    }
}

/// The GC hard-sweep's existence probe (`BlobObjectStore::head`, S5.3) can
/// carry an error — so a timeout there must BE an error, never `Ok(false)`:
/// for liveness-relevant reads, "I don't know" must never become "not
/// live"/"absent", or a hung bucket lets the sweep treat objects as gone.
#[test]
fn stalled_direct_endpoint_gc_head_is_backend_error_never_absent() {
    use nodalmerge_gc::contracts::BlobObjectStore as _;

    let endpoint = stalled_endpoint();
    let store = S3BlobObjectStore::new(direct_cfg_against(&endpoint)).unwrap();

    let out = bounded(move || store.head("stalled-bucket", "blobs/blake3/00ff"));
    let (res, _) = out.unwrap_or_else(|_| {
        panic!(
            "finding #11: GC hard-sweep HEAD against a stalled bucket was still running \
             after {TEST_BOUND:?}"
        )
    });
    match res {
        Err(nodalmerge_gc::GcError::Backend(_)) => {}
        Ok(exists) => panic!(
            "a bucket timeout surfaced as Ok({exists}) from the GC head probe — \
             'I don't know' must never become an existence answer"
        ),
        Err(other) => panic!("expected GcError::Backend, got {other:?}"),
    }
}

/// `verify_uploaded` already has an error channel (`Err(String)`); a timeout
/// must take it, not the `Ok(false)` → "object missing after upload" path
/// that would tell the uploader its bytes vanished.
#[test]
fn stalled_direct_endpoint_verify_uploaded_reports_backend_failure_not_missing() {
    let endpoint = stalled_endpoint();
    let store = S3BlobStore::new(direct_cfg_against(&endpoint)).unwrap();

    let out = bounded(move || store.verify_uploaded("room-1", &Hash::of(b"blob")));
    let (res, _) = out.unwrap_or_else(|_| {
        panic!(
            "finding #11: verify_uploaded against a stalled bucket was still running \
             after {TEST_BOUND:?}"
        )
    });
    let err = res.expect_err("a hung HEAD must be an error, not a verified upload");
    assert!(
        err.contains("HEAD failed"),
        "timeout must report as a HEAD failure, never as 'object missing after upload'; got: {err}"
    );
    assert!(
        !err.contains("object missing after upload"),
        "timeout misclassified as missing-after-upload: {err}"
    );
}

/// `has_blob`'s signature (`-> bool`) cannot carry an error. Its only
/// callers are the blob HTTP origin's existence probes (HEAD 404 / PUT
/// dedupe short-circuit) — NOT GC liveness — so the conservative value on
/// "I don't know" is `false`: a spurious 404 makes a client fall back to
/// WS or re-PUT (idempotent under content addressing), while a spurious
/// `true` would 200 a HEAD for bytes we can't serve. This test pins the
/// *bounded* half: the answer must arrive within the timeout, not never.
#[test]
fn stalled_direct_endpoint_has_blob_answers_false_within_the_timeout() {
    let endpoint = stalled_endpoint();
    let store = S3BlobStore::new(direct_cfg_against(&endpoint)).unwrap();

    let out = bounded(move || store.has_blob(&Hash::of(b"blob")));
    let (exists, _) = out.unwrap_or_else(|_| {
        panic!(
            "finding #11: has_blob against a stalled bucket was still running after \
             {TEST_BOUND:?}"
        )
    });
    assert!(!exists, "a hung bucket must not report existence");
}

/// `blob_gc_sweep` against a hung bucket must return 0 within the bridge
/// bound — deleting nothing is the conservative sweep outcome, identical to
/// a crash mid-sweep, which the two-phase tombstone protocol already
/// tolerates (slice 1.3).
#[test]
fn stalled_direct_endpoint_gc_sweep_deletes_nothing_and_returns_within_the_timeout() {
    let endpoint = stalled_endpoint();
    let store = S3BlobStore::new(direct_cfg_against(&endpoint)).unwrap();

    let out = bounded(move || {
        store.blob_gc_sweep(&std::collections::HashSet::new(), Duration::from_secs(3600))
    });
    let (deleted, _) = out.unwrap_or_else(|_| {
        panic!(
            "finding #11: blob_gc_sweep against a stalled bucket was still running after \
             {TEST_BOUND:?}"
        )
    });
    assert_eq!(deleted, 0, "a timed-out sweep must report zero deletions");
}
