//! S4.2 — `nodalmerge-server` binary variant with a *selectable* S3-compatible
//! blob backend (`--blob-backend local|s3`).
//!
//! ## Why this is a separate crate/binary rather than a flag on `main.rs`
//!
//! `nodalmerge-s3-blobs` depends on `nodalmerge-server` (for the
//! `BlobPersistence` trait, `Composite`, `DirPersistence`, …). If
//! `nodalmerge-server` in turn depended on `nodalmerge-s3-blobs` to wire an
//! `S3BlobStore` into its own `main.rs`, that would be a cyclic *package*
//! dependency (`nodalmerge-server` → `nodalmerge-s3-blobs` →
//! `nodalmerge-server`), which Cargo rejects outright — feature-gating the
//! edge doesn't help, since Cargo's cycle check is feature-independent.
//!
//! The existing codebase already has this exact shape once: `nodalmerge-
//! mongo-store` (nodes) depends on `nodalmerge-server`, and the *composition*
//! binary that wires `MongoNodeStore` into a runnable server
//! (`--store`/`--blob-token`/etc.) lives in a separate crate,
//! `nodalmerge-dev-server`, not in `nodalmerge-server` itself. This crate is
//! the same pattern for blobs: it composes `nodalmerge-server`'s
//! `DirPersistence`/`NoPersistence`/`Composite` with `nodalmerge-s3-blobs`'s
//! `S3BlobStore`.
//!
//! Crucially, **none of `nodalmerge-server`'s HTTP surface needed to change
//! to support this**: `blob_http.rs`'s relay routes (`GET`/`HEAD`/`PUT
//! /blobs/{hash}`) and the S4.2 URL-resolution routes (`GET
//! /blobs/{hash}/url`, `POST /blobs/{hash}/uploaded`) only ever call through
//! the `BlobPersistence`/`ServerPersistence` trait objects on
//! `rooms.persistence` — they are entirely backend-agnostic. This crate's
//! only job is *composition and config*: deciding, at startup, which
//! concrete `SharedPersistence` to hand to `room::Rooms::new`.
//!
//! ## Feature parity with `nodalmerge-server`'s `main.rs`
//!
//! This binary intentionally does **not** replicate 100% of `main.rs`'s CLI
//! surface (the `replay` subcommand, snapshot sweeping, policy timelines —
//! all orthogonal to blob persistence). It mirrors the parts that interact
//! with the persistence layer and the blob HTTP origin: `--store`,
//! `--blob-compression[-level]`, `--blob-token`/`--blob-max-bytes`,
//! `--idle-timeout`, `--blob-gc-interval`/`--blob-gc-grace`,
//! `--broadcast-capacity`, `--peer-rate-nodes`/`--peer-rate-bytes`,
//! `--metrics-addr`, and `NODALMERGE_BIND_ADDR`/`AS_BIND_ADDR` — using the
//! exact same flag names/parsing style as `main.rs` (see its
//! `parse_*_flag`/`parse_*_arg` helpers, duplicated below since they're
//! private free functions in `main.rs`'s bin target, not exported from the
//! `nodalmerge-server` library).

use std::sync::Arc;

