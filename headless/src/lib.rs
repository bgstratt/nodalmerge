//! Headless peer worker: websocket sync to a reflector + [`PeerLocalPersistence`](nodalmerge_runtime_local::PeerLocalPersistence).

mod config;
mod report;
mod sync;
mod worker;

pub use config::{SessionTimings, SyncSessionStats, WorkerConfig, WorkerReport};
pub use report::WorkerSessionReport;
pub use worker::run_worker_session;
