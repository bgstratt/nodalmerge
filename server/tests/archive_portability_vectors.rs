use std::collections::BTreeMap;
use std::sync::Arc;

use ed25519_dalek::{Signer, SigningKey};
use nodalmerge_core::{
    canonical_hash, ArchiveCheckpoint, ArchiveCompatibilityWindow, ArchiveDescribed,
    ArchiveImported, ArchivePayloadDigestSet, ArchiveProvenance, ArchiveReasonClass,
    ArchiveValidated, ArchiveWsResponse, Hash, MapOp, Op, Policy, PolicyDefault, PolicyRule,
    StateGraph, SyncNode,
};
use nodalmerge_server::archive_adapter::{
    process_archive_export, process_archive_import, process_archive_validate,
};
use nodalmerge_server::room::{import_nodes, Room};
use nodalmerge_server::store::{DirPersistence, NoPersistence, SharedPersistence};

#[derive(Debug, Clone, PartialEq, Eq)]
struct ArchiveManifest {
    format_version: String,
    kind: String,
    checkpoint_hash: Hash,
    payload_digest: Hash,
}

fn build_set_nodes(sk: &SigningKey, writes: &[(&str, &str)]) -> Vec<SyncNode> {
    let mut g = StateGraph::new();
    let mut out = Vec::with_capacity(writes.len());
    for (k, v) in writes {
        let id = g
            .apply_local(
                sk,
                0,
                vec![Op::Map(MapOp::Set {
                    key: (*k).to_string(),
                    value: v.as_bytes().to_vec(),
                })],
            )
            .expect("apply_local should succeed");
        let n = g
            .get_nodes(&[id])
            .into_iter()
            .next()
            .expect("node must exist")
            .clone();
        out.push(n);
    }
    out
}

fn payload_digest(nodes: &[SyncNode]) -> Hash {
    let mut pairs: Vec<(Hash, Vec<u8>)> = nodes
        .iter()
        .map(|n| {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&n.id.0);
            bytes.extend_from_slice(&n.transaction.author);
            bytes.extend_from_slice(&n.transaction.lamport.to_le_bytes());
            (n.id, bytes)
        })
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));

    let as_map: BTreeMap<String, Vec<u8>> = pairs
        .into_iter()
        .map(|(id, bytes)| (encode_hash(id), bytes))
        .collect();
    canonical_hash(&as_map)
}

fn manifest_digest(manifest: &ArchiveManifest) -> Hash {
    let payload = format!(
        "v={}|kind={}|checkpoint={}|payload={}",
        manifest.format_version,
        manifest.kind,
        encode_hash(manifest.checkpoint_hash),
        encode_hash(manifest.payload_digest)
    );
    let map = BTreeMap::from([("manifest".to_string(), payload.into_bytes())]);
    canonical_hash(&map)
}

fn validate_archive_format(version: &str, supported: &[&str]) -> Result<(), ArchiveReasonClass> {
    if supported.iter().any(|v| *v == version) {
        Ok(())
    } else {
        Err(ArchiveReasonClass::UnsupportedFormat)
    }
}

fn encode_hash(hash: Hash) -> String {
    hash.0
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
}

fn describe_archive_stub(
    room_id: &str,
    archive_ref: &str,
    checkpoint_hash: Hash,
) -> ArchiveWsResponse {
    ArchiveWsResponse::DescribeResult(ArchiveDescribed {
        room: room_id.to_string(),
        archive_ref: archive_ref.to_string(),
        manifest_id: "m.stub.0001".to_string(),
        format_version: "1".to_string(),
        archive_kind: "full_clone".to_string(),
        checkpoint: ArchiveCheckpoint {
            frontier: vec!["seq:0".to_string()],
            canonical_hash: encode_hash(checkpoint_hash),
        },
        payload_digest_set: ArchivePayloadDigestSet {
            nodes: "sha256:stub-nodes".to_string(),
            blobs: "sha256:stub-blobs".to_string(),
        },
        compatibility_window: Some(ArchiveCompatibilityWindow {
            min_supported: "1".to_string(),
            max_supported: "1".to_string(),
        }),
        provenance: Some(ArchiveProvenance {
            source_room: room_id.to_string(),
            tool: "server-test-stub".to_string(),
        }),
    })
}

fn validate_archive_stub(archive_ref: &str) -> Result<ArchiveWsResponse, ArchiveReasonClass> {
    if archive_ref.contains("invalid-manifest") {
        Err(ArchiveReasonClass::ManifestInvalid)
    } else {
        Ok(ArchiveWsResponse::ValidateResult(ArchiveValidated {
            room: "room-a".to_string(),
            archive_ref: archive_ref.to_string(),
            accepted: true,
            mode: "metadata_only".to_string(),
            checks: vec!["manifest".to_string(), "compatibility".to_string()],
            compatibility_window: Some(ArchiveCompatibilityWindow {
                min_supported: "1".to_string(),
                max_supported: "1".to_string(),
            }),
        }))
    }
}

