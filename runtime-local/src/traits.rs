use nodalmerge_core::{Hash, SyncNode};

use crate::error::LocalPersistResult;
use crate::report::{
    AppendReport, CheckpointMeta, CheckpointReport, FlushReport, HydrateReport, NodeLogTail,
    RecoveryReport,
};

/// Peer-local persistence facade (headless worker / SDK plugin surface).
///
/// Implementations must preserve deterministic node ordering on hydrate/recover
/// so replay yields the same canonical hash as a live session at the same tail.
pub trait PeerLocalPersistence: Send + Sync {
    /// `true` when this backend survives process restart (filesystem, DB).
    /// In-memory backends return `false` unless reopened from a shared store handle.
    fn is_durable(&self) -> bool;

    fn hydrate(&self, room_id: &str) -> LocalPersistResult<HydrateReport>;

    fn append_nodes(
        &self,
        room_id: &str,
        nodes: &[SyncNode],
        expected_tail: Option<NodeLogTail>,
    ) -> LocalPersistResult<AppendReport>;

    fn flush(&self, room_id: &str) -> LocalPersistResult<FlushReport>;

    fn checkpoint(
        &self,
        room_id: &str,
        meta: CheckpointMeta,
    ) -> LocalPersistResult<CheckpointReport>;

    fn recover(&self, room_id: &str) -> LocalPersistResult<RecoveryReport>;

    fn put_blob(&self, room_id: &str, hash: &Hash, bytes: &[u8]) -> LocalPersistResult<()>;

    fn get_blob(&self, room_id: &str, hash: &Hash) -> LocalPersistResult<Option<Vec<u8>>>;
}
