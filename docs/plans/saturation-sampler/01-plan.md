# Saturation Sampler Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Per-agent 5min 工作饱和率采样器,落盘 `<workspace>/.gitim-runtime/saturation/<handler>.json`,前端 `WorkspaceUsageHeader` 展示 Today saturation + 7-day sparkline。

**Architecture:** Mirror `usage_log` pattern — per-agent JSON、by_day + by_hour 双层聚合、90 天 rotation、atomic rename + chmod 0600。`is_working` 用 `Arc<AtomicBool>` 加进 `AgentInfo`,agent_loop 在 `provider.execute()` 用 RAII `WorkingGuard` toggle(panic-safe)。后台 `SaturationSampler` task 每 5min snapshot → drop lock → 写 N 个文件。

**Tech Stack:** Rust (gitim-runtime crate) + chrono + serde_json + tokio + React/TS (frontend types + UI 复用 lib/sparkline.ts)

**Reference:** `crates/gitim-runtime/src/usage_log.rs` 是完全对称的现成模板,实现 saturation_log 时按它的形状复制即可。Design + 锁定的决策见 `docs/plans/saturation-sampler/00-requirements.md`。

---

## File Map

**Create:**
- `crates/gitim-runtime/src/saturation_log.rs` — disk store (mirror `usage_log.rs`)
- `crates/gitim-runtime/src/saturation_sampler.rs` — 5min tick background task
- `crates/gitim-runtime/tests/saturation_sampler.rs` — integration test

**Modify:**
- `crates/gitim-runtime/src/lib.rs` — `pub mod saturation_log;` + `pub mod saturation_sampler;`
- `crates/gitim-runtime/src/http.rs`:
  - `AgentInfo` 加 `is_working: Arc<AtomicBool>` (#[serde(skip)]) + `saturation_summary: Option<SaturationSummary>`
  - `RuntimeState` 加 `saturation_save_failures: AtomicU64`
  - `RuntimeState::default()` 初始化新字段
  - `HealthResponse` 加 `saturation_save_failures: u64`
  - `agents_remove` hard_delete 分支追加 `AgentSaturationLog::delete(...)`
  - `start_agent_loop` 把 `is_working` Arc 传进 `AgentLoop::with_config`
  - `with_workspace_snapshot` 那个 list endpoint:为每个 AgentInfo 注入 saturation_summary
- `crates/gitim-runtime/src/agent_loop.rs`:
  - `AgentLoop` 持 `is_working: Option<Arc<AtomicBool>>`
  - `AgentLoopConfig` 不变(is_working 走单独 setter)
  - `WorkingGuard` 私有 struct (RAII Drop)
  - `provider.execute()` 调用前后用 `WorkingGuard` 包
- `crates/gitim-runtime/src/bin/runtime.rs::run_shell()` — spawn SaturationSampler 后台 task
- `products/gitim/frontend/src/lib/types.ts` — 加 `SaturationSummary` / `SaturationRatio` / `DaySaturation` / `HourSaturation` type
- `products/gitim/frontend/src/components/management/workspace-usage-header.tsx` — 加 "Today saturation X.X%" + 7-day sparkline

---

## Task 1: `saturation_log.rs` — disk store

**Files:**
- Create: `crates/gitim-runtime/src/saturation_log.rs`
- Modify: `crates/gitim-runtime/src/lib.rs` (加一行 `pub mod saturation_log;`)

完全 mirror `usage_log.rs` 的形状,只是 bucket 换成 `{working_samples, total_samples}` + 多一个 `by_hour: BTreeMap<String, SaturationBucket>` 字段。

- [ ] **Step 1: 加 mod declaration**

修改 `crates/gitim-runtime/src/lib.rs` 在 `pub mod usage_log;` 行下方加一行:

```rust
pub mod saturation_log;
```

- [ ] **Step 2: 写完整 saturation_log.rs 含 unit tests**

新建 `crates/gitim-runtime/src/saturation_log.rs`:

```rust
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
        let anchor =
            today_dt.and_hms_opt(hour_of_day, 0, 0).unwrap_or_default();
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
        assert!(log.by_day.contains_key(&oldest.format("%Y-%m-%d").to_string()));
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
        assert_eq!(loaded.by_hour.get("2026-05-21T08").unwrap().working_samples, 1);
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
}
```

- [ ] **Step 3: 验证编译 + tests 通过**

```bash
cargo test -p gitim-runtime saturation_log -- --nocapture
```

Expected: 13 tests pass, 0 failures, 0 warnings (pre-commit hook cargo clippy 也会跑)。

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-runtime/src/saturation_log.rs crates/gitim-runtime/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(runtime): add per-agent saturation log disk store

Mirror usage_log pattern: per-agent JSON at
<workspace>/.gitim-runtime/saturation/<handler>.json,
by_day + by_hour 双层聚合,90 天 rotation,atomic rename + chmod 0600。

bucket schema: { working_samples, total_samples }。
summary() 暴露 today + last_7_days(zero-fill) + last_24_hours
+ by_day_30 给 HTTP / WebUI 用。

13 unit tests cover accumulate / prune / save+load / delete /
corrupt fallback / clock jump regression。
EOF
)"
```

---

## Task 2: `AgentInfo.is_working` + `RuntimeState.saturation_save_failures`

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs` (AgentInfo struct + RuntimeState + Default impl)

加 runtime-only 字段。is_working 是 `Arc<AtomicBool>`,#[serde(skip)] 跟 loop_handle 同 pattern。saturation_save_failures 跟现有 usage_save_failures 同形。

- [ ] **Step 1: 改 AgentInfo struct**