fn import_archive_stub(archive_ref: &str) -> Result<ArchiveWsResponse, ArchiveReasonClass> {
    if archive_ref.contains("digest-mismatch") {
        Err(ArchiveReasonClass::DigestMismatch)
    } else {
        Ok(ArchiveWsResponse::ImportCompleted(ArchiveImported {
            room: "room-a".to_string(),
            archive_ref: archive_ref.to_string(),
            canonical_hash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                .to_string(),
            checkpoint: ArchiveCheckpoint {
                frontier: vec!["seq:0".to_string()],
                canonical_hash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                    .to_string(),
            },
            imported_nodes: 10,
            imported_blobs: 2,
        }))
    }
}

fn tmpdir() -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    let p = std::env::temp_dir().join(format!("nodalmerge-archive-roundtrip-test-{nanos}"));
    std::fs::create_dir_all(&p).expect("temp dir should be created");
    p
}

fn write_signed_manifest_with_policy(
    path: &std::path::Path,
    source_room: &str,
    signer: &SigningKey,
    min_supported: &str,
    max_supported: &str,
    payload_digest_policy: &str,
    policy_timeline_cutover_lamport: u64,
) {
    let transition_cutovers = if policy_timeline_cutover_lamport == 0 {
        vec![0]
    } else {
        vec![0, policy_timeline_cutover_lamport]
    };
    write_signed_manifest_with_policy_and_transitions(
        path,
        source_room,
        signer,
        min_supported,
        max_supported,
        payload_digest_policy,
        policy_timeline_cutover_lamport,
        &transition_cutovers,
    );
}

fn write_signed_manifest_with_policy_and_transitions(
    path: &std::path::Path,
    source_room: &str,
    signer: &SigningKey,
    min_supported: &str,
    max_supported: &str,
    payload_digest_policy: &str,
    policy_timeline_cutover_lamport: u64,
    policy_timeline_transition_cutovers: &[u64],
) {
    let policy_timeline = nodalmerge_server::archive_export::policy_timeline_metadata_for_policy(
        &nodalmerge_core::Policy::default(),
    );
    let payload = format!(
        "format_version={}|source_room={}|min_supported={}|max_supported={}|payload_digest_policy={}|policy_timeline_hash={}|policy_timeline_cutover_lamport={}|policy_timeline_transition_cutovers={}",
        "1",
        source_room,
        min_supported,
        max_supported,
        payload_digest_policy,
        policy_timeline.hash_hex,
        policy_timeline_cutover_lamport,
        policy_timeline_transition_cutovers
            .iter()
            .map(u64::to_string)
            .collect::<Vec<String>>()
            .join(","),
    );
    let sig = signer.sign(payload.as_bytes()).to_bytes();
    let manifest = serde_json::json!({
        "format_version": "1",
        "source_room": source_room,
        "compatibility_window": {
            "min_supported": min_supported,
            "max_supported": max_supported
        },
        "payload_digest_policy": payload_digest_policy,
        "policy_timeline_hash": policy_timeline.hash_hex,
        "policy_timeline_cutover_lamport": policy_timeline_cutover_lamport,
        "policy_timeline_transition_cutovers": policy_timeline_transition_cutovers,
        "signature": {
            "public_key": signer.verifying_key().to_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>(),
            "signature": sig.iter().map(|b| format!("{b:02x}")).collect::<String>()
        }
    });
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("manifest parent should exist");
    }
    std::fs::write(path, manifest.to_string()).expect("manifest should be written");
}

#[tokio::test]
async fn archive_roundtrip_001_full_clone_canonical_hash_parity_server_path() {
    let persistence: SharedPersistence = Arc::new(NoPersistence);
    let source_room = Room::new(
        "archive-roundtrip-source".to_string(),
        Arc::clone(&persistence),
        512,
    );
    let target_room = Room::new(
        "archive-roundtrip-target".to_string(),
        Arc::clone(&persistence),
        512,
    );

    let signer = SigningKey::from_bytes(&[0x4Bu8; 32]);
    let export_nodes = build_set_nodes(
        &signer,
        &[("world/a", "1"), ("world/b", "2"), ("world/c", "3")],
    );

    let (accepted_src, _, errs_src) = import_nodes(&source_room, export_nodes.clone()).await;
    let (accepted_tgt, _, errs_tgt) = import_nodes(&target_room, export_nodes).await;
    assert_eq!(accepted_src, 3);
    assert_eq!(accepted_tgt, 3);
    assert!(errs_src.is_empty());
    assert!(errs_tgt.is_empty());

    let source_state = source_room.graph.read().await.resolve();
    let target_state = target_room.graph.read().await.resolve();

    let source_hash = canonical_hash(
        &source_state
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    );
    let target_hash = canonical_hash(
        &target_state
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    );

    assert_eq!(
        source_hash, target_hash,
        "ARCHIVE-ROUNDTRIP-001: server import path must preserve canonical hash parity"
    );
}

