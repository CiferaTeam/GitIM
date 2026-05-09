use crate::api::{Event, Response};
use crate::handlers::ensure_author_not_departed;
use crate::state::SharedState;
use gitim_core::dm::parse_dm_filename;
use gitim_core::formatter::format_event;
use gitim_core::parser::parse_thread;
use gitim_core::types::{ChannelMeta, Handler, ThreadEntry, UserMeta};
use gitim_sync::git::GitError;
use std::path::Path;
use tracing::{info, warn};

const MAX_PUSH_RETRIES: u32 = 3;

pub async fn handle_register_user(
    state: SharedState,
    handler: String,
    display_name: String,
    role: String,
    introduction: String,
) -> Response {
    // Validate handler format
    if let Err(e) = Handler::new(&handler) {
        return Response::error(format!("invalid handler: {}", e));
    }

    // Archive Contract 2: handlers are terminally unique once departed.
    // Reject re-registration before touching `users/` so a stale call
    // can't recreate an active meta.yaml that would race with the
    // archived one.
    let archive_meta = state
        .repo_root
        .join("archive/users")
        .join(format!("{}.meta.yaml", handler));
    if archive_meta.exists() {
        return Response::error(format!(
            "handler @{} is reserved (previously departed)",
            handler
        ));
    }

    let users_dir = state.repo_root.join("users");
    std::fs::create_dir_all(&users_dir).ok();
    let meta_path = users_dir.join(format!("{}.meta.yaml", handler));

    // If already exists, ensure user is in memory list and return success
    if meta_path.exists() {
        let mut users = state.users.write().await;
        if !users.contains(&handler) {
            users.push(handler.clone());
            users.sort();
        }
        let payload = gitim_core::responses::RegisterUserResponse {
            handler,
            exists: true,
        };
        return Response::success(serde_json::to_value(payload).unwrap());
    }

    // Create meta file
    let meta = UserMeta {
        display_name,
        role,
        introduction,
    };
    let meta_str = serde_yaml::to_string(&meta).unwrap();

    if let Err(e) = std::fs::write(&meta_path, &meta_str) {
        return Response::error(format!("failed to write user meta: {}", e));
    }

    // Add to users list
    {
        let mut users = state.users.write().await;
        if !users.contains(&handler) {
            users.push(handler.clone());
            users.sort();
        }
    }

    // Git add + commit (best effort)
    let (author_name, author_email) = state.author_for(&handler);
    let _ = state.git_storage.add_and_commit_as(
        &[&format!("users/{}.meta.yaml", handler)],
        &format!("user: register @{}", handler),
        Some((&author_name, &author_email)),
    );

    let payload = gitim_core::responses::RegisterUserResponse {
        handler,
        exists: false,
    };
    Response::success(serde_json::to_value(payload).unwrap())
}

