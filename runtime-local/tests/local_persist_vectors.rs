//! Wave 2 headless persistence conformance vectors.

use ed25519_dalek::SigningKey;
use nodalmerge_core::{canonical_hash, replay, MapOp, Op, StateGraph, SyncNode};
use nodalmerge_runtime_local::{
    open_registered, parse_backend_kind, CheckpointMeta, CompositeLocalPersistence,
    BackendOpenOptions, FileLocalPersistence, LocalPersistReason, MemoryLocalPersistence,
    NodeLogTail, PeerLocalPersistence, PersistenceHandle,
};

fn make_chain(sk: &SigningKey, writes: &[(&str, &[u8])]) -> Vec<SyncNode> {
    let mut g = StateGraph::new();
    let mut out = Vec::with_capacity(writes.len());
    for (key, val) in writes {
        let id = g
            .apply_local(
                sk,
                0,
                vec![Op::Map(MapOp::Set {
                    key: (*key).into(),
                    value: val.to_vec(),
                })],
            )
            .expect("apply_local");
        out.push(g.get_nodes(&[id]).into_iter().next().unwrap().clone());
    }
    out
}

fn canonical_hash_from_nodes(nodes: &[SyncNode]) -> nodalmerge_core::Hash {
    let state = replay(nodes, None).expect("replay");
    canonical_hash(&state.map)
}

/// LOCAL-PERSIST-001: after append + flush + checkpoint, a simulated restart
/// (new adapter on the same store) recovers nodes that replay to the same
/// canonical hash as before restart.
#[test]
fn local_persist_001_restart_recovery_restores_identical_canonical_hash() {
    let room = "local-persist-001-room";
    let sk = SigningKey::from_bytes(&[0x61u8; 32]);
    let nodes = make_chain(&sk, &[("world/a", b"1"), ("world/b", b"2"), ("world/c", b"3")]);

    let session = MemoryLocalPersistence::open();
    session
        .append_nodes(room, &nodes, None)
        .expect("append");
    session.flush(room).expect("flush");
    let pre_hash = canonical_hash_from_nodes(&nodes);
    session
        .checkpoint(
            room,
            CheckpointMeta {
                seq: 3,
                canonical_hash: Some(pre_hash),
            },
        )
        .expect("checkpoint");

    let store = session.store();
    drop(session);

    let restarted = MemoryLocalPersistence::reopen(store);
    let recovery = restarted.recover(room).expect("recover after restart");
    assert_eq!(recovery.nodes.len(), 3, "LOCAL-PERSIST-001: expected 3 recovered nodes");
    assert_eq!(recovery.tail.seq, 3);

    let post_hash = canonical_hash_from_nodes(&recovery.nodes);
    assert_eq!(
        post_hash, pre_hash,
        "LOCAL-PERSIST-001: canonical hash must match across restart recovery"
    );

    let hydrate = restarted.hydrate(room).expect("hydrate");
    let recovered_ids: Vec<_> = recovery.nodes.iter().map(|n| n.id).collect();
    let hydrate_ids: Vec<_> = hydrate.nodes.iter().map(|n| n.id).collect();
    assert_eq!(recovered_ids, hydrate_ids);
    assert_eq!(canonical_hash_from_nodes(&hydrate.nodes), pre_hash);
}

/// LOCAL-PERSIST-002: same node chain on memory vs file backends replays to identical hash.
#[test]
fn local_persist_002_backend_switch_preserves_canonical_hash() {
    let room = "local-persist-002-room";
    let sk = SigningKey::from_bytes(&[0x65u8; 32]);
    let nodes = make_chain(&sk, &[("world/x", b"a"), ("world/y", b"b")]);
    let expected = canonical_hash_from_nodes(&nodes);

    let memory = MemoryLocalPersistence::open();
    memory.append_nodes(room, &nodes, None).expect("memory append");
    memory.flush(room).expect("memory flush");
    let memory_hash = canonical_hash_from_nodes(&memory.recover(room).expect("memory recover").nodes);

    let dir = tempfile::tempdir().expect("tempdir");
    let file = FileLocalPersistence::open(dir.path()).expect("file open");
    file.append_nodes(room, &nodes, None).expect("file append");
    file.flush(room).expect("file flush");
    let file_hash = canonical_hash_from_nodes(&file.recover(room).expect("file recover").nodes);

    assert_eq!(memory_hash, expected, "LOCAL-PERSIST-002: memory hash");
    assert_eq!(file_hash, expected, "LOCAL-PERSIST-002: file hash");
    assert_eq!(memory_hash, file_hash, "LOCAL-PERSIST-002: backend parity");
}

/// LOCAL-PERSIST-004 (partial): tail conflict uses bounded reason class.
#[test]
fn local_persist_004_tail_conflict_uses_bounded_reason_class() {
    let room = "local-persist-004-room";
    let sk = SigningKey::from_bytes(&[0x62u8; 32]);
    let nodes = make_chain(&sk, &[("k", b"v")]);

    let session = MemoryLocalPersistence::open();
    session.append_nodes(room, &nodes, None).expect("append");

    let err = session
        .append_nodes(room, &[], Some(NodeLogTail { seq: 0 }))
        .expect_err("stale tail must reject");

    assert_eq!(err.reason, LocalPersistReason::TailConflict);
    assert_eq!(err.reason_class(), "reject.local_persist_tail_conflict");
    assert_eq!(err.reason.recovery_posture(), "retryable");
}

