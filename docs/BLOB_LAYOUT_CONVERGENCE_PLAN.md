# Blob Layout Convergence Plan — one canonical CAS across runtimes

Status: implemented 2026-07-06. Canonical contract lives in
`docs/BLOB_STORAGE_LAYOUT.md`; both runtimes converged, version-bumped to
0.2.0 in lockstep, full workspace + solution regression green.
Prerequisite reading: the S5 schema-contract pattern
(`docs/PERSISTENCE_SCHEMA.md`) and the CAPCOMP vectors pattern
(`docs/CAPCOMP_PARITY_PLAN.md`) — this plan is the third application of the
same converge-then-lock playbook, applied to blob storage.

## 1. Problem (audit findings, 2026-07-06)

Blob content addressing agrees everywhere (Blake3, lowercase 64-hex), but
every layer above the hash diverged because the two runtimes were built at
different times for different deployments (Rust direct-S3 first; .NET
delegated-S3 for SpeechSlate later, shipped under deadline):

| Aspect | Rust file (`DirPersistence`) | .NET file (`FileBlobStoreProvider`) |
|---|---|---|
| Path | `<root>/blobs/<sanitized_room>/<hex>` (`server/server/src/store.rs:355-357,510`) | `<root>/<hex[..2]>/<hex>.blob` (`FileBlobStoreProvider.cs:60-69`) |
| Room-scoped | yes (duplicates shared blobs per room) | no (global CAS pool) |
| Tombstones | empty file `blob-tombstones/<room>/<hex>` (`store.rs:361-363,529-608`) | `.gc-tombstones/<hex>.tomb` (`FileBlobGcCoordinator.cs:52-126`) |

Neither runtime can read the other's tree, and each GC **silently ignores**
the other's files (Rust's `hash_from_hex` rejects anything not exactly
64 hex chars, `store.rs:626-629`; .NET globs `*.blob`). On S3: Rust
constructs keys itself (`<path_prefix><room>/<hex>`, pinned by test at
`server/s3-blobs/src/lib.rs:646-656`) while .NET's delegated resolver never
builds a key — the app (SpeechSlate API) owns it, and the live production
data is `blake3/<hash>` at bucket root. Both runtimes have a *delegate
presign protocol* but with different JSON shapes (Rust:
`{op, room_id, hash, size, ttl_seconds, algorithm, content_type}` in one
endpoint, `lib.rs:371-395`; .NET: `{room, namespace, hash, size,
contentType}` across separate put/get endpoints). The shared GC contracts
(`core/gc/src/contracts.rs`) operate on opaque strings and constrain none
of this.

## 2. Decisions (made 2026-07-06 — do not re-litigate)

1. **Blob storage must be runtime-interchangeable.** A deployment must be
   able to move between Rust/.NET (and future runtimes) over the same blob
   store. Layouts and the delegate wire protocol converge now.
2. **Global CAS pool, no room scoping, no sharding, no extension.** Full
   cross-room dedup. GC becomes union-of-all-rooms mark + global sweep
   (this is already .NET's shape — `FileBlobGcCoordinator` takes a
   caller-supplied live set over a global pool; Rust converges to it).
3. **Hash-algorithm path segment** (`blake3/`) for future algo agility —
   the OCI `blobs/<alg>/<hex>` convention. The live SpeechSlate S3 data
   already follows it (`assets/blake3/<hash>`, where `assets/` is the
   app-chosen prefix), so **production delegated data needs zero
   migration**. The prefix itself is app/operator configuration NodalMerge
   never constrains (see §3).
4. **Scope: layouts + delegate protocol only.** Each runtime keeps its
   current mode matrix (Rust: direct + delegate; .NET: delegate only).
   Adding .NET direct-S3 is a separately tracked follow-up, not this plan.
5. **File-store migration is automatic and one-time**, guarded by a marker
   file — no manual step, no per-startup legacy scan. Rust direct-S3
   migration is manual/documented (bucket listing shouldn't happen
   implicitly at startup; no known production deployments).
   Migration is *sufficient* for existing DAGs because blob references are
   content addresses end to end: nodes and wire messages carry blake3
   hashes only, and both `IBlobStoreProvider.TryGetBlobAsync(hashHex)` and
   Rust's `BlobPersistence` derive the path from the hash at access time.
   No path string is persisted anywhere in the DAG — verified 2026-07-06.
   (The one persisted-path exception in the ecosystem is SpeechSlate's
   assets DB storing `S3Key` strings; see Phase 4.)
6. **Everything versions to 0.2.0 in lockstep** (crates, NuGets, npm).
   0.1.x blob stores are declared incompatible with 0.2.x in release notes.

