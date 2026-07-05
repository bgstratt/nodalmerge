use std::time::Duration;

use serde_json::Value;

use crate::topology::{parse_token, TopologyCliError, TopologyGlobalOpts};
use crate::ws_session::{build_ws_url, WsSessionConfig};

pub fn ws_session_for_room(
    globals: &TopologyGlobalOpts,
    room_id: &str,
) -> Result<WsSessionConfig, TopologyCliError> {
    let server = globals
        .server
        .clone()
        .or_else(|| std::env::var("NODALMERGE_SERVER_URL").ok())
        .ok_or_else(|| {
            TopologyCliError::Msg("server URL required (--server or NODALMERGE_SERVER_URL)".into())
        })?;
    let token = parse_token(globals)?;
    Ok(WsSessionConfig {
        ws_url: build_ws_url(&server, room_id),
        room_id: room_id.to_string(),
        peer_seed: globals.peer_seed,
        token,
        timeout: Duration::from_secs(globals.timeout_secs.max(1)),
    })
}

pub fn load_json_file(path: &std::path::Path) -> Result<Value, TopologyCliError> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| TopologyCliError::Msg(format!("read {}: {e}", path.display())))?;
    serde_json::from_str(&raw)
        .map_err(|e| TopologyCliError::Msg(format!("parse JSON {}: {e}", path.display())))
}
