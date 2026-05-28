use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use nodalmerge_core::{Frontier, Hash, MerkleSearchTree, NodeId, Policy, PolicyDefault, PolicyRule, RoomToken, StateGraph, SyncCapabilities, SyncNode, pack_nodes, unpack_nodes};
use nodalmerge_core::conflicts::ConflictFingerprint;
use crate::api::{
    BlobEntry, BlobRedirectEntry, CapabilitySet, CommandEnvelope, CommandResult, ConflictEntry,
    HostCommand,
    HostEvent, ListEntry, MapEntry, SessionId, TextEntry,
};
use crate::errors::{HostCoreError, HostCoreResult};
use crate::traits::HostBlobUrlResolver;
use crate::protocol::{ClientIbfInput, assemble_catchup_pack_envelope, assemble_welcome_envelope};
use serde_json::{Map, Value};

#[derive(Debug, Clone)]
struct SessionState {
    room_id: String,
    peer_pubkey_hex: String,
}

#[derive(Debug, Clone)]
struct TextCharState {
    id: String,
    after_id: Option<String>,
    ch: char,
    tombstoned: bool,
}

#[derive(Debug, Clone)]
struct ListItemState {
    id: String,
    value: Value,
}

#[derive(Debug, Clone)]
struct PresenceState {
    from_peer_pubkey: String,
    data: Value,
    expires_at_unix_ms: Option<u64>,
}

const CONFLICT_HISTORY_LIMIT: usize = 256;

pub struct HostEngine {
    rooms: HashSet<String>,
    sessions: HashMap<SessionId, SessionState>,
    room_maps: HashMap<String, BTreeMap<String, Value>>,
    room_texts: HashMap<String, BTreeMap<String, Vec<TextCharState>>>,
    room_text_next_id: HashMap<String, u64>,
    room_lists: HashMap<String, BTreeMap<String, Vec<ListItemState>>>,
    room_list_next_id: HashMap<String, u64>,
    room_blobs: HashMap<String, BTreeMap<String, String>>,
    room_presence: HashMap<String, HashMap<SessionId, PresenceState>>,
    room_subscriptions: HashMap<String, HashMap<SessionId, Vec<String>>>,
    room_auth_keys: HashMap<String, [u8; 32]>,
    room_policies: HashMap<String, Policy>,
    room_sync_graphs: HashMap<String, StateGraph>,
    room_conflict_fingerprints: HashMap<String, HashSet<ConflictFingerprint>>,
    room_conflict_history: HashMap<String, Vec<ConflictEntry>>,
    server_caps: CapabilitySet,
    blob_url_resolver: Option<Arc<dyn HostBlobUrlResolver>>,
}

impl Default for HostEngine {
    fn default() -> Self {
        Self {
            rooms: HashSet::new(),
            sessions: HashMap::new(),
            room_maps: HashMap::new(),
            room_texts: HashMap::new(),
            room_text_next_id: HashMap::new(),
            room_lists: HashMap::new(),
            room_list_next_id: HashMap::new(),
            room_blobs: HashMap::new(),
            room_presence: HashMap::new(),
            room_subscriptions: HashMap::new(),
            room_auth_keys: HashMap::new(),
            room_policies: HashMap::new(),
            room_sync_graphs: HashMap::new(),
            room_conflict_fingerprints: HashMap::new(),
            room_conflict_history: HashMap::new(),
            server_caps: CapabilitySet::default(),
            blob_url_resolver: None,
        }
    }
}

fn normalize_subscription_patterns(patterns: Vec<String>) -> Vec<String> {
    let mut cleaned = patterns
        .into_iter()
        .map(|pattern| pattern.trim().to_string())
        .filter(|pattern| !pattern.is_empty())
        .collect::<Vec<_>>();

    if cleaned.is_empty() || cleaned.iter().any(|pattern| pattern == "**") {
        return vec!["**".to_string()];
    }

    cleaned.shrink_to_fit();
    cleaned
}

fn glob_match(mut pattern: &[u8], mut path: &[u8]) -> bool {
    loop {
        if pattern == b"/**" {
            return path.is_empty() || path.starts_with(b"/");
        }

        if pattern.starts_with(b"/**/") {
            let rest = &pattern[4..];
            if !path.starts_with(b"/") {
                return false;
            }

            let mut idx = 1usize;
            loop {
                if glob_match(rest, &path[idx..]) {
                    return true;
                }
                while idx < path.len() && path[idx] != b'/' {
                    idx += 1;
                }
                if idx >= path.len() {
                    return false;
                }
                idx += 1;
            }
        }

        if pattern.starts_with(b"**/") {
            let rest = &pattern[3..];
            if glob_match(rest, path) {
                return true;
            }
        }

        if pattern.starts_with(b"**") {
            let rest = &pattern[2..];
            if rest.is_empty() {
                return true;
            }
            for i in 0..=path.len() {
                if glob_match(rest, &path[i..]) {
                    return true;
                }
            }
            return false;
        }

        if pattern.first() == Some(&b'*') {
            let rest = &pattern[1..];
            for i in 0..=path.len() {
                if glob_match(rest, &path[i..]) {
                    return true;
                }
                if i < path.len() && path[i] == b'/' {
                    return false;
                }
            }
            return false;
        }

        if pattern.is_empty() {
            return path.is_empty();
        }
        if path.is_empty() {
            return false;
        }
        if pattern[0] != path[0] {
            return false;
        }
        pattern = &pattern[1..];
        path = &path[1..];
    }
}

fn subscription_matches_path(patterns: &[String], path: &str) -> bool {
    if patterns.iter().any(|pattern| pattern == "**") {
        return true;
    }

    patterns.iter().any(|pattern| glob_match(pattern.as_bytes(), path.as_bytes()))
}

fn subscription_accepts_node(node: &SyncNode, patterns: &[String]) -> bool {
    if patterns.iter().any(|pattern| pattern == "**") {
        return true;
    }

    if node.transaction.ops.is_empty() {
        return true;
    }

    for op in &node.transaction.ops {
        let Some(key) = op.key() else {
            continue;
        };

        if key.as_bytes().first().copied() == Some(0u8) {
            return true;
        }

        if subscription_matches_path(patterns, key) {
            return true;
        }
    }

    false
}

pub fn filter_pack_envelope_by_subscription(env: &str, patterns: &[String]) -> Option<String> {
    if patterns.is_empty() || patterns.iter().any(|pattern| pattern == "**") {
        return Some(env.to_string());
    }

    let parsed: Value = match serde_json::from_str(env) {
        Ok(value) => value,
        Err(_) => return Some(env.to_string()),
    };

    if parsed["type"] != "pack" {
        return Some(env.to_string());
    }

    let Some(nodes_b64) = parsed["nodes"].as_str() else {
        return Some(env.to_string());
    };

    let bytes = match base64_decode(nodes_b64) {
        Ok(decoded) => decoded,
        Err(_) => return Some(env.to_string()),
    };

    let nodes = match unpack_nodes(&bytes) {
        Ok(decoded) => decoded,
        Err(_) => return Some(env.to_string()),
    };

    let kept = nodes
        .into_iter()
        .filter(|node| subscription_accepts_node(node, patterns))
        .collect::<Vec<_>>();

    if kept.is_empty() {
        return None;
    }

    let kept_refs = kept.iter().collect::<Vec<_>>();
    let mut out = parsed.clone();
    out["nodes"] = Value::String(base64_encode(&pack_nodes(&kept_refs)));
    Some(out.to_string())
}

fn scoped_map_key(namespace: &str, key: &str) -> String {
    if namespace.is_empty() {
        return key.to_string();
    }

    format!("{namespace}/{key}")
}

fn strip_namespace_from_scoped_key(namespace: &str, scoped_key: &str) -> Option<String> {
    if namespace.is_empty() {
        return Some(scoped_key.to_string());
    }

    let prefix = format!("{namespace}/");
    scoped_key.strip_prefix(&prefix).map(ToOwned::to_owned)
}

fn next_text_id(room_id: &str, room_text_next_id: &mut HashMap<String, u64>) -> String {
    let next = room_text_next_id.entry(room_id.to_string()).or_insert(0);
    *next += 1;
    format!("text-{next}")
}

fn next_list_id(room_id: &str, room_list_next_id: &mut HashMap<String, u64>) -> String {
    let next = room_list_next_id.entry(room_id.to_string()).or_insert(0);
    *next += 1;
    format!("list-{next}")
}

fn text_entries_in_rga_order(chars: &[TextCharState]) -> Vec<TextEntry> {
    let mut children: HashMap<Option<String>, Vec<String>> = HashMap::new();
    let mut id_to_state: HashMap<String, &TextCharState> = HashMap::new();

    for st in chars {
        id_to_state.insert(st.id.clone(), st);
        children
            .entry(st.after_id.clone())
            .or_default()
            .push(st.id.clone());
    }

    for sibling_ids in children.values_mut() {
        sibling_ids.sort_unstable_by(|a, b| b.cmp(a));
    }

    let mut ordered = Vec::new();
    let mut stack: Vec<String> = Vec::new();

    if let Some(roots) = children.get(&None) {
        for id in roots.iter().rev() {
            stack.push(id.clone());
        }
    }

    while let Some(id) = stack.pop() {
        if let Some(st) = id_to_state.get(&id) {
            if !st.tombstoned {
                ordered.push(TextEntry {
                    id: st.id.clone(),
                    ch: st.ch.to_string(),
                });
            }
        }

        if let Some(kids) = children.get(&Some(id)) {
            for kid in kids.iter().rev() {
                stack.push(kid.clone());
            }
        }
    }

    ordered
}

fn text_string_from_entries(entries: &[TextEntry]) -> String {
    entries.iter().flat_map(|e| e.ch.chars().take(1)).collect()
}

