# Blob Storage Layout Contract (v3)

Canonical, cross-runtime specification for how NodalMerge stores
content-addressed blobs. Both the Rust server (`DirPersistence`,
`nodalmerge-s3-blobs`) and the .NET host (`FileBlobStoreProvider`,
`S3DelegatedBlobUrlResolverProvider`) implement this contract; the vectors
in `engine/commands/blob-layout-vectors.v1.json` pin it against drift.
Rationale and history live in `docs/BLOB_LAYOUT_CONVERGENCE_PLAN.md`.

v3 = v2 + §8 (optional zstd at-rest encoding). A v2 store is a valid v3
store; a v3 store containing no `.zst` entries is byte-identical to a v2
store. No migration or marker is required.

## 1. Content address

- Hash algorithm: **Blake3**, 32 bytes, rendered as **64 lowercase hex
  characters**.
- The hash is the sole identity of a blob. No path, key, room id, or
  namespace is ever part of that identity — they are all storage-location
  metadata layered on top.

## 2. Canonical relative layout

Every store defines a **blob root** (a filesystem directory for file
stores; a key prefix for S3 stores). Relative to that root:

```
<blob_root>/blake3/<64-lowercase-hex>              blob content, flat, no extension
<blob_root>/.tombstones/blake3/<hex>                empty marker file; mtime = tombstone time
<blob_root>/.layout-v2                              empty marker file; presence = migrated to this layout
```

The `blake3/` segment is the hash algorithm, not a directory chosen by
convenience — see §5.

