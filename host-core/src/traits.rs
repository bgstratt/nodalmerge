use activesync_core::SyncNode;

pub trait HostPersistence: Send + Sync {
    fn persist_nodes(&self, room_id: &str, nodes: &[SyncNode]) -> Result<(), String>;
}

pub trait HostClock: Send + Sync {
    fn now_unix_ms(&self) -> u64;
}

pub trait HostAuth: Send + Sync {
    fn authorize_peer(&self, room_id: &str, peer_id: &[u8]) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresignedBlobUrl {
    pub url: String,
    pub expires_at_unix: u64,
}

pub trait HostBlobUrlResolver: Send + Sync {
    fn resolve_put_url(
        &self,
        room_id: &str,
        namespace: &str,
        hash: &str,
        size_bytes: u64,
        content_type: Option<&str>,
    ) -> Option<PresignedBlobUrl>;

    fn resolve_get_url(
        &self,
        room_id: &str,
        namespace: &str,
        hash: &str,
    ) -> Option<PresignedBlobUrl>;
}
