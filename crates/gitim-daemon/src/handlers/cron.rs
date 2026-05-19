//! Cron trigger handlers.
//!
//! See `docs/plans/2026-05-09-cron-trigger/design.md` for the protocol-level
//! framing — `crons/<name>/spec.yaml` + `crons/<name>/<theoretical_ts>.thread`.
//! The engine that scans + fires lives in `cron_engine.rs` (Task 2.5+); this
//! module only covers the IPC surface (create, list, show, history, enable,
//! disable, delete).
//!
//! Validation is layered:
//!   - `Handler::new` for any handler-shaped string (target, author).
//!   - `gitim_core::types::validate_cron_name` for the directory stem
//!     (lowercase a-z 0-9 hyphen, 1–63 chars, no leading hyphen, not a
//!     reserved word). Lifted to gitim-core so the runtime HTTP layer
//!     (which builds `crons/<name>/<ts>.thread` paths from URL segments)
//!     enforces the exact same rule before any path join — single source
//!     of truth for the path-traversal canary.
//!   - `CronSpec::validate` (in gitim-core) for schedule, timezone, prompt
//!     size, version, created_at format. We construct an in-memory spec and
//!     hand it through that single source of truth so the daemon and
//!     yaml-loader can never disagree.

use std::collections::BTreeMap;

use chrono::SecondsFormat;
use gitim_core::responses::CreateCronResponse;
use gitim_core::types::{validate_cron_name as core_validate_cron_name, CronSpec, Handler};
use tracing::info;

use crate::api::Response;
use crate::cron_paths::parse_thread_filename_ts;
use crate::handlers::ensure_author_not_departed;
use crate::state::SharedState;

/// Wrap the shared validator so daemon call sites can keep their existing
/// `Result<(), Response>` shape. The `error_code: "invalid_name"` contract
/// is identical across every variant — clients only ever differentiate on
/// the code, not the human message.
fn validate_cron_name(name: &str) -> Result<(), Response> {
    core_validate_cron_name(name)
        .map_err(|e| Response::error_with_code(e.to_string(), "invalid_name"))
}

