use std::path::PathBuf;

use serde_json::Value;

use crate::session::{load_json_file, ws_session_for_room};
use crate::topology::TopologyGlobalOpts;
use crate::ws_session::send_ws_command;

#[derive(Debug, thiserror::Error)]
pub enum QueryCliError {
    #[error("{0}")]
    Msg(String),
}

#[derive(Debug, Clone)]
pub enum QueryCommand {
    RegisterSpec {
        query_spec_id: String,
        version: String,
        descriptor_file: PathBuf,
    },
    BuildProjection {
        projection_id: String,
        query_spec_id: String,
        selector: String,
        canonical_seq: Option<u64>,
    },
    ListProjections {
        query_spec_id: Option<String>,
    },
    ReadProjection {
        projection_id: String,
        limit: u64,
        page_token: Option<String>,
    },
    InvalidateProjection {
        projection_id: String,
        reason: String,
    },
}

pub async fn run_query_command(
    globals: &TopologyGlobalOpts,
    cmd: QueryCommand,
) -> Result<Value, QueryCliError> {
    let room_id = globals.room.clone().ok_or_else(|| {
        QueryCliError::Msg("room id required (--room or NODALMERGE_ROOM)".into())
    })?;
    let cfg = ws_session_for_room(globals, &room_id)
        .map_err(|e| QueryCliError::Msg(e.to_string()))?;

    let (message, success_types): (Value, &[&str]) = match cmd {
        QueryCommand::RegisterSpec {
            query_spec_id,
            version,
            descriptor_file,
        } => {
            let descriptor = load_json_file(&descriptor_file)
                .map_err(|e| QueryCliError::Msg(e.to_string()))?;
            (
                serde_json::json!({
                    "type": "query.register",
                    "query_spec_id": query_spec_id,
                    "version": version,
                    "descriptor": descriptor,
                }),
                &["query.registered", "query.register.rejected"],
            )
        }
        QueryCommand::BuildProjection {
            projection_id,
            query_spec_id,
            selector,
            canonical_seq,
        } => {
            let mut target_checkpoint = serde_json::json!({ "selector": selector });
            if let Some(seq) = canonical_seq {
                target_checkpoint["canonical_seq"] = serde_json::json!(seq);
            }
            (
                serde_json::json!({
                    "type": "projection.build",
                    "projection_id": projection_id,
                    "query_spec_id": query_spec_id,
                    "target_checkpoint": target_checkpoint,
                }),
                &["projection.build.completed", "projection.build.rejected"],
            )
        }
        QueryCommand::ListProjections { query_spec_id } => (
            serde_json::json!({
                "type": "projection.list",
                "query_spec_id": query_spec_id,
            }),
            &["projection.list.result", "projection.list.rejected"],
        ),
        QueryCommand::ReadProjection {
            projection_id,
            limit,
            page_token,
        } => {
            let mut msg = serde_json::json!({
                "type": "projection.read",
                "projection_id": projection_id,
                "limit": limit,
            });
            if let Some(token) = page_token.filter(|t| !t.is_empty()) {
                msg["page_token"] = Value::String(token);
            }
            (
                msg,
                &["projection.read.result", "projection.read.rejected"],
            )
        }
        QueryCommand::InvalidateProjection {
            projection_id,
            reason,
        } => (
            serde_json::json!({
                "type": "projection.invalidate",
                "projection_id": projection_id,
                "reason": reason,
            }),
            &[
                "projection.invalidated",
                "projection.invalidate.rejected",
            ],
        ),
    };

    send_ws_command(&cfg, message, success_types)
        .await
        .map_err(QueryCliError::Msg)
}
