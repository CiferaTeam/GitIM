use crate::api::Event;
use gitim_core::types::{Config, ThreadFile};
use gitim_sync::git::GitStorage;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use tokio::sync::{broadcast, Notify, RwLock};

pub type SharedState = Arc<AppState>;

/// In-flight tracker for a locally-committed message that the sync loop
/// has not yet pushed to the remote.
///
/// `send` enqueues an entry after a successful local commit and returns
/// to the caller immediately. The sync loop drains entries on push
/// success (emitting `Event::MessagesPushed`) and rewrites `line_number`
/// when rebase renumbers the message.
///
/// There is no per-entry result channel: push outcome is observable via
/// `Event::MessagesPushed` (success) and sync_loop log + `auth_failed`
/// circuit breaker (failure). Callers do not block on push.
#[derive(Debug)]
pub struct PendingMessage {
    pub channel: String,
    pub line_number: u64,
}

pub struct AppState {
    pub repo_root: PathBuf,
    pub config: Config,
    pub git_storage: GitStorage,
    pub thread_cache: RwLock<HashMap<String, ThreadFile>>,
    pub users: RwLock<Vec<String>>,
    pub event_tx: broadcast::Sender<Event>,
    pub current_user: RwLock<Option<String>>,
    /// Optional verified email read from `.gitim/me.json` → `github_email`.
    /// When present, daemon-created commits use this as `author.email` so
    /// they attribute to the GitHub account on the contribution graph.
    /// Absent → fallback to `<handler>@gitim` (legacy behavior).
    ///
    /// Wrapped in `std::sync::RwLock` so onboard can set it after daemon
    /// startup (me.json is written *during* onboard, not before) and
    /// handler paths can read without needing async context.
    pub github_email: std::sync::RwLock<Option<String>>,
    pub pending_push: std::sync::RwLock<Vec<PendingMessage>>,
    pub push_notify: Arc<Notify>,
    pub has_remote: bool,
    pub sync_started: AtomicBool,
    /// Latched by `spawn_cron_engine` so the engine task is started at most
    /// once per daemon lifetime. Mirrors `sync_started` — both are CAS-gated
    /// because the spawn point is reached from both `main` (restart with
    /// existing identity) and `handle_onboard` (first-time identity setup),
    /// and a second spawn would double the fire-rate.
    pub cron_engine_started: AtomicBool,
    pub is_admin: AtomicBool,
    pub is_guest: AtomicBool,
    pub index: std::sync::RwLock<Option<Arc<gitim_index::Index>>>,
    /// Parsed `gitim.epoch.yaml` for this repo, refreshed on daemon boot and
    /// after every sync cycle. `None` covers both "file does not exist"
    /// (legacy repos predating snapshot pack — treated as Active) and
    /// "daemon hasn't refreshed yet".
    ///
    /// Wrapped in `std::sync::RwLock` to match `github_email` / `index`
    /// pattern: readers don't need an async context, writers are the boot
    /// path + sync_loop's `on_synced` callback (both sync code).
    ///
    /// Phase A is read-only awareness — Subtask C will consume `is_redirected`
    /// to gate write paths; Subtask D will expose `epoch_status_snapshot`
    /// through the status API.
    pub epoch_status: std::sync::RwLock<Option<gitim_core::epoch::EpochFile>>,
    /// Epoch seconds of last client connection. Used by idle watchdog.
    pub last_client_activity: AtomicU64,
    /// Tripped by sync_loop after 3 consecutive auth failures. Readers can
    /// check this to surface "PAT expired" to the UI (e.g. `Status` exposes it
    /// as `auth_circuit_open`). Self-heals: once tripped, the circuit half-opens
    /// and retries one probe per interval, clearing the flag on the first
    /// successful remote op — no daemon restart required.
    pub auth_failed: Arc<AtomicBool>,
    /// **Commit-tree invariant**: any in-process operation that mutates the
    /// local commit tree MUST hold this lock for the duration of that
    /// mutation. That covers:
    ///   - handler write paths (read thread → append → `git commit`)
    ///   - sync_loop's `git rebase` onto origin
    ///   - conflict resolution (write merged files + commit)
    ///
    /// It does NOT cover the network-only ops (`git fetch`, `git push`) —
    /// those don't touch the commit tree, and holding the lock through a slow
    /// network round-trip would let a single fetch stall every writer.
    ///
    /// Shared as `Arc` so the sync_loop spawn can cheaply clone-and-own a
    /// handle. `std::sync::Mutex` is deliberate: every critical section is
    /// blocking I/O (fs + `git` subprocess), so there is no await point for
    /// the guard to cross, and a tokio Mutex would force sync_loop —
    /// currently a plain `fn` — into async plumbing for no gain.
    pub commit_lock: Arc<StdMutex<()>>,
}

