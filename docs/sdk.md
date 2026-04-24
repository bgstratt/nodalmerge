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
| `serverUrl`      | `string`     | — (required)   | Without trailing `/ws/<room>` — the SDK appends it. |
| `room`           | `string`     | — (required)   | Room id. Transport boundary; see PLAN.md decision log. |
| `authorSeed`     | `Uint8Array` | random 32 B    | Persist this to keep identity across sessions. |
| `roomSeed`       | `Uint8Array` | `null`         | Room private-key seed (C3). If set, a capability token is signed on every connect. |
| `tokenCaps`      | `string[]`   | `[]`           | Path-scoped grants, e.g. `["write:intent/**"]`. Empty = full access. |
| `tokenExpirySecs`| `number`     | `86400`        | Token TTL. |
| `autoConnect`    | `boolean`    | `true`         | If false, call `doc.connect()` manually. |
| `subscribe`      | `string[]`   | `["**"]`       | F3a: glob patterns for client-side materialization. See [Subscriptions](#subscriptions-f3a). |
| `presenceHeartbeatMs` | `number` | `15000`        | Interval between presence re-broadcasts. `0` disables. |
| `presenceStaleMs`| `number`     | `45000`        | Treat a silent peer as gone after this long. |
| `logger`         | `function`   | `console`      | `(level, ...args) => void`. |

### `Doc`

- `doc.map(namespace)` → `MapHandle` — LWW-Map over `<namespace>/<key>` paths.
- `doc.text(key)` → `TextHandle` — per-character RGA with tombstones.
- `doc.list(key)` → **throws**. Deferred to post-F (fractional-index + move op).
- `doc.onChange(cb)`, `doc.onConnect(cb)`, `doc.onDisconnect(cb)`, `doc.onError(cb)` — each returns an unsubscribe function.
- `doc.connect()`, `doc.disconnect()`, `doc.close()`.
- `doc.pubkeyHex`, `doc.authorSeed`, `doc.isConnected`.
- `doc.store` — escape hatch to the raw `SyncStore` for advanced use (speculative reads, frontier inspection).

### `MapHandle`

- `set(key, value)` — `value` is any JSON-serializable value.
- `setBlob(key, bytes)` — stores `bytes` in the CAS, sets the key to its blob hash.
- `get(key)` — returns the JSON value or `undefined`.
- `getBlob(hashOrKey)` — returns the blob bytes, looking the key up if needed.
- `delete(key)`.
- `all()` — returns every key currently resolved under this namespace, stripped of the prefix.
- `onChange(cb)`.

### `TextHandle`

- `insert(pos, str)`, `delete(pos, len = 1)`.
- `toString()`.
- `onChange(cb)`.

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
  SDK uses WebSocket only; the demo app (`web/demo.js`) shows the WebRTC glue.
- **Client-side subscription filters** — F3a. Today every peer replicates the
  entire room.
- **Speculative vs. canonical split** — E2 is in the bridge (`read_speculative`
  / `read_canonical`); surface via `doc.store` if needed before it's exposed.
- **Persistence** — browser-side IndexedDB adapter lives in the demo today.
  The SDK itself is stateless; bring your own adapter or wait for the next SDK
  cut.

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