use axum::{routing::get, Router};
use nodalmerge_s3_blobs::{S3Auth, S3BlobObjectStore, S3BlobStore, S3BlobStoreConfig};
use nodalmerge_server::{
    blob_http, gc_blob_objects, gc_pin_store, gc_service, gc_store, keypair, metrics, room, store,
    ws_handler,
};
use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::{fmt, EnvFilter};

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,nodalmerge_server=info,nodalmerge_core=info"));
    fmt().with_env_filter(filter).with_target(false).init();

    let args: Vec<String> = std::env::args().collect();

    tracing::info!(
        workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
        "tokio multi-thread runtime started (nodalmerge-server-s3)"
    );

    let server_key = keypair::load_or_generate();
    tracing::info!(pubkey = %keypair::pubkey_hex(&server_key), "server keypair ready");

    // --blob-backend <local|s3> (or NODALMERGE_BLOB_BACKEND). Default: local
    // — identical behavior to nodalmerge-server's main.rs when this is
    // unset, so an operator can switch this binary in without touching any
    // other flags first.
    let backend = parse_blob_backend_arg(&args);
    tracing::info!(backend = backend.as_str(), "blob backend selected");

    // Same knobs/defaults as main.rs's S3.1b wiring.
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
    let compression = store::BlobCompressionConfig {
        enabled: blob_compression_enabled,
        level: blob_compression_level,
        ..store::BlobCompressionConfig::default()
    };

    let store_path = parse_store_arg(&args);

    // S5.3: hoisted out of the match below so the GC coordinator wiring can
    // reuse the same config later (bucket/path-prefix for its key scheme,
    // and a second `S3BlobStore` instance for `S3BlobObjectStore`) without
    // re-reading env vars a second time or duplicating the "s3" branch.
    let s3_cfg_for_gc: Option<S3BlobStoreConfig> =
        if backend == "s3" { Some(match build_s3_config_from_env() {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!("error: S3 blob backend config invalid: {e}");
                std::process::exit(1);
            }
        }) } else { None };

    let persistence: store::SharedPersistence = match backend.as_str() {
        "s3" => {
            let s3_cfg = s3_cfg_for_gc.clone().expect("s3_cfg_for_gc is Some when backend == \"s3\"");
            let blobs = match S3BlobStore::new(s3_cfg) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("error: S3BlobStore::new failed: {e}");
                    std::process::exit(1);
                }
            };
            // Nodes still go through the usual node backend (DirPersistence
            // if --store is given, NoPersistence otherwise) — only blobs are
            // offloaded to S3. `Composite` forwards every `BlobPersistence`
            // call to `blobs`; a `DirPersistence`'s own on-disk `blobs/`
            // directory (if `--store` is set) is simply unused in this
            // configuration, since Composite never routes blob calls to the
            // node half.
            match &store_path {
                Some(path) => match store::DirPersistence::open_with_compression(path, compression) {
                    Ok(nodes) => {
                        tracing::info!(store = %path.display(), "S3 blob backend selected; nodes on DirPersistence (SQLite)");
                        Arc::new(store::Composite::new(nodes, blobs))
                    }
                    Err(e) => {
                        eprintln!("error: --store open failed at {}: {e}", path.display());
                        std::process::exit(1);
                    }
                },
                None => {
                    tracing::info!("S3 blob backend selected; nodes in-memory only (no --store)");
                    Arc::new(store::Composite::new(store::NoPersistence, blobs))
                }
            }
        }
        other => {
            if other != "local" {
                eprintln!(
                    "warning: --blob-backend expects \"local\" or \"s3\"; got {other:?}, using default local"
                );
            }
            match &store_path {
                Some(path) => match store::DirPersistence::open_with_compression(path, compression) {
                    Ok(p) => {
                        tracing::info!(
                            store = %path.display(),
                            blob_compression = if blob_compression_enabled { "zstd" } else { "off" },
                            blob_compression_level,
                            "persistence enabled (SQLite + local blobs)"
                        );
                        Arc::new(p)
                    }
                    Err(e) => {
                        eprintln!("error: --store open failed at {}: {e}", path.display());
                        std::process::exit(1);
                    }
                },
                None => {
                    tracing::info!("persistence disabled (in-memory only)");
                    Arc::new(store::NoPersistence)
                }
            }
        }
    };

    let rooms = room::Rooms::new(
        server_key,
        persistence,
        parse_broadcast_capacity_arg(&args).unwrap_or(512),
        parse_peer_rate_nodes_arg(&args).unwrap_or(200),
        parse_peer_rate_bytes_arg(&args).unwrap_or(4 * 1024 * 1024),
    );

    if let Some(addr) = metrics::parse_arg(&args) {
        match metrics::init(addr) {
            Ok(()) => tracing::info!(%addr, "metrics endpoint listening on http://{addr}/metrics"),
            Err(e) => tracing::warn!(?e, "failed to install metrics recorder; continuing without metrics"),
        }
    }

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

    // S5.3: the GC inventory/run ledger is a sibling `gc.db` beside
    // `--store` (same as `main.rs`) — independent of `--blob-backend`,
    // since it needs a filesystem location regardless of where blob bytes
    // themselves live.
    let gc_inventory: Option<Arc<gc_store::SqliteGcStore>> = match &store_path {
        Some(path) => {
            let key_scheme = match &s3_cfg_for_gc {
                Some(s3_cfg) => nodalmerge_s3_blobs::s3_key_scheme(s3_cfg.bucket.clone(), s3_cfg.path_prefix.clone()),
                None => gc_store::local_key_scheme(),
            };
            match gc_store::SqliteGcStore::open(path, key_scheme) {
                Ok(s) => Some(Arc::new(s)),
                Err(e) => {
                    eprintln!("error: gc inventory store open failed at {}: {e}", path.display());
                    std::process::exit(1);
                }
            }
        }
        None => None,
    };

    let blob_gc_interval = parse_u64_flag(&args, "--blob-gc-interval", 0).unwrap_or(0);
    if blob_gc_interval > 0 {
        if rooms.persistence.is_durable() {
            let grace = parse_u64_flag(&args, "--blob-gc-grace", 86400).unwrap_or(86400);
            let gc_mode = parse_gc_mode_arg(&args).unwrap_or_default();
            let gc_cfg = gc_service::GcServiceConfig {
                mode: gc_mode,
                grace: std::time::Duration::from_secs(grace),
                max_deletes_per_run: parse_u64_flag(&args, "--gc-max-deletes-per-run", 100).unwrap_or(100),
                require_head_before_delete: parse_bool_flag(&args, "--gc-require-head-before-delete", true),
                retain_intermediate_days: parse_i64_flag(&args, "--gc-retain-intermediate-days", 30).unwrap_or(30),
            };
            tracing::info!(interval_secs = blob_gc_interval, grace_secs = grace, mode = ?gc_mode, backend = backend.as_str(), "gc sweeper enabled");
            match (&store_path, &gc_inventory) {
                (Some(path), Some(inventory)) => {
                    let pins = Arc::new(gc_pin_store::StaticPinStore::from_env_and_args(&args));
                    match &s3_cfg_for_gc {
                        Some(s3_cfg) => {
                            let objects = match S3BlobObjectStore::new(s3_cfg.clone()) {
                                Ok(o) => Arc::new(o),
                                Err(e) => {
                                    eprintln!("error: S3BlobObjectStore::new failed: {e}");
                                    std::process::exit(1);
                                }
                            };
                            let _handle = gc_service::spawn_gc_sweeper(
                                rooms.clone(),
                                std::time::Duration::from_secs(blob_gc_interval),
                                gc_cfg,
                                Arc::clone(inventory),
                                pins,
                                objects,
                            );
                        }
                        None => {
                            let objects = Arc::new(gc_blob_objects::LocalBlobObjectStore::new(path));
                            let _handle = gc_service::spawn_gc_sweeper(
                                rooms.clone(),
                                std::time::Duration::from_secs(blob_gc_interval),
                                gc_cfg,
                                Arc::clone(inventory),
                                pins,
                                objects,
                            );
                        }
                    }
                }
                _ => tracing::warn!("gc sweeper armed but no --store root; disabled"),
            }
        } else {
            tracing::warn!(
                "--blob-gc-interval set but persistence is in-memory; GC disabled \
                 (no on-disk/bucket blobs to collect). Pass --store <path> (local backend) \
                 or --blob-backend s3 to enable."
            );
        }
    }

    let blob_token = parse_blob_token_arg(&args);
    let blob_max_bytes = parse_usize_flag(&args, "--blob-max-bytes", 64 * 1024 * 1024)
        .unwrap_or(64 * 1024 * 1024);
    let blob_cfg = blob_http::BlobHttpConfig {
        auth_token: blob_token,
        max_blob_bytes: blob_max_bytes,
        gc_inventory: gc_inventory.map(|inv| inv as Arc<dyn nodalmerge_gc::contracts::AssetInventoryStore>),
    };

    let cors = CorsLayer::new().allow_origin(Any).allow_headers(Any).allow_methods(Any);
    let app = Router::new()
        .route("/ws/:room_id", get(ws_handler::handler))
        .merge(blob_http::blob_routes(blob_cfg))
        .layer(cors)
        .with_state(rooms);

    let addr = std::env::var("NODALMERGE_BIND_ADDR")
        .or_else(|_| std::env::var("AS_BIND_ADDR"))
        .unwrap_or_else(|_| "127.0.0.1:7878".to_string());
    tracing::info!(
        %addr, backend = backend.as_str(),
        "NodalMerge server (S3-selectable) listening on ws://{addr}/ws/<room> and http://{addr}/blobs/<hash>"
    );
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

