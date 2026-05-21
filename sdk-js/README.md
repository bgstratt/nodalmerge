# activesync-sdk-js

High-level JavaScript SDK wrapper for ActiveSync using the `activesync-bridge` WASM runtime.

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
10. migration shim (`sdk.compat`)

## Install

```bash
npm install activesync-sdk-js
```

## Quick Start

```ts
import { createActiveSyncSdk } from "activesync-sdk-js";

const sdk = await createActiveSyncSdk({
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
    persistenceKey: "activesync:demo-room:outbox"
  }
});

await sdk.room.connect();

sdk.sync.set("username", "alice");
sdk.sync.push();

const username = sdk.sync.get("username");
const hash = sdk.replay.canonicalHash();

sdk.presence.set({ cursor: { x: 120, y: 220 } }, { ttlMs: 5_000 });
sdk.signaling.offer("peer-b", "v=0...");
sdk.compat.requestServerPack();

const stop = sdk.compat.onRuntimeEvent("pack", (msg) => {
  console.log("pack received", msg);
});

stop();
```

## Notes

- This SDK intentionally wraps the lower-level bridge with straightforward defaults.
- It can queue outbound messages while offline and flush after reconnect.
- Optional outbox persistence uses `localStorage` when `offline.persistenceKey` is provided.
- Reconnect policy is configurable through `reconnect` options.
- Transport policy (`ws-only` or `auto`) is configurable through `transport.mode`.
- Runtime message parsing is exposed via `parseRuntimeMessage` and `runtime-message` events.
- For advanced protocol control, use `activesync-bridge` directly.
