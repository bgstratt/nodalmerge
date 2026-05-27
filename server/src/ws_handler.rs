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

use std::sync::{Arc, OnceLock};
use std::path::PathBuf;
use std::collections::HashSet;
use axum::{
    extract::{Path, State, WebSocketUpgrade, ws::{CloseFrame, Message, WebSocket}},
    response::Response,
};
use futures_util::{SinkExt, StreamExt, stream::SplitSink};
use serde_json::Value;
use nodalmerge_core::{ArchiveWsResponse, BlobStore, MerkleSearchTree, Policy, PolicyDefault, PolicyRule,
                      RoomToken, SyncCapabilities, SyncNode, pack_nodes, unpack_nodes,
                      compact, rebuild_from_snapshot, pack_snapshot_pack, verify_snapshot};
use nodalmerge_host_core::protocol::{
    BlobPackEntry,
    BlobRedirectEntry,
    assemble_blob_available_envelope,
    assemble_blob_pack_envelope,
    assemble_error_envelope,
    assemble_mst_response_envelope,
    assemble_blob_redirect_envelope,
    assemble_normalized_peer_stamped_relay_envelope,
    assemble_presence_envelope,
    assemble_peer_pack_relay_envelope,
    assemble_peer_joined_envelope,
    assemble_peer_left_envelope,
    assemble_rate_limit_exceeded_close_frame,
    assemble_resync_required_close_frame,
    assemble_room_locked_envelope,
    assemble_server_overload_close_frame,
    assemble_server_pack_reply_envelope,
    assemble_server_info_envelope,
    assemble_set_room_key_rejected_envelope,
    assemble_token_expired_close_frame,
    assemble_tick_start_envelope,
    assemble_tick_stopped_envelope,
    assemble_snapshot_pack_envelope,
    assemble_compact_ack_envelope,
    assemble_policy_set_envelope,
    assemble_subscribe_ack_envelope,
    assemble_upload_denied_envelope,
    assemble_upload_granted_envelope,
    assemble_upload_rejected_envelope,
    negotiate_capabilities,
    welcome_mst_root_hex,
    decide_sync_diff,
    ClientIbfInput,
    CloseFrameSpec,
    SyncDiffInput,
    serialize_archive_ws_response,
};
use nodalmerge_host_core::engine::{
    assemble_welcome_catchup_package,
    classify_hello_payload,
    parse_client_capabilities,
    parse_client_frontier_node_ids,
    parse_client_ibf_input,
    classify_request_upload_resolve_put_url_outcome,
    plan_direct_blob_io_bad_hash_gate,
    plan_direct_blob_io_negotiation_gate,
    classify_direct_blob_io_verify_outcome,
    DirectBlobIoVerifyOutcome,
    RequestUploadResolvePutUrlOutcome,
    PackImportMutationAction,
    BlobStoreMutationAction,
    plan_has_catchup,
    plan_pack_import_mutation,
    plan_blob_store_mutation,
    plan_peer_rate_limiters,
    plan_token_deadline_remaining_secs,
    plan_filtered_catchup_send_payload,
    serialize_catchup_pack_envelope_json,
    should_break_main_loop_after_push_send,
    should_terminate_after_blob_upload_verify_failure_reply_send,
    should_terminate_after_compact_room_ack_send,
    should_terminate_after_filtered_catchup_send,
    should_ignore_unknown_client_message_type,
    should_terminate_after_server_info_reply_send,
    should_terminate_after_set_policy_reply_send,
    should_terminate_after_set_room_key_locked_ack_send,
    should_terminate_after_start_tick_reply_send,
    should_terminate_after_stop_tick_reply_send,
    should_terminate_after_subscribe_ack_send,
    should_terminate_after_mst_request_reply_send,
    should_terminate_after_mst_done_server_pack_send,
    should_terminate_after_request_server_pack_send,
    should_terminate_after_blob_request_redirect_reply_send,
    should_terminate_after_blob_request_blob_pack_reply_send,
    should_terminate_after_request_upload_reply_send,
    should_terminate_after_welcome_send,
    classify_set_room_key_lock_state,
    classify_set_room_key_parse_result,
    extract_blob_upload_entry_data_b64,
    extract_blob_upload_entry_hash_text,
    extract_blob_uploaded_hash_text,
    extract_blob_request_hashes,
    extract_hello_pubkey_text,
    extract_pack_nodes_payload_b64,
    extract_mst_done_ids,
    extract_mst_request_paths,
    extract_request_upload_content_type,
    extract_request_upload_hash_text,
    extract_request_upload_size,
    extract_presence_data_payload,
    extract_set_room_key_pubkey_text,
    extract_set_policy_can_write_hex_text,
    extract_set_policy_can_write_values,
    extract_set_policy_default_text,
    extract_set_policy_rule_values,
    extract_start_tick_interval_ms,
    extract_start_tick_intent_prefix,
    extract_webrtc_relay_fields,
    SetRoomKeyLockStateResult,
    SetRoomKeyParseResult,
    shape_set_room_key_already_locked_rejection_reason,
    shape_set_room_key_invalid_pubkey_rejection_reason,
    classify_set_policy_default,
    classify_set_policy_parse_result,
    shape_set_policy_unknown_default_error,
    SetPolicyDefaultParseResult,
    SetPolicyParseResult,
    classify_compact_room_compaction_result,
    classify_compact_room_verify_result,
    classify_compact_room_rebuild_result,
    shape_compact_room_compaction_failed_error,
    shape_compact_room_verify_failed_error,
    shape_compact_room_rebuild_failed_error,
    CompactRoomCompactionResult,
    CompactRoomVerifyResult,
    CompactRoomRebuildResult,
    shape_blob_uploaded_verify_failure_rejection_payload,
    shape_client_known_id_set,
    shape_request_known_id_set,
    shape_blob_uploaded_available_hashes,
    shape_catchup_pack_payload_b64,
    shape_welcome_missing_hex,
    shape_welcome_peer_list,
    shape_welcome_root_hex,
    shape_welcome_server_frontier_hex,
    should_skip_self_echo_broadcast_envelope,
    HelloPayloadClassification,
};
use ed25519_dalek::{SigningKey, VerifyingKey};
use governor::{Quota, RateLimiter, clock::DefaultClock, state::{InMemoryState, NotKeyed}};
use std::num::NonZeroU32;

use crate::adapter_context::{
    ClientDispatchBuild,
    ClientDispatchCommand,
    build_client_dispatch_context,
};
use crate::archive_adapter::{
    process_archive_describe,
    process_archive_export,
    process_archive_import,
    process_archive_validate,
};
use crate::capability_profile::{
    CapabilityProfile,
    flatten_capabilities,
    load_capability_profile_from_path,
    profile_supports_version,
};
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

#[derive(Clone, Debug, Default)]
struct ScopeFilterOutcome {
    filtered_env: Option<String>,
    filter_applied: bool,
    accepted_count: usize,
    rejected_count: usize,
    incoming_bytes: usize,
    accepted_bytes: usize,
}

impl ScopeFilterOutcome {
    fn filtered_nodes(&self) -> usize {
        self.rejected_count
    }

    fn filtered_bytes(&self) -> usize {
        self.incoming_bytes.saturating_sub(self.accepted_bytes)
    }
}