/// Create `crons/<name>/spec.yaml` with the given schedule + target +
/// prompt, validate every field, then commit under `commit_lock`.
///
/// `target` accepts the literal string `@self`, which is rewritten in
/// place to the resolved author handler before the spec is built.
/// Resolution happens here (not at the dispatch layer) because the
/// author is the only place we can be sure of the substitution context.
///
/// Validation order is intentional: cheap structural checks first
/// (name, target, author handler shape), then a probe of the on-disk
/// archive/active spec collisions, then the spec body — schedule,
/// timezone, prompt — through `CronSpec::validate`. Each step returns
/// a typed `error_code` so clients (CLI, WebUI) can render specific
/// messages without parsing the human-readable `error`.
pub async fn handle_create_cron(
    state: SharedState,
    name: String,
    schedule: String,
    timezone: Option<String>,
    target: String,
    prompt: String,
    author: String,
) -> Response {
    // 1. Validate name — bounds + charset + reserved words.
    if let Err(resp) = validate_cron_name(&name) {
        return resp;
    }

    // 2. Validate author handler format. Same source of truth as every
    //    other write path (handle_send / handle_archive_*).
    let author_handler = match Handler::new(&author) {
        Ok(h) => h,
        Err(e) => {
            return Response::error_with_code(format!("invalid author: {}", e), "invalid_author")
        }
    };

    // Departed-author guard. Same shape as send / channel / dm / card /
    // user mutation handlers — once `archive/users/<author>.meta.yaml`
    // exists the actor identity is terminally retired and cannot author
    // new cron specs (or any active-path commit). The error_code lets
    // the CLI surface the reason without parsing English.
    if let Err(resp) = ensure_author_not_departed(&state, author_handler.as_str()) {
        // The shared helper returns a generic error without a code; we
        // re-wrap with `self_departed` so cron-aware clients can branch.
        let _ = resp; // keep for parity if helper ever grows codes
        return Response::error_with_code(
            format!("user @{} is departed", author_handler.as_str()),
            "self_departed",
        );
    }

    // 3. Resolve `@self` (case-insensitive) → author. Anything else must
    //    parse as a Handler and exist in `users/<target>.meta.yaml`. We
    //    don't allow creating crons for departed users —
    //    `archive/users/<h>.meta.yaml` doesn't count as "exists".
    //
    //    Case-insensitive on `@self` mirrors how CLI users naturally
    //    type it. `@SELF` / `@Self` would otherwise fall through into
    //    `Handler::new("SELF")` which rejects on the uppercase rule —
    //    confusing for what is supposed to be a sugar alias, not a
    //    handler.
    let stripped_at = target.strip_prefix('@').unwrap_or(&target);
    let resolved_target = if stripped_at.eq_ignore_ascii_case("self") {
        author_handler.clone()
    } else {
        let h = match Handler::new(stripped_at) {
            Ok(h) => h,
            Err(e) => {
                return Response::error_with_code(
                    format!("invalid target handler: {}", e),
                    "invalid_target",
                )
            }
        };
        let target_meta = state
            .repo_root
            .join("users")
            .join(format!("{}.meta.yaml", h.as_str()));
        if !target_meta.exists() {
            return Response::error_with_code(
                format!("target user @{} not found", h.as_str()),
                "target_not_found",
            );
        }
        h
    };

    // 4. Uniqueness — both active and archived paths. Doing this before
    //    spec construction so the user gets a name-conflict error rather
    //    than a generic "spec invalid" one.
    let cron_dir = state.repo_root.join("crons").join(&name);
    let active_spec = cron_dir.join("spec.yaml");
    if active_spec.exists() {
        return Response::error_with_code(
            format!("cron '{}' already exists", name),
            "name_conflict",
        );
    }
    let archive_spec = state
        .repo_root
        .join("archive/crons")
        .join(&name)
        .join("spec.yaml");
    if archive_spec.exists() {
        return Response::error_with_code(
            format!("cron '{}' exists in archive", name),
            "name_conflict",
        );
    }

    // 5. Build the spec in memory + run `validate`. This is the single
    //    source of truth shared with the YAML loader — schedule,
    //    timezone, prompt, version, created_at all checked in one shot.
    let now_iso = chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let spec = CronSpec {
        version: gitim_core::types::cron::CURRENT_VERSION,
        schedule: schedule.clone(),
        timezone: timezone.clone(),
        target: resolved_target.clone(),
        prompt,
        enabled: true,
        created_by: author_handler.clone(),
        created_at: now_iso,
        extra: BTreeMap::new(),
    };

    if let Err(e) = spec.validate() {
        // Map the typed error into a stable error_code. This is the only
        // place the wire surfaces these distinctions; the CLI/WebUI maps
        // them to user-facing messages.
        let code = match &e {
            gitim_core::types::CronSpecError::InvalidSchedule(_) => "invalid_schedule",
            gitim_core::types::CronSpecError::InvalidTimezone(_) => "invalid_timezone",
            gitim_core::types::CronSpecError::EmptyPrompt => "prompt_empty",
            gitim_core::types::CronSpecError::OversizedPrompt { .. } => "prompt_too_large",
            gitim_core::types::CronSpecError::InvalidVersion(_) => "invalid_version",
            gitim_core::types::CronSpecError::InvalidCreatedAt(_)
            | gitim_core::types::CronSpecError::CreatedAtNotUtc(_) => "invalid_created_at",
            gitim_core::types::CronSpecError::Yaml(_) => "invalid_spec",
        };
        return Response::error_with_code(format!("{}", e), code);
    }

    // 6. Serialize the spec for disk. Done before taking the lock so the
    //    critical section is just fs + git work.
    let spec_yaml = match spec.to_yaml() {
        Ok(s) => s,
        Err(e) => {
            return Response::error_with_code(
                format!("failed to serialize cron spec: {}", e),
                "serialize_failed",
            )
        }
    };

    // 7. Write under the commit-tree lock. Same pattern as
    //    `handle_send` / `handle_archive_dm`: read-mutate-commit happens
    //    while holding `commit_lock` so a concurrent writer (or
    //    sync_loop's rebase) can't slip a `git add` in between our
    //    file write and our `add_and_commit_as`.
    {
        let _commit_guard = state.commit_lock.lock().unwrap_or_else(|e| e.into_inner());

        // Re-check under lock — between our pre-check and now another
        // create_cron could have raced us into the same directory.
        if active_spec.exists() {
            return Response::error_with_code(
                format!("cron '{}' already exists", name),
                "name_conflict",
            );
        }

        if let Err(e) = std::fs::create_dir_all(&cron_dir) {
            return Response::error_with_code(
                format!("failed to create cron dir: {}", e),
                "fs_error",
            );
        }
        if let Err(e) = std::fs::write(&active_spec, &spec_yaml) {
            // Best-effort cleanup of the (likely empty) directory so a
            // retry sees a clean slate. Ignore failure — fs_error already
            // signals user the operation didn't complete.
            let _ = std::fs::remove_dir(&cron_dir);
            return Response::error_with_code(
                format!("failed to write spec.yaml: {}", e),
                "fs_error",
            );
        }

        let spec_rel = format!("crons/{}/spec.yaml", name);
        let commit_msg = format!("cron: create {} by @{}", name, author_handler.as_str());
        let (author_name, author_email) = state.author_for(author_handler.as_str());
        if let Err(e) = state.git_storage.add_and_commit_as(
            &[&spec_rel],
            &commit_msg,
            Some((&author_name, &author_email)),
        ) {
            // Roll back the on-disk write so the working tree mirrors HEAD.
            // Best-effort — if cleanup fails the user's retry will hit the
            // re-check above and report name_conflict, which surfaces the
            // problem clearly enough.
            let _ = std::fs::remove_file(&active_spec);
            let _ = std::fs::remove_dir(&cron_dir);
            return Response::error_with_code(
                format!("create_cron commit failed: {}", e),
                "commit_failed",
            );
        }

        // commit_guard drops at end of scope before any await below.
    }

    info!(
        "cron '{}' created by @{} (target=@{})",
        name,
        author_handler.as_str(),
        resolved_target.as_str()
    );

    let payload = CreateCronResponse {
        name,
        created_by: author_handler.as_str().to_string(),
        target: resolved_target.as_str().to_string(),
    };
    Response::success(serde_json::to_value(payload).unwrap_or_else(|e| { tracing::error!("serializing response: {e}"); serde_json::Value::Null }))
}