impl AppState {
    pub fn new(
        repo_root: PathBuf,
        config: Config,
        event_tx: broadcast::Sender<Event>,
        current_user: Option<String>,
    ) -> Self {
        Self::new_with_email(repo_root, config, event_tx, current_user, None)
    }

    pub fn new_with_email(
        repo_root: PathBuf,
        config: Config,
        event_tx: broadcast::Sender<Event>,
        current_user: Option<String>,
        github_email: Option<String>,
    ) -> Self {
        let git_storage = GitStorage::new(&repo_root);
        let has_remote = git_storage.has_remote();
        Self {
            repo_root,
            config,
            git_storage,
            thread_cache: RwLock::new(HashMap::new()),
            users: RwLock::new(Vec::new()),
            event_tx,
            current_user: RwLock::new(current_user),
            github_email: std::sync::RwLock::new(github_email),
            pending_push: std::sync::RwLock::new(Vec::new()),
            push_notify: Arc::new(Notify::new()),
            has_remote,
            sync_started: AtomicBool::new(false),
            cron_engine_started: AtomicBool::new(false),
            is_admin: AtomicBool::new(false),
            is_guest: AtomicBool::new(false),
            index: std::sync::RwLock::new(None),
            epoch_status: std::sync::RwLock::new(None),
            last_client_activity: AtomicU64::new(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_else(|e| {
                        tracing::error!("system time before epoch: {e}");
                        Default::default()
                    })
                    .as_secs(),
            ),
            auth_failed: Arc::new(AtomicBool::new(false)),
            commit_lock: Arc::new(StdMutex::new(())),
        }
    }

    /// Build the `(name, email)` pair used as commit `author` when this
    /// daemon writes on behalf of `handler`. Email comes from
    /// `github_email` when set, otherwise the legacy `<handler>@gitim`
    /// fallback so existing workspaces keep working unchanged.
    ///
    /// Read is a single `Option<String>` clone; never held across
    /// await, safe from any handler context.
    pub fn author_for(&self, handler: &str) -> (String, String) {
        let email = self
            .github_email
            .read()
            .ok()
            .and_then(|g| g.clone())
            .unwrap_or_else(|| format!("{}@gitim", handler));
        (handler.to_string(), email)
    }

    /// Read `<repo_root>/gitim.epoch.yaml` and store the result in
    /// `self.epoch_status`. Called once at daemon boot and once per
    /// successful sync cycle.
    ///
    /// File-not-found is a normal state (legacy repos and freshly cloned
    /// pre-pack workspaces both have no epoch file) — the lock is cleared
    /// to `None` and `Ok(())` returned. Parse / validate errors propagate
    /// as `Err(String)` so the caller can log without us deciding the
    /// daemon's tolerance for a malformed file.
    pub fn refresh_epoch_status(&self) -> Result<(), String> {
        let path = self.repo_root.join("gitim.epoch.yaml");
        // `load_from_path` returns `Ok(None)` for the missing-file case
        // (legacy repos, freshly-cloned pre-pack workspaces) — only true
        // parse / validate / non-NotFound IO failures surface as `Err`.
        let parsed = gitim_core::epoch::EpochFile::load_from_path(&path)
            .map_err(|e| format!("failed to load {}: {}", path.display(), e))?;
        let mut guard = self
            .epoch_status
            .write()
            .map_err(|e| format!("epoch_status lock poisoned: {}", e))?;
        *guard = parsed;
        Ok(())
    }

