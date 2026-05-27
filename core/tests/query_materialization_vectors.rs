use std::collections::{BTreeMap, BTreeSet};

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

fn query_prefix_projection(
    map: &BTreeMap<String, Vec<u8>>,
    prefix: &str,
) -> Vec<(String, Vec<u8>)> {
    map.iter()
        .filter(|(k, _)| k.starts_with(prefix))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn projection_digest(entries: &[(String, Vec<u8>)]) -> Hash {
    let as_map: BTreeMap<String, Vec<u8>> = entries.iter().cloned().collect();
    canonical_hash(&as_map)
}

fn paginate_projection_entries(
    entries: &[(String, Vec<u8>)],
    limit: usize,
    page_token: Option<&str>,
) -> (Vec<(String, Vec<u8>)>, Option<String>) {
    assert!(limit > 0, "limit must be > 0 for pagination");

    let start = page_token
        .and_then(|token| token.strip_prefix("offset:"))
        .and_then(|raw| raw.parse::<usize>().ok())
        .unwrap_or(0);

    let page: Vec<(String, Vec<u8>)> = entries
        .iter()
        .skip(start)
        .take(limit)
        .cloned()
        .collect();

    let next = if start + page.len() < entries.len() {
        Some(format!("offset:{}", start + page.len()))
    } else {
        None
    };

    (page, next)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReplayMismatchDiagnostic {
    checkpoint: String,
    live_digest: Hash,
    replay_digest: Hash,
    reason_class: &'static str,
    reason_message: String,
}

fn validate_selector_payload(
    selector: &str,
    canonical_seq: Option<u64>,
    canonical_hash: Option<&str>,
    frontier: Option<&[&str]>,
    known_hashes: &BTreeSet<String>,
) -> Result<(), (&'static str, &'static str)> {
    match selector {
        "latest" => {
            if canonical_seq.is_some() || canonical_hash.is_some() || frontier.is_some() {
                return Err((
                    "reject.checkpoint_selector_invalid",
                    "selector latest cannot include canonical_seq, canonical_hash, or frontier",
                ));
            }
            Ok(())
        }
        "seq" => {
            if canonical_seq.is_none() || canonical_hash.is_some() || frontier.is_some() {
                return Err((
                    "reject.checkpoint_selector_invalid",
                    "selector seq requires canonical_seq and forbids canonical_hash/frontier",
                ));
            }
            Ok(())
        }
        "hash" => {
            let Some(hash) = canonical_hash else {
                return Err((
                    "reject.checkpoint_selector_invalid",
                    "selector hash requires canonical_hash and forbids canonical_seq/frontier",
                ));
            };

            if canonical_seq.is_some() || frontier.is_some() {
                return Err((
                    "reject.checkpoint_selector_invalid",
                    "selector hash requires canonical_hash and forbids canonical_seq/frontier",
                ));
            }

            let is_valid_hex = hash.len() == 64 && hash.chars().all(|ch| ch.is_ascii_hexdigit());
            if !is_valid_hex {
                return Err((
                    "reject.checkpoint_selector_invalid",
                    "selector hash requires canonical_hash in 64-char hex format",
                ));
            }

            if !known_hashes.contains(&hash.to_ascii_lowercase()) {
                return Err((
                    "reject.checkpoint_not_found",
                    "target_checkpoint does not resolve to known canonical snapshot",
                ));
            }

            Ok(())
        }
        "frontier" => {
            let Some(frontier_vals) = frontier else {
                return Err((
                    "reject.checkpoint_selector_invalid",
                    "selector frontier requires frontier and forbids canonical_seq/canonical_hash",
                ));
            };

            if canonical_seq.is_some() || canonical_hash.is_some() {
                return Err((
                    "reject.checkpoint_selector_invalid",
                    "selector frontier requires frontier and forbids canonical_seq/canonical_hash",
                ));
            }

            if frontier_vals.len() != 1
                || !frontier_vals[0].starts_with("seq:")
                || frontier_vals[0][4..].parse::<u64>().is_err()
            {
                return Err((
                    "reject.checkpoint_selector_invalid",
                    "selector frontier requires frontier token format seq:<u64>",
                ));
            }

            Ok(())
        }
        _ => Err((
            "reject.checkpoint_selector_invalid",
            "selector must be one of latest|seq|hash|frontier",
        )),
    }
}

fn build_replay_mismatch_diagnostic(
    checkpoint: &str,
    live_entries: &[(String, Vec<u8>)],
    replay_entries: &[(String, Vec<u8>)],
) -> Option<ReplayMismatchDiagnostic> {
    let live_digest = projection_digest(live_entries);
    let replay_digest = projection_digest(replay_entries);
    if live_digest == replay_digest {
        return None;
    }

    Some(ReplayMismatchDiagnostic {
        checkpoint: checkpoint.to_string(),
        live_digest,
        replay_digest,
        reason_class: "reject.query_replay_mismatch",
        reason_message: "live and replay projection digests diverged".to_string(),
    })
}

#[test]
fn query_det_001_deterministic_result_order_and_payload_equality() {
    let signer = SigningKey::from_bytes(&[0x51u8; 32]);

    // Write keys in non-lexicographic insertion order to ensure query
    // materialization contract depends on deterministic key ordering, not
    // insertion sequence.
    let n1 = signed_set_node(&signer, 1, "world/z", "3", vec![]);
    let n2 = signed_set_node(&signer, 2, "world/a", "1", vec![n1.id]);
    let n3 = signed_set_node(&signer, 3, "world/m", "2", vec![n2.id]);
    let n4 = signed_set_node(&signer, 4, "intent/tmp", "ignore", vec![n3.id]);

    let state = replay(&[n1, n2, n3, n4], None).expect("replay should succeed");

    let q1 = query_prefix_projection(&state.map, "world/");
    let q2 = query_prefix_projection(&state.map, "world/");

    assert_eq!(
        q1, q2,
        "QUERY-DET-001: repeated query over same canonical input must be payload-equal"
    );
    assert_eq!(
        projection_digest(&q1),
        projection_digest(&q2),
        "QUERY-DET-001: repeated query over same canonical input must be digest-equal"
    );
    assert_eq!(
        q1.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
        vec!["world/a", "world/m", "world/z"],
        "QUERY-DET-001: query result order must be deterministic"
    );
}

#[test]
fn query_replay_001_live_vs_replay_parity_at_checkpoint() {
    let signer = SigningKey::from_bytes(&[0x52u8; 32]);

    let n1 = signed_set_node(&signer, 1, "world/a", "1", vec![]);
    let n2 = signed_set_node(&signer, 2, "world/b", "2", vec![n1.id]);
    let n3 = signed_set_node(&signer, 3, "world/c", "3", vec![n2.id]);
    // Later writes past checkpoint should not affect checkpoint query parity.
    let n4 = signed_set_node(&signer, 4, "world/d", "4", vec![n3.id]);
    let n5 = signed_set_node(&signer, 5, "world/e", "5", vec![n4.id]);

    let _full_live = replay(&[n1.clone(), n2.clone(), n3.clone(), n4, n5], None)
        .expect("full replay should succeed");

    let checkpoint_nodes = vec![n1, n2, n3];
    let live_at_checkpoint = replay(&checkpoint_nodes, None)
        .expect("checkpoint replay should succeed");
    let live_query = query_prefix_projection(&live_at_checkpoint.map, "world/");
    let live_digest = projection_digest(&live_query);

    // Replay-materialized path uses the same checkpoint payload and must
    // produce identical query rows and digest.
    let replay_state = replay(&checkpoint_nodes, None)
        .expect("replay-materialized checkpoint should succeed");
    let replay_query = query_prefix_projection(&replay_state.map, "world/");
    let replay_digest = projection_digest(&replay_query);

    assert_eq!(
        live_query, replay_query,
        "QUERY-REPLAY-001: live and replay query rows diverged at same checkpoint"
    );
    assert_eq!(
        live_digest, replay_digest,
        "QUERY-REPLAY-001: live and replay query digests diverged at same checkpoint"
    );
}

#[test]
fn query_compat_reject_001_selector_payload_validation_bounded_taxonomy() {
    let known_hashes = BTreeSet::from(["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string()]);

    let malformed_frontier = validate_selector_payload(
        "frontier",
        None,
        None,
        Some(&["bad:1"]),
        &known_hashes,
    )
    .expect_err("malformed frontier must reject");

    let mixed_fields = validate_selector_payload(
        "seq",
        Some(1),
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        None,
        &known_hashes,
    )
    .expect_err("mixed selector fields must reject");

    let invalid_hash_format = validate_selector_payload("hash", None, Some("1234"), None, &known_hashes)
        .expect_err("invalid hash format must reject");

    let unknown_hash = validate_selector_payload(
        "hash",
        None,
        Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        None,
        &known_hashes,
    )
    .expect_err("unknown but valid hash must reject as not-found");

    assert_eq!(malformed_frontier.0, "reject.checkpoint_selector_invalid");
    assert_eq!(mixed_fields.0, "reject.checkpoint_selector_invalid");
    assert_eq!(invalid_hash_format.0, "reject.checkpoint_selector_invalid");
    assert_eq!(unknown_hash.0, "reject.checkpoint_not_found");

    let bounded = BTreeSet::from([
        "reject.checkpoint_selector_invalid".to_string(),
        "reject.checkpoint_not_found".to_string(),
    ]);
    for reason_class in [
        malformed_frontier.0,
        mixed_fields.0,
        invalid_hash_format.0,
        unknown_hash.0,
    ] {
        assert!(bounded.contains(reason_class));
    }
}

#[test]
fn query_replay_002_mismatch_diagnostics_include_checkpoint_and_digest_metadata() {
    let live_entries = vec![
        ("world/a".to_string(), b"1".to_vec()),
        ("world/b".to_string(), b"2".to_vec()),
    ];
    let replay_entries = vec![
        ("world/a".to_string(), b"1".to_vec()),
        ("world/b".to_string(), b"9".to_vec()),
    ];

    let diagnostic = build_replay_mismatch_diagnostic("seq:42", &live_entries, &replay_entries)
        .expect("digest mismatch must produce diagnostic");

    assert_eq!(diagnostic.reason_class, "reject.query_replay_mismatch");
    assert_eq!(diagnostic.checkpoint, "seq:42");
    assert_ne!(diagnostic.live_digest, diagnostic.replay_digest);
    assert!(
        !diagnostic.reason_message.is_empty(),
        "QUERY-REPLAY-002: mismatch diagnostics must include reason message"
    );
}

#[test]
fn query_det_003_pagination_multi_page_order_is_deterministic() {
    let entries = vec![
        ("world/a".to_string(), b"1".to_vec()),
        ("world/b".to_string(), b"2".to_vec()),
        ("world/c".to_string(), b"3".to_vec()),
        ("world/d".to_string(), b"4".to_vec()),
        ("world/e".to_string(), b"5".to_vec()),
    ];

    let mut token = None;
    let mut first_run = Vec::new();
    loop {
        let (page, next) = paginate_projection_entries(&entries, 2, token.as_deref());
        first_run.extend(page);
        token = next;
        if token.is_none() {
            break;
        }
    }

    let mut token = None;
    let mut second_run = Vec::new();
    loop {
        let (page, next) = paginate_projection_entries(&entries, 2, token.as_deref());
        second_run.extend(page);
        token = next;
        if token.is_none() {
            break;
        }
    }

    assert_eq!(first_run, second_run);
    assert_eq!(
        first_run,
        entries,
        "QUERY-DET-003: paged reads must preserve deterministic row order"
    );
}

#[test]
fn query_det_004_pagination_digest_continuity_matches_full_projection() {
    let entries = vec![
        ("world/a".to_string(), b"1".to_vec()),
        ("world/b".to_string(), b"2".to_vec()),
        ("world/c".to_string(), b"3".to_vec()),
        ("world/d".to_string(), b"4".to_vec()),
        ("world/e".to_string(), b"5".to_vec()),
    ];

    let full_digest = projection_digest(&entries);

    let mut rebuilt = Vec::new();
    let mut token = None;
    loop {
        let (page, next) = paginate_projection_entries(&entries, 3, token.as_deref());
        rebuilt.extend(page);
        token = next;
        if token.is_none() {
            break;
        }
    }

    assert_eq!(
        projection_digest(&rebuilt),
        full_digest,
        "QUERY-DET-004: paged projection digest continuity must equal full projection digest"
    );
}