/// Move `users/<handler>.meta.yaml` → `archive/users/<handler>.meta.yaml`
/// in a single commit. Mirrors `handle_archive_channel` in shape: validate,
/// `git mv`, commit-as-author, push with retry, rollback on failure.
///
/// Unlike channels, users have no creator-only permission gate at this
/// layer — the depart_user composite (A.4) is the principal caller and
/// daemon resolves the author. A direct call from a registered user is
/// treated as authoritative; refining permissions is deferred to A.5/A.7.
pub async fn handle_archive_user(state: SharedState, handler: String, author: String) -> Response {
    // 1. Validate handler format.
    if let Err(e) = Handler::new(&handler) {
        return Response::error(format!("invalid handler: {}", e));
    }

    // 2. Validate author is registered + not already departed.
    // archive_user gates on departed author for consistency with the rest
    // of Contract 2 — a departed actor can't author further commits, even
    // ones that archive someone else. unarchive_user, by contrast, must
    // remain reachable so departure is reversible.
    if let Err(resp) = ensure_author_not_departed(&state, &author) {
        return resp;
    }
    {
        let users = state.users.read().await;
        if !users.contains(&author) {
            return Response::error(format!("unknown user: {}", author));
        }
    }

    // Check archive path before active path: if both somehow exist (e.g., split-brain
    // recovery), the user-actionable state is "already archived"; if only archive exists,
    // reporting "not found" would mislead the caller.
    let archive_dir = state.repo_root.join("archive/users");
    let archive_path = archive_dir.join(format!("{}.meta.yaml", handler));
    if archive_path.exists() {
        return Response::error(format!("user @{} is already archived", handler));
    }

    // 4. Validate active path exists.
    let active_path = state.repo_root.join(format!("users/{}.meta.yaml", handler));
    if !active_path.exists() {
        return Response::error(format!("user @{} not found", handler));
    }

    // 5. Ensure archive/users/ directory exists.
    if let Err(e) = std::fs::create_dir_all(&archive_dir) {
        return Response::error(format!("failed to create archive/users dir: {}", e));
    }

    // Commit-tree lock: held across git mv + commit + push so a concurrent
    // `handle_send` (also takes this lock) can't slip a `git add` + `git
    // commit` in between our staged mv and our `add_and_commit_as`, which
    // would bundle the unrelated send into our archive commit. Critical
    // section is all blocking subprocess calls; std::sync::Mutex guard
    // must not cross any `.await`.
    let _commit_guard = state.commit_lock.lock().expect("commit_lock poisoned");

    // 6. git mv users/<h>.meta.yaml → archive/users/<h>.meta.yaml
    let from_rel = format!("users/{}.meta.yaml", handler);
    let to_rel = format!("archive/users/{}.meta.yaml", handler);
    if let Err(e) = state.git_storage.mv(&from_rel, &to_rel) {
        return Response::error(format!("git mv failed: {}", e));
    }

    // 7. Commit. On failure, reverse the git mv to leave the tree clean.
    let commit_msg = format!("archive: depart user @{}", handler);
    let (author_name, author_email) = state.author_for(&author);
    if let Err(e) = state.git_storage.add_and_commit_as(
        &[&to_rel],
        &commit_msg,
        Some((&author_name, &author_email)),
    ) {
        if let Err(rb) = state.git_storage.mv(&to_rel, &from_rel) {
            warn!("archive_user: rollback git mv also failed: {}", rb);
        }
        return Response::error(format!(
            "archive_user commit failed: {}; rolled back git mv",
            e
        ));
    }

    // 8. Push with retry (skip if no remote).
    if state.git_storage.has_remote() {
        let mut pushed = false;
        for attempt in 1..=MAX_PUSH_RETRIES {
            match state.git_storage.push() {
                Ok(()) => {
                    pushed = true;
                    break;
                }
                Err(GitError::PushConflict) => {
                    warn!(
                        "archive_user: push conflict (attempt {}/{}), rebasing",
                        attempt, MAX_PUSH_RETRIES
                    );
                    if let Err(e) = state.git_storage.fetch() {
                        return Response::error(format!("archive_user fetch failed: {}", e));
                    }
                    if let Err(e) = state.git_storage.rebase_onto_origin() {
                        return Response::error(format!("archive_user rebase failed: {}", e));
                    }
                }
                Err(e) => {
                    return Response::error(format!("archive_user push failed: {}", e));
                }
            }
        }
        if !pushed {
            return Response::error(format!(
                "archive_user: push still conflicting after {} retries",
                MAX_PUSH_RETRIES
            ));
        }
    }

    // Commit tree is stable — drop the lock BEFORE any `.await` below.
    // std::sync::MutexGuard must not cross await points, and everything
    // from here on (in-memory users update, event broadcast) is non-mutating
    // for the commit tree.
    drop(_commit_guard);

    // 9. Drop archived handler from in-memory users list. The post-sync
    //    refresh in state.rs already does this from disk after the next
    //    cycle, but updating now keeps `list_users` consistent before sync.
    {
        let mut users = state.users.write().await;
        users.retain(|u| u != &handler);
    }

    // Broadcast SSE event so subscribers (WebUI / runtime) can react without
    // waiting for the next sync cycle. Symmetric with Event::CardArchived.
    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let _ = state.event_tx.send(Event::UserArchived {
        handler: handler.clone(),
        archived_by: author.clone(),
        timestamp,
    });

    info!("user @{} archived by @{}", handler, author);

    let payload = gitim_core::responses::ArchiveUserResponse {
        handler,
        archived_by: author,
    };
    Response::success(serde_json::to_value(payload).unwrap())
}

