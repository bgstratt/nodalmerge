//! Per-room shared state: a StateGraph, a BlobStore, and a broadcast channel.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use activesync_core::{MemoryBlobStore, BlobStore, Op, MapOp, Policy, StateGraph, SyncNode, pack_nodes};
use ed25519_dalek::{SigningKey, VerifyingKey};
use tokio::sync::{broadcast, RwLock};

use crate::store::SharedPersistence;

/// A JSON-encoded server→client push message.
pub type Envelope = String;

/// E1 (tick loop): a snapshot of intent key-values passed to the tick function.
/// `key` is the full intent path (e.g. `"intent/player1/move"`).
/// `value` is the raw bytes stored in the LWW map for that key.
pub struct IntentEntry {
    pub key: String,
    pub value: Vec<u8>,
}

/// E1 (tick loop): the result the tick function wants to write to the DAG.
/// `key` is a canonical path (e.g. `"world/player1/pos"`).
/// `value` is the raw bytes to store.
pub struct CanonicalWrite {
    pub key: String,
    pub value: Vec<u8>,
}

#[derive(Debug)]
pub struct Room {
    /// The server's own DAG for this room — identical in type to a client SyncStore.
    /// The server is a full peer: it merges incoming nodes, enforces policy,
    /// and can sign authoritative canonical nodes (E1).
    pub graph: RwLock<StateGraph>,
    pub blobs: RwLock<MemoryBlobStore>,
    /// Optional Ed25519 verifying key that locks this room (C3).
    /// `None` = open room (anyone may join).
    /// `Some(vk)` = locked; joining requires a valid capability token.
    pub auth_key: RwLock<Option<VerifyingKey>>,
    /// Broadcast channel for pushing merged packs to all peers in the room.
    pub tx: broadcast::Sender<Envelope>,
    /// E1: abort handle for the room's background tick loop, if running.
    pub tick_abort: Mutex<Option<tokio::task::AbortHandle>>,
    /// D2: set of currently-connected peer pubkeys (hex). Used to populate
    /// the `peers` field of the `welcome` message and to drive WebRTC initiation.
    pub connected_peers: RwLock<HashSet<String>>,
    /// F4: shared persistence handle. `NoPersistence` in the default in-memory build.
    pub persistence: SharedPersistence,
    /// F4: this room's id, passed to every `persistence.persist_*` call.
    pub room_id: String,
    /// Idle-eviction clock. `Some(t)` once `connected_peers` first empties,
    /// reset to `None` the moment any peer (re)connects. The background
    /// sweeper in [`Rooms::sweep_idle`] drops rooms whose idle timer has
    /// exceeded the configured timeout. `None` while any peer is connected.
    pub idle_since: Mutex<Option<Instant>>,
}

impl Room {
    pub fn new(room_id: String, persistence: SharedPersistence) -> Arc<Self> {
        let (tx, _) = broadcast::channel(512);
        // F4: hydrate graph + blobs from disk before the room becomes visible.
        let mut graph = StateGraph::new();
        let persisted_nodes = persistence.load_room_nodes(&room_id);
        if !persisted_nodes.is_empty() {
            let res = graph.apply_remote_batch(persisted_nodes);
            if !res.rejected.is_empty() {
                tracing::warn!(
                    room = %room_id,
                    rejected = res.rejected.len(),
                    "rejected nodes during hydrate (corrupt/tampered persisted row)"
                );
            }
        }
        let mut blobs = MemoryBlobStore::new();
        for (_h, bytes) in persistence.load_room_blobs(&room_id) {
            blobs.put(bytes);
        }
        Arc::new(Room {
            graph:           RwLock::new(graph),
            blobs:           RwLock::new(blobs),
            auth_key:        RwLock::new(None),
            tx,
            tick_abort:      Mutex::new(None),
            connected_peers: RwLock::new(HashSet::new()),
            persistence,
            room_id,
            // Brand-new room has no peers yet, so the idle clock starts now.
            // The first `register_peer` call will clear it.
            idle_since:      Mutex::new(Some(Instant::now())),
        })
    }

    /// Register a newly-connected peer. Clears the idle-eviction clock.
    pub async fn register_peer(&self, pubkey_hex: String) {
        self.connected_peers.write().await.insert(pubkey_hex);
        *self.idle_since.lock().expect("idle_since poisoned") = None;
    }