**The blob root itself is deployment configuration, not part of this
contract.** Rust `DirPersistence` roots blobs at `<store_root>/blobs/`;
.NET `FileBlobStorageOptions` uses its configured root directly; Rust
`nodalmerge-s3-blobs` roots at its `path_prefix` (default `"blobs/"`); a
delegated S3 app can choose any prefix (e.g. SpeechSlate's `assets/`). Two
stores interoperate only if they agree on the same root/prefix — that
agreement is an operator/deployment concern, not something the software
enforces.

## 3. Naming rules

- Writers always produce lowercase hex.
- Readers and GC treat any entry under `blake3/` whose name is not exactly
  64 lowercase hex characters as **foreign** and skip it silently (never
  delete, never error). This is what makes the layout forward-compatible
  with a future `blake3/` sibling for a different algorithm, and what lets
  a store safely coexist with unrelated files an operator might place in
  the same root.
- No room id, namespace, or other request metadata is ever encoded into a
  blob path or S3 key.

## 4. Tombstone / GC semantics

Two-phase mark-and-sweep, unchanged in spirit from the pre-v2 Rust design:

1. A sweep that finds a blob not in the caller-supplied *live* set and has
   no tombstone yet creates one (`.tombstones/blake3/<hex>`, empty file,
   write time = "now").
2. A sweep that finds a blob not in the live set **with** a tombstone
   older than the configured grace window deletes both the blob and its
   tombstone.
3. A sweep that finds a blob **in** the live set with a leftover tombstone
   clears the tombstone (handles the set→unset→reset race).
4. `grace = 0` collapses phases 1–2 into one pass.

**The live set is global**, not scoped to one room: it is the union of
every blob hash referenced by any node in any room known to the store —
resident/loaded rooms and non-resident/cold rooms alike. A live-set
computation that only considers currently-loaded rooms will incorrectly
mark and eventually delete blobs belonging to idle rooms; this was safe
under the old per-room-directory layout (a sweep only ever touched its own
room's directory) and is **not** safe under the flat global layout.

S3 stores collapse the two phases into one (list-and-delete against the
live set) since object versioning, where enabled, is the operator's own
grace mechanism.

## 5. Why the algorithm segment is required

A content-addressed reference is really the pair `(algorithm, digest)` —
not just the digest. This layout mirrors the OCI image-layout convention
(`blobs/sha256/<hex>`), which exists for exactly this reason: Blake3 and
SHA-256 digests are both 64 hex characters and otherwise indistinguishable
as bare filenames. The segment is what makes a future hash-algorithm
migration mechanically possible instead of ambiguous.

**The algorithm itself is protocol-fixed (`blake3`), not a per-deployment
choice.** Blob hashes are DAG node identity — the content addresses that
`RoomToken`/node signatures bind to and that MST/IBF sync converges on. A
mixed-algorithm deployment doesn't customize storage, it forks the sync
protocol between peers. If the algorithm ever changes, that is a
deliberate, one-time, protocol-versioned migration (its own plan), which
is exactly what this segment and the `algorithm` field in the delegate
protocol (§7) exist to make possible.

## 6. File-store migration

On first open after upgrading, a file store checks for `.layout-v2` in its
blob root:

- **Present** → already canonical; proceed normally, no scan.
- **Absent** → run a one-time migration: locate every blob file under the
  legacy layout, verify its content still hashes to its claimed name,
  move it to `blake3/<hex>` (a file that already exists at the
  destination is left alone — that collision *is* the cross-room
  deduplication), quarantine anything that fails verification or doesn't
  parse as a legacy blob path into a `.migration-skipped/` directory
  instead of deleting it, then write `.layout-v2` **last**. Legacy
  tombstones are discarded (worst case a still-referenced-but-unreferenced
  blob survives until the next GC pass, which is the same outcome as any
  normal tombstone-grace race).

This makes migration idempotent and crash-safe: re-running before the
marker is written simply repeats the scan (already-moved files are found
at their new location and skipped), and nothing is destructive.

Direct-S3 stores are migrated manually (documented in `docs/operator.md`)
— listing a bucket implicitly at process startup is not something this
contract does.

## 7. Delegated presign protocol v1

One endpoint, operation in the request body:

```
POST <presign_endpoint>
{
  "op": "put" | "get",
  "room": "<room-id>",              // metadata only; MUST NOT affect the key
  "hash": "<64-lowercase-hex>",
  "algorithm": "blake3",
  "size": <bytes>,                  // present for "put"
  "ttl_seconds": <n>,
  "content_type": "<mime>",         // optional
  "namespace": "<app metadata>"     // optional; MUST NOT affect the key
}
→ 200 { "url": "<presigned url>" }
```

The app-side key derivation is `<its own prefix><algorithm>/<hash>` — the
`room`/`namespace` fields are metadata the app may log or authorize
against, never key inputs.

## 8. At-rest content encoding (v3)

A blob with hash `<hex>` exists as **exactly one** of:

```
<blob_root>/blake3/<hex>          identity encoding: raw bytes; Blake3(these bytes) = <hex>
<blob_root>/blake3/<hex>.zst      zstd encoding: a single zstd frame; Blake3(DECOMPRESSED bytes) = <hex>
```

- **Invariant (normative): the hash is always Blake3 of the uncompressed
  bytes.** Compression is a storage/transport encoding, never an identity
  change. This is the same rule that keeps any future chunk/delta encoding
  honest (§1 still holds: the hash is the sole identity).
- The `.zst` suffix is the only encoding signal. No sidecar metadata files
  (they break single-file temp+rename atomicity), no `zstd/` subdirectory
  (breaks one-place-to-look and GC enumeration), and **no content
  sniffing** (a user's *content* can itself legitimately be a zstd file).
- Naming rules (§3) amendment: `<64-lowercase-hex>.zst` is **canonical in
  v3**. Everything else under `blake3/` remains foreign-skip. A v2 reader
  treats `.zst` entries as foreign and skips them — degraded (the blob
  appears missing to that reader) but never corrupt and never deleted.
- If both files exist for one hash (writers never do this), readers prefer
  the identity file; GC may delete the `.zst` duplicate.
- Tombstones are keyed by **bare hex** exactly as in §4
  (`.tombstones/blake3/<hex>`), covering whichever encoding exists. GC
  strips a `.zst` suffix before parsing an entry name as a hash; the
  live-set check uses the bare hash.
- Readers that verify on read verify the **decompressed** bytes. A corrupt
  frame or a decompressed-hash mismatch is treated as a missing blob
  (plus a warning), never served.
- Writers compress at their discretion; parity does **not** require two
  stores to hold the same blob under the same encoding. Recommended
  defaults: compression **on** for server-side durable stores (zstd level
  3), **off** for peer-local caches (local disk is cheaper than
  per-materialize CPU). Recommended skip heuristic (guidance, not
  contract): skip declared compressed-media content types (`image/*`,
  `video/*`, `audio/*`, `application/zip|gzip|zstd|x-7z*|wasm`); sample
  the first min(64 KiB, len) and store raw if the sampled ratio > 0.98;
  never compress blobs < 4096 bytes.
- S3 stores: the client compresses before upload; key =
  `<prefix>blake3/<hex>.zst` with a `contentEncoding` object metadata
  entry. (Seam only until Phase 4 builds it.)
  **Slice 4.3 clarification (2026-07-15):** the reference presign backend
  (`nodalmerge-s3-blobs::S3BlobStore::key_for`) always signs
  `<prefix>blake3/<hex>` — the bare hex key, never a `.zst` variant — for
  both GET and PUT, regardless of client-side compression; there is no
  key-selection step. The `.zst`-suffixed-key half of this bullet therefore
  remains an unbuilt seam. What slice 4.3's client
  (`S3DirectBlobStoreProvider`) actually does, and what makes the
  `contentEncoding object metadata entry` half true today: it PUTs to the
  one bare-hex key with a standard HTTP `Content-Encoding: zstd` request
  header when it compressed, and reads that same header back verbatim on a
  later presigned GET — S3-compatible object stores persist and return
  `Content-Encoding` as object metadata unconditionally (they don't
  negotiate the way an app server does), so this is sufficient to convey
  the encoding without any key-suffix scheme. A future change that has the
  presign backend itself choose between `<hex>` and `<hex>.zst` keys (e.g.
  to let an operator distinguish encodings by listing the bucket) is still
  open, and would need presign-time knowledge of whether the upcoming PUT
  will be compressed.
- HTTP surface: see `docs/BLOB_HTTP_SURFACE.md` §content-encoding — a
  server MAY serve stored `.zst` bytes with `Content-Encoding: zstd` when
  the client advertises `Accept-Encoding: zstd`; the client decompresses
  **before** hash verification and caching.

Vectors: the `encoding_vectors_v3` and `name_conformance_vectors_v3`
arrays in `engine/commands/blob-layout-vectors.v1.json` pin the encoded
path derivation, v3 name-conformance, tombstone keying, and the
both-files-exist preference. Pre-v3 harnesses ignore those arrays.

## 9. Non-goals of this contract

- It does not require every runtime to support every storage mode (file,
  S3-direct, S3-delegated) — see the convergence plan §2.4 for the current
  mode matrix and what's tracked as a follow-up.
- It does not cover peer-local/offline client caching
  (`peer/runtime-local`), which is a different persistence surface with no
  cross-runtime interop requirement.
- It does not specify a pluggable hashing interface. One algorithm, one
  seam (`nodalmerge_core::Hash::of` on the Rust side).
