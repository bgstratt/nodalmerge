//! Slice 7.3 (blob-cas-remediation.md): shared proleptic-Gregorian date
//! conversion. Previously duplicated as one direction each: `blob_http.rs`
//! carried `civil_from_days` (days-since-epoch → (y, m, d), used by
//! `format_iso8601_utc`) and `studio_live_hashes.rs` carried its inverse,
//! `days_from_civil` ((y, m, d) → days-since-epoch, used by
//! `parse_timestamp_nanos`). Both are Howard Hinnant's algorithms
//! <http://howardhinnant.github.io/date_algorithms.html>, hand-rolled to
//! avoid adding a `chrono`/`time` dependency to this crate (the same
//! reasoning `blob_http.rs`'s doc comment gives for hand-rolling base64).
//!
//! ## Where this landed and why
//! Both call sites already live in the SAME crate (`nodalmerge-server`,
//! `server/server/src`), just in different files — so the lowest crate
//! either can reach without a new dependency edge is this one; no need to
//! push it down into `nodalmerge-core` or a new leaf crate. `pub(crate)`
//! visibility, since no consumer outside this crate needs it.

/// Days-since-epoch (1970-01-01) → (year, month, day), proleptic Gregorian.
/// <http://howardhinnant.github.io/date_algorithms.html#civil_from_days>
pub(crate) fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

/// (year, month, day) → days-since-epoch (1970-01-01), proleptic Gregorian —
/// the inverse of [`civil_from_days`].
/// <http://howardhinnant.github.io/date_algorithms.html#days_from_civil>
pub(crate) fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mm = m as i64;
    let doy = (153 * (if mm > 2 { mm - 3 } else { mm + 9 }) + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Slice 7.3: the round-trip property test the split copies lacked
    /// (each file only ever exercised its own direction). Wide contiguous
    /// range — roughly years 1560 through 2380 — covers every 4-year leap
    /// rule, every /100 non-leap-century exception, and every /400
    /// re-inclusion (1600, 2000 leap; 1700, 1800, 1900, 2100, 2200, 2300
    /// not) with no gaps.
    #[test]
    fn round_trip_wide_range_incl_leap_years_and_century_boundaries() {
        for days in -150_000i64..=150_000 {
            let (y, m, d) = civil_from_days(days);
            let back = days_from_civil(y, m, d);
            assert_eq!(
                back, days,
                "round-trip failed for days={days} -> {y:04}-{m:02}-{d:02} -> {back}"
            );
        }
    }

    /// The exact leap-day pin that already existed (indirectly, via
    /// `format_iso8601_utc`) in `blob_http.rs`'s tests: 2024-02-29 —
    /// 1_709_164_800 unix seconds, i.e. day 19778 since epoch.
    #[test]
    fn pinned_2024_02_29_leap_day_round_trips() {
        let days = 1_709_164_800i64 / 86_400;
        assert_eq!(civil_from_days(days), (2024, 2, 29));
        assert_eq!(days_from_civil(2024, 2, 29), days);
    }

    /// Century-boundary spot checks (the /100-not-/400 exception), each
    /// direction independently, not just via the wide-range loop above.
    #[test]
    fn century_boundary_spot_checks() {
        for &(y, m, d) in &[
            (1600, 2, 29), // divisible by 400 -> leap
            (1700, 3, 1),  // divisible by 100, not 400 -> not leap; no Feb 29
            (1800, 3, 1),
            (1900, 3, 1),
            (2000, 2, 29), // divisible by 400 -> leap
            (2100, 3, 1),  // not leap
        ] {
            let days = days_from_civil(y, m, d);
            assert_eq!(civil_from_days(days), (y, m, d), "mismatch for {y:04}-{m:02}-{d:02}");
        }
    }

    #[test]
    fn epoch_is_day_zero() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(days_from_civil(1970, 1, 1), 0);
    }
}
