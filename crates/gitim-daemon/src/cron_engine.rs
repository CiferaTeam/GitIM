//! Cron engine — scans `crons/<name>/spec.yaml`, decides which specs are
//! due for the **current** clone, and writes the matching theoretical-time
//! `<ts>.thread` files. Driven by a 60-second interval task spawned in
//! `lifecycle` (see Task 2.7).
//!
//! The engine never *catches up* historically (per design.md "Catch-up
//! strategy"). It computes one "next fire after the latest existing fire"
//! per spec each tick; if that timestamp is `<= now`, it fires once. The
//! next tick will re-scan and pick up the next theoretical occurrence.
//!
//! ## Three invariants (out-of-band failures route here first)
//!
//! 1. **Ownership**: a clone only fires specs whose `target` matches the
//!    handler in `.gitim/me.json`. Two clones for `@alice` and `@bob` on
//!    the same workspace would otherwise both fire `@alice`'s cron (or
//!    `@bob`'s). The filter is the only thing keeping multi-clone
//!    workspaces from doubling fires.
//!
//! 2. **Idempotency**: file existence at the theoretical timestamp IS the
//!    proof of fire. `scan_due` derives `last_fire` from the most recent
//!    `<ts>.thread` filename; on bootstrap (no fires yet) it falls back to
//!    `spec.created_at`. Repeated scans see the same `next_due` →
//!    deterministic filename → `fire()` either skips or fails-closed.
//!
//! 3. **Bootstrap**: a fresh spec uses `created_at` as the anchor, NOT
//!    `now`. "Create then immediately fire" only happens if the schedule
//!    legitimately matches between created_at and now (e.g. user creates
//!    a `* * * * *` cron — they asked for it).
//!
//! Scan is pure (no fs writes); only `fire` mutates state, and only ever
//! under `commit_lock`.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use gitim_core::types::cron::next_fire_after;
use gitim_core::types::{CronSpec, Handler};
use thiserror::Error;
use tracing::warn;

use crate::cron_paths::{format_thread_filename_ts, parse_thread_filename_ts};

/// One pending fire computed by `scan_due`. The engine re-derives the
/// destination path from `spec_name` + `theoretical_ts` when it actually
/// fires; we don't pre-compute it because the scan is pure and shouldn't
/// touch path types from outside the workspace.
#[derive(Debug, Clone, PartialEq)]
pub struct FireRequest {
    /// Directory stem under `crons/`. Always matches the `<name>` segment
    /// of `crons/<name>/spec.yaml`.
    pub spec_name: String,
    /// Full validated spec; carried forward so `fire` doesn't re-read +
    /// re-parse it under the lock (race window between scan and fire is
    /// tolerated; the spec is snapshot at scan time).
    pub spec: CronSpec,
    /// Theoretical fire timestamp — the next-after-anchor result of
    /// `next_fire_after`. Becomes the `<ts>.thread` filename and the
    /// in-message header timestamp.
    pub theoretical_ts: DateTime<Utc>,
}

#[derive(Error, Debug)]
pub enum CronEngineError {
    /// fs read on `crons/` failed in a way the engine can't recover from
    /// (permission, deleted while reading, etc). Per-spec read errors are
    /// logged + skipped — only failures iterating the parent directory
    /// surface here.
    #[error("failed to read crons directory: {0}")]
    CronsDirRead(std::io::Error),
    /// `fire` could not write the thread file. Network / disk full /
    /// permission. Caller (engine loop) logs and continues to next fire
    /// — a single bad spec must not stall the loop.
    #[error("failed to write thread file at {path}: {source}")]
    ThreadWrite {
        path: PathBuf,
        source: std::io::Error,
    },
    /// `git add` + `git commit` failed under the lock. Engine rolls back
    /// the on-disk write and surfaces this so the caller can log; next
    /// tick will retry from scratch.
    #[error("git commit for cron fire failed: {0}")]
    GitCommit(String),
}

