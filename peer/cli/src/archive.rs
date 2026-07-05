use std::path::PathBuf;

use serde_json::Value;

use crate::session::{load_json_file, ws_session_for_room};
use crate::topology::TopologyGlobalOpts;
use crate::ws_session::send_ws_command;

#[derive(Debug, thiserror::Error)]
pub enum ArchiveCliError {
    #[error("{0}")]
    Msg(String),
}

#[derive(Debug, Clone)]
pub enum ArchiveCommand {
    Describe {
        archive_ref: String,
    },
    Validate {
        archive_ref: String,
        mode: String,
    },
    Export {
        source_room: String,
        archive_ref: String,
    },
    Import {
        archive_ref: String,
        import_mode: String,
        expected_checkpoint_file: Option<PathBuf>,
    },
}

pub async fn run_archive_command(
    globals: &TopologyGlobalOpts,
    cmd: ArchiveCommand,
) -> Result<Value, ArchiveCliError> {
    let room_id = globals.room.clone().ok_or_else(|| {
        ArchiveCliError::Msg("room id required (--room or NODALMERGE_ROOM)".into())
    })?;
    let cfg = ws_session_for_room(globals, &room_id)
        .map_err(|e| ArchiveCliError::Msg(e.to_string()))?;

    let (message, success_types): (Value, &[&str]) = match cmd {
        ArchiveCommand::Describe { archive_ref } => (
            serde_json::json!({
                "type": "archive.describe",
                "archive_ref": archive_ref,
            }),
            &["archive.describe.result"],
        ),
        ArchiveCommand::Validate { archive_ref, mode } => (
            serde_json::json!({
                "type": "archive.validate",
                "archive_ref": archive_ref,
                "mode": mode,
            }),
            &["archive.validate.result"],
        ),
        ArchiveCommand::Export {
            source_room,
            archive_ref,
        } => (
            serde_json::json!({
                "type": "archive.export",
                "source_room": source_room,
                "archive_ref": archive_ref,
            }),
            &["archive.export.result"],
        ),
        ArchiveCommand::Import {
            archive_ref,
            import_mode,
            expected_checkpoint_file,
        } => {
            let mut msg = serde_json::json!({
                "type": "archive.import",
                "archive_ref": archive_ref,
                "import_mode": import_mode,
            });
            if let Some(path) = expected_checkpoint_file {
                let checkpoint = load_json_file(&path)
                    .map_err(|e| ArchiveCliError::Msg(e.to_string()))?;
                msg["expected_checkpoint"] = checkpoint;
            }
            (msg, &["archive.import.completed", "archive.import.rejected"])
        }
    };

    send_ws_command(&cfg, message, success_types)
        .await
        .map_err(ArchiveCliError::Msg)
}
