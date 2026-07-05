#![cfg(feature = "text_projection")]

// Absolute-number companion to the percentage-heavy projection/replay benchmarks:
// char op throughput (ops/sec), wire cost (bytes/op), and cold-start convergence
// time (a fresh peer catching up on an existing large room), modeled loosely on
// https://github.com/dmonad/crdt-benchmarks' real-world-trace replay measurements.
// This is a reporting spike, not a regression gate — no threshold assertions on
// the timing/size numbers themselves, only on correctness (convergence).

use nodalmerge_core::{Hash, Op, OpId, StateGraph, SyncNode, TextOp, TextProjectionMode, Transaction};
use serde::Deserialize;
use std::{
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
    time::Instant,
};

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
    let file = File::open(&path).unwrap_or_else(|e| panic!("failed to open {}: {e}", path.display()));
    serde_json::from_reader(BufReader::new(file))
        .unwrap_or_else(|e| panic!("failed to parse {}: {e}", path.display()))
}

fn build_unsigned_text_node(lamport: &mut u64, parents: &mut Vec<Hash>, op: TextOp) -> (SyncNode, OpId) {
    *lamport += 1;
    let tx = Transaction {
        author: BENCH_AUTHOR,
        lamport: *lamport,
        wall_ms: *lamport,
        ops: vec![Op::Text(op)],
        parents: parents.clone(),
    };
    let node = SyncNode::new(tx);
    *parents = vec![node.id];
    (
        node,
        OpId {
            lamport: *lamport,
            author: BENCH_AUTHOR,
        },
    )
}

fn apply_unsigned_text_op(
    graph: &mut StateGraph,
    lamport: &mut u64,
    parents: &mut Vec<Hash>,
    op: TextOp,
) -> OpId {
    let (node, op_id) = build_unsigned_text_node(lamport, parents, op);
    graph
        .apply_remote(node)
        .unwrap_or_else(|e| panic!("apply_remote failed at lamport {}: {}", op_id.lamport, e));
    op_id
}

fn replay_trace_unsigned(trace: &EditingTrace, max_ops: Option<usize>) -> (StateGraph, usize) {
    let mut graph = StateGraph::new();
    graph.set_text_projection_mode(TextProjectionMode::Enabled);
    let mut lamport = 0_u64;
    let mut parents: Vec<Hash> = Vec::new();
    let mut visible: Vec<OpId> = Vec::with_capacity(trace.end_content.chars().count());
    let mut applied_ops = 0_usize;
    let progress_started = Instant::now();
    let mut last_progress = Instant::now();

    macro_rules! progress_or_stop {
        () => {
            if let Some(limit) = max_ops {
                if applied_ops >= limit {
                    return (graph, applied_ops);
                }
            }
            if last_progress.elapsed().as_secs() >= 2 {
                eprintln!(
                    "  progress: applied_ops={applied_ops}, elapsed={:.1}s",
                    progress_started.elapsed().as_secs_f64()
                );
                last_progress = Instant::now();
            }
        };
    }

    for ch in trace.start_content.chars() {
        let after = visible.last().copied();
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
        applied_ops += 1;
        progress_or_stop!();
    }

    for txn in &trace.txns {
        for (pos, del_count, insert_text) in &txn.patches {
            let mut cur_pos = *pos;
            for _ in 0..*del_count {
                let target = visible[cur_pos];
                apply_unsigned_text_op(
                    &mut graph,
                    &mut lamport,
                    &mut parents,
                    TextOp::Delete {
                        key: TEXT_KEY.to_string(),
                        target,
                    },
                );
                visible.remove(cur_pos);
                applied_ops += 1;
                progress_or_stop!();
            }

            for ch in insert_text.chars() {
                let after = if cur_pos == 0 {
                    None
                } else {
                    Some(visible[cur_pos - 1])
                };
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
                visible.insert(
                    cur_pos,
                    OpId {
                        lamport,
                        author: BENCH_AUTHOR,
                    },
                );
                cur_pos += 1;
                applied_ops += 1;
                progress_or_stop!();
            }
        }
    }

    (graph, applied_ops)
}

fn flush_batch(graph: &mut StateGraph, batch: &mut Vec<SyncNode>) {
    if batch.is_empty() {
        return;
    }
    let nodes = std::mem::take(batch);
    let result = graph.apply_remote_batch(nodes);
    assert!(
        result.rejected.is_empty(),
        "unexpected rejection during batched replay: {:?}",
        result.rejected
    );
}