/// Scan `crons_dir` and compute which specs need firing right now.
///
/// Pure-ish: reads directory listings + spec yaml files, but writes
/// nothing. Returns a vec of `FireRequest` entries owned by the caller —
/// the engine loop then iterates and calls `fire` on each.
///
/// `archive/` (sibling to `crons/` at workspace root) is naturally
/// excluded because we only iterate `crons/` itself.
///
/// ### Failure isolation
///
/// A single malformed `spec.yaml` MUST NOT crash the scan. Bad specs are
/// logged via `tracing::warn!` and skipped; valid specs in the same
/// workspace continue to be processed. This keeps a hand-edited or
/// half-written spec from blocking every other cron in the workspace.
///
/// Also tolerated:
/// - non-directory entries inside `crons/` (a stray file is just skipped)
/// - directories without `spec.yaml` (skipped — a future create might be
///   mid-flight, or a `git mv` left an empty dir behind)
/// - parse failures on `spec.created_at` (skipped, warned)
pub fn scan_due(
    crons_dir: &Path,
    self_handler: &Handler,
    now: DateTime<Utc>,
) -> Result<Vec<FireRequest>, CronEngineError> {
    let mut due: Vec<FireRequest> = Vec::new();

    // The crons directory may not exist yet (workspace freshly initialised
    // with no specs). That's not an error — just an empty result.
    let entries = match std::fs::read_dir(crons_dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(due),
        Err(e) => return Err(CronEngineError::CronsDirRead(e)),
    };

    for entry in entries.flatten() {
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(e) => {
                warn!(
                    "cron_engine: cannot stat entry under crons/: {e} — skipping"
                );
                continue;
            }
        };
        if !ft.is_dir() {
            // Stray file (e.g. a user dropped a README) — not a cron spec.
            continue;
        }
        let spec_name = entry.file_name().to_string_lossy().to_string();
        let spec_dir = entry.path();
        if let Some(req) = scan_one(&spec_dir, &spec_name, self_handler, now) {
            due.push(req);
        }
    }

    Ok(due)
}

/// Per-spec scan body. Pulled out so logging context (`spec_name`) stays
/// at the call site and the failure paths read top-to-bottom. Returns
/// `Some(FireRequest)` if the spec is due, `None` otherwise (any reason —
/// disabled, not-owned, parse-error, no-due-fire). Reasons are logged
/// inline at the appropriate level.
fn scan_one(
    spec_dir: &Path,
    spec_name: &str,
    self_handler: &Handler,
    now: DateTime<Utc>,
) -> Option<FireRequest> {
    let spec_path = spec_dir.join("spec.yaml");
    let body = match std::fs::read_to_string(&spec_path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Empty cron dir — possible right after a `git mv` or a
            // future delete-without-archive race. Quiet skip.
            return None;
        }
        Err(e) => {
            warn!(
                "cron_engine: cannot read {} — skipping: {e}",
                spec_path.display()
            );
            return None;
        }
    };
    let spec = match CronSpec::from_yaml(&body) {
        Ok(s) => s,
        Err(e) => {
            // Malformed yaml or schema-violating spec. Per
            // `scan_malformed_spec_logged_skipped` invariant: log, skip,
            // continue. The handler-side validate at create time should
            // make this rare; tolerated for hand-edited workspaces.
            warn!(
                "cron_engine: spec '{}' failed to parse — skipping: {}",
                spec_name, e
            );
            return None;
        }
    };

    if !spec.is_active() {
        return None;
    }

    // ① Ownership filter. Multi-clone workspaces only fire their own
    // specs; otherwise two clones with @alice and @bob would each see
    // every spec. The filter is the sole guarantor of no-double-fire
    // across clones.
    if spec.target.as_str() != self_handler.as_str() {
        return None;
    }

    // ② / ③ Anchor: most recent existing fire OR `created_at`. Listing
    // `<ts>.thread` files and parsing their stems is how we materialize
    // the last_fire fact. If no fires yet, fall back to `created_at` —
    // never to `now` (would let "create + immediate fire" sneak in).
    let last_fire = latest_fire_in_dir(spec_dir);
    let anchor = match last_fire {
        Some(ts) => ts,
        None => match DateTime::parse_from_rfc3339(&spec.created_at) {
            Ok(dt) => dt.with_timezone(&Utc),
            Err(e) => {
                // Defensive: created_at format already validated at
                // create time, but a hand edit could regress.
                warn!(
                    "cron_engine: spec '{}' has unparseable created_at '{}' — skipping: {}",
                    spec_name, spec.created_at, e
                );
                return None;
            }
        },
    };

    let next_due = match next_fire_after(&spec, anchor) {
        Ok(ts) => ts,
        Err(e) => {
            warn!(
                "cron_engine: next_fire_after failed for spec '{}' — skipping: {}",
                spec_name, e
            );
            return None;
        }
    };

    if next_due > now {
        return None;
    }

    // Final fail-safe: even though anchor was the latest existing fire,
    // a clock skew between machines could in principle let `next_due`
    // collide with an existing file. Re-check by stem to avoid a useless
    // commit attempt — fire's own check still guards correctness, this
    // is just the optimistic path.
    if dest_already_exists(spec_dir, next_due) {
        return None;
    }

    Some(FireRequest {
        spec_name: spec_name.to_string(),
        spec,
        theoretical_ts: next_due,
    })
}

