# Peer-local persistence migration

## Default behavior (unchanged)

`createNodalMergeSdk` does **not** enable peer-local graph persistence unless you opt in:

```js
const sdk = await createNodalMergeSdk({
  wsUrl: "ws://127.0.0.1:5271/ws/demo",
  roomId: "demo"
});
```

`offline.persistenceKey` still controls **outbox** persistence in `localStorage` only. It is unrelated to graph/node durability.

## Adapter-driven mode (Phase C)

Enable IndexedDB peer-local persistence:

```js
const sdk = await createNodalMergeSdk({
  wsUrl: "ws://127.0.0.1:5271/ws/demo",
  roomId: "demo",
  persistence: {
    enabled: true,
    adapter: "indexeddb",
    dbName: "nodalmerge-peer-local"
  }
});
```

On `initialize()`:

1. Opens the IndexedDB database.
2. Hydrates the room's node pack and blobs into the WASM `SyncStore` **before** `room.connect()`.

On local edits and inbound `pack` / `blob-pack` messages, the SDK debounces a full-graph save (nodes + local blobs).

Use `sdk.persistence.flush()` before page unload or `sdk.room.disconnect()` (flush is invoked automatically on disconnect).

## Migrating from app-managed IndexedDB (e.g. `web/demo.js`)

1. Remove hand-rolled `idbPut('nodes', 'all', …)` and blob cursor hydration.
2. Pass `persistence: { enabled: true }` to `createNodalMergeSdk` (or keep using `createDoc` from `web/sdk.js` until that path is ported).
3. Call `await sdk.initialize()` then `await sdk.room.connect()` — hydration happens during initialize.
4. Keep `offline.persistenceKey` if you still need disconnected outbox replay.

Room keys in IndexedDB are scoped by `roomId` (nodes store) and `roomId:hash` (blobs store), so multiple rooms can share one database name.

## Custom adapters

Pass an object implementing `PeerLocalPersistenceAdapter` as `persistence.adapter` instead of `"indexeddb"`.

## Headless / pod parity

Browser IndexedDB maps to the same conceptual contract as `nodalmerge-runtime-local` (`hydrate`, `flush`, `recover`). For pods, use `NODALMERGE_HEADLESS_BACKEND=file` or `embedded` with a mounted data directory — not IndexedDB.