// Same replay as `replay_trace_unsigned`, but batches each trace "txn" (the
// editing-transaction grouping already present in the trace data — one paste,
// one keystroke burst, etc.) into a single `apply_remote_batch` call instead of
// one `apply_remote` call per character. This is the realistic client-SDK path:
// a real editor submits one bulk-paste event as one transaction, not thousands
// of individual round-trips. See benchmarks/benchmarks.md for why the unbatched
// path is pathological specifically for bulk-paste events in this trace.
fn replay_trace_unsigned_batched(trace: &EditingTrace, max_ops: Option<usize>) -> (StateGraph, usize) {
    let mut graph = StateGraph::new();
    graph.set_text_projection_mode(TextProjectionMode::Enabled);
    let mut lamport = 0_u64;
    let mut parents: Vec<Hash> = Vec::new();
    let mut visible: Vec<OpId> = Vec::with_capacity(trace.end_content.chars().count());
    let mut applied_ops = 0_usize;
    let progress_started = Instant::now();
    let mut last_progress = Instant::now();
    let mut batch: Vec<SyncNode> = Vec::new();
    let mut limit_hit = false;

    macro_rules! stopped {
        () => {{
            if let Some(limit) = max_ops {
                applied_ops >= limit
            } else {
                false
            }
        }};
    }

    'seed: for ch in trace.start_content.chars() {
        if stopped!() {
            limit_hit = true;
            break 'seed;
        }
        let after = visible.last().copied();
        let (node, op_id) = build_unsigned_text_node(
            &mut lamport,
            &mut parents,
            TextOp::Insert {
                key: TEXT_KEY.to_string(),
                after,
                ch,
            },
        );
        batch.push(node);
        visible.push(op_id);
        applied_ops += 1;
    }
    flush_batch(&mut graph, &mut batch);

    'txns: for txn in &trace.txns {
        if limit_hit {
            break 'txns;
        }
        for (pos, del_count, insert_text) in &txn.patches {
            let mut cur_pos = *pos;
            for _ in 0..*del_count {
                if stopped!() {
                    limit_hit = true;
                    break;
                }
                let target = visible[cur_pos];
                let (node, _) = build_unsigned_text_node(
                    &mut lamport,
                    &mut parents,
                    TextOp::Delete {
                        key: TEXT_KEY.to_string(),
                        target,
                    },
                );
                batch.push(node);
                visible.remove(cur_pos);
                applied_ops += 1;
            }
            if limit_hit {
                break;
            }

            for ch in insert_text.chars() {
                if stopped!() {
                    limit_hit = true;
                    break;
                }
                let after = if cur_pos == 0 {
                    None
                } else {
                    Some(visible[cur_pos - 1])
                };
                let (node, op_id) = build_unsigned_text_node(
                    &mut lamport,
                    &mut parents,
                    TextOp::Insert {
                        key: TEXT_KEY.to_string(),
                        after,
                        ch,
                    },
                );
                batch.push(node);
                visible.insert(cur_pos, op_id);
                cur_pos += 1;
                applied_ops += 1;
            }
            if limit_hit {
                break;
            }
        }

        flush_batch(&mut graph, &mut batch);

        if last_progress.elapsed().as_secs() >= 2 {
            eprintln!(
                "  progress (batched): applied_ops={applied_ops}, elapsed={:.1}s",
                progress_started.elapsed().as_secs_f64()
            );
            last_progress = Instant::now();
        }
    }
    flush_batch(&mut graph, &mut batch);

    (graph, applied_ops)
}

// Default op cap: the ~980k-op full trace scales badly superlinearly in
// `TextProjectionMode::Enabled` (still O(n^1.5-2)-ish even in the batched path
// — see benchmarks/benchmarks.md), so it's impractical for a normal `cargo
// test` run. Bounded to 50k by default for a fast smoke test.
//
// Caveat: the first ~42k ops of this trace are one single large sequential
// append (a paste into an empty doc), which the engine's per-op
// `can_fast_append` path already handles efficiently — so the 50k default
// mostly lands *inside* the case batching doesn't help. Batching only shows
// its ~1.6x win once scattered/non-append edits dominate, around 150k+ ops.
// Use ACTIVESYNC_TEXT_THROUGHPUT_MAX_OPS=150000 (or higher / "full") to see
// that regime; expect several minutes at that size.
const DEFAULT_MAX_OPS: usize = 50_000;

fn resolve_max_ops() -> Option<usize> {
    match std::env::var("ACTIVESYNC_TEXT_THROUGHPUT_MAX_OPS") {
        Ok(v) if v.eq_ignore_ascii_case("full") || v == "0" => None,
        Ok(v) => v.parse::<usize>().ok().or(Some(DEFAULT_MAX_OPS)),
        Err(_) => Some(DEFAULT_MAX_OPS),
    }
}

