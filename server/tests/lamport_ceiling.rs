//! G5 integration test — Lamport ceiling & wall-clock skew at the
//! `import_nodes` layer.
//!
//! Drives `Rooms` + `import_nodes` directly (no WS) and asserts that:
//!   - a benign node is accepted,
//!   - a node with `lamport > LAMPORT_SLACK` is rejected, errors contain
//!     the variant's `Display`,
//!   - a node with `wall_ms > now + 24h + 1ms` is rejected,
//!   - the counts returned by `import_nodes` match what we expect.

use std::sync::Arc;

use activesync_core::{LAMPORT_SLACK, MapOp, Op, SyncNode, Transaction, WALL_SKEW_MAX_MS};
use activesync_core::node::Signature;
use activesync_server::room::{import_nodes, Rooms};
use activesync_server::store::{NoPersistence, SharedPersistence};
use ed25519_dalek::{Signer, SigningKey};

fn sign(tx: Transaction, sk: &SigningKey) -> SyncNode {
    let id = tx.hash();
    let sig = sk.sign(id.as_bytes());
    SyncNode { id, transaction: tx, signature: Signature(sig.to_bytes()) }
}

fn node_at(sk: &SigningKey, lamport: u64, wall_ms: u64, key: &str) -> SyncNode {
    let tx = Transaction {
        author: sk.verifying_key().to_bytes(),
        lamport,
        wall_ms,
        ops: vec![Op::Map(MapOp::Set { key: key.into(), value: b"v".to_vec() })],
        parents: vec![],
    };
    sign(tx, sk)
}

#[tokio::test]
async fn import_nodes_rejects_lamport_ceiling_and_wall_skew() {
    let persistence: SharedPersistence = Arc::new(NoPersistence);
    let rooms = Rooms::new(
        SigningKey::from_bytes(&[0x11u8; 32]),
        persistence,
        512,
        0,
        0,
    );
    let room = rooms.get_or_create("g5-room").await;
    let sk = SigningKey::from_bytes(&[0x22u8; 32]);

    let now_ms: u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;

    // One healthy node, one Lamport-over-ceiling, one wall-skew.
    // Push `bad_wall` a full hour past the ceiling so the test isn't
    // racy against `import_nodes`' own `SystemTime::now()` capture
    // (it samples a few ms later than us).
    let good = node_at(&sk, 1, now_ms, "a");
    let bad_lamport = node_at(&sk, LAMPORT_SLACK + 10, now_ms, "b");
    let bad_wall = node_at(&sk, 2, now_ms + WALL_SKEW_MAX_MS + 3_600_000, "c");

    let (accepted, errors) = import_nodes(&room, vec![
        good,
        bad_lamport,
        bad_wall,
    ]).await;

    assert_eq!(accepted, 1, "only the healthy node should be accepted");
    // Expect exactly the two sanity-check errors to surface (MissingParent
    // is silenced, DuplicateNode is silenced, these two are not).
    let joined = errors.join(" | ");
    assert!(
        joined.contains("lamport ceiling exceeded"),
        "missing lamport error in: {joined:?}"
    );
    assert!(
        joined.contains("wall clock skew"),
        "missing wall skew error in: {joined:?}"
    );
}

#[tokio::test]
async fn import_nodes_accepts_nodes_within_both_bounds() {
    let persistence: SharedPersistence = Arc::new(NoPersistence);
    let rooms = Rooms::new(
        SigningKey::from_bytes(&[0x33u8; 32]),
        persistence,
        512,
        0,
        0,
    );
    let room = rooms.get_or_create("g5-ok").await;
    let sk = SigningKey::from_bytes(&[0x44u8; 32]);

    let now_ms: u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;

    // At the Lamport boundary, and safely inside the wall-clock window
    // (one minute past "now" — small enough that server-side re-sampling
    // of `SystemTime::now()` can't push it over the 24 h ceiling).
    let at_lamport_edge = node_at(&sk, LAMPORT_SLACK, now_ms, "e1");
    let at_wall_edge = node_at(&sk, 1, now_ms + 60_000, "e2");

    let (accepted, errors) = import_nodes(&room, vec![at_lamport_edge, at_wall_edge]).await;
    assert_eq!(accepted, 2, "boundary nodes must be accepted; errors={errors:?}");
    assert!(errors.is_empty(), "unexpected errors: {errors:?}");
}
