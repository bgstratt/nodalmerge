use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArchiveReasonClass {
    #[serde(rename = "reject.archive_unsupported_format")]
    UnsupportedFormat,
    #[serde(rename = "reject.archive_manifest_invalid")]
    ManifestInvalid,
    #[serde(rename = "reject.archive_digest_mismatch")]
    DigestMismatch,
    #[serde(rename = "reject.archive_signature_invalid")]
    SignatureInvalid,
    #[serde(rename = "reject.archive_checkpoint_not_found")]
    CheckpointNotFound,
    #[serde(rename = "reject.archive_policy_timeline_mismatch")]
    PolicyTimelineMismatch,
}

impl ArchiveReasonClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedFormat => "reject.archive_unsupported_format",
            Self::ManifestInvalid => "reject.archive_manifest_invalid",
            Self::DigestMismatch => "reject.archive_digest_mismatch",
            Self::SignatureInvalid => "reject.archive_signature_invalid",
            Self::CheckpointNotFound => "reject.archive_checkpoint_not_found",
            Self::PolicyTimelineMismatch => "reject.archive_policy_timeline_mismatch",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveCheckpoint {
    pub frontier: Vec<String>,
    pub canonical_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchivePayloadDigestSet {
    pub nodes: String,
    pub blobs: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveCompatibilityWindow {
    pub min_supported: String,
    pub max_supported: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveProvenance {
    pub source_room: String,
    pub tool: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveDescribed {
    pub room: String,
    pub archive_ref: String,
    pub manifest_id: String,
    pub format_version: String,
    pub archive_kind: String,
    pub checkpoint: ArchiveCheckpoint,
    pub payload_digest_set: ArchivePayloadDigestSet,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compatibility_window: Option<ArchiveCompatibilityWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ArchiveProvenance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveValidated {
    pub room: String,
    pub archive_ref: String,
    pub accepted: bool,
    pub mode: String,
    pub checks: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compatibility_window: Option<ArchiveCompatibilityWindow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveRejected {
    pub room: String,
    pub archive_ref: String,
    pub reason_class: ArchiveReasonClass,
    pub reason_message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveImported {
    pub room: String,
    pub archive_ref: String,
    pub canonical_hash: String,
    pub checkpoint: ArchiveCheckpoint,
    pub imported_nodes: u64,
    pub imported_blobs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveExported {
    pub room: String,
    pub source_room: String,
    pub archive_ref: String,
    pub manifest_id: String,
    pub checkpoint: ArchiveCheckpoint,
    pub payload_digest_set: ArchivePayloadDigestSet,
    pub compatibility_window: ArchiveCompatibilityWindow,
    pub payload_digest_policy: String,
    pub policy_timeline_hash: String,
    pub policy_timeline_cutover_lamport: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ArchiveWsRequest {
    #[serde(rename = "archive.describe")]
    Describe {
        room: String,
        archive_ref: String,
    },
    #[serde(rename = "archive.validate")]
    Validate {
        room: String,
        archive_ref: String,
        mode: String,
    },
    #[serde(rename = "archive.import")]
    Import {
        room: String,
        archive_ref: String,
        import_mode: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        expected_checkpoint: Option<ArchiveCheckpoint>,
    },
    #[serde(rename = "archive.export")]
    Export {
        room: String,
        source_room: String,
        archive_ref: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ArchiveWsResponse {
    #[serde(rename = "archive.describe.result")]
    DescribeResult(ArchiveDescribed),
    #[serde(rename = "archive.validate.result")]
    ValidateResult(ArchiveValidated),
    #[serde(rename = "archive.validate.rejected")]
    ValidateRejected(ArchiveRejected),
    #[serde(rename = "archive.import.completed")]
    ImportCompleted(ArchiveImported),
    #[serde(rename = "archive.import.rejected")]
    ImportRejected(ArchiveRejected),
    #[serde(rename = "archive.export.result")]
    ExportResult(ArchiveExported),
    #[serde(rename = "archive.export.rejected")]
    ExportRejected(ArchiveRejected),
}
