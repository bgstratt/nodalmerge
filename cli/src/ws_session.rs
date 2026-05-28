use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use nodalmerge_core::RoomToken;
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message as WsMessage;

#[derive(Debug, Clone)]
pub struct WsSessionConfig {
    pub ws_url: String,
    pub room_id: String,
    pub peer_seed: [u8; 32],
    pub token: Option<RoomToken>,
    pub timeout: Duration,
}

pub async fn send_topology_message(
    cfg: &WsSessionConfig,
    message: Value,
) -> Result<Value, String> {
    send_ws_command(cfg, message, topology_success_types()).await
}

/// Send a control-plane message after hello; succeed on first matching `type`.
pub async fn send_ws_command(
    cfg: &WsSessionConfig,
    message: Value,
    success_types: &[&str],
) -> Result<Value, String> {
    let peer_sk = ed25519_dalek::SigningKey::from_bytes(&cfg.peer_seed);
    let peer_hex = hex_lower(&peer_sk.verifying_key().to_bytes());

    let (ws, _) = tokio_tungstenite::connect_async(&cfg.ws_url)
        .await
        .map_err(|e| format!("websocket connect failed: {e}"))?;
    let (mut sink, mut stream) = ws.split();

    let mut hello = serde_json::json!({
        "type": "hello",
        "pubkey": peer_hex,
        "frontier": [],
        "subscribe": ["**"]
    });
    if let Some(tok) = &cfg.token {
        hello["token"] = token_json(tok);
    }

    sink.send(WsMessage::Text(hello.to_string().into()))
        .await
        .map_err(|e| format!("send hello: {e}"))?;

    let deadline = tokio::time::Instant::now() + cfg.timeout;
    drain_until_welcome(&mut stream, deadline).await?;

    sink.send(WsMessage::Text(message.to_string().into()))
        .await
        .map_err(|e| format!("send command: {e}"))?;

    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if let Ok(Some(Ok(WsMessage::Text(text)))) =
            tokio::time::timeout(remaining.min(Duration::from_millis(500)), stream.next()).await
        {
            if text.contains("reject.") {
                return Err(text);
            }
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                if message_type_matches(&v, success_types) {
                    return Ok(v);
                }
            }
        }
    }

    Err(format!(
        "timed out waiting for response (expected one of: {})",
        success_types.join(", ")
    ))
}

fn message_type_matches(v: &Value, success_types: &[&str]) -> bool {
    v.get("type")
        .and_then(|t| t.as_str())
        .is_some_and(|t| success_types.contains(&t))
}

fn topology_success_types() -> &'static [&'static str] {
    &[
        "topology.create-child.completed",
        "topology.list-children.result",
        "topology.describe-lineage.result",
        "topology.propose-promotion.completed",
        "topology.validate-promotion.completed",
        "topology.apply-promotion.completed",
    ]
}

async fn drain_until_welcome(
    stream: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    deadline: tokio::time::Instant,
) -> Result<(), String> {
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if let Ok(Some(Ok(WsMessage::Text(text)))) =
            tokio::time::timeout(remaining.min(Duration::from_millis(500)), stream.next()).await
        {
            if text.contains("reject.") {
                return Err(text);
            }
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                if v.get("type").and_then(|t| t.as_str()) == Some("welcome") {
                    return Ok(());
                }
            }
        }
    }
    Err("timed out before welcome".into())
}

fn token_json(tok: &RoomToken) -> Value {
    serde_json::json!({
        "peer_pubkey": hex_lower(&tok.peer_pubkey),
        "expiry": tok.expiry_secs,
        "caps": tok.capabilities,
        "sig": hex_lower(&tok.signature),
    })
}

pub fn hex_lower(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(H[(b >> 4) as usize] as char);
        s.push(H[(b & 0x0f) as usize] as char);
    }
    s
}

pub fn build_ws_url(host: &str, room_id: &str) -> String {
    let host = host.trim_end_matches('/');
    if host.contains("/ws/") {
        host.to_string()
    } else {
        format!("{host}/ws/{room_id}")
    }
}
