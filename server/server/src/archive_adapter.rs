use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use ed25519_dalek::{Signature, SigningKey, Verifier, VerifyingKey};
use nodalmerge_core::{
    canonical_hash, pack_nodes, replay, ArchiveCheckpoint, ArchiveCompatibilityWindow,
    ArchiveDescribed, ArchiveExported, ArchiveImported, ArchivePayloadDigestSet, ArchiveProvenance,
    ArchiveReasonClass, ArchiveRejected, ArchiveValidated, ArchiveWsRequest, ArchiveWsResponse,
    BlobStore, Hash, SyncNode,
};
use serde_json::Value;

use crate::archive_export::{
    build_external_manifest_document, policy_timeline_metadata_for_timeline, write_file_manifest,
    write_object_manifest, EXPORT_COMPAT_MAX_SUPPORTED, EXPORT_COMPAT_MIN_SUPPORTED,
    EXPORT_DIGEST_POLICY_STRICT_SHA256_V1, EXPORT_FORMAT_VERSION,
};
use crate::room::{import_nodes, Room};

struct LoadedArchive {
    source_room: String,
    manifest_policy_timeline_hash: Option<String>,
    manifest_policy_timeline_cutover_lamport: Option<u64>,
    manifest_policy_timeline_transition_cutovers: Option<Vec<u64>>,
    nodes: Vec<SyncNode>,
    blobs: Vec<(Hash, Vec<u8>)>,
    checkpoint_hash: Option<String>,
    frontier: Vec<String>,
    nodes_digest: Option<String>,
    blobs_digest: Option<String>,
}

pub async fn process_archive_describe(
    room: &Arc<Room>,
    current_room_id: &str,
    message: &Value,
) -> Result<ArchiveWsResponse, ArchiveRejected> {
    let operation_start = Instant::now();
    let req = parse_archive_describe_request(current_room_id, message)?;
    let ArchiveWsRequest::Describe {
        room: _,
        archive_ref,
    } = req
    else {
        unreachable!("describe parser must return describe request")
    };

    let load_start = Instant::now();
    let loaded = load_archive_from_ref(room, current_room_id, &archive_ref, true)?;
    observe_archive_stage("describe", "manifest_load", load_start);

    let checkpoint_hash = loaded
        .checkpoint_hash
        .clone()
        .expect("describe load must include checkpoint hash");
    let nodes_digest = loaded
        .nodes_digest
        .clone()
        .expect("describe load must include node digest");
    let blobs_digest = loaded
        .blobs_digest
        .clone()
        .expect("describe load must include blob digest");

    observe_archive_stage("describe", "total", operation_start);
    Ok(ArchiveWsResponse::DescribeResult(ArchiveDescribed {
        room: current_room_id.to_string(),
        archive_ref,
        manifest_id: format!(
            "m.{}.{}",
            loaded.source_room,
            &checkpoint_hash[..12.min(checkpoint_hash.len())]
        ),
        format_version: "1".to_string(),
        archive_kind: "full_clone".to_string(),
        checkpoint: ArchiveCheckpoint {
            frontier: loaded.frontier,
            canonical_hash: checkpoint_hash,
        },
        payload_digest_set: ArchivePayloadDigestSet {
            nodes: nodes_digest,
            blobs: blobs_digest,
        },
        compatibility_window: Some(ArchiveCompatibilityWindow {
            min_supported: "1".to_string(),
            max_supported: "1".to_string(),
        }),
        provenance: Some(ArchiveProvenance {
            source_room: loaded.source_room,
            tool: "server-archive-adapter".to_string(),
        }),
    }))
}

pub async fn process_archive_validate(
    room: &Arc<Room>,
    current_room_id: &str,
    message: &Value,
) -> ArchiveWsResponse {
    let operation_start = Instant::now();
    let req = match parse_archive_validate_request(current_room_id, message) {
        Ok(req) => req,
        Err(rejected) => return ArchiveWsResponse::ValidateRejected(rejected),
    };

    let ArchiveWsRequest::Validate {
        room: _,
        archive_ref,
        mode,
    } = req
    else {
        unreachable!("validate parser must return validate request")
    };

    if mode != "metadata_only" && mode != "full_integrity" {
        return ArchiveWsResponse::ValidateRejected(rejected(
            current_room_id,
            &archive_ref,
            ArchiveReasonClass::ManifestInvalid,
            "unsupported validate mode",
        ));
    }

    let load_start = Instant::now();
    let loaded = match load_archive_from_ref(room, current_room_id, &archive_ref, false) {
        Ok(loaded) => loaded,
        Err(rejected) => return ArchiveWsResponse::ValidateRejected(rejected),
    };
    observe_archive_stage("validate", "manifest_load", load_start);

    if let (Some(manifest_policy_timeline_hash), Some(manifest_policy_timeline_cutover_lamport)) = (
        &loaded.manifest_policy_timeline_hash,
        loaded.manifest_policy_timeline_cutover_lamport,
    ) {
        let timeline_validation_start = Instant::now();
        let current_policy_timeline = current_room_policy_timeline_metadata(room).await;
        if manifest_policy_timeline_hash != &current_policy_timeline.hash_hex
            || manifest_policy_timeline_cutover_lamport != current_policy_timeline.cutover_lamport
        {
            return ArchiveWsResponse::ValidateRejected(rejected(
                current_room_id,
                &archive_ref,
                ArchiveReasonClass::PolicyTimelineMismatch,
                "external archive manifest policy timeline parity metadata mismatches target room policy",
            ));
        }
        if let Some(transition_cutovers) = &loaded.manifest_policy_timeline_transition_cutovers {
            if transition_cutovers != &current_policy_timeline.transition_cutovers {
                return ArchiveWsResponse::ValidateRejected(rejected(
                    current_room_id,
                    &archive_ref,
                    ArchiveReasonClass::PolicyTimelineMismatch,
                    "external archive manifest policy timeline transition metadata mismatches target room policy",
                ));
            }
        }
        observe_archive_stage("validate", "timeline_validation", timeline_validation_start);
    }

    if mode == "full_integrity" {
        let digest_check_start = Instant::now();
        if loaded
            .blobs
            .iter()
            .any(|(expected_hash, bytes)| Hash::of(bytes) != *expected_hash)
        {
            return ArchiveWsResponse::ValidateRejected(rejected(
                current_room_id,
                &archive_ref,
                ArchiveReasonClass::DigestMismatch,
                "blob digest verification failed",
            ));
        }
        observe_archive_stage("validate", "digest_check", digest_check_start);
    }

    let checks = if mode == "full_integrity" {
        vec![
            "manifest".to_string(),
            "compatibility".to_string(),
            "digest_set".to_string(),
            "checkpoint".to_string(),
            "policy_timeline".to_string(),
        ]
    } else {
        vec!["manifest".to_string(), "compatibility".to_string()]
    };

    observe_archive_stage("validate", "total", operation_start);
    ArchiveWsResponse::ValidateResult(ArchiveValidated {
        room: current_room_id.to_string(),
        archive_ref,
        accepted: true,
        mode,
        checks,
        compatibility_window: Some(ArchiveCompatibilityWindow {
            min_supported: "1".to_string(),
            max_supported: "1".to_string(),
        }),
    })
}