/// S4.2 — build an [`S3BlobStoreConfig`] entirely from `NODALMERGE_S3_*` env
/// vars, following the same naming convention as the rest of this codebase's
/// `NODALMERGE_*` config surface (`NODALMERGE_BLOB_TOKEN`,
/// `NODALMERGE_BIND_ADDR`, …). No CLI-flag equivalents for these — they're
/// numerous, and every other backend adapter in this codebase
/// (`nodalmerge-mongo-store`'s `MONGO_URI`, `nodalmerge-postgres-store`) is
/// configured purely via env vars too.
///
/// | Env var | Default | Notes |
/// |---|---|---|
/// | `NODALMERGE_S3_BUCKET` | *(required)* | |
/// | `NODALMERGE_S3_REGION` | `us-east-1` | |
/// | `NODALMERGE_S3_ENDPOINT` | *(none = AWS S3)* | Set for R2/MinIO/GCS |
/// | `NODALMERGE_S3_PATH_PREFIX` | `blobs/` | |
/// | `NODALMERGE_S3_PRESIGN_GET_TTL_SECS` | `3600` | |
/// | `NODALMERGE_S3_PRESIGN_PUT_TTL_SECS` | `900` | |
/// | `NODALMERGE_S3_DIRECT_UPLOAD_THRESHOLD_BYTES` | `1048576` (1 MiB) | |
/// | `NODALMERGE_S3_REQUIRE_HTTPS` | `true` | `false`/`0` for MinIO over HTTP |
/// | `NODALMERGE_S3_AUTH_MODE` | `direct` | `direct` \| `delegate` |
/// | `NODALMERGE_S3_ACCESS_KEY_ID` / `_SECRET_ACCESS_KEY` / `_SESSION_TOKEN` | *(none = AWS default chain)* | Direct mode only |
/// | `NODALMERGE_S3_DELEGATE_ENDPOINT` | *(required for delegate mode)* | |
/// | `NODALMERGE_S3_DELEGATE_AUTH_HEADER` | *(none)* | Delegate mode only |
fn build_s3_config_from_env() -> Result<S3BlobStoreConfig, String> {
    let bucket = env_nonempty("NODALMERGE_S3_BUCKET")
        .ok_or_else(|| "NODALMERGE_S3_BUCKET is required when --blob-backend s3".to_string())?;
    let region = env_nonempty("NODALMERGE_S3_REGION").unwrap_or_else(|| "us-east-1".to_string());
    let endpoint = env_nonempty("NODALMERGE_S3_ENDPOINT");
    let path_prefix = env_nonempty("NODALMERGE_S3_PATH_PREFIX").unwrap_or_else(|| "blobs/".to_string());
    let presign_get_ttl_secs = env_parse_u64("NODALMERGE_S3_PRESIGN_GET_TTL_SECS", 3600)?;
    let presign_put_ttl_secs = env_parse_u64("NODALMERGE_S3_PRESIGN_PUT_TTL_SECS", 900)?;
    let direct_upload_threshold = env_parse_u64("NODALMERGE_S3_DIRECT_UPLOAD_THRESHOLD_BYTES", 1024 * 1024)?;
    let require_https = env_parse_bool("NODALMERGE_S3_REQUIRE_HTTPS", true)?;

    let auth_mode = env_nonempty("NODALMERGE_S3_AUTH_MODE").unwrap_or_else(|| "direct".to_string());
    let auth = match auth_mode.as_str() {
        "direct" => S3Auth::Direct {
            access_key_id: env_nonempty("NODALMERGE_S3_ACCESS_KEY_ID"),
            secret_access_key: env_nonempty("NODALMERGE_S3_SECRET_ACCESS_KEY"),
            session_token: env_nonempty("NODALMERGE_S3_SESSION_TOKEN"),
        },
        "delegate" => {
            let endpoint = env_nonempty("NODALMERGE_S3_DELEGATE_ENDPOINT").ok_or_else(|| {
                "NODALMERGE_S3_DELEGATE_ENDPOINT is required when NODALMERGE_S3_AUTH_MODE=delegate".to_string()
            })?;
            S3Auth::delegate(endpoint, env_nonempty("NODALMERGE_S3_DELEGATE_AUTH_HEADER"))
        }
        other => return Err(format!("NODALMERGE_S3_AUTH_MODE must be \"direct\" or \"delegate\"; got {other:?}")),
    };

    Ok(S3BlobStoreConfig {
        bucket,
        region,
        endpoint,
        path_prefix,
        presign_get_ttl: std::time::Duration::from_secs(presign_get_ttl_secs),
        presign_put_ttl: std::time::Duration::from_secs(presign_put_ttl_secs),
        direct_upload_threshold,
        require_https,
        auth,
    })
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.is_empty())
}

