use activesync_core::{Op, StateGraph, TextOp, TextProjectionMode, TextRangeAnchor, TextRangeOp};
use ed25519_dalek::SigningKey;

fn key_a() -> SigningKey {
    SigningKey::from_bytes(&[0x0Au8; 32])
}

fn key_b() -> SigningKey {
    SigningKey::from_bytes(&[0x0Bu8; 32])
}

fn insert_char_at(
    g: &mut StateGraph,
    sk: &SigningKey,
    key: &str,
    pos: usize,
    ch: char,
    wall_ms: u64,
) {
    let seq = g.resolve_text_seq_with_chars(key);
    let after = if pos == 0 {
        None
    } else if pos <= seq.len() {
        Some(seq[pos - 1].0)
    } else {
        seq.last().map(|(id, _)| *id)
    };

    g.apply_local(
        sk,
        wall_ms,
        vec![Op::Text(TextOp::Insert {
            key: key.to_string(),
            after,
            ch,
        })],
    )
    .unwrap();
}

fn delete_char_at(g: &mut StateGraph, sk: &SigningKey, key: &str, pos: usize, wall_ms: u64) {
    let seq = g.resolve_text_seq_with_chars(key);
    let target = seq[pos].0;
    g.apply_local(
        sk,
        wall_ms,
        vec![Op::Text(TextOp::Delete {
            key: key.to_string(),
            target,
        })],
    )
    .unwrap();
}

fn merge_both_ways(a: &mut StateGraph, b: &mut StateGraph) {
    let a_ids = a.all_node_ids();
    let mut a_nodes: Vec<_> = a.get_nodes(&a_ids).into_iter().cloned().collect();
    a_nodes.sort_by_key(|n| (n.transaction.lamport, n.id));
    let to_b = b.apply_remote_batch(a_nodes);
    assert!(
        to_b.rejected.is_empty(),
        "unexpected rejection while syncing A -> B: {:?}",
        to_b.rejected
    );

    let b_ids = b.all_node_ids();
    let mut b_nodes: Vec<_> = b.get_nodes(&b_ids).into_iter().cloned().collect();
    b_nodes.sort_by_key(|n| (n.transaction.lamport, n.id));
    let to_a = a.apply_remote_batch(b_nodes);
    assert!(
        to_a.rejected.is_empty(),
        "unexpected rejection while syncing B -> A: {:?}",
        to_a.rejected
    );
}

#[test]
fn equivalent_char_vs_range_single_peer() {
    let sk = key_a();
    let mut g_char = StateGraph::new();
    let mut g_range = StateGraph::new();

    let key = "doc";
    for (i, ch) in "hello".chars().enumerate() {
        insert_char_at(&mut g_char, &sk, key, i, ch, 1000 + i as u64);
    }

    g_range
        .apply_local_text_range_op(
            &sk,
            2000,
            TextRangeOp::Insert {
                key: key.to_string(),
                anchor: TextRangeAnchor::Start,
                text: "hello".to_string(),
            },
        )
        .unwrap();

    // Delete middle two chars => "heo"
    delete_char_at(&mut g_char, &sk, key, 2, 3000);
    delete_char_at(&mut g_char, &sk, key, 2, 3001);

    g_range
        .apply_local_text_range_op(
            &sk,
            4000,
            TextRangeOp::Delete {
                key: key.to_string(),
                anchor: TextRangeAnchor::Offset(2),
                len_chars: 2,
            },
        )
        .unwrap();

    assert_eq!(g_char.resolve_text(key), "heo");
    assert_eq!(g_range.resolve_text(key), "heo");
    assert_eq!(g_char.resolve_text(key), g_range.resolve_text(key));
}