pub async fn process_archive_import(
    room: &Arc<Room>,
    current_room_id: &str,
    message: &Value,
) -> ArchiveWsResponse {
    let operation_start = Instant::now();
    let req = match parse_archive_import_request(current_room_id, message) {
        Ok(req) => req,
        Err(rejected) => return ArchiveWsResponse::ImportRejected(rejected),
    };

    let ArchiveWsRequest::Import {
        room: _,
        archive_ref,
        import_mode,
        expected_checkpoint,
    } = req
    else {
        unreachable!("import parser must return import request")
    };

    if import_mode != "full_apply" && import_mode != "metadata_only" {
        return ArchiveWsResponse::ImportRejected(rejected(
            current_room_id,
            &archive_ref,
            ArchiveReasonClass::ManifestInvalid,
            "unsupported import mode",
        ));
    }

    let load_start = Instant::now();
    let loaded = match load_archive_from_ref(room, current_room_id, &archive_ref, false) {
        Ok(loaded) => loaded,
        Err(rejected) => return ArchiveWsResponse::ImportRejected(rejected),
    };
    observe_archive_stage("import", "manifest_load", load_start);

    if let (Some(manifest_policy_timeline_hash), Some(manifest_policy_timeline_cutover_lamport)) = (
        &loaded.manifest_policy_timeline_hash,
        loaded.manifest_policy_timeline_cutover_lamport,
    ) {
        let timeline_validation_start = Instant::now();
        let current_policy_timeline = current_room_policy_timeline_metadata(room).await;
        if manifest_policy_timeline_hash != &current_policy_timeline.hash_hex
            || manifest_policy_timeline_cutover_lamport != current_policy_timeline.cutover_lamport
        {
            return ArchiveWsResponse::ImportRejected(rejected(
                current_room_id,
                &archive_ref,
                ArchiveReasonClass::PolicyTimelineMismatch,
                "external archive manifest policy timeline parity metadata mismatches target room policy",
            ));
        }
        if let Some(transition_cutovers) = &loaded.manifest_policy_timeline_transition_cutovers {
            if transition_cutovers != &current_policy_timeline.transition_cutovers {
                return ArchiveWsResponse::ImportRejected(rejected(
                    current_room_id,
                    &archive_ref,
                    ArchiveReasonClass::PolicyTimelineMismatch,
                    "external archive manifest policy timeline transition metadata mismatches target room policy",
                ));
            }
        }
        observe_archive_stage("import", "timeline_validation", timeline_validation_start);
    }

    let loaded_frontier = loaded.frontier.clone();
    let import_apply_start = Instant::now();
    let (accepted, _, errors) = import_nodes(room, loaded.nodes.clone()).await;
    observe_archive_stage("import", "import_apply", import_apply_start);
    if !errors.is_empty() {
        return ArchiveWsResponse::ImportRejected(rejected(
            current_room_id,
            &archive_ref,
            ArchiveReasonClass::ManifestInvalid,
            &format!("import node verification failed: {}", errors.join("; ")),
        ));
    }

    let imported_blobs = if import_mode == "metadata_only" {
        0u64
    } else {
        let mut store = room.blobs.write().await;
        for (hash, bytes) in &loaded.blobs {
            store.put(bytes.clone());
            room.persistence.persist_blob(hash, bytes);
        }
        loaded.blobs.len() as u64
    };

    let imported_state = room.graph.read().await.resolve();
    let imported_checkpoint_hash = canonical_hash(
        &imported_state
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<BTreeMap<String, Vec<u8>>>(),
    )
    .to_hex();

    if let Some(expected) = expected_checkpoint {
        if expected.canonical_hash != imported_checkpoint_hash {
            return ArchiveWsResponse::ImportRejected(rejected(
                current_room_id,
                &archive_ref,
                ArchiveReasonClass::DigestMismatch,
                "expected checkpoint canonical hash mismatch",
            ));
        }
    }

    observe_archive_stage("import", "total", operation_start);
    ArchiveWsResponse::ImportCompleted(ArchiveImported {
        room: current_room_id.to_string(),
        archive_ref,
        canonical_hash: imported_checkpoint_hash.clone(),
        checkpoint: ArchiveCheckpoint {
            frontier: loaded_frontier,
            canonical_hash: imported_checkpoint_hash,
        },
        imported_nodes: accepted as u64,
        imported_blobs,
    })
}

pub async fn process_archive_export(
    room: &Arc<Room>,
    current_room_id: &str,
    message: &Value,
    signer: &SigningKey,
) -> ArchiveWsResponse {
    let operation_start = Instant::now();
    let req = match parse_archive_export_request(current_room_id, message) {
        Ok(req) => req,
        Err(rejected) => return ArchiveWsResponse::ExportRejected(rejected),
    };

    let ArchiveWsRequest::Export {
        room: _,
        source_room,
        archive_ref,
    } = req
    else {
        unreachable!("export parser must return export request")
    };

    let timeline_metadata_start = Instant::now();
    let export_policy_timeline = current_room_policy_timeline_metadata(room).await;
    observe_archive_stage("export", "timeline_validation", timeline_metadata_start);

    let manifest_build_start = Instant::now();
    let manifest = match build_external_manifest_document(
        &*room.persistence,
        &source_room,
        signer,
        &export_policy_timeline.hash_hex,
        export_policy_timeline.cutover_lamport,
        &export_policy_timeline.transition_cutovers,
    ) {
        Ok(manifest) => manifest,
        Err(reason_message) => {
            return ArchiveWsResponse::ExportRejected(rejected(
                current_room_id,
                &archive_ref,
                ArchiveReasonClass::CheckpointNotFound,
                &reason_message,
            ));
        }
    };
    observe_archive_stage("export", "manifest_build", manifest_build_start);

    let manifest_write_start = Instant::now();
    let write_result = match parse_archive_ref(&archive_ref) {
        Ok(ArchiveSourceRef::FileManifest { path }) => write_file_manifest(&path, &manifest),
        Ok(ArchiveSourceRef::ObjectManifest { bucket, key }) => {
            let Some(root) = std::env::var("NODALMERGE_ARCHIVE_OBJECT_ROOT").ok() else {
                return ArchiveWsResponse::ExportRejected(rejected(
                    current_room_id,
                    &archive_ref,
                    ArchiveReasonClass::CheckpointNotFound,
                    "object archive root is not configured",
                ));
            };
            write_object_manifest(Path::new(&root), &bucket, &key, &manifest)
        }
        Ok(ArchiveSourceRef::Room { .. }) => {
            return ArchiveWsResponse::ExportRejected(rejected(
                current_room_id,
                &archive_ref,
                ArchiveReasonClass::UnsupportedFormat,
                "archive.export destination must be file:// or object://",
            ));
        }
        Err(_) => {
            return ArchiveWsResponse::ExportRejected(rejected(
                current_room_id,
                &archive_ref,
                ArchiveReasonClass::UnsupportedFormat,
                "archive.export destination must be file:// or object://",
            ));
        }
    };
    observe_archive_stage("export", "manifest_write", manifest_write_start);

    if let Err(reason_message) = write_result {
        return ArchiveWsResponse::ExportRejected(rejected(
            current_room_id,
            &archive_ref,
            ArchiveReasonClass::ManifestInvalid,
            &format!("failed to write archive manifest: {reason_message}"),
        ));
    }

    observe_archive_stage("export", "total", operation_start);
    ArchiveWsResponse::ExportResult(ArchiveExported {
        room: current_room_id.to_string(),
        source_room: source_room.clone(),
        archive_ref,
        manifest_id: format!(
            "m.{}.{}",
            source_room,
            &manifest.checkpoint.canonical_hash[..12.min(manifest.checkpoint.canonical_hash.len())]
        ),
        checkpoint: ArchiveCheckpoint {
            frontier: manifest.checkpoint.frontier,
            canonical_hash: manifest.checkpoint.canonical_hash,
        },
        payload_digest_set: nodalmerge_core::ArchivePayloadDigestSet {
            nodes: manifest.payload_digest_set.nodes,
            blobs: manifest.payload_digest_set.blobs,
        },
        compatibility_window: ArchiveCompatibilityWindow {
            min_supported: manifest.compatibility_window.min_supported,
            max_supported: manifest.compatibility_window.max_supported,
        },
        payload_digest_policy: manifest.payload_digest_policy,
        policy_timeline_hash: manifest.policy_timeline_hash,
        policy_timeline_cutover_lamport: manifest.policy_timeline_cutover_lamport,
        policy_timeline_transition_cutovers: manifest.policy_timeline_transition_cutovers,
    })
}

async fn current_room_policy_timeline_metadata(
    room: &Arc<Room>,
) -> crate::archive_export::PolicyTimelineParityMetadata {
    let timeline = room.current_policy_timeline().await;
    policy_timeline_metadata_for_timeline(&timeline)
}