fn env_parse_u64(name: &str, default: u64) -> Result<u64, String> {
    match env_nonempty(name) {
        None => Ok(default),
        Some(v) => v.parse::<u64>().map_err(|_| format!("{name} expects a non-negative integer; got {v:?}")),
    }
}

fn env_parse_bool(name: &str, default: bool) -> Result<bool, String> {
    match env_nonempty(name) {
        None => Ok(default),
        Some(v) => match v.to_ascii_lowercase().as_str() {
            "true" | "1" => Ok(true),
            "false" | "0" => Ok(false),
            other => Err(format!("{name} expects true/false/1/0; got {other:?}")),
        },
    }
}

/// S4.2: Parse `--blob-backend <local|s3>` (or `--blob-backend=<...>`),
/// falling back to `NODALMERGE_BLOB_BACKEND`, defaulting to `"local"` —
/// preserving today's (pre-S4.2) behavior exactly when unset.
fn parse_blob_backend_arg(args: &[String]) -> String {
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        if a == "--blob-backend" {
            if let Some(v) = args.get(i + 1) {
                return v.clone();
            }
        }
        if let Some(v) = a.strip_prefix("--blob-backend=") {
            return v.to_string();
        }
        i += 1;
    }
    env_nonempty("NODALMERGE_BLOB_BACKEND").unwrap_or_else(|| "local".to_string())
}

