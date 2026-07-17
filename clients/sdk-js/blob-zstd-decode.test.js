// blob-zstd-decode.test.js — slice 3.3 (blob-cas-remediation, finding #6).
//
// Drives the presigned-GET blob store path the way Safari/Node receive it:
// `fetch` hands back the RAW zstd frame an s3-direct uploader stored in the
// bucket (no transparent Content-Encoding decode, unlike Chrome 123+/FF 126+).
// Pre-3.3 the SDK hashed those raw bytes, the store rejected them, and the
// blob never landed. Post-3.3, `storeFetchedBlobBytes` (the exact function
// `fetchBlobViaUrl` uses — exported from web/sdk.js, not a copy) falls back
// to a wasm-bridge zstd decode and re-verifies.
//
// Runs against the REAL bridge (../web/pkg — wasm-pack output of
// clients/bridge-wasm), so BLAKE3 verification and the zstd decoder are the
// production ones, not stubs. The zstd fixtures are the shared Phase 0
// cross-runtime goldens (engine/commands/zstd-interop-vectors.v1.json):
// frames produced by the actual production encoders — .NET
// ZstdSharp.Compressor.Wrap (what S3DirectBlobStoreProvider uploads) and
// Rust zstd::stream::encode_all — never by the ruzstd decoder under test.

import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

import * as sdk from "../web/sdk.js";

// RED shim: pre-3.3 web/sdk.js has no storeFetchedBlobBytes export; this
// fallback is a verbatim copy of what fetchBlobViaUrl did instead
// (`store.store_blob_bytes(hash, bytes)`, nothing else), so running these
// tests against the pre-fix SDK fails BEHAVIORALLY — store rejects the
// frame, blob never lands — rather than on a missing import.
const storeFetched =
  sdk.storeFetchedBlobBytes ??
  ((store, hash, bytes) => {
    store.store_blob_bytes(hash, bytes);
    return bytes;
  });

const wasmBytes = await readFile(
  new URL("../web/pkg/nodalmerge_bridge_bg.wasm", import.meta.url)
);
await sdk.ready(wasmBytes);

const vectorsUrl = new URL(
  "../../engine/commands/zstd-interop-vectors.v1.json",
  import.meta.url
);
const vectors = JSON.parse(await readFile(vectorsUrl, "utf8"));

async function loadFixture(id) {
  const entry = vectors.fixtures.find((f) => f.id === id);
  assert.ok(entry, `fixture ${id} present in zstd-interop-vectors.v1.json`);
  const bytes = new Uint8Array(
    await readFile(new URL(`../../engine/commands/${entry.fixture_path}`, import.meta.url))
  );
  return {
    frame: bytes,
    hash: entry.decoded_plaintext_blake3,
    plaintextLen: entry.decoded_plaintext_len,
  };
}

// The .NET ZstdSharp frame is the primary case: it is byte-for-byte what the
// s3-direct uploader (S3DirectBlobStoreProvider.PutBlobAsync) puts in the
// bucket when compression is opted in.
const dotnet = await loadFixture("dotnet-compressor-wrap-with-size-header");
// The Rust encode_all frame pins the no-content-size-header shape (finding
// #4's lesson: the two encoders emit different frame headers).
const rust = await loadFixture("rust-encode-all-no-size-header");

function freshStore() {
  const seed = new Uint8Array(32).fill(7);
  return new sdk.SyncStore(seed);
}

test("oracle pin: store_blob_bytes rejects a raw zstd frame (the Safari/Node failure mode)", () => {
  // This is why the decode fallback exists: the store hashes whatever bytes
  // it is given, and a zstd frame never matches the plaintext's BLAKE3.
  const store = freshStore();
  assert.throws(() => store.store_blob_bytes(dotnet.hash, dotnet.frame));
  assert.equal(store.has_blob(dotnet.hash), false);
});

test("zstd frame from the .NET s3-direct uploader decodes, verifies, and lands", () => {
  const store = freshStore();
  const stored = storeFetched(store, dotnet.hash, dotnet.frame);
  assert.equal(stored.length, dotnet.plaintextLen);
  assert.equal(store.has_blob(dotnet.hash), true);
  // has_blob(hash)=true after store_blob_bytes IS the hash-match assertion —
  // the store BLAKE3-verifies before accepting.
  assert.equal(store.get_blob_bytes(dotnet.hash).length, dotnet.plaintextLen);
});

test("zstd frame with no content-size header (Rust encoder / pre-flip bucket) also lands", () => {
  // Compression now defaults OFF on the .NET side, but buckets filled before
  // the flip still hold zstd frames — the decode path stays for them.
  const store = freshStore();
  const stored = storeFetched(store, rust.hash, rust.frame);
  assert.equal(stored.length, rust.plaintextLen);
  assert.equal(store.has_blob(rust.hash), true);
});

test("raw bytes still take the verify-raw-first path (browser already decoded, or compression off)", () => {
  // Simulate Chrome/FF transparently decoding before we see the bytes: hand
  // the helper the PLAINTEXT. It must store on the first attempt — this is
  // the common case and must not regress.
  const decoder = freshStore();
  const plaintext = storeFetched(decoder, dotnet.hash, dotnet.frame);

  const store = freshStore();
  const stored = storeFetched(store, dotnet.hash, plaintext);
  assert.equal(stored, plaintext);
  assert.equal(store.has_blob(dotnet.hash), true);
});

test("garbage without the zstd magic fails with the original integrity error", () => {
  const store = freshStore();
  const garbage = new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8]);
  assert.throws(() => storeFetched(store, dotnet.hash, garbage));
  assert.equal(store.has_blob(dotnet.hash), false);
});

test("a valid zstd frame that decodes to the WRONG content still fails verification", () => {
  // Decode succeeding is never enough — the decoded bytes must match the
  // requested hash. Feed the .NET frame under the Rust fixture's hash.
  const store = freshStore();
  assert.throws(() => storeFetched(store, rust.hash, dotnet.frame));
  assert.equal(store.has_blob(rust.hash), false);
});
