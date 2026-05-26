# nodalmerge-sdk-js

High-level JavaScript SDK wrapper for NodalMerge using the `nodalmerge-bridge` WASM runtime.

## Dead-Simple API

The wrapper groups operations into:

1. room
2. sync
3. replay
4. offline
5. CAS
6. topology
7. presence
8. signaling
9. transport policy and runtime-message typing
10. transport policy and runtime-message typing

## Install

```bash
npm install nodalmerge-sdk-js
```

## Quick Start

```ts
import { createNodalMergeSdk } from "nodalmerge-sdk-js";

const sdk = await createNodalMergeSdk({
  wsUrl: "ws://127.0.0.1:8787/ws/runtime",
  roomId: "demo-room",
  transport: {
    mode: "auto"
  },
  reconnect: {
    enabled: true,
    initialDelayMs: 500,
    maxDelayMs: 8000
  },
  offline: {
    persistenceKey: "nodalmerge:demo-room:outbox"
  }
});

await sdk.room.connect();

sdk.sync.set("username", "alice");
sdk.sync.push();

const username = sdk.sync.get("username");
const hash = sdk.replay.canonicalHash();

sdk.presence.set({ cursor: { x: 120, y: 220 } }, { ttlMs: 5_000 });
sdk.signaling.offer("peer-b", "v=0...");
const stop = sdk.on("runtime-message", (msg) => {
  console.log("pack received", msg);
});

stop();
```

## Text Range Convenience API

The SDK now includes range-oriented text helpers under `sdk.sync`:

- `insertTextAt(key, pos, text)`
- `deleteTextAt(key, pos, len)`
- `insertTextRange(key, anchor, text)`
- `deleteTextRange(key, anchor, len)`

Anchor object shapes:

- Insert anchors:
  - `{ kind: "offset", pos }`
  - `{ kind: "start" }`
  - `{ kind: "end" }`
  - `{ kind: "after", lamport, author }`
- Delete anchors:
  - `{ kind: "offset", pos }`
  - `{ kind: "start" }`
  - `{ kind: "after", lamport, author }`

Example:

```ts
sdk.sync.insertTextRange("doc:title", { kind: "start" }, "Hello");
sdk.sync.insertTextRange("doc:title", { kind: "end" }, " world");
sdk.sync.deleteTextRange("doc:title", { kind: "offset", pos: 5 }, 1);

// Anchor after a specific op id (lamport + 64-char author hex):
sdk.sync.insertTextRange("doc:title", {
  kind: "after",
  lamport: 42,
  author: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
}, "!");
```

These helpers apply locally; call `sdk.sync.push()` to send changes.

## Notes

- This SDK intentionally wraps the lower-level bridge with straightforward defaults.
- It can queue outbound messages while offline and flush after reconnect.
- Optional outbox persistence uses `localStorage` when `offline.persistenceKey` is provided.
- Reconnect policy is configurable through `reconnect` options.
- Transport policy (`ws-only` or `auto`) is configurable through `transport.mode`.
- Runtime message parsing is exposed via `parseRuntimeMessage` and `runtime-message` events.
- For advanced protocol control, use `nodalmerge-bridge` directly.
