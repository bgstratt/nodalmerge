//! Axum WebSocket handler — one task per connected peer.
//!
//! # Wire Protocol (JSON messages)
//!
//! Client → Server:
//!   hello     { type, room, root, known:[hex], pubkey }
//!   pack      { type, nodes:[SyncNode] }
//!   request   { type, known:[hex] }
//!   blob-upload  { type, blobs:[{hash,data_b64}] }
//!   blob-request { type, hashes:[hex] }
//!   request-upload { type, hash, size }              ← F6
//!   blob-uploaded  { type, hash }                    ← F6
//!   presence  { type, data:{...} }   ← ephemeral, not stored
//!
//! Server → Client:
//!   welcome   { type, root, missing:[hex] }
//!   pack      { type, from, nodes:[SyncNode] }
//!   blob-pack { type, blobs:[{hash,data_b64}] }
//!   blob-redirect { type, redirects:[{hash,url,expires_at_unix}] }  ← F6
//!   upload-granted  { type, hash, url, expires_at_unix }            ← F6
//!   upload-denied   { type, hash, reason }                          ← F6
//!   upload-rejected { type, hash, reason }                          ← F6
//!   presence  { type, from, data:{...} }
//!   error     { type, msg }

use std::sync::Arc;
use axum::{
    extract::{Path, State, WebSocketUpgrade, ws::{CloseFrame, Message, WebSocket}},
    response::Response,
};
use futures_util::{SinkExt, StreamExt, stream::SplitSink};
use serde_json::Value;
use activesync_core::{BlobStore, Ibf, MerkleSearchTree, Policy, PolicyDefault, PolicyRule,
                      RoomToken, SyncCapabilities, SyncNode, pack_nodes, unpack_nodes,
                      compact, rebuild_from_snapshot, pack_snapshot_pack, verify_snapshot};
use ed25519_dalek::{SigningKey, VerifyingKey};
use governor::{Quota, RateLimiter, clock::DefaultClock, state::{InMemoryState, NotKeyed}};
use std::num::NonZeroU32;

use crate::room::{Rooms, Room, import_nodes};

/// G3: per-peer direct rate limiter. `None` = limit disabled for this
/// session (either the CLI default is `0`, or the peer is the server
/// itself — the authoritative tick loop and internal writes are exempt).
type PeerLimiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock>;

// -----------------------------------------------------------------------------
// F3b — server-side subscription filter.
// -----------------------------------------------------------------------------
// Each peer may declare `subscribe: ["world/**", …]` in its hello (or send a
// subsequent `subscribe` message). The server filters every relayed `pack` to
// drop nodes whose ops fall entirely outside the peer's subscription. Default
// subscription is `["**"]` (see `Subscription::everything`) which short-circuits
// filtering. Writes are NOT filtered — authority over paths is Policy's job
// (A5); subscriptions are a bandwidth/visibility tool only.

#[derive(Clone)]
struct Subscription {
    /// Compiled path matchers. An empty vec means "match everything" (fast path).
    matchers: Vec<GlobMatcher>,
    /// Original patterns (kept for diagnostics / token-scope derivation).
    #[allow(dead_code)]
    patterns: Vec<String>,
}

impl Subscription {
    fn everything() -> Self {
        Self { matchers: vec![], patterns: vec!["**".to_string()] }
    }

    fn from_patterns(patterns: Vec<String>) -> Self {
        // Empty list or explicit `**` → fast path.
        if patterns.is_empty() || patterns.iter().any(|p| p == "**") {
            return Self::everything();
        }
        let matchers = patterns.iter().map(|p| GlobMatcher::compile(p)).collect();
        Self { matchers, patterns }
    }

    /// Parse from a JSON value: accepts a JSON array of strings, else defaults.
    fn from_hello_value(v: &Value) -> Self {
        match v.as_array() {
            Some(arr) => {
                let patterns: Vec<String> = arr.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect();
                Self::from_patterns(patterns)
            }
            None => Self::everything(),
        }
    }

    /// Returns true when every node passes (i.e. `**`).
    #[inline]
    fn matches_everything(&self) -> bool { self.matchers.is_empty() }

    /// Does any compiled pattern match this path?
    fn matches_path(&self, path: &str) -> bool {
        if self.matches_everything() { return true; }
        self.matchers.iter().any(|m| m.is_match(path))
    }

    /// Should the node be relayed to the subscriber?
    /// Policy:
    ///   - nodes with no ops → always relayed (structural/empty)
    ///   - nodes with a sentinel-key op (key starts with `\x00`) → always
    ///     relayed (E2EE envelope, snapshot meta). Filtering these would
    ///     silently corrupt encrypted rooms.
    ///   - otherwise: at least one op's key must match the subscription.
    fn accepts_node(&self, node: &SyncNode) -> bool {
        if self.matches_everything() { return true; }
        let ops = &node.transaction.ops;
        if ops.is_empty() { return true; }
        for op in ops {
            let k = match op.key() { Some(k) => k, None => continue };
            if k.as_bytes().first().copied() == Some(0u8) { return true; }
            if self.matches_path(k) { return true; }
        }
        false
    }
}

/// Compiled glob matcher. Semantics match the SDK's `compileGlob` in `web/sdk.js`:
///   - `**` on its own = any path
///   - `/**` trailing = optional subtree (so `world/**` matches `world`)
///   - `/**/` in the middle = optional subtree (so `a/**/c` matches `a/c` and `a/b/c`)
///   - `**` elsewhere = any chars including `/`
///   - `*` = any chars excluding `/`
///   - everything else literal
#[derive(Clone)]
struct GlobMatcher {
    pattern: Vec<u8>,
}

impl GlobMatcher {
    fn compile(pattern: &str) -> Self {
        Self { pattern: pattern.as_bytes().to_vec() }
    }

    fn is_match(&self, s: &str) -> bool {
        if self.pattern == b"**" { return true; }
        glob_match(&self.pattern, s.as_bytes())
    }
}