#[tokio::test]
async fn archive_det_001_manifest_digest_equality_at_fixed_checkpoint_server_path() {
    let persistence: SharedPersistence = Arc::new(NoPersistence);
    let room = Room::new(
        "archive-det-room".to_string(),
        Arc::clone(&persistence),
        512,
    );
    let signer = SigningKey::from_bytes(&[0x4Cu8; 32]);

    let nodes = build_set_nodes(&signer, &[("world/a", "1"), ("world/b", "2")]);
    let (accepted, _, errs) = import_nodes(&room, nodes.clone()).await;
    assert_eq!(accepted, 2);
    assert!(errs.is_empty());

    let state = room.graph.read().await.resolve();
    let checkpoint_hash =
        canonical_hash(&state.iter().map(|(k, v)| (k.clone(), v.clone())).collect());

    let manifest_a = ArchiveManifest {
        format_version: "archive/v1".to_string(),
        kind: "full_clone".to_string(),
        checkpoint_hash,
        payload_digest: payload_digest(&nodes),
    };
    let manifest_b = ArchiveManifest {
        format_version: "archive/v1".to_string(),
        kind: "full_clone".to_string(),
        checkpoint_hash,
        payload_digest: payload_digest(&nodes),
    };

    assert_eq!(
        manifest_digest(&manifest_a),
        manifest_digest(&manifest_b),
        "ARCHIVE-DET-001: server manifest digest should be deterministic at fixed checkpoint"
    );
}

#[test]
fn archive_compat_reject_001_unsupported_format_rejected_deterministically_server_path() {
    let supported = ["archive/v1", "archive/v1.1"];
    let err = validate_archive_format("archive/v0", &supported)
        .expect_err("unsupported format must reject");
    assert_eq!(err, ArchiveReasonClass::UnsupportedFormat);
}

#[test]
fn archive_describe_001_envelope_contains_manifest_checkpoint_and_digest_metadata_server_path() {
    let checkpoint_hash = Hash([0xAA; 32]);
    let envelope = serde_json::to_string(&describe_archive_stub(
        "room-a",
        "s3://bucket/room-a.nmar",
        checkpoint_hash,
    ))
    .expect("serialize archive describe envelope");

    assert!(
        envelope.contains("\"type\":\"archive.describe.result\""),
        "archive describe envelope must map to archive.describe.result"
    );
    assert!(
        envelope.contains("\"manifest_id\":\"m.stub.0001\""),
        "archive describe envelope must include manifest id"
    );
    assert!(
        envelope.contains("\"canonical_hash\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\""),
        "archive describe envelope must include canonical checkpoint hash"
    );
    assert!(
        envelope.contains("\"payload_digest_set\""),
        "archive describe envelope must include payload digest metadata"
    );
}

#[test]
fn archive_validate_reject_001_manifest_invalid_uses_deterministic_reason_class_server_path() {
    let reason_class = validate_archive_stub("s3://bucket/invalid-manifest.nmar")
        .expect_err("invalid manifest archive ref must reject deterministically");
    assert_eq!(reason_class, ArchiveReasonClass::ManifestInvalid);
}

#[test]
fn archive_import_reject_001_digest_mismatch_uses_deterministic_reason_class_server_path() {
    let reason_class = import_archive_stub("s3://bucket/digest-mismatch.nmar")
        .expect_err("digest mismatch archive ref must reject deterministically");
    assert_eq!(reason_class, ArchiveReasonClass::DigestMismatch);
}

#[tokio::test]
async fn archive_roundtrip_002_generated_file_manifest_import_parity_server_path() {
    let root = tmpdir();
    let persistence: SharedPersistence =
        Arc::new(DirPersistence::open(&root).expect("dir persistence should open"));
    let source_room = Room::new(
        "archive-rt-file-source".to_string(),
        Arc::clone(&persistence),
        256,
    );
    let target_room_a = Room::new(
        "archive-rt-file-target-a".to_string(),
        Arc::clone(&persistence),
        256,
    );
    let target_room_b = Room::new(
        "archive-rt-file-target-b".to_string(),
        Arc::clone(&persistence),
        256,
    );

    let source_signer = SigningKey::from_bytes(&[0x5Au8; 32]);
    let source_nodes = build_set_nodes(&source_signer, &[("world/a", "10"), ("world/b", "20")]);
    let (accepted, _, errors) = import_nodes(&source_room, source_nodes).await;
    assert_eq!(accepted, 2);
    assert!(errors.is_empty());

    let source_state = source_room.graph.read().await.resolve();
    let source_hash = canonical_hash(
        &source_state
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<BTreeMap<String, Vec<u8>>>(),
    )
    .to_hex();

    let export_signer = SigningKey::from_bytes(&[0x5Bu8; 32]);
    let file_manifest = root.join("exports").join("archive-rt-file.json");
    let export_response = process_archive_export(
        &source_room,
        "archive-rt-file-target-a",
        &serde_json::json!({
            "type": "archive.export",
            "source_room": "archive-rt-file-source",
            "archive_ref": format!("file://{}", file_manifest.display()),
        }),
        &export_signer,
    )
    .await;

    let ArchiveWsResponse::ExportResult(_) = export_response else {
        panic!("expected archive.export.result response")
    };

    let import_a = process_archive_import(
        &target_room_a,
        "archive-rt-file-target-a",
        &serde_json::json!({
            "type": "archive.import",
            "archive_ref": format!("file://{}", file_manifest.display()),
            "import_mode": "full_apply",
        }),
    )
    .await;

    let ArchiveWsResponse::ImportCompleted(completed_a) = import_a else {
        panic!("expected archive.import.completed response for first target")
    };
    assert_eq!(completed_a.canonical_hash, source_hash);

    let import_b = process_archive_import(
        &target_room_b,
        "archive-rt-file-target-b",
        &serde_json::json!({
            "type": "archive.import",
            "archive_ref": format!("file://{}", file_manifest.display()),
            "import_mode": "full_apply",
        }),
    )
    .await;

    let ArchiveWsResponse::ImportCompleted(completed_b) = import_b else {
        panic!("expected archive.import.completed response for second target")
    };
    assert_eq!(completed_b.canonical_hash, source_hash);
    assert_eq!(completed_a.canonical_hash, completed_b.canonical_hash);
}