/// Iterate `<ts>.thread` filenames in `spec_dir` and return the latest
/// parseable timestamp. Stray non-fire files are silently filtered.
fn latest_fire_in_dir(spec_dir: &Path) -> Option<DateTime<Utc>> {
    let rd = std::fs::read_dir(spec_dir).ok()?;
    let mut latest: Option<DateTime<Utc>> = None;
    for entry in rd.flatten() {
        let fname = entry.file_name().to_string_lossy().to_string();
        let stem = match fname.strip_suffix(".thread") {
            Some(s) => s.to_string(),
            None => continue,
        };
        if let Some(ts) = parse_thread_filename_ts(&stem) {
            latest = Some(latest.map_or(ts, |cur| cur.max(ts)));
        }
    }
    latest
}

/// Check if a thread file for `theoretical_ts` already exists in the spec
/// dir. Used as a final pre-fire guard so the engine doesn't commit a
/// duplicate.
fn dest_already_exists(spec_dir: &Path, theoretical_ts: DateTime<Utc>) -> bool {
    let stem = format_thread_filename_ts(theoretical_ts);
    spec_dir.join(format!("{stem}.thread")).exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    fn alice() -> Handler {
        Handler::new("alice").unwrap()
    }

    fn bob() -> Handler {
        Handler::new("bob").unwrap()
    }

    fn fixed_now() -> DateTime<Utc> {
        // 2026-05-09 (Saturday) 15:00 UTC — picked so a "@daily" anchored
        // at midnight already matches and "0 9 * * 1" anchored Sunday is
        // tomorrow.
        Utc.with_ymd_and_hms(2026, 5, 9, 15, 0, 0).unwrap()
    }

    fn write_spec(crons_root: &Path, name: &str, body: &str) -> PathBuf {
        let dir = crons_root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("spec.yaml");
        std::fs::write(&path, body).unwrap();
        path
    }

    /// Helper: build a CronSpec body with sane defaults (target=alice,
    /// schedule=@daily) and serialize it.
    fn spec_yaml(target: &str, schedule: &str, created_at: &str, enabled: bool) -> String {
        format!(
            "version: 1\nschedule: \"{schedule}\"\ntarget: {target}\nprompt: hi\nenabled: {enabled}\ncreated_by: alice\ncreated_at: \"{created_at}\"\n"
        )
    }

    fn write_thread_file(spec_dir: &Path, ts: DateTime<Utc>) {
        let stem = format_thread_filename_ts(ts);
        let path = spec_dir.join(format!("{stem}.thread"));
        // Body shape doesn't matter for scan_due — only the filename.
        std::fs::write(path, "[L000001][P000000][@system][20260101T000000Z] cron(x): y\n")
            .unwrap();
    }

    // ─── scan ─────────────────────────────────────────────────────────────────

    #[test]
    fn scan_empty_workspace() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("crons");
        // Don't create the directory — scan should treat that the same
        // as "no specs" (workspace freshly initialised).
        let due = scan_due(&root, &alice(), fixed_now()).unwrap();
        assert!(due.is_empty());
    }

    #[test]
    fn scan_disabled_excluded() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("crons");
        write_spec(
            &root,
            "weekly",
            &spec_yaml("alice", "@daily", "2026-05-01T00:00:00Z", false),
        );
        let due = scan_due(&root, &alice(), fixed_now()).unwrap();
        assert!(due.is_empty(), "disabled spec must not fire");
    }

    #[test]
    fn scan_ownership_filter() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("crons");
        write_spec(
            &root,
            "for-bob",
            &spec_yaml("bob", "@daily", "2026-05-01T00:00:00Z", true),
        );
        // Self = alice, target = bob → no fire.
        let due = scan_due(&root, &alice(), fixed_now()).unwrap();
        assert!(due.is_empty(), "alice clone must not fire bob's spec");
    }

    #[test]
    fn scan_due_returned() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("crons");
        // Created at 2026-05-01, schedule @daily — by now (2026-05-09)
        // many fires would have been due. Engine fires only the next
        // theoretical fire after the anchor (= created_at since no
        // thread files yet).
        write_spec(
            &root,
            "daily",
            &spec_yaml("alice", "@daily", "2026-05-01T00:00:00Z", true),
        );
        let due = scan_due(&root, &alice(), fixed_now()).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].spec_name, "daily");
        // First @daily fire after 2026-05-01T00:00:00 is 2026-05-02T00:00:00.
        assert_eq!(
            due[0].theoretical_ts,
            Utc.with_ymd_and_hms(2026, 5, 2, 0, 0, 0).unwrap()
        );
    }

    #[test]
    fn scan_already_fired_skipped() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("crons");
        let dir = root.join("daily");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("spec.yaml"),
            spec_yaml("alice", "@daily", "2026-05-01T00:00:00Z", true),
        )
        .unwrap();
        // Pretend the most recent fire is at the very next-due timestamp
        // we'd otherwise pick.
        write_thread_file(
            &dir,
            Utc.with_ymd_and_hms(2026, 5, 2, 0, 0, 0).unwrap(),
        );

        let due = scan_due(&root, &alice(), fixed_now()).unwrap();
        assert_eq!(due.len(), 1, "next fire after the fired one is also due");
        assert_eq!(
            due[0].theoretical_ts,
            Utc.with_ymd_and_hms(2026, 5, 3, 0, 0, 0).unwrap(),
            "should advance past the fired timestamp"
        );
    }

    #[test]
    fn scan_already_fired_at_next_due_skipped_idempotent() {
        // Nailed-down idempotency: simulate a re-scan where the latest
        // existing fire IS the next-due timestamp. Engine should advance
        // past it (next-after) and not re-emit the same FireRequest.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("crons");
        let dir = root.join("daily");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("spec.yaml"),
            spec_yaml("alice", "@daily", "2026-05-01T00:00:00Z", true),
        )
        .unwrap();
        // Two existing fires; the next theoretical (5/3) would already
        // exist if we somehow raced.
        write_thread_file(&dir, Utc.with_ymd_and_hms(2026, 5, 2, 0, 0, 0).unwrap());
        write_thread_file(&dir, Utc.with_ymd_and_hms(2026, 5, 3, 0, 0, 0).unwrap());
        let due = scan_due(&root, &alice(), fixed_now()).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(
            due[0].theoretical_ts,
            Utc.with_ymd_and_hms(2026, 5, 4, 0, 0, 0).unwrap(),
            "should pick the fire after the latest existing one"
        );
    }

    #[test]
    fn scan_bootstrap_no_thread_files() {
        // Fresh spec with `created_at` after fixed_now's threshold —
        // no fire yet. Then move created_at back so a fire is due.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("crons");
        write_spec(
            &root,
            "fresh",
            &spec_yaml("alice", "0 9 * * *", "2026-05-08T08:00:00Z", true),
        );
        // Anchor = created_at (2026-05-08 08:00). Next fire = 2026-05-08 09:00.
        // fixed_now is 2026-05-09 15:00, so 09:00 on 5/8 is well past due.
        let due = scan_due(&root, &alice(), fixed_now()).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(
            due[0].theoretical_ts,
            Utc.with_ymd_and_hms(2026, 5, 8, 9, 0, 0).unwrap(),
            "bootstrap anchor uses created_at, not now"
        );
    }

    #[test]
    fn scan_bootstrap_no_immediate_fire() {
        // Edge case the design calls out: "create then immediately fire"
        // must NOT happen unless the schedule legitimately matches. Here
        // created_at is *after* the most recent schedule match, so the
        // next fire is genuinely in the future.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("crons");
        // 0 9 * * * with anchor at 10:00 UTC — next fire is tomorrow's 09:00.
        write_spec(
            &root,
            "fresh",
            &spec_yaml("alice", "0 9 * * *", "2026-05-09T10:00:00Z", true),
        );
        let due = scan_due(&root, &alice(), fixed_now()).unwrap();
        assert!(
            due.is_empty(),
            "fresh spec whose next fire is future must not fire on creation tick"
        );
    }

    #[test]
    fn scan_dst_forward_no_double_fire() {
        // 2026-03-08: US DST forward day. Schedule "30 2 * * *" in
        // America/Los_Angeles is in the gap; croner snaps to 03:00 PDT
        // (= UTC 10:00). A scan run later that day must produce exactly
        // one fire — not one for the missed PST 02:30 + one for the
        // snapped 03:00.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("crons");
        let dir = root.join("dst");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("spec.yaml"),
            format!(
                "version: 1\nschedule: \"30 2 * * *\"\ntimezone: \"America/Los_Angeles\"\ntarget: alice\nprompt: dst\nenabled: true\ncreated_by: alice\ncreated_at: \"2026-03-08T00:00:00Z\"\n"
            ),
        )
        .unwrap();
        // Now is later that same day — UTC 18:00 (= LA 11:00 PDT).
        let now = Utc.with_ymd_and_hms(2026, 3, 8, 18, 0, 0).unwrap();
        let due = scan_due(&root, &alice(), now).unwrap();
        assert_eq!(
            due.len(),
            1,
            "DST forward day must produce exactly one fire"
        );
        // That one fire is the snapped 03:00 LA PDT = UTC 10:00.
        assert_eq!(
            due[0].theoretical_ts,
            Utc.with_ymd_and_hms(2026, 3, 8, 10, 0, 0).unwrap()
        );
    }

    #[test]
    fn scan_archive_dir_skipped() {
        // archive/crons/<name>/spec.yaml lives at workspace root, not
        // inside crons/. scan_due is given crons_dir = .../crons, so
        // archive entries simply aren't reached. Verified by setting up
        // an archive entry that would otherwise be due and asserting it
        // doesn't show in the result.
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();
        let crons = workspace.join("crons");
        let archive_crons = workspace.join("archive/crons");
        write_spec(
            &archive_crons,
            "old",
            &spec_yaml("alice", "@daily", "2026-05-01T00:00:00Z", true),
        );
        // Empty crons/ — only the archive entry exists.
        std::fs::create_dir_all(&crons).unwrap();
        let due = scan_due(&crons, &alice(), fixed_now()).unwrap();
        assert!(due.is_empty(), "archived specs must not fire");
    }

    #[test]
    fn scan_malformed_spec_logged_skipped() {
        // One garbage spec, one valid spec. Garbage must be skipped
        // without taking the valid one down with it.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("crons");
        write_spec(&root, "broken", "this is: not\n  - valid: yaml: ::\n");
        write_spec(
            &root,
            "valid",
            &spec_yaml("alice", "@daily", "2026-05-01T00:00:00Z", true),
        );
        let due = scan_due(&root, &alice(), fixed_now()).unwrap();
        assert_eq!(due.len(), 1, "valid spec survives a broken sibling");
        assert_eq!(due[0].spec_name, "valid");
    }

    #[test]
    fn scan_directory_without_spec_skipped() {
        // `git mv` leaving an empty dir or a half-written future-create
        // state. Engine should treat as "not yet a cron" rather than
        // crashing.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("crons");
        std::fs::create_dir_all(root.join("empty")).unwrap();
        let due = scan_due(&root, &alice(), fixed_now()).unwrap();
        assert!(due.is_empty());
    }

    #[test]
    fn scan_skips_when_other_handler_owns() {
        // Mixed workspace: alice's crons + bob's crons; alice clone only
        // sees its own.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("crons");
        write_spec(
            &root,
            "for-alice",
            &spec_yaml("alice", "@daily", "2026-05-01T00:00:00Z", true),
        );
        write_spec(
            &root,
            "for-bob",
            &spec_yaml("bob", "@daily", "2026-05-01T00:00:00Z", true),
        );
        let due = scan_due(&root, &alice(), fixed_now()).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].spec_name, "for-alice");

        let due_bob = scan_due(&root, &bob(), fixed_now()).unwrap();
        assert_eq!(due_bob.len(), 1);
        assert_eq!(due_bob[0].spec_name, "for-bob");
    }

    // ─── FireRequest equality + Debug ────────────────────────────────────────

    #[test]
    fn fire_request_equality_matches_spec_name_and_ts() {
        let s = CronSpec {
            version: 1,
            schedule: "@daily".to_string(),
            timezone: None,
            target: alice(),
            prompt: "hi".to_string(),
            enabled: true,
            created_by: alice(),
            created_at: "2026-05-01T00:00:00Z".to_string(),
            extra: BTreeMap::new(),
        };
        let req1 = FireRequest {
            spec_name: "x".into(),
            spec: s.clone(),
            theoretical_ts: Utc.with_ymd_and_hms(2026, 5, 2, 0, 0, 0).unwrap(),
        };
        let req2 = req1.clone();
        assert_eq!(req1, req2);
    }
}
