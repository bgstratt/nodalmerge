use std::path::PathBuf;
use std::time::Duration;

use nodalmerge_headless::{run_worker_session, WorkerConfig, WorkerSessionReport};
use nodalmerge_runtime_local::{parse_backend_kind, PeerLocalPersistence, PersistenceHandle};

#[derive(Debug, thiserror::Error)]
pub enum RunCliError {
    #[error("{0}")]
    Msg(String),
}

#[derive(Debug, Clone)]
pub struct RunWorkerOpts {
    pub server_ws_url: String,
    pub room_id: String,
    pub backend: String,
    pub data_dir: Option<PathBuf>,
    pub run_secs: u64,
    pub negotiate_ibf: bool,
    pub negotiate_mst: bool,
    pub report_json: Option<String>,
}

pub async fn run_worker(opts: RunWorkerOpts) -> Result<(), RunCliError> {
    let backend = parse_backend_kind(&opts.backend, opts.data_dir.clone())
        .map_err(RunCliError::Msg)?;
    let durable = PersistenceHandle::open(backend.clone())
        .map_err(|e| RunCliError::Msg(format!("open persistence: {e}")))?
        .is_durable();

    let cfg = WorkerConfig {
        server_ws_url: opts.server_ws_url,
        room_id: opts.room_id.clone(),
        backend,
        run_for: Duration::from_secs(opts.run_secs),
        negotiate_ibf: opts.negotiate_ibf,
        negotiate_mst: opts.negotiate_mst,
    };
    let cfg_snapshot = cfg.clone();

    let report = run_worker_session(cfg)
        .await
        .map_err(RunCliError::Msg)?;

    eprintln!(
        "nodalmerge run: welcome={} packs={} nodes={} canonical_hash={} total_ms={}",
        report.saw_welcome,
        report.packs_applied,
        report.nodes_persisted_total,
        report.canonical_hash_hex,
        report.timings_ms.total_ms,
    );

    if let Some(path) = opts.report_json {
        let session = WorkerSessionReport::from_session(&cfg_snapshot, &report, durable);
        let json = serde_json::to_string_pretty(&session)
            .map_err(|e| RunCliError::Msg(e.to_string()))?;
        if path == "-" {
            println!("{json}");
        } else {
            std::fs::write(&path, format!("{json}\n"))
                .map_err(|e| RunCliError::Msg(format!("write {path}: {e}")))?;
        }
    }

    if !report.saw_welcome {
        return Err(RunCliError::Msg(
            "handshake did not receive welcome".into(),
        ));
    }

    Ok(())
}