/// Restore `archive/users/<handler>.meta.yaml` → `users/<handler>.meta.yaml`.
/// Symmetric reverse of `handle_archive_user`; same rollback semantics.
pub async fn handle_unarchive_user(
    state: SharedState,
    handler: String,
    author: String,
) -> Response {
    // 1. Validate handler format.
    if let Err(e) = Handler::new(&handler) {
        return Response::error(format!("invalid handler: {}", e));
    }

    // 2. Validate author is registered.
    {
        let users = state.users.read().await;
        if !users.contains(&author) {
            return Response::error(format!("unknown user: {}", author));
        }
    }

    // Check archive path before active path: the actionable error in the
    // common case is "no archive entry to restore". If the active path
    // already exists too (split-brain), the conflict guard below catches it
    // — but reporting "archive source missing" first when neither exists
    // matches the user's mental model of unarchive.
    let archive_path = state
        .repo_root
        .join(format!("archive/users/{}.meta.yaml", handler));
    if !archive_path.exists() {
        return Response::error(format!(
            "archive source does not exist for user @{}",
            handler
        ));
    }

    // 4. Active path must not already exist (handler reuse conflict).
    let active_path = state.repo_root.join(format!("users/{}.meta.yaml", handler));
    if active_path.exists() {
        return Response::error(format!(
            "user @{} already exists in active location; unarchive aborted",
            handler
        ));
    }

    // 5. Ensure users/ parent dir exists.
    let users_dir = state.repo_root.join("users");
    if let Err(e) = std::fs::create_dir_all(&users_dir) {
        return Response::error(format!("failed to create users dir: {}", e));
    }

    // Commit-tree lock: held across git mv + commit + push so a concurrent
    // `handle_send` (also takes this lock) can't slip a `git add` + `git
    // commit` in between our staged mv and our `add_and_commit_as`, which
    // would bundle the unrelated send into our unarchive commit. Critical
    // section is all blocking subprocess calls; std::sync::Mutex guard
    // must not cross any `.await`.
    let _commit_guard = state.commit_lock.lock().expect("commit_lock poisoned");

    // 6. git mv archive → active.
    let from_rel = format!("archive/users/{}.meta.yaml", handler);
    let to_rel = format!("users/{}.meta.yaml", handler);
    if let Err(e) = state.git_storage.mv(&from_rel, &to_rel) {
        return Response::error(format!("git mv failed: {}", e));
    }

    // 7. Commit. Rollback git mv on failure.
    let commit_msg = format!("archive: restore user @{}", handler);
    let (author_name, author_email) = state.author_for(&author);
    if let Err(e) = state.git_storage.add_and_commit_as(
        &[&to_rel],
        &commit_msg,
        Some((&author_name, &author_email)),
    ) {
        if let Err(rb) = state.git_storage.mv(&to_rel, &from_rel) {
            warn!("unarchive_user: rollback git mv also failed: {}", rb);
        }
        return Response::error(format!(
            "unarchive_user commit failed: {}; rolled back git mv",
            e
        ));
    }

    // 8. Push with retry.
    if state.git_storage.has_remote() {
        let mut pushed = false;
        for attempt in 1..=MAX_PUSH_RETRIES {
            match state.git_storage.push() {
                Ok(()) => {
                    pushed = true;
                    break;
                }
                Err(GitError::PushConflict) => {
                    warn!(
                        "unarchive_user: push conflict (attempt {}/{}), rebasing",
                        attempt, MAX_PUSH_RETRIES
                    );
                    if let Err(e) = state.git_storage.fetch() {
                        return Response::error(format!("unarchive_user fetch failed: {}", e));
                    }
                    if let Err(e) = state.git_storage.rebase_onto_origin() {
                        return Response::error(format!("unarchive_user rebase failed: {}", e));
                    }
                }
                Err(e) => {
                    return Response::error(format!("unarchive_user push failed: {}", e));
                }
            }
        }
        if !pushed {
            return Response::error(format!(
                "unarchive_user: push still conflicting after {} retries",
                MAX_PUSH_RETRIES
            ));
        }
    }

    // Commit tree is stable — drop the lock BEFORE any `.await` below.
    // std::sync::MutexGuard must not cross await points, and everything
    // from here on (in-memory users update, event broadcast) is non-mutating
    // for the commit tree.
    drop(_commit_guard);

    // 9. Re-add restored handler to in-memory users list (mirror archive's
    //    drop above).
    {
        let mut users = state.users.write().await;
        if !users.contains(&handler) {
            users.push(handler.clone());
            users.sort();
        }
    }

    // Broadcast SSE event so subscribers (WebUI / runtime) can react without
    // waiting for the next sync cycle. Symmetric with Event::CardUnarchived.
    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let _ = state.event_tx.send(Event::UserUnarchived {
        handler: handler.clone(),
        unarchived_by: author.clone(),
        timestamp,
    });

    info!("user @{} unarchived by @{}", handler, author);

    let payload = gitim_core::responses::UnarchiveUserResponse {
        handler,
        unarchived_by: author,
    };
    Response::success(serde_json::to_value(payload).unwrap())
}

