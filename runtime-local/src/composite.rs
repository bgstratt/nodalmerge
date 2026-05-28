//! Write-through composite: in-process read cache + durable file backing (§4b pilot).

use std::path::Path;

use nodalmerge_core::{Hash, SyncNode};

use crate::error::LocalPersistResult;
use crate::fs::FileLocalPersistence;
use crate::memory::MemoryLocalPersistence;
use crate::report::{
    AppendReport, CheckpointMeta, CheckpointReport, FlushReport, HydrateReport, NodeLogTail,
    RecoveryReport,
};
use crate::traits::PeerLocalPersistence;

/// Memory hot cache with [`FileLocalPersistence`] write-through (durable across restart).
#[derive(Debug)]
pub struct CompositeLocalPersistence {
    cache: MemoryLocalPersistence,
    durable: FileLocalPersistence,
}

impl CompositeLocalPersistence {
    pub fn open(data_dir: impl AsRef<Path>) -> LocalPersistResult<Self> {
        Ok(Self {
            cache: MemoryLocalPersistence::open(),
            durable: FileLocalPersistence::open(data_dir)?,
        })
    }

    fn refresh_cache_from_durable(&self, room_id: &str) -> LocalPersistResult<()> {
        let report = self.durable.hydrate(room_id)?;
        self.cache.store().reset_room(room_id)?;
        if report.nodes.is_empty() {
            return Ok(());
        }
        self.cache.append_nodes(room_id, &report.nodes, None)?;
        Ok(())
    }
}

impl PeerLocalPersistence for CompositeLocalPersistence {
    fn is_durable(&self) -> bool {
        true
    }

    fn hydrate(&self, room_id: &str) -> LocalPersistResult<HydrateReport> {
        let report = self.durable.hydrate(room_id)?;
        self.cache.store().reset_room(room_id)?;
        if !report.nodes.is_empty() {
            self.cache.append_nodes(room_id, &report.nodes, None)?;
        }
        Ok(report)
    }

    fn append_nodes(
        &self,
        room_id: &str,
        nodes: &[SyncNode],
        expected_tail: Option<NodeLogTail>,
    ) -> LocalPersistResult<AppendReport> {
        let report = self.durable.append_nodes(room_id, nodes, expected_tail)?;
        self.refresh_cache_from_durable(room_id)?;
        Ok(report)
    }

    fn flush(&self, room_id: &str) -> LocalPersistResult<FlushReport> {
        self.durable.flush(room_id)
    }

    fn checkpoint(
        &self,
        room_id: &str,
        meta: CheckpointMeta,
    ) -> LocalPersistResult<CheckpointReport> {
        let report = self.durable.checkpoint(room_id, meta.clone())?;
        let _ = self.cache.checkpoint(room_id, meta);
        Ok(report)
    }

    fn recover(&self, room_id: &str) -> LocalPersistResult<RecoveryReport> {
        let report = self.durable.recover(room_id)?;
        self.refresh_cache_from_durable(room_id)?;
        Ok(report)
    }

    fn put_blob(&self, room_id: &str, hash: &Hash, bytes: &[u8]) -> LocalPersistResult<()> {
        self.durable.put_blob(room_id, hash, bytes)?;
        self.cache.put_blob(room_id, hash, bytes)
    }

    fn get_blob(&self, room_id: &str, hash: &Hash) -> LocalPersistResult<Option<Vec<u8>>> {
        if let Some(bytes) = self.cache.get_blob(room_id, hash)? {
            return Ok(Some(bytes));
        }
        let bytes = self.durable.get_blob(room_id, hash)?;
        if let Some(ref b) = bytes {
            let _ = self.cache.put_blob(room_id, hash, b);
        }
        Ok(bytes)
    }
}
