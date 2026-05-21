//! Per-agent saturation accumulator.
//!
//! Persists to `<workspace>/.gitim-runtime/saturation/<handler>.json` every
//! sampler tick. Tracks `(working_samples, total_samples)` per day and per
//! UTC hour so the WebUI can render both daily ratio and hour-of-day
//! distribution. The runtime is the single writer per agent (sampler tick
//! runs serially per file path) so no cross-process coordination is needed.
//!
//! See `docs/plans/saturation-sampler/00-requirements.md` for the contract.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// Day-window kept on disk. 90 matches `usage_log` so retention semantics
/// stay symmetric. At 90 day-buckets + 24*90 hour-buckets the file stays
/// well under 100KB per agent.
const RETENTION_DAYS: i64 = 90;

/// Window the WebUI sparkline reads (7-day rolling).
const SUMMARY_WINDOW_DAYS: i64 = 7;

/// Window for the hour-of-day breakdown (last 24 hours).
const SUMMARY_WINDOW_HOURS: i64 = 24;

/// Window mirrored from `usage_log` for the 30-day overview that the
/// `WorkspaceUsageHeader` already paints — we expose the same shape so the
/// frontend can stack saturation under usage on the same axis.
const OVERVIEW_WINDOW_DAYS: i64 = 30;

/// Schema version. Bumped when layout changes incompatibly. Forward-versioned
/// files load as v1 so a downgrade doesn't lose data.
const CURRENT_VERSION: u32 = 1;

/// (working, total) counter scoped to a single UTC day, a single UTC hour,
/// or the agent's lifetime (when used as `totals`). Saturating to avoid
/// overflow under a degenerate sampler retry storm.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SaturationBucket {
    #[serde(default)]
    pub working_samples: u64,
    #[serde(default)]
    pub total_samples: u64,
}

impl SaturationBucket {
    fn record(&mut self, working: bool) {
        self.total_samples = self.total_samples.saturating_add(1);
        if working {
            self.working_samples = self.working_samples.saturating_add(1);
        }
    }
}

/// On-disk schema for `<workspace>/.gitim-runtime/saturation/<handler>.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentSaturationLog {
    pub version: u32,
    pub handler: String,
    pub first_seen: String,
    pub last_updated: String,
    pub totals: SaturationBucket,
    /// Key = `YYYY-MM-DD` (UTC).
    pub by_day: BTreeMap<String, SaturationBucket>,
    /// Key = `YYYY-MM-DDTHH` (UTC) — first 10 chars are the date part used
    /// for prune. New in v1; #[serde(default)] keeps forward-compat hooks
    /// open if we ever add per-tick buckets in a future schema bump.
    #[serde(default)]
    pub by_hour: BTreeMap<String, SaturationBucket>,
}

/// View of one calendar day shaped for the HTTP layer. 7 of these are emitted
/// from `summary()` regardless of how sparse `by_day` is — zero-fill keeps
/// the sparkline continuous.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaySaturation {
    pub date: String,
    pub bucket: SaturationBucket,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HourSaturation {
    /// `YYYY-MM-DDTHH` UTC.
    pub hour: String,
    pub bucket: SaturationBucket,
}

/// HTTP-shaped projection. Always 7 day-entries + 24 hour-entries with
/// zero-fill so the UI doesn't have to handle missing buckets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SaturationSummary {
    pub first_seen: String,
    pub last_updated: String,
    pub totals: SaturationBucket,
    pub today: SaturationBucket,
    pub last_7_days: Vec<DaySaturation>,
    pub last_24_hours: Vec<HourSaturation>,
    /// Mirrors `usage_log::UsageSummary::by_day.len() == 30` so a future
    /// frontend can render both metrics on the same time axis without a
    /// shape mismatch.
    pub by_day_30: Vec<DaySaturation>,
}

impl AgentSaturationLog {
    pub fn path(workspace_root: &Path, handler: &str) -> PathBuf {
        workspace_root
            .join(".gitim-runtime")
            .join("saturation")
            .join(format!("{handler}.json"))
    }