/// Composite "burn" operation. Idempotent multi-commit:
///
/// ```text
/// Phase 1: append `leave-workspace` event to every active thread where
///          <handler> has authored at least one entry. One commit per thread.
/// Phase 2: git mv each `dm/X--<handler>.thread` (or `<handler>--X.thread`)
///          into `archive/dm/`. One commit per DM.
/// Phase 3: remove <handler> from each `channels/<ch>.meta.yaml#members`
///          list that contains it. One commit per channel.
/// Phase 4: git mv `users/<handler>.meta.yaml` → `archive/users/`. One commit.
///          Terminal state.
/// ```
///
/// The terminal-state judgment is `archive/users/<handler>.meta.yaml`
/// existing — once that path is on disk the burn is complete and any
/// retry returns success without doing further work. Each phase step
/// also self-checks: a thread whose last entry is already <handler>'s
/// `leave-workspace` is skipped, an already-archived DM is skipped, a
/// channel meta whose members list no longer contains <handler> is
/// skipped. Failure of any commit/push leaves the previous successful
/// commits in place — there is no rollback. A retry resumes from the
/// first not-yet-completed step.
///
/// **Author-bypass scope**: this entire flow runs while the user entry
/// is still active, so `ensure_author_not_departed` (which keys off
/// `archive/users/<handler>.meta.yaml`) does not fire for any of the
/// pre-Phase-4 commits. Phase 4 is the single git mv that creates the
/// archive entry; it doesn't write a thread under the departing handle,
/// so the gate cannot reject it. No bypass needed.
///
/// **Already-archived DMs**: Phase 1 skips them. They are frozen audit
/// data — appending a system-level leave-workspace event to an archived
/// thread would either require breaking Contract 2's "no writes to
/// archive paths" or carving out a privileged side door. The audit
/// record is satisfied by the active DMs, which receive the leave event
/// before being archived in Phase 2 (the leave event ends up in
/// archive/dm/ along with the rest of the thread).
pub async fn handle_depart_user(state: SharedState, handler: String) -> Response {
    // 1. Validate handler format.
    if let Err(e) = Handler::new(&handler) {
        return Response::error(format!("invalid handler: {}", e));
    }

    // 2. Terminal-state judgment. If the user is already in archive/users/,
    //    the burn is complete. Idempotent retry returns success with
    //    `commits: 0, already_departed: true`.
    if is_already_departed(&state, &handler) {
        let payload = gitim_core::responses::DepartUserResponse {
            handler,
            commits: 0,
            already_departed: true,
        };
        return Response::success(serde_json::to_value(payload).unwrap());
    }

    // 3. Validate active user actually exists. This catches the
    //    "depart_user on never-registered handler" case cleanly — without
    //    this the function would do nothing through Phases 1-3 and then
    //    fail at Phase 4 with a confusing git mv error.
    let active_meta = state.repo_root.join(format!("users/{}.meta.yaml", handler));
    if !active_meta.exists() {
        return Response::error(format!("user @{} not found", handler));
    }

    // 4. Run phases. Each phase returns `Result<u64, Response>` where the
    //    u64 is the count of commits produced. Any phase failure
    //    short-circuits. Counts accumulate so the response can report
    //    how much work this invocation actually did.
    let mut total_commits: u64 = 0;

    let n = match phase1_write_leave_events(&state, &handler).await {
        Ok(n) => n,
        Err(resp) => return resp,
    };
    total_commits += n;

    let n = match phase2_archive_dms(&state, &handler).await {
        Ok(n) => n,
        Err(resp) => return resp,
    };
    total_commits += n;

    let n = match phase3_clean_channel_members(&state, &handler).await {
        Ok(n) => n,
        Err(resp) => return resp,
    };
    total_commits += n;

    let n = match phase4_archive_user(&state, &handler).await {
        Ok(n) => n,
        Err(resp) => return resp,
    };
    total_commits += n;

    info!(
        "user @{} departed across {} commit(s)",
        handler, total_commits
    );

    let payload = gitim_core::responses::DepartUserResponse {
        handler,
        commits: total_commits,
        already_departed: false,
    };
    Response::success(serde_json::to_value(payload).unwrap())
}

/// `archive/users/<handler>.meta.yaml` existing is the single source of
/// truth that a depart has completed. All retries gate on this stat.
fn is_already_departed(state: &SharedState, handler: &str) -> bool {
    state
        .repo_root
        .join("archive/users")
        .join(format!("{}.meta.yaml", handler))
        .exists()
}

