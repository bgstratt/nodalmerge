# NodalMerge Quickstart

A 10-minute, end-to-end walkthrough: stand up a server, connect a browser
client, and understand the three data types (Map, Text, Blob). For the full
API reference see [sdk.md](sdk.md); for deployment see
[deployment.md](deployment.md) and [self-host.md](self-host.md).

---

## 1. Run the server

```bash
# Native (from the repo root)
cargo run -p nodalmerge-server -- \
  --store ./data \
  --metrics-addr 127.0.0.1:9090 \
  --idle-timeout 300
```

Or via Docker:

```bash
docker run -p 7878:7878 -v $PWD/data:/data nodalmerge/server
```

What each flag does:

| Flag | Default | Purpose |
|---|---|---|
| `--store <dir>` | in-memory | SQLite + file-per-blob persistence. Omit for dev/testing. |
| `--metrics-addr <ip:port>` | off | Prometheus `/metrics` listener (G7). |
| `--idle-timeout <secs>` | `300` | Evict rooms with zero connected peers after N seconds. `0` disables. |
| `--broadcast-capacity <N>` | `512` | Per-room broadcast ring size (G1). |
| `--peer-rate-nodes <N>` | `200` | Per-peer node-rate ceiling (G3). `0` disables. |
| `--peer-rate-bytes <MiB>` | `4` | Per-peer ingress byte-rate ceiling (G3). |
| `--blob-gc-interval <secs>` | `0` | Periodic blob GC sweep (G4). `0` disables. |
| `--blob-gc-grace <secs>` | `86400` | Tombstone grace before a blob is deleted. |

The server logs `NodalMerge server listening on ws://127.0.0.1:7878/ws/<room>`
once ready. Hit `http://127.0.0.1:9090/metrics` for Prometheus scrape.

---

## 2. Connect a client

```js
import { createDoc } from './sdk.js';

const doc = await createDoc({
  serverUrl: 'ws://localhost:7878',
  room:      'my-first-room',
});

console.log('connected as', doc.pubkeyHex);
```

`createDoc` is the only entry point you need. It handles: WebSocket reconnect,
Ed25519 identity, the handshake (frontier + IBF/MST diff), catch-up pack, and
steady-state broadcast.

**Identity persistence.** Save `doc.authorSeed` (a 32-byte Uint8Array) to
localStorage and pass it back next time so the same user keeps the same
pubkey:

```js
const seedHex = localStorage.getItem('seed');
const authorSeed = seedHex ? hexToBytes(seedHex) : undefined;

const doc = await createDoc({ serverUrl, room, authorSeed });
localStorage.setItem('seed', bytesToHex(doc.authorSeed));
```

---

## 3. Pick the right data type

NodalMerge ships **three** primitives. Use the one whose merge semantics match
your data shape — you cannot change this per-key later.

### Map — LWW key/value store (objects, configs, references)

Use when last-writer-wins is correct: settings, user profiles, a "currently
selected item" pointer, anything object-shaped where simultaneous writes to
the same key should pick one winner deterministically.

```js
const users = doc.map('users');

users.set('alice', { name: 'Alice', color: '#f43' });
users.set('alice', { name: 'Alice', color: '#3af' });  // overwrites
const alice = users.get('alice');                      // { name, color }
users.delete('alice');
users.all();                                            // { /* live keys */ }

users.onChange(({ key, value, source }) => rerender());
```

Tie-break: higher Lamport wins; on a tie, higher pubkey wins. Offline edits
still converge — both sides see the same winner once they re-sync.

### Text — per-character RGA (collaborative editing)

Use for any string that multiple users may edit concurrently: documents,
comments, chat messages, code. Correctly handles offline edits, concurrent
typing, and cursor interleaving.

```js
const notes = doc.text('notes/welcome');

notes.insert(0, 'Hello, ');
notes.insert(7, 'world!');
notes.delete(5, 2);           // delete ", "
notes.toString();             // "Helloworld!"

notes.onChange(() => render(notes.toString()));
```

Known limits (tracked in PLAN.md):
- No "move" op — reordering paragraphs is delete+insert today.
- No rich-text spans (bold/italic ranges). Store formatting in a parallel Map
  with offset ranges if you need it now.
- No run compression — a 10 KB paste is 10 000 ops. Fine in practice, but
  noticeable on the wire for very large pastes.

### Blob — content-addressed binary (images, audio, files)

Use for anything larger than ~1 KB that isn't textually merge-able: images,
audio clips, PDFs, file uploads. Blobs are stored **content-addressed** by
their Blake3 hash. The hash goes in a Map value; the bytes ride a separate
CAS channel.

```js
const assets = doc.map('assets');

// Writer side
const bytes = new Uint8Array(await file.arrayBuffer());
const hash  = await assets.setBlob('logo.png', bytes);
// assets.get('logo.png') now returns the hash; the bytes are in the CAS.

// Reader side (any peer)
const hash  = assets.get('logo.png');
const bytes = await assets.getBlob('logo.png');   // fetches on demand
```

Properties:
- **Deduped automatically** — two identical uploads share one hash, one copy.
- **On-demand** — peers don't pull blob bytes until something asks for them.
- **Garbage collected** — when no Map still references a hash, the server
  tombstones the blob and deletes it after `--blob-gc-grace` seconds.

---

## 4. Presence (cursors, typing indicators)

Ephemeral side-channel, never stored in the DAG. Perfect for "who's here
right now" and live cursors.