fn filter_pack_for_subscriber_with_stats(env: &str, sub: &Subscription) -> ScopeFilterOutcome {
    if sub.matches_everything() {
        return ScopeFilterOutcome {
            filtered_env: Some(env.to_string()),
            ..ScopeFilterOutcome::default()
        };
    }
    // Parse just enough to decide.
    let v: Value = match serde_json::from_str(env) {
        Ok(v) => v,
        Err(_) => {
            return ScopeFilterOutcome {
                filtered_env: Some(env.to_string()),
                ..ScopeFilterOutcome::default()
            }
        }
    };
    if v["type"] != "pack" {
        return ScopeFilterOutcome {
            filtered_env: Some(env.to_string()),
            ..ScopeFilterOutcome::default()
        };
    }
    let nodes_b64 = match v["nodes"].as_str() {
        Some(s) => s,
        None => {
            return ScopeFilterOutcome {
                filtered_env: Some(env.to_string()),
                ..ScopeFilterOutcome::default()
            }
        }
    };
    let bytes = match base64_decode(nodes_b64) {
        Ok(b) => b,
        Err(_) => {
            return ScopeFilterOutcome {
                filtered_env: Some(env.to_string()),
                ..ScopeFilterOutcome::default()
            }
        }
    };
    let nodes: Vec<SyncNode> = match unpack_nodes(&bytes) {
        Ok(n) => n,
        Err(_) => {
            return ScopeFilterOutcome {
                filtered_env: Some(env.to_string()),
                ..ScopeFilterOutcome::default()
            }
        }
    };
    let incoming_count = nodes.len();
    let incoming_bytes = bytes.len();
    let kept: Vec<SyncNode> = nodes.into_iter().filter(|n| sub.accepts_node(n)).collect();
    let accepted_count = kept.len();
    let rejected_count = incoming_count.saturating_sub(accepted_count);
    if kept.is_empty() {
        return ScopeFilterOutcome {
            filtered_env: None,
            filter_applied: true,
            accepted_count,
            rejected_count,
            incoming_bytes,
            accepted_bytes: 0,
        };
    }
    let kept_refs: Vec<&SyncNode> = kept.iter().collect();
    let accepted_bytes_raw = pack_nodes(&kept_refs);
    let accepted_bytes = accepted_bytes_raw.len();
    let mut out = v.clone();
    out["nodes"] = Value::String(base64_encode(&accepted_bytes_raw));
    ScopeFilterOutcome {
        filtered_env: Some(out.to_string()),
        filter_applied: true,
        accepted_count,
        rejected_count,
        incoming_bytes,
        accepted_bytes,
    }
}

#[derive(Clone, Copy, Debug)]
struct ScopeCatchupBudget {
    max_filtered_node_count: usize,
    max_filtered_payload_bytes: usize,
}

fn scoped_catchup_budget() -> ScopeCatchupBudget {
    static BUDGET: OnceLock<ScopeCatchupBudget> = OnceLock::new();
    *BUDGET.get_or_init(|| ScopeCatchupBudget {
        max_filtered_node_count: env_var_primary_legacy(
            "NODALMERGE_SCOPE_MAX_FILTERED_CATCHUP_NODES",
            "ACTIVESYNC_SCOPE_MAX_FILTERED_CATCHUP_NODES",
        )
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(4096),
        max_filtered_payload_bytes: env_var_primary_legacy(
            "NODALMERGE_SCOPE_MAX_FILTERED_CATCHUP_BYTES",
            "ACTIVESYNC_SCOPE_MAX_FILTERED_CATCHUP_BYTES",
        )
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1024 * 1024),
    })
}

fn env_var_primary_legacy(primary: &str, legacy: &str) -> Result<String, std::env::VarError> {
    std::env::var(primary).or_else(|_| std::env::var(legacy))
}

fn should_drop_filtered_catchup_for_budget(
    accepted_count: usize,
    accepted_bytes: usize,
    budget: ScopeCatchupBudget,
) -> bool {
    accepted_count > budget.max_filtered_node_count
        || accepted_bytes > budget.max_filtered_payload_bytes
}

fn record_scope_filter_metrics(room_id: &str, stage: &str, outcome: &ScopeFilterOutcome) {
    if !outcome.filter_applied {
        return;
    }

    let filtered_nodes = outcome.filtered_nodes();
    let filtered_bytes = outcome.filtered_bytes();
    if filtered_nodes > 0 {
        metrics::counter!(
            "nodalmerge_filtered_nodes_total",
            "room" => room_id.to_string(),
            "stage" => stage.to_string(),
        )
        .increment(filtered_nodes as u64);
        metrics::counter!(
            "nodalmerge_filtered_nodes_total",
            "room" => room_id.to_string(),
            "stage" => stage.to_string(),
        )
        .increment(filtered_nodes as u64);
    }
    if filtered_bytes > 0 {
        metrics::counter!(
            "nodalmerge_filtered_bytes_total",
            "room" => room_id.to_string(),
            "stage" => stage.to_string(),
        )
        .increment(filtered_bytes as u64);
        metrics::counter!(
            "nodalmerge_filtered_bytes_total",
            "room" => room_id.to_string(),
            "stage" => stage.to_string(),
        )
        .increment(filtered_bytes as u64);
    }
}

