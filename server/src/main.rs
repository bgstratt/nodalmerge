use activesync_server::{keypair, metrics, room, store, ws_handler};

use axum::{Router, routing::get};
use tower_http::cors::{CorsLayer, Any};
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    // Log filter: honor RUST_LOG, default to info for our crates.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,activesync_server=info,activesync_core=info"));
    fmt().with_env_filter(filter).with_target(false).init();

    tracing::info!(
        workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
        "tokio multi-thread runtime started"
    );
    let args: Vec<String> = std::env::args().collect();

    // D4: `activesync-server replay <pack-file>` subcommand.
    // Reads a base64-encoded postcard node pack from a file (or stdin if "-"),
    // replays it, and prints the resolved state + canonical hash to stdout.
    if args.get(1).map(|s| s.as_str()) == Some("replay") {
        let source = args.get(2).map(|s| s.as_str()).unwrap_or("-");
        return run_replay(source);
    }

    // E1: load or generate persistent server keypair.
    let server_key = keypair::load_or_generate();
    let server_pubkey = keypair::pubkey_hex(&server_key);
    tracing::info!(pubkey = %server_pubkey, "server keypair ready");

    // F4: optional on-disk persistence. `--store <path>` enables a SQLite+files
    // backend rooted at `<path>`; absent it, the server is in-memory only.
    let persistence: store::SharedPersistence = match parse_store_arg(&args) {
        Some(path) => {
            match store::DirPersistence::open(&path) {
                Ok(p) => {
                    tracing::info!(store = %path.display(), "persistence enabled (SQLite + blobs)");
                    std::sync::Arc::new(p)
                }
                Err(e) => {
                    eprintln!("error: --store open failed at {}: {e}", path.display());
                    std::process::exit(1);
                }
            }
        }
        None => {
            tracing::info!("persistence disabled (in-memory only)");
            std::sync::Arc::new(store::NoPersistence)
        }
    };

    let rooms = room::Rooms::new(server_key, persistence);

    // G7: optional Prometheus metrics endpoint on an admin port. `--metrics-addr
    // <ip:port>` enables it; absent, no recorder is installed. Install failure
    // (port taken, etc.) is logged — server continues without observability.
    if let Some(addr) = metrics::parse_arg(&args) {
        match metrics::init(addr) {
            Ok(()) => tracing::info!(%addr, "metrics endpoint listening on http://{addr}/metrics"),
            Err(e) => tracing::warn!(?e, "failed to install metrics recorder; continuing without metrics"),
        }
    }

    // Idle-eviction sweeper: drop in-memory rooms whose peer count has been
    // zero for longer than `--idle-timeout`. Default 300 s (5 min). `0`
    // disables eviction. Also disabled automatically when persistence is
    // in-memory (otherwise eviction would be data loss).
    let idle_timeout = parse_idle_timeout_arg(&args).unwrap_or(300);
    if idle_timeout > 0 {
        if rooms.persistence.is_durable() {
            tracing::info!(secs = idle_timeout, "idle-room eviction enabled");
            let _handle = room::spawn_idle_sweeper(
                rooms.clone(),
                std::time::Duration::from_secs(idle_timeout),
                std::time::Duration::from_secs(60),
            );
        } else {
            tracing::warn!(
                "--idle-timeout set but persistence is in-memory; eviction disabled \
                 (would cause data loss). Pass --store <path> to enable."
            );
        }
    } else {
        tracing::info!("idle-room eviction disabled (idle-timeout = 0)");
    }

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_headers(Any)
        .allow_methods(Any);

    let app = Router::new()
        .route("/ws/:room_id", get(ws_handler::handler))
        .layer(cors)
        .with_state(rooms);

    let addr = "127.0.0.1:7878";
    tracing::info!(%addr, "ActiveSync server listening on ws://{addr}/ws/<room>");
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

/// F4: Parse `--store <path>` (or `--store=<path>`) from the CLI.
/// Returns `None` when absent, so the default stays in-memory.
fn parse_store_arg(args: &[String]) -> Option<std::path::PathBuf> {
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        if a == "--store" {
            return args.get(i + 1).map(std::path::PathBuf::from);
        }
        if let Some(val) = a.strip_prefix("--store=") {
            return Some(std::path::PathBuf::from(val));
        }
        i += 1;
    }
    None
}