fn glob_match(mut pattern: &[u8], mut path: &[u8]) -> bool {
    loop {
        // `/**` at the very end — optional subtree.
        if pattern == b"/**" {
            return path.is_empty() || path.starts_with(b"/");
        }
        // `/**/` in the middle — optional subtree; require a leading `/` in path,
        // then match `rest` at every subsequent segment boundary.
        if pattern.starts_with(b"/**/") {
            let rest = &pattern[4..];
            if !path.starts_with(b"/") { return false; }
            let mut idx = 1usize;
            loop {
                if glob_match(rest, &path[idx..]) { return true; }
                while idx < path.len() && path[idx] != b'/' { idx += 1; }
                if idx >= path.len() { return false; }
                idx += 1;
            }
        }
        // `**/` at the very start / after a `/` — optional prefix subtree.
        // Lets `**/msg` match bare `msg` as well as `a/msg`, `a/b/msg`.
        if pattern.starts_with(b"**/") {
            let rest = &pattern[3..];
            // Zero-segment case: the `**/` consumes nothing.
            if glob_match(rest, path) { return true; }
            // One-or-more-segments case: fall through to the generic `**` handler.
        }
        // `**` — match any substring (including empty, including slashes).
        if pattern.starts_with(b"**") {
            let rest = &pattern[2..];
            if rest.is_empty() { return true; }
            for i in 0..=path.len() {
                if glob_match(rest, &path[i..]) { return true; }
            }
            return false;
        }
        // `*` — match any substring without crossing `/`.
        if pattern.first() == Some(&b'*') {
            let rest = &pattern[1..];
            for i in 0..=path.len() {
                if glob_match(rest, &path[i..]) { return true; }
                if i < path.len() && path[i] == b'/' { return false; }
            }
            return false;
        }
        // Literal char.
        if pattern.is_empty() { return path.is_empty(); }
        if path.is_empty() { return false; }
        if pattern[0] != path[0] { return false; }
        pattern = &pattern[1..];
        path = &path[1..];
    }
}

/// Apply the peer's subscription to a broadcast envelope. Returns the original
/// envelope string when no filtering is needed, an owned string when nodes
/// were filtered, or `None` when every node was filtered out (caller should
/// skip the send entirely).
fn filter_pack_for_subscriber(env: &str, sub: &Subscription) -> Option<String> {
    if sub.matches_everything() { return Some(env.to_string()); }
    // Parse just enough to decide.
    let v: Value = match serde_json::from_str(env) { Ok(v) => v, Err(_) => return Some(env.to_string()) };
    if v["type"] != "pack" { return Some(env.to_string()); }
    let nodes_b64 = match v["nodes"].as_str() { Some(s) => s, None => return Some(env.to_string()) };
    let bytes = match base64_decode(nodes_b64) { Ok(b) => b, Err(_) => return Some(env.to_string()) };
    let nodes: Vec<SyncNode> = match unpack_nodes(&bytes) { Ok(n) => n, Err(_) => return Some(env.to_string()) };
    let kept: Vec<SyncNode> = nodes.into_iter().filter(|n| sub.accepts_node(n)).collect();
    if kept.is_empty() { return None; }
    let kept_refs: Vec<&SyncNode> = kept.iter().collect();
    let mut out = v.clone();
    out["nodes"] = Value::String(base64_encode(&pack_nodes(&kept_refs)));
    Some(out.to_string())
}

pub async fn handler(
    ws: WebSocketUpgrade,
    Path(room_id): Path<String>,
    State(rooms): State<Rooms>,
) -> Response {
    tracing::debug!(%room_id, "ws upgrade request");
    let server_key = Arc::clone(&rooms.server_key);
    ws.on_upgrade(move |socket| {
        tracing::trace!(%room_id, "on_upgrade callback firing");
        handle_socket(socket, room_id, rooms, server_key)
    })
}

