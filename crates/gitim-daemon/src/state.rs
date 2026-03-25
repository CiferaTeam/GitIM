use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{broadcast, Notify, RwLock};
use gitim_core::types::{Config, ThreadFile};
use gitim_sync::git::GitStorage;
use crate::api::Event;

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
    pub pending_push: std::sync::RwLock<Vec<PendingMessage>>,
    pub push_notify: Arc<Notify>,
    pub has_remote: bool,
    pub sync_started: AtomicBool,
    pub index: std::sync::RwLock<Option<Arc<gitim_index::Index>>>,
}

impl AppState {
    pub fn new(repo_root: PathBuf, config: Config, event_tx: broadcast::Sender<Event>, current_user: Option<String>) -> Self {
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
            pending_push: std::sync::RwLock::new(Vec::new()),
            push_notify: Arc::new(Notify::new()),
            has_remote,
            sync_started: AtomicBool::new(false),
            index: std::sync::RwLock::new(None),
        }
    }

    /// Open (or rebuild) the search index at `.gitim/index.db`.
    /// Compares stored commit with HEAD; does incremental update or full rebuild as needed.
    pub fn initialize_index(state: &SharedState) -> Result<(), String> {
        let db_path = state.repo_root.join(".gitim").join("index.db");
        let index = gitim_index::Index::open(&db_path)
            .map_err(|e| format!("failed to open index: {}", e))?;

        let current_head = state.git_storage.rev_parse("HEAD")
            .map_err(|e| format!("failed to get HEAD: {}", e))?;

        let stored_commit = index.get_commit_id()
            .map_err(|e| format!("failed to get stored commit: {}", e))?;

        match stored_commit {
            Some(ref stored) if stored == &current_head => {
                tracing::info!("index up to date at {}", &current_head[..8]);
            }
            Some(ref stored) if is_ancestor(stored, &current_head, &state.repo_root) => {
                tracing::info!("index incremental update {}..{}", &stored[..8], &current_head[..8]);
                let diff = state.git_storage.diff_range(stored, &current_head)
                    .map_err(|e| format!("diff_range failed: {}", e))?;
                let diff_strings: HashMap<String, String> = diff
                    .into_iter()
                    .map(|(k, v)| (k.to_string_lossy().to_string(), v))
                    .collect();
                let count = index.append_from_diff(&diff_strings, &current_head)
                    .map_err(|e| format!("append_from_diff failed: {}", e))?;
                tracing::info!("index updated: {} messages added", count);
            }
            _ => {
                tracing::info!("index full rebuild for {}", &current_head[..8]);
                let count = index.rebuild(&state.repo_root, &current_head)
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
        if state.sync_started.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            tracing::warn!("spawn_sync_loop called but sync loop already running — ignoring");
            return;
        }

        let sync_interval = state.config.daemon.sync_interval;
        let sync_root = state.repo_root.clone();
        let push_state = state.clone();
        let renum_state = state.clone();
        let synced_state = state.clone();

        tokio::spawn(async move {
            gitim_sync::sync_loop::start_sync_loop(
                &sync_root,
                sync_interval,
                move || {
                    // on_pushed: clear pending_push and broadcast MessagesPushed events
                    let mut pending = push_state.pending_push.write().unwrap();
                    let mut by_channel: std::collections::HashMap<String, Vec<u64>> =
                        std::collections::HashMap::new();
                    for msg in pending.drain(..) {
                        by_channel.entry(msg.channel).or_default().push(msg.line_number);
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
                    // on_synced: update index after each sync cycle
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
                                Ok(n) => tracing::info!("on_synced: index updated, {} messages added", n),
                                Err(e) => tracing::warn!("on_synced: append_from_diff failed: {}", e),
                            }
                        }
                        _ => {
                            // No stored commit or not ancestor — full rebuild
                            match index.rebuild(&synced_state.repo_root, &head_commit) {
                                Ok(n) => tracing::info!("on_synced: index rebuilt, {} messages indexed", n),
                                Err(e) => tracing::warn!("on_synced: rebuild failed: {}", e),
                            }
                        }
                    }
                },
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
