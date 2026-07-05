use std::{
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
};

use nodalmerge_core::{
    Hash, Op, OpId, StateGraph, SyncNode, TextOp, TextProjectionMode, Transaction,
};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use serde::Deserialize;

const TEXT_KEY: &str = "doc";
const BENCH_AUTHOR: [u8; 32] = [0xA5; 32];

#[derive(Debug, Deserialize)]
struct EditingTrace {
    #[serde(default, rename = "startContent")]
    start_content: String,
    #[serde(rename = "endContent")]
    end_content: String,
    txns: Vec<TraceTxn>,
}

#[derive(Debug, Deserialize)]
struct TraceTxn {
    patches: Vec<(usize, usize, String)>,
}

fn trace_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
        .join("rustcode.json")
}

fn load_trace() -> EditingTrace {
    let path = trace_path();
    let file = File::open(&path)
        .unwrap_or_else(|e| panic!("failed to open {}: {e}", path.display()));
    serde_json::from_reader(BufReader::new(file))
        .unwrap_or_else(|e| panic!("failed to parse {}: {e}", path.display()))
}

fn apply_unsigned_text_op(
    graph: &mut StateGraph,
    lamport: &mut u64,
    parents: &mut Vec<Hash>,
    op: TextOp,
) -> OpId {
    *lamport += 1;
    let tx = Transaction {
        author: BENCH_AUTHOR,
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
        author: BENCH_AUTHOR,
    }
}

fn try_apply_unsigned_text_op(
    graph: &mut StateGraph,
    lamport: &mut u64,
    parents: &mut Vec<Hash>,
    op: TextOp,
    max_ops: Option<usize>,
    applied_ops: &mut usize,
) -> bool {
    if let Some(limit) = max_ops {
        if *applied_ops >= limit {
            return false;
        }
    }
    apply_unsigned_text_op(graph, lamport, parents, op);
    *applied_ops += 1;
    true
}

fn parse_projection_mode(raw: &str) -> TextProjectionMode {
    match raw.trim().to_ascii_lowercase().as_str() {
        "enabled" => TextProjectionMode::Enabled,
        "parity" | "paritycheck" => TextProjectionMode::ParityCheck,
        _ => TextProjectionMode::Disabled,
    }
}

fn projection_mode_label(mode: TextProjectionMode) -> &'static str {
    match mode {
        TextProjectionMode::Disabled => "disabled",
        TextProjectionMode::Enabled => "enabled",
        TextProjectionMode::ParityCheck => "parity",
    }
}

fn replay_trace_unsigned(
    trace: &EditingTrace,
    max_ops: Option<usize>,
    projection_mode: TextProjectionMode,
    parity_sample_every: u64,
) -> (StateGraph, usize) {
    let mut graph = StateGraph::new();
    graph.set_text_projection_mode(projection_mode);
    if projection_mode == TextProjectionMode::ParityCheck {
        graph.set_text_parity_sample_every(parity_sample_every);
    }
    let mut lamport = 0_u64;
    let mut parents: Vec<Hash> = Vec::new();
    let mut visible: Vec<OpId> = Vec::with_capacity(trace.end_content.chars().count());
    let mut applied_ops = 0_usize;

    for ch in trace.start_content.chars() {
        let after = visible.last().copied();
        if !try_apply_unsigned_text_op(
            &mut graph,
            &mut lamport,
            &mut parents,
            TextOp::Insert {
                key: TEXT_KEY.to_string(),
                after,
                ch,
            },
            max_ops,
            &mut applied_ops,
        ) {
            return (graph, applied_ops);
        }
        visible.push(OpId {
            lamport,
            author: BENCH_AUTHOR,
        });
    }

    for txn in &trace.txns {
        for (pos, del_count, insert_text) in &txn.patches {
            let mut cur_pos = *pos;
            if cur_pos > visible.len() {
                panic!(
                    "patch position {} out of bounds for visible length {}",
                    cur_pos,
                    visible.len()
                );
            }

            for _ in 0..*del_count {
                if cur_pos >= visible.len() {
                    panic!(
                        "delete position {} out of bounds for visible length {}",
                        cur_pos,
                        visible.len()
                    );
                }
                let target = visible[cur_pos];
                if !try_apply_unsigned_text_op(
                    &mut graph,
                    &mut lamport,
                    &mut parents,
                    TextOp::Delete {
                        key: TEXT_KEY.to_string(),
                        target,
                    },
                    max_ops,
                    &mut applied_ops,
                ) {
                    return (graph, applied_ops);
                }
                visible.remove(cur_pos);
            }

            for ch in insert_text.chars() {
                let after = if cur_pos == 0 {
                    None
                } else {
                    Some(visible[cur_pos - 1])
                };
                if !try_apply_unsigned_text_op(
                    &mut graph,
                    &mut lamport,
                    &mut parents,
                    TextOp::Insert {
                        key: TEXT_KEY.to_string(),
                        after,
                        ch,
                    },
                    max_ops,
                    &mut applied_ops,
                ) {
                    return (graph, applied_ops);
                }
                visible.insert(
                    cur_pos,
                    OpId {
                        lamport,
                        author: BENCH_AUTHOR,
                    },
                );
                cur_pos += 1;
            }
        }
    }

    (graph, applied_ops)
}

