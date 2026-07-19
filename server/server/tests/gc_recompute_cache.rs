//! blob-cas-remediation.md slice 6.4 — GC recomputation caching.
//!
//! Every GC tick used to reload every cold room's full node history twice
//! over: once for `Rooms::sweep_blobs`' live-blob union and once (replayed
//! into a fresh `StateGraph`, signatures re-verified) for the studio-domain
//! live-hash collection. 6.4 caches both projections per room, keyed by
//! `NodePersistence::room_nodes_version` (a monotonic per-room version:
//! `MAX(seq)` on the SQLite store).
//!
//! **Correctness is the gate**: a stale cache that under-reports live
//! hashes = GC deletes live data. So the tests here are, in order of
//! importance:
//!
//! 1. **Cache equivalence** — the live set computed through a warm cache
//!    must equal a from-scratch computation (a fresh `Rooms` over the same
//!    store) after every mutation we can produce (new blob refs, new
//!    rooms, out-of-band room deletion, retention-config changes).
//! 2. **Teeth** — equivalence-by-construction proves nothing unless a
//!    *deliberately* stale cache makes it fail. The `debug_gc_plant_*`
//!    sabotage seams plant an entry that claims the room's current version
//!    while holding wrong data — exactly the bug version-keying prevents —
//!    and the same equivalence check must catch the divergence.
//! 3. **Deterministic re-scan accounting** — the second tick must do
//!    near-zero re-scan work, asserted via `GcScanCacheStats` counters,
//!    never wall time.
//!
//! Plus an `#[ignore]`d bench (loose ≥3x bound, real numbers in the slice
//! report) — CI runs only the deterministic tests.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use ed25519_dalek::SigningKey;
use nodalmerge_blobstore_conformance::make_setblob_node;
use nodalmerge_core::{Hash, MapOp, Op, StateGraph};
use nodalmerge_server::room::Rooms;
use nodalmerge_server::store::{
    BlobPersistence, Composite, DirPersistence, NodePersistence, SharedPersistence,
};
use nodalmerge_server::studio_live_hashes::collect_studio_live_hashes;

fn tmpdir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!("nodalmerge-6.4-{tag}-{nanos}"));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn fresh_rooms(persistence: &SharedPersistence) -> Rooms {
    Rooms::new(
        SigningKey::from_bytes(&[0x64u8; 32]),
        Arc::clone(persistence),
        512,
        0,
        0,
    )
}

/// Persist one SetBlob node straight into a room's history WITHOUT making
/// the room resident — these tests are about the cold-room paths.
fn persist_cold_setblob(
    persistence: &SharedPersistence,
    sk: &SigningKey,
    room_id: &str,
    key: &str,
    blob_hash: Hash,
) {
    let node = make_setblob_node(sk, key, blob_hash);
    persistence.persist_node(room_id, &node);
}

/// Same, for a studio engine-map entry (a promoted `MapSet`'s op shape —
/// see studio_gc.rs / studio_live_hashes.rs module docs).
fn persist_cold_mapset(
    persistence: &SharedPersistence,
    sk: &SigningKey,
    room_id: &str,
    key: &str,
    value: Vec<u8>,
) {
    let mut g = StateGraph::new();
    let id = g
        .apply_local(sk, 0, vec![Op::Map(MapOp::Set { key: key.into(), value })])
        .unwrap();
    let node = g.get_nodes(&[id]).into_iter().next().unwrap().clone();
    persistence.persist_node(room_id, &node);
}

fn tree_v2_blob(entries: serde_json::Value) -> (Hash, Vec<u8>) {
    let bytes = serde_json::to_vec(&serde_json::json!({
        "nodalmerge": "tree", "version": 2, "entries": entries
    }))
    .unwrap();
    (Hash::of(&bytes), bytes)
}

fn snapshot_envelope(
    snapshot_id: &str,
    repository_id: &str,
    generation: i64,
    created_at: &str,
    tree_hash: &str,
) -> Vec<u8> {
    snapshot_envelope_with_source(snapshot_id, repository_id, generation, created_at, tree_hash, None)
}

