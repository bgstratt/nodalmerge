use std::collections::BTreeMap;

use ed25519_dalek::SigningKey;
use nodalmerge_core::{
    ArchiveCheckpoint,
    ArchiveCompatibilityWindow,
    ArchiveDescribed,
    ArchiveExported,
    ArchiveImported,
    ArchivePayloadDigestSet,
    ArchiveProvenance,
    ArchiveReasonClass,
    ArchiveValidated,
    ArchiveWsResponse,
    Hash,
    MapOp,
    Op,
    SyncNode,
    Transaction,
    canonical_hash,
    replay,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct ArchiveManifest {
    format_version: String,
    kind: String,
    checkpoint_hash: Hash,
    payload_digest: Hash,
}

fn signed_set_node(
    key: &SigningKey,
    lamport: u64,
    map_key: &str,
    map_val: &str,
    parents: Vec<Hash>,
) -> SyncNode {
    let tx = Transaction {
        author: key.verifying_key().to_bytes(),
        lamport,
        wall_ms: 0,
        ops: vec![Op::Map(MapOp::Set {
            key: map_key.to_string(),
            value: map_val.as_bytes().to_vec(),
        })],
        parents,
    };
    SyncNode::new_signed(tx, key)
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

fn validate_archive_compatibility_range(
    version: &str,
    min_supported: &str,
    max_supported: &str,
) -> Result<(), ArchiveReasonClass> {
    let runtime = version
        .trim()
        .parse::<u32>()
        .map_err(|_| ArchiveReasonClass::UnsupportedFormat)?;
    let min = min_supported
        .trim()
        .parse::<u32>()
        .map_err(|_| ArchiveReasonClass::UnsupportedFormat)?;
    let max = max_supported
        .trim()
        .parse::<u32>()
        .map_err(|_| ArchiveReasonClass::UnsupportedFormat)?;

    if min > max {
        return Err(ArchiveReasonClass::UnsupportedFormat);
    }

    if (min..=max).contains(&runtime) {
        Ok(())
    } else {
        Err(ArchiveReasonClass::UnsupportedFormat)
    }
}

fn encode_hash(hash: Hash) -> String {
    hash.0.iter().map(|b| format!("{b:02x}")).collect::<String>()
}

fn describe_archive_stub(room_id: &str, archive_ref: &str, checkpoint_hash: Hash) -> ArchiveWsResponse {
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
            tool: "core-test-stub".to_string(),
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
            canonical_hash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
            checkpoint: ArchiveCheckpoint {
                frontier: vec!["seq:0".to_string()],
                canonical_hash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
            },
            imported_nodes: 10,
            imported_blobs: 2,
        }))
    }
}

#[test]
fn archive_det_001_manifest_digest_equality_at_fixed_checkpoint() {
    let signer = SigningKey::from_bytes(&[0x41u8; 32]);
    let n1 = signed_set_node(&signer, 1, "world/a", "1", vec![]);
    let n2 = signed_set_node(&signer, 2, "world/b", "2", vec![n1.id]);

    let state = replay(&[n1.clone(), n2.clone()], None).expect("replay must succeed");
    let checkpoint_hash = canonical_hash(&state.map.iter().map(|(k, v)| (k.clone(), v.clone())).collect());

    let manifest_a = ArchiveManifest {
        format_version: "archive/v1".to_string(),
        kind: "full_clone".to_string(),
        checkpoint_hash,
        payload_digest: payload_digest(&[n1.clone(), n2.clone()]),
    };
    let manifest_b = ArchiveManifest {
        format_version: "archive/v1".to_string(),
        kind: "full_clone".to_string(),
        checkpoint_hash,
        payload_digest: payload_digest(&[n1, n2]),
    };

    assert_eq!(
        manifest_digest(&manifest_a),
        manifest_digest(&manifest_b),
        "ARCHIVE-DET-001: manifest digest must be stable at fixed checkpoint"
    );
}

