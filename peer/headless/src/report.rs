//! Machine-readable session report (Phase E observability).

use serde::Serialize;

use crate::config::{SessionTimings, WorkerConfig, WorkerReport};

#[derive(Debug, Clone, Serialize)]
pub struct WorkerSessionReport {
    pub artifact: &'static str,
    pub room_id: String,
    pub server_ws_url: String,
    pub backend: String,
    pub durable: bool,
    pub negotiate_ibf: bool,
    pub negotiate_mst: bool,
    pub run_for_secs: u64,
    pub saw_welcome: bool,
    pub packs_applied: usize,
    pub mst_requests: usize,
    pub mst_nodes_fetched: usize,
    pub used_ibf: bool,
    pub nodes_persisted_total: usize,
    pub canonical_hash_hex: String,
    pub timings_ms: SessionTimings,
}

impl WorkerSessionReport {
    pub fn from_session(
        cfg: &WorkerConfig,
        report: &WorkerReport,
        durable: bool,
    ) -> Self {
        Self {
            artifact: "nodalmerge-headless-session",
            room_id: cfg.room_id.clone(),
            server_ws_url: cfg.server_ws_url.clone(),
            backend: cfg.backend.label(),
            durable,
            negotiate_ibf: cfg.negotiate_ibf,
            negotiate_mst: cfg.negotiate_mst,
            run_for_secs: cfg.run_for.as_secs(),
            saw_welcome: report.saw_welcome,
            packs_applied: report.packs_applied,
            mst_requests: report.mst_requests,
            mst_nodes_fetched: report.mst_nodes_fetched,
            used_ibf: report.used_ibf,
            nodes_persisted_total: report.nodes_persisted_total,
            canonical_hash_hex: report.canonical_hash_hex.clone(),
            timings_ms: report.timings_ms.clone(),
        }
    }
}