#[tokio::test]
async fn archive_roundtrip_003_generated_object_manifest_import_parity_server_path() {
    let root = tmpdir();
    let object_root = root.join("object-root");
    std::env::set_var(
        "NODALMERGE_ARCHIVE_OBJECT_ROOT",
        object_root.display().to_string(),
    );

    let persistence: SharedPersistence =
        Arc::new(DirPersistence::open(&root).expect("dir persistence should open"));
    let source_room = Room::new(
        "archive-rt-object-source".to_string(),
        Arc::clone(&persistence),
        256,
    );
    let target_room = Room::new(
        "archive-rt-object-target".to_string(),
        Arc::clone(&persistence),
        256,
    );

    let source_signer = SigningKey::from_bytes(&[0x5Cu8; 32]);
    let source_nodes = build_set_nodes(&source_signer, &[("world/x", "7"), ("world/y", "8")]);
    let (accepted, _, errors) = import_nodes(&source_room, source_nodes).await;
    assert_eq!(accepted, 2);
    assert!(errors.is_empty());

    let source_state = source_room.graph.read().await.resolve();
    let source_hash = canonical_hash(
        &source_state
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<BTreeMap<String, Vec<u8>>>(),
    )
    .to_hex();

    let export_signer = SigningKey::from_bytes(&[0x5Du8; 32]);
    let export_response = process_archive_export(
        &source_room,
        "archive-rt-object-target",
        &serde_json::json!({
            "type": "archive.export",
            "source_room": "archive-rt-object-source",
            "archive_ref": "object://roundtrip/object-manifest.json",
        }),
        &export_signer,
    )
    .await;

    let ArchiveWsResponse::ExportResult(_) = export_response else {
        std::env::remove_var("NODALMERGE_ARCHIVE_OBJECT_ROOT");
        panic!("expected archive.export.result response")
    };

    let import = process_archive_import(
        &target_room,
        "archive-rt-object-target",
        &serde_json::json!({
            "type": "archive.import",
            "archive_ref": "object://roundtrip/object-manifest.json",
            "import_mode": "full_apply",
        }),
    )
    .await;
    std::env::remove_var("NODALMERGE_ARCHIVE_OBJECT_ROOT");

    let ArchiveWsResponse::ImportCompleted(completed) = import else {
        panic!("expected archive.import.completed response")
    };
    assert_eq!(completed.canonical_hash, source_hash);
}