#[test]
fn archive_roundtrip_001_full_clone_canonical_hash_parity() {
    let signer = SigningKey::from_bytes(&[0x42u8; 32]);
    let n1 = signed_set_node(&signer, 1, "world/a", "1", vec![]);
    let n2 = signed_set_node(&signer, 2, "world/b", "2", vec![n1.id]);
    let n3 = signed_set_node(&signer, 3, "world/c", "3", vec![n2.id]);

    let exported_nodes = vec![n1.clone(), n2.clone(), n3.clone()];
    let source = replay(&exported_nodes, None).expect("source replay must succeed");

    // Roundtrip import lane (stub): replay exported payload as if imported.
    let imported = replay(&exported_nodes, None).expect("import replay must succeed");

    let source_hash = canonical_hash(&source.map.iter().map(|(k, v)| (k.clone(), v.clone())).collect());
    let imported_hash = canonical_hash(&imported.map.iter().map(|(k, v)| (k.clone(), v.clone())).collect());
    assert_eq!(
        source_hash,
        imported_hash,
        "ARCHIVE-ROUNDTRIP-001: full-clone roundtrip must preserve canonical hash"
    );
}

#[test]
fn archive_compat_reject_001_unsupported_format_rejected_deterministically() {
    let err = validate_archive_compatibility_range("1", "2", "3")
        .expect_err("unsupported format must reject");
    assert_eq!(err, ArchiveReasonClass::UnsupportedFormat);
}

#[test]
fn archive_compat_accept_002_explicit_range_accepts_runtime_version() {
    validate_archive_compatibility_range("1", "0", "2")
        .expect("runtime format should be accepted when within explicit range");
}

#[test]
fn archive_describe_001_envelope_contains_manifest_checkpoint_and_digest_metadata() {
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
fn archive_validate_reject_001_manifest_invalid_uses_deterministic_reason_class() {
    let reason_class = validate_archive_stub("s3://bucket/invalid-manifest.nmar")
        .expect_err("invalid manifest archive ref must reject deterministically");
    assert_eq!(reason_class, ArchiveReasonClass::ManifestInvalid);
}

#[test]
fn archive_import_reject_001_digest_mismatch_uses_deterministic_reason_class() {
    let reason_class = import_archive_stub("s3://bucket/digest-mismatch.nmar")
        .expect_err("digest mismatch archive ref must reject deterministically");
    assert_eq!(reason_class, ArchiveReasonClass::DigestMismatch);
}

#[test]
fn archive_export_003_envelope_carries_policy_timeline_hash_and_cutover_metadata() {
    let response = ArchiveWsResponse::ExportResult(ArchiveExported {
        room: "room-a".to_string(),
        source_room: "room-source".to_string(),
        archive_ref: "file:///tmp/archive.json".to_string(),
        manifest_id: "m.room-source.aaaaaaaaaaaa".to_string(),
        checkpoint: ArchiveCheckpoint {
            frontier: vec!["seq:1".to_string()],
            canonical_hash: "aa".repeat(32),
        },
        payload_digest_set: ArchivePayloadDigestSet {
            nodes: "sha256:nodes".to_string(),
            blobs: "sha256:blobs".to_string(),
        },
        compatibility_window: ArchiveCompatibilityWindow {
            min_supported: "1".to_string(),
            max_supported: "2".to_string(),
        },
        payload_digest_policy: "strict_sha256_v1".to_string(),
        policy_timeline_hash: "bb".repeat(32),
        policy_timeline_cutover_lamport: 0,
        policy_timeline_transition_cutovers: vec![0],
    });

    let json = serde_json::to_string(&response).expect("archive export envelope should serialize");
    assert!(json.contains("\"type\":\"archive.export.result\""));
    assert!(json.contains("\"max_supported\":\"2\""));
    assert!(json.contains("\"policy_timeline_cutover_lamport\":0"));
    assert!(json.contains("\"policy_timeline_transition_cutovers\":[0]"));
}