    /// True iff the last refresh observed a redirected epoch file. `None`
    /// (no file / not yet refreshed) and Active both return false — Phase A
    /// only treats explicit `status: redirected` as a write-block signal.
    pub fn is_redirected(&self) -> bool {
        match self.epoch_status.read() {
            Ok(g) => matches!(
                g.as_ref(),
                Some(file) if file.status == gitim_core::epoch::EpochStatus::Redirected
            ),
            // Poisoned lock means a previous writer panicked mid-refresh.
            // Safer to report "not redirected" than to claim redirected on a
            // corrupted state and stall writes — Subtask C's gate will hit
            // the same branch on its own read.
            Err(_) => false,
        }
    }

    /// Snapshot the current epoch state for status-API consumers (Subtask D).
    /// Cloning is cheap (`EpochFile` is plain data) and lets the caller hold
    /// the value across an await without touching the lock again.
    pub fn epoch_status_snapshot(&self) -> Option<gitim_core::epoch::EpochFile> {
        self.epoch_status.read().ok().and_then(|g| g.clone())
    }

    /// Open (or rebuild) the search index at `.gitim/index.db`.
    /// Compares stored commit with HEAD; does incremental update or full rebuild as needed.
    /// Returns immediately without touching SQLite or state.index when `enabled` is false.
    pub fn initialize_index(state: &SharedState) -> Result<(), String> {
        if !state.config.indexer.enabled {
            tracing::info!("indexer disabled by config");
            return Ok(());
        }

        let db_path = state.repo_root.join(".gitim").join("index.db");
        let index = gitim_index::Index::open(&db_path)
            .map_err(|e| format!("failed to open index: {}", e))?;

        let current_head = state
            .git_storage
            .rev_parse("HEAD")
            .map_err(|e| format!("failed to get HEAD: {}", e))?;

        let stored_commit = index
            .get_commit_id()
            .map_err(|e| format!("failed to get stored commit: {}", e))?;

        match stored_commit {
            Some(ref stored) if stored == &current_head => {
                tracing::info!("index up to date at {}", &current_head[..8]);
            }
            Some(ref stored) if is_ancestor(stored, &current_head, &state.repo_root) => {
                tracing::info!(
                    "index incremental update {}..{}",
                    &stored[..8],
                    &current_head[..8]
                );
                let diff = state
                    .git_storage
                    .diff_range(stored, &current_head)
                    .map_err(|e| format!("diff_range failed: {}", e))?;
                let diff_strings: HashMap<String, String> = diff
                    .into_iter()
                    .map(|(k, v)| (k.to_string_lossy().to_string(), v))
                    .collect();
                let count = index
                    .append_from_diff(&diff_strings, &current_head)
                    .map_err(|e| format!("append_from_diff failed: {}", e))?;
                tracing::info!("index updated: {} messages added", count);
            }
            _ => {
                tracing::info!("index full rebuild for {}", &current_head[..8]);
                let count = index
                    .rebuild(&state.repo_root, &current_head)
                    .map_err(|e| format!("rebuild failed: {}", e))?;
                tracing::info!("index rebuilt: {} messages indexed", count);
            }
        }

        let arc_index = Arc::new(index);
        *state.index.write().unwrap_or_else(|e| e.into_inner()) = Some(arc_index);
        Ok(())
    }

    /// Spawn the sync loop for this state.  Safe to call from both main (on
    /// restart) and from handle_onboard (after first-time identity setup).
    /// The AtomicBool ensures the loop is only ever started once.
    ///
    /// A supervisor task watches the worker: if the worker panics or is
    /// cancelled, the daemon exits so runtime can restart a fresh instance.
    /// Normal early return (auto-sync disabled / no remote) clears
    /// `sync_started` without exiting.
    pub fn spawn_sync_loop(state: SharedState) {
        // CAS: only the first caller proceeds; all others return immediately.
        if state
            .sync_started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            tracing::warn!("spawn_sync_loop called but sync loop already running — ignoring");
            return;
        }