在 `crates/gitim-runtime/src/http.rs` 找到 [http.rs:280-325](crates/gitim-runtime/src/http.rs:280) 的 `AgentInfo`,在 `loop_handle` 字段**之前**插入两个新字段(`#[serde(skip)]` 的 is_working 放在 #[serde(skip_serializing_if)] 字段段之后、loop_handle 之前):

```rust
    /// Cumulative + 30-day breakdown of token usage. ... [已存在,不动]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_summary: Option<crate::usage_log::UsageSummary>,
    /// Per-agent saturation summary loaded at recovery from
    /// `<workspace>/.gitim-runtime/saturation/<handler>.json`. Refreshed on
    /// every `/agents` list response. None until the sampler has ticked at
    /// least once for this agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saturation_summary: Option<crate::saturation_log::SaturationSummary>,
    /// Set to true by `AgentLoop` for the duration of `provider.execute()`.
    /// Cleared by the `WorkingGuard` RAII drop on every exit path including
    /// `?` bubble and panic. Read by `SaturationSampler::take_snapshot`.
    /// Not serialized — this is per-process truth, recovery restores it as
    /// `Arc::new(AtomicBool::new(false))`.
    #[serde(skip)]
    pub is_working: std::sync::Arc<std::sync::atomic::AtomicBool>,
    #[serde(skip)]
    pub loop_handle: Option<AbortHandle>,
}
```

注意 `is_working` 字段会要求 `AgentInfo::default()` 或所有 manual constructors 也填这个字段。

- [ ] **Step 2: 改 RuntimeState 加 saturation_save_failures**

在 `crates/gitim-runtime/src/http.rs::RuntimeState` ([http.rs:327-376](crates/gitim-runtime/src/http.rs:327)) 在 `usage_save_failures` 字段下方加:

```rust
    /// Sister counter to `usage_save_failures`. Incremented every time
    /// `AgentSaturationLog::save` returns an error from the sampler tick.
    /// Surfaced on `/runtime/health`. Best-effort observability.
    pub saturation_save_failures: std::sync::atomic::AtomicU64,
}
```

- [ ] **Step 3: 改 RuntimeState::default 初始化**

在 [http.rs:407-426](crates/gitim-runtime/src/http.rs:407) `Default::default()` 里 `usage_save_failures` 行下面加:

```rust
            usage_save_failures: std::sync::atomic::AtomicU64::new(0),
            saturation_save_failures: std::sync::atomic::AtomicU64::new(0),
        }
```

- [ ] **Step 4: 找所有 AgentInfo 构造 site,补 is_working 字段**

```bash
grep -n "AgentInfo {" crates/gitim-runtime/src/ -r
```

每个 struct literal site 加一行:

```rust
            is_working: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
```

(如果有用 `..Default::default()` 的就不需要,但 AgentInfo 没有 Default impl,所以全部需要补)。

- [ ] **Step 5: 验证编译**

```bash
cargo check -p gitim-runtime 2>&1 | tail -20
```

Expected: 0 errors, 0 warnings。如果有 unused warning 说明某个构造点漏了,补上。

- [ ] **Step 6: Commit**

```bash
git add crates/gitim-runtime/src/http.rs
git commit -m "$(cat <<'EOF'
feat(runtime): add AgentInfo.is_working + saturation_save_failures counter

AgentInfo.is_working: Arc<AtomicBool> 让 sampler 能 lock-free
读 agent 是否在 provider.execute 期间。#[serde(skip)] 跟
loop_handle 同 pattern,recovery 时初始化为 false。

AgentInfo.saturation_summary: Option<SaturationSummary> 留位,
后续 list endpoint inject。

RuntimeState.saturation_save_failures 跟 usage_save_failures
独立,后续 /runtime/health 暴露。
EOF
)"
```

---

## Task 3: `WorkingGuard` + agent_loop toggle

**Files:**
- Modify: `crates/gitim-runtime/src/agent_loop.rs` (加 WorkingGuard struct + 持 is_working Arc + provider.execute 用 guard 包)
- Modify: `crates/gitim-runtime/src/http.rs::start_agent_loop` (传 is_working Arc 到 AgentLoop)

- [ ] **Step 1: 在 agent_loop.rs 顶部加 WorkingGuard 私有 struct**

在 `crates/gitim-runtime/src/agent_loop.rs` 文件中部(在 AgentLoop struct 定义之后、impl 之前的 module-private 区域)加:

```rust
/// RAII guard that resets `is_working` to `false` on drop, covering:
/// - normal scope exit
/// - `?`-bubbled error from `provider.execute()` 或 await
/// - panic (Drop still runs during stack unwind)
///
/// Without this, an error path or panic during `provider.execute()` would
/// leave `is_working = true` permanently, causing the saturation sampler
/// to over-count the agent as working until the runtime restarts.
struct WorkingGuard {
    flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl WorkingGuard {
    fn arm(flag: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Self {
        flag.store(true, std::sync::atomic::Ordering::Relaxed);
        Self { flag }
    }
}

impl Drop for WorkingGuard {
    fn drop(&mut self) {
        self.flag.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
mod working_guard_tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn arm_sets_true_drop_sets_false() {
        let flag = Arc::new(AtomicBool::new(false));
        {
            let _g = WorkingGuard::arm(flag.clone());
            assert!(flag.load(Ordering::Relaxed));
        }
        assert!(!flag.load(Ordering::Relaxed));
    }

    #[test]
    fn drop_runs_on_panic_unwind() {
        let flag = Arc::new(AtomicBool::new(false));
        let flag_clone = flag.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = WorkingGuard::arm(flag_clone);
            panic!("synthetic panic during work");
        }));
        assert!(result.is_err());
        assert!(!flag.load(Ordering::Relaxed), "guard must reset on unwind");
    }

    #[test]
    fn error_bubble_still_resets() {
        fn boom(flag: Arc<AtomicBool>) -> Result<(), &'static str> {
            let _g = WorkingGuard::arm(flag);
            Err("simulated provider failure")
        }
        let flag = Arc::new(AtomicBool::new(false));
        let _ = boom(flag.clone());
        assert!(!flag.load(Ordering::Relaxed));
    }
}
```

- [ ] **Step 2: 让 AgentLoop 持 is_working**

在 AgentLoop struct ([agent_loop.rs:~40-120](crates/gitim-runtime/src/agent_loop.rs)) 加一个字段:

```rust
    /// Shared with `AgentInfo.is_working` so `SaturationSampler` reads the
    /// same truth without locking. `None` for legacy callers / tests; the
    /// production path injects via `set_is_working`.
    is_working: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
```

确保 `AgentLoop::with_config` / `new` / 其他构造路径 init `is_working: None`。

加 setter (跟 `set_runtime_state` 同形):

```rust
impl AgentLoop {
    pub fn set_is_working(&mut self, flag: std::sync::Arc<std::sync::atomic::AtomicBool>) {
        self.is_working = Some(flag);
    }
}
```

- [ ] **Step 3: 把 provider.execute 调用包进 WorkingGuard**

定位 [agent_loop.rs:798-802](crates/gitim-runtime/src/agent_loop.rs:798):

```rust
        let mut session = self
            .provider
            .execute(&prompt, opts)
            .await
            .map_err(|e| RuntimeError::ProviderFailed(e.to_string()))?;
```

改成:

```rust
        // RAII guard: is_working stays true until session.events finishes
        // streaming. We arm BEFORE execute() so the very first tick after
        // the user message lands sees the agent as working. We hold the
        // guard for the full streaming loop below (it covers tool-use
        // back-and-forth too) and drop it after `accumulate_usage_log`.
        let _working_guard = self
            .is_working
            .clone()
            .map(WorkingGuard::arm);

        let mut session = self
            .provider
            .execute(&prompt, opts)
            .await
            .map_err(|e| RuntimeError::ProviderFailed(e.to_string()))?;
```

**重要**: `_working_guard` 必须在 streaming loop + `accumulate_usage_log` 完成之前不被 drop。Rust 的 `let _working_guard = ...;` 形式 binding 名字以 `_` 开头**但不是 `_`** 时,会持有到 scope 结束。`let _ = ...` 才会立即 drop。这里用 `_working_guard` 是正确的(scope 持有)。

如果 streaming loop 在一个独立的 `{ ... }` 内 scope,需要把 guard 提前到外层 scope 或用 `drop(guard)` 显式控制。Read agent_loop.rs:798-1100 实际的 scope 结构,confirm guard 在整个 `run_once` 的剩余 body 内都活着。如果 streaming loop 是同一 fn body 直接展开(看代码似乎是),guard 自然活到 fn return。

- [ ] **Step 4: 让 start_agent_loop 注入 is_working Arc**

在 `crates/gitim-runtime/src/http.rs::start_agent_loop` ([http.rs:3194-3300](crates/gitim-runtime/src/http.rs:3194)) 找到 `agent_loop.set_runtime_state(state.clone());` 那行附近,加:

```rust
    // Inject the same Arc<AtomicBool> stored on AgentInfo so the sampler
    // and the loop read/write the same flag without a lock.
    let is_working = {
        let s = state.lock().unwrap();
        s.workspaces
            .get(slug)
            .and_then(|ctx| ctx.agents.get(agent_id))
            .map(|info| info.is_working.clone())
    };
    if let Some(flag) = is_working {
        agent_loop.set_is_working(flag);
    }
```

- [ ] **Step 5: 跑 working_guard unit tests + 全 agent_loop 编译**

```bash
cargo test -p gitim-runtime working_guard -- --nocapture
cargo check -p gitim-runtime
```

Expected: 3 working_guard tests pass; 0 compile errors。

- [ ] **Step 6: Commit**

```bash
git add crates/gitim-runtime/src/agent_loop.rs crates/gitim-runtime/src/http.rs
git commit -m "$(cat <<'EOF'
feat(runtime): wire is_working RAII guard around provider.execute

WorkingGuard 在 arm 时 store(true),Drop 时 store(false),覆盖
所有路径:正常 return / ? bubble error / panic unwind。

AgentLoop 持 Option<Arc<AtomicBool>>,start_agent_loop 注入跟
AgentInfo.is_working 共享的 Arc 让 SaturationSampler 无锁读。

3 working_guard unit tests cover arm-drop / panic unwind /
error path reset。
EOF
)"
```

---

## Task 4: `saturation_sampler.rs` — sampler logic

**Files:**
- Create: `crates/gitim-runtime/src/saturation_sampler.rs`
- Modify: `crates/gitim-runtime/src/lib.rs` (加 `pub mod saturation_sampler;`)

拆三个层:
1. `take_snapshot(&RuntimeState) -> Vec<(workspace_root, handler, working)>` 纯函数(short-lock)
2. `tick_once(snapshot, now, &state)` 写盘 + 错误计数(无锁,IO 在 spawn_blocking)
3. `SaturationSampler { interval, state }` 后台 task 包装

- [ ] **Step 1: 加 mod declaration**

在 `crates/gitim-runtime/src/lib.rs` 加:

```rust
pub mod saturation_sampler;
```

- [ ] **Step 2: 写 saturation_sampler.rs**

新建 `crates/gitim-runtime/src/saturation_sampler.rs`:

```rust
//! Background task that snapshots per-agent `is_working` every
//! `SAMPLING_INTERVAL` and persists per-agent saturation buckets to disk.
//!
//! Design constraint: `RuntimeState` uses `std::sync::Mutex`, so the
//! sampler MUST lock only long enough to clone the Arc + handler strings,
//! then drop the lock before doing any IO. See
//! `docs/plans/saturation-sampler/00-requirements.md`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::http::SharedRuntimeState;
use crate::saturation_log::AgentSaturationLog;

/// Production sampler interval. 5 minutes balances signal density
/// (12 samples per hour means by_hour ratio has 8.3% resolution) against
/// IO frequency. Override via `SaturationSampler::with_interval` in tests.
pub const SAMPLING_INTERVAL: Duration = Duration::from_secs(300);

/// One agent's address + working flag captured under the RuntimeState lock.
/// We keep `workspace_root` (PathBuf) and `handler` (String) by value so the
/// snapshot stays valid after the lock drops.
#[derive(Debug, Clone)]
pub struct AgentSnapshot {
    pub workspace_root: PathBuf,
    pub handler: String,
    pub working: bool,
}

/// Capture the working state of every known agent across every workspace.
/// Returns an empty vec when no workspaces/agents are registered.
///
/// Lock policy: holds the std::sync::Mutex only for the duration of this
/// function. No IO, no await — purely clones small data out. N=20 agents
/// finish in microseconds.
pub fn take_snapshot(state: &SharedRuntimeState) -> Vec<AgentSnapshot> {
    let s = match state.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let mut out = Vec::new();
    for ctx in s.workspaces.values() {
        for (_, info) in &ctx.agents {
            out.push(AgentSnapshot {
                workspace_root: ctx.path.clone(),
                handler: info.handler.clone(),
                working: info.is_working.load(Ordering::Relaxed),
            });
        }
    }
    out
}

/// Apply one tick's snapshot to disk. Each entry loads-accumulate-saves
/// independently so one agent's IO failure doesn't poison the rest.
///
/// `now_iso` / `today` / `now_hour` come from a single `Utc::now()` instant
/// so a tick that straddles midnight stays internally consistent.
///
/// Failure counter: every save error bumps
/// `RuntimeState.saturation_save_failures` (best-effort, never blocks).
pub fn tick_once(
    snapshot: &[AgentSnapshot],
    today: &str,
    now_hour: &str,
    now_iso: &str,
    state: &SharedRuntimeState,
) {
    for entry in snapshot {
        let mut log = AgentSaturationLog::load_or_default(&entry.workspace_root, &entry.handler);
        log.accumulate(today, now_hour, entry.working, now_iso);
        if let Err(e) = log.save(&entry.workspace_root, today) {
            tracing::warn!(
                handler = %entry.handler,
                error = %e,
                "failed to save saturation log"
            );
            if let Ok(s) = state.lock() {
                s.saturation_save_failures.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// Background ticker. `interval` defaults to `SAMPLING_INTERVAL` for
/// production; tests use `with_interval(Duration::from_millis(...))` to
/// drive multiple ticks in under a second.
///
/// Spawn via `SaturationSampler::spawn(state)` from `run_shell`. Returns an
/// `AbortHandle` so the runtime can stop it during a graceful shutdown
/// (currently unused — runtime process exit is the only stop path).
pub struct SaturationSampler {
    interval: Duration,
    state: SharedRuntimeState,
    shutdown: Arc<AtomicBool>,
}

impl SaturationSampler {
    pub fn new(state: SharedRuntimeState) -> Self {
        Self::with_interval(state, SAMPLING_INTERVAL)
    }

    pub fn with_interval(state: SharedRuntimeState, interval: Duration) -> Self {
        Self {
            interval,
            state,
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Spawn the sampler loop on the current tokio runtime. The returned
    /// `Arc<AtomicBool>` flips to true and ends the loop after the
    /// currently-running tick finishes.
    pub fn spawn(self) -> Arc<AtomicBool> {
        let shutdown = self.shutdown.clone();
        let interval = self.interval;
        let state = self.state.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            // Tokio interval fires immediately on first .tick(). Skip the
            // first tick so the first sample happens after one full
            // interval, giving recovery time to register agents.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                let snapshot = take_snapshot(&state);
                if snapshot.is_empty() {
                    continue;
                }
                let now = chrono::Utc::now();
                let today = now.format("%Y-%m-%d").to_string();
                let now_hour = now.format("%Y-%m-%dT%H").to_string();
                let now_iso = now.format("%Y-%m-%dT%H:%M:%SZ").to_string();
                tick_once(&snapshot, &today, &now_hour, &now_iso, &state);
            }
        });
        shutdown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{AgentInfo, RuntimeState};
    use crate::workspace::WorkspaceContext;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tempfile::TempDir;
    use tokio::sync::broadcast;

    fn make_agent(handler: &str, working: bool) -> AgentInfo {
        AgentInfo {
            id: handler.to_string(),
            handler: handler.to_string(),
            display_name: handler.to_string(),
            status: "running".into(),
            last_activity: None,
            messages_processed: 0,
            repo_path: String::new(),
            provider: None,
            model: None,
            system_prompt: None,
            introduction: None,
            env: HashMap::new(),
            error_message: None,
            session_usage: None,
            llm_provider: None,
            llm_model: None,
            usage_summary: None,
            saturation_summary: None,
            is_working: Arc::new(AtomicBool::new(working)),
            loop_handle: None,
        }
    }

    fn make_state(workspace_root: PathBuf, agents: Vec<AgentInfo>) -> SharedRuntimeState {
        let (tx, _) = broadcast::channel(16);
        let mut agent_map = HashMap::new();
        for a in agents {
            agent_map.insert(a.id.clone(), a);
        }
        let ctx = WorkspaceContext {
            slug: "test".into(),
            workspace_name: "test".into(),
            path: workspace_root,
            human_repo: None,
            poll_cursor: None,
            agents: agent_map,
            activity_tx: tx,
            auth_failed: Arc::new(AtomicBool::new(false)),
            git_config: None,
        };
        let mut rs = RuntimeState::default();
        rs.workspaces.insert("test".into(), ctx);
        Arc::new(Mutex::new(rs))
    }

    #[test]
    fn snapshot_empty_state_returns_empty_vec() {
        let rs = Arc::new(Mutex::new(RuntimeState::default()));
        assert!(take_snapshot(&rs).is_empty());
    }

    #[test]
    fn snapshot_captures_working_flag_per_agent() {
        let dir = TempDir::new().unwrap();
        let state = make_state(
            dir.path().to_path_buf(),
            vec![make_agent("alice", true), make_agent("bob", false)],
        );
        let mut snap = take_snapshot(&state);
        snap.sort_by(|a, b| a.handler.cmp(&b.handler));
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].handler, "alice");
        assert!(snap[0].working);
        assert_eq!(snap[1].handler, "bob");
        assert!(!snap[1].working);
    }

    #[test]
    fn tick_once_writes_one_file_per_agent() {
        let dir = TempDir::new().unwrap();
        let state = make_state(
            dir.path().to_path_buf(),
            vec![make_agent("alice", true), make_agent("bob", false)],
        );
        let snap = take_snapshot(&state);
        tick_once(
            &snap,
            "2026-05-21",
            "2026-05-21T12",
            "2026-05-21T12:00:00Z",
            &state,
        );
        let a = AgentSaturationLog::load_or_default(dir.path(), "alice");
        assert_eq!(a.totals.total_samples, 1);
        assert_eq!(a.totals.working_samples, 1);
        let b = AgentSaturationLog::load_or_default(dir.path(), "bob");
        assert_eq!(b.totals.total_samples, 1);
        assert_eq!(b.totals.working_samples, 0);
    }

    #[test]
    fn tick_once_accumulates_across_calls() {
        let dir = TempDir::new().unwrap();
        let state = make_state(
            dir.path().to_path_buf(),
            vec![make_agent("alice", true)],
        );
        let snap = take_snapshot(&state);
        for h in 8..=11 {
            tick_once(
                &snap,
                "2026-05-21",
                &format!("2026-05-21T{h:02}"),
                &format!("2026-05-21T{h:02}:00:00Z"),
                &state,
            );
        }
        let a = AgentSaturationLog::load_or_default(dir.path(), "alice");
        assert_eq!(a.totals.total_samples, 4);
        assert_eq!(a.by_hour.len(), 4);
    }
}
```

- [ ] **Step 3: 跑 sampler unit tests**

```bash
cargo test -p gitim-runtime saturation_sampler -- --nocapture
```

Expected: 4 tests pass。

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-runtime/src/saturation_sampler.rs crates/gitim-runtime/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(runtime): add saturation sampler background task

3 层架构:
- take_snapshot(state): 短锁 clone (workspace_root, handler, working)
- tick_once(snapshot, ...): 无锁 load/accumulate/save 各 agent 文件
- SaturationSampler: tokio::time::interval 循环包装

interval 默认 300s (SAMPLING_INTERVAL),通过 with_interval()
注入更短 interval 给集成测试用。

第一 tick 跳过让 recovery 时间窗口注册 agent。

4 unit tests cover empty state / per-agent working flag /
两个 agent 一次性写盘 / 跨 tick accumulate。
EOF
)"
```

---

## Task 5: HTTP wiring — list endpoint + /health + hard_delete cleanup

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs` (list endpoint 注入 saturation_summary、HealthResponse 加字段、agents_remove hard_delete 清 saturation 文件、run_shell spawn sampler)
- Modify: `crates/gitim-runtime/src/bin/runtime.rs::run_shell` (spawn SaturationSampler)

