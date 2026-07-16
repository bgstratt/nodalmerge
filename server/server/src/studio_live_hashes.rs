//! S5.3 — the studio-domain `LiveHashSource`.
//!
//! Computes retention-aware GC reachability over every room the server
//! holds, by reading `"studio"`-namespace engine-map entries directly out of
//! each room's `StateGraph` (per `nodalmerge-studio/docs/STUDIO_ROOM_SCHEMA.md`
//! §a) and mirroring the classification semantics of nodalmerge-studio's
//! `SnapshotRetentionPolicy` (Pinned / Active / Intermediate) — see slice
//! 5.1's findings note in `plans/cas-distribution-and-storage.md` for the
//! exact rules this ports.
//!
//! ## Where the bytes come from
//!
//! Studio writes via the host-core `MapSet{namespace: "studio", key, value}`
//! command. `namespace` is *not* concatenated into the CRDT key — only the
//! raw `key` (`"{kind}/{entityId}"`, e.g. `"studio/work-unit/v1/WU-42"`)
//! ever reaches `core::crdt::MapOp::Set`, once a checkpoint is promoted into
//! the room's graph (`PromoteCheckpointToGraph`). So a plain
//! `StateGraph::resolve_with_meta()` filtered by `k.starts_with("studio/")`
//! sees every studio record — no separate namespace decoding needed. The
//! promotion step encodes JSON object values as their compact UTF-8 text
//! (`serde_json::Value::to_string()`), so the resolved bytes for a
//! non-blob entry are exactly the envelope's JSON text
//! (`{"v":1,"kind":"...","payload":{...}}`), parseable with `serde_json`
//! directly.
//!
//! ## JSON field casing (verified against the C# records, not the
//! illustrative `studio-room-schema-vectors.v1.json` payload examples —
//! see this slice's final report for why)
//!
//! `IStudioNodeStore.WriteNodeAsync` payloads are produced by bare
//! `JsonSerializer.Serialize(entity)` calls with no naming-policy options
//! anywhere on that path, so JSON keys are **exact C# PascalCase property
//! names**, and enums serialize as their **integer ordinal** (no
//! `JsonStringEnumConverter` on this path). This module's `#[serde(rename =
//! ...)]` attributes and integer status constants below match that, not the
//! camelCase/string-enum placeholder payloads in the frozen vectors file
//! (those vectors only pin the envelope/key/repositories-map/pinned-
//! reference shapes, never a real `WorkUnit`/`RepositorySnapshot` payload —
//! confirmed by reading `StudioRoomSchemaVectorTests.cs`).
//!
//! ## Fail-closed, not fail-safe
//!
//! `SnapshotRetentionPolicy.cs` (5.1) is a read-only *report*: malformed
//! nodes are classified Active-with-anomaly so nothing is ever silently
//! under-protected, but nothing consumes the report to delete anything
//! yet. This module's output directly drives real deletion (5.3), so it is
//! deliberately **stricter**: any envelope/payload that fails to parse, any
//! `TreeHash` that isn't canonical hex, or any tree/blob a retained
//! snapshot's tree walk can't resolve aborts the **entire** collection with
//! `Err` — no partial live sets, ever. This is an intentional, documented
//! divergence from 5.1's fail-safe-to-Active posture, justified by
//! `docs/delegated-storage-gc.md`'s normative "fail closed" rule applying to
//! a component that gates real deletes, not a report.
//!
//! ## Non-repo rooms
//!
//! Only ~13 `StudioNodeKind`s route to `repo/{repoId}` rooms as of 6.3a;
//! everything else (and anything written before that migration) can still
//! live in the legacy `"studio"` room or a `"workgroup"` room. Per the
//! plan's instruction, this module still scans every room (not just
//! `repo/*`) for the same three kinds, and — being unable to run full
//! cross-work-unit classification without the rest of that repo's data —
//! conservatively retains **every** valid snapshot found there forever
//! (skips Intermediate age-out entirely). This is deliberately more
//! generous than the `repo/*` classification; see the final report for the
//! parity note.

use std::collections::{HashMap, HashSet};

use nodalmerge_core::{Hash, StateGraph, SyncNode};
use nodalmerge_gc::{GcError, GcResult};
use serde::Deserialize;
use serde_json::Value;

use crate::room::Rooms;
use crate::store::{hash_from_hex, BlobPersistence};
use crate::tree_walk;

/// Prefix every studio engine-map key carries (`STUDIO_ROOM_SCHEMA.md` §a).
const STUDIO_KEY_PREFIX: &str = "studio/";

const KIND_REPOSITORY_SNAPSHOT: &str = "studio/repository-snapshot/v1";
const KIND_WORK_UNIT: &str = "studio/work-unit/v1";
const KIND_REPOSITORY_OP: &str = "studio/repository-op/v1";

/// `WorkUnitStatus` ordinals (nodalmerge-studio
/// `src/NodalMerge.Studio.Contracts/Domain/WorkUnit.cs`), declaration order:
/// `Created(0), Active(1), Waiting(2), Completed(3), Failed(4),
/// Cancelled(5), Queued(6), Executing(7), Proposed(8), Reviewing(9),
/// Merged(10), DeadLettered(11), Retrying(12)`.
///
/// Terminal set: a status is terminal here only if `WorkUnitTransitions.
/// CanTransition` has **zero outgoing edges** from it — i.e. the GC can
/// prove the work unit's seed snapshot will never be resumed, and so is
/// safe to let age out under `RetainIntermediateDays` instead of being
/// retained forever like a Pinned/Active seed.
///
/// `Cancelled`/`DeadLettered` are deliberately *not* terminal here (unlike
/// the narrower cache-eviction terminal set elsewhere in Studio): both have
/// live revival edges (`Cancelled -> Queued`/`Executing`;
/// `DeadLettered -> Retrying`/`Proposed`/`Merged`/`Queued`/`Executing`).
///
/// `Failed` was terminal here until slice 1.6 (blob-cas-remediation.md
/// finding #30), on the documented-but-false claim that `CanTransition` had
/// zero outgoing edges from `Completed`/`Failed`/`Merged`. It does not:
/// `WorkUnit.cs`'s `(_, Cancelled) when from is not Completed and not
/// Merged` rule permits `Failed -> Cancelled`, and `Cancelled ->
/// Queued`/`Executing` are legal — so `Failed -> Cancelled -> Queued`
/// revives a work unit out of a status this GC was retention-aging, which
/// could delete a seed snapshot a human later returns to resume. Slice 1.6
/// dropped `Failed` from `TERMINAL_STATUSES` so it is protected the same
/// way as `Cancelled`/`DeadLettered` (product decision, recorded in the
/// plan: a failed work unit is resumable). Accepted consequence: `Failed`
/// seeds now retain indefinitely, same as `Cancelled`/`DeadLettered` — see
/// the plan's "Follow-ups filed" section for the (out-of-scope) revival-
/// window policy that would bound this for all three.
const STATUS_COMPLETED: i64 = 3;
const STATUS_MERGED: i64 = 10;
const TERMINAL_STATUSES: [i64; 2] = [STATUS_COMPLETED, STATUS_MERGED];

