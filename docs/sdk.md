# ActiveSync SDK

A Firebase/Replicache-style document API over the ActiveSync CRDT engine.
Thin JS wrapper around the low-level `SyncStore` — no core changes.

## Install

The SDK ships alongside the WASM bridge under `web/` in this repo. Copy
`web/sdk.js`, `web/sdk.d.ts`, and the `web/pkg/` folder (produced by
`wasm-pack build bridge --target web --out-dir ../web/pkg`) into your app.

## Quick start

```js
import { createDoc } from './sdk.js';

const doc = await createDoc({
  serverUrl: 'ws://localhost:7878',
  room:      'demo-room-1',
});

const world   = doc.map('world');
const intents = doc.map('intent');
const notes   = doc.text('notes/welcome');

world.set('player1', { x: 10, y: 20 });
notes.insert(0, 'Hello, collaborators.');

doc.onChange(ev => {
  console.log(ev.source, ev.type, ev.path ?? ev.namespace);
});
```

## API

### `createDoc(opts) → Promise<Doc>`

| Option           | Type         | Default        | Notes |
|------------------|--------------|----------------|-------|
| `serverUrl`      | `string`     | — (optional)   | Without trailing `/ws/<room>` — the SDK appends it. Omit to run in local-only mode; call `doc.attachServer(...)` to attach later. |
| `room`           | `string`     | — (required)   | Room id. Transport boundary; see PLAN.md decision log. |
| `authorSeed`     | `Uint8Array` | random 32 B    | Persist this to keep identity across sessions. |
| `roomSeed`       | `Uint8Array` | `null`         | Room private-key seed (C3). If set, a capability token is signed on every connect. |
| `tokenCaps`      | `string[]`   | `[]`           | Path-scoped grants, e.g. `["write:intent/**"]`. Empty = full access. |
| `tokenExpirySecs`| `number`     | `86400`        | Token TTL. |
| `tokenProvider`  | `() => Promise<string>` | — | Optional async function to mint/refresh a JWT for server attach/connect. Used by `attachServer` to authenticate when moving from local-only to server-backed mode. |
| `autoConnect`    | `boolean`    | `true`         | If false, call `doc.connect()` manually. |
| `subscribe`      | `string[]`   | `["**"]`       | F3a: glob patterns for client-side materialization. See [Subscriptions](#subscriptions-f3a). |
| `presenceHeartbeatMs` | `number` | `15000`        | Interval between presence re-broadcasts. `0` disables. |
| `presenceStaleMs`| `number`     | `45000`        | Treat a silent peer as gone after this long. |
| `logger`         | `function`   | `console`      | `(level, ...args) => void`. |

### `Doc`

- `doc.map(namespace)` → `MapHandle` — LWW-Map over `<namespace>/<key>` paths.
- `doc.text(key)` → `TextHandle` — per-character RGA with tombstones.
- `doc.list(key)` → `ListHandle<T>` — ordered list with fractional-index CRDT semantics (F8).
- `doc.onChange(cb)`, `doc.onConnect(cb)`, `doc.onDisconnect(cb)`, `doc.onError(cb)` — each returns an unsubscribe function.
- `doc.connect()`, `doc.disconnect()`, `doc.close()`.
- `doc.pubkeyHex`, `doc.authorSeed`, `doc.isConnected`.
- `doc.store` — escape hatch to the raw `SyncStore` for advanced use (speculative reads, frontier inspection).
 - `doc.computeBlake3(bytes)` → `Promise<string>` — helper to compute canonical Blake3 id for `bytes` (hex).
 - `doc.attachServer(serverUrl, options?)` → `Promise<void>` — in-place transport swap and replay local state to a server. `options` may include `{ mode: 'immediate' | 'wait-for-welcome' | 'manual' | 'hybrid' }`.

### `MapHandle`

- `set(key, value)` — `value` is any JSON-serializable value.
 - `setBlob(key, bytes)` — stores `bytes` in the CAS and sets the key to the blob hash. The SDK computes a canonical Blake3 id for `bytes` (via `computeBlake3`) and calls the WASM `store.set_blob(...)`. If the WASM-derived id and the computed Blake3 disagree, the SDK prefers the computed Blake3 id and stores the bytes under the canonical `assets/blake3/<hex>` layout. When the server advertises `supports_direct_blob_io` (F6) and `bytes.length >= 1 MiB`, the SDK requests a presigned S3 PUT URL and uploads directly to object storage; smaller blobs and pre-F6 servers fall back transparently to the WebSocket `blob-upload` path. Blob bytes are persisted to the browser IndexedDB (`activesync-sdk` DB, `blobs` store) for offline-first use.
- `get(key)` — returns the JSON value or `undefined`.
- `getBlob(hashOrKey)` — returns the blob bytes, looking the key up if needed. Under F6, the server may respond to a `blob-request` with `blob-redirect` payloads carrying presigned GET URLs; the SDK fetches the URL, caches it for reuse, and falls back to WebSocket transport on `403`/expired or any network failure.
- `delete(key)`.
- `all()` — returns every key currently resolved under this namespace, stripped of the prefix.
- `onChange(cb)`.

### `TextHandle`

- `insert(pos, str)`, `delete(pos, len = 1)`.
- `toString()`.
- `onChange(cb)`.

### List status

The fractional-index `List` API is not yet stabilized in this SDK release.
While the engine contains list primitives, the high-level `ListHandle`
surface (stable ordering, gestures, and production-ready helpers) is still
experimental and may change. For production UIs (for example, a soundboard
button layout) use a `Map` with an explicit ordering field (for example
`map('buttons').set(id, { order: 'a0', ... })`) and implement fractional-index
sorting on the client. We will document migration steps when `ListHandle` is
stabilized.

### `doc.presence` (ephemeral awareness)
Ephemeral side-channel for cursors, selections, typing indicators, viewport
scroll position, etc. Never stored in the DAG, never replayed on reconnect.
Synced peer-to-peer via the room's relay; falls away when a peer disconnects.

```js
doc.presence.set({ name: 'Brad', color: '#f43' });
doc.presence.set({ cursor: { x: 312, y: 44 } }); // merges into previous state

doc.presence.onJoin(({ pubkey, state }) => console.log('join', pubkey, state));
doc.presence.onUpdate(({ pubkey, state }) => redrawCursor(pubkey, state));
doc.presence.onLeave(({ pubkey, reason }) => removeCursor(pubkey));

const everyone = doc.presence.others(); // [{ pubkey, state, lastSeen }]
```

- **`set(patch)`** merges `patch` into current local state and broadcasts.
  Heartbeats fire on an interval so joiners eventually see you even if they
  missed the first broadcast.
- **`clear()`** stops broadcasting and tells peers you're gone.
- **`others()`** / **`get(pubkey)`** return the latest seen state per remote peer.
- Leave reasons: `'disconnect'` (server peer-left), `'stale'` (no heartbeat
  within `presenceStaleMs`), `'cleared'` (peer called `clear()`).

## Data model

- **Map = LWW with Lamport + pubkey tie-break.** Correct for object-shaped state
  where last writer wins is acceptable (configs, cursors, selected item IDs).
  Not suitable for counters, sets, or ordered lists — use the appropriate type.
- **Text = per-character RGA with tombstones.** Correct under concurrent typing,
  interleaving, and offline convergence. Known limits tracked in PLAN.md: no
  move operation, no rich-text spans, no run compression for large pastes.
- **List = not yet.** Fractional-index collection with move op is tracked as
  post-F in PLAN.md. Today, reorderable collections should be modeled as a Map
  with an explicit `order` field per item or by using fractional keys from the
  app layer.

## What the SDK handles for you

- WebSocket connection, heartbeat, and exponential-backoff reconnect.
- Ed25519 identity + capability token (C3) when a `roomSeed` is provided.
- Handshake: frontier (A6), capability negotiation (A7), IBF (B1), MST (B2).
- Delta sync — only new nodes since the last successful pack are sent.
- Blob CAS round-trip: local uploads on reconnect, on-demand fetch of missing
  blobs after every merge.
- Event dispatch (`onChange` with `source: 'local' | 'remote'`).

## Subscriptions (F3a)

A **subscription** is a list of glob patterns controlling which paths the SDK
materializes for the local app. It narrows `map.all()`, `map.get()`,
`map.onChange`, and gates `doc.text(key)` at construction.

```js
const doc = await createDoc({
  serverUrl, room,
  subscribe: ['world/**', 'notes/welcome'],
});

doc.map('world').all();       // only entries under world/**
doc.text('notes/welcome');    // OK
doc.text('chat/42');          // throws — outside subscription

doc.subscribe(['**']);        // widen at runtime
doc.onSubscriptionChange(({ patterns }) => rerender());
doc.isSubscribed('world/player1'); // true
```

**Glob semantics:**
- `**` matches any path (including `/`).
- `foo/**` matches `foo` and any descendant.
- `*` matches within a single path segment (no `/`).
- Literals match literally.

**Caveats:** As of F3b the server filters relayed packs against each peer's
subscription — non-matching nodes are dropped on the server side for both the
initial catch-up and live broadcasts. Subscription updates are pushed live via
a `{type:"subscribe", patterns}` message when you call `doc.subscribe(...)`.
Local **writes** are still not filtered by the subscription (path authority is
Policy's job — see A5 / `RoomToken.capabilities`); the subscription is a
bandwidth/visibility tool only. Sentinel-prefixed keys (E2EE envelopes,
snapshot meta) always pass through. `doc.onChange` still fires for raw remote
packs — subscribe on the specific `MapHandle` / `TextHandle` for filtered
events.

## What the SDK does not handle (yet)

- **WebRTC P2P** — D2 is shipped in the bridge but not surfaced here yet. The
  SDK currently uses WebSocket only; the demo app (`web/demo.js`) contains
  experimental WebRTC glue.
- **Client-side subscription filters** — F3a. The server may filter relayed
  packs per-peer, but client-side selective materialization is limited to the
  subscription pattern set in `createDoc` and live updates; per-write filtering
  is still policy's job.
- **Full speculative/canonical surface** — The bridge exposes `read_speculative`
  and `read_canonical`; the SDK currently exposes the raw `doc.store` escape
  hatch for advanced consumers. A higher-level public API for speculative vs
  canonical reads may be added later.

Note on persistence: this SDK release includes a built-in IndexedDB persistence
adapter used for Free/offline flows. The runtime persists node packs to
`activesync-sdk` → `nodes` and stores raw blob bytes in `activesync-sdk` →
`blobs`. `createDoc` will hydrate from IndexedDB when available; call
`doc.close()` to clear any live persistence hooks. The persistence is opt-out
— if you prefer a different storage layer, you can replace or disable it in
your app.

## Compatibility

- Wire format is backwards-compatible with any server running the same wire
  schema as the bridge package version. Capability negotiation (A7) means a
  peer that doesn't support IBF/MST will silently fall back to the legacy pack
  path.
- The SDK reuses the low-level `SyncStore` unchanged — no protocol divergence
  between SDK users and apps that drop to the bridge directly.

## Example: two tabs

Open two browser tabs pointing at the same snippet. They will converge under
concurrent edits because each `doc` holds its own DAG and the server relays
node packs between them.

```js
const doc = await createDoc({ serverUrl: 'ws://localhost:7878', room: 'demo' });
const notes = doc.text('shared');
document.querySelector('textarea').oninput = (e) => {
  // Replace the whole text with each keystroke — the SDK will send a minimal
  // delta over the wire via RGA ops.
  // (Production apps should use a diff-driven insert/delete path; see demo.js.)
};
notes.onChange(() => {
  document.querySelector('textarea').value = notes.toString();
});
```

## See also

- [PLAN.md](../PLAN.md) — full engineering plan, including the decision log
  and the Phase F roadmap this SDK is part of.
- [ARCHITECTURE.md](../ARCHITECTURE.md) — engine internals.
- [`web/demo.js`](../web/demo.js) — full-fidelity demo using the low-level
  bridge directly; will be ported onto `createDoc` once the SDK proves out.

## **Attach Server Modes**

- **Immediate Replay (default):** attach swaps in a server transport, connects,
  and immediately replays local nodes and then pushes blob bytes. Pros: fast
  convergence and quick server backup of local edits. Cons: may attempt direct
  blob uploads or registration before the server's capabilities or a fresh
  token are observed, which can trigger fallbacks or extra retries.
- **Wait-for-Welcome (recommended for strict handoff):** attach connects but
  waits for the server "welcome" / capability negotiation (the handshake that
  advertises `supports_direct_blob_io`, protocol version, etc.) before
  replaying nodes or uploading blobs. Pros: avoids unnecessary presign attempts
  and respects server capabilities and auth state. Cons: adds a small delay to
  the replay path.
- **Manual-Control Mode:** attach only establishes the transport and returns —
  the app explicitly calls `store.export_nodes_missing_from(...)` and
  `transport.sendBlobs(...)` when it decides to replay. Use this when the app
  needs to show a UI confirmation, perform additional auth steps, or sequence
  uploads to avoid bursts.
- **Hybrid (capability-aware) Replay:** attach waits for welcome, replays the
  node pack immediately, but defers large blob uploads until `supports_direct_blob_io`
  is confirmed; otherwise it uses the websocket `blob-upload` fallback. This
  minimizes wasted presign attempts while keeping node convergence fast.

Token & auth notes:
- `attachServer` uses the SDK's `tokenProvider` / `ensureFreshToken` hooks when
  the transport needs a token for the handshake. For best results ensure the
  user is logged in and a valid token can be minted before calling
  `attachServer`. If your token minting is asynchronous or slow, prefer the
  Wait-for-Welcome or Manual-Control modes so the app can surface progress to
  the user while authentication completes.

When to pick which mode:
- Quick UX and optimistic uploads: Immediate Replay.
- Strict server capability / auth correctness: Wait-for-Welcome.
- App-driven sequencing or user confirmation: Manual-Control.

## Room IDs and Free users

Do not reuse user-provided human-readable room names as the canonical room ID
for server-backed rooms. For Free/offline users the SDK should be initialized
with a globally-unique identifier so that later attaching to the server does
not accidentally merge two distinct users who chose the same human name.
Recommendation: when creating local-only docs generate a UUID-based room id
(`room: 'board-' + crypto.randomUUID()`), store a user-visible label separately
(`map('meta').set('label', 'My Soundboard')`), and only use the stable UUID
when calling `attachServer(...)`.

## **Testing & Validation**

- **Run services:** Start the API, ActiveSync bridge, and the frontend/dev server
  before end-to-end tests. Example commands (adjust paths/ports as needed):

```bash
# API
cd PWASoundboard.Api
dotnet run --urls http://0.0.0.0:8080

# ActiveSync bridge (repo root)
cd activesync
# depending on your dev workflow: cargo run or start the dev server that
# hosts the bridge at the configured bridge URL
cargo run

# Frontend (web-react)
cd web-react
npm run dev
```

- **Offline hydration test:**
  1. Open app and call `createDoc({ room, /* no serverUrl */ })`.
  2. Call `map('assets').setBlob('fx/boom', bytes)` and `store` local state.
  3. Close the page and reopen; confirm `tryHydratePersistence()` restores the
     nodes pack and blob bytes (`map.get('fx/boom')` and `map.getBlob(...)`).

- **AttachServer end-to-end test:**
  1. Start a second client connected to the server (normal `createDoc({ serverUrl, room })`).
  2. On the first (local-only) client, call `await doc.attachServer(serverUrl)`.
  3. Verify server logs show the incoming pack and `/sync/assets/register` calls
     for uploaded blobs. Confirm the second client receives nodes and can fetch/play
     the uploaded blobs.

- **Token delay simulation:**
  - If your token provider can be slowed (e.g., return a Promise that resolves
    after a delay), test `attachServer` in Immediate and Wait-for-Welcome modes
    to ensure the UX and upload behavior are acceptable. Consider hardening
    `attachServer` to `await ensureFreshToken()` before initiating the connect
    attempt if your app requires it.

- **Checks to verify:**
  - Asset keys on the server follow `assets/{algorithm}/{hash}` (e.g.
    `assets/blake3/<hex>`).
  - The API received `POST /sync/assets/register` for each direct-uploaded blob.
  - Other clients can successfully `getBlob(...)` and decode/play the audio.

If you want, I can implement an optional `attachServer(..., { mode: 'wait-for-welcome'|'manual'|'immediate' })` overload and add a small demo harness that runs the three smoke tests automatically.