fn parse_archive_describe_request(
    current_room_id: &str,
    message: &Value,
) -> Result<ArchiveWsRequest, ArchiveRejected> {
    let room = message["room"]
        .as_str()
        .unwrap_or(current_room_id)
        .to_string();
    let archive_ref = message["archive_ref"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_string();
    if archive_ref.is_empty() {
        return Err(rejected(
            current_room_id,
            "",
            ArchiveReasonClass::ManifestInvalid,
            "missing archive_ref",
        ));
    }
    Ok(ArchiveWsRequest::Describe { room, archive_ref })
}

fn parse_archive_validate_request(
    current_room_id: &str,
    message: &Value,
) -> Result<ArchiveWsRequest, ArchiveRejected> {
    let room = message["room"]
        .as_str()
        .unwrap_or(current_room_id)
        .to_string();
    let archive_ref = message["archive_ref"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_string();
    if archive_ref.is_empty() {
        return Err(rejected(
            current_room_id,
            "",
            ArchiveReasonClass::ManifestInvalid,
            "missing archive_ref",
        ));
    }
    let mode = message["mode"]
        .as_str()
        .unwrap_or("metadata_only")
        .to_string();
    Ok(ArchiveWsRequest::Validate {
        room,
        archive_ref,
        mode,
    })
}

fn parse_archive_import_request(
    current_room_id: &str,
    message: &Value,
) -> Result<ArchiveWsRequest, ArchiveRejected> {
    let room = message["room"]
        .as_str()
        .unwrap_or(current_room_id)
        .to_string();
    let archive_ref = message["archive_ref"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_string();
    if archive_ref.is_empty() {
        return Err(rejected(
            current_room_id,
            "",
            ArchiveReasonClass::ManifestInvalid,
            "missing archive_ref",
        ));
    }

    let import_mode = message["import_mode"]
        .as_str()
        .unwrap_or("full_apply")
        .to_string();
    let expected_checkpoint = message
        .get("expected_checkpoint")
        .and_then(|v| serde_json::from_value::<ArchiveCheckpoint>(v.clone()).ok());

    Ok(ArchiveWsRequest::Import {
        room,
        archive_ref,
        import_mode,
        expected_checkpoint,
    })
}

fn parse_archive_export_request(
    current_room_id: &str,
    message: &Value,
) -> Result<ArchiveWsRequest, ArchiveRejected> {
    let room = message["room"]
        .as_str()
        .unwrap_or(current_room_id)
        .to_string();
    let source_room = message["source_room"]
        .as_str()
        .unwrap_or(current_room_id)
        .trim()
        .to_string();
    if source_room.is_empty() {
        return Err(rejected(
            current_room_id,
            "",
            ArchiveReasonClass::ManifestInvalid,
            "missing source_room",
        ));
    }
    let archive_ref = message["archive_ref"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_string();
    if archive_ref.is_empty() {
        return Err(rejected(
            current_room_id,
            "",
            ArchiveReasonClass::ManifestInvalid,
            "missing archive_ref",
        ));
    }

    Ok(ArchiveWsRequest::Export {
        room,
        source_room,
        archive_ref,
    })
}

fn parse_archive_ref(archive_ref: &str) -> Result<ArchiveSourceRef, ArchiveReasonClass> {
    const ROOM_PREFIX: &str = "room://";
    const FILE_PREFIX: &str = "file://";
    const OBJECT_PREFIX: &str = "object://";

    if let Some(room) = archive_ref.strip_prefix(ROOM_PREFIX) {
        let source_room = room.trim();
        if source_room.is_empty() {
            return Err(ArchiveReasonClass::ManifestInvalid);
        }
        return Ok(ArchiveSourceRef::Room {
            source_room: source_room.to_string(),
        });
    }

    if let Some(path_text) = archive_ref.strip_prefix(FILE_PREFIX) {
        let path_text = path_text.trim();
        if path_text.is_empty() {
            return Err(ArchiveReasonClass::ManifestInvalid);
        }

        let normalized = if path_text.starts_with('/') && path_text.len() >= 3 {
            let bytes = path_text.as_bytes();
            if bytes[2] == b':' {
                &path_text[1..]
            } else {
                path_text
            }
        } else {
            path_text
        };

        return Ok(ArchiveSourceRef::FileManifest {
            path: PathBuf::from(normalized),
        });
    }

    if let Some(ref_text) = archive_ref.strip_prefix(OBJECT_PREFIX) {
        let ref_text = ref_text.trim();
        let mut parts = ref_text.splitn(2, '/');
        let bucket = parts.next().unwrap_or_default().trim();
        let key = parts.next().unwrap_or_default().trim();
        if bucket.is_empty() || key.is_empty() {
            return Err(ArchiveReasonClass::ManifestInvalid);
        }

        return Ok(ArchiveSourceRef::ObjectManifest {
            bucket: bucket.to_string(),
            key: key.to_string(),
        });
    }

    Err(ArchiveReasonClass::UnsupportedFormat)
}

fn load_archive_from_ref(
    room: &Arc<Room>,
    current_room_id: &str,
    archive_ref: &str,
    include_summary_digests: bool,
) -> Result<LoadedArchive, ArchiveRejected> {
    let source_ref = match parse_archive_ref(archive_ref) {
        Ok(source_ref) => source_ref,
        Err(reason_class) => {
            return Err(rejected(
                current_room_id,
                archive_ref,
                reason_class,
                "unsupported archive reference format",
            ));
        }
    };

    let external_manifest_metadata = match source_ref {
        ArchiveSourceRef::Room { source_room } => ExternalManifestMetadata {
            source_room,
            policy_timeline_hash: None,
            policy_timeline_cutover_lamport: None,
            policy_timeline_transition_cutovers: None,
        },
        ArchiveSourceRef::FileManifest { path } => {
            load_external_manifest_source_room(&path, current_room_id, archive_ref)?
        }
        ArchiveSourceRef::ObjectManifest { bucket, key } => {
            let path = resolve_object_manifest_path(&bucket, &key).ok_or_else(|| {
                rejected(
                    current_room_id,
                    archive_ref,
                    ArchiveReasonClass::CheckpointNotFound,
                    "object archive root is not configured",
                )
            })?;
            load_external_manifest_source_room(&path, current_room_id, archive_ref)?
        }
    };

    let source_room = external_manifest_metadata.source_room.clone();

    let mut nodes = room.persistence.load_room_nodes(&source_room);
    if nodes.is_empty() {
        return Err(rejected(
            current_room_id,
            archive_ref,
            ArchiveReasonClass::CheckpointNotFound,
            "archive source room has no persisted nodes",
        ));
    }

    nodes.sort_by(|a, b| a.id.cmp(&b.id));

    // Blobs are a global CAS pool (docs/BLOB_STORAGE_LAYOUT.md); recover
    // the source room's blob set via point lookups against the hashes its
    // own nodes reference.
    let blobs: Vec<(Hash, Vec<u8>)> = crate::room::blob_hashes_referenced_by(nodes.iter())
        .into_iter()
        .filter_map(|hash| room.persistence.get_blob(&hash).map(|bytes| (hash, bytes)))
        .collect();

    let (checkpoint_hash, nodes_digest, blobs_digest) = if include_summary_digests {
        let replayed = replay(&nodes, None).map_err(|_| {
            rejected(
                current_room_id,
                archive_ref,
                ArchiveReasonClass::ManifestInvalid,
                "persisted archive nodes could not be replayed",
            )
        })?;

        let checkpoint_hash = canonical_hash(
            &replayed
                .map
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<BTreeMap<String, Vec<u8>>>(),
        )
        .to_hex();

        let node_refs: Vec<&SyncNode> = nodes.iter().collect();
        let nodes_pack = pack_nodes(&node_refs);
        let nodes_digest = format!("sha256:{}", Hash::of(&nodes_pack).to_hex());

        let blob_digest_map: BTreeMap<String, Vec<u8>> = blobs
            .iter()
            .map(|(hash, bytes)| (hash.to_hex(), bytes.clone()))
            .collect();
        let blobs_digest = format!("sha256:{}", canonical_hash(&blob_digest_map).to_hex());
        (
            Some(checkpoint_hash),
            Some(nodes_digest),
            Some(blobs_digest),
        )
    } else {
        (None, None, None)
    };

    let node_count = nodes.len();

    Ok(LoadedArchive {
        source_room,
        manifest_policy_timeline_hash: external_manifest_metadata.policy_timeline_hash,
        manifest_policy_timeline_cutover_lamport: external_manifest_metadata
            .policy_timeline_cutover_lamport,
        manifest_policy_timeline_transition_cutovers: external_manifest_metadata
            .policy_timeline_transition_cutovers,
        nodes,
        blobs,
        checkpoint_hash,
        frontier: vec![format!("seq:{}", node_count)],
        nodes_digest,
        blobs_digest,
    })
}

fn resolve_object_manifest_path(bucket: &str, key: &str) -> Option<PathBuf> {
    let root = std::env::var("NODALMERGE_ARCHIVE_OBJECT_ROOT").ok()?;
    Some(Path::new(&root).join(bucket).join(key))
}

fn load_external_manifest_source_room(
    path: &Path,
    current_room_id: &str,
    archive_ref: &str,
) -> Result<ExternalManifestMetadata, ArchiveRejected> {
    let cache_key = path.to_path_buf();
    let revision = manifest_file_revision(path);
    let cache_lookup_start = Instant::now();
    if let Some(cached) = load_cached_external_manifest_metadata(&cache_key, revision) {
        observe_manifest_cache_lookup("hit", archive_ref, cache_lookup_start);
        return Ok(cached);
    }
    observe_manifest_cache_lookup("miss", archive_ref, cache_lookup_start);

    let raw = std::fs::read_to_string(path).map_err(|_| {
        rejected(
            current_room_id,
            archive_ref,
            ArchiveReasonClass::CheckpointNotFound,
            "external archive manifest was not found",
        )
    })?;

    let manifest: ExternalArchiveManifest = serde_json::from_str(&raw).map_err(|_| {
        rejected(
            current_room_id,
            archive_ref,
            ArchiveReasonClass::ManifestInvalid,
            "external archive manifest is invalid",
        )
    })?;

    if manifest.format_version.trim() != EXPORT_FORMAT_VERSION {
        return Err(rejected(
            current_room_id,
            archive_ref,
            ArchiveReasonClass::UnsupportedFormat,
            "external archive manifest format_version is unsupported",
        ));
    }

    let min_supported = manifest.compatibility_window.min_supported.trim();
    let max_supported = manifest.compatibility_window.max_supported.trim();
    let manifest_window = parse_compatibility_window_range(min_supported, max_supported)
        .ok_or_else(|| {
            rejected(
                current_room_id,
                archive_ref,
                ArchiveReasonClass::ManifestInvalid,
                "external archive manifest compatibility_window is invalid",
            )
        })?;
    let runtime_window =
        parse_compatibility_window_range(EXPORT_COMPAT_MIN_SUPPORTED, EXPORT_COMPAT_MAX_SUPPORTED)
            .expect("runtime compatibility window constants must be valid");

    if !ranges_overlap(&manifest_window, &runtime_window)
        || !manifest_window.contains(
            &parse_format_version(EXPORT_FORMAT_VERSION)
                .expect("runtime format version must parse"),
        )
    {
        return Err(rejected(
            current_room_id,
            archive_ref,
            ArchiveReasonClass::UnsupportedFormat,
            "external archive manifest compatibility_window is unsupported by runtime",
        ));
    }

    if manifest.policy_timeline_hash.trim().is_empty() {
        return Err(rejected(
            current_room_id,
            archive_ref,
            ArchiveReasonClass::ManifestInvalid,
            "external archive manifest policy_timeline_hash is required",
        ));
    }
    if manifest.payload_digest_policy.trim() != EXPORT_DIGEST_POLICY_STRICT_SHA256_V1 {
        return Err(rejected(
            current_room_id,
            archive_ref,
            ArchiveReasonClass::PolicyTimelineMismatch,
            "external archive manifest payload_digest_policy is unsupported",
        ));
    }

    verify_external_manifest_signature(&manifest).map_err(|reason_message| {
        rejected(
            current_room_id,
            archive_ref,
            ArchiveReasonClass::SignatureInvalid,
            &reason_message,
        )
    })?;

    let source_room = manifest.source_room.trim();
    if source_room.is_empty() {
        return Err(rejected(
            current_room_id,
            archive_ref,
            ArchiveReasonClass::ManifestInvalid,
            "external archive manifest source_room is required",
        ));
    }

    if manifest.policy_timeline_transition_cutovers.is_empty() {
        return Err(rejected(
            current_room_id,
            archive_ref,
            ArchiveReasonClass::ManifestInvalid,
            "external archive manifest policy_timeline_transition_cutovers is required",
        ));
    }
    let mut previous = None;
    for &cutover in &manifest.policy_timeline_transition_cutovers {
        if let Some(prev) = previous {
            if cutover <= prev {
                return Err(rejected(
                    current_room_id,
                    archive_ref,
                    ArchiveReasonClass::ManifestInvalid,
                    "external archive manifest policy_timeline_transition_cutovers must be strictly increasing",
                ));
            }
        }
        previous = Some(cutover);
    }
    if manifest.policy_timeline_transition_cutovers.last().copied()
        != Some(manifest.policy_timeline_cutover_lamport)
    {
        return Err(rejected(
            current_room_id,
            archive_ref,
            ArchiveReasonClass::ManifestInvalid,
            "external archive manifest policy_timeline_transition_cutovers must end at policy_timeline_cutover_lamport",
        ));
    }

    let metadata = ExternalManifestMetadata {
        source_room: source_room.to_string(),
        policy_timeline_hash: Some(manifest.policy_timeline_hash.trim().to_string()),
        policy_timeline_cutover_lamport: Some(manifest.policy_timeline_cutover_lamport),
        policy_timeline_transition_cutovers: Some(manifest.policy_timeline_transition_cutovers),
    };

    cache_external_manifest_metadata(cache_key, revision, &metadata);
    Ok(metadata)
}

fn manifest_file_revision(path: &Path) -> Option<ManifestCacheRevision> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    let modified_unix_nanos = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(ManifestCacheRevision {
        len_bytes: metadata.len(),
        modified_unix_nanos,
    })
}

fn external_manifest_cache() -> &'static Mutex<HashMap<PathBuf, CachedExternalManifestMetadata>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedExternalManifestMetadata>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(test)]
fn clear_external_manifest_cache() {
    let cache = external_manifest_cache();
    if let Ok(mut guard) = cache.lock() {
        guard.clear();
    }
}

fn load_cached_external_manifest_metadata(
    path: &PathBuf,
    revision: Option<ManifestCacheRevision>,
) -> Option<ExternalManifestMetadata> {
    let revision = revision?;
    let cache = external_manifest_cache();
    let guard = cache.lock().ok()?;
    let cached = guard.get(path)?;
    if cached.revision != revision {
        return None;
    }
    Some(cached.metadata.clone())
}

fn cache_external_manifest_metadata(
    path: PathBuf,
    revision: Option<ManifestCacheRevision>,
    metadata: &ExternalManifestMetadata,
) {
    let Some(revision) = revision else {
        return;
    };
    let cache = external_manifest_cache();
    if let Ok(mut guard) = cache.lock() {
        guard.insert(
            path,
            CachedExternalManifestMetadata {
                revision,
                metadata: metadata.clone(),
            },
        );
    }
}

fn observe_manifest_cache_lookup(outcome: &str, archive_ref: &str, started_at: Instant) {
    let source = if archive_ref.starts_with("object://") {
        "object"
    } else {
        "file"
    };
    metrics::counter!(
        "nodalmerge_archive_manifest_cache_lookup_total",
        "outcome" => outcome.to_string(),
        "source" => source.to_string()
    )
    .increment(1);
    metrics::histogram!(
        "nodalmerge_archive_manifest_cache_lookup_seconds",
        "outcome" => outcome.to_string(),
        "source" => source.to_string()
    )
    .record(started_at.elapsed().as_secs_f64());
}

fn verify_external_manifest_signature(manifest: &ExternalArchiveManifest) -> Result<(), String> {
    let Some(sig) = &manifest.signature else {
        return Err("external archive manifest signature is required".to_string());
    };

    let public_key_bytes = decode_hex_exact(&sig.public_key, 32)
        .ok_or_else(|| "external manifest public key hex is invalid".to_string())?;
    let signature_bytes = decode_hex_exact(&sig.signature, 64)
        .ok_or_else(|| "external manifest signature hex is invalid".to_string())?;

    let public_key_array: [u8; 32] = public_key_bytes
        .as_slice()
        .try_into()
        .map_err(|_| "external manifest public key length is invalid".to_string())?;
    let signature_array: [u8; 64] = signature_bytes
        .as_slice()
        .try_into()
        .map_err(|_| "external manifest signature length is invalid".to_string())?;

    let public_key = VerifyingKey::from_bytes(&public_key_array)
        .map_err(|_| "external manifest public key decode failed".to_string())?;
    let signature = Signature::from_bytes(&signature_array);

    let payload = external_manifest_signature_payload(manifest);
    public_key
        .verify(payload.as_bytes(), &signature)
        .map_err(|_| "external manifest signature verification failed".to_string())
}

fn external_manifest_signature_payload(manifest: &ExternalArchiveManifest) -> String {
    let transition_cutovers = manifest
        .policy_timeline_transition_cutovers
        .iter()
        .map(u64::to_string)
        .collect::<Vec<String>>()
        .join(",");
    if transition_cutovers.is_empty() {
        return format!(
            "format_version={}|source_room={}|min_supported={}|max_supported={}|payload_digest_policy={}|policy_timeline_hash={}|policy_timeline_cutover_lamport={}",
            manifest.format_version,
            manifest.source_room,
            manifest.compatibility_window.min_supported,
            manifest.compatibility_window.max_supported,
            manifest.payload_digest_policy,
            manifest.policy_timeline_hash,
            manifest.policy_timeline_cutover_lamport,
        );
    }
    format!(
        "format_version={}|source_room={}|min_supported={}|max_supported={}|payload_digest_policy={}|policy_timeline_hash={}|policy_timeline_cutover_lamport={}|policy_timeline_transition_cutovers={}",
        manifest.format_version,
        manifest.source_room,
        manifest.compatibility_window.min_supported,
        manifest.compatibility_window.max_supported,
        manifest.payload_digest_policy,
        manifest.policy_timeline_hash,
        manifest.policy_timeline_cutover_lamport,
        transition_cutovers,
    )
}

fn parse_format_version(version: &str) -> Option<u32> {
    version.trim().parse::<u32>().ok()
}

fn parse_compatibility_window_range(
    min_supported: &str,
    max_supported: &str,
) -> Option<std::ops::RangeInclusive<u32>> {
    let min = parse_format_version(min_supported)?;
    let max = parse_format_version(max_supported)?;
    if min > max {
        return None;
    }
    Some(min..=max)
}

fn ranges_overlap(a: &std::ops::RangeInclusive<u32>, b: &std::ops::RangeInclusive<u32>) -> bool {
    a.start() <= b.end() && b.start() <= a.end()
}

fn decode_hex_exact(input: &str, expected_len: usize) -> Option<Vec<u8>> {
    let trimmed = input.trim();
    if trimmed.len() != expected_len * 2 {
        return None;
    }

    let mut out = Vec::with_capacity(expected_len);
    let bytes = trimmed.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let hi = hex_nibble(bytes[i])?;
        let lo = hex_nibble(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }

    Some(out)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn observe_archive_stage(operation: &str, stage: &str, started_at: Instant) {
    metrics::histogram!(
        "nodalmerge_archive_operation_seconds",
        "operation" => operation.to_string(),
        "stage" => stage.to_string()
    )
    .record(started_at.elapsed().as_secs_f64());
}

fn rejected(
    room: &str,
    archive_ref: &str,
    reason_class: ArchiveReasonClass,
    reason_message: &str,
) -> ArchiveRejected {
    ArchiveRejected {
        room: room.to_string(),
        archive_ref: archive_ref.to_string(),
        reason_class,
        reason_message: reason_message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    use ed25519_dalek::{Signer, SigningKey};
    use nodalmerge_core::{MapOp, Op, StateGraph};

    use super::*;
    use crate::room::Room;
    use crate::store::{DirPersistence, SharedPersistence};

    fn percentile_ms(samples: &[f64], p: f64) -> f64 {
        let mut sorted = samples.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
        sorted[idx]
    }

    fn summarize_ms(samples: &[f64]) -> serde_json::Value {
        let min = samples
            .iter()
            .copied()
            .fold(f64::INFINITY, |acc, v| if v < acc { v } else { acc });
        let max = samples
            .iter()
            .copied()
            .fold(0.0f64, |acc, v| if v > acc { v } else { acc });
        serde_json::json!({
            "count": samples.len(),
            "min_ms": (min * 100.0).round() / 100.0,
            "p50_ms": (percentile_ms(samples, 0.50) * 100.0).round() / 100.0,
            "p95_ms": (percentile_ms(samples, 0.95) * 100.0).round() / 100.0,
            "max_ms": (max * 100.0).round() / 100.0
        })
    }

    fn tmpdir() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        let p = std::env::temp_dir().join(format!("nodalmerge-archive-adapter-test-{nanos}"));
        std::fs::create_dir_all(&p).expect("temp dir should be created");
        p
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

    fn write_external_manifest(path: &Path, source_room: &str, sign_with: Option<&SigningKey>) {
        let policy_timeline = crate::archive_export::policy_timeline_metadata_for_policy(
            &nodalmerge_core::Policy::default(),
        );
        let payload = format!(
            "format_version={}|source_room={}|min_supported={}|max_supported={}|payload_digest_policy={}|policy_timeline_hash={}|policy_timeline_cutover_lamport={}|policy_timeline_transition_cutovers={}",
            "1",
            source_room,
            "1",
            "2",
            "strict_sha256_v1",
            policy_timeline.hash_hex,
            policy_timeline.cutover_lamport,
            policy_timeline
                .transition_cutovers
                .iter()
                .map(u64::to_string)
                .collect::<Vec<String>>()
                .join(","),
        );
        let signature = sign_with.map(|signing_key| {
            let sig = signing_key.sign(payload.as_bytes());
            serde_json::json!({
                "public_key": hex_lower(&signing_key.verifying_key().to_bytes()),
                "signature": hex_lower(&sig.to_bytes()),
            })
        });

        let manifest = serde_json::json!({
            "format_version": "1",
            "source_room": source_room,
            "compatibility_window": {
                "min_supported": "1",
                "max_supported": "2"
            },
            "payload_digest_policy": "strict_sha256_v1",
            "policy_timeline_hash": policy_timeline.hash_hex,
            "policy_timeline_cutover_lamport": policy_timeline.cutover_lamport,
            "policy_timeline_transition_cutovers": policy_timeline.transition_cutovers,
            "signature": signature,
        });

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("manifest parent dir should exist");
        }
        std::fs::write(path, manifest.to_string()).expect("manifest file should be written");
    }

    async fn seed_source_room(room: &Arc<Room>) {
        let blob = b"archive-adapter-blob".to_vec();
        let hash = Hash::of(&blob);

        let mut g = StateGraph::new();
        let sk = SigningKey::from_bytes(&[0x77; 32]);
        let id = g
            .apply_local(
                &sk,
                0,
                vec![
                    Op::Map(MapOp::Set {
                        key: "world/a".to_string(),
                        value: b"1".to_vec(),
                    }),
                    Op::Map(MapOp::SetBlob {
                        key: "world/blob".to_string(),
                        blob_hash: hash,
                    }),
                ],
            )
            .expect("seed apply_local should succeed");
        let node = g
            .get_nodes(&[id])
            .into_iter()
            .next()
            .expect("seed node should be present")
            .clone();

        let _ = import_nodes(room, vec![node]).await;

        room.blobs.write().await.put(blob.clone());
        room.persistence.persist_blob(&hash, &blob);
    }

    #[tokio::test]
    async fn describe_and_import_use_persisted_archive_source() {
        let root = tmpdir();
        let persistence: SharedPersistence =
            Arc::new(DirPersistence::open(&root).expect("dir persistence should open"));

        let source_room = Room::new("source-room".to_string(), Arc::clone(&persistence), 64);
        seed_source_room(&source_room).await;

        let describe = process_archive_describe(
            &source_room,
            "target-room",
            &serde_json::json!({
                "type": "archive.describe",
                "archive_ref": "room://source-room"
            }),
        )
        .await
        .expect("describe should succeed");

        let ArchiveWsResponse::DescribeResult(envelope) = describe else {
            panic!("expected describe result response")
        };
        assert_eq!(envelope.archive_kind, "full_clone");
        assert!(envelope.payload_digest_set.nodes.starts_with("sha256:"));

        let target_room = Room::new("target-room".to_string(), Arc::clone(&persistence), 64);
        let import = process_archive_import(
            &target_room,
            "target-room",
            &serde_json::json!({
                "type": "archive.import",
                "archive_ref": "room://source-room",
                "import_mode": "full_apply"
            }),
        )
        .await;

        let ArchiveWsResponse::ImportCompleted(result) = import else {
            panic!("expected import completed response")
        };
        assert!(result.imported_nodes >= 1);
        assert!(result.imported_blobs >= 1);
    }

    #[tokio::test]
    async fn import_rejects_on_expected_checkpoint_mismatch() {
        let root = tmpdir();
        let persistence: SharedPersistence =
            Arc::new(DirPersistence::open(&root).expect("dir persistence should open"));
        let source_room = Room::new("source-room-2".to_string(), Arc::clone(&persistence), 64);
        seed_source_room(&source_room).await;
        let target_room = Room::new("target-room-2".to_string(), Arc::clone(&persistence), 64);

        let import = process_archive_import(
            &target_room,
            "target-room-2",
            &serde_json::json!({
                "type": "archive.import",
                "archive_ref": "room://source-room-2",
                "import_mode": "full_apply",
                "expected_checkpoint": {
                    "frontier": ["seq:999"],
                    "canonical_hash": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
                }
            }),
        )
        .await;

        let ArchiveWsResponse::ImportRejected(reject) = import else {
            panic!("expected import rejected response")
        };
        assert_eq!(reject.reason_class, ArchiveReasonClass::DigestMismatch);
    }

    #[tokio::test]
    async fn validate_rejects_external_file_manifest_with_invalid_signature() {
        let root = tmpdir();
        let persistence: SharedPersistence =
            Arc::new(DirPersistence::open(&root).expect("dir persistence should open"));
        let source_room = Room::new(
            "source-room-signature".to_string(),
            Arc::clone(&persistence),
            64,
        );
        seed_source_room(&source_room).await;
        let target_room = Room::new(
            "target-room-signature".to_string(),
            Arc::clone(&persistence),
            64,
        );

        let manifest_path = root.join("archives").join("invalid-signature.json");
        let signer_a = SigningKey::from_bytes(&[0x33; 32]);
        let signer_b = SigningKey::from_bytes(&[0x34; 32]);
        write_external_manifest(&manifest_path, "source-room-signature", Some(&signer_a));

        let policy_timeline = crate::archive_export::policy_timeline_metadata_for_policy(
            &nodalmerge_core::Policy::default(),
        );
        let payload = format!(
            "format_version={}|source_room={}|min_supported={}|max_supported={}|payload_digest_policy={}|policy_timeline_hash={}|policy_timeline_cutover_lamport={}|policy_timeline_transition_cutovers={}",
            "1",
            "source-room-signature",
            "1",
            "2",
            "strict_sha256_v1",
            policy_timeline.hash_hex,
            policy_timeline.cutover_lamport,
            policy_timeline
                .transition_cutovers
                .iter()
                .map(u64::to_string)
                .collect::<Vec<String>>()
                .join(","),
        );
        let forged_sig = signer_b.sign(payload.as_bytes());
        let forged_manifest = serde_json::json!({
            "format_version": "1",
            "source_room": "source-room-signature",
            "compatibility_window": {
                "min_supported": "1",
                "max_supported": "2"
            },
            "payload_digest_policy": "strict_sha256_v1",
            "policy_timeline_hash": policy_timeline.hash_hex,
            "policy_timeline_cutover_lamport": policy_timeline.cutover_lamport,
            "policy_timeline_transition_cutovers": policy_timeline.transition_cutovers,
            "signature": {
                "public_key": hex_lower(&signer_a.verifying_key().to_bytes()),
                "signature": hex_lower(&forged_sig.to_bytes())
            }
        });
        std::fs::write(&manifest_path, forged_manifest.to_string())
            .expect("forged manifest should be written");

        let response = process_archive_validate(
            &target_room,
            "target-room-signature",
            &serde_json::json!({
                "type": "archive.validate",
                "archive_ref": format!("file://{}", manifest_path.display()),
                "mode": "full_integrity"
            }),
        )
        .await;

        let ArchiveWsResponse::ValidateRejected(reject) = response else {
            panic!("expected validate rejected response")
        };
        assert_eq!(reject.reason_class, ArchiveReasonClass::SignatureInvalid);
    }

    #[tokio::test]
    async fn validate_rejects_external_object_manifest_when_source_checkpoint_missing() {
        let root = tmpdir();
        let persistence: SharedPersistence =
            Arc::new(DirPersistence::open(&root).expect("dir persistence should open"));
        let target_room = Room::new(
            "target-room-object".to_string(),
            Arc::clone(&persistence),
            64,
        );

        let signer = SigningKey::from_bytes(&[0x35; 32]);
        let object_root = root.join("object-root");
        let manifest_path = object_root.join("bucket-a").join("manifest.json");
        write_external_manifest(&manifest_path, "missing-source-room", Some(&signer));
        std::env::set_var(
            "NODALMERGE_ARCHIVE_OBJECT_ROOT",
            object_root.display().to_string(),
        );

        let response = process_archive_validate(
            &target_room,
            "target-room-object",
            &serde_json::json!({
                "type": "archive.validate",
                "archive_ref": "object://bucket-a/manifest.json",
                "mode": "full_integrity"
            }),
        )
        .await;

        let ArchiveWsResponse::ValidateRejected(reject) = response else {
            panic!("expected validate rejected response")
        };
        assert_eq!(reject.reason_class, ArchiveReasonClass::CheckpointNotFound);
        std::env::remove_var("NODALMERGE_ARCHIVE_OBJECT_ROOT");
    }

    #[tokio::test]
    async fn validate_rejects_unsupported_archive_ref_scheme() {
        let root = tmpdir();
        let persistence: SharedPersistence =
            Arc::new(DirPersistence::open(&root).expect("dir persistence should open"));
        let target_room = Room::new(
            "target-room-scheme".to_string(),
            Arc::clone(&persistence),
            64,
        );

        let response = process_archive_validate(
            &target_room,
            "target-room-scheme",
            &serde_json::json!({
                "type": "archive.validate",
                "archive_ref": "ftp://archive/source.nmar",
                "mode": "full_integrity"
            }),
        )
        .await;

        let ArchiveWsResponse::ValidateRejected(reject) = response else {
            panic!("expected validate rejected response")
        };
        assert_eq!(reject.reason_class, ArchiveReasonClass::UnsupportedFormat);
    }

    #[test]
    fn external_manifest_cache_reuses_and_invalidates_by_revision() {
        let root = tmpdir();
        let manifest_path = root.join("archives").join("cache-behavior.json");
        let signer = SigningKey::from_bytes(&[0x44; 32]);

        clear_external_manifest_cache();
        write_external_manifest(&manifest_path, "cache-source-a", Some(&signer));

        let first = load_external_manifest_source_room(
            &manifest_path,
            "cache-room",
            &format!("file://{}", manifest_path.display()),
        )
        .expect("first load should succeed");
        assert_eq!(first.source_room, "cache-source-a");

        let revision_a = manifest_file_revision(&manifest_path)
            .expect("first manifest revision should be available");
        let cached_after_first =
            load_cached_external_manifest_metadata(&manifest_path, Some(revision_a))
                .expect("cache should be populated after first load");
        assert_eq!(cached_after_first.source_room, "cache-source-a");

        let second = load_external_manifest_source_room(
            &manifest_path,
            "cache-room",
            &format!("file://{}", manifest_path.display()),
        )
        .expect("second load should reuse cache");
        assert_eq!(second.source_room, "cache-source-a");

        write_external_manifest(&manifest_path, "cache-source-b-longer", Some(&signer));

        let third = load_external_manifest_source_room(
            &manifest_path,
            "cache-room",
            &format!("file://{}", manifest_path.display()),
        )
        .expect("third load should invalidate stale cache");
        assert_eq!(third.source_room, "cache-source-b-longer");
    }

    #[tokio::test]
    async fn archive_profile_001_operation_runtime_sections_reports_p50_p95() {
        let iterations = 25usize;
        let root = tmpdir();
        let persistence: SharedPersistence =
            Arc::new(DirPersistence::open(&root).expect("dir persistence should open"));
        let source_room = Room::new(
            "archive-profile-source".to_string(),
            Arc::clone(&persistence),
            256,
        );
        seed_source_room(&source_room).await;

        let control_room = Room::new(
            "archive-profile-control".to_string(),
            Arc::clone(&persistence),
            256,
        );
        let manifest_path = root.join("exports").join("archive-profile.json");
        let archive_ref = format!("file://{}", manifest_path.display());
        let export_signer = SigningKey::from_bytes(&[0x7Cu8; 32]);

        let warmup_export = process_archive_export(
            &source_room,
            "archive-profile-control",
            &serde_json::json!({
                "type": "archive.export",
                "source_room": "archive-profile-source",
                "archive_ref": archive_ref.clone(),
            }),
            &export_signer,
        )
        .await;
        let ArchiveWsResponse::ExportResult(_) = warmup_export else {
            panic!("archive profile warmup export must succeed")
        };

        let mut operation_samples: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        let mut stage_samples: BTreeMap<String, Vec<f64>> = BTreeMap::new();

        for i in 0..iterations {
            let export_start = Instant::now();
            let export_response = process_archive_export(
                &source_room,
                "archive-profile-control",
                &serde_json::json!({
                    "type": "archive.export",
                    "source_room": "archive-profile-source",
                    "archive_ref": archive_ref.clone(),
                }),
                &export_signer,
            )
            .await;
            let ArchiveWsResponse::ExportResult(_) = export_response else {
                panic!("export operation sample must succeed")
            };
            operation_samples
                .entry("export_total_ms".to_string())
                .or_default()
                .push(export_start.elapsed().as_secs_f64() * 1000.0);

            let manifest_stage_start = Instant::now();
            let metadata = load_external_manifest_source_room(
                &manifest_path,
                "archive-profile-control",
                &archive_ref,
            )
            .expect("manifest load stage should succeed");
            stage_samples
                .entry("manifest_load_signature_ms".to_string())
                .or_default()
                .push(manifest_stage_start.elapsed().as_secs_f64() * 1000.0);

            let load_stage_start = Instant::now();
            let loaded = load_archive_from_ref(
                &control_room,
                "archive-profile-control",
                &archive_ref,
                false,
            )
            .expect("archive load stage should succeed");
            stage_samples
                .entry("archive_data_load_ms".to_string())
                .or_default()
                .push(load_stage_start.elapsed().as_secs_f64() * 1000.0);

            let timeline_stage_start = Instant::now();
            let timeline = current_room_policy_timeline_metadata(&control_room).await;
            assert_eq!(
                metadata.policy_timeline_hash.as_deref(),
                Some(timeline.hash_hex.as_str())
            );
            assert_eq!(
                metadata.policy_timeline_cutover_lamport,
                Some(timeline.cutover_lamport)
            );
            stage_samples
                .entry("timeline_validation_ms".to_string())
                .or_default()
                .push(timeline_stage_start.elapsed().as_secs_f64() * 1000.0);

            let digest_stage_start = Instant::now();
            let digest_ok = loaded
                .blobs
                .iter()
                .all(|(expected_hash, bytes)| Hash::of(bytes) == *expected_hash);
            assert!(digest_ok, "digest stage should pass");
            stage_samples
                .entry("digest_check_ms".to_string())
                .or_default()
                .push(digest_stage_start.elapsed().as_secs_f64() * 1000.0);

            let validate_start = Instant::now();
            let validate_response = process_archive_validate(
                &control_room,
                "archive-profile-control",
                &serde_json::json!({
                    "type": "archive.validate",
                    "archive_ref": archive_ref.clone(),
                    "mode": "full_integrity",
                }),
            )
            .await;
            let ArchiveWsResponse::ValidateResult(validated) = validate_response else {
                panic!("validate operation sample must succeed")
            };
            assert!(validated.accepted);
            operation_samples
                .entry("validate_total_ms".to_string())
                .or_default()
                .push(validate_start.elapsed().as_secs_f64() * 1000.0);

            let import_stage_room = Room::new(
                format!("archive-profile-stage-import-{i}"),
                Arc::clone(&persistence),
                256,
            );
            let import_apply_stage_start = Instant::now();
            let (_, _, errors) = import_nodes(&import_stage_room, loaded.nodes.clone()).await;
            assert!(errors.is_empty(), "import apply stage should not fail");
            stage_samples
                .entry("import_apply_ms".to_string())
                .or_default()
                .push(import_apply_stage_start.elapsed().as_secs_f64() * 1000.0);

            let import_operation_room = Room::new(
                format!("archive-profile-op-import-{i}"),
                Arc::clone(&persistence),
                256,
            );
            let import_start = Instant::now();
            let import_response = process_archive_import(
                &import_operation_room,
                &format!("archive-profile-op-import-{i}"),
                &serde_json::json!({
                    "type": "archive.import",
                    "archive_ref": archive_ref.clone(),
                    "import_mode": "full_apply",
                }),
            )
            .await;
            let ArchiveWsResponse::ImportCompleted(_) = import_response else {
                panic!("import operation sample must succeed")
            };
            operation_samples
                .entry("import_total_ms".to_string())
                .or_default()
                .push(import_start.elapsed().as_secs_f64() * 1000.0);
        }

        let mut operation_summary = serde_json::Map::new();
        for (key, samples) in &operation_samples {
            operation_summary.insert(key.clone(), summarize_ms(samples));
        }
        let mut stage_summary = serde_json::Map::new();
        for (key, samples) in &stage_samples {
            stage_summary.insert(key.clone(), summarize_ms(samples));
        }

        let summary = serde_json::json!({
            "iterations": iterations,
            "operation_runtime": operation_summary,
            "stage_runtime": stage_summary
        });
        println!(
            "ARCHIVE_PROFILE_RUN={}",
            serde_json::to_string(&summary).expect("profile summary must serialize")
        );
    }

    #[tokio::test]
    async fn archive_profile_002_object_manifest_parity_reports_p50_p95() {
        let iterations = 25usize;
        let root = tmpdir();
        let object_root = root.join("object-root");
        std::env::set_var(
            "NODALMERGE_ARCHIVE_OBJECT_ROOT",
            object_root.display().to_string(),
        );

        let persistence: SharedPersistence =
            Arc::new(DirPersistence::open(&root).expect("dir persistence should open"));
        let source_room = Room::new(
            "archive-profile-parity-source".to_string(),
            Arc::clone(&persistence),
            256,
        );
        seed_source_room(&source_room).await;

        let control_room = Room::new(
            "archive-profile-parity-control".to_string(),
            Arc::clone(&persistence),
            256,
        );
        let file_manifest_path = root
            .join("exports")
            .join("archive-profile-parity-file.json");
        let object_manifest_path = object_root
            .join("parity")
            .join("archive-profile-parity-object.json");
        let file_archive_ref = format!("file://{}", file_manifest_path.display());
        let object_archive_ref = "object://parity/archive-profile-parity-object.json".to_string();
        let export_signer = SigningKey::from_bytes(&[0x7Du8; 32]);

        let warmup_export_file = process_archive_export(
            &source_room,
            "archive-profile-parity-control",
            &serde_json::json!({
                "type": "archive.export",
                "source_room": "archive-profile-parity-source",
                "archive_ref": file_archive_ref.clone(),
            }),
            &export_signer,
        )
        .await;
        let ArchiveWsResponse::ExportResult(_) = warmup_export_file else {
            panic!("archive parity warmup file export must succeed")
        };

        let warmup_export_object = process_archive_export(
            &source_room,
            "archive-profile-parity-control",
            &serde_json::json!({
                "type": "archive.export",
                "source_room": "archive-profile-parity-source",
                "archive_ref": object_archive_ref.clone(),
            }),
            &export_signer,
        )
        .await;
        let ArchiveWsResponse::ExportResult(_) = warmup_export_object else {
            panic!("archive parity warmup object export must succeed")
        };

        let mut file_operation_samples: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        let mut file_stage_samples: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        let mut object_operation_samples: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        let mut object_stage_samples: BTreeMap<String, Vec<f64>> = BTreeMap::new();

        for i in 0..iterations {
            let file_export_start = Instant::now();
            let file_export_response = process_archive_export(
                &source_room,
                "archive-profile-parity-control",
                &serde_json::json!({
                    "type": "archive.export",
                    "source_room": "archive-profile-parity-source",
                    "archive_ref": file_archive_ref.clone(),
                }),
                &export_signer,
            )
            .await;
            let ArchiveWsResponse::ExportResult(_) = file_export_response else {
                panic!("file export sample must succeed")
            };
            file_operation_samples
                .entry("export_total_ms".to_string())
                .or_default()
                .push(file_export_start.elapsed().as_secs_f64() * 1000.0);

            let file_manifest_stage_start = Instant::now();
            let file_metadata = load_external_manifest_source_room(
                &file_manifest_path,
                "archive-profile-parity-control",
                &file_archive_ref,
            )
            .expect("file manifest load stage should succeed");
            file_stage_samples
                .entry("manifest_load_signature_ms".to_string())
                .or_default()
                .push(file_manifest_stage_start.elapsed().as_secs_f64() * 1000.0);

            let file_load_stage_start = Instant::now();
            let file_loaded = load_archive_from_ref(
                &control_room,
                "archive-profile-parity-control",
                &file_archive_ref,
                false,
            )
            .expect("file archive load stage should succeed");
            file_stage_samples
                .entry("archive_data_load_ms".to_string())
                .or_default()
                .push(file_load_stage_start.elapsed().as_secs_f64() * 1000.0);

            let file_timeline_stage_start = Instant::now();
            let timeline = current_room_policy_timeline_metadata(&control_room).await;
            assert_eq!(
                file_metadata.policy_timeline_hash.as_deref(),
                Some(timeline.hash_hex.as_str())
            );
            assert_eq!(
                file_metadata.policy_timeline_cutover_lamport,
                Some(timeline.cutover_lamport)
            );
            file_stage_samples
                .entry("timeline_validation_ms".to_string())
                .or_default()
                .push(file_timeline_stage_start.elapsed().as_secs_f64() * 1000.0);

            let file_digest_stage_start = Instant::now();
            let file_digest_ok = file_loaded
                .blobs
                .iter()
                .all(|(expected_hash, bytes)| Hash::of(bytes) == *expected_hash);
            assert!(file_digest_ok, "file digest stage should pass");
            file_stage_samples
                .entry("digest_check_ms".to_string())
                .or_default()
                .push(file_digest_stage_start.elapsed().as_secs_f64() * 1000.0);

            let file_validate_start = Instant::now();
            let file_validate_response = process_archive_validate(
                &control_room,
                "archive-profile-parity-control",
                &serde_json::json!({
                    "type": "archive.validate",
                    "archive_ref": file_archive_ref.clone(),
                    "mode": "full_integrity",
                }),
            )
            .await;
            let ArchiveWsResponse::ValidateResult(file_validated) = file_validate_response else {
                panic!("file validate sample must succeed")
            };
            assert!(file_validated.accepted);
            file_operation_samples
                .entry("validate_total_ms".to_string())
                .or_default()
                .push(file_validate_start.elapsed().as_secs_f64() * 1000.0);

            let file_import_stage_room = Room::new(
                format!("archive-profile-file-stage-import-{i}"),
                Arc::clone(&persistence),
                256,
            );
            let file_import_apply_stage_start = Instant::now();
            let (_, _, file_errors) =
                import_nodes(&file_import_stage_room, file_loaded.nodes.clone()).await;
            assert!(
                file_errors.is_empty(),
                "file import apply stage should not fail"
            );
            file_stage_samples
                .entry("import_apply_ms".to_string())
                .or_default()
                .push(file_import_apply_stage_start.elapsed().as_secs_f64() * 1000.0);

            let file_import_operation_room = Room::new(
                format!("archive-profile-file-op-import-{i}"),
                Arc::clone(&persistence),
                256,
            );
            let file_import_start = Instant::now();
            let file_import_response = process_archive_import(
                &file_import_operation_room,
                &format!("archive-profile-file-op-import-{i}"),
                &serde_json::json!({
                    "type": "archive.import",
                    "archive_ref": file_archive_ref.clone(),
                    "import_mode": "full_apply",
                }),
            )
            .await;
            let ArchiveWsResponse::ImportCompleted(_) = file_import_response else {
                panic!("file import operation sample must succeed")
            };
            file_operation_samples
                .entry("import_total_ms".to_string())
                .or_default()
                .push(file_import_start.elapsed().as_secs_f64() * 1000.0);

            let object_export_start = Instant::now();
            let object_export_response = process_archive_export(
                &source_room,
                "archive-profile-parity-control",
                &serde_json::json!({
                    "type": "archive.export",
                    "source_room": "archive-profile-parity-source",
                    "archive_ref": object_archive_ref.clone(),
                }),
                &export_signer,
            )
            .await;
            let ArchiveWsResponse::ExportResult(_) = object_export_response else {
                panic!("object export sample must succeed")
            };
            object_operation_samples
                .entry("export_total_ms".to_string())
                .or_default()
                .push(object_export_start.elapsed().as_secs_f64() * 1000.0);

            let object_manifest_stage_start = Instant::now();
            let object_metadata = load_external_manifest_source_room(
                &object_manifest_path,
                "archive-profile-parity-control",
                &object_archive_ref,
            )
            .expect("object manifest load stage should succeed");
            object_stage_samples
                .entry("manifest_load_signature_ms".to_string())
                .or_default()
                .push(object_manifest_stage_start.elapsed().as_secs_f64() * 1000.0);

            let object_load_stage_start = Instant::now();
            let object_loaded = load_archive_from_ref(
                &control_room,
                "archive-profile-parity-control",
                &object_archive_ref,
                false,
            )
            .expect("object archive load stage should succeed");
            object_stage_samples
                .entry("archive_data_load_ms".to_string())
                .or_default()
                .push(object_load_stage_start.elapsed().as_secs_f64() * 1000.0);

            let object_timeline_stage_start = Instant::now();
            let object_timeline = current_room_policy_timeline_metadata(&control_room).await;
            assert_eq!(
                object_metadata.policy_timeline_hash.as_deref(),
                Some(object_timeline.hash_hex.as_str())
            );
            assert_eq!(
                object_metadata.policy_timeline_cutover_lamport,
                Some(object_timeline.cutover_lamport)
            );
            object_stage_samples
                .entry("timeline_validation_ms".to_string())
                .or_default()
                .push(object_timeline_stage_start.elapsed().as_secs_f64() * 1000.0);

            let object_digest_stage_start = Instant::now();
            let object_digest_ok = object_loaded
                .blobs
                .iter()
                .all(|(expected_hash, bytes)| Hash::of(bytes) == *expected_hash);
            assert!(object_digest_ok, "object digest stage should pass");
            object_stage_samples
                .entry("digest_check_ms".to_string())
                .or_default()
                .push(object_digest_stage_start.elapsed().as_secs_f64() * 1000.0);

            let object_validate_start = Instant::now();
            let object_validate_response = process_archive_validate(
                &control_room,
                "archive-profile-parity-control",
                &serde_json::json!({
                    "type": "archive.validate",
                    "archive_ref": object_archive_ref.clone(),
                    "mode": "full_integrity",
                }),
            )
            .await;
            let ArchiveWsResponse::ValidateResult(object_validated) = object_validate_response
            else {
                panic!("object validate sample must succeed")
            };
            assert!(object_validated.accepted);
            object_operation_samples
                .entry("validate_total_ms".to_string())
                .or_default()
                .push(object_validate_start.elapsed().as_secs_f64() * 1000.0);

            let object_import_stage_room = Room::new(
                format!("archive-profile-object-stage-import-{i}"),
                Arc::clone(&persistence),
                256,
            );
            let object_import_apply_stage_start = Instant::now();
            let (_, _, object_errors) =
                import_nodes(&object_import_stage_room, object_loaded.nodes.clone()).await;
            assert!(
                object_errors.is_empty(),
                "object import apply stage should not fail"
            );
            object_stage_samples
                .entry("import_apply_ms".to_string())
                .or_default()
                .push(object_import_apply_stage_start.elapsed().as_secs_f64() * 1000.0);

            let object_import_operation_room = Room::new(
                format!("archive-profile-object-op-import-{i}"),
                Arc::clone(&persistence),
                256,
            );
            let object_import_start = Instant::now();
            let object_import_response = process_archive_import(
                &object_import_operation_room,
                &format!("archive-profile-object-op-import-{i}"),
                &serde_json::json!({
                    "type": "archive.import",
                    "archive_ref": object_archive_ref.clone(),
                    "import_mode": "full_apply",
                }),
            )
            .await;
            let ArchiveWsResponse::ImportCompleted(_) = object_import_response else {
                panic!("object import operation sample must succeed")
            };
            object_operation_samples
                .entry("import_total_ms".to_string())
                .or_default()
                .push(object_import_start.elapsed().as_secs_f64() * 1000.0);
        }

        let mut file_operation_summary = serde_json::Map::new();
        for (key, samples) in &file_operation_samples {
            file_operation_summary.insert(key.clone(), summarize_ms(samples));
        }
        let mut file_stage_summary = serde_json::Map::new();
        for (key, samples) in &file_stage_samples {
            file_stage_summary.insert(key.clone(), summarize_ms(samples));
        }

        let mut object_operation_summary = serde_json::Map::new();
        for (key, samples) in &object_operation_samples {
            object_operation_summary.insert(key.clone(), summarize_ms(samples));
        }
        let mut object_stage_summary = serde_json::Map::new();
        for (key, samples) in &object_stage_samples {
            object_stage_summary.insert(key.clone(), summarize_ms(samples));
        }

        let export_p95_delta_ms = (percentile_ms(
            object_operation_samples
                .get("export_total_ms")
                .expect("object export samples should exist"),
            0.95,
        ) - percentile_ms(
            file_operation_samples
                .get("export_total_ms")
                .expect("file export samples should exist"),
            0.95,
        ))
        .abs();
        let validate_p95_delta_ms = (percentile_ms(
            object_operation_samples
                .get("validate_total_ms")
                .expect("object validate samples should exist"),
            0.95,
        ) - percentile_ms(
            file_operation_samples
                .get("validate_total_ms")
                .expect("file validate samples should exist"),
            0.95,
        ))
        .abs();
        let import_p95_delta_ms = (percentile_ms(
            object_operation_samples
                .get("import_total_ms")
                .expect("object import samples should exist"),
            0.95,
        ) - percentile_ms(
            file_operation_samples
                .get("import_total_ms")
                .expect("file import samples should exist"),
            0.95,
        ))
        .abs();

        let parity_threshold_ms = 5.0;
        let summary = serde_json::json!({
            "iterations": iterations,
            "file": {
                "operation_runtime": file_operation_summary,
                "stage_runtime": file_stage_summary,
            },
            "object": {
                "operation_runtime": object_operation_summary,
                "stage_runtime": object_stage_summary,
            },
            "parity": {
                "threshold_ms": parity_threshold_ms,
                "export_p95_abs_delta_ms": (export_p95_delta_ms * 100.0).round() / 100.0,
                "validate_p95_abs_delta_ms": (validate_p95_delta_ms * 100.0).round() / 100.0,
                "import_p95_abs_delta_ms": (import_p95_delta_ms * 100.0).round() / 100.0,
                "pass": {
                    "export": export_p95_delta_ms <= parity_threshold_ms,
                    "validate": validate_p95_delta_ms <= parity_threshold_ms,
                    "import": import_p95_delta_ms <= parity_threshold_ms,
                }
            }
        });
        println!(
            "ARCHIVE_OBJECT_PARITY_RUN={}",
            serde_json::to_string(&summary).expect("object parity summary must serialize")
        );

        std::env::remove_var("NODALMERGE_ARCHIVE_OBJECT_ROOT");
    }
}

#[derive(Debug, Clone)]
enum ArchiveSourceRef {
    Room { source_room: String },
    FileManifest { path: PathBuf },
    ObjectManifest { bucket: String, key: String },
}

#[derive(Debug, Clone)]
struct ExternalManifestMetadata {
    source_room: String,
    policy_timeline_hash: Option<String>,
    policy_timeline_cutover_lamport: Option<u64>,
    policy_timeline_transition_cutovers: Option<Vec<u64>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ManifestCacheRevision {
    len_bytes: u64,
    modified_unix_nanos: u128,
}

#[derive(Debug, Clone)]
struct CachedExternalManifestMetadata {
    revision: ManifestCacheRevision,
    metadata: ExternalManifestMetadata,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct ExternalArchiveManifest {
    format_version: String,
    source_room: String,
    compatibility_window: ExternalArchiveCompatibilityWindow,
    payload_digest_policy: String,
    policy_timeline_hash: String,
    policy_timeline_cutover_lamport: u64,
    #[serde(default)]
    policy_timeline_transition_cutovers: Vec<u64>,
    #[serde(default)]
    signature: Option<ExternalArchiveSignature>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct ExternalArchiveCompatibilityWindow {
    min_supported: String,
    max_supported: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct ExternalArchiveSignature {
    public_key: String,
    signature: String,
}