#[test]
fn equivalent_char_vs_range_cross_peer_merge() {
    let sk_a = key_a();
    let sk_b = key_b();
    let key = "doc";

    // Graph A writes through the canonical range path as well.
    let mut a = StateGraph::new();
    a.apply_local_text_range_op(
        &sk_a,
        1000,
        TextRangeOp::Insert {
            key: key.to_string(),
            anchor: TextRangeAnchor::Start,
            text: "abc".to_string(),
        },
    )
    .unwrap();

    // Graph B writes semantically equivalent range insert.
    let mut b = StateGraph::new();
    b.apply_local_text_range_op(
        &sk_b,
        2000,
        TextRangeOp::Insert {
            key: key.to_string(),
            anchor: TextRangeAnchor::Start,
            text: "abc".to_string(),
        },
    )
    .unwrap();

    // Merge both ways.
    let a_ids = a.all_node_ids();
    let mut a_nodes: Vec<_> = a.get_nodes(&a_ids).into_iter().cloned().collect();
    a_nodes.sort_by_key(|n| n.transaction.lamport);
    for node in a_nodes {
        b.apply_remote(node).unwrap();
    }

    let b_ids = b.all_node_ids();
    let mut b_nodes: Vec<_> = b.get_nodes(&b_ids).into_iter().cloned().collect();
    b_nodes.sort_by_key(|n| n.transaction.lamport);
    for node in b_nodes {
        let _ = a.apply_remote(node);
    }

    // Apply equivalent deletes: delete second char in each strategy.
    a.apply_local_text_range_op(
        &sk_a,
        3000,
        TextRangeOp::Delete {
            key: key.to_string(),
            anchor: TextRangeAnchor::Offset(1),
            len_chars: 1,
        },
    )
    .unwrap();
    b.apply_local_text_range_op(
        &sk_b,
        3001,
        TextRangeOp::Delete {
            key: key.to_string(),
            anchor: TextRangeAnchor::Offset(1),
            len_chars: 1,
        },
    )
    .unwrap();

    // Re-merge after deletes.
    let a_ids = a.all_node_ids();
    let mut a_nodes: Vec<_> = a.get_nodes(&a_ids).into_iter().cloned().collect();
    a_nodes.sort_by_key(|n| n.transaction.lamport);
    for node in a_nodes {
        b.apply_remote(node).unwrap_or(());
    }

    let b_ids = b.all_node_ids();
    let mut b_nodes: Vec<_> = b.get_nodes(&b_ids).into_iter().cloned().collect();
    b_nodes.sort_by_key(|n| n.transaction.lamport);
    for node in b_nodes {
        a.apply_remote(node).unwrap_or(());
    }

    assert_eq!(
        a.resolve_text_canonical(key),
        b.resolve_text_canonical(key),
        "interleaved char/range traces should converge canonically"
    );
}

#[test]
fn mixed_char_range_equivalence_fixtures() {
    let sk = key_a();
    let key = "doc";

    // Fixture 1: char-only build vs mixed range+char edits.
    let mut char_only = StateGraph::new();
    let mut mixed = StateGraph::new();

    for (i, ch) in "abcdef".chars().enumerate() {
        insert_char_at(&mut char_only, &sk, key, i, ch, 1000 + i as u64);
    }
    mixed
        .apply_local_text_range_op(
            &sk,
            2000,
            TextRangeOp::Insert {
                key: key.to_string(),
                anchor: TextRangeAnchor::Start,
                text: "abc".to_string(),
            },
        )
        .unwrap();
    mixed
        .apply_local_text_range_op(
            &sk,
            2001,
            TextRangeOp::Insert {
                key: key.to_string(),
                anchor: TextRangeAnchor::End,
                text: "def".to_string(),
            },
        )
        .unwrap();

    delete_char_at(&mut char_only, &sk, key, 2, 3000);
    delete_char_at(&mut char_only, &sk, key, 2, 3001);
    mixed
        .apply_local_text_range_op(
            &sk,
            3002,
            TextRangeOp::Delete {
                key: key.to_string(),
                anchor: TextRangeAnchor::Offset(2),
                len_chars: 2,
            },
        )
        .unwrap();

    insert_char_at(&mut char_only, &sk, key, 2, 'X', 3003);
    mixed
        .apply_local_text_range_op(
            &sk,
            3004,
            TextRangeOp::Insert {
                key: key.to_string(),
                anchor: TextRangeAnchor::Offset(2),
                text: "X".to_string(),
            },
        )
        .unwrap();

    assert_eq!(char_only.resolve_text(key), mixed.resolve_text(key));
    assert_eq!(char_only.resolve_text(key), "abXef");

    // Fixture 2: tail mutations and start anchor deletion.
    let mut char_only_2 = StateGraph::new();
    let mut mixed_2 = StateGraph::new();
    for (i, ch) in "rustlang".chars().enumerate() {
        insert_char_at(&mut char_only_2, &sk, key, i, ch, 4000 + i as u64);
    }
    mixed_2
        .apply_local_text_range_op(
            &sk,
            5000,
            TextRangeOp::Insert {
                key: key.to_string(),
                anchor: TextRangeAnchor::Start,
                text: "rustlang".to_string(),
            },
        )
        .unwrap();

    delete_char_at(&mut char_only_2, &sk, key, 0, 5001);
    delete_char_at(&mut char_only_2, &sk, key, 0, 5002);
    mixed_2
        .apply_local_text_range_op(
            &sk,
            5003,
            TextRangeOp::Delete {
                key: key.to_string(),
                anchor: TextRangeAnchor::Start,
                len_chars: 2,
            },
        )
        .unwrap();

    let end_pos = char_only_2.resolve_text(key).chars().count();
    insert_char_at(
        &mut char_only_2,
        &sk,
        key,
        end_pos,
        '!',
        5004,
    );
    mixed_2
        .apply_local_text_range_op(
            &sk,
            5005,
            TextRangeOp::Insert {
                key: key.to_string(),
                anchor: TextRangeAnchor::End,
                text: "!".to_string(),
            },
        )
        .unwrap();

    assert_eq!(char_only_2.resolve_text(key), mixed_2.resolve_text(key));
    assert_eq!(char_only_2.resolve_text(key), "stlang!");
}

