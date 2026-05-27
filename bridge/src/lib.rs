use nodalmerge_core::{
    BlobStore, Ibf, MerkleSearchTree, MemoryBlobStore, MapOp, Op, TextOp, StateGraph, SyncCapabilities,
    TextRangeAnchor, TextRangeOp,
    TickConfig,
    derive_room_key, decrypt_ops, is_encrypted_node, extract_encrypted_payload, wrap_encrypted_ops,
    E2EE_KEY,
    pack_nodes, unpack_nodes,
    replay, canonical_hash,
    compact, rebuild_from_snapshot, unpack_snapshot_pack, verify_snapshot,
};
use nodalmerge_core::conflicts::{ConflictEvent, ConflictFingerprint};
use ed25519_dalek::SigningKey;
use std::collections::{HashSet, VecDeque};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

/// JavaScript-facing handle to a `StateGraph`.
///
/// Usage from JS/TS:
/// ```js
/// import init, { SyncStore } from './nodalmerge_bridge.js';
/// await init();
/// const store = new SyncStore(new Uint8Array(32)); // 32-byte author key
/// store.set("username", new TextEncoder().encode("Alice"));
/// const state = JSON.parse(store.resolve_json());
/// ```
#[wasm_bindgen]
pub struct SyncStore {
    graph: StateGraph,
    signing_key: SigningKey,
    blobs: MemoryBlobStore,
    /// D1: AES-256-GCM key derived from the room seed.  `None` = plaintext mode.
    room_key: Option<[u8; 32]>,
    /// E3: Tick-based batching config. `None` = immediate commit (default).
    tick_config: Option<TickConfig>,
    /// E3: Raw ops buffered during a tick window, flushed as one signed node.
    pending_tick_ops: Vec<Op>,
    /// G9: conflicts already delivered to the SDK. Fingerprint set so
    /// `take_conflicts_json` only yields new ones. Bounded by
    /// [`CONFLICT_SEEN_MAX`] with LRU-style eviction so long-lived rooms
    /// don't grow this set unboundedly.
    conflicts_seen: HashSet<ConflictFingerprint>,
    /// G9: FIFO of fingerprints in insertion order for bounded eviction.
    conflicts_seen_order: VecDeque<ConflictFingerprint>,
}

/// G9 — maximum remembered conflict fingerprints. Anything beyond this
/// may re-deliver (harmless — apps dedup on their side or show a
/// duplicate toast). 4096 covers ~months of normal app use.
const CONFLICT_SEEN_MAX: usize = 4096;

#[wasm_bindgen]
impl SyncStore {
    /// Create a new store. `author_key` must be exactly 32 bytes — used as
    /// the Ed25519 signing key seed. The corresponding verifying key becomes
    /// the peer's public identity.
    #[wasm_bindgen(constructor)]
    pub fn new(author_key: &[u8]) -> Result<SyncStore, JsValue> {
        if author_key.len() != 32 {
            return Err(JsValue::from_str("author_key must be 32 bytes"));
        }
        let mut seed = [0u8; 32];
        seed.copy_from_slice(author_key);
        let signing_key = SigningKey::from_bytes(&seed);
        Ok(SyncStore { graph: StateGraph::new(), signing_key, blobs: MemoryBlobStore::new(), room_key: None, tick_config: None, pending_tick_ops: Vec::new(), conflicts_seen: HashSet::new(), conflicts_seen_order: VecDeque::new() })
    }

    /// Hex-encoded Ed25519 public key — the peer's stable identity.
    pub fn pubkey_hex(&self) -> String {
        self.signing_key.verifying_key().to_bytes()
            .iter().map(|b| format!("{:02x}", b)).collect()
    }

    // -------------------------------------------------------------------------
    // D1: E2EE room key management
    // -------------------------------------------------------------------------

    /// Enable E2EE for this store.  `room_seed` must be 32 bytes — the same
    /// seed used with `sign_room_token` / `room_pubkey_hex`.
    ///
    /// After calling this, every `set`/`delete` produces encrypted nodes and
    /// every `import_pack` transparently decrypts incoming encrypted nodes.
    pub fn set_room_key(&mut self, room_seed: &[u8]) -> Result<(), JsValue> {
        if room_seed.len() != 32 {
            return Err(JsValue::from_str("room_seed must be 32 bytes"));
        }
        let mut seed = [0u8; 32];
        seed.copy_from_slice(room_seed);
        self.room_key = Some(derive_room_key(&seed));
        Ok(())
    }

    /// Disable E2EE — future writes will be plaintext.
    pub fn clear_room_key(&mut self) {
        self.room_key = None;
    }

    /// Returns `true` if E2EE is currently active.
    pub fn has_room_key(&self) -> bool {
        self.room_key.is_some()
    }

    // -------------------------------------------------------------------------
    // Internal: local op submission (E2EE-aware)
    // -------------------------------------------------------------------------