/// LOCAL-PERSIST-001 (filesystem): true process restart via drop + reopen same data_dir.
#[test]
fn local_persist_001_fs_restart_recovery_restores_identical_canonical_hash() {
    let room = "local-persist-001-fs-room";
    let sk = SigningKey::from_bytes(&[0x64u8; 32]);
    let nodes = make_chain(&sk, &[("world/a", b"1"), ("world/b", b"2"), ("world/c", b"3")]);

    let dir = tempfile::tempdir().expect("tempdir");
    let data_dir = dir.path().to_path_buf();

    {
        let session = FileLocalPersistence::open(&data_dir).expect("open");
        session.append_nodes(room, &nodes, None).expect("append");
        session.flush(room).expect("flush");
        let pre_hash = canonical_hash_from_nodes(&nodes);
        session
            .checkpoint(
                room,
                CheckpointMeta {
                    seq: 3,
                    canonical_hash: Some(pre_hash),
                },
            )
            .expect("checkpoint");
    }

    let restarted = FileLocalPersistence::open(&data_dir).expect("reopen after restart");
    let recovery = restarted.recover(room).expect("recover");
    assert_eq!(recovery.nodes.len(), 3);
    let post_hash = canonical_hash_from_nodes(&recovery.nodes);
    let pre_hash = canonical_hash_from_nodes(&nodes);
    assert_eq!(post_hash, pre_hash, "LOCAL-PERSIST-001 fs: hash parity");
    assert!(restarted.is_durable());
}

/// LOCAL-PERSIST-003 (partial): blob roundtrip on filesystem adapter.
#[test]
fn local_persist_003_fs_blob_roundtrip() {
    let room = "local-persist-003-fs-blob";
    let dir = tempfile::tempdir().expect("tempdir");
    let session = FileLocalPersistence::open(dir.path()).expect("open");
    let hash = nodalmerge_core::Hash::of(b"blob-payload");
    session
        .put_blob(room, &hash, b"blob-payload")
        .expect("put");
    session.flush(room).expect("flush");
    drop(session);

    let reopened = FileLocalPersistence::open(dir.path()).expect("reopen");
    let bytes = reopened.get_blob(room, &hash).expect("get").expect("exists");
    assert_eq!(bytes, b"blob-payload");
}

/// LOCAL-PERSIST-001 (composite): write-through cache + file survives restart.
#[test]
fn local_persist_composite_restart_recovery_restores_identical_canonical_hash() {
    let room = "local-persist-composite-001";
    let sk = SigningKey::from_bytes(&[0x66u8; 32]);
    let nodes = make_chain(&sk, &[("world/c1", b"1"), ("world/c2", b"2")]);

    let dir = tempfile::tempdir().expect("tempdir");
    let data_dir = dir.path().to_path_buf();

    {
        let session = CompositeLocalPersistence::open(&data_dir).expect("open");
        session.append_nodes(room, &nodes, None).expect("append");
        session.flush(room).expect("flush");
        let pre_hash = canonical_hash_from_nodes(&nodes);
        session
            .checkpoint(
                room,
                CheckpointMeta {
                    seq: 2,
                    canonical_hash: Some(pre_hash),
                },
            )
            .expect("checkpoint");
        assert!(session.is_durable());
    }

    let restarted = CompositeLocalPersistence::open(&data_dir).expect("reopen");
    let recovery = restarted.recover(room).expect("recover");
    assert_eq!(recovery.nodes.len(), 2);
    assert_eq!(
        canonical_hash_from_nodes(&recovery.nodes),
        canonical_hash_from_nodes(&nodes)
    );
}

/// `embedded` and `sqlite` are aliases for the file (SQLite) backend.
#[test]
fn parse_backend_embedded_aliases_file_backend() {
    let dir = tempfile::tempdir().expect("tempdir");
    for name in ["embedded", "sqlite"] {
        let kind = parse_backend_kind(name, Some(dir.path().to_path_buf())).expect("parse");
        let handle = PersistenceHandle::open(kind).expect("open");
        assert!(handle.is_durable(), "{name} should open durable file backend");
    }
}

/// Registry pilot: `registered:composite` opens the same adapter as built-in composite.
#[test]
fn local_persist_registry_opens_composite_backend() {
    let dir = tempfile::tempdir().expect("tempdir");
    let kind = parse_backend_kind("registered:composite", Some(dir.path().to_path_buf()))
        .expect("parse");
    let handle = PersistenceHandle::open(kind).expect("open");
    assert!(handle.is_durable());

    let arc = open_registered(
        "composite",
        &BackendOpenOptions {
            data_dir: Some(dir.path().to_path_buf()),
        },
    )
    .expect("registry open");
    assert!(arc.is_durable());
}

/// LOCAL-PERSIST-004 (partial): read-only adapter rejects writes with stable class.
#[test]
fn local_persist_004_readonly_rejects_append() {
    let room = "local-persist-readonly-room";
    let sk = SigningKey::from_bytes(&[0x63u8; 32]);
    let nodes = make_chain(&sk, &[("k", b"v")]);

    let store = MemoryLocalPersistence::open().store();
    let ro = MemoryLocalPersistence::read_only_view(store);

    let err = ro.append_nodes(room, &nodes, None).expect_err("readonly");
    assert_eq!(err.reason, LocalPersistReason::ReadOnly);
    assert_eq!(err.reason_class(), "reject.local_persist_readonly");
}