fn is_terminal_status(status: i64) -> bool {
    TERMINAL_STATUSES.contains(&status)
}

/// Default `RetainIntermediateDays` (mirrors
/// `RetentionPolicyOptions.RetainIntermediateDays`'s default of 30).
pub const DEFAULT_RETAIN_INTERMEDIATE_DAYS: i64 = 30;

// ─── Payload shapes (exact C# PascalCase field names; see module docs) ────

#[derive(Debug, Clone, Deserialize)]
struct SnapshotPayloadRaw {
    #[serde(rename = "SnapshotId")]
    snapshot_id: String,
    #[serde(rename = "RepositoryId")]
    repository_id: String,
    #[serde(rename = "TreeHash")]
    tree_hash: String,
    #[serde(rename = "Generation")]
    generation: i64,
    #[serde(rename = "CreatedAt")]
    created_at: String,
    #[serde(rename = "WorkUnitId", default)]
    work_unit_id: Option<String>,
    #[serde(rename = "Source", default)]
    source: Option<String>,
    #[serde(rename = "TreeEntries", default)]
    tree_entries: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Deserialize)]
struct WorkUnitPayloadRaw {
    #[serde(rename = "WorkUnitId")]
    work_unit_id: String,
    #[serde(rename = "Status")]
    status: i64,
    #[serde(rename = "CreatedAt")]
    created_at: String,
    #[serde(rename = "UpdatedAt")]
    updated_at: String,
    #[serde(rename = "RepositoryId", default)]
    repository_id: Option<String>,
    #[serde(rename = "Metadata", default)]
    metadata: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Deserialize)]
struct RepositoryOpPayloadRaw {
    #[serde(rename = "OldBlobId", default)]
    old_blob_id: Option<String>,
    #[serde(rename = "NewBlobId", default)]
    new_blob_id: Option<String>,
}

/// Parsed + timestamp-resolved snapshot, ready for classification.
#[derive(Debug, Clone)]
struct ParsedSnapshot {
    snapshot_id: String,
    repository_id: String,
    tree_hash: String,
    generation: i64,
    created_at_nanos: i128,
    work_unit_id: Option<String>,
    source: Option<String>,
    tree_entries: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone)]
struct ParsedWorkUnit {
    work_unit_id: String,
    status: i64,
    created_at_nanos: i128,
    updated_at_nanos: i128,
    repository_id: Option<String>,
    metadata: Option<HashMap<String, String>>,
}

/// Longest-prefix-first candidate kinds (`STUDIO_ROOM_SCHEMA.md` §a parsing
/// rule) — only the three kinds this walk needs.
fn known_kinds_longest_first() -> [&'static str; 3] {
    let mut kinds = [KIND_REPOSITORY_SNAPSHOT, KIND_REPOSITORY_OP, KIND_WORK_UNIT];
    kinds.sort_by_key(|k| std::cmp::Reverse(k.len()));
    kinds
}

fn parse_studio_key(key: &str) -> Option<(&'static str, &str)> {
    for kind in known_kinds_longest_first() {
        if let Some(entity_id) = key.strip_prefix(kind).and_then(|rest| rest.strip_prefix('/')) {
            return Some((kind, entity_id));
        }
    }
    None
}

// ─── Timestamp parsing (hand-rolled, no chrono/time dependency — same
// posture as `blob_http.rs`'s `civil_from_days`/`format_iso8601_utc`) ──────

/// Parse a .NET `DateTimeOffset` round-trip ("O"/RFC3339-ish) string into
/// nanoseconds since the Unix epoch (UTC), for ordering/arithmetic. Accepts
/// `Z` or a numeric `+HH:MM`/`-HH:MM` offset and 0-9 fractional digits.
fn parse_timestamp_nanos(s: &str) -> Option<i128> {
    let bytes = s.as_bytes();
    if bytes.len() < 20 {
        return None;
    }
    let year: i64 = s.get(0..4)?.parse().ok()?;
    if bytes.get(4) != Some(&b'-') {
        return None;
    }
    let month: u32 = s.get(5..7)?.parse().ok()?;
    if bytes.get(7) != Some(&b'-') {
        return None;
    }
    let day: u32 = s.get(8..10)?.parse().ok()?;
    if !matches!(bytes.get(10), Some(b'T') | Some(b't')) {
        return None;
    }
    let hour: i64 = s.get(11..13)?.parse().ok()?;
    if bytes.get(13) != Some(&b':') {
        return None;
    }
    let minute: i64 = s.get(14..16)?.parse().ok()?;
    if bytes.get(16) != Some(&b':') {
        return None;
    }
    let second: i64 = s.get(17..19)?.parse().ok()?;

    let mut idx = 19;
    let mut frac_nanos: i64 = 0;
    if bytes.get(idx) == Some(&b'.') {
        idx += 1;
        let start = idx;
        while idx < bytes.len() && bytes[idx].is_ascii_digit() {
            idx += 1;
        }
        let frac_str = &s[start..idx];
        if !frac_str.is_empty() {
            let mut digits = frac_str.to_string();
            if digits.len() > 9 {
                digits.truncate(9);
            }
            while digits.len() < 9 {
                digits.push('0');
            }
            frac_nanos = digits.parse().ok()?;
        }
    }

    let offset_seconds: i64 = match bytes.get(idx) {
        Some(b'Z') | Some(b'z') => 0,
        Some(b'+') | Some(b'-') => {
            let sign: i64 = if bytes[idx] == b'-' { -1 } else { 1 };
            let rest = &s[idx + 1..];
            if rest.len() < 5 {
                return None;
            }
            let oh: i64 = rest.get(0..2)?.parse().ok()?;
            let om: i64 = rest.get(3..5)?.parse().ok()?;
            sign * (oh * 3600 + om * 60)
        }
        _ => return None,
    };

    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + hour * 3600 + minute * 60 + second - offset_seconds;
    Some((secs as i128) * 1_000_000_000 + frac_nanos as i128)
}

