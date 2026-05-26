use serde::{Deserialize, Serialize};
use serde_json::Value;
use nodalmerge_core::conflicts::ConflictEvent;

pub type SessionId = u64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitySet {
    pub supports_ibf: bool,
    pub supports_mst: bool,
}

impl Default for CapabilitySet {
    fn default() -> Self {
        Self {
            supports_ibf: true,
            supports_mst: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientHelloPayload {
    pub peer_pubkey_hex: String,
    pub client_frontier: Vec<String>,
    pub capabilities: CapabilitySet,
    pub token: Option<HelloTokenPayload>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloTokenPayload {
    pub peer_pubkey: String,
    pub expiry: u64,
    pub caps: Vec<String>,
    pub sig: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyRulePayload {
    pub path_glob: String,
    pub can_write: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum HostCommand {
    EnsureRoom,
    OpenSession {
        session_id: SessionId,
        peer_pubkey_hex: String,
    },
    CloseSession {
        session_id: SessionId,
    },
    ClientHello {
        session_id: SessionId,
        hello: ClientHelloPayload,
    },
    MapSet {
        namespace: String,
        key: String,
        value: Value,
    },
    MapDelete {
        namespace: String,
        key: String,
    },
    MapGet {
        namespace: String,
        key: String,
    },
    MapAll {
        namespace: String,
    },
    TextInsert {
        namespace: String,
        key: String,
        after_id: Option<String>,
        ch: String,
    },
    TextDelete {
        namespace: String,
        key: String,
        target_id: String,
    },
    TextGet {
        namespace: String,
        key: String,
    },
    TextGetCanonical {
        namespace: String,
        key: String,
    },
    ListPush {
        namespace: String,
        key: String,
        value: Value,
    },
    ListInsert {
        namespace: String,
        key: String,
        index: u64,
        value: Value,
    },
    ListDelete {
        namespace: String,
        key: String,
        index: u64,
    },
    ListMove {
        namespace: String,
        key: String,
        from_index: u64,
        to_index: u64,
    },
    ListUpdate {
        namespace: String,
        key: String,
        index: u64,
        value: Value,
    },
    ListGet {
        namespace: String,
        key: String,
    },
    BlobSet {
        namespace: String,
        hash: String,
        data_b64: String,
    },
    BlobGet {
        namespace: String,
        hash: String,
    },
    BlobGetMany {
        namespace: String,
        hashes: Vec<String>,
    },
    RequestUpload {
        namespace: String,
        hash: String,
        size_bytes: u64,
        content_type: Option<String>,
    },
    BlobRequest {
        namespace: String,
        hashes: Vec<String>,
    },
    PresenceSet {
        session_id: SessionId,
        data: Value,
        ttl_ms: Option<u64>,
        now_unix_ms: Option<u64>,
    },
    PresenceGetAll,
    PresenceSweep {
        now_unix_ms: u64,
    },
    Subscribe {
        session_id: SessionId,
        patterns: Vec<String>,
    },
    SetRoomKey {
        pubkey_hex: String,
    },
    SetPolicy {
        default: String,
        rules: Vec<PolicyRulePayload>,
    },
    ImportPack {
        nodes_b64: String,
    },
    RequestServerPack {
        known_ids: Vec<String>,
    },
    MstRequest {
        paths: Vec<String>,
    },
    MstDone {
        ids: Vec<String>,
    },
    GetRecentConflicts {
        since_unix_ms: Option<u64>,
    },
    RelayPeerSignal {
        session_id: SessionId,
        msg_type: String,
        to_peer_pubkey: String,
        payload: Value,
    },
    Noop,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum HostEvent {
    RoomEnsured {
        room_id: String,
    },
    SessionOpened {
        room_id: String,
        session_id: SessionId,
    },
    SessionClosed {
        room_id: String,
        session_id: SessionId,
    },
    WelcomePrepared {
        room_id: String,
        session_id: SessionId,
        negotiated: CapabilitySet,
        missing_from_server_count: usize,
    },
    MapValueUpserted {
        room_id: String,
        namespace: String,
        key: String,
        value: Value,
    },
    MapValueDeleted {
        room_id: String,
        namespace: String,
        key: String,
        found: bool,
    },
    MapValueRead {
        room_id: String,
        namespace: String,
        key: String,
        value: Option<Value>,
    },
    MapEntriesListed {
        room_id: String,
        namespace: String,
        entries: Vec<MapEntry>,
    },
    TextValueInserted {
        room_id: String,
        namespace: String,
        key: String,
        id: String,
        ch: String,
    },
    TextValueDeleted {
        room_id: String,
        namespace: String,
        key: String,
        target_id: String,
        found: bool,
    },
    TextValueRead {
        room_id: String,
        namespace: String,
        key: String,
        value: String,
        entries: Vec<TextEntry>,
    },
    ListValuePushed {
        room_id: String,
        namespace: String,
        key: String,
        id: String,
        index: u64,
        value: Value,
    },
    ListValueInserted {
        room_id: String,
        namespace: String,
        key: String,
        id: String,
        index: u64,
        value: Value,
    },
    ListValueDeleted {
        room_id: String,
        namespace: String,
        key: String,
        index: u64,
        found: bool,
        removed: Option<Value>,
    },
    ListValueMoved {
        room_id: String,
        namespace: String,
        key: String,
        from_index: u64,
        to_index: u64,
        found: bool,
        id: Option<String>,
    },
    ListValueUpdated {
        room_id: String,
        namespace: String,
        key: String,
        index: u64,
        found: bool,
        id: Option<String>,
        value: Option<Value>,
    },
    ListValueRead {
        room_id: String,
        namespace: String,
        key: String,
        entries: Vec<ListEntry>,
    },
    BlobValueStored {
        room_id: String,
        namespace: String,
        hash: String,
        stored: bool,
    },
    BlobValueRead {
        room_id: String,
        namespace: String,
        hash: String,
        found: bool,
        data_b64: Option<String>,
    },
    BlobValuesRead {
        room_id: String,
        namespace: String,
        entries: Vec<BlobEntry>,
        missing: Vec<String>,
    },
    UploadGranted {
        room_id: String,
        namespace: String,
        hash: String,
        url: String,
        expires_at_unix: u64,
    },
    UploadDenied {
        room_id: String,
        namespace: String,
        hash: String,
        reason: String,
    },
    BlobRedirectPrepared {
        room_id: String,
        namespace: String,
        redirects: Vec<BlobRedirectEntry>,
    },
    BlobPackPrepared {
        room_id: String,
        namespace: String,
        blobs: Vec<BlobEntry>,
        requested: Vec<String>,
    },
    PresenceValueSet {
        room_id: String,
        session_id: SessionId,
        from_peer_pubkey: String,
        data: Value,
        joined: bool,
    },
    PresenceValueRemoved {
        room_id: String,
        session_id: SessionId,
        from_peer_pubkey: String,
        reason: String,
    },
    PresenceValuesListed {
        room_id: String,
        entries: Vec<PresenceEntry>,
    },
    SubscriptionUpdated {
        room_id: String,
        session_id: SessionId,
        patterns: Vec<String>,
    },
    RoomLocked {
        room_id: String,
        pubkey_hex: String,
    },
    SetRoomKeyRejected {
        room_id: String,
        msg: String,
    },
    PolicySet {
        room_id: String,
    },
    SetPolicyRejected {
        room_id: String,
        msg: String,
    },
    PackImported {
        room_id: String,
        incoming_count: usize,
        accepted_count: usize,
        rejected_count: usize,
    },
    ServerPackPrepared {
        room_id: String,
        nodes_b64: String,
        root_hex: String,
    },
    MstResponsePrepared {
        room_id: String,
        nodes: Vec<Value>,
    },
    ConflictsObserved {
        room_id: String,
        entries: Vec<ConflictEntry>,
    },
    RecentConflictsListed {
        room_id: String,
        entries: Vec<ConflictEntry>,
    },
    PeerSignalRelayed {
        room_id: String,
        from_peer_pubkey: String,
        msg_type: String,
        to_peer_pubkey: String,
        payload: Value,
    },
    NoopAck,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandResult {
    pub events: Vec<HostEvent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapEntry {
    pub key: String,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextEntry {
    pub id: String,
    pub ch: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListEntry {
    pub id: String,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlobEntry {
    pub hash: String,
    pub data_b64: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlobRedirectEntry {
    pub hash: String,
    pub url: String,
    pub expires_at_unix: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresenceEntry {
    pub session_id: SessionId,
    pub from_peer_pubkey: String,
    pub data: Value,
    pub expires_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictEntry {
    pub at_unix_ms: u64,
    pub event: ConflictEvent,
}

impl CommandResult {
    pub fn empty() -> Self {
        Self { events: Vec::new() }
    }
}

#[derive(Debug, Clone)]
pub struct CommandEnvelope {
    pub room_id: String,
    pub command: HostCommand,
}

impl CommandEnvelope {
    pub fn new(room_id: impl Into<String>, command: HostCommand) -> Self {
        Self {
            room_id: room_id.into(),
            command,
        }
    }
}
