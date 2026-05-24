use activesync_core::{
    Hash, Op, OpId, StateGraph, SyncNode, TextOp, TextProjectionMode, TextProjectionResidencyPolicy,
    TextRangeAnchor, TextRuntimeTemperatureThresholds, Transaction,
};
use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use ed25519_dalek::SigningKey;

const TEXT_KEY: &str = "doc";
const LOCAL_AUTHOR_SEED: [u8; 32] = [0x31; 32];
const REMOTE_AUTHOR_A: [u8; 32] = [0xA1; 32];
const REMOTE_AUTHOR_B: [u8; 32] = [0xB2; 32];

fn parse_projection_mode(raw: &str) -> TextProjectionMode {
    match raw.trim().to_ascii_lowercase().as_str() {
        "disabled" => TextProjectionMode::Disabled,
        "parity" | "paritycheck" => TextProjectionMode::ParityCheck,
        _ => TextProjectionMode::Enabled,
    }
}

fn projection_mode_label(mode: TextProjectionMode) -> &'static str {
    match mode {
        TextProjectionMode::Disabled => "disabled",
        TextProjectionMode::Enabled => "enabled",
        TextProjectionMode::ParityCheck => "parity",
    }
}

fn apply_unsigned_text_op(
    graph: &mut StateGraph,
    lamport: &mut u64,
    parents: &mut Vec<Hash>,
    author: [u8; 32],
    op: TextOp,
) -> OpId {
    *lamport += 1;
    let tx = Transaction {
        author,
        lamport: *lamport,
        wall_ms: *lamport,
        ops: vec![Op::Text(op)],
        parents: parents.clone(),
    };
    let node = SyncNode::new(tx);
    let node_id = node.id;
    graph
        .apply_remote(node)
        .unwrap_or_else(|e| panic!("apply_remote failed at lamport {}: {e}", *lamport));
    *parents = vec![node_id];
    OpId {
        lamport: *lamport,
        author,
    }
}

fn build_remote_append_stream(author: [u8; 32], count: usize) -> Vec<SyncNode> {
    let mut lamport = 0u64;
    let mut parents: Vec<Hash> = Vec::new();
    let mut after: Option<OpId> = None;
    let mut out = Vec::with_capacity(count);

    for i in 0..count {
        lamport += 1;
        let ch = (b'a' + (i % 26) as u8) as char;
        let tx = Transaction {
            author,
            lamport,
            wall_ms: lamport,
            ops: vec![Op::Text(TextOp::Insert {
                key: TEXT_KEY.to_string(),
                after,
                ch,
            })],
            parents: parents.clone(),
        };
        let node = SyncNode::new(tx);
        let node_id = node.id;
        parents = vec![node_id];
        after = Some(OpId { lamport, author });
        out.push(node);
    }

    out
}

fn make_paste_payload(len: usize) -> String {
    (0..len)
        .map(|i| (b'a' + (i % 26) as u8) as char)
        .collect::<String>()
}

fn make_churn_insert_payload(step: usize, len: usize) -> String {
    (0..len)
        .map(|i| (b'a' + ((step + i) % 26) as u8) as char)
        .collect::<String>()
}

fn emit_snapshot(label: &str, graph: &StateGraph) {
    let apply = graph.text_apply_runtime_counters();
    let reads = graph.text_runtime_counters();
    eprintln!(
        "text_write_path_snapshot: label={label}, projection_update_calls={}, projection_update_total_ns={}, projection_invalidation_count={}, projection_rebuild_count={}, index_maintenance_total_ns={}, index_rebuild_total_ns={}, dirty_range_count_total={}, dirty_span_chars_total={}, dirty_range_merge_count_total={}, projection_hits={}, replay_fallbacks={}",
        apply.projection_update_calls,
        apply.projection_update_total_ns,
        apply.projection_invalidation_count,
        apply.projection_rebuild_count,
        apply.index_maintenance_total_ns,
        apply.index_rebuild_total_ns,
        apply.dirty_range_count_total,
        apply.dirty_span_chars_total,
        apply.dirty_range_merge_count_total,
        reads.projection_hits,
        reads.replay_fallbacks,
    );
}

