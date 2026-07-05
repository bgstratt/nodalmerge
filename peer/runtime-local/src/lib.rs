//! Peer-local persistence adapters for headless and SDK embeddings (Wave 2).
//!
//! See `docs/HEADLESS_RUNTIME_PERSISTENCE_EXECUTION_PLAN.md` Phase A–B.

mod backend;
mod composite;
mod error;
mod fs;
mod memory;
mod registry;
mod report;
mod traits;
mod util;

pub use backend::{
    parse_backend_kind, PersistBackendKind, PersistenceHandle, SharedPeerLocalPersistence,
};
pub use composite::CompositeLocalPersistence;
pub use registry::{
    open_registered, register_backend, registered_backend_names, BackendFactory, BackendOpenOptions,
};
pub use error::{LocalPersistError, LocalPersistReason, LocalPersistResult};
pub use fs::FileLocalPersistence;
pub use memory::{MemoryLocalPersistence, MemoryLocalStore};
pub use report::{
    AppendReport, CheckpointMeta, CheckpointReport, FlushReport, HydrateReport, NodeLogTail,
    RecoveryReport,
};
pub use traits::PeerLocalPersistence;
