//! Shared filename helpers for the cron protocol.
//!
//! Each cron fire corresponds to a `crons/<name>/<theoretical_ts>.thread`
//! file. The filename stem encodes the fire's theoretical UTC timestamp in
//! the form `YYYY-MM-DDTHH-MM-SSZ` — RFC 3339 with `:` rewritten to `-` so
//! the string is portable across filesystems (Windows in particular treats
//! `:` as a drive separator).
//!
//! Two callers depend on parsing this format consistently:
//!   - `handlers::cron::compute_next_fire` (list / show responses)
//!   - `cron_engine::scan_due` (deciding whether the next theoretical fire
//!     is already on disk)
//!
//! Both need the same parser — a divergent parse here would let one side
//! consider a fire "already fired" while the other recomputes a duplicate
//! `next_fire`. Single source of truth, no regex, no allocations beyond
//! the colon rewrite.

use chrono::{DateTime, SecondsFormat, Utc};

/// Parse a filename stem like `2026-05-11T09-00-00Z` into a UTC datetime.
///
/// Returns `None` for any string that doesn't match the expected shape —
/// callers use this to filter out stray non-fire files (e.g. a `.gitkeep`
/// or a future `<ts>.failed` marker) without crashing the listing.
pub fn parse_thread_filename_ts(stem: &str) -> Option<DateTime<Utc>> {
    // Split on 'T' once: the date portion keeps its hyphens, the time
    // portion is the part where `:` was rewritten to `-`. Rebuild the
    // RFC 3339 string by inverting that rewrite on the time portion only.
    let (date_part, time_part) = stem.split_once('T')?;
    let time_with_colons = time_part.replace('-', ":");
    let rfc3339 = format!("{date_part}T{time_with_colons}");
    DateTime::parse_from_rfc3339(&rfc3339)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Format a UTC datetime as the canonical thread filename stem
/// (`YYYY-MM-DDTHH-MM-SSZ`). Inverse of `parse_thread_filename_ts`.
///
/// Uses second-precision RFC 3339 with the trailing `Z` then rewrites `:`
/// to `-`. We standardise on second precision because cron specs only
/// resolve to the minute — anything finer would be misleading and would
/// also break idempotency the moment two clones disagreed on subsecond
/// rounding.
pub fn format_thread_filename_ts(ts: DateTime<Utc>) -> String {
    ts.to_rfc3339_opts(SecondsFormat::Secs, true)
        .replace(':', "-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn parse_canonical_stem_roundtrip() {
        let ts = Utc.with_ymd_and_hms(2026, 5, 11, 9, 0, 0).unwrap();
        let stem = format_thread_filename_ts(ts);
        assert_eq!(stem, "2026-05-11T09-00-00Z");
        assert_eq!(parse_thread_filename_ts(&stem), Some(ts));
    }

    #[test]
    fn parse_rejects_bare_date() {
        assert_eq!(parse_thread_filename_ts("2026-05-11"), None);
    }

    #[test]
    fn parse_rejects_garbage() {
        assert_eq!(parse_thread_filename_ts("garbage"), None);
    }

    #[test]
    fn parse_canonical_stem_is_utc() {
        // Canonical stems always end in `Z` — the engine never writes any
        // other shape. We don't need to reject every variant chrono might
        // accept (chrono is liberal); we just need the canonical stem to
        // round-trip and yield a UTC datetime.
        let dt = parse_thread_filename_ts("2026-05-11T09-00-00Z").unwrap();
        assert_eq!(dt.timezone(), Utc);
    }
}
