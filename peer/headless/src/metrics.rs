use std::net::SocketAddr;

use metrics::{describe_counter, describe_histogram, describe_gauge, Unit};
use metrics_exporter_prometheus::PrometheusBuilder;

use crate::report::WorkerSessionReport;

pub fn init(addr: SocketAddr) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let sync_buckets = [
        0.000_5, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5,
    ];
    let total_buckets = [0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0];

    PrometheusBuilder::new()
        .with_http_listener(addr)
        .set_buckets_for_metric(
            metrics_exporter_prometheus::Matcher::Full(
                "nodalmerge_headless_websocket_sync_seconds".to_string(),
            ),
            &sync_buckets,
        )?
        .set_buckets_for_metric(
            metrics_exporter_prometheus::Matcher::Full(
                "nodalmerge_headless_session_total_seconds".to_string(),
            ),
            &total_buckets,
        )?
        .install()?;

    describe_counter!(
        "nodalmerge_headless_sessions_total",
        "Headless session outcomes. Labels: backend, durable, outcome."
    );
    describe_counter!(
        "nodalmerge_headless_packs_applied_total",
        "Packs applied by headless sessions. Labels: backend, durable."
    );
    describe_counter!(
        "nodalmerge_headless_mst_requests_total",
        "MST requests issued by headless sessions. Labels: backend, durable."
    );
    describe_counter!(
        "nodalmerge_headless_mst_nodes_fetched_total",
        "MST nodes fetched by headless sessions. Labels: backend, durable."
    );
    describe_gauge!(
        "nodalmerge_headless_nodes_persisted",
        "Nodes persisted in peer-local state after a session. Labels: backend, durable."
    );
    describe_histogram!(
        "nodalmerge_headless_websocket_sync_seconds",
        Unit::Seconds,
        "Websocket sync phase duration. Labels: backend, durable."
    );
    describe_histogram!(
        "nodalmerge_headless_session_total_seconds",
        Unit::Seconds,
        "Total session duration. Labels: backend, durable."
    );
    Ok(())
}

pub fn parse_metrics_arg(args: &[String]) -> Option<SocketAddr> {
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

pub fn record_session(report: &WorkerSessionReport) {
    let backend = report.backend.clone();
    let durable = if report.durable { "true" } else { "false" }.to_string();
    let outcome = if report.saw_welcome { "ok" } else { "handshake_missing_welcome" }.to_string();

    metrics::counter!(
        "nodalmerge_headless_sessions_total",
        "backend" => backend.clone(),
        "durable" => durable.clone(),
        "outcome" => outcome
    )
    .increment(1);
    metrics::counter!(
        "nodalmerge_headless_packs_applied_total",
        "backend" => backend.clone(),
        "durable" => durable.clone()
    )
    .increment(report.packs_applied as u64);
    metrics::counter!(
        "nodalmerge_headless_mst_requests_total",
        "backend" => backend.clone(),
        "durable" => durable.clone()
    )
    .increment(report.mst_requests as u64);
    metrics::counter!(
        "nodalmerge_headless_mst_nodes_fetched_total",
        "backend" => backend.clone(),
        "durable" => durable.clone()
    )
    .increment(report.mst_nodes_fetched as u64);

    metrics::gauge!(
        "nodalmerge_headless_nodes_persisted",
        "backend" => backend.clone(),
        "durable" => durable.clone()
    )
    .set(report.nodes_persisted_total as f64);
    metrics::histogram!(
        "nodalmerge_headless_websocket_sync_seconds",
        "backend" => backend.clone(),
        "durable" => durable.clone()
    )
    .record(report.timings_ms.websocket_sync_ms as f64 / 1000.0);
    metrics::histogram!(
        "nodalmerge_headless_session_total_seconds",
        "backend" => backend,
        "durable" => durable
    )
    .record(report.timings_ms.total_ms as f64 / 1000.0);
}
