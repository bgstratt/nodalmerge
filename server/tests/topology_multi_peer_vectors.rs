//! AUTH-TOPOLOGY-007: multi-peer WS promotion workflow parity.

use std::sync::Arc;
use std::time::Duration;

use axum::{routing::get, Router};
use ed25519_dalek::SigningKey;
use futures_util::{SinkExt, StreamExt};
use nodalmerge_core::{MapOp, Op, RoomToken, StateGraph, TopologyWsResponse};
use nodalmerge_host_core::protocol::serialize_topology_ws_response;
use nodalmerge_server::lineage::snapshot_room_canonical_hash;
use nodalmerge_server::room::{import_nodes, Rooms};
use nodalmerge_server::store::{NoPersistence, SharedPersistence};
use nodalmerge_server::topology_adapter::{
    apply_promotion_completed, propose_promotion_completed, validate_promotion_completed,
};
use nodalmerge_server::ws_handler;
use tokio_tungstenite::tungstenite::Message as TMessage;

fn make_map_set_node(sk: &SigningKey, key: &str, val: &[u8]) -> nodalmerge_core::SyncNode {
    let mut g = StateGraph::new();
    let id = g
        .apply_local(
            sk,
            0,
            vec![Op::Map(MapOp::Set {
                key: key.into(),
                value: val.to_vec(),
            })],
        )
        .expect("apply_local");
    g.get_nodes(&[id]).into_iter().next().unwrap().clone()
}

fn hex_lower(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(H[(b >> 4) as usize] as char);
        s.push(H[(b & 0x0f) as usize] as char);
    }
    s
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn token_json(tok: &RoomToken) -> serde_json::Value {
    serde_json::json!({
        "peer_pubkey": hex_lower(&tok.peer_pubkey),
        "expiry": tok.expiry_secs,
        "caps": tok.capabilities,
        "sig": hex_lower(&tok.signature),
    })
}

async fn spawn_server() -> (std::net::SocketAddr, Rooms, SigningKey) {
    let server_key = SigningKey::from_bytes(&[0xB1u8; 32]);
    let persistence: SharedPersistence = Arc::new(NoPersistence);
    let rooms = Rooms::new(server_key.clone(), persistence, 512, 0, 0);
    let app = Router::new()
        .route("/ws/:room_id", get(ws_handler::handler))
        .with_state(rooms.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    (addr, rooms, server_key)
}

async fn connect_topology_peer(
    addr: std::net::SocketAddr,
    room_id: &str,
    peer_sk: &SigningKey,
    room_key: &SigningKey,
) -> (
    futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        TMessage,
    >,
    futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) {
    let url = format!("ws://{addr}/ws/{room_id}");
    let (ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let (mut sink, mut stream) = ws.split();
    let token = RoomToken::sign(
        room_id,
        &peer_sk.verifying_key().to_bytes(),
        now_secs() + 300,
        &["topology.admin".to_string()],
        room_key,
    );
    let hello = serde_json::json!({
        "type": "hello",
        "pubkey": hex_lower(&peer_sk.verifying_key().to_bytes()),
        "frontier": [],
        "token": token_json(&token),
    });
    sink.send(TMessage::Text(hello.to_string().into()))
        .await
        .expect("hello");
    drain_until_welcome(&mut stream).await;
    (sink, stream)
}

async fn drain_until_welcome(
    stream: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(TMessage::Text(t)))) =
            tokio::time::timeout(Duration::from_millis(300), stream.next()).await
        {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                if v.get("type").and_then(|x| x.as_str()) == Some("welcome") {
                    return;
                }
            }
        }
    }
    panic!("never received welcome");
}

async fn expect_topology_response(
    stream: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    expected_type: &str,
) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(TMessage::Text(t)))) =
            tokio::time::timeout(Duration::from_millis(500), stream.next()).await
        {
            let v: serde_json::Value = serde_json::from_str(&t).expect("ws text should be json");
            if v.get("type").and_then(|x| x.as_str()) == Some(expected_type) {
                return v;
            }
            if v.get("type").and_then(|x| x.as_str()) == Some("error") {
                panic!("unexpected error envelope: {t}");
            }
        }
    }
    panic!("timed out waiting for {expected_type}");
}