fn snapshot_envelope_with_source(
    snapshot_id: &str,
    repository_id: &str,
    generation: i64,
    created_at: &str,
    tree_hash: &str,
    source: Option<&str>,
) -> Vec<u8> {
    let payload = serde_json::json!({
        "SnapshotId": snapshot_id,
        "RepositoryId": repository_id,
        "TreeHash": tree_hash,
        "Generation": generation,
        "CreatedAt": created_at,
        "WorkUnitId": null,
        "Source": source,
        "TreeEntries": null,
    });
    serde_json::to_vec(&serde_json::json!({
        "v": 1, "kind": "studio/repository-snapshot/v1", "payload": payload
    }))
    .unwrap()
}

/// Store a file + a v2 tree naming it, returning (tree_hash, file_hash).
fn store_tree_with_file(persistence: &SharedPersistence, seed: &str) -> (Hash, Hash) {
    let file_bytes = format!("file-{seed}").into_bytes();
    let file_hash = Hash::of(&file_bytes);
    persistence.persist_blob(&file_hash, &file_bytes).unwrap();
    let (tree_hash, tree_bytes) = tree_v2_blob(serde_json::json!([
        {"n": format!("{seed}.txt"), "k": "f", "h": file_hash.to_hex()}
    ]));
    persistence.persist_blob(&tree_hash, &tree_bytes).unwrap();
    (tree_hash, file_hash)
}

/// The equivalence oracle: the warm-cache instance's answer must equal a
/// brand-new instance's from-scratch answer over the same store. Read-only
/// (no sweeps), so it can run between arbitrary mutations.
async fn assert_blob_live_equivalence(
    cached: &Rooms,
    persistence: &SharedPersistence,
) -> HashSet<Hash> {
    let via_cache = cached
        .collect_global_live_blob_hashes()
        .await
        .expect("store is enumerable");
    let from_scratch = fresh_rooms(persistence)
        .collect_global_live_blob_hashes()
        .await
        .expect("store is enumerable");
    assert_eq!(
        via_cache, from_scratch,
        "cached live-blob set must equal a from-scratch computation"
    );
    via_cache
}

async fn assert_studio_live_equivalence(
    cached: &Rooms,
    persistence: &SharedPersistence,
    retain_days: i64,
) -> HashSet<String> {
    let via_cache = collect_studio_live_hashes(cached, retain_days)
        .await
        .expect("collection succeeds");
    let from_scratch = collect_studio_live_hashes(&fresh_rooms(persistence), retain_days)
        .await
        .expect("collection succeeds");
    assert_eq!(
        via_cache, from_scratch,
        "cached studio live set must equal a from-scratch computation"
    );
    via_cache
}

// ─── the version seam itself ────────────────────────────────────────────────

/// `room_nodes_version` is monotonic per room and `None` for unknown rooms —
/// the two properties every cache decision above rests on.
#[test]
fn room_nodes_version_is_monotonic_and_none_for_unknown_rooms() {
    let persistence: SharedPersistence =
        Arc::new(DirPersistence::open(tmpdir("version")).unwrap());
    let sk = SigningKey::from_bytes(&[0x01u8; 32]);

    assert_eq!(persistence.room_nodes_version("nope"), None, "unknown room is None, not 0");

    persist_cold_setblob(&persistence, &sk, "r1", "k1", Hash::of(b"b1"));
    let v1 = persistence.room_nodes_version("r1").expect("versioned after first node");
    persist_cold_setblob(&persistence, &sk, "r1", "k2", Hash::of(b"b2"));
    let v2 = persistence.room_nodes_version("r1").expect("still versioned");
    assert!(v2 > v1, "a persisted node must advance the room's version ({v1} -> {v2})");

    // Another room's writes advance ONLY that room's version.
    persist_cold_setblob(&persistence, &sk, "r2", "k", Hash::of(b"b3"));
    assert_eq!(persistence.room_nodes_version("r1"), Some(v2));
}

/// The `Composite` forwarding pin — same hole-shape as 2.1/2.2's
/// (`room_nodes_version` is a *defaulted* trait method, so a missing
/// forward compiles silently). Here the default's `None` is safe-but-slow
/// rather than wrong: it would disable the 6.4 caches for the production
/// `Composite<node-store, S3BlobStore>` wiring — precisely the deployments
/// they exist for.
#[test]
fn composite_forwards_room_nodes_version_to_the_node_half() {
    let nodes = DirPersistence::open(tmpdir("composite-nodes")).unwrap();
    let blobs = DirPersistence::open(tmpdir("composite-blobs")).unwrap();
    let sk = SigningKey::from_bytes(&[0x02u8; 32]);

    let node = make_setblob_node(&sk, "k", Hash::of(b"payload"));
    nodes.persist_node("room-x", &node);
    let direct = nodes.room_nodes_version("room-x");
    assert!(direct.is_some(), "sanity: the node half can version the room");

    let composite = Composite::new(nodes, blobs);
    assert_eq!(
        composite.room_nodes_version("room-x"),
        direct,
        "Composite must forward room_nodes_version to its node half — inheriting the \
         None default would silently disable the 6.4 GC scan caches in production"
    );
}

