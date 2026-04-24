//! `activesync-server` library surface — exposes the room registry, the
//! persistence layer, and the websocket handler for integration tests and
//! external embedders. The `activesync-server` binary (see `main.rs`) is the
//! canonical consumer.

pub mod keypair;
pub mod room;
pub mod store;
pub mod ws_handler;