    /// Deregister a departing peer. If this was the last connected peer,
    /// starts the idle-eviction clock.
    pub async fn deregister_peer(&self, pubkey_hex: &str) {
        let mut peers = self.connected_peers.write().await;
        peers.remove(pubkey_hex);
        if peers.is_empty() {
            *self.idle_since.lock().expect("idle_since poisoned") = Some(Instant::now());
        }
    }

    /// Install a room-level write policy.  Called when a client sends
    /// `set-policy`.  Subsequent `apply_remote` calls will enforce it.
    pub async fn set_policy(&self, policy: Policy) {
        self.graph.write().await.set_policy(policy);
    }

    /// E1: Start the Authoritative tick loop for this room if not already running.
    ///
    /// Returns `true` on first start, `false` if a loop was already running.
    /// The loop runs at `interval_ms` ms per tick, reading intent keys under
    /// `intent_prefix` and emitting canonical state under `world/…`.
    pub fn start_tick(self: &Arc<Self>, server_key: Arc<SigningKey>, interval_ms: u64, intent_prefix: String) -> bool {
        let mut guard = self.tick_abort.lock().expect("tick_abort mutex poisoned");
        if guard.is_some() {
            return false;
        }
        let handle = spawn_tick_loop(Arc::clone(self), server_key, interval_ms, intent_prefix);
        *guard = Some(handle);
        true
    }

    /// E1: Stop the tick loop if it is running.
    pub fn stop_tick(&self) {
        if let Some(handle) = self.tick_abort.lock().expect("tick_abort mutex poisoned").take() {
            handle.abort();
        }
    }

    /// E1: Super-Peer tick loop — core of the Authoritative mode game loop.
    ///
    /// 1. Reads all current LWW values whose keys match `intent_prefix` from
    ///    the graph's speculative view (canonical from the server's perspective,
    ///    since the server never calls `apply_local` for intents).
    /// 2. Passes them to `tick_fn`, a pure synchronous function supplied by
    ///    the caller (the ws_handler task or a dedicated tick task).
    /// 3. Any `CanonicalWrite`s returned are committed with `apply_local` using
    ///    the server's signing key, creating authoritative nodes.
    /// 4. Returns a base64-encoded postcard pack of the new node(s) so the
    ///    caller can broadcast them to all connected peers.  Returns `None` if
    ///    `tick_fn` produced no writes (nothing to broadcast).
    ///
    /// The tick function signature is intentionally synchronous and pure — it
    /// must not do I/O.  Async game logic should compute results outside this
    /// call and pass them in via a channel.
    pub async fn process_tick(
        &self,
        server_key: &SigningKey,
        intent_prefix: &str,
        tick_fn: impl FnOnce(Vec<IntentEntry>) -> Vec<CanonicalWrite>,
    ) -> Option<String> {
        // 1. Collect intent entries under the given prefix.
        let intents: Vec<IntentEntry> = {
            let graph = self.graph.read().await;
            graph.resolve()
                .into_iter()
                .filter(|(k, _)| k.starts_with(intent_prefix))
                .map(|(key, value)| IntentEntry { key, value })
                .collect()
        };

        // 2. Run the tick function (pure, sync).
        let writes = tick_fn(intents);
        if writes.is_empty() {
            return None;
        }

        // 3. Commit canonical writes as a single signed node.
        let ops: Vec<Op> = writes.into_iter().map(|w| {
            Op::Map(MapOp::Set { key: w.key, value: w.value })
        }).collect();

        let node_id = {
            let mut graph = self.graph.write().await;
            match graph.apply_local(server_key, 0 /* wall_ms=0, lamport drives ordering */, ops) {
                Ok(id) => id,
                Err(_) => return None,
            }
        };

        // 4. Encode new node as a base64 postcard pack for broadcast.
        let b64 = {
            let graph = self.graph.read().await;
            let nodes: Vec<&SyncNode> = graph.get_nodes(&[node_id]);
            // F4: write-through persistence for the authoritative tick node.
            if let Some(n) = nodes.first() {
                self.persistence.persist_node(&self.room_id, n);
            }
            base64_encode(&pack_nodes(&nodes))
        };

        Some(b64)
    }
}

