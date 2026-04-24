//! G7: Prometheus metrics for the server.
//!
//! The exporter installs a global recorder and spins up a small HTTP listener
//! on a dedicated admin address (typically `127.0.0.1:9090`). The public WS
//! port never serves metrics — keeping the two surfaces separate means you
//! can bind the admin port to loopback or a private VPC subnet and leave the
//! WS port internet-facing.
//!
//! **Registered metrics** (cardinality-conscious: `room` label only where it
//! buys us something; `peer` label is a 12-char pubkey prefix so a runaway
//! peer churn can't explode the series count):
//!
//! | Metric | Kind | Labels | Source gap |
//! |---|---|---|---|
//! | `activesync_rooms_total` | gauge | — | baseline |
//! | `activesync_peers_total` | gauge | `room` | baseline |
//! | `activesync_nodes_accepted_total` | counter | `room` | baseline |
//! | `activesync_merge_batch_seconds` | histogram | — | baseline |
//! | `activesync_persistence_write_seconds` | histogram | `kind` | baseline |
//! | `activesync_eviction_total` | counter | — | F4-followup |
//! | `activesync_broadcast_lagged_total` | counter | `room` | G1 |
//! | `activesync_ws_send_timeout_total` | counter | `room` | G1 |
//! | `activesync_rate_limit_drops_total` | counter | `peer` | G3 |
//!
//! Subsequent gaps (G4/G5/G6) will register their counters at their own
//! instrumentation sites; describing the whole list here keeps the doc in
//! one place.

use std::net::SocketAddr;

use metrics::{describe_counter, describe_gauge, describe_histogram, Unit};
use metrics_exporter_prometheus::PrometheusBuilder;

/// Install the global recorder and start the `/metrics` HTTP listener on
/// `addr`. Call exactly once, early in `main`.
///
/// Returns an error if the builder fails to install (already-installed
/// recorder, bind failure, etc.). Callers typically log-and-ignore so the
/// server still starts without observability.
pub fn init(addr: SocketAddr) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Merge-batch latency: the main server hot path. Buckets cover
    // sub-millisecond single-node packs up through the 10k-node catchup
    // bench (~31 ms on the reference machine).
    let merge_buckets = [
        0.000_05, 0.000_1, 0.000_25, 0.000_5,
        0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5,
    ];
    // Persistence writes: typically <1 ms for a node INSERT, slightly more
    // for a blob rename. Reuse merge-ish buckets; anything >100 ms is an
    // alerting signal.
    let persist_buckets = [
        0.000_1, 0.000_25, 0.000_5,
        0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5,
    ];

    PrometheusBuilder::new()
        .with_http_listener(addr)
        .set_buckets_for_metric(
            metrics_exporter_prometheus::Matcher::Full(
                "activesync_merge_batch_seconds".to_string(),
            ),
            &merge_buckets,
        )?
        .set_buckets_for_metric(
            metrics_exporter_prometheus::Matcher::Full(
                "activesync_persistence_write_seconds".to_string(),
            ),
            &persist_buckets,
        )?
        .install()?;

    // Describe the baseline set up front so they appear in `/metrics` before
    // the first sample is recorded. Gap-specific metrics (G1/G3/…) describe
    // themselves at their instrumentation sites.
    describe_gauge!(
        "activesync_rooms_total",
        "Number of rooms currently held in the registry (including idle)."
    );
    describe_gauge!(
        "activesync_peers_total",
        "Connected WS peers per room."
    );
    describe_counter!(
        "activesync_nodes_accepted_total",
        "Total nodes accepted by `import_nodes` (post-verify, post-policy)."
    );
    describe_histogram!(
        "activesync_merge_batch_seconds",
        Unit::Seconds,
        "Wall time spent inside `import_nodes` per call (one call = one pack)."
    );
    describe_histogram!(
        "activesync_persistence_write_seconds",
        Unit::Seconds,
        "Wall time spent in `ServerPersistence::persist_*`. Labeled by `kind` (`node`/`nodes_batch`/`blob`)."
    );
    describe_counter!(
        "activesync_eviction_total",
        "Rooms evicted by the idle sweeper (F4 follow-up)."
    );
    Ok(())
}

/// Parse `--metrics-addr <addr>` / `--metrics-addr=<addr>`. Returns
/// `None` when absent (metrics disabled). Invalid values log and return
/// `None` so a typo doesn't take the server down.
pub fn parse_arg(args: &[String]) -> Option<SocketAddr> {
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        let raw = if a == "--metrics-addr" {
            args.get(i + 1).map(|s| s.as_str())
        } else if let Some(v) = a.strip_prefix("--metrics-addr=") {
            Some(v)
        } else {
            None
        };
        if let Some(s) = raw {
            return match s.parse::<SocketAddr>() {
                Ok(a) => Some(a),
                Err(e) => {
                    eprintln!("warning: --metrics-addr {s:?} is not a valid socket address ({e}); metrics disabled");
                    None
                }
            };
        }
        i += 1;
    }
    None
}

/// 12-char pubkey prefix for the `peer` label. Keeps cardinality bounded
/// while still letting operators triage which peer is misbehaving via the
/// server logs (which log the same prefix via `tracing`).
pub fn peer_label(pubkey_hex: &str) -> String {
    pubkey_hex.chars().take(12).collect()
}