/// Howard Hinnant's `days_from_civil` (inverse of `blob_http.rs`'s
/// `civil_from_days`): proleptic-Gregorian (year, month, day) → days since
/// 1970-01-01. <http://howardhinnant.github.io/date_algorithms.html#days_from_civil>
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mm = m as i64;
    let doy = (153 * (if mm > 2 { mm - 3 } else { mm + 9 }) + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// Convert `SystemTime` to nanoseconds since the Unix epoch for comparison
/// against parsed payload timestamps. Saturates rather than panics on a
/// pre-epoch clock (never expected in practice).
fn system_time_to_nanos(t: std::time::SystemTime) -> i128 {
    match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i128,
        Err(e) => -(e.duration().as_nanos() as i128),
    }
}

// ─── Parsing a room's resolved map into typed rows ─────────────────────────

#[derive(Debug)]
struct RoomStudioData {
    snapshots: Vec<ParsedSnapshot>,
    work_units: Vec<ParsedWorkUnit>,
    /// Rule (b): every op's referenced blob hashes, lowercase hex.
    op_referenced_hashes: Vec<String>,
    /// Direct `SetBlob` pointers found under a `studio/` key (not part of
    /// today's documented encoding, but protected conservatively if ever
    /// seen — see module docs).
    direct_blob_hashes: Vec<Hash>,
}

fn parse_room_studio_data(
    room_id: &str,
    resolved: &HashMap<String, (Vec<u8>, bool)>,
) -> GcResult<RoomStudioData> {
    let mut snapshots = Vec::new();
    let mut work_units = Vec::new();
    let mut op_referenced_hashes = Vec::new();
    let mut direct_blob_hashes = Vec::new();

    for (key, (bytes, is_blob)) in resolved {
        let Some((kind, entity_id)) = parse_studio_key(key) else {
            continue;
        };

        if *is_blob {
            if bytes.len() != 32 {
                return Err(GcError::Backend(format!(
                    "room {room_id} key {key}: SetBlob value is {} bytes, expected 32",
                    bytes.len()
                )));
            }
            let mut arr = [0u8; 32];
            arr.copy_from_slice(bytes);
            direct_blob_hashes.push(Hash(arr));
            continue;
        }

        let envelope: Value = serde_json::from_slice(bytes).map_err(|e| {
            GcError::Backend(format!("room {room_id} key {key}: malformed envelope JSON: {e}"))
        })?;
        let payload = envelope.get("payload").cloned().ok_or_else(|| {
            GcError::Backend(format!("room {room_id} key {key}: envelope missing 'payload'"))
        })?;

        match kind {
            KIND_REPOSITORY_SNAPSHOT => {
                let raw: SnapshotPayloadRaw = serde_json::from_value(payload).map_err(|e| {
                    GcError::Backend(format!("room {room_id} snapshot {entity_id}: {e}"))
                })?;
                let created_at_nanos = parse_timestamp_nanos(&raw.created_at).ok_or_else(|| {
                    GcError::Backend(format!(
                        "room {room_id} snapshot {entity_id}: CreatedAt {:?} did not parse as an ISO-8601 timestamp",
                        raw.created_at
                    ))
                })?;
                snapshots.push(ParsedSnapshot {
                    snapshot_id: raw.snapshot_id,
                    repository_id: raw.repository_id,
                    tree_hash: raw.tree_hash,
                    generation: raw.generation,
                    created_at_nanos,
                    work_unit_id: raw.work_unit_id,
                    source: raw.source,
                    tree_entries: raw.tree_entries,
                });
            }
            KIND_WORK_UNIT => {
                let raw: WorkUnitPayloadRaw = serde_json::from_value(payload).map_err(|e| {
                    GcError::Backend(format!("room {room_id} work-unit {entity_id}: {e}"))
                })?;
                let created_at_nanos = parse_timestamp_nanos(&raw.created_at).ok_or_else(|| {
                    GcError::Backend(format!(
                        "room {room_id} work-unit {entity_id}: CreatedAt {:?} did not parse",
                        raw.created_at
                    ))
                })?;
                let updated_at_nanos = parse_timestamp_nanos(&raw.updated_at).ok_or_else(|| {
                    GcError::Backend(format!(
                        "room {room_id} work-unit {entity_id}: UpdatedAt {:?} did not parse",
                        raw.updated_at
                    ))
                })?;
                work_units.push(ParsedWorkUnit {
                    work_unit_id: raw.work_unit_id,
                    status: raw.status,
                    created_at_nanos,
                    updated_at_nanos,
                    repository_id: raw.repository_id,
                    metadata: raw.metadata,
                });
            }
            KIND_REPOSITORY_OP => {
                let raw: RepositoryOpPayloadRaw = serde_json::from_value(payload).map_err(|e| {
                    GcError::Backend(format!("room {room_id} repository-op {entity_id}: {e}"))
                })?;
                if let Some(h) = raw.old_blob_id {
                    op_referenced_hashes.push(h.to_lowercase());
                }
                if let Some(h) = raw.new_blob_id {
                    op_referenced_hashes.push(h.to_lowercase());
                }
            }
            _ => {}
        }
    }

    Ok(RoomStudioData {
        snapshots,
        work_units,
        op_referenced_hashes,
        direct_blob_hashes,
    })
}