async fn handle_socket(socket: WebSocket, room_id: String, rooms: Rooms, server_key: Arc<SigningKey>) {
    let room = rooms.get_or_create(&room_id).await;
    let server_pubkey_hex = crate::keypair::pubkey_hex(&server_key);
    let (mut sink, mut stream) = socket.split();
    let mut bcast_rx = room.tx.subscribe();

    tracing::debug!(%room_id, "socket opened, waiting for hello");

    // -------------------------------------------------------------------------
    // Handshake: wait for hello (5-second timeout so stale sockets don't leak)
    // -------------------------------------------------------------------------
    let hello_result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        stream.next(),
    ).await;

    let hello = match hello_result {
        Err(_) => {
            tracing::warn!(%room_id, "hello timeout (5s) — closing");
            return;
        }
        Ok(Some(Ok(Message::Text(t)))) => {
            match serde_json::from_str::<Value>(&t) {
                Ok(v) if v["type"] == "hello" => v,
                Ok(v) => {
                    tracing::warn!(ty = ?v["type"], "expected hello, got other type — closing");
                    send_error(&mut sink, "expected hello").await;
                    return;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "hello parse error — closing");
                    send_error(&mut sink, "expected hello").await;
                    return;
                }
            }
        }
        Ok(Some(Ok(m))) => {
            tracing::warn!("expected text hello, got binary/other — closing");
            let _ = m;
            return;
        }
        Ok(Some(Err(e))) => {
            tracing::warn!(error = %e, "socket error waiting for hello");
            return;
        }
        Ok(None) => {
            tracing::debug!(%room_id, "socket closed before hello");
            return;
        }
    };

    let pubkey_hex = hello["pubkey"].as_str().unwrap_or("").to_string();
    let short = &pubkey_hex[..8.min(pubkey_hex.len())];
    tracing::info!(room = %room_id, peer = %short, "peer connected");

    // G3: per-peer rate limiters. The server's own pubkey is exempt so
    // authoritative tick writes are never throttled. Zero disables the
    // corresponding limiter outright.
    let is_server_peer = pubkey_hex == server_pubkey_hex;
    let node_limiter: Option<PeerLimiter> = match (is_server_peer, NonZeroU32::new(rooms.peer_rate_nodes)) {
        (false, Some(nz)) => Some(RateLimiter::direct(Quota::per_second(nz))),
        _ => None,
    };
    let byte_limiter: Option<PeerLimiter> = match (is_server_peer, NonZeroU32::new(rooms.peer_rate_bytes)) {
        (false, Some(nz)) => Some(RateLimiter::direct(Quota::per_second(nz))),
        _ => None,
    };

    // F3b: parse this peer's subscription from hello. Defaults to everything.
    // Updated at runtime via the `subscribe` client message.
    let subscription = Arc::new(std::sync::RwLock::new(
        Subscription::from_hello_value(&hello["subscribe"]),
    ));

    // C3: if the room is locked, verify the capability token before proceeding.
    // G6: record the token's expiry so the main loop can disconnect the
    // session the instant it lapses (short-lived tokens + SDK refresh hook).
    let token_deadline: Option<tokio::time::Instant> = {
        let auth_key = room.auth_key.read().await;
        if let Some(ref room_vk) = *auth_key {
            match verify_hello_token(&hello, room_vk, &pubkey_hex, &room_id) {
                Ok(expiry_secs) => {
                    let now_secs = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    // `verify` already rejected `now >= expiry`, so this
                    // subtraction is safe; guard anyway for belt-and-braces.
                    let remaining = expiry_secs.saturating_sub(now_secs);
                    Some(tokio::time::Instant::now() + std::time::Duration::from_secs(remaining))
                }
                Err(e) => {
                    send_error(&mut sink, &format!("auth: {e}")).await;
                    return;
                }
            }
        } else {
            None
        }
    };

    let client_known: Vec<activesync_core::NodeId> = hello["frontier"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| parse_hex_hash(v.as_str()?)).collect())
        .unwrap_or_default();

    // Capability negotiation (A7): parse client caps (missing field → defaults).
    let client_caps: SyncCapabilities = hello.get("caps")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let server_caps = SyncCapabilities::default();
    let negotiated_caps = server_caps.intersect(&client_caps);

    // IBF set-reconciliation (B1): when both peers support IBF and the client
    // included an `ibf` field, decode the symmetric diff directly. Otherwise
    // fall back to the frontier-based over-send approach.
    let (only_in_client, only_in_server): (Vec<_>, Vec<_>) = if negotiated_caps.supports_ibf {
        if let Some(ibf_b64) = hello["ibf"].as_str() {
            match base64_decode(ibf_b64).ok().and_then(|b| Ibf::decode_bytes(&b).ok()) {
                Some(client_ibf) => {
                    let graph = room.graph.read().await;
                    let server_ids = graph.all_node_ids();
                    let mut diff = Ibf::from_ids(&server_ids);
                    diff.subtract(&client_ibf);
                    match diff.decode() {
                        Some((in_server, in_client)) => (in_client, in_server),
                        None => {
                            // IBF too small to decode — fall back gracefully.
                            let all: std::collections::HashSet<_> = server_ids.into_iter().collect();
                            (vec![], all.into_iter().collect())
                        }
                    }
                }
                None => {
                    let graph = room.graph.read().await;
                    (vec![], graph.all_node_ids())
                }
            }
        } else {
            // Client claims IBF support but didn't send one — fall back.
            let graph = room.graph.read().await;
            let client_known_set: std::collections::HashSet<_> = client_known.iter().copied().collect();
            let all: std::collections::HashSet<_> = graph.all_node_ids().into_iter().collect();
            let only_in_server: Vec<_> = all.difference(&client_known_set).copied().collect();
            (vec![], only_in_server)
        }
    } else {
        // Legacy frontier-based diff.
        let graph = room.graph.read().await;
        let client_known_set: std::collections::HashSet<_> = client_known.iter().copied().collect();
        let missing_from_us = graph.missing_hashes(&client_known_set);
        let all: std::collections::HashSet<_> = graph.all_node_ids().into_iter().collect();
        let only_in_server: Vec<_> = all.difference(&client_known_set).copied().collect();
        (missing_from_us, only_in_server)
    };
    tracing::debug!(peer = %short, only_in_client = only_in_client.len(), only_in_server = only_in_server.len(), "diff computed");

    // Send welcome: tell client which of ITS nodes WE are missing, and give it
    // any nodes it is missing.  In MST mode we include the server's MST root
    // so the client can drive iterative descent; we still push a catch-up pack
    // for the nodes we know the client is missing via IBF.
    let (welcome_json, catchup_b64, has_catchup) = {
        let graph = room.graph.read().await;
        let nodes = graph.get_nodes(&only_in_server);
        let pack_b64 = base64_encode(&pack_nodes(&nodes));
        let has_catchup = !nodes.is_empty();
        let server_frontier = graph.frontier();
        let missing_hex: Vec<String> = only_in_client.iter().map(|h| h.to_hex()).collect();

        // Build MST root when MST is negotiated (B2).
        let mst_root_hex = if negotiated_caps.supports_mst {
            let all_ids = graph.all_node_ids();
            MerkleSearchTree::from_ids(&all_ids).root_hash_hex()
        } else {
            String::new()
        };

        // D2: collect currently-connected peers for the welcome message so the
        // new peer knows who to initiate WebRTC connections with immediately.
        let current_peers: Vec<String> = room.connected_peers.read().await
            .iter()
            .filter(|p| p.as_str() != pubkey_hex)
            .cloned()
            .collect();

        let mut w = serde_json::json!({
            "type":        "welcome",
            "root":        graph.merkle_root().to_hex(),
            "frontier":    server_frontier.to_hex_vec(),
            "missing":     missing_hex,
            "caps":        negotiated_caps,
            "server_pubkey": server_pubkey_hex,
            "peers":       current_peers,
        });
        if !mst_root_hex.is_empty() {
            w["mst_root"] = serde_json::json!(mst_root_hex);
        }
        (w.to_string(), pack_b64, has_catchup)
    };

    // D2: register this peer and broadcast peer-joined to the room.
    room.register_peer(pubkey_hex.clone()).await;

    // Small per-connection stabilization delay to avoid many peers
    // immediately provoking DB selection storms on first Mongo call.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let _ = room.tx.send(serde_json::json!({
        "type":   "peer-joined",
        "from":   pubkey_hex,
        "pubkey": pubkey_hex,
    }).to_string());

    if !ws_send(&mut sink, &room_id, welcome_json).await {
        tracing::warn!(peer = %short, "welcome send failed");
        room.deregister_peer(&pubkey_hex).await;
        return;
    }
    tracing::debug!(peer = %short, "welcome sent, entering main loop");

    // Send catch-up pack if non-empty. F3b: filter through the peer's
    // subscription before sending.
    if has_catchup {
        let env = serde_json::json!({"type":"pack","from":"server","nodes":catchup_b64}).to_string();
        let sub = subscription.read().unwrap().clone();
        if let Some(filtered) = filter_pack_for_subscriber(&env, &sub) {
            if !ws_send(&mut sink, &room_id, filtered).await {
                room.deregister_peer(&pubkey_hex).await;
                return;
            }
        }
    }

    // -------------------------------------------------------------------------
    // Main loop — multiplex incoming client messages and broadcast pushes.
    // All early exits use `break` so cleanup (peer-left) always runs.
    // -------------------------------------------------------------------------
    loop {
        tokio::select! {
            // --- G6: capability token expired mid-session ---
            // `sleep_until` is driven by an absolute `Instant`, so each
            // iteration reconstructs the future idempotently. The `if` guard
            // suppresses the arm entirely when the room is unlocked (no
            // token = no deadline). On fire we bump the metric, close with
            // `4002 token expired`, and break so cleanup runs. The SDK's
            // `getToken` refresh hook re-fetches a fresh token and
            // reconnects via the normal hello handshake.
            _ = async {
                match token_deadline {
                    Some(d) => tokio::time::sleep_until(d).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                metrics::counter!(
                    "activesync_token_expired_disconnects_total",
                    "room" => room_id.clone(),
                ).increment(1);
                tracing::info!(peer = %short, room = %room_id, "token expired — closing with 4002");
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(1),
                    sink.send(Message::Close(Some(CloseFrame {
                        code: 4002,
                        reason: std::borrow::Cow::Borrowed("token expired"),
                    }))),
                ).await;
                break;
            }
            // --- inbound from this client ---
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if !handle_client_message(&text, &room, &pubkey_hex, &room_id, &server_key, &mut sink, &subscription, node_limiter.as_ref(), byte_limiter.as_ref(), &negotiated_caps).await {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        tracing::debug!(peer = %short, "got Close frame — breaking");
                        break;
                    }
                    None => {
                        tracing::debug!(peer = %short, "stream ended — breaking");
                        break;
                    }
                    Some(Ok(Message::Ping(p))) => {
                        let _ = sink.send(Message::Pong(p)).await;
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Binary(_))) => {}
                    Some(Err(e)) => {
                        tracing::warn!(peer = %short, error = %e, "stream error — closing");
                        break;
                    }
                }
            }
            // --- push from other peers in the room ---
            push = bcast_rx.recv() => {
                match push {
                    Ok(env) => {
                        // Skip messages we sent ourselves (self-echo suppression)
                        if let Ok(v) = serde_json::from_str::<Value>(&env) {
                            if v["from"].as_str() == Some(pubkey_hex.as_str()) {
                                continue;
                            }
                        }
                        // F3b: subscription filter. Pack envelopes may be rewritten
                        // to drop nodes outside this peer's subscription, or
                        // dropped entirely when nothing matches. All other
                        // message types pass through unchanged.
                        let sub = subscription.read().unwrap().clone();
                        let out = match filter_pack_for_subscriber(&env, &sub) {
                            Some(s) => s,
                            None => continue,
                        };
                        if !ws_send(&mut sink, &room_id, out).await { break; }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        // G1: slow consumer fell behind the ring buffer.
                        // Previously silently ignored, which left the peer
                        // desynced without any signal. Now: increment the
                        // metric, send a 4001 close frame, and break so the
                        // SDK's exp-backoff reconnect path rebuilds via the
                        // normal hello → IBF diff → catch-up pack handshake.
                        metrics::counter!(
                            "activesync_broadcast_lagged_total",
                            "room" => room_id.clone(),
                        ).increment(1);
                        tracing::warn!(
                            peer = %short,
                            room = %room_id,
                            lagged = n,
                            "broadcast lagged — closing with 4001 resync required"
                        );
                        let _ = tokio::time::timeout(
                            std::time::Duration::from_secs(1),
                            sink.send(Message::Close(Some(CloseFrame {
                                code: 4001,
                                reason: std::borrow::Cow::Borrowed("resync required"),
                            }))),
                        ).await;
                        break;
                    }
                    Err(_) => break,
                }
            }
        }
    }

    tracing::debug!(peer = %short, "cleanup: deregistering peer");
    // D2: deregister peer and notify remaining peers so they can close their
    // WebRTC connections.
    room.deregister_peer(&pubkey_hex).await;
    let _ = room.tx.send(serde_json::json!({
        "type": "peer-left",
        "from": pubkey_hex,
    }).to_string());
    tracing::debug!(peer = %short, "handler fully exited");
}