        let sync_interval = state.config.daemon.sync_interval;
        let sync_root = state.repo_root.clone();
        let push_notify = state.push_notify.clone();
        let auth_failed = state.auth_failed.clone();
        let commit_lock = state.commit_lock.clone();
        let push_state = state.clone();
        let renum_state = state.clone();
        let synced_state = state.clone();

        // Snapshot (handler, email) for rebase-resolution commits. Each
        // daemon only ever writes commits on behalf of its owner, so the
        // snapshot is stable for the lifetime of this sync loop. Guest /
        // unauthenticated → None → legacy git-config fallback.
        let rebase_author_state = state.clone();

        let supervisor_state = state.clone();
        let worker = tokio::spawn(async move {
            let rebase_author = {
                let current = rebase_author_state.current_user.read().await.clone();
                current.map(|u| rebase_author_state.author_for(&u))
            };
            gitim_sync::sync_loop::start_sync_loop(
                &sync_root,
                sync_interval,
                push_notify,
                auth_failed,
                commit_lock,
                move || {
                    // on_pushed: drain pending_push and broadcast
                    // MessagesPushed events grouped by channel. Push result
                    // is no longer reported back to the request handler —
                    // SSE consumers (WebUI, runtime) get the event instead.
                    let mut pending = push_state
                        .pending_push
                        .write()
                        .unwrap_or_else(|e| e.into_inner());
                    let mut by_channel: std::collections::HashMap<String, Vec<u64>> =
                        std::collections::HashMap::new();
                    for msg in pending.drain(..) {
                        by_channel
                            .entry(msg.channel)
                            .or_default()
                            .push(msg.line_number);
                    }
                    drop(pending);
                    for (channel, line_numbers) in by_channel {
                        let _ = push_state.event_tx.send(Event::MessagesPushed {
                            channel,
                            line_numbers,
                        });
                    }
                },
                move |file, old_line, new_line| {
                    // on_renumbered: broadcast MessageRenumbered and update pending_push
                    // Extract channel name from file path (e.g. "channels/general.thread" -> "general")
                    let channel_name = file
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_string();
                    let mut pending = renum_state
                        .pending_push
                        .write()
                        .unwrap_or_else(|e| e.into_inner());
                    for msg in pending.iter_mut() {
                        if msg.channel == channel_name && msg.line_number == old_line {
                            let _ = renum_state.event_tx.send(Event::MessageRenumbered {
                                channel: msg.channel.clone(),
                                old_line,
                                new_line,
                            });
                            msg.line_number = new_line;
                        }
                    }
                },
                move |head_commit| {
                    // on_synced: refresh users list from disk
                    let users_dir = synced_state.repo_root.join("users");
                    if let Ok(entries) = std::fs::read_dir(&users_dir) {
                        let mut fresh: Vec<String> = entries
                            .flatten()
                            .filter_map(|e| {
                                let name = e.file_name().to_string_lossy().to_string();
                                name.strip_suffix(".meta.yaml").map(|h| h.to_string())
                            })
                            .collect();
                        fresh.sort();
                        if let Ok(mut users) = synced_state.users.try_write() {
                            if *users != fresh {
                                tracing::info!(
                                    "on_synced: users list refreshed ({} users)",
                                    fresh.len()
                                );
                                *users = fresh;
                            }
                        }
                    }

                    // Re-read gitim.epoch.yaml after every successful sync.
                    // A remote-published redirect (Phase B's coordinator
                    // writes this) becomes visible to this daemon on the
                    // next cycle; Subtask C's write gate consumes the
                    // updated state. Done before the index block because
                    // that block has several early-return paths (disabled
                    // indexer, no diff to apply, etc.) and the refresh
                    // must happen on every cycle regardless.
                    if let Err(e) = synced_state.refresh_epoch_status() {
                        tracing::warn!("on_synced: epoch status refresh failed: {}", e);
                    }

                    // update index after each sync cycle
                    let index_guard = synced_state.index.read().unwrap_or_else(|e| e.into_inner());
                    let index = match &*index_guard {
                        Some(idx) => idx.clone(),
                        None => return,
                    };
                    drop(index_guard);

                    let stored = match index.get_commit_id() {
                        Ok(Some(s)) if s == head_commit => return, // already up to date
                        Ok(Some(s)) => Some(s),
                        Ok(None) => None,
                        Err(e) => {
                            tracing::warn!("on_synced: failed to get stored commit: {}", e);
                            return;
                        }
                    };

                    match stored {
                        Some(ref s) if is_ancestor(s, &head_commit, &synced_state.repo_root) => {
                            let diff = match synced_state.git_storage.diff_range(s, &head_commit) {
                                Ok(d) => d,
                                Err(e) => {
                                    tracing::warn!("on_synced: diff_range failed: {}", e);
                                    return;
                                }
                            };
                            let diff_strings: HashMap<String, String> = diff
                                .into_iter()
                                .map(|(k, v)| (k.to_string_lossy().to_string(), v))
                                .collect();
                            match index.append_from_diff(&diff_strings, &head_commit) {
                                Ok(n) => {
                                    tracing::info!("on_synced: index updated, {} messages added", n)
                                }
                                Err(e) => {
                                    tracing::warn!("on_synced: append_from_diff failed: {}", e)
                                }
                            }
                        }
                        _ => {
                            // No stored commit or not ancestor — full rebuild
                            match index.rebuild(&synced_state.repo_root, &head_commit) {
                                Ok(n) => tracing::info!(
                                    "on_synced: index rebuilt, {} messages indexed",
                                    n
                                ),
                                Err(e) => tracing::warn!("on_synced: rebuild failed: {}", e),
                            }
                        }
                    }
                },
                || {
                    // on_cycle_done: no per-request waiters to notify. Kept
                    // as a no-op so the sync_loop callback signature stays
                    // stable for any future per-cycle hook (metrics, etc.).
                },
                rebase_author,
            )
            .await;
        });

