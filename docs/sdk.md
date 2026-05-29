# NodalMerge SDK

A Firebase/Replicache-style document API over the NodalMerge CRDT engine.
Thin JS wrapper around the low-level `SyncStore` — no core changes.

## Install

The SDK ships alongside the WASM bridge under `web/` in this repo. Copy
`web/sdk.js`, `web/sdk.d.ts`, and the `web/pkg/` folder (produced by
`wasm-pack build bridge --target web --out-dir ../web/pkg`) into your app.

Also available as package surfaces used by wrappers/integrations:

1. `nodalmerge-sdk-js` (`createNodalMergeSdk`) for runtime-oriented clients.
2. `nodalmerge-bridge` for WASM bridge packaging.

## Quick start

```js
import { createDoc, namespaceCapabilities } from './sdk.js';

const tokenCaps = namespaceCapabilities({
  read: ['world'],
  write: ['intent'],
});

const doc = await createDoc({
  serverUrl: 'ws://localhost:7878',
  room:      'demo-room-1',
  tokenCaps,
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
| `serverUrl`      | `string`     | — (required)   | Without trailing `/ws/<room>` — the SDK appends it. |
| `room`           | `string`     | — (required)   | Room id. Transport boundary; see PLAN.md decision log. |
| `authorSeed`     | `Uint8Array` | random 32 B    | Persist this to keep identity across sessions. |
| `roomSeed`       | `Uint8Array` | `null`         | Room private-key seed (C3). If set, a capability token is signed on every connect. |
| `tokenCaps`      | `string[] \| { read?: string[], write?: string[], derive?: string[] }`   | `[]`           | Path-scoped grants. String-array form accepts canonical values (for example `["write:intent/**"]`). Object form is namespace ergonomic (for example `{ read:["world"], write:["intent"] }`). Empty = full access. |
| `tokenExpirySecs`| `number`     | `86400`        | Token TTL. |
| `tokenProvider`  | `(ctx) => Promise<{ peer_pubkey_hex, expiry_secs, capabilities, sig_hex, continuity? }>` | — | Optional async mint/refresh hook for server-signed RoomToken payloads. Takes precedence over `roomSeed` signing when provided. `continuity` supports Phase C device-switch/key-rotation overlap flow. |
| `onRejection`    | `(ev) => void` | `null`      | Typed server rejection callback. Receives parsed `reasonClass`, `command`, `requiredCapability`, `message`, and raw envelope. |
| `autoConnect`    | `boolean`    | `true`         | If false, call `doc.connect()` manually. |
| `subscribe`      | `string[]`   | `["**"]`       | F3a: glob patterns for client-side materialization. See [Subscriptions](#subscriptions-f3a). |
| `transport`      | `'auto' \| 'ws-only'` | `'auto'` | `'auto'` enables WS + WebRTC mesh when available; `'ws-only'` disables WebRTC. |
| `iceServers`     | `RTCIceServer[]` | Google STUN defaults | Override STUN/TURN servers for mesh connections. |
| `presenceHeartbeatMs` | `number` | `15000`        | Interval between presence re-broadcasts. `0` disables. |
| `presenceStaleMs`| `number`     | `45000`        | Treat a silent peer as gone after this long. |
| `onMetric`       | `(ev) => void` | `null`      | G8 metric hook. Receives `{ kind, value, labels, timestamp }`. |
| `onDirectUpload` | `(args) => void \| Promise<void>` | `null` | Called after successful direct blob PUT (`{ hash, length }`). |
| `logger`         | `function`   | `console`      | `(level, ...args) => void`. |

### `Doc`

- `doc.map(namespace)` → `MapHandle` — LWW-Map over `<namespace>/<key>` paths.
- `doc.text(key)` → `TextHandle` — per-character RGA with tombstones.
- `doc.list(key)` → `ListHandle<T>` — ordered list with fractional-index CRDT semantics (F8).
- `doc.onChange(cb)`, `doc.onConnect(cb)`, `doc.onDisconnect(cb)`, `doc.onError(cb)` — each returns an unsubscribe function.
- `doc.onRejection(cb)`, `doc.recentRejections(sinceMs?)` — typed server rejection stream + bounded history buffer.
- `doc.onConflict(cb)`, `doc.recentConflicts(sinceMs?)` — conflict surfacing hook + bounded history buffer.
- `doc.connect()`, `doc.disconnect()`, `doc.close()`.
- `doc.peers()` — peer transport view (`ws` vs `webrtc`) and channel readiness.
- `doc.undoManager({ scope?, captureTimeout?, maxItems? })` — app-layer undo/redo manager using compensating ops.
- `doc.pubkeyHex`, `doc.authorSeed`, `doc.isConnected`.
- `doc.store` — escape hatch to the raw `SyncStore` for advanced use (speculative reads, frontier inspection).

### Capability helper APIs

- `capability(scope, pathPattern)` → builds one capability string.
- `namespaceCapabilities(spec)` → builds canonical sorted capability strings from namespace roots.

```js
import { capability, namespaceCapabilities } from './sdk.js';