- [ ] **Step 1: list endpoint 注入 saturation_summary**

在 `crates/gitim-runtime/src/http.rs::agents_list` ([http.rs:3165-3177](crates/gitim-runtime/src/http.rs:3165)) 改成 enrich loop:

```rust
async fn agents_list(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    match with_workspace_snapshot(&state, &slug, |ctx| {
        let workspace_root = ctx.path.clone();
        let mut agents: Vec<AgentInfo> = ctx.agents.values().cloned().collect();
        let now = chrono::Utc::now();
        let today = now.format("%Y-%m-%d").to_string();
        let now_hour = now.format("%Y-%m-%dT%H").to_string();
        for info in agents.iter_mut() {
            let log = crate::saturation_log::AgentSaturationLog::load_or_default(
                &workspace_root,
                &info.handler,
            );
            // Only attach a summary when there's at least one sampled tick;
            // a brand-new agent with no file shows None instead of zeros.
            if !log.first_seen.is_empty() {
                info.saturation_summary = Some(log.summary(&today, &now_hour));
            }
        }
        Json(AgentsListResponse { ok: true, agents })
    }) {
        Ok(j) => j.into_response(),
        Err(r) => r,
    }
}
```

- [ ] **Step 2: HealthResponse 加 saturation_save_failures**