/// Default for `Request::HistoryCron.limit` when the client omits the
/// field. 50 mirrors the daemon's other `default_limit` and matches the
/// "show recent runs" mental model — a year of weekly fires fits.
const DEFAULT_HISTORY_LIMIT: u32 = 50;

/// Hard cap so a malicious or buggy client can't ask for unbounded I/O.
/// 1000 is generous: even at one fire per minute that's ~17 hours of
/// uninterrupted history.
const MAX_HISTORY_LIMIT: u32 = 1000;

/// Number of recent runs surfaced by `show_cron`. Larger queries should
/// go through `history_cron` instead — `show` is the "at a glance"
/// endpoint, history is the paginated one.
const SHOW_RECENT_RUNS: usize = 5;

/// List all active (non-archived) cron triggers, sorted by name.
/// Archived crons under `archive/crons/` are intentionally skipped — the
/// design says the active list excludes archived (mirrors
/// `ListChannelsResponse` vs `ListArchivedChannelsResponse`).
///
/// `next_fire` is computed at list time via `next_fire_after`, anchored
/// from the latest existing `<ts>.thread` filename or, on a fresh spec,
/// from `created_at`. Disabled specs still expose `next_fire` so the UI
/// can render greyed-out future occurrences without recomputing.
///
/// Specs that fail to parse or whose schedule fails to evaluate are
/// **silently dropped** with a warn-log — the list endpoint should never
/// crash on a single bad spec. Defensive: validation runs at create
/// time, but a hand-edited spec.yaml could regress.
pub async fn handle_list_crons(state: SharedState) -> Response {
    use gitim_core::responses::{CronSummary, ListCronsResponse};

    let crons_dir = state.repo_root.join("crons");
    let mut summaries: Vec<CronSummary> = Vec::new();
    let now = chrono::Utc::now();

    if let Ok(entries) = std::fs::read_dir(&crons_dir) {
        let mut names: Vec<String> = entries
            .flatten()
            .filter_map(|e| {
                let ft = e.file_type().ok()?;
                if !ft.is_dir() {
                    return None;
                }
                Some(e.file_name().to_string_lossy().to_string())
            })
            .collect();
        names.sort();
        for name in names {
            let spec_path = crons_dir.join(&name).join("spec.yaml");
            let spec = match read_spec(&spec_path) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let next_fire = compute_next_fire(&crons_dir.join(&name), &spec, now);
            summaries.push(CronSummary {
                name: name.clone(),
                schedule: spec.schedule.clone(),
                timezone: spec.timezone.clone(),
                target: spec.target.as_str().to_string(),
                enabled: spec.enabled,
                created_by: spec.created_by.as_str().to_string(),
                created_at: spec.created_at.clone(),
                next_fire,
            });
        }
    }

    let payload = ListCronsResponse { crons: summaries };
    Response::success(serde_json::to_value(payload).unwrap_or_else(|e| { tracing::error!("serializing response: {e}"); serde_json::Value::Null }))
}

