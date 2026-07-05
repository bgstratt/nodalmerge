//! `nodalmerge-headless` — headless peer worker for pods and workstations.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use nodalmerge_headless::metrics::{init as init_metrics, parse_metrics_arg, record_session};
use nodalmerge_headless::{run_worker_session, WorkerConfig, WorkerSessionReport};
use nodalmerge_runtime_local::{parse_backend_kind, PersistenceHandle, PeerLocalPersistence};
use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("nodalmerge_headless=info".parse().unwrap()),
        )
        .init();

    if let Err(e) = run_from_env() {
        eprintln!("nodalmerge-headless: {e}");
        std::process::exit(1);
    }
}

fn run_from_env() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return Ok(());
    }

    if args.iter().any(|a| a == "--health") {
        return run_health_probe();
    }

    let server_ws_url = env_or_flag(&args, "NODALMERGE_HEADLESS_SERVER_URL", "--server-url")?;
    let room_id = env_or_flag(&args, "NODALMERGE_HEADLESS_ROOM", "--room")?;
    let backend_name = std::env::var("NODALMERGE_HEADLESS_BACKEND")
        .unwrap_or_else(|_| "memory".to_string());
    let data_dir = env_or_flag_opt(&args, "NODALMERGE_HEADLESS_DATA_DIR", "--data-dir")
        .map(PathBuf::from);
    let run_secs: u64 = std::env::var("NODALMERGE_HEADLESS_RUN_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let negotiate_ibf = env_bool("NODALMERGE_HEADLESS_NEGOTIATE_IBF", true);
    let negotiate_mst = env_bool("NODALMERGE_HEADLESS_NEGOTIATE_MST", true);
    let report_json = env_or_flag_opt(&args, "NODALMERGE_HEADLESS_REPORT_JSON", "--report-json");
    let metrics_addr = std::env::var("NODALMERGE_HEADLESS_METRICS_ADDR")
        .ok()
        .and_then(|s| s.parse::<std::net::SocketAddr>().ok())
        .or_else(|| parse_metrics_arg(&args));

    if let Some(addr) = metrics_addr {
        if let Err(e) = init_metrics(addr) {
            tracing::warn!(%addr, error = %e, "failed to initialize metrics exporter");
        } else {
            tracing::info!(%addr, "metrics endpoint listening on http://{addr}/metrics");
        }
    }

    let backend = parse_backend_kind(&backend_name, data_dir)?;
    let durable = PersistenceHandle::open(backend.clone())
        .map_err(|e| format!("open persistence for durable probe: {e}"))?
        .is_durable();

    let cfg = WorkerConfig {
        server_ws_url,
        room_id,
        backend,
        run_for: Duration::from_secs(run_secs),
        negotiate_ibf,
        negotiate_mst,
    };
    let cfg_snapshot = cfg.clone();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    let report = rt.block_on(run_worker_session(cfg))?;

    println!(
        "nodalmerge-headless: welcome={} packs={} mst_requests={} mst_nodes={} ibf={} nodes={} canonical_hash={} total_ms={}",
        report.saw_welcome,
        report.packs_applied,
        report.mst_requests,
        report.mst_nodes_fetched,
        report.used_ibf,
        report.nodes_persisted_total,
        report.canonical_hash_hex,
        report.timings_ms.total_ms,
    );

    let session = WorkerSessionReport::from_session(&cfg_snapshot, &report, durable);
    record_session(&session);

    if let Some(path) = report_json {
        let json = serde_json::to_string_pretty(&session).map_err(|e| e.to_string())?;
        if path == "-" {
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(json.as_bytes()).map_err(|e| e.to_string())?;
            stdout.write_all(b"\n").map_err(|e| e.to_string())?;
        } else {
            fs::write(&path, format!("{json}\n")).map_err(|e| format!("write {path}: {e}"))?;
            tracing::info!(path = %path, "wrote session report JSON");
        }
    }

    if !report.saw_welcome {
        return Err("handshake did not receive welcome".to_string());
    }
    Ok(())
}

fn env_or_flag(args: &[String], env: &str, flag: &str) -> Result<String, String> {
    if let Ok(v) = std::env::var(env) {
        if !v.is_empty() {
            return Ok(v);
        }
    }
    flag_value(args, flag).ok_or_else(|| format!("missing {env} or {flag}"))
}

fn env_or_flag_opt(args: &[String], env: &str, flag: &str) -> Option<String> {
    std::env::var(env)
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| flag_value(args, flag))
}

fn env_bool(name: &str, default: bool) -> bool {
    std::env::var(name)
        .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
        .unwrap_or(default)
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn run_health_probe() -> Result<(), String> {
    let backend_name = std::env::var("NODALMERGE_HEADLESS_BACKEND")
        .unwrap_or_else(|_| "memory".to_string());
    let data_dir = std::env::var("NODALMERGE_HEADLESS_DATA_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    let backend = parse_backend_kind(&backend_name, data_dir)?;
    let durable = PersistenceHandle::open(backend.clone())
        .map_err(|e| format!("open persistence: {e}"))?
        .is_durable();
    println!(
        "{{\"artifact\":\"nodalmerge-headless-health\",\"backend\":\"{}\",\"durable\":{}}}",
        backend.label(),
        durable
    );
    Ok(())
}

fn print_help() {
    eprintln!(
        r"nodalmerge-headless — headless peer worker

Usage:
  nodalmerge-headless [--health] [--server-url URL] [--room ROOM] [--data-dir PATH] [--report-json PATH]

Environment:
  NODALMERGE_HEADLESS_SERVER_URL      websocket URL (ws://host:port/ws/room-id)
  NODALMERGE_HEADLESS_ROOM            room id
  NODALMERGE_HEADLESS_BACKEND         memory | file | embedded | composite  (default memory)
  NODALMERGE_HEADLESS_DATA_DIR        peer-local data directory (required for file)
  NODALMERGE_HEADLESS_RUN_SECS        catch-up window seconds (default 10)
  NODALMERGE_HEADLESS_NEGOTIATE_IBF   1/true to send IBF in hello (default on)
  NODALMERGE_HEADLESS_NEGOTIATE_MST   1/true for MST descent after welcome (default on)
  NODALMERGE_HEADLESS_REPORT_JSON     write machine-readable session report (use - for stdout)
  NODALMERGE_HEADLESS_METRICS_ADDR    optional Prometheus listener (e.g. 127.0.0.1:9191)

  --health                            print JSON backend/durable probe and exit 0
  --metrics-addr <ip:port>            optional Prometheus listener (same as env var)

Peer-local backends are configurable; built-in: memory, file, embedded, composite.
Custom backends implement nodalmerge_runtime_local::PeerLocalPersistence.
"
    );
}
