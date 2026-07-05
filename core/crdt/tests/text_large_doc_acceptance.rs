#![cfg(feature = "text_projection")]

use nodalmerge_core::{Op, OpId, StateGraph, TextOp, TextProjectionMode};
use ed25519_dalek::SigningKey;
use std::collections::HashMap;
use std::mem::size_of;
use std::time::Instant;

const DEFAULT_MAX_FULL_REBUILDS: u64 = 16;
const DEFAULT_MAX_CAPACITY_RATIO: f64 = 5.0;
const DEFAULT_MAX_DURATION_MS: u64 = 20_000;

fn key_a() -> SigningKey {
    SigningKey::from_bytes(&[0x0Au8; 32])
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default)
}

fn env_f64(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(default)
}

fn estimate_projection_memory(
    stats: &nodalmerge_core::TextProjectionDebugStats,
) -> (usize, usize, usize, f64, f64) {
    // Phase 5 densification stores visible ids separately from UTF-8 payload,
    // not as duplicated `(OpId, char)` tuples.
    let compact_visible_id_bytes = size_of::<u32>() + size_of::<u64>();
    let visible_cache_bytes = stats.visible_string_capacity
        + stats
            .visible_len
            .saturating_mul(compact_visible_id_bytes);
    let index_bytes = stats
        .index_weights_len
        .saturating_mul(size_of::<u32>())
        .saturating_add(stats.index_fenwick_len.saturating_mul(size_of::<u32>()));
    let compact_metadata_id_bytes = size_of::<u32>() + size_of::<u64>();
    let metadata_entry_bytes = stats
        .metadata_entries
        .saturating_mul(compact_metadata_id_bytes + size_of::<char>());
    let tombstone_span_bytes = stats
        .tombstone_span_count
        .saturating_mul(size_of::<u64>() + size_of::<u64>())
        .saturating_add(
            stats
                .tombstone_author_bucket_count
                .saturating_mul(size_of::<u32>()),
        );
    let metadata_bytes_lower_bound = metadata_entry_bytes.saturating_add(tombstone_span_bytes);

    let projection_bytes_lower_bound = visible_cache_bytes
        .saturating_add(index_bytes)
        .saturating_add(metadata_bytes_lower_bound);
    let bytes_per_visible = if stats.visible_len == 0 {
        0.0
    } else {
        projection_bytes_lower_bound as f64 / stats.visible_len as f64
    };
    let bytes_per_tombstone = if stats.tombstone_count == 0 {
        0.0
    } else {
        metadata_bytes_lower_bound as f64 / stats.tombstone_count as f64
    };

    (
        projection_bytes_lower_bound,
        index_bytes,
        metadata_bytes_lower_bound,
        bytes_per_visible,
        bytes_per_tombstone,
    )
}