// ─── Retention classification (mirrors SnapshotRetentionPolicy.cs) ─────────

/// Classify a `repo/*` room's snapshots per 5.1's Pinned/Active/Intermediate
/// rules, returning the set of retained `SnapshotId`s.
fn classify_repo_room(
    snapshots: &[ParsedSnapshot],
    work_units: &[ParsedWorkUnit],
    retain_intermediate_days: i64,
    now_nanos: i128,
) -> HashSet<String> {
    // Current head per repository: highest Generation, ties broken by
    // CreatedAt then SnapshotId (ordinal string compare) — mirrors
    // SnapshotRetentionPolicy.cs exactly.
    let mut head_by_repo: HashMap<&str, &ParsedSnapshot> = HashMap::new();
    for snap in snapshots {
        let replace = match head_by_repo.get(snap.repository_id.as_str()) {
            None => true,
            Some(existing) => {
                snap.generation > existing.generation
                    || (snap.generation == existing.generation
                        && snap.created_at_nanos > existing.created_at_nanos)
                    || (snap.generation == existing.generation
                        && snap.created_at_nanos == existing.created_at_nanos
                        && snap.snapshot_id.as_str() > existing.snapshot_id.as_str())
            }
        };
        if replace {
            head_by_repo.insert(snap.repository_id.as_str(), snap);
        }
    }

    // Pinned: bootstrap generation, applied-proposal stamp.
    let mut pinned: HashSet<String> = HashSet::new();
    for snap in snapshots {
        if snap.source.as_deref() == Some("Bootstrap") {
            pinned.insert(snap.snapshot_id.clone());
        }
    }
    for wu in work_units {
        if let Some(meta) = &wu.metadata {
            if let Some(applied) = meta.get("appliedSnapshotId") {
                if !applied.is_empty() {
                    pinned.insert(applied.clone());
                }
            }
        }
    }

    // Active: current head per repo, and non-terminal work units' branch
    // seed (latest snapshot with CreatedAt <= the work unit's own CreatedAt).
    let mut active: HashSet<String> = HashSet::new();
    for snap in head_by_repo.values() {
        active.insert(snap.snapshot_id.clone());
    }
    for wu in work_units {
        if is_terminal_status(wu.status) {
            continue;
        }
        let mut seed: Option<&ParsedSnapshot> = None;
        for snap in snapshots {
            if let Some(repo_filter) = &wu.repository_id {
                // Repo room should hold exactly one repository's data; if a
                // work unit names one explicitly, honor it. If it doesn't
                // (`RepositoryId` unset), fall back to matching any snapshot
                // in the room rather than dropping the work unit's
                // protection entirely — fail-safe bias.
                if &snap.repository_id != repo_filter {
                    continue;
                }
            }
            if snap.created_at_nanos > wu.created_at_nanos {
                continue;
            }
            let better = match seed {
                None => true,
                Some(s) => {
                    snap.created_at_nanos > s.created_at_nanos
                        || (snap.created_at_nanos == s.created_at_nanos
                            && snap.generation > s.generation)
                }
            };
            if better {
                seed = Some(snap);
            }
        }
        if let Some(s) = seed {
            active.insert(s.snapshot_id.clone());
        }
    }

    let work_units_by_id: HashMap<&str, &ParsedWorkUnit> =
        work_units.iter().map(|w| (w.work_unit_id.as_str(), w)).collect();

    // Assemble: Pinned > Active > Intermediate (age-out) precedence.
    let mut retained: HashSet<String> = HashSet::new();
    for snap in snapshots {
        if pinned.contains(&snap.snapshot_id) || active.contains(&snap.snapshot_id) {
            retained.insert(snap.snapshot_id.clone());
            continue;
        }
        let base_nanos = match &snap.work_unit_id {
            Some(wid) => match work_units_by_id.get(wid.as_str()) {
                Some(wu) if is_terminal_status(wu.status) => wu.updated_at_nanos,
                _ => snap.created_at_nanos,
            },
            None => snap.created_at_nanos,
        };
        let expires_at = base_nanos + (retain_intermediate_days as i128) * 86_400 * 1_000_000_000;
        if now_nanos < expires_at {
            retained.insert(snap.snapshot_id.clone());
        }
    }
    retained
}

// ─── Tree resolution ────────────────────────────────────────────────────────

fn resolve_snapshot_into(
    persistence: &dyn BlobPersistence,
    room_id: &str,
    snap: &ParsedSnapshot,
    live: &mut HashSet<String>,
) -> GcResult<()> {
    if let Some(entries) = &snap.tree_entries {
        // Legacy inline map (pre-Phase-1, or a producer that never adopted
        // cas-tree) — no tree blob to protect, just the file blobs it names.
        for hash_hex in entries.values() {
            live.insert(hash_hex.to_lowercase());
        }
        return Ok(());
    }

    let root_hash = hash_from_hex(&snap.tree_hash).ok_or_else(|| {
        GcError::Backend(format!(
            "room {room_id} snapshot {}: TreeHash {:?} is not canonical 64-lowercase-hex",
            snap.snapshot_id, snap.tree_hash
        ))
    })?;
    let walked = tree_walk::walk_tree(persistence, &root_hash).map_err(|e| {
        GcError::Backend(format!(
            "room {room_id} snapshot {}: tree walk from {} failed: {e}",
            snap.snapshot_id, snap.tree_hash
        ))
    })?;
    for h in walked {
        live.insert(h.to_hex());
    }
    Ok(())
}

