//! Post-handshake catch-up: IBF-aware hello, MST descent, and pack application.

use std::collections::{HashSet, VecDeque};
use std::time::Instant;

use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use nodalmerge_core::{
    unpack_nodes, Hash, Ibf, MerkleSearchTree, MstNodeWire, NodeId, SyncCapabilities, SyncNode,
};
use nodalmerge_runtime_local::PeerLocalPersistence;
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::config::SyncSessionStats;

const MST_BATCH: usize = 16;
const MST_MAX_ROUNDS: usize = 64;

pub fn local_node_ids(nodes: &[SyncNode]) -> Vec<NodeId> {
    nodes.iter().map(|n| n.id).collect()
}

pub fn build_hello_extras(
    nodes: &[SyncNode],
    negotiate_ibf: bool,
    negotiate_mst: bool,
) -> (serde_json::Value, bool) {
    let ids = local_node_ids(nodes);
    let mut hello_extra = serde_json::json!({
        "caps": {
            "supports_ibf": negotiate_ibf,
            "supports_mst": negotiate_mst && negotiate_ibf,
        }
    });
    let mut used_ibf = false;

    if negotiate_ibf && !ids.is_empty() {
        let ibf_b64 = base64::engine::general_purpose::STANDARD.encode(Ibf::from_ids(&ids).encode());
        hello_extra["ibf"] = serde_json::Value::String(ibf_b64);
        used_ibf = true;
    }
    if negotiate_mst && negotiate_ibf && !ids.is_empty() {
        let mst_root = MerkleSearchTree::from_ids(&ids).root_hash_hex();
        hello_extra["mst_root"] = serde_json::Value::String(mst_root);
    }

    (hello_extra, used_ibf)
}

pub fn parse_welcome_caps(welcome: &Value) -> SyncCapabilities {
    welcome
        .get("caps")
        .cloned()
        .and_then(|c| serde_json::from_value(c).ok())
        .unwrap_or_default()
}

pub fn decode_pack_nodes(v: &Value) -> Option<Vec<SyncNode>> {
    let nodes_b64 = v.get("nodes")?.as_str()?;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(nodes_b64)
        .ok()?;
    unpack_nodes(&raw).ok()
}

