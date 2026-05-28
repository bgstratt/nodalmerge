use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use nodalmerge_core::{ParentCheckpoint, RoomToken};
use serde_json::Value;

use crate::ws_session::{WsSessionConfig, build_ws_url, send_topology_message};

#[derive(Debug, thiserror::Error)]
pub enum TopologyCliError {
    #[error("{0}")]
    Msg(String),
}

pub struct TopologyGlobalOpts {
    pub server: Option<String>,
    pub room: Option<String>,
    pub token_json: Option<String>,
    pub timeout_secs: u64,
    pub peer_seed: [u8; 32],
}

#[derive(Debug, Clone)]
pub enum TopologyCommand {
    CreateChild {
        parent_room: String,
        child_room: String,
        purpose: String,
        policy: String,
        created_by: Option<String>,
        parent_checkpoint_file: PathBuf,
    },
    ListChildren {
        parent_room: String,
    },
    ShowLineage {
        room: String,
    },
    ProposePromotion {
        parent_room: String,
        child_room: String,
        child_checkpoint: String,
        payload_ref: String,
        idempotency_key: Option<String>,
    },
    ValidatePromotion {
        proposal_id: String,
    },
    ApplyPromotion {
        proposal_id: String,
    },
}

pub async fn run_topology_command(
    globals: &TopologyGlobalOpts,
    cmd: TopologyCommand,
) -> Result<Value, TopologyCliError> {
    let room_id = globals
        .room
        .clone()
        .or_else(|| match &cmd {
            TopologyCommand::CreateChild { parent_room, .. } => Some(parent_room.clone()),
            TopologyCommand::ListChildren { parent_room } => Some(parent_room.clone()),
            TopologyCommand::ShowLineage { room } => Some(room.clone()),
            TopologyCommand::ProposePromotion { parent_room, .. } => Some(parent_room.clone()),
            TopologyCommand::ValidatePromotion { .. } => None,
            TopologyCommand::ApplyPromotion { .. } => None,
        })
        .ok_or_else(|| TopologyCliError::Msg("room id required (flag or NODALMERGE_ROOM)".into()))?;

    let server = globals
        .server
        .clone()
        .or_else(|| std::env::var("NODALMERGE_SERVER_URL").ok())
        .ok_or_else(|| {
            TopologyCliError::Msg("server URL required (--server or NODALMERGE_SERVER_URL)".into())
        })?;

    let token = parse_token(globals)?;
    let ws_url = build_ws_url(&server, &room_id);
    let cfg = WsSessionConfig {
        ws_url,
        room_id,
        peer_seed: globals.peer_seed,
        token,
        timeout: Duration::from_secs(globals.timeout_secs.max(1)),
    };

    let message = command_message(cmd)?;
    send_topology_message(&cfg, message)
        .await
        .map_err(TopologyCliError::Msg)
}

pub fn parse_token(globals: &TopologyGlobalOpts) -> Result<Option<RoomToken>, TopologyCliError> {
    let raw = globals
        .token_json
        .clone()
        .or_else(|| std::env::var("NODALMERGE_TOKEN_JSON").ok());
    let Some(raw) = raw else {
        return Ok(None);
    };
    let v: Value = serde_json::from_str(&raw)
        .map_err(|e| TopologyCliError::Msg(format!("invalid token JSON: {e}")))?;
    let peer = v
        .get("peer_pubkey")
        .and_then(|x| x.as_str())
        .ok_or_else(|| TopologyCliError::Msg("token.peer_pubkey required".into()))?;
    let expiry = v
        .get("expiry")
        .and_then(|x| x.as_u64())
        .ok_or_else(|| TopologyCliError::Msg("token.expiry required".into()))?;
    let caps: Vec<String> = v
        .get("caps")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|c| c.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let sig = v
        .get("sig")
        .and_then(|x| x.as_str())
        .ok_or_else(|| TopologyCliError::Msg("token.sig required".into()))?;
    RoomToken::from_wire(peer, expiry, caps, sig)
        .map(Some)
        .map_err(|e| TopologyCliError::Msg(format!("token parse: {e}")))
}

fn command_message(cmd: TopologyCommand) -> Result<Value, TopologyCliError> {
    Ok(match cmd {
        TopologyCommand::CreateChild {
            parent_room,
            child_room,
            purpose,
            policy,
            created_by,
            parent_checkpoint_file,
        } => {
            let text = fs::read_to_string(&parent_checkpoint_file).map_err(|e| {
                TopologyCliError::Msg(format!("read parent checkpoint file: {e}"))
            })?;
            let checkpoint: ParentCheckpoint = serde_json::from_str(&text).map_err(|e| {
                TopologyCliError::Msg(format!("parse parent checkpoint JSON: {e}"))
            })?;
            serde_json::json!({
                "type": "topology.create-child",
                "parent_room_id": parent_room,
                "child_room_id": child_room,
                "child_purpose": purpose,
                "promotion_policy_id": policy,
                "created_by": created_by.unwrap_or_else(|| "cli".to_string()),
                "parent_checkpoint": checkpoint,
            })
        }
        TopologyCommand::ListChildren { parent_room } => serde_json::json!({
            "type": "topology.list-children",
            "parent_room_id": parent_room,
        }),
        TopologyCommand::ShowLineage { room } => serde_json::json!({
            "type": "topology.describe-lineage",
            "room_id": room,
        }),
        TopologyCommand::ProposePromotion {
            parent_room,
            child_room,
            child_checkpoint,
            payload_ref,
            idempotency_key,
        } => {
            let mut msg = serde_json::json!({
                "type": "topology.propose-promotion",
                "parent_room_id": parent_room,
                "child_room_id": child_room,
                "child_checkpoint_hash": child_checkpoint,
                "payload_ref": payload_ref,
            });
            if let Some(key) = idempotency_key {
                msg["idempotency_key"] = Value::String(key);
            }
            msg
        }
        TopologyCommand::ValidatePromotion { proposal_id } => serde_json::json!({
            "type": "topology.validate-promotion",
            "proposal_id": proposal_id,
        }),
        TopologyCommand::ApplyPromotion { proposal_id } => serde_json::json!({
            "type": "topology.apply-promotion",
            "proposal_id": proposal_id,
        }),
    })
}