/// Read a single cron spec + the most recent `SHOW_RECENT_RUNS` past
/// fires + the computed next-fire timestamp. 404 on missing or
/// unreadable spec.yaml.
pub async fn handle_show_cron(state: SharedState, name: String) -> Response {
    use gitim_core::responses::CronDetail;

    if let Err(resp) = validate_cron_name(&name) {
        return resp;
    }

    let cron_dir = state.repo_root.join("crons").join(&name);
    let spec_path = cron_dir.join("spec.yaml");
    if !spec_path.exists() {
        return Response::error_with_code(format!("cron '{}' does not exist", name), "not_found");
    }
    let spec = match read_spec(&spec_path) {
        Ok(s) => s,
        Err(e) => {
            return Response::error_with_code(
                format!("failed to read cron '{}': {}", name, e),
                "spec_unreadable",
            )
        }
    };

    // Surface the raw yaml as a structured value rather than re-serializing
    // the typed `CronSpec` — this preserves any `extra` (forward-compat)
    // fields verbatim and avoids a double round-trip through the type.
    let raw_yaml = std::fs::read_to_string(&spec_path).unwrap_or_default();
    let spec_value: serde_yaml::Value = match serde_yaml::from_str(&raw_yaml) {
        Ok(v) => v,
        Err(e) => {
            return Response::error_with_code(
                format!("failed to parse cron '{}' yaml: {}", name, e),
                "spec_unreadable",
            )
        }
    };

    let runs = list_thread_runs(&cron_dir, Some(SHOW_RECENT_RUNS));
    let now = chrono::Utc::now();
    let next_fire = compute_next_fire(&cron_dir, &spec, now);

    let payload = CronDetail {
        name,
        spec: spec_value,
        recent_runs: runs,
        next_fire,
    };
    Response::success(serde_json::to_value(payload).unwrap_or_else(|e| { tracing::error!("serializing response: {e}"); serde_json::Value::Null }))
}

/// List `<ts>.thread` files for a cron, newest first, capped by `limit`
/// (default 50, max 1000). 404 on missing cron directory.
pub async fn handle_history_cron(state: SharedState, name: String, limit: Option<u32>) -> Response {
    use gitim_core::responses::HistoryCronResponse;

    if let Err(resp) = validate_cron_name(&name) {
        return resp;
    }

    let cron_dir = state.repo_root.join("crons").join(&name);
    if !cron_dir.is_dir() {
        return Response::error_with_code(format!("cron '{}' does not exist", name), "not_found");
    }
    if !cron_dir.join("spec.yaml").exists() {
        // A bare directory without spec.yaml shouldn't happen in normal
        // operation but treat it as a not-found rather than returning
        // possibly-stale runs from an orphaned dir.
        return Response::error_with_code(format!("cron '{}' does not exist", name), "not_found");
    }

    let limit_u = limit
        .unwrap_or(DEFAULT_HISTORY_LIMIT)
        .min(MAX_HISTORY_LIMIT) as usize;
    let runs = list_thread_runs(&cron_dir, Some(limit_u));

    let payload = HistoryCronResponse { name, runs };
    Response::success(serde_json::to_value(payload).unwrap_or_else(|e| { tracing::error!("serializing response: {e}"); serde_json::Value::Null }))
}

/// Read + parse a `spec.yaml`. Errors are surfaced as `String` because
/// callers (list / show / history) handle them differently — list just
/// skips, show returns 404-ish.
fn read_spec(path: &std::path::Path) -> Result<CronSpec, String> {
    let body = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    CronSpec::from_yaml(&body).map_err(|e| e.to_string())
}

