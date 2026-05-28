use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use ed25519_dalek::SigningKey;
use nodalmerge_core::{canonical_hash, Hash, MapOp, Op, StateGraph};
use nodalmerge_server::room::{import_nodes, Room};
use nodalmerge_server::store::{NoPersistence, SharedPersistence};

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReplayMismatchDiagnostic {
    checkpoint: String,
    live_digest: Hash,
    replay_digest: Hash,
    reason_class: &'static str,
    reason_message: String,
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

    let page: Vec<(String, Vec<u8>)> = entries.iter().skip(start).take(limit).cloned().collect();

    let next = if start + page.len() < entries.len() {
        Some(format!("offset:{}", start + page.len()))
    } else {
        None
    };

    (page, next)
}

fn projection_rows(
    state: &std::collections::HashMap<String, Vec<u8>>,
    prefix: &str,
) -> Vec<(String, Vec<u8>)> {
    let mut rows: Vec<(String, Vec<u8>)> = state
        .iter()
        .filter(|(k, _)| k.starts_with(prefix))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

fn build_set_nodes(sk: &SigningKey, writes: &[(&str, &str)]) -> Vec<nodalmerge_core::SyncNode> {
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

#[tokio::test]
async fn server_query_replay_001_live_vs_replay_parity_at_checkpoint() {
    let persistence: SharedPersistence = Arc::new(NoPersistence);
    let signer = SigningKey::from_bytes(&[0x79u8; 32]);

    let checkpoint_room = Room::new(
        "query-replay-checkpoint".to_string(),
        Arc::clone(&persistence),
        512,
    );
    let replay_room = Room::new(
        "query-replay-materialized".to_string(),
        Arc::clone(&persistence),
        512,
    );

    let checkpoint_nodes = build_set_nodes(
        &signer,
        &[("world/a", "1"), ("world/b", "2"), ("world/c", "3")],
    );

    let (accepted_a, _, errs_a) = import_nodes(&checkpoint_room, checkpoint_nodes.clone()).await;
    let (accepted_b, _, errs_b) = import_nodes(&replay_room, checkpoint_nodes).await;
    assert_eq!(accepted_a, 3);
    assert_eq!(accepted_b, 3);
    assert!(errs_a.is_empty());
    assert!(errs_b.is_empty());

    let checkpoint_state = checkpoint_room.graph.read().await.resolve();
    let replay_state = replay_room.graph.read().await.resolve();

    let checkpoint_rows = projection_rows(&checkpoint_state, "world/");
    let replay_rows = projection_rows(&replay_state, "world/");

    assert_eq!(checkpoint_rows, replay_rows);
    assert_eq!(
        projection_digest(&checkpoint_rows),
        projection_digest(&replay_rows)
    );
}

/// Phase E run-03 (server lane): materialize the same checkpoint twice via `import_nodes`,
/// build the prefix projection from resolved room state, and assert paged reads preserve
/// digest continuity (mirrors core `query_phasee_replay_003_e2e_checkpoint_pagination_digest_parity`).
#[tokio::test]
async fn server_query_phasee_replay_003_e2e_checkpoint_pagination_digest_parity() {
    let persistence: SharedPersistence = Arc::new(NoPersistence);
    let signer = SigningKey::from_bytes(&[0x7au8; 32]);

    let room_a = Room::new(
        "query-phasee-replay-a".to_string(),
        Arc::clone(&persistence),
        512,
    );
    let room_b = Room::new(
        "query-phasee-replay-b".to_string(),
        Arc::clone(&persistence),
        512,
    );

    let checkpoint_nodes = build_set_nodes(
        &signer,
        &[("world/a", "1"), ("world/b", "2"), ("world/c", "3")],
    );

    let (accepted_a, _, errs_a) = import_nodes(&room_a, checkpoint_nodes.clone()).await;
    let (accepted_b, _, errs_b) = import_nodes(&room_b, checkpoint_nodes).await;
    assert_eq!(accepted_a, 3);
    assert_eq!(accepted_b, 3);
    assert!(errs_a.is_empty());
    assert!(errs_b.is_empty());

    let rows_a = projection_rows(&room_a.graph.read().await.resolve(), "world/");
    let rows_b = projection_rows(&room_b.graph.read().await.resolve(), "world/");

    assert_eq!(rows_a, rows_b);
    let full_digest = projection_digest(&rows_a);
    const PAGE_LIMIT: usize = 2;

    for (label, rows) in [("room_a", &rows_a), ("room_b", &rows_b)] {
        let mut rebuilt = Vec::new();
        let mut token = None;
        loop {
            let (page, next) = paginate_projection_entries(rows, PAGE_LIMIT, token.as_deref());
            rebuilt.extend(page);
            token = next;
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            *rows, rebuilt,
            "QUERY-PHASEE-REPLAY-E2E-001 server: pagination must reconstruct full row set ({label})"
        );
        assert_eq!(
            projection_digest(&rebuilt),
            full_digest,
            "QUERY-PHASEE-REPLAY-E2E-001 server: paged projection digest must equal full projection digest ({label})"
        );
    }
}

#[test]
fn server_query_compat_reject_001_selector_payload_validation_bounded_taxonomy() {
    let known_hashes = BTreeSet::from([
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
    ]);

    let malformed_frontier =
        validate_selector_payload("frontier", None, None, Some(&["bad:1"]), &known_hashes)
            .expect_err("malformed frontier must reject");

    let mixed_fields = validate_selector_payload(
        "seq",
        Some(1),
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        None,
        &known_hashes,
    )
    .expect_err("mixed selector fields must reject");

    let invalid_hash_format =
        validate_selector_payload("hash", None, Some("1234"), None, &known_hashes)
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
fn server_query_replay_002_mismatch_diagnostics_include_checkpoint_and_digest_metadata() {
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
    assert!(!diagnostic.reason_message.is_empty());
}

#[test]
fn server_query_det_003_pagination_multi_page_order_is_deterministic() {
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
    assert_eq!(first_run, entries);
}

#[test]
fn server_query_det_004_pagination_digest_continuity_matches_full_projection() {
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

    assert_eq!(projection_digest(&rebuilt), full_digest);
}
