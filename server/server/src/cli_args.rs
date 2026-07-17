//! Slice 7.1 (blob-cas-remediation.md): shared CLI-flag parsing + GC/blob
//! bootstrap helpers, extracted out of `server/main.rs` and
//! `server-s3/main.rs`, which had carried byte-for-byte identical copies of
//! every function below (server-s3's copy carried a "keep in sync by hand"
//! comment suggesting exactly this module — named `cli_args` on purpose,
//! matching that comment). Both binaries now call through here; there is
//! exactly one definition of each flag parser and of the GC-sweeper
//! bootstrap it feeds.
//!
//! ## What stayed local (deliberately NOT moved here)
//! - server-s3's `--blob-backend`/`NODALMERGE_S3_*` env-var config
//!   (`build_s3_config_from_env` & friends in `server-s3/src/main.rs`) is
//!   genuinely S3-only and has no twin in `main.rs` at all — stays put per
//!   the slice's "genuinely s3-only, no CLI-flag duplication" carve-out
//!   (same posture 6.1 already established for the `NODALMERGE_S3_*`
//!   timeout vars).
//! - The final object-store selection + `spawn_gc_sweeper` call:
//!   `main.rs` always builds a `LocalBlobObjectStore`; `server-s3`
//!   additionally branches on `--blob-backend` to build an
//!   `S3BlobObjectStore` instead. That is a real capability difference
//!   between the two binaries, not accidental drift, so it stays local to
//!   each `main.rs`. Everything upstream of it — flag parsing,
//!   `GcServiceConfig` construction, the studio-union live-hash collector,
//!   the admin pin store — was byte-for-byte identical and now lives once,
//!   in [`parse_gc_sweep_common`].
//! - `dev-server` never spawns GC (7.5) and reads its blob-token/max-bytes
//!   config from env vars only, with no CLI-flag fallback at all — a
//!   genuinely different config surface (env-only vs. CLI-with-env-
//!   fallback), not a drifted copy of these parsers. It is deliberately
//!   NOT switched onto `parse_blob_token_arg`/`parse_usize_flag` here:
//!   doing so would silently *add* a `--blob-token`/`--blob-max-bytes` CLI
//!   flag to a binary that never accepted one, which is a behavior change,
//!   not a refactor. `dev-server` does already share the one thing it
//!   genuinely has in common with the other two binaries — `metrics::parse_arg`
//!   — via the pre-existing `metrics` module, untouched by this slice.
//!
//! ## Duplication-inventory classification (recorded here, not just in the
//! slice report, since this module IS the resolution)
//! Every one of the 14 flag parsers below was **identical** between the two
//! `main.rs` files at the code-statement level; the only diffs were doc
//! comments trimmed in `server-s3`'s copy (its file-level comment said as
//! much: "duplicated below since they're private free functions in
//! `main.rs`'s bin target, not exported from the `nodalmerge-server`
//! library"). No drifted-meaningful flag-parsing block was found. The
//! GC/blob wiring block *did* carry a meaningful, deliberate divergence
//! (the object-store selection above) — captured by leaving it local
//! rather than forcing a false merge.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::{gc_pin_store, gc_service, room, studio_live_hashes};

/// F4: Parse `--store <path>` (or `--store=<path>`) from the CLI.
/// Returns `None` when absent, so the caller applies the default (in-memory
/// persistence only).
pub fn parse_store_arg(args: &[String]) -> Option<PathBuf> {
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        if a == "--store" {
            return args.get(i + 1).map(PathBuf::from);
        }
        if let Some(val) = a.strip_prefix("--store=") {
            return Some(PathBuf::from(val));
        }
        i += 1;
    }
    None
}

