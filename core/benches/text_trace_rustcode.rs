use std::{
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
};

use activesync_core::{Hash, Op, OpId, StateGraph, SyncNode, TextOp, Transaction};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
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
) {
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

fn replay_trace_unsigned(trace: &EditingTrace, max_ops: Option<usize>) -> (StateGraph, usize) {
    let mut graph = StateGraph::new();
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

fn bench_text_trace_rustcode(c: &mut Criterion) {
    let trace = load_trace();
    let max_ops = std::env::var("ACTIVESYNC_TEXT_TRACE_MAX_OPS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok());

    // Sanity-check once so the benchmark only measures replay cost.
    let (sanity_graph, sanity_ops) = replay_trace_unsigned(&trace, max_ops);
    if max_ops.is_none() {
        let sanity_text = sanity_graph.resolve_text(TEXT_KEY);
        assert_eq!(
            sanity_text, trace.end_content,
            "trace replay result does not match endContent"
        );
    }
    eprintln!("text_trace_rustcode_unsigned: applied_ops={sanity_ops}, max_ops={max_ops:?}");

    c.bench_function("text_trace_rustcode_unsigned", |b| {
        b.iter(|| {
            let (graph, applied_ops) = replay_trace_unsigned(black_box(&trace), max_ops);
            black_box(applied_ops);
            black_box(graph.resolve_text(TEXT_KEY).len());
        });
    });
}

criterion_group!(benches, bench_text_trace_rustcode);
criterion_main!(benches);
