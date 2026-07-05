#![cfg(feature = "text_projection")]

// [B4] Real-world editing dataset — dmonad/crdt-benchmarks parity harness.
//
// Replays the exact same trace crdt-benchmarks' B4 uses (the automerge-perf
// edit-by-index trace: the LaTeX source of https://arxiv.org/abs/1608.03960,
// 182,315 single-char insertions + 77,463 single-char deletions = 259,778
// ops, 104,852-char final document), applied the same way their drivers do:
// one transaction per edit, position-based (`insertText(pos, str)` /
// `deleteText(pos, len)` ⇒ our `InsertRange`/`DeleteRange` with `Offset`
// anchors), single client, then extract the content and compare to the
// trace's `finalText`.
//
// Differences vs the JS harness that keep the comparison honest rather than
// flattering: every edit here is a full hash-linked DAG node (blake3 id,
// parent pointers, postcard encoding) — integrity costs the JS CRDTs don't
// pay — and nodes are unsigned, matching how the JS libraries don't sign
// either.
//
// Trace file: benchmarks/data/b4-editing-trace.json, extracted 1:1 from
// crdt-benchmarks/js-lib/b4-editing-trace.js (`{ finalText, edits }`).
//
// Reported metrics (analogs of the B4 rows):
// - apply_ms / ops_per_sec: replay all edits + final content check ("time")
// - total_update_bytes / avg_update_bytes: per-node postcard sizes
//   ("updateSize" / "avgUpdateSize")
// - cold_start_ms: fresh peer bulk-applies the full history via
//   `apply_remote_batch` + content check (closest analog of "parseTime";
//   ours replays ops, theirs decodes a state snapshot — labeled as such)

use nodalmerge_core::{Hash, Op, StateGraph, SyncNode, TextOp, TextProjectionMode, Transaction};
use nodalmerge_core::TextRangeAnchor;
use std::{
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
    time::Instant,
};

const TEXT_KEY: &str = "doc";
const BENCH_AUTHOR: [u8; 32] = [0xB4; 32];

fn trace_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("benchmarks")
        .join("data")
        .join("b4-editing-trace.json")
}

struct B4Trace {
    final_text: String,
    /// `(pos, delete_count, insert_text)` — insert may be empty.
    edits: Vec<(usize, usize, String)>,
}

fn load_trace() -> B4Trace {
    let path = trace_path();
    let file = File::open(&path)
        .unwrap_or_else(|e| panic!("failed to open {}: {e}", path.display()));
    let raw: serde_json::Value = serde_json::from_reader(BufReader::new(file))
        .unwrap_or_else(|e| panic!("failed to parse {}: {e}", path.display()));
    let final_text = raw["finalText"].as_str().expect("finalText").to_string();
    let edits = raw["edits"]
        .as_array()
        .expect("edits array")
        .iter()
        .map(|row| {
            let row = row.as_array().expect("edit row");
            let pos = row[0].as_u64().expect("pos") as usize;
            let del = row.get(1).and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let ins = row
                .get(2)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            (pos, del, ins)
        })
        .collect();
    B4Trace { final_text, edits }
}

/// Build one unsigned DAG node per edit, mirroring `apply_local`'s Lamport
/// reservation for range inserts (each inserted char consumes one Lamport id).
fn build_edit_node(
    lamport: &mut u64,
    parents: &mut Vec<Hash>,
    ops: Vec<Op>,
    inserted_chars: u64,
) -> SyncNode {
    *lamport += 1;
    let tx = Transaction {
        author: BENCH_AUTHOR,
        lamport: *lamport,
        wall_ms: *lamport,
        ops,
        parents: parents.clone(),
    };
    *lamport = lamport.saturating_add(inserted_chars.saturating_sub(1));
    let node = SyncNode::new(tx);
    *parents = vec![node.id];
    node
}