/// Parse `--idle-timeout <seconds>` (or `--idle-timeout=<seconds>`).
/// `0` disables eviction. Returns `None` to fall through to the default
/// (300 s, 5 min). Invalid values also fall back to the default with a
/// warning — the server does not refuse to start on a typo.
fn parse_idle_timeout_arg(args: &[String]) -> Option<u64> {
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        let raw = if a == "--idle-timeout" {
            args.get(i + 1).map(|s| s.as_str())
        } else if let Some(v) = a.strip_prefix("--idle-timeout=") {
            Some(v)
        } else {
            None
        };
        if let Some(s) = raw {
            return match s.parse::<u64>() {
                Ok(n) => Some(n),
                Err(_) => {
                    eprintln!("warning: --idle-timeout expects a non-negative integer (seconds); got {s:?}, using default 300");
                    None
                }
            };
        }
        i += 1;
    }
    None
}

/// D4: Replay a base64-encoded node pack and print resolved state + hash.
/// `source` is a file path, or "-" to read from stdin.
fn run_replay(source: &str) {    use activesync_core::{unpack_nodes, replay};

    // Read raw bytes from file or stdin.
    let raw_bytes: Vec<u8> = if source == "-" {
        use std::io::Read;
        let mut buf = Vec::new();
        std::io::stdin().read_to_end(&mut buf).expect("failed to read stdin");
        buf
    } else {
        std::fs::read(source).unwrap_or_else(|e| {
            eprintln!("error reading {source}: {e}");
            std::process::exit(1);
        })
    };

    // Strip optional newline / whitespace, then base64-decode.
    let b64 = String::from_utf8_lossy(&raw_bytes);
    let b64 = b64.trim();
    let pack_bytes = base64_decode(b64).unwrap_or_else(|| {
        eprintln!("error: input is not valid base64");
        std::process::exit(1);
    });

    let nodes = unpack_nodes(&pack_bytes).unwrap_or_else(|e| {
        eprintln!("error decoding pack: {e}");
        std::process::exit(1);
    });

    eprintln!("Replaying {} node(s)...", nodes.len());

    let state = replay(&nodes, None).unwrap_or_else(|e| {
        eprintln!("replay error: {e}");
        std::process::exit(1);
    });

    eprintln!("\nResolved state ({} key(s)):", state.map.len());
    for (key, value) in &state.map {
        // Print values as UTF-8 if valid, otherwise show hex.
        let display = std::str::from_utf8(value)
            .map(|s| format!("{:?}", s))
            .unwrap_or_else(|_| format!("0x{}", hex_encode(value)));
        eprintln!("  {key} = {display}");
    }

    eprintln!("\nCanonical hash: {}", state.hash.to_hex());
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8; 128] = b"\
        \xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\
        \xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\
        \xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\x3e\xff\xff\xff\x3f\
        \x34\x35\x36\x37\x38\x39\x3a\x3b\x3c\x3d\xff\xff\xff\xff\xff\xff\
        \xff\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0c\x0d\x0e\
        \x0f\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19\xff\xff\xff\xff\xff\
        \xff\x1a\x1b\x1c\x1d\x1e\x1f\x20\x21\x22\x23\x24\x25\x26\x27\x28\
        \x29\x2a\x2b\x2c\x2d\x2e\x2f\x30\x31\x32\x33\xff\xff\xff\xff\xff";
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut i = 0;
    while i < bytes.len() {
        let b0 = *bytes.get(i)?;
        let b1 = *bytes.get(i + 1)?;
        if b0 == b'=' { break; }
        let v0 = *TABLE.get(b0 as usize)? as u32;
        let v1 = *TABLE.get(b1 as usize)? as u32;
        if v0 == 0xff || v1 == 0xff { return None; }
        out.push(((v0 << 2) | (v1 >> 4)) as u8);
        let b2 = bytes.get(i + 2).copied().unwrap_or(b'=');
        if b2 != b'=' {
            let v2 = *TABLE.get(b2 as usize)? as u32;
            if v2 == 0xff { return None; }
            out.push(((v1 << 4) | (v2 >> 2)) as u8);
            let b3 = bytes.get(i + 3).copied().unwrap_or(b'=');
            if b3 != b'=' {
                let v3 = *TABLE.get(b3 as usize)? as u32;
                if v3 == 0xff { return None; }
                out.push(((v2 << 6) | v3) as u8);
            }
        }
        i += 4;
    }
    Some(out)
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
