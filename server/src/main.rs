use activesync_server::{keypair, metrics, room, store, ws_handler};

use std::sync::Arc;
use axum::{Router, routing::get};
use activesync_core::PolicyTimelineEntry;
use serde::Deserialize;
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
        let timeline = load_replay_policy_timeline(&args).unwrap_or_else(|e| {
            eprintln!("error: {e}");
            std::process::exit(1);
        });
        return run_replay(source, timeline.as_deref());
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

    let rooms = room::Rooms::new(
        server_key,
        persistence,
        parse_broadcast_capacity_arg(&args).unwrap_or(512),
        parse_peer_rate_nodes_arg(&args).unwrap_or(200),
        parse_peer_rate_bytes_arg(&args).unwrap_or(4 * 1024 * 1024),
    );

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

    // G4: optional blob GC sweeper. `--blob-gc-interval <secs>` (default 0 =
    // disabled) arms the task; `--blob-gc-grace <secs>` (default 86400 = 24 h)
    // is the tombstone-to-delete grace window. Like idle eviction, this is
    // durable-only: on in-memory builds blobs never hit disk so there is
    // nothing to collect.
    let blob_gc_interval = parse_u64_flag(&args, "--blob-gc-interval", 0).unwrap_or(0);
    if blob_gc_interval > 0 {
        if rooms.persistence.is_durable() {
            let grace = parse_u64_flag(&args, "--blob-gc-grace", 86400).unwrap_or(86400);
            tracing::info!(
                interval_secs = blob_gc_interval,
                grace_secs = grace,
                "blob GC enabled"
            );
            let _handle = room::spawn_blob_gc_sweeper(
                rooms.clone(),
                std::time::Duration::from_secs(blob_gc_interval),
                std::time::Duration::from_secs(grace),
            );
        } else {
            tracing::warn!(
                "--blob-gc-interval set but persistence is in-memory; GC disabled \
                 (no on-disk blobs to collect). Pass --store <path> to enable."
            );
        }
    }

    // E4: optional snapshot sweeper. `--snapshot-interval <N>` (default 0 =
    // disabled) triggers a compaction snapshot every N new nodes per room.
    // `--snapshot-max-chain <K>` (default 10) limits incremental chain depth
    // before the sweeper falls back to a full snapshot.
    let snapshot_interval = parse_usize_flag(&args, "--snapshot-interval", 0).unwrap_or(0);
    if snapshot_interval > 0 {
        let max_chain = parse_usize_flag(&args, "--snapshot-max-chain", 10).unwrap_or(10);
        tracing::info!(
            interval_nodes = snapshot_interval,
            max_chain_depth = max_chain,
            "snapshot sweeper enabled"
        );
        let _handle = room::spawn_snapshot_sweeper(
            rooms.clone(),
            Arc::clone(&rooms.server_key),
            snapshot_interval,
            max_chain,
            std::time::Duration::from_secs(30), // check every 30 s
        );
    }

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_headers(Any)
        .allow_methods(Any);

    let app = Router::new()
        .route("/ws/:room_id", get(ws_handler::handler))
        .layer(cors)
        .with_state(rooms);

    let addr = std::env::var("AS_BIND_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:7878".to_string());
    tracing::info!(%addr, "ActiveSync server listening on ws://{addr}/ws/<room>");
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
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

/// G1: Parse `--broadcast-capacity <N>` (or `--broadcast-capacity=<N>`).
/// Returns `None` to fall through to the default (512). Zero or invalid
/// values log a warning and fall back to the default — the server does
/// not refuse to start on a typo, and `tokio::sync::broadcast::channel`
/// rejects capacity=0 outright.
fn parse_broadcast_capacity_arg(args: &[String]) -> Option<usize> {
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        let raw = if a == "--broadcast-capacity" {
            args.get(i + 1).map(|s| s.as_str())
        } else if let Some(v) = a.strip_prefix("--broadcast-capacity=") {
            Some(v)
        } else {
            None
        };
        if let Some(s) = raw {
            return match s.parse::<usize>() {
                Ok(0) => {
                    eprintln!("warning: --broadcast-capacity must be > 0; got 0, using default 512");
                    None
                }
                Ok(n) => Some(n),
                Err(_) => {
                    eprintln!("warning: --broadcast-capacity expects a positive integer; got {s:?}, using default 512");
                    None
                }
            };
        }
        i += 1;
    }
    None
}