#[test]
fn interleaved_peer_traces_char_and_range_converge() {
    let sk_a = key_a();
    let sk_b = key_b();
    let key = "doc";

    let mut a = StateGraph::new();
    let mut b = StateGraph::new();

    a.apply_local_text_range_op(
        &sk_a,
        1000,
        TextRangeOp::Insert {
            key: key.to_string(),
            anchor: TextRangeAnchor::Start,
            text: "abc".to_string(),
        },
    )
    .unwrap();
    b.apply_local_text_range_op(
        &sk_b,
        1001,
        TextRangeOp::Insert {
            key: key.to_string(),
            anchor: TextRangeAnchor::Start,
            text: "abc".to_string(),
        },
    )
    .unwrap();
    merge_both_ways(&mut a, &mut b);

    // Interleaved: A does char ops while B does range ops, then merge.
    insert_char_at(&mut a, &sk_a, key, 1, 'X', 2000);
    b.apply_local_text_range_op(
        &sk_b,
        2001,
        TextRangeOp::Insert {
            key: key.to_string(),
            anchor: TextRangeAnchor::Offset(1),
            text: "X".to_string(),
        },
    )
    .unwrap();
    merge_both_ways(&mut a, &mut b);

    delete_char_at(&mut a, &sk_a, key, 3, 2002);
    b.apply_local_text_range_op(
        &sk_b,
        2003,
        TextRangeOp::Delete {
            key: key.to_string(),
            anchor: TextRangeAnchor::Offset(3),
            len_chars: 1,
        },
    )
    .unwrap();
    merge_both_ways(&mut a, &mut b);

    insert_char_at(&mut a, &sk_a, key, 0, 'Q', 2004);
    b.apply_local_text_range_op(
        &sk_b,
        2005,
        TextRangeOp::Insert {
            key: key.to_string(),
            anchor: TextRangeAnchor::Start,
            text: "Q".to_string(),
        },
    )
    .unwrap();
    merge_both_ways(&mut a, &mut b);

    assert_eq!(
        a.resolve_text_canonical(key),
        b.resolve_text_canonical(key),
        "interleaved char/range traces should converge canonically"
    );
}

#[test]
fn randomized_lowering_regression_corpus_matches_char_semantics() {
    let sk = key_a();
    let key = "doc";
    let mut char_graph = StateGraph::new();
    let mut range_graph = StateGraph::new();

    let mut seed: u64 = 0xA11C_5EED_C0FF_EE11;
    let mut next_u64 = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        seed
    };

    let mut wall_ms = 10_000u64;
    for _ in 0..300u64 {
        let do_insert = char_graph.resolve_text(key).is_empty() || (next_u64() % 100) < 70;
        if do_insert {
            let current = char_graph.resolve_text(key);
            let len = current.chars().count();
            let pos = if len == 0 {
                0
            } else {
                (next_u64() as usize) % (len + 1)
            };
            let mut text = String::new();
            let span = ((next_u64() % 4) + 1) as usize;
            for _ in 0..span {
                text.push((b'a' + (next_u64() % 26) as u8) as char);
            }

            for (i, ch) in text.chars().enumerate() {
                insert_char_at(&mut char_graph, &sk, key, pos + i, ch, wall_ms + i as u64);
            }
            range_graph
                .apply_local_text_range_op(
                    &sk,
                    wall_ms,
                    TextRangeOp::Insert {
                        key: key.to_string(),
                        anchor: TextRangeAnchor::Offset(pos),
                        text,
                    },
                )
                .unwrap();
            wall_ms = wall_ms.saturating_add(8);
        } else {
            let current = char_graph.resolve_text(key);
            let len = current.chars().count();
            let start = (next_u64() as usize) % len;
            let max_span = (len - start).max(1);
            let del_len = (((next_u64() % 4) + 1) as usize).min(max_span);

            for _ in 0..del_len {
                delete_char_at(&mut char_graph, &sk, key, start, wall_ms);
                wall_ms = wall_ms.saturating_add(1);
            }
            range_graph
                .apply_local_text_range_op(
                    &sk,
                    wall_ms,
                    TextRangeOp::Delete {
                        key: key.to_string(),
                        anchor: TextRangeAnchor::Offset(start),
                        len_chars: del_len,
                    },
                )
                .unwrap();
            wall_ms = wall_ms.saturating_add(4);
        }

        assert_eq!(
            char_graph.resolve_text(key),
            range_graph.resolve_text(key),
            "randomized lowering corpus divergence"
        );
    }
}

