use std::sync::Arc;

use nodalmerge_server::{
    blob_http::{self, BlobHttpConfig},
    keypair,
    room::Rooms,
    store::{NoPersistence, SharedPersistence},
    ws_handler,
};
use axum::{routing::get, Router};
use ed25519_dalek::SigningKey;
use tower_http::cors::{Any, CorsLayer};

#[derive(Clone, Debug)]
pub struct HostAxumConfig {
    pub broadcast_capacity: usize,
    pub peer_rate_nodes: u32,
    pub peer_rate_bytes: u32,
}

impl Default for HostAxumConfig {
    fn default() -> Self {
        Self {
            broadcast_capacity: 512,
            peer_rate_nodes: 200,
            peer_rate_bytes: 4 * 1024 * 1024,
        }
    }
}

pub fn load_or_generate_server_key() -> SigningKey {
    keypair::load_or_generate()
}

pub fn create_rooms(
    server_key: SigningKey,
    persistence: SharedPersistence,
    config: HostAxumConfig,
) -> Rooms {
    Rooms::new(
        server_key,
        persistence,
        config.broadcast_capacity,
        config.peer_rate_nodes,
        config.peer_rate_bytes,
    )
}

pub fn create_in_memory_rooms(server_key: SigningKey, config: HostAxumConfig) -> Rooms {
    create_rooms(server_key, Arc::new(NoPersistence), config)
}

pub fn build_router(rooms: Rooms) -> Router {
    Router::new()
        .route("/ws/:room_id", get(ws_handler::handler))
        .with_state(rooms)
}

pub fn build_router_with_permissive_cors(rooms: Rooms) -> Router {
    build_router(rooms).layer(
        CorsLayer::new()
            .allow_origin(Any)
            .allow_headers(Any)
            .allow_methods(Any),
    )
}

/// S2.1b — like [`build_router`], plus the blob HTTP origin
/// (`GET`/`HEAD`/`PUT /blobs/:hash`, see `docs/BLOB_HTTP_SURFACE.md`),
/// backed by whatever `SharedPersistence` `rooms` was built with. Existing
/// callers of `build_router`/`build_router_with_permissive_cors` are
/// unaffected — this is an additive function, not a signature change.
pub fn build_router_with_blobs(rooms: Rooms, blob_cfg: BlobHttpConfig) -> Router {
    Router::new()
        .route("/ws/:room_id", get(ws_handler::handler))
        .merge(blob_http::blob_routes(blob_cfg))
        .with_state(rooms)
}

/// S2.1b — [`build_router_with_blobs`] plus the same permissive CORS layer
/// as [`build_router_with_permissive_cors`].
pub fn build_router_with_blobs_and_permissive_cors(
    rooms: Rooms,
    blob_cfg: BlobHttpConfig,
) -> Router {
    build_router_with_blobs(rooms, blob_cfg).layer(
        CorsLayer::new()
            .allow_origin(Any)
            .allow_headers(Any)
            .allow_methods(Any),
    )
}