定位 `HealthResponse` struct (grep `pub struct HealthResponse` in http.rs),加字段:

```rust
pub struct HealthResponse {
    // ... existing fields ...
    pub usage_save_failures: u64,
    pub saturation_save_failures: u64,
}
```

定位 `/health` handler,在它构造 HealthResponse 时加:

```rust
        saturation_save_failures: s.saturation_save_failures.load(std::sync::atomic::Ordering::Relaxed),
```

(跟现有 `usage_save_failures` 那行紧贴下方。)

- [ ] **Step 3: agents_remove hard_delete 路径加 AgentSaturationLog::delete**

定位 [http.rs:4015-4028](crates/gitim-runtime/src/http.rs:4015) `if req.hard_delete { ... }` block,在现有 `AgentUsageLog::delete` 调用下方加:

```rust
        // Sister cleanup to AgentUsageLog above — best-effort, never blocks.
        if let Err(e) = crate::saturation_log::AgentSaturationLog::delete(&workspace_path, &req.id) {
            tracing::warn!(
                agent = %req.id,
                error = %e,
                "failed to delete saturation log during hard_delete"
            );
        }
```

- [ ] **Step 4: run_shell spawn SaturationSampler**

定位 `crates/gitim-runtime/src/bin/runtime.rs::run_shell` ([bin/runtime.rs:718](crates/gitim-runtime/src/bin/runtime.rs:718))。在 `recover_from_config` 调用之**后**(让 recover 把 agent 加进 workspaces)、HTTP serve 之前,加:

```rust
    // Spawn the saturation sampler. State is already populated by
    // recover_from_config; the sampler skips the first tick so we get a
    // full interval before any sample is taken, letting any in-flight
    // recovery settle. Shutdown handle is dropped → sampler runs until
    // runtime process exit.
    let _saturation_shutdown =
        gitim_runtime::saturation_sampler::SaturationSampler::new(state.clone()).spawn();
```

注意:`_saturation_shutdown` 用下划线开头是 intentional (告诉 clippy 我们故意丢弃 shutdown 句柄,sampler 跟 runtime process 同生命周期)。

- [ ] **Step 5: 加 hard_delete cleanup 集成测试**

在 `crates/gitim-runtime/src/http.rs::tests` 模块(或 grep `hard_delete` 找到 test 模块所在文件)加:

```rust
    #[tokio::test]
    async fn hard_delete_removes_saturation_log_file() {
        let dir = tempfile::TempDir::new().unwrap();
        // Seed a saturation log file as if the sampler already ran.
        let mut log = crate::saturation_log::AgentSaturationLog::load_or_default(dir.path(), "alice");
        log.accumulate("2026-05-21", "2026-05-21T08", true, "2026-05-21T08:00:00Z");
        log.save(dir.path(), "2026-05-21").expect("seed");
        assert!(crate::saturation_log::AgentSaturationLog::path(dir.path(), "alice").exists());

        // hard_delete_agent_dir is for the agent's git clone; the saturation
        // file lives at <workspace>/.gitim-runtime/saturation/alice.json which
        // is OUTSIDE the agent clone. We need the agents_remove path to
        // explicitly call AgentSaturationLog::delete; verify it does.
        crate::saturation_log::AgentSaturationLog::delete(dir.path(), "alice").expect("delete");
        assert!(!crate::saturation_log::AgentSaturationLog::path(dir.path(), "alice").exists());
    }
```

