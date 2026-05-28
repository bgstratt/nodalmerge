use std::time::{Duration, Instant};

use ed25519_dalek::SigningKey;
use futures_util::{SinkExt, StreamExt};
use nodalmerge_core::{canonical_hash, replay, Hash};
use nodalmerge_runtime_local::{CheckpointMeta, PersistenceHandle, PeerLocalPersistence};
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::config::{SessionTimings, SyncSessionStats, WorkerConfig, WorkerReport};
use crate::sync::{
    apply_pack_to_persist, build_hello_extras, decode_pack_nodes, local_node_ids,
    parse_welcome_caps, run_mst_catchup,
};

pub async fn run_worker_session(cfg: WorkerConfig) -> Result<WorkerReport, String> {
    let session_start = Instant::now();
    let mut timings = SessionTimings::default();

    let persist = PersistenceHandle::open(cfg.backend.clone())
        .map_err(|e| format!("open peer-local persistence: {e}"))?;

    let room = &cfg.room_id;
    let hydrate_start = Instant::now();
    let hydrate = persist.hydrate(room).map_err(|e| e.to_string())?;
    timings.hydrate_ms = hydrate_start.elapsed().as_millis() as u64;

    tracing::info!(
        room = %room,
        hydrated_nodes = hydrate.nodes.len(),
        tail_seq = hydrate.tail.seq,
        durable = persist.is_durable(),
        hydrate_ms = timings.hydrate_ms,
        "peer-local hydrate complete"
    );

    let peer_sk = SigningKey::from_bytes(&[0x71u8; 32]);
    let peer_hex = hex_lower(&peer_sk.verifying_key().to_bytes());
    let local_ids = local_node_ids(&hydrate.nodes);

    let frontier: Vec<String> = hydrate
        .nodes
        .iter()
        .map(|n| hex_lower(n.id.as_bytes()))
        .collect();

    let (hello_extra, used_ibf) =
        build_hello_extras(&hydrate.nodes, cfg.negotiate_ibf, cfg.negotiate_mst);

    let sync_start = Instant::now();
    let (ws, _) = tokio_tungstenite::connect_async(&cfg.server_ws_url)
        .await
        .map_err(|e| format!("websocket connect failed: {e}"))?;
    let (mut sink, mut stream) = ws.split();

    let mut hello = serde_json::json!({
        "type": "hello",
        "pubkey": peer_hex,
        "frontier": frontier,
        "subscribe": ["**"]
    });
    if let Some(obj) = hello_extra.as_object() {
        for (k, v) in obj {
            hello[k] = v.clone();
        }
    }

    sink.send(WsMessage::Text(hello.to_string().into()))
        .await
        .map_err(|e| format!("send hello: {e}"))?;

    let deadline = Instant::now() + cfg.run_for;
    let mut saw_welcome = false;
    let mut stats = SyncSessionStats {
        used_ibf,
        ..Default::default()
    };

    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let next = tokio::time::timeout(remaining.min(Duration::from_millis(500)), stream.next())
            .await;

        match next {
            Ok(Some(Ok(WsMessage::Text(text)))) => {
                if text.contains("reject.") {
                    timings.websocket_sync_ms = sync_start.elapsed().as_millis() as u64;
                    timings.total_ms = session_start.elapsed().as_millis() as u64;
                    return Err(text);
                }
                let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                let Some(ty) = v.get("type").and_then(|t| t.as_str()) else {
                    continue;
                };
                match ty {
                    "welcome" => {
                        saw_welcome = true;
                        let negotiated = parse_welcome_caps(&v);
                        tracing::debug!(room = %room, ?negotiated, "received welcome");

                        if cfg.negotiate_mst && negotiated.supports_mst {
                            let mst_nodes = run_mst_catchup(
                                &v,
                                &negotiated,
                                &local_ids,
                                &mut sink,
                                &mut stream,
                                deadline,
                                &mut stats,
                            )
                            .await?;
                            if !mst_nodes.is_empty() {
                                apply_pack_to_persist(&persist, room, &mst_nodes)?;
                                stats.packs_applied += 1;
                            }
                        }
                    }
                    "pack" => {
                        if let Some(nodes) = decode_pack_nodes(&v) {
                            apply_pack_to_persist(&persist, room, &nodes)?;
                            stats.packs_applied += 1;
                            tracing::debug!(
                                room = %room,
                                node_count = nodes.len(),
                                "applied pack to peer-local log"
                            );
                        }
                    }
                    _ => {}
                }
            }
            Ok(Some(Ok(WsMessage::Close(_)))) => break,
            Ok(Some(Ok(_))) | Ok(None) => {}
            Ok(Some(Err(e))) => {
                timings.websocket_sync_ms = sync_start.elapsed().as_millis() as u64;
                timings.total_ms = session_start.elapsed().as_millis() as u64;
                return Err(format!("websocket error: {e}"));
            }
            Err(_) => break,
        }
    }
    timings.websocket_sync_ms = sync_start.elapsed().as_millis() as u64;

    let flush_start = Instant::now();
    let recovery = persist.recover(room).map_err(|e| e.to_string())?;
    persist.flush(room).map_err(|e| e.to_string())?;
    timings.flush_ms = flush_start.elapsed().as_millis() as u64;

    let canonical_hash = if recovery.nodes.is_empty() {
        Hash::of(b"")
    } else {
        let state = replay(&recovery.nodes, None).map_err(|e| format!("replay: {e}"))?;
        canonical_hash(&state.map)
    };

    let checkpoint_start = Instant::now();
    persist
        .checkpoint(
            room,
            CheckpointMeta {
                seq: recovery.tail.seq,
                canonical_hash: Some(canonical_hash),
            },
        )
        .map_err(|e| e.to_string())?;
    timings.checkpoint_ms = checkpoint_start.elapsed().as_millis() as u64;
    timings.total_ms = session_start.elapsed().as_millis() as u64;

    tracing::info!(
        room = %room,
        packs_applied = stats.packs_applied,
        mst_requests = stats.mst_requests,
        total_ms = timings.total_ms,
        flush_ms = timings.flush_ms,
        "headless session complete"
    );

    Ok(WorkerReport {
        saw_welcome,
        packs_applied: stats.packs_applied,
        mst_requests: stats.mst_requests,
        mst_nodes_fetched: stats.mst_nodes_fetched,
        used_ibf: stats.used_ibf,
        nodes_persisted_total: recovery.nodes.len(),
        canonical_hash_hex: hex_lower(canonical_hash.as_bytes()),
        timings_ms: timings,
    })
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