// ─── cold-room live-blob cache (Rooms::sweep_blobs' union) ─────────────────

#[tokio::test]
async fn cold_room_live_set_cache_is_equivalent_under_mutations_and_skips_rescans() {
    let dir = tmpdir("cold-equiv");
    let persistence: SharedPersistence = Arc::new(DirPersistence::open(&dir).unwrap());
    let sk = SigningKey::from_bytes(&[0x11u8; 32]);
    let rooms = fresh_rooms(&persistence);

    // Four cold rooms, one referenced blob each. Never made resident.
    let mut hashes = Vec::new();
    for i in 0..4 {
        let h = Hash::of(format!("cold-blob-{i}").as_bytes());
        persistence
            .persist_blob(&h, format!("cold-blob-{i}").as_bytes())
            .unwrap();
        persist_cold_setblob(&persistence, &sk, &format!("room-{i}"), "k", h);
        hashes.push(h);
    }

    // Tick 1: every cold room is scanned once; result equals from-scratch.
    let live = assert_blob_live_equivalence(&rooms, &persistence).await;
    assert!(hashes.iter().all(|h| live.contains(h)));
    let s1 = rooms.gc_scan_cache_stats();
    assert_eq!((s1.cold_scans, s1.cold_hits), (4, 0), "tick 1 scans all 4 cold rooms");

    // Tick 2: nothing changed — near-zero re-scan work, same answer.
    assert_blob_live_equivalence(&rooms, &persistence).await;
    let s2 = rooms.gc_scan_cache_stats();
    assert_eq!(
        (s2.cold_scans, s2.cold_hits),
        (4, 4),
        "tick 2 must be served entirely from cache (deterministic counter, not wall time)"
    );

    // Mutation A — a new blob ref lands in room-0 (its seq advances).
    let late = Hash::of(b"late-arriving-ref");
    persistence.persist_blob(&late, b"late-arriving-ref").unwrap();
    persist_cold_setblob(&persistence, &sk, "room-0", "k2", late);
    let live = assert_blob_live_equivalence(&rooms, &persistence).await;
    assert!(live.contains(&late), "the new ref must be live immediately, not a tick later");
    let s3 = rooms.gc_scan_cache_stats();
    assert_eq!(
        (s3.cold_scans, s3.cold_hits),
        (5, 7),
        "only the mutated room re-scans; the other three hit"
    );

    // Mutation B — an entirely new room appears.
    let newcomer = Hash::of(b"new-room-blob");
    persistence.persist_blob(&newcomer, b"new-room-blob").unwrap();
    persist_cold_setblob(&persistence, &sk, "room-new", "k", newcomer);
    let live = assert_blob_live_equivalence(&rooms, &persistence).await;
    assert!(live.contains(&newcomer));
    let s4 = rooms.gc_scan_cache_stats();
    assert_eq!((s4.cold_scans, s4.cold_hits), (6, 11));

    // Mutation C — a room is deleted out-of-band (there is no in-trait
    // delete path; an operator dropping rows is the closest real-world
    // shape). Its hash must leave the live set, its cache entry must not
    // resurrect it, and equivalence must hold.
    {
        let conn = rusqlite::Connection::open(dir.join("nodalmerge.db")).unwrap();
        conn.execute("DELETE FROM nodes WHERE room_id = 'room-3'", []).unwrap();
    }
    assert_eq!(persistence.room_nodes_version("room-3"), None, "deleted room answers None");
    let live = assert_blob_live_equivalence(&rooms, &persistence).await;
    assert!(
        !live.contains(&hashes[3]),
        "a deleted room's blob refs must drop out of the live set despite the warm cache"
    );
}

