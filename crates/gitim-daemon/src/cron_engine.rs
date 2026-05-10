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

use chrono::{DateTime, Duration, Utc};
use gitim_core::types::cron::next_fire_after;
use gitim_core::types::{CronSpec, Handler};
use thiserror::Error;
use tracing::{info, warn};

use crate::cron_paths::{format_thread_filename_ts, parse_thread_filename_ts};
use crate::state::AppState;

/// Maximum age (relative to `now`) of a fire that this engine will still
/// emit. Anchors older than `now - GRACE_WINDOW` are clamped forward, so a
/// daemon that comes back online after a long gap doesn't replay every
/// backlogged occurrence one tick at a time.
///
/// Sized as 2 tick intervals (the engine ticks every 60s) so a fire whose
/// theoretical ts genuinely lands at the boundary of a tick can still be
/// emitted on the immediately-following scan even with mild clock drift.
/// Anything older counts as "missed = miss" per design.md
/// "Catch-up 策略：不补跑": catching up burns context on stale schedules.
///
/// Constant lives at module scope so `scan_due` and any future windowing
/// helper agree on the same number; the `120s` is documented in design.md
/// as well — both should move together if we ever revisit it.
const GRACE_WINDOW: Duration = Duration::seconds(120);

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
                warn!("cron_engine: cannot stat entry under crons/: {e} — skipping");
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
    let raw_anchor = match last_fire {
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

    // Clamp the anchor forward to `now - GRACE_WINDOW`. Without this,
    // a daemon that's been offline for hours would walk `next_fire_after`
    // through every overdue occurrence, firing one per tick — a daily
    // cron created May 1 with daemon offline until May 9 would otherwise
    // burn 8 fires of agent context over ~8 minutes. Per design.md
    // "Catch-up 策略：不补跑", we silently drop anything older than 2
    // ticks. The agent missed it; missed = miss.
    //
    // For fresh / regularly-firing crons the clamp is a no-op (anchor is
    // already within the window). It only kicks in after the daemon was
    // genuinely down longer than 2 minutes.
    let cutoff = now - GRACE_WINDOW;
    let anchor = if raw_anchor < cutoff {
        cutoff
    } else {
        raw_anchor
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

/// Side-effecting fire: writes one `<ts>.thread` file + commits via
/// `state.git_storage`. Called by the engine loop after `scan_due`.
///
/// Holds `state.commit_lock` for the entire write+commit window so a
/// concurrent `handle_send` (or sync_loop's rebase) can't slip a write
/// between our `fs::write` and `add_and_commit_as`. This is the same
/// pattern `handle_send` uses; cron fires are just another writer of
/// thread content that has to coordinate with everyone else.
///
/// ### Idempotent behaviour
///
/// If the destination file already exists (because another scan already
/// fired this theoretical ts, or the engine restarted mid-tick), this
/// returns `Ok(())` without writing or committing. The caller treats the
/// fire as completed — the file is the proof.
///
/// ### Crash semantics
///
/// - Crash before write: lost fire (no file, no commit). Surfaces as a
///   "missed" entry in the calendar UI computation; not retried (per
///   design.md "no catch-up").
/// - Crash between write and commit: working tree dirty. Sync loop will
///   pick up the file on its next cycle. Worst case the fire is
///   committed without our `cron(<name>): ...` commit-message tag, but
///   the file content + theoretical ts is identical so attribution from
///   the protocol's POV is unchanged.
/// - Crash after commit: clean. No different from a successful fire.
///
/// ### Author email
///
/// Author goes through `state.author_for(<emit_handle>)` so the commit
/// is attributed to the daemon's git owner email when configured (and
/// thus shows up on the GitHub contribution graph for that account).
/// `<emit_handle>` is the daemon's running `current_user` if set, else
/// the literal `system` — falling back lets out-of-the-box tests run
/// without onboarding plumbing. The in-message `[@system]` token is
/// preserved as the *content* author either way: it's the protocol's
/// signal for "synthesised by daemon", distinct from "who scheduled it".
pub async fn fire(state: &AppState, request: FireRequest) -> Result<(), CronEngineError> {
    let FireRequest {
        spec_name,
        spec,
        theoretical_ts,
    } = request;

    let stem = format_thread_filename_ts(theoretical_ts);
    let filename = format!("{stem}.thread");
    let spec_dir = state.repo_root.join("crons").join(&spec_name);
    let dest_path = spec_dir.join(&filename);
    let rel_path = format!("crons/{spec_name}/{filename}");

    // Build the message body OUTSIDE the lock — formatting is pure and
    // shouldn't extend the critical section.
    let body = format_cron_body(&spec_name, &spec.prompt, theoretical_ts);

    // Snapshot the daemon's running identity for the commit author. The
    // engine fires on behalf of self_handler (= spec.target post-ownership
    // filter), which is the same handler the rest of this clone's commits
    // attribute to.
    let commit_author_handle = state
        .current_user
        .read()
        .await
        .clone()
        .unwrap_or_else(|| "system".to_string());
    let (author_name, author_email) = state.author_for(&commit_author_handle);

    {
        let _commit_guard = state.commit_lock.lock().expect("commit_lock poisoned");

        // Delete-then-fire race guard. Between scan_due and this point a
        // concurrent `delete_cron` may have moved the spec dir to
        // `archive/crons/<name>/`. If we proceed, `create_dir_all` would
        // resurrect an empty active dir and `fs::write` would land an
        // orphan thread file with no spec.yaml beside it — engine would
        // then re-fire it on every subsequent tick (no spec → no anchor
        // → bootstrap path can't recover).
        //
        // Re-stat spec.yaml under the lock; if it's gone, treat the fire
        // as cancelled. Use `Ok(())` (not Err) because the operator
        // intent is "this cron no longer exists" — that's a successful
        // resolution, not a fault. Log a warn so a steady stream of
        // these surfaces in monitoring (legitimate post-delete
        // last-tick races vs. a bug producing them constantly).
        if !spec_dir.join("spec.yaml").exists() {
            warn!(
                spec_name = %spec_name,
                "cron spec was deleted between scan and fire, skipping"
            );
            return Ok(());
        }

        // Race-safe idempotency check: another scan loop on this same
        // daemon could have raced us between scan_due and now. We skip
        // rather than overwrite — file existence is the canonical proof
        // of fire.
        if dest_path.exists() {
            return Ok(());
        }

        // Ensure the cron dir exists. Should already, but a freshly
        // restarted daemon hitting an empty workspace is no excuse to
        // crash.
        if let Err(e) = std::fs::create_dir_all(&spec_dir) {
            return Err(CronEngineError::ThreadWrite {
                path: dest_path.clone(),
                source: e,
            });
        }

        if let Err(e) = std::fs::write(&dest_path, &body) {
            return Err(CronEngineError::ThreadWrite {
                path: dest_path.clone(),
                source: e,
            });
        }

        let commit_msg = format!("cron: fire {spec_name} at {stem}");
        if let Err(e) = state.git_storage.add_and_commit_as(
            &[&rel_path],
            &commit_msg,
            Some((&author_name, &author_email)),
        ) {
            // Roll back the on-disk write so the working tree mirrors HEAD.
            // Best-effort — if rollback fails too, sync_loop's next cycle
            // will pick up the dirty file.
            let _ = std::fs::remove_file(&dest_path);
            return Err(CronEngineError::GitCommit(e.to_string()));
        }
        // commit_guard drops here.
    }

    info!(
        "cron_engine: fired {} at {} (target=@{})",
        spec_name,
        stem,
        spec.target.as_str()
    );

    Ok(())
}

/// Build the first-line body for a cron fire thread, using the same
/// `format_message` plumbing as `handle_send`. This guarantees the
/// resulting file parses cleanly with `gitim_core::parser::parse_thread`.
///
/// The author handle on the line itself is `system` — the protocol-level
/// "who voiced this" signal that distinguishes cron fires from human
/// messages and from agent replies. Multi-line prompts get the
/// continuation-line treatment formatter already does (lines after the
/// first inherit no `[L...]` prefix).
pub(crate) fn format_cron_body(
    spec_name: &str,
    prompt: &str,
    theoretical_ts: DateTime<Utc>,
) -> String {
    // `Handler::system()` is the only path that constructs the reserved
    // `system` handle — `Handler::new("system")` rejects it. Both daemon
    // emit (here) and parser read-back (`parser::parse_thread`'s carve-out)
    // route through that single factory so the "no user-input forges
    // @system" invariant holds.
    let system = Handler::system();
    // The in-message timestamp uses the compact format `YYYYMMDDTHHMMSSZ`
    // expected by `gitim_core::parser::parse_thread` — distinct from the
    // filename stem which uses `YYYY-MM-DDTHH-MM-SSZ`. Both encode the
    // same instant.
    let ts_compact = theoretical_ts.format("%Y%m%dT%H%M%SZ").to_string();
    let body_text = format!("cron({spec_name}): {prompt}");
    gitim_core::formatter::format_message(1, 0, &system, &ts_compact, &body_text)
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
        // 2026-05-09 (Saturday) 00:00:30 UTC — 30s past midnight. Picked
        // deliberately so `@daily` evaluated against the clamped anchor
        // (`now - 120s` = 2026-05-08 23:58:30) returns 2026-05-09 00:00:00,
        // which is `<= now` and therefore due. Earlier choices of `now`
        // (15:00 UTC) worked before the GRACE_WINDOW clamp because the
        // anchor was free to walk from `created_at` forward — once the
        // clamp lands, midnight-aligned schedules need a `now` that's
        // close to midnight for the test fire to actually be due.
        Utc.with_ymd_and_hms(2026, 5, 9, 0, 0, 30).unwrap()
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
        std::fs::write(
            path,
            "[L000001][P000000][@system][20260101T000000Z] cron(x): y\n",
        )
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
        // Created at 2026-05-01, schedule @daily. After the GRACE_WINDOW
        // clamp the anchor is now-120s = 2026-05-08T23:58:30. Next
        // `@daily` fire after that is midnight 2026-05-09 — which is
        // `<= fixed_now()` (00:00:30) so it's due.
        write_spec(
            &root,
            "daily",
            &spec_yaml("alice", "@daily", "2026-05-01T00:00:00Z", true),
        );
        let due = scan_due(&root, &alice(), fixed_now()).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].spec_name, "daily");
        // After clamp, anchor is now-120s (within today), so the next
        // fire is today's midnight, not 2026-05-02 (the pre-clamp value).
        assert_eq!(
            due[0].theoretical_ts,
            Utc.with_ymd_and_hms(2026, 5, 9, 0, 0, 0).unwrap()
        );
    }

    #[test]
    fn scan_already_fired_skipped() {
        // Idempotency: a thread file exists for an old fire. The clamp
        // pushes the anchor forward to now-120s (since the thread's ts
        // is older than that), so the next-due fire is today's midnight
        // — which is `<= now` and therefore returned.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("crons");
        let dir = root.join("daily");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("spec.yaml"),
            spec_yaml("alice", "@daily", "2026-05-01T00:00:00Z", true),
        )
        .unwrap();
        // Existing fire well outside the grace window — clamped away.
        write_thread_file(&dir, Utc.with_ymd_and_hms(2026, 5, 2, 0, 0, 0).unwrap());

        let due = scan_due(&root, &alice(), fixed_now()).unwrap();
        assert_eq!(due.len(), 1, "next fire after clamped anchor is due");
        assert_eq!(
            due[0].theoretical_ts,
            Utc.with_ymd_and_hms(2026, 5, 9, 0, 0, 0).unwrap(),
            "after clamp, next fire is today's midnight, not 5/3"
        );
    }

    #[test]
    fn scan_already_fired_at_next_due_skipped_idempotent() {
        // Nailed-down idempotency: simulate a re-scan with two existing
        // fires. After the clamp, the anchor is `now - 120s` regardless
        // of how recently the last fire happened, so the next-after is
        // today's midnight (not 5/4 as the pre-clamp version expected).
        // The point of this test under the clamp is that an existing
        // fire at exactly today's midnight would short-circuit the
        // emission — `dest_already_exists` catches the duplicate.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("crons");
        let dir = root.join("daily");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("spec.yaml"),
            spec_yaml("alice", "@daily", "2026-05-01T00:00:00Z", true),
        )
        .unwrap();
        // Pre-existing fire AT today's midnight — engine should detect
        // dest_already_exists and emit nothing.
        write_thread_file(&dir, Utc.with_ymd_and_hms(2026, 5, 9, 0, 0, 0).unwrap());
        let due = scan_due(&root, &alice(), fixed_now()).unwrap();
        assert!(
            due.is_empty(),
            "fire already on disk for today's midnight — must not re-emit"
        );
    }

    #[test]
    fn scan_bootstrap_no_thread_files() {
        // Bootstrap path: no `<ts>.thread` files exist yet, so the anchor
        // falls back to `spec.created_at`. With the GRACE_WINDOW clamp,
        // a fresh spec whose `created_at` lands inside the grace window
        // still uses `created_at` as the anchor (clamp is a no-op when
        // raw_anchor >= cutoff). Use a `@daily` schedule so the next-due
        // fire is today's midnight — within the grace window relative
        // to fixed_now.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("crons");
        // created_at = 2026-05-08T23:59:00Z, just inside the 120s window
        // before fixed_now (2026-05-09T00:00:30Z). Anchor stays at
        // created_at, next `@daily` fire = midnight 5/9.
        write_spec(
            &root,
            "fresh",
            &spec_yaml("alice", "@daily", "2026-05-08T23:59:00Z", true),
        );
        let due = scan_due(&root, &alice(), fixed_now()).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(
            due[0].theoretical_ts,
            Utc.with_ymd_and_hms(2026, 5, 9, 0, 0, 0).unwrap(),
            "bootstrap anchor stays at created_at when within grace window"
        );
    }

    #[test]
    fn scan_bootstrap_no_immediate_fire() {
        // Edge case the design calls out: "create then immediately fire"
        // must NOT happen unless the schedule legitimately matches. Here
        // created_at is *after* fixed_now, so the next fire is genuinely
        // in the future. The clamp can never push the anchor backward, so
        // a future-dated created_at stays untouched.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("crons");
        // 0 9 * * * with anchor at 2026-05-09T10:00 UTC (after now). Next
        // fire is tomorrow's 09:00 — still in the future relative to now.
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
        // (= UTC 10:00). A scan run shortly after the snapped fire must
        // produce exactly one fire — not one for the missed PST 02:30
        // + one for the snapped 03:00.
        //
        // Note: the GRACE_WINDOW clamp pushes any pre-now-120s anchor
        // forward. Pick `now` shortly after the expected snapped fire
        // (10:01 UTC) so the clamped anchor (now-120s = 09:59 UTC) is
        // still before the snapped fire (10:00 UTC) and the engine
        // returns it as due.
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
        // Now = UTC 10:01 (= LA 03:01 PDT), one minute after the
        // snapped fire timestamp.
        let now = Utc.with_ymd_and_hms(2026, 3, 8, 10, 1, 0).unwrap();
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
        // sees its own. The clamp pushes both stale anchors forward to
        // now-120s, then `@daily` next-fire = today's midnight (within
        // grace window of fixed_now), so each handler sees exactly its
        // own due fire.
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

    // ─── GRACE_WINDOW clamp ─────────────────────────────────────────────────

    /// Daemon offline for ~10 minutes; spec is `* * * * *` (every minute).
    /// Without the clamp, scan_due would walk through all 10 backlogged
    /// fires one tick at a time. With the clamp, the anchor jumps to
    /// now-120s and only the most-recent-within-grace fire is eligible —
    /// at most one FireRequest, never the full backlog.
    #[test]
    fn scan_skips_ancient_backlog() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("crons");
        // created_at 10 minutes before fixed_now. Way outside grace.
        let created_at = (fixed_now() - chrono::Duration::seconds(600))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        write_spec(
            &root,
            "every-min",
            &spec_yaml("alice", "* * * * *", &created_at, true),
        );
        let due = scan_due(&root, &alice(), fixed_now()).unwrap();
        assert!(
            due.len() <= 1,
            "ancient backlog must not replay; got {} fires",
            due.len()
        );
        // Whatever fires, it must be within the grace window.
        if let Some(req) = due.first() {
            let cutoff = fixed_now() - chrono::Duration::seconds(120);
            assert!(
                req.theoretical_ts >= cutoff,
                "emitted fire {} is older than grace cutoff {}",
                req.theoretical_ts,
                cutoff
            );
        }
    }

    /// A spec whose `created_at` is INSIDE the grace window must still
    /// fire normally — the clamp is a no-op when the raw anchor is
    /// already ≥ cutoff.
    #[test]
    fn scan_fires_within_grace_window() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("crons");
        // created_at = now - 100s, well inside the 120s grace. Schedule
        // `* * * * *` so the next fire is the next whole minute (which
        // is fixed_now() itself: 2026-05-09 00:00:30 → next minute
        // is 00:01:00, NOT due yet). Use a schedule whose next-fire
        // after now-100s lands at now-30s instead.
        let created_at = (fixed_now() - chrono::Duration::seconds(100))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        write_spec(
            &root,
            "minute",
            &spec_yaml("alice", "* * * * *", &created_at, true),
        );
        let due = scan_due(&root, &alice(), fixed_now()).unwrap();
        // raw_anchor = 2026-05-08T23:58:50, cutoff = 2026-05-08T23:58:30.
        // raw_anchor > cutoff, so anchor stays at 23:58:50.
        // Next * * * * * after 23:58:50 = 23:59:00 (still <= now 00:00:30) → due.
        assert_eq!(due.len(), 1, "fresh spec inside grace window must fire");
        assert_eq!(
            due[0].theoretical_ts,
            Utc.with_ymd_and_hms(2026, 5, 8, 23, 59, 0).unwrap(),
            "fire should be the first schedule match after raw anchor"
        );
    }

    // ─── format_cron_body — formatter contract with parser ───────────────────

    #[test]
    fn format_cron_body_parses_back() {
        // Sanity: the body we write must be parseable by the very parser
        // that reads back thread files. Otherwise the cron fire would
        // poison the thread cache + every reader.
        let body = format_cron_body(
            "weekly-report",
            "scan #general for highlights",
            Utc.with_ymd_and_hms(2026, 5, 11, 9, 0, 0).unwrap(),
        );
        let parsed = gitim_core::parser::parse_thread(&body).expect("parses");
        assert_eq!(parsed.entries.len(), 1);
        match &parsed.entries[0] {
            gitim_core::types::ThreadEntry::Message(m) => {
                assert_eq!(m.line_number, 1);
                assert_eq!(m.point_to, 0);
                assert_eq!(m.author.as_str(), "system");
                assert!(
                    m.body.starts_with("cron(weekly-report):"),
                    "body: {}",
                    m.body
                );
            }
            other => panic!("expected message entry, got {other:?}"),
        }
    }

    #[test]
    fn format_cron_body_handles_multiline_prompt() {
        // Multi-line prompt: subsequent lines must come through as
        // continuation lines (no `[L...]` prefix). Verified by the
        // parser collapsing them into a single `body` field.
        let prompt = "first line\nsecond line\nthird line";
        let body = format_cron_body(
            "multi",
            prompt,
            Utc.with_ymd_and_hms(2026, 5, 11, 9, 0, 0).unwrap(),
        );
        let parsed = gitim_core::parser::parse_thread(&body).expect("parses");
        assert_eq!(parsed.entries.len(), 1);
        match &parsed.entries[0] {
            gitim_core::types::ThreadEntry::Message(m) => {
                assert!(m.body.contains("first line"));
                assert!(m.body.contains("second line"));
                assert!(m.body.contains("third line"));
            }
            other => panic!("expected message, got {other:?}"),
        }
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