/// G3: Parse `--peer-rate-nodes <N>` (or `--peer-rate-nodes=<N>`), the
/// per-peer ceiling on inbound nodes per second. Returns `None` to fall
/// through to the default (200 nodes/s). `0` explicitly disables the
/// node-count limiter. Invalid values log a warning and fall back to the
/// default.
fn parse_peer_rate_nodes_arg(args: &[String]) -> Option<u32> {
    parse_u32_flag(args, "--peer-rate-nodes", 200)
}

/// G3: Parse `--peer-rate-bytes <MIB>` (or `--peer-rate-bytes=<MIB>`), the
/// per-peer ceiling on inbound decoded-pack *bytes* per second. The CLI
/// value is in MiB for ergonomics; we convert to bytes here. Returns
/// `None` to fall through to the default (4 MiB/s = 4 194 304 B/s). `0`
/// explicitly disables the byte-rate limiter. Values that would overflow
/// `u32` after MiB→bytes conversion fall back to the default with a
/// warning.
fn parse_peer_rate_bytes_arg(args: &[String]) -> Option<u32> {
    // Read as u32 MiB, multiply by 1 MiB, saturating (u32::MAX ≈ 4 GiB).
    let mib = parse_u32_flag(args, "--peer-rate-bytes", 4)?;
    Some(mib.saturating_mul(1024 * 1024))
}

/// Shared helper for `--peer-rate-*` flags: parses a non-negative `u32`.
/// `default_for_msg` is only used in the warning text so the operator
/// sees the correct fallback per flag. Returns `None` when the flag is
/// absent or invalid.
fn parse_u32_flag(args: &[String], flag: &str, default_for_msg: u32) -> Option<u32> {
    let eq_prefix = format!("{flag}=");
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        let raw = if a == flag {
            args.get(i + 1).map(|s| s.as_str())
        } else if let Some(v) = a.strip_prefix(&eq_prefix) {
            Some(v)
        } else {
            None
        };
        if let Some(s) = raw {
            return match s.parse::<u32>() {
                Ok(n) => Some(n),
                Err(_) => {
                    eprintln!("warning: {flag} expects a non-negative integer; got {s:?}, using default {default_for_msg}");
                    None
                }
            };
        }
        i += 1;
    }
    None
}

/// G4: shared helper for `--blob-gc-*` flags (and anything else that wants
/// a non-negative `u64`). Mirrors `parse_u32_flag`.
fn parse_u64_flag(args: &[String], flag: &str, default_for_msg: u64) -> Option<u64> {
    let eq_prefix = format!("{flag}=");
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        let raw = if a == flag {
            args.get(i + 1).map(|s| s.as_str())
        } else if let Some(v) = a.strip_prefix(&eq_prefix) {
            Some(v)
        } else {
            None
        };
        if let Some(s) = raw {
            return match s.parse::<u64>() {
                Ok(n) => Some(n),
                Err(_) => {
                    eprintln!("warning: {flag} expects a non-negative integer; got {s:?}, using default {default_for_msg}");
                    None
                }
            };
        }
        i += 1;
    }
    None
}