    /// Commit `ops` directly to the DAG (bypasses tick buffer). Encrypts if
    /// a room key is set.  Returns the new node's ID (used by `flush_tick`).
    fn commit_ops_immediate(
        &mut self,
        ops: Vec<Op>,
        wall_ms: u64,
    ) -> Result<nodalmerge_core::NodeId, JsValue> {
        let final_ops = if let Some(ref key) = self.room_key {
            // Pre-compute the lamport clock the graph will assign so the
            // nonce derivation matches the transaction's actual value.
            // Must mirror `StateGraph::apply_local` exactly — pure logical
            // clock, no wall-time fold.
            let _ = wall_ms; // wall-clock is never part of the Lamport value.
            let next_lamport = self.graph.lamport() + 1;
            let author = self.signing_key.verifying_key().to_bytes();
            wrap_encrypted_ops(key, &author, next_lamport, &ops)
                .map_err(|e| JsValue::from_str(&e.to_string()))?
        } else {
            ops
        };
        self.graph
            .apply_local(&self.signing_key, wall_ms, final_ops)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Submit `ops` locally. When tick mode is active (E3), ops are buffered
    /// and emitted as a single signed node on the next `flush_tick()` call
    /// (or immediately when `max_ops_per_tick` is reached).
    fn apply_local_ops(&mut self, ops: Vec<Op>, wall_ms: u64) -> Result<(), JsValue> {
        if self.tick_config.is_some() {
            self.pending_tick_ops.extend(ops);
            let limit = self.tick_config.as_ref().unwrap().max_ops_per_tick;
            if self.pending_tick_ops.len() >= limit {
                // Auto-flush when the buffer hits the size limit.
                let buffered = std::mem::take(&mut self.pending_tick_ops);
                self.commit_ops_immediate(buffered, wall_ms)?;
            }
            return Ok(());
        }
        self.commit_ops_immediate(ops, wall_ms).map(|_| ())
    }

    /// Set a key to a byte-array value.
    /// If E2EE is active (`set_room_key` was called) the op is encrypted
    /// before being committed to the DAG.
    pub fn set(&mut self, key: &str, value: &[u8]) -> Result<(), JsValue> {
        let op = Op::Map(MapOp::Set { key: key.to_string(), value: value.to_vec() });
        let now_ms = js_sys::Date::now() as u64;
        self.apply_local_ops(vec![op], now_ms)
    }

    /// Delete a key.
    pub fn delete(&mut self, key: &str) -> Result<(), JsValue> {
        let op = Op::Map(MapOp::Delete { key: key.to_string() });
        let now_ms = js_sys::Date::now() as u64;
        self.apply_local_ops(vec![op], now_ms)
    }

    /// Returns the resolved state as a JSON object: `{ key: base64_value, ... }`.
    /// Values are base64-encoded because JS doesn't have raw bytes in JSON.
    /// The E2EE sentinel key (`\x00e2ee`) is automatically filtered out.
    pub fn resolve_json(&self) -> String {
        use std::collections::HashMap;
        let state = self.graph.resolve();
        let b64: HashMap<String, String> = state
            .into_iter()
            .filter(|(k, _)| k != E2EE_KEY)
            .map(|(k, v)| (k, base64_encode(&v)))
            .collect();
        serde_json::to_string(&b64).unwrap_or_else(|_| "{}".into())
    }

    /// Like `resolve_json` but includes HLC metadata for each key so the UI
    /// can show *why* a value won. Returns:
    /// `{ key: { value: base64|null, lamport: N, author: "hex", blob_hash?: "hex", blob_size?: N } }`
    /// For blob keys: `value` is the blob bytes (base64) if we have them locally,
    /// `null` if the blob is still pending. `blob_hash` is always set for blob keys.
    pub fn resolve_json_with_meta(&self) -> String {
        let state = self.graph.resolve_with_meta();
        let mut map = serde_json::Map::new();
        for (key, (lamport, author, value, is_blob)) in state {
            let author_hex: String = author.iter()
                .map(|b| format!("{:02x}", b))
                .collect();
            let entry = if is_blob && value.len() == 32 {
                let mut hash_bytes = [0u8; 32];
                hash_bytes.copy_from_slice(&value);
                let blob_hash = nodalmerge_core::Hash(hash_bytes);
                let hash_hex = blob_hash.to_hex();
                if let Some(blob_data) = self.blobs.get(&blob_hash) {
                    serde_json::json!({
                        "value":     base64_encode(&blob_data),
                        "lamport":   lamport,
                        "author":    author_hex,
                        "blob_hash": hash_hex,
                        "blob_size": blob_data.len(),
                    })
                } else {
                    serde_json::json!({
                        "value":     null,
                        "lamport":   lamport,
                        "author":    author_hex,
                        "blob_hash": hash_hex,
                        "blob_size": null,
                    })
                }
            } else {
                serde_json::json!({
                    "value":   base64_encode(&value),
                    "lamport": lamport,
                    "author":  author_hex,
                })
            };
            map.insert(key, entry);
        }
        serde_json::Value::Object(map).to_string()
    }

    // -------------------------------------------------------------------------
    // E2: Speculative vs. Canonical State
    // -------------------------------------------------------------------------

    /// Canonical state: like `resolve_json` but excludes local writes that
    /// have not yet been confirmed by a remote peer.
    ///
    /// In Authoritative mode (E1), a write becomes canonical only after the
    /// server re-broadcasts a signed node for the same key. In AllowAll /
    /// Cooperative mode, all remote peer writes are canonical.
    pub fn resolve_json_canonical(&self) -> String {
        use std::collections::HashMap;
        let state = self.graph.resolve_canonical();
        let b64: HashMap<String, String> = state
            .into_iter()
            .filter(|(k, _)| k != E2EE_KEY)
            .map(|(k, v)| (k, base64_encode(&v)))
            .collect();
        serde_json::to_string(&b64).unwrap_or_else(|_| "{}".into())
    }

    // -------------------------------------------------------------------------
    // G9: Conflict surfacing
    // -------------------------------------------------------------------------

    /// Return any conflicts that have arisen since the last call, as a
    /// JSON array. Each entry:
    /// `{ kind, key, winner_author, winner_lamport, winner_op, loser_author, loser_lamport, loser_op }`
    ///
    /// Authors are hex strings; ops are tagged objects matching
    /// [`nodalmerge_core::conflicts::ConflictOp`] (`{kind:"set",value:base64}`,
    /// `{kind:"delete"}`, `{kind:"set_blob",blob_hash:"hex"}`, etc.).
    ///
    /// Idempotent: two back-to-back calls with no intervening graph
    /// changes return `"[]"` on the second call. The in-process seen-set
    /// is bounded ([`CONFLICT_SEEN_MAX`]), so extremely conflict-heavy
    /// long-lived rooms may see older events re-surface; apps should
    /// dedup on their side if they care.
    pub fn take_conflicts_json(&mut self) -> String {
        let conflicts = self.graph.detect_conflicts();
        let mut fresh: Vec<&ConflictEvent> = Vec::new();
        for c in &conflicts {
            let fp = c.fingerprint();
            if self.conflicts_seen.contains(&fp) {
                continue;
            }
            self.conflicts_seen.insert(fp.clone());
            self.conflicts_seen_order.push_back(fp);
            while self.conflicts_seen_order.len() > CONFLICT_SEEN_MAX {
                if let Some(old) = self.conflicts_seen_order.pop_front() {
                    self.conflicts_seen.remove(&old);
                }
            }
            fresh.push(c);
        }

        let arr: Vec<serde_json::Value> = fresh
            .into_iter()
            .map(|c| {
                serde_json::json!({
                    "kind":            serde_json::to_value(c.kind).unwrap_or(serde_json::Value::Null),
                    "key":             c.key,
                    "winner_author":   hex32(&c.winner_author),
                    "winner_lamport":  c.winner_lamport,
                    "winner_op":       serde_json::to_value(&c.winner_op).unwrap_or(serde_json::Value::Null),
                    "loser_author":    hex32(&c.loser_author),
                    "loser_lamport":   c.loser_lamport,
                    "loser_op":        serde_json::to_value(&c.loser_op).unwrap_or(serde_json::Value::Null),
                })
            })
            .collect();
        serde_json::to_string(&arr).unwrap_or_else(|_| "[]".into())
    }

    /// Read a single key from the **speculative** view (local + remote writes).
    /// Returns base64-encoded bytes, or an empty string if the key is absent.
    pub fn read_speculative(&self, key: &str) -> String {
        if key == E2EE_KEY { return String::new(); }
        self.graph.read_speculative(key)
            .map(|v| base64_encode(&v))
            .unwrap_or_default()
    }

    /// Read a single key from the **canonical** view (remote-only writes).
    /// Returns base64-encoded bytes, or an empty string if the key is absent.
    pub fn read_canonical(&self, key: &str) -> String {
        if key == E2EE_KEY { return String::new(); }
        self.graph.read_canonical(key)
            .map(|v| base64_encode(&v))
            .unwrap_or_default()
    }

    // -------------------------------------------------------------------------
    // E3: Tick-Based Batching
    // -------------------------------------------------------------------------

    /// Enable tick-based op batching. While active, `set`/`delete` calls
    /// accumulate into a buffer instead of immediately creating signed nodes.
    ///
    /// Call `flush_tick()` from `setInterval(fn, interval_ms)` in JS to emit
    /// the batched node on a timer. The buffer also auto-flushes if
    /// `max_ops_per_tick` ops accumulate before the next timer fires.
    pub fn set_tick_config(&mut self, interval_ms: u64, max_ops_per_tick: u32) {
        self.tick_config = Some(TickConfig {
            interval_ms,
            max_ops_per_tick: max_ops_per_tick as usize,
        });
    }

    /// Disable tick batching. Any pending buffered ops are committed immediately
    /// as a single signed node.
    pub fn clear_tick_config(&mut self) -> Result<(), JsValue> {
        self.tick_config = None;
        if !self.pending_tick_ops.is_empty() {
            let wall_ms = js_sys::Date::now() as u64;
            let buffered = std::mem::take(&mut self.pending_tick_ops);
            self.commit_ops_immediate(buffered, wall_ms).map(|_| ())?;
        }
        Ok(())
    }

    /// Flush any buffered ops as a single signed node and return a
    /// base64-encoded postcard pack of that node (ready to broadcast).
    /// Returns an empty string if there were no buffered ops to flush.
    ///
    /// Typical JS usage:
    /// ```js
    /// setInterval(() => {
    ///   const pack = store.flush_tick();
    ///   if (pack) ws.send(JSON.stringify({ type: 'pack', nodes: pack }));
    /// }, 16); // 60 fps
    /// ```
    pub fn flush_tick(&mut self) -> Result<String, JsValue> {
        if self.pending_tick_ops.is_empty() {
            return Ok(String::new());
        }
        let wall_ms = js_sys::Date::now() as u64;
        let buffered = std::mem::take(&mut self.pending_tick_ops);
        let node_id = self.commit_ops_immediate(buffered, wall_ms)?;
        let nodes = self.graph.get_nodes(&[node_id]);
        Ok(base64_encode(&pack_nodes(&nodes)))
    }

    // -------------------------------------------------------------------------
    // D4: Deterministic Replay
    // -------------------------------------------------------------------------

    /// Replay a base64-encoded postcard node pack and return the resolved state
    /// and its canonical Blake3 hash as a JSON object:
    /// ```json
    /// { "map": { "key": "base64_value", ... }, "hash": "hex64" }
    /// ```
    /// The `hash` is the same value used in D3 snapshot nodes — it is
    /// byte-identical on every peer that replays the same op log.
    ///
    /// Typical use: browser-side snapshot verification after compaction.
    pub fn replay_nodes_json(&self, b64: &str) -> Result<String, JsValue> {
        let bytes = base64_decode(b64)
            .map_err(|_| JsValue::from_str("invalid base64 in pack"))?;
        let nodes = unpack_nodes(&bytes)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let state = replay(&nodes, Some(self.graph.policy()))
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let map_b64: std::collections::HashMap<String, String> = state.map
            .into_iter()
            .map(|(k, v)| (k, base64_encode(&v)))
            .collect();
        let out = serde_json::json!({
            "map":  map_b64,
            "hash": state.hash.to_hex(),
        });
        serde_json::to_string(&out).map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Compute the canonical Blake3 hash of the current store's resolved state
    /// without modifying any state.  Equivalent to `replay` on all current
    /// nodes, but avoids re-applying every node from scratch.
    pub fn resolved_state_hash_hex(&self) -> String {
        use std::collections::BTreeMap;
        let raw: BTreeMap<String, Vec<u8>> = self.graph
            .resolve()
            .into_iter()
            .filter(|(k, _)| k != E2EE_KEY)
            .collect();
        canonical_hash(&raw).to_hex()
    }

    // -------------------------------------------------------------------------
    // C1: RGA Collaborative Text
    // -------------------------------------------------------------------------

    /// Insert the first character of `ch_str` at position `pos` (0-based)
    /// in the RGA text sequence identified by `key`.
    ///
    /// `pos = 0` inserts before all existing characters.  `pos` beyond the
    /// current length inserts at the end.
    ///
    /// Each call commits exactly one signed DAG node so that every character
    /// has a unique, stable RGA identity `(lamport, author)`.
    pub fn insert_text(&mut self, key: &str, pos: u32, ch_str: &str) -> Result<(), JsValue> {
        let ch = ch_str.chars().next()
            .ok_or_else(|| JsValue::from_str("ch must be a non-empty string"))?;

        let seq = self.graph.resolve_text_seq_with_chars(key);
        let pos = pos as usize;
        let after = if pos == 0 {
            None
        } else if pos <= seq.len() {
            Some(seq[pos - 1].0)
        } else {
            seq.last().map(|(id, _)| *id)
        };

        let now_ms = js_sys::Date::now() as u64;
        // Bypass tick buffer — each character needs its own unique (lamport, author) id.
        let op = Op::Text(TextOp::Insert { key: key.to_string(), after, ch });
        self.commit_ops_immediate(vec![op], now_ms).map(|_| ())
    }

    /// Delete the character at position `pos` (0-based) in the RGA text
    /// sequence for `key`.  Returns an error if `pos` is out of range.
    pub fn delete_text(&mut self, key: &str, pos: u32) -> Result<(), JsValue> {
        let seq = self.graph.resolve_text_seq_with_chars(key);
        let target = seq.get(pos as usize)
            .map(|(id, _)| *id)
            .ok_or_else(|| JsValue::from_str("delete_text: position out of range"))?;
        let op = Op::Text(TextOp::Delete { key: key.to_string(), target });
        let now_ms = js_sys::Date::now() as u64;
        self.commit_ops_immediate(vec![op], now_ms).map(|_| ())
    }

    /// Resolve the RGA text for `key` as a plain UTF-8 string.
    /// Returns an empty string if no text ops for this key exist yet.
    pub fn resolve_text(&self, key: &str) -> String {
        self.graph.resolve_text(key)
    }

    /// Resolve canonical replay-derived text for `key`.
    ///
    /// This is intended for persistence/export/audit call-sites that must not
    /// depend on runtime projection mode.
    pub fn resolve_text_canonical(&self, key: &str) -> String {
        self.graph.resolve_text_canonical(key)
    }

    /// Internal helper: persist one canonical range op node.
    fn commit_text_range_op(&mut self, range_op: TextRangeOp, wall_ms: u64) -> Result<(), JsValue> {
        self.graph
            .apply_local_text_range_op(&self.signing_key, wall_ms, range_op)
            .map(|_| ())
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Insert `text` at character position `pos` (0-based) using range-op
    /// lowering, then commit the lowered stream as deterministic one-char ops.
    pub fn insert_text_range(&mut self, key: &str, pos: u32, text: &str) -> Result<(), JsValue> {
        if text.is_empty() {
            return Ok(());
        }

        let now_ms = js_sys::Date::now() as u64;
        let range_op = TextRangeOp::Insert {
            key: key.to_string(),
            anchor: TextRangeAnchor::Offset(pos as usize),
            text: text.to_string(),
        };
        self.commit_text_range_op(range_op, now_ms)
    }

    /// Delete `len` characters starting at position `pos` (0-based) using
    /// range-op lowering into deterministic one-char deletions.
    pub fn delete_text_range(&mut self, key: &str, pos: u32, len: u32) -> Result<(), JsValue> {
        if len == 0 {
            return Ok(());
        }

        let now_ms = js_sys::Date::now() as u64;
        let range_op = TextRangeOp::Delete {
            key: key.to_string(),
            anchor: TextRangeAnchor::Offset(pos as usize),
            len_chars: len as usize,
        };
        self.commit_text_range_op(range_op, now_ms)
    }

    /// Insert `text` at start-of-document anchor.
    pub fn insert_text_range_start(&mut self, key: &str, text: &str) -> Result<(), JsValue> {
        if text.is_empty() {
            return Ok(());
        }
        let now_ms = js_sys::Date::now() as u64;
        let range_op = TextRangeOp::Insert {
            key: key.to_string(),
            anchor: TextRangeAnchor::Start,
            text: text.to_string(),
        };
        self.commit_text_range_op(range_op, now_ms)
    }

    /// Insert `text` at end-of-document anchor.
    pub fn insert_text_range_end(&mut self, key: &str, text: &str) -> Result<(), JsValue> {
        if text.is_empty() {
            return Ok(());
        }
        let now_ms = js_sys::Date::now() as u64;
        let range_op = TextRangeOp::Insert {
            key: key.to_string(),
            anchor: TextRangeAnchor::End,
            text: text.to_string(),
        };
        self.commit_text_range_op(range_op, now_ms)
    }

    /// Insert `text` after a specific anchor char id (`lamport`, `author_hex32`).
    pub fn insert_text_range_after(
        &mut self,
        key: &str,
        after_lamport: u64,
        after_author_hex32: &str,
        text: &str,
    ) -> Result<(), JsValue> {
        if text.is_empty() {
            return Ok(());
        }
        let after = parse_op_id(after_lamport, after_author_hex32)?;
        let now_ms = js_sys::Date::now() as u64;
        let range_op = TextRangeOp::Insert {
            key: key.to_string(),
            anchor: TextRangeAnchor::After(after),
            text: text.to_string(),
        };
        self.commit_text_range_op(range_op, now_ms)
    }

    /// Delete `len` chars beginning at start-of-document anchor.
    pub fn delete_text_range_start(&mut self, key: &str, len: u32) -> Result<(), JsValue> {
        if len == 0 {
            return Ok(());
        }
        let now_ms = js_sys::Date::now() as u64;
        let range_op = TextRangeOp::Delete {
            key: key.to_string(),
            anchor: TextRangeAnchor::Start,
            len_chars: len as usize,
        };
        self.commit_text_range_op(range_op, now_ms)
    }

    /// Delete `len` chars beginning after a specific anchor char id.
    pub fn delete_text_range_after(
        &mut self,
        key: &str,
        after_lamport: u64,
        after_author_hex32: &str,
        len: u32,
    ) -> Result<(), JsValue> {
        if len == 0 {
            return Ok(());
        }
        let after = parse_op_id(after_lamport, after_author_hex32)?;
        let now_ms = js_sys::Date::now() as u64;
        let range_op = TextRangeOp::Delete {
            key: key.to_string(),
            anchor: TextRangeAnchor::After(after),
            len_chars: len as usize,
        };
        self.commit_text_range_op(range_op, now_ms)
    }

    // -------------------------------------------------------------------------
    // F8: Fractional-index List CRDT
    // -------------------------------------------------------------------------
    //
    // The list stores only ordering — `(ItemId, FracIdx)` pairs. Item *content*
    // lives in a sidecar Map keyed by hex(item_id); the SDK composes the two.
    // Each list op commits one signed node so concurrent edits get unique
    // `(lamport, author)` LWW priorities.
    //
    // Index-based helpers (`list_insert_at`, `list_move_to`) hide fractional-
    // index math from the SDK: pass an index, get an op committed.

    /// Insert `item_id` at `index` (0-based) in the list at `key`.
    ///
    /// `index = 0` puts it before all existing items; `index >= length` appends.
    /// `item_id_hex` must be 32 hex chars (a 16-byte id, typically a v4 UUID
    /// without dashes — the SDK generates it).
    pub fn list_insert_at(&mut self, key: &str, index: u32, item_id_hex: &str)
        -> Result<(), JsValue>
    {
        let item_id = parse_item_id(item_id_hex)?;
        let seq = self.graph.resolve_list(key);
        let position = position_for_index(&seq, index as usize, None);
        let op = Op::List(nodalmerge_core::ListOp::Insert {
            list_key: key.to_string(),
            item_id,
            position,
        });
        let now_ms = js_sys::Date::now() as u64;
        // Bypass tick buffer — each list op needs its own (lamport, author) id
        // to LWW correctly against concurrent edits.
        self.commit_ops_immediate(vec![op], now_ms).map(|_| ())
    }

    /// Move `item_id` to `index` (0-based) in the list at `key`. Returns an
    /// error if the item is not currently present (deleted or never inserted).
    ///
    /// The destination index is interpreted in the list **with the moved item
    /// removed**, so `move_to(item, length-1)` always lands the item at the end.
    pub fn list_move_to(&mut self, key: &str, item_id_hex: &str, index: u32)
        -> Result<(), JsValue>
    {
        let item_id = parse_item_id(item_id_hex)?;
        let seq = self.graph.resolve_list(key);
        if !seq.iter().any(|(id, _)| *id == item_id) {
            return Err(JsValue::from_str("list_move_to: item not in list"));
        }
        let position = position_for_index(&seq, index as usize, Some(item_id));
        let op = Op::List(nodalmerge_core::ListOp::Move {
            list_key: key.to_string(),
            item_id,
            position,
        });
        let now_ms = js_sys::Date::now() as u64;
        self.commit_ops_immediate(vec![op], now_ms).map(|_| ())
    }

    /// Tombstone `item_id` in the list at `key`. Idempotent — deleting an
    /// already-deleted or unknown item is a no-op (returns Ok).
    pub fn list_delete(&mut self, key: &str, item_id_hex: &str)
        -> Result<(), JsValue>
    {
        let item_id = parse_item_id(item_id_hex)?;
        let op = Op::List(nodalmerge_core::ListOp::Delete {
            list_key: key.to_string(),
            item_id,
        });
        let now_ms = js_sys::Date::now() as u64;
        self.commit_ops_immediate(vec![op], now_ms).map(|_| ())
    }

    /// Resolve the visible list at `key` as JSON:
    /// `[{ "id": "<hex32>", "position": "<frac>" }, ...]` sorted by position.
    /// Tombstoned items are omitted. Returns `"[]"` for an unknown key.
    pub fn list_resolve_json(&self, key: &str) -> String {
        let seq = self.graph.resolve_list(key);
        let arr: Vec<serde_json::Value> = seq.into_iter().map(|(id, pos)| {
            serde_json::json!({ "id": id.to_hex(), "position": pos.0 })
        }).collect();
        serde_json::Value::Array(arr).to_string()
    }

    /// Number of visible items in the list at `key`.
    pub fn list_length(&self, key: &str) -> u32 {
        self.graph.resolve_list(key).len() as u32
    }

    /// Hex item ids of the visible list at `key`, in order — convenience for
    /// the SDK when it doesn't need positions.
    pub fn list_ids_json(&self, key: &str) -> String {
        let ids: Vec<String> = self.graph.resolve_list(key)
            .into_iter()
            .map(|(id, _)| id.to_hex())
            .collect();
        serde_json::to_string(&ids).unwrap_or_else(|_| "[]".into())
    }

    // -------------------------------------------------------------------------
    // D2: IBF-based peer-to-peer diff
    // -------------------------------------------------------------------------
    ///
    /// `their_ibf_b64` is the base64-encoded postcard IBF the remote peer sent.
    /// Returns JSON `{ "only_mine": ["hex",...], "only_theirs": ["hex",...] }` on
    /// success, or an `"ibf-overflow"` error string if the diff exceeds the IBF
    /// decode capacity (~26 nodes).  The caller should fall back to a full-pack
    /// exchange when this errors.
    ///
    /// Used by the D2 WebRTC `sync` data channel to do efficient IBF-based node
    /// reconciliation directly between peers without the server.
    pub fn diff_with_ibf_b64(&self, their_ibf_b64: &str) -> Result<String, JsValue> {
        let their_bytes = base64_decode(their_ibf_b64)
            .map_err(|_| JsValue::from_str("invalid base64 in IBF"))?;
        let their_ibf = Ibf::decode_bytes(&their_bytes)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let ids = self.graph.all_node_ids();
        let mut my_ibf = Ibf::from_ids(&ids);
        my_ibf.subtract(&their_ibf);
        match my_ibf.decode() {
            Some((only_mine, only_theirs)) => {
                let mine_hex: Vec<String>   = only_mine.iter().map(|h| h.to_hex()).collect();
                let theirs_hex: Vec<String> = only_theirs.iter().map(|h| h.to_hex()).collect();
                let out = serde_json::json!({ "only_mine": mine_hex, "only_theirs": theirs_hex });
                serde_json::to_string(&out).map_err(|e| JsValue::from_str(&e.to_string()))
            }
            None => Err(JsValue::from_str("ibf-overflow")),
        }
    }

    // -------------------------------------------------------------------------
    // D3: DAG Compaction
    // -------------------------------------------------------------------------

    /// Create a compaction snapshot of the current graph state.
    ///
    /// Returns a JSON object:
    /// ```json
    /// { "snapshot_b64": "<base64>", "snapshot_hash": "<hex64>" }
    /// ```
    /// `snapshot_b64` is a postcard-encoded pack of exactly one node — the
    /// snapshot sentinel — that can be sent to peers.  `snapshot_hash` is the
    /// Blake3 hash of the resolved map at the time of compaction (same value
    /// that `rebuild_from_snapshot` / `apply_snapshot_pack_json` verifies).
    ///
    /// After calling this, replace the store's graph with one seeded from the
    /// snapshot by calling `apply_snapshot_pack_json` so that future writes
    /// parent off the snapshot.
    pub fn create_snapshot_json(&self) -> Result<String, JsValue> {
        let snap = compact(&self.graph, &self.signing_key)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let hash_hex = verify_snapshot(&snap)
            .map_err(|e| JsValue::from_str(&e.to_string()))?
            .snapshot_hash
            .to_hex();
        let pack_b64 = base64_encode(&pack_nodes(&[&snap]));
        let out = serde_json::json!({
            "snapshot_b64":  pack_b64,
            "snapshot_hash": hash_hex,
        });
        serde_json::to_string(&out).map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Install a snapshot pack (snapshot node + optional delta nodes) received
    /// from a peer.  Replaces the entire graph with the rebuilt state.
    ///
    /// `pack_b64` is a base64-encoded postcard `Vec<SyncNode>` where the first
    /// node is the snapshot sentinel and subsequent nodes are post-snapshot
    /// delta writes.
    ///
    /// Returns the snapshot hash as a 64-char hex string on success.
    pub fn apply_snapshot_pack_json(&mut self, pack_b64: &str) -> Result<String, JsValue> {
        let bytes = base64_decode(pack_b64)
            .map_err(|_| JsValue::from_str("invalid base64 in snapshot pack"))?;
        let (snap, delta) = unpack_snapshot_pack(&bytes)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let meta = verify_snapshot(&snap)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let rebuilt = rebuild_from_snapshot(snap, &delta)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        self.graph = rebuilt;
        Ok(meta.snapshot_hash.to_hex())
    }

    // -----------------------------------------------------------------
    // Blob operations (Phase 3)
    // -----------------------------------------------------------------

    /// Hash `data`, store it in the local BlobStore, and emit a `SetBlob` op
    /// into the DAG. Returns the 64-char hex hash so JS can display it.
    pub fn set_blob(&mut self, key: &str, data: &[u8]) -> Result<String, JsValue> {
        let hash = self.blobs.put(data.to_vec());
        let op = Op::Map(MapOp::SetBlob { key: key.to_string(), blob_hash: hash });
        let now_ms = js_sys::Date::now() as u64;
        self.apply_local_ops(vec![op], now_ms)?;
        Ok(hash.to_hex())
    }

    /// True if this store already holds the blob with the given hex hash.
    pub fn has_blob(&self, hash_hex: &str) -> bool {
        parse_hex_hash(hash_hex)
            .map(|h| self.blobs.contains(&h))
            .unwrap_or(false)
    }

    /// Retrieve blob bytes by hex hash. Errors if not present.
    pub fn get_blob_bytes(&self, hash_hex: &str) -> Result<js_sys::Uint8Array, JsValue> {
        let hash = parse_hex_hash(hash_hex)
            .ok_or_else(|| JsValue::from_str("invalid hash hex"))?;
        let data = self.blobs.get(&hash)
            .ok_or_else(|| JsValue::from_str("blob not found"))?;
        Ok(js_sys::Uint8Array::from(data.as_slice()))
    }

    /// Store a blob received from a peer. Verifies Blake3 integrity before
    /// accepting — rejects corrupt/mismatched data.
    pub fn store_blob_bytes(&mut self, hash_hex: &str, data: &[u8]) -> Result<(), JsValue> {
        let expected = parse_hex_hash(hash_hex)
            .ok_or_else(|| JsValue::from_str("invalid hash hex"))?;
        let actual = nodalmerge_core::Hash::of(data);
        if actual != expected {
            return Err(JsValue::from_str("blob integrity check failed — hash mismatch"));
        }
        self.blobs.put(data.to_vec());
        Ok(())
    }

    /// JSON array of hex hashes for every blob currently held in the local
    /// BlobStore. Useful for re-uploading to a server that restarted.
    pub fn local_blob_hashes_json(&self) -> String {
        let hashes: Vec<String> = self.blobs.hashes().map(|h| h.to_hex()).collect();
        serde_json::to_string(&hashes).unwrap_or_else(|_| "[]".into())
    }

    /// JSON array of hex hashes that the graph references via `SetBlob` but
    /// that are not yet in the local BlobStore. Send this list as a
    /// `blob-request` to peers so they can serve the missing bytes.
    pub fn missing_blob_hashes_json(&self) -> String {
        let referenced = self.graph.referenced_blob_hashes();
        let missing: Vec<String> = referenced
            .iter()
            .filter(|h| !self.blobs.contains(h))
            .map(|h| h.to_hex())
            .collect();
        serde_json::to_string(&missing).unwrap_or_else(|_| "[]".into())
    }

    /// Serialize the requested blobs as `[{hash, data}]` for transport.
    /// Hashes not found locally are silently skipped.
    pub fn export_blobs_json(&self, hashes_json: &str) -> Result<String, JsValue> {
        let hex_list: Vec<String> = serde_json::from_str(hashes_json).unwrap_or_default();
        let mut arr = Vec::new();
        for hex in &hex_list {
            if let Some(hash) = parse_hex_hash(hex) {
                if let Some(data) = self.blobs.get(&hash) {
                    arr.push(serde_json::json!({ "hash": hex, "data": base64_encode(&data) }));
                }
            }
        }
        serde_json::to_string(&arr).map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Current Merkle root as a 64-char hex string.
    pub fn merkle_root_hex(&self) -> String {
        self.graph.merkle_root().to_hex()
    }

    /// Serialize all nodes to postcard binary (base64-encoded) for transport.
    /// Used when a late-joining peer needs the full history, and for
    /// `localStorage` persistence.
    pub fn export_all_nodes(&self) -> Result<String, JsValue> {
        let all_ids = self.graph.all_node_ids();
        let nodes = self.graph.get_nodes(&all_ids);
        Ok(base64_encode(&pack_nodes(&nodes)))
    }

    /// Import nodes received from a remote peer.
    /// Accepts a base64-encoded postcard byte string (A3 wire format).
    /// Handles out-of-order delivery by retrying nodes whose parents haven't
    /// arrived yet (multi-pass topological insertion). Duplicates are ignored.
    ///
    /// D1: If a room key is set, encrypted nodes (those carrying the E2EE
    /// sentinel op) are transparently decrypted before insertion so that
    /// `resolve_json` sees plaintext key-value pairs.
    pub fn import_pack(&mut self, b64: &str) -> Result<(), JsValue> {
        let bytes = base64_decode(b64)
            .map_err(|_| JsValue::from_str("invalid base64 in pack"))?;
        let nodes = unpack_nodes(&bytes)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;

        // Decrypt encrypted nodes if we have the room key.
        let nodes = if let Some(ref key) = self.room_key.clone() {
            nodes.into_iter()
                .map(|node| decrypt_node_if_needed(node, key))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e: nodalmerge_core::SyncError| JsValue::from_str(&e.to_string()))?
        } else {
            nodes
        };

        // Multi-pass: keep retrying nodes that failed due to missing parents
        // until no progress is made (which would indicate a genuinely broken pack).
        let mut pending = nodes;
        loop {
            if pending.is_empty() { break; }
            let before = pending.len();
            let mut still_pending = Vec::new();
            for node in pending {
                match self.graph.apply_remote(node.clone()) {
                    Ok(_) => {}
                    Err(nodalmerge_core::SyncError::DuplicateNode(_)) => {}
                    Err(nodalmerge_core::SyncError::MissingParent(_)) => {
                        still_pending.push(node); // retry after parents arrive
                    }
                    Err(e) => return Err(JsValue::from_str(&e.to_string())),
                }
            }
            pending = still_pending;
            if pending.len() == before {
                // No progress — pack is genuinely missing ancestor nodes.
                break;
            }
        }
        Ok(())
    }

    pub fn node_count(&self) -> u32 {
        self.graph.node_count() as u32
    }

    /// Returns a JSON array of all node IDs (as 64-char hex strings) this
    /// store knows about. Include this in announce messages so peers can
    /// compute only the delta they need to send.
    pub fn all_node_ids_json(&self) -> String {
        let ids: Vec<String> = self.graph.all_node_ids()
            .iter()
            .map(|h| h.to_hex())
            .collect();
        serde_json::to_string(&ids).unwrap_or_else(|_| "[]".into())
    }

    /// Returns a JSON array of frontier head IDs (64-char hex strings).
    /// Use this instead of `all_node_ids_json` in the `hello` handshake (A6):
    /// the frontier is O(concurrent branches) instead of O(total nodes).
    pub fn frontier_hex_json(&self) -> String {
        let hexes = self.graph.frontier().to_hex_vec();
        serde_json::to_string(&hexes).unwrap_or_else(|_| "[]".into())
    }

    /// Returns the capabilities this peer supports as a JSON object.
    /// Include this as the `caps` field in the `hello` message (A7).
    pub fn our_capabilities_json(&self) -> String {
        serde_json::to_string(&SyncCapabilities::default())
            .unwrap_or_else(|_| "{}".into())
    }

    /// Compute an Invertible Bloom Filter (IBF) over all node IDs and return
    /// it as a base64-encoded postcard string (B1). Include this as the `ibf`
    /// field in the `hello` message alongside `frontier` and `caps`.
    ///
    /// Wire cost: ~3.2KB regardless of how many nodes the graph contains.
    pub fn compute_ibf_b64(&self) -> String {
        let ids = self.graph.all_node_ids();
        let ibf = Ibf::from_ids(&ids);
        base64_encode(&ibf.encode())
    }

    /// Return the MST root hash as a 64-char hex string (B2).
    /// Include this as the `mst_root` field in the `hello` message.
    /// Two peers with identical node sets will always produce the same value.
    pub fn mst_root_hex(&self) -> String {
        let ids = self.graph.all_node_ids();
        MerkleSearchTree::from_ids(&ids).root_hash_hex()
    }

    /// Look up a single MST wire node at `path_hex` (e.g. `"3f"` or `""`
    /// for the root).  Used by the server to answer `mst-request` messages.
    /// Returns a JSON string of an `MstNodeWire`, or `"null"` if not found.
    pub fn mst_get_node_json(&self, path_hex: &str) -> String {
        let ids = self.graph.all_node_ids();
        let mst = MerkleSearchTree::from_ids(&ids);
        match mst.get_node_wire(path_hex) {
            Some(node) => serde_json::to_string(&node).unwrap_or("null".into()),
            None => "null".into(),
        }
    }

    /// Process an `mst-response` JSON payload (array of `MstNodeWire`) and
    /// return a JSON object describing the current descent state:
    /// `{ next_paths: ["hex", ...], only_mine: ["id_hex", ...], only_theirs: ["id_hex", ...] }`
    ///
    /// The JS caller feeds back `next_paths` as the next `mst-request`,
    /// repeating until `next_paths` is empty — at which point `only_theirs`
    /// is the list of missing node IDs to request from the server and
    /// `only_mine` is the list of node IDs to upload to the server.
    pub fn mst_process_response_json(&self, server_nodes_json: &str, my_root_hex: &str, their_root_hex: &str) -> String {
        use nodalmerge_core::mst::MstNodeWire;
        // Build client-side MST (same for every call during descent).
        let ids = self.graph.all_node_ids();
        let my_mst = MerkleSearchTree::from_ids(&ids);

        // Decode server nodes from JSON.
        let server_nodes: Vec<MstNodeWire> = match serde_json::from_str(server_nodes_json) {
            Ok(n) => n,
            Err(_) => return "{\"next_paths\":[],\"only_mine\":[],\"only_theirs\":[]}".into(),
        };

        // For each server node, compare with our local node to find divergence.
        let mut next_paths: Vec<String> = Vec::new();
        let mut only_mine: Vec<String> = Vec::new();
        let mut only_theirs: Vec<String> = Vec::new();

        for their_node in &server_nodes {
            let my_node = my_mst.get_node_wire(&their_node.path);
            if my_node.as_ref().map(|n| n.hash.as_str()) == Some(&their_node.hash) {
                continue; // subtrees match
            }

            // Compare children.
            let my_children = my_node.as_ref().map(|n| &n.children);
            let nibbles = "0123456789abcdef";
            for ch in nibbles.chars() {
                let ch_str = ch.to_string();
                let my_child_hash  = my_children.and_then(|c| c.get(&ch_str)).cloned();
                let their_child_hash = their_node.children.get(&ch_str).cloned();
                if my_child_hash == their_child_hash { continue; }

                let child_path = format!("{}{}", their_node.path, ch);

                // If this child is a leaf in the server's tree (no children), compare keys.
                // We approximate: if server_nodes contains this child, check it;
                // otherwise schedule for next round.
                let their_child_node = server_nodes.iter().find(|n| n.path == child_path);
                let my_child_node = my_mst.get_node_wire(&child_path);

                let their_has_children = their_child_node.map_or(false, |n| !n.children.is_empty());
                let my_has_children    = my_child_node.as_ref().map_or(false, |n| !n.children.is_empty());

                if !their_has_children && !my_has_children {
                    // Leaf diff — extract now.
                    let their_keys: std::collections::HashSet<&str> = their_child_node
                        .map(|n| n.keys.iter().map(|k| k.as_str()).collect())
                        .unwrap_or_default();
                    let my_keys: std::collections::HashSet<String> = my_child_node
                        .map(|n| n.keys.into_iter().collect())
                        .unwrap_or_default();
                    for k in my_keys.iter() { if !their_keys.contains(k.as_str()) { only_mine.push(k.clone()); } }
                    for &k in their_keys.iter() { if !my_keys.contains(k) { only_theirs.push(k.to_string()); } }
                } else {
                    next_paths.push(child_path);
                }
            }

            // Also compare leaf keys at this node directly.
            if their_node.children.is_empty() {
                let their_keys: std::collections::HashSet<&str> =
                    their_node.keys.iter().map(|k| k.as_str()).collect();
                let my_keys: std::collections::HashSet<String> = my_node
                    .map(|n| n.keys.into_iter().collect())
                    .unwrap_or_default();
                for k in my_keys.iter() { if !their_keys.contains(k.as_str()) { only_mine.push(k.clone()); } }
                for &k in their_keys.iter() { if !my_keys.contains(k) { only_theirs.push(k.to_string()); } }
            }
        }

        // De-duplicate (may appear via multiple paths).
        next_paths.sort(); next_paths.dedup();
        only_mine.sort(); only_mine.dedup();
        only_theirs.sort(); only_theirs.dedup();

        // Suppress the unused params warning — they're kept for future use
        // (e.g., early exit when roots match).
        let _ = (my_root_hex, their_root_hex);

        serde_json::json!({
            "next_paths":  next_paths,
            "only_mine":   only_mine,
            "only_theirs": only_theirs,
        }).to_string()
    }

    /// Export only the nodes NOT present in `known_ids_json` (a JSON array
    /// of 64-char hex strings). Returns a base64-encoded postcard byte string.
    pub fn export_nodes_missing_from(&self, known_ids_json: &str) -> Result<String, JsValue> {
        let known_hex: Vec<String> = serde_json::from_str(known_ids_json)
            .unwrap_or_default();
        let known: std::collections::HashSet<nodalmerge_core::NodeId> = known_hex
            .iter()
            .filter_map(|h| parse_hex_hash(h))
            .collect();
        let missing = self.graph.missing_hashes(&known);
        let nodes = self.graph.get_nodes(&missing);
        Ok(base64_encode(&pack_nodes(&nodes)))
    }
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

/// Parse a 32-char hex string into an `ItemId` (16 raw bytes).
fn parse_item_id(hex: &str) -> Result<nodalmerge_core::ItemId, JsValue> {
    if hex.len() != 32 {
        return Err(JsValue::from_str("item_id must be 32 hex chars"));
    }
    let mut bytes = [0u8; 16];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let hi = hex_nibble(chunk[0])
            .ok_or_else(|| JsValue::from_str("item_id: invalid hex"))?;
        let lo = hex_nibble(chunk[1])
            .ok_or_else(|| JsValue::from_str("item_id: invalid hex"))?;
        bytes[i] = (hi << 4) | lo;
    }
    Ok(nodalmerge_core::ItemId(bytes))
}

fn parse_op_id(lamport: u64, author_hex32: &str) -> Result<nodalmerge_core::OpId, JsValue> {
    let author = hex_to_array_32(author_hex32)
        .ok_or_else(|| JsValue::from_str("author must be 64 hex chars"))?;
    Ok(nodalmerge_core::OpId { lamport, author })
}

/// Compute the fractional position to drop a (possibly-moving) item at logical
/// `index` within the resolved list `seq`.
///
/// `exclude_id = Some(id)` is used by `list_move_to`: the moving item's current
/// position is filtered out before the index is interpreted, so
/// `move_to(item, len-1)` always lands the item at the end regardless of its
/// current position. `exclude_id = None` is used by `list_insert_at`.
///
/// The index is clamped: any value `>= filtered_len` becomes "append".
fn position_for_index(
    seq: &[(nodalmerge_core::ItemId, nodalmerge_core::FracIdx)],
    index: usize,
    exclude_id: Option<nodalmerge_core::ItemId>,
) -> nodalmerge_core::FracIdx {
    let filtered: Vec<&nodalmerge_core::FracIdx> = seq
        .iter()
        .filter(|(id, _)| Some(*id) != exclude_id)
        .map(|(_, p)| p)
        .collect();
    let i = index.min(filtered.len());
    let left  = if i == 0              { None } else { Some(filtered[i - 1]) };
    let right = if i >= filtered.len() { None } else { Some(filtered[i])     };
    nodalmerge_core::between(left, right)
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

fn hex32(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
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

fn hex_to_array_32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 { return None; }
    let mut bytes = [0u8; 32];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        let hi = hex_nibble(chunk[0])?;
        let lo = hex_nibble(chunk[1])?;
        bytes[i] = (hi << 4) | lo;
    }
    Some(bytes)
}

// ---------------------------------------------------------------------------
// C3: Free functions for room capability tokens
// ---------------------------------------------------------------------------

/// Return the Ed25519 verifying key (public key) corresponding to `room_seed`
/// as a 64-char hex string.  Pass this to `set-room-key` to lock a room.
///
/// `room_seed` must be exactly 32 bytes.
#[wasm_bindgen]
pub fn room_pubkey_hex(room_seed: &[u8]) -> Result<String, JsValue> {
    if room_seed.len() != 32 {
        return Err(JsValue::from_str("room_seed must be 32 bytes"));
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(room_seed);
    let sk = SigningKey::from_bytes(&seed);
    let vk_bytes = sk.verifying_key().to_bytes();
    Ok(vk_bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Sign a capability token authorising `peer_pubkey_hex` to join `room_id`
/// until `expiry_secs` (Unix timestamp).
///
/// `caps` is an optional JS array of capability strings such as
/// `["read:world/**", "write:intent/**"]`.  Pass `undefined` or `null`
/// (or an empty array) for full access — the current default.  The capabilities
/// are included in the signed message and cannot be tampered with after signing.
///
/// Returns a JSON object:
/// `{"peer_pubkey":"hex","expiry":u64,"caps":[…],"sig":"hex"}`
/// Include this verbatim as the `token` field of the `hello` message.
///
/// `room_seed` must be exactly 32 bytes (the room admin's Ed25519 seed).
#[wasm_bindgen]
pub fn sign_room_token(
    room_id: &str,
    peer_pubkey_hex: &str,
    expiry_secs: u64,
    room_seed: &[u8],
    caps: JsValue,
) -> Result<String, JsValue> {
    if room_seed.len() != 32 {
        return Err(JsValue::from_str("room_seed must be 32 bytes"));
    }
    let peer_bytes = hex_to_array_32(peer_pubkey_hex)
        .ok_or_else(|| JsValue::from_str("invalid peer_pubkey_hex"))?;
    let mut seed = [0u8; 32];
    seed.copy_from_slice(room_seed);
    let sk = SigningKey::from_bytes(&seed);

    // Parse optional JS string-array into Vec<String>.
    let capabilities: Vec<String> = if let Some(arr) = caps.dyn_ref::<js_sys::Array>() {
        arr.iter()
            .filter_map(|v| v.as_string())
            .collect()
    } else {
        vec![]
    };

    let token =
        nodalmerge_core::RoomToken::sign(room_id, &peer_bytes, expiry_secs, &capabilities, &sk);
    let json = serde_json::json!({
        "peer_pubkey": token.peer_pubkey_hex(),
        "expiry":      token.expiry_secs,
        "caps":        token.capabilities,
        "sig":         token.sig_hex(),
    });
    Ok(json.to_string())
}

// ---------------------------------------------------------------------------
// D1: free-function helpers
// ---------------------------------------------------------------------------

/// Given a 32-byte room seed, return the 32-byte AES-256-GCM key (as a
/// `Uint8Array`) that peers use for E2EE.  Call `store.set_room_key(seed)`
/// instead when using a `SyncStore` — this is exposed for out-of-band
/// key derivation or testing.
#[wasm_bindgen]
pub fn derive_e2ee_key(room_seed: &[u8]) -> Result<js_sys::Uint8Array, JsValue> {
    if room_seed.len() != 32 {
        return Err(JsValue::from_str("room_seed must be 32 bytes"));
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(room_seed);
    let key = derive_room_key(&seed);
    Ok(js_sys::Uint8Array::from(key.as_slice()))
}

// ---------------------------------------------------------------------------
// D1: internal node decryption helper
// ---------------------------------------------------------------------------

/// If `node` carries an E2EE sentinel op and `room_key` is provided, decrypt
/// the ops and return a node with the same id/signature/parents but with the
/// decrypted ops in its transaction.  On decryption failure the original node
/// is returned unchanged (so the graph stores the ciphertext and `resolve`
/// will see the sentinel key instead of plaintext — a safe fallback).
fn decrypt_node_if_needed(
    mut node: nodalmerge_core::SyncNode,
    room_key: &[u8; 32],
) -> Result<nodalmerge_core::SyncNode, nodalmerge_core::SyncError> {
    if !is_encrypted_node(&node.transaction.ops) {
        return Ok(node);
    }
    let payload = match extract_encrypted_payload(&node.transaction.ops) {
        Some(p) => p.to_vec(),
        None => return Ok(node),
    };
    let decrypted_ops = decrypt_ops(room_key, &payload)?;
    node.transaction.ops = decrypted_ops;
    Ok(node)
}
