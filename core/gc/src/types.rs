use std::time::SystemTime;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssetState {
    Uploading,
    Active,
    Grace,
    SweepCandidate,
    Pinned,
    PendingDelete,
    Deleted,
    Quarantined,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetRecord {
    pub hash: String,
    pub object_key: String,
    pub bucket: String,
    pub namespace: String,
    pub first_seen_at: SystemTime,
    pub last_seen_at: SystemTime,
    pub state: AssetState,
    pub pending_delete_at: Option<SystemTime>,
    pub deleted_at: Option<SystemTime>,
    pub last_marked_run_id: Option<String>,
    pub mark_count: u64,
    pub size_bytes: Option<u64>,
    pub content_type: Option<String>,
    pub is_admin_pinned: bool,
    pub pin_reason: Option<String>,
    pub updated_at: SystemTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GcRunMode {
    DryRun,
    MarkOnly,
    SweepSoft,
    SweepHard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GcRunStatus {
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcRunStart {
    pub mode: GcRunMode,
    pub started_at: SystemTime,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct GcRunDelta {
    pub marked_count: u64,
    pub newly_pending_count: u64,
    pub hard_deleted_count: u64,
    pub skipped_pinned_count: u64,
    pub error_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcRunFinish {
    pub status: GcRunStatus,
    pub finished_at: SystemTime,
    pub notes: Option<String>,
}
