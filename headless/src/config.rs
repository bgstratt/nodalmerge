use std::time::Duration;

use nodalmerge_runtime_local::PersistBackendKind;
use serde::Serialize;

/// Headless worker session configuration.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub server_ws_url: String,
    pub room_id: String,
    pub backend: PersistBackendKind,
    /// How long to stay connected after handshake (catch-up window).
    pub run_for: Duration,
    /// Negotiate IBF set-reconciliation in hello (when local node log is non-empty).
    pub negotiate_ibf: bool,
    /// Negotiate MST sync in hello (requires IBF); runs descent after welcome when roots differ.
    pub negotiate_mst: bool,
}

/// Counters updated during a worker sync session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncSessionStats {
    pub packs_applied: usize,
    pub mst_requests: usize,
    pub mst_nodes_fetched: usize,
    pub used_ibf: bool,
}

/// Per-phase timings for operator dashboards and acceptance baselines.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SessionTimings {
    pub hydrate_ms: u64,
    pub websocket_sync_ms: u64,
    pub flush_ms: u64,
    pub checkpoint_ms: u64,
    pub total_ms: u64,
}

/// Summary returned after a worker session completes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerReport {
    pub saw_welcome: bool,
    pub packs_applied: usize,
    pub mst_requests: usize,
    pub mst_nodes_fetched: usize,
    pub used_ibf: bool,
    pub nodes_persisted_total: usize,
    pub canonical_hash_hex: String,
    pub timings_ms: SessionTimings,
}
