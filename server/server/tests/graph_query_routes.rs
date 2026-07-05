//! S4: the `graph.*` WS routes on nodalmerge-server must produce the same
//! answers as the shared engine functions they delegate to, in the same
//! envelope shapes the .NET host emits for the equivalent HostEvents.

use std::sync::Arc;

use ed25519_dalek::SigningKey;
use nodalmerge_core::{MapOp, Op, StateGraph};
use nodalmerge_server::graph_query::{
    process_graph_compute_sync_diff, process_graph_get_canonical_resolution,
    process_graph_get_causal_parents, process_graph_get_frontier,
};
use nodalmerge_server::room::{import_nodes, Room};
use nodalmerge_server::store::{NoPersistence, SharedPersistence};
use serde_json::json;

fn build_set_nodes(sk: &SigningKey, writes: &[(&str, &str)]) -> Vec<nodalmerge_core::SyncNode> {
    let mut g = StateGraph::new();
    let mut out = Vec::with_capacity(writes.len());
    for (k, v) in writes {
        let id = g
            .apply_local(
                sk,
                0,
                vec![Op::Map(MapOp::Set {
                    key: (*k).to_string(),
                    value: v.as_bytes().to_vec(),
                })],
            )
            .expect("apply_local must succeed");
        out.push(g.get_nodes(&[id]).into_iter().next().unwrap().clone());
    }
    out
}

async fn seeded_room() -> (Arc<Room>, Vec<String>) {
    let persistence: SharedPersistence = Arc::new(NoPersistence);
    let room = Room::new("graph-query-room".to_string(), persistence, 512);
    let sk = SigningKey::from_bytes(&[0x42u8; 32]);
    let nodes = build_set_nodes(&sk, &[("world/a", "1"), ("world/b", "2")]);
    let node_ids: Vec<String> = nodes.iter().map(|n| n.id.to_hex()).collect();
    let (accepted, _, errs) = import_nodes(&room, nodes).await;
    assert_eq!(accepted, 2);
    assert!(errs.is_empty());
    (room, node_ids)
}

#[tokio::test]
async fn frontier_route_reports_graph_frontier() {
    let (room, node_ids) = seeded_room().await;
    let response = process_graph_get_frontier(&room, "graph-query-room").await;

    assert_eq!(response["type"], "frontier-queried");
    assert_eq!(response["room"], "graph-query-room");
    let frontier: Vec<&str> = response["frontier"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    // The two writes chain (second's parent is first), so the frontier is
    // exactly the second node.
    assert_eq!(frontier, vec![node_ids[1].as_str()]);
}

#[tokio::test]
async fn causal_parents_route_reports_parents_and_missing_nodes() {
    let (room, node_ids) = seeded_room().await;

    let response =
        process_graph_get_causal_parents(&room, "graph-query-room", &json!({ "node_id_hex": node_ids[1] }))
            .await;
    assert_eq!(response["type"], "causal-parents-queried");
    assert_eq!(response["node_found"], true);
    assert_eq!(
        response["parent_ids"].as_array().unwrap()[0].as_str().unwrap(),
        node_ids[0]
    );

    let missing = process_graph_get_causal_parents(
        &room,
        "graph-query-room",
        &json!({ "node_id_hex": "ff".repeat(32) }),
    )
    .await;
    assert_eq!(missing["node_found"], false);
    assert!(missing["parent_ids"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn canonical_resolution_route_reports_sorted_entries() {
    let (room, _) = seeded_room().await;
    let response = process_graph_get_canonical_resolution(&room, "graph-query-room").await;

    assert_eq!(response["type"], "canonical-resolution-queried");
    assert_eq!(response["entry_count"], 2);
    let entries = response["entries"].as_array().unwrap();
    assert_eq!(entries[0]["key"], "world/a");
    assert_eq!(entries[1]["key"], "world/b");
    // Values are base64 of the raw bytes ("1" => "MQ==").
    assert_eq!(entries[0]["value_bytes_b64"], "MQ==");
}

#[tokio::test]
async fn sync_diff_route_reports_both_directions_sorted() {
    let (room, node_ids) = seeded_room().await;
    let phantom = "ab".repeat(32);

    let response = process_graph_compute_sync_diff(
        &room,
        "graph-query-room",
        &json!({ "peer_node_ids_hex": [node_ids[0], phantom] }),
    )
    .await;

    assert_eq!(response["type"], "sync-diff-computed");
    let only_in_server: Vec<&str> = response["only_in_server"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let only_in_peer: Vec<&str> = response["only_in_peer"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(only_in_server, vec![node_ids[1].as_str()]);
    assert_eq!(only_in_peer, vec![phantom.as_str()]);
}