/// The RED-with-teeth half: equivalence testing is only evidence if a cache
/// that is deliberately NOT invalidated makes it fail. Plant an entry that
/// claims room-0's current version but holds an empty hash set — the exact
/// state a forgotten invalidation would produce — and the oracle must see
/// the divergence (specifically: the cached side under-reports, the
/// dangerous direction). Then prove version-keying self-heals it.
#[tokio::test]
async fn planted_stale_cold_cache_entry_diverges_and_version_advance_heals_it() {
    let persistence: SharedPersistence =
        Arc::new(DirPersistence::open(tmpdir("cold-teeth")).unwrap());
    let sk = SigningKey::from_bytes(&[0x22u8; 32]);
    let rooms = fresh_rooms(&persistence);

    let real = Hash::of(b"the-live-blob");
    persistence.persist_blob(&real, b"the-live-blob").unwrap();
    persist_cold_setblob(&persistence, &sk, "room-0", "k", real);

    // Sabotage: current-version entry, wrong (empty) content.
    assert!(
        rooms.debug_gc_plant_stale_cold_cache_entry("room-0", HashSet::new()),
        "seam must plant (the room is versioned)"
    );

    let via_cache = rooms
        .collect_global_live_blob_hashes()
        .await
        .expect("enumerable");
    let from_scratch = fresh_rooms(&persistence)
        .collect_global_live_blob_hashes()
        .await
        .expect("enumerable");
    assert_ne!(
        via_cache, from_scratch,
        "a non-invalidated stale cache MUST be detectable by the equivalence oracle — \
         if this passes, the equivalence tests in this file prove nothing"
    );
    assert!(
        !via_cache.contains(&real) && from_scratch.contains(&real),
        "and the divergence is the dangerous direction: the stale cache under-reports"
    );

    // Any real mutation advances the version past the planted entry.
    let second = Hash::of(b"second-ref");
    persistence.persist_blob(&second, b"second-ref").unwrap();
    persist_cold_setblob(&persistence, &sk, "room-0", "k2", second);
    let healed = assert_blob_live_equivalence(&rooms, &persistence).await;
    assert!(healed.contains(&real) && healed.contains(&second));
}

// ─── cold-room studio-map cache (collect_studio_live_hashes) ───────────────

#[tokio::test]
async fn studio_map_cache_is_equivalent_under_mutations_and_skips_replays() {
    let persistence: SharedPersistence =
        Arc::new(DirPersistence::open(tmpdir("studio-equiv")).unwrap());
    let sk = SigningKey::from_bytes(&[0x33u8; 32]);
    let rooms = fresh_rooms(&persistence);

    // Two cold repo rooms, one retained (head) snapshot each.
    let (tree_a, file_a) = store_tree_with_file(&persistence, "repo-a-head");
    persist_cold_mapset(
        &persistence, &sk, "repo/repo-a",
        "studio/repository-snapshot/v1/gen-0",
        snapshot_envelope("gen-0", "repo-a", 0, "2026-01-01T00:00:00Z", &tree_a.to_hex()),
    );
    let (tree_b, file_b) = store_tree_with_file(&persistence, "repo-b-head");
    persist_cold_mapset(
        &persistence, &sk, "repo/repo-b",
        "studio/repository-snapshot/v1/gen-0",
        snapshot_envelope("gen-0", "repo-b", 0, "2026-01-01T00:00:00Z", &tree_b.to_hex()),
    );

    // Tick 1: both rooms replayed; live set equals from-scratch.
    let live = assert_studio_live_equivalence(&rooms, &persistence, 30).await;
    for h in [&tree_a, &file_a, &tree_b, &file_b] {
        assert!(live.contains(&h.to_hex()), "expected {} live", h.to_hex());
    }
    let s1 = rooms.gc_scan_cache_stats();
    assert_eq!((s1.studio_scans, s1.studio_hits), (2, 0));

    // Tick 2: cache hits only — the full-history replay (signature
    // re-verification and all) must not run again.
    assert_studio_live_equivalence(&rooms, &persistence, 30).await;
    let s2 = rooms.gc_scan_cache_stats();
    assert_eq!(
        (s2.studio_scans, s2.studio_hits),
        (2, 2),
        "tick 2 must replay nothing (deterministic counter, not wall time)"
    );

    // Mutation: a new snapshot lands in repo-a (a studio-map edit IS a new
    // node, so the room's version advances — there is no mutation path to a
    // cold room's studio map that bypasses persisted nodes).
    let (tree_a2, file_a2) = store_tree_with_file(&persistence, "repo-a-gen1");
    persist_cold_mapset(
        &persistence, &sk, "repo/repo-a",
        "studio/repository-snapshot/v1/gen-1",
        snapshot_envelope("gen-1", "repo-a", 1, "2026-02-01T00:00:00Z", &tree_a2.to_hex()),
    );
    let live = assert_studio_live_equivalence(&rooms, &persistence, 30).await;
    assert!(live.contains(&tree_a2.to_hex()) && live.contains(&file_a2.to_hex()));
    let s3 = rooms.gc_scan_cache_stats();
    assert_eq!(
        (s3.studio_scans, s3.studio_hits),
        (3, 3),
        "only the mutated room replays; the other hits"
    );
}