```js
doc.presence.set({ name: 'Brad', cursor: { x: 312, y: 44 } });

doc.presence.onJoin  (({ pubkey, state }) => addCursor(pubkey, state));
doc.presence.onUpdate(({ pubkey, state }) => moveCursor(pubkey, state));
doc.presence.onLeave (({ pubkey })       => removeCursor(pubkey));

doc.presence.others();   // [{ pubkey, state, lastSeen }]
```

Presence is automatically heartbeat'd every 15 s and swept at 45 s of silence.
It evaporates on disconnect — if you want persistence, write to a Map instead.

---

## 5. React to changes

```js
doc.onChange((ev) => {
  // ev.source:   'local' | 'remote'
  // ev.type:     'map' | 'text' | 'pack' | 'blob'
  // ev.transport?: 'ws' | 'webrtc'    (remote only)
  // ev.from?:      <pubkey hex>        (remote only)
  saveToIndexedDb();
  rerender();
});
```

For finer-grained events, subscribe on the individual handle:
`doc.map('users').onChange(cb)` only fires for that namespace.

---

## 6. Offline / reconnect

The SDK reconnects automatically with exponential backoff. Every merge is
content-addressed, so replays and duplicate deliveries are free (`import_pack`
dedupes on node id). What you need to do:

1. Persist local state to IndexedDB on every `doc.onChange`. The demo
   (`web/demo.js`) is the reference implementation.
2. On startup, hydrate from IndexedDB **before** calling `createDoc` — the
   SDK will merge the server's view into your local DAG during the handshake.
3. Nothing special for offline writes — the SDK buffers them and flushes on
   reconnect.

---

## 7. Locking a room (optional, for production)

Rooms are public by default. To require signed tokens:

1. Generate a room seed (32 bytes) and keep it on your auth server.
2. Pass the seed as `roomSeed` when calling `createDoc` from a trusted
   context (server-rendered page, Electron main process), or use the
   **JWT bridge** (`jwt-bridge/` crate) to mint tokens from your existing
   auth provider (Clerk/Supabase/Auth0). See [self-host.md](self-host.md).

```js
const doc = await createDoc({
  serverUrl, room,
  roomSeed:        bytesFromServer,         // don't ship this to browsers
  tokenCaps:       ['write:intent/**', 'read:world/**'],
  tokenExpirySecs: 3600,
});
```

Expired tokens disconnect the client with close code `4002`; the SDK's
`onDisconnect` fires and your app can refresh the token and reconnect.

---

## 8. What to read next

## 9. Operational Replay With Policy Timeline

Use this when you need replay verification to match policy-at-time behavior.

Replay with timeline from a JSON file:

```bash
nodalmerge-server replay ./pack.b64 --policy-timeline ./timeline.json
```

Replay with inline timeline JSON:

```bash
nodalmerge-server replay ./pack.b64 --policy-timeline-json '[{"effective_lamport":2,"policy":{"rules":[{"path_glob":"protected/**","can_write":[[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1]],"can_read":[],"can_derive":[]}],"default":"DenyAll"}}]'
```

Supported payload shapes:

1. Bare array of timeline entries.
2. Wrapped object with `timeline` property.

```json
[
  {
    "effective_lamport": 2,
    "policy": {
      "rules": [
        {
          "path_glob": "protected/**",
          "can_write": [[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1]],
          "can_read": [],
          "can_derive": []
        }
      ],
      "default": "DenyAll"
    }
  }
]
```

```json
{
  "timeline": [
    {
      "effective_lamport": 2,
      "policy": {
        "rules": [],
        "default": "AllowAll"
      }
    }
  ]
}
```

Timeline entries use the same core model as `PolicyTimelineEntry`, and replay
dispatches through the same timeline path used by core tests.

- **[sdk.md](sdk.md)** — complete API reference, all options, edge cases.
- **[deployment.md](deployment.md)** — ops guide: backups, tuning, metrics.
- **[self-host.md](self-host.md)** — 5-minute Docker + JWT bridge walkthrough.
- **[operations-inventory.md](operations-inventory.md)** — cross-surface operation index (SDK, wire, server, core, auth, GC) with gap-analysis matrix.
- **[../PLAN.md](../PLAN.md)** — engineering plan + decision log (why things
  are the way they are).
- **[../web/demo.js](../web/demo.js)** — full reference client: IndexedDB,
  blob caching, reconnect UI, presence.

## 10. Control-Plane Capability Parity (Operator Quick Reference)

`.github/workflows/control-plane-capability-parity.yml` checks that the Rust
server (`server/src/ws_handler.rs`) and the .NET host
(`RuntimeProtocolMapper.cs`) require the same capability for the same
control-plane commands (`set-policy`, `set-room-key`, `start-tick`, etc.),
each asserted against the same canonical table via a real test on its own
side — see `server/tests/control_plane_capability_parity.rs` and
`nodalmerge-host/tests/NodalMerge.DotNetHost.Tests/ControlPlaneCapabilityParityTests.cs`.
It runs on pushes/PRs touching those files, or on demand:

```bash
gh workflow run control-plane-capability-parity.yml
```

It replaces the old `authz-conformance-nightly` workflow, which compared two
placeholder oracles that never independently derived a result (the Rust
runner echoed each vector's declared `expected` back as `actual`, and the
.NET step mapped vector IDs to unrelated pre-existing unit tests) and so
never caught a real regression.

For full artifact schema and promotion-gate semantics, see
[AUTHORIZATION_CONFORMANCE_SPEC.md](AUTHORIZATION_CONFORMANCE_SPEC.md).