    pub fn load_or_default(workspace_root: &Path, handler: &str) -> Self {
        let path = Self::path(workspace_root, handler);
        match std::fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str::<AgentSaturationLog>(&content) {
                Ok(log) => log,
                Err(e) => {
                    tracing::error!(
                        handler = %handler,
                        path = %path.display(),
                        error = %e,
                        "saturation log unparseable; starting fresh"
                    );
                    Self::fresh(handler)
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => Self::fresh(handler),
            Err(e) => {
                tracing::error!(
                    handler = %handler,
                    path = %path.display(),
                    error = %e,
                    "saturation log read failed; starting fresh"
                );
                Self::fresh(handler)
            }
        }
    }

    fn fresh(handler: &str) -> Self {
        Self {
            version: CURRENT_VERSION,
            handler: handler.to_string(),
            first_seen: String::new(),
            last_updated: String::new(),
            totals: SaturationBucket::default(),
            by_day: BTreeMap::new(),
            by_hour: BTreeMap::new(),
        }
    }

    /// Record one sampler tick. `today` and `hour` come from the same
    /// `chrono::Utc::now()` instant so day/hour boundary handling stays
    /// consistent within a tick.
    pub fn accumulate(&mut self, today: &str, hour: &str, working: bool, now_iso: &str) {
        if self.first_seen.is_empty() {
            self.first_seen = now_iso.to_string();
        }
        if now_iso > self.last_updated.as_str() {
            self.last_updated = now_iso.to_string();
        }
        self.totals.record(working);
        self.by_day
            .entry(today.to_string())
            .or_default()
            .record(working);
        self.by_hour
            .entry(hour.to_string())
            .or_default()
            .record(working);
    }

    pub fn prune_by_day(&mut self, today: &str) {
        let Ok(today_dt) = NaiveDate::parse_from_str(today, "%Y-%m-%d") else {
            return;
        };
        let cutoff = today_dt - chrono::Duration::days(RETENTION_DAYS - 1);
        self.by_day.retain(|date, _| {
            NaiveDate::parse_from_str(date, "%Y-%m-%d")
                .map(|d| d >= cutoff)
                .unwrap_or(false)
        });
    }

    pub fn prune_by_hour(&mut self, today: &str) {
        let Ok(today_dt) = NaiveDate::parse_from_str(today, "%Y-%m-%d") else {
            return;
        };
        let cutoff = today_dt - chrono::Duration::days(RETENTION_DAYS - 1);
        self.by_hour.retain(|key, _| {
            let date_part = key.get(..10).unwrap_or("");
            NaiveDate::parse_from_str(date_part, "%Y-%m-%d")
                .map(|d| d >= cutoff)
                .unwrap_or(false)
        });
    }

    fn last_n_days(&self, today: &str, n: i64) -> Vec<DaySaturation> {
        let Ok(today_dt) = NaiveDate::parse_from_str(today, "%Y-%m-%d") else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(n as usize);
        for offset in (0..n).rev() {
            let date = today_dt - chrono::Duration::days(offset);
            let key = date.format("%Y-%m-%d").to_string();
            let bucket = self.by_day.get(&key).cloned().unwrap_or_default();
            out.push(DaySaturation { date: key, bucket });
        }
        out
    }

    fn last_24_hours(&self, now_hour: &str) -> Vec<HourSaturation> {
        // Parse YYYY-MM-DDTHH back into a chrono DateTime; we only need the
        // hour iteration so build via NaiveDate + hour-of-day.
        let Some(date_part) = now_hour.get(..10) else {
            return Vec::new();
        };
        let Some(hour_part) = now_hour.get(11..13) else {
            return Vec::new();
        };
        let (Ok(today_dt), Ok(hour_of_day)) = (
            NaiveDate::parse_from_str(date_part, "%Y-%m-%d"),
            hour_part.parse::<u32>(),
        ) else {
            return Vec::new();
        };
        let anchor = today_dt.and_hms_opt(hour_of_day, 0, 0).unwrap_or_default();
        let mut out = Vec::with_capacity(SUMMARY_WINDOW_HOURS as usize);
        for offset in (0..SUMMARY_WINDOW_HOURS).rev() {
            let dt = anchor - chrono::Duration::hours(offset);
            let key = dt.format("%Y-%m-%dT%H").to_string();
            let bucket = self.by_hour.get(&key).cloned().unwrap_or_default();
            out.push(HourSaturation { hour: key, bucket });
        }
        out
    }

    pub fn summary(&self, today: &str, now_hour: &str) -> SaturationSummary {
        let today_bucket = self.by_day.get(today).cloned().unwrap_or_default();
        SaturationSummary {
            first_seen: self.first_seen.clone(),
            last_updated: self.last_updated.clone(),
            totals: self.totals.clone(),
            today: today_bucket,
            last_7_days: self.last_n_days(today, SUMMARY_WINDOW_DAYS),
            last_24_hours: self.last_24_hours(now_hour),
            by_day_30: self.last_n_days(today, OVERVIEW_WINDOW_DAYS),
        }
    }

    pub fn save(&mut self, workspace_root: &Path, today: &str) -> io::Result<()> {
        self.prune_by_day(today);
        self.prune_by_hour(today);

        let path = Self::path(workspace_root, &self.handler);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let serialized =
            serde_json::to_vec_pretty(self).map_err(|e| io::Error::other(e.to_string()))?;

        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &serialized)?;
        chmod_0600(&tmp)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn delete(workspace_root: &Path, handler: &str) -> io::Result<()> {
        let path = Self::path(workspace_root, handler);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

#[cfg(unix)]
fn chmod_0600(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, perms)
}

#[cfg(not(unix))]
fn chmod_0600(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn lazy_init_returns_fresh_log() {
        let dir = TempDir::new().unwrap();
        let log = AgentSaturationLog::load_or_default(dir.path(), "alice");
        assert_eq!(log.handler, "alice");
        assert!(log.first_seen.is_empty());
        assert!(log.by_day.is_empty());
        assert!(log.by_hour.is_empty());
        assert_eq!(log.totals.total_samples, 0);
    }

    #[test]
    fn accumulate_increments_by_day_by_hour_totals() {
        let mut log = AgentSaturationLog::load_or_default(TempDir::new().unwrap().path(), "alice");
        log.accumulate("2026-05-21", "2026-05-21T08", true, "2026-05-21T08:00:00Z");
        log.accumulate("2026-05-21", "2026-05-21T08", false, "2026-05-21T08:05:00Z");
        log.accumulate("2026-05-21", "2026-05-21T09", true, "2026-05-21T09:00:00Z");

        let day = log.by_day.get("2026-05-21").unwrap();
        assert_eq!(day.total_samples, 3);
        assert_eq!(day.working_samples, 2);

        let h8 = log.by_hour.get("2026-05-21T08").unwrap();
        assert_eq!(h8.total_samples, 2);
        assert_eq!(h8.working_samples, 1);

        let h9 = log.by_hour.get("2026-05-21T09").unwrap();
        assert_eq!(h9.total_samples, 1);
        assert_eq!(h9.working_samples, 1);

        assert_eq!(log.totals.total_samples, 3);
        assert_eq!(log.totals.working_samples, 2);
        assert_eq!(log.first_seen, "2026-05-21T08:00:00Z");
        assert_eq!(log.last_updated, "2026-05-21T09:00:00Z");
    }

    #[test]
    fn accumulate_with_working_false_only_grows_denominator() {
        let mut log = AgentSaturationLog::load_or_default(TempDir::new().unwrap().path(), "alice");
        for _ in 0..10 {
            log.accumulate("2026-05-21", "2026-05-21T08", false, "2026-05-21T08:00:00Z");
        }
        assert_eq!(log.totals.total_samples, 10);
        assert_eq!(log.totals.working_samples, 0);
    }

    #[test]
    fn last_updated_does_not_regress_on_clock_jump() {
        let mut log = AgentSaturationLog::load_or_default(TempDir::new().unwrap().path(), "alice");
        log.accumulate("2026-05-21", "2026-05-21T12", true, "2026-05-21T12:00:00Z");
        log.accumulate("2026-05-21", "2026-05-21T11", false, "2026-05-21T11:59:59Z");
        assert_eq!(log.last_updated, "2026-05-21T12:00:00Z");
    }

    #[test]
    fn prune_by_day_drops_entries_older_than_retention() {
        let mut log = AgentSaturationLog::load_or_default(TempDir::new().unwrap().path(), "alice");
        let today = NaiveDate::parse_from_str("2026-05-21", "%Y-%m-%d").unwrap();
        for offset in 0..100 {
            let day = (today - chrono::Duration::days(offset))
                .format("%Y-%m-%d")
                .to_string();
            let hour = format!("{day}T12");
            log.accumulate(&day, &hour, true, &format!("{day}T12:00:00Z"));
        }
        log.prune_by_day("2026-05-21");
        assert_eq!(log.by_day.len(), RETENTION_DAYS as usize);
        assert!(log.by_day.contains_key("2026-05-21"));
        let oldest = today - chrono::Duration::days(RETENTION_DAYS - 1);
        assert!(log
            .by_day
            .contains_key(&oldest.format("%Y-%m-%d").to_string()));
    }

    #[test]
    fn prune_by_hour_drops_hours_outside_retention_window() {
        let mut log = AgentSaturationLog::load_or_default(TempDir::new().unwrap().path(), "alice");
        let today = NaiveDate::parse_from_str("2026-05-21", "%Y-%m-%d").unwrap();
        // Seed 100 days × 1 hour each.
        for offset in 0..100 {
            let date = (today - chrono::Duration::days(offset))
                .format("%Y-%m-%d")
                .to_string();
            let hour = format!("{date}T08");
            log.accumulate(&date, &hour, true, &format!("{date}T08:00:00Z"));
        }
        log.prune_by_hour("2026-05-21");
        assert_eq!(log.by_hour.len(), RETENTION_DAYS as usize);
        assert!(log.by_hour.contains_key("2026-05-21T08"));
    }

    #[test]
    fn last_7_days_zero_fills_sparse_history() {
        let mut log = AgentSaturationLog::load_or_default(TempDir::new().unwrap().path(), "alice");
        log.accumulate("2026-05-21", "2026-05-21T12", true, "2026-05-21T12:00:00Z");
        let s = log.summary("2026-05-21", "2026-05-21T12");
        assert_eq!(s.last_7_days.len(), 7);
        assert_eq!(s.last_7_days.last().unwrap().date, "2026-05-21");
        assert_eq!(s.last_7_days.last().unwrap().bucket.working_samples, 1);
        assert_eq!(s.last_7_days.first().unwrap().bucket.total_samples, 0);
    }

    #[test]
    fn last_24_hours_zero_fills_and_orders_oldest_first() {
        let mut log = AgentSaturationLog::load_or_default(TempDir::new().unwrap().path(), "alice");
        log.accumulate("2026-05-21", "2026-05-21T10", true, "2026-05-21T10:00:00Z");
        log.accumulate("2026-05-21", "2026-05-21T12", false, "2026-05-21T12:00:00Z");
        let s = log.summary("2026-05-21", "2026-05-21T12");
        assert_eq!(s.last_24_hours.len(), 24);
        assert_eq!(s.last_24_hours.last().unwrap().hour, "2026-05-21T12");
        assert_eq!(s.last_24_hours.last().unwrap().bucket.total_samples, 1);
        // Two-hours-back is 2026-05-21T10 with 1 working sample.
        let two_back = &s.last_24_hours[s.last_24_hours.len() - 3];
        assert_eq!(two_back.hour, "2026-05-21T10");
        assert_eq!(two_back.bucket.working_samples, 1);
    }

    #[test]
    fn bucket_zero_total_does_not_panic() {
        // Divide-by-zero protection: callers computing working/total ratios
        // must handle total_samples == 0; SaturationBucket itself stays valid.
        let b = SaturationBucket::default();
        assert_eq!(b.total_samples, 0);
        assert_eq!(b.working_samples, 0);
        // Ratio computed safely by caller: avoid division
        let rate = if b.total_samples == 0 {
            0.0f64
        } else {
            b.working_samples as f64 / b.total_samples as f64
        };
        assert_eq!(rate, 0.0);
    }

    #[test]
    fn save_writes_atomically_with_chmod_0600() {
        let dir = TempDir::new().unwrap();
        let mut log = AgentSaturationLog::load_or_default(dir.path(), "alice");
        log.accumulate("2026-05-21", "2026-05-21T08", true, "2026-05-21T08:00:00Z");
        log.save(dir.path(), "2026-05-21").expect("save");

        let path = AgentSaturationLog::path(dir.path(), "alice");
        assert!(path.exists());
        assert!(!path.with_extension("tmp").exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "expected 0600, got {:o}", mode);
        }
    }

    #[test]
    fn save_round_trips_through_disk() {
        let dir = TempDir::new().unwrap();
        let mut log = AgentSaturationLog::load_or_default(dir.path(), "alice");
        log.accumulate("2026-05-21", "2026-05-21T08", true, "2026-05-21T08:00:00Z");
        log.accumulate("2026-05-21", "2026-05-21T09", false, "2026-05-21T09:00:00Z");
        log.save(dir.path(), "2026-05-21").expect("save");

        let loaded = AgentSaturationLog::load_or_default(dir.path(), "alice");
        assert_eq!(loaded.totals.total_samples, 2);
        assert_eq!(loaded.totals.working_samples, 1);
        assert_eq!(
            loaded.by_hour.get("2026-05-21T08").unwrap().working_samples,
            1
        );
    }

    #[test]
    fn save_creates_parent_dir_if_missing() {
        let dir = TempDir::new().unwrap();
        // No .gitim-runtime/saturation/ pre-created.
        let mut log = AgentSaturationLog::load_or_default(dir.path(), "alice");
        log.accumulate("2026-05-21", "2026-05-21T08", true, "2026-05-21T08:00:00Z");
        log.save(dir.path(), "2026-05-21")
            .expect("save should create parent dirs");
        assert!(AgentSaturationLog::path(dir.path(), "alice").exists());
    }

    #[test]
    fn delete_is_best_effort_when_missing() {
        let dir = TempDir::new().unwrap();
        AgentSaturationLog::delete(dir.path(), "ghost").expect("delete missing is ok");
    }

    #[test]
    fn delete_removes_existing_file() {
        let dir = TempDir::new().unwrap();
        let mut log = AgentSaturationLog::load_or_default(dir.path(), "alice");
        log.accumulate("2026-05-21", "2026-05-21T08", true, "2026-05-21T08:00:00Z");
        log.save(dir.path(), "2026-05-21").expect("save");
        AgentSaturationLog::delete(dir.path(), "alice").expect("delete");
        assert!(!AgentSaturationLog::path(dir.path(), "alice").exists());
    }

    #[test]
    fn corrupt_file_falls_back_to_fresh_log() {
        let dir = TempDir::new().unwrap();
        let path = AgentSaturationLog::path(dir.path(), "alice");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{not valid json").unwrap();
        let log = AgentSaturationLog::load_or_default(dir.path(), "alice");
        assert!(log.first_seen.is_empty());
        assert!(log.by_day.is_empty());
    }

    #[test]
    fn by_day_30_always_has_30_entries() {
        let mut log = AgentSaturationLog::load_or_default(TempDir::new().unwrap().path(), "alice");
        // Only one day of data.
        log.accumulate("2026-05-21", "2026-05-21T08", true, "2026-05-21T08:00:00Z");
        let s = log.summary("2026-05-21", "2026-05-21T08");
        assert_eq!(s.by_day_30.len(), 30);
        assert_eq!(s.by_day_30.last().unwrap().date, "2026-05-21");
        assert_eq!(s.by_day_30.last().unwrap().bucket.working_samples, 1);
        // Older days are zero-filled.
        assert_eq!(s.by_day_30.first().unwrap().bucket.total_samples, 0);
    }
}