/// Returns `false` if the connection should be closed.
async fn handle_client_message(
    text: &str,
    room: &Arc<Room>,
    pubkey_hex: &str,
    room_id: &str,
    server_key: &Arc<SigningKey>,
    sink: &mut SplitSink<WebSocket, Message>,
    subscription: &Arc<std::sync::RwLock<Subscription>>,
    node_limiter: Option<&PeerLimiter>,
    byte_limiter: Option<&PeerLimiter>,
    negotiated_caps: &SyncCapabilities,
) -> bool {
    let msg: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => { send_error(sink, "invalid JSON").await; return true; }
    };

    match msg["type"].as_str().unwrap_or("") {

        // F3b: update this peer's subscription at runtime -------------------
        "subscribe" => {
            let sub = Subscription::from_hello_value(&msg["patterns"]);
            *subscription.write().unwrap() = sub;
            let reply = serde_json::json!({"type":"subscribe-ack"}).to_string();
            if !ws_send(sink, room_id, reply).await { return false; }
        }

        // Client pushes a pack of new nodes --------------------------------
        "pack" => {
            let nodes_b64 = msg["nodes"].as_str().unwrap_or("");
            let decoded = match base64_decode(nodes_b64) {
                Ok(b) => b,
                Err(_) => { send_error(sink, "invalid pack: bad base64").await; return true; }
            };
            let nodes: Vec<SyncNode> = match unpack_nodes(&decoded) {
                Ok(n) => n,
                Err(_) => { send_error(sink, "invalid pack: bad postcard").await; return true; }
            };
            let incoming = nodes.len();
            let incoming_bytes = decoded.len();

            // G3: rate-limit BEFORE kicking off Ed25519 verification so a
            // flood can't burn CPU even once. Either limiter tripping ends
            // the session with a 4008 close; the SDK treats 4008 as fatal
            // and surfaces it to the app (unlike 4001 resync-required).
            if let Some(lim) = node_limiter {
                if let Some(n) = NonZeroU32::new(incoming as u32) {
                    if check_peer_rate(lim, n, pubkey_hex, room_id, sink, "nodes").await.is_err() {
                        return false;
                    }
                }
            }
            if let Some(lim) = byte_limiter {
                if let Some(n) = NonZeroU32::new(incoming_bytes.min(u32::MAX as usize) as u32) {
                    if check_peer_rate(lim, n, pubkey_hex, room_id, sink, "bytes").await.is_err() {
                        return false;
                    }
                }
            }

            let (accepted, errs) = import_nodes(room, nodes).await;
            tracing::info!(peer = %&pubkey_hex[..8.min(pubkey_hex.len())], incoming, accepted, errs = errs.len(), "pack received");
            if !errs.is_empty() {
                tracing::warn!(errors = %errs.join("; "), "pack errors");
                let err = serde_json::json!({"type":"error","msg":errs.join("; ")}).to_string();
                let _ = sink.send(Message::Text(err.into())).await;
            }
            if accepted > 0 {
                // Build merged pack of new leaf nodes and broadcast to room
                let bcast = {
                    let graph = room.graph.read().await;
                    let leaf_ids: Vec<_> = graph.leaf_ids().iter().copied().collect();
                    let nodes = graph.get_nodes(&leaf_ids);
                    let nodes_b64 = base64_encode(&pack_nodes(&nodes));
                    serde_json::json!({
                        "type":  "pack",
                        "from":  pubkey_hex,
                        "nodes": nodes_b64,
                        "root":  graph.merkle_root().to_hex(),
                    }).to_string()
                };
                let _ = room.tx.send(bcast);
            }
        }

        // Client requests MST nodes at specific paths (B2) -------------------
        "mst-request" => {
            let paths: Vec<String> = msg["paths"]
                .as_array().unwrap_or(&vec![])
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            let graph = room.graph.read().await;
            let all_ids = graph.all_node_ids();
            let mst = MerkleSearchTree::from_ids(&all_ids);
            drop(graph);
            let nodes: Vec<_> = paths.iter()
                .filter_map(|p| mst.get_node_wire(p))
                .collect();
            let reply = serde_json::json!({
                "type":  "mst-response",
                "nodes": nodes,
            }).to_string();
            if !ws_send(sink, room_id, reply).await { return false; }
        }

        // Client completed MST descent, requests specific missing nodes -------
        "mst-done" => {
            let ids: Vec<activesync_core::NodeId> = msg["ids"]
                .as_array().unwrap_or(&vec![])
                .iter()
                .filter_map(|v| parse_hex_hash(v.as_str()?))
                .collect();
            if !ids.is_empty() {
                let graph = room.graph.read().await;
                let nodes = graph.get_nodes(&ids);
                let nodes_b64 = base64_encode(&pack_nodes(&nodes));
                let reply = serde_json::json!({
                    "type":  "pack",
                    "from":  "server",
                    "nodes": nodes_b64,
                    "root":  graph.merkle_root().to_hex(),
                }).to_string();
                drop(graph);
                if !ws_send(sink, room_id, reply).await { return false; }
            }
        }

        // Client requests nodes it doesn't have ----------------------------
        "request" => {
            let known_set: std::collections::HashSet<_> =
                parse_hex_list(&msg["known"]).into_iter().collect();
            let graph = room.graph.read().await;
            let missing = graph.missing_hashes(&known_set);
            let nodes = graph.get_nodes(&missing);
            let nodes_b64 = base64_encode(&pack_nodes(&nodes));
            let reply = serde_json::json!({
                "type":  "pack",
                "from":  "server",
                "nodes": nodes_b64,
                "root":  graph.merkle_root().to_hex(),
            }).to_string();
            if !ws_send(sink, room_id, reply).await { return false; }
        }

        // Client uploads blobs ---------------------------------------------
        "blob-upload" => {
            let blobs = match msg["blobs"].as_array() {
                Some(a) => a.clone(),
                None => return true,
            };
            let mut stored = 0usize;
            let mut blob_store = room.blobs.write().await;
            for entry in &blobs {
                let hash_hex = entry["hash"].as_str().unwrap_or("");
                let data_b64 = entry["data"].as_str().unwrap_or("");
                if let (Some(expected), Ok(bytes)) =
                    (parse_hex_hash(hash_hex), base64_decode(data_b64))
                {
                    let actual = activesync_core::Hash::of(&bytes);
                    if actual == expected {
                        // F4: write-through persistence for accepted blobs.
                        room.persistence.persist_blob(&room.room_id, &actual, &bytes);
                        blob_store.put(bytes);
                        stored += 1;
                    }
                }
            }
            // Blobs are served on-demand via blob-request.
            // Broadcast a lightweight notification so peers know new blobs are
            // available and can request them without waiting for the next delta.
            if stored > 0 {
                let available: Vec<_> = blobs.iter()
                    .filter_map(|e| e["hash"].as_str())
                    .collect();
                let bcast = serde_json::json!({
                    "type":  "blob-available",
                    "hashes": available,
                }).to_string();
                let _ = room.tx.send(bcast);
            }
        }

        // Client requests specific blobs -----------------------------------
        "blob-request" => {
            let hashes: Vec<String> = msg["hashes"]
                .as_array().unwrap_or(&vec![])
                .iter().filter_map(|v| v.as_str().map(str::to_string)).collect();

            // F6: split into "redirect" and "fall-through" buckets. Only
            // peers that negotiated `supports_direct_blob_io` get redirects.
            let mut redirects: Vec<serde_json::Value> = Vec::new();
            let mut fallthrough: Vec<String> = Vec::new();
            if negotiated_caps.supports_direct_blob_io {
                for h in &hashes {
                    let Some(hash) = parse_hex_hash(h) else { fallthrough.push(h.clone()); continue };
                    match room.persistence.resolve_get_url(&room.room_id, &hash, None) {
                        Some(p) => redirects.push(serde_json::json!({
                            "hash": h,
                            "url": p.url,
                            "expires_at_unix": p.expires_at_unix,
                        })),
                        None => fallthrough.push(h.clone()),
                    }
                }
            } else {
                fallthrough = hashes.clone();
            }
            if !redirects.is_empty() {
                let reply = serde_json::json!({
                    "type": "blob-redirect",
                    "redirects": redirects,
                }).to_string();
                if !ws_send(sink, room_id, reply).await { return false; }
            }

            // For everything not redirected, fall through to bytes-over-WS.
            let blob_store = room.blobs.read().await;
            let entries: Vec<_> = fallthrough.iter().filter_map(|h| {
                let hash = parse_hex_hash(h)?;
                let data = blob_store.get(&hash)?;
                Some(serde_json::json!({"hash": h, "data": base64_encode(&data)}))
            }).collect();
            // Always reply including the originally requested hashes so the
            // client can clear its pending set even for unavailable blobs.
            let reply = serde_json::json!({
                "type":      "blob-pack",
                "blobs":     entries,
                "requested": fallthrough,
            }).to_string();
            if !ws_send(sink, room_id, reply).await { return false; }
        }

        // F6: client wants a presigned PUT URL ----------------------------
        // C→S: { type:"request-upload", hash, size }
        // S→C: { type:"upload-granted", hash, url, expires_at_unix }
        //   or { type:"upload-denied",  hash, reason }
        "request-upload" => {
            if !negotiated_caps.supports_direct_blob_io {
                send_error(sink, "request-upload not negotiated").await;
                return true;
            }
            let hash_hex = msg["hash"].as_str().unwrap_or("");
            let size = msg["size"].as_u64().unwrap_or(0);
            let content_type = msg["content_type"].as_str().map(|s| s.to_string());
            let Some(hash) = parse_hex_hash(hash_hex) else {
                send_error(sink, "request-upload: bad hash").await;
                return true;
            };
            let reply = match room.persistence.resolve_put_url(&room.room_id, &hash, size, content_type.as_deref()) {
                Some(p) => serde_json::json!({
                    "type": "upload-granted",
                    "hash": hash_hex,
                    "url":  p.url,
                    "expires_at_unix": p.expires_at_unix,
                }),
                None => serde_json::json!({
                    "type":   "upload-denied",
                    "hash":   hash_hex,
                    "reason": "use-ws",
                }),
            };
            if !ws_send(sink, room_id, reply.to_string()).await { return false; }
        }

        // F6: client claims a presigned PUT completed --------------------
        // C→S: { type:"blob-uploaded", hash }
        // Server verifies via BlobPersistence::verify_uploaded (HEAD on S3)
        // and broadcasts blob-available so other peers can fetch it.
        "blob-uploaded" => {
            if !negotiated_caps.supports_direct_blob_io {
                send_error(sink, "blob-uploaded not negotiated").await;
                return true;
            }
            let hash_hex = msg["hash"].as_str().unwrap_or("");
            let Some(hash) = parse_hex_hash(hash_hex) else {
                send_error(sink, "blob-uploaded: bad hash").await;
                return true;
            };
            match room.persistence.verify_uploaded(&room.room_id, &hash) {
                Ok(()) => {
                    let bcast = serde_json::json!({
                        "type":   "blob-available",
                        "hashes": [hash_hex],
                    }).to_string();
                    let _ = room.tx.send(bcast);
                }
                Err(e) => {
                    tracing::warn!(?e, hash = %hash_hex, "blob-uploaded verify failed");
                    let reply = serde_json::json!({
                        "type":   "upload-rejected",
                        "hash":   hash_hex,
                        "reason": e,
                    }).to_string();
                    if !ws_send(sink, room_id, reply).await { return false; }
                }
            }
        }

        // Ephemeral presence — forward, do not store -----------------------
        "presence" => {
            let bcast = serde_json::json!({
                "type": "presence",
                "from": pubkey_hex,
                "data": msg["data"],
            }).to_string();
            let _ = room.tx.send(bcast);
        }

        // C3: lock the room with an Ed25519 verifying key -------------------
        // Only accepted if the room is currently open (auth_key is None).
        // Once set, the key cannot be changed without restarting the server.
        "set-room-key" => {
            let vk_hex = msg["pubkey"].as_str().unwrap_or("");
            match parse_verifying_key(vk_hex) {
                Some(vk) => {
                    let mut auth_key = room.auth_key.write().await;
                    if auth_key.is_none() {
                        *auth_key = Some(vk);
                        let reply = serde_json::json!({
                            "type":   "room-locked",
                            "pubkey": vk_hex,
                        }).to_string();
                        if !ws_send(sink, room_id, reply).await { return false; }
                    } else {
                        send_error(sink, "room already locked").await;
                    }
                }
                None => { send_error(sink, "set-room-key: invalid pubkey hex").await; }
            }
        }

        // E1: install a room-level write policy --------------------------------
        // Wire format:
        //   { type: "set-policy",
        //     default: "allow" | "deny",
        //     rules: [ { path_glob, can_write: ["<hex64>", …] }, … ] }
        //
        // `can_write` entries are 64-char hex Ed25519 verifying keys.
        // The server's own pubkey may be listed to designate it as the sole
        // authority for a protected path (Authoritative mode).
        "set-policy" => {
            let policy = parse_policy(&msg, sink).await;
            match policy {
                Some(p) => {
                    room.set_policy(p).await;
                    let reply = serde_json::json!({"type":"policy-set"}).to_string();
                    if !ws_send(sink, room_id, reply).await { return false; }
                }
                None => {} // parse_policy already sent the error
            }
        }

        // E1: client requests the server's public key -------------------------
        // Useful for building policies that reference the server as authority.
        "server-info" => {
            let vk_hex = hex_from_bytes(&server_key.verifying_key().to_bytes());
            let reply = serde_json::json!({
                "type":   "server-info",
                "pubkey": vk_hex,
            }).to_string();
            if !ws_send(sink, room_id, reply).await { return false; }
        }

        // E1: start the Authoritative tick loop for this room -----------------
        // Wire format:
        //   { type: "start-tick",
        //     interval_ms: 16,            // optional, default 16 (≈60fps)
        //     intent_prefix: "intent/" }  // optional, default "intent/"
        //
        // The server reads all LWW values under `intent_prefix`, calls the
        // default pass-through tick function (intent/x → world/x), commits a
        // signed canonical node, and broadcasts it to all peers.
        //
        // Respond with "tick-started" (first call) or "tick-already-running"
        // (idempotent: a loop is already active for this room).
        "start-tick" => {
            let interval_ms = msg["interval_ms"].as_u64().unwrap_or(16).max(1);
            let intent_prefix = msg["intent_prefix"]
                .as_str()
                .unwrap_or("intent/")
                .to_string();
            let started = room.start_tick(Arc::clone(server_key), interval_ms, intent_prefix);
            let reply = serde_json::json!({
                "type": if started { "tick-started" } else { "tick-already-running" },
                "interval_ms": interval_ms,
            }).to_string();
            if !ws_send(sink, room_id, reply).await { return false; }
        }

        // E1: stop the tick loop for this room --------------------------------
        "stop-tick" => {
            room.stop_tick();
            let reply = serde_json::json!({"type":"tick-stopped"}).to_string();
            if !ws_send(sink, room_id, reply).await { return false; }
        }

        // D3: Compact the room graph into a snapshot --------------------------
        // Wire format (client → server):
        //   { type: "compact-room" }
        //
        // The server:
        //   1. Compacts the current graph into a snapshot sentinel node.
        //   2. Rebuilds the room graph from the snapshot (dropping old history).
        //   3. Broadcasts the snapshot pack to all connected peers so they can
        //      install it with `apply_snapshot_pack_json` on the JS side.
        //
        // Server → Client broadcast:
        //   { type: "snapshot-pack", pack_b64: "<base64>", snapshot_hash: "<hex64>" }
        "compact-room" => {
            // 1. Compact current graph into a snapshot node.
            let snap = {
                let graph = room.graph.read().await;
                match compact(&*graph, server_key) {
                    Ok(s) => s,
                    Err(e) => { send_error(sink, &format!("compact failed: {e}")).await; return true; }
                }
            };

            // 2. Verify and extract metadata.
            let meta = match verify_snapshot(&snap) {
                Ok(m) => m,
                Err(e) => { send_error(sink, &format!("snapshot verify failed: {e}")).await; return true; }
            };
            let hash_hex = meta.snapshot_hash.to_hex();

            // 3. Rebuild the room graph from the snapshot (prunes old nodes).
            let rebuilt = match rebuild_from_snapshot(snap.clone(), &[]) {
                Ok(g) => g,
                Err(e) => { send_error(sink, &format!("rebuild failed: {e}")).await; return true; }
            };
            *room.graph.write().await = rebuilt;

            // 4. Pack the snapshot node and broadcast to all peers.
            let pack_bytes = pack_snapshot_pack(&snap, &[]);
            let pack_b64 = base64_encode(&pack_bytes);
            let bcast = serde_json::json!({
                "type":          "snapshot-pack",
                "pack_b64":      pack_b64,
                "snapshot_hash": hash_hex,
            }).to_string();
            let _ = room.tx.send(bcast);

            // Confirm to the requesting client.
            let reply = serde_json::json!({
                "type":          "compact-ack",
                "snapshot_hash": hash_hex,
            }).to_string();
            if !ws_send(sink, room_id, reply).await { return false; }
        }

        // D2: WebRTC signaling relay ----------------------------------------
        // The server is an ICE signaling relay only — it never touches the
        // WebRTC media or data. Messages are broadcast to the room; each client
        // filters by the `to` field so only the intended recipient processes them.
        //
        // Wire format (client → server):
        //   { type: "webrtc-offer"|"webrtc-answer"|"webrtc-ice",
        //     to: "<peer_pubkey_hex>",
        //     sdp?: RTCSessionDescriptionInit,      // offer / answer
        //     candidate?: RTCIceCandidateInit }     // ICE
        //
        // Server → room broadcast (adds `from` field):
        //   { type: "webrtc-offer"|"webrtc-answer"|"webrtc-ice",
        //     from: "<our_pubkey_hex>", to: "...", ... }
        "webrtc-offer" | "webrtc-answer" | "webrtc-ice" => {
            let mut relay = msg.clone();
            relay["from"] = serde_json::json!(pubkey_hex);
            let _ = room.tx.send(relay.to_string());
        }

        _ => {}
    }

    true
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn send_error(sink: &mut SplitSink<WebSocket, Message>, msg: &str) {
    let j = serde_json::json!({"type":"error","msg":msg}).to_string();
    let _ = sink.send(Message::Text(j.into())).await;
}