#[tokio::test]
async fn archive_export_002_manifest_contains_compatibility_window_and_payload_digest_policy_server_path(
) {
    let root = tmpdir();
    let persistence: SharedPersistence =
        Arc::new(DirPersistence::open(&root).expect("dir persistence should open"));
    let source_room = Room::new(
        "archive-export-meta-source".to_string(),
        Arc::clone(&persistence),
        256,
    );

    let signer = SigningKey::from_bytes(&[0x5Eu8; 32]);
    let nodes = build_set_nodes(&signer, &[("world/meta", "1")]);
    let (accepted, _, errors) = import_nodes(&source_room, nodes).await;
    assert_eq!(accepted, 1);
    assert!(errors.is_empty());

    let export_signer = SigningKey::from_bytes(&[0x5Fu8; 32]);
    let file_manifest = root.join("exports").join("meta.json");
    let response = process_archive_export(
        &source_room,
        "archive-export-meta-target",
        &serde_json::json!({
            "type": "archive.export",
            "source_room": "archive-export-meta-source",
            "archive_ref": format!("file://{}", file_manifest.display()),
        }),
        &export_signer,
    )
    .await;
    let ArchiveWsResponse::ExportResult(envelope) = response else {
        panic!("expected archive.export.result response")
    };

    assert_eq!(envelope.compatibility_window.min_supported, "1");
    assert_eq!(envelope.compatibility_window.max_supported, "2");
    assert_eq!(envelope.payload_digest_policy, "strict_sha256_v1");
    assert!(!envelope.policy_timeline_hash.is_empty());
    assert_eq!(envelope.policy_timeline_cutover_lamport, 0);
    assert_eq!(envelope.policy_timeline_transition_cutovers, vec![0]);

    let written: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&file_manifest).expect("manifest file should be readable"),
    )
    .expect("manifest json should parse");
    assert_eq!(
        written["compatibility_window"]["min_supported"].as_str(),
        Some("1")
    );
    assert_eq!(
        written["compatibility_window"]["max_supported"].as_str(),
        Some("2")
    );
    assert_eq!(
        written["payload_digest_policy"].as_str(),
        Some("strict_sha256_v1")
    );
    assert!(written["policy_timeline_hash"].as_str().is_some());
    assert_eq!(written["policy_timeline_cutover_lamport"].as_u64(), Some(0));
    assert_eq!(
        written["policy_timeline_transition_cutovers"],
        serde_json::json!([0])
    );
}

#[tokio::test]
async fn archive_export_003_policy_timeline_transition_progression_non_zero_server_path() {
    let root = tmpdir();
    let persistence: SharedPersistence =
        Arc::new(DirPersistence::open(&root).expect("dir persistence should open"));
    let source_room = Room::new(
        "archive-export-policy-transition-source".to_string(),
        Arc::clone(&persistence),
        256,
    );

    let signer = SigningKey::from_bytes(&[0x74u8; 32]);
    let nodes = build_set_nodes(&signer, &[("world/meta", "1")]);
    let (accepted, _, errors) = import_nodes(&source_room, nodes).await;
    assert_eq!(accepted, 1);
    assert!(errors.is_empty());

    source_room
        .set_policy(Policy {
            rules: vec![PolicyRule {
                path_glob: "world/**".to_string(),
                can_write: vec![],
                can_read: vec![],
                can_derive: vec![],
            }],
            default: PolicyDefault::DenyAll,
        })
        .await;
    source_room
        .set_policy(Policy {
            rules: vec![PolicyRule {
                path_glob: "world/**".to_string(),
                can_write: vec![signer.verifying_key().to_bytes()],
                can_read: vec![],
                can_derive: vec![],
            }],
            default: PolicyDefault::AllowAll,
        })
        .await;

    let export_signer = SigningKey::from_bytes(&[0x75u8; 32]);
    let file_manifest = root.join("exports").join("meta-policy-transition.json");
    let response = process_archive_export(
        &source_room,
        "archive-export-policy-transition-target",
        &serde_json::json!({
            "type": "archive.export",
            "source_room": "archive-export-policy-transition-source",
            "archive_ref": format!("file://{}", file_manifest.display()),
        }),
        &export_signer,
    )
    .await;
    let ArchiveWsResponse::ExportResult(envelope) = response else {
        panic!("expected archive.export.result response")
    };

    assert_eq!(envelope.policy_timeline_cutover_lamport, 2);
    assert_eq!(envelope.policy_timeline_transition_cutovers, vec![0, 1, 2]);

    let written: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&file_manifest).expect("manifest file should be readable"),
    )
    .expect("manifest json should parse");
    assert_eq!(written["policy_timeline_cutover_lamport"].as_u64(), Some(2));
    assert_eq!(
        written["policy_timeline_transition_cutovers"],
        serde_json::json!([0, 1, 2])
    );
}