fn build_linear_text_graph(len: usize, mode: TextProjectionMode) -> StateGraph {
    let mut graph = StateGraph::new();
    graph.set_text_projection_mode(mode);
    let mut lamport = 0_u64;
    let mut parents: Vec<Hash> = Vec::new();
    let mut visible: Vec<OpId> = Vec::with_capacity(len);

    for i in 0..len {
        let after = visible.last().copied();
        let ch = (b'a' + (i % 26) as u8) as char;
        apply_unsigned_text_op(
            &mut graph,
            &mut lamport,
            &mut parents,
            TextOp::Insert {
                key: TEXT_KEY.to_string(),
                after,
                ch,
            },
        );
        visible.push(OpId {
            lamport,
            author: BENCH_AUTHOR,
        });
    }

    graph
}

fn build_run_split_churn_graph(
    base_len: usize,
    churn_steps: usize,
    churn_insert_len: usize,
    churn_delete_span: usize,
    mode: TextProjectionMode,
    parity_sample_every: u64,
) -> StateGraph {
    let mut graph = StateGraph::new();
    graph.set_text_projection_mode(mode);
    if mode == TextProjectionMode::ParityCheck {
        graph.set_text_parity_sample_every(parity_sample_every);
    }

    let mut lamport = 0_u64;
    let mut parents: Vec<Hash> = Vec::new();
    let mut visible: Vec<OpId> = Vec::with_capacity(base_len);

    for i in 0..base_len {
        let after = visible.last().copied();
        let ch = (b'a' + (i % 26) as u8) as char;
        let inserted = apply_unsigned_text_op(
            &mut graph,
            &mut lamport,
            &mut parents,
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
        for i in 0..churn_insert_len {
            let ch = (b'a' + ((step + i) % 26) as u8) as char;
            let after = if insert_at == 0 {
                None
            } else {
                Some(visible[insert_at - 1])
            };
            let inserted = apply_unsigned_text_op(
                &mut graph,
                &mut lamport,
                &mut parents,
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

    graph
}

fn bench_text_trace_rustcode(c: &mut Criterion) {
    let trace = load_trace();
    let max_ops = std::env::var("ACTIVESYNC_TEXT_TRACE_MAX_OPS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok());
    let range_len = std::env::var("ACTIVESYNC_TEXT_TRACE_RANGE_LEN")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(256);
    let range_stride = std::env::var("ACTIVESYNC_TEXT_TRACE_RANGE_STRIDE")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(512);
    let range_windows = std::env::var("ACTIVESYNC_TEXT_TRACE_RANGE_WINDOWS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(8);
    let parity_sample_every = std::env::var("ACTIVESYNC_TEXT_TRACE_PARITY_SAMPLE_EVERY")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(16);
    let run_split_base_len = std::env::var("ACTIVESYNC_TEXT_TRACE_RUN_SPLIT_BASE_LEN")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(4096);
    let run_split_churn_steps = std::env::var("ACTIVESYNC_TEXT_TRACE_RUN_SPLIT_CHURN_STEPS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1000);
    let run_split_insert_len = std::env::var("ACTIVESYNC_TEXT_TRACE_RUN_SPLIT_INSERT_LEN")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(4);
    let run_split_delete_span = std::env::var("ACTIVESYNC_TEXT_TRACE_RUN_SPLIT_DELETE_SPAN")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(3);
    let run_split_windows = std::env::var("ACTIVESYNC_TEXT_TRACE_RUN_SPLIT_WINDOWS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(16);
    let projection_mode = std::env::var("ACTIVESYNC_TEXT_TRACE_PROJECTION_MODE")
        .ok()
        .map(|v| parse_projection_mode(&v))
        .unwrap_or(TextProjectionMode::Disabled);
    let mode_label = projection_mode_label(projection_mode);

    // Sanity-check once so the benchmark only measures replay cost.
    let (sanity_graph, sanity_ops) =
        replay_trace_unsigned(&trace, max_ops, projection_mode, parity_sample_every);
    if max_ops.is_none() {
        let sanity_text = sanity_graph.resolve_text(TEXT_KEY);
        assert_eq!(
            sanity_text, trace.end_content,
            "trace replay result does not match endContent"
        );
    }
    let counters = sanity_graph.text_runtime_counters();
    let sanity_len = sanity_graph.resolve_text(TEXT_KEY).chars().count();
    eprintln!(
        "text_trace_rustcode_unsigned: applied_ops={sanity_ops}, max_ops={max_ops:?}, mode={mode_label}, parity_sample_every={parity_sample_every}, projection_hits={}, replay_fallbacks={}",
        counters.projection_hits,
        counters.replay_fallbacks,
    );

    let mut range_starts = Vec::new();
    let mut cursor = 0usize;
    while cursor < sanity_len && range_starts.len() < range_windows {
        range_starts.push(cursor);
        cursor = cursor.saturating_add(range_stride);
    }
    if range_starts.is_empty() {
        range_starts.push(0);
    }

    let run_split_graph = build_run_split_churn_graph(
        run_split_base_len,
        run_split_churn_steps,
        run_split_insert_len,
        run_split_delete_span,
        projection_mode,
        parity_sample_every,
    );
    let run_split_len = run_split_graph.resolve_text(TEXT_KEY).chars().count();
    let run_split_stride = std::cmp::max(1, run_split_len / run_split_windows.max(1));
    let mut run_split_starts = Vec::new();
    let mut run_split_cursor = 0usize;
    while run_split_cursor < run_split_len && run_split_starts.len() < run_split_windows {
        run_split_starts.push(run_split_cursor);
        run_split_cursor = run_split_cursor.saturating_add(run_split_stride);
    }
    if run_split_starts.is_empty() {
        run_split_starts.push(0);
    }

    // Full apply+read throughput, reported as ops/sec (elements = ops applied per iteration)
    // rather than raw wall-clock only, so it's directly comparable across trace sizes/runs.
    let mut throughput_group = c.benchmark_group("text_trace_rustcode_throughput");
    throughput_group.throughput(Throughput::Elements(sanity_ops as u64));

    let bench_name = format!("text_trace_rustcode_unsigned_{mode_label}");
    throughput_group.bench_function(&bench_name, |b| {
        b.iter(|| {
            let (graph, applied_ops) =
                replay_trace_unsigned(black_box(&trace), max_ops, projection_mode, parity_sample_every);
            black_box(applied_ops);
            black_box(graph.resolve_text(TEXT_KEY).len());
            black_box(graph.text_runtime_counters());
        });
    });

    // Split metrics: apply-only cost (no read). This is the cleanest char op
    // insert/delete throughput number (ops/sec), isolated from read cost.
    let apply_only_bench_name = format!("text_trace_rustcode_apply_only_unsigned_{mode_label}");
    throughput_group.bench_function(&apply_only_bench_name, |b| {
        b.iter(|| {
            let (_graph, applied_ops) =
                replay_trace_unsigned(black_box(&trace), max_ops, projection_mode, parity_sample_every);
            black_box(applied_ops);
        });
    });
    throughput_group.finish();

    let range_bench_name = format!("text_trace_rustcode_range_unsigned_{mode_label}");
    c.bench_function(&range_bench_name, |b| {
        b.iter(|| {
            let (graph, applied_ops) =
                replay_trace_unsigned(black_box(&trace), max_ops, projection_mode, parity_sample_every);
            black_box(applied_ops);
            for start in &range_starts {
                black_box(graph.resolve_text_range(TEXT_KEY, *start, range_len).len());
            }
            black_box(graph.text_runtime_counters());
        });
    });

    // Split metrics: read-only full resolve cost (graph pre-built once).
    let full_read_only_bench_name =
        format!("text_trace_rustcode_read_only_full_unsigned_{mode_label}");
    c.bench_function(&full_read_only_bench_name, |b| {
        b.iter(|| {
            black_box(sanity_graph.resolve_text(TEXT_KEY).len());
            black_box(sanity_graph.text_runtime_counters());
        });
    });

    // Split metrics: read-only range resolve cost (graph pre-built once).
    let range_read_only_bench_name =
        format!("text_trace_rustcode_read_only_range_unsigned_{mode_label}");
    c.bench_function(&range_read_only_bench_name, |b| {
        b.iter(|| {
            for start in &range_starts {
                black_box(sanity_graph.resolve_text_range(TEXT_KEY, *start, range_len).len());
            }
            black_box(sanity_graph.text_runtime_counters());
        });
    });

    // Phase 3 scaffold metric: cursor mapping baseline for offset<->anchor.
    let cursor_mapping_bench_name =
        format!("text_trace_rustcode_cursor_mapping_unsigned_{mode_label}");
    c.bench_function(&cursor_mapping_bench_name, |b| {
        b.iter(|| {
            for start in &range_starts {
                let anchor = sanity_graph.resolve_text_anchor_for_offset(TEXT_KEY, *start);
                black_box(sanity_graph.resolve_text_offset_for_anchor(TEXT_KEY, anchor));
            }
        });
    });

    // Run-split-heavy read profile: focuses on range reads over churned mid-run edits.
    let run_split_read_only_bench_name =
        format!("text_trace_rustcode_read_only_range_run_split_unsigned_{mode_label}");
    c.bench_function(&run_split_read_only_bench_name, |b| {
        b.iter(|| {
            for start in &run_split_starts {
                black_box(run_split_graph.resolve_text_range(TEXT_KEY, *start, range_len).len());
            }
            black_box(run_split_graph.text_runtime_counters());
        });
    });

    // Run-split-heavy cursor profile: offset<->anchor mapping after repeated split/merge churn.
    let run_split_cursor_mapping_bench_name =
        format!("text_trace_rustcode_cursor_mapping_run_split_unsigned_{mode_label}");
    c.bench_function(&run_split_cursor_mapping_bench_name, |b| {
        b.iter(|| {
            for start in &run_split_starts {
                let anchor = run_split_graph.resolve_text_anchor_for_offset(TEXT_KEY, *start);
                black_box(run_split_graph.resolve_text_offset_for_anchor(TEXT_KEY, anchor));
            }
        });
    });
}

fn bench_cursor_mapping_size_sweep(c: &mut Criterion) {
    let sizes = [1000usize, 2000, 4000, 8000, 16000];
    let mut group = c.benchmark_group("text_cursor_mapping_size_sweep");

    // Keep this benchmark practical in CI while still showing scaling shape.
    group.sample_size(10);
    group.measurement_time(std::time::Duration::from_secs(1));
    group.warm_up_time(std::time::Duration::from_secs(1));

    for size in sizes {
        let graph = build_linear_text_graph(size, TextProjectionMode::Enabled);
        let probes: Vec<usize> = (0usize..64)
            .map(|i| i.saturating_mul(size / 64).min(size))
            .collect();
        group.throughput(Throughput::Elements(probes.len() as u64));
        group.bench_with_input(BenchmarkId::new("enabled", size), &size, |b, _| {
            b.iter(|| {
                for p in &probes {
                    let anchor = graph.resolve_text_anchor_for_offset(TEXT_KEY, *p);
                    black_box(graph.resolve_text_offset_for_anchor(TEXT_KEY, anchor));
                }
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_text_trace_rustcode, bench_cursor_mapping_size_sweep);
criterion_main!(benches);
