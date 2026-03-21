use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{broadcast, RwLock};
use gitim_core::types::{Config, ThreadFile};
use gitim_sync::git::GitStorage;
use crate::api::Event;

pub type SharedState = Arc<AppState>;

#[derive(Clone, Debug)]
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
    pub pending_push: std::sync::RwLock<Vec<PendingMessage>>,
    pub sync_started: AtomicBool,
}

impl AppState {
    pub fn new(repo_root: PathBuf, config: Config, event_tx: broadcast::Sender<Event>, current_user: Option<String>) -> Self {
        let git_storage = GitStorage::new(&repo_root);
        Self {
            repo_root,
            config,
            git_storage,
            thread_cache: RwLock::new(HashMap::new()),
            users: RwLock::new(Vec::new()),
            event_tx,
            current_user: RwLock::new(current_user),
            pending_push: std::sync::RwLock::new(Vec::new()),
            sync_started: AtomicBool::new(false),
        }
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
            )
            .await;
        });

        tracing::info!("sync loop started");
    }
}