// G1 — backpressure & slow-client policy.
//
// `ws_send` wraps every application-level outbound message in a 5-second
// timeout. A stalled TCP write (dead TLS terminator, kernel socket buffer
// full for a peer whose NIC is gone, etc.) would otherwise wedge the WS
// task until the OS times out — potentially minutes.
//
// On timeout we:
//   1. increment `activesync_ws_send_timeout_total{room}` (G7-registered),
//   2. best-effort deliver a `1011 server overload` close frame (bounded
//      by another short timeout so a fully-wedged socket can't re-trap us),
//   3. return `false` so the caller exits the loop, which triggers the
//      normal cleanup path (deregister_peer, peer-left broadcast).
//
// Non-timeout send errors (peer already hung up) also return `false`; no
// metric because that's the common graceful-close path and not a defect.
const WS_SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const WS_CLOSE_FRAME_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

async fn ws_send(
    sink: &mut SplitSink<WebSocket, Message>,
    room_id: &str,
    text: String,
) -> bool {
    match tokio::time::timeout(WS_SEND_TIMEOUT, sink.send(Message::Text(text.into()))).await {
        Ok(Ok(())) => true,
        Ok(Err(_)) => false,
        Err(_) => {
            metrics::counter!(
                "activesync_ws_send_timeout_total",
                "room" => room_id.to_string(),
            ).increment(1);
            tracing::warn!(room = %room_id, "ws send timed out — closing with 1011 server overload");
            let _ = tokio::time::timeout(
                WS_CLOSE_FRAME_TIMEOUT,
                sink.send(Message::Close(Some(CloseFrame {
                    code: 1011,
                    reason: std::borrow::Cow::Borrowed("server overload"),
                }))),
            ).await;
            false
        }
    }
}