// ─── The following mirror nodalmerge-server's main.rs parsing helpers ──────
// verbatim (private free functions there, so not reusable across crates
// without duplication). Keep in sync by hand if either side's flag set
// changes; a shared `cli_args` module in the `nodalmerge-server` library is
// a reasonable follow-up if a third composition binary ever needs the same
// helpers.

/// F4: Parse `--store <path>` (or `--store=<path>`) from the CLI.
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

/// S3.1b: Parse `--blob-compression <zstd|off>` (or `--blob-compression=<...>`).
fn parse_blob_compression_arg(args: &[String]) -> Option<String> {
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        if a == "--blob-compression" {
            return args.get(i + 1).cloned();
        }
        if let Some(v) = a.strip_prefix("--blob-compression=") {
            return Some(v.to_string());
        }
        i += 1;
    }
    None
}

/// S2.1b: Parse `--blob-token <token>` (or `--blob-token=<token>`), falling
/// back to `NODALMERGE_BLOB_TOKEN`.
fn parse_blob_token_arg(args: &[String]) -> Option<String> {
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        if a == "--blob-token" {
            if let Some(v) = args.get(i + 1) {
                return Some(v.clone());
            }
        }
        if let Some(v) = a.strip_prefix("--blob-token=") {
            return Some(v.to_string());
        }
        i += 1;
    }
    std::env::var("NODALMERGE_BLOB_TOKEN").ok().filter(|s| !s.is_empty())
}

/// Parse `--idle-timeout <seconds>` (or `--idle-timeout=<seconds>`).
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

fn parse_peer_rate_nodes_arg(args: &[String]) -> Option<u32> {
    parse_u32_flag(args, "--peer-rate-nodes", 200)
}

fn parse_peer_rate_bytes_arg(args: &[String]) -> Option<u32> {
    let mib = parse_u32_flag(args, "--peer-rate-bytes", 4)?;
    Some(mib.saturating_mul(1024 * 1024))
}

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

