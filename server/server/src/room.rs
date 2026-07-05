//! Per-room shared state: a StateGraph, a BlobStore, and a broadcast channel.

use ed25519_dalek::{SigningKey, VerifyingKey};
use nodalmerge_core::{
    pack_nodes, BlobStore, MapOp, MemoryBlobStore, Op, Policy, PolicyTimelineEntry, RoomLineage,
    StateGraph, SyncNode,
};
use nodalmerge_host_core::engine::{
    plan_deregister_peer_membership, plan_register_peer_membership, PeerCountGaugeUpdate,
};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::AtomicUsize,
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::{broadcast, RwLock, Semaphore};

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

#[derive(Debug, Clone)]
pub struct QuerySpecState {
    pub query_spec_id: String,
    pub version: String,
    pub descriptor: Value,
}

#[derive(Debug, Clone)]
pub struct ProjectionState {
    pub projection_id: String,
    pub query_spec_id: String,
    pub checkpoint: Value,
    pub rows: Vec<Value>,
    pub digest: String,
    pub invalidated: bool,
    pub invalidation_reason: Option<String>,
}

#[derive(Debug)]
pub struct Room {
    /// The server's own DAG for this room — identical in type to a client SyncStore.
    /// The server is a full peer: it merges incoming nodes, enforces policy,
    /// and can sign authoritative canonical nodes (E1).
    pub graph: RwLock<StateGraph>,
    pub policy_timeline: RwLock<Vec<PolicyTimelineEntry>>,
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
    /// D2/FSE-05: active connection reference counts per peer pubkey.
    /// `connected_peers` contains exactly the keys whose count is > 0.
    pub connected_peer_sessions: RwLock<HashMap<String, usize>>,
    /// F4: shared persistence handle. `NoPersistence` in the default in-memory build.
    pub persistence: SharedPersistence,
    /// F4: this room's id, passed to every `persistence.persist_*` call.
    pub room_id: String,
    /// Idle-eviction clock. `Some(t)` once `connected_peers` first empties,
    /// reset to `None` the moment any peer (re)connects. The background
    /// sweeper in [`Rooms::sweep_idle`] drops rooms whose idle timer has
    /// exceeded the configured timeout. `None` while any peer is connected.
    pub idle_since: Mutex<Option<Instant>>,
    /// Wave 2 topology: set once for child work rooms; `None` for mainline rooms.
    pub lineage: RwLock<Option<RoomLineage>>,
    /// Query control-plane registered specs (in-memory runtime state).
    pub query_specs: RwLock<HashMap<String, QuerySpecState>>,
    /// Query control-plane built projections (in-memory runtime state).
    pub projections: RwLock<HashMap<String, ProjectionState>>,
    /// Fair FIFO admission for projection.build (FSE-03 slice 3).
    pub(crate) projection_build_semaphore: Mutex<Option<(usize, Arc<Semaphore>)>>,
    /// Waiters blocked on `projection_build_semaphore` (bounded by env max queue).
    pub(crate) projection_build_waiting: AtomicUsize,
}

impl Room {
    pub fn new(
        room_id: String,
        persistence: SharedPersistence,
        broadcast_capacity: usize,
    ) -> Arc<Self> {
        // G1: channel capacity plumbed from the CLI. Smaller = faster
        // divergence detection on slow clients; larger = more slack for
        // brief stalls. Zero is rejected at arg-parse time.
        let (tx, _) = broadcast::channel(broadcast_capacity);
        // F4: start with an empty in-memory graph and blob store. Persistence
        // hydration is performed asynchronously by the caller so we don't
        // perform blocking I/O during the WebSocket handshake.
        let graph = StateGraph::new();
        let policy_timeline = vec![PolicyTimelineEntry {
            effective_lamport: 0,
            policy: Policy::default(),
        }];
        let blobs = MemoryBlobStore::new();
        Arc::new(Room {
            graph: RwLock::new(graph),
            policy_timeline: RwLock::new(policy_timeline),
            blobs: RwLock::new(blobs),
            auth_key: RwLock::new(None),
            tx,
            tick_abort: Mutex::new(None),
            connected_peers: RwLock::new(HashSet::new()),
            connected_peer_sessions: RwLock::new(HashMap::new()),
            persistence,
            room_id,
            // Brand-new room has no peers yet, so the idle clock starts now.
            // The first `register_peer` call will clear it.
            idle_since: Mutex::new(Some(Instant::now())),
            lineage: RwLock::new(None),
            query_specs: RwLock::new(HashMap::new()),
            projections: RwLock::new(HashMap::new()),
            projection_build_semaphore: Mutex::new(None),
            projection_build_waiting: AtomicUsize::new(0),
        })
    }