capability('read', 'world/**');
// -> "read:world/**"

namespaceCapabilities({
  read: ['world'],
  write: ['intent', 'world/patches/**'],
});
// -> ["read:world/**", "write:intent/**", "write:world/patches/**"]
```

### Typed rejection surfaces

Server error envelopes and deterministic `reject.*` prefixes are normalized into
typed rejection events.

```js
doc.onRejection((ev) => {
  if (ev.reasonClass === 'reject.control_plane_forbidden') {
    console.warn('Denied command:', ev.command, 'requires', ev.requiredCapability);
  }
});

doc.onError((err) => {
  // Backward compatible: still an Error object.
  if (err.rejection) {
    console.log('typed rejection from onError', err.rejection.reasonClass);
  }
});

const recent = doc.recentRejections();
console.log('recent rejections', recent.length);
```

### Schema/version rollout guidance (FSE-06 baseline)

Use `docs/MIGRATION_COOKBOOK.md` as the canonical rollout template.
For rollout PR gating, use `docs/MIGRATION_ANTI_PATTERNS_CHECKLIST.md`.

SDK-side rules:

1. treat unsupported-window rejects (for example `reject.query_unsupported_version`) as non-retry-until-version-changes
2. surface requested vs supported version details in UI/logs when available
3. during migration windows, prefer dual-read compatibility and avoid hard cutovers in a single release

### Replay range read (runtime-oriented SDK surface)

The `nodalmerge-sdk-js` runtime surface can consume `replay.read-range` envelopes for history panes and analytics readers.

Request envelope:

```json
{ "type": "replay.read-range", "key_prefix": "world/", "from_lamport": 0, "limit": 100, "cursor": "offset:0" }
```

Result envelope:

```json
{
  "type": "replay.read-range.result",
  "key_prefix": "world/",
  "from_lamport": 0,
  "items": [{ "lamport": 1, "node_id": "<hex>", "touched_keys": ["world/a"] }],
  "next_cursor": "offset:100"
}
```

Guidance:

1. treat `cursor` / `next_cursor` as opaque server cursors
2. use bounded `limit` windows to keep UI latency predictable

Runtime-oriented helper (`nodalmerge-sdk-js`):

```js
const page1 = await sdk.query.readReplayRange({
  keyPrefix: "world/",
  fromLamport: 0,
  limit: 100
});

const page2 = await sdk.query.readReplayRange({
  keyPrefix: "world/",
  fromLamport: 0,
  limit: 100,
  cursor: page1.next_cursor
});
```

### Intent vs canonical refinement

Use separate namespaces to represent optimistic intent and authoritative state.

```js
const intent = doc.map('intent/player');
const world = doc.map('world/player');

// Optimistic local write.
intent.set('move', { dx: 1, dy: 0, nonce: crypto.randomUUID() });

// Authoritative canonical update arrives asynchronously.
world.onChange(() => {
  renderPlayer(world.get('position'));
});

// Typed rejection informs user-visible refinement.
doc.onRejection((ev) => {
  if (ev.reasonClass?.startsWith('reject.')) {
    toast(`Action denied: ${ev.message}`);
  }
});
```

### Local-first with authoritative refinement

Pattern: render speculative UI immediately, then reconcile when canonical state
or typed rejections arrive.

```js
const intent = doc.map('intent/orders');
const world = doc.map('world/orders');

