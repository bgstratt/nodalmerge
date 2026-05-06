# Schema Migrations (App Layer)

ActiveSync treats `Op::Map` values as opaque bytes. Schema evolution is a
product concern implemented in app payloads and key conventions.

## Rules of the road

1. Ops are forever
- Historical ops are immutable and remain part of history until compaction.
- Compaction preserves resulting state, not replayable original op payloads.

2. Prefer additive changes
- Add optional fields first.
- Keep old readers functional until all active clients understand the new shape.

3. Renames are copy-then-tombstone
- Write the new key with equivalent value.
- Tombstone the old key in a separate write.
- Avoid in-place semantic repurposing of an existing key.

4. Deletion means tombstone, not absence
- In LWW semantics, a delete is an explicit write that wins by `(lamport, author)`.
- Do not infer deletion from missing fields alone.

5. Version inside the value
- Include an app-level `version` field in the serialized payload.
- Readers should parse old versions and migrate forward at read boundaries.

## Recommended rollout pattern

1. Add support for reading old + new versions.
2. Start writing the new version.
3. Backfill hot data lazily on write or read-repair.
4. Tombstone deprecated keys once compatibility window closes.
5. Remove legacy readers only after observed fleet convergence.

## Anti-patterns

- Reusing an existing key for a different semantic meaning.
- Dropping old fields without a compatibility window.
- Depending on strict wall-clock order instead of CRDT/LWW semantics.