pub fn apply_pack_to_persist<P: PeerLocalPersistence>(
    persist: &P,
    room: &str,
    nodes: &[SyncNode],
) -> Result<(), String> {
    if nodes.is_empty() {
        return Ok(());
    }
    let tail = persist.hydrate(room).ok().map(|h| h.tail);
    persist
        .append_nodes(room, nodes, tail)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// After `welcome`, run MST descent + `mst-done` when negotiated and roots differ.
pub async fn run_mst_catchup<S>(
    welcome: &Value,
    negotiated: &SyncCapabilities,
    local_ids: &[NodeId],
    sink: &mut S,
    stream: &mut (impl StreamExt<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin),
    deadline: Instant,
    stats: &mut SyncSessionStats,
) -> Result<Vec<SyncNode>, String>
where
    S: SinkExt<WsMessage> + Unpin,
    S::Error: std::fmt::Display,
{
    if !negotiated.supports_mst {
        return Ok(vec![]);
    }
    let Some(server_root) = welcome.get("mst_root").and_then(|v| v.as_str()) else {
        return Ok(vec![]);
    };

    let local_mst = MerkleSearchTree::from_ids(local_ids);
    if local_mst.root_hash_hex() == server_root {
        tracing::debug!("MST roots match — skipping descent");
        return Ok(vec![]);
    }

    let mut queue: VecDeque<String> = VecDeque::from([String::new()]);
    let mut need_ids: Vec<String> = Vec::new();
    let local_key_set: HashSet<NodeId> = local_ids.iter().copied().collect();

    for round in 0..MST_MAX_ROUNDS {
        if queue.is_empty() {
            break;
        }
        if Instant::now() >= deadline {
            return Err("MST catch-up timed out".into());
        }

        let mut batch = Vec::new();
        while batch.len() < MST_BATCH {
            match queue.pop_front() {
                Some(p) => batch.push(p),
                None => break,
            }
        }
        if batch.is_empty() {
            break;
        }

        let req = serde_json::json!({
            "type": "mst-request",
            "paths": batch,
        });
        sink.send(WsMessage::Text(req.to_string().into()))
            .await
            .map_err(|e| format!("send mst-request: {e}"))?;
        stats.mst_requests += 1;

        let response = recv_until(deadline, stream, |v| {
            v.get("type").and_then(|t| t.as_str()) == Some("mst-response")
        })
        .await?;

        let wires: Vec<MstNodeWire> = response
            .get("nodes")
            .and_then(|n| serde_json::from_value(n.clone()).ok())
            .unwrap_or_default();

        for server_wire in wires {
            need_ids.extend(collect_missing_key_hex(
                &local_mst,
                &local_key_set,
                &server_wire,
            ));
            for child_path in divergent_child_paths(&local_mst, &server_wire) {
                queue.push_back(child_path);
            }
        }

        if round + 1 >= MST_MAX_ROUNDS && !queue.is_empty() {
            tracing::warn!("MST descent hit round cap with pending paths");
            break;
        }
    }

    if need_ids.is_empty() {
        return Ok(vec![]);
    }

    let done = serde_json::json!({
        "type": "mst-done",
        "ids": need_ids,
    });
    sink.send(WsMessage::Text(done.to_string().into()))
        .await
        .map_err(|e| format!("send mst-done: {e}"))?;

    let pack = recv_until(deadline, stream, |v| {
        v.get("type").and_then(|t| t.as_str()) == Some("pack")
            && v.get("nodes").is_some()
    })
    .await?;

    let nodes = decode_pack_nodes(&pack).unwrap_or_default();
    stats.mst_nodes_fetched = nodes.len();
    Ok(nodes)
}

fn collect_missing_key_hex(
    local_mst: &MerkleSearchTree,
    local_keys: &HashSet<NodeId>,
    server_wire: &MstNodeWire,
) -> Vec<String> {
    if server_wire.keys.is_empty() {
        return vec![];
    }
    let local_at_path: HashSet<String> = local_mst
        .get_node_wire(&server_wire.path)
        .map(|w| w.keys.iter().cloned().collect())
        .unwrap_or_default();
    server_wire
        .keys
        .iter()
        .filter(|k| !local_at_path.contains(*k))
        .filter(|k| parse_node_id_hex(k).is_none_or(|id| !local_keys.contains(&id)))
        .cloned()
        .collect()
}

fn divergent_child_paths(local_mst: &MerkleSearchTree, server_wire: &MstNodeWire) -> Vec<String> {
    if !server_wire.keys.is_empty() {
        return vec![];
    }
    let Some(local_wire) = local_mst.get_node_wire(&server_wire.path) else {
        return server_wire
            .children
            .keys()
            .map(|nib| format!("{}{}", server_wire.path, nib))
            .collect();
    };
    if local_wire.hash == server_wire.hash {
        return vec![];
    }
    let mut out = Vec::new();
    for (nib, srv_hash) in &server_wire.children {
        let loc = local_wire.children.get(nib).map(|s| s.as_str());
        if loc != Some(srv_hash.as_str()) {
            out.push(format!("{}{}", server_wire.path, nib));
        }
    }
    out
}

async fn recv_until(
    deadline: Instant,
    stream: &mut (impl StreamExt<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin),
    mut pred: impl FnMut(&Value) -> bool,
) -> Result<Value, String> {
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let next = tokio::time::timeout(
            remaining.min(std::time::Duration::from_millis(500)),
            stream.next(),
        )
        .await;

        match next {
            Ok(Some(Ok(WsMessage::Text(text)))) => {
                if text.contains("reject.") {
                    return Err(text);
                }
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    if pred(&v) {
                        return Ok(v);
                    }
                }
            }
            Ok(Some(Ok(WsMessage::Close(_)))) => return Err("websocket closed".into()),
            Ok(Some(Ok(_))) | Ok(None) => {}
            Ok(Some(Err(e))) => return Err(format!("websocket error: {e}")),
            Err(_) => return Err("timed out waiting for protocol message".into()),
        }
    }
    Err("timed out waiting for protocol message".into())
}

fn parse_node_id_hex(hex: &str) -> Option<NodeId> {
    if hex.len() != 64 {
        return None;
    }
    let mut bytes = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        if i >= 32 {
            return None;
        }
        let hi = (chunk[0] as char).to_digit(16)? as u8;
        let lo = (chunk.get(1).copied()? as char).to_digit(16)? as u8;
        bytes[i] = (hi << 4) | lo;
    }
    Some(Hash(bytes))
}