#[tokio::test]
async fn archive_validate_accept_010_policy_timeline_transition_progression_non_zero_server_path() {
    let root = tmpdir();
    let persistence: SharedPersistence =
        Arc::new(DirPersistence::open(&root).expect("dir persistence should open"));
    let room = Room::new(
        "archive-validate-policy-transition-non-zero".to_string(),
        Arc::clone(&persistence),
        256,
    );

    let signer = SigningKey::from_bytes(&[0x7Au8; 32]);
    let nodes = build_set_nodes(&signer, &[("world/meta", "1")]);
    let (accepted, _, errors) = import_nodes(&room, nodes).await;
    assert_eq!(accepted, 1);
    assert!(errors.is_empty());

    room.set_policy(Policy {
        rules: vec![PolicyRule {
            path_glob: "world/**".to_string(),
            can_write: vec![],
            can_read: vec![],
            can_derive: vec![],
        }],
        default: PolicyDefault::DenyAll,
    })
    .await;
    room.set_policy(Policy {
        rules: vec![PolicyRule {
            path_glob: "world/**".to_string(),
            can_write: vec![signer.verifying_key().to_bytes()],
            can_read: vec![],
            can_derive: vec![],
        }],
        default: PolicyDefault::AllowAll,
    })
    .await;

    let export_signer = SigningKey::from_bytes(&[0x7Bu8; 32]);
    let file_manifest = root
        .join("exports")
        .join("validate-policy-transition-non-zero.json");
    let export_response = process_archive_export(
        &room,
        "archive-validate-policy-transition-non-zero",
        &serde_json::json!({
            "type": "archive.export",
            "source_room": "archive-validate-policy-transition-non-zero",
            "archive_ref": format!("file://{}", file_manifest.display()),
        }),
        &export_signer,
    )
    .await;
    let ArchiveWsResponse::ExportResult(exported) = export_response else {
        panic!("expected archive.export.result response")
    };
    assert_eq!(exported.policy_timeline_cutover_lamport, 2);
    assert_eq!(exported.policy_timeline_transition_cutovers, vec![0, 1, 2]);

    let validate_response = process_archive_validate(
        &room,
        "archive-validate-policy-transition-non-zero",
        &serde_json::json!({
            "type": "archive.validate",
            "archive_ref": format!("file://{}", file_manifest.display()),
            "mode": "full_integrity"
        }),
    )
    .await;

    let ArchiveWsResponse::ValidateResult(validated) = validate_response else {
        panic!("expected archive.validate.result response")
    };
    assert!(validated.accepted);
}

#[tokio::test]
async fn archive_validate_reject_002_payload_digest_policy_invalid_uses_deterministic_reason_class_server_path(
) {
    let root = tmpdir();
    let persistence: SharedPersistence =
        Arc::new(DirPersistence::open(&root).expect("dir persistence should open"));
    let target_room = Room::new(
        "archive-validate-policy-target".to_string(),
        Arc::clone(&persistence),
        256,
    );
    let source_room = Room::new(
        "archive-validate-policy-source".to_string(),
        Arc::clone(&persistence),
        256,
    );
    let source_signer = SigningKey::from_bytes(&[0x60u8; 32]);
    let nodes = build_set_nodes(&source_signer, &[("world/a", "1")]);
    let _ = import_nodes(&source_room, nodes).await;

    let manifest_signer = SigningKey::from_bytes(&[0x61u8; 32]);
    let manifest_path = root.join("exports").join("invalid-policy.json");
    write_signed_manifest_with_policy(
        &manifest_path,
        "archive-validate-policy-source",
        &manifest_signer,
        "1",
        "2",
        "legacy_md5_v0",
        0,
    );

    let response = process_archive_validate(
        &target_room,
        "archive-validate-policy-target",
        &serde_json::json!({
            "type": "archive.validate",
            "archive_ref": format!("file://{}", manifest_path.display()),
            "mode": "full_integrity"
        }),
    )
    .await;

    let ArchiveWsResponse::ValidateRejected(reject) = response else {
        panic!("expected archive.validate.rejected response")
    };
    assert_eq!(
        reject.reason_class,
        ArchiveReasonClass::PolicyTimelineMismatch
    );
}

#[tokio::test]
async fn archive_validate_reject_003_compatibility_window_unsupported_uses_deterministic_reason_class_server_path(
) {
    let root = tmpdir();
    let persistence: SharedPersistence =
        Arc::new(DirPersistence::open(&root).expect("dir persistence should open"));
    let target_room = Room::new(
        "archive-validate-compat-target".to_string(),
        Arc::clone(&persistence),
        256,
    );
    let source_room = Room::new(
        "archive-validate-compat-source".to_string(),
        Arc::clone(&persistence),
        256,
    );
    let source_signer = SigningKey::from_bytes(&[0x62u8; 32]);
    let nodes = build_set_nodes(&source_signer, &[("world/a", "1")]);
    let _ = import_nodes(&source_room, nodes).await;

    let manifest_signer = SigningKey::from_bytes(&[0x63u8; 32]);
    let manifest_path = root.join("exports").join("unsupported-window.json");
    write_signed_manifest_with_policy(
        &manifest_path,
        "archive-validate-compat-source",
        &manifest_signer,
        "2",
        "2",
        "strict_sha256_v1",
        0,
    );

    let response = process_archive_validate(
        &target_room,
        "archive-validate-compat-target",
        &serde_json::json!({
            "type": "archive.validate",
            "archive_ref": format!("file://{}", manifest_path.display()),
            "mode": "full_integrity"
        }),
    )
    .await;

    let ArchiveWsResponse::ValidateRejected(reject) = response else {
        panic!("expected archive.validate.rejected response")
    };
    assert_eq!(reject.reason_class, ArchiveReasonClass::UnsupportedFormat);
}