fn record_scope_filter_drop(room_id: &str, stage: &str, reason: &str) {
    metrics::counter!(
        "nodalmerge_filtered_pack_dropped_total",
        "room" => room_id.to_string(),
        "stage" => stage.to_string(),
        "reason" => reason.to_string(),
    )
    .increment(1);
    metrics::counter!(
        "nodalmerge_filtered_pack_dropped_total",
        "room" => room_id.to_string(),
        "stage" => stage.to_string(),
        "reason" => reason.to_string(),
    )
    .increment(1);
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
    // Handshake: wait for hello (longer timeout so slower demo boots don't
    // get dropped while the WASM bundle or browser is still coming up).
    // -------------------------------------------------------------------------
    let hello_result = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        stream.next(),
    ).await;

    let hello = match hello_result {
        Err(_) => {
            tracing::warn!(%room_id, "hello timeout (15s) — closing");
            return;
        }
        Ok(Some(Ok(Message::Text(t)))) => {
            match classify_hello_payload(&t) {
                HelloPayloadClassification::ValidHello(v) => v,
                HelloPayloadClassification::ProtocolErrorExpectedHello => {
                    tracing::warn!("invalid/non-hello handshake payload — closing");
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

    let pubkey_hex = extract_hello_pubkey_text(&hello);
    let short = &pubkey_hex[..8.min(pubkey_hex.len())];
    tracing::info!(room = %room_id, peer = %short, "peer connected");

    // G3: per-peer rate limiters. The server's own pubkey is exempt so
    // authoritative tick writes are never throttled. Zero disables the
    // corresponding limiter outright.
    let is_server_peer = pubkey_hex == server_pubkey_hex;
    let limiter_plan = plan_peer_rate_limiters(
        is_server_peer,
        rooms.peer_rate_nodes,
        rooms.peer_rate_bytes,
    );
    let node_limiter: Option<PeerLimiter> = if limiter_plan.enable_node_limiter {
        NonZeroU32::new(rooms.peer_rate_nodes).map(|nz| RateLimiter::direct(Quota::per_second(nz)))
    } else {
        None
    };
    let byte_limiter: Option<PeerLimiter> = if limiter_plan.enable_byte_limiter {
        NonZeroU32::new(rooms.peer_rate_bytes).map(|nz| RateLimiter::direct(Quota::per_second(nz)))
    } else {
        None
    };

    // F3b: parse this peer's subscription from hello. Defaults to everything.
    // Updated at runtime via the `subscribe` client message.
    let subscription = Arc::new(std::sync::RwLock::new(
        Subscription::from_hello_value(&hello["subscribe"]),
    ));

    // C3: if the room is locked, verify the capability token before proceeding.
    // G6: record the token's expiry so the main loop can disconnect the
    // session the instant it lapses (short-lived tokens + SDK refresh hook).
    let (token_deadline, session_caps): (Option<tokio::time::Instant>, HashSet<String>) = {
        let auth_key = room.auth_key.read().await;
        let verified_token = if let Some(ref room_vk) = *auth_key {
            match verify_hello_token(&hello, room_vk, &pubkey_hex, &room_id) {
                Ok(v) => Some(v),
                Err(e) => {
                    send_error(&mut sink, &format!("auth: {e}")).await;
                    return;
                }
            }
        } else {
            None
        };

        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let deadline = plan_token_deadline_remaining_secs(
            verified_token.as_ref().map(|t| t.expiry_secs),
            now_secs,
        )
        .map(|remaining| tokio::time::Instant::now() + std::time::Duration::from_secs(remaining));
        let caps = verified_token
            .map(|t| t.capabilities.into_iter().collect())
            .unwrap_or_else(HashSet::new);
        (deadline, caps)
    };

    let client_known = parse_client_frontier_node_ids(&hello);

    // Capability negotiation (A7): parse client caps (missing field → defaults).
    let client_caps: SyncCapabilities = parse_client_capabilities(&hello);
    let negotiated_caps = negotiate_capabilities(&client_caps);

    let client_ibf: ClientIbfInput = parse_client_ibf_input(&hello);

    // Precompute graph inputs used by diff decision helper.
    let (server_ids, missing_from_us) = {
        let graph = room.graph.read().await;
        let client_known_set = shape_client_known_id_set(&client_known);
        (graph.all_node_ids(), graph.missing_hashes(&client_known_set))
    };

    // IBF set-reconciliation (B1): when both peers support IBF and the client
    // included an `ibf` field, decode the symmetric diff directly. Otherwise
    // fall back to the frontier-based over-send approach.
    let (only_in_client, only_in_server): (Vec<_>, Vec<_>) = decide_sync_diff(SyncDiffInput {
        negotiated_supports_ibf: negotiated_caps.supports_ibf,
        client_known,
        client_ibf,
        server_ids,
        legacy_missing_from_us: missing_from_us,
    });
    tracing::debug!(peer = %short, only_in_client = only_in_client.len(), only_in_server = only_in_server.len(), "diff computed");

    // Send welcome: tell client which of ITS nodes WE are missing, and give it
    // any nodes it is missing.  In MST mode we include the server's MST root
    // so the client can drive iterative descent; we still push a catch-up pack
    // for the nodes we know the client is missing via IBF.
    let welcome_and_catchup = {
        let graph = room.graph.read().await;
        let nodes = graph.get_nodes(&only_in_server);
        let pack_b64 = shape_catchup_pack_payload_b64(&nodes);
        let has_catchup = plan_has_catchup(nodes.len());
        let server_frontier = graph.frontier();
        let missing_hex = shape_welcome_missing_hex(&only_in_client);

        // Build MST root when MST is negotiated (B2).
        let all_ids = graph.all_node_ids();
        let mst_root_hex = welcome_mst_root_hex(&negotiated_caps, &all_ids);

        // D2: collect currently-connected peers for the welcome message so the
        // new peer knows who to initiate WebRTC connections with immediately.
        let connected_peers: Vec<String> = room
            .connected_peers
            .read()
            .await
            .iter()
            .cloned()
            .collect();
        let current_peers: Vec<String> = shape_welcome_peer_list(&connected_peers, &pubkey_hex);

        assemble_welcome_catchup_package(
            shape_welcome_root_hex(&graph.merkle_root()),
            shape_welcome_server_frontier_hex(&server_frontier),
            missing_hex,
            negotiated_caps.clone(),
            server_pubkey_hex.clone(),
            current_peers,
            mst_root_hex,
            pack_b64,
            has_catchup,
        )
    };
    let welcome_json = welcome_and_catchup.welcome_json;
    let catchup_b64 = welcome_and_catchup.catchup_b64;
    let has_catchup = welcome_and_catchup.has_catchup;

    // D2: register this peer and broadcast peer-joined to the room.
    room.register_peer(pubkey_hex.clone()).await;

    // Small per-connection stabilization delay to avoid many peers
    // immediately provoking DB selection storms on first Mongo call.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let peer_joined = assemble_peer_joined_envelope(pubkey_hex.clone());
    let peer_joined_json = serde_json::to_string(&peer_joined)
        .expect("peer-joined envelope should always serialize");
    emit_room_broadcast(&room, peer_joined_json);

    let welcome_send_ok = emit_single_send(&mut sink, &room_id, welcome_json).await;
    if should_terminate_after_welcome_send(welcome_send_ok) {
        tracing::warn!(peer = %short, "welcome send failed");
        room.deregister_peer(&pubkey_hex).await;
        return;
    }
    tracing::debug!(peer = %short, "welcome sent, entering main loop");

    // Send catch-up pack if non-empty. F3b: filter through the peer's
    // subscription before sending.
    if has_catchup {
        let env = serialize_catchup_pack_envelope_json(catchup_b64);
        let sub = subscription.read().unwrap().clone();
        let outcome = filter_pack_for_subscriber_with_stats(&env, &sub);
        record_scope_filter_metrics(&room_id, "catchup", &outcome);
        let planned = plan_filtered_catchup_send_payload(outcome.filtered_env.clone());
        if let Some(filtered) = planned {
            if should_drop_filtered_catchup_for_budget(
                outcome.accepted_count,
                outcome.accepted_bytes,
                scoped_catchup_budget(),
            ) {
                tracing::info!(
                    peer = %short,
                    room = %room_id,
                    accepted_count = outcome.accepted_count,
                    accepted_bytes = outcome.accepted_bytes,
                    "dropping filtered catchup payload due to scoped catchup budget"
                );
                record_scope_filter_drop(&room_id, "catchup", "budget_exceeded");
            } else {
                let send_ok = emit_single_send(&mut sink, &room_id, filtered).await;
                if should_terminate_after_filtered_catchup_send(send_ok) {
                    room.deregister_peer(&pubkey_hex).await;
                    return;
                }
            }
        } else if outcome.filter_applied {
            record_scope_filter_drop(&room_id, "catchup", "empty_after_filter");
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
                    "nodalmerge_token_expired_disconnects_total",
                    "room" => room_id.clone(),
                ).increment(1);
                metrics::counter!(
                    "nodalmerge_token_expired_disconnects_total",
                    "room" => room_id.clone(),
                ).increment(1);
                tracing::info!(peer = %short, room = %room_id, "token expired — closing with 4002");
                emit_close_frame(&mut sink, assemble_token_expired_close_frame()).await;
                break;
            }
            // --- inbound from this client ---
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if !handle_client_message(&text, &room, &pubkey_hex, is_server_peer, &room_id, &server_key, &mut sink, &subscription, &session_caps, node_limiter.as_ref(), byte_limiter.as_ref(), &negotiated_caps).await {
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
                        if should_skip_self_echo_broadcast_envelope(&env, &pubkey_hex) {
                            continue;
                        }
                        // F3b: subscription filter. Pack envelopes may be rewritten
                        // to drop nodes outside this peer's subscription, or
                        // dropped entirely when nothing matches. All other
                        // message types pass through unchanged.
                        let sub = subscription.read().unwrap().clone();
                        let outcome = filter_pack_for_subscriber_with_stats(&env, &sub);
                        record_scope_filter_metrics(&room_id, "broadcast", &outcome);
                        let out = match outcome.filtered_env {
                            Some(s) => s,
                            None => {
                                if outcome.filter_applied {
                                    record_scope_filter_drop(&room_id, "broadcast", "empty_after_filter");
                                }
                                continue;
                            }
                        };
                        let send_ok = emit_single_send(&mut sink, &room_id, out).await;
                        if should_break_main_loop_after_push_send(send_ok) { break; }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        // G1: slow consumer fell behind the ring buffer.
                        // Previously silently ignored, which left the peer
                        // desynced without any signal. Now: increment the
                        // metric, send a 4001 close frame, and break so the
                        // SDK's exp-backoff reconnect path rebuilds via the
                        // normal hello → IBF diff → catch-up pack handshake.
                        metrics::counter!(
                            "nodalmerge_broadcast_lagged_total",
                            "room" => room_id.clone(),
                        ).increment(1);
                        metrics::counter!(
                            "nodalmerge_broadcast_lagged_total",
                            "room" => room_id.clone(),
                        ).increment(1);
                        tracing::warn!(
                            peer = %short,
                            room = %room_id,
                            lagged = n,
                            "broadcast lagged — closing with 4001 resync required"
                        );
                        emit_close_frame(&mut sink, assemble_resync_required_close_frame()).await;
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
    let peer_left = assemble_peer_left_envelope(pubkey_hex.clone());
    let peer_left_json = serde_json::to_string(&peer_left)
        .expect("peer-left envelope should always serialize");
    emit_room_broadcast(&room, peer_left_json);
    tracing::debug!(peer = %short, "handler fully exited");
}

/// Returns `false` if the connection should be closed.
async fn handle_client_message(
    text: &str,
    room: &Arc<Room>,
    pubkey_hex: &str,
    is_server_peer: bool,
    room_id: &str,
    server_key: &Arc<SigningKey>,
    sink: &mut SplitSink<WebSocket, Message>,
    subscription: &Arc<std::sync::RwLock<Subscription>>,
    session_caps: &HashSet<String>,
    node_limiter: Option<&PeerLimiter>,
    byte_limiter: Option<&PeerLimiter>,
    negotiated_caps: &SyncCapabilities,
) -> bool {
    let dispatch_ctx = match build_client_dispatch_context(text) {
        ClientDispatchBuild::Ready(ctx) => ctx,
        ClientDispatchBuild::Malformed(plan) => {
            send_error(sink, plan.error_message).await;
            return plan.keep_connection_open;
        }
    };
    let msg = &dispatch_ctx.message;
    let message_type = dispatch_ctx.message_type;

    match &dispatch_ctx.command {

        // F3b: update this peer's subscription at runtime -------------------
        ClientDispatchCommand::Subscribe => {
            let sub = Subscription::from_hello_value(&msg["patterns"]);
            *subscription.write().unwrap() = sub;
            let reply_env = assemble_subscribe_ack_envelope();
            let reply = serde_json::to_string(&reply_env)
                .expect("subscribe-ack envelope should always serialize");
            if should_terminate_after_subscribe_ack_send(emit_single_send(sink, room_id, reply).await) { return false; }
        }

        // Client pushes a pack of new nodes --------------------------------
        ClientDispatchCommand::Pack => {
            let nodes_b64 = extract_pack_nodes_payload_b64(&msg);
            let decoded = match base64_decode(&nodes_b64) {
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

            let (accepted, accepted_ids, errs) = import_nodes(room, nodes).await;
            tracing::info!(peer = %&pubkey_hex[..8.min(pubkey_hex.len())], incoming, accepted, errs = errs.len(), "pack received");
            if !errs.is_empty() {
                tracing::warn!(errors = %errs.join("; "), "pack errors");
                let err = serde_json::to_string(&assemble_error_envelope(errs.join("; ")))
                    .expect("error envelope should always serialize");
                let _ = sink.send(Message::Text(err.into())).await;
            }
            if plan_pack_import_mutation(accepted)
                == PackImportMutationAction::BroadcastAcceptedLeafPack {
                // Broadcast exactly the accepted nodes from this import pass.
                // Using current leafs can drop required parent ops (e.g. map
                // sidecar before list insert), which breaks downstream convergence.
                let bcast = {
                    let graph = room.graph.read().await;
                    let nodes = graph.get_nodes(&accepted_ids);
                    let nodes_b64 = base64_encode(&pack_nodes(&nodes));
                    serde_json::to_string(&assemble_peer_pack_relay_envelope(
                        pubkey_hex.to_string(),
                        nodes_b64,
                        graph.merkle_root().to_hex(),
                    )).expect("peer pack relay envelope should always serialize")
                };
                emit_room_broadcast(room, bcast);
            }
        }

        // Client requests MST nodes at specific paths (B2) -------------------
        ClientDispatchCommand::MstRequest => {
            let paths = extract_mst_request_paths(&msg);
            let graph = room.graph.read().await;
            let all_ids = graph.all_node_ids();
            let mst = MerkleSearchTree::from_ids(&all_ids);
            drop(graph);
            let nodes: Vec<_> = paths.iter()
                .filter_map(|p| mst.get_node_wire(p))
                .collect();
            let reply = serde_json::to_string(&assemble_mst_response_envelope(nodes))
                .expect("mst-response envelope should always serialize");
            if should_terminate_after_mst_request_reply_send(emit_single_send(sink, room_id, reply).await) { return false; }
        }

        // Client completed MST descent, requests specific missing nodes -------
        ClientDispatchCommand::MstDone => {
            let ids = extract_mst_done_ids(&msg);
            if !ids.is_empty() {
                let graph = room.graph.read().await;
                let nodes = graph.get_nodes(&ids);
                let nodes_b64 = base64_encode(&pack_nodes(&nodes));
                let reply_env = assemble_server_pack_reply_envelope(
                    nodes_b64,
                    graph.merkle_root().to_hex(),
                );
                let reply = serde_json::to_string(&reply_env)
                    .expect("server pack reply envelope should always serialize");
                drop(graph);
                if should_terminate_after_mst_done_server_pack_send(emit_single_send(sink, room_id, reply).await) { return false; }
            }
        }

        // Client requests nodes it doesn't have ----------------------------
        ClientDispatchCommand::Request => {
            let known_set = shape_request_known_id_set(&msg);
            let graph = room.graph.read().await;
            let missing = graph.missing_hashes(&known_set);
            let nodes = graph.get_nodes(&missing);
            let nodes_b64 = base64_encode(&pack_nodes(&nodes));
            let reply_env = assemble_server_pack_reply_envelope(
                nodes_b64,
                graph.merkle_root().to_hex(),
            );
            let reply = serde_json::to_string(&reply_env)
                .expect("server pack reply envelope should always serialize");
            if should_terminate_after_request_server_pack_send(emit_single_send(sink, room_id, reply).await) { return false; }
        }

        // Client uploads blobs ---------------------------------------------
        ClientDispatchCommand::BlobUpload => {
            let blobs = match msg["blobs"].as_array() {
                Some(a) => a.clone(),
                None => return true,
            };
            let mut stored = 0usize;
            let mut blob_store = room.blobs.write().await;
            for entry in &blobs {
                let hash_hex = extract_blob_upload_entry_hash_text(entry);
                let data_b64 = extract_blob_upload_entry_data_b64(entry);
                if let (Some(expected), Ok(bytes)) =
                    (parse_hex_hash(&hash_hex), base64_decode(&data_b64))
                {
                    let actual = nodalmerge_core::Hash::of(&bytes);
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
            if plan_blob_store_mutation(stored)
                == BlobStoreMutationAction::BroadcastBlobAvailable {
                let available: Vec<String> = blobs.iter()
                    .filter_map(|e| e["hash"].as_str())
                    .map(str::to_string)
                    .collect();
                let bcast = serde_json::to_string(&assemble_blob_available_envelope(available))
                    .expect("blob-available envelope should always serialize");
                emit_room_broadcast(room, bcast);
            }
        }

        // Client requests specific blobs -----------------------------------
        ClientDispatchCommand::BlobRequest => {
            let hashes = extract_blob_request_hashes(&msg);

            // F6: split into "redirect" and "fall-through" buckets. Only
            // peers that negotiated `supports_direct_blob_io` get redirects.
            let mut redirects: Vec<BlobRedirectEntry> = Vec::new();
            let mut fallthrough: Vec<String> = Vec::new();
            if negotiated_caps.supports_direct_blob_io {
                for h in &hashes {
                    let Some(hash) = parse_hex_hash(h) else { fallthrough.push(h.clone()); continue };
                    match room.persistence.resolve_get_url(&room.room_id, &hash, None) {
                        Some(p) => redirects.push(BlobRedirectEntry {
                            hash: h.clone(),
                            url: p.url,
                            expires_at_unix: p.expires_at_unix,
                        }),
                        None => fallthrough.push(h.clone()),
                    }
                }
            } else {
                fallthrough = hashes.clone();
            }
            if !redirects.is_empty() {
                let reply_env = assemble_blob_redirect_envelope(redirects);
                let reply = serde_json::to_string(&reply_env)
                    .expect("blob-redirect envelope should always serialize");
                if should_terminate_after_blob_request_redirect_reply_send(emit_single_send(sink, room_id, reply).await) { return false; }
            }

            // For everything not redirected, fall through to bytes-over-WS.
            let blob_store = room.blobs.read().await;
            let entries: Vec<BlobPackEntry> = fallthrough.iter().filter_map(|h| {
                let hash = parse_hex_hash(h)?;
                let data = blob_store.get(&hash)?;
                Some(BlobPackEntry {
                    hash: h.clone(),
                    data: base64_encode(&data),
                })
            }).collect();
            // Always reply including the originally requested hashes so the
            // client can clear its pending set even for unavailable blobs.
            let reply_env = assemble_blob_pack_envelope(entries, fallthrough);
            let reply = serde_json::to_string(&reply_env)
                .expect("blob-pack envelope should always serialize");
            if should_terminate_after_blob_request_blob_pack_reply_send(emit_single_send(sink, room_id, reply).await) { return false; }
        }

        // F6: client wants a presigned PUT URL ----------------------------
        // C→S: { type:"request-upload", hash, size }
        // S→C: { type:"upload-granted", hash, url, expires_at_unix }
        //   or { type:"upload-denied",  hash, reason }
        ClientDispatchCommand::RequestUpload => {
            if let Some(plan) = plan_direct_blob_io_negotiation_gate(
                negotiated_caps.supports_direct_blob_io,
                "request-upload",
            ) {
                send_error(sink, plan.error_message).await;
                return plan.keep_connection_open;
            }
            let hash_hex = extract_request_upload_hash_text(&msg);
            let size = extract_request_upload_size(&msg);
            let content_type = extract_request_upload_content_type(&msg);
            let hash_opt = parse_hex_hash(&hash_hex);
            if let Some(plan) = plan_direct_blob_io_bad_hash_gate(
                hash_opt.is_some(),
                "request-upload",
            ) {
                send_error(sink, plan.error_message).await;
                return plan.keep_connection_open;
            };
            let hash = hash_opt.expect("hash should be valid after bad-hash gate");
            let put_url = room
                .persistence
                .resolve_put_url(&room.room_id, &hash, size, content_type.as_deref());
            let reply = match classify_request_upload_resolve_put_url_outcome(put_url.is_some()) {
                RequestUploadResolvePutUrlOutcome::GrantUpload => {
                    let p = put_url.expect("put-url should be present when outcome grants upload");
                    serde_json::to_string(&assemble_upload_granted_envelope(
                    hash_hex.to_string(),
                    p.url,
                    p.expires_at_unix,
                )).expect("upload-granted envelope should always serialize")
                }
                RequestUploadResolvePutUrlOutcome::DenyUseWs => serde_json::to_string(&assemble_upload_denied_envelope(
                    hash_hex.to_string(),
                    "use-ws".to_string(),
                )).expect("upload-denied envelope should always serialize"),
            };
            if should_terminate_after_request_upload_reply_send(emit_single_send(sink, room_id, reply).await) { return false; }
        }

        // F6: client claims a presigned PUT completed --------------------
        // C→S: { type:"blob-uploaded", hash }
        // Server verifies via BlobPersistence::verify_uploaded (HEAD on S3)
        // and broadcasts blob-available so other peers can fetch it.
        ClientDispatchCommand::BlobUploaded => {
            if let Some(plan) = plan_direct_blob_io_negotiation_gate(
                negotiated_caps.supports_direct_blob_io,
                "blob-uploaded",
            ) {
                send_error(sink, plan.error_message).await;
                return plan.keep_connection_open;
            }
            let hash_hex = extract_blob_uploaded_hash_text(&msg);
            let hash_opt = parse_hex_hash(&hash_hex);
            if let Some(plan) = plan_direct_blob_io_bad_hash_gate(
                hash_opt.is_some(),
                "blob-uploaded",
            ) {
                send_error(sink, plan.error_message).await;
                return plan.keep_connection_open;
            };
            let hash = hash_opt.expect("hash should be valid after bad-hash gate");
            match room.persistence.verify_uploaded(&room.room_id, &hash) {
                Ok(()) => match classify_direct_blob_io_verify_outcome(true) {
                    DirectBlobIoVerifyOutcome::BroadcastAvailable => {
                    let available_hashes = shape_blob_uploaded_available_hashes(&hash_hex);
                    let bcast = serde_json::to_string(&assemble_blob_available_envelope(
                        available_hashes,
                    )).expect("blob-available envelope should always serialize");
                    emit_room_broadcast(room, bcast);
                    }
                    DirectBlobIoVerifyOutcome::SendUploadRejected => {}
                },
                Err(e) => match classify_direct_blob_io_verify_outcome(false) {
                    DirectBlobIoVerifyOutcome::BroadcastAvailable => {}
                    DirectBlobIoVerifyOutcome::SendUploadRejected => {
                    tracing::warn!(?e, hash = %hash_hex, "blob-uploaded verify failed");
                    let rejection_payload =
                        shape_blob_uploaded_verify_failure_rejection_payload(&hash_hex, &e.to_string());
                    let reply = serde_json::to_string(&assemble_upload_rejected_envelope(
                        rejection_payload.hash,
                        rejection_payload.reason,
                    )).expect("upload-rejected envelope should always serialize");
                    if should_terminate_after_blob_upload_verify_failure_reply_send(
                        emit_single_send(sink, room_id, reply).await,
                    ) { return false; }
                    }
                },
            }
        }

        // Ephemeral presence — forward, do not store -----------------------
        ClientDispatchCommand::Presence => {
            let bcast = serde_json::to_string(&assemble_presence_envelope(
                pubkey_hex.to_string(),
                extract_presence_data_payload(&msg),
            )).expect("presence envelope should always serialize");
            emit_room_broadcast(room, bcast);
        }

        // C3: lock the room with an Ed25519 verifying key -------------------
        // Only accepted if the room is currently open (auth_key is None).
        // Once set, the key cannot be changed without restarting the server.
        ClientDispatchCommand::SetRoomKey => {
            if !is_control_plane_allowed(is_server_peer, session_caps, "room.admin") {
                send_error(sink, "reject.control_plane_forbidden: command=set-room-key requires=room.admin").await;
                return true;
            }
            let vk_hex = extract_set_room_key_pubkey_text(&msg);
            let parsed_vk = parse_verifying_key(&vk_hex);
            match classify_set_room_key_parse_result(parsed_vk.is_some()) {
                SetRoomKeyParseResult::Parsed => {
                    let vk = parsed_vk.expect("verifying key should be present when parse succeeds");
                    let mut auth_key = room.auth_key.write().await;
                    match classify_set_room_key_lock_state(auth_key.is_none()) {
                        SetRoomKeyLockStateResult::LockRoom => {
                            *auth_key = Some(vk);
                            let reply = serde_json::to_string(&assemble_room_locked_envelope(
                                vk_hex.to_string(),
                            )).expect("room-locked envelope should always serialize");
                            if should_terminate_after_set_room_key_locked_ack_send(emit_single_send(sink, room_id, reply).await) { return false; }
                        }
                        SetRoomKeyLockStateResult::RejectAlreadyLocked => {
                            let reply = serde_json::to_string(&assemble_set_room_key_rejected_envelope(
                                shape_set_room_key_already_locked_rejection_reason().to_string(),
                            )).expect("set-room-key rejected envelope should always serialize");
                            let _ = sink.send(Message::Text(reply.into())).await;
                        }
                    }
                }
                SetRoomKeyParseResult::InvalidPubkeyHex => {
                    let reply = serde_json::to_string(&assemble_set_room_key_rejected_envelope(
                        shape_set_room_key_invalid_pubkey_rejection_reason().to_string(),
                    )).expect("set-room-key rejected envelope should always serialize");
                    let _ = sink.send(Message::Text(reply.into())).await;
                }
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
        ClientDispatchCommand::SetPolicy => {
            if !is_control_plane_allowed(is_server_peer, session_caps, "policy.admin") {
                send_error(sink, "reject.control_plane_forbidden: command=set-policy requires=policy.admin").await;
                return true;
            }
            let policy = parse_policy(&msg, sink).await;
            match classify_set_policy_parse_result(policy.is_some()) {
                SetPolicyParseResult::ApplyPolicy => {
                    let p = policy.expect("policy should be present when parse succeeds");
                    room.set_policy(p).await;
                    let reply = serde_json::to_string(&assemble_policy_set_envelope())
                        .expect("policy-set envelope should always serialize");
                    if should_terminate_after_set_policy_reply_send(emit_single_send(sink, room_id, reply).await) { return false; }
                }
                SetPolicyParseResult::ParseFailed => {} // parse_policy already sent the error
            }
        }

        // E1: client requests the server's public key -------------------------
        // Useful for building policies that reference the server as authority.
        ClientDispatchCommand::ServerInfo => {
            let vk_hex = hex_from_bytes(&server_key.verifying_key().to_bytes());
            let reply = serde_json::to_string(&assemble_server_info_envelope(vk_hex))
                .expect("server-info envelope should always serialize");
            if should_terminate_after_server_info_reply_send(emit_single_send(sink, room_id, reply).await) { return false; }
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
        ClientDispatchCommand::StartTick => {
            if !is_control_plane_allowed(is_server_peer, session_caps, "tick.admin") {
                send_error(sink, "reject.control_plane_forbidden: command=start-tick requires=tick.admin").await;
                return true;
            }
            let interval_ms = extract_start_tick_interval_ms(&msg);
            let intent_prefix = extract_start_tick_intent_prefix(&msg);
            let started = room.start_tick(Arc::clone(server_key), interval_ms, intent_prefix);
            let reply = serde_json::to_string(&assemble_tick_start_envelope(started, interval_ms))
                .expect("tick start envelope should always serialize");
            if should_terminate_after_start_tick_reply_send(emit_single_send(sink, room_id, reply).await) { return false; }
        }

        // E1: stop the tick loop for this room --------------------------------
        ClientDispatchCommand::StopTick => {
            if !is_control_plane_allowed(is_server_peer, session_caps, "tick.admin") {
                send_error(sink, "reject.control_plane_forbidden: command=stop-tick requires=tick.admin").await;
                return true;
            }
            room.stop_tick();
            let reply = serde_json::to_string(&assemble_tick_stopped_envelope())
                .expect("tick-stopped envelope should always serialize");
            if should_terminate_after_stop_tick_reply_send(emit_single_send(sink, room_id, reply).await) { return false; }
        }

        ClientDispatchCommand::ArchiveDescribe => {
            if let Err(rejection) =
                evaluate_control_plane_authorization(is_server_peer, session_caps, "archive.describe")
            {
                send_error(sink, &rejection).await;
                return true;
            }

            match process_archive_describe(room, room_id, msg).await {
                Ok(response) => {
                    if !emit_archive_response(sink, room_id, &response).await {
                        return false;
                    }
                }
                Err(rejected) => {
                    send_error(
                        sink,
                        &format!("{}: {}", rejected.reason_class.as_str(), rejected.reason_message),
                    )
                    .await;
                }
            }
        }

        ClientDispatchCommand::ArchiveValidate => {
            if let Err(rejection) =
                evaluate_control_plane_authorization(is_server_peer, session_caps, "archive.validate")
            {
                send_error(sink, &rejection).await;
                return true;
            }

            let response = process_archive_validate(room, room_id, msg).await;
            if !emit_archive_response(sink, room_id, &response).await {
                return false;
            }
        }

        ClientDispatchCommand::ArchiveImport => {
            if let Err(rejection) =
                evaluate_control_plane_authorization(is_server_peer, session_caps, "archive.import")
            {
                send_error(sink, &rejection).await;
                return true;
            }

            let response = process_archive_import(room, room_id, msg).await;
            if !emit_archive_response(sink, room_id, &response).await {
                return false;
            }
        }

        ClientDispatchCommand::ArchiveExport => {
            if let Err(rejection) =
                evaluate_control_plane_authorization(is_server_peer, session_caps, "archive.export")
            {
                send_error(sink, &rejection).await;
                return true;
            }

            let response = process_archive_export(room, room_id, msg, server_key.as_ref()).await;
            if !emit_archive_response(sink, room_id, &response).await {
                return false;
            }
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
        ClientDispatchCommand::CompactRoom => {
            // 1. Compact current graph into a snapshot node.
            let snap = {
                let graph = room.graph.read().await;
                let compact_result = compact(&*graph, server_key);
                match classify_compact_room_compaction_result(compact_result.is_ok()) {
                    CompactRoomCompactionResult::UseSnapshot => {
                        compact_result.expect("snapshot should be present when compaction succeeds")
                    }
                    CompactRoomCompactionResult::SendCompactFailed => {
                        let err = compact_result
                            .err()
                            .expect("compaction error should be present when compaction fails");
                        send_error(sink, &shape_compact_room_compaction_failed_error(&err.to_string())).await;
                        return true;
                    }
                }
            };

            // 2. Verify and extract metadata.
            let verify_result = verify_snapshot(&snap);
            let meta = match classify_compact_room_verify_result(verify_result.is_ok()) {
                CompactRoomVerifyResult::UseVerifiedSnapshot => {
                    verify_result.expect("verified snapshot metadata should be present when verify succeeds")
                }
                CompactRoomVerifyResult::SendVerifyFailed => {
                    let err = verify_result
                        .err()
                        .expect("verify error should be present when verify fails");
                    send_error(sink, &shape_compact_room_verify_failed_error(&err.to_string())).await;
                    return true;
                }
            };
            let hash_hex = meta.snapshot_hash.to_hex();

            // 3. Rebuild the room graph from the snapshot (prunes old nodes).
            let rebuild_result = rebuild_from_snapshot(snap.clone(), &[]);
            let rebuilt = match classify_compact_room_rebuild_result(rebuild_result.is_ok()) {
                CompactRoomRebuildResult::InstallRebuiltGraph => {
                    rebuild_result.expect("rebuilt graph should be present when rebuild succeeds")
                }
                CompactRoomRebuildResult::SendRebuildFailed => {
                    let err = rebuild_result
                        .err()
                        .expect("rebuild error should be present when rebuild fails");
                    send_error(sink, &shape_compact_room_rebuild_failed_error(&err.to_string())).await;
                    return true;
                }
            };
            *room.graph.write().await = rebuilt;

            // 4. Pack the snapshot node and broadcast to all peers.
            let pack_bytes = pack_snapshot_pack(&snap, &[]);
            let pack_b64 = base64_encode(&pack_bytes);
            let bcast = serde_json::to_string(&assemble_snapshot_pack_envelope(
                pack_b64,
                hash_hex.clone(),
            )).expect("snapshot-pack envelope should always serialize");
            emit_room_broadcast(room, bcast);

            // Confirm to the requesting client.
            let reply = serde_json::to_string(&assemble_compact_ack_envelope(hash_hex))
                .expect("compact-ack envelope should always serialize");
            if should_terminate_after_compact_room_ack_send(emit_single_send(sink, room_id, reply).await) { return false; }
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
        ClientDispatchCommand::Relay => {
            let relay_fields = extract_webrtc_relay_fields(&msg);
            let relay = serde_json::to_string(&assemble_normalized_peer_stamped_relay_envelope(
                pubkey_hex.to_string(),
                relay_fields,
            )).expect("webrtc relay envelope should always serialize");
            emit_room_broadcast(room, relay);
        }

        ClientDispatchCommand::Unknown => {
            if !should_ignore_unknown_client_message_type(&message_type) {
                return false;
            }
        }
    }

    true
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn send_error(sink: &mut SplitSink<WebSocket, Message>, msg: &str) {
    let j = serde_json::to_string(&assemble_error_envelope(msg.to_string()))
        .expect("error envelope should always serialize");
    let _ = sink.send(Message::Text(j.into())).await;
}

async fn emit_archive_response(
    sink: &mut SplitSink<WebSocket, Message>,
    room_id: &str,
    response: &ArchiveWsResponse,
) -> bool {
    match serialize_archive_ws_response(response) {
        Ok(payload) => emit_single_send(sink, room_id, payload).await,
        Err(_) => {
            send_error(sink, "archive adapter response serialization failed").await;
            true
        }
    }
}

#[inline]
fn is_control_plane_allowed(
    is_server_peer: bool,
    session_caps: &HashSet<String>,
    required_capability: &str,
) -> bool {
    is_server_peer || session_caps.contains(required_capability)
}

/// Canonical control-plane capability mapping used by the Rust server ingress.
pub fn required_capability_for_control_plane_command(command: &str) -> Option<&'static str> {
    match command {
        "set-policy" => Some("policy.admin"),
        "set-room-key" => Some("room.admin"),
        "start-tick" | "stop-tick" => Some("tick.admin"),
        "archive.describe" => Some("archive.read"),
        "archive.validate" | "archive.import" | "archive.export" => Some("archive.admin"),
        _ => None,
    }
}

/// Evaluate whether a control-plane command is allowed for a session.
///
/// Returns `Ok(())` if allowed. On deny, returns the deterministic rejection
/// message emitted by this ingress path.
pub fn evaluate_control_plane_authorization(
    is_server_peer: bool,
    session_caps: &HashSet<String>,
    command: &str,
) -> Result<(), String> {
    let Some(required) = required_capability_for_control_plane_command(command) else {
        return Ok(());
    };

    if is_control_plane_allowed(is_server_peer, session_caps, required) {
        Ok(())
    } else {
        Err(format!(
            "reject.control_plane_forbidden: command={command} requires={required}"
        ))
    }
}

/// Shared adapter emitter for single session-targeted text envelopes.
async fn emit_single_send(
    sink: &mut SplitSink<WebSocket, Message>,
    room_id: &str,
    text: String,
) -> bool {
    ws_send(sink, room_id, text).await
}

/// Shared adapter emitter for room fan-out envelopes.
fn emit_room_broadcast(room: &Arc<Room>, envelope: String) {
    let _ = room.tx.send(envelope);
}

/// Shared adapter emitter for session close frames.
async fn emit_close_frame(
    sink: &mut SplitSink<WebSocket, Message>,
    spec: CloseFrameSpec,
) {
    send_close_with_timeout(sink, spec).await;
}

// G1 — backpressure & slow-client policy.
//
// `ws_send` wraps every application-level outbound message in a 5-second
// timeout. A stalled TCP write (dead TLS terminator, kernel socket buffer
// full for a peer whose NIC is gone, etc.) would otherwise wedge the WS
// task until the OS times out — potentially minutes.
//
// On timeout we:
//   1. increment `nodalmerge_ws_send_timeout_total{room}` (plus legacy alias),
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
                "nodalmerge_ws_send_timeout_total",
                "room" => room_id.to_string(),
            ).increment(1);
            metrics::counter!(
                "nodalmerge_ws_send_timeout_total",
                "room" => room_id.to_string(),
            ).increment(1);
            tracing::warn!(room = %room_id, "ws send timed out — closing with 1011 server overload");
            emit_close_frame(sink, assemble_server_overload_close_frame()).await;
            false
        }
    }
}

fn close_message_from_spec(spec: CloseFrameSpec) -> Message {
    Message::Close(Some(CloseFrame {
        code: spec.code,
        reason: std::borrow::Cow::Owned(spec.reason),
    }))
}

async fn send_close_with_timeout(
    sink: &mut SplitSink<WebSocket, Message>,
    spec: CloseFrameSpec,
) {
    let _ = tokio::time::timeout(
        WS_CLOSE_FRAME_TIMEOUT,
        sink.send(close_message_from_spec(spec)),
    ).await;
}

// G3 — per-peer rate limiting.
//
// `check_peer_rate` charges `n` units against the supplied limiter. On
// success returns `Ok(())`. On either (a) `NotUntil` (rate exceeded) or
// (b) `InsufficientCapacity` (single request larger than the 1-second
// burst), the peer is closed with WS code `4008 rate limit exceeded`,
// `nodalmerge_rate_limit_drops_total{peer=<12-char hex>}` is incremented
// (plus legacy alias),
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
        "nodalmerge_rate_limit_drops_total",
        "peer" => peer_label.clone(),
    ).increment(1);
    metrics::counter!(
        "nodalmerge_rate_limit_drops_total",
        "peer" => peer_label.clone(),
    ).increment(1);
    tracing::warn!(
        peer = %peer_label,
        room = %room_id,
        dim,
        kind,
        "peer rate limit tripped — closing with 4008"
    );
    emit_close_frame(sink, assemble_rate_limit_exceeded_close_frame()).await;
}

/// Parse a `set-policy` JSON message into a `Policy`.
/// Sends an error and returns `None` on any parse failure.
async fn parse_policy(
    msg: &Value,
    sink: &mut SplitSink<WebSocket, Message>,
) -> Option<Policy> {
    let default_raw = extract_set_policy_default_text(msg);
    let default = match classify_set_policy_default(&default_raw) {
        SetPolicyDefaultParseResult::DenyAll => PolicyDefault::DenyAll,
        SetPolicyDefaultParseResult::AllowAll => PolicyDefault::AllowAll,
        SetPolicyDefaultParseResult::Unknown => {
            send_error(sink, &shape_set_policy_unknown_default_error(&default_raw)).await;
            return None;
        }
    };

    let mut rules = Vec::new();
    let rule_arr = extract_set_policy_rule_values(msg);
    for rule_val in &rule_arr {
        let path_glob = match rule_val["path_glob"].as_str() {
            Some(s) => s.to_string(),
            None => {
                send_error(sink, "set-policy: each rule must have a 'path_glob' string").await;
                return None;
            }
        };
        let mut keys = Vec::new();
        for v in &extract_set_policy_can_write_values(rule_val) {
            let hex = extract_set_policy_can_write_hex_text(v);
            match parse_hex_32(&hex) {
                Some(b) => keys.push(b),
                None => {
                    send_error(sink, &format!("set-policy: invalid pubkey hex '{hex}'")).await;
                    return None;
                }
            }
        }
        let can_write = keys;
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
struct VerifiedToken {
    expiry_secs: u64,
    capabilities: Vec<String>,
}

fn validate_identity_continuity_v1(tok: &Value, current_peer: &[u8; 32], now_secs: u64) -> Result<(), String> {
    let Some(continuity) = tok.get("continuity") else {
        return Ok(());
    };

    let predecessor_hex = continuity
        .get("predecessor_peer_pubkey")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing token.continuity.predecessor_peer_pubkey".to_string())?;
    let predecessor = parse_hex_32(predecessor_hex)
        .ok_or_else(|| "invalid token.continuity.predecessor_peer_pubkey".to_string())?;
    if &predecessor == current_peer {
        return Err("token continuity predecessor must differ from current peer".to_string());
    }

    let overlap_not_after = continuity
        .get("overlap_not_after")
        .and_then(Value::as_u64)
        .ok_or_else(|| "missing token.continuity.overlap_not_after".to_string())?;
    if now_secs > overlap_not_after {
        return Err("token continuity overlap window expired".to_string());
    }

    let revoked: HashSet<String> = continuity
        .get("revoked_predecessors")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(|s| s.trim().to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default();
    if revoked.contains(&predecessor_hex.trim().to_ascii_lowercase()) {
        return Err("token continuity predecessor is revoked".to_string());
    }

    Ok(())
}

static CAPABILITY_PROFILE_CACHE: OnceLock<Result<Option<CapabilityProfile>, String>> = OnceLock::new();

fn configured_capability_profile() -> Result<Option<&'static CapabilityProfile>, String> {
    let loaded = CAPABILITY_PROFILE_CACHE.get_or_init(|| {
        let path_raw = std::env::var_os("NODALMERGE_CAPABILITY_PROFILE_PATH")
            .or_else(|| std::env::var_os("ACTIVESYNC_CAPABILITY_PROFILE_PATH"));
        let Some(path_raw) = path_raw else {
            return Ok(None);
        };

        let path = PathBuf::from(path_raw);
        let profile = load_capability_profile_from_path(&path)
            .map_err(|e| format!("failed to load capability profile {}: {e}", path.display()))?;
        Ok(Some(profile))
    });

    match loaded {
        Ok(Some(profile)) => Ok(Some(profile)),
        Ok(None) => Ok(None),
        Err(err) => Err(err.clone()),
    }
}

fn maybe_expand_profile_capabilities(
    caps: &[String],
    token_profile_version: Option<&str>,
) -> Result<Vec<String>, String> {
    let Some(profile) = configured_capability_profile()? else {
        return Ok(caps.to_vec());
    };

    expand_profile_capabilities_with_profile(profile, caps, token_profile_version)
}

fn expand_profile_capabilities_with_profile(
    profile: &CapabilityProfile,
    caps: &[String],
    token_profile_version: Option<&str>,
) -> Result<Vec<String>, String> {

    let presented = token_profile_version
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "missing token.capability_profile_version while capability composition is enabled".to_string())?;

    if !profile_supports_version(profile, presented) {
        return Err(format!(
            "unknown token.capability_profile_version '{presented}' (expected '{}' or configured compatibility versions)",
            profile.profile_version
        ));
    }

    flatten_capabilities(profile, &caps.to_vec())
        .map_err(|e| format!("capability profile expansion failed: {e}"))
}

fn verify_hello_token(
    hello: &Value,
    room_vk: &VerifyingKey,
    peer_pubkey_hex: &str,
    room_id: &str,
) -> Result<VerifiedToken, String> {
    let tok = hello.get("token").ok_or("room is locked — include a capability token in hello")?;
    let peer_hex   = tok["peer_pubkey"].as_str().ok_or("missing token.peer_pubkey")?;
    let expiry     = tok["expiry"].as_u64().ok_or("missing token.expiry")?;
    let sig_hex    = tok["sig"].as_str().ok_or("missing token.sig")?;
    let token_profile_version = tok["capability_profile_version"]
        .as_str()
        .or_else(|| tok["profile_version"].as_str());
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
    validate_identity_continuity_v1(tok, &peer_bytes, now)?;
    let flattened_caps = maybe_expand_profile_capabilities(&token.capabilities, token_profile_version)?;

    Ok(VerifiedToken {
        expiry_secs: expiry,
        capabilities: flattened_caps,
    })
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

fn parse_hex_hash(hex: &str) -> Option<nodalmerge_core::Hash> {
    if hex.len() != 64 { return None; }
    let mut bytes = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let hi = hex_nibble(chunk[0])?;
        let lo = hex_nibble(chunk[1])?;
        bytes[i] = (hi << 4) | lo;
    }
    Some(nodalmerge_core::Hash(bytes))
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

    #[test]
    fn scoped_catchup_budget_drops_when_node_limit_exceeded() {
        let budget = ScopeCatchupBudget {
            max_filtered_node_count: 2,
            max_filtered_payload_bytes: 512,
        };

        assert!(should_drop_filtered_catchup_for_budget(3, 128, budget));
        assert!(!should_drop_filtered_catchup_for_budget(2, 128, budget));
    }

    #[test]
    fn scoped_catchup_budget_drops_when_payload_limit_exceeded() {
        let budget = ScopeCatchupBudget {
            max_filtered_node_count: 10,
            max_filtered_payload_bytes: 128,
        };

        assert!(should_drop_filtered_catchup_for_budget(3, 129, budget));
        assert!(!should_drop_filtered_catchup_for_budget(3, 128, budget));
    }
}

#[cfg(test)]
mod control_plane_auth_tests {
    use super::*;

    #[test]
    fn server_peer_always_allowed() {
        let caps = HashSet::new();
        assert!(is_control_plane_allowed(true, &caps, "policy.admin"));
    }

    #[test]
    fn matching_capability_allows_non_server_peer() {
        let mut caps = HashSet::new();
        caps.insert("tick.admin".to_string());
        assert!(is_control_plane_allowed(false, &caps, "tick.admin"));
    }

    #[test]
    fn missing_capability_denies_non_server_peer() {
        let mut caps = HashSet::new();
        caps.insert("policy.admin".to_string());
        assert!(!is_control_plane_allowed(false, &caps, "room.admin"));
    }

    #[test]
    fn archive_control_plane_capability_mapping_is_deterministic() {
        assert_eq!(
            required_capability_for_control_plane_command("archive.describe"),
            Some("archive.read")
        );
        assert_eq!(
            required_capability_for_control_plane_command("archive.validate"),
            Some("archive.admin")
        );
        assert_eq!(
            required_capability_for_control_plane_command("archive.import"),
            Some("archive.admin")
        );
        assert_eq!(
            required_capability_for_control_plane_command("archive.export"),
            Some("archive.admin")
        );
    }

    #[test]
    fn archive_validate_forbidden_message_uses_required_capability() {
        let caps = HashSet::new();
        let rejection = evaluate_control_plane_authorization(false, &caps, "archive.validate")
            .expect_err("missing archive.admin capability should reject");
        assert_eq!(
            rejection,
            "reject.control_plane_forbidden: command=archive.validate requires=archive.admin"
        );
    }

    #[test]
    fn archive_export_forbidden_message_uses_required_capability() {
        let caps = HashSet::new();
        let rejection = evaluate_control_plane_authorization(false, &caps, "archive.export")
            .expect_err("missing archive.admin capability should reject");
        assert_eq!(
            rejection,
            "reject.control_plane_forbidden: command=archive.export requires=archive.admin"
        );
    }
}

#[cfg(test)]
mod capability_profile_runtime_tests {
    use super::expand_profile_capabilities_with_profile;
    use crate::capability_profile::{CapabilityNode, CapabilityProfile};

    #[test]
    fn compatibility_window_accepts_supported_profile_version() {
        let profile = CapabilityProfile {
            profile_version: "capprof-v3".to_string(),
            supported_profile_versions: vec!["capprof-v2".to_string()],
            nodes: vec![CapabilityNode {
                capability: "policy.admin".to_string(),
                inherits: vec![],
            }],
            limits: None,
        };

        let caps = vec!["policy.admin".to_string()];
        let expanded =
            expand_profile_capabilities_with_profile(&profile, &caps, Some("capprof-v2"))
                .expect("expected version within compatibility window");
        assert_eq!(expanded, vec!["policy.admin".to_string()]);
    }

    #[test]
    fn compatibility_window_rejects_unsupported_profile_version() {
        let profile = CapabilityProfile {
            profile_version: "capprof-v3".to_string(),
            supported_profile_versions: vec!["capprof-v2".to_string()],
            nodes: vec![CapabilityNode {
                capability: "policy.admin".to_string(),
                inherits: vec![],
            }],
            limits: None,
        };

        let caps = vec!["policy.admin".to_string()];
        let err = expand_profile_capabilities_with_profile(&profile, &caps, Some("capprof-v1"))
            .expect_err("expected rejection for unsupported version");
        assert!(err.contains("unknown token.capability_profile_version"));
    }
}

#[cfg(test)]
mod identity_continuity_runtime_tests {
    use super::*;
    use serde_json::json;

    fn peer(hex_byte: u8) -> [u8; 32] {
        [hex_byte; 32]
    }

    fn peer_hex(hex_byte: u8) -> String {
        hex_from_bytes(&[hex_byte; 32])
    }

    #[test]
    fn continuity_validation_allows_predecessor_within_overlap() {
        let tok = json!({
            "continuity": {
                "predecessor_peer_pubkey": peer_hex(0x11),
                "overlap_not_after": 200,
                "revoked_predecessors": []
            }
        });

        let result = validate_identity_continuity_v1(&tok, &peer(0x22), 150);
        assert!(result.is_ok());
    }

    #[test]
    fn continuity_validation_rejects_expired_overlap() {
        let tok = json!({
            "continuity": {
                "predecessor_peer_pubkey": peer_hex(0x11),
                "overlap_not_after": 100,
                "revoked_predecessors": []
            }
        });

        let err = validate_identity_continuity_v1(&tok, &peer(0x22), 101)
            .expect_err("expected overlap expiry rejection");
        assert!(err.contains("overlap window expired"));
    }

    #[test]
    fn continuity_validation_rejects_revoked_predecessor() {
        let predecessor = peer_hex(0x11);
        let tok = json!({
            "continuity": {
                "predecessor_peer_pubkey": predecessor,
                "overlap_not_after": 200,
                "revoked_predecessors": [peer_hex(0x11)]
            }
        });

        let err = validate_identity_continuity_v1(&tok, &peer(0x22), 120)
            .expect_err("expected revoked predecessor rejection");
        assert!(err.contains("predecessor is revoked"));
    }
}
