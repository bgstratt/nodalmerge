use std::collections::BTreeMap;
use std::path::Path;

use ed25519_dalek::{Signer, SigningKey};
use nodalmerge_core::{
    canonical_hash, pack_nodes, policy_timeline_cutover_lamport, policy_timeline_hash, replay,
    Hash, Policy, PolicyTimelineEntry, SyncNode,
};
use serde::{Deserialize, Serialize};

use crate::store::{HydrateError, ServerPersistence};

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
    // slice 2.3 (finding #7): propagate rather than silently digest a subset.
    let blobs_digest = blobs_digest_for_referenced(persistence, &nodes)?;

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

/// blob-cas-remediation.md slice 2.3 (finding #7) — resolve every blob a
/// room's nodes reference, through the **hydrating** path, failing loudly
/// rather than silently exporting fewer blobs than the room actually has.
///
/// ## Why this is not a hole in the offloading policy (and where it differs
/// ## from `tree_walk`, slice 2.2's caller)
///
/// [`BlobPersistence::hydrate_blob`](crate::store::BlobPersistence::hydrate_blob)'s
/// doc says callers wanting *file* bytes on an S3 backend "have no business
/// here" and should mint a URL instead. **Archive export is the documented
/// exception, and it is an exception by arithmetic, not by preference:**
/// `blobs_digest` is `canonical_hash` over the blobs' *bytes*. There is no
/// formulation of this function that produces a correct digest without
/// reading them. Unlike the tree walk — which reads only small JSON index
/// objects — this really does pull full file payloads into the process. That
/// is inherent to what an export *is* (a self-contained copy), it is bounded
/// by an operator-initiated, non-periodic request rather than a GC tick, and
/// it is the cost of the alternative to a digest that is silently wrong. See
/// this slice's report for the runtime-bridge consequence (finding #11 /
/// slice 6.1), which this call site makes materially more expensive.
///
/// ## Which failures are fatal, and why they are not all the same
///
/// Slice 2.2's [`HydrateError`] split is load-bearing here — the three
/// variants are three different facts and get three different answers:
///
/// * [`HydrateError::Unhydratable`] → **fatal.** A fact about the
///   *deployment* (S3 Delegate mode holds no bucket credentials and the
///   delegate presign protocol has no bytes-fetch op). The plan permits
///   "explicit warning + manifest marker" as an alternative to failing, but
///   not here: a manifest whose blobs digest is the empty-set digest is one
///   that mismatches **every** Dir-backed peer's digest for the same room. A
///   marker admitting that does not make the artifact usable; it annotates
///   finding #7's damage instead of preventing it. An operator can act on
///   this (configure Direct-mode credentials, or export from a node that
///   has them), so the honest answer is a loud, actionable refusal at export
///   time.
/// * [`HydrateError::Backend`] → **fatal.** Transient and retryable, and it
///   says nothing about whether the object exists. Dropping it would mint a
///   *signed* manifest whose digest permanently disagrees with every peer
///   because of one 503, with no error anywhere. Retrying is cheap; a wrong
///   signed digest is forever.
/// * [`HydrateError::Missing`] → **tolerated**, counted and warned. This is
///   a fact about the **data**, not the backend: a Dir-backed peer and an
///   S3-backed peer both see it and both exclude the hash, so the digests
///   still agree — which is precisely the property finding #7 is about.
///   Failing here would instead brick export forever for any room carrying a
///   dangling `SetBlob` (a node committed for an upload that never
///   completed), with no operator remedy, and would regress today's
///   `DirPersistence` behavior, which finding #7 does not allege is wrong.
///   Note a **corrupt** blob arrives here too (`DirPersistence::get_blob`
///   verifies BLAKE3 on read; see slice 3.2) — so a Dir peer holding a
///   corrupt blob and an S3 peer holding a good one *will* disagree on the
///   digest. That disagreement is correct: they genuinely hold different
///   bytes, and a digest that hid it would be worse than one that surfaces
///   it.
///
/// The dividing line: **fail on faults an operator can act on; count, warn
/// and continue on facts about the data that every peer shares.**
pub(crate) fn resolve_referenced_blobs(
    persistence: &dyn ServerPersistence,
    nodes: &[SyncNode],
) -> Result<Vec<(Hash, Vec<u8>)>, String> {
    let mut blobs: Vec<(Hash, Vec<u8>)> = Vec::new();
    let mut missing = 0usize;

    for hash in crate::room::blob_hashes_referenced_by(nodes.iter()) {
        match persistence.hydrate_blob(&hash) {
            Ok(bytes) => blobs.push((hash, bytes)),
            Err(HydrateError::Missing) => {
                missing += 1;
                metrics::counter!(
                    "nodalmerge_archive_blob_unresolved_total",
                    "reason" => "missing"
                )
                .increment(1);
                tracing::warn!(
                    hash = %hash.to_hex(),
                    "archive export: a referenced blob's bytes are not present (dangling \
                     SetBlob, or a corrupt blob failing verify-on-read). It is excluded \
                     from the blobs digest — which is what a peer that also lacks it \
                     computes, so digests still agree — but the exported archive is \
                     incomplete for this hash"
                );
            }
            Err(HydrateError::Unhydratable { backend, detail }) => {
                metrics::counter!(
                    "nodalmerge_archive_blob_unresolved_total",
                    "reason" => "unhydratable_backend"
                )
                .increment(1);
                tracing::error!(
                    hash = %hash.to_hex(),
                    %backend,
                    %detail,
                    "archive export refused: this blob backend can never read blob bytes \
                     into the server process, so any archive it produced would carry a \
                     blobs digest computed over an incomplete set"
                );
                return Err(format!(
                    "DEPLOYMENT CONFIGURATION: archive export cannot hydrate referenced \
                     blob {hash}: this server's blob backend ({backend}) can never read \
                     blob bytes into the server process, so the export's blobs digest \
                     would be computed over an incomplete set and would not match a \
                     Dir-backed peer's digest for the same room. This is a configuration \
                     fact, not data loss — the object is very likely intact in the \
                     bucket. Backend detail: {detail}",
                    hash = hash.to_hex(),
                ));
            }
            Err(HydrateError::Backend(detail)) => {
                metrics::counter!(
                    "nodalmerge_archive_blob_unresolved_total",
                    "reason" => "backend_error"
                )
                .increment(1);
                tracing::error!(
                    hash = %hash.to_hex(),
                    %detail,
                    "archive export aborted: a referenced blob could not be read this \
                     attempt. Retryable — deliberately not treated as an absent blob"
                );
                return Err(format!(
                    "referenced blob {hash} could not be read from the blob backend: \
                     {detail}. The export was aborted rather than sign a digest over an \
                     incomplete blob set; this is retryable",
                    hash = hash.to_hex(),
                ));
            }
        }
    }

    if missing > 0 {
        tracing::warn!(
            resolved = blobs.len(),
            missing,
            "archive export: {missing} referenced blob(s) had no bytes and are excluded \
             from the archive and its blobs digest"
        );
    }
    Ok(blobs)
}

fn blobs_digest_for_referenced(
    persistence: &dyn ServerPersistence,
    nodes: &[SyncNode],
) -> Result<String, String> {
    let map: BTreeMap<String, Vec<u8>> = resolve_referenced_blobs(persistence, nodes)?
        .into_iter()
        .map(|(hash, bytes)| (hash.to_hex(), bytes))
        .collect();
    Ok(format!("sha256:{}", canonical_hash(&map).to_hex()))
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