#[test]
fn text_trace_throughput_wire_cost_and_cold_start_convergence() {
    let trace = load_trace();
    let max_ops = resolve_max_ops();

    // --- Unbatched replay: one apply_remote call per character (worst case / baseline) ---
    let unbatched_started = Instant::now();
    let (peer_a_unbatched, unbatched_applied_ops) = replay_trace_unsigned(&trace, max_ops);
    let unbatched_elapsed = unbatched_started.elapsed();

    // --- Batched replay: one apply_remote_batch call per trace txn (realistic client-SDK path) ---
    let batched_started = Instant::now();
    let (peer_a, applied_ops) = replay_trace_unsigned_batched(&trace, max_ops);
    let batched_elapsed = batched_started.elapsed();

    assert_eq!(
        unbatched_applied_ops, applied_ops,
        "batched and unbatched replay should apply the same op count"
    );
    assert_eq!(
        peer_a_unbatched.resolve_text(TEXT_KEY),
        peer_a.resolve_text(TEXT_KEY),
        "batched and unbatched replay should converge to the same resolved text"
    );

    if max_ops.is_none() {
        assert_eq!(
            peer_a.resolve_text(TEXT_KEY),
            trace.end_content,
            "trace replay result does not match endContent"
        );
    }

    let unbatched_wall_ms = unbatched_elapsed.as_secs_f64() * 1000.0;
    let batched_wall_ms = batched_elapsed.as_secs_f64() * 1000.0;
    let unbatched_ops_per_sec = applied_ops as f64 / unbatched_elapsed.as_secs_f64();
    let batched_ops_per_sec = applied_ops as f64 / batched_elapsed.as_secs_f64();
    let batched_speedup_x = if unbatched_wall_ms > 0.0 {
        unbatched_wall_ms / batched_wall_ms
    } else {
        0.0
    };

    // Wire cost, cold-start convergence, and the summary/JSON below all use the
    // batched replay's graph — same node set/content either way (batching only
    // changes *how* nodes are applied, not what they contain), and batched is
    // the realistic path going forward.

    // --- Wire cost: per-node postcard encoding size across the whole applied set ---
    let all_ids = peer_a.all_node_ids();
    let all_nodes: Vec<SyncNode> = peer_a.get_nodes(&all_ids).into_iter().cloned().collect();
    assert_eq!(all_nodes.len(), applied_ops, "node count should match applied op count");

    let mut total_wire_bytes: u64 = 0;
    let mut min_bytes_per_op: u64 = u64::MAX;
    let mut max_bytes_per_op: u64 = 0;
    for node in &all_nodes {
        let len = node.to_postcard().len() as u64;
        total_wire_bytes += len;
        min_bytes_per_op = min_bytes_per_op.min(len);
        max_bytes_per_op = max_bytes_per_op.max(len);
    }
    let mean_bytes_per_op = total_wire_bytes as f64 / all_nodes.len() as f64;

    // --- Cold-start convergence: fresh peer B bulk-catches-up on peer A's full history ---
    let mut sorted_nodes = all_nodes.clone();
    sorted_nodes.sort_by_key(|n| (n.transaction.lamport, n.id));

    let mut peer_b = StateGraph::new();
    peer_b.set_text_projection_mode(TextProjectionMode::Enabled);
    let convergence_started = Instant::now();
    let batch_result = peer_b.apply_remote_batch(sorted_nodes);
    let convergence_elapsed = convergence_started.elapsed();

    assert!(
        batch_result.rejected.is_empty(),
        "unexpected rejection while cold-start syncing peer B: {:?}",
        batch_result.rejected
    );
    let converged = peer_b.resolve_text(TEXT_KEY) == peer_a.resolve_text(TEXT_KEY);
    assert!(converged, "peer B did not converge to peer A's resolved text");

    let batch_apply_ms = convergence_elapsed.as_secs_f64() * 1000.0;

    eprintln!(
        "text_throughput_and_convergence: trace_op_count={applied_ops}, unbatched_ops_per_sec={unbatched_ops_per_sec:.1}, batched_ops_per_sec={batched_ops_per_sec:.1}, batched_speedup_x={batched_speedup_x:.2}, unbatched_wall_ms={unbatched_wall_ms:.3}, batched_wall_ms={batched_wall_ms:.3}, mean_bytes_per_op={mean_bytes_per_op:.2}, min_bytes_per_op={min_bytes_per_op}, max_bytes_per_op={max_bytes_per_op}, total_wire_bytes={total_wire_bytes}, batch_apply_ms={batch_apply_ms:.3}, converged={converged}"
    );

    if let Ok(path) = std::env::var("ACTIVESYNC_TEXT_THROUGHPUT_METRICS_PATH") {
        let payload = serde_json::json!({
            "test": "text_trace_throughput_wire_cost_and_cold_start_convergence",
            "trace_op_count": applied_ops,
            "throughput": {
                "unbatched_ops_per_sec": unbatched_ops_per_sec,
                "unbatched_wall_ms": unbatched_wall_ms,
                "batched_ops_per_sec": batched_ops_per_sec,
                "batched_wall_ms": batched_wall_ms,
                "batched_speedup_x": batched_speedup_x
            },
            "wire_cost": {
                "mean_bytes_per_op": mean_bytes_per_op,
                "min_bytes_per_op": min_bytes_per_op,
                "max_bytes_per_op": max_bytes_per_op,
                "total_wire_bytes": total_wire_bytes
            },
            "convergence_cold_start": {
                "node_count": all_nodes.len(),
                "batch_apply_ms": batch_apply_ms,
                "converged": converged
            }
        });
        let json = serde_json::to_string_pretty(&payload).unwrap();
        let _ = std::fs::write(path, json);
    }
}