/// Glob `<cron_dir>/*.thread`, parse each filename as a theoretical fire
/// timestamp, sort newest-first, optionally truncate to `limit`. The
/// filename stem doubles as the canonical `ts` field in the response.
fn list_thread_runs(
    cron_dir: &std::path::Path,
    limit: Option<usize>,
) -> Vec<gitim_core::responses::CronRunEntry> {
    use gitim_core::responses::CronRunEntry;

    let mut entries: Vec<CronRunEntry> = Vec::new();
    let rd = match std::fs::read_dir(cron_dir) {
        Ok(r) => r,
        Err(_) => return entries,
    };
    for entry in rd.flatten() {
        let fname = entry.file_name().to_string_lossy().to_string();
        let stem = match fname.strip_suffix(".thread") {
            Some(s) => s.to_string(),
            None => continue,
        };
        // Skip non-fire thread files defensively — we don't expect any
        // (cron threads are always `<ts>.thread`) but a stray file
        // shouldn't crash the listing.
        if parse_thread_filename_ts(&stem).is_none() {
            continue;
        }
        entries.push(CronRunEntry {
            ts: stem,
            filename: fname,
        });
    }
    // Newest first — the filename stems are ISO 8601 UTC with `:` → `-`,
    // which sorts lexicographically the same as chronologically.
    entries.sort_by(|a, b| b.ts.cmp(&a.ts));
    if let Some(n) = limit {
        entries.truncate(n);
    }
    entries
}