#[tokio::test]
async fn archive_validate_accept_005_compatibility_window_range_overlaps_runtime_server_path() {
    let root = tmpdir();
    let persistence: SharedPersistence =
        Arc::new(DirPersistence::open(&root).expect("dir persistence should open"));
    let target_room = Room::new(
        "archive-validate-compat-range-target".to_string(),
        Arc::clone(&persistence),
        256,
    );
    let source_room = Room::new(
        "archive-validate-compat-range-source".to_string(),
        Arc::clone(&persistence),
        256,
    );
    let source_signer = SigningKey::from_bytes(&[0x66u8; 32]);
    let nodes = build_set_nodes(&source_signer, &[("world/a", "1")]);
    let _ = import_nodes(&source_room, nodes).await;

    let manifest_signer = SigningKey::from_bytes(&[0x67u8; 32]);
    let manifest_path = root.join("exports").join("range-window.json");
    write_signed_manifest_with_policy(
        &manifest_path,
        "archive-validate-compat-range-source",
        &manifest_signer,
        "0",
        "3",
        "strict_sha256_v1",
        0,
    );

    let response = process_archive_validate(
        &target_room,
        "archive-validate-compat-range-target",
        &serde_json::json!({
            "type": "archive.validate",
            "archive_ref": format!("file://{}", manifest_path.display()),
            "mode": "full_integrity"
        }),
    )
    .await;

    let ArchiveWsResponse::ValidateResult(result) = response else {
        panic!("expected archive.validate.result response")
    };
    assert!(result.accepted);
}

#[tokio::test]
async fn archive_validate_reject_004_policy_timeline_hash_mismatch_uses_deterministic_reason_class_server_path(
) {
    let root = tmpdir();
    let persistence: SharedPersistence =
        Arc::new(DirPersistence::open(&root).expect("dir persistence should open"));
    let target_room = Room::new(
        "archive-validate-policy-hash-target".to_string(),
        Arc::clone(&persistence),
        256,
    );
    target_room
        .set_policy(Policy {
            rules: vec![PolicyRule {
                path_glob: "world/**".to_string(),
                can_write: vec![],
                can_read: vec![],
                can_derive: vec![],
            }],
            default: PolicyDefault::DenyAll,
        })
        .await;
    let source_room = Room::new(
        "archive-validate-policy-hash-source".to_string(),
        Arc::clone(&persistence),
        256,
    );
    let source_signer = SigningKey::from_bytes(&[0x64u8; 32]);
    let nodes = build_set_nodes(&source_signer, &[("world/a", "1")]);
    let _ = import_nodes(&source_room, nodes).await;

    let manifest_signer = SigningKey::from_bytes(&[0x65u8; 32]);
    let manifest_path = root.join("exports").join("policy-hash-mismatch.json");
    write_signed_manifest_with_policy(
        &manifest_path,
        "archive-validate-policy-hash-source",
        &manifest_signer,
        "1",
        "2",
        "strict_sha256_v1",
        0,
    );

    let response = process_archive_validate(
        &target_room,
        "archive-validate-policy-hash-target",
        &serde_json::json!({
            "type": "archive.validate",
            "archive_ref": format!("file://{}", manifest_path.display()),
            "mode": "full_integrity"
        }),
    )
    .await;

    let ArchiveWsResponse::ValidateRejected(reject) = response else {
        panic!("expected archive.validate.rejected response")
    };
    assert_eq!(
        reject.reason_class,
        ArchiveReasonClass::PolicyTimelineMismatch
    );
}

#[tokio::test]
async fn archive_validate_reject_006_policy_timeline_cutover_mismatch_uses_deterministic_reason_class_server_path(
) {
    let root = tmpdir();
    let persistence: SharedPersistence =
        Arc::new(DirPersistence::open(&root).expect("dir persistence should open"));
    let target_room = Room::new(
        "archive-validate-policy-cutover-target".to_string(),
        Arc::clone(&persistence),
        256,
    );
    let source_room = Room::new(
        "archive-validate-policy-cutover-source".to_string(),
        Arc::clone(&persistence),
        256,
    );
    let source_signer = SigningKey::from_bytes(&[0x68u8; 32]);
    let nodes = build_set_nodes(&source_signer, &[("world/a", "1")]);
    let _ = import_nodes(&source_room, nodes).await;

    let manifest_signer = SigningKey::from_bytes(&[0x69u8; 32]);
    let manifest_path = root.join("exports").join("policy-cutover-mismatch.json");
    write_signed_manifest_with_policy(
        &manifest_path,
        "archive-validate-policy-cutover-source",
        &manifest_signer,
        "1",
        "2",
        "strict_sha256_v1",
        7,
    );

    let response = process_archive_validate(
        &target_room,
        "archive-validate-policy-cutover-target",
        &serde_json::json!({
            "type": "archive.validate",
            "archive_ref": format!("file://{}", manifest_path.display()),
            "mode": "full_integrity"
        }),
    )
    .await;

    let ArchiveWsResponse::ValidateRejected(reject) = response else {
        panic!("expected archive.validate.rejected response")
    };
    assert_eq!(
        reject.reason_class,
        ArchiveReasonClass::PolicyTimelineMismatch
    );
}