/// Phase 1 — append `[E:leave-workspace]` to each active thread where
/// <handler> has authored at least one entry. One commit per thread.
///
/// Scans `channels/*.thread` and `dm/*.thread`. Skips `archive/dm/*`
/// (frozen audit data, see top-level handle_depart_user docs) and
/// `archive/channels/*` (likewise frozen, plus archived channels are
/// already past the visibility horizon).
///
/// Per-thread idempotency: if the thread's last entry is already a
/// leave-workspace event by <handler>, skip without committing. This is
/// what lets a partial-failure retry resume cleanly.
async fn phase1_write_leave_events(state: &SharedState, handler: &str) -> Result<u64, Response> {
    let mut thread_paths: Vec<std::path::PathBuf> = Vec::new();
    collect_thread_paths(&state.repo_root.join("channels"), &mut thread_paths);
    collect_thread_paths(&state.repo_root.join("dm"), &mut thread_paths);
    // Stable order so a retry visits threads in the same sequence;
    // makes "skip first 5, finish last 5" recovery semantics testable.
    thread_paths.sort();

    let handler_h = match Handler::new(handler) {
        Ok(h) => h,
        Err(e) => return Err(Response::error(format!("invalid handler: {}", e))),
    };

    let mut commits: u64 = 0;
    for thread_path in &thread_paths {
        // Helper returns Ok(true) when a leave-workspace event was actually
        // appended + committed; Ok(false) when the thread was a clean skip
        // (handler never spoke OR last entry already leave-workspace).
        if append_leave_event_to_thread(state, &handler_h, thread_path).await? {
            commits += 1;
        }
    }
    Ok(commits)
}

/// Read directory entries ending in `.thread`. Silent skip on dirs that
/// don't exist — Phase 1 must work in a workspace with no channels yet.
fn collect_thread_paths(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    if !dir.is_dir() {
        return;
    }
    let rd = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return,
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("thread") {
            out.push(path);
        }
    }
}

/// Inspect a thread, decide whether to write a leave-workspace event,
/// and (if so) append + commit it. Returns Ok(true) when a commit was
/// produced, Ok(false) when the thread was skipped.
///
/// Skip conditions:
/// - `<handler>` has not authored any entry → nothing to record
/// - the thread's last entry is already `<handler>`'s `leave-workspace`
///   event → already done in a prior run
///
/// Concurrency: the commit_lock is held only across the write+commit
/// section, exactly mirroring `write_channel_event`. Push-with-retry
/// runs after the lock is dropped (push is the slow remote step and
/// must not block other writers).
async fn append_leave_event_to_thread(
    state: &SharedState,
    handler: &Handler,
    thread_path: &Path,
) -> Result<bool, Response> {
    // Read once outside the lock to short-circuit the common no-op cases
    // without blocking other writers. Recheck under the lock before
    // committing to defend against an interleaved write.
    let pre = match std::fs::read_to_string(thread_path) {
        Ok(s) => s,
        Err(e) => {
            warn!(
                "phase1: failed to read {} (skipping): {}",
                thread_path.display(),
                e
            );
            return Ok(false);
        }
    };
    let parsed = match parse_thread(&pre) {
        Ok(f) => f,
        Err(e) => {
            warn!(
                "phase1: failed to parse {} (skipping): {}",
                thread_path.display(),
                e
            );
            return Ok(false);
        }
    };

    if !thread_has_author(&parsed.entries, handler) {
        return Ok(false);
    }
    if last_entry_is_leave_workspace(&parsed.entries, handler) {
        return Ok(false);
    }

    // Resolve a relative path for git add. The thread must live under
    // repo_root; if not, something is structurally wrong — skip + warn.
    let rel = match thread_path.strip_prefix(&state.repo_root) {
        Ok(r) => r.to_string_lossy().to_string(),
        Err(_) => {
            warn!(
                "phase1: thread path {} not under repo root, skipping",
                thread_path.display()
            );
            return Ok(false);
        }
    };

    // Hold the commit_lock across read → re-parse → append → commit so
    // a concurrent send can't slip an unrelated commit between our git
    // add and git commit. Mirrors handle_send / write_channel_event.
    {
        let _commit_guard = state.commit_lock.lock().expect("commit_lock poisoned");

        // Re-read under lock — another writer (or a sync_loop rebase)
        // may have moved the file or appended after our pre-read.
        let cur = std::fs::read_to_string(thread_path).unwrap_or_default();
        let cur_parsed = match parse_thread(&cur) {
            Ok(f) => f,
            Err(e) => {
                return Err(Response::error(format!(
                    "phase1: failed to parse {} under lock: {}",
                    thread_path.display(),
                    e
                )));
            }
        };
        // Re-check skip conditions: another phase1 retry running
        // concurrently could have already written this event.
        if !thread_has_author(&cur_parsed.entries, handler) {
            return Ok(false);
        }
        if last_entry_is_leave_workspace(&cur_parsed.entries, handler) {
            return Ok(false);
        }

        let next_line = cur_parsed.last_line_number() + 1;
        let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let event_line = format_event(
            next_line,
            handler,
            &now,
            "leave-workspace",
            &serde_json::json!({}),
        );

        use std::io::Write;
        match std::fs::OpenOptions::new()
            .create(false)
            .append(true)
            .open(thread_path)
        {
            Ok(mut f) => {
                if let Err(e) = f.write_all(event_line.as_bytes()) {
                    return Err(Response::error(format!(
                        "phase1: append to {} failed: {}",
                        thread_path.display(),
                        e
                    )));
                }
            }
            Err(e) => {
                return Err(Response::error(format!(
                    "phase1: open {} failed: {}",
                    thread_path.display(),
                    e
                )));
            }
        }

        let commit_msg = format!("event: @{} leave-workspace", handler.as_str());
        let (an, ae) = state.author_for(handler.as_str());
        if let Err(e) =
            state
                .git_storage
                .add_and_commit_as(&[rel.as_str()], &commit_msg, Some((&an, &ae)))
        {
            // Truncate the appended line so the working tree mirrors HEAD
            // and a retry can re-append cleanly. This is best-effort —
            // if truncate fails the user retries via the same skip
            // logic and gets the same result.
            if let Err(t) = std::fs::write(thread_path, cur.as_bytes()) {
                warn!(
                    "phase1: rollback truncate of {} failed: {}",
                    thread_path.display(),
                    t
                );
            }
            return Err(Response::error(format!(
                "phase1: commit failed for {}: {}",
                thread_path.display(),
                e
            )));
        }

        // commit_guard drops here at end of scope before push.
    }

    // Push outside lock with retry/rebase. Same shape as
    // archive_user / archive_dm.
    push_with_retry(state, "phase1")?;

    // Invalidate any cached parse for this thread so subsequent
    // reads see the new event without waiting on sync_loop.
    if let Some(stem) = thread_path.file_stem().and_then(|s| s.to_str()) {
        state.thread_cache.write().await.remove(stem);
    }

    Ok(true)
}