        tokio::spawn(async move {
            match worker.await {
                Ok(()) => {
                    supervisor_state.sync_started.store(false, Ordering::SeqCst);
                    tracing::info!(
                        "sync loop task finished (auto-sync disabled or no remote configured)"
                    );
                }
                Err(join_error) => {
                    supervisor_state.sync_started.store(false, Ordering::SeqCst);
                    if join_error.is_panic() {
                        tracing::error!("sync loop task panicked: {join_error}");
                    } else if join_error.is_cancelled() {
                        tracing::error!("sync loop task cancelled: {join_error}");
                    } else {
                        tracing::error!("sync loop task failed: {join_error}");
                    }
                    tracing::error!("daemon shutting down due to sync loop failure");
                    std::process::exit(1);
                }
            }
        });

        tracing::info!("sync loop started");
    }

    /// Spawn the cron engine task for this state. Mirrors `spawn_sync_loop`:
    /// CAS-gated, called from both `main` (restart with existing identity)
    /// and `handle_onboard` (first-time setup) so identity-deferred startup
    /// works the same way for both subsystems.
    ///
    /// The task itself runs a 60-second tokio interval. Each tick:
    ///   1. `scan_due(crons_dir, &self_handler, now)` — pure compute over
    ///      the on-disk specs. Errors are logged + tick continues.
    ///   2. For each `FireRequest`: `cron_engine::fire(&state, req).await`.
    ///      Per-fire errors are logged via `tracing::warn!`; one bad spec
    ///      must NOT stall the rest of the tick.
    ///
    /// `self_handler` is read once at task start and cached — handler is
    /// immutable for the lifetime of a daemon (changing identity requires
    /// restart, see CLAUDE.md "Handler 冲突防护"). Caching avoids a per-tick
    /// `.read().await` on the `current_user` RwLock and keeps the loop body
    /// fully sync apart from `fire`'s `.await`.
    ///
    /// ### First-tick startup throttle
    ///
    /// First tick waits one full interval before scanning. Why: many
    /// workspaces have specs whose `last_fire` is older than `now`, so a
    /// fresh-startup scan would emit a burst of "due" fires for everything
    /// that should have happened while the daemon was offline. Even though
    /// idempotency protects against duplication on the same machine, the
    /// burst spams the contribution graph and overflows agent context. One
    /// missed cycle on startup is acceptable per design.md "no catch-up".
    ///
    /// ### Shutdown
    ///
    /// No explicit cancellation token. The task dies with the tokio
    /// runtime when `main` exits via `std::process::exit`. Same model
    /// `spawn_sync_loop` uses — graceful shutdown of long-running tasks
    /// isn't part of the daemon's contract.
    pub fn spawn_cron_engine(state: SharedState) {
        if state
            .cron_engine_started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            tracing::warn!("spawn_cron_engine called but engine already running — ignoring");
            return;
        }

        tokio::spawn(async move {
            // Read identity once at task start. If it's None (shouldn't
            // happen in production — caller gates on identity, same as
            // sync_loop), bail out cleanly so the daemon doesn't get a
            // dead engine that fires nothing forever.
            let self_handler_str = match state.current_user.read().await.clone() {
                Some(h) => h,
                None => {
                    tracing::warn!("cron_engine: no current_user — engine task exiting");
                    return;
                }
            };
            let self_handler = match gitim_core::types::Handler::new(&self_handler_str) {
                Ok(h) => h,
                Err(e) => {
                    tracing::warn!(
                        "cron_engine: current_user '{}' is not a valid handler: {} — engine task exiting",
                        self_handler_str,
                        e
                    );
                    return;
                }
            };

            let crons_dir = state.repo_root.join("crons");
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            // First tick fires immediately by default — burn it so the
            // startup throttle is honored. The next tick comes 60s later.
            interval.tick().await;

            tracing::info!("cron engine started for @{}", self_handler.as_str());

            loop {
                interval.tick().await;
                let now = chrono::Utc::now();
                let requests = match crate::cron_engine::scan_due(&crons_dir, &self_handler, now) {
                    Ok(reqs) => reqs,
                    Err(e) => {
                        tracing::warn!("cron_engine: scan_due failed: {} — skipping tick", e);
                        continue;
                    }
                };
                for req in requests {
                    let spec_name = req.spec_name.clone();
                    if let Err(e) = crate::cron_engine::fire(&state, req).await {
                        tracing::warn!("cron_engine: fire for spec '{}' failed: {}", spec_name, e);
                    }
                }
            }
        });
    }
}