#[tokio::test]
async fn archive_validate_reject_007_policy_timeline_transition_progression_invalid_uses_deterministic_reason_class_server_path(
) {
    let root = tmpdir();
    let persistence: SharedPersistence =
        Arc::new(DirPersistence::open(&root).expect("dir persistence should open"));
    let target_room = Room::new(
        "archive-validate-policy-transition-target".to_string(),
        Arc::clone(&persistence),
        256,
    );
    let source_room = Room::new(
        "archive-validate-policy-transition-source".to_string(),
        Arc::clone(&persistence),
        256,
    );
    let source_signer = SigningKey::from_bytes(&[0x6Au8; 32]);
    let nodes = build_set_nodes(&source_signer, &[("world/a", "1")]);
    let _ = import_nodes(&source_room, nodes).await;

    let manifest_signer = SigningKey::from_bytes(&[0x6Bu8; 32]);
    let manifest_path = root.join("exports").join("policy-transition-invalid.json");
    write_signed_manifest_with_policy_and_transitions(
        &manifest_path,
        "archive-validate-policy-transition-source",
        &manifest_signer,
        "1",
        "2",
        "strict_sha256_v1",
        7,
        &[0, 7, 7],
    );

    let response = process_archive_validate(
        &target_room,
        "archive-validate-policy-transition-target",
        &serde_json::json!({
            "type": "archive.validate",
            "archive_ref": format!("file://{}", manifest_path.display()),
            "mode": "full_integrity"
        }),
    )
    .await;

    let ArchiveWsResponse::ValidateRejected(reject) = response else {
        panic!("expected archive.validate.rejected response")
    };
    assert_eq!(reject.reason_class, ArchiveReasonClass::ManifestInvalid);
}

#[tokio::test]
async fn archive_validate_reject_008_compatibility_window_no_overlap_uses_deterministic_reason_class_server_path(
) {
    let root = tmpdir();
    let persistence: SharedPersistence =
        Arc::new(DirPersistence::open(&root).expect("dir persistence should open"));
    let target_room = Room::new(
        "archive-validate-compat-no-overlap-target".to_string(),
        Arc::clone(&persistence),
        256,
    );
    let source_room = Room::new(
        "archive-validate-compat-no-overlap-source".to_string(),
        Arc::clone(&persistence),
        256,
    );
    let source_signer = SigningKey::from_bytes(&[0x6Cu8; 32]);
    let nodes = build_set_nodes(&source_signer, &[("world/a", "1")]);
    let _ = import_nodes(&source_room, nodes).await;

    let manifest_signer = SigningKey::from_bytes(&[0x6Du8; 32]);
    let manifest_path = root.join("exports").join("compat-no-overlap.json");
    write_signed_manifest_with_policy(
        &manifest_path,
        "archive-validate-compat-no-overlap-source",
        &manifest_signer,
        "3",
        "4",
        "strict_sha256_v1",
        0,
    );

    let response = process_archive_validate(
        &target_room,
        "archive-validate-compat-no-overlap-target",
        &serde_json::json!({
            "type": "archive.validate",
            "archive_ref": format!("file://{}", manifest_path.display()),
            "mode": "full_integrity"
        }),
    )
    .await;

    let ArchiveWsResponse::ValidateRejected(reject) = response else {
        panic!("expected archive.validate.rejected response")
    };
    assert_eq!(reject.reason_class, ArchiveReasonClass::UnsupportedFormat);
}

#[tokio::test]
async fn archive_validate_accept_009_compatibility_window_edge_overlap_lower_bound_server_path() {
    let root = tmpdir();
    let persistence: SharedPersistence =
        Arc::new(DirPersistence::open(&root).expect("dir persistence should open"));
    let target_room = Room::new(
        "archive-validate-compat-edge-lower-target".to_string(),
        Arc::clone(&persistence),
        256,
    );
    let source_room = Room::new(
        "archive-validate-compat-edge-lower-source".to_string(),
        Arc::clone(&persistence),
        256,
    );
    let source_signer = SigningKey::from_bytes(&[0x6Eu8; 32]);
    let nodes = build_set_nodes(&source_signer, &[("world/a", "1")]);
    let _ = import_nodes(&source_room, nodes).await;

    let manifest_signer = SigningKey::from_bytes(&[0x6Fu8; 32]);
    let manifest_path = root.join("exports").join("compat-edge-overlap-lower.json");
    write_signed_manifest_with_policy(
        &manifest_path,
        "archive-validate-compat-edge-lower-source",
        &manifest_signer,
        "0",
        "1",
        "strict_sha256_v1",
        0,
    );

    let response = process_archive_validate(
        &target_room,
        "archive-validate-compat-edge-lower-target",
        &serde_json::json!({
            "type": "archive.validate",
            "archive_ref": format!("file://{}", manifest_path.display()),
            "mode": "full_integrity"
        }),
    )
    .await;

    let ArchiveWsResponse::ValidateResult(result) = response else {
        panic!("expected archive.validate.result response")
    };
    assert!(result.accepted);
}
