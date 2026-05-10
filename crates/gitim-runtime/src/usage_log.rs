//! Per-agent token usage accumulator.
//!
//! Persists to `<workspace>/.gitim-runtime/usage/<handler>.json` on every
//! turn. Decoupled from `AgentState` (`<agent-clone>/.gitim/agent-state.json`)
//! so a session reset never wipes cumulative statistics. The runtime is the
//! single writer per agent and `agent_loop` is per-agent serial, so no
//! cross-process coordination is required.
//!
//! See `docs/plans/2026-05-10-agent-token-usage/design.md` for the full
//! contract — schema, retention policy, hard-delete semantics.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use gitim_agent_provider::ProviderUsage;
use serde::{Deserialize, Serialize};

/// Window of `by_day` entries kept on disk. 90 covers the 30-day sparkline
/// the WebUI renders today plus a 60-day cushion for "near 3 months" recall.
/// File size at 90 entries × ~80 bytes + header is well under 10KB.
const RETENTION_DAYS: i64 = 90;

/// Window the WebUI sparkline reads. Filled with zero buckets when the
/// agent has no entry for a given date so the UI renders a continuous band.
const SUMMARY_WINDOW_DAYS: i64 = 30;

/// Schema version for the on-disk file. Bumped when the layout changes
/// incompatibly. Readers tolerate forward-versioned files (treat as v1) so
/// a runtime downgrade does not destroy data.
const CURRENT_VERSION: u32 = 1;

/// Five-counter bucket scoped to a single UTC day (or to the agent's whole
/// lifetime when used as `totals`). All counters are saturating so a
/// degenerate provider report can never overflow `u64::MAX`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageBucket {
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub cache_read: u64,
    #[serde(default)]
    pub cache_creation: u64,
    #[serde(default)]
    pub turns: u64,
}

impl UsageBucket {
    fn add_delta(&mut self, delta: &ProviderUsage) {
        self.input = self.input.saturating_add(delta.input_tokens.unwrap_or(0));
        self.output = self
            .output
            .saturating_add(delta.output_tokens.unwrap_or(0));
        self.cache_read = self
            .cache_read
            .saturating_add(delta.cache_read_tokens.unwrap_or(0));
        self.cache_creation = self
            .cache_creation
            .saturating_add(delta.cache_creation_tokens.unwrap_or(0));
    }
}

/// On-disk schema for `<workspace>/.gitim-runtime/usage/<handler>.json`.
///
/// `BTreeMap` for `by_day` so JSON output is sorted by date, which doubles
/// as the sparkline's natural left-to-right order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentUsageLog {
    pub version: u32,
    pub handler: String,
    pub provider: String,
    pub model: String,
    pub provider_reports_usage: bool,
    pub first_seen: String,
    pub last_updated: String,
    pub totals: UsageBucket,
    pub by_day: BTreeMap<String, UsageBucket>,
}

/// View of one calendar day rendered for the HTTP/WebUI layer. Always 30
/// of these are emitted from `summary()` regardless of how sparse `by_day`
/// is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DayEntry {
    pub date: String,
    pub bucket: UsageBucket,
}

/// HTTP-shaped projection of an `AgentUsageLog`. Distinct from the on-disk
/// type because we want to include `today` as a convenience field and
/// flatten `by_day` into a sorted vector for stable JSON ordering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageSummary {
    pub provider_reports_usage: bool,
    pub first_seen: String,
    pub last_updated: String,
    pub totals: UsageBucket,
    pub today: UsageBucket,
    pub by_day: Vec<DayEntry>,
}

impl AgentUsageLog {
    /// Path to the on-disk file. Always under `.gitim-runtime/usage/` so
    /// runtime-owned data lives separately from the per-agent git clone.
    pub fn path(workspace_root: &Path, handler: &str) -> PathBuf {
        workspace_root
            .join(".gitim-runtime")
            .join("usage")
            .join(format!("{handler}.json"))
    }