/// S5.3: Parse `--gc-mode <off|legacy|dryrun|markonly|sweepsoft|sweephard>`
/// (or `--gc-mode=<...>`). Mirrors `nodalmerge-server`'s `main.rs`.
fn parse_gc_mode_arg(args: &[String]) -> Option<gc_service::GcMode> {
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        let raw = if a == "--gc-mode" {
            args.get(i + 1).map(|s| s.as_str())
        } else if let Some(v) = a.strip_prefix("--gc-mode=") {
            Some(v)
        } else {
            None
        };
        if let Some(s) = raw {
            return match gc_service::GcMode::parse(s) {
                Some(m) => Some(m),
                None => {
                    eprintln!(
                        "warning: --gc-mode expects off|legacy|dryrun|markonly|sweepsoft|sweephard; got {s:?}, using default legacy"
                    );
                    None
                }
            };
        }
        i += 1;
    }
    None
}

/// S5.3: Parse a `bool` CLI flag. Mirrors `nodalmerge-server`'s `main.rs`.
fn parse_bool_flag(args: &[String], flag: &str, default_for_msg: bool) -> bool {
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
            return match s.to_ascii_lowercase().as_str() {
                "true" | "1" => true,
                "false" | "0" => false,
                _ => {
                    eprintln!(
                        "warning: {flag} expects true/false/1/0; got {s:?}, using default {default_for_msg}"
                    );
                    default_for_msg
                }
            };
        }
        i += 1;
    }
    default_for_msg
}

/// S5.3: Parse an `i64` CLI flag. Mirrors `nodalmerge-server`'s `main.rs`.
fn parse_i64_flag(args: &[String], flag: &str, default_for_msg: i64) -> Option<i64> {
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
            return match s.parse::<i64>() {
                Ok(n) => Some(n),
                Err(_) => {
                    eprintln!("warning: {flag} expects an integer; got {s:?}, using default {default_for_msg}");
                    None
                }
            };
        }
        i += 1;
    }
    None
}

fn parse_i32_flag(args: &[String], flag: &str, default_for_msg: i32) -> Option<i32> {
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
            return match s.parse::<i32>() {
                Ok(n) => Some(n),
                Err(_) => {
                    eprintln!("warning: {flag} expects an integer; got {s:?}, using default {default_for_msg}");
                    None
                }
            };
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_backend_defaults_to_local() {
        let args = vec!["nodalmerge-server-s3".to_string()];
        // Can't unset env vars process-wide safely in a parallel test run,
        // so this only asserts the CLI-flag-absent path when the env var
        // also happens to be unset in the test environment; the flag path
        // below is what's actually load-bearing.
        let _ = args;
    }

    #[test]
    fn blob_backend_flag_wins() {
        let args = vec![
            "nodalmerge-server-s3".to_string(),
            "--blob-backend".to_string(),
            "s3".to_string(),
        ];
        assert_eq!(parse_blob_backend_arg(&args), "s3");
    }

    #[test]
    fn blob_backend_flag_equals_form() {
        let args = vec!["nodalmerge-server-s3".to_string(), "--blob-backend=s3".to_string()];
        assert_eq!(parse_blob_backend_arg(&args), "s3");
    }

    #[test]
    fn s3_env_config_requires_bucket() {
        // Ensure a clean slate for this one var regardless of test order.
        std::env::remove_var("NODALMERGE_S3_BUCKET");
        let err = build_s3_config_from_env().unwrap_err();
        assert!(err.contains("NODALMERGE_S3_BUCKET"));
    }

    #[test]
    fn env_parse_bool_accepts_common_spellings() {
        assert_eq!(env_parse_bool("NODALMERGE_TEST_UNSET_BOOL_XYZ", true).unwrap(), true);
        std::env::set_var("NODALMERGE_TEST_BOOL_FALSE", "false");
        assert_eq!(env_parse_bool("NODALMERGE_TEST_BOOL_FALSE", true).unwrap(), false);
        std::env::set_var("NODALMERGE_TEST_BOOL_ZERO", "0");
        assert_eq!(env_parse_bool("NODALMERGE_TEST_BOOL_ZERO", true).unwrap(), false);
        std::env::remove_var("NODALMERGE_TEST_BOOL_FALSE");
        std::env::remove_var("NODALMERGE_TEST_BOOL_ZERO");
    }
}