// G3 — per-peer rate limiting.
//
// `check_peer_rate` charges `n` units against the supplied limiter. On
// success returns `Ok(())`. On either (a) `NotUntil` (rate exceeded) or
// (b) `InsufficientCapacity` (single request larger than the 1-second
// burst), the peer is closed with WS code `4008 rate limit exceeded`,
// `activesync_rate_limit_drops_total{peer=<12-char hex>}` is incremented,
// and `Err(())` is returned so the handler can break out of its loop.
//
// The close frame itself is sent under the same bounded timeout used for
// 1011 in G1 — we don't want a wedged TCP write to trap a violator's
// cleanup. We deliberately do NOT send a JSON `error` message first; the
// SDK treats 4008 as fatal and shouldn't see inconsistent state.
async fn check_peer_rate(
    lim: &PeerLimiter,
    n: NonZeroU32,
    peer_hex: &str,
    room_id: &str,
    sink: &mut SplitSink<WebSocket, Message>,
    dim: &'static str,
) -> Result<(), ()> {
    match lim.check_n(n) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_not_until)) => {
            deny_peer_rate(peer_hex, room_id, sink, dim, "exceeded").await;
            Err(())
        }
        Err(_insufficient_capacity) => {
            // Single pack larger than the configured 1-second burst. We
            // still disconnect with 4008: a well-behaved client should
            // split large packs; an unsplit one is either malicious or
            // mis-configured relative to the operator's ceiling.
            deny_peer_rate(peer_hex, room_id, sink, dim, "oversized").await;
            Err(())
        }
    }
}