    /// Load the file or hand back a fresh log when it's missing/corrupt.
    /// Corruption is logged and downgraded to a default rather than
    /// surfaced — token statistics are non-critical and we'd rather lose a
    /// few days than break the message loop.
    pub fn load_or_default(
        workspace_root: &Path,
        handler: &str,
        provider: &str,
        model: &str,
        provider_reports_usage: bool,
    ) -> Self {
        let path = Self::path(workspace_root, handler);
        match std::fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str::<AgentUsageLog>(&content) {
                Ok(log) => {
                    // Provider / model / provider_reports_usage are all
                    // stamped by the agent loop the first time it writes the
                    // file and are immutable thereafter (provider/model per
                    // CLAUDE.md; reports_usage is a pure function of provider).
                    // Trust the file over the caller's hint — recovery passes
                    // empty strings + `true` because it doesn't know the
                    // provider, and overwriting would silently flip
                    // gemini/openclaw to reports_usage=true and the WebUI
                    // would render the numeric path instead of the "该
                    // provider 不上报 token" degradation.
                    if !provider.is_empty()
                        && (log.provider != provider || log.model != model)
                    {
                        tracing::warn!(
                            handler = %handler,
                            file_provider = %log.provider,
                            file_model = %log.model,
                            arg_provider = %provider,
                            arg_model = %model,
                            "usage log provider/model differs from caller"
                        );
                    }
                    let _ = provider_reports_usage; // intentional: file is canonical
                    log
                }
                Err(e) => {
                    tracing::error!(
                        handler = %handler,
                        path = %path.display(),
                        error = %e,
                        "usage log unparseable; starting fresh"
                    );
                    Self::fresh(handler, provider, model, provider_reports_usage)
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Self::fresh(handler, provider, model, provider_reports_usage)
            }
            Err(e) => {
                tracing::error!(
                    handler = %handler,
                    path = %path.display(),
                    error = %e,
                    "usage log read failed; starting fresh"
                );
                Self::fresh(handler, provider, model, provider_reports_usage)
            }
        }
    }

    fn fresh(handler: &str, provider: &str, model: &str, provider_reports_usage: bool) -> Self {
        Self {
            version: CURRENT_VERSION,
            handler: handler.to_string(),
            provider: provider.to_string(),
            model: model.to_string(),
            provider_reports_usage,
            first_seen: String::new(),
            last_updated: String::new(),
            totals: UsageBucket::default(),
            by_day: BTreeMap::new(),
        }
    }

    /// Apply one turn's accumulated tokens to the day bucket and totals.
    ///
    /// `delta` arrives already normalized to per-turn semantics by the
    /// runtime's normalize step (cumulative providers had `last_seen`
    /// subtracted off via `saturating_sub`). When the provider does not
    /// report usage, only `turns` advances.
    ///
    /// `last_updated` is `max(prev, now_iso)` so a clock that jumps
    /// backward (NTP correction near midnight) cannot rewrite history.
    pub fn accumulate(&mut self, today: &str, delta: Option<&ProviderUsage>, now_iso: &str) {
        if self.first_seen.is_empty() {
            self.first_seen = now_iso.to_string();
        }
        if now_iso > self.last_updated.as_str() {
            self.last_updated = now_iso.to_string();
        }

        let bucket = self.by_day.entry(today.to_string()).or_default();
        bucket.turns = bucket.turns.saturating_add(1);
        self.totals.turns = self.totals.turns.saturating_add(1);

        if let Some(delta) = delta {
            if self.provider_reports_usage {
                bucket.add_delta(delta);
                self.totals.add_delta(delta);
            }
        }
    }

    /// Drop entries older than RETENTION_DAYS so the file stays bounded.
    /// Run from `save()` so callers don't have to remember.
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

    /// 30-entry slice ending today, zero-filling missing days. Returns an
    /// empty vec if `today` is unparseable. In production callers always
    /// pass `chrono::Utc::now().format("%Y-%m-%d")` so this branch is
    /// unreachable; the assertion catches a future refactor that swaps in
    /// a malformed date format before it silently ships an empty
    /// sparkline.
    pub fn last_30_days(&self, today: &str) -> Vec<DayEntry> {
        let Ok(today_dt) = NaiveDate::parse_from_str(today, "%Y-%m-%d") else {
            debug_assert!(false, "last_30_days got unparseable date: {today}");
            tracing::error!(today = %today, "last_30_days: unparseable date string");
            return Vec::new();
        };
        let mut out = Vec::with_capacity(SUMMARY_WINDOW_DAYS as usize);
        for offset in (0..SUMMARY_WINDOW_DAYS).rev() {
            let date = today_dt - chrono::Duration::days(offset);
            let key = date.format("%Y-%m-%d").to_string();
            let bucket = self.by_day.get(&key).cloned().unwrap_or_default();
            out.push(DayEntry { date: key, bucket });
        }
        out
    }