/// Global registry of rooms, lazily created on first connection.
///
/// Also carries the server's persistent Ed25519 keypair (E1) so every
/// handler can sign authoritative nodes.
#[derive(Clone)]
pub struct Rooms {
    rooms:  Arc<RwLock<HashMap<String, Arc<Room>>>>,
    /// Persistent server keypair generated/loaded at startup.
    pub server_key: Arc<SigningKey>,
    /// F4: shared persistence handle threaded into every newly-created room.
    pub persistence: SharedPersistence,
}

impl Rooms {
    pub fn new(server_key: SigningKey, persistence: SharedPersistence) -> Self {
        Rooms {
            rooms: Arc::new(RwLock::new(HashMap::new())),
            server_key: Arc::new(server_key),
            persistence,
        }
    }

    pub async fn get_or_create(&self, id: &str) -> Arc<Room> {
        {
            let r = self.rooms.read().await;
            if let Some(room) = r.get(id) {
                return Arc::clone(room);
            }
        }
        let mut w = self.rooms.write().await;
        w.entry(id.to_string())
            .or_insert_with(|| Room::new(id.to_string(), Arc::clone(&self.persistence)))
            .clone()
    }

    /// Evict rooms that have had no connected peers for longer than `timeout`.
    ///
    /// Only safe when `self.persistence.is_durable()` — otherwise dropping a
    /// `Room` loses its in-memory DAG. When the caller skips this guard,
    /// eviction becomes outright data loss.
    ///
    /// Returns the list of evicted room ids (for logging / tests).
    ///
    /// Eviction model: peer-count == 0 AND the idle clock has elapsed. We
    /// also require `Arc::strong_count == 1` (only the map holds the Arc) so
    /// a mid-handshake join that has cloned the Arc but not yet called
    /// `register_peer` isn't evicted out from under it. On eviction the
    /// room's tick loop is aborted; persisted nodes and blobs remain intact
    /// and are re-hydrated by the next `get_or_create` call.
    pub async fn sweep_idle(&self, timeout: Duration, now: Instant) -> Vec<String> {
        if !self.persistence.is_durable() {
            return Vec::new();
        }
        let mut evicted = Vec::new();
        let mut map = self.rooms.write().await;
        // Two-pass: first decide, then remove. `retain_async` doesn't exist,
        // and we want to release per-room locks before dropping the Arc.
        let candidate_ids: Vec<String> = {
            let mut out = Vec::new();
            for (id, room) in map.iter() {
                if Arc::strong_count(room) != 1 {
                    continue; // someone else is using it
                }
                let idle = *room.idle_since.lock().expect("idle_since poisoned");
                if let Some(since) = idle {
                    if now.duration_since(since) >= timeout {
                        // Double-check under the peers lock that nobody reconnected.
                        if room.connected_peers.read().await.is_empty() {
                            out.push(id.clone());
                        }
                    }
                }
            }
            out
        };
        for id in candidate_ids {
            if let Some(room) = map.remove(&id) {
                room.stop_tick();
                drop(room); // may or may not free; any surviving Arc (e.g. a
                            // stray broadcast subscriber) drops naturally.
                evicted.push(id);
            }
        }
        evicted
    }
}

/// Spawn the background idle-eviction sweeper.
///
/// Wakes every `interval` and calls [`Rooms::sweep_idle`]. `timeout == 0`
/// disables eviction (this function returns `None`). Callers store the
/// returned `JoinHandle` if they want to `abort()` on shutdown; the handle
/// can be dropped in normal operation — aborting the tokio runtime drops
/// the task.
pub fn spawn_idle_sweeper(
    rooms: Rooms,
    timeout: Duration,
    interval: Duration,
) -> Option<tokio::task::JoinHandle<()>> {
    if timeout.is_zero() {
        return None;
    }
    Some(tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Skip the immediate first tick (interval fires at t=0).
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let evicted = rooms.sweep_idle(timeout, Instant::now()).await;
            for id in evicted {
                tracing::info!(room = %id, "evicted idle room");
            }
        }
    }))
}

// ---------------------------------------------------------------------------
// E1: Tick loop helpers
// ---------------------------------------------------------------------------

/// Default tick function — pass-through compute.
///
/// For each intent key `"intent/foo/bar"`, emits a canonical write to
/// `"world/foo/bar"` with the same bytes.  Developers replace or augment this
/// by extending the `start-tick` protocol or sub-classing the tick task.
pub fn default_tick_fn(intents: Vec<IntentEntry>) -> Vec<CanonicalWrite> {
    intents
        .into_iter()
        .filter_map(|e| {
            let rest = e.key.strip_prefix("intent/")?;
            Some(CanonicalWrite { key: format!("world/{rest}"), value: e.value })
        })
        .collect()
}