fn collect_room_live_hashes(
    persistence: &dyn BlobPersistence,
    room_id: &str,
    resolved: &HashMap<String, (Vec<u8>, bool)>,
    is_repo_room: bool,
    retain_intermediate_days: i64,
    now_nanos: i128,
    live: &mut HashSet<String>,
) -> GcResult<()> {
    let data = parse_room_studio_data(room_id, resolved)?;

    // Rule (b), mirrored as-is (5.2 findings note: this is a documented
    // over-retention leak — op rows already folded into a snapshot via
    // compaction should eventually drop out of this contribution; that
    // refinement is a recorded follow-up, not this slice's job).
    for h in &data.op_referenced_hashes {
        live.insert(h.clone());
    }
    for h in &data.direct_blob_hashes {
        live.insert(h.to_hex());
    }

    let retained_snapshot_ids: HashSet<String> = if is_repo_room {
        classify_repo_room(&data.snapshots, &data.work_units, retain_intermediate_days, now_nanos)
    } else {
        // Non-repo room (legacy "studio" peer room, "workgroup", …): be
        // conservative — retain every valid snapshot forever rather than
        // attempt classification without the rest of that repo's data. See
        // module docs.
        data.snapshots.iter().map(|s| s.snapshot_id.clone()).collect()
    };

    for snap in &data.snapshots {
        if !retained_snapshot_ids.contains(&snap.snapshot_id) {
            continue; // expired Intermediate — its bytes are reclaimable
        }
        resolve_snapshot_into(persistence, room_id, snap, live)?;
    }

    Ok(())
}

// ─── Room enumeration + resolved-map fetch (resident + cold rooms) ─────────

async fn resolve_room_studio_map(
    rooms: &Rooms,
    room_id: &str,
) -> Result<HashMap<String, (Vec<u8>, bool)>, String> {
    let resident = {
        let map = rooms.rooms.read().await;
        map.get(room_id).cloned()
    };

    let resolved_with_meta: HashMap<String, (u64, [u8; 32], Vec<u8>, bool)> =
        if let Some(room) = resident {
            room.graph.read().await.resolve_with_meta()
        } else {
            // Cold room: replay persisted nodes into an ephemeral graph.
            // `load_room_nodes` returns nodes in the order they were
            // originally accepted (insertion/`seq` order), so a single-pass
            // batch apply is expected to succeed — any rejection here means
            // this room's history can't be faithfully reconstructed, which
            // must abort the whole collection (fail closed), not silently
            // scan a partial graph.
            let nodes: Vec<SyncNode> = rooms.persistence.load_room_nodes(room_id);
            let mut graph = StateGraph::new();
            let result = graph.apply_remote_batch(nodes);
            if !result.rejected.is_empty() {
                return Err(format!(
                    "{} node(s) rejected while replaying room history for GC scan",
                    result.rejected.len()
                ));
            }
            graph.resolve_with_meta()
        };

    Ok(resolved_with_meta
        .into_iter()
        .filter(|(k, _)| k.starts_with(STUDIO_KEY_PREFIX))
        .map(|(k, (_, _, v, is_blob))| (k, (v, is_blob)))
        .collect())
}

async fn enumerate_all_room_ids(rooms: &Rooms) -> Vec<String> {
    let mut ids: HashSet<String> = rooms.persistence.known_room_ids().into_iter().collect();
    let resident_ids: Vec<String> = rooms.rooms.read().await.keys().cloned().collect();
    ids.extend(resident_ids);
    let mut out: Vec<String> = ids.into_iter().collect();
    out.sort();
    out
}

