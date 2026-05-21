# SDK Expansion Plan (Phase 1)

Goal: move reusable collaboration/runtime behavior from demo glue into `activesync-sdk-js`.

## Current Baseline

The SDK currently provides:

1. room connect/disconnect
2. sync set/get/del + push/pull
3. replay state/hash helpers
4. in-memory outbound queue
5. CAS blob helpers
6. topology snapshot

## Move Into SDK (Phase 1)

These are generic integrator concerns and should be packaged:

1. reconnect policy and retry backoff
2. optional persisted outbound queue (localStorage)
3. generic presence API wrappers
4. generic signaling relay wrappers
5. richer lifecycle/event hooks (`connected`, `disconnected`, `reconnect`, `presence`, `signal`)

## Keep Out of SDK

These remain demo/app domain concerns:

1. tactical/card-battle/scenario semantics
2. workspace-specific DTO models and replay UI controls
3. domain-specific optimistic rendering rules

## Phase 1 API Additions

1. `reconnect` options in `ActiveSyncSdkOptions`
2. `offline.persistenceKey`
3. `presence.set/getAll/sweep`
4. `signaling.relay/offer/answer/ice`
5. event types:
   - `connected`
   - `disconnected`
   - `reconnect`
   - `presence`
   - `signal`

## Validation

1. package can still `npm pack`
2. existing `room/sync/replay/offline/CAS/topology` flows remain functional
3. new API additions are additive (non-breaking)

## Move Into SDK (Phase 2)

1. transport policy (`ws-only` vs `auto`)
2. typed runtime message discriminators + parser
3. migration shim surface (`sdk.compat`) for reducing demo cutover friction

Phase 2 done criteria:

1. transport policy appears in topology snapshot
2. runtime-message events are emitted and type-parseable
3. compat aliases exist for common legacy pack/presence/signaling calls
