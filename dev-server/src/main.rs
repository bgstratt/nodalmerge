use activesync_server::{keypair, metrics, room, store, ws_handler};
use activesync_mongo_store::{MongoNodeStore, MongoNodeStoreConfig};

use axum::{Router, routing::get};
use tower_http::cors::{CorsLayer, Any};
use tracing_subscriber::{EnvFilter, fmt};
use tracing_log::LogTracer;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,activesync_server=info,activesync_core=info"));
    // Initialize tracing subscriber first so it can receive events. If that
    // succeeds, install `LogTracer` to forward `log` crate messages (used by
    // the Mongo driver) into `tracing`. If subscriber init fails (e.g. a
    // logger is already installed), fall back to `env_logger` so `RUST_LOG`
    // controlled logs still appear on stderr.
    if let Err(e) = fmt().with_env_filter(filter).with_target(false).try_init() {
        eprintln!("warning: tracing subscriber init failed: {e:?}");
        // Fallback: initialize env_logger so `log` crate logs are visible.
        let _ = env_logger::Builder::from_env(env_logger::Env::default()).try_init();
    } else {
        // Subscriber installed; bridge `log` -> `tracing` so the mongo
        // driver's `log`-based messages are captured by the tracing subscriber.
        LogTracer::init().ok();
    }

    // Emit a small set of test logs to verify both `tracing` and `log` crate
    // messages are being routed correctly. These can be removed once
    // you've confirmed Mongo driver logs appear at the expected level.
    let rust_log = std::env::var("RUST_LOG").unwrap_or_else(|_| "<unset>".to_string());
    tracing::info!(workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1), %rust_log, "tokio multi-thread runtime started");
    tracing::debug!("tracing debug test");
    log::debug!("log crate debug test");

    let server_key = keypair::load_or_generate();
    tracing::info!(pubkey = %keypair::pubkey_hex(&server_key), "server keypair ready");

    // If MONGO_URI is present, wire MongoNodeStore for nodes and NoPersistence for blobs.
    let persistence: store::SharedPersistence = match std::env::var("MONGO_URI") {
        Ok(uri) if !uri.is_empty() => {
            let db = std::env::var("MONGO_DATABASE").unwrap_or_else(|_| "activesync".to_string());
            tracing::info!(%uri, %db, "MONGO_URI present; wiring MongoNodeStore for nodes");
            let cfg = MongoNodeStoreConfig::new(uri, db);
            match MongoNodeStore::connect(cfg).await {
                Ok(nodes) => {
                    // Run a quick test write to exercise the driver and
                    // surface any early selection/connection issues in logs.
                    if let Err(e) = nodes.test_write().await {
                        tracing::warn!(error = ?e, "startup mongo test_write failed");
                    } else {
                        tracing::info!("startup mongo test_write succeeded");
                    }
                    let composite = store::Composite::new(nodes, store::NoPersistence);
                    std::sync::Arc::new(composite)
                }
                Err(e) => {
                    eprintln!("error: MongoNodeStore::connect failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        _ => {
            tracing::info!("MONGO_URI not set; dev-server requires MONGO_URI to run against Atlas");
            std::process::exit(1);
        }
    };

    let rooms = room::Rooms::new(
        server_key,
        persistence,
        512,
        200,
        4 * 1024 * 1024,
    );

    if let Some(addr) = metrics::parse_arg(&std::env::args().collect::<Vec<_>>()) {
        let _ = metrics::init(addr);
    }

    let cors = CorsLayer::new().allow_origin(Any).allow_headers(Any).allow_methods(Any);
    let app = Router::new().route("/ws/:room_id", get(ws_handler::handler)).layer(cors).with_state(rooms);

    let addr = std::env::var("AS_BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:7878".to_string());
    tracing::info!(%addr, "Dev ActiveSync server listening on ws://{addr}/ws/<room>");
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