    /// Register a newly-connected peer. Clears the idle-eviction clock.
    pub async fn register_peer(&self, pubkey_hex: String) -> PeerCountGaugeUpdate {
        let mut peer_sessions = self.connected_peer_sessions.write().await;
        let prior = peer_sessions.get(&pubkey_hex).copied().unwrap_or(0);
        peer_sessions.insert(pubkey_hex.clone(), prior.saturating_add(1));
        drop(peer_sessions);

        let inserted = if prior == 0 {
            self.connected_peers.write().await.insert(pubkey_hex)
        } else {
            false
        };
        let plan = plan_register_peer_membership(inserted);
        if plan.clear_idle_since {
            *self.idle_since.lock().expect("idle_since poisoned") = None;
        }
        if plan.gauge_update == PeerCountGaugeUpdate::Increment {
            metrics::gauge!("nodalmerge_peers_total", "room" => self.room_id.clone())
                .increment(1.0);
            metrics::gauge!("nodalmerge_peers_total", "room" => self.room_id.clone())
                .increment(1.0);
        }
        plan.gauge_update
    }

    /// Deregister a departing peer. If this was the last connected peer,
    /// starts the idle-eviction clock.
    pub async fn deregister_peer(&self, pubkey_hex: &str) -> PeerCountGaugeUpdate {
        let mut peer_sessions = self.connected_peer_sessions.write().await;
        let mut removed = false;
        match peer_sessions.get(pubkey_hex).copied() {
            Some(1) => {
                peer_sessions.remove(pubkey_hex);
                removed = true;
            }
            Some(n) if n > 1 => {
                peer_sessions.insert(pubkey_hex.to_string(), n - 1);
            }
            _ => {}
        }
        drop(peer_sessions);

        let mut peers = self.connected_peers.write().await;
        if removed {
            peers.remove(pubkey_hex);
        }
        let plan = plan_deregister_peer_membership(removed, peers.is_empty());
        if plan.set_idle_since_now {
            *self.idle_since.lock().expect("idle_since poisoned") = Some(Instant::now());
        }
        if plan.gauge_update == PeerCountGaugeUpdate::Decrement {
            metrics::gauge!("nodalmerge_peers_total", "room" => self.room_id.clone())
                .decrement(1.0);
            metrics::gauge!("nodalmerge_peers_total", "room" => self.room_id.clone())
                .decrement(1.0);
        }
        plan.gauge_update
    }

    /// Install a room-level write policy.  Called when a client sends
    /// `set-policy`.  Subsequent `apply_remote` calls will enforce it.
    pub async fn set_policy(&self, policy: Policy) {
        {
            let mut timeline = self.policy_timeline.write().await;
            let next_cutover = timeline
                .last()
                .map(|entry| entry.effective_lamport.saturating_add(1))
                .unwrap_or(0);
            timeline.push(PolicyTimelineEntry {
                effective_lamport: next_cutover,
                policy: policy.clone(),
            });
        }
        self.graph.write().await.set_policy(policy);
    }

    pub async fn current_policy_timeline(&self) -> Vec<PolicyTimelineEntry> {
        self.policy_timeline.read().await.clone()
    }

