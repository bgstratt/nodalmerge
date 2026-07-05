# Canonical Node-Store Schema (plan S5)

Status: canonical as of 2026-07-05. Both runtimes write and read this shape;
the split schemas that preceded it were inadvertent drift (decision recorded
in docs/RESTRUCTURE_AND_PARITY_PLAN.md §7).

Writers/readers bound by this contract:

- Rust: `server/stores/mongo` (`MongoNodeStore`)
- .NET: `hosts/dotnet/src/NodalMerge.Host.Composition/MongoNodeStoreProvider.cs`

## Collection: `accepted_nodes`

```jsonc
{
  "_id":          "<room_id>:<node_id_hex>",   // deterministic; $setOnInsert only
  "room_id":      "<room_id>",
  "node_id_hex":  "<64-hex node id>"           // single-node records (Rust server)
               // | "pack:<sha256-hex>"        // pack records (.NET host)
  "payload":      BinData,                      // postcard pack_nodes bytes, 1..n nodes
  "payload_kind": "pack",
  "causal_parent_node_ids": ["<hex>", ...],
  "frontier_hash_hex": null | "<hex>",
  "applied":      bool,
  "is_tombstone": bool,
  "accepted_at_utc": ISODate,                   // $setOnInsert only — hydration sort key
  "eligible_for_compaction_at_utc": null | ISODate,
  "updated_at_utc": ISODate
}
```

Indexes (identical names/options on both runtimes — creating with different
names on the same keys would conflict at boot):

- `ux_room_node` — unique on `(room_id, node_id_hex)`; this is the record's
  real identity regardless of `_id` form
- `ix_room_compaction_eligibility` — `(room_id, eligible_for_compaction_at_utc)`

## Rules

1. **Write = idempotent upsert.** `updateOne(filter: {room_id, node_id_hex},
   {$set: <fields>, $setOnInsert: {_id, accepted_at_utc}}, upsert: true)`.
   Never `replaceOne`/`insert` (immutable `_id` breaks against legacy docs;
   inserts race the unique index).
2. **`accepted_at_utc` never moves** once set — it is the hydration sort key
   (`accepted_at_utc asc, node_id_hex asc`) and re-writes must not reorder
   history. `updated_at_utc` always advances.
3. **`payload` is a postcard pack of 1..n nodes.** The Rust store writes
   single-node packs; the .NET host writes whole inbound/snapshot packs.
   Readers must accept any node count and must tolerate documents whose
   payload fails to unpack (skip + warn, never crash hydration).
4. **Legacy tolerance.** Documents may carry an ObjectId `_id` (pre-S5 .NET
   writes) or the old Rust fields (`bytes` instead of `payload`, a `seq`
   field, binary `node_id`). Readers fall back to `bytes` when `payload` is
   absent; writers upgrade legacy docs in place via the filter-based upsert.
   Old docs without `accepted_at_utc` sort first (missing-before-values),
   which is correct — they predate the schema.
5. **Retired:** the Rust `nodalmerge_seq` counter collection and `seq` field.
   Ordering comes from rule 2; graph hydration is causal-order tolerant on
   both runtimes (`apply_remote_batch` / bridge pack import).

## Verified by

- `server/stores/mongo/tests/mongo_round_trip.rs` — live conformance suite
  plus cross-runtime interop: hydrates a .NET-shaped multi-node pack record,
  asserts Rust-written docs carry every canonical field, asserts idempotency
  and `accepted_at_utc` stability.

## Upgrade path

No migration required. Mixed-era collections work: legacy docs stay
readable and are upgraded field-wise on their next write; fresh docs get
deterministic `_id`s. To fully retire legacy shapes later, a one-off script
can rewrite `bytes`→`payload` and drop `nodalmerge_seq`.
