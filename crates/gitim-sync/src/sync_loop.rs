use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time;
use tracing::{info, warn};

use crate::conflict::{self, build_rebase_commit_msg};
use crate::git::GitStorage;

/// Start the sync loop with push-first strategy.
///
/// - `on_pushed`: called after a successful push (all pending messages are now remote)
/// - `on_renumbered`: called for each message that was renumbered during conflict resolution
///   (file, old_line, new_line)
/// - `on_synced`: called after every sync cycle completes, with the current HEAD commit hash.
///   The index layer uses this to decide whether incremental updates are needed.
/// - `on_cycle_done`: called at the very end of every cycle, regardless of success or failure.
///   Used to notify remaining waiters that the push did not succeed.
pub async fn start_sync_loop<F1, F2, F3, F4>(
    repo_root: &Path,
    interval_secs: u32,
    push_notify: Arc<Notify>,
    on_pushed: F1,
    on_renumbered: F2,
    on_synced: F3,
    on_cycle_done: F4,
) where
    F1: Fn() + Send + 'static,
    F2: Fn(PathBuf, u64, u64) + Send + 'static,
    F3: Fn(String) + Send + 'static,
    F4: Fn() + Send + 'static,
{
    if interval_secs == 0 {
        info!("sync_interval=0, auto-sync disabled");
        return;
    }

    let repo = GitStorage::new(repo_root);

    if !repo.has_remote() {
        info!("no remote configured, sync loop disabled");
        return;
    }

    let interval = Duration::from_secs(interval_secs as u64);
    info!("sync loop started, interval={}s", interval_secs);

    let mut ticker = time::interval(interval);
    ticker.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    ticker.tick().await; // skip first immediate tick

    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            _ = push_notify.notified() => {}
        }
        run_sync_cycle(&repo, &on_pushed, &on_renumbered, &on_synced, &on_cycle_done);
    }
}

/// Execute one sync cycle. Completely self-contained — never panics, always logs.
fn run_sync_cycle<F1, F2, F3, F4>(repo: &GitStorage, on_pushed: &F1, on_renumbered: &F2, on_synced: &F3, on_cycle_done: &F4)
where
    F1: Fn(),
    F2: Fn(PathBuf, u64, u64),
    F3: Fn(String),
    F4: Fn(),
{
    let has_unpushed = match repo.has_unpushed_commits() {
        Ok(v) => v,
        Err(e) => {
            warn!("sync: failed to check unpushed commits: {}", e);
            on_cycle_done();
            return;
        }
    };

    if has_unpushed {
        sync_with_push(repo, on_pushed, on_renumbered);
    } else {
        // Nothing local to push, just pull
        match repo.pull_rebase() {
            Ok(()) => info!("sync: pull complete"),
            Err(e) => {
                warn!("sync: pull failed: {}", e);
                let _ = repo.discard_unpushed();
            }
        }
    }

    match repo.rev_parse("HEAD") {
        Ok(head) => on_synced(head),
        Err(e) => warn!("sync: failed to get HEAD for on_synced: {}", e),
    }

    on_cycle_done();
}

/// Push-first strategy: try push, fallback to fetch+rebase, then conflict resolution.
/// Retries up to 3 times if push fails after conflict resolution.
const MAX_SYNC_RETRIES: usize = 3;

