//! Built-in backend selection. Custom stores implement [`PeerLocalPersistence`](crate::traits::PeerLocalPersistence) directly.

use std::path::PathBuf;
use std::sync::Arc;

use crate::composite::CompositeLocalPersistence;
use crate::error::LocalPersistResult;
use crate::fs::FileLocalPersistence;
use crate::memory::{MemoryLocalPersistence, MemoryLocalStore};
use crate::registry::{open_registered, BackendOpenOptions};
use crate::traits::PeerLocalPersistence;

/// Configurable peer-local backend (built-in + registered custom).
///
/// Follow-on backends plug in via [`crate::registry::register_backend`] or
/// `PersistBackendKind::Registered` — see `docs/HEADLESS_RUNTIME_PERSISTENCE_EXECUTION_PLAN.md` §4b.
#[derive(Debug, Clone)]
pub enum PersistBackendKind {
    /// Ephemeral in-process store (`MemoryLocalStore` can be shared for tests).
    Memory,
    /// Durable directory (`FileLocalPersistence`).
    File { data_dir: PathBuf },
    /// Write-through memory cache + file durability (§4b pilot).
    Composite { data_dir: PathBuf },
    /// Named factory from [`crate::registry`] (e.g. future Mongo/Redis pilots).
    Registered {
        name: String,
        data_dir: Option<PathBuf>,
    },
}

impl PersistBackendKind {
    pub fn label(&self) -> String {
        match self {
            Self::Memory => "memory".to_string(),
            Self::File { .. } => "file".to_string(),
            Self::Composite { .. } => "composite".to_string(),
            Self::Registered { name, .. } => format!("registered:{name}"),
        }
    }
}

/// Type-erased handle for built-in and registered backends.
pub enum PersistenceHandle {
    Memory(MemoryLocalPersistence),
    File(FileLocalPersistence),
    Composite(CompositeLocalPersistence),
    Dyn(Arc<dyn PeerLocalPersistence>),
}

impl PersistenceHandle {
    pub fn open(kind: PersistBackendKind) -> LocalPersistResult<Self> {
        match kind {
            PersistBackendKind::Memory => Ok(Self::Memory(MemoryLocalPersistence::open())),
            PersistBackendKind::File { data_dir } => {
                Ok(Self::File(FileLocalPersistence::open(data_dir)?))
            }
            PersistBackendKind::Composite { data_dir } => {
                Ok(Self::Composite(CompositeLocalPersistence::open(data_dir)?))
            }
            PersistBackendKind::Registered { name, data_dir } => {
                let arc = open_registered(
                    &name,
                    &BackendOpenOptions { data_dir },
                )?;
                Ok(Self::Dyn(arc))
            }
        }
    }

    /// Reopen memory backend against an existing shared store (tests / advanced embedding).
    pub fn reopen_memory(store: MemoryLocalStore) -> Self {
        Self::Memory(MemoryLocalPersistence::reopen(store))
    }

    /// Open from an externally supplied adapter (embedding / custom pilot).
    pub fn from_arc(adapter: Arc<dyn PeerLocalPersistence>) -> Self {
        Self::Dyn(adapter)
    }

    pub fn as_dyn(&self) -> &dyn PeerLocalPersistence {
        match self {
            Self::Memory(p) => p,
            Self::File(p) => p,
            Self::Composite(p) => p,
            Self::Dyn(p) => p.as_ref(),
        }
    }
}

impl PeerLocalPersistence for PersistenceHandle {
    fn is_durable(&self) -> bool {
        self.as_dyn().is_durable()
    }

    fn hydrate(&self, room_id: &str) -> LocalPersistResult<crate::HydrateReport> {
        self.as_dyn().hydrate(room_id)
    }

    fn append_nodes(
        &self,
        room_id: &str,
        nodes: &[nodalmerge_core::SyncNode],
        expected_tail: Option<crate::NodeLogTail>,
    ) -> LocalPersistResult<crate::AppendReport> {
        self.as_dyn().append_nodes(room_id, nodes, expected_tail)
    }

    fn flush(&self, room_id: &str) -> LocalPersistResult<crate::FlushReport> {
        self.as_dyn().flush(room_id)
    }

    fn checkpoint(
        &self,
        room_id: &str,
        meta: crate::CheckpointMeta,
    ) -> LocalPersistResult<crate::CheckpointReport> {
        self.as_dyn().checkpoint(room_id, meta)
    }

    fn recover(&self, room_id: &str) -> LocalPersistResult<crate::RecoveryReport> {
        self.as_dyn().recover(room_id)
    }

    fn put_blob(
        &self,
        room_id: &str,
        hash: &nodalmerge_core::Hash,
        bytes: &[u8],
    ) -> LocalPersistResult<()> {
        self.as_dyn().put_blob(room_id, hash, bytes)
    }

    fn get_blob(
        &self,
        room_id: &str,
        hash: &nodalmerge_core::Hash,
    ) -> LocalPersistResult<Option<Vec<u8>>> {
        self.as_dyn().get_blob(room_id, hash)
    }
}

/// Parse backend from config string.
///
/// Built-in: `memory` | `file` | `composite` (requires `data_dir`).
/// Registered: `registered:<name>` (e.g. `registered:composite`) — same factories as enum variants.
pub fn parse_backend_kind(
    backend: &str,
    data_dir: Option<PathBuf>,
) -> Result<PersistBackendKind, String> {
    let lower = backend.to_ascii_lowercase();
    match lower.as_str() {
        "memory" => Ok(PersistBackendKind::Memory),
        "file" | "filesystem" | "fs" | "embedded" | "sqlite" => {
            let data_dir = data_dir.ok_or_else(|| {
                "file/embedded backend requires --data-dir or NODALMERGE_HEADLESS_DATA_DIR".to_string()
            })?;
            Ok(PersistBackendKind::File { data_dir })
        }
        "composite" => {
            let data_dir = data_dir.ok_or_else(|| {
                "composite backend requires --data-dir or NODALMERGE_HEADLESS_DATA_DIR".to_string()
            })?;
            Ok(PersistBackendKind::Composite { data_dir })
        }
        _ if lower.starts_with("registered:") => {
            let name = lower.trim_start_matches("registered:").to_string();
            Ok(PersistBackendKind::Registered { name, data_dir })
        }
        other => Err(format!(
            "unknown peer-local backend {other:?}; built-in: memory, file, embedded, composite; registered:<name>"
        )),
    }
}

/// Shared ownership for embeddings that need `Arc<dyn PeerLocalPersistence>`.
pub type SharedPeerLocalPersistence = Arc<dyn PeerLocalPersistence>;