/// Compute the full studio-domain live hash set across every room this
/// server holds (resident or cold), per the module docs' classification and
/// fail-closed rules. `retain_intermediate_days` mirrors
/// `RetentionPolicyOptions.RetainIntermediateDays`.
pub async fn collect_studio_live_hashes(
    rooms: &Rooms,
    retain_intermediate_days: i64,
) -> GcResult<HashSet<String>> {
    let now_nanos = system_time_to_nanos(std::time::SystemTime::now());
    let room_ids = enumerate_all_room_ids(rooms).await;
    let mut live: HashSet<String> = HashSet::new();

    for room_id in &room_ids {
        let resolved = resolve_room_studio_map(rooms, room_id)
            .await
            .map_err(|e| GcError::Backend(format!("room {room_id}: {e}")))?;
        let is_repo_room = room_id.starts_with("repo/");
        collect_room_live_hashes(
            rooms.persistence.as_ref(),
            room_id,
            &resolved,
            is_repo_room,
            retain_intermediate_days,
            now_nanos,
            &mut live,
        )?;
    }

    Ok(live)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(kind: &str, payload: Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"v": 1, "kind": kind, "payload": payload})).unwrap()
    }

    fn snapshot_payload(
        snapshot_id: &str,
        repo: &str,
        generation: i64,
        created_at: &str,
        tree_hash: &str,
        entries: Option<serde_json::Value>,
        source: Option<&str>,
        work_unit_id: Option<&str>,
    ) -> Value {
        // `TreeFormat` is intentionally omitted here: it isn't part of
        // `SnapshotPayloadRaw` (the parser decides inline-vs-cas-tree purely
        // from whether `TreeEntries` is present), so including it would only
        // add an unused field to these fixtures.
        serde_json::json!({
            "SnapshotId": snapshot_id,
            "RepositoryId": repo,
            "TreeHash": tree_hash,
            "Generation": generation,
            "CreatedAt": created_at,
            "WorkUnitId": work_unit_id,
            "Source": source,
            "TreeEntries": entries,
        })
    }

    fn work_unit_payload(
        work_unit_id: &str,
        status: i64,
        created_at: &str,
        updated_at: &str,
        repository_id: Option<&str>,
        metadata: Option<serde_json::Value>,
    ) -> Value {
        serde_json::json!({
            "WorkUnitId": work_unit_id,
            "Status": status,
            "CreatedAt": created_at,
            "UpdatedAt": updated_at,
            "RepositoryId": repository_id,
            "Metadata": metadata,
        })
    }

    #[test]
    fn parses_pascal_case_int_enum_payloads() {
        let bytes = envelope(
            KIND_REPOSITORY_SNAPSHOT,
            snapshot_payload(
                "SNAP-1", "repo-a", 0, "2026-01-01T00:00:00.0000000+00:00",
                "4b825dc642cb6eb9a060e54bf8d69288fbee4904ab00000000000000000000",
                None, Some("Bootstrap"), None,
            ),
        );
        let mut resolved = HashMap::new();
        resolved.insert("studio/repository-snapshot/v1/SNAP-1".to_string(), (bytes, false));
        let data = parse_room_studio_data("repo/repo-a", &resolved).expect("parses");
        assert_eq!(data.snapshots.len(), 1);
        assert_eq!(data.snapshots[0].source.as_deref(), Some("Bootstrap"));
    }

    #[test]
    fn entity_id_containing_slash_round_trips() {
        // Regression for the "base/{proposalId}" case the frozen schema
        // doc calls out — not a kind we classify, but the parser must still
        // resolve (kind, entityId) correctly for any studio/-prefixed key.
        let (kind, entity_id) = parse_studio_key("studio/work-unit/v1/base/WU-9").unwrap();
        assert_eq!(kind, KIND_WORK_UNIT);
        assert_eq!(entity_id, "base/WU-9");
    }

    #[test]
    fn malformed_envelope_json_is_an_error() {
        let mut resolved = HashMap::new();
        resolved.insert(
            "studio/repository-snapshot/v1/SNAP-BAD".to_string(),
            (b"not json".to_vec(), false),
        );
        let err = parse_room_studio_data("repo/repo-a", &resolved).unwrap_err();
        assert!(matches!(err, GcError::Backend(_)));
    }

    #[test]
    fn classify_bootstrap_is_pinned_and_head_is_active() {
        let snapshots = vec![
            ParsedSnapshot {
                snapshot_id: "gen-0".into(), repository_id: "repo-a".into(),
                tree_hash: "t0".into(), generation: 0,
                created_at_nanos: 0, work_unit_id: None,
                source: Some("Bootstrap".into()), tree_entries: None,
            },
            ParsedSnapshot {
                snapshot_id: "gen-1".into(), repository_id: "repo-a".into(),
                tree_hash: "t1".into(), generation: 1,
                created_at_nanos: 1_000_000_000, work_unit_id: None,
                source: None, tree_entries: None,
            },
        ];
        let retained = classify_repo_room(&snapshots, &[], 30, 2_000_000_000);
        assert!(retained.contains("gen-0")); // Pinned (Bootstrap)
        assert!(retained.contains("gen-1")); // Active (head)
    }

    #[test]
    fn classify_active_work_unit_seed_survives_past_retention_window() {
        let day_ns: i128 = 86_400 * 1_000_000_000;
        let snapshots = vec![
            ParsedSnapshot {
                snapshot_id: "gen-0".into(), repository_id: "repo-a".into(),
                tree_hash: "t0".into(), generation: 0,
                created_at_nanos: 0, work_unit_id: None,
                source: Some("Bootstrap".into()), tree_entries: None,
            },
            ParsedSnapshot {
                snapshot_id: "gen-seed".into(), repository_id: "repo-a".into(),
                tree_hash: "t-seed".into(), generation: 1,
                created_at_nanos: day_ns, work_unit_id: None,
                source: None, tree_entries: None,
            },
            ParsedSnapshot {
                snapshot_id: "gen-head".into(), repository_id: "repo-a".into(),
                tree_hash: "t-head".into(), generation: 2,
                created_at_nanos: 40 * day_ns, work_unit_id: None,
                source: None, tree_entries: None,
            },
        ];
        let work_units = vec![ParsedWorkUnit {
            work_unit_id: "WU-1".into(),
            status: 7, // Executing — non-terminal
            created_at_nanos: 2 * day_ns,
            updated_at_nanos: 2 * day_ns,
            repository_id: Some("repo-a".into()),
            metadata: None,
        }];
        // now = 100 days past epoch — gen-seed is >30 days old (past the
        // Intermediate window) but must still be retained because it seeds
        // an in-flight (non-terminal) work unit.
        let now_nanos = 100 * day_ns;
        let retained = classify_repo_room(&snapshots, &work_units, 30, now_nanos);
        assert!(retained.contains("gen-seed"), "seed must stay Active regardless of age");
        assert!(retained.contains("gen-head"), "head is always Active");
    }

    #[test]
    fn classify_expired_intermediate_is_not_retained() {
        let day_ns: i128 = 86_400 * 1_000_000_000;
        let snapshots = vec![
            ParsedSnapshot {
                snapshot_id: "gen-0".into(), repository_id: "repo-a".into(),
                tree_hash: "t0".into(), generation: 0,
                created_at_nanos: 0, work_unit_id: None,
                source: Some("Bootstrap".into()), tree_entries: None,
            },
            ParsedSnapshot {
                snapshot_id: "gen-stale".into(), repository_id: "repo-a".into(),
                tree_hash: "t-stale".into(), generation: 1,
                created_at_nanos: day_ns, work_unit_id: None,
                source: None, tree_entries: None,
            },
            ParsedSnapshot {
                snapshot_id: "gen-head".into(), repository_id: "repo-a".into(),
                tree_hash: "t-head".into(), generation: 2,
                created_at_nanos: 50 * day_ns, work_unit_id: None,
                source: None, tree_entries: None,
            },
        ];
        let now_nanos = 100 * day_ns; // gen-stale is 99 days old, no seed/head protection
        let retained = classify_repo_room(&snapshots, &[], 30, now_nanos);
        assert!(!retained.contains("gen-stale"));
        assert!(retained.contains("gen-0"));
        assert!(retained.contains("gen-head"));
    }

    #[test]
    fn applied_proposal_pin_protects_regardless_of_age() {
        let day_ns: i128 = 86_400 * 1_000_000_000;
        let snapshots = vec![
            ParsedSnapshot {
                snapshot_id: "gen-0".into(), repository_id: "repo-a".into(),
                tree_hash: "t0".into(), generation: 0,
                created_at_nanos: 0, work_unit_id: None,
                source: Some("Bootstrap".into()), tree_entries: None,
            },
            ParsedSnapshot {
                snapshot_id: "gen-applied".into(), repository_id: "repo-a".into(),
                tree_hash: "t-applied".into(), generation: 1,
                created_at_nanos: day_ns, work_unit_id: None,
                source: None, tree_entries: None,
            },
            ParsedSnapshot {
                snapshot_id: "gen-head".into(), repository_id: "repo-a".into(),
                tree_hash: "t-head".into(), generation: 2,
                created_at_nanos: 50 * day_ns, work_unit_id: None,
                source: None, tree_entries: None,
            },
        ];
        let mut metadata = HashMap::new();
        metadata.insert("appliedSnapshotId".to_string(), "gen-applied".to_string());
        let work_units = vec![ParsedWorkUnit {
            work_unit_id: "WU-done".into(),
            status: STATUS_MERGED, // terminal
            created_at_nanos: day_ns,
            updated_at_nanos: 2 * day_ns,
            repository_id: Some("repo-a".into()),
            metadata: Some(metadata),
        }];
        let now_nanos = 100 * day_ns;
        let retained = classify_repo_room(&snapshots, &work_units, 30, now_nanos);
        assert!(retained.contains("gen-applied"), "applied-proposal stamp must pin forever");
    }

    #[test]
    fn timestamp_parses_utc_and_offset_forms() {
        let a = parse_timestamp_nanos("2026-07-15T12:34:56.7654321+00:00").unwrap();
        let b = parse_timestamp_nanos("2026-07-15T12:34:56.7654321Z").unwrap();
        assert_eq!(a, b);
        let c = parse_timestamp_nanos("2026-07-15T13:34:56+01:00").unwrap();
        let d = parse_timestamp_nanos("2026-07-15T12:34:56Z").unwrap();
        assert_eq!(c, d);
    }

    #[test]
    fn timestamp_ordering_matches_civility() {
        let earlier = parse_timestamp_nanos("2026-01-01T00:00:00Z").unwrap();
        let later = parse_timestamp_nanos("2026-01-02T00:00:00Z").unwrap();
        assert!(later - earlier == 86_400 * 1_000_000_000);
    }

    #[test]
    fn non_repo_room_retains_every_valid_snapshot() {
        let bytes = envelope(
            KIND_REPOSITORY_SNAPSHOT,
            snapshot_payload(
                "SNAP-OLD", "repo-a", 0, "2000-01-01T00:00:00Z",
                "4b825dc642cb6eb9a060e54bf8d69288fbee4904ab00000000000000000000",
                None, None, None,
            ),
        );
        let mut resolved = HashMap::new();
        resolved.insert("studio/repository-snapshot/v1/SNAP-OLD".to_string(), (bytes, false));
        let data = parse_room_studio_data("studio", &resolved).unwrap();
        // Simulate the non-repo-room branch directly.
        let retained: HashSet<String> = data.snapshots.iter().map(|s| s.snapshot_id.clone()).collect();
        assert!(retained.contains("SNAP-OLD"));
    }

    #[test]
    fn work_unit_payload_parses_expected_shape() {
        let mut meta = serde_json::Map::new();
        meta.insert("appliedSnapshotId".to_string(), Value::String("gen-1".to_string()));
        let bytes = envelope(
            KIND_WORK_UNIT,
            work_unit_payload("WU-1", STATUS_COMPLETED, "2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z", Some("repo-a"), Some(Value::Object(meta))),
        );
        let mut resolved = HashMap::new();
        resolved.insert("studio/work-unit/v1/WU-1".to_string(), (bytes, false));
        let data = parse_room_studio_data("repo/repo-a", &resolved).unwrap();
        assert_eq!(data.work_units.len(), 1);
        assert_eq!(data.work_units[0].status, STATUS_COMPLETED);
        assert_eq!(
            data.work_units[0].metadata.as_ref().unwrap().get("appliedSnapshotId").unwrap(),
            "gen-1"
        );
    }

    #[test]
    fn repository_op_blob_refs_are_collected() {
        let bytes = envelope(
            KIND_REPOSITORY_OP,
            serde_json::json!({
                "OperationId": "OP-1", "RepositoryId": "repo-a", "ParentSnapshotId": null,
                "Kind": 1, "Path": "src/a.rs", "Timestamp": "2026-01-01T00:00:00Z",
                "OldBlobId": "aa".repeat(32), "NewBlobId": "bb".repeat(32),
            }),
        );
        let mut resolved = HashMap::new();
        resolved.insert("studio/repository-op/v1/OP-1".to_string(), (bytes, false));
        let data = parse_room_studio_data("repo/repo-a", &resolved).unwrap();
        assert_eq!(data.op_referenced_hashes.len(), 2);
    }

    // ─── Cross-repo contract vectors (finding #14 / slice 0.4) ─────────────
    //
    // Inline vector tests, not an external `tests/` crate — mirrors the
    // established precedent of `blob_layout_vectors_match_s3_key_derivation`
    // in `server/s3-blobs/src/lib.rs`, which reaches this module's private
    // items from inside `#[cfg(test)] mod tests` instead of exporting a
    // test-only `pub` surface. An earlier draft of this slice added
    // `pub fn parse_work_unit_status_for_vectors` / `pub struct
    // ParsedWorkUnitStatusVector` purely so an external `tests/` crate could
    // reach the parser; that widened the crate's public API for a test's
    // convenience and has been removed in favor of this inline suite.

    /// Test-only helper exercising the *real* production parse path for a
    /// `studio/work-unit/v1` envelope: the real `WorkUnitPayloadRaw`
    /// deserializer (so its `#[serde(rename = ...)]` PascalCase mapping is
    /// genuinely exercised) and the real `is_terminal_status`. Not `pub` —
    /// it lives entirely inside this test module.
    fn parse_work_unit_envelope_for_test(
        envelope_json: &[u8],
    ) -> Result<(String, i64, bool, Option<String>, Option<HashMap<String, String>>), String> {
        let envelope: Value = serde_json::from_slice(envelope_json).map_err(|e| e.to_string())?;
        let kind = envelope
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| "envelope missing string `kind`".to_string())?;
        if kind != KIND_WORK_UNIT {
            return Err(format!("expected kind `{KIND_WORK_UNIT}`, got `{kind}`"));
        }
        let payload = envelope
            .get("payload")
            .cloned()
            .ok_or_else(|| "envelope missing `payload`".to_string())?;
        let raw: WorkUnitPayloadRaw = serde_json::from_value(payload).map_err(|e| e.to_string())?;
        Ok((
            raw.work_unit_id,
            raw.status,
            is_terminal_status(raw.status),
            raw.repository_id,
            raw.metadata,
        ))
    }

    /// Asserts the Rust GC live-hash classifier's `WorkUnitStatus` parsing
    /// against the canonical vectors
    /// (`engine/commands/work-unit-status-vectors.v1.json`). Freezes finding
    /// #14 (`nodalmerge-studio/plans/blob-cas-remediation.md`, Phase 0 slice
    /// 0.4): the studio C# `WorkUnitStatus` enum's ordinals and PascalCase
    /// JSON casing were hand-mirrored here with no vector pinning either
    /// side. The .NET mirror is
    /// `nodalmerge-studio/tests/NodalMerge.Studio.Contracts.Tests/WorkUnitStatusVectorTests.cs`.
    /// To change a `WorkUnitStatus` ordinal/name, update the vectors and
    /// BOTH harnesses together — a drift here means the GC coordinator
    /// misclassifies live work-unit-seeded snapshots as garbage in
    /// production, instead of failing a test.
    #[test]
    fn terminal_status_ordinals_match_frozen_constants() {
        const VECTORS_JSON: &str =
            include_str!("../../../engine/commands/work-unit-status-vectors.v1.json");

        #[derive(Debug, serde::Deserialize)]
        struct VectorsFile {
            status_ordinals: Vec<StatusOrdinal>,
            terminal_status_names: Vec<String>,
        }
        #[derive(Debug, serde::Deserialize)]
        struct StatusOrdinal {
            name: String,
            ordinal: i64,
        }

        let file: VectorsFile = serde_json::from_str(VECTORS_JSON)
            .expect("engine/commands/work-unit-status-vectors.v1.json must parse");
        let by_name: HashMap<&str, i64> = file
            .status_ordinals
            .iter()
            .map(|s| (s.name.as_str(), s.ordinal))
            .collect();

        let mut failures = Vec::new();
        for name in &file.terminal_status_names {
            let Some(&ordinal) = by_name.get(name.as_str()) else {
                failures.push(format!("terminal status `{name}` has no entry in status_ordinals"));
                continue;
            };
            let bytes = envelope(
                KIND_WORK_UNIT,
                work_unit_payload(
                    "synthetic",
                    ordinal,
                    "2026-01-01T00:00:00.0000000+00:00",
                    "2026-01-01T00:00:00.0000000+00:00",
                    None,
                    None,
                ),
            );
            match parse_work_unit_envelope_for_test(&bytes) {
                Ok((_, _, is_terminal, _, _)) if is_terminal => {}
                Ok((_, _, is_terminal, _, _)) => failures.push(format!(
                    "status `{name}` (ordinal {ordinal}) round-tripped through the real parser as is_terminal={is_terminal}, expected true"
                )),
                Err(e) => failures.push(format!("status `{name}` (ordinal {ordinal}) failed to parse: {e}")),
            }
        }

        assert!(
            failures.is_empty(),
            "work-unit-status vectors drifted from studio_live_hashes.rs's terminal-status constants:\n{}",
            failures.join("\n")
        );
    }

    /// Exercises the real envelope -> `WorkUnitPayloadRaw` -> classification
    /// path (the exact thing GC calls in production) against each frozen
    /// envelope vector, covering ordinal, PascalCase field casing, optional
    /// fields, and the deliberately-non-terminal `DeadLettered` case.
    #[test]
    fn work_unit_envelope_vectors_match_the_real_parse_path() {
        const VECTORS_JSON: &str =
            include_str!("../../../engine/commands/work-unit-status-vectors.v1.json");

        #[derive(Debug, serde::Deserialize)]
        struct VectorsFile {
            work_unit_envelope_vectors: Vec<EnvelopeVector>,
        }
        #[derive(Debug, serde::Deserialize)]
        struct EnvelopeVector {
            id: String,
            envelope: Value,
            expected: ExpectedWorkUnit,
        }
        #[derive(Debug, serde::Deserialize)]
        struct ExpectedWorkUnit {
            work_unit_id: String,
            status_ordinal: i64,
            is_terminal: bool,
            repository_id: Option<String>,
            metadata: Option<HashMap<String, String>>,
        }

        let file: VectorsFile = serde_json::from_str(VECTORS_JSON)
            .expect("engine/commands/work-unit-status-vectors.v1.json must parse");
        let mut failures = Vec::new();

        for vector in &file.work_unit_envelope_vectors {
            let bytes = serde_json::to_vec(&vector.envelope).unwrap();
            match parse_work_unit_envelope_for_test(&bytes) {
                Ok((work_unit_id, status_ordinal, is_terminal, repository_id, metadata)) => {
                    if work_unit_id != vector.expected.work_unit_id {
                        failures.push(format!(
                            "vector `{}`: expected work_unit_id {:?}, got {:?}",
                            vector.id, vector.expected.work_unit_id, work_unit_id
                        ));
                    }
                    if status_ordinal != vector.expected.status_ordinal {
                        failures.push(format!(
                            "vector `{}`: expected status_ordinal {}, got {}",
                            vector.id, vector.expected.status_ordinal, status_ordinal
                        ));
                    }
                    if is_terminal != vector.expected.is_terminal {
                        failures.push(format!(
                            "vector `{}`: expected is_terminal={}, got {}",
                            vector.id, vector.expected.is_terminal, is_terminal
                        ));
                    }
                    if repository_id != vector.expected.repository_id {
                        failures.push(format!(
                            "vector `{}`: expected repository_id {:?}, got {:?}",
                            vector.id, vector.expected.repository_id, repository_id
                        ));
                    }
                    if metadata != vector.expected.metadata {
                        failures.push(format!(
                            "vector `{}`: expected metadata {:?}, got {:?}",
                            vector.id, vector.expected.metadata, metadata
                        ));
                    }
                }
                Err(e) => failures.push(format!("vector `{}` failed to parse: {e}", vector.id)),
            }
        }

        assert!(
            failures.is_empty(),
            "work-unit-status envelope vectors drifted from the real Rust parse path:\n{}",
            failures.join("\n")
        );
    }
}