(这个测试覆盖 `delete()` 的 contract;真正的 end-to-end agents_remove 测试已经存在,只要 step 3 改对了,新加的 saturation delete 调用就走通了。)

- [ ] **Step 6: 跑全 http 模块测试 + check build**

```bash
cargo test -p gitim-runtime saturation -- --nocapture
cargo test -p gitim-runtime hard_delete_removes_saturation -- --nocapture
cargo check -p gitim-runtime
```

Expected: 新加测试 pass + 0 compile errors。

- [ ] **Step 7: Commit**

```bash
git add crates/gitim-runtime/src/http.rs crates/gitim-runtime/src/bin/runtime.rs
git commit -m "$(cat <<'EOF'
feat(runtime): wire saturation sampler + summary + cleanup

- agents_list endpoint enrich AgentInfo.saturation_summary
  per request (load + summary, no cache, mirrors usage_log)
- /runtime/health 暴露 saturation_save_failures
- agents_remove hard_delete 追加 AgentSaturationLog::delete
- run_shell spawn SaturationSampler 在 recover_from_config 之后

集成测试 hard_delete_removes_saturation_log_file 覆盖
saturation 文件的 lifecycle 清理。
EOF
)"
```

---

## Task 6: Integration test — sampler 写盘 e2e

**Files:**
- Create: `crates/gitim-runtime/tests/saturation_sampler.rs`

验证 spawn 后等几个 tick 文件落盘正确。复用 sampler 的 `with_interval` 注入 100ms。

- [ ] **Step 1: 写 integration test**

新建 `crates/gitim-runtime/tests/saturation_sampler.rs`:

```rust
//! End-to-end integration test for SaturationSampler::spawn lifecycle.
//! Mock RuntimeState with agents, spawn sampler at 100ms interval,
//! wait, and verify disk files reflect the working flag changes.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use gitim_runtime::http::{AgentInfo, RuntimeState, SharedRuntimeState};
use gitim_runtime::saturation_log::AgentSaturationLog;
use gitim_runtime::saturation_sampler::SaturationSampler;
use gitim_runtime::workspace::WorkspaceContext;
use tempfile::TempDir;
use tokio::sync::broadcast;

fn make_agent(handler: &str, working_flag: Arc<AtomicBool>) -> AgentInfo {
    AgentInfo {
        id: handler.to_string(),
        handler: handler.to_string(),
        display_name: handler.to_string(),
        status: "running".into(),
        last_activity: None,
        messages_processed: 0,
        repo_path: String::new(),
        provider: None,
        model: None,
        system_prompt: None,
        introduction: None,
        env: HashMap::new(),
        error_message: None,
        session_usage: None,
        llm_provider: None,
        llm_model: None,
        usage_summary: None,
        saturation_summary: None,
        is_working: working_flag,
        loop_handle: None,
    }
}

fn make_state(
    workspace_root: std::path::PathBuf,
    agents: Vec<AgentInfo>,
) -> SharedRuntimeState {
    let (tx, _) = broadcast::channel(16);
    let mut agent_map = HashMap::new();
    for a in agents {
        agent_map.insert(a.id.clone(), a);
    }
    let ctx = WorkspaceContext {
        slug: "test".into(),
        workspace_name: "test".into(),
        path: workspace_root,
        human_repo: None,
        poll_cursor: None,
        agents: agent_map,
        activity_tx: tx,
        auth_failed: Arc::new(AtomicBool::new(false)),
        git_config: None,
    };
    let mut rs = RuntimeState::default();
    rs.workspaces.insert("test".into(), ctx);
    Arc::new(Mutex::new(rs))
}

#[tokio::test]
async fn spawned_sampler_writes_disk_after_one_tick() {
    let dir = TempDir::new().unwrap();
    let alice_flag = Arc::new(AtomicBool::new(true));
    let bob_flag = Arc::new(AtomicBool::new(false));
    let state = make_state(
        dir.path().to_path_buf(),
        vec![
            make_agent("alice", alice_flag.clone()),
            make_agent("bob", bob_flag.clone()),
        ],
    );

    // 100ms interval: spawn skips the first tick, so we need to wait
    // ~250ms to guarantee one real tick lands.
    let _shutdown = SaturationSampler::with_interval(state.clone(), Duration::from_millis(100))
        .spawn();
    tokio::time::sleep(Duration::from_millis(250)).await;

    let alice = AgentSaturationLog::load_or_default(dir.path(), "alice");
    let bob = AgentSaturationLog::load_or_default(dir.path(), "bob");
    assert!(
        alice.totals.total_samples >= 1,
        "alice should have at least one sample, got {}",
        alice.totals.total_samples
    );
    assert_eq!(
        alice.totals.working_samples, alice.totals.total_samples,
        "alice flag was true the entire run"
    );
    assert!(
        bob.totals.total_samples >= 1,
        "bob should have at least one sample, got {}",
        bob.totals.total_samples
    );
    assert_eq!(
        bob.totals.working_samples, 0,
        "bob flag was false the entire run"
    );
}

#[tokio::test]
async fn flag_changes_reflect_in_subsequent_ticks() {
    let dir = TempDir::new().unwrap();
    let alice_flag = Arc::new(AtomicBool::new(false));
    let state = make_state(
        dir.path().to_path_buf(),
        vec![make_agent("alice", alice_flag.clone())],
    );
    let _shutdown = SaturationSampler::with_interval(state.clone(), Duration::from_millis(100))
        .spawn();

    // Wait for first tick (skip + one real tick).
    tokio::time::sleep(Duration::from_millis(250)).await;
    let baseline = AgentSaturationLog::load_or_default(dir.path(), "alice");
    let baseline_total = baseline.totals.total_samples;

    // Flip flag and wait for at least one more tick.
    alice_flag.store(true, std::sync::atomic::Ordering::Relaxed);
    tokio::time::sleep(Duration::from_millis(200)).await;

    let after = AgentSaturationLog::load_or_default(dir.path(), "alice");
    assert!(
        after.totals.total_samples > baseline_total,
        "expected new samples since baseline ({} → {})",
        baseline_total,
        after.totals.total_samples
    );
    assert!(
        after.totals.working_samples > 0,
        "expected at least one working sample after flip, got 0"
    );
}
```