/// E4: Parse a `usize` CLI flag (e.g. `--snapshot-interval 100`).
fn parse_usize_flag(args: &[String], flag: &str, default_for_msg: usize) -> Option<usize> {
    let eq_prefix = format!("{flag}=");
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        let raw = if a == flag {
            args.get(i + 1).map(|s| s.as_str())
        } else if let Some(v) = a.strip_prefix(&eq_prefix) {
            Some(v)
        } else {
            None
        };
        if let Some(s) = raw {
            return match s.parse::<usize>() {
                Ok(n) => Some(n),
                Err(_) => {
                    eprintln!("warning: {flag} expects a non-negative integer; got {s:?}, using default {default_for_msg}");
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
///
/// Optional policy timeline input can be provided via:
/// - `--policy-timeline <json-file>`
/// - `--policy-timeline-json '<json-array-or-object>'`
fn run_replay(source: &str, timeline: Option<&[PolicyTimelineEntry]>) {
    use activesync_core::{replay, replay_with_policy_timeline, unpack_nodes};

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

    if let Some(tl) = timeline {
        eprintln!(
            "Replaying {} node(s) with {} policy timeline entrie(s)...",
            nodes.len(),
            tl.len()
        );
    } else {
        eprintln!("Replaying {} node(s)...", nodes.len());
    }

    let state = match timeline {
        Some(tl) => replay_with_policy_timeline(&nodes, tl),
        None => replay(&nodes, None),
    }
    .unwrap_or_else(|e| {
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

#[derive(Debug)]
enum ReplayTimelineArg {
    File(std::path::PathBuf),
    Json(String),
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ReplayTimelinePayload {
    Entries(Vec<PolicyTimelineEntry>),
    Wrapped { timeline: Vec<PolicyTimelineEntry> },
}

fn load_replay_policy_timeline(args: &[String]) -> Result<Option<Vec<PolicyTimelineEntry>>, String> {
    let arg = parse_replay_policy_timeline_arg(args)?;
    let Some(arg) = arg else {
        return Ok(None);
    };

    let raw = match arg {
        ReplayTimelineArg::File(path) => {
            if path.as_os_str() == "-" {
                return Err("--policy-timeline '-' is not supported; pass a JSON file path or use --policy-timeline-json".to_string());
            }
            std::fs::read_to_string(&path)
                .map_err(|e| format!("failed to read policy timeline file {}: {e}", path.display()))?
        }
        ReplayTimelineArg::Json(json) => json,
    };

    let payload: ReplayTimelinePayload = serde_json::from_str(&raw)
        .map_err(|e| format!("invalid policy timeline JSON: {e}"))?;

    let timeline = match payload {
        ReplayTimelinePayload::Entries(entries) => entries,
        ReplayTimelinePayload::Wrapped { timeline } => timeline,
    };
    Ok(Some(timeline))
}

fn parse_replay_policy_timeline_arg(args: &[String]) -> Result<Option<ReplayTimelineArg>, String> {
    let mut file_value: Option<String> = None;
    let mut json_value: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        let a = &args[i];

        if a == "--policy-timeline" {
            let Some(v) = args.get(i + 1) else {
                return Err("--policy-timeline requires a path argument".to_string());
            };
            file_value = Some(v.clone());
            i += 2;
            continue;
        }
        if let Some(v) = a.strip_prefix("--policy-timeline=") {
            file_value = Some(v.to_string());
            i += 1;
            continue;
        }

        if a == "--policy-timeline-json" {
            let Some(v) = args.get(i + 1) else {
                return Err("--policy-timeline-json requires a JSON argument".to_string());
            };
            json_value = Some(v.clone());
            i += 2;
            continue;
        }
        if let Some(v) = a.strip_prefix("--policy-timeline-json=") {
            json_value = Some(v.to_string());
            i += 1;
            continue;
        }

        i += 1;
    }

    if file_value.is_some() && json_value.is_some() {
        return Err("use only one of --policy-timeline or --policy-timeline-json".to_string());
    }

    if let Some(path) = file_value {
        return Ok(Some(ReplayTimelineArg::File(std::path::PathBuf::from(path))));
    }
    if let Some(json) = json_value {
        return Ok(Some(ReplayTimelineArg::Json(json)));
    }
    Ok(None)
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

#[cfg(test)]
mod tests {
    use super::*;
    use activesync_core::{Policy, PolicyDefault, PolicyRule};

    fn sample_timeline_json() -> String {
        let entry = PolicyTimelineEntry {
            effective_lamport: 2,
            policy: Policy {
                rules: vec![PolicyRule {
                    path_glob: "protected/**".to_string(),
                    can_write: vec![[1u8; 32]],
                    can_read: vec![],
                    can_derive: vec![],
                }],
                default: PolicyDefault::DenyAll,
            },
        };
        serde_json::to_string(&vec![entry]).unwrap()
    }

    #[test]
    fn parse_replay_policy_timeline_arg_prefers_file_flag() {
        let args = vec![
            "activesync-server".to_string(),
            "replay".to_string(),
            "pack.b64".to_string(),
            "--policy-timeline".to_string(),
            "timeline.json".to_string(),
        ];

        let parsed = parse_replay_policy_timeline_arg(&args).unwrap();
        assert!(matches!(parsed, Some(ReplayTimelineArg::File(_))));
    }

    #[test]
    fn parse_replay_policy_timeline_arg_rejects_conflicting_flags() {
        let args = vec![
            "activesync-server".to_string(),
            "replay".to_string(),
            "pack.b64".to_string(),
            "--policy-timeline=timeline.json".to_string(),
            "--policy-timeline-json=[]".to_string(),
        ];

        assert!(parse_replay_policy_timeline_arg(&args).is_err());
    }

    #[test]
    fn load_replay_policy_timeline_from_inline_json() {
        let args = vec![
            "activesync-server".to_string(),
            "replay".to_string(),
            "pack.b64".to_string(),
            format!("--policy-timeline-json={}", sample_timeline_json()),
        ];

        let timeline = load_replay_policy_timeline(&args).unwrap().unwrap();
        assert_eq!(timeline.len(), 1);
        assert_eq!(timeline[0].effective_lamport, 2);
    }
}
