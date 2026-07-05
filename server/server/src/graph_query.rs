//! WS routes for the `graph.*` introspection commands (plan S4).
//!
//! These are the first server routes whose semantics live in the shared
//! engine layer: each handler takes the room's `StateGraph` read lock and
//! calls the same `nodalmerge_host_core::engine::graph_*` functions that
//! `HostEngine::apply` runs for the FFI/.NET path, then shapes the exact
//! outbound envelope the .NET host emits (`frontier-queried`,
//! `causal-parents-queried`, `canonical-resolution-queried`,
//! `sync-diff-computed` — see RuntimeProtocolMapper.cs's event mapping).
//! Capability gating (`query.admin`) is enforced by the dispatcher in
//! `ws_handler.rs`, same as every other control-plane command.

use nodalmerge_host_core::engine::{
    graph_canonical_resolution_entries, graph_causal_parents_hex, graph_frontier_heads_hex,
    graph_sync_diff_hex,
};
use serde_json::{json, Value};

use crate::room::Room;

pub async fn process_graph_get_frontier(room: &Room, room_id: &str) -> Value {
    let graph = room.graph.read().await;
    json!({
        "type": "frontier-queried",
        "room": room_id,
        "frontier": graph_frontier_heads_hex(&graph),
    })
}

pub async fn process_graph_get_causal_parents(room: &Room, room_id: &str, msg: &Value) -> Value {
    let node_id_hex = msg
        .get("node_id_hex")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let graph = room.graph.read().await;
    let (parent_ids, node_found) = graph_causal_parents_hex(&graph, &node_id_hex);
    json!({
        "type": "causal-parents-queried",
        "room": room_id,
        "node_id_hex": node_id_hex,
        "parent_ids": parent_ids,
        "node_found": node_found,
    })
}

pub async fn process_graph_get_canonical_resolution(room: &Room, room_id: &str) -> Value {
    let graph = room.graph.read().await;
    let entries = graph_canonical_resolution_entries(&graph);
    let entry_count = entries.len();
    json!({
        "type": "canonical-resolution-queried",
        "room": room_id,
        "entries": entries,
        "entry_count": entry_count,
    })
}

pub async fn process_graph_compute_sync_diff(room: &Room, room_id: &str, msg: &Value) -> Value {
    let peer_node_ids_hex: Vec<String> = msg
        .get("peer_node_ids_hex")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let graph = room.graph.read().await;
    let (only_in_server, only_in_peer) = graph_sync_diff_hex(&graph, &peer_node_ids_hex);
    json!({
        "type": "sync-diff-computed",
        "room": room_id,
        "only_in_server": only_in_server,
        "only_in_peer": only_in_peer,
    })
}