/// Spawn a background tick task for `room`.
///
/// On each tick the task:
/// 1. Reads all LWW values whose keys start with `intent_prefix`.
/// 2. Calls `default_tick_fn` to produce canonical writes.
/// 3. Commits the writes as a signed node via `room.process_tick`.
/// 4. Broadcasts the resulting pack to all room peers.
///
/// The caller stores the returned `AbortHandle`; aborting it cleanly stops
/// the loop.  No clean-up is needed: the `Arc<Room>` ref is dropped when the
/// task finishes.
pub fn spawn_tick_loop(
    room: Arc<Room>,
    server_key: Arc<SigningKey>,
    interval_ms: u64,
    intent_prefix: String,
) -> tokio::task::AbortHandle {
    let task = tokio::spawn(async move {
        let period = Duration::from_millis(interval_ms.max(1));
        let mut ticker = tokio::time::interval(period);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let prefix = intent_prefix.clone();
            if let Some(pack_b64) = room.process_tick(&server_key, &prefix, default_tick_fn).await {
                let msg = serde_json::json!({
                    "type":  "pack",
                    "from":  "server-tick",
                    "nodes": pack_b64,
                })
                .to_string();
                let _ = room.tx.send(msg);
            }
        }
    });
    task.abort_handle()
}

/// Multi-pass topological import using batched signature verification.
///
/// Each pass calls `StateGraph::apply_remote_batch` once: dedupe + parallel
/// `verify_batch` happen inside core. Nodes rejected for `MissingParent` are
/// retried in the next pass (their parent may arrive later in the same batch
/// or in a follow-up pack); other rejections are recorded as errors and
/// surfaced to the caller.
pub async fn import_nodes(room: &Room, nodes: Vec<SyncNode>) -> (usize, Vec<String>) {
    let mut pending = nodes;
    let mut accepted = 0usize;
    let mut accepted_ids: Vec<activesync_core::NodeId> = Vec::new();
    let mut errors = Vec::new();
    let mut graph = room.graph.write().await;

    loop {
        if pending.is_empty() { break; }
        let before = pending.len();

        // Index pending by id so we can resurrect MissingParent rejects for
        // the next pass without re-cloning the originals up front.
        let mut by_id: std::collections::HashMap<activesync_core::NodeId, SyncNode> =
            pending.drain(..).map(|n| (n.id, n)).collect();
        let batch: Vec<SyncNode> = by_id.values().cloned().collect();

        let result = graph.apply_remote_batch(batch);
        accepted += result.accepted.len();
        accepted_ids.extend(result.accepted.iter().copied());

        let mut still_pending = Vec::new();
        for (id, err) in result.rejected {
            match err {
                activesync_core::SyncError::MissingParent(_) => {
                    if let Some(n) = by_id.remove(&id) {
                        still_pending.push(n);
                    }
                }
                activesync_core::SyncError::DuplicateNode(_) => {}
                e => { errors.push(e.to_string()); }
            }
        }

        pending = still_pending;
        if pending.len() == before { break; } // no progress
    }

    // Anything still in `pending` at this point is MissingParent — couldn't
    // resolve within this pack. Surface the count so the caller can log it;
    // a sustained non-zero here means the client isn't catching the server up.
    if !pending.is_empty() {
        errors.push(format!("missing_parent: {} node(s) unresolved", pending.len()));
    }

    // F4: write-through persistence for accepted nodes. Batched so the
    // SQLite backend fsyncs once per pack instead of once per node.
    if !accepted_ids.is_empty() {
        let nodes_ref: Vec<&SyncNode> = accepted_ids
            .iter()
            .filter_map(|id| graph.get_nodes(&[*id]).into_iter().next())
            .collect();
        if !nodes_ref.is_empty() {
            room.persistence.persist_nodes(&room.room_id, &nodes_ref);
        }
    }

    (accepted, errors)
}

// ---------------------------------------------------------------------------
// Internal base64 encoder (no external deps)
// ---------------------------------------------------------------------------

pub fn base64_encode(data: &[u8]) -> String {
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