/// Peer A proposes on parent room; peer B validates and applies on the same parent WS session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_topology_007_two_peer_promotion_workflow() {
    let parent_id = "auth-topo-multi-parent";
    let child_id = "auth-topo-multi-child";
    let (addr, rooms, _server_key) = spawn_server().await;
    let room_key = SigningKey::from_bytes(&[0xB2u8; 32]);
    let peer_a = SigningKey::from_bytes(&[0xB3u8; 32]);
    let peer_b = SigningKey::from_bytes(&[0xB4u8; 32]);

    let parent = rooms.get_or_create(parent_id).await;
    *parent.auth_key.write().await = Some(room_key.verifying_key());
    let author = SigningKey::from_bytes(&[0xB5u8; 32]);
    import_nodes(&parent, vec![make_map_set_node(&author, "world/mp", b"p")]).await;
    let checkpoint = nodalmerge_server::lineage::snapshot_parent_checkpoint(&parent)
        .await
        .unwrap();
    rooms
        .create_child_room(
            parent_id,
            child_id,
            checkpoint,
            "multi-peer".to_string(),
            "mgr".to_string(),
            "promotion-based".to_string(),
        )
        .await
        .unwrap();

    let child = rooms.get_or_create(child_id).await;
    import_nodes(&child, vec![make_map_set_node(&author, "world/mc", b"c")]).await;
    let child_hash = snapshot_room_canonical_hash(&child).await.unwrap();

    let (mut sink_a, mut stream_a) =
        connect_topology_peer(addr, parent_id, &peer_a, &room_key).await;
    let propose_msg = serde_json::json!({
        "type": "topology.propose-promotion",
        "parent_room_id": parent_id,
        "child_room_id": child_id,
        "child_checkpoint_hash": child_hash,
        "payload_ref": "artifact://multi-peer",
        "idempotency_key": "prop-007",
    });
    sink_a
        .send(TMessage::Text(propose_msg.to_string().into()))
        .await
        .expect("propose");
    let proposed_json =
        expect_topology_response(&mut stream_a, "topology.propose-promotion.completed").await;
    let proposed = match serde_json::from_value::<TopologyWsResponse>(proposed_json)
        .expect("propose envelope")
    {
        TopologyWsResponse::ProposePromotionCompleted(p) => p,
        other => panic!("expected propose completed, got {:?}", other.wire_type()),
    };
    let golden_propose =
        serialize_topology_ws_response(&propose_promotion_completed(proposed.clone()))
            .expect("golden propose");
    assert!(golden_propose.contains("\"type\":\"topology.propose-promotion.completed\""));

    let (mut sink_b, mut stream_b) =
        connect_topology_peer(addr, parent_id, &peer_b, &room_key).await;
    let validate_msg = serde_json::json!({
        "type": "topology.validate-promotion",
        "proposal_id": proposed.proposal_id,
    });
    sink_b
        .send(TMessage::Text(validate_msg.to_string().into()))
        .await
        .expect("validate");
    let validated_json =
        expect_topology_response(&mut stream_b, "topology.validate-promotion.completed").await;
    let validated = match serde_json::from_value::<TopologyWsResponse>(validated_json)
        .expect("validate envelope")
    {
        TopologyWsResponse::ValidatePromotionCompleted(v) => v,
        other => panic!("expected validate completed, got {:?}", other.wire_type()),
    };
    let golden_validate =
        serialize_topology_ws_response(&validate_promotion_completed(validated.clone()))
            .expect("golden validate");
    assert!(golden_validate.contains("\"validation_digest\""));

    let apply_msg = serde_json::json!({
        "type": "topology.apply-promotion",
        "proposal_id": proposed.proposal_id,
    });
    sink_b
        .send(TMessage::Text(apply_msg.to_string().into()))
        .await
        .expect("apply");
    let applied_json =
        expect_topology_response(&mut stream_b, "topology.apply-promotion.completed").await;
    let applied =
        match serde_json::from_value::<TopologyWsResponse>(applied_json).expect("apply envelope") {
            TopologyWsResponse::ApplyPromotionCompleted(a) => a,
            other => panic!("expected apply completed, got {:?}", other.wire_type()),
        };
    let golden_apply = serialize_topology_ws_response(&apply_promotion_completed(applied.clone()))
        .expect("golden apply");
    assert!(golden_apply.contains("\"audit_key\":\"_topology/promotion/prop-007\""));
    assert!(applied.audit_key.contains("prop-007"));
}
