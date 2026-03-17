use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::time;
use tracing::{info, warn};

use crate::conflict;
use crate::git::GitRepo;

/// Start the sync loop with push-first strategy.
///
/// - `on_pushed`: called after a successful push (all pending messages are now remote)
/// - `on_renumbered`: called for each message that was renumbered during conflict resolution
///   (file, old_line, new_line)
pub async fn start_sync_loop<F1, F2>(
    repo_root: &Path,
    interval_secs: u32,
    on_pushed: F1,
    on_renumbered: F2,
) where
    F1: Fn() + Send + 'static,
    F2: Fn(PathBuf, u64, u64) + Send + 'static,
{
    if interval_secs == 0 {
        info!("sync_interval=0, auto-sync disabled");
        return;
    }

    let repo = GitRepo::new(repo_root);

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
        ticker.tick().await;
        run_sync_cycle(&repo, &on_pushed, &on_renumbered);
    }
}

/// Execute one sync cycle. Completely self-contained — never panics, always logs.
fn run_sync_cycle<F1, F2>(repo: &GitRepo, on_pushed: &F1, on_renumbered: &F2)
where
    F1: Fn(),
    F2: Fn(PathBuf, u64, u64),
{
    let has_unpushed = match repo.has_unpushed_commits() {
        Ok(v) => v,
        Err(e) => {
            warn!("sync: failed to check unpushed commits: {}", e);
            return;
        }
    };

    if has_unpushed {
        sync_with_push(repo, on_pushed, on_renumbered);
    } else {
        // Nothing local to push, just pull
        match repo.pull_rebase() {
            Ok(()) => info!("sync: pull complete"),
            Err(e) => warn!("sync: pull failed: {}", e),
        }
    }
}

/// Push-first strategy: try push, fallback to fetch+rebase, then conflict resolution.
/// Retries up to 3 times if push fails after conflict resolution.
const MAX_SYNC_RETRIES: usize = 3;

fn sync_with_push<F1, F2>(repo: &GitRepo, on_pushed: &F1, on_renumbered: &F2)
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
            Err(_) => {
                // Push rejected — remote has new commits, need to sync
            }
        }

        // Fetch remote changes
        if let Err(e) = repo.fetch() {
            warn!("sync: fetch failed: {}", e);
            return;
        }

        // Capture local additions BEFORE attempting rebase
        let local_additions = match repo.diff_unpushed_thread_additions() {
            Ok(v) => v,
            Err(e) => {
                warn!("sync: failed to diff unpushed additions: {}", e);
                return;
            }
        };

        // Try rebase
        match repo.pull_rebase() {
            Ok(()) => {
                // Rebase succeeded (no .thread conflicts), push again
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
                // Rebase failed (conflict), use thread-aware resolution
                if !local_additions.is_empty() {
                    match conflict::resolve_thread_conflicts(repo, &local_additions) {
                        Ok(mappings) => {
                            for m in &mappings {
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
                        Err(e) => {
                            warn!("sync: conflict resolution failed: {}", e);
                            return;
                        }
                    }
                } else {
                    // Non-thread conflict (shouldn't happen normally)
                    let _ = repo.rebase_abort();
                    warn!("sync: non-thread rebase conflict, aborted");
                    return;
                }
            }
        }
    }

    warn!("sync: push failed after {} retries, giving up", MAX_SYNC_RETRIES);
}