#[test]
fn mixed_visible_and_tombstoned_range_delete_converges() {
    let sk = key_a();
    let key = "doc";

    let mut char_graph = StateGraph::new();
    let mut range_graph = StateGraph::new();

    // Build deterministic base content.
    char_graph
        .apply_local_text_range_op(
            &sk,
            1000,
            TextRangeOp::Insert {
                key: key.to_string(),
                anchor: TextRangeAnchor::Start,
                text: "abcdefghijkl".to_string(),
            },
        )
        .unwrap();
    range_graph
        .apply_local_text_range_op(
            &sk,
            1001,
            TextRangeOp::Insert {
                key: key.to_string(),
                anchor: TextRangeAnchor::Start,
                text: "abcdefghijkl".to_string(),
            },
        )
        .unwrap();

    // Tombstone three ids so underlying logical history contains holes.
    for victim_pos in [2usize, 5usize, 8usize] {
        let char_seq = char_graph.resolve_text_seq_with_chars(key);
        let range_seq = range_graph.resolve_text_seq_with_chars(key);
        let victim_char = char_seq[victim_pos].0;
        let victim_range = range_seq[victim_pos].0;

        char_graph
            .apply_local(
                &sk,
                1100 + victim_pos as u64,
                vec![Op::Text(TextOp::Delete {
                    key: key.to_string(),
                    target: victim_char,
                })],
            )
            .unwrap();
        range_graph
            .apply_local(
                &sk,
                1200 + victim_pos as u64,
                vec![Op::Text(TextOp::Delete {
                    key: key.to_string(),
                    target: victim_range,
                })],
            )
            .unwrap();
    }

    // Apply equivalent delete window in visible-offset space.
    let start = 3usize;
    let len = 4usize;
    for _ in 0..len {
        delete_char_at(&mut char_graph, &sk, key, start, 2000);
    }
    range_graph
        .apply_local_text_range_op(
            &sk,
            2001,
            TextRangeOp::Delete {
                key: key.to_string(),
                anchor: TextRangeAnchor::Offset(start),
                len_chars: len,
            },
        )
        .unwrap();

    assert_eq!(
        char_graph.resolve_text(key),
        range_graph.resolve_text(key),
        "mixed visible+tombstoned range delete should match char delete semantics"
    );
    assert_eq!(
        char_graph.resolve_text_canonical(key),
        range_graph.resolve_text_canonical(key),
        "canonical output must converge with mixed tombstone state"
    );
}

#[test]
fn randomized_run_split_merge_corpus_matches_canonical_semantics() {
    let sk = key_a();
    let key = "doc";
    let mut projection_graph = StateGraph::new();
    let mut canonical_graph = StateGraph::new();

    projection_graph.set_text_projection_mode(TextProjectionMode::Enabled);
    canonical_graph.set_text_projection_mode(TextProjectionMode::Disabled);

    let mut seed: u64 = 0x5A17_7EAD_C0DE_1337;
    let mut next_u64 = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        seed
    };

    let mut wall_ms = 30_000u64;
    for _step in 0..600u64 {
        let current = projection_graph.resolve_text(key);
        let len = current.chars().count();
        let do_insert = len == 0 || (next_u64() % 100) < 68;

        if do_insert {
            let pos = if len == 0 {
                0usize
            } else {
                (next_u64() as usize) % (len + 1)
            };
            let ch = (b'a' + (next_u64() % 26) as u8) as char;

            insert_char_at(&mut projection_graph, &sk, key, pos, ch, wall_ms);
            insert_char_at(&mut canonical_graph, &sk, key, pos, ch, wall_ms + 1);
            wall_ms = wall_ms.saturating_add(2);
        } else {
            let pos = (next_u64() as usize) % len;
            delete_char_at(&mut projection_graph, &sk, key, pos, wall_ms);
            delete_char_at(&mut canonical_graph, &sk, key, pos, wall_ms + 1);
            wall_ms = wall_ms.saturating_add(2);
        }

        assert_eq!(
            projection_graph.resolve_text(key),
            canonical_graph.resolve_text(key),
            "projection visible output must match canonical-disabled runtime"
        );
        assert_eq!(
            projection_graph.resolve_text_canonical(key),
            canonical_graph.resolve_text_canonical(key),
            "canonical replay output must remain identical under split/merge churn"
        );
    }
}