/// S3.1b: Parse `--blob-compression <zstd|off>` (or `--blob-compression=<...>`).
/// Returns `None` when absent, so the caller applies the default (`zstd`,
/// i.e. on) — see `docs/BLOB_STORAGE_LAYOUT.md` §8.
pub fn parse_blob_compression_arg(args: &[String]) -> Option<String> {
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
/// back to the `NODALMERGE_BLOB_TOKEN` env var when the flag is absent.
/// `None` means the blob HTTP origin is anonymous (no token configured).
pub fn parse_blob_token_arg(args: &[String]) -> Option<String> {
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
/// `0` disables eviction. Returns `None` to fall through to the default
/// (300 s, 5 min). Invalid values also fall back to the default with a
/// warning — the server does not refuse to start on a typo.
pub fn parse_idle_timeout_arg(args: &[String]) -> Option<u64> {
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
pub fn parse_broadcast_capacity_arg(args: &[String]) -> Option<usize> {
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
pub fn parse_peer_rate_nodes_arg(args: &[String]) -> Option<u32> {
    parse_u32_flag(args, "--peer-rate-nodes", 200)
}

/// G3: Parse `--peer-rate-bytes <MIB>` (or `--peer-rate-bytes=<MIB>`), the
/// per-peer ceiling on inbound decoded-pack *bytes* per second. The CLI
/// value is in MiB for ergonomics; we convert to bytes here. Returns
/// `None` to fall through to the default (4 MiB/s = 4 194 304 B/s). `0`
/// explicitly disables the byte-rate limiter. Values that would overflow
/// `u32` after MiB→bytes conversion fall back to the default with a
/// warning.
pub fn parse_peer_rate_bytes_arg(args: &[String]) -> Option<u32> {
    // Read as u32 MiB, multiply by 1 MiB, saturating (u32::MAX ≈ 4 GiB).
    let mib = parse_u32_flag(args, "--peer-rate-bytes", 4)?;
    Some(mib.saturating_mul(1024 * 1024))
}

/// Shared helper for `--peer-rate-*` flags: parses a non-negative `u32`.
/// `default_for_msg` is only used in the warning text so the operator
/// sees the correct fallback per flag. Returns `None` when the flag is
/// absent or invalid.
pub fn parse_u32_flag(args: &[String], flag: &str, default_for_msg: u32) -> Option<u32> {
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
pub fn parse_u64_flag(args: &[String], flag: &str, default_for_msg: u64) -> Option<u64> {
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
pub fn parse_usize_flag(args: &[String], flag: &str, default_for_msg: usize) -> Option<usize> {
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
/// (or `--gc-mode=<...>`). `None` when absent or unrecognized — callers fall
/// back to `GcMode::default()` (`Legacy`, preserving pre-S5.3 behavior).
pub fn parse_gc_mode_arg(args: &[String]) -> Option<gc_service::GcMode> {
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

/// S5.3: Parse a `bool` CLI flag (e.g. `--gc-require-head-before-delete
/// false`). Accepts `true`/`false`/`1`/`0` (case-insensitive). Returns
/// `default_for_msg` (not `Option`, since every caller of this flag wants a
/// concrete value, not a further fallback decision) when absent or invalid.
pub fn parse_bool_flag(args: &[String], flag: &str, default_for_msg: bool) -> bool {
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

/// S5.3: Parse an `i64` CLI flag (e.g. `--gc-retain-intermediate-days 30`).
/// Mirrors `parse_i32_flag`; signed since it's compared against an `i128`
/// nanosecond timestamp domain, not used as a byte-count/index.
pub fn parse_i64_flag(args: &[String], flag: &str, default_for_msg: i64) -> Option<i64> {
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

/// S3.1b: Parse an `i32` CLI flag (e.g. `--blob-compression-level 3`).
/// Mirrors `parse_usize_flag`; signed because zstd's C API takes a signed
/// level (negative "fast" levels are valid, even though we default to 3).
pub fn parse_i32_flag(args: &[String], flag: &str, default_for_msg: i32) -> Option<i32> {
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

/// Slice 7.1 — the `--blob-gc-grace`/`--gc-mode`/`--gc-max-deletes-per-run`/
/// `--gc-require-head-before-delete`/`--gc-retain-intermediate-days` knobs,
/// bundled with the studio-union live-hash collector and the admin pin
/// store built from them. Both `main.rs` and `server-s3/main.rs` built this
/// exact set, in this exact order, byte-for-byte identically (only the
/// eventual object-store choice differs — see module docs).
pub struct GcSweepCommon {
    pub gc_cfg: gc_service::GcServiceConfig,
    pub live: Arc<dyn gc_service::LiveHashCollector>,
    pub pins: Arc<gc_pin_store::StaticPinStore>,
}

/// Parse the common GC-sweeper flags against `args` and assemble the
/// studio-union live-hash collector against `rooms`. Callers still own the
/// `--blob-gc-interval`/durability gating and the final object-store choice
/// + `spawn_gc_sweeper` call (server-s3's `--blob-backend` branch lives
/// there, not here).
pub fn parse_gc_sweep_common(args: &[String], rooms: &room::Rooms) -> GcSweepCommon {
    let grace = parse_u64_flag(args, "--blob-gc-grace", 86400).unwrap_or(86400);
    let gc_mode = parse_gc_mode_arg(args).unwrap_or_default();
    let gc_cfg = gc_service::GcServiceConfig {
        mode: gc_mode,
        grace: Duration::from_secs(grace),
        max_deletes_per_run: parse_u64_flag(args, "--gc-max-deletes-per-run", 100).unwrap_or(100),
        require_head_before_delete: parse_bool_flag(args, "--gc-require-head-before-delete", true),
    };
    // Slice 7.5 — `--gc-retain-intermediate-days` is a studio classification
    // knob, so it rides on the studio collector built just below, not on
    // the generic `GcServiceConfig`.
    let retain_intermediate_days =
        parse_i64_flag(args, "--gc-retain-intermediate-days", 30).unwrap_or(30);
    let pins = Arc::new(gc_pin_store::StaticPinStore::from_env_and_args(args));
    // Slice 1.2 — the source is the UNION of the studio-domain classifier
    // and the room-DAG SetBlob references (the protection the legacy sweep
    // always had): either alone under-reports, and an under-reported live
    // set is a delete list.
    let live: Arc<dyn gc_service::LiveHashCollector> =
        Arc::new(gc_service::UnionLiveHashCollector::new(vec![
            Arc::new(studio_live_hashes::StudioLiveHashCollector::new(
                rooms.clone(),
                retain_intermediate_days,
            )) as Arc<dyn gc_service::LiveHashCollector>,
            Arc::new(room::RoomDagLiveHashCollector::new(rooms.clone())),
        ]));
    GcSweepCommon { gc_cfg, live, pins }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── The golden matrix: flag → parsed value, for every flag this module
    // owns, covering absent / space form / `=` form / invalid / boundary —
    // pinned so the "keep in sync by hand" comment this module replaces can
    // never silently regress. Values mirror exactly what main.rs's and
    // server-s3/main.rs's pre-extraction copies produced (verified via a
    // line-for-line diff of both original functions before this slice).

    fn args(v: &[&str]) -> Vec<String> {
        std::iter::once("nodalmerge-server".to_string())
            .chain(v.iter().map(|s| s.to_string()))
            .collect()
    }

    #[test]
    fn store_arg_absent_is_none() {
        assert_eq!(parse_store_arg(&args(&[])), None);
    }

    #[test]
    fn store_arg_space_form() {
        assert_eq!(parse_store_arg(&args(&["--store", "C:/data"])), Some(PathBuf::from("C:/data")));
    }

    #[test]
    fn store_arg_equals_form() {
        assert_eq!(parse_store_arg(&args(&["--store=C:/data"])), Some(PathBuf::from("C:/data")));
    }

    #[test]
    fn blob_compression_absent_is_none() {
        assert_eq!(parse_blob_compression_arg(&args(&[])), None);
    }

    #[test]
    fn blob_compression_space_and_equals_forms() {
        assert_eq!(parse_blob_compression_arg(&args(&["--blob-compression", "off"])), Some("off".to_string()));
        assert_eq!(parse_blob_compression_arg(&args(&["--blob-compression=zstd"])), Some("zstd".to_string()));
    }

    #[test]
    fn blob_token_flag_wins_over_env() {
        // Flag form takes precedence and never touches the env fallback.
        assert_eq!(parse_blob_token_arg(&args(&["--blob-token", "abc"])), Some("abc".to_string()));
        assert_eq!(parse_blob_token_arg(&args(&["--blob-token=xyz"])), Some("xyz".to_string()));
    }

    #[test]
    fn blob_token_absent_falls_back_to_env() {
        std::env::set_var("NODALMERGE_BLOB_TOKEN", "from-env");
        assert_eq!(parse_blob_token_arg(&args(&[])), Some("from-env".to_string()));
        std::env::remove_var("NODALMERGE_BLOB_TOKEN");
        assert_eq!(parse_blob_token_arg(&args(&[])), None);
    }

    #[test]
    fn idle_timeout_valid_and_invalid() {
        assert_eq!(parse_idle_timeout_arg(&args(&[])), None);
        assert_eq!(parse_idle_timeout_arg(&args(&["--idle-timeout", "0"])), Some(0));
        assert_eq!(parse_idle_timeout_arg(&args(&["--idle-timeout=120"])), Some(120));
        // Invalid falls back to None (caller applies default 300) with a warning.
        assert_eq!(parse_idle_timeout_arg(&args(&["--idle-timeout", "nope"])), None);
    }

    #[test]
    fn broadcast_capacity_rejects_zero_and_invalid() {
        assert_eq!(parse_broadcast_capacity_arg(&args(&[])), None);
        assert_eq!(parse_broadcast_capacity_arg(&args(&["--broadcast-capacity", "1024"])), Some(1024));
        // Zero is explicitly rejected (tokio::sync::broadcast::channel panics on 0).
        assert_eq!(parse_broadcast_capacity_arg(&args(&["--broadcast-capacity", "0"])), None);
        assert_eq!(parse_broadcast_capacity_arg(&args(&["--broadcast-capacity=nope"])), None);
    }

    #[test]
    fn peer_rate_nodes_default_and_zero_disables() {
        assert_eq!(parse_peer_rate_nodes_arg(&args(&[])), None);
        assert_eq!(parse_peer_rate_nodes_arg(&args(&["--peer-rate-nodes", "0"])), Some(0));
        assert_eq!(parse_peer_rate_nodes_arg(&args(&["--peer-rate-nodes", "50"])), Some(50));
    }

    #[test]
    fn peer_rate_bytes_converts_mib_to_bytes() {
        assert_eq!(parse_peer_rate_bytes_arg(&args(&[])), None);
        assert_eq!(parse_peer_rate_bytes_arg(&args(&["--peer-rate-bytes", "1"])), Some(1024 * 1024));
        assert_eq!(parse_peer_rate_bytes_arg(&args(&["--peer-rate-bytes", "0"])), Some(0));
    }

    #[test]
    fn peer_rate_bytes_saturates_on_overflow() {
        // u32::MAX MiB * 1 MiB overflows u32; saturating_mul clamps to u32::MAX.
        let huge = (u32::MAX).to_string();
        assert_eq!(parse_peer_rate_bytes_arg(&args(&["--peer-rate-bytes", &huge])), Some(u32::MAX));
    }

    #[test]
    fn u64_flag_matrix() {
        assert_eq!(parse_u64_flag(&args(&[]), "--blob-gc-interval", 0), None);
        assert_eq!(parse_u64_flag(&args(&["--blob-gc-interval", "60"]), "--blob-gc-interval", 0), Some(60));
        assert_eq!(parse_u64_flag(&args(&["--blob-gc-interval=60"]), "--blob-gc-interval", 0), Some(60));
        assert_eq!(parse_u64_flag(&args(&["--blob-gc-interval", "-1"]), "--blob-gc-interval", 0), None);
    }

    #[test]
    fn usize_flag_matrix() {
        assert_eq!(parse_usize_flag(&args(&[]), "--snapshot-interval", 0), None);
        assert_eq!(parse_usize_flag(&args(&["--snapshot-interval", "100"]), "--snapshot-interval", 0), Some(100));
        assert_eq!(parse_usize_flag(&args(&["--snapshot-interval", "-5"]), "--snapshot-interval", 0), None);
    }

    #[test]
    fn gc_mode_arg_every_variant_and_invalid() {
        assert_eq!(parse_gc_mode_arg(&args(&[])), None);
        assert_eq!(parse_gc_mode_arg(&args(&["--gc-mode", "off"])), Some(gc_service::GcMode::Off));
        assert_eq!(parse_gc_mode_arg(&args(&["--gc-mode", "legacy"])), Some(gc_service::GcMode::Legacy));
        assert!(parse_gc_mode_arg(&args(&["--gc-mode=dryrun"])).is_some());
        assert!(parse_gc_mode_arg(&args(&["--gc-mode=markonly"])).is_some());
        assert!(parse_gc_mode_arg(&args(&["--gc-mode=sweepsoft"])).is_some());
        assert!(parse_gc_mode_arg(&args(&["--gc-mode=sweephard"])).is_some());
        // 1.3/S5.3's warn+default posture: unrecognized -> None, caller
        // falls back to GcMode::default() (Legacy).
        assert_eq!(parse_gc_mode_arg(&args(&["--gc-mode", "bogus"])), None);
        assert_eq!(gc_service::GcMode::default(), gc_service::GcMode::Legacy);
    }

    #[test]
    fn bool_flag_accepts_all_spellings_and_falls_back_on_garbage() {
        assert!(parse_bool_flag(&args(&[]), "--gc-require-head-before-delete", true));
        assert!(!parse_bool_flag(&args(&["--gc-require-head-before-delete", "false"]), "--gc-require-head-before-delete", true));
        assert!(!parse_bool_flag(&args(&["--gc-require-head-before-delete=0"]), "--gc-require-head-before-delete", true));
        assert!(parse_bool_flag(&args(&["--gc-require-head-before-delete=TRUE"]), "--gc-require-head-before-delete", false));
        assert!(parse_bool_flag(&args(&["--gc-require-head-before-delete", "1"]), "--gc-require-head-before-delete", false));
        // Invalid falls back to `default_for_msg` directly (not None — every
        // caller wants a concrete bool, per the doc comment).
        assert!(parse_bool_flag(&args(&["--gc-require-head-before-delete", "maybe"]), "--gc-require-head-before-delete", true));
    }

    #[test]
    fn i64_flag_matrix() {
        assert_eq!(parse_i64_flag(&args(&[]), "--gc-retain-intermediate-days", 30), None);
        assert_eq!(parse_i64_flag(&args(&["--gc-retain-intermediate-days", "0"]), "--gc-retain-intermediate-days", 30), Some(0));
        assert_eq!(parse_i64_flag(&args(&["--gc-retain-intermediate-days=-7"]), "--gc-retain-intermediate-days", 30), Some(-7));
        assert_eq!(parse_i64_flag(&args(&["--gc-retain-intermediate-days", "nope"]), "--gc-retain-intermediate-days", 30), None);
    }

    #[test]
    fn i32_flag_matrix() {
        assert_eq!(parse_i32_flag(&args(&[]), "--blob-compression-level", 3), None);
        assert_eq!(parse_i32_flag(&args(&["--blob-compression-level", "-1"]), "--blob-compression-level", 3), Some(-1));
        assert_eq!(parse_i32_flag(&args(&["--blob-compression-level=19"]), "--blob-compression-level", 3), Some(19));
    }

    fn test_rooms() -> room::Rooms {
        room::Rooms::new(
            ed25519_dalek::SigningKey::from_bytes(&[0x42; 32]),
            Arc::new(crate::store::NoPersistence),
            512,
            200,
            4 * 1024 * 1024,
        )
    }

    #[test]
    fn gc_sweep_common_defaults_match_pre_extraction_defaults() {
        let rooms = test_rooms();
        let common = parse_gc_sweep_common(&args(&[]), &rooms);
        assert_eq!(common.gc_cfg.mode, gc_service::GcMode::Legacy);
        assert_eq!(common.gc_cfg.grace, Duration::from_secs(86400));
        assert_eq!(common.gc_cfg.max_deletes_per_run, 100);
        assert!(common.gc_cfg.require_head_before_delete);
    }

    #[test]
    fn gc_sweep_common_honors_every_flag() {
        let rooms = test_rooms();
        let common = parse_gc_sweep_common(
            &args(&[
                "--blob-gc-grace", "3600",
                "--gc-mode", "sweepsoft",
                "--gc-max-deletes-per-run", "7",
                "--gc-require-head-before-delete", "false",
                "--gc-retain-intermediate-days", "5",
            ]),
            &rooms,
        );
        assert_eq!(common.gc_cfg.grace, Duration::from_secs(3600));
        assert!(matches!(common.gc_cfg.mode, gc_service::GcMode::New(_)));
        assert_eq!(common.gc_cfg.max_deletes_per_run, 7);
        assert!(!common.gc_cfg.require_head_before_delete);
    }
}