function placeOrder(order) {
  // Local-first optimistic state.
  intent.set(order.id, { ...order, state: 'pending' });
  renderPending(order.id);
}

// Canonical truth from authoritative host/server path.
world.onChange(() => {
  renderCanonical(world.all());
});

// Rejection-driven refinement.
doc.onRejection((ev) => {
  if (!ev.reasonClass) return;
  renderRejected(ev.message, {
    reasonClass: ev.reasonClass,
    command: ev.command,
    requiredCapability: ev.requiredCapability,
  });
});
```

### Device switch and key rotation flow (Phase C slice 2)

Use this flow when a user moves from old device key A to new device key B.

1. Keep key A active while key B performs first handshake.
2. Mint key B token with continuity metadata:
   - `predecessor_peer_pubkey`: key A
   - `overlap_not_after`: Unix-seconds cutoff for overlap window
   - `revoked_predecessors`: empty during overlap
3. After overlap window and client cutover are complete, mint key B tokens with key A listed in `revoked_predecessors`.
4. Persist new device author seed and stop issuing key A tokens.

Example `tokenProvider` output during overlap:

```js
const tokenProvider = async ({ room, pubkeyHex }) => {
  const token = await fetch('/sync/token', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ room, peer_pubkey_hex: pubkeyHex }),
  }).then(r => r.json());

  return {
    peer_pubkey_hex: token.peer_pubkey_hex,
    expiry_secs: token.expiry_secs,
    capabilities: token.capabilities,
    sig_hex: token.sig_hex,
    continuity: {
      predecessor_peer_pubkey: token.continuity.predecessor_peer_pubkey,
      overlap_not_after: token.continuity.overlap_not_after,
      revoked_predecessors: token.continuity.revoked_predecessors ?? [],
    },
  };
};
```

Expected behavior during migration:

1. Successor key is allowed while `now <= overlap_not_after`.
2. Successor key is rejected after overlap expiry.
3. Successor key is rejected when predecessor is listed as revoked.

### `MapHandle`

- `set(key, value)` — `value` is any JSON-serializable value.
 - `setBlob(key, bytes, options?)` — stores `bytes` in CAS, sets the key to the blob hash, and returns the hash string. `options` supports `{ contentType }`. Large blobs may use F6 direct upload when supported; fallback is transparent.
- `get(key)` — returns the JSON value or `undefined`.
- `getBlob(hashOrKey)` — returns blob bytes or `undefined`, looking up key first when needed. Under F6, `blob-redirect` GET URLs are used when available; fallback to WS is automatic.
- `delete(key)`.
- `all()` — returns every key currently resolved under this namespace, stripped of the prefix.
- `onChange(cb)`.

### `TextHandle`

- `insert(pos, str)`, `delete(pos, len = 1)`.
- `toString()`.
- `onChange(cb)`.

### List status

ListHandle is shipped and intended for production use:
- `push`, `insert`, `insertAfter`, `insertBefore`, `move`, `delete`, `update`.
- `ids`, `get`, `toArray`, `length`, `onChange`, `onReorder`.
- `gestures` helpers: `dropOnto`, `dropBetween`, `swap`, `dropBefore`, `dropAfter`.

Current limitation: list deletes are tombstones and are intentionally not
undoable by the built-in undo manager.

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
- **List = shipped (F8).** Fractional-index list operations (`insert/move/delete`)
  with stable item ids are available through `doc.list(...)`.

## What the SDK handles for you

- WebSocket connection, heartbeat, and exponential-backoff reconnect.
- Optional WebRTC mesh transport (`transport: 'auto'`) with WS as authoritative fallback.
- Ed25519 identity + capability token (C3) when a `roomSeed` is provided.
- Handshake: frontier (A6), capability negotiation (A7), IBF (B1), MST (B2).
- Delta sync — only new nodes since the last successful pack are sent.
- Blob CAS round-trip: local uploads on reconnect, on-demand fetch of missing
  blobs after every merge.
- G8 metric emission via `onMetric` callback.
- G9 conflict surfacing via `onConflict` and `recentConflicts`.
- E3 app-layer undo/redo via `doc.undoManager(...)`.
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

- **Rich-text spans and text move operations** — Text is RGA character insert/delete.
- **True CRDT undo for destructive ops** — `text.delete` and `list.delete` are
  intentionally excluded from the built-in undo manager.
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

Offline persistence ownership contract:

- Browser/offline hydrate-save flows (IndexedDB node/blob caches) are SDK-owned.
- Host adapters (including .NET runtime host and host-ffi) do not implement or
  mirror browser IndexedDB persistence semantics.
- Host adapters only expose deterministic sync command/event primitives; SDK
  layers choose when and how to persist local graph/blob state for offline UX.

Undo/transport ownership contract:

- `doc.undoManager(...)` is SDK/app-layer behavior (compensating operations and
  capture-window policy), intentionally not a host-core adapter state machine.
- Host adapters expose deterministic map/text/list/blob command/event
  primitives that the SDK undo manager composes; adapters do not own undo UX
  policy, history grouping, or capture semantics.
- Reconnect/backoff policy and `transport: 'auto' | 'ws-only'` selection are
  SDK transport responsibilities.
- Host adapters remain WS-first authoritative sync bridges and may expose
  optional WebRTC signaling relay primitives used by SDK mesh logic.

Metrics ownership contract:

- `onMetric` emission is SDK-owned and app-facing; events are emitted as point
  telemetry envelopes (`{ kind, value, labels, timestamp }`) when configured.
- Host adapters and server processes own operational telemetry sinks/exporters
  (for example Prometheus endpoint wiring and runtime logging/tracing).
- Host-core/host-ffi/.NET runtime host do not expose a duplicate generic
  metrics wire command/event stream; SDK metrics are derived from runtime
  behavior and host instrumentation is emitted by the adapter process.

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
- [`web/demo.js`](../web/demo.js) — reference consumer built on `createDoc`.

## **Attach Server Modes**

The SDK exports a lightweight helper:

```js
import { attachServer } from './sdk.js';