#[test]
fn large_doc_windowed_reads_and_allocation_behavior() {
    let sk = key_a();
    let mut g = StateGraph::new();
    g.set_text_projection_mode(TextProjectionMode::Enabled);
    let started = Instant::now();

    let key = "doc";
    let mut visible_ids: Vec<OpId> = Vec::new();

    // Seed a large doc by appending deterministic content.
    for i in 0..5000u64 {
        let after = visible_ids.last().copied();
        let ch = (b'a' + (i % 26) as u8) as char;
        g.apply_local(
            &sk,
            1000 + i,
            vec![Op::Text(TextOp::Insert {
                key: key.into(),
                after,
                ch,
            })],
        )
        .unwrap();
        visible_ids.push(OpId {
            lamport: g.lamport(),
            author: sk.verifying_key().to_bytes(),
        });
    }

    // Deterministic mixed small edits + viewport reads.
    let mut seed: u64 = 0x1234_5678_9ABC_DEF0;
    let mut next_u64 = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        seed
    };

    for step in 0..700u64 {
        let do_insert = visible_ids.is_empty() || (next_u64() % 100) < 65;
        if do_insert {
            let pos = if visible_ids.is_empty() {
                0usize
            } else {
                (next_u64() as usize) % (visible_ids.len() + 1)
            };
            let after = if pos == 0 {
                None
            } else {
                Some(visible_ids[pos - 1])
            };
            let ch = (b'a' + (next_u64() % 26) as u8) as char;
            g.apply_local(
                &sk,
                10_000 + step,
                vec![Op::Text(TextOp::Insert {
                    key: key.into(),
                    after,
                    ch,
                })],
            )
            .unwrap();
            visible_ids.insert(
                pos,
                OpId {
                    lamport: g.lamport(),
                    author: sk.verifying_key().to_bytes(),
                },
            );
        } else {
            let pos = (next_u64() as usize) % visible_ids.len();
            let target = visible_ids.remove(pos);
            g.apply_local(
                &sk,
                10_000 + step,
                vec![Op::Text(TextOp::Delete {
                    key: key.into(),
                    target,
                })],
            )
            .unwrap();
        }

        let full = g.resolve_text(key);
        let full_chars: Vec<char> = full.chars().collect();
        let len = full_chars.len();
        let start = if len == 0 {
            0
        } else {
            (next_u64() as usize) % len
        };
        let win = ((next_u64() as usize) % 256).max(1);
        let expected: String = full_chars.iter().skip(start).take(win).copied().collect();
        let actual = g.resolve_text_range(key, start, win);
        assert_eq!(actual, expected, "range read mismatch at step {step}");
    }

    let stats = g
        .text_projection_debug_stats(key)
        .expect("projection stats should exist for hot key");
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let ratio = if stats.visible_len == 0 {
        0.0
    } else {
        stats.visible_string_capacity as f64 / stats.visible_len as f64
    };
    let (
        projection_bytes_lower_bound,
        index_bytes_lower_bound,
        metadata_bytes_lower_bound,
        bytes_per_visible_char,
        bytes_per_tombstone,
    ) = estimate_projection_memory(&stats);

    let max_full_rebuilds = env_u64(
        "NODALMERGE_TEXT_LARGE_DOC_MAX_FULL_REBUILDS",
        DEFAULT_MAX_FULL_REBUILDS,
    );
    let max_capacity_ratio = env_f64(
        "NODALMERGE_TEXT_LARGE_DOC_MAX_CAPACITY_RATIO",
        DEFAULT_MAX_CAPACITY_RATIO,
    );
    let max_duration_ms = env_u64(
        "NODALMERGE_TEXT_LARGE_DOC_MAX_DURATION_MS",
        DEFAULT_MAX_DURATION_MS,
    );

    // Acceptance checks for Phase 2.5 harness:
    // - repeated viewport reads should not force many full rematerializations
    // - UTF-8 backing capacity should stay bounded relative to visible chars
    // - end-to-end deterministic workload should complete under target wall time
    assert!(
        stats.full_rebuild_count <= max_full_rebuilds,
        "full rebuild count too high: actual={}, max={}",
        stats.full_rebuild_count,
        max_full_rebuilds
    );
    assert!(
        ratio <= max_capacity_ratio,
        "capacity ratio too high: actual={ratio:.3}, max={max_capacity_ratio:.3}, cap={}, len={}",
        stats.visible_string_capacity,
        stats.visible_len,
    );
    assert!(
        elapsed_ms <= max_duration_ms,
        "large-doc workload duration too high: actual={}ms, max={}ms",
        elapsed_ms,
        max_duration_ms
    );

    if let Ok(path) = std::env::var("NODALMERGE_TEXT_LARGE_DOC_METRICS_PATH") {
        let payload = serde_json::json!({
            "test": "large_doc_windowed_reads_and_allocation_behavior",
            "duration_ms": elapsed_ms,
            "visible_len": stats.visible_len,
            "metadata_entries": stats.metadata_entries,
            "tombstone_count": stats.tombstone_count,
            "tombstone_span_count": stats.tombstone_span_count,
            "tombstone_author_bucket_count": stats.tombstone_author_bucket_count,
            "visible_string_capacity": stats.visible_string_capacity,
            "capacity_to_visible_ratio": ratio,
            "projection_bytes_lower_bound": projection_bytes_lower_bound,
            "metadata_bytes_lower_bound": metadata_bytes_lower_bound,
            "index_bytes_lower_bound": index_bytes_lower_bound,
            "bytes_per_visible_char_lower_bound": bytes_per_visible_char,
            "bytes_per_tombstone_lower_bound": bytes_per_tombstone,
            "index_weights_len": stats.index_weights_len,
            "index_fenwick_len": stats.index_fenwick_len,
            "full_rebuild_count": stats.full_rebuild_count,
            "thresholds": {
                "max_full_rebuilds": max_full_rebuilds,
                "max_capacity_ratio": max_capacity_ratio,
                "max_duration_ms": max_duration_ms
            }
        });
        let json = serde_json::to_string_pretty(&payload).unwrap();
        let _ = std::fs::write(path, json);
    }
}