fn sync_with_push<F1, F2>(repo: &GitStorage, on_pushed: &F1, on_renumbered: &F2)
where
    F1: Fn(),
    F2: Fn(PathBuf, u64, u64),
{
    for attempt in 1..=MAX_SYNC_RETRIES {
        // Try push directly
        match repo.push() {
            Ok(()) => {
                on_pushed();
                info!("sync: push complete (attempt {})", attempt);
                return;
            }
            Err(crate::git::GitError::PushConflict) => {
                // Remote has diverged, need to sync
            }
            Err(e) => {
                warn!("sync: push failed (non-conflict): {}", e);
                return;
            }
        }

        // Fetch remote changes
        if let Err(e) = repo.fetch() {
            warn!("sync: fetch failed: {}", e);
            return;
        }

        // Capture local additions BEFORE attempting rebase
        let local_additions = match repo.diff_unpushed("*.thread") {
            Ok(v) => v,
            Err(e) => {
                warn!("sync: failed to diff unpushed additions: {}", e);
                return;
            }
        };

        // Capture changed meta files BEFORE attempting rebase
        let changed_meta_files = repo.changed_files_unpushed("*.meta.yaml").unwrap_or_default();
        let mut local_metas: HashMap<PathBuf, String> = HashMap::new();
        for rel_path in &changed_meta_files {
            let abs_path = repo.root().join(rel_path);
            if let Ok(content) = std::fs::read_to_string(&abs_path) {
                local_metas.insert(rel_path.clone(), content);
            }
        }

        // Try rebase (fast path: no .thread conflicts)
        match repo.rebase_onto_origin() {
            Ok(()) => {
                match repo.push() {
                    Ok(()) => {
                        on_pushed();
                        info!("sync: push complete after rebase (attempt {})", attempt);
                        return;
                    }
                    Err(_) => {
                        warn!("sync: push failed after rebase (attempt {}), retrying", attempt);
                        continue;
                    }
                }
            }
            Err(_) => {
                // Rebase failed — use thread-aware + meta conflict resolution
                if local_additions.is_empty() && local_metas.is_empty() {
                    let _ = repo.discard_unpushed();
                    warn!("sync: non-thread/meta rebase conflict, aborted");
                    return;
                }

                // SyncLoop manages git state; resolve_content does pure content transform
                if let Err(e) = repo.discard_unpushed() {
                    warn!("sync: discard_unpushed failed: {}", e);
                    return;
                }

                let mut modified_paths: Vec<String> = Vec::new();

                // Thread resolution
                let thread_mappings = if !local_additions.is_empty() {
                    match conflict::resolve_content(&local_additions, repo.root()) {
                        Ok((resolved_files, mappings)) => {
                            for resolved in &resolved_files {
                                let abs_path = repo.root().join(&resolved.path);
                                if let Err(e) = std::fs::write(&abs_path, &resolved.content) {
                                    warn!("sync: failed to write resolved file: {}", e);
                                    return;
                                }
                                modified_paths.push(resolved.path.to_str().unwrap_or("").to_string());
                            }
                            mappings
                        }
                        Err(e) => {
                            warn!("sync: conflict resolution failed: {}", e);
                            return;
                        }
                    }
                } else {
                    Vec::new()
                };

                // Meta resolution
                for (rel_path, local_content) in &local_metas {
                    let abs_path = repo.root().join(rel_path);
                    if rel_path.starts_with("channels/") {
                        // Channel meta: merge members as union, scalars take remote
                        let remote_content = match std::fs::read_to_string(&abs_path) {
                            Ok(c) => c,
                            Err(e) => {
                                warn!("sync: failed to read remote meta {}: {}", rel_path.display(), e);
                                continue;
                            }
                        };
                        let local_meta: gitim_core::types::ChannelMeta = match serde_yaml::from_str(local_content) {
                            Ok(m) => m,
                            Err(e) => {
                                warn!("sync: failed to parse local meta {}: {}", rel_path.display(), e);
                                continue;
                            }
                        };
                        let remote_meta: gitim_core::types::ChannelMeta = match serde_yaml::from_str(&remote_content) {
                            Ok(m) => m,
                            Err(e) => {
                                warn!("sync: failed to parse remote meta {}: {}", rel_path.display(), e);
                                continue;
                            }
                        };
                        let merged = conflict::merge_channel_meta(&local_meta, &remote_meta);
                        match serde_yaml::to_string(&merged) {
                            Ok(yaml) => {
                                if let Err(e) = std::fs::write(&abs_path, &yaml) {
                                    warn!("sync: failed to write merged meta: {}", e);
                                    continue;
                                }
                            }
                            Err(e) => {
                                warn!("sync: failed to serialize merged meta: {}", e);
                                continue;
                            }
                        }
                    } else {
                        // User meta or other: write local content back as-is
                        if let Err(e) = std::fs::write(&abs_path, local_content) {
                            warn!("sync: failed to write back local meta: {}", e);
                            continue;
                        }
                    }
                    modified_paths.push(rel_path.to_str().unwrap_or("").to_string());
                }

                // Commit resolved content
                if !modified_paths.is_empty() {
                    let path_refs: Vec<&str> = modified_paths.iter().map(|s| s.as_str()).collect();
                    let commit_msg = if !thread_mappings.is_empty() {
                        build_rebase_commit_msg(&thread_mappings, &local_additions)
                    } else {
                        "meta: sync after rebase".to_string()
                    };
                    if let Err(e) = repo.add_and_commit(&path_refs, &commit_msg) {
                        warn!("sync: commit after conflict resolution failed: {}", e);
                        return;
                    }
                }

                for m in &thread_mappings {
                    on_renumbered(m.file.clone(), m.old_line, m.new_line);
                }

                match repo.push() {
                    Ok(()) => {
                        on_pushed();
                        info!("sync: push complete after conflict resolution (attempt {})", attempt);
                        return;
                    }
                    Err(_) => {
                        warn!("sync: push failed after conflict resolution (attempt {}), retrying", attempt);
                        continue;
                    }
                }
            }
        }
    }

    warn!("sync: push failed after {} retries, giving up", MAX_SYNC_RETRIES);
}
