use std::collections::HashSet;

use nodalmerge_core::{ArchiveWsResponse, Ibf, MerkleSearchTree, NodeId, SyncCapabilities, TopologyWsResponse};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A7 helper: negotiate active capabilities for a session.
pub fn negotiate_capabilities(client_caps: &SyncCapabilities) -> SyncCapabilities {
    let server_caps = SyncCapabilities::default();
    server_caps.intersect(client_caps)
}

/// B2 helper: compute optional MST root for welcome when negotiated.
pub fn welcome_mst_root_hex(
    negotiated_caps: &SyncCapabilities,
    all_ids: &[NodeId],
) -> Option<String> {
    if !negotiated_caps.supports_mst {
        return None;
    }
    Some(MerkleSearchTree::from_ids(all_ids).root_hash_hex())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WelcomeEnvelope {
    #[serde(rename = "type")]
    pub kind: String,
    pub root: String,
    pub frontier: Vec<String>,
    pub missing: Vec<String>,
    pub caps: SyncCapabilities,
    pub server_pubkey: String,
    pub peers: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mst_root: Option<String>,
}

pub fn assemble_welcome_envelope(
    root_hex: String,
    frontier_hex: Vec<String>,
    missing_hex: Vec<String>,
    caps: SyncCapabilities,
    server_pubkey_hex: String,
    peers: Vec<String>,
    mst_root_hex: Option<String>,
) -> WelcomeEnvelope {
    WelcomeEnvelope {
        kind: "welcome".to_string(),
        root: root_hex,
        frontier: frontier_hex,
        missing: missing_hex,
        caps,
        server_pubkey: server_pubkey_hex,
        peers,
        mst_root: mst_root_hex,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatchupPackEnvelope {
    #[serde(rename = "type")]
    pub kind: String,
    pub from: String,
    pub nodes: String,
}

pub fn assemble_catchup_pack_envelope(nodes_b64: String) -> CatchupPackEnvelope {
    CatchupPackEnvelope {
        kind: "pack".to_string(),
        from: "server".to_string(),
        nodes: nodes_b64,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerPackReplyEnvelope {
    #[serde(rename = "type")]
    pub kind: String,
    pub from: String,
    pub nodes: String,
    pub root: String,
}

pub fn assemble_server_pack_reply_envelope(
    nodes_b64: String,
    merkle_root_hex: String,
) -> ServerPackReplyEnvelope {
    ServerPackReplyEnvelope {
        kind: "pack".to_string(),
        from: "server".to_string(),
        nodes: nodes_b64,
        root: merkle_root_hex,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerPackRelayEnvelope {
    #[serde(rename = "type")]
    pub kind: String,
    pub from: String,
    pub nodes: String,
    pub root: String,
}

pub fn assemble_peer_pack_relay_envelope(
    from_peer_hex: String,
    nodes_b64: String,
    merkle_root_hex: String,
) -> PeerPackRelayEnvelope {
    PeerPackRelayEnvelope {
        kind: "pack".to_string(),
        from: from_peer_hex,
        nodes: nodes_b64,
        root: merkle_root_hex,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MstResponseEnvelope<T> {
    #[serde(rename = "type")]
    pub kind: String,
    pub nodes: Vec<T>,
}

pub fn assemble_mst_response_envelope<T>(nodes: Vec<T>) -> MstResponseEnvelope<T> {
    MstResponseEnvelope {
        kind: "mst-response".to_string(),
        nodes,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerStampedRelayEnvelope<T> {
    #[serde(rename = "type")]
    pub kind: String,
    pub from: String,
    #[serde(flatten)]
    pub payload: T,
}

pub fn assemble_peer_stamped_relay_envelope<T>(
    kind: String,
    from_peer_hex: String,
    payload: T,
) -> PeerStampedRelayEnvelope<T> {
    PeerStampedRelayEnvelope {
        kind,
        from: from_peer_hex,
        payload,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRelayPayload {
    #[serde(flatten)]
    pub fields: Map<String, Value>,
}

pub fn normalize_peer_stamped_relay_parts(mut fields: Map<String, Value>) -> (String, JsonRelayPayload) {
    let kind = fields
        .remove("type")
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    fields.remove("from");
    (kind, JsonRelayPayload { fields })
}

pub fn assemble_normalized_peer_stamped_relay_envelope(
    from_peer_hex: String,
    fields: Map<String, Value>,
) -> PeerStampedRelayEnvelope<JsonRelayPayload> {
    let (kind, payload) = normalize_peer_stamped_relay_parts(fields);
    assemble_peer_stamped_relay_envelope(kind, from_peer_hex, payload)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribeAckEnvelope {
    #[serde(rename = "type")]
    pub kind: String,
}

pub fn assemble_subscribe_ack_envelope() -> SubscribeAckEnvelope {
    SubscribeAckEnvelope {
        kind: "subscribe-ack".to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobRedirectEntry {
    pub hash: String,
    pub url: String,
    pub expires_at_unix: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobRedirectEnvelope {
    #[serde(rename = "type")]
    pub kind: String,
    pub redirects: Vec<BlobRedirectEntry>,
}

pub fn assemble_blob_redirect_envelope(redirects: Vec<BlobRedirectEntry>) -> BlobRedirectEnvelope {
    BlobRedirectEnvelope {
        kind: "blob-redirect".to_string(),
        redirects,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobPackEntry {
    pub hash: String,
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobPackEnvelope {
    #[serde(rename = "type")]
    pub kind: String,
    pub blobs: Vec<BlobPackEntry>,
    pub requested: Vec<String>,
}

pub fn assemble_blob_pack_envelope(
    blobs: Vec<BlobPackEntry>,
    requested: Vec<String>,
) -> BlobPackEnvelope {
    BlobPackEnvelope {
        kind: "blob-pack".to_string(),
        blobs,
        requested,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadGrantedEnvelope {
    #[serde(rename = "type")]
    pub kind: String,
    pub hash: String,
    pub url: String,
    pub expires_at_unix: u64,
}

pub fn assemble_upload_granted_envelope(
    hash_hex: String,
    url: String,
    expires_at_unix: u64,
) -> UploadGrantedEnvelope {
    UploadGrantedEnvelope {
        kind: "upload-granted".to_string(),
        hash: hash_hex,
        url,
        expires_at_unix,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadDeniedEnvelope {
    #[serde(rename = "type")]
    pub kind: String,
    pub hash: String,
    pub reason: String,
}

pub fn assemble_upload_denied_envelope(hash_hex: String, reason: String) -> UploadDeniedEnvelope {
    UploadDeniedEnvelope {
        kind: "upload-denied".to_string(),
        hash: hash_hex,
        reason,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadRejectedEnvelope {
    #[serde(rename = "type")]
    pub kind: String,
    pub hash: String,
    pub reason: String,
}

pub fn assemble_upload_rejected_envelope(
    hash_hex: String,
    reason: String,
) -> UploadRejectedEnvelope {
    UploadRejectedEnvelope {
        kind: "upload-rejected".to_string(),
        hash: hash_hex,
        reason,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobAvailableEnvelope {
    #[serde(rename = "type")]
    pub kind: String,
    pub hashes: Vec<String>,
}

pub fn assemble_blob_available_envelope(hashes: Vec<String>) -> BlobAvailableEnvelope {
    BlobAvailableEnvelope {
        kind: "blob-available".to_string(),
        hashes,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceEnvelope<T> {
    #[serde(rename = "type")]
    pub kind: String,
    pub from: String,
    pub data: T,
}

pub fn assemble_presence_envelope<T>(from: String, data: T) -> PresenceEnvelope<T> {
    PresenceEnvelope {
        kind: "presence".to_string(),
        from,
        data,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomLockedEnvelope {
    #[serde(rename = "type")]
    pub kind: String,
    pub pubkey: String,
}

pub fn assemble_room_locked_envelope(pubkey_hex: String) -> RoomLockedEnvelope {
    RoomLockedEnvelope {
        kind: "room-locked".to_string(),
        pubkey: pubkey_hex,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetRoomKeyRejectedEnvelope {
    #[serde(rename = "type")]
    pub kind: String,
    pub msg: String,
}

pub fn assemble_set_room_key_rejected_envelope(msg: String) -> SetRoomKeyRejectedEnvelope {
    SetRoomKeyRejectedEnvelope {
        kind: "error".to_string(),
        msg,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorEnvelope {
    #[serde(rename = "type")]
    pub kind: String,
    pub msg: String,
}

pub fn assemble_error_envelope(msg: String) -> ErrorEnvelope {
    ErrorEnvelope {
        kind: "error".to_string(),
        msg,
    }
}

pub fn serialize_archive_ws_response(
    response: &ArchiveWsResponse,
) -> Result<String, serde_json::Error> {
    serde_json::to_string(response)
}

pub fn serialize_topology_ws_response(
    response: &TopologyWsResponse,
) -> Result<String, serde_json::Error> {
    serde_json::to_string(response)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicySetEnvelope {
    #[serde(rename = "type")]
    pub kind: String,
}

pub fn assemble_policy_set_envelope() -> PolicySetEnvelope {
    PolicySetEnvelope {
        kind: "policy-set".to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerInfoEnvelope {
    #[serde(rename = "type")]
    pub kind: String,
    pub pubkey: String,
}

pub fn assemble_server_info_envelope(pubkey_hex: String) -> ServerInfoEnvelope {
    ServerInfoEnvelope {
        kind: "server-info".to_string(),
        pubkey: pubkey_hex,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TickStartEnvelope {
    #[serde(rename = "type")]
    pub kind: String,
    pub interval_ms: u64,
}

pub fn assemble_tick_start_envelope(started: bool, interval_ms: u64) -> TickStartEnvelope {
    TickStartEnvelope {
        kind: if started {
            "tick-started".to_string()
        } else {
            "tick-already-running".to_string()
        },
        interval_ms,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TickStoppedEnvelope {
    #[serde(rename = "type")]
    pub kind: String,
}

pub fn assemble_tick_stopped_envelope() -> TickStoppedEnvelope {
    TickStoppedEnvelope {
        kind: "tick-stopped".to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotPackEnvelope {
    #[serde(rename = "type")]
    pub kind: String,
    pub pack_b64: String,
    pub snapshot_hash: String,
}

pub fn assemble_snapshot_pack_envelope(
    pack_b64: String,
    snapshot_hash: String,
) -> SnapshotPackEnvelope {
    SnapshotPackEnvelope {
        kind: "snapshot-pack".to_string(),
        pack_b64,
        snapshot_hash,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactAckEnvelope {
    #[serde(rename = "type")]
    pub kind: String,
    pub snapshot_hash: String,
}

pub fn assemble_compact_ack_envelope(snapshot_hash: String) -> CompactAckEnvelope {
    CompactAckEnvelope {
        kind: "compact-ack".to_string(),
        snapshot_hash,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerJoinedEnvelope {
    #[serde(rename = "type")]
    pub kind: String,
    pub from: String,
    pub pubkey: String,
}

pub fn assemble_peer_joined_envelope(peer_pubkey_hex: String) -> PeerJoinedEnvelope {
    PeerJoinedEnvelope {
        kind: "peer-joined".to_string(),
        from: peer_pubkey_hex.clone(),
        pubkey: peer_pubkey_hex,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerLeftEnvelope {
    #[serde(rename = "type")]
    pub kind: String,
    pub from: String,
}

pub fn assemble_peer_left_envelope(peer_pubkey_hex: String) -> PeerLeftEnvelope {
    PeerLeftEnvelope {
        kind: "peer-left".to_string(),
        from: peer_pubkey_hex,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseFrameSpec {
    pub code: u16,
    pub reason: String,
}

pub fn assemble_token_expired_close_frame() -> CloseFrameSpec {
    CloseFrameSpec {
        code: 4002,
        reason: "token expired".to_string(),
    }
}

pub fn assemble_resync_required_close_frame() -> CloseFrameSpec {
    CloseFrameSpec {
        code: 4001,
        reason: "resync required".to_string(),
    }
}

pub fn assemble_server_overload_close_frame() -> CloseFrameSpec {
    CloseFrameSpec {
        code: 1011,
        reason: "server overload".to_string(),
    }
}

pub fn assemble_rate_limit_exceeded_close_frame() -> CloseFrameSpec {
    CloseFrameSpec {
        code: 4008,
        reason: "rate limit exceeded".to_string(),
    }
}

#[derive(Debug, Clone)]
pub enum ClientIbfInput {
    Present(Ibf),
    InvalidOrUndecodable,
    Missing,
}

#[derive(Debug, Clone)]
pub struct SyncDiffInput {
    pub negotiated_supports_ibf: bool,
    pub client_known: Vec<NodeId>,
    pub client_ibf: ClientIbfInput,
    pub server_ids: Vec<NodeId>,
    pub legacy_missing_from_us: Vec<NodeId>,
}

/// Decide the sync diff branch for welcome/catchup.
///
/// Returns `(only_in_client, only_in_server)` matching ws_handler behavior.
pub fn decide_sync_diff(input: SyncDiffInput) -> (Vec<NodeId>, Vec<NodeId>) {
    if input.negotiated_supports_ibf {
        return match input.client_ibf {
            ClientIbfInput::Present(client_ibf) => {
                let mut diff = Ibf::from_ids(&input.server_ids);
                diff.subtract(&client_ibf);
                match diff.decode() {
                    Some((in_server, in_client)) => (in_client, in_server),
                    None => {
                        // Preserve existing ws_handler fallback semantics.
                        let all: HashSet<_> = input.server_ids.into_iter().collect();
                        (vec![], all.into_iter().collect())
                    }
                }
            }
            ClientIbfInput::InvalidOrUndecodable => (vec![], input.server_ids),
            ClientIbfInput::Missing => {
                let client_known_set: HashSet<_> = input.client_known.iter().copied().collect();
                let all: HashSet<_> = input.server_ids.into_iter().collect();
                let only_in_server: Vec<_> = all.difference(&client_known_set).copied().collect();
                (vec![], only_in_server)
            }
        };
    }

    let client_known_set: HashSet<_> = input.client_known.iter().copied().collect();
    let all: HashSet<_> = input.server_ids.into_iter().collect();
    let only_in_server: Vec<_> = all.difference(&client_known_set).copied().collect();
    (input.legacy_missing_from_us, only_in_server)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nodalmerge_core::{
        ArchiveCheckpoint,
        ArchiveExported,
        ArchiveImported,
        ArchiveReasonClass,
        ArchiveRejected,
        ChildRoomCreated,
        ParentCheckpoint,
        PromotionApplied,
        RoomLineage,
        TopologyWsResponse,
        Hash,
    };

    #[test]
    fn negotiate_caps_intersection() {
        let client = SyncCapabilities {
            supports_ibf: true,
            supports_mst: false,
            ..Default::default()
        };
        let n = negotiate_capabilities(&client);
        assert!(n.supports_ibf);
        assert!(!n.supports_mst);
    }

    #[test]
    fn mst_root_only_when_negotiated() {
        let ids = vec![Hash([1u8; 32]), Hash([2u8; 32])];
        let off = SyncCapabilities {
            supports_mst: false,
            ..Default::default()
        };
        assert!(welcome_mst_root_hex(&off, &ids).is_none());

        let on = SyncCapabilities {
            supports_mst: true,
            ..Default::default()
        };
        let root = welcome_mst_root_hex(&on, &ids);
        assert!(root.is_some());
        assert!(!root.unwrap().is_empty());
    }

    #[test]
    fn diff_legacy_branch_returns_missing_and_server_diff() {
        let server_ids = vec![Hash([1u8; 32]), Hash([2u8; 32])];
        let legacy_missing = vec![Hash([9u8; 32])];
        let (only_in_client, only_in_server) = decide_sync_diff(SyncDiffInput {
            negotiated_supports_ibf: false,
            client_known: vec![Hash([1u8; 32])],
            client_ibf: ClientIbfInput::Missing,
            server_ids: server_ids.clone(),
            legacy_missing_from_us: legacy_missing.clone(),
        });
        assert_eq!(only_in_client, legacy_missing);
        assert_eq!(only_in_server.len(), 1);
    }

    #[test]
    fn diff_ibf_missing_falls_back_to_frontier_diff() {
        let server_ids = vec![Hash([1u8; 32]), Hash([2u8; 32])];
        let (only_in_client, only_in_server) = decide_sync_diff(SyncDiffInput {
            negotiated_supports_ibf: true,
            client_known: vec![Hash([1u8; 32])],
            client_ibf: ClientIbfInput::Missing,
            server_ids,
            legacy_missing_from_us: vec![],
        });
        assert!(only_in_client.is_empty());
        assert_eq!(only_in_server.len(), 1);
    }

    #[test]
    fn diff_ibf_invalid_returns_all_server_ids() {
        let server_ids = vec![Hash([3u8; 32]), Hash([4u8; 32])];
        let (_only_in_client, only_in_server) = decide_sync_diff(SyncDiffInput {
            negotiated_supports_ibf: true,
            client_known: vec![],
            client_ibf: ClientIbfInput::InvalidOrUndecodable,
            server_ids: server_ids.clone(),
            legacy_missing_from_us: vec![],
        });
        assert_eq!(only_in_server, server_ids);
    }

    #[test]
    fn assemble_welcome_includes_mst_when_provided() {
        let caps = SyncCapabilities {
            supports_ibf: true,
            supports_mst: true,
            ..Default::default()
        };
        let w = assemble_welcome_envelope(
            "root-hex".to_string(),
            vec!["f1".to_string()],
            vec!["m1".to_string()],
            caps.clone(),
            "server-pk".to_string(),
            vec!["peer-a".to_string()],
            Some("mst-root".to_string()),
        );

        assert_eq!(w.kind, "welcome");
        assert_eq!(w.caps, caps);
        assert_eq!(w.mst_root, Some("mst-root".to_string()));
    }

    #[test]
    fn assemble_welcome_omits_mst_when_absent() {
        let w = assemble_welcome_envelope(
            "root".to_string(),
            vec![],
            vec![],
            SyncCapabilities::default(),
            "server".to_string(),
            vec![],
            None,
        );
        assert!(w.mst_root.is_none());
    }

    #[test]
    fn assemble_catchup_pack_envelope_defaults_to_server_sender() {
        let p = assemble_catchup_pack_envelope("pack-b64".to_string());
        assert_eq!(p.kind, "pack");
        assert_eq!(p.from, "server");
        assert_eq!(p.nodes, "pack-b64");
    }

    #[test]
    fn assemble_catchup_pack_envelope_is_stable_on_reconstruction() {
        let p = assemble_catchup_pack_envelope("abc123".to_string());
        let p2 = assemble_catchup_pack_envelope(p.nodes.clone());
        assert_eq!(p, p2);
    }

    #[test]
    fn assemble_peer_joined_envelope_sets_type_and_sender() {
        let p = assemble_peer_joined_envelope("peer-abc".to_string());
        assert_eq!(p.kind, "peer-joined");
        assert_eq!(p.from, "peer-abc");
        assert_eq!(p.pubkey, "peer-abc");
    }

    #[test]
    fn assemble_peer_joined_envelope_is_stable_on_reconstruction() {
        let p = assemble_peer_joined_envelope("peer-xyz".to_string());
        let p2 = assemble_peer_joined_envelope(p.pubkey.clone());
        assert_eq!(p, p2);
    }

    #[test]
    fn assemble_peer_left_envelope_sets_type_and_sender() {
        let p = assemble_peer_left_envelope("peer-abc".to_string());
        assert_eq!(p.kind, "peer-left");
        assert_eq!(p.from, "peer-abc");
    }

    #[test]
    fn assemble_peer_left_envelope_is_stable_on_reconstruction() {
        let p = assemble_peer_left_envelope("peer-xyz".to_string());
        let p2 = assemble_peer_left_envelope(p.from.clone());
        assert_eq!(p, p2);
    }

    #[test]
    fn assemble_server_pack_reply_envelope_sets_server_sender_and_root() {
        let p = assemble_server_pack_reply_envelope(
            "nodes-b64".to_string(),
            "root-hex".to_string(),
        );
        assert_eq!(p.kind, "pack");
        assert_eq!(p.from, "server");
        assert_eq!(p.nodes, "nodes-b64");
        assert_eq!(p.root, "root-hex");
    }

    #[test]
    fn assemble_server_pack_reply_envelope_is_stable_on_reconstruction() {
        let p = assemble_server_pack_reply_envelope(
            "abc123".to_string(),
            "root1".to_string(),
        );
        let p2 = assemble_server_pack_reply_envelope(p.nodes.clone(), p.root.clone());
        assert_eq!(p, p2);
    }

    #[test]
    fn assemble_peer_pack_relay_envelope_sets_expected_fields() {
        let p = assemble_peer_pack_relay_envelope(
            "peer-abc".to_string(),
            "nodes-b64".to_string(),
            "root-hex".to_string(),
        );
        assert_eq!(p.kind, "pack");
        assert_eq!(p.from, "peer-abc");
        assert_eq!(p.nodes, "nodes-b64");
        assert_eq!(p.root, "root-hex");
    }

    #[test]
    fn serialize_archive_import_completed_envelope_preserves_wire_type() {
        let response = ArchiveWsResponse::ImportCompleted(ArchiveImported {
            room: "room-a".to_string(),
            archive_ref: "s3://bucket/room-a.nmar".to_string(),
            canonical_hash: "aa".repeat(32),
            checkpoint: ArchiveCheckpoint {
                frontier: vec!["seq:0".to_string()],
                canonical_hash: "aa".repeat(32),
            },
            imported_nodes: 10,
            imported_blobs: 2,
        });

        let json = serialize_archive_ws_response(&response)
            .expect("archive response should serialize");
        assert!(json.contains("\"type\":\"archive.import.completed\""));
        assert!(json.contains("\"imported_nodes\":10"));
    }

    #[test]
    fn serialize_archive_validate_rejected_envelope_preserves_reason_class() {
        let response = ArchiveWsResponse::ValidateRejected(ArchiveRejected {
            room: "room-a".to_string(),
            archive_ref: "s3://bucket/invalid-manifest.nmar".to_string(),
            reason_class: ArchiveReasonClass::ManifestInvalid,
            reason_message: "archive manifest failed schema validation".to_string(),
        });

        let json = serialize_archive_ws_response(&response)
            .expect("archive response should serialize");
        assert!(json.contains("\"type\":\"archive.validate.rejected\""));
        assert!(json.contains("reject.archive_manifest_invalid"));
    }

    #[test]
    fn serialize_topology_create_child_completed_preserves_wire_type() {
        let response = TopologyWsResponse::CreateChildCompleted(ChildRoomCreated {
            child_room_id: "child-a".to_string(),
            lineage: RoomLineage {
                parent_room_id: "parent-a".to_string(),
                parent_checkpoint: ParentCheckpoint {
                    frontier: vec!["seq:1".to_string()],
                    canonical_hash: "aa".repeat(32),
                    policy_timeline_hash: None,
                },
                child_purpose: "task".to_string(),
                created_by: "mgr".to_string(),
                created_at_hlc: 1,
                promotion_policy_id: "promotion-based".to_string(),
            },
        });
        let json = serialize_topology_ws_response(&response).expect("topology response should serialize");
        assert!(json.contains("\"type\":\"topology.create-child.completed\""));
        assert!(json.contains("\"child_room_id\":\"child-a\""));
    }

    #[test]
    fn serialize_topology_apply_promotion_completed_preserves_wire_type() {
        let response = TopologyWsResponse::ApplyPromotionCompleted(PromotionApplied {
            proposal_id: "prop-1".to_string(),
            parent_room_id: "parent-a".to_string(),
            parent_new_canonical_hash: "bb".repeat(32),
            audit_key: "_topology/promotion/prop-1".to_string(),
        });
        let json = serialize_topology_ws_response(&response).expect("topology response should serialize");
        assert!(json.contains("\"type\":\"topology.apply-promotion.completed\""));
        assert!(json.contains("\"audit_key\":\"_topology/promotion/prop-1\""));
    }

    #[test]
    fn serialize_archive_export_result_envelope_preserves_wire_type() {
        let response = ArchiveWsResponse::ExportResult(ArchiveExported {
            room: "room-a".to_string(),
            source_room: "room-a-source".to_string(),
            archive_ref: "file:///tmp/archive.json".to_string(),
            manifest_id: "m.room-a-source.aaaaaaaaaaaa".to_string(),
            checkpoint: ArchiveCheckpoint {
                frontier: vec!["seq:2".to_string()],
                canonical_hash: "aa".repeat(32),
            },
            payload_digest_set: nodalmerge_core::ArchivePayloadDigestSet {
                nodes: "sha256:nodes".to_string(),
                blobs: "sha256:blobs".to_string(),
            },
            compatibility_window: nodalmerge_core::ArchiveCompatibilityWindow {
                min_supported: "1".to_string(),
                max_supported: "2".to_string(),
            },
            payload_digest_policy: "strict_sha256_v1".to_string(),
            policy_timeline_hash: "bb".repeat(32),
            policy_timeline_cutover_lamport: 0,
            policy_timeline_transition_cutovers: vec![0],
        });

        let json = serialize_archive_ws_response(&response)
            .expect("archive response should serialize");
        assert!(json.contains("\"type\":\"archive.export.result\""));
        assert!(json.contains("\"manifest_id\":\"m.room-a-source.aaaaaaaaaaaa\""));
    }

    #[test]
    fn assemble_mst_response_envelope_sets_expected_fields() {
        let e = assemble_mst_response_envelope(vec!["n1".to_string(), "n2".to_string()]);
        assert_eq!(e.kind, "mst-response");
        assert_eq!(e.nodes, vec!["n1".to_string(), "n2".to_string()]);
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct RelayPayloadFixture {
        to: String,
    }

    #[test]
    fn assemble_peer_stamped_relay_envelope_sets_expected_fields() {
        let e = assemble_peer_stamped_relay_envelope(
            "webrtc-offer".to_string(),
            "peer-a".to_string(),
            RelayPayloadFixture {
                to: "peer-b".to_string(),
            },
        );
        assert_eq!(e.kind, "webrtc-offer");
        assert_eq!(e.from, "peer-a");
        assert_eq!(e.payload.to, "peer-b");
    }

    #[test]
    fn assemble_normalized_peer_stamped_relay_envelope_strips_type_and_from_fields() {
        let mut fields = Map::new();
        fields.insert("type".to_string(), Value::String("webrtc-answer".to_string()));
        fields.insert("from".to_string(), Value::String("spoofed".to_string()));
        fields.insert("to".to_string(), Value::String("peer-b".to_string()));

        let e = assemble_normalized_peer_stamped_relay_envelope("peer-a".to_string(), fields);

        assert_eq!(e.kind, "webrtc-answer");
        assert_eq!(e.from, "peer-a");
        assert_eq!(e.payload.fields.get("to"), Some(&Value::String("peer-b".to_string())));
        assert!(!e.payload.fields.contains_key("type"));
        assert!(!e.payload.fields.contains_key("from"));
    }

    #[test]
    fn assemble_subscribe_ack_envelope_sets_type() {
        let a = assemble_subscribe_ack_envelope();
        assert_eq!(a.kind, "subscribe-ack");
    }

    #[test]
    fn assemble_subscribe_ack_envelope_is_stable() {
        let a1 = assemble_subscribe_ack_envelope();
        let a2 = assemble_subscribe_ack_envelope();
        assert_eq!(a1, a2);
    }

    #[test]
    fn assemble_blob_redirect_envelope_sets_type_and_entries() {
        let r = vec![BlobRedirectEntry {
            hash: "h1".to_string(),
            url: "https://example.test/blob".to_string(),
            expires_at_unix: 123,
        }];
        let e = assemble_blob_redirect_envelope(r.clone());
        assert_eq!(e.kind, "blob-redirect");
        assert_eq!(e.redirects, r);
    }

    #[test]
    fn assemble_blob_redirect_envelope_is_stable_on_reconstruction() {
        let r = vec![BlobRedirectEntry {
            hash: "h2".to_string(),
            url: "https://example.test/blob2".to_string(),
            expires_at_unix: 456,
        }];
        let e1 = assemble_blob_redirect_envelope(r.clone());
        let e2 = assemble_blob_redirect_envelope(e1.redirects.clone());
        assert_eq!(e1, e2);
    }

    #[test]
    fn assemble_blob_pack_envelope_sets_type_and_payload() {
        let blobs = vec![BlobPackEntry {
            hash: "h1".to_string(),
            data: "b64data".to_string(),
        }];
        let requested = vec!["h1".to_string(), "h2".to_string()];
        let e = assemble_blob_pack_envelope(blobs.clone(), requested.clone());
        assert_eq!(e.kind, "blob-pack");
        assert_eq!(e.blobs, blobs);
        assert_eq!(e.requested, requested);
    }

    #[test]
    fn assemble_blob_pack_envelope_is_stable_on_reconstruction() {
        let blobs = vec![BlobPackEntry {
            hash: "h3".to_string(),
            data: "d3".to_string(),
        }];
        let requested = vec!["h3".to_string()];
        let e1 = assemble_blob_pack_envelope(blobs, requested);
        let e2 = assemble_blob_pack_envelope(e1.blobs.clone(), e1.requested.clone());
        assert_eq!(e1, e2);
    }

    #[test]
    fn assemble_upload_granted_envelope_sets_expected_fields() {
        let e = assemble_upload_granted_envelope(
            "h1".to_string(),
            "https://example.test/put".to_string(),
            123,
        );
        assert_eq!(e.kind, "upload-granted");
        assert_eq!(e.hash, "h1");
        assert_eq!(e.url, "https://example.test/put");
        assert_eq!(e.expires_at_unix, 123);
    }

    #[test]
    fn assemble_upload_denied_envelope_sets_expected_fields() {
        let e = assemble_upload_denied_envelope("h2".to_string(), "use-ws".to_string());
        assert_eq!(e.kind, "upload-denied");
        assert_eq!(e.hash, "h2");
        assert_eq!(e.reason, "use-ws");
    }

    #[test]
    fn assemble_upload_rejected_envelope_sets_expected_fields() {
        let e = assemble_upload_rejected_envelope("h3".to_string(), "verify-failed".to_string());
        assert_eq!(e.kind, "upload-rejected");
        assert_eq!(e.hash, "h3");
        assert_eq!(e.reason, "verify-failed");
    }

    #[test]
    fn assemble_blob_available_envelope_sets_expected_fields() {
        let e = assemble_blob_available_envelope(vec!["h1".to_string(), "h2".to_string()]);
        assert_eq!(e.kind, "blob-available");
        assert_eq!(e.hashes, vec!["h1".to_string(), "h2".to_string()]);
    }

    #[test]
    fn assemble_presence_envelope_sets_expected_fields() {
        let e = assemble_presence_envelope("peer-1".to_string(), "payload".to_string());
        assert_eq!(e.kind, "presence");
        assert_eq!(e.from, "peer-1");
        assert_eq!(e.data, "payload");
    }

    #[test]
    fn assemble_room_locked_envelope_sets_expected_fields() {
        let e = assemble_room_locked_envelope("pk1".to_string());
        assert_eq!(e.kind, "room-locked");
        assert_eq!(e.pubkey, "pk1");
    }

    #[test]
    fn assemble_set_room_key_rejected_envelope_sets_expected_fields() {
        let e = assemble_set_room_key_rejected_envelope("room already locked".to_string());
        assert_eq!(e.kind, "error");
        assert_eq!(e.msg, "room already locked");
    }

    #[test]
    fn assemble_error_envelope_sets_expected_fields() {
        let e = assemble_error_envelope("invalid JSON".to_string());
        assert_eq!(e.kind, "error");
        assert_eq!(e.msg, "invalid JSON");
    }

    #[test]
    fn assemble_policy_set_envelope_sets_expected_fields() {
        let e = assemble_policy_set_envelope();
        assert_eq!(e.kind, "policy-set");
    }

    #[test]
    fn assemble_server_info_envelope_sets_expected_fields() {
        let e = assemble_server_info_envelope("pk1".to_string());
        assert_eq!(e.kind, "server-info");
        assert_eq!(e.pubkey, "pk1");
    }

    #[test]
    fn assemble_tick_start_envelope_sets_started_variant() {
        let e = assemble_tick_start_envelope(true, 16);
        assert_eq!(e.kind, "tick-started");
        assert_eq!(e.interval_ms, 16);
    }

    #[test]
    fn assemble_tick_start_envelope_sets_already_running_variant() {
        let e = assemble_tick_start_envelope(false, 33);
        assert_eq!(e.kind, "tick-already-running");
        assert_eq!(e.interval_ms, 33);
    }

    #[test]
    fn assemble_tick_stopped_envelope_sets_expected_fields() {
        let e = assemble_tick_stopped_envelope();
        assert_eq!(e.kind, "tick-stopped");
    }

    #[test]
    fn assemble_snapshot_pack_envelope_sets_expected_fields() {
        let e = assemble_snapshot_pack_envelope("packb64".to_string(), "h1".to_string());
        assert_eq!(e.kind, "snapshot-pack");
        assert_eq!(e.pack_b64, "packb64");
        assert_eq!(e.snapshot_hash, "h1");
    }

    #[test]
    fn assemble_compact_ack_envelope_sets_expected_fields() {
        let e = assemble_compact_ack_envelope("h2".to_string());
        assert_eq!(e.kind, "compact-ack");
        assert_eq!(e.snapshot_hash, "h2");
    }

    #[test]
    fn assemble_token_expired_close_frame_sets_expected_fields() {
        let c = assemble_token_expired_close_frame();
        assert_eq!(c.code, 4002);
        assert_eq!(c.reason, "token expired");
    }

    #[test]
    fn assemble_resync_required_close_frame_sets_expected_fields() {
        let c = assemble_resync_required_close_frame();
        assert_eq!(c.code, 4001);
        assert_eq!(c.reason, "resync required");
    }

    #[test]
    fn assemble_server_overload_close_frame_sets_expected_fields() {
        let c = assemble_server_overload_close_frame();
        assert_eq!(c.code, 1011);
        assert_eq!(c.reason, "server overload");
    }

    #[test]
    fn assemble_rate_limit_exceeded_close_frame_sets_expected_fields() {
        let c = assemble_rate_limit_exceeded_close_frame();
        assert_eq!(c.code, 4008);
        assert_eq!(c.reason, "rate limit exceeded");
    }
}