async fn deny_peer_rate(
    peer_hex: &str,
    room_id: &str,
    sink: &mut SplitSink<WebSocket, Message>,
    dim: &'static str,
    kind: &'static str,
) {
    let peer_label = crate::metrics::peer_label(peer_hex);
    metrics::counter!(
        "activesync_rate_limit_drops_total",
        "peer" => peer_label.clone(),
    ).increment(1);
    tracing::warn!(
        peer = %peer_label,
        room = %room_id,
        dim,
        kind,
        "peer rate limit tripped — closing with 4008"
    );
    let _ = tokio::time::timeout(
        WS_CLOSE_FRAME_TIMEOUT,
        sink.send(Message::Close(Some(CloseFrame {
            code: 4008,
            reason: std::borrow::Cow::Borrowed("rate limit exceeded"),
        }))),
    ).await;
}

/// Parse a `set-policy` JSON message into a `Policy`.
/// Sends an error and returns `None` on any parse failure.
async fn parse_policy(
    msg: &Value,
    sink: &mut SplitSink<WebSocket, Message>,
) -> Option<Policy> {
    let default = match msg["default"].as_str().unwrap_or("allow") {
        "deny"  => PolicyDefault::DenyAll,
        "allow" => PolicyDefault::AllowAll,
        other   => {
            send_error(sink, &format!("set-policy: unknown default '{other}', use 'allow' or 'deny'")).await;
            return None;
        }
    };

    let mut rules = Vec::new();
    let rule_arr = msg["rules"].as_array().cloned().unwrap_or_default();
    for rule_val in &rule_arr {
        let path_glob = match rule_val["path_glob"].as_str() {
            Some(s) => s.to_string(),
            None => {
                send_error(sink, "set-policy: each rule must have a 'path_glob' string").await;
                return None;
            }
        };
        let can_write: Vec<[u8; 32]> = match rule_val["can_write"].as_array() {
            Some(arr) => {
                let mut keys = Vec::new();
                for v in arr {
                    let hex = v.as_str().unwrap_or("");
                    match parse_hex_32(hex) {
                        Some(b) => keys.push(b),
                        None => {
                            send_error(sink, &format!("set-policy: invalid pubkey hex '{hex}'")).await;
                            return None;
                        }
                    }
                }
                keys
            }
            None => vec![],
        };
        rules.push(PolicyRule { path_glob, can_write, can_read: vec![], can_derive: vec![] });
    }

    Some(Policy { rules, default })
}

fn hex_from_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}