/// True if any entry in the thread is authored by <handler>.
fn thread_has_author(entries: &[ThreadEntry], handler: &Handler) -> bool {
    entries
        .iter()
        .any(|e| e.author().as_str() == handler.as_str())
}

/// True if the last entry is a `leave-workspace` event authored by
/// <handler>. Used as the per-thread idempotency check.
fn last_entry_is_leave_workspace(entries: &[ThreadEntry], handler: &Handler) -> bool {
    match entries.last() {
        Some(ThreadEntry::Event(ev)) => {
            ev.event_type == "leave-workspace" && ev.author.as_str() == handler.as_str()
        }
        _ => false,
    }
}

/// Phase 2 — git mv each active DM thread that includes <handler> into
/// `archive/dm/`. One commit per DM. Skips DMs already archived (file
/// only exists in archive/dm/, not in dm/).
async fn phase2_archive_dms(state: &SharedState, handler: &str) -> Result<u64, Response> {
    let dm_dir = state.repo_root.join("dm");
    let mut targets: Vec<String> = Vec::new(); // filename stems (without .thread)
    if dm_dir.is_dir() {
        if let Ok(rd) = std::fs::read_dir(&dm_dir) {
            for entry in rd.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                let stem = match fname.strip_suffix(".thread") {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                let (a, b) = match parse_dm_filename(&stem) {
                    Some(p) => p,
                    None => continue,
                };
                if a == handler || b == handler {
                    targets.push(stem);
                }
            }
        }
    }
    targets.sort();

    let archive_dir = state.repo_root.join("archive/dm");
    if !targets.is_empty() {
        if let Err(e) = std::fs::create_dir_all(&archive_dir) {
            return Err(Response::error(format!(
                "phase2: failed to create archive/dm dir: {}",
                e
            )));
        }
    }

    let mut commits: u64 = 0;
    for stem in &targets {
        let from_rel = format!("dm/{}.thread", stem);
        let to_rel = format!("archive/dm/{}.thread", stem);
        let archive_path = state.repo_root.join(&to_rel);
        let active_path = state.repo_root.join(&from_rel);

        // Idempotency: archive copy already exists OR active is gone.
        // Either way, this DM has been moved already; skip.
        if archive_path.exists() || !active_path.exists() {
            continue;
        }

        {
            let _commit_guard = state.commit_lock.lock().expect("commit_lock poisoned");

            if let Err(e) = state.git_storage.mv(&from_rel, &to_rel) {
                return Err(Response::error(format!(
                    "phase2: git mv {} failed: {}",
                    stem, e
                )));
            }
            let commit_msg = format!("archive: dm {}", stem);
            let (an, ae) = state.author_for(handler);
            if let Err(e) =
                state
                    .git_storage
                    .add_and_commit_as(&[&to_rel], &commit_msg, Some((&an, &ae)))
            {
                if let Err(rb) = state.git_storage.mv(&to_rel, &from_rel) {
                    warn!("phase2: rollback git mv {} also failed: {}", stem, rb);
                }
                return Err(Response::error(format!(
                    "phase2: commit failed for {}: {}",
                    stem, e
                )));
            }
            // commit_guard drops here.
        }

        push_with_retry(state, "phase2")?;
        commits += 1;
    }
    Ok(commits)
}

