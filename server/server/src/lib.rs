//! `nodalmerge-server` library surface — exposes the room registry, the
//! persistence layer, and the websocket handler for integration tests and
//! external embedders. The `nodalmerge-server` binary (see `main.rs`) is the
//! canonical consumer.

pub mod adapter_context;
pub mod archive_adapter;
pub mod archive_export;
pub mod blob_http;
pub mod gc_adapter;
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
pub mod topology_adapter;
pub mod ws_handler;