/// Compute the next theoretical fire after `now`, anchored from the
/// latest existing `<ts>.thread` filename or, if none exist,
/// `spec.created_at`. Mirrors the engine's `last_fire` resolution
/// precisely so list/show stay consistent with what scan_due will see.
fn compute_next_fire(
    cron_dir: &std::path::Path,
    spec: &CronSpec,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<String> {
    use gitim_core::types::cron::next_fire_after;

    let mut latest: Option<chrono::DateTime<chrono::Utc>> = None;
    if let Ok(rd) = std::fs::read_dir(cron_dir) {
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
    }

    let raw_anchor = match latest {
        Some(ts) => ts,
        None => match chrono::DateTime::parse_from_rfc3339(&spec.created_at) {
            Ok(dt) => dt.with_timezone(&chrono::Utc),
            Err(_) => return None,
        },
    };

    // Mirror the engine's GRACE_WINDOW clamp so list/show reports
    // exactly what `cron_engine::scan_due` will see on its next tick.
    // `next_fire` may land in the past 120s when a spec is overdue but
    // inside the grace window — the engine will fire it on the very
    // next tick. UI layers wanting to display "about to fire" branch
    // on that locally.
    //
    // The 120s window is duplicated from `cron_engine::GRACE_WINDOW`
    // (kept module-private — the engine is source of truth).
    let cutoff = now - chrono::Duration::seconds(120);
    let anchor = if raw_anchor < cutoff {
        cutoff
    } else {
        raw_anchor
    };
    next_fire_after(spec, anchor)
        .ok()
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

/// Set `spec.yaml#enabled = true`. Idempotent: when the spec is already
/// enabled, returns `changed: false` and produces no commit.
pub async fn handle_enable_cron(state: SharedState, name: String, author: String) -> Response {
    toggle_enabled(state, name, author, true).await
}

/// Set `spec.yaml#enabled = false`. Idempotent: when the spec is already
/// disabled, returns `changed: false` and produces no commit.
pub async fn handle_disable_cron(state: SharedState, name: String, author: String) -> Response {
    toggle_enabled(state, name, author, false).await
}

/// Soft-delete a cron: `git mv crons/<name>/ archive/crons/<name>/`.
/// History (every `<ts>.thread`) moves with it.
///
/// Conventions matched from `handle_archive_channel`:
/// - top-level `archive/<dir>/<name>/` (not `<dir>/.archive/<name>/`)
/// - `git mv` rather than `mv + rm`, so blame and rename detection work
/// - rollback the mv on commit failure so the working tree mirrors HEAD
///
/// Differs in that the cron archive moves a *directory*, not two files.
/// `git mv <src-dir> <dest-dir>` requires `<dest-dir>`'s parent to exist
/// but `<dest-dir>` itself must not — we guarantee that with
/// `create_dir_all(archive/crons)` and a stat on `archive/crons/<name>/`
/// before issuing the mv.
pub async fn handle_delete_cron(state: SharedState, name: String, author: String) -> Response {
    use gitim_core::responses::DeleteCronResponse;

    if let Err(resp) = validate_cron_name(&name) {
        return resp;
    }

    let author_handler = match Handler::new(&author) {
        Ok(h) => h,
        Err(e) => {
            return Response::error_with_code(format!("invalid author: {}", e), "invalid_author")
        }
    };

    // Departed-author guard — same as create / enable / disable. Without
    // this, a departed user (or someone editing me.json to a departed
    // handler) could still archive crons.
    if ensure_author_not_departed(&state, author_handler.as_str()).is_err() {
        return Response::error_with_code(
            format!("user @{} is departed", author_handler.as_str()),
            "self_departed",
        );
    }

    let cron_dir = state.repo_root.join("crons").join(&name);
    let active_spec = cron_dir.join("spec.yaml");
    if !active_spec.exists() {
        // Either missing or already in archive — caller's mental model is
        // "not there" either way. The CLI / WebUI can re-list to confirm.
        return Response::error_with_code(format!("cron '{}' does not exist", name), "not_found");
    }
    let archive_target = state.repo_root.join("archive/crons").join(&name);
    if archive_target.exists() {
        // Orphaned: active path exists AND archive path exists.
        // Refuse rather than silently overwrite the archive — the user
        // can resolve manually (look at git log + decide which to keep).
        return Response::error_with_code(
            format!(
                "cron '{}' already has an archive entry; delete aborted",
                name
            ),
            "archive_conflict",
        );
    }

    // Ensure archive/crons/ parent exists so `git mv` doesn't fail with
    // "No such file or directory". `git mv crons/foo archive/crons/foo`
    // renames the directory iff archive/crons/ exists and
    // archive/crons/foo does not.
    let archive_parent = state.repo_root.join("archive/crons");
    if let Err(e) = std::fs::create_dir_all(&archive_parent) {
        return Response::error_with_code(
            format!("failed to create archive/crons dir: {}", e),
            "fs_error",
        );
    }

    {
        let _commit_guard = state.commit_lock.lock().unwrap_or_else(|e| e.into_inner());

        // Re-stat under lock — a concurrent delete could have raced us.
        if !active_spec.exists() {
            return Response::error_with_code(
                format!("cron '{}' does not exist", name),
                "not_found",
            );
        }

        let from_rel = format!("crons/{}", name);
        let to_rel = format!("archive/crons/{}", name);
        if let Err(e) = state.git_storage.mv(&from_rel, &to_rel) {
            return Response::error_with_code(format!("git mv failed: {}", e), "git_error");
        }

        let commit_msg = format!("cron: delete {} by @{}", name, author_handler.as_str());
        let (author_name, author_email) = state.author_for(author_handler.as_str());
        // For dir-rename, `git add <dest-dir>` is sufficient — git mv
        // already staged the rename, and a path-list `add` covers any
        // post-mv content. Pass the destination dir; git resolves it
        // recursively against the index.
        if let Err(e) = state.git_storage.add_and_commit_as(
            &[&to_rel],
            &commit_msg,
            Some((&author_name, &author_email)),
        ) {
            // Rollback: reverse the rename.
            if let Err(rb) = state.git_storage.mv(&to_rel, &from_rel) {
                tracing::warn!("delete_cron: rollback git mv also failed: {}", rb);
            }
            return Response::error_with_code(
                format!("delete_cron commit failed: {}", e),
                "commit_failed",
            );
        }
        // commit_guard drops at end of scope.
    }

    info!("cron '{}' deleted by @{}", name, author_handler.as_str());

    let payload = DeleteCronResponse {
        name,
        deleted_by: author_handler.as_str().to_string(),
    };
    Response::success(serde_json::to_value(payload).unwrap_or_else(|e| { tracing::error!("serializing response: {e}"); serde_json::Value::Null }))
}

/// Shared body for enable + disable. Reads spec, compares state, and
/// either commits a flipped spec or short-circuits as a no-op.
///
/// `not_found` is returned for archived crons too — once a spec has
/// been moved to `archive/crons/<name>/`, enable/disable refuse rather
/// than silently writing into the archive (which would also break
/// "archive is frozen audit data" — see the channel-archive precedent).
async fn toggle_enabled(
    state: SharedState,
    name: String,
    author: String,
    target: bool,
) -> Response {
    use gitim_core::responses::ToggleCronResponse;

    if let Err(resp) = validate_cron_name(&name) {
        return resp;
    }
    let author_handler = match Handler::new(&author) {
        Ok(h) => h,
        Err(e) => {
            return Response::error_with_code(format!("invalid author: {}", e), "invalid_author")
        }
    };

    // Departed-author guard — disable / enable mutate spec.yaml.enabled
    // and commit; both are active-path writes that must respect the
    // archive-protocol Contract 2 rule.
    if ensure_author_not_departed(&state, author_handler.as_str()).is_err() {
        return Response::error_with_code(
            format!("user @{} is departed", author_handler.as_str()),
            "self_departed",
        );
    }

    let spec_path = state.repo_root.join("crons").join(&name).join("spec.yaml");
    if !spec_path.exists() {
        return Response::error_with_code(format!("cron '{}' does not exist", name), "not_found");
    }

    let mut spec = match read_spec(&spec_path) {
        Ok(s) => s,
        Err(e) => {
            return Response::error_with_code(
                format!("failed to read cron '{}': {}", name, e),
                "spec_unreadable",
            )
        }
    };

    if spec.enabled == target {
        // Idempotent no-op. No write, no commit. Surface `changed: false`
        // so the caller can distinguish this from a real toggle.
        let payload = ToggleCronResponse {
            name,
            enabled: target,
            changed: false,
        };
        return Response::success(serde_json::to_value(payload).unwrap_or_else(|e| { tracing::error!("serializing response: {e}"); serde_json::Value::Null }));
    }

    spec.enabled = target;
    let new_yaml = match spec.to_yaml() {
        Ok(s) => s,
        Err(e) => {
            return Response::error_with_code(
                format!("failed to serialize cron spec: {}", e),
                "serialize_failed",
            )
        }
    };

    {
        let _commit_guard = state.commit_lock.lock().unwrap_or_else(|e| e.into_inner());

        // Re-read under lock and bail out if another writer already
        // toggled to the requested state. Guards against an interleaved
        // enable/disable racing us.
        let cur = match read_spec(&spec_path) {
            Ok(s) => s,
            Err(e) => {
                return Response::error_with_code(
                    format!("failed to re-read cron under lock: {}", e),
                    "spec_unreadable",
                )
            }
        };
        if cur.enabled == target {
            let payload = ToggleCronResponse {
                name,
                enabled: target,
                changed: false,
            };
            return Response::success(serde_json::to_value(payload).unwrap_or_else(|e| { tracing::error!("serializing response: {e}"); serde_json::Value::Null }));
        }

        if let Err(e) = std::fs::write(&spec_path, &new_yaml) {
            return Response::error_with_code(
                format!("failed to write spec.yaml: {}", e),
                "fs_error",
            );
        }

        let action = if target { "enable" } else { "disable" };
        let commit_msg = format!("cron: {} {} by @{}", action, name, author_handler.as_str());
        let spec_rel = format!("crons/{}/spec.yaml", name);
        let (author_name, author_email) = state.author_for(author_handler.as_str());
        if let Err(e) = state.git_storage.add_and_commit_as(
            &[&spec_rel],
            &commit_msg,
            Some((&author_name, &author_email)),
        ) {
            // Rollback: restore previous yaml so working tree mirrors HEAD.
            if let Err(rb) = cur
                .to_yaml()
                .map_err(|e| e.to_string())
                .and_then(|y| std::fs::write(&spec_path, y).map_err(|e| e.to_string()))
            {
                tracing::warn!("toggle_enabled: rollback restore failed: {}", rb);
            }
            return Response::error_with_code(
                format!("toggle commit failed: {}", e),
                "commit_failed",
            );
        }
        // commit_guard drops here.
    }

    info!(
        "cron '{}' {} by @{}",
        name,
        if target { "enabled" } else { "disabled" },
        author_handler.as_str()
    );

    let payload = ToggleCronResponse {
        name,
        enabled: target,
        changed: true,
    };
    Response::success(serde_json::to_value(payload).unwrap_or_else(|e| { tracing::error!("serializing response: {e}"); serde_json::Value::Null }))
}

#[cfg(test)]
mod compute_next_fire_tests {
    use super::*;
    use chrono::TimeZone;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    fn alice() -> Handler {
        Handler::new("alice").unwrap()
    }

    fn build_spec(schedule: &str, created_at: &str) -> CronSpec {
        CronSpec {
            version: 1,
            schedule: schedule.to_string(),
            timezone: None,
            target: alice(),
            prompt: "hi".to_string(),
            enabled: true,
            created_by: alice(),
            created_at: created_at.to_string(),
            extra: BTreeMap::new(),
        }
    }

    /// After GRACE_WINDOW alignment, compute_next_fire reports the
    /// engine's actual next fire — which can land in the past 120s
    /// when the spec is overdue but inside the window.
    #[test]
    fn returns_overdue_when_in_grace_window() {
        let tmp = TempDir::new().unwrap();
        // Spec with a stale anchor (well outside the grace window).
        // Engine clamps the anchor to now-120s, and `* * * * *` after
        // now-120s lands in the past relative to `now`.
        let spec = build_spec("* * * * *", "2026-05-01T00:00:00Z");
        // now = 2026-05-09 00:00:30; cutoff = now - 120s = 23:58:30.
        // `* * * * *` fires on each minute boundary, so next after
        // 23:58:30 is 23:59:00 — ≤ now → returned.
        let now = chrono::Utc.with_ymd_and_hms(2026, 5, 9, 0, 0, 30).unwrap();
        let nf = compute_next_fire(tmp.path(), &spec, now).expect("returns a value");
        let parsed: chrono::DateTime<chrono::Utc> = chrono::DateTime::parse_from_rfc3339(&nf)
            .unwrap()
            .with_timezone(&chrono::Utc);
        // Either the past-but-recent minute boundary (engine will fire
        // it on next tick), or the immediately upcoming one. Crucially,
        // it MUST be ≥ cutoff (no ancient backlog leaking through).
        let cutoff = now - chrono::Duration::seconds(120);
        assert!(
            parsed >= cutoff,
            "compute_next_fire returned {} which is older than cutoff {}",
            parsed,
            cutoff
        );
        // And it must be earlier than `now + 60s` — within one tick of the
        // engine — i.e. `compute_next_fire` is not over-clamping forward.
        assert!(
            parsed <= now + chrono::Duration::seconds(60),
            "compute_next_fire returned {}, expected within ±60s of now {}",
            parsed,
            now
        );
    }

    /// When the most recent fire IS within the grace window, the anchor
    /// stays at that fire and next_fire_after produces the very next
    /// schedule match — no clamp interference. Sanity check that the
    /// fix doesn't break the common case.
    #[test]
    fn returns_next_after_recent_fire() {
        let tmp = TempDir::new().unwrap();
        let now = chrono::Utc.with_ymd_and_hms(2026, 5, 9, 12, 0, 30).unwrap();

        // Drop a fake `<ts>.thread` file at now-30s so latest_fire picks it up.
        let recent_fire_ts = now - chrono::Duration::seconds(30);
        let stem = crate::cron_paths::format_thread_filename_ts(recent_fire_ts);
        std::fs::write(
            tmp.path().join(format!("{stem}.thread")),
            "[L000001][P000000][@system][20260509T120000Z] cron(x): y\n",
        )
        .unwrap();

        let spec = build_spec("* * * * *", "2026-05-01T00:00:00Z");
        let nf = compute_next_fire(tmp.path(), &spec, now).expect("returns a value");
        let parsed: chrono::DateTime<chrono::Utc> = chrono::DateTime::parse_from_rfc3339(&nf)
            .unwrap()
            .with_timezone(&chrono::Utc);
        // Recent fire was at :00:00, schedule `* * * * *` → next match
        // strictly after :00:00 is :01:00 — strictly future relative to now (:00:30).
        assert!(
            parsed > recent_fire_ts,
            "next_fire {} should be strictly after the recent fire {}",
            parsed,
            recent_fire_ts
        );
    }
}