/// Phase 3 — for every `channels/<ch>.meta.yaml` whose `members` list
/// contains <handler>, rewrite the file removing the entry. One commit
/// per channel. Channels already missing <handler> are skipped.
async fn phase3_clean_channel_members(state: &SharedState, handler: &str) -> Result<u64, Response> {
    let channels_dir = state.repo_root.join("channels");
    let mut targets: Vec<String> = Vec::new(); // channel names
    if channels_dir.is_dir() {
        if let Ok(rd) = std::fs::read_dir(&channels_dir) {
            for entry in rd.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                let name = match fname.strip_suffix(".meta.yaml") {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                targets.push(name);
            }
        }
    }
    targets.sort();

    let mut commits: u64 = 0;
    for ch in &targets {
        let meta_rel = format!("channels/{}.meta.yaml", ch);
        let meta_path = state.repo_root.join(&meta_rel);

        // Read fresh per channel — phase3 is read+rewrite, no shared state
        // outside the lock.
        let meta_str = match std::fs::read_to_string(&meta_path) {
            Ok(s) => s,
            Err(e) => {
                warn!("phase3: failed to read {} (skipping): {}", meta_rel, e);
                continue;
            }
        };
        let mut meta: ChannelMeta = match serde_yaml::from_str(&meta_str) {
            Ok(m) => m,
            Err(e) => {
                warn!("phase3: failed to parse {} (skipping): {}", meta_rel, e);
                continue;
            }
        };
        if !meta.members.iter().any(|m| m == handler) {
            continue; // Idempotent skip — already removed (or never present).
        }

        {
            let _commit_guard = state.commit_lock.lock().expect("commit_lock poisoned");

            // Re-read under lock for the same reason as phase1.
            let cur_str = match std::fs::read_to_string(&meta_path) {
                Ok(s) => s,
                Err(e) => {
                    return Err(Response::error(format!(
                        "phase3: read {} under lock: {}",
                        meta_rel, e
                    )));
                }
            };
            let mut cur_meta: ChannelMeta = match serde_yaml::from_str(&cur_str) {
                Ok(m) => m,
                Err(e) => {
                    return Err(Response::error(format!(
                        "phase3: parse {} under lock: {}",
                        meta_rel, e
                    )));
                }
            };
            if !cur_meta.members.iter().any(|m| m == handler) {
                continue; // Another writer beat us to it.
            }
            cur_meta.members.retain(|m| m != handler);
            meta = cur_meta;

            let new_yaml = match serde_yaml::to_string(&meta) {
                Ok(s) => s,
                Err(e) => {
                    return Err(Response::error(format!(
                        "phase3: serialize {} failed: {}",
                        meta_rel, e
                    )));
                }
            };
            if let Err(e) = std::fs::write(&meta_path, &new_yaml) {
                return Err(Response::error(format!(
                    "phase3: write {} failed: {}",
                    meta_rel, e
                )));
            }

            let commit_msg = format!("channel: remove @{} from #{} members", handler, ch);
            let (an, ae) = state.author_for(handler);
            if let Err(e) =
                state
                    .git_storage
                    .add_and_commit_as(&[&meta_rel], &commit_msg, Some((&an, &ae)))
            {
                // Restore previous yaml so the working tree mirrors HEAD.
                if let Err(rb) = std::fs::write(&meta_path, cur_str.as_bytes()) {
                    warn!("phase3: rollback write {} also failed: {}", meta_rel, rb);
                }
                return Err(Response::error(format!(
                    "phase3: commit {} failed: {}",
                    meta_rel, e
                )));
            }
            // commit_guard drops here.
        }

        push_with_retry(state, "phase3")?;
        commits += 1;
    }
    Ok(commits)
}