/// Retention config is an input the caches must NOT capture: the cached
/// layer is the pure node-history projection, and classification (which
/// reads `retain_intermediate_days` and "now") reruns on top every tick.
/// So changing the retention window on a fully-warm cache must change the
/// live set exactly as it would from scratch — no seq advanced anywhere.
#[tokio::test]
async fn retention_config_change_takes_effect_on_a_warm_cache_without_any_seq_advance() {
    let persistence: SharedPersistence =
        Arc::new(DirPersistence::open(tmpdir("studio-retention")).unwrap());
    let sk = SigningKey::from_bytes(&[0x44u8; 32]);
    let rooms = fresh_rooms(&persistence);

    // gen-stale: old, non-head, non-pinned → Intermediate, expired under a
    // 30-day window. gen-head: current head, always retained.
    let (tree_stale, file_stale) = store_tree_with_file(&persistence, "stale");
    persist_cold_mapset(
        &persistence, &sk, "repo/repo-r",
        "studio/repository-snapshot/v1/gen-stale",
        snapshot_envelope("gen-stale", "repo-r", 0, "2000-01-01T00:00:00Z", &tree_stale.to_hex()),
    );
    let (tree_head, _file_head) = store_tree_with_file(&persistence, "head");
    persist_cold_mapset(
        &persistence, &sk, "repo/repo-r",
        "studio/repository-snapshot/v1/gen-head",
        snapshot_envelope("gen-head", "repo-r", 1, "2026-01-01T00:00:00Z", &tree_head.to_hex()),
    );

    // Warm the cache under the 30-day window: stale generation excluded.
    let narrow = assert_studio_live_equivalence(&rooms, &persistence, 30).await;
    assert!(!narrow.contains(&file_stale.to_hex()), "expired Intermediate is not live");

    // Same warm instance, wide window: the SAME cached map must now
    // classify gen-stale as retained. No node was persisted in between.
    let wide = assert_studio_live_equivalence(&rooms, &persistence, 1_000_000).await;
    assert!(
        wide.contains(&file_stale.to_hex()) && wide.contains(&tree_stale.to_hex()),
        "a retention-window change must take effect through the warm cache"
    );
    assert_ne!(narrow, wide, "sanity: the config change actually changed the answer");

    let stats = rooms.gc_scan_cache_stats();
    assert_eq!(
        stats.studio_scans, 1,
        "both windows were computed from ONE replay — classification is never cached"
    );
}

