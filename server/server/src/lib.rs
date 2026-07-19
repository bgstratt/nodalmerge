//! `nodalmerge-server` library surface — exposes the room registry, the
//! persistence layer, and the websocket handler for integration tests and
//! external embedders. The `nodalmerge-server` binary (see `main.rs`) is the
//! canonical consumer.

pub mod adapter_context;
pub mod archive_adapter;
pub mod archive_export;
pub mod blob_http;
pub mod cli_args;
// Internal-only (both call sites live in this crate); no external consumer
// needs it, so it stays crate-private unlike the other modules here.
mod date_util;
pub mod gc_adapter;
pub mod gc_blob_objects;
pub mod gc_pin_store;
pub mod gc_service;
pub mod gc_store;
pub mod graph_query;
pub mod keypair;
pub mod lineage;
pub mod lineage_store;
pub mod metrics;
pub mod promotion;
pub mod promotion_metrics;
pub mod promotion_store;
pub mod query_control;
pub mod room;
pub mod store;
pub mod studio_live_hashes;
pub mod topology_adapter;
pub mod tree_walk;
pub mod ws_handler;
