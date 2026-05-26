//! G7 integration test — Prometheus `/metrics` endpoint.
//!
//! Installs the global recorder on a loopback admin port, drives a tiny bit
//! of room activity, and scrapes the endpoint via a raw HTTP/1.1 request.
//!
//! **Single-install caveat:** `metrics::init` installs a global recorder
//! process-wide. A second install in the same process returns `Err`. Keep
//! this crate's metrics coverage in *one* integration test file (this one)
//! so no two tests race for the slot.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nodalmerge_core::{MapOp, Op, StateGraph};
use nodalmerge_server::metrics as server_metrics;
use nodalmerge_server::room::{import_nodes, Rooms};
use nodalmerge_server::store::{NoPersistence, SharedPersistence};
use ed25519_dalek::SigningKey;

fn make_node(sk: &SigningKey, key: &str, val: &[u8]) -> nodalmerge_core::SyncNode {
    let mut g = StateGraph::new();
    let id = g
        .apply_local(sk, 0, vec![Op::Map(MapOp::Set { key: key.into(), value: val.to_vec() })])
        .unwrap();
    g.get_nodes(&[id]).into_iter().next().unwrap().clone()
}

/// Scrape `/metrics` via a minimal HTTP/1.1 GET. Retries for up to ~2s so a
/// just-bound listener has time to come up.
fn scrape(addr: SocketAddr) -> String {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut last_err: Option<String> = None;
    while Instant::now() < deadline {
        match TcpStream::connect_timeout(&addr, Duration::from_millis(200)) {
            Ok(mut sock) => {
                sock.set_read_timeout(Some(Duration::from_secs(2))).ok();
                let req = format!(
                    "GET /metrics HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
                );
                if let Err(e) = sock.write_all(req.as_bytes()) {
                    last_err = Some(format!("write: {e}"));
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
                let mut buf = String::new();
                if let Err(e) = sock.read_to_string(&mut buf) {
                    last_err = Some(format!("read: {e}"));
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
                return buf;
            }
            Err(e) => {
                last_err = Some(format!("connect: {e}"));
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    panic!("scrape {addr} failed: {last_err:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metrics_endpoint_exposes_baseline_series() {
    // Pick a fixed loopback port. If another instance of the test binary is
    // running (shouldn't happen — cargo test serializes by default across
    // a single integration target), the bind will fail and init returns Err
    // — we treat that as fatal for this test.
    let addr: SocketAddr = "127.0.0.1:19999".parse().unwrap();
    server_metrics::init(addr).expect("metrics init must succeed exactly once per process");

    // Drive some activity so the baseline series have recorded values.
    let persistence: SharedPersistence = Arc::new(NoPersistence);
    let server_key = SigningKey::from_bytes(&[0x42u8; 32]);
    let rooms = Rooms::new(server_key, Arc::clone(&persistence), 512, 0, 0);
    let room = rooms.get_or_create("metrics-test").await;
    room.register_peer("deadbeef".into()).await;

    let peer_key = SigningKey::from_bytes(&[0x55u8; 32]);
    let n = make_node(&peer_key, "k", b"v");
    let (accepted, _, errs) = import_nodes(&room, vec![n]).await;
    assert_eq!(accepted, 1, "import_nodes should accept 1 node (errors: {errs:?})");

    room.deregister_peer("deadbeef").await;

    // Give the exporter a moment to flush recent samples before we scrape.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Scrape on a blocking thread so the synchronous TCP read doesn't starve
    // the runtime the exporter is serving requests on.
    let body = tokio::task::spawn_blocking(move || scrape(addr))
        .await
        .expect("scrape task panicked");

    // Status line.
    assert!(body.starts_with("HTTP/1.1 200"), "expected 200 OK, got: {body:?}");

    // Baseline metrics must appear. We check for metric *names* rather than
    // specific values to stay robust to exporter formatting changes.
    for name in [
        "nodalmerge_rooms_total",
        "nodalmerge_peers_total",
        "nodalmerge_nodes_accepted_total",
        "nodalmerge_merge_batch_seconds",
        "activesync_rooms_total",
        "activesync_peers_total",
        "activesync_nodes_accepted_total",
        "activesync_merge_batch_seconds",
    ] {
        assert!(
            body.contains(name),
            "expected `{name}` in /metrics body, got:\n{body}"
        );
    }
    // `# HELP` lines come from the describe_* calls.
    assert!(body.contains("# HELP"), "expected HELP lines in /metrics body");
}

#[test]
fn parse_arg_accepts_flag_and_equals_form() {
    let long = vec![
        "nodalmerge-server".into(),
        "--metrics-addr".into(),
        "127.0.0.1:9091".into(),
    ];
    assert_eq!(
        server_metrics::parse_arg(&long),
        Some("127.0.0.1:9091".parse().unwrap())
    );

    let eq = vec![
        "nodalmerge-server".into(),
        "--metrics-addr=127.0.0.1:9092".into(),
    ];
    assert_eq!(
        server_metrics::parse_arg(&eq),
        Some("127.0.0.1:9092".parse().unwrap())
    );

    let missing: Vec<String> = vec!["nodalmerge-server".into()];
    assert_eq!(server_metrics::parse_arg(&missing), None);

    let bogus = vec![
        "nodalmerge-server".into(),
        "--metrics-addr=not-an-addr".into(),
    ];
    assert_eq!(server_metrics::parse_arg(&bogus), None);
}

#[test]
fn peer_label_truncates_to_12_chars() {
    assert_eq!(server_metrics::peer_label("0123456789abcdef0123"), "0123456789ab");
    assert_eq!(server_metrics::peer_label("short"), "short");
}