fn bench_text_write_path(c: &mut Criterion) {
    let projection_mode = std::env::var("ACTIVESYNC_TEXT_WRITE_PROJECTION_MODE")
        .ok()
        .map(|v| parse_projection_mode(&v))
        .unwrap_or(TextProjectionMode::Enabled);
    let mode_label = projection_mode_label(projection_mode);

    let typing_ops = std::env::var("ACTIVESYNC_TEXT_WRITE_TYPING_OPS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(2000);
    let typing_read_every = std::env::var("ACTIVESYNC_TEXT_WRITE_TYPING_READ_EVERY")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(32);
    let paste_bursts = std::env::var("ACTIVESYNC_TEXT_WRITE_PASTE_BURSTS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(128);
    let paste_len = std::env::var("ACTIVESYNC_TEXT_WRITE_PASTE_LEN")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(32);
    let remote_burst_nodes = std::env::var("ACTIVESYNC_TEXT_WRITE_REMOTE_BURST_NODES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(2000);
    let mixed_steps = std::env::var("ACTIVESYNC_TEXT_WRITE_MIXED_STEPS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(2000);
    let mixed_remote_every = std::env::var("ACTIVESYNC_TEXT_WRITE_MIXED_REMOTE_EVERY")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(3);
    let emit_counters_snapshot = std::env::var("ACTIVESYNC_TEXT_WRITE_EMIT_SNAPSHOT")
        .ok()
        .map(|v| {
            let lower = v.trim().to_ascii_lowercase();
            lower == "1" || lower == "true" || lower == "yes"
        })
        .unwrap_or(false);
    let rebuild_doc_len = std::env::var("ACTIVESYNC_TEXT_WRITE_REBUILD_DOC_LEN")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(4096);
    let rebuild_window = std::env::var("ACTIVESYNC_TEXT_WRITE_REBUILD_WINDOW")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(256);
    let churn_base_len = std::env::var("ACTIVESYNC_TEXT_WRITE_CHURN_BASE_LEN")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1024);
    let churn_steps = std::env::var("ACTIVESYNC_TEXT_WRITE_CHURN_STEPS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1000);
    let churn_insert_len = std::env::var("ACTIVESYNC_TEXT_WRITE_CHURN_INSERT_LEN")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(4);
    let churn_delete_span = std::env::var("ACTIVESYNC_TEXT_WRITE_CHURN_DELETE_SPAN")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(3);

    let paste_payload = make_paste_payload(paste_len);
    let remote_nodes = build_remote_append_stream(REMOTE_AUTHOR_A, remote_burst_nodes);
    let mixed_remote_nodes = build_remote_append_stream(
        REMOTE_AUTHOR_B,
        mixed_steps.saturating_div(mixed_remote_every).saturating_add(1),
    );

    if emit_counters_snapshot {
        // One deterministic pass per workload emits apply/read counter snapshots
        // for manual run-to-run comparisons outside Criterion statistics.
        let mut graph = StateGraph::new();
        graph.set_text_projection_mode(projection_mode);
        let mut lamport = 0u64;
        let mut parents: Vec<Hash> = Vec::new();
        let mut after: Option<OpId> = None;
        for i in 0..typing_ops {
            let ch = (b'a' + (i % 26) as u8) as char;
            let inserted = apply_unsigned_text_op(
                &mut graph,
                &mut lamport,
                &mut parents,
                REMOTE_AUTHOR_A,
                TextOp::Insert {
                    key: TEXT_KEY.to_string(),
                    after,
                    ch,
                },
            );
            after = Some(inserted);
        }
        let _ = graph.resolve_text_range(TEXT_KEY, 0, 64);
        emit_snapshot("rapid_typing", &graph);

        let mut graph = StateGraph::new();
        graph.set_text_projection_mode(projection_mode);
        let mut lamport = 0u64;
        let mut parents: Vec<Hash> = Vec::new();
        for _ in 0..paste_bursts {
            apply_unsigned_text_op(
                &mut graph,
                &mut lamport,
                &mut parents,
                REMOTE_AUTHOR_A,
                TextOp::InsertRange {
                    key: TEXT_KEY.to_string(),
                    anchor: TextRangeAnchor::End,
                    text: paste_payload.clone(),
                },
            );
        }
        let _ = graph.resolve_text_range(TEXT_KEY, 0, 64);
        emit_snapshot("paste_bursts", &graph);

        let mut graph = StateGraph::new();
        graph.set_text_projection_mode(projection_mode);
        let _ = graph.apply_remote_batch(remote_nodes.clone());
        let _ = graph.resolve_text_range(TEXT_KEY, 0, 64);
        emit_snapshot("remote_burst", &graph);

        let mut graph = StateGraph::new();
        graph.set_text_projection_mode(projection_mode);
        let local_sk = SigningKey::from_bytes(&LOCAL_AUTHOR_SEED);
        let local_author = local_sk.verifying_key().to_bytes();
        let mut local_after: Option<OpId> = None;
        let mut remote_idx = 0usize;
        for step in 0..mixed_steps {
            if step % mixed_remote_every == 0 && remote_idx < mixed_remote_nodes.len() {
                graph
                    .apply_remote(mixed_remote_nodes[remote_idx].clone())
                    .unwrap_or_else(|e| panic!("snapshot mixed remote apply failed at step {step}: {e}"));
                remote_idx += 1;
            }
            let ch = (b'a' + (step % 26) as u8) as char;
            graph
                .apply_local(
                    &local_sk,
                    step as u64,
                    vec![Op::Text(TextOp::Insert {
                        key: TEXT_KEY.to_string(),
                        after: local_after,
                        ch,
                    })],
                )
                .unwrap_or_else(|e| panic!("snapshot mixed local apply failed at step {step}: {e}"));
            local_after = Some(OpId {
                lamport: graph.lamport(),
                author: local_author,
            });
        }
        let _ = graph.resolve_text_range(TEXT_KEY, 0, 64);
        emit_snapshot("mixed_local_remote", &graph);

        let mut graph = StateGraph::new();
        graph.set_text_projection_mode(projection_mode);
        let mut lamport = 0u64;
        let mut parents: Vec<Hash> = Vec::new();
        let mut visible: Vec<OpId> = Vec::with_capacity(churn_base_len);
        for i in 0..churn_base_len {
            let after = visible.last().copied();
            let ch = (b'a' + (i % 26) as u8) as char;
            let inserted = apply_unsigned_text_op(
                &mut graph,
                &mut lamport,
                &mut parents,
                REMOTE_AUTHOR_A,
                TextOp::Insert {
                    key: TEXT_KEY.to_string(),
                    after,
                    ch,
                },
            );
            visible.push(inserted);
        }

        for step in 0..churn_steps {
            let mut insert_at = visible.len() / 2;
            let insert_text = make_churn_insert_payload(step, churn_insert_len);
            for ch in insert_text.chars() {
                let after = if insert_at == 0 {
                    None
                } else {
                    Some(visible[insert_at - 1])
                };
                let inserted = apply_unsigned_text_op(
                    &mut graph,
                    &mut lamport,
                    &mut parents,
                    REMOTE_AUTHOR_A,
                    TextOp::Insert {
                        key: TEXT_KEY.to_string(),
                        after,
                        ch,
                    },
                );
                visible.insert(insert_at, inserted);
                insert_at += 1;
            }

            if visible.len() > 1 {
                let delete_count = churn_delete_span.min(visible.len() - 1);
                let mut delete_at = visible.len() / 2;
                if delete_at >= visible.len() {
                    delete_at = visible.len() - 1;
                }
                for _ in 0..delete_count {
                    let target = visible[delete_at];
                    apply_unsigned_text_op(
                        &mut graph,
                        &mut lamport,
                        &mut parents,
                        REMOTE_AUTHOR_A,
                        TextOp::Delete {
                            key: TEXT_KEY.to_string(),
                            target,
                        },
                    );
                    visible.remove(delete_at);
                    if visible.is_empty() {
                        break;
                    }
                    if delete_at >= visible.len() {
                        delete_at = visible.len() - 1;
                    }
                }
            }
        }

        let start = visible.len().saturating_sub(64) / 2;
        let _ = graph.resolve_text_range(TEXT_KEY, start, 64);
        emit_snapshot("mid_range_churn", &graph);
    }

    {
        let mut group = c.benchmark_group(format!("text_write_rapid_typing_{mode_label}"));
        group.throughput(Throughput::Elements(typing_ops as u64));
        group.bench_function("append_single_char", |b| {
            b.iter(|| {
                let mut graph = StateGraph::new();
                graph.set_text_projection_mode(projection_mode);
                let mut lamport = 0u64;
                let mut parents: Vec<Hash> = Vec::new();
                let mut after: Option<OpId> = None;

                for i in 0..typing_ops {
                    let ch = (b'a' + (i % 26) as u8) as char;
                    let inserted = apply_unsigned_text_op(
                        &mut graph,
                        &mut lamport,
                        &mut parents,
                        REMOTE_AUTHOR_A,
                        TextOp::Insert {
                            key: TEXT_KEY.to_string(),
                            after,
                            ch,
                        },
                    );
                    after = Some(inserted);
                }

                black_box(graph.resolve_text_range(TEXT_KEY, 0, 64).len());
            });
        });
        group.bench_function("append_single_char_apply_only", |b| {
            b.iter(|| {
                let mut graph = StateGraph::new();
                graph.set_text_projection_mode(projection_mode);
                let mut lamport = 0u64;
                let mut parents: Vec<Hash> = Vec::new();
                let mut after: Option<OpId> = None;

                for i in 0..typing_ops {
                    let ch = (b'a' + (i % 26) as u8) as char;
                    let inserted = apply_unsigned_text_op(
                        &mut graph,
                        &mut lamport,
                        &mut parents,
                        REMOTE_AUTHOR_A,
                        TextOp::Insert {
                            key: TEXT_KEY.to_string(),
                            after,
                            ch,
                        },
                    );
                    after = Some(inserted);
                }

                black_box(graph.lamport());
            });
        });
        group.bench_function("append_single_char_periodic_read", |b| {
            b.iter(|| {
                let mut graph = StateGraph::new();
                graph.set_text_projection_mode(projection_mode);
                let mut lamport = 0u64;
                let mut parents: Vec<Hash> = Vec::new();
                let mut after: Option<OpId> = None;

                for i in 0..typing_ops {
                    let ch = (b'a' + (i % 26) as u8) as char;
                    let inserted = apply_unsigned_text_op(
                        &mut graph,
                        &mut lamport,
                        &mut parents,
                        REMOTE_AUTHOR_A,
                        TextOp::Insert {
                            key: TEXT_KEY.to_string(),
                            after,
                            ch,
                        },
                    );
                    after = Some(inserted);

                    if (i + 1) % typing_read_every == 0 {
                        black_box(graph.resolve_text_range(TEXT_KEY, 0, 64).len());
                    }
                }

                black_box(graph.resolve_text_range(TEXT_KEY, 0, 64).len());
            });
        });
        group.finish();
    }

    {
        let mut group = c.benchmark_group(format!("text_write_paste_bursts_{mode_label}"));
        group.throughput(Throughput::Elements(
            paste_bursts.saturating_mul(paste_len) as u64,
        ));
        group.bench_function("insert_range_end", |b| {
            b.iter(|| {
                let mut graph = StateGraph::new();
                graph.set_text_projection_mode(projection_mode);
                let mut lamport = 0u64;
                let mut parents: Vec<Hash> = Vec::new();

                for _ in 0..paste_bursts {
                    apply_unsigned_text_op(
                        &mut graph,
                        &mut lamport,
                        &mut parents,
                        REMOTE_AUTHOR_A,
                        TextOp::InsertRange {
                            key: TEXT_KEY.to_string(),
                            anchor: TextRangeAnchor::End,
                            text: paste_payload.clone(),
                        },
                    );
                }

                black_box(graph.resolve_text_range(TEXT_KEY, 0, 64).len());
            });
        });
        group.finish();
    }

    {
        let mut group = c.benchmark_group(format!("text_write_remote_burst_{mode_label}"));
        group.throughput(Throughput::Elements(remote_burst_nodes as u64));
        group.bench_function("apply_remote_batch", |b| {
            b.iter(|| {
                let mut graph = StateGraph::new();
                graph.set_text_projection_mode(projection_mode);
                let batch = graph.apply_remote_batch(remote_nodes.clone());
                black_box(batch.accepted.len());
                black_box(graph.resolve_text_range(TEXT_KEY, 0, 64).len());
            });
        });
        group.bench_function("apply_remote_per_op", |b| {
            b.iter(|| {
                let mut graph = StateGraph::new();
                graph.set_text_projection_mode(projection_mode);
                for node in remote_nodes.clone() {
                    graph
                        .apply_remote(node)
                        .unwrap_or_else(|e| panic!("per-op remote apply failed: {e}"));
                }
                black_box(graph.resolve_text_range(TEXT_KEY, 0, 64).len());
            });
        });
        group.finish();
    }

    {
        let mut group = c.benchmark_group(format!("text_write_mixed_local_remote_{mode_label}"));
        group.throughput(Throughput::Elements(mixed_steps as u64));
        group.bench_function("interleaved_local_remote", |b| {
            b.iter(|| {
                let mut graph = StateGraph::new();
                graph.set_text_projection_mode(projection_mode);
                let local_sk = SigningKey::from_bytes(&LOCAL_AUTHOR_SEED);
                let local_author = local_sk.verifying_key().to_bytes();
                let mut local_after: Option<OpId> = None;
                let mut remote_idx = 0usize;

                for step in 0..mixed_steps {
                    if step % mixed_remote_every == 0 && remote_idx < mixed_remote_nodes.len() {
                        graph
                            .apply_remote(mixed_remote_nodes[remote_idx].clone())
                            .unwrap_or_else(|e| panic!("mixed remote apply failed at step {step}: {e}"));
                        remote_idx += 1;
                    }

                    let ch = (b'a' + (step % 26) as u8) as char;
                    graph
                        .apply_local(
                            &local_sk,
                            step as u64,
                            vec![Op::Text(TextOp::Insert {
                                key: TEXT_KEY.to_string(),
                                after: local_after,
                                ch,
                            })],
                        )
                        .unwrap_or_else(|e| panic!("mixed local apply failed at step {step}: {e}"));

                    local_after = Some(OpId {
                        lamport: graph.lamport(),
                        author: local_author,
                    });
                }

                black_box(graph.resolve_text_range(TEXT_KEY, 0, 64).len());
            });
        });
        group.finish();
    }

    {
        let mut group = c.benchmark_group(format!("text_write_run_churn_{mode_label}"));
        group.throughput(Throughput::Elements(churn_steps as u64));
        group.bench_function("mid_range_insert_delete_churn", |b| {
            b.iter(|| {
                let mut graph = StateGraph::new();
                graph.set_text_projection_mode(projection_mode);
                let mut lamport = 0u64;
                let mut parents: Vec<Hash> = Vec::new();
                let mut visible: Vec<OpId> = Vec::with_capacity(churn_base_len);

                for i in 0..churn_base_len {
                    let after = visible.last().copied();
                    let ch = (b'a' + (i % 26) as u8) as char;
                    let inserted = apply_unsigned_text_op(
                        &mut graph,
                        &mut lamport,
                        &mut parents,
                        REMOTE_AUTHOR_A,
                        TextOp::Insert {
                            key: TEXT_KEY.to_string(),
                            after,
                            ch,
                        },
                    );
                    visible.push(inserted);
                }

                for step in 0..churn_steps {
                    let mut insert_at = visible.len() / 2;
                    let insert_text = make_churn_insert_payload(step, churn_insert_len);
                    for ch in insert_text.chars() {
                        let after = if insert_at == 0 {
                            None
                        } else {
                            Some(visible[insert_at - 1])
                        };
                        let inserted = apply_unsigned_text_op(
                            &mut graph,
                            &mut lamport,
                            &mut parents,
                            REMOTE_AUTHOR_A,
                            TextOp::Insert {
                                key: TEXT_KEY.to_string(),
                                after,
                                ch,
                            },
                        );
                        visible.insert(insert_at, inserted);
                        insert_at += 1;
                    }

                    if visible.len() > 1 {
                        let delete_count = churn_delete_span.min(visible.len() - 1);
                        let mut delete_at = visible.len() / 2;
                        if delete_at >= visible.len() {
                            delete_at = visible.len() - 1;
                        }
                        for _ in 0..delete_count {
                            let target = visible[delete_at];
                            apply_unsigned_text_op(
                                &mut graph,
                                &mut lamport,
                                &mut parents,
                                REMOTE_AUTHOR_A,
                                TextOp::Delete {
                                    key: TEXT_KEY.to_string(),
                                    target,
                                },
                            );
                            visible.remove(delete_at);
                            if visible.is_empty() {
                                break;
                            }
                            if delete_at >= visible.len() {
                                delete_at = visible.len() - 1;
                            }
                        }
                    }
                }

                let start = visible.len().saturating_sub(64) / 2;
                black_box(graph.resolve_text_range(TEXT_KEY, start, 64).len());
            });
        });
        group.finish();
    }

    if projection_mode != TextProjectionMode::Disabled {
        let mut group = c.benchmark_group(format!("text_projection_rebuild_latency_{mode_label}"));
        group.throughput(Throughput::Elements(rebuild_doc_len as u64));
        group.bench_function("evict_then_rebuild_resident", |b| {
            b.iter(|| {
                let mut graph = StateGraph::new();
                graph.set_text_projection_mode(projection_mode);
                graph.set_text_runtime_temperature_thresholds(TextRuntimeTemperatureThresholds {
                    hot_read_calls: u64::MAX,
                    hot_write_calls: u64::MAX,
                });
                graph.set_text_projection_residency_policy(TextProjectionResidencyPolicy {
                    max_resident_keys: 1,
                });

                let mut lamport = 0u64;
                let mut parents: Vec<Hash> = Vec::new();
                let mut after: Option<OpId> = None;
                for i in 0..rebuild_doc_len {
                    let ch = (b'a' + (i % 26) as u8) as char;
                    let inserted = apply_unsigned_text_op(
                        &mut graph,
                        &mut lamport,
                        &mut parents,
                        REMOTE_AUTHOR_A,
                        TextOp::Insert {
                            key: TEXT_KEY.to_string(),
                            after,
                            ch,
                        },
                    );
                    after = Some(inserted);
                }

                // Touch another key to trigger policy eviction of the older warm key.
                apply_unsigned_text_op(
                    &mut graph,
                    &mut lamport,
                    &mut parents,
                    REMOTE_AUTHOR_A,
                    TextOp::Insert {
                        key: "aux".to_string(),
                        after: None,
                        ch: 'z',
                    },
                );

                black_box(graph.ensure_text_projection_resident(TEXT_KEY));
                black_box(graph.resolve_text_range(TEXT_KEY, 0, rebuild_window).len());
            });
        });
        group.finish();
    }
}

criterion_group!(benches, bench_text_write_path);
criterion_main!(benches);