fn edit_ops(pos: usize, del: usize, ins: &str) -> (Vec<Op>, u64) {
    let mut ops = Vec::with_capacity(2);
    if del > 0 {
        ops.push(Op::Text(TextOp::DeleteRange {
            key: TEXT_KEY.to_string(),
            anchor: TextRangeAnchor::Offset(pos),
            len_chars: del,
        }));
    }
    let inserted = ins.chars().count() as u64;
    if inserted > 0 {
        ops.push(Op::Text(TextOp::InsertRange {
            key: TEXT_KEY.to_string(),
            anchor: TextRangeAnchor::Offset(pos),
            text: ins.to_string(),
        }));
    }
    (ops, inserted)
}

#[test]
fn b4_real_world_editing_dataset() {
    let trace = load_trace();
    let edit_count = trace.edits.len();

    // --- Apply: one transaction (one DAG node) per edit, like the B4 drivers ---
    let mut graph = StateGraph::new();
    graph.set_text_projection_mode(TextProjectionMode::Enabled);
    let mut lamport = 0_u64;
    let mut parents: Vec<Hash> = Vec::new();

    let apply_started = Instant::now();
    for (pos, del, ins) in &trace.edits {
        let (ops, inserted) = edit_ops(*pos, *del, ins);
        if ops.is_empty() {
            continue;
        }
        let node = build_edit_node(&mut lamport, &mut parents, ops, inserted);
        graph
            .apply_remote(node)
            .unwrap_or_else(|e| panic!("apply_remote failed at lamport {lamport}: {e}"));
    }
    let content = graph.resolve_text(TEXT_KEY);
    let apply_elapsed = apply_started.elapsed();

    assert_eq!(
        content, trace.final_text,
        "replayed document does not match the trace's finalText"
    );

    // --- Update size: per-node wire encoding across the full history ---
    let all_ids = graph.all_node_ids();
    let all_nodes: Vec<SyncNode> = graph.get_nodes(&all_ids).into_iter().cloned().collect();
    let total_update_bytes: u64 = all_nodes.iter().map(|n| n.to_postcard().len() as u64).sum();

    // --- Cold start: fresh peer bulk-applies the full history ---
    let mut sorted_nodes = all_nodes;
    sorted_nodes.sort_by_key(|n| (n.transaction.lamport, n.id));
    let mut peer_b = StateGraph::new();
    peer_b.set_text_projection_mode(TextProjectionMode::Enabled);
    let cold_started = Instant::now();
    let result = peer_b.apply_remote_batch(sorted_nodes);
    let cold_content = peer_b.resolve_text(TEXT_KEY);
    let cold_elapsed = cold_started.elapsed();
    assert!(result.rejected.is_empty(), "cold-start rejections: {:?}", result.rejected);
    assert_eq!(cold_content, trace.final_text, "cold-start peer did not converge");

    let apply_ms = apply_elapsed.as_secs_f64() * 1000.0;
    let cold_ms = cold_elapsed.as_secs_f64() * 1000.0;
    let ops_per_sec = edit_count as f64 / apply_elapsed.as_secs_f64();
    let avg_update_bytes = total_update_bytes as f64 / edit_count as f64;

    eprintln!(
        "b4_real_world_editing_dataset: edits={edit_count}, apply_ms={apply_ms:.1}, ops_per_sec={ops_per_sec:.1}, total_update_bytes={total_update_bytes}, avg_update_bytes={avg_update_bytes:.1}, cold_start_ms={cold_ms:.1}, final_chars={}",
        trace.final_text.chars().count()
    );

    if let Ok(path) = std::env::var("ACTIVESYNC_B4_METRICS_PATH") {
        let payload = serde_json::json!({
            "test": "b4_real_world_editing_dataset",
            "edit_count": edit_count,
            "apply_ms": apply_ms,
            "ops_per_sec": ops_per_sec,
            "total_update_bytes": total_update_bytes,
            "avg_update_bytes": avg_update_bytes,
            "cold_start_ms": cold_ms,
            "converged": true,
        });
        let _ = std::fs::write(path, serde_json::to_string_pretty(&payload).unwrap());
    }
}
