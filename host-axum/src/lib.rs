use std::sync::Arc;

use nodalmerge_server::{
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