    /// E1: Start the Authoritative tick loop for this room if not already running.
    ///
    /// Returns `true` on first start, `false` if a loop was already running.
    /// The loop runs at `interval_ms` ms per tick, reading intent keys under
    /// `intent_prefix` and emitting canonical state under `world/…`.
    pub fn start_tick(
        self: &Arc<Self>,
        server_key: Arc<SigningKey>,
        interval_ms: u64,
        intent_prefix: String,
    ) -> bool {
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
        if let Some(handle) = self
            .tick_abort
            .lock()
            .expect("tick_abort mutex poisoned")
            .take()
        {
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
            graph
                .resolve()
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
        let ops: Vec<Op> = writes
            .into_iter()
            .map(|w| {
                Op::Map(MapOp::Set {
                    key: w.key,
                    value: w.value,
                })
            })
            .collect();

        let node_id = {
            let mut graph = self.graph.write().await;
            match graph.apply_local(
                server_key, 0, /* wall_ms=0, lamport drives ordering */
                ops,
            ) {
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
    pub(crate) rooms: Arc<RwLock<HashMap<String, Arc<Room>>>>,
    /// Parent room id → child room ids (topology list-children).
    pub(crate) children_index: Arc<RwLock<HashMap<String, Vec<String>>>>,
    /// Bounded topology promotion fair-queue semaphore.
    pub(crate) promotion_semaphore: Arc<Mutex<Option<(usize, Arc<Semaphore>)>>>,
    /// Waiters currently queued for topology promotion operations.
    pub(crate) promotion_waiting: Arc<AtomicUsize>,
    /// Max concurrent topology promotion operations.
    pub(crate) promotion_max_inflight: usize,
    /// Max queued topology promotion waiters before rejection.
    pub(crate) promotion_max_queue: usize,
    /// Optional retention cap per parent for in-memory children index.
    pub(crate) lineage_children_index_cap: Option<usize>,
    /// Room lineage metadata — volatile or SQLite-backed durability.
    pub(crate) lineage_store: Arc<crate::lineage_store::LineageStoreHandle>,
    /// Promotion proposals — volatile or SQLite-backed (topology Phase C / Wave 3).
    pub(crate) promotion_store: Arc<crate::promotion_store::PromotionStoreHandle>,
    /// Persistent server keypair generated/loaded at startup.
    pub server_key: Arc<SigningKey>,
    /// F4: shared persistence handle threaded into every newly-created room.
    pub persistence: SharedPersistence,
    /// G1: per-room broadcast channel capacity. Threaded into `Room::new`.
    pub broadcast_capacity: usize,
    /// G3: per-peer rate limit on accepted nodes per second. `0` disables
    /// node-count limiting. Read by the WS handler at session start to
    /// build a per-peer [`governor`] direct rate limiter.
    pub peer_rate_nodes: u32,
    /// G3: per-peer rate limit on accepted bytes per second (raw decoded
    /// pack bytes, not wire JSON). `0` disables byte-rate limiting.
    pub peer_rate_bytes: u32,
}

impl Rooms {
    pub fn new(
        server_key: SigningKey,
        persistence: SharedPersistence,
        broadcast_capacity: usize,
        peer_rate_nodes: u32,
        peer_rate_bytes: u32,
    ) -> Self {
        let promotion_queue_concurrency =
            parse_env_usize("NODALMERGE_TOPOLOGY_PROMOTION_MAX_INFLIGHT", 4);
        let promotion_queue_max = parse_env_usize("NODALMERGE_TOPOLOGY_PROMOTION_MAX_QUEUE", 32);
        let lineage_children_index_cap =
            parse_optional_env_usize("NODALMERGE_LINEAGE_CHILDREN_INDEX_MAX");
        Self::new_with_topology_limits(
            server_key,
            persistence,
            broadcast_capacity,
            peer_rate_nodes,
            peer_rate_bytes,
            promotion_queue_concurrency,
            promotion_queue_max,
            lineage_children_index_cap,
        )
    }

    pub fn new_with_topology_limits(
        server_key: SigningKey,
        persistence: SharedPersistence,
        broadcast_capacity: usize,
        peer_rate_nodes: u32,
        peer_rate_bytes: u32,
        promotion_queue_concurrency: usize,
        promotion_queue_max: usize,
        lineage_children_index_cap: Option<usize>,
    ) -> Self {
        let store_root = crate::store::topology_store_root(&persistence);
        let lineage_store = Arc::new(crate::lineage_store::LineageStoreHandle::open(
            store_root.clone(),
        ));
        let promotion_store = Arc::new(crate::promotion_store::PromotionStoreHandle::open(
            store_root,
        ));
        Rooms {
            rooms: Arc::new(RwLock::new(HashMap::new())),
            children_index: Arc::new(RwLock::new(HashMap::new())),
            promotion_semaphore: Arc::new(Mutex::new(None)),
            promotion_waiting: Arc::new(AtomicUsize::new(0)),
            promotion_max_inflight: promotion_queue_concurrency,
            promotion_max_queue: promotion_queue_max,
            lineage_children_index_cap,
            lineage_store,
            promotion_store,
            server_key: Arc::new(server_key),
            persistence,
            broadcast_capacity,
            peer_rate_nodes,
            peer_rate_bytes,
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
        let mut created = false;
        let room = w
            .entry(id.to_string())
            .or_insert_with(|| {
                created = true;
                Room::new(
                    id.to_string(),
                    Arc::clone(&self.persistence),
                    self.broadcast_capacity,
                )
            })
            .clone();
        if created {
            metrics::gauge!("nodalmerge_rooms_total").increment(1.0);
            metrics::gauge!("nodalmerge_rooms_total").increment(1.0);
            // Spawn background hydration so the WebSocket handshake doesn't
            // block on potentially slow persistence I/O (e.g. Mongo).
            let room_clone = Arc::clone(&room);
            let persistence = Arc::clone(&self.persistence);
            let lineage_store = Arc::clone(&self.lineage_store);
            let children_index = Arc::clone(&self.children_index);
            tokio::spawn(async move {
                if persistence.is_durable() {
                    let hydrate_start = Instant::now();
                    tracing::info!(room = %room_clone.room_id, "async persistence hydrate started");

                    // Load nodes and apply to the in-memory graph.
                    let load_nodes_start = Instant::now();
                    let nodes = persistence.load_room_nodes(&room_clone.room_id);
                    let load_nodes_elapsed = load_nodes_start.elapsed();
                    tracing::info!(
                        room = %room_clone.room_id,
                        loaded_nodes = nodes.len(),
                        elapsed_ms = load_nodes_elapsed.as_millis(),
                        "async persistence hydrate stage: load_room_nodes"
                    );

                    if !nodes.is_empty() {
                        let apply_nodes_start = Instant::now();
                        let res = room_clone.graph.write().await.apply_remote_batch(nodes);
                        let apply_nodes_elapsed = apply_nodes_start.elapsed();
                        tracing::info!(
                            room = %room_clone.room_id,
                            accepted = res.accepted.len(),
                            rejected = res.rejected.len(),
                            elapsed_ms = apply_nodes_elapsed.as_millis(),
                            "async persistence hydrate stage: apply_remote_batch"
                        );
                        if !res.rejected.is_empty() {
                            tracing::warn!(room = %room_clone.room_id, rejected = res.rejected.len(), "rejected nodes during async hydrate");
                        }
                    }

                    // Load blobs and insert into memory blob store.
                    let load_blobs_start = Instant::now();
                    let blobs = persistence.load_room_blobs(&room_clone.room_id);
                    let load_blobs_elapsed = load_blobs_start.elapsed();
                    tracing::info!(
                        room = %room_clone.room_id,
                        loaded_blobs = blobs.len(),
                        elapsed_ms = load_blobs_elapsed.as_millis(),
                        "async persistence hydrate stage: load_room_blobs"
                    );

                    if !blobs.is_empty() {
                        let apply_blobs_start = Instant::now();
                        let mut store = room_clone.blobs.write().await;
                        for (_h, bytes) in blobs {
                            store.put(bytes);
                        }
                        let apply_blobs_elapsed = apply_blobs_start.elapsed();
                        tracing::info!(
                            room = %room_clone.room_id,
                            elapsed_ms = apply_blobs_elapsed.as_millis(),
                            "async persistence hydrate stage: apply_blobs"
                        );
                    }

                    // Load topology lineage metadata for this room and rebuild
                    // parent→child index entries opportunistically on demand.
                    if let Some(lineage) = lineage_store.get_lineage(&room_clone.room_id).await {
                        *room_clone.lineage.write().await = Some(lineage.clone());
                        let mut index = children_index.write().await;
                        let children = index.entry(lineage.parent_room_id).or_default();
                        if !children.iter().any(|id| id == &room_clone.room_id) {
                            children.push(room_clone.room_id.clone());
                        }
                    }
                    tracing::info!(
                        room = %room_clone.room_id,
                        total_elapsed_ms = hydrate_start.elapsed().as_millis(),
                        "async persistence hydrate completed"
                    );
                }
            });
        }
        room
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
                metrics::counter!("nodalmerge_eviction_total").increment(1);
                metrics::counter!("nodalmerge_eviction_total").increment(1);
                metrics::gauge!("nodalmerge_rooms_total").decrement(1.0);
                metrics::gauge!("nodalmerge_rooms_total").decrement(1.0);
            }
        }
        evicted
    }

    /// G4 — blob garbage collection across every currently-loaded room.
    ///
    /// For each live room this computes the set of blob hashes referenced
    /// by any `Op::Map::SetBlob` op across the *entire* DAG (not just
    /// `resolve()` output, because older SetBlob ops still need to carry
    /// their blob to catching-up peers) and calls
    /// [`ServerPersistence::blob_gc_sweep`] with the caller's grace
    /// window. Aggregates the total number of deleted blobs for logging
    /// and bumps `nodalmerge_blob_gc_deleted_total{room}` (plus legacy alias) per room.
    ///
    /// Rooms that are persisted on disk but not currently loaded are
    /// **not** swept — they will be covered the next time they hydrate
    /// and the sweeper runs afterwards. This keeps the lock surface
    /// minimal and matches the "only GC what's hot" operational model.
    ///
    /// No-op on non-durable backends.
    pub async fn sweep_blobs(&self, grace: Duration) -> usize {
        if !self.persistence.is_durable() {
            return 0;
        }
        // Snapshot the room list so we don't hold the outer lock while
        // reading per-room graphs.
        let rooms: Vec<(String, Arc<Room>)> = {
            let map = self.rooms.read().await;
            map.iter()
                .map(|(k, v)| (k.clone(), Arc::clone(v)))
                .collect()
        };
        let mut total = 0;
        for (id, room) in rooms {
            let live = collect_live_blob_hashes(&room).await;

            // PR-05 compatibility bridge: run shared GC coordinator in
            // MarkOnly mode to exercise host-neutral contracts without
            // changing deletion behavior. Legacy blob_gc_sweep remains the
            // source of physical delete behavior for now.
            match crate::gc_adapter::run_mark_only_preflight(&id, &live) {
                Ok(delta) => {
                    metrics::counter!("nodalmerge_gc_runs_total", "mode" => "mark-only", "status" => "ok")
                        .increment(1);
                    metrics::counter!("nodalmerge_gc_runs_total", "mode" => "mark-only", "status" => "ok")
                        .increment(1);
                    metrics::counter!("nodalmerge_gc_marked_total", "room" => id.clone())
                        .increment(delta.marked_count);
                    metrics::counter!("nodalmerge_gc_marked_total", "room" => id.clone())
                        .increment(delta.marked_count);
                }
                Err(e) => {
                    metrics::counter!("nodalmerge_gc_runs_total", "mode" => "mark-only", "status" => "error")
                        .increment(1);
                    metrics::counter!("nodalmerge_gc_runs_total", "mode" => "mark-only", "status" => "error")
                        .increment(1);
                    metrics::counter!("nodalmerge_gc_error_total", "room" => id.clone())
                        .increment(1);
                    metrics::counter!("nodalmerge_gc_error_total", "room" => id.clone())
                        .increment(1);
                    tracing::warn!(room = %id, ?e, "GC mark-only preflight failed; continuing with legacy sweep");
                }
            }

            let deleted = self.persistence.blob_gc_sweep(&id, &live, grace);
            if deleted > 0 {
                metrics::counter!("nodalmerge_blob_gc_deleted_total", "room" => id.clone())
                    .increment(deleted as u64);
                metrics::counter!("nodalmerge_blob_gc_deleted_total", "room" => id.clone())
                    .increment(deleted as u64);
                tracing::info!(room = %id, deleted, "blob GC reclaimed blobs");
            }
            total += deleted;
        }
        total
    }
}

fn parse_env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .unwrap_or(default)
}

fn parse_optional_env_usize(name: &str) -> Option<usize> {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|v| *v > 0)
}

/// G4 helper — union of every `SetBlob.blob_hash` across all nodes in the
/// room's DAG. Called with a read lock on the graph; does no I/O.
async fn collect_live_blob_hashes(
    room: &Arc<Room>,
) -> std::collections::HashSet<nodalmerge_core::Hash> {
    use nodalmerge_core::{MapOp, Op};
    let mut live = std::collections::HashSet::new();
    let graph = room.graph.read().await;
    let ids = graph.all_node_ids();
    for node in graph.get_nodes(&ids) {
        for op in &node.transaction.ops {
            if let Op::Map(MapOp::SetBlob { blob_hash, .. }) = op {
                live.insert(*blob_hash);
            }
        }
    }
    live
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

/// G4 — spawn the background blob GC sweeper.
///
/// Wakes every `interval` and calls [`Rooms::sweep_blobs`] with the
/// configured `grace` window. `interval.is_zero()` disables GC (this
/// function returns `None`); `grace == ZERO` collapses the two-phase
/// tombstone protocol into a single-pass delete.
///
/// Callers can drop the returned `JoinHandle` — aborting the tokio
/// runtime drops the task.
pub fn spawn_blob_gc_sweeper(
    rooms: Rooms,
    interval: Duration,
    grace: Duration,
) -> Option<tokio::task::JoinHandle<()>> {
    if interval.is_zero() {
        return None;
    }
    Some(tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await; // skip t=0
        loop {
            ticker.tick().await;
            let _deleted = rooms.sweep_blobs(grace).await;
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
            Some(CanonicalWrite {
                key: format!("world/{rest}"),
                value: e.value,
            })
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
            if let Some(pack_b64) = room
                .process_tick(&server_key, &prefix, default_tick_fn)
                .await
            {
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
pub async fn import_nodes(
    room: &Room,
    nodes: Vec<SyncNode>,
) -> (usize, Vec<nodalmerge_core::NodeId>, Vec<String>) {
    let t0 = Instant::now();
    let mut pending = nodes;
    let mut accepted = 0usize;
    let mut accepted_ids: Vec<nodalmerge_core::NodeId> = Vec::new();
    let mut errors = Vec::new();
    let mut graph = room.graph.write().await;

    // G5: server-side wall-clock anchor. Passed to core so nodes whose
    // `wall_ms` is more than `WALL_SKEW_MAX_MS` past our clock are
    // rejected with `SyncError::WallClockSkew`. Computed once per
    // `import_nodes` call — cheap and good enough (a 24 h window
    // doesn't care about sub-second drift).
    let now_ms: u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    loop {
        if pending.is_empty() {
            break;
        }
        let before = pending.len();

        // Index pending by id so we can resurrect MissingParent rejects for
        // the next pass without re-cloning the originals up front. Keep the
        // first-seen id order from `pending` so parent-before-child ordering
        // in caller-provided batches is preserved for core's in-order apply.
        let mut by_id: std::collections::HashMap<nodalmerge_core::NodeId, SyncNode> =
            std::collections::HashMap::with_capacity(before);
        let mut order: Vec<nodalmerge_core::NodeId> = Vec::with_capacity(before);
        for n in pending.drain(..) {
            order.push(n.id);
            by_id.insert(n.id, n);
        }
        let mut seen: std::collections::HashSet<nodalmerge_core::NodeId> =
            std::collections::HashSet::with_capacity(order.len());
        let batch: Vec<SyncNode> = order
            .into_iter()
            .filter(|id| seen.insert(*id))
            .filter_map(|id| by_id.get(&id).cloned())
            .collect();

        let result = graph.apply_remote_batch_checked(batch, Some(now_ms));
        accepted += result.accepted.len();
        accepted_ids.extend(result.accepted.iter().copied());

        let mut still_pending = Vec::new();
        for (id, err) in result.rejected {
            match err {
                nodalmerge_core::SyncError::MissingParent(_) => {
                    if let Some(n) = by_id.remove(&id) {
                        still_pending.push(n);
                    }
                }
                nodalmerge_core::SyncError::DuplicateNode(_) => {}
                // G5: bucket sanity-check rejects into a labeled counter
                // so operators can spot misbehaving clients (or their own
                // clock drift) without scraping logs.
                e @ nodalmerge_core::SyncError::LamportCeiling { .. } => {
                    metrics::counter!(
                        "nodalmerge_lamport_rejected_total",
                        "reason" => "ceiling"
                    )
                    .increment(1);
                    metrics::counter!(
                        "nodalmerge_lamport_rejected_total",
                        "reason" => "ceiling"
                    )
                    .increment(1);
                    errors.push(e.to_string());
                }
                e @ nodalmerge_core::SyncError::WallClockSkew { .. } => {
                    metrics::counter!(
                        "nodalmerge_lamport_rejected_total",
                        "reason" => "wall_skew"
                    )
                    .increment(1);
                    metrics::counter!(
                        "nodalmerge_lamport_rejected_total",
                        "reason" => "wall_skew"
                    )
                    .increment(1);
                    errors.push(e.to_string());
                }
                e => {
                    errors.push(e.to_string());
                }
            }
        }

        pending = still_pending;
        if pending.len() == before {
            break;
        } // no progress
    }

    // Anything still in `pending` at this point is MissingParent — couldn't
    // resolve within this pack. Surface the count so the caller can log it;
    // a sustained non-zero here means the client isn't catching the server up.
    if !pending.is_empty() {
        errors.push(format!(
            "missing_parent: {} node(s) unresolved",
            pending.len()
        ));
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

    metrics::histogram!("nodalmerge_merge_batch_seconds").record(t0.elapsed().as_secs_f64());
    metrics::histogram!("nodalmerge_merge_batch_seconds").record(t0.elapsed().as_secs_f64());
    if accepted > 0 {
        metrics::counter!("nodalmerge_nodes_accepted_total", "room" => room.room_id.clone())
            .increment(accepted as u64);
        metrics::counter!("nodalmerge_nodes_accepted_total", "room" => room.room_id.clone())
            .increment(accepted as u64);
    }
    // G11 — observability-only resident-bytes gauge. Approximate: flat
    // 512 B per node header/transaction + exact blob resident bytes.
    // Cheap to compute (one iteration over blob hashes) and only runs
    // after each accepted pack, so it cannot outrun the WS hot path.
    {
        let node_count = graph.node_count() as u64;
        drop(graph);
        let blob_bytes: u64 = {
            let blobs = room.blobs.read().await;
            let mut total: u64 = 0;
            for h in blobs.hashes() {
                if let Some(sz) = blobs.size(h) {
                    total += sz;
                }
            }
            total
        };
        const NODE_EST_BYTES: u64 = 512;
        let resident = node_count
            .saturating_mul(NODE_EST_BYTES)
            .saturating_add(blob_bytes);
        metrics::gauge!(
            "nodalmerge_room_bytes_resident",
            "room" => room.room_id.clone()
        )
        .set(resident as f64);
        metrics::gauge!(
            "nodalmerge_room_bytes_resident",
            "room" => room.room_id.clone()
        )
        .set(resident as f64);
    }

    (accepted, accepted_ids, errors)
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
        if chunk.len() > 1 {
            out.push(CHARS[((b1 & 0xf) << 2) | (b2 >> 6)] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(CHARS[b2 & 0x3f] as char);
        } else {
            out.push('=');
        }
    }
    out
}

// ---------------------------------------------------------------------------
// E4: Snapshot sweeper
// ---------------------------------------------------------------------------

/// E4 — Spawn the background snapshot sweeper.
///
/// Wakes every `check_interval` and checks each live room.  Once a room
/// accumulates `node_interval` or more new nodes since the last snapshot, the
/// server compacts the room's graph and broadcasts the resulting snapshot node
/// to all connected peers.
///
/// Incremental vs. full snapshots:
/// - The first snapshot is always a *full* snapshot (no chain pointer).
/// - Each subsequent snapshot within a chain is *incremental* (carries
///   `\x00snap:base` pointing to the previous snapshot's node ID).
/// - Once the chain reaches `max_chain_depth`, the sweeper resets to a full
///   snapshot to prevent unbounded chain growth.
///
/// `node_interval == 0` disables snapshotting (returns `None`).
/// Callers may drop the returned `JoinHandle` — the runtime drops the task.
pub fn spawn_snapshot_sweeper(
    rooms: Rooms,
    server_key: std::sync::Arc<ed25519_dalek::SigningKey>,
    node_interval: usize,
    max_chain_depth: usize,
    check_interval: Duration,
) -> Option<tokio::task::JoinHandle<()>> {
    use nodalmerge_core::compaction::{compact, compact_incremental, pack_snapshot_pack};

    if node_interval == 0 {
        return None;
    }

    Some(tokio::spawn(async move {
        // Per-room snapshot state: (last_node_count, chain_depth, last_snapshot_id).
        let mut room_state: HashMap<String, (usize, usize, Option<nodalmerge_core::NodeId>)> =
            HashMap::new();

        let mut ticker = tokio::time::interval(check_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await; // skip t=0

        loop {
            ticker.tick().await;

            // Snapshot the room list so we don't hold the outer lock during compaction.
            let room_list: Vec<(String, Arc<Room>)> = {
                let map = rooms.rooms.read().await;
                map.iter()
                    .map(|(k, v)| (k.clone(), Arc::clone(v)))
                    .collect()
            };

            for (room_id, room) in room_list {
                let graph = room.graph.read().await;
                let current_count = graph.node_count();
                drop(graph);

                let (last_count, chain_depth, last_snap_id) = room_state
                    .entry(room_id.clone())
                    .or_insert((current_count, 0, None));

                let delta = current_count.saturating_sub(*last_count);
                if delta < node_interval {
                    continue; // not enough new nodes yet
                }

                // Time to snapshot. Choose full vs. incremental.
                let graph = room.graph.read().await;
                let snap_result = if *chain_depth >= max_chain_depth || last_snap_id.is_none() {
                    // Full snapshot: resets the chain.
                    compact(&graph, &server_key)
                } else {
                    // Incremental snapshot: chains from previous.
                    compact_incremental(&graph, &server_key, last_snap_id.unwrap())
                };
                drop(graph);

                let snap_node = match snap_result {
                    Ok(n) => n,
                    Err(e) => {
                        tracing::warn!(room = %room_id, ?e, "snapshot sweeper: compact failed");
                        continue;
                    }
                };

                let snap_id = snap_node.id;
                let is_full = last_snap_id.is_none() || *chain_depth >= max_chain_depth;
                let kind = if is_full { "full" } else { "incremental" };

                // Insert the snapshot node into the room's graph.
                {
                    let mut graph = room.graph.write().await;
                    if let Err(e) = graph.apply_remote(snap_node.clone()) {
                        tracing::warn!(room = %room_id, ?e, "snapshot sweeper: apply_remote failed");
                        continue;
                    }
                }

                // Persist the snapshot node if durable.
                room.persistence.persist_node(&room_id, &snap_node);

                // Broadcast the snapshot pack to all connected peers.
                let pack_bytes = pack_snapshot_pack(&snap_node, &[]);
                let pack_b64 = base64_encode(&pack_bytes);
                let msg = serde_json::json!({
                    "type":  "pack",
                    "from":  "server-snapshot",
                    "nodes": pack_b64,
                })
                .to_string();
                let _ = room.tx.send(msg);

                metrics::counter!("nodalmerge_snapshot_total", "kind" => kind, "room" => room_id.clone())
                    .increment(1);
                metrics::counter!("nodalmerge_snapshot_total", "kind" => kind, "room" => room_id.clone())
                    .increment(1);
                tracing::info!(
                    room = %room_id,
                    kind,
                    snap = %snap_id.to_hex(),
                    delta,
                    chain_depth = if is_full { 0 } else { *chain_depth + 1 },
                    "snapshot emitted"
                );

                // Update per-room state.
                *last_count = current_count;
                if is_full {
                    *chain_depth = 0;
                } else {
                    *chain_depth += 1;
                }
                *last_snap_id = Some(snap_id);
            }
        }
    }))
}
