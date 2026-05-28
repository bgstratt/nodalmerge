use nodalmerge_core::{Hash, SyncNode};

/// Monotonic append cursor for optimistic concurrency on the node log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeLogTail {
    pub seq: u64,
}

/// Runtime-advised checkpoint marker persisted with the node log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointMeta {
    pub seq: u64,
    pub canonical_hash: Option<Hash>,
}

#[derive(Debug, Clone)]
pub struct HydrateReport {
    pub room_id: String,
    pub tail: NodeLogTail,
    pub nodes: Vec<SyncNode>,
    pub checkpoints: Vec<CheckpointMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendReport {
    pub room_id: String,
    pub appended: usize,
    pub tail: NodeLogTail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlushReport {
    pub room_id: String,
    pub tail: NodeLogTail,
    pub durable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointReport {
    pub room_id: String,
    pub checkpoint: CheckpointMeta,
}

#[derive(Debug, Clone)]
pub struct RecoveryReport {
    pub room_id: String,
    pub nodes: Vec<SyncNode>,
    pub tail: NodeLogTail,
}
