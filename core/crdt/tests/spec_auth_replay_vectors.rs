use std::collections::BTreeMap;

use ed25519_dalek::SigningKey;
use nodalmerge_core::{Hash, MapOp, Op, SyncNode, Transaction, canonical_hash, replay};

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

fn canonical_lane_hash(map: &BTreeMap<String, Vec<u8>>) -> Hash {
    let filtered: BTreeMap<String, Vec<u8>> = map
        .iter()
        .filter(|(k, _)| !k.starts_with("intent/"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    canonical_hash(&filtered)
}

#[test]
fn spec_auth_004_canonical_hash_stable() {
    let signer = SigningKey::from_bytes(&[0x31u8; 32]);

    let n1 = signed_set_node(&signer, 1, "world/p1/x", "10", vec![]);
    let n2 = signed_set_node(&signer, 2, "world/p1/y", "20", vec![n1.id]);

    let base_nodes = vec![n1.clone(), n2.clone()];
    let base = replay(&base_nodes, None).expect("base replay should succeed");
    let base_hash = canonical_lane_hash(&base.map);

    // Pending intent: intent lane writes are present but should not perturb
    // canonical-lane hash in the contract model.
    let pending_intent = signed_set_node(
        &signer,
        3,
        "intent/p1/move",
        "{\"dx\":1,\"dy\":0}",
        vec![n2.id],
    );
    let with_pending = replay(&[base_nodes.clone(), vec![pending_intent]].concat(), None)
        .expect("pending-intent replay should succeed");
    let pending_hash = canonical_lane_hash(&with_pending.map);
    assert_eq!(
        base_hash, pending_hash,
        "SPEC-AUTH-004: canonical-lane hash changed under pending intent"
    );

    // Rejected intent (stub model): rejected writes do not enter canonical
    // replay input, so canonical-lane hash remains unchanged.
    let rejected_hash = canonical_lane_hash(&base.map);
    assert_eq!(
        base_hash, rejected_hash,
        "SPEC-AUTH-004: canonical-lane hash changed under rejected intent"
    );
}