impl HostEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_blob_url_resolver(mut self, resolver: Arc<dyn HostBlobUrlResolver>) -> Self {
        self.blob_url_resolver = Some(resolver);
        self
    }

    pub fn apply(&mut self, envelope: CommandEnvelope) -> HostCoreResult<CommandResult> {
        if envelope.room_id.is_empty() {
            return Err(HostCoreError::InvalidCommand);
        }

        match envelope.command {
            HostCommand::EnsureRoom => {
                self.rooms.insert(envelope.room_id.clone());
                self.room_maps
                    .entry(envelope.room_id.clone())
                    .or_default();
                self.room_texts
                    .entry(envelope.room_id.clone())
                    .or_default();
                self.room_lists
                    .entry(envelope.room_id.clone())
                    .or_default();
                self.room_blobs
                    .entry(envelope.room_id.clone())
                    .or_default();
                self.room_presence
                    .entry(envelope.room_id.clone())
                    .or_default();
                self.room_subscriptions
                    .entry(envelope.room_id.clone())
                    .or_default();
                self.room_policies
                    .entry(envelope.room_id.clone())
                    .or_default();
                self.room_sync_graphs
                    .entry(envelope.room_id.clone())
                    .or_default();
                self.room_conflict_fingerprints
                    .entry(envelope.room_id.clone())
                    .or_default();
                self.room_conflict_history
                    .entry(envelope.room_id.clone())
                    .or_default();
                Ok(CommandResult {
                    events: vec![HostEvent::RoomEnsured {
                        room_id: envelope.room_id,
                    }],
                })
            }
            HostCommand::OpenSession {
                session_id,
                peer_pubkey_hex,
            } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }
                if self.sessions.contains_key(&session_id) {
                    return Err(HostCoreError::SessionAlreadyOpen);
                }
                self.sessions.insert(
                    session_id,
                    SessionState {
                        room_id: envelope.room_id.clone(),
                        peer_pubkey_hex,
                    },
                );
                Ok(CommandResult {
                    events: vec![HostEvent::SessionOpened {
                        room_id: envelope.room_id,
                        session_id,
                    }],
                })
            }
            HostCommand::CloseSession { session_id } => {
                let Some(state) = self.sessions.remove(&session_id) else {
                    return Err(HostCoreError::SessionNotFound);
                };
                let room_id = state.room_id.clone();

                let mut events = vec![HostEvent::SessionClosed {
                    room_id: room_id.clone(),
                    session_id,
                }];

                if let Some(room_presence) = self.room_presence.get_mut(&room_id) {
                    if let Some(removed) = room_presence.remove(&session_id) {
                        events.push(HostEvent::PresenceValueRemoved {
                            room_id: room_id.clone(),
                            session_id,
                            from_peer_pubkey: removed.from_peer_pubkey,
                            reason: "leave".to_string(),
                        });
                    }
                }

                if let Some(room_subscriptions) = self.room_subscriptions.get_mut(&room_id) {
                    room_subscriptions.remove(&session_id);
                }

                Ok(CommandResult {
                    events,
                })
            }
            HostCommand::ClientHello { session_id, hello } => {
                let Some(state) = self.sessions.get(&session_id) else {
                    return Err(HostCoreError::SessionNotFound);
                };
                if state.room_id != envelope.room_id {
                    return Err(HostCoreError::ProtocolViolation);
                }
                if state.peer_pubkey_hex != hello.peer_pubkey_hex {
                    return Err(HostCoreError::ProtocolViolation);
                }

                if let Some(room_vk_bytes) = self.room_auth_keys.get(&envelope.room_id) {
                    let token = hello.token.as_ref().ok_or(HostCoreError::AuthViolation)?;
                    let peer_pubkey = parse_hex_32(&hello.peer_pubkey_hex)
                        .ok_or(HostCoreError::AuthViolation)?;
                    let room_vk = ed25519_dalek::VerifyingKey::from_bytes(room_vk_bytes)
                        .map_err(|_| HostCoreError::InternalInvariant)?;
                    let room_token = RoomToken::from_wire(
                        &token.peer_pubkey,
                        token.expiry,
                        token.caps.clone(),
                        &token.sig,
                    )
                    .map_err(|_| HostCoreError::AuthViolation)?;

                    let now_secs = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    room_token
                        .verify(&envelope.room_id, &room_vk, &peer_pubkey, now_secs)
                        .map_err(|_| HostCoreError::AuthViolation)?;
                }

                let negotiated = CapabilitySet {
                    supports_ibf: self.server_caps.supports_ibf && hello.capabilities.supports_ibf,
                    supports_mst: self.server_caps.supports_mst && hello.capabilities.supports_mst,
                };

                Ok(CommandResult {
                    events: vec![HostEvent::WelcomePrepared {
                        room_id: envelope.room_id,
                        session_id,
                        negotiated,
                        // Host-core will own frontier diff logic in later extraction slices.
                        missing_from_server_count: 0,
                    }],
                })
            }
            HostCommand::MapSet {
                namespace,
                key,
                value,
            } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }
                if key.is_empty() {
                    return Err(HostCoreError::InvalidCommand);
                }

                let scoped_key = scoped_map_key(&namespace, &key);
                let room_map = self
                    .room_maps
                    .entry(envelope.room_id.clone())
                    .or_default();
                room_map.insert(scoped_key, value.clone());

                Ok(CommandResult {
                    events: vec![HostEvent::MapValueUpserted {
                        room_id: envelope.room_id,
                        namespace,
                        key,
                        value,
                    }],
                })
            }
            HostCommand::MapDelete { namespace, key } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }
                if key.is_empty() {
                    return Err(HostCoreError::InvalidCommand);
                }

                let scoped_key = scoped_map_key(&namespace, &key);
                let found = self
                    .room_maps
                    .entry(envelope.room_id.clone())
                    .or_default()
                    .remove(&scoped_key)
                    .is_some();

                Ok(CommandResult {
                    events: vec![HostEvent::MapValueDeleted {
                        room_id: envelope.room_id,
                        namespace,
                        key,
                        found,
                    }],
                })
            }
            HostCommand::MapGet { namespace, key } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }
                if key.is_empty() {
                    return Err(HostCoreError::InvalidCommand);
                }

                let scoped_key = scoped_map_key(&namespace, &key);
                let value = self
                    .room_maps
                    .entry(envelope.room_id.clone())
                    .or_default()
                    .get(&scoped_key)
                    .cloned();

                Ok(CommandResult {
                    events: vec![HostEvent::MapValueRead {
                        room_id: envelope.room_id,
                        namespace,
                        key,
                        value,
                    }],
                })
            }
            HostCommand::MapAll { namespace } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }

                let entries = self
                    .room_maps
                    .entry(envelope.room_id.clone())
                    .or_default()
                    .iter()
                    .filter_map(|(scoped_key, value)| {
                        let key = strip_namespace_from_scoped_key(&namespace, scoped_key)?;
                        Some(MapEntry {
                            key,
                            value: value.clone(),
                        })
                    })
                    .collect::<Vec<_>>();

                Ok(CommandResult {
                    events: vec![HostEvent::MapEntriesListed {
                        room_id: envelope.room_id,
                        namespace,
                        entries,
                    }],
                })
            }
            HostCommand::TextInsert {
                namespace,
                key,
                after_id,
                ch,
            } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }
                if key.is_empty() {
                    return Err(HostCoreError::InvalidCommand);
                }

                let mut chars = ch.chars();
                let Some(insert_char) = chars.next() else {
                    return Err(HostCoreError::InvalidCommand);
                };
                if chars.next().is_some() {
                    return Err(HostCoreError::InvalidCommand);
                }

                let scoped_key = scoped_map_key(&namespace, &key);
                let room_docs = self
                    .room_texts
                    .entry(envelope.room_id.clone())
                    .or_default();
                let doc = room_docs.entry(scoped_key).or_default();

                if let Some(anchor) = after_id.as_ref() {
                    let anchor_exists = doc.iter().any(|item| item.id == *anchor);
                    if !anchor_exists {
                        return Err(HostCoreError::InvalidCommand);
                    }
                }

                let new_id = next_text_id(&envelope.room_id, &mut self.room_text_next_id);
                doc.push(TextCharState {
                    id: new_id.clone(),
                    after_id,
                    ch: insert_char,
                    tombstoned: false,
                });

                Ok(CommandResult {
                    events: vec![HostEvent::TextValueInserted {
                        room_id: envelope.room_id,
                        namespace,
                        key,
                        id: new_id,
                        ch: insert_char.to_string(),
                    }],
                })
            }
            HostCommand::TextDelete {
                namespace,
                key,
                target_id,
            } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }
                if key.is_empty() || target_id.is_empty() {
                    return Err(HostCoreError::InvalidCommand);
                }

                let scoped_key = scoped_map_key(&namespace, &key);
                let room_docs = self
                    .room_texts
                    .entry(envelope.room_id.clone())
                    .or_default();
                let doc = room_docs.entry(scoped_key).or_default();

                let mut found = false;
                for st in doc.iter_mut() {
                    if st.id == target_id {
                        st.tombstoned = true;
                        found = true;
                        break;
                    }
                }

                Ok(CommandResult {
                    events: vec![HostEvent::TextValueDeleted {
                        room_id: envelope.room_id,
                        namespace,
                        key,
                        target_id,
                        found,
                    }],
                })
            }
            HostCommand::TextGet { namespace, key } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }
                if key.is_empty() {
                    return Err(HostCoreError::InvalidCommand);
                }

                let scoped_key = scoped_map_key(&namespace, &key);
                let room_docs = self
                    .room_texts
                    .entry(envelope.room_id.clone())
                    .or_default();
                let doc = room_docs.entry(scoped_key).or_default();
                let entries = text_entries_in_rga_order(doc);
                let value = text_string_from_entries(&entries);

                Ok(CommandResult {
                    events: vec![HostEvent::TextValueRead {
                        room_id: envelope.room_id,
                        namespace,
                        key,
                        value,
                        entries,
                    }],
                })
            }
            HostCommand::TextGetCanonical { namespace, key } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }
                if key.is_empty() {
                    return Err(HostCoreError::InvalidCommand);
                }

                let scoped_key = scoped_map_key(&namespace, &key);
                let room_docs = self
                    .room_texts
                    .entry(envelope.room_id.clone())
                    .or_default();
                let doc = room_docs.entry(scoped_key).or_default();
                let entries = text_entries_in_rga_order(doc);
                let value = text_string_from_entries(&entries);

                Ok(CommandResult {
                    events: vec![HostEvent::TextValueRead {
                        room_id: envelope.room_id,
                        namespace,
                        key,
                        value,
                        entries,
                    }],
                })
            }
            HostCommand::ListPush {
                namespace,
                key,
                value,
            } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }
                if key.is_empty() {
                    return Err(HostCoreError::InvalidCommand);
                }

                let scoped_key = scoped_map_key(&namespace, &key);
                let room_lists = self
                    .room_lists
                    .entry(envelope.room_id.clone())
                    .or_default();
                let list = room_lists.entry(scoped_key).or_default();
                let index = list.len() as u64;
                let new_id = next_list_id(&envelope.room_id, &mut self.room_list_next_id);
                list.push(ListItemState {
                    id: new_id.clone(),
                    value: value.clone(),
                });

                Ok(CommandResult {
                    events: vec![HostEvent::ListValuePushed {
                        room_id: envelope.room_id,
                        namespace,
                        key,
                        id: new_id,
                        index,
                        value,
                    }],
                })
            }
            HostCommand::ListInsert {
                namespace,
                key,
                index,
                value,
            } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }
                if key.is_empty() {
                    return Err(HostCoreError::InvalidCommand);
                }

                let scoped_key = scoped_map_key(&namespace, &key);
                let room_lists = self
                    .room_lists
                    .entry(envelope.room_id.clone())
                    .or_default();
                let list = room_lists.entry(scoped_key).or_default();
                if index > list.len() as u64 {
                    return Err(HostCoreError::InvalidCommand);
                }

                let new_id = next_list_id(&envelope.room_id, &mut self.room_list_next_id);
                list.insert(
                    index as usize,
                    ListItemState {
                        id: new_id.clone(),
                        value: value.clone(),
                    },
                );

                Ok(CommandResult {
                    events: vec![HostEvent::ListValueInserted {
                        room_id: envelope.room_id,
                        namespace,
                        key,
                        id: new_id,
                        index,
                        value,
                    }],
                })
            }
            HostCommand::ListDelete {
                namespace,
                key,
                index,
            } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }
                if key.is_empty() {
                    return Err(HostCoreError::InvalidCommand);
                }

                let scoped_key = scoped_map_key(&namespace, &key);
                let room_lists = self
                    .room_lists
                    .entry(envelope.room_id.clone())
                    .or_default();
                let list = room_lists.entry(scoped_key).or_default();

                let (found, removed) = if (index as usize) < list.len() {
                    let removed = list.remove(index as usize).value;
                    (true, Some(removed))
                } else {
                    (false, None)
                };

                Ok(CommandResult {
                    events: vec![HostEvent::ListValueDeleted {
                        room_id: envelope.room_id,
                        namespace,
                        key,
                        index,
                        found,
                        removed,
                    }],
                })
            }
            HostCommand::ListMove {
                namespace,
                key,
                from_index,
                to_index,
            } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }
                if key.is_empty() {
                    return Err(HostCoreError::InvalidCommand);
                }

                let scoped_key = scoped_map_key(&namespace, &key);
                let room_lists = self
                    .room_lists
                    .entry(envelope.room_id.clone())
                    .or_default();
                let list = room_lists.entry(scoped_key).or_default();

                let (found, moved_id) = if (from_index as usize) < list.len() && (to_index as usize) < list.len() {
                    if from_index == to_index {
                        (true, Some(list[from_index as usize].id.clone()))
                    } else {
                        let moved = list.remove(from_index as usize);
                        let moved_id = moved.id.clone();
                        list.insert(to_index as usize, moved);
                        (true, Some(moved_id))
                    }
                } else {
                    (false, None)
                };

                Ok(CommandResult {
                    events: vec![HostEvent::ListValueMoved {
                        room_id: envelope.room_id,
                        namespace,
                        key,
                        from_index,
                        to_index,
                        found,
                        id: moved_id,
                    }],
                })
            }
            HostCommand::ListUpdate {
                namespace,
                key,
                index,
                value,
            } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }
                if key.is_empty() {
                    return Err(HostCoreError::InvalidCommand);
                }

                let scoped_key = scoped_map_key(&namespace, &key);
                let room_lists = self
                    .room_lists
                    .entry(envelope.room_id.clone())
                    .or_default();
                let list = room_lists.entry(scoped_key).or_default();

                let (found, item_id, updated_value) = if let Some(item) = list.get_mut(index as usize) {
                    item.value = value.clone();
                    (true, Some(item.id.clone()), Some(item.value.clone()))
                } else {
                    (false, None, None)
                };

                Ok(CommandResult {
                    events: vec![HostEvent::ListValueUpdated {
                        room_id: envelope.room_id,
                        namespace,
                        key,
                        index,
                        found,
                        id: item_id,
                        value: updated_value,
                    }],
                })
            }
            HostCommand::ListGet { namespace, key } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }
                if key.is_empty() {
                    return Err(HostCoreError::InvalidCommand);
                }

                let scoped_key = scoped_map_key(&namespace, &key);
                let room_lists = self
                    .room_lists
                    .entry(envelope.room_id.clone())
                    .or_default();
                let list = room_lists.entry(scoped_key).or_default();
                let entries = list
                    .iter()
                    .map(|item| ListEntry {
                        id: item.id.clone(),
                        value: item.value.clone(),
                    })
                    .collect::<Vec<_>>();

                Ok(CommandResult {
                    events: vec![HostEvent::ListValueRead {
                        room_id: envelope.room_id,
                        namespace,
                        key,
                        entries,
                    }],
                })
            }
            HostCommand::BlobSet {
                namespace,
                hash,
                data_b64,
            } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }
                if hash.is_empty() || data_b64.is_empty() {
                    return Err(HostCoreError::InvalidCommand);
                }

                let scoped_hash = scoped_map_key(&namespace, &hash);
                let room_blobs = self
                    .room_blobs
                    .entry(envelope.room_id.clone())
                    .or_default();
                let stored = room_blobs
                    .insert(scoped_hash, data_b64)
                    .is_none();

                Ok(CommandResult {
                    events: vec![HostEvent::BlobValueStored {
                        room_id: envelope.room_id,
                        namespace,
                        hash,
                        stored,
                    }],
                })
            }
            HostCommand::BlobGet { namespace, hash } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }
                if hash.is_empty() {
                    return Err(HostCoreError::InvalidCommand);
                }

                let scoped_hash = scoped_map_key(&namespace, &hash);
                let room_blobs = self
                    .room_blobs
                    .entry(envelope.room_id.clone())
                    .or_default();
                let data_b64 = room_blobs.get(&scoped_hash).cloned();

                Ok(CommandResult {
                    events: vec![HostEvent::BlobValueRead {
                        room_id: envelope.room_id,
                        namespace,
                        hash,
                        found: data_b64.is_some(),
                        data_b64,
                    }],
                })
            }
            HostCommand::BlobGetMany { namespace, hashes } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }

                let room_blobs = self
                    .room_blobs
                    .entry(envelope.room_id.clone())
                    .or_default();

                let mut entries = Vec::new();
                let mut missing = Vec::new();

                for hash in hashes {
                    if hash.is_empty() {
                        continue;
                    }

                    let scoped_hash = scoped_map_key(&namespace, &hash);
                    if let Some(data_b64) = room_blobs.get(&scoped_hash) {
                        entries.push(BlobEntry {
                            hash,
                            data_b64: data_b64.clone(),
                        });
                    } else {
                        missing.push(hash);
                    }
                }

                Ok(CommandResult {
                    events: vec![HostEvent::BlobValuesRead {
                        room_id: envelope.room_id,
                        namespace,
                        entries,
                        missing,
                    }],
                })
            }
            HostCommand::RequestUpload {
                namespace,
                hash,
                size_bytes,
                content_type,
            } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }
                if hash.is_empty() || size_bytes == 0 {
                    return Err(HostCoreError::InvalidCommand);
                }

                let put_url = self.blob_url_resolver.as_ref().and_then(|resolver| {
                    resolver.resolve_put_url(
                        &envelope.room_id,
                        &namespace,
                        &hash,
                        size_bytes,
                        content_type.as_deref(),
                    )
                });

                let outcome = classify_request_upload_resolve_put_url_outcome(put_url.is_some());

                let event = match outcome {
                    RequestUploadResolvePutUrlOutcome::GrantUpload => {
                        let put_url = put_url.expect("put_url should exist for grant outcome");
                        HostEvent::UploadGranted {
                            room_id: envelope.room_id,
                            namespace,
                            hash,
                            url: put_url.url,
                            expires_at_unix: put_url.expires_at_unix,
                        }
                    }
                    RequestUploadResolvePutUrlOutcome::DenyUseWs => HostEvent::UploadDenied {
                        room_id: envelope.room_id,
                        namespace,
                        hash,
                        reason: "use-ws".to_string(),
                    },
                };

                Ok(CommandResult { events: vec![event] })
            }
            HostCommand::BlobRequest { namespace, hashes } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }

                let requested = hashes
                    .into_iter()
                    .filter(|hash| !hash.is_empty())
                    .collect::<Vec<_>>();
                if requested.is_empty() {
                    return Err(HostCoreError::InvalidCommand);
                }

                let room_blobs = self
                    .room_blobs
                    .entry(envelope.room_id.clone())
                    .or_default();

                let redirects = if let Some(resolver) = self.blob_url_resolver.as_ref() {
                    requested
                        .iter()
                        .filter_map(|hash| {
                            let scoped_hash = scoped_map_key(&namespace, hash);
                            if !room_blobs.contains_key(&scoped_hash) {
                                return None;
                            }
                            resolver
                                .resolve_get_url(&envelope.room_id, &namespace, hash)
                                .map(|granted| BlobRedirectEntry {
                                    hash: hash.clone(),
                                    url: granted.url,
                                    expires_at_unix: granted.expires_at_unix,
                                })
                        })
                        .collect::<Vec<_>>()
                } else {
                    Vec::new()
                };

                if !redirects.is_empty() {
                    return Ok(CommandResult {
                        events: vec![HostEvent::BlobRedirectPrepared {
                            room_id: envelope.room_id,
                            namespace,
                            redirects,
                        }],
                    });
                }

                let blobs = requested
                    .iter()
                    .filter_map(|hash| {
                        let scoped_hash = scoped_map_key(&namespace, hash);
                        room_blobs.get(&scoped_hash).map(|data_b64| BlobEntry {
                            hash: hash.clone(),
                            data_b64: data_b64.clone(),
                        })
                    })
                    .collect::<Vec<_>>();

                Ok(CommandResult {
                    events: vec![HostEvent::BlobPackPrepared {
                        room_id: envelope.room_id,
                        namespace,
                        blobs,
                        requested,
                    }],
                })
            }
            HostCommand::PresenceSet {
                session_id,
                data,
                ttl_ms,
                now_unix_ms,
            } => {
                let Some(state) = self.sessions.get(&session_id) else {
                    return Err(HostCoreError::SessionNotFound);
                };
                if state.room_id != envelope.room_id {
                    return Err(HostCoreError::ProtocolViolation);
                }
                let from_peer_pubkey = state.peer_pubkey_hex.clone();

                let expires_at_unix_ms = match (ttl_ms, now_unix_ms) {
                    (Some(ttl), Some(now)) => Some(now.saturating_add(ttl)),
                    _ => None,
                };

                let room_presence = self
                    .room_presence
                    .entry(envelope.room_id.clone())
                    .or_default();
                let joined = !room_presence.contains_key(&session_id);
                room_presence.insert(
                    session_id,
                    PresenceState {
                        from_peer_pubkey: from_peer_pubkey.clone(),
                        data: data.clone(),
                        expires_at_unix_ms,
                    },
                );

                Ok(CommandResult {
                    events: vec![HostEvent::PresenceValueSet {
                        room_id: envelope.room_id,
                        session_id,
                        from_peer_pubkey,
                        data,
                        joined,
                    }],
                })
            }
            HostCommand::PresenceGetAll => {
                let entries = self
                    .room_presence
                    .entry(envelope.room_id.clone())
                    .or_default()
                    .iter()
                    .map(|(session_id, presence)| crate::api::PresenceEntry {
                        session_id: *session_id,
                        from_peer_pubkey: presence.from_peer_pubkey.clone(),
                        data: presence.data.clone(),
                        expires_at_unix_ms: presence.expires_at_unix_ms,
                    })
                    .collect::<Vec<_>>();

                Ok(CommandResult {
                    events: vec![HostEvent::PresenceValuesListed {
                        room_id: envelope.room_id,
                        entries,
                    }],
                })
            }
            HostCommand::PresenceSweep { now_unix_ms } => {
                let mut removed_events = Vec::new();
                if let Some(room_presence) = self.room_presence.get_mut(&envelope.room_id) {
                    let stale = room_presence
                        .iter()
                        .filter_map(|(session_id, presence)| {
                            presence
                                .expires_at_unix_ms
                                .filter(|expires| *expires <= now_unix_ms)
                                .map(|_| (*session_id, presence.from_peer_pubkey.clone()))
                        })
                        .collect::<Vec<_>>();

                    for (session_id, from_peer_pubkey) in stale {
                        room_presence.remove(&session_id);
                        removed_events.push(HostEvent::PresenceValueRemoved {
                            room_id: envelope.room_id.clone(),
                            session_id,
                            from_peer_pubkey,
                            reason: "stale".to_string(),
                        });
                    }
                }

                Ok(CommandResult {
                    events: removed_events,
                })
            }
            HostCommand::Subscribe {
                session_id,
                patterns,
            } => {
                let Some(state) = self.sessions.get(&session_id) else {
                    return Err(HostCoreError::SessionNotFound);
                };
                if state.room_id != envelope.room_id {
                    return Err(HostCoreError::ProtocolViolation);
                }

                let normalized_patterns = normalize_subscription_patterns(patterns);
                let room_subscriptions = self
                    .room_subscriptions
                    .entry(envelope.room_id.clone())
                    .or_default();
                room_subscriptions.insert(
                    session_id,
                    normalized_patterns.clone(),
                );

                Ok(CommandResult {
                    events: vec![HostEvent::SubscriptionUpdated {
                        room_id: envelope.room_id,
                        session_id,
                        patterns: normalized_patterns,
                    }],
                })
            }
            HostCommand::SetRoomKey { pubkey_hex } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }

                let Some(pubkey_bytes) = parse_hex_32(&pubkey_hex) else {
                    return Ok(CommandResult {
                        events: vec![HostEvent::SetRoomKeyRejected {
                            room_id: envelope.room_id,
                            msg: shape_set_room_key_invalid_pubkey_rejection_reason().to_string(),
                        }],
                    });
                };

                if self.room_auth_keys.contains_key(&envelope.room_id) {
                    return Ok(CommandResult {
                        events: vec![HostEvent::SetRoomKeyRejected {
                            room_id: envelope.room_id,
                            msg: shape_set_room_key_already_locked_rejection_reason().to_string(),
                        }],
                    });
                }

                self.room_auth_keys.insert(envelope.room_id.clone(), pubkey_bytes);
                Ok(CommandResult {
                    events: vec![HostEvent::RoomLocked {
                        room_id: envelope.room_id,
                        pubkey_hex,
                    }],
                })
            }
            HostCommand::SetPolicy { default, rules } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }

                let default_policy = match classify_set_policy_default(&default) {
                    SetPolicyDefaultParseResult::AllowAll => PolicyDefault::AllowAll,
                    SetPolicyDefaultParseResult::DenyAll => PolicyDefault::DenyAll,
                    SetPolicyDefaultParseResult::Unknown => {
                        return Ok(CommandResult {
                            events: vec![HostEvent::SetPolicyRejected {
                                room_id: envelope.room_id,
                                msg: shape_set_policy_unknown_default_error(&default),
                            }],
                        });
                    }
                };

                let mut parsed_rules = Vec::with_capacity(rules.len());
                for rule in rules {
                    if rule.path_glob.is_empty() {
                        return Ok(CommandResult {
                            events: vec![HostEvent::SetPolicyRejected {
                                room_id: envelope.room_id,
                                msg: "set-policy: each rule must have a 'path_glob' string".to_string(),
                            }],
                        });
                    }

                    let mut can_write = Vec::with_capacity(rule.can_write.len());
                    for key_hex in rule.can_write {
                        match parse_hex_32(&key_hex) {
                            Some(parsed) => can_write.push(parsed),
                            None => {
                                return Ok(CommandResult {
                                    events: vec![HostEvent::SetPolicyRejected {
                                        room_id: envelope.room_id,
                                        msg: format!("set-policy: invalid pubkey hex '{key_hex}'"),
                                    }],
                                });
                            }
                        }
                    }

                    parsed_rules.push(PolicyRule {
                        path_glob: rule.path_glob,
                        can_write,
                        can_read: vec![],
                        can_derive: vec![],
                    });
                }

                self.room_policies.insert(
                    envelope.room_id.clone(),
                    Policy {
                        rules: parsed_rules,
                        default: default_policy,
                    },
                );

                if let Some(graph) = self.room_sync_graphs.get_mut(&envelope.room_id) {
                    graph.set_policy(Policy {
                        rules: self
                            .room_policies
                            .get(&envelope.room_id)
                            .map(|p| p.rules.clone())
                            .unwrap_or_default(),
                        default: self
                            .room_policies
                            .get(&envelope.room_id)
                            .map(|p| p.default.clone())
                            .unwrap_or(PolicyDefault::AllowAll),
                    });
                }

                Ok(CommandResult {
                    events: vec![HostEvent::PolicySet {
                        room_id: envelope.room_id,
                    }],
                })
            }
            HostCommand::ImportPack { nodes_b64 } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }

                let decoded = base64_decode(&nodes_b64).map_err(|_| HostCoreError::InvalidCommand)?;
                let nodes: Vec<SyncNode> = unpack_nodes(&decoded).map_err(|_| HostCoreError::InvalidCommand)?;
                let incoming_count = nodes.len();

                let graph = self
                    .room_sync_graphs
                    .entry(envelope.room_id.clone())
                    .or_default();
                let applied = graph.apply_remote_batch(nodes);

                let now_unix_ms = now_unix_ms();
                let detected = graph.detect_conflicts();
                let seen_fingerprints = self
                    .room_conflict_fingerprints
                    .entry(envelope.room_id.clone())
                    .or_default();
                let history = self
                    .room_conflict_history
                    .entry(envelope.room_id.clone())
                    .or_default();
                let mut new_conflicts = Vec::new();
                for conflict in detected {
                    if seen_fingerprints.insert(conflict.fingerprint()) {
                        let entry = ConflictEntry {
                            at_unix_ms: now_unix_ms,
                            event: conflict,
                        };
                        history.push(entry.clone());
                        new_conflicts.push(entry);
                    }
                }
                if history.len() > CONFLICT_HISTORY_LIMIT {
                    let overflow = history.len() - CONFLICT_HISTORY_LIMIT;
                    history.drain(0..overflow);
                }

                let mut events = vec![HostEvent::PackImported {
                    room_id: envelope.room_id.clone(),
                    incoming_count,
                    accepted_count: applied.accepted.len(),
                    rejected_count: applied.rejected.len(),
                }];
                if !new_conflicts.is_empty() {
                    events.push(HostEvent::ConflictsObserved {
                        room_id: envelope.room_id,
                        entries: new_conflicts,
                    });
                }

                Ok(CommandResult {
                    events,
                })
            }
            HostCommand::RequestServerPack { known_ids } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }

                let known = known_ids
                    .into_iter()
                    .filter_map(|hex| parse_hex_32(&hex).map(Hash))
                    .collect::<HashSet<NodeId>>();

                let graph = self
                    .room_sync_graphs
                    .entry(envelope.room_id.clone())
                    .or_default();
                let missing = graph.missing_hashes(&known);
                let nodes = graph.get_nodes(&missing);
                let nodes_b64 = shape_catchup_pack_payload_b64(&nodes);

                Ok(CommandResult {
                    events: vec![HostEvent::ServerPackPrepared {
                        room_id: envelope.room_id,
                        nodes_b64,
                        root_hex: graph.merkle_root().to_hex(),
                    }],
                })
            }
            HostCommand::MstRequest { paths } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }

                let graph = self
                    .room_sync_graphs
                    .entry(envelope.room_id.clone())
                    .or_default();
                let all_ids = graph.all_node_ids();
                let mst = MerkleSearchTree::from_ids(&all_ids);
                let nodes = paths
                    .iter()
                    .filter_map(|path| mst.get_node_wire(path))
                    .filter_map(|wire| serde_json::to_value(wire).ok())
                    .collect::<Vec<_>>();

                Ok(CommandResult {
                    events: vec![HostEvent::MstResponsePrepared {
                        room_id: envelope.room_id,
                        nodes,
                    }],
                })
            }
            HostCommand::MstDone { ids } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }

                let requested = ids
                    .into_iter()
                    .filter_map(|hex| parse_hex_32(&hex).map(Hash))
                    .collect::<Vec<NodeId>>();
                let graph = self
                    .room_sync_graphs
                    .entry(envelope.room_id.clone())
                    .or_default();
                let nodes = graph.get_nodes(&requested);
                let nodes_b64 = shape_catchup_pack_payload_b64(&nodes);

                Ok(CommandResult {
                    events: vec![HostEvent::ServerPackPrepared {
                        room_id: envelope.room_id,
                        nodes_b64,
                        root_hex: graph.merkle_root().to_hex(),
                    }],
                })
            }
            HostCommand::GetRecentConflicts { since_unix_ms } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }

                let since = since_unix_ms.unwrap_or(0);
                let entries = self
                    .room_conflict_history
                    .entry(envelope.room_id.clone())
                    .or_default()
                    .iter()
                    .filter(|entry| entry.at_unix_ms >= since)
                    .cloned()
                    .collect::<Vec<_>>();

                Ok(CommandResult {
                    events: vec![HostEvent::RecentConflictsListed {
                        room_id: envelope.room_id,
                        entries,
                    }],
                })
            }
            HostCommand::RelayPeerSignal {
                session_id,
                msg_type,
                to_peer_pubkey,
                payload,
            } => {
                if !self.rooms.contains(&envelope.room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }

                let Some(state) = self.sessions.get(&session_id) else {
                    return Err(HostCoreError::SessionNotFound);
                };
                if state.room_id != envelope.room_id {
                    return Err(HostCoreError::ProtocolViolation);
                }
                if to_peer_pubkey.is_empty() || msg_type.is_empty() {
                    return Err(HostCoreError::InvalidCommand);
                }

                Ok(CommandResult {
                    events: vec![HostEvent::PeerSignalRelayed {
                        room_id: envelope.room_id,
                        from_peer_pubkey: state.peer_pubkey_hex.clone(),
                        msg_type,
                        to_peer_pubkey,
                        payload,
                    }],
                })
            }
            HostCommand::CreateTopologyChild {
                parent_room_id,
                child_room_id,
                child_purpose,
                created_by,
                promotion_policy_id,
                parent_checkpoint,
            } => {
                if child_room_id.is_empty()
                    || child_purpose.is_empty()
                    || promotion_policy_id.is_empty()
                {
                    return Err(HostCoreError::InvalidCommand);
                }
                if !self.rooms.contains(&parent_room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }
                self.rooms.insert(child_room_id.clone());
                self.room_maps.entry(child_room_id.clone()).or_default();
                self.room_texts.entry(child_room_id.clone()).or_default();
                self.room_lists.entry(child_room_id.clone()).or_default();
                self.room_blobs.entry(child_room_id.clone()).or_default();
                self.room_presence.entry(child_room_id.clone()).or_default();
                self.room_subscriptions.entry(child_room_id.clone()).or_default();
                self.room_policies.entry(child_room_id.clone()).or_default();
                self.room_sync_graphs.entry(child_room_id.clone()).or_default();
                self.room_conflict_fingerprints
                    .entry(child_room_id.clone())
                    .or_default();
                self.room_conflict_history
                    .entry(child_room_id.clone())
                    .or_default();

                let lineage = serde_json::json!({
                    "parent_room_id": parent_room_id,
                    "parent_checkpoint": parent_checkpoint,
                    "child_purpose": child_purpose,
                    "created_by": created_by,
                    "created_at_hlc": 1,
                    "promotion_policy_id": promotion_policy_id
                });
                self.room_maps
                    .entry(child_room_id.clone())
                    .or_default()
                    .insert("_topology/lineage".to_string(), lineage.clone());
                Ok(CommandResult {
                    events: vec![HostEvent::ChildRoomCreated {
                        child_room_id,
                        lineage,
                    }],
                })
            }
            HostCommand::DescribeRoomLineage { room_id } => {
                if !self.rooms.contains(&room_id) {
                    return Err(HostCoreError::RoomNotFound);
                }
                let lineage = self
                    .room_maps
                    .entry(room_id.clone())
                    .or_default()
                    .get("_topology/lineage")
                    .cloned();
                Ok(CommandResult {
                    events: vec![HostEvent::RoomLineageDescribed {
                        room_id,
                        lineage,
                        ancestors: Vec::new(),
                    }],
                })
            }
            HostCommand::Noop => Ok(CommandResult {
                events: vec![HostEvent::NoopAck],
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerRateLimiterPlan {
    pub enable_node_limiter: bool,
    pub enable_byte_limiter: bool,
}

/// Plan per-peer limiter activation from host inputs.
///
/// Server peer sessions are always exempt from limiter setup.
/// A limiter is enabled only when its configured per-second quota is non-zero.
pub fn plan_peer_rate_limiters(
    is_server_peer: bool,
    peer_rate_nodes: u32,
    peer_rate_bytes: u32,
) -> PeerRateLimiterPlan {
    if is_server_peer {
        return PeerRateLimiterPlan {
            enable_node_limiter: false,
            enable_byte_limiter: false,
        };
    }

    PeerRateLimiterPlan {
        enable_node_limiter: peer_rate_nodes > 0,
        enable_byte_limiter: peer_rate_bytes > 0,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum HelloPayloadClassification {
    ValidHello(Value),
    ProtocolErrorExpectedHello,
}

/// Classify a text handshake payload into either a valid hello message or
/// the adapter's existing protocol-error branch.
pub fn classify_hello_payload(text: &str) -> HelloPayloadClassification {
    match serde_json::from_str::<Value>(text) {
        Ok(v) if v["type"] == "hello" => HelloPayloadClassification::ValidHello(v),
        _ => HelloPayloadClassification::ProtocolErrorExpectedHello,
    }
}

/// Convert an optional verified token expiry into a relative deadline in
/// seconds from "now". `None` indicates auth is disabled for the room.
pub fn plan_token_deadline_remaining_secs(
    verified_expiry_secs: Option<u64>,
    now_unix_secs: u64,
) -> Option<u64> {
    verified_expiry_secs.map(|expiry_secs| expiry_secs.saturating_sub(now_unix_secs))
}

/// Parse `hello.frontier` hex strings into NodeIds, ignoring malformed entries.
pub fn parse_client_frontier_node_ids(hello: &Value) -> Vec<NodeId> {
    hello["frontier"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    let s = v.as_str()?;
                    parse_hex_hash(s)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse `hello.caps` into SyncCapabilities, defaulting when missing/invalid.
pub fn parse_client_capabilities(hello: &Value) -> SyncCapabilities {
    hello
        .get("caps")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

/// Parse `hello.ibf` into the same tri-state classification used by sync diff logic.
pub fn parse_client_ibf_input(hello: &Value) -> ClientIbfInput {
    if let Some(ibf_b64) = hello["ibf"].as_str() {
        match base64_decode(ibf_b64)
            .ok()
            .and_then(|b| nodalmerge_core::Ibf::decode_bytes(&b).ok())
        {
            Some(ibf) => ClientIbfInput::Present(ibf),
            None => ClientIbfInput::InvalidOrUndecodable,
        }
    } else {
        ClientIbfInput::Missing
    }
}

/// Shape `welcome.peers` by excluding the current peer from the connected list.
pub fn shape_welcome_peer_list(
    connected_peers: &[String],
    self_peer_pubkey_hex: &str,
) -> Vec<String> {
    connected_peers
        .iter()
        .filter(|peer| peer.as_str() != self_peer_pubkey_hex)
        .cloned()
        .collect()
}

/// Plan whether a catchup payload should be sent from the computed node count.
pub fn plan_has_catchup(catchup_node_count: usize) -> bool {
    catchup_node_count > 0
}

/// Shape welcome `missing` payload entries from node IDs to hex strings.
pub fn shape_welcome_missing_hex(only_in_client: &[NodeId]) -> Vec<String> {
    only_in_client.iter().map(|id| id.to_hex()).collect()
}

/// Shape welcome `frontier` payload entries from a server frontier.
pub fn shape_welcome_server_frontier_hex(frontier: &Frontier) -> Vec<String> {
    frontier.to_hex_vec()
}

/// Shape welcome `root` payload from the server's merkle root hash.
pub fn shape_welcome_root_hex(root: &Hash) -> String {
    root.to_hex()
}

/// Shape catchup pack payload by postcard-packing nodes then base64-encoding.
pub fn shape_catchup_pack_payload_b64(nodes: &[&SyncNode]) -> String {
    let packed = pack_nodes(nodes);
    base64_encode(&packed)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WelcomeCatchupPackage {
    pub welcome_json: String,
    pub catchup_b64: String,
    pub has_catchup: bool,
}

/// Assemble the ws-ready welcome json and paired catchup tuple.
pub fn assemble_welcome_catchup_package(
    root_hex: String,
    frontier_hex: Vec<String>,
    missing_hex: Vec<String>,
    negotiated_caps: SyncCapabilities,
    server_pubkey_hex: String,
    current_peers: Vec<String>,
    mst_root_hex: Option<String>,
    catchup_b64: String,
    has_catchup: bool,
) -> WelcomeCatchupPackage {
    let welcome = assemble_welcome_envelope(
        root_hex,
        frontier_hex,
        missing_hex,
        negotiated_caps,
        server_pubkey_hex,
        current_peers,
        mst_root_hex,
    );
    let welcome_json = serde_json::to_string(&welcome)
        .expect("welcome envelope should always serialize");

    WelcomeCatchupPackage {
        welcome_json,
        catchup_b64,
        has_catchup,
    }
}

/// Serialize a catchup payload into the ws `pack` envelope JSON.
pub fn serialize_catchup_pack_envelope_json(catchup_b64: String) -> String {
    let catchup_env = assemble_catchup_pack_envelope(catchup_b64);
    serde_json::to_string(&catchup_env)
        .expect("catchup pack envelope should always serialize")
}

/// Decide whether filtered catchup payload should be sent.
///
/// `None` means the filter dropped the payload and no ws send should occur.
pub fn plan_filtered_catchup_send_payload(filtered_env: Option<String>) -> Option<String> {
    filtered_env
}

/// Decide whether a filtered catchup send failure should terminate the session.
pub fn should_terminate_after_filtered_catchup_send(send_ok: bool) -> bool {
    !send_ok
}

/// Decide whether a welcome send failure should terminate the session.
pub fn should_terminate_after_welcome_send(send_ok: bool) -> bool {
    !send_ok
}

/// Decide whether a failed main-loop push send should break the connection loop.
pub fn should_break_main_loop_after_push_send(send_ok: bool) -> bool {
    !send_ok
}

/// Decide whether a failed per-message reply send should terminate handling.
pub fn should_terminate_after_reply_send(send_ok: bool) -> bool {
    !send_ok
}

/// Decide whether a failed stop-tick reply send should terminate handling.
pub fn should_terminate_after_stop_tick_reply_send(send_ok: bool) -> bool {
    !send_ok
}

/// Decide whether a failed compact-room ack send should terminate handling.
pub fn should_terminate_after_compact_room_ack_send(send_ok: bool) -> bool {
    !send_ok
}

/// Decide whether a failed set-room-key room-locked ack send should
/// terminate handling.
pub fn should_terminate_after_set_room_key_locked_ack_send(send_ok: bool) -> bool {
    !send_ok
}

/// Decide whether a failed start-tick reply send should terminate handling.
pub fn should_terminate_after_start_tick_reply_send(send_ok: bool) -> bool {
    !send_ok
}

/// Decide whether a failed server-info reply send should terminate handling.
pub fn should_terminate_after_server_info_reply_send(send_ok: bool) -> bool {
    !send_ok
}

/// Decide whether a failed set-policy reply send should terminate handling.
pub fn should_terminate_after_set_policy_reply_send(send_ok: bool) -> bool {
    !send_ok
}

/// Decide whether a failed subscribe-ack reply send should terminate handling.
pub fn should_terminate_after_subscribe_ack_send(send_ok: bool) -> bool {
    !send_ok
}

/// Decide whether a failed mst-request reply send should terminate handling.
pub fn should_terminate_after_mst_request_reply_send(send_ok: bool) -> bool {
    !send_ok
}

/// Decide whether a failed mst-done server-pack reply send should terminate
/// handling.
pub fn should_terminate_after_mst_done_server_pack_send(send_ok: bool) -> bool {
    !send_ok
}

/// Decide whether a failed request server-pack reply send should terminate
/// handling.
pub fn should_terminate_after_request_server_pack_send(send_ok: bool) -> bool {
    !send_ok
}

/// Decide whether a failed blob-request redirect reply send should terminate
/// handling.
pub fn should_terminate_after_blob_request_redirect_reply_send(send_ok: bool) -> bool {
    !send_ok
}

/// Decide whether a failed blob-request blob-pack reply send should terminate
/// handling.
pub fn should_terminate_after_blob_request_blob_pack_reply_send(send_ok: bool) -> bool {
    !send_ok
}

/// Decide whether a failed request-upload reply send should terminate
/// handling.
pub fn should_terminate_after_request_upload_reply_send(send_ok: bool) -> bool {
    !send_ok
}

/// Decide whether a blob-upload verify-failure reply send failure should
/// terminate handling.
pub fn should_terminate_after_blob_upload_verify_failure_reply_send(send_ok: bool) -> bool {
    !send_ok
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientJsonErrorHandlingPlan {
    pub error_message: &'static str,
    pub keep_connection_open: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClientMessageParseClassification {
    Parsed(Value),
    MalformedJson(ClientJsonErrorHandlingPlan),
}

/// Classify a client message parse attempt into parsed payload or a
/// deterministic malformed-json handling plan.
pub fn classify_client_message_json(text: &str) -> ClientMessageParseClassification {
    match serde_json::from_str::<Value>(text) {
        Ok(v) => ClientMessageParseClassification::Parsed(v),
        Err(_) => ClientMessageParseClassification::MalformedJson(ClientJsonErrorHandlingPlan {
            error_message: "invalid JSON",
            keep_connection_open: true,
        }),
    }
}

/// Decide whether an unknown client message type should be ignored.
pub fn should_ignore_unknown_client_message_type(_message_type: &str) -> bool {
    true
}

/// Decide whether a room broadcast envelope should be skipped for the current
/// peer because it is a self-echo.
pub fn should_skip_self_echo_broadcast_envelope(
    envelope_json: &str,
    self_peer_pubkey_hex: &str,
) -> bool {
    serde_json::from_str::<Value>(envelope_json)
        .ok()
        .and_then(|v| v["from"].as_str().map(str::to_string))
        .is_some_and(|from| from == self_peer_pubkey_hex)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerCountGaugeUpdate {
    Increment,
    Decrement,
    NoChange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegisterPeerMembershipPlan {
    pub clear_idle_since: bool,
    pub gauge_update: PeerCountGaugeUpdate,
}

/// Plan register-peer side effects for idle clock and peer-count gauge.
pub fn plan_register_peer_membership(inserted: bool) -> RegisterPeerMembershipPlan {
    RegisterPeerMembershipPlan {
        clear_idle_since: true,
        gauge_update: if inserted {
            PeerCountGaugeUpdate::Increment
        } else {
            PeerCountGaugeUpdate::NoChange
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeregisterPeerMembershipPlan {
    pub set_idle_since_now: bool,
    pub gauge_update: PeerCountGaugeUpdate,
}

/// Plan deregister-peer side effects for idle clock and peer-count gauge.
pub fn plan_deregister_peer_membership(
    removed: bool,
    peers_empty_after: bool,
) -> DeregisterPeerMembershipPlan {
    DeregisterPeerMembershipPlan {
        set_idle_since_now: peers_empty_after,
        gauge_update: if removed {
            PeerCountGaugeUpdate::Decrement
        } else {
            PeerCountGaugeUpdate::NoChange
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackImportMutationAction {
    BroadcastAcceptedLeafPack,
    NoBroadcast,
}

/// Plan post-import room mutation behavior for an inbound pack.
pub fn plan_pack_import_mutation(accepted_nodes: usize) -> PackImportMutationAction {
    if accepted_nodes > 0 {
        PackImportMutationAction::BroadcastAcceptedLeafPack
    } else {
        PackImportMutationAction::NoBroadcast
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobStoreMutationAction {
    BroadcastBlobAvailable,
    NoBroadcast,
}

/// Plan post-store room mutation behavior for inbound blob upload batches.
pub fn plan_blob_store_mutation(stored_blobs: usize) -> BlobStoreMutationAction {
    if stored_blobs > 0 {
        BlobStoreMutationAction::BroadcastBlobAvailable
    } else {
        BlobStoreMutationAction::NoBroadcast
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebRtcRelayBranchClassification {
    Relay,
    NotRelay,
}

/// Classify whether a client message type should route through the WebRTC
/// signaling relay branch.
pub fn classify_webrtc_relay_branch(message_type: &str) -> WebRtcRelayBranchClassification {
    match message_type {
        "webrtc-offer" | "webrtc-answer" | "webrtc-ice" => {
            WebRtcRelayBranchClassification::Relay
        }
        _ => WebRtcRelayBranchClassification::NotRelay,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectBlobIoNegotiationGatePlan {
    pub error_message: &'static str,
    pub keep_connection_open: bool,
}

/// Plan behavior when handling a direct-blob-io message type whose capability
/// may not have been negotiated.
pub fn plan_direct_blob_io_negotiation_gate(
    supports_direct_blob_io: bool,
    message_type: &str,
) -> Option<DirectBlobIoNegotiationGatePlan> {
    if supports_direct_blob_io {
        return None;
    }

    let error_message = match message_type {
        "request-upload" => "request-upload not negotiated",
        "blob-uploaded" => "blob-uploaded not negotiated",
        _ => "direct-blob-io not negotiated",
    };

    Some(DirectBlobIoNegotiationGatePlan {
        error_message,
        keep_connection_open: true,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectBlobIoBadHashGatePlan {
    pub error_message: &'static str,
    pub keep_connection_open: bool,
}

/// Plan behavior when a direct-blob-io message contains an invalid hash.
pub fn plan_direct_blob_io_bad_hash_gate(
    hash_is_valid: bool,
    message_type: &str,
) -> Option<DirectBlobIoBadHashGatePlan> {
    if hash_is_valid {
        return None;
    }

    let error_message = match message_type {
        "request-upload" => "request-upload: bad hash",
        "blob-uploaded" => "blob-uploaded: bad hash",
        _ => "direct-blob-io: bad hash",
    };

    Some(DirectBlobIoBadHashGatePlan {
        error_message,
        keep_connection_open: true,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectBlobIoVerifyOutcome {
    BroadcastAvailable,
    SendUploadRejected,
}

/// Classify direct-blob-io verify result into deterministic branch intent.
pub fn classify_direct_blob_io_verify_outcome(
    verify_succeeded: bool,
) -> DirectBlobIoVerifyOutcome {
    if verify_succeeded {
        DirectBlobIoVerifyOutcome::BroadcastAvailable
    } else {
        DirectBlobIoVerifyOutcome::SendUploadRejected
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestUploadResolvePutUrlOutcome {
    GrantUpload,
    DenyUseWs,
}

/// Classify request-upload URL resolution into deterministic branch intent.
pub fn classify_request_upload_resolve_put_url_outcome(
    put_url_available: bool,
) -> RequestUploadResolvePutUrlOutcome {
    if put_url_available {
        RequestUploadResolvePutUrlOutcome::GrantUpload
    } else {
        RequestUploadResolvePutUrlOutcome::DenyUseWs
    }
}

/// Shape blob-available hash list for a successful blob-uploaded verify path.
pub fn shape_blob_uploaded_available_hashes(hash_hex: &str) -> Vec<String> {
    vec![hash_hex.to_string()]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobUploadedVerifyFailureRejectionPayload {
    pub hash: String,
    pub reason: String,
}

/// Shape upload-rejected payload fields for blob-uploaded verify failures.
pub fn shape_blob_uploaded_verify_failure_rejection_payload(
    hash_hex: &str,
    reason: &str,
) -> BlobUploadedVerifyFailureRejectionPayload {
    BlobUploadedVerifyFailureRejectionPayload {
        hash: hash_hex.to_string(),
        reason: reason.to_string(),
    }
}

/// Extract blob-uploaded hash text with adapter-compatible defaulting.
pub fn extract_blob_uploaded_hash_text(message: &Value) -> String {
    message["hash"].as_str().unwrap_or("").to_string()
}

/// Extract request-upload hash text with adapter-compatible defaulting.
pub fn extract_request_upload_hash_text(message: &Value) -> String {
    message["hash"].as_str().unwrap_or("").to_string()
}

/// Extract request-upload size with adapter-compatible defaulting.
pub fn extract_request_upload_size(message: &Value) -> u64 {
    message["size"].as_u64().unwrap_or(0)
}

/// Extract request-upload content type when present.
pub fn extract_request_upload_content_type(message: &Value) -> Option<String> {
    message["content_type"].as_str().map(|s| s.to_string())
}

/// Extract hello pubkey text with adapter-compatible defaulting.
pub fn extract_hello_pubkey_text(hello: &Value) -> String {
    hello["pubkey"].as_str().unwrap_or("").to_string()
}

/// Extract client message type text with adapter-compatible defaulting.
pub fn extract_client_message_type_text(message: &Value) -> String {
    message["type"].as_str().unwrap_or("").to_string()
}

/// Extract presence payload data with adapter-compatible defaulting.
pub fn extract_presence_data_payload(message: &Value) -> Value {
    message["data"].clone()
}

/// Extract pack nodes payload text with adapter-compatible defaulting.
pub fn extract_pack_nodes_payload_b64(message: &Value) -> String {
    message["nodes"].as_str().unwrap_or("").to_string()
}

/// Extract blob-upload entry hash text with adapter-compatible defaulting.
pub fn extract_blob_upload_entry_hash_text(entry: &Value) -> String {
    entry["hash"].as_str().unwrap_or("").to_string()
}

/// Extract blob-upload entry data payload text with adapter-compatible defaulting.
pub fn extract_blob_upload_entry_data_b64(entry: &Value) -> String {
    entry["data"].as_str().unwrap_or("").to_string()
}

/// Extract set-policy default text with adapter-compatible defaulting.
pub fn extract_set_policy_default_text(message: &Value) -> String {
    message["default"].as_str().unwrap_or("allow").to_string()
}

/// Extract set-policy rule entries with adapter-compatible defaulting.
pub fn extract_set_policy_rule_values(message: &Value) -> Vec<Value> {
    message["rules"].as_array().cloned().unwrap_or_default()
}

/// Extract set-policy can_write entries with adapter-compatible defaulting.
pub fn extract_set_policy_can_write_values(rule: &Value) -> Vec<Value> {
    rule["can_write"].as_array().cloned().unwrap_or_default()
}

/// Extract a set-policy can_write pubkey hex text with adapter-compatible defaulting.
pub fn extract_set_policy_can_write_hex_text(value: &Value) -> String {
    value.as_str().unwrap_or("").to_string()
}

/// Extract blob-request hashes with adapter-compatible defaulting.
pub fn extract_blob_request_hashes(message: &Value) -> Vec<String> {
    message["hashes"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Extract WebRTC relay fields from a client message with adapter-compatible
/// defaulting for non-object payloads.
pub fn extract_webrtc_relay_fields(message: &Value) -> Map<String, Value> {
    message.as_object().cloned().unwrap_or_default()
}

/// Extract mst-request paths with adapter-compatible defaulting.
pub fn extract_mst_request_paths(message: &Value) -> Vec<String> {
    message["paths"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Extract mst-done node IDs by parsing hex strings and filtering invalid values.
pub fn extract_mst_done_ids(message: &Value) -> Vec<NodeId> {
    message["ids"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| parse_hex_hash(v.as_str()?))
                .collect()
        })
        .unwrap_or_default()
}

/// Shape request `known` IDs into a hash set for missing-hash diffing.
pub fn shape_request_known_id_set(message: &Value) -> HashSet<NodeId> {
    message["known"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| parse_hex_hash(v.as_str()?))
                .collect()
        })
        .unwrap_or_default()
}

/// Shape client-known IDs into a hash set for graph diff input.
pub fn shape_client_known_id_set(client_known: &[NodeId]) -> HashSet<NodeId> {
    client_known.iter().copied().collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetRoomKeyParseResult {
    Parsed,
    InvalidPubkeyHex,
}

/// Classify set-room-key pubkey parse result into deterministic branch intent.
pub fn classify_set_room_key_parse_result(parse_succeeded: bool) -> SetRoomKeyParseResult {
    if parse_succeeded {
        SetRoomKeyParseResult::Parsed
    } else {
        SetRoomKeyParseResult::InvalidPubkeyHex
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetRoomKeyLockStateResult {
    LockRoom,
    RejectAlreadyLocked,
}

/// Classify set-room-key lock-state check into deterministic branch intent.
pub fn classify_set_room_key_lock_state(can_lock_room: bool) -> SetRoomKeyLockStateResult {
    if can_lock_room {
        SetRoomKeyLockStateResult::LockRoom
    } else {
        SetRoomKeyLockStateResult::RejectAlreadyLocked
    }
}

/// Shape deterministic rejection reason for invalid set-room-key pubkey.
pub fn shape_set_room_key_invalid_pubkey_rejection_reason() -> &'static str {
    "set-room-key: invalid pubkey hex"
}

/// Shape deterministic rejection reason for already-locked set-room-key room.
pub fn shape_set_room_key_already_locked_rejection_reason() -> &'static str {
    "room already locked"
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetPolicyParseResult {
    ApplyPolicy,
    ParseFailed,
}

/// Classify set-policy parse output into deterministic branch intent.
pub fn classify_set_policy_parse_result(policy_is_some: bool) -> SetPolicyParseResult {
    if policy_is_some {
        SetPolicyParseResult::ApplyPolicy
    } else {
        SetPolicyParseResult::ParseFailed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetPolicyDefaultParseResult {
    DenyAll,
    AllowAll,
    Unknown,
}

/// Classify set-policy default string into deterministic parse outcome.
pub fn classify_set_policy_default(default_value: &str) -> SetPolicyDefaultParseResult {
    match default_value {
        "deny" => SetPolicyDefaultParseResult::DenyAll,
        "allow" => SetPolicyDefaultParseResult::AllowAll,
        _ => SetPolicyDefaultParseResult::Unknown,
    }
}

/// Shape deterministic unknown-default parse error text for set-policy.
pub fn shape_set_policy_unknown_default_error(default_value: &str) -> String {
    format!(
        "set-policy: unknown default '{default_value}', use 'allow' or 'deny'"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactRoomCompactionResult {
    UseSnapshot,
    SendCompactFailed,
}

/// Classify compact-room compaction result into deterministic branch intent.
pub fn classify_compact_room_compaction_result(
    compaction_succeeded: bool,
) -> CompactRoomCompactionResult {
    if compaction_succeeded {
        CompactRoomCompactionResult::UseSnapshot
    } else {
        CompactRoomCompactionResult::SendCompactFailed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactRoomVerifyResult {
    UseVerifiedSnapshot,
    SendVerifyFailed,
}

/// Classify compact-room snapshot verify result into deterministic branch intent.
pub fn classify_compact_room_verify_result(verify_succeeded: bool) -> CompactRoomVerifyResult {
    if verify_succeeded {
        CompactRoomVerifyResult::UseVerifiedSnapshot
    } else {
        CompactRoomVerifyResult::SendVerifyFailed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactRoomRebuildResult {
    InstallRebuiltGraph,
    SendRebuildFailed,
}

/// Classify compact-room rebuild result into deterministic branch intent.
pub fn classify_compact_room_rebuild_result(rebuild_succeeded: bool) -> CompactRoomRebuildResult {
    if rebuild_succeeded {
        CompactRoomRebuildResult::InstallRebuiltGraph
    } else {
        CompactRoomRebuildResult::SendRebuildFailed
    }
}

/// Shape compact-room compaction failure error text.
pub fn shape_compact_room_compaction_failed_error(reason: &str) -> String {
    format!("compact failed: {reason}")
}

/// Shape compact-room verify failure error text.
pub fn shape_compact_room_verify_failed_error(reason: &str) -> String {
    format!("snapshot verify failed: {reason}")
}

/// Shape compact-room rebuild failure error text.
pub fn shape_compact_room_rebuild_failed_error(reason: &str) -> String {
    format!("rebuild failed: {reason}")
}

/// Extract set-room-key pubkey text with adapter-compatible defaulting.
pub fn extract_set_room_key_pubkey_text(message: &Value) -> String {
    message["pubkey"].as_str().unwrap_or("").to_string()
}

/// Extract start-tick interval with adapter-compatible defaulting/clamp.
pub fn extract_start_tick_interval_ms(message: &Value) -> u64 {
    message["interval_ms"].as_u64().unwrap_or(16).max(1)
}

/// Extract start-tick intent prefix with adapter-compatible defaulting.
pub fn extract_start_tick_intent_prefix(message: &Value) -> String {
    message["intent_prefix"]
        .as_str()
        .unwrap_or("intent/")
        .to_string()
}

fn parse_hex_hash(hex: &str) -> Option<NodeId> {
    if hex.len() != 64 {
        return None;
    }

    let mut bytes = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let hi = hex_nibble(chunk[0])?;
        let lo = hex_nibble(chunk[1])?;
        bytes[i] = (hi << 4) | lo;
    }

    Some(nodalmerge_core::Hash(bytes))
}

fn parse_hex_32(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }

    let mut bytes = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let hi = hex_nibble(chunk[0])?;
        let lo = hex_nibble(chunk[1])?;
        bytes[i] = (hi << 4) | lo;
    }

    Some(bytes)
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
        if b0 == b'=' {
            break;
        }
        let v0 = *TABLE.get(b0 as usize).ok_or(())? as u32;
        let v1 = *TABLE.get(b1 as usize).ok_or(())? as u32;
        if v0 == 0xff || v1 == 0xff {
            return Err(());
        }
        out.push(((v0 << 2) | (v1 >> 4)) as u8);

        let b2 = bytes.get(i + 2).copied().unwrap_or(b'=');
        if b2 != b'=' {
            let v2 = *TABLE.get(b2 as usize).ok_or(())? as u32;
            if v2 == 0xff {
                return Err(());
            }
            out.push((((v1 & 0x0f) << 4) | (v2 >> 2)) as u8);

            let b3 = bytes.get(i + 3).copied().unwrap_or(b'=');
            if b3 != b'=' {
                let v3 = *TABLE.get(b3 as usize).ok_or(())? as u32;
                if v3 == 0xff {
                    return Err(());
                }
                out.push((((v2 & 0x03) << 6) | v3) as u8);
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

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_peer_rate_limiters_exempts_server_peer() {
        let plan = plan_peer_rate_limiters(true, 100, 200);
        assert!(!plan.enable_node_limiter);
        assert!(!plan.enable_byte_limiter);
    }

    #[test]
    fn plan_peer_rate_limiters_enables_non_zero_non_server_limits() {
        let plan = plan_peer_rate_limiters(false, 10, 20);
        assert!(plan.enable_node_limiter);
        assert!(plan.enable_byte_limiter);
    }

    #[test]
    fn plan_peer_rate_limiters_disables_zero_limits() {
        let plan = plan_peer_rate_limiters(false, 0, 42);
        assert!(!plan.enable_node_limiter);
        assert!(plan.enable_byte_limiter);
    }

    #[test]
    fn classify_hello_payload_accepts_valid_hello() {
        let text = r#"{"type":"hello","pubkey":"abc"}"#;
        let result = classify_hello_payload(text);
        match result {
            HelloPayloadClassification::ValidHello(v) => {
                assert_eq!(v["type"], "hello");
            }
            HelloPayloadClassification::ProtocolErrorExpectedHello => {
                panic!("expected valid hello classification");
            }
        }
    }

    #[test]
    fn classify_hello_payload_rejects_non_hello_type() {
        let text = r#"{"type":"request"}"#;
        let result = classify_hello_payload(text);
        assert_eq!(
            result,
            HelloPayloadClassification::ProtocolErrorExpectedHello
        );
    }

    #[test]
    fn classify_hello_payload_rejects_invalid_json() {
        let text = "{not-json";
        let result = classify_hello_payload(text);
        assert_eq!(
            result,
            HelloPayloadClassification::ProtocolErrorExpectedHello
        );
    }

    #[test]
    fn plan_token_deadline_remaining_secs_handles_auth_disabled() {
        let plan = plan_token_deadline_remaining_secs(None, 100);
        assert_eq!(plan, None);
    }

    #[test]
    fn plan_token_deadline_remaining_secs_computes_remaining() {
        let plan = plan_token_deadline_remaining_secs(Some(130), 100);
        assert_eq!(plan, Some(30));
    }

    #[test]
    fn plan_token_deadline_remaining_secs_saturates_at_zero() {
        let plan = plan_token_deadline_remaining_secs(Some(90), 100);
        assert_eq!(plan, Some(0));
    }

    #[test]
    fn parse_client_frontier_node_ids_filters_invalid_entries() {
        let valid_hex = "0101010101010101010101010101010101010101010101010101010101010101";
        let hello = serde_json::json!({
            "frontier": [
                valid_hex,
                "not-a-hash",
                123
            ]
        });

        let parsed = parse_client_frontier_node_ids(&hello);
        assert_eq!(parsed.len(), 1);
    }

    #[test]
    fn parse_client_frontier_node_ids_handles_missing_frontier() {
        let hello = serde_json::json!({ "type": "hello" });
        let parsed = parse_client_frontier_node_ids(&hello);
        assert!(parsed.is_empty());
    }

    #[test]
    fn parse_client_capabilities_defaults_when_missing() {
        let hello = serde_json::json!({ "type": "hello" });
        let caps = parse_client_capabilities(&hello);
        assert_eq!(caps, SyncCapabilities::default());
    }

    #[test]
    fn parse_client_capabilities_parses_valid_caps() {
        let hello = serde_json::json!({
            "type": "hello",
            "caps": {
                "supports_ibf": false,
                "supports_mst": false,
                "supports_postcard": true,
                "supports_encryption": true,
                "supports_webrtc": true,
                "max_tick_interval_ms": 250,
                "supports_list_crdt": true,
                "supports_direct_blob_io": false
            }
        });

        let caps = parse_client_capabilities(&hello);
        assert!(!caps.supports_ibf);
        assert!(!caps.supports_mst);
        assert!(caps.supports_webrtc);
        assert_eq!(caps.max_tick_interval_ms, Some(250));
        assert!(!caps.supports_direct_blob_io);
    }

    #[test]
    fn parse_client_capabilities_defaults_when_invalid_shape() {
        let hello = serde_json::json!({
            "type": "hello",
            "caps": "not-an-object"
        });

        let caps = parse_client_capabilities(&hello);
        assert_eq!(caps, SyncCapabilities::default());
    }

    #[test]
    fn parse_client_ibf_input_missing_when_field_absent() {
        let hello = serde_json::json!({ "type": "hello" });
        let parsed = parse_client_ibf_input(&hello);
        match parsed {
            ClientIbfInput::Missing => {}
            _ => panic!("expected missing ibf classification"),
        }
    }

    #[test]
    fn parse_client_ibf_input_invalid_when_base64_is_invalid() {
        let hello = serde_json::json!({
            "type": "hello",
            "ibf": "$$$not-base64$$$"
        });
        let parsed = parse_client_ibf_input(&hello);
        match parsed {
            ClientIbfInput::InvalidOrUndecodable => {}
            _ => panic!("expected invalid/undecodable ibf classification"),
        }
    }

    #[test]
    fn parse_client_ibf_input_invalid_when_payload_not_ibf() {
        let hello = serde_json::json!({
            "type": "hello",
            "ibf": "AQID"
        });
        let parsed = parse_client_ibf_input(&hello);
        match parsed {
            ClientIbfInput::InvalidOrUndecodable => {}
            _ => panic!("expected invalid/undecodable ibf classification"),
        }
    }

    #[test]
    fn parse_client_ibf_input_present_when_payload_decodes() {
        let ibf_bytes = nodalmerge_core::Ibf::new().encode();
        let ibf_b64 = base64_encode_for_test(&ibf_bytes);
        let hello = serde_json::json!({
            "type": "hello",
            "ibf": ibf_b64
        });
        let parsed = parse_client_ibf_input(&hello);
        match parsed {
            ClientIbfInput::Present(_) => {}
            _ => panic!("expected present ibf classification"),
        }
    }

    #[test]
    fn shape_welcome_peer_list_excludes_self() {
        let peers = vec![
            "peer-a".to_string(),
            "peer-self".to_string(),
            "peer-b".to_string(),
        ];

        let shaped = shape_welcome_peer_list(&peers, "peer-self");
        assert_eq!(
            shaped,
            vec!["peer-a".to_string(), "peer-b".to_string()]
        );
    }

    #[test]
    fn shape_welcome_peer_list_preserves_order_when_self_absent() {
        let peers = vec!["peer-a".to_string(), "peer-b".to_string()];

        let shaped = shape_welcome_peer_list(&peers, "peer-self");
        assert_eq!(
            shaped,
            vec!["peer-a".to_string(), "peer-b".to_string()]
        );
    }

    #[test]
    fn plan_has_catchup_false_when_empty() {
        assert!(!plan_has_catchup(0));
    }

    #[test]
    fn plan_has_catchup_true_when_non_empty() {
        assert!(plan_has_catchup(1));
    }

    #[test]
    fn shape_welcome_missing_hex_converts_ids_to_hex() {
        let ids = vec![
            nodalmerge_core::Hash([0x01; 32]),
            nodalmerge_core::Hash([0xab; 32]),
        ];

        let missing = shape_welcome_missing_hex(&ids);
        assert_eq!(missing.len(), 2);
        assert_eq!(missing[0], "0101010101010101010101010101010101010101010101010101010101010101");
        assert_eq!(missing[1], "abababababababababababababababababababababababababababababababab");
    }

    #[test]
    fn shape_welcome_missing_hex_handles_empty_input() {
        let missing = shape_welcome_missing_hex(&[]);
        assert!(missing.is_empty());
    }

    #[test]
    fn shape_welcome_server_frontier_hex_converts_frontier_heads() {
        let frontier = nodalmerge_core::Frontier::from_heads(vec![
            nodalmerge_core::Hash([0x11; 32]),
            nodalmerge_core::Hash([0x22; 32]),
        ]);

        let hexes = shape_welcome_server_frontier_hex(&frontier);
        assert_eq!(hexes.len(), 2);
        assert_eq!(hexes[0], "1111111111111111111111111111111111111111111111111111111111111111");
        assert_eq!(hexes[1], "2222222222222222222222222222222222222222222222222222222222222222");
    }

    #[test]
    fn shape_welcome_server_frontier_hex_handles_empty_frontier() {
        let frontier = nodalmerge_core::Frontier::default();
        let hexes = shape_welcome_server_frontier_hex(&frontier);
        assert!(hexes.is_empty());
    }

    #[test]
    fn shape_welcome_root_hex_converts_hash_to_hex() {
        let root = nodalmerge_core::Hash([0xcd; 32]);
        let hex = shape_welcome_root_hex(&root);
        assert_eq!(hex, "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd");
    }

    #[test]
    fn shape_catchup_pack_payload_b64_empty_nodes_encodes_empty_pack() {
        let nodes: Vec<&nodalmerge_core::SyncNode> = Vec::new();
        let payload = shape_catchup_pack_payload_b64(&nodes);
        let decoded = base64_decode(&payload).expect("catchup payload should base64 decode");
        let unpacked = nodalmerge_core::unpack_nodes(&decoded)
            .expect("decoded catchup payload should unpack into nodes");
        assert!(unpacked.is_empty());
    }

    #[test]
    fn shape_catchup_pack_payload_b64_roundtrips_via_decode_and_unpack() {
        let tx = nodalmerge_core::Transaction {
            author: [0u8; 32],
            lamport: 1,
            wall_ms: 1,
            ops: vec![nodalmerge_core::Op::Map(nodalmerge_core::MapOp::Set {
                key: "k".to_string(),
                value: b"v".to_vec(),
            })],
            parents: vec![],
        };
        let node = nodalmerge_core::SyncNode::new(tx);
        let nodes = vec![&node];

        let payload = shape_catchup_pack_payload_b64(&nodes);
        let decoded = base64_decode(&payload).expect("catchup payload should base64 decode");
        let unpacked = nodalmerge_core::unpack_nodes(&decoded)
            .expect("decoded catchup payload should unpack into nodes");
        assert_eq!(unpacked.len(), 1);
        assert_eq!(unpacked[0].id, node.id);
    }

    #[test]
    fn assemble_welcome_catchup_package_sets_expected_tuple_fields() {
        let pkg = assemble_welcome_catchup_package(
            "roothex".to_string(),
            vec!["frontierhex".to_string()],
            vec!["missinghex".to_string()],
            SyncCapabilities::default(),
            "server-pubkey".to_string(),
            vec!["peer-a".to_string()],
            None,
            "catchup-b64".to_string(),
            true,
        );

        let parsed: serde_json::Value = serde_json::from_str(&pkg.welcome_json)
            .expect("welcome_json should parse as valid json");
        assert_eq!(parsed["type"], "welcome");
        assert_eq!(parsed["root"], "roothex");
        assert_eq!(pkg.catchup_b64, "catchup-b64");
        assert!(pkg.has_catchup);
    }

    #[test]
    fn serialize_catchup_pack_envelope_json_sets_type_and_data() {
        let json = serialize_catchup_pack_envelope_json("payload-b64".to_string());
        let parsed: serde_json::Value = serde_json::from_str(&json)
            .expect("catchup json should parse as valid json");
        assert_eq!(parsed["type"], "pack");
        assert_eq!(parsed["nodes"], "payload-b64");
    }

    #[test]
    fn filter_pack_envelope_by_subscription_keeps_non_pack_messages() {
        let message = r#"{"type":"welcome","room":"room-a"}"#;
        let filtered = filter_pack_envelope_by_subscription(
            message,
            &["world/**".to_string()],
        );
        assert_eq!(filtered, Some(message.to_string()));
    }

    #[test]
    fn filter_pack_envelope_by_subscription_drops_pack_when_no_nodes_match() {
        let tx = nodalmerge_core::Transaction {
            author: [0u8; 32],
            lamport: 1,
            wall_ms: 1,
            ops: vec![nodalmerge_core::Op::Map(nodalmerge_core::MapOp::Set {
                key: "other/path".to_string(),
                value: b"v".to_vec(),
            })],
            parents: vec![],
        };
        let node = nodalmerge_core::SyncNode::new(tx);
        let nodes_b64 = base64_encode(&pack_nodes(&[&node]));
        let env = serde_json::json!({
            "type": "pack",
            "from": "peer-a",
            "nodes": nodes_b64,
        })
        .to_string();

        let filtered = filter_pack_envelope_by_subscription(
            &env,
            &["world/**".to_string()],
        );
        assert_eq!(filtered, None);
    }

    #[test]
    fn filter_pack_envelope_by_subscription_keeps_matching_and_sentinel_nodes() {
        let world_tx = nodalmerge_core::Transaction {
            author: [0u8; 32],
            lamport: 1,
            wall_ms: 1,
            ops: vec![nodalmerge_core::Op::Map(nodalmerge_core::MapOp::Set {
                key: "world/player1".to_string(),
                value: b"v1".to_vec(),
            })],
            parents: vec![],
        };
        let sentinel_tx = nodalmerge_core::Transaction {
            author: [0u8; 32],
            lamport: 2,
            wall_ms: 2,
            ops: vec![nodalmerge_core::Op::Map(nodalmerge_core::MapOp::Set {
                key: "\u{0000}meta".to_string(),
                value: b"s".to_vec(),
            })],
            parents: vec![],
        };
        let other_tx = nodalmerge_core::Transaction {
            author: [0u8; 32],
            lamport: 3,
            wall_ms: 3,
            ops: vec![nodalmerge_core::Op::Map(nodalmerge_core::MapOp::Set {
                key: "chat/general".to_string(),
                value: b"x".to_vec(),
            })],
            parents: vec![],
        };

        let world_node = nodalmerge_core::SyncNode::new(world_tx);
        let sentinel_node = nodalmerge_core::SyncNode::new(sentinel_tx);
        let other_node = nodalmerge_core::SyncNode::new(other_tx);
        let nodes_b64 = base64_encode(&pack_nodes(&[&world_node, &sentinel_node, &other_node]));

        let env = serde_json::json!({
            "type": "pack",
            "from": "peer-a",
            "nodes": nodes_b64,
        })
        .to_string();

        let filtered = filter_pack_envelope_by_subscription(
            &env,
            &["world/**".to_string()],
        )
        .expect("expected filtered pack output");

        let filtered_json: serde_json::Value = serde_json::from_str(&filtered)
            .expect("filtered payload should parse");
        let filtered_nodes_b64 = filtered_json["nodes"]
            .as_str()
            .expect("filtered pack must contain nodes b64");
        let filtered_nodes = nodalmerge_core::unpack_nodes(
            &base64_decode(filtered_nodes_b64).expect("nodes should base64 decode"),
        )
        .expect("nodes should unpack");

        assert_eq!(filtered_nodes.len(), 2);
        assert_eq!(filtered_nodes[0].id, world_node.id);
        assert_eq!(filtered_nodes[1].id, sentinel_node.id);
    }

    #[test]
    fn plan_filtered_catchup_send_payload_passes_through_payload() {
        let planned = plan_filtered_catchup_send_payload(Some("payload".to_string()));
        assert_eq!(planned, Some("payload".to_string()));
    }

    #[test]
    fn plan_filtered_catchup_send_payload_drops_when_none() {
        let planned = plan_filtered_catchup_send_payload(None);
        assert_eq!(planned, None);
    }

    #[test]
    fn should_terminate_after_filtered_catchup_send_true_on_send_failure() {
        assert!(should_terminate_after_filtered_catchup_send(false));
    }

    #[test]
    fn should_terminate_after_filtered_catchup_send_false_on_send_success() {
        assert!(!should_terminate_after_filtered_catchup_send(true));
    }

    #[test]
    fn should_terminate_after_welcome_send_true_on_send_failure() {
        assert!(should_terminate_after_welcome_send(false));
    }

    #[test]
    fn should_terminate_after_welcome_send_false_on_send_success() {
        assert!(!should_terminate_after_welcome_send(true));
    }

    #[test]
    fn should_break_main_loop_after_push_send_true_on_send_failure() {
        assert!(should_break_main_loop_after_push_send(false));
    }

    #[test]
    fn should_break_main_loop_after_push_send_false_on_send_success() {
        assert!(!should_break_main_loop_after_push_send(true));
    }

    #[test]
    fn should_terminate_after_reply_send_true_on_send_failure() {
        assert!(should_terminate_after_reply_send(false));
    }

    #[test]
    fn should_terminate_after_reply_send_false_on_send_success() {
        assert!(!should_terminate_after_reply_send(true));
    }

    #[test]
    fn should_terminate_after_blob_upload_verify_failure_reply_send_true_on_send_failure() {
        assert!(should_terminate_after_blob_upload_verify_failure_reply_send(false));
    }

    #[test]
    fn should_terminate_after_blob_upload_verify_failure_reply_send_false_on_send_success() {
        assert!(!should_terminate_after_blob_upload_verify_failure_reply_send(true));
    }

    #[test]
    fn classify_client_message_json_parses_valid_json_payload() {
        let parsed = classify_client_message_json(r#"{"type":"presence"}"#);
        match parsed {
            ClientMessageParseClassification::Parsed(v) => {
                assert_eq!(v["type"], "presence");
            }
            ClientMessageParseClassification::MalformedJson(_) => {
                panic!("expected parsed classification");
            }
        }
    }

    #[test]
    fn classify_client_message_json_returns_malformed_plan_for_invalid_json() {
        let parsed = classify_client_message_json("{not-json");
        match parsed {
            ClientMessageParseClassification::MalformedJson(plan) => {
                assert_eq!(plan.error_message, "invalid JSON");
                assert!(plan.keep_connection_open);
            }
            ClientMessageParseClassification::Parsed(_) => {
                panic!("expected malformed-json classification");
            }
        }
    }

    #[test]
    fn should_ignore_unknown_client_message_type_is_true_for_non_empty() {
        assert!(should_ignore_unknown_client_message_type("not-a-known-type"));
    }

    #[test]
    fn should_ignore_unknown_client_message_type_is_true_for_empty() {
        assert!(should_ignore_unknown_client_message_type(""));
    }

    #[test]
    fn should_skip_self_echo_broadcast_envelope_true_for_matching_from_peer() {
        let env = serde_json::json!({"type":"pack","from":"peer-a"}).to_string();
        assert!(should_skip_self_echo_broadcast_envelope(&env, "peer-a"));
    }

    #[test]
    fn should_skip_self_echo_broadcast_envelope_false_for_non_matching_from_peer() {
        let env = serde_json::json!({"type":"pack","from":"peer-b"}).to_string();
        assert!(!should_skip_self_echo_broadcast_envelope(&env, "peer-a"));
    }

    #[test]
    fn should_skip_self_echo_broadcast_envelope_false_for_invalid_json() {
        assert!(!should_skip_self_echo_broadcast_envelope("{not-json", "peer-a"));
    }

    #[test]
    fn plan_register_peer_membership_increments_when_inserted() {
        let plan = plan_register_peer_membership(true);
        assert!(plan.clear_idle_since);
        assert_eq!(plan.gauge_update, PeerCountGaugeUpdate::Increment);
    }

    #[test]
    fn plan_register_peer_membership_no_change_when_existing_peer() {
        let plan = plan_register_peer_membership(false);
        assert!(plan.clear_idle_since);
        assert_eq!(plan.gauge_update, PeerCountGaugeUpdate::NoChange);
    }

    #[test]
    fn plan_deregister_peer_membership_sets_idle_and_decrements_when_last_removed() {
        let plan = plan_deregister_peer_membership(true, true);
        assert!(plan.set_idle_since_now);
        assert_eq!(plan.gauge_update, PeerCountGaugeUpdate::Decrement);
    }

    #[test]
    fn plan_deregister_peer_membership_no_idle_and_no_gauge_change_when_not_removed() {
        let plan = plan_deregister_peer_membership(false, false);
        assert!(!plan.set_idle_since_now);
        assert_eq!(plan.gauge_update, PeerCountGaugeUpdate::NoChange);
    }

    #[test]
    fn plan_pack_import_mutation_broadcasts_when_any_nodes_accepted() {
        let action = plan_pack_import_mutation(1);
        assert_eq!(action, PackImportMutationAction::BroadcastAcceptedLeafPack);
    }

    #[test]
    fn plan_pack_import_mutation_does_not_broadcast_when_none_accepted() {
        let action = plan_pack_import_mutation(0);
        assert_eq!(action, PackImportMutationAction::NoBroadcast);
    }

    #[test]
    fn plan_blob_store_mutation_broadcasts_when_any_blobs_stored() {
        let action = plan_blob_store_mutation(2);
        assert_eq!(action, BlobStoreMutationAction::BroadcastBlobAvailable);
    }

    #[test]
    fn plan_blob_store_mutation_does_not_broadcast_when_none_stored() {
        let action = plan_blob_store_mutation(0);
        assert_eq!(action, BlobStoreMutationAction::NoBroadcast);
    }

    #[test]
    fn classify_webrtc_relay_branch_routes_offer_answer_and_ice() {
        assert_eq!(
            classify_webrtc_relay_branch("webrtc-offer"),
            WebRtcRelayBranchClassification::Relay,
        );
        assert_eq!(
            classify_webrtc_relay_branch("webrtc-answer"),
            WebRtcRelayBranchClassification::Relay,
        );
        assert_eq!(
            classify_webrtc_relay_branch("webrtc-ice"),
            WebRtcRelayBranchClassification::Relay,
        );
    }

    #[test]
    fn classify_webrtc_relay_branch_rejects_non_webrtc_types() {
        assert_eq!(
            classify_webrtc_relay_branch("presence"),
            WebRtcRelayBranchClassification::NotRelay,
        );
        assert_eq!(
            classify_webrtc_relay_branch(""),
            WebRtcRelayBranchClassification::NotRelay,
        );
    }

    #[test]
    fn should_terminate_after_stop_tick_reply_send_true_on_send_failure() {
        assert!(should_terminate_after_stop_tick_reply_send(false));
    }

    #[test]
    fn should_terminate_after_stop_tick_reply_send_false_on_send_success() {
        assert!(!should_terminate_after_stop_tick_reply_send(true));
    }

    #[test]
    fn should_terminate_after_compact_room_ack_send_true_on_send_failure() {
        assert!(should_terminate_after_compact_room_ack_send(false));
    }

    #[test]
    fn should_terminate_after_compact_room_ack_send_false_on_send_success() {
        assert!(!should_terminate_after_compact_room_ack_send(true));
    }

    #[test]
    fn should_terminate_after_set_room_key_locked_ack_send_true_on_send_failure() {
        assert!(should_terminate_after_set_room_key_locked_ack_send(false));
    }

    #[test]
    fn should_terminate_after_set_room_key_locked_ack_send_false_on_send_success() {
        assert!(!should_terminate_after_set_room_key_locked_ack_send(true));
    }

    #[test]
    fn should_terminate_after_start_tick_reply_send_true_on_send_failure() {
        assert!(should_terminate_after_start_tick_reply_send(false));
    }

    #[test]
    fn should_terminate_after_start_tick_reply_send_false_on_send_success() {
        assert!(!should_terminate_after_start_tick_reply_send(true));
    }

    #[test]
    fn should_terminate_after_server_info_reply_send_true_on_send_failure() {
        assert!(should_terminate_after_server_info_reply_send(false));
    }

    #[test]
    fn should_terminate_after_server_info_reply_send_false_on_send_success() {
        assert!(!should_terminate_after_server_info_reply_send(true));
    }

    #[test]
    fn should_terminate_after_set_policy_reply_send_true_on_send_failure() {
        assert!(should_terminate_after_set_policy_reply_send(false));
    }

    #[test]
    fn should_terminate_after_set_policy_reply_send_false_on_send_success() {
        assert!(!should_terminate_after_set_policy_reply_send(true));
    }

    #[test]
    fn should_terminate_after_subscribe_ack_send_true_on_send_failure() {
        assert!(should_terminate_after_subscribe_ack_send(false));
    }

    #[test]
    fn should_terminate_after_subscribe_ack_send_false_on_send_success() {
        assert!(!should_terminate_after_subscribe_ack_send(true));
    }

    #[test]
    fn should_terminate_after_mst_request_reply_send_true_on_send_failure() {
        assert!(should_terminate_after_mst_request_reply_send(false));
    }

    #[test]
    fn should_terminate_after_mst_request_reply_send_false_on_send_success() {
        assert!(!should_terminate_after_mst_request_reply_send(true));
    }

    #[test]
    fn should_terminate_after_mst_done_server_pack_send_true_on_send_failure() {
        assert!(should_terminate_after_mst_done_server_pack_send(false));
    }

    #[test]
    fn should_terminate_after_mst_done_server_pack_send_false_on_send_success() {
        assert!(!should_terminate_after_mst_done_server_pack_send(true));
    }

    #[test]
    fn should_terminate_after_request_server_pack_send_true_on_send_failure() {
        assert!(should_terminate_after_request_server_pack_send(false));
    }

    #[test]
    fn should_terminate_after_request_server_pack_send_false_on_send_success() {
        assert!(!should_terminate_after_request_server_pack_send(true));
    }

    #[test]
    fn should_terminate_after_blob_request_redirect_reply_send_true_on_send_failure() {
        assert!(should_terminate_after_blob_request_redirect_reply_send(false));
    }

    #[test]
    fn should_terminate_after_blob_request_redirect_reply_send_false_on_send_success() {
        assert!(!should_terminate_after_blob_request_redirect_reply_send(true));
    }

    #[test]
    fn should_terminate_after_blob_request_blob_pack_reply_send_true_on_send_failure() {
        assert!(should_terminate_after_blob_request_blob_pack_reply_send(false));
    }

    #[test]
    fn should_terminate_after_blob_request_blob_pack_reply_send_false_on_send_success() {
        assert!(!should_terminate_after_blob_request_blob_pack_reply_send(true));
    }

    #[test]
    fn should_terminate_after_request_upload_reply_send_true_on_send_failure() {
        assert!(should_terminate_after_request_upload_reply_send(false));
    }

    #[test]
    fn should_terminate_after_request_upload_reply_send_false_on_send_success() {
        assert!(!should_terminate_after_request_upload_reply_send(true));
    }

    #[test]
    fn plan_direct_blob_io_negotiation_gate_none_when_capability_negotiated() {
        let plan = plan_direct_blob_io_negotiation_gate(true, "request-upload");
        assert_eq!(plan, None);
    }

    #[test]
    fn plan_direct_blob_io_negotiation_gate_request_upload_when_not_negotiated() {
        let plan = plan_direct_blob_io_negotiation_gate(false, "request-upload")
            .expect("expected negotiation gate plan");
        assert_eq!(plan.error_message, "request-upload not negotiated");
        assert!(plan.keep_connection_open);
    }

    #[test]
    fn plan_direct_blob_io_negotiation_gate_blob_uploaded_when_not_negotiated() {
        let plan = plan_direct_blob_io_negotiation_gate(false, "blob-uploaded")
            .expect("expected negotiation gate plan");
        assert_eq!(plan.error_message, "blob-uploaded not negotiated");
        assert!(plan.keep_connection_open);
    }

    #[test]
    fn plan_direct_blob_io_bad_hash_gate_none_when_hash_valid() {
        let plan = plan_direct_blob_io_bad_hash_gate(true, "request-upload");
        assert_eq!(plan, None);
    }

    #[test]
    fn plan_direct_blob_io_bad_hash_gate_request_upload_when_invalid_hash() {
        let plan = plan_direct_blob_io_bad_hash_gate(false, "request-upload")
            .expect("expected bad-hash gate plan");
        assert_eq!(plan.error_message, "request-upload: bad hash");
        assert!(plan.keep_connection_open);
    }

    #[test]
    fn plan_direct_blob_io_bad_hash_gate_blob_uploaded_when_invalid_hash() {
        let plan = plan_direct_blob_io_bad_hash_gate(false, "blob-uploaded")
            .expect("expected bad-hash gate plan");
        assert_eq!(plan.error_message, "blob-uploaded: bad hash");
        assert!(plan.keep_connection_open);
    }

    #[test]
    fn classify_direct_blob_io_verify_outcome_broadcasts_on_success() {
        let outcome = classify_direct_blob_io_verify_outcome(true);
        assert_eq!(outcome, DirectBlobIoVerifyOutcome::BroadcastAvailable);
    }

    #[test]
    fn classify_direct_blob_io_verify_outcome_rejects_on_failure() {
        let outcome = classify_direct_blob_io_verify_outcome(false);
        assert_eq!(outcome, DirectBlobIoVerifyOutcome::SendUploadRejected);
    }

    #[test]
    fn classify_request_upload_resolve_put_url_outcome_grants_when_available() {
        let outcome = classify_request_upload_resolve_put_url_outcome(true);
        assert_eq!(outcome, RequestUploadResolvePutUrlOutcome::GrantUpload);
    }

    #[test]
    fn classify_request_upload_resolve_put_url_outcome_denies_when_missing() {
        let outcome = classify_request_upload_resolve_put_url_outcome(false);
        assert_eq!(outcome, RequestUploadResolvePutUrlOutcome::DenyUseWs);
    }

    #[test]
    fn shape_blob_uploaded_available_hashes_emits_single_hash() {
        let hashes = shape_blob_uploaded_available_hashes("abc123");
        assert_eq!(hashes, vec!["abc123".to_string()]);
    }

    #[test]
    fn shape_blob_uploaded_available_hashes_preserves_empty_hash() {
        let hashes = shape_blob_uploaded_available_hashes("");
        assert_eq!(hashes, vec!["".to_string()]);
    }

    #[test]
    fn shape_blob_uploaded_verify_failure_rejection_payload_sets_hash_and_reason() {
        let payload = shape_blob_uploaded_verify_failure_rejection_payload(
            "hash-123",
            "verify failed",
        );
        assert_eq!(payload.hash, "hash-123".to_string());
        assert_eq!(payload.reason, "verify failed".to_string());
    }

    #[test]
    fn shape_blob_uploaded_verify_failure_rejection_payload_preserves_empty_reason() {
        let payload = shape_blob_uploaded_verify_failure_rejection_payload("hash-123", "");
        assert_eq!(payload.hash, "hash-123".to_string());
        assert_eq!(payload.reason, "".to_string());
    }

    #[test]
    fn extract_blob_uploaded_hash_text_returns_hash_when_present() {
        let msg = serde_json::json!({"hash":"abc123"});
        let hash = extract_blob_uploaded_hash_text(&msg);
        assert_eq!(hash, "abc123".to_string());
    }

    #[test]
    fn extract_blob_uploaded_hash_text_defaults_to_empty_when_missing() {
        let msg = serde_json::json!({});
        let hash = extract_blob_uploaded_hash_text(&msg);
        assert_eq!(hash, "".to_string());
    }

    #[test]
    fn extract_request_upload_hash_text_returns_hash_when_present() {
        let msg = serde_json::json!({"hash":"abc123"});
        let hash = extract_request_upload_hash_text(&msg);
        assert_eq!(hash, "abc123".to_string());
    }

    #[test]
    fn extract_request_upload_hash_text_defaults_to_empty_when_missing() {
        let msg = serde_json::json!({});
        let hash = extract_request_upload_hash_text(&msg);
        assert_eq!(hash, "".to_string());
    }

    #[test]
    fn extract_request_upload_size_returns_value_when_present() {
        let msg = serde_json::json!({"size": 42});
        let size = extract_request_upload_size(&msg);
        assert_eq!(size, 42);
    }

    #[test]
    fn extract_request_upload_size_defaults_to_zero_when_missing() {
        let msg = serde_json::json!({});
        let size = extract_request_upload_size(&msg);
        assert_eq!(size, 0);
    }

    #[test]
    fn extract_request_upload_content_type_returns_value_when_present() {
        let msg = serde_json::json!({"content_type": "audio/ogg"});
        let content_type = extract_request_upload_content_type(&msg);
        assert_eq!(content_type, Some("audio/ogg".to_string()));
    }

    #[test]
    fn extract_request_upload_content_type_returns_none_when_missing() {
        let msg = serde_json::json!({});
        let content_type = extract_request_upload_content_type(&msg);
        assert_eq!(content_type, None);
    }

    #[test]
    fn extract_hello_pubkey_text_returns_value_when_present() {
        let hello = serde_json::json!({"pubkey":"abcd"});
        let pubkey = extract_hello_pubkey_text(&hello);
        assert_eq!(pubkey, "abcd".to_string());
    }

    #[test]
    fn extract_hello_pubkey_text_defaults_to_empty_when_missing() {
        let hello = serde_json::json!({});
        let pubkey = extract_hello_pubkey_text(&hello);
        assert_eq!(pubkey, "".to_string());
    }

    #[test]
    fn extract_client_message_type_text_returns_value_when_present() {
        let msg = serde_json::json!({"type": "presence"});
        let message_type = extract_client_message_type_text(&msg);
        assert_eq!(message_type, "presence".to_string());
    }

    #[test]
    fn extract_client_message_type_text_defaults_to_empty_when_missing() {
        let msg = serde_json::json!({});
        let message_type = extract_client_message_type_text(&msg);
        assert_eq!(message_type, "".to_string());
    }

    #[test]
    fn extract_presence_data_payload_returns_value_when_present() {
        let msg = serde_json::json!({"data": {"cursor": 12}});
        let data = extract_presence_data_payload(&msg);
        assert_eq!(data, serde_json::json!({"cursor": 12}));
    }

    #[test]
    fn extract_presence_data_payload_defaults_to_null_when_missing() {
        let msg = serde_json::json!({});
        let data = extract_presence_data_payload(&msg);
        assert_eq!(data, serde_json::Value::Null);
    }

    #[test]
    fn extract_pack_nodes_payload_b64_returns_value_when_present() {
        let msg = serde_json::json!({"nodes":"Zm9v"});
        let nodes_b64 = extract_pack_nodes_payload_b64(&msg);
        assert_eq!(nodes_b64, "Zm9v".to_string());
    }

    #[test]
    fn extract_pack_nodes_payload_b64_defaults_to_empty_when_missing() {
        let msg = serde_json::json!({});
        let nodes_b64 = extract_pack_nodes_payload_b64(&msg);
        assert_eq!(nodes_b64, "".to_string());
    }

    #[test]
    fn extract_blob_upload_entry_hash_text_returns_value_when_present() {
        let entry = serde_json::json!({"hash":"abc123"});
        let hash = extract_blob_upload_entry_hash_text(&entry);
        assert_eq!(hash, "abc123".to_string());
    }

    #[test]
    fn extract_blob_upload_entry_hash_text_defaults_to_empty_when_missing() {
        let entry = serde_json::json!({});
        let hash = extract_blob_upload_entry_hash_text(&entry);
        assert_eq!(hash, "".to_string());
    }

    #[test]
    fn extract_blob_upload_entry_data_b64_returns_value_when_present() {
        let entry = serde_json::json!({"data":"Zm9v"});
        let data_b64 = extract_blob_upload_entry_data_b64(&entry);
        assert_eq!(data_b64, "Zm9v".to_string());
    }

    #[test]
    fn extract_blob_upload_entry_data_b64_defaults_to_empty_when_missing() {
        let entry = serde_json::json!({});
        let data_b64 = extract_blob_upload_entry_data_b64(&entry);
        assert_eq!(data_b64, "".to_string());
    }

    #[test]
    fn extract_set_policy_default_text_returns_value_when_present() {
        let msg = serde_json::json!({"default":"deny"});
        let default_text = extract_set_policy_default_text(&msg);
        assert_eq!(default_text, "deny".to_string());
    }

    #[test]
    fn extract_set_policy_default_text_defaults_to_allow_when_missing() {
        let msg = serde_json::json!({});
        let default_text = extract_set_policy_default_text(&msg);
        assert_eq!(default_text, "allow".to_string());
    }

    #[test]
    fn extract_set_policy_rule_values_returns_values_when_present() {
        let msg = serde_json::json!({"rules":[{"path_glob":"a/**"}]});
        let rules = extract_set_policy_rule_values(&msg);
        assert_eq!(rules.len(), 1);
    }

    #[test]
    fn extract_set_policy_rule_values_defaults_to_empty_when_missing() {
        let msg = serde_json::json!({});
        let rules = extract_set_policy_rule_values(&msg);
        assert!(rules.is_empty());
    }

    #[test]
    fn extract_set_policy_can_write_values_returns_values_when_present() {
        let rule = serde_json::json!({"can_write":["abc"]});
        let can_write = extract_set_policy_can_write_values(&rule);
        assert_eq!(can_write.len(), 1);
    }

    #[test]
    fn extract_set_policy_can_write_values_defaults_to_empty_when_missing() {
        let rule = serde_json::json!({});
        let can_write = extract_set_policy_can_write_values(&rule);
        assert!(can_write.is_empty());
    }

    #[test]
    fn extract_set_policy_can_write_hex_text_returns_value_when_present() {
        let v = serde_json::json!("abc");
        let hex = extract_set_policy_can_write_hex_text(&v);
        assert_eq!(hex, "abc".to_string());
    }

    #[test]
    fn extract_set_policy_can_write_hex_text_defaults_to_empty_when_missing() {
        let v = serde_json::json!(null);
        let hex = extract_set_policy_can_write_hex_text(&v);
        assert_eq!(hex, "".to_string());
    }

    #[test]
    fn extract_blob_request_hashes_returns_values_when_present() {
        let msg = serde_json::json!({"hashes": ["a", "b", 7]});
        let hashes = extract_blob_request_hashes(&msg);
        assert_eq!(hashes, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn extract_blob_request_hashes_defaults_to_empty_when_missing() {
        let msg = serde_json::json!({});
        let hashes = extract_blob_request_hashes(&msg);
        assert!(hashes.is_empty());
    }

    #[test]
    fn extract_webrtc_relay_fields_returns_object_when_present() {
        let msg = serde_json::json!({"type":"webrtc-offer","to":"peer-b","sdp":{"type":"offer"}});
        let fields = extract_webrtc_relay_fields(&msg);
        assert_eq!(fields.get("type").and_then(|v| v.as_str()), Some("webrtc-offer"));
        assert_eq!(fields.get("to").and_then(|v| v.as_str()), Some("peer-b"));
        assert!(fields.get("sdp").is_some());
    }

    #[test]
    fn extract_webrtc_relay_fields_defaults_to_empty_for_non_object() {
        let msg = serde_json::json!("not-an-object");
        let fields = extract_webrtc_relay_fields(&msg);
        assert!(fields.is_empty());
    }

    #[test]
    fn extract_mst_request_paths_returns_values_when_present() {
        let msg = serde_json::json!({"paths": ["a/b", "c/d", 7]});
        let paths = extract_mst_request_paths(&msg);
        assert_eq!(paths, vec!["a/b".to_string(), "c/d".to_string()]);
    }

    #[test]
    fn extract_mst_request_paths_defaults_to_empty_when_missing() {
        let msg = serde_json::json!({});
        let paths = extract_mst_request_paths(&msg);
        assert!(paths.is_empty());
    }

    #[test]
    fn extract_mst_done_ids_returns_only_valid_hex_ids() {
        let valid = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let msg = serde_json::json!({"ids": [valid, "bad", 7]});
        let ids = extract_mst_done_ids(&msg);
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0].to_hex(), valid.to_string());
    }

    #[test]
    fn extract_mst_done_ids_defaults_to_empty_when_missing() {
        let msg = serde_json::json!({});
        let ids = extract_mst_done_ids(&msg);
        assert!(ids.is_empty());
    }

    #[test]
    fn shape_request_known_id_set_returns_only_valid_hex_ids() {
        let valid = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let msg = serde_json::json!({"known": [valid, "bad", 9]});
        let ids = shape_request_known_id_set(&msg);
        assert_eq!(ids.len(), 1);
        assert!(ids.iter().any(|id| id.to_hex() == valid));
    }

    #[test]
    fn shape_request_known_id_set_defaults_to_empty_when_missing() {
        let msg = serde_json::json!({});
        let ids = shape_request_known_id_set(&msg);
        assert!(ids.is_empty());
    }

    #[test]
    fn shape_client_known_id_set_collects_unique_ids() {
        let id = parse_hex_hash(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ).expect("valid node id");
        let set = shape_client_known_id_set(&[id, id]);
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn classify_set_room_key_parse_result_parsed_when_parse_succeeds() {
        let result = classify_set_room_key_parse_result(true);
        assert_eq!(result, SetRoomKeyParseResult::Parsed);
    }

    #[test]
    fn classify_set_room_key_parse_result_invalid_when_parse_fails() {
        let result = classify_set_room_key_parse_result(false);
        assert_eq!(result, SetRoomKeyParseResult::InvalidPubkeyHex);
    }

    #[test]
    fn classify_set_room_key_lock_state_locks_when_room_open() {
        let result = classify_set_room_key_lock_state(true);
        assert_eq!(result, SetRoomKeyLockStateResult::LockRoom);
    }

    #[test]
    fn classify_set_room_key_lock_state_rejects_when_room_locked() {
        let result = classify_set_room_key_lock_state(false);
        assert_eq!(result, SetRoomKeyLockStateResult::RejectAlreadyLocked);
    }

    #[test]
    fn shape_set_room_key_invalid_pubkey_rejection_reason_is_stable() {
        assert_eq!(
            shape_set_room_key_invalid_pubkey_rejection_reason(),
            "set-room-key: invalid pubkey hex",
        );
    }

    #[test]
    fn shape_set_room_key_already_locked_rejection_reason_is_stable() {
        assert_eq!(
            shape_set_room_key_already_locked_rejection_reason(),
            "room already locked",
        );
    }

    #[test]
    fn classify_set_policy_parse_result_apply_when_policy_present() {
        let result = classify_set_policy_parse_result(true);
        assert_eq!(result, SetPolicyParseResult::ApplyPolicy);
    }

    #[test]
    fn classify_set_policy_parse_result_failed_when_policy_missing() {
        let result = classify_set_policy_parse_result(false);
        assert_eq!(result, SetPolicyParseResult::ParseFailed);
    }

    #[test]
    fn classify_set_policy_default_maps_allow_and_deny() {
        assert_eq!(
            classify_set_policy_default("allow"),
            SetPolicyDefaultParseResult::AllowAll,
        );
        assert_eq!(
            classify_set_policy_default("deny"),
            SetPolicyDefaultParseResult::DenyAll,
        );
    }

    #[test]
    fn classify_set_policy_default_marks_unknown_values() {
        assert_eq!(
            classify_set_policy_default("custom"),
            SetPolicyDefaultParseResult::Unknown,
        );
    }

    #[test]
    fn shape_set_policy_unknown_default_error_is_stable() {
        let err = shape_set_policy_unknown_default_error("custom");
        assert_eq!(
            err,
            "set-policy: unknown default 'custom', use 'allow' or 'deny'".to_string(),
        );
    }

    #[test]
    fn classify_compact_room_compaction_result_uses_snapshot_when_compact_succeeds() {
        let result = classify_compact_room_compaction_result(true);
        assert_eq!(result, CompactRoomCompactionResult::UseSnapshot);
    }

    #[test]
    fn classify_compact_room_compaction_result_sends_error_when_compact_fails() {
        let result = classify_compact_room_compaction_result(false);
        assert_eq!(result, CompactRoomCompactionResult::SendCompactFailed);
    }

    #[test]
    fn classify_compact_room_verify_result_uses_verified_snapshot_on_success() {
        let result = classify_compact_room_verify_result(true);
        assert_eq!(result, CompactRoomVerifyResult::UseVerifiedSnapshot);
    }

    #[test]
    fn classify_compact_room_verify_result_sends_error_on_failure() {
        let result = classify_compact_room_verify_result(false);
        assert_eq!(result, CompactRoomVerifyResult::SendVerifyFailed);
    }

    #[test]
    fn classify_compact_room_rebuild_result_installs_rebuilt_graph_on_success() {
        let result = classify_compact_room_rebuild_result(true);
        assert_eq!(result, CompactRoomRebuildResult::InstallRebuiltGraph);
    }

    #[test]
    fn classify_compact_room_rebuild_result_sends_error_on_failure() {
        let result = classify_compact_room_rebuild_result(false);
        assert_eq!(result, CompactRoomRebuildResult::SendRebuildFailed);
    }

    #[test]
    fn shape_compact_room_compaction_failed_error_is_stable() {
        let err = shape_compact_room_compaction_failed_error("boom");
        assert_eq!(err, "compact failed: boom".to_string());
    }

    #[test]
    fn shape_compact_room_verify_failed_error_is_stable() {
        let err = shape_compact_room_verify_failed_error("bad snap");
        assert_eq!(err, "snapshot verify failed: bad snap".to_string());
    }

    #[test]
    fn shape_compact_room_rebuild_failed_error_is_stable() {
        let err = shape_compact_room_rebuild_failed_error("bad graph");
        assert_eq!(err, "rebuild failed: bad graph".to_string());
    }

    #[test]
    fn extract_set_room_key_pubkey_text_returns_value_when_present() {
        let msg = serde_json::json!({"pubkey": "abcd"});
        let pubkey = extract_set_room_key_pubkey_text(&msg);
        assert_eq!(pubkey, "abcd".to_string());
    }

    #[test]
    fn extract_set_room_key_pubkey_text_defaults_to_empty_when_missing() {
        let msg = serde_json::json!({});
        let pubkey = extract_set_room_key_pubkey_text(&msg);
        assert_eq!(pubkey, "".to_string());
    }

    #[test]
    fn extract_start_tick_interval_ms_returns_value_when_present() {
        let msg = serde_json::json!({"interval_ms": 24});
        let interval_ms = extract_start_tick_interval_ms(&msg);
        assert_eq!(interval_ms, 24);
    }

    #[test]
    fn extract_start_tick_interval_ms_defaults_when_missing() {
        let msg = serde_json::json!({});
        let interval_ms = extract_start_tick_interval_ms(&msg);
        assert_eq!(interval_ms, 16);
    }

    #[test]
    fn extract_start_tick_interval_ms_clamps_zero_to_one() {
        let msg = serde_json::json!({"interval_ms": 0});
        let interval_ms = extract_start_tick_interval_ms(&msg);
        assert_eq!(interval_ms, 1);
    }

    #[test]
    fn extract_start_tick_intent_prefix_returns_value_when_present() {
        let msg = serde_json::json!({"intent_prefix": "game/intent/"});
        let intent_prefix = extract_start_tick_intent_prefix(&msg);
        assert_eq!(intent_prefix, "game/intent/".to_string());
    }

    #[test]
    fn extract_start_tick_intent_prefix_defaults_when_missing() {
        let msg = serde_json::json!({});
        let intent_prefix = extract_start_tick_intent_prefix(&msg);
        assert_eq!(intent_prefix, "intent/".to_string());
    }

    fn base64_encode_for_test(bytes: &[u8]) -> String {
        const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
        let mut i = 0usize;
        while i < bytes.len() {
            let b0 = bytes[i] as u32;
            let b1 = if i + 1 < bytes.len() { bytes[i + 1] as u32 } else { 0 };
            let b2 = if i + 2 < bytes.len() { bytes[i + 2] as u32 } else { 0 };
            let n = (b0 << 16) | (b1 << 8) | b2;

            out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
            out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
            if i + 1 < bytes.len() {
                out.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
            } else {
                out.push('=');
            }
            if i + 2 < bytes.len() {
                out.push(TABLE[(n & 0x3f) as usize] as char);
            } else {
                out.push('=');
            }
            i += 3;
        }
        out
    }
}