/// Verify the capability token in a `hello` message against the room's key (C3).
///
/// On success returns the token's `expiry_secs` (Unix seconds) so the
/// session loop can disconnect the peer the moment it expires (G6).
fn verify_hello_token(
    hello: &Value,
    room_vk: &VerifyingKey,
    peer_pubkey_hex: &str,
    room_id: &str,
) -> Result<u64, String> {
    let tok = hello.get("token").ok_or("room is locked — include a capability token in hello")?;
    let peer_hex   = tok["peer_pubkey"].as_str().ok_or("missing token.peer_pubkey")?;
    let expiry     = tok["expiry"].as_u64().ok_or("missing token.expiry")?;
    let sig_hex    = tok["sig"].as_str().ok_or("missing token.sig")?;
    // Capabilities: optional array of strings; empty = full access.
    let caps: Vec<String> = tok["caps"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();

    let peer_bytes = parse_hex_32(peer_pubkey_hex)
        .ok_or_else(|| "invalid peer pubkey in hello".to_string())?;

    let token = RoomToken::from_wire(peer_hex, expiry, caps, sig_hex)
        .map_err(|e| e.to_string())?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    token.verify(room_id, room_vk, &peer_bytes, now)
        .map_err(|e| e.to_string())?;
    Ok(expiry)
}

/// Parse a 32-byte Ed25519 verifying key from a 64-char hex string.
fn parse_verifying_key(hex: &str) -> Option<VerifyingKey> {
    let bytes = parse_hex_32(hex)?;
    VerifyingKey::from_bytes(&bytes).ok()
}

/// Parse 32 raw bytes from a 64-char hex string.
fn parse_hex_32(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 { return None; }
    let mut bytes = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let hi = hex_nibble(chunk[0])?;
        let lo = hex_nibble(chunk[1])?;
        bytes[i] = (hi << 4) | lo;
    }
    Some(bytes)
}

fn parse_hex_list(val: &Value) -> Vec<activesync_core::NodeId> {
    val.as_array().unwrap_or(&vec![])
        .iter()
        .filter_map(|v| parse_hex_hash(v.as_str()?))
        .collect()
}

fn parse_hex_hash(hex: &str) -> Option<activesync_core::Hash> {
    if hex.len() != 64 { return None; }
    let mut bytes = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let hi = hex_nibble(chunk[0])?;
        let lo = hex_nibble(chunk[1])?;
        bytes[i] = (hi << 4) | lo;
    }
    Some(activesync_core::Hash(bytes))
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn base64_decode(s: &str) -> Result<Vec<u8>, ()> {
    const TABLE: &[u8; 128] = b"\
        \xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\
        \xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\
        \xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\x3e\xff\xff\xff\x3f\
        \x34\x35\x36\x37\x38\x39\x3a\x3b\x3c\x3d\xff\xff\xff\xff\xff\xff\
        \xff\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0c\x0d\x0e\
        \x0f\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19\xff\xff\xff\xff\xff\
        \xff\x1a\x1b\x1c\x1d\x1e\x1f\x20\x21\x22\x23\x24\x25\x26\x27\x28\
        \x29\x2a\x2b\x2c\x2d\x2e\x2f\x30\x31\x32\x33\xff\xff\xff\xff\xff";
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut i = 0;
    while i < bytes.len() {
        let b0 = *bytes.get(i).ok_or(())?;
        let b1 = *bytes.get(i + 1).ok_or(())?;
        if b0 == b'=' { break; }
        let v0 = *TABLE.get(b0 as usize).ok_or(())? as u32;
        let v1 = *TABLE.get(b1 as usize).ok_or(())? as u32;
        if v0 == 0xff || v1 == 0xff { return Err(()); }
        out.push(((v0 << 2) | (v1 >> 4)) as u8);
        let b2 = bytes.get(i + 2).copied().unwrap_or(b'=');
        if b2 != b'=' {
            let v2 = *TABLE.get(b2 as usize).ok_or(())? as u32;
            if v2 == 0xff { return Err(()); }
            out.push(((v1 << 4) | (v2 >> 2)) as u8);
            let b3 = bytes.get(i + 3).copied().unwrap_or(b'=');
            if b3 != b'=' {
                let v3 = *TABLE.get(b3 as usize).ok_or(())? as u32;
                if v3 == 0xff { return Err(()); }
                out.push(((v2 << 6) | v3) as u8);
            }
        }
        i += 4;
    }
    Ok(out)
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
        let b2 = chunk.get(2).copied().unwrap_or(0) as usize;
        out.push(CHARS[b0 >> 2] as char);
        out.push(CHARS[((b0 & 3) << 4) | (b1 >> 4)] as char);
        if chunk.len() > 1 { out.push(CHARS[((b1 & 0xf) << 2) | (b2 >> 6)] as char); } else { out.push('='); }
        if chunk.len() > 2 { out.push(CHARS[b2 & 0x3f] as char); } else { out.push('='); }
    }
    out
}


#[cfg(test)]
mod subscription_tests {
    use super::*;

    fn m(p: &str, s: &str) -> bool { glob_match(p.as_bytes(), s.as_bytes()) }

    #[test]
    fn trailing_slash_star_star_is_optional_subtree() {
        assert!(m("world/**", "world"));
        assert!(m("world/**", "world/a"));
        assert!(m("world/**", "world/a/b"));
        assert!(!m("world/**", "worldx"));
        assert!(!m("world/**", "other"));
    }

    #[test]
    fn segment_star_does_not_cross_slash() {
        assert!(m("world/*", "world/a"));
        assert!(!m("world/*", "world/a/b"));
        assert!(!m("world/*", "world"));
    }

    #[test]
    fn literal_match() {
        assert!(m("notes/welcome", "notes/welcome"));
        assert!(!m("notes/welcome", "notes/other"));
    }

    #[test]
    fn double_star_alone_matches_everything() {
        assert!(GlobMatcher::compile("**").is_match(""));
        assert!(GlobMatcher::compile("**").is_match("a/b/c"));
    }

    #[test]
    fn star_dot_escapes_are_literal() {
        assert!(m("chat.*/msg", "chat.v1/msg"));
        assert!(!m("chat.*/msg", "chatXv1/msg"));
    }

    #[test]
    fn leading_double_star_then_literal() {
        assert!(m("**/msg", "msg"));
        assert!(m("**/msg", "a/msg"));
        assert!(m("**/msg", "a/b/msg"));
        assert!(!m("**/msg", "a/msgx"));
    }

    #[test]
    fn middle_double_star_optional_subtree() {
        assert!(m("a/**/c", "a/c"));
        assert!(m("a/**/c", "a/b/c"));
        assert!(m("a/**/c", "a/b/x/c"));
        assert!(!m("a/**/c", "a/cx"));
        assert!(!m("a/**/c", "ax/c"));
    }

    #[test]
    fn subscription_accepts_everything_by_default() {
        let s = Subscription::everything();
        assert!(s.matches_everything());
        assert!(s.matches_path("anything/at/all"));
    }

    #[test]
    fn subscription_from_patterns_or() {
        let s = Subscription::from_patterns(vec!["world/**".into(), "chat/*".into()]);
        assert!(s.matches_path("world"));
        assert!(s.matches_path("world/a/b"));
        assert!(s.matches_path("chat/x"));
        assert!(!s.matches_path("chat/x/y"));
        assert!(!s.matches_path("other"));
    }

    #[test]
    fn empty_pattern_list_means_everything() {
        let s = Subscription::from_patterns(vec![]);
        assert!(s.matches_everything());
    }
}
