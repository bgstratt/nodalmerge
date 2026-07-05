# activesync-host-axum

Paper-thin Axum adapter for embedding the ActiveSync host runtime.

## Design Goal

Keep integration simple without adding runtime overhead:

1. No extra queues.
2. No additional protocol state machine.
3. No copy of host-core logic.

This crate only provides room/state wiring and route registration.

## What It Exposes

1. `create_rooms(...)` and `create_in_memory_rooms(...)`
2. `build_router(...)`
3. `build_router_with_permissive_cors(...)`
4. `load_or_generate_server_key()`

## Example

```rust
use activesync_host_axum::{
    HostAxumConfig,
    load_or_generate_server_key,
    create_in_memory_rooms,
    build_router,
};

#[tokio::main]
async fn main() {
    let key = load_or_generate_server_key();
    let rooms = create_in_memory_rooms(key, HostAxumConfig::default());
    let app = build_router(rooms);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:7878").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
```
