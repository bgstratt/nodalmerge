use std::collections::BTreeMap;
use std::path::Path;

use ed25519_dalek::{Signer, SigningKey};
use nodalmerge_core::{
    canonical_hash, pack_nodes, policy_timeline_cutover_lamport, policy_timeline_hash, replay,
    Hash, Policy, PolicyTimelineEntry, SyncNode,
};
use serde::{Deserialize, Serialize};

use crate::store::ServerPersistence;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportManifestDigestSet {
    pub nodes: String,
    pub blobs: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportManifestCheckpoint {
    pub frontier: Vec<String>,
    pub canonical_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportManifestSignature {
    pub public_key: String,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportManifestCompatibilityWindow {
    pub min_supported: String,
    pub max_supported: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportManifestDocument {
    pub format_version: String,
    pub source_room: String,
    pub checkpoint: ExportManifestCheckpoint,
    pub payload_digest_set: ExportManifestDigestSet,
    pub compatibility_window: ExportManifestCompatibilityWindow,
    pub payload_digest_policy: String,
    pub policy_timeline_hash: String,
    pub policy_timeline_cutover_lamport: u64,
    pub policy_timeline_transition_cutovers: Vec<u64>,
    pub signature: ExportManifestSignature,
}

pub const EXPORT_FORMAT_VERSION: &str = "1";
pub const EXPORT_COMPAT_MIN_SUPPORTED: &str = "1";
pub const EXPORT_COMPAT_MAX_SUPPORTED: &str = "2";
pub const EXPORT_DIGEST_POLICY_STRICT_SHA256_V1: &str = "strict_sha256_v1";

pub fn build_external_manifest_document(
    persistence: &dyn ServerPersistence,
    source_room: &str,
    signer: &SigningKey,
    policy_timeline_hash_hex: &str,
    policy_timeline_cutover_lamport: u64,
    policy_timeline_transition_cutovers: &[u64],
) -> Result<ExportManifestDocument, String> {
    let mut nodes = persistence.load_room_nodes(source_room);
    if nodes.is_empty() {
        return Err("archive source room has no persisted nodes".to_string());
    }
    nodes.sort_by(|a, b| a.id.cmp(&b.id));
    let checkpoint_hash = checkpoint_hash_for_nodes(&nodes)?;
    let nodes_digest = nodes_digest_for_nodes(&nodes);
    let blobs_digest = blobs_digest_for_referenced(persistence, &nodes);

    let compatibility_window = ExportManifestCompatibilityWindow {
        min_supported: EXPORT_COMPAT_MIN_SUPPORTED.to_string(),
        max_supported: EXPORT_COMPAT_MAX_SUPPORTED.to_string(),
    };
    let payload_digest_policy = EXPORT_DIGEST_POLICY_STRICT_SHA256_V1.to_string();

    let payload = signature_payload(
        EXPORT_FORMAT_VERSION,
        source_room,
        &compatibility_window,
        &payload_digest_policy,
        policy_timeline_hash_hex,
        policy_timeline_cutover_lamport,
        policy_timeline_transition_cutovers,
    );
    let signature = signer.sign(payload.as_bytes()).to_bytes();

    Ok(ExportManifestDocument {
        format_version: EXPORT_FORMAT_VERSION.to_string(),
        source_room: source_room.to_string(),
        checkpoint: ExportManifestCheckpoint {
            frontier: vec![format!("seq:{}", nodes.len())],
            canonical_hash: checkpoint_hash,
        },
        payload_digest_set: ExportManifestDigestSet {
            nodes: nodes_digest,
            blobs: blobs_digest,
        },
        compatibility_window,
        payload_digest_policy,
        policy_timeline_hash: policy_timeline_hash_hex.to_string(),
        policy_timeline_cutover_lamport,
        policy_timeline_transition_cutovers: policy_timeline_transition_cutovers.to_vec(),
        signature: ExportManifestSignature {
            public_key: hex_lower(&signer.verifying_key().to_bytes()),
            signature: hex_lower(&signature),
        },
    })
}

pub struct PolicyTimelineParityMetadata {
    pub hash_hex: String,
    pub cutover_lamport: u64,
    pub transition_cutovers: Vec<u64>,
}

pub fn policy_timeline_hash_hex_for_policy(policy: &Policy) -> String {
    policy_timeline_metadata_for_policy(policy).hash_hex
}

pub fn policy_timeline_metadata_for_policy(policy: &Policy) -> PolicyTimelineParityMetadata {
    let timeline = vec![PolicyTimelineEntry {
        effective_lamport: 0,
        policy: policy.clone(),
    }];
    policy_timeline_metadata_for_timeline(&timeline)
}

pub fn policy_timeline_metadata_for_timeline(
    timeline: &[PolicyTimelineEntry],
) -> PolicyTimelineParityMetadata {
    let mut sorted_timeline = if timeline.is_empty() {
        vec![PolicyTimelineEntry {
            effective_lamport: 0,
            policy: Policy::default(),
        }]
    } else {
        timeline.to_vec()
    };
    sorted_timeline.sort_by_key(|entry| entry.effective_lamport);
    let cutover_lamport = policy_timeline_cutover_lamport(&sorted_timeline).unwrap_or(0);
    let mut transition_cutovers = sorted_timeline
        .iter()
        .map(|entry| entry.effective_lamport)
        .collect::<Vec<u64>>();
    transition_cutovers.dedup();
    if transition_cutovers.is_empty() {
        transition_cutovers.push(0);
    }
    let transition_cutovers = if cutover_lamport == 0 {
        vec![0]
    } else {
        transition_cutovers
    };
    PolicyTimelineParityMetadata {
        hash_hex: policy_timeline_hash(&sorted_timeline).to_hex(),
        cutover_lamport,
        transition_cutovers,
    }
}

pub fn write_file_manifest(path: &Path, manifest: &ExportManifestDocument) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let body = serde_json::to_string_pretty(manifest).map_err(|e| e.to_string())?;
    std::fs::write(path, body).map_err(|e| e.to_string())
}

pub fn write_object_manifest(
    object_root: &Path,
    bucket: &str,
    key: &str,
    manifest: &ExportManifestDocument,
) -> Result<(), String> {
    let path = object_root.join(bucket).join(key);
    write_file_manifest(&path, manifest)
}

fn checkpoint_hash_for_nodes(nodes: &[SyncNode]) -> Result<String, String> {
    let replayed =
        replay(nodes, None).map_err(|_| "persisted nodes could not be replayed".to_string())?;
    let hash = canonical_hash(
        &replayed
            .map
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<BTreeMap<String, Vec<u8>>>(),
    );
    Ok(hash.to_hex())
}

fn nodes_digest_for_nodes(nodes: &[SyncNode]) -> String {
    let refs: Vec<&SyncNode> = nodes.iter().collect();
    let packed = pack_nodes(&refs);
    format!("sha256:{}", Hash::of(&packed).to_hex())
}

fn blobs_digest_for_referenced(persistence: &dyn ServerPersistence, nodes: &[SyncNode]) -> String {
    let mut map = BTreeMap::new();
    for hash in crate::room::blob_hashes_referenced_by(nodes.iter()) {
        if let Some(bytes) = persistence.get_blob(&hash) {
            map.insert(hash.to_hex(), bytes);
        }
    }
    format!("sha256:{}", canonical_hash(&map).to_hex())
}

fn signature_payload(
    format_version: &str,
    source_room: &str,
    compatibility_window: &ExportManifestCompatibilityWindow,
    payload_digest_policy: &str,
    policy_timeline_hash_hex: &str,
    policy_timeline_cutover_lamport: u64,
    policy_timeline_transition_cutovers: &[u64],
) -> String {
    let transition_cutovers = policy_timeline_transition_cutovers
        .iter()
        .map(u64::to_string)
        .collect::<Vec<String>>()
        .join(",");
    format!(
        "format_version={}|source_room={}|min_supported={}|max_supported={}|payload_digest_policy={}|policy_timeline_hash={}|policy_timeline_cutover_lamport={}|policy_timeline_transition_cutovers={}",
        format_version,
        source_room,
        compatibility_window.min_supported,
        compatibility_window.max_supported,
        payload_digest_policy,
        policy_timeline_hash_hex,
        policy_timeline_cutover_lamport,
        transition_cutovers,
    )
}

fn hex_lower(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(H[(b >> 4) as usize] as char);
        s.push(H[(b & 0x0f) as usize] as char);
    }
    s
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ed25519_dalek::SigningKey;
    use nodalmerge_core::{BlobStore, MapOp, Op, StateGraph};

    use super::*;
    use crate::room::{import_nodes, Room};
    use crate::store::{DirPersistence, SharedPersistence};

    fn tmpdir() -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        let p = std::env::temp_dir().join(format!("nodalmerge-archive-export-test-{nanos}"));
        std::fs::create_dir_all(&p).expect("temp dir should be created");
        p
    }

    async fn seed_room(room: &Arc<Room>) {
        let mut g = StateGraph::new();
        let sk = SigningKey::from_bytes(&[0x66; 32]);
        let id = g
            .apply_local(
                &sk,
                0,
                vec![Op::Map(MapOp::Set {
                    key: "world/export".to_string(),
                    value: b"ok".to_vec(),
                })],
            )
            .expect("seed apply_local should succeed");
        let node = g
            .get_nodes(&[id])
            .into_iter()
            .next()
            .expect("seed node should exist")
            .clone();
        let _ = import_nodes(room, vec![node]).await;

        let blob = b"export-blob".to_vec();
        let hash = Hash::of(&blob);
        room.blobs.write().await.put(blob.clone());
        room.persistence.persist_blob(&hash, &blob);
    }

    #[tokio::test]
    async fn deterministic_manifest_builder_is_stable_for_same_checkpoint() {
        let root = tmpdir();
        let persistence: SharedPersistence =
            Arc::new(DirPersistence::open(&root).expect("dir persistence should open"));
        let source_room = Room::new("export-source".to_string(), Arc::clone(&persistence), 64);
        seed_room(&source_room).await;

        let signer = SigningKey::from_bytes(&[0x22; 32]);
        let policy_metadata = policy_timeline_metadata_for_policy(&Policy::default());
        let m1 = build_external_manifest_document(
            &*persistence,
            "export-source",
            &signer,
            &policy_metadata.hash_hex,
            policy_metadata.cutover_lamport,
            &policy_metadata.transition_cutovers,
        )
        .expect("manifest should build");
        let m2 = build_external_manifest_document(
            &*persistence,
            "export-source",
            &signer,
            &policy_metadata.hash_hex,
            policy_metadata.cutover_lamport,
            &policy_metadata.transition_cutovers,
        )
        .expect("manifest should build");

        assert_eq!(m1, m2);
    }

    #[tokio::test]
    async fn manifest_writers_emit_file_and_object_paths() {
        let root = tmpdir();
        let persistence: SharedPersistence =
            Arc::new(DirPersistence::open(&root).expect("dir persistence should open"));
        let source_room = Room::new("export-source-2".to_string(), Arc::clone(&persistence), 64);
        seed_room(&source_room).await;

        let signer = SigningKey::from_bytes(&[0x23; 32]);
        let policy_metadata = policy_timeline_metadata_for_policy(&Policy::default());
        let manifest = build_external_manifest_document(
            &*persistence,
            "export-source-2",
            &signer,
            &policy_metadata.hash_hex,
            policy_metadata.cutover_lamport,
            &policy_metadata.transition_cutovers,
        )
        .expect("manifest should build");

        let file_path = root.join("exports").join("manifest.json");
        write_file_manifest(&file_path, &manifest).expect("file manifest should be written");
        assert!(file_path.exists());

        let object_root = root.join("object-root");
        write_object_manifest(&object_root, "bucket-a", "m.json", &manifest)
            .expect("object manifest should be written");
        assert!(object_root.join("bucket-a").join("m.json").exists());
    }
}