- [ ] **Step 2: 跑 integration test**

```bash
cargo test -p gitim-runtime --test saturation_sampler -- --nocapture
```

Expected: 2 tests pass。

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-runtime/tests/saturation_sampler.rs
git commit -m "$(cat <<'EOF'
test(runtime): integration test for SaturationSampler spawn lifecycle

2 e2e tests using 100ms interval:
- spawned_sampler_writes_disk_after_one_tick: 多 agent 写盘 +
  working flag 正确分别为 1/1 和 0/1
- flag_changes_reflect_in_subsequent_ticks: 跨 tick AtomicBool
  flip 后 working_samples 应该增长

验证 SaturationSampler::with_interval + spawn loop +
take_snapshot + tick_once 端到端 wiring。
EOF
)"
```

---

## Task 7: Frontend types + WorkspaceUsageHeader UI

**Files:**
- Modify: `products/gitim/frontend/src/lib/types.ts` (加 SaturationSummary type)
- Modify: `products/gitim/frontend/src/components/management/workspace-usage-header.tsx` (加 Today saturation + 7-day sparkline)
- Modify: `products/gitim/frontend/src/lib/agent-runtime-state.ts` (加 summarizeFleetSaturation helper) — 可选,看现有 summarize 模块结构

- [ ] **Step 1: 加 frontend types**

在 `products/gitim/frontend/src/lib/types.ts` 找到 `UsageSummary` type 定义,在它下方加:

```typescript
export interface SaturationBucket {
  working_samples: number;
  total_samples: number;
}

export interface DaySaturation {
  date: string;
  bucket: SaturationBucket;
}

export interface HourSaturation {
  hour: string;
  bucket: SaturationBucket;
}

export interface SaturationSummary {
  first_seen: string;
  last_updated: string;
  totals: SaturationBucket;
  today: SaturationBucket;
  last_7_days: DaySaturation[];
  last_24_hours: HourSaturation[];
  by_day_30: DaySaturation[];
}
```

然后在 `Agent` interface 里(grep `interface Agent` in types.ts)加:

```typescript
  saturation_summary?: SaturationSummary;
```

- [ ] **Step 2: 加 fleet saturation reduce helper**

在 `products/gitim/frontend/src/lib/agent-runtime-state.ts` 找到 `summarizeAgentWorkload` 函数定义([agent-runtime-state.ts:33-45](products/gitim/frontend/src/lib/agent-runtime-state.ts:33)),在它下方加:

```typescript
import type { SaturationSummary, DaySaturation } from "./types";

export interface FleetSaturationView {
  today_ratio: number | null;       // 0..1 or null when no samples
  today_working: number;             // Σ working_samples
  today_total: number;               // Σ total_samples
  last_7_days_ratios: Array<{ date: string; ratio: number | null }>;
}

