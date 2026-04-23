//! Micro-bench: compare `serde_json::to_vec` (current) vs `postcard::to_allocvec`
//! (proposed) as the canonical serialization used by `Transaction::hash`.
//!
//! This is a read-only probe — it does NOT change `Transaction::hash`. Its
//! purpose is to quantify the cost difference before we commit to a wire-
//! breaking migration.
//!
//! Encodes a representative 3-op `Transaction` (matches what `merge_10k`
//! builds, plus a second op, plus one parent link). Measures:
//!   1. serialize-only  (`serde_json` vs `postcard`)
//!   2. serialize + blake3 hash end-to-end
//!   3. output size in bytes (printed once as context)

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use activesync_core::{Op, MapOp, Transaction};
use activesync_core::hash::Hash;

fn sample_tx() -> Transaction {
    Transaction {
        author:  [7u8; 32],
        lamport: 42,
        wall_ms: 1_700_000_000_000,
        ops: vec![
            Op::Map(MapOp::Set {
                key:   "user.name".to_string(),
                value: b"alice".to_vec(),
            }),
            Op::Map(MapOp::Set {
                key:   "user.email".to_string(),
                value: b"alice@example.com".to_vec(),
            }),
            Op::Map(MapOp::Delete {
                key: "user.temp".to_string(),
            }),
        ],
        parents: vec![Hash::of(b"parent-a")],
    }
}

fn bench_tx_hash(c: &mut Criterion) {
    let tx = sample_tx();

    let json_bytes = serde_json::to_vec(&tx).unwrap();
    let postcard_bytes = postcard::to_allocvec(&tx).unwrap();
    eprintln!(
        "tx_hash sizes: json={} bytes, postcard={} bytes (ratio {:.2}x)",
        json_bytes.len(),
        postcard_bytes.len(),
        json_bytes.len() as f64 / postcard_bytes.len() as f64,
    );

    // ---- serialize only ----
    c.bench_function("tx_serialize_json", |b| {
        b.iter(|| {
            let v = serde_json::to_vec(black_box(&tx)).unwrap();
            black_box(v);
        });
    });

    c.bench_function("tx_serialize_postcard", |b| {
        b.iter(|| {
            let v = postcard::to_allocvec(black_box(&tx)).unwrap();
            black_box(v);
        });
    });

    // ---- serialize + blake3 (the actual op: matches Transaction::hash shape) ----
    c.bench_function("tx_hash_json_plus_blake3", |b| {
        b.iter(|| {
            let v = serde_json::to_vec(black_box(&tx)).unwrap();
            black_box(Hash::of(&v));
        });
    });

    c.bench_function("tx_hash_postcard_plus_blake3", |b| {
        b.iter(|| {
            let v = postcard::to_allocvec(black_box(&tx)).unwrap();
            black_box(Hash::of(&v));
        });
    });
}

criterion_group!(benches, bench_tx_hash);
criterion_main!(benches);