/// Phase 4 — terminal step. `git mv users/<handler>.meta.yaml →
/// archive/users/<handler>.meta.yaml`. Single commit. Idempotent: if
/// the file is already in archive/users/, skips with `Ok(0)`.
///
/// After Phase 4 succeeds, the in-memory users list drops <handler>
/// and an `Event::UserArchived` SSE is emitted. This mirrors
/// `handle_archive_user`'s post-commit shape.
async fn phase4_archive_user(state: &SharedState, handler: &str) -> Result<u64, Response> {
    let from_rel = format!("users/{}.meta.yaml", handler);
    let to_rel = format!("archive/users/{}.meta.yaml", handler);
    let active_path = state.repo_root.join(&from_rel);
    let archive_path = state.repo_root.join(&to_rel);

    if archive_path.exists() {
        // Either a previous Phase 4 already finished, or another path put
        // the file there. Terminal state met — no work to do here.
        return Ok(0);
    }
    if !active_path.exists() {
        // Active gone, archive missing — broken state from a partially
        // failed previous run that this composite isn't designed to
        // repair. Surface clearly so the user can manually intervene.
        return Err(Response::error(format!(
            "phase4: users/{}.meta.yaml is missing and not in archive/ either",
            handler
        )));
    }

    let archive_dir = state.repo_root.join("archive/users");
    if let Err(e) = std::fs::create_dir_all(&archive_dir) {
        return Err(Response::error(format!(
            "phase4: failed to create archive/users dir: {}",
            e
        )));
    }

    {
        let _commit_guard = state.commit_lock.lock().expect("commit_lock poisoned");

        if let Err(e) = state.git_storage.mv(&from_rel, &to_rel) {
            return Err(Response::error(format!("phase4: git mv failed: {}", e)));
        }
        let commit_msg = format!("archive: depart user @{}", handler);
        let (an, ae) = state.author_for(handler);
        if let Err(e) =
            state
                .git_storage
                .add_and_commit_as(&[&to_rel], &commit_msg, Some((&an, &ae)))
        {
            if let Err(rb) = state.git_storage.mv(&to_rel, &from_rel) {
                warn!("phase4: rollback git mv also failed: {}", rb);
            }
            return Err(Response::error(format!(
                "phase4: commit failed: {}; rolled back git mv",
                e
            )));
        }
        // commit_guard drops here.
    }

    push_with_retry(state, "phase4")?;

    // In-memory users list update — mirror handle_archive_user. The
    // post-sync refresh in state.rs will redo this from disk eventually,
    // but we want list_users to be immediately consistent.
    {
        let mut users = state.users.write().await;
        users.retain(|u| u != handler);
    }

    // SSE so subscribers (WebUI / runtime) can react without waiting on
    // the next sync cycle. archived_by = handler matches the leave-channel
    // semantic (the agent self-departs).
    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let _ = state.event_tx.send(Event::UserArchived {
        handler: handler.to_string(),
        archived_by: handler.to_string(),
        timestamp,
    });

    Ok(1)
}

/// Push-with-retry helper used by every depart_user phase. Same shape as
/// the inline retry blocks in handle_archive_user / handle_archive_dm —
/// extracted because we need it in 4 places. Skips when there is no
/// remote.
fn push_with_retry(state: &SharedState, phase: &str) -> Result<(), Response> {
    if !state.git_storage.has_remote() {
        return Ok(());
    }
    for attempt in 1..=MAX_PUSH_RETRIES {
        match state.git_storage.push() {
            Ok(()) => return Ok(()),
            Err(GitError::PushConflict) => {
                warn!(
                    "{}: push conflict (attempt {}/{}), rebasing",
                    phase, attempt, MAX_PUSH_RETRIES
                );
                if let Err(e) = state.git_storage.fetch() {
                    return Err(Response::error(format!("{}: fetch failed: {}", phase, e)));
                }
                if let Err(e) = state.git_storage.rebase_onto_origin() {
                    return Err(Response::error(format!("{}: rebase failed: {}", phase, e)));
                }
            }
            Err(e) => {
                return Err(Response::error(format!("{}: push failed: {}", phase, e)));
            }
        }
    }
    Err(Response::error(format!(
        "{}: push still conflicting after {} retries",
        phase, MAX_PUSH_RETRIES
    )))
}