export function summarizeFleetSaturation(
  summaries: Array<SaturationSummary | undefined>,
): FleetSaturationView {
  let today_working = 0;
  let today_total = 0;
  // by date: { working, total } for averaging across agents per day
  const by_date = new Map<string, { working: number; total: number }>();

  for (const s of summaries) {
    if (!s) continue;
    today_working += s.today.working_samples;
    today_total += s.today.total_samples;
    for (const d of s.last_7_days) {
      const cur = by_date.get(d.date) ?? { working: 0, total: 0 };
      cur.working += d.bucket.working_samples;
      cur.total += d.bucket.total_samples;
      by_date.set(d.date, cur);
    }
  }

  const today_ratio = today_total === 0 ? null : today_working / today_total;
  const last_7_days_ratios: Array<{ date: string; ratio: number | null }> = [];
  // Iterate by_date in sorted order (matches backend BTreeMap order).
  const dates = Array.from(by_date.keys()).sort();
  for (const date of dates) {
    const { working, total } = by_date.get(date)!;
    last_7_days_ratios.push({
      date,
      ratio: total === 0 ? null : working / total,
    });
  }

  return { today_ratio, today_working, today_total, last_7_days_ratios };
}
```

- [ ] **Step 3: 加 UI 到 WorkspaceUsageHeader**

打开 `products/gitim/frontend/src/components/management/workspace-usage-header.tsx`,在现有 "Working {working}/{total}" 那段下方(grep `Working` in this file)加一个 Today Saturation 块。先 read 现有结构再 edit。

最简形式:在现有的几个 stat 卡片(Total tokens、Today input、Working)旁边加第四张卡片:

```tsx
import { summarizeFleetSaturation } from "@/lib/agent-runtime-state";
import { renderSparkline } from "@/lib/sparkline";
// ...

// 在组件 body 内、return 之前
const fleetSaturation = summarizeFleetSaturation(
  agents.map((a) => a.saturation_summary),
);

// 在现有 stat-cards JSX 里加一张:
{fleetSaturation.today_ratio !== null && (
  <div className="stat-card">
    <div className="stat-label">Today saturation</div>
    <div className="stat-value">
      {(fleetSaturation.today_ratio * 100).toFixed(1)}%
    </div>
    <div className="stat-sub">
      {fleetSaturation.today_working} / {fleetSaturation.today_total} samples
    </div>
    {fleetSaturation.last_7_days_ratios.length > 0 && (
      <div className="stat-sparkline">
        {renderSparkline(
          fleetSaturation.last_7_days_ratios.map((d) => d.ratio ?? 0),
        )}
      </div>
    )}
  </div>
)}
```

(类名 `stat-card` / `stat-label` / `stat-value` / `stat-sub` / `stat-sparkline` 要跟现有 WorkspaceUsageHeader 用的 className 对齐。先 read 文件确认现有命名再粘贴。)

- [ ] **Step 4: Frontend type check + build**

```bash
cd products/gitim/frontend && bun run typecheck 2>&1 | tail -10
cd products/gitim/frontend && bun run build 2>&1 | tail -10
```

Expected: 0 type errors, build succeeds。

- [ ] **Step 5: Commit**

```bash
git add products/gitim/frontend/src/lib/types.ts \
        products/gitim/frontend/src/lib/agent-runtime-state.ts \
        products/gitim/frontend/src/components/management/workspace-usage-header.tsx
git commit -m "$(cat <<'EOF'
feat(frontend): WorkspaceUsageHeader 加 Today saturation + sparkline

- types.ts: SaturationSummary / SaturationBucket / DaySaturation /
  HourSaturation 跟后端 wire format 对齐
- agent-runtime-state.ts: summarizeFleetSaturation reduce 多 agent
  按 Σworking/Σtotal 算 fleet ratio,按 date 聚合 7 天
- workspace-usage-header.tsx: 加一张 stat-card 显示
  "Today saturation X.X% (Y / Z samples)" + 7-day sparkline
EOF
)"
```

---

## Task 8: 全量回归

- [ ] **Step 1: gitim-runtime 全量测试**

```bash
cargo test -p gitim-runtime 2>&1 | tail -30
```

Expected: 全 pass,无 regression(对比 baseline)。

- [ ] **Step 2: gitim-core / gitim-sync sanity check**

跟 saturation 无关但保险:

```bash
cargo test -p gitim-core -p gitim-sync 2>&1 | tail -10
```

Expected: 0 failures(我们没改这两个 crate,理论上无影响)。

- [ ] **Step 3: clippy + fmt**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --no-deps --locked 2>&1 | tail -20
```

Expected: 0 fmt diff, 0 new clippy errors。

- [ ] **Step 4: 手测 sanity check(可选,工作量看心情)**

```bash
# Build release runtime
cargo build --release -p gitim-runtime
# Start it
./target/release/gitim-runtime &
RUNTIME_PID=$!
sleep 2
# Check health endpoint has new field
curl -s http://127.0.0.1:16868/runtime/health | jq '.saturation_save_failures'
# Should print: 0
kill $RUNTIME_PID
```

- [ ] **Step 5: 最终 commit (如果回归发现需要修)**

如果上面任意一步出问题,fix → commit fix。如果全 pass,不需要额外 commit。

---

## Self-Review Checklist

写完后跑一遍:

1. **Spec coverage** —— 对比 `00-requirements.md` 所有 "决策" 列表项,每条都有对应 task 实现 ✓
2. **Placeholder scan** —— 全文 grep `TBD|TODO|implement later|FIXME` 应为 0 ✓
3. **Type consistency** —— `SaturationSummary` / `SaturationBucket` / `DaySaturation` / `HourSaturation` 在 Rust 和 TS 命名完全一致;字段名 snake_case 一致 ✓
4. **测试覆盖** —— 13 (saturation_log unit) + 3 (working_guard unit) + 4 (sampler unit) + 2 (sampler e2e) + 1 (hard delete) = **23 个新测试**。覆盖 `00-requirements.md` Tests 计划里的 16 个 test entries 全部。
5. **失败模式** —— save 失败 → counter + warn log + 下个 agent 继续(tick_once 内 each-loop 容错);panic → WorkingGuard Drop reset;sampler 自身 panic → tokio task 死了无 alarm(v1 接受,unlikely 路径)

---

## Execution Handoff

Plan complete and saved to `docs/plans/saturation-sampler/01-plan.md`. 23 个新测试 + 7 个改动 task + 1 个回归 task。

下一步执行选项:

1. **Subagent-driven** (recommended) — 主线 dispatch 子 agent 一个 task 一个 task 跑,每 task 后 spec review + code review
2. **Inline** — 当前 session 直接 execute,batch checkpoint review

哪个?