/// Teeth for the studio-map cache, mirroring the cold-live one: a planted
/// current-version entry holding an empty studio map must make the
/// equivalence oracle fail (under-reporting again), and a real studio edit
/// (a new node) must heal it.
#[tokio::test]
async fn planted_stale_studio_map_entry_diverges_and_version_advance_heals_it() {
    let persistence: SharedPersistence =
        Arc::new(DirPersistence::open(tmpdir("studio-teeth")).unwrap());
    let sk = SigningKey::from_bytes(&[0x55u8; 32]);
    let rooms = fresh_rooms(&persistence);

    // Bootstrap ⇒ Pinned forever — so gen-0's blobs stay live even after a
    // later generation becomes head (an Intermediate gen-0 would age out
    // and make the heal assertion below meaningless).
    let (tree, file) = store_tree_with_file(&persistence, "teeth");
    persist_cold_mapset(
        &persistence, &sk, "repo/repo-t",
        "studio/repository-snapshot/v1/gen-0",
        snapshot_envelope_with_source(
            "gen-0", "repo-t", 0, "2026-01-01T00:00:00Z", &tree.to_hex(), Some("Bootstrap"),
        ),
    );

    assert!(
        rooms.debug_gc_plant_stale_studio_map_entry("repo/repo-t", HashMap::new()),
        "seam must plant (the room is versioned)"
    );

    let via_cache = collect_studio_live_hashes(&rooms, 30).await.expect("collects");
    let from_scratch = collect_studio_live_hashes(&fresh_rooms(&persistence), 30)
        .await
        .expect("collects");
    assert_ne!(via_cache, from_scratch, "the stale studio map must be detectable");
    assert!(
        !via_cache.contains(&file.to_hex()) && from_scratch.contains(&file.to_hex()),
        "divergence is the dangerous direction: cached side under-reports"
    );

    // A studio edit = a new node = version advance = rescan.
    let (tree2, _file2) = store_tree_with_file(&persistence, "teeth-gen1");
    persist_cold_mapset(
        &persistence, &sk, "repo/repo-t",
        "studio/repository-snapshot/v1/gen-1",
        snapshot_envelope("gen-1", "repo-t", 1, "2026-02-01T00:00:00Z", &tree2.to_hex()),
    );
    let healed = assert_studio_live_equivalence(&rooms, &persistence, 30).await;
    assert!(healed.contains(&file.to_hex()) && healed.contains(&tree2.to_hex()));
}

// ─── bench (not CI-gating) ──────────────────────────────────────────────────

/// Bench: many-cold-room GC tick, first (cold caches) vs second (warm).
/// Loose ≥3x sanity bound only; measured numbers go in the slice report.
#[tokio::test]
#[ignore = "bench: run by hand with --ignored --nocapture (slice 6.4 perf validation)"]
async fn bench_second_tick_on_many_cold_rooms() {
    const ROOMS: usize = 120;
    const NODES_PER_ROOM: usize = 60;

    let persistence: SharedPersistence =
        Arc::new(DirPersistence::open(tmpdir("bench")).unwrap());
    let sk = SigningKey::from_bytes(&[0x66u8; 32]);
    let rooms = fresh_rooms(&persistence);

    for r in 0..ROOMS {
        let room_id = format!("repo/bench-{r}");
        let mut nodes = Vec::with_capacity(NODES_PER_ROOM);
        for n in 0..NODES_PER_ROOM {
            nodes.push(make_setblob_node(
                &sk,
                &format!("k{n}"),
                Hash::of(format!("{r}/{n}").as_bytes()),
            ));
        }
        let refs: Vec<&nodalmerge_core::SyncNode> = nodes.iter().collect();
        persistence.persist_nodes(&room_id, &refs);
        // One studio snapshot per room so the studio replay has real work.
        let (tree, _file) = store_tree_with_file(&persistence, &format!("bench-{r}"));
        persist_cold_mapset(
            &persistence, &sk, &room_id,
            "studio/repository-snapshot/v1/gen-0",
            snapshot_envelope("gen-0", &format!("bench-{r}"), 0, "2026-01-01T00:00:00Z", &tree.to_hex()),
        );
    }

    let t0 = std::time::Instant::now();
    let live1 = rooms.collect_global_live_blob_hashes().await.unwrap();
    let studio1 = collect_studio_live_hashes(&rooms, 30).await.unwrap();
    let cold_tick = t0.elapsed();

    let t0 = std::time::Instant::now();
    let live2 = rooms.collect_global_live_blob_hashes().await.unwrap();
    let studio2 = collect_studio_live_hashes(&rooms, 30).await.unwrap();
    let warm_tick = t0.elapsed();

    assert_eq!(live1, live2);
    assert_eq!(studio1, studio2);
    let stats = rooms.gc_scan_cache_stats();
    println!(
        "GC tick over {ROOMS} cold rooms x {NODES_PER_ROOM} nodes: cold {cold_tick:?}, warm {warm_tick:?}, \
         speedup {:.1}x; stats {stats:?}",
        cold_tick.as_secs_f64() / warm_tick.as_secs_f64(),
    );
    assert_eq!(stats.cold_scans, ROOMS, "warm tick re-scanned nothing (blob side)");
    assert_eq!(stats.studio_scans, ROOMS, "warm tick replayed nothing (studio side)");
    assert!(
        warm_tick < cold_tick / 3,
        "warm tick should be at least ~3x faster (loose sanity bound, not a CI gate): \
         cold {cold_tick:?} vs warm {warm_tick:?}"
    );
}
