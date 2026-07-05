use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use nodalmerge_core::{Hash, SyncNode};

use crate::error::{LocalPersistError, LocalPersistReason, LocalPersistResult};
use crate::report::{
    AppendReport, CheckpointMeta, CheckpointReport, FlushReport, HydrateReport, NodeLogTail,
    RecoveryReport,
};
use crate::traits::PeerLocalPersistence;

#[derive(Debug, Default, Clone)]
struct RoomLog {
    nodes: Vec<SyncNode>,
    tail_seq: u64,
    checkpoints: Vec<CheckpointMeta>,
    blobs: BTreeMap<Hash, Vec<u8>>,
}

/// Shared backing store for in-memory peer-local persistence.
///
/// Cloning this `Arc` and opening a new [`MemoryLocalPersistence`] simulates
/// process restart while preserving data (used by `LOCAL-PERSIST-001`).
#[derive(Debug, Default, Clone)]
pub struct MemoryLocalStore {
    inner: Arc<Mutex<BTreeMap<String, RoomLog>>>,
}

/// In-memory [`PeerLocalPersistence`] for tests and ephemeral headless pods.
#[derive(Debug, Clone)]
pub struct MemoryLocalPersistence {
    store: MemoryLocalStore,
    read_only: bool,
}

impl MemoryLocalPersistence {
    pub fn open() -> Self {
        Self {
            store: MemoryLocalStore::default(),
            read_only: false,
        }
    }

    /// Reattach to an existing store after simulated restart.
    pub fn reopen(store: MemoryLocalStore) -> Self {
        Self {
            store,
            read_only: false,
        }
    }

    pub fn store(&self) -> MemoryLocalStore {
        self.store.clone()
    }

    pub fn read_only_view(store: MemoryLocalStore) -> Self {
        Self {
            store,
            read_only: true,
        }
    }
}

impl MemoryLocalStore {
    /// Clear in-memory room state (used by composite backend to resync from durable layer).
    pub(crate) fn reset_room(&self, room_id: &str) -> LocalPersistResult<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| LocalPersistError::new(LocalPersistReason::Unavailable, "store lock poisoned"))?;
        guard.remove(room_id);
        Ok(())
    }

    fn with_room<F, T>(&self, room_id: &str, f: F) -> LocalPersistResult<T>
    where
        F: FnOnce(&mut RoomLog) -> LocalPersistResult<T>,
    {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| LocalPersistError::new(LocalPersistReason::Unavailable, "store lock poisoned"))?;
        let room = guard.entry(room_id.to_string()).or_default();
        f(room)
    }

    fn read_room<F, T>(&self, room_id: &str, f: F) -> LocalPersistResult<T>
    where
        F: FnOnce(&RoomLog) -> LocalPersistResult<T>,
    {
        let guard = self
            .inner
            .lock()
            .map_err(|_| LocalPersistError::new(LocalPersistReason::Unavailable, "store lock poisoned"))?;
        let room = guard.get(room_id).cloned().unwrap_or_default();
        f(&room)
    }
}

impl PeerLocalPersistence for MemoryLocalPersistence {
    fn is_durable(&self) -> bool {
        false
    }

    fn hydrate(&self, room_id: &str) -> LocalPersistResult<HydrateReport> {
        self.store.read_room(room_id, |room| {
            Ok(HydrateReport {
                room_id: room_id.to_string(),
                tail: NodeLogTail { seq: room.tail_seq },
                nodes: room.nodes.clone(),
                checkpoints: room.checkpoints.clone(),
            })
        })
    }

    fn append_nodes(
        &self,
        room_id: &str,
        nodes: &[SyncNode],
        expected_tail: Option<NodeLogTail>,
    ) -> LocalPersistResult<AppendReport> {
        if self.read_only {
            return Err(LocalPersistError::new(
                LocalPersistReason::ReadOnly,
                "memory adapter is read-only",
            ));
        }

        self.store.with_room(room_id, |room| {
            if let Some(expected) = expected_tail {
                if expected.seq != room.tail_seq {
                    return Err(LocalPersistError::new(
                        LocalPersistReason::TailConflict,
                        format!(
                            "expected tail seq {} but room tail is {}",
                            expected.seq, room.tail_seq
                        ),
                    ));
                }
            }

            let mut appended = 0usize;
            for node in nodes {
                if room.nodes.iter().any(|n| n.id == node.id) {
                    continue;
                }
                room.nodes.push(node.clone());
                room.tail_seq = room.tail_seq.saturating_add(1);
                appended += 1;
            }

            Ok(AppendReport {
                room_id: room_id.to_string(),
                appended,
                tail: NodeLogTail { seq: room.tail_seq },
            })
        })
    }

    fn flush(&self, room_id: &str) -> LocalPersistResult<FlushReport> {
        self.store.read_room(room_id, |room| {
            Ok(FlushReport {
                room_id: room_id.to_string(),
                tail: NodeLogTail { seq: room.tail_seq },
                durable: false,
            })
        })
    }

    fn checkpoint(
        &self,
        room_id: &str,
        meta: CheckpointMeta,
    ) -> LocalPersistResult<CheckpointReport> {
        if self.read_only {
            return Err(LocalPersistError::new(
                LocalPersistReason::ReadOnly,
                "memory adapter is read-only",
            ));
        }

        self.store.with_room(room_id, |room| {
            room.checkpoints.push(meta.clone());
            Ok(CheckpointReport {
                room_id: room_id.to_string(),
                checkpoint: meta,
            })
        })
    }

    fn recover(&self, room_id: &str) -> LocalPersistResult<RecoveryReport> {
        self.store.read_room(room_id, |room| {
            Ok(RecoveryReport {
                room_id: room_id.to_string(),
                nodes: room.nodes.clone(),
                tail: NodeLogTail { seq: room.tail_seq },
            })
        })
    }

    fn put_blob(&self, room_id: &str, hash: &Hash, bytes: &[u8]) -> LocalPersistResult<()> {
        if self.read_only {
            return Err(LocalPersistError::new(
                LocalPersistReason::ReadOnly,
                "memory adapter is read-only",
            ));
        }

        self.store.with_room(room_id, |room| {
            room.blobs.insert(*hash, bytes.to_vec());
            Ok(())
        })
    }

    fn get_blob(&self, room_id: &str, hash: &Hash) -> LocalPersistResult<Option<Vec<u8>>> {
        self.store.read_room(room_id, |room| Ok(room.blobs.get(hash).cloned()))
    }
}