await attachServer(doc, serverUrl, { mode: 'wait-for-welcome' });
```

Supported modes in the current helper are: `'immediate'`, `'wait-for-welcome'`, and `'manual'`.

- **Immediate (default):** ensures the document is connected now.
- **Wait-for-Welcome:** waits for a connect/welcome event (with timeout) before returning.
- **Manual:** returns early after kicking off connect, so app code controls follow-up flow.

This helper is intentionally transport-lifecycle focused. It does not perform
custom replay orchestration by itself.

Token & auth notes:
- `attachServer` uses the SDK's `tokenProvider` / `ensureFreshToken` hooks when
  the transport needs a token for the handshake. For best results ensure the
  user is logged in and a valid token can be minted before calling
  `attachServer`. If your token minting is asynchronous or slow, prefer the
  Wait-for-Welcome or Manual modes so the app can surface progress to
  the user while authentication completes.

When to pick which mode:
- Quick connect path: Immediate.
- Strict server capability / auth correctness: Wait-for-Welcome.
- App-driven sequencing or user confirmation: Manual.

## Room IDs

Do not reuse user-provided human-readable room names as the canonical room ID
for shared/server-backed rooms. Prefer stable UUID-style ids for canonical room
identity and keep human labels in app-level data.

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
  1. Open app and call `createDoc({ serverUrl, room })`.
  2. Call `map('assets').setBlob('fx/boom', bytes)` and `store` local state.
  3. Close the page and reopen; confirm `tryHydratePersistence()` restores the
     nodes pack and blob bytes (`map.get('fx/boom')` and `map.getBlob(...)`).

- **AttachServer end-to-end test:**
  1. Start a client with `createDoc({ serverUrl, room, autoConnect: false })`.
  2. Call `await attachServer(doc, serverUrl, { mode: 'wait-for-welcome' })`.
  3. Verify the client is connected and receives remote packs/updates.

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

If you need app-specific attach semantics beyond these three modes, wrap
`attachServer` in your product layer and keep the core SDK contract stable.
