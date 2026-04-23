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

#[derive(Debug)]
pub enum PushResult {
    Pushed { commit_id: String },
    Failed { reason: String },
}

#[derive(Debug)]
pub struct PendingMessage {
    pub channel: String,
    pub line_number: u64,
    pub result_tx: Option<tokio::sync::oneshot::Sender<PushResult>>,
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
    pub is_admin: AtomicBool,
    pub is_guest: AtomicBool,
    pub index: std::sync::RwLock<Option<Arc<gitim_index::Index>>>,
    /// Epoch seconds of last client connection. Used by idle watchdog.
    pub last_client_activity: AtomicU64,
    /// Latched by sync_loop when 3 consecutive auth failures trip the circuit.
    /// Readers can check this to surface "PAT expired" to the UI; the flag stays
    /// set until daemon restart (v1).
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
            is_admin: AtomicBool::new(false),
            is_guest: AtomicBool::new(false),
            index: std::sync::RwLock::new(None),
            last_client_activity: AtomicU64::new(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
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

    /// Open (or rebuild) the search index at `.gitim/index.db`.
    /// Compares stored commit with HEAD; does incremental update or full rebuild as needed.
    pub fn initialize_index(state: &SharedState) -> Result<(), String> {
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
        *state.index.write().unwrap() = Some(arc_index);
        Ok(())
    }

    /// Spawn the sync loop for this state.  Safe to call from both main (on
    /// restart) and from handle_onboard (after first-time identity setup).
    /// The AtomicBool ensures the loop is only ever started once.
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
        let cycle_done_state = state.clone();

        // Snapshot (handler, email) for rebase-resolution commits. Each
        // daemon only ever writes commits on behalf of its owner, so the
        // snapshot is stable for the lifetime of this sync loop. Guest /
        // unauthenticated → None → legacy git-config fallback.
        let rebase_author_state = state.clone();

        tokio::spawn(async move {
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
                    // on_pushed: get commit_id, send PushResult::Pushed to waiters,
                    // clear pending_push and broadcast MessagesPushed events
                    let commit_id = push_state
                        .git_storage
                        .rev_parse("HEAD")
                        .unwrap_or_else(|e| {
                            tracing::warn!("on_pushed: failed to get HEAD: {}", e);
                            "unknown".to_string()
                        });
                    let mut pending = push_state.pending_push.write().unwrap();
                    let mut by_channel: std::collections::HashMap<String, Vec<u64>> =
                        std::collections::HashMap::new();
                    for mut msg in pending.drain(..) {
                        if let Some(tx) = msg.result_tx.take() {
                            let _ = tx.send(PushResult::Pushed {
                                commit_id: commit_id.clone(),
                            });
                        }
                        by_channel
                            .entry(msg.channel)
                            .or_default()
                            .push(msg.line_number);
                    }
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
                    let mut pending = renum_state.pending_push.write().unwrap();
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

                    // update index after each sync cycle
                    let index_guard = synced_state.index.read().unwrap();
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
                move || {
                    // on_cycle_done: notify remaining waiters (with result_tx) that push failed
                    let mut pending = cycle_done_state.pending_push.write().unwrap();
                    pending.retain_mut(|msg| {
                        if msg.result_tx.is_some() {
                            if let Some(tx) = msg.result_tx.take() {
                                let _ = tx.send(PushResult::Failed {
                                    reason: "push cycle completed without success".to_string(),
                                });
                            }
                            false // remove entries that had waiters
                        } else {
                            true // keep entries without waiters (from sync_loop's own tracking)
                        }
                    });
                },
                rebase_author,
            )
            .await;
        });

        tracing::info!("sync loop started");
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

        *state.github_email.write().unwrap() = Some("alice@example.com".to_string());
        assert_eq!(state.author_for("alice").1, "alice@example.com");
    }
}