/// Check if `ancestor` is an ancestor of `descendant` in the git repo at `repo_root`.
fn is_ancestor(ancestor: &str, descendant: &str, repo_root: &PathBuf) -> bool {
    std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .current_dir(repo_root)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gitim_core::types::Config;

    fn make_state(github_email: Option<String>) -> AppState {
        let tmp = tempfile::tempdir().unwrap();
        let (tx, _) = broadcast::channel(16);
        AppState::new_with_email(
            tmp.path().to_path_buf(),
            Config::default(),
            tx,
            None,
            github_email,
        )
    }

    #[test]
    fn author_for_uses_github_email_when_configured() {
        let state = make_state(Some("flame0743@gmail.com".to_string()));
        let (name, email) = state.author_for("framer-gpt");
        assert_eq!(name, "framer-gpt");
        assert_eq!(email, "flame0743@gmail.com");
    }

    #[test]
    fn author_for_falls_back_to_gitim_domain_when_no_email() {
        let state = make_state(None);
        let (name, email) = state.author_for("framer-gpt");
        assert_eq!(name, "framer-gpt");
        assert_eq!(email, "framer-gpt@gitim");
    }

    #[test]
    fn author_for_reflects_runtime_update() {
        // Simulates the onboard flow: daemon starts with no email, then
        // handle_onboard writes github_email into AppState.
        let state = make_state(None);
        assert_eq!(state.author_for("alice").1, "alice@gitim");

        *state
            .github_email
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Some("alice@example.com".to_string());
        assert_eq!(state.author_for("alice").1, "alice@example.com");
    }
}
