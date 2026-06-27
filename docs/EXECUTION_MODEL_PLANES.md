# Execution Model: Three Planes, One Explicit Bridge

Status: Draft — describes the engine state model as it exists today, plus the
one new bridge between its planes (`PromoteCheckpointToGraph`).

## Why this document exists

`HostEngine` (`host-core/src/engine.rs`) looks, from the outside, like a single
CRDT-backed room engine. In practice it currently maintains **three separate,
mostly-unconnected representations of room state**. Conflating them — assuming
that querying one tells you about the others — produces misleading results.
This document names them explicitly so future work (querying causal data,
deciding what to promote and when, building new client-facing capabilities)
starts from an accurate model rather than the convenient-sounding one.

## The three planes

### 1. Runtime State plane

Fields: `room_maps`, `room_texts`, `room_lists` (`engine.rs:85-89`).

Mutated directly by the everyday live commands: `MapSet`/`MapDelete`
(`engine.rs:645-718`), `TextInsert`/`TextDelete`, `ListPush`/`ListInsert`/etc.
No causality, no history — last-write-wins, in-memory, per-room. This is the
fast path: what a single client sees immediately after a write.

### 2. Canonical Checkpoint plane

Fields: `room_canonical_rows`, `room_canonical_seq`, `room_canonical_snapshots`,
`room_canonical_hash_index` (`engine.rs:101-104`).

A sequence-numbered, hashed snapshot is taken automatically alongside every
qualifying `MapSet`/`MapDelete` (`engine.rs:663-692`) — keys starting with `_`
are excluded, treated as internal/system metadata rather than checkpointed
content. Each `CanonicalSnapshotState` (`engine.rs:73-78`) carries a `sequence`,
the full row set at that point, a `canonical_hash`, and a `frontier` field.

This is already the system's "save point" concept — it's what
`HostCommand::BuildProjection`'s `target_checkpoint` selector
(`seq`/`hash`/`frontier`/`latest`) resolves against
(`engine.rs:2075`–onward, now factored into the shared
`resolve_canonical_checkpoint` helper). Until the promotion boundary below
existed, every snapshot's `frontier` field was a placeholder string —
`"seq:<n>"` (`engine.rs:682`) — not a real CRDT frontier. It satisfied
`BuildProjection`'s structural validation but carried no causal meaning.

### 3. Causal/Sync Graph plane

Field: `room_sync_graphs: HashMap<String, StateGraph>` (`engine.rs:95`).

The real CRDT engine (`core/src/graph.rs`): `frontier()`, `resolve_canonical()`,
`detect_conflicts()`, causal parents via `SyncNode::parents()`, IBF/MST-based
sync-diff. Fully implemented, zero stubs. Historically, the **only** way a node
entered this plane was `HostCommand::ImportPack`
(`engine.rs:1627-1684`, via `StateGraph::apply_remote_batch`) — the
replica-to-replica sync path. Live `MapSet`/`TextInsert`/`ListPush` commands
never touched it.

This means querying frontier/causal-parent/conflict data before any promotion
has happened would answer questions about pack-imported nodes only — not about
the live state most rooms actually accumulate. That's why `GetFrontier`,
`GetCausalParents`, `GetCanonicalResolution`, and `ComputeSyncDiff` (a set of
query commands scoped during this investigation) are deliberately **not**
implemented yet — see "Deferred" below.

## The bridge: `PromoteCheckpointToGraph`

Rather than routing every live write through `StateGraph` (a much larger
change — see "Rejected alternative" below), there is now one explicit,
manual, opt-in command that bridges plane 2 into plane 3:

`HostCommand::PromoteCheckpointToGraph { selector: Option<Value> }`
(`host-core/src/api.rs`), handled in `engine.rs` right after
`GetRecentConflicts`. Given a checkpoint selector (same `seq`/`hash`/`latest`
vocabulary as `BuildProjection`), it:

1. Resolves the target `CanonicalSnapshotState` via the shared
   `resolve_canonical_checkpoint` helper.
2. Computes a deterministic `promotion_id` — `Hash::of(room_id + seq +
   canonical_hash)` — independent of the graph's current frontier/lamport, so
   repeated promotion attempts of the same checkpoint are identifiable even if
   the frontier moved in between.
3. If a node with that `promotion_id` already exists in the graph (checked via
   a `_promotion/id` marker op), short-circuits and returns the existing
   node's identity — no duplicate node, no frontier movement.
4. Otherwise, builds one `Transaction` whose `ops` are one
   `Op::Map(MapOp::Set)` per row in the snapshot, plus two marker ops:
   `_promotion/source_snapshot_seq` (the originating seq, for traceability —
   any promoted node can be traced back to the checkpoint it materialized
   from) and `_promotion/id` (the idempotency key from step 2). `parents` is
   the graph's current frontier heads.
5. Wraps it as an unsigned `SyncNode` (consistent with the existing
   legacy/system-authored convention — `verify_signature()` already passes
   zero-signature nodes) and applies it via `StateGraph::apply_remote_batch`
   — the same entry point `ImportPack` uses.
6. Writes the real frontier hex back into the snapshot's `frontier` field,
   replacing the `"seq:<n>"` placeholder. Unpromoted snapshots are unaffected.
7. Returns `HostEvent::CheckpointPromoted { room_id, seq, node_id_hex,
   frontier_heads_hex }`.

Conceptually, this is a **synthetic CRDT origin event representing a
canonical-snapshot materialization** — not a "real" user-authored edit. It is
distinguishable from organically imported nodes by its marker ops and its
zero-signature/zero-author identity.

Reachable end-to-end via the existing generic `as_host_submit_command_json_ex`
FFI export (no new FFI surface needed) and the WebSocket protocol's
`"checkpoint.promote"` inbound message / `"checkpoint-promoted"` outbound
message (`RuntimeProtocolMapper.cs`), gated by the same `query.admin`
capability `projection.build` already requires.

## Rejected alternative: unifying the execution model

The alternative — routing `MapSet`/`TextInsert`/`ListPush` directly through
`StateGraph` so every live write is causal by construction — was considered
and explicitly rejected for now. It's a much larger change (engine rewrite,
ordering/backpressure semantics, a likely breaking change to every command
above the engine layer) in service of a property ("everything is
distributed-first, always") nothing currently requires. The promotion boundary
gets the same practical value — real causal data for state someone has
decided is worth promoting — without that cost, and without foreclosing a
future move to full unification if it ever becomes necessary.

## Deferred

- **`GetFrontier` / `GetCausalParents` / `GetCanonicalResolution` /
  `ComputeSyncDiff`** — read-only queries against the Causal/Sync Graph plane.
  Not implemented yet: before promotion existed, they would have silently
  returned empty/misleading data for any room that hadn't gone through
  `ImportPack`. Revisit once promotion has a real caller.
- **Automatic/implicit promotion** (e.g. on every Nth write, or tied to a
  domain event) — this phase is manual, explicit promotion only.
- **Signing promoted nodes** with a real host-authored key — unsigned for now,
  consistent with `SyncNode::new`'s existing convention.
