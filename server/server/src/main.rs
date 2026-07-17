use nodalmerge_server::{
    blob_http, cli_args, gc_blob_objects, gc_service, gc_store, keypair, metrics, room, store,
    ws_handler,
};
use nodalmerge_server::cli_args::{
    parse_blob_compression_arg, parse_blob_token_arg, parse_broadcast_capacity_arg,
    parse_i32_flag, parse_idle_timeout_arg, parse_peer_rate_bytes_arg, parse_peer_rate_nodes_arg,
    parse_store_arg, parse_u64_flag, parse_usize_flag,
};

use std::sync::Arc;
use axum::{Router, routing::get};
use nodalmerge_core::PolicyTimelineEntry;
use serde::Deserialize;
use tower_http::cors::{CorsLayer, Any};
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    // Log filter: honor RUST_LOG, default to info for our crates.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,nodalmerge_server=info,nodalmerge_core=info"));
    fmt().with_env_filter(filter).with_target(false).init();

    tracing::info!(
        workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
        "tokio multi-thread runtime started"
    );
    let args: Vec<String> = std::env::args().collect();

    // D4: `nodalmerge-server replay <pack-file>` subcommand.
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

    // S3.1b: `--blob-compression zstd|off` governs at-rest blob encoding
    // (docs/BLOB_STORAGE_LAYOUT.md §8) for a `--store`-backed persistence.
    // Default is `zstd` (on) — the doc's recommended default for a
    // server-side durable store; pass `--blob-compression off` to opt out
    // and keep writing identity bytes only. `--blob-compression-level <n>`
    // (default 3) sets the zstd level used when compression is on. Neither
    // flag has any effect without `--store` (in-memory persistence never
    // touches disk).
    let blob_compression_enabled = match parse_blob_compression_arg(&args).as_deref() {
        Some("off") => false,
        Some("zstd") => true,
        Some(other) => {
            eprintln!(
                "warning: --blob-compression expects \"zstd\" or \"off\"; got {other:?}, using default zstd"
            );
            true
        }
        None => true,
    };
    let blob_compression_level = parse_i32_flag(&args, "--blob-compression-level", 3).unwrap_or(3);

    // F4: optional on-disk persistence. `--store <path>` enables a SQLite+files
    // backend rooted at `<path>`; absent it, the server is in-memory only.
    let store_path = parse_store_arg(&args);
    let persistence: store::SharedPersistence = match &store_path {
        Some(path) => {
            let compression = store::BlobCompressionConfig {
                enabled: blob_compression_enabled,
                level: blob_compression_level,
                ..store::BlobCompressionConfig::default()
            };
            match store::DirPersistence::open_with_compression(path, compression) {
                Ok(p) => {
                    tracing::info!(
                        store = %path.display(),
                        blob_compression = if blob_compression_enabled { "zstd" } else { "off" },
                        blob_compression_level,
                        "persistence enabled (SQLite + blobs)"
                    );
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

    // S5.3: the GC inventory/run ledger lives beside `--store` (a sibling
    // `gc.db`, same pattern as `nodalmerge.db`) whenever on-disk persistence
    // is configured — independent of whether the periodic sweeper below is
    // armed, so `PUT`/`uploaded`-confirm events (wired in `blob_http.rs`)
    // can start tracking assets immediately, ready for whenever an operator
    // turns on `--blob-gc-interval`/`--gc-mode`.
    let gc_inventory: Option<std::sync::Arc<gc_store::SqliteGcStore>> = match &store_path {
        Some(path) => match gc_store::SqliteGcStore::open(path, gc_store::local_key_scheme()) {
            Ok(s) => Some(std::sync::Arc::new(s)),
            Err(e) => {
                eprintln!("error: gc inventory store open failed at {}: {e}", path.display());
                std::process::exit(1);
            }
        },
        None => None,
    };

    // blob-cas-remediation.md slice 1.3 (finding #3): the same inventory
    // handle `BlobHttpConfig` writes upload rows through also feeds
    // `Rooms::sweep_blobs`'s live set, so a confirmed-but-not-yet-referenced
    // upload isn't swept out from under the client that just uploaded it.
    // Both must point at the same store or the union protects nothing. The
    // window is `room::DEFAULT_BLOB_UPLOAD_GRACE` (1 h) — no CLI flag yet;
    // that config surface lands with slice 7.1's shared bootstrap module.
    let rooms = match &gc_inventory {
        Some(inv) => rooms.with_gc_inventory(
            std::sync::Arc::clone(inv) as std::sync::Arc<dyn nodalmerge_gc::contracts::AssetInventoryStore>
        ),
        None => rooms,
    };

    // G4/S5.3: optional GC sweeper on the existing `--blob-gc-interval`
    // schedule. `--gc-mode off|legacy|dryrun|markonly|sweepsoft|sweephard`
    // (default `legacy`) picks which deletion path runs on each tick — see
    // `gc_service`'s module docs for how the two coexist. `legacy` is
    // today's exact behavior (the no-op MarkOnly preflight +
    // `blob_gc_sweep`'s tombstone/grace/delete dance) so landing this slice
    // changes nothing until an operator opts in to the new coordinator.
    let blob_gc_interval = parse_u64_flag(&args, "--blob-gc-interval", 0).unwrap_or(0);
    if blob_gc_interval > 0 {
        if rooms.persistence.is_durable() {
            // Slice 7.1 — grace/gc_mode/max_deletes/require_head/
            // retain_intermediate_days parsing + the studio-union live-hash
            // collector + the admin pin store: byte-for-byte identical to
            // server-s3/main.rs's copy, now shared in one place.
            let common = cli_args::parse_gc_sweep_common(&args, &rooms);
            tracing::info!(
                interval_secs = blob_gc_interval,
                grace_secs = common.gc_cfg.grace.as_secs(),
                mode = ?common.gc_cfg.mode,
                "gc sweeper enabled"
            );
            // `rooms.persistence.is_durable()` already guarantees
            // `store_path`/`gc_inventory` are `Some` (in-memory persistence
            // is never durable), but guard explicitly rather than assume.
            match (&store_path, &gc_inventory) {
                (Some(path), Some(inventory)) => {
                    // This binary always backs the GC object store with the
                    // local on-disk blob root (`server-s3`'s binary is the
                    // one that additionally selects an S3 object store —
                    // see its own comment; that divergence is real, not
                    // accidental, so it isn't shared here).
                    let objects = std::sync::Arc::new(gc_blob_objects::LocalBlobObjectStore::new(path));
                    let _handle = gc_service::spawn_gc_sweeper(
                        rooms.clone(),
                        std::time::Duration::from_secs(blob_gc_interval),
                        common.gc_cfg,
                        common.live,
                        std::sync::Arc::clone(inventory),
                        common.pins,
                        objects,
                    );
                }
                _ => tracing::warn!("gc sweeper armed but no --store root; disabled"),
            }
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

    // S2.1b: blob HTTP origin (`GET`/`HEAD`/`PUT /blobs/:hash`). `--blob-token`
    // (or `NODALMERGE_BLOB_TOKEN`) gates it behind a static bearer token;
    // absent either, the endpoints are anonymous. `--blob-max-bytes` caps
    // accepted PUT bodies (default 64 MiB). See docs/BLOB_HTTP_SURFACE.md.
    let blob_token = parse_blob_token_arg(&args);
    let blob_max_bytes = parse_usize_flag(&args, "--blob-max-bytes", 64 * 1024 * 1024)
        .unwrap_or(64 * 1024 * 1024);
    let blob_cfg = blob_http::BlobHttpConfig {
        auth_token: blob_token,
        max_blob_bytes: blob_max_bytes,
        gc_inventory: gc_inventory.map(|inv| inv as std::sync::Arc<dyn nodalmerge_gc::contracts::AssetInventoryStore>),
    };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_headers(Any)
        .allow_methods(Any);

    let app = Router::new()
        .route("/ws/:room_id", get(ws_handler::handler))
        .merge(blob_http::blob_routes(blob_cfg))
        .layer(cors)
        .with_state(rooms);

    let addr = std::env::var("NODALMERGE_BIND_ADDR").or_else(|_| std::env::var("AS_BIND_ADDR"))
        .unwrap_or_else(|_| "127.0.0.1:7878".to_string());
    tracing::info!(%addr, "NodalMerge server listening on ws://{addr}/ws/<room> and http://{addr}/blobs/<hash>");
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

/// D4: Replay a base64-encoded node pack and print resolved state + hash.
/// `source` is a file path, or "-" to read from stdin.
///
/// Optional policy timeline input can be provided via:
/// - `--policy-timeline <json-file>`
/// - `--policy-timeline-json '<json-array-or-object>'`
fn run_replay(source: &str, timeline: Option<&[PolicyTimelineEntry]>) {
    use nodalmerge_core::{replay, replay_with_policy_timeline, unpack_nodes};

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
    use nodalmerge_core::{Policy, PolicyDefault, PolicyRule};

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
            "nodalmerge-server".to_string(),
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
            "nodalmerge-server".to_string(),
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
            "nodalmerge-server".to_string(),
            "replay".to_string(),
            "pack.b64".to_string(),
            format!("--policy-timeline-json={}", sample_timeline_json()),
        ];

        let timeline = load_replay_policy_timeline(&args).unwrap().unwrap();
        assert_eq!(timeline.len(), 1);
        assert_eq!(timeline[0].effective_lamport, 2);
    }
}
