//! Parent/child room lineage and topology control-plane contracts (Wave 2).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineageReasonClass {
    #[serde(rename = "reject.lineage_parent_checkpoint_mismatch")]
    ParentCheckpointMismatch,
    #[serde(rename = "reject.lineage_policy_unknown")]
    PolicyUnknown,
    #[serde(rename = "reject.lineage_parent_not_found")]
    ParentNotFound,
    #[serde(rename = "reject.lineage_child_already_exists")]
    ChildAlreadyExists,
    #[serde(rename = "reject.room_not_found")]
    RoomNotFound,
    #[serde(rename = "reject.lineage_invalid_checkpoint")]
    InvalidCheckpoint,
}

impl LineageReasonClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ParentCheckpointMismatch => "reject.lineage_parent_checkpoint_mismatch",
            Self::PolicyUnknown => "reject.lineage_policy_unknown",
            Self::ParentNotFound => "reject.lineage_parent_not_found",
            Self::ChildAlreadyExists => "reject.lineage_child_already_exists",
            Self::RoomNotFound => "reject.room_not_found",
            Self::InvalidCheckpoint => "reject.lineage_invalid_checkpoint",
        }
    }
}

/// Explicit parent cut a child room is bound to (immutable after child creation).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParentCheckpoint {
    pub frontier: Vec<String>,
    pub canonical_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_timeline_hash: Option<String>,
}

/// Immutable lineage metadata recorded when a child room is created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomLineage {
    pub parent_room_id: String,
    pub parent_checkpoint: ParentCheckpoint,
    pub child_purpose: String,
    pub created_by: String,
    pub created_at_hlc: u64,
    pub promotion_policy_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineageRejected {
    pub reason_class: LineageReasonClass,
    pub reason_message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildRoomCreated {
    pub child_room_id: String,
    pub lineage: RoomLineage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomLineageDescribed {
    pub room_id: String,
    pub lineage: Option<RoomLineage>,
    pub ancestors: Vec<RoomLineage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildRoomSummary {
    pub child_room_id: String,
    pub child_purpose: String,
    pub promotion_policy_id: String,
    pub created_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildrenListed {
    pub parent_room_id: String,
    pub children: Vec<ChildRoomSummary>,
}

// --- Promotion pipeline (topology Phase C) ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromotionReasonClass {
    #[serde(rename = "reject.promotion_child_checkpoint_mismatch")]
    ChildCheckpointMismatch,
    #[serde(rename = "reject.promotion_policy_denied")]
    PolicyDenied,
    #[serde(rename = "reject.promotion_invalid_lineage")]
    InvalidLineage,
    #[serde(rename = "reject.promotion_stale_parent")]
    StaleParent,
    #[serde(rename = "reject.promotion_apply_conflict")]
    ApplyConflict,
    #[serde(rename = "reject.promotion_not_found")]
    NotFound,
    #[serde(rename = "reject.promotion_not_validated")]
    NotValidated,
}

impl PromotionReasonClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ChildCheckpointMismatch => "reject.promotion_child_checkpoint_mismatch",
            Self::PolicyDenied => "reject.promotion_policy_denied",
            Self::InvalidLineage => "reject.promotion_invalid_lineage",
            Self::StaleParent => "reject.promotion_stale_parent",
            Self::ApplyConflict => "reject.promotion_apply_conflict",
            Self::NotFound => "reject.promotion_not_found",
            Self::NotValidated => "reject.promotion_not_validated",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionRejected {
    pub reason_class: PromotionReasonClass,
    pub reason_message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromotionLifecycle {
    Proposed,
    Validated,
    Applied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionProposed {
    pub proposal_id: String,
    pub parent_room_id: String,
    pub child_room_id: String,
    pub child_checkpoint_hash: String,
    pub payload_ref: String,
    pub proposal_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionValidated {
    pub proposal_id: String,
    pub validation_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionApplied {
    pub proposal_id: String,
    pub parent_room_id: String,
    pub parent_new_canonical_hash: String,
    pub audit_key: String,
}