#[test]
fn long_lived_projection_memory_telemetry_profile() {
    let sk = key_a();
    let mut g = StateGraph::new();
    g.set_text_projection_mode(TextProjectionMode::Enabled);

    let keys = ["doc", "title", "notes"];
    let mut visible_ids: HashMap<&str, Vec<OpId>> = HashMap::new();
    for key in &keys {
        visible_ids.insert(*key, Vec::new());
    }

    let mut wall_ms = 1000u64;

    // Seed keys with deterministic content.
    for key in &keys {
        let ids = visible_ids.get_mut(key).unwrap();
        for i in 0..1200u64 {
            let after = ids.last().copied();
            let ch = (b'a' + (i % 26) as u8) as char;
            g.apply_local(
                &sk,
                wall_ms,
                vec![Op::Text(TextOp::Insert {
                    key: (*key).into(),
                    after,
                    ch,
                })],
            )
            .unwrap();
            ids.push(OpId {
                lamport: g.lamport(),
                author: sk.verifying_key().to_bytes(),
            });
            wall_ms = wall_ms.saturating_add(1);
        }
    }

    // Deterministic long-lived mixed edits across multiple keys.
    let mut seed: u64 = 0xD15EA5E_CAFE_BABE;
    let mut next_u64 = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        seed
    };

    for _step in 0..4000u64 {
        let key_idx = (next_u64() as usize) % keys.len();
        let key = keys[key_idx];
        let ids = visible_ids.get_mut(key).unwrap();
        let do_insert = ids.is_empty() || (next_u64() % 100) < 60;

        if do_insert {
            let pos = if ids.is_empty() {
                0usize
            } else {
                (next_u64() as usize) % (ids.len() + 1)
            };
            let after = if pos == 0 { None } else { Some(ids[pos - 1]) };
            let ch = (b'a' + (next_u64() % 26) as u8) as char;
            g.apply_local(
                &sk,
                wall_ms,
                vec![Op::Text(TextOp::Insert {
                    key: key.into(),
                    after,
                    ch,
                })],
            )
            .unwrap();
            ids.insert(
                pos,
                OpId {
                    lamport: g.lamport(),
                    author: sk.verifying_key().to_bytes(),
                },
            );
        } else {
            let pos = (next_u64() as usize) % ids.len();
            let target = ids.remove(pos);
            g.apply_local(
                &sk,
                wall_ms,
                vec![Op::Text(TextOp::Delete {
                    key: key.into(),
                    target,
                })],
            )
            .unwrap();
        }

        wall_ms = wall_ms.saturating_add(1);
    }

    let mut per_key = serde_json::Map::new();
    let mut total_projection_bytes = 0usize;
    let mut total_visible_chars = 0usize;
    let mut total_tombstones = 0usize;
    let mut total_index_bytes = 0usize;

    for key in &keys {
        let stats = g
            .text_projection_debug_stats(key)
            .unwrap_or_else(|| panic!("missing projection stats for key {key}"));
        let (
            projection_bytes_lower_bound,
            index_bytes_lower_bound,
            metadata_bytes_lower_bound,
            bytes_per_visible_char,
            bytes_per_tombstone,
        ) = estimate_projection_memory(&stats);

        total_projection_bytes = total_projection_bytes.saturating_add(projection_bytes_lower_bound);
        total_visible_chars = total_visible_chars.saturating_add(stats.visible_len);
        total_tombstones = total_tombstones.saturating_add(stats.tombstone_count);
        total_index_bytes = total_index_bytes.saturating_add(index_bytes_lower_bound);

        per_key.insert(
            (*key).to_string(),
            serde_json::json!({
                "projection_residency": {
                    "metadata_entries": stats.metadata_entries,
                    "visible_len": stats.visible_len,
                    "tombstone_count": stats.tombstone_count,
                    "tombstone_span_count": stats.tombstone_span_count,
                    "tombstone_author_bucket_count": stats.tombstone_author_bucket_count,
                    "pending_deletes_count": stats.pending_deletes_count,
                    "child_bucket_count": stats.child_bucket_count
                },
                "memory": {
                    "projection_bytes_lower_bound": projection_bytes_lower_bound,
                    "metadata_bytes_lower_bound": metadata_bytes_lower_bound,
                    "index_bytes_lower_bound": index_bytes_lower_bound,
                    "bytes_per_visible_char_lower_bound": bytes_per_visible_char,
                    "bytes_per_tombstone_lower_bound": bytes_per_tombstone,
                    "index_weights_len": stats.index_weights_len,
                    "index_fenwick_len": stats.index_fenwick_len
                }
            }),
        );
    }

    let global_bytes_per_visible = if total_visible_chars == 0 {
        0.0
    } else {
        total_projection_bytes as f64 / total_visible_chars as f64
    };
    let global_bytes_per_tombstone = if total_tombstones == 0 {
        0.0
    } else {
        total_projection_bytes as f64 / total_tombstones as f64
    };

    // Sanity guardrails for the profile: all keys should remain populated.
    assert!(total_visible_chars > 0, "expected visible characters in profile");

    let max_bytes_per_visible = env_f64("NODALMERGE_TEXT_MEMORY_MAX_BYTES_PER_VISIBLE", 256.0);
    assert!(
        global_bytes_per_visible <= max_bytes_per_visible,
        "global bytes/visible too high: actual={global_bytes_per_visible:.3}, max={max_bytes_per_visible:.3}"
    );

    if let Ok(path) = std::env::var("NODALMERGE_TEXT_MEMORY_TELEMETRY_PATH") {
        let payload = serde_json::json!({
            "test": "long_lived_projection_memory_telemetry_profile",
            "global": {
                "projection_bytes_lower_bound": total_projection_bytes,
                "visible_chars": total_visible_chars,
                "tombstones": total_tombstones,
                "index_bytes_lower_bound": total_index_bytes,
                "bytes_per_visible_char_lower_bound": global_bytes_per_visible,
                "bytes_per_tombstone_lower_bound": global_bytes_per_tombstone
            },
            "per_key": per_key
        });
        let json = serde_json::to_string_pretty(&payload).unwrap();
        let _ = std::fs::write(path, json);
    }
}