## 3. The canonical layout (Phase 0 extracts this into `docs/BLOB_STORAGE_LAYOUT.md`)

The canonical layout is the **relative** part: `<algorithm>/<hex>`.
Everything to its left is deployment/app configuration that NodalMerge
deliberately does not care about — the blob root for file stores (Rust
`DirPersistence`: `<store_root>/blobs/`; .NET `FileBlobStorageOptions`:
the configured root), the `path_prefix` for direct S3, and for delegated
S3 whatever prefix the app chooses (SpeechSlate uses `assets/`; any value
is conformant). The contract is only that within one deployment, every
reader/writer of a given store agrees on the same root/prefix.

```
Blob:            <blob_root>/blake3/<64-lowercase-hex>          (flat; no extension)
Tombstone:       <blob_root>/.tombstones/blake3/<hex>           (empty file; mtime = tombstone time)
Layout marker:   <blob_root>/.layout-v2                         (empty file; presence = migrated/canonical)
S3 direct key:   <prefix>blake3/<64-lowercase-hex>              (prefix configurable; default "blobs/", unchanged from today)
```

Rules:
- **The algorithm path segment is required, not decorative.** A CAS
  reference is really the pair `(algorithm, digest)` — the layout mirrors
  the OCI image-layout convention (`blobs/sha256/<hex>`), making the store
  self-describing. This is load-bearing here because sha256 and blake3
  digests are both 64 hex chars and therefore indistinguishable as bare
  names; the segment is what makes any future hash migration (see below)
  mechanically possible.
- **The hash algorithm is protocol-fixed (blake3), not user-configurable.**
  Blob hashes are DAG node identity: content addresses that ed25519
  signatures bind to, that MST/IBF sync converges on, and that stores
  verify on read. A per-deployment algorithm choice would fork the wire
  protocol, not customize storage. If the algorithm ever changes, it is a
  one-time, protocol-versioned migration (its own plan) — the algo path
  segment and the `algorithm` field in the delegate protocol exist to make
  that future migration possible, not to make the algorithm a knob. No
  pluggable hashing interface ships now: both runtimes already funnel
  hashing through one seam (`nodalmerge_core::Hash::of`; .NET receives
  hashes over wire/FFI), and an interface with one implementation is
  scaffolding, not extensibility.
- Producers always write lowercase hex. Readers/GC treat any name under
  `blake3/` that is not exactly 64 lowercase hex chars as foreign and skip
  it (Rust's existing strict `hash_from_hex` behavior becomes the contract
  on both sides).
- Room ids appear **nowhere** in blob paths or S3 keys. The room-id
  sanitization helpers stop being used for blob paths (keep them if other
  path types — e.g. topology sidecars — still need them; implementer
  verifies).
- GC mark/sweep: live set = union of live blob hashes across **all rooms
  known to persistence** (not just resident rooms — see Phase 1 risk).
  Two-phase tombstone + grace window semantics are unchanged from today's
  Rust design (`docs/delegated-storage-gc.md`).

**Delegate presign protocol v1** (single endpoint, op in body — Rust's
current shape, with .NET's field spellings reconciled):

```
POST <presign_endpoint>            (auth header per existing config)
{
  "op": "put" | "get",
  "room": "<room-id>",             // metadata only; MUST NOT influence the key
  "hash": "<64-lowercase-hex>",
  "algorithm": "blake3",
  "size": <bytes, optional>,
  "ttl_seconds": <n>,
  "content_type": "<mime, optional>",
  "namespace": "<optional metadata; never part of the key>"
}
→ 200 { "url": "<presigned url>" }
```

Renames this implies: Rust `room_id` → `room` (keep everything else);
.NET gains `op`/`algorithm`/`ttl_seconds`, folds its separate put/get
endpoints into one endpoint + `op`, and `contentType` → `content_type`.
The SpeechSlate API's presign endpoint must accept v1 in the same release
window (its *key layout* is already canonical, only the request shape
changes).

## 4. Implementation phases

### Phase 0 — Contract doc
Write `docs/BLOB_STORAGE_LAYOUT.md` from §3 (layout, hash rules, tombstone
semantics, marker, migration behavior, delegate protocol v1, GC union
semantics). This doc is the source of truth the vectors encode; the plan
you are reading stays as rationale/history.

### Phase 1 — Rust file store (`server/server/src/store.rs`)
1. `DirPersistence` blob read/write → `<blob_root>/blake3/<hex>`; delete
   `blobs_dir_for(room)` usage for blobs.
2. Tombstones → `<blob_root>/.tombstones/blake3/<hex>`.
3. **GC signature change**: `BlobPersistence::blob_gc_sweep(room_id, live,
   grace)` → `blob_gc_sweep(live, grace)` (global). The GC driver in
   `room.rs` computes the union live set before sweeping.
   **Correctness risk — cold rooms**: today's per-room GC only ever
   touched resident rooms' dirs; a global sweep must not collect blobs
   referenced only by rooms that aren't currently loaded. The driver must
   enumerate all room ids from persistence (sqlite `nodalmerge.db` knows
   them), obtain each room's live blob hashes (resident state where
   loaded; replay otherwise), and union. This makes GC a heavier,
   less-frequent background operation — acceptable; the grace window
   already protects against sweep-vs-concurrent-upload races. If replay
   cost proves prohibitive later, a persistent blob-reference index is the
   escape hatch (out of scope now).