    /// HTTP/SSE-shaped projection.
    pub fn summary(&self, today: &str) -> UsageSummary {
        let today_bucket = self.by_day.get(today).cloned().unwrap_or_default();
        UsageSummary {
            provider_reports_usage: self.provider_reports_usage,
            first_seen: self.first_seen.clone(),
            last_updated: self.last_updated.clone(),
            totals: self.totals.clone(),
            today: today_bucket,
            by_day: self.last_30_days(today),
        }
    }

    /// Serialize and atomically swap the file in. Creates the parent
    /// directory if missing, prunes stale days, applies chmod 0600.
    ///
    /// `today` here only feeds `prune_by_day`; the on-disk timestamp
    /// already reflects whatever `accumulate` set.
    pub fn save(&mut self, workspace_root: &Path, today: &str) -> io::Result<()> {
        self.prune_by_day(today);

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

    /// Best-effort delete used by hard-delete-agent. Missing file is not an
    /// error; any other I/O failure is propagated.
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
    // Windows is out-of-scope for v1 (CLAUDE.md non-goals).
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn delta(input: u64, output: u64, cr: u64, cc: u64) -> ProviderUsage {
        ProviderUsage {
            input_tokens: Some(input),
            output_tokens: Some(output),
            cache_read_tokens: Some(cr),
            cache_creation_tokens: Some(cc),
            used_percent: None,
        }
    }

    #[test]
    fn lazy_init_returns_fresh_log() {
        let dir = TempDir::new().unwrap();
        let log = AgentUsageLog::load_or_default(dir.path(), "alice", "claude", "sonnet-4-6", true);
        assert_eq!(log.handler, "alice");
        assert!(log.first_seen.is_empty());
        assert!(log.by_day.is_empty());
        assert_eq!(log.totals.turns, 0);
        assert!(log.provider_reports_usage);
    }

    #[test]
    fn accumulate_increments_today_and_totals() {
        let mut log = AgentUsageLog::load_or_default(
            TempDir::new().unwrap().path(),
            "alice",
            "claude",
            "sonnet",
            true,
        );
        log.accumulate(
            "2026-05-10",
            Some(&delta(1000, 200, 5000, 100)),
            "2026-05-10T08:00:00Z",
        );
        log.accumulate(
            "2026-05-10",
            Some(&delta(500, 100, 2000, 50)),
            "2026-05-10T09:00:00Z",
        );

        let today = log.by_day.get("2026-05-10").unwrap();
        assert_eq!(today.input, 1500);
        assert_eq!(today.output, 300);
        assert_eq!(today.cache_read, 7000);
        assert_eq!(today.cache_creation, 150);
        assert_eq!(today.turns, 2);
        assert_eq!(log.totals.input, 1500);
        assert_eq!(log.totals.turns, 2);
        assert_eq!(log.first_seen, "2026-05-10T08:00:00Z");
        assert_eq!(log.last_updated, "2026-05-10T09:00:00Z");
    }

    #[test]
    fn accumulate_without_provider_reports_only_counts_turns() {
        let mut log = AgentUsageLog::load_or_default(
            TempDir::new().unwrap().path(),
            "ada",
            "gemini",
            "gemini-2.0",
            false,
        );
        log.accumulate(
            "2026-05-10",
            Some(&delta(9999, 9999, 9999, 9999)),
            "2026-05-10T10:00:00Z",
        );
        let today = log.by_day.get("2026-05-10").unwrap();
        assert_eq!(today.turns, 1);
        assert_eq!(today.input, 0, "tokens dropped when provider does not report");
        assert_eq!(today.output, 0);
        assert_eq!(log.totals.input, 0);
        assert_eq!(log.totals.turns, 1);
    }

    #[test]
    fn accumulate_with_none_delta_only_counts_turns() {
        let mut log = AgentUsageLog::load_or_default(
            TempDir::new().unwrap().path(),
            "ada",
            "claude",
            "sonnet",
            true,
        );
        log.accumulate("2026-05-10", None, "2026-05-10T10:00:00Z");
        let today = log.by_day.get("2026-05-10").unwrap();
        assert_eq!(today.turns, 1);
        assert_eq!(today.input, 0);
    }

    #[test]
    fn last_updated_does_not_regress_on_clock_jump() {
        let mut log = AgentUsageLog::load_or_default(
            TempDir::new().unwrap().path(),
            "alice",
            "claude",
            "sonnet",
            true,
        );
        log.accumulate(
            "2026-05-10",
            Some(&delta(1, 1, 0, 0)),
            "2026-05-10T12:00:00Z",
        );
        log.accumulate(
            "2026-05-10",
            Some(&delta(1, 1, 0, 0)),
            "2026-05-10T11:59:59Z", // NTP rolled back 1 second
        );
        assert_eq!(log.last_updated, "2026-05-10T12:00:00Z");
    }

    #[test]
    fn prune_by_day_drops_entries_older_than_retention() {
        let mut log = AgentUsageLog::load_or_default(
            TempDir::new().unwrap().path(),
            "alice",
            "claude",
            "sonnet",
            true,
        );
        // Seed a 100-day window so 10 days fall off after pruning.
        let today = NaiveDate::parse_from_str("2026-05-10", "%Y-%m-%d").unwrap();
        for offset in 0..100 {
            let day = (today - chrono::Duration::days(offset))
                .format("%Y-%m-%d")
                .to_string();
            log.accumulate(
                &day,
                Some(&delta(1, 1, 0, 0)),
                &format!("{day}T12:00:00Z"),
            );
        }
        log.prune_by_day("2026-05-10");
        assert_eq!(log.by_day.len(), RETENTION_DAYS as usize);
        assert!(log.by_day.contains_key("2026-05-10"));
        // Oldest kept entry is exactly RETENTION_DAYS - 1 days back.
        let oldest_kept = today - chrono::Duration::days(RETENTION_DAYS - 1);
        assert!(log
            .by_day
            .contains_key(&oldest_kept.format("%Y-%m-%d").to_string()));
    }

    #[test]
    fn last_30_days_zero_fills_sparse_history() {
        let mut log = AgentUsageLog::load_or_default(
            TempDir::new().unwrap().path(),
            "alice",
            "claude",
            "sonnet",
            true,
        );
        log.accumulate(
            "2026-05-10",
            Some(&delta(100, 50, 0, 0)),
            "2026-05-10T12:00:00Z",
        );
        let entries = log.last_30_days("2026-05-10");
        assert_eq!(entries.len(), 30);
        assert_eq!(entries.last().unwrap().date, "2026-05-10");
        assert_eq!(entries.last().unwrap().bucket.input, 100);
        // Day 0 of the window should be 30 days back from today and empty.
        assert_eq!(entries.first().unwrap().bucket.turns, 0);
    }

    #[test]
    fn save_writes_atomically_with_chmod_0600() {
        let dir = TempDir::new().unwrap();
        let mut log = AgentUsageLog::load_or_default(
            dir.path(),
            "alice",
            "claude",
            "sonnet",
            true,
        );
        log.accumulate(
            "2026-05-10",
            Some(&delta(1, 1, 0, 0)),
            "2026-05-10T12:00:00Z",
        );
        log.save(dir.path(), "2026-05-10").expect("save");

        let path = AgentUsageLog::path(dir.path(), "alice");
        assert!(path.exists());
        // Tmp file must be cleaned up.
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
        let mut log = AgentUsageLog::load_or_default(
            dir.path(),
            "alice",
            "claude",
            "sonnet",
            true,
        );
        log.accumulate(
            "2026-05-10",
            Some(&delta(1234, 567, 8901, 23)),
            "2026-05-10T12:00:00Z",
        );
        log.save(dir.path(), "2026-05-10").expect("save");

        let loaded = AgentUsageLog::load_or_default(dir.path(), "alice", "claude", "sonnet", true);
        assert_eq!(loaded.totals.input, 1234);
        assert_eq!(loaded.by_day.get("2026-05-10").unwrap().output, 567);
        assert_eq!(loaded.first_seen, "2026-05-10T12:00:00Z");
    }

    #[test]
    fn delete_is_best_effort_when_missing() {
        let dir = TempDir::new().unwrap();
        AgentUsageLog::delete(dir.path(), "ghost").expect("delete missing is ok");
    }

    #[test]
    fn delete_removes_existing_file() {
        let dir = TempDir::new().unwrap();
        let mut log = AgentUsageLog::load_or_default(
            dir.path(),
            "alice",
            "claude",
            "sonnet",
            true,
        );
        log.accumulate("2026-05-10", None, "2026-05-10T12:00:00Z");
        log.save(dir.path(), "2026-05-10").expect("save");
        AgentUsageLog::delete(dir.path(), "alice").expect("delete");
        assert!(!AgentUsageLog::path(dir.path(), "alice").exists());
    }

    #[test]
    fn load_or_default_trusts_disk_provider_reports_usage_over_caller_hint() {
        // Recovery passes `provider_reports_usage: true` because it doesn't
        // know the provider. A gemini/openclaw agent has `false` stamped on
        // disk; the loader must keep the file's value, otherwise the WebUI
        // renders the numeric path instead of the "该 provider 不上报"
        // degradation.
        let dir = TempDir::new().unwrap();
        let path = AgentUsageLog::path(dir.path(), "ada");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let initial = AgentUsageLog {
            version: 1,
            handler: "ada".into(),
            provider: "gemini".into(),
            model: "gemini-2.0".into(),
            provider_reports_usage: false,
            first_seen: "2026-05-09T12:00:00Z".into(),
            last_updated: "2026-05-09T12:00:00Z".into(),
            totals: UsageBucket {
                turns: 3,
                ..Default::default()
            },
            by_day: BTreeMap::new(),
        };
        std::fs::write(&path, serde_json::to_string(&initial).unwrap()).unwrap();

        // Caller hint says reports_usage = true; loader must ignore that.
        let loaded =
            AgentUsageLog::load_or_default(dir.path(), "ada", "", "", true);
        assert!(
            !loaded.provider_reports_usage,
            "disk value must win, otherwise gemini/openclaw recovery silently flips the UI path"
        );
        assert_eq!(loaded.provider, "gemini");
    }

    #[test]
    fn corrupt_file_falls_back_to_fresh_log() {
        let dir = TempDir::new().unwrap();
        let path = AgentUsageLog::path(dir.path(), "alice");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{not valid json").unwrap();

        let log = AgentUsageLog::load_or_default(dir.path(), "alice", "claude", "sonnet", true);
        assert!(log.first_seen.is_empty());
        assert!(log.by_day.is_empty());
    }

    #[test]
    fn summary_today_reflects_current_day_bucket() {
        let mut log = AgentUsageLog::load_or_default(
            TempDir::new().unwrap().path(),
            "alice",
            "claude",
            "sonnet",
            true,
        );
        log.accumulate(
            "2026-05-09",
            Some(&delta(100, 50, 0, 0)),
            "2026-05-09T12:00:00Z",
        );
        log.accumulate(
            "2026-05-10",
            Some(&delta(200, 80, 0, 0)),
            "2026-05-10T12:00:00Z",
        );

        let s = log.summary("2026-05-10");
        assert_eq!(s.today.input, 200);
        assert_eq!(s.today.output, 80);
        assert_eq!(s.totals.input, 300);
        assert_eq!(s.by_day.len(), 30);
        assert_eq!(s.by_day.last().unwrap().bucket.input, 200);
        assert!(s.provider_reports_usage);
    }

    #[test]
    fn summary_today_is_zero_when_no_turn_today() {
        let mut log = AgentUsageLog::load_or_default(
            TempDir::new().unwrap().path(),
            "alice",
            "claude",
            "sonnet",
            true,
        );
        log.accumulate(
            "2026-05-09",
            Some(&delta(100, 50, 0, 0)),
            "2026-05-09T12:00:00Z",
        );
        let s = log.summary("2026-05-10");
        assert_eq!(s.today.turns, 0);
        assert_eq!(s.today.input, 0);
        assert_eq!(s.totals.input, 100);
    }
}