4. **Auto-migration on open**: if `.layout-v2` marker absent and legacy
   room-dir layout detected — for each `blobs/<room>/<hex>` file, verify
   content hashes to `<hex>` (Rust already verifies on read), move to
   `blake3/<hex>` (skip if already present — that *is* the dedup), move
   non-conforming files to `<blob_root>/.migration-skipped/` rather than
   deleting, discard legacy tombstones (worst case: a garbage blob
   survives until the next GC), write marker last (crash-safe: re-running
   is idempotent). Log a summary line.
5. Update `blob_tamper_rejected` and `sanitize_safe_chars` tests; add
   migration tests (fresh store, legacy store, half-migrated store).

### Phase 2 — .NET file store (`FileBlobStoreProvider`, `FileBlobGcCoordinator`)
1. `GetPath` → `<root>/blake3/<hex>` (drop 2-char shard and `.blob`).
2. Tombstones → `<root>/.tombstones/blake3/<hex>` (empty file, not
   `.tomb`), coordinator's enumeration updated to the strict-64-hex rule.
3. Same marker + auto-migration on provider construction (move
   `<shard>/<hex>.blob` → `blake3/<hex>`, verify hash, skip-dir for
   non-conforming, marker last). .NET must ship a Blake3 verify for
   migration — check what the host already uses for hashes; if nothing
   manages Blake3 in .NET today, verification can go through the existing
   `nodalmerge_host_ffi` hashing surface or the Blake3 NuGet already
   referenced by Studio (`Blake3` 2.2.1 in Studio's
   Directory.Packages.props — confirm the host repo's options).
4. Update `FileBlobGcCoordinatorTests` and add migration tests mirroring
   Phase 1's.

### Phase 3 — Rust S3 direct (`server/s3-blobs`)
1. `key_for` → `<prefix>blake3/<hex>`; drop the room segment and the
   room-sanitization path; default `path_prefix` stays `"blobs/"` (the
   prefix is pure config — see §3).
2. Update the pinned key-format test
   (`key_layout_round_trips_funny_room_ids` becomes a hash-only round-trip).
3. GC/list operations updated for the new key shape.
4. Manual migration documented in `docs/operator.md` (one `aws s3 mv`
   loop or a short script in `scripts/`); no auto-migration.

### Phase 4 — Delegate protocol v1

Production reality (verified in PWASoundboard.Api, 2026-07-06): there are
**two separate presign implementations** today, and only one is in live
use —

- `BlobsPresignController` (`api/blobs/presign`): server-to-server for the
  Rust `S3Auth::Delegate` caller. Static shared-secret bearer auth.
  Accepts Rust's exact request shape. Keys:
  `<S3:Prefix default "assets/">blake3/<hash>` when `algorithm` is sent
  (Rust always sends it), else legacy `activesync/<sanitized_room>/<hash>`.
  GET additionally requires the hash to be a registered asset whose stored
  `S3Key` string matches (an app-side authorization policy coupled to the
  assets DB).
- `BlobPresignController` (`v1/blobs/presign-put`/`presign-get`): ApiKey
  auth, camelCase `{hash, contentType}` → `{url, expiresAtEpochSeconds}`.
  Keys: `blake3/<hash>` at bucket root — **this is where the live data
  is**. The .NET host's delegated resolver posts `{room, namespace, hash,
  size, contentType}` here and the extra fields are silently dropped by
  the model binder, which is why `room`/`namespace` already have no effect
  on keys in production.

Work:
1. Rust delegate request: `room_id` → `room` (everything else already
   matches v1).
2. .NET delegated resolver: single endpoint + `op`, add `algorithm` and
   `ttl_seconds`, `contentType` → `content_type`, keep `namespace` as
   optional metadata.
3. SpeechSlate API: converge on ONE presign endpoint accepting protocol
   v1 (evolving `BlobsPresignController`'s shape is the shorter path since
   it already parses v1 minus the `room` rename), with key derivation
   fixed to `<app-chosen-prefix><algorithm>/<hash-lower>` derived from
   only the request's `algorithm` + `hash`. SpeechSlate keeps its
   `assets/` prefix (the live data's home); what gets deleted is the
   *inconsistency*: the legacy `activesync/<room>/` branch and the fact
   that its two controllers currently write to two different prefixes
   (`assets/blake3/` vs bare `blake3/`) — pick one prefix, list the bucket
   for objects under the other, and `aws s3 mv` the strays. Whether the
   asset-registration GET gate applies is app policy — the protocol
   permits any 403/404 denial; keep or drop it per SpeechSlate's needs,
   but if kept, stored `S3Key` values registered under old prefixes must
   be reconciled. Auth scheme (shared-secret vs ApiKey) is deployment
   config, not protocol — pick one and note it in the endpoint's docs.
   Ships in the same release window as the 0.2.0 packages; the old
   endpoints can 410 or forward during the window.

### Phase 5 — Parity lock (the S5/CAPCOMP move)
1. `engine/commands/blob-layout-vectors.v1.json`: given a hash (+ prefix
   for S3), the expected relative file path, tombstone path, and S3 key;
   rejection vectors for uppercase/short/long/non-hex names; delegate
   protocol v1 request-shape vectors (golden JSON for put and get).
2. Harnesses: Rust (in `server/server` or a small shared location the
   store and s3-blobs tests can both reach) and .NET
   (`BlobLayoutParityTests.cs`, vectors linked into the test csproj like
   `command-registry.json` and `capcomp-vectors.v1.json`).
3. **Cross-runtime interop test**: a golden fixture blob tree committed
   under test data (canonical layout, two known blobs + one tombstone);
   both runtimes' tests read it and assert content round-trips; each
   runtime's writer tests assert produced paths equal the vectors. This is
   the "Rust writes / .NET reads" guarantee without needing both runtimes
   in one process.
4. Extend `.github/workflows/control-plane-capability-parity.yml` (or a
   sibling job) with the new paths and test steps.

### Phase 6 — Version + docs + release
1. Bump every crate, NuGet, and npm package to **0.2.0**; release notes
   state plainly: *0.1.x blob stores are not readable by 0.2.x and vice
   versa; file stores migrate themselves on first open; direct-S3
   operators run the documented migration; delegated-S3 data is already
   canonical.*
2. Docs sweep: `ARCHITECTURE.md` §5 persistence table,
   `docs/deployment.md` (backup paths: `blobs/blake3/`),
   `docs/operator.md`, `docs/delegated-storage-gc.md` (path examples),
   `docs/integration.md` (`path_prefix` default change).
3. Consumer validation per the established flow: pack 0.2.0-local, verify
   docs demo host + Studio, then publish and repoint (the
   `pack-local-nuget.ps1` / `Directory.Packages.props` routine from the
   0.1.5 release).

## 5. Non-goals

- No .NET direct-S3 mode, no Rust behavioral changes to delegate *mode
  selection* — mode-matrix parity is a tracked follow-up.
- No change to blob wire messages (WS `blob-set`/`request-upload`/etc.),
  node storage, or the GC abstraction traits' semantics beyond the sweep
  signature.
- No persistent blob-reference index (noted as the escape hatch if
  union-replay GC is ever too slow).
- No change to SpeechSlate's S3 key layout (already canonical).

## 6. Risk notes for the implementer

- **Cold-room GC is the correctness landmine** (Phase 1.3). A sweep that
  unions only resident rooms will delete blobs belonging to idle rooms.
  Test this explicitly: create room A with a blob, evict/close it, run GC
  with room B resident, assert A's blob survives.
- Migration must be idempotent and crash-safe: marker written last;
  re-entry skips already-moved files; hash-verify before adopting a file
  into the CAS; never delete non-conforming files (quarantine dir).
- Rust's `blob_tamper_rejected` test and the S3 key test are the two
  places the old layout is pinned — they must move with the change, not
  be deleted.
- If any Rust-delegate traffic ever hit the SpeechSlate API, blobs may
  exist under `assets/blake3/<hash>` or `activesync/<room>/<hash>` in the
  bucket (the `BlobsPresignController` key branches). Before declaring the
  bucket canonical, list for those prefixes; if present, `aws s3 mv` them
  into `blake3/` alongside the direct-S3 migration doc.
- `.NET` Blake3 for migration verification needs a dependency decision
  (FFI surface vs Blake3 NuGet) — small, but don't hand-roll.
- The known `archive_profile_002` server test flake (passes in isolation)
  predates this work; don't chase it.
- Windows/WSL share `target/`: `cargo clean --release` after any WSL
  build before a Windows release build (E0514 otherwise).
