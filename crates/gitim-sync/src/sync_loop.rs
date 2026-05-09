use rand::Rng;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use tracing::{error, info, warn};

use crate::conflict::{self, build_rebase_commit_msg};
use crate::git::{GitError, GitStorage};

/// Outcome of a single sync cycle, used to determine backoff.
pub enum SyncOutcome {
    Normal,
    RateLimited,
    /// Auth circuit is tripped; loop should idle without making git calls.
    AuthCircuitOpen,
}

/// Consecutive auth failures at which the circuit trips.
/// Credentials can fail for 1-2 cycles during rotation; 3 strikes is where we're
/// confident the PAT is revoked rather than transiently noisy.
pub const AUTH_FAILURE_TRIP_THRESHOLD: u32 = 3;

/// Tracks consecutive auth failures and latches the `tripped` flag shared with daemon state.
/// A successful remote op resets the counter; once tripped, the flag stays set
/// until the daemon clears it (v1: restart = fresh state).
pub struct AuthCircuit {
    pub tripped: Arc<AtomicBool>,
    consecutive_failures: u32,
}

impl AuthCircuit {
    pub fn new(tripped: Arc<AtomicBool>) -> Self {
        Self {
            tripped,
            consecutive_failures: 0,
        }
    }

    pub fn is_tripped(&self) -> bool {
        self.tripped.load(Ordering::SeqCst)
    }

    /// Feed the circuit a push/fetch result. Returns true iff this call transitioned
    /// the circuit from closed to tripped (caller logs once on that edge).
    pub fn record(&mut self, result: &Result<(), GitError>) -> bool {
        match result {
            Ok(()) => {
                self.consecutive_failures = 0;
                false
            }
            Err(GitError::AuthFailed(_)) => {
                self.consecutive_failures = self.consecutive_failures.saturating_add(1);
                if self.consecutive_failures >= AUTH_FAILURE_TRIP_THRESHOLD
                    && !self.tripped.swap(true, Ordering::SeqCst)
                {
                    return true;
                }
                false
            }
            // Non-auth errors neither reset nor advance the counter. A network
            // blip between two auth failures shouldn't mask credential decay,
            // and a non-auth failure shouldn't count toward the auth budget.
            Err(_) => false,
        }
    }

    #[cfg(test)]
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }
}

/// Start the sync loop with push-first strategy.
///
/// - `commit_lock`: serializes every mutation of the local commit tree. Held
///   only around `git rebase` and the conflict-resolution write+commit
///   sequence — never around the network-only `fetch`/`push`. The daemon's
///   write handlers hold the same lock around their own commits, so `rebase`
///   is guaranteed never to run while a handler is mid-append.
/// - `on_pushed`: called after a successful push (all pending messages are now remote)
/// - `on_renumbered`: called for each message that was renumbered during conflict resolution
///   (file, old_line, new_line)
/// - `on_synced`: called after every sync cycle completes, with the current HEAD commit hash.
///   The index layer uses this to decide whether incremental updates are needed.
/// - `on_cycle_done`: called at the very end of every cycle, regardless of success or failure.
///   Used to notify remaining waiters that the push did not succeed.
/// - `rebase_author`: snapshot of `(name, email)` to stamp on the rebase-resolution
///   commit. `None` falls back to git config (legacy behaviour). Daemon passes the
///   `current_user` handler + workspace github email so that rebased commits
///   attribute to the daemon owner instead of whoever the OS-level git
///   config happens to name.
#[allow(clippy::too_many_arguments)]
pub async fn start_sync_loop<F1, F2, F3, F4>(
    repo_root: &Path,
    interval_secs: u32,
    push_notify: Arc<Notify>,
    auth_failed: Arc<AtomicBool>,
    commit_lock: Arc<Mutex<()>>,
    on_pushed: F1,
    on_renumbered: F2,
    on_synced: F3,
    on_cycle_done: F4,
    rebase_author: Option<(String, String)>,
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

    let base_ms = interval_secs as u64 * 1000;
    let jitter_range = base_ms / 3;
    let mut consecutive_rate_limits: u32 = 0;
    let mut circuit = AuthCircuit::new(auth_failed);

    info!(
        "sync loop started, interval={}s (jitter +0..{}ms)",
        interval_secs, jitter_range
    );

    // Initial delay before first cycle (skip immediate fire)
    let mut next_delay = Duration::from_millis(base_ms);

    loop {
        if consecutive_rate_limits > 0 || circuit.is_tripped() {
            // During rate-limit backoff or tripped auth circuit, ignore
            // push_notify: hammering the remote just burns rate-limit budget.
            tokio::time::sleep(next_delay).await;
        } else {
            tokio::select! {
                _ = tokio::time::sleep(next_delay) => {}
                _ = push_notify.notified() => {}
            }
        }

        let outcome = run_sync_cycle(
            &repo,
            &mut circuit,
            &commit_lock,
            &on_pushed,
            &on_renumbered,
            &on_synced,
            &on_cycle_done,
            rebase_author.as_ref(),
        );

        next_delay = match outcome {
            SyncOutcome::Normal => {
                consecutive_rate_limits = 0;
                let jitter = if jitter_range > 0 {
                    rand::rng().random_range(0..jitter_range)
                } else {
                    0
                };
                Duration::from_millis(base_ms + jitter)
            }
            SyncOutcome::RateLimited => {
                consecutive_rate_limits = consecutive_rate_limits.saturating_add(1);
                let backoff_ms = base_ms * 2u64.pow(consecutive_rate_limits.min(5));
                let capped_ms = backoff_ms.min(120_000);
                // Jitter on backoff too — prevent thundering herd when
                // multiple agents get rate-limited simultaneously.
                let backoff_jitter = rand::rng().random_range(0..capped_ms / 3 + 1);
                warn!(
                    "sync: rate limited, backing off {}ms (consecutive: {})",
                    capped_ms + backoff_jitter,
                    consecutive_rate_limits
                );
                Duration::from_millis(capped_ms + backoff_jitter)
            }
            SyncOutcome::AuthCircuitOpen => {
                // Idle on the regular cadence. Flag stays latched until the
                // daemon clears it (v1: restart). No git calls get made.
                Duration::from_millis(base_ms)
            }
        };
    }
}

/// Execute one sync cycle. Completely self-contained — never panics, always logs.
/// Made `pub` so integration tests can drive cycles deterministically without
/// spawning the async loop.
#[allow(clippy::too_many_arguments)]
pub fn run_sync_cycle<F1, F2, F3, F4>(
    repo: &GitStorage,
    circuit: &mut AuthCircuit,
    commit_lock: &Mutex<()>,
    on_pushed: &F1,
    on_renumbered: &F2,
    on_synced: &F3,
    on_cycle_done: &F4,
    rebase_author: Option<&(String, String)>,
) -> SyncOutcome
where
    F1: Fn(),
    F2: Fn(PathBuf, u64, u64),
    F3: Fn(String),
    F4: Fn(),
{
    if circuit.is_tripped() {
        on_cycle_done();
        return SyncOutcome::AuthCircuitOpen;
    }

    let has_unpushed = match repo.has_unpushed_commits() {
        Ok(v) => v,
        Err(e) => {
            warn!("sync: failed to check unpushed commits: {}", e);
            on_cycle_done();
            return SyncOutcome::Normal;
        }
    };

    let outcome = if has_unpushed {
        sync_with_push(
            repo,
            circuit,
            commit_lock,
            on_pushed,
            on_renumbered,
            rebase_author,
        )
    } else {
        sync_pull_only(repo, circuit, commit_lock)
    };

    match repo.rev_parse("HEAD") {
        Ok(head) => on_synced(head),
        Err(e) => warn!("sync: failed to get HEAD for on_synced: {}", e),
    }

    on_cycle_done();
    outcome
}

/// Push-first strategy: try push, fallback to fetch+rebase, then conflict resolution.
/// Retries up to 3 times if push fails after conflict resolution.
const MAX_SYNC_RETRIES: usize = 3;

/// Every remote operation in the sync loop funnels its result through this helper
/// so the auth circuit observes every push/fetch. Callers check the returned flag
/// once and trip-log if it transitioned.
fn observe_auth(circuit: &mut AuthCircuit, result: &Result<(), GitError>) {
    if circuit.record(result) {
        error!(
            "sync: auth circuit tripped after {} consecutive auth failures — \
             sync loop will idle until daemon restart",
            AUTH_FAILURE_TRIP_THRESHOLD
        );
    }
}

fn sync_with_push<F1, F2>(
    repo: &GitStorage,
    circuit: &mut AuthCircuit,
    commit_lock: &Mutex<()>,
    on_pushed: &F1,
    on_renumbered: &F2,
    rebase_author: Option<&(String, String)>,
) -> SyncOutcome
where
    F1: Fn(),
    F2: Fn(PathBuf, u64, u64),
{
    for attempt in 1..=MAX_SYNC_RETRIES {
        // Try push directly
        let push_result = repo.push();
        observe_auth(circuit, &push_result);
        match push_result {
            Ok(()) => {
                on_pushed();
                info!("sync: push complete (attempt {})", attempt);
                return SyncOutcome::Normal;
            }
            Err(GitError::RateLimited) => {
                warn!("sync: push rate limited (attempt {})", attempt);
                return SyncOutcome::RateLimited;
            }
            Err(GitError::AuthFailed(_)) => {
                warn!("sync: push auth failed (attempt {})", attempt);
                if circuit.is_tripped() {
                    return SyncOutcome::AuthCircuitOpen;
                }
                return SyncOutcome::Normal;
            }
            Err(GitError::PushConflict) => {
                // Remote has diverged, need to sync
            }
            Err(e) => {
                warn!("sync: push failed (non-conflict): {}", e);
                return SyncOutcome::Normal;
            }
        }

        // Fetch remote changes
        let fetch_result = repo.fetch();
        observe_auth(circuit, &fetch_result);
        match fetch_result {
            Err(GitError::RateLimited) => {
                warn!("sync: fetch rate limited (attempt {})", attempt);
                return SyncOutcome::RateLimited;
            }
            Err(GitError::AuthFailed(_)) => {
                warn!("sync: fetch auth failed (attempt {})", attempt);
                if circuit.is_tripped() {
                    return SyncOutcome::AuthCircuitOpen;
                }
                return SyncOutcome::Normal;
            }
            Err(e) => {
                warn!("sync: fetch failed: {}", e);
                return SyncOutcome::Normal;
            }
            Ok(()) => {}
        }

        // Capture local additions BEFORE attempting rebase
        let local_additions = match repo.diff_unpushed("*.thread") {
            Ok(v) => v,
            Err(e) => {
                warn!("sync: failed to diff unpushed additions: {}", e);
                return SyncOutcome::Normal;
            }
        };

        // Capture changed meta files BEFORE attempting rebase
        let changed_meta_files = repo
            .changed_files_unpushed("*.meta.yaml")
            .unwrap_or_default();
        let mut local_metas: HashMap<PathBuf, String> = HashMap::new();
        for rel_path in &changed_meta_files {
            let abs_path = repo.root().join(rel_path);
            if let Ok(content) = std::fs::read_to_string(&abs_path) {
                local_metas.insert(rel_path.clone(), content);
            }
        }

        // Rebase + optional conflict resolution mutates the local commit
        // tree; hold commit_lock across the whole block so handler writes
        // never interleave with it. Push happens *after* the guard drops so
        // a slow remote can't stall handler writers.
        let rebase_guard = commit_lock.lock().expect("commit_lock poisoned");

        // Try rebase (fast path: no .thread conflicts)
        match repo.rebase_onto_origin() {
            Ok(()) => {
                drop(rebase_guard);
                let push_after_rebase = repo.push();
                observe_auth(circuit, &push_after_rebase);
                match push_after_rebase {
                    Ok(()) => {
                        on_pushed();
                        info!("sync: push complete after rebase (attempt {})", attempt);
                        return SyncOutcome::Normal;
                    }
                    Err(GitError::RateLimited) => {
                        warn!("sync: push rate limited after rebase (attempt {})", attempt);
                        return SyncOutcome::RateLimited;
                    }
                    Err(GitError::AuthFailed(_)) => {
                        warn!("sync: push auth failed after rebase (attempt {})", attempt);
                        if circuit.is_tripped() {
                            return SyncOutcome::AuthCircuitOpen;
                        }
                        return SyncOutcome::Normal;
                    }
                    Err(_) => {
                        warn!(
                            "sync: push failed after rebase (attempt {}), retrying",
                            attempt
                        );
                        // Blocking sleep OK: run_sync_cycle is already synchronous (git commands)
                        std::thread::sleep(Duration::from_millis(200 * 2u64.pow(attempt as u32)));
                        continue;
                    }
                }
            }
            Err(_) => {
                // Rebase failed. Two failure modes from here:
                //
                //   1. All local unpushed files belong to the resolvable set
                //      (`*.thread` additions, `*.meta.yaml` changes). Discard
                //      the partial rebase, re-apply via the content-aware
                //      resolvers below.
                //   2. Any local unpushed file is OUTSIDE that set
                //      (`crons/<name>/spec.yaml`, an arbitrary doc, a binary
                //      asset, future protocol additions). The resolvable
                //      path's `discard_unpushed` would `git reset --hard
                //      @{upstream}` and silently destroy those edits. There
                //      is no generic content-aware resolver for arbitrary
                //      files, so we bail: abort the rebase (which leaves HEAD
                //      and working tree intact), warn, and let the next cycle
                //      retry. The user keeps their commit.
                //
                // The detection is "did the unpushed range touch any file
                // outside `*.thread` and `*.meta.yaml`" — not "is the
                // conflict on a non-resolvable file". A single unpushed
                // commit that touches both a thread file and a cron spec
                // would otherwise have its spec destroyed even when only the
                // thread side actually conflicts.
                let all_unpushed = repo.changed_files_unpushed_all().unwrap_or_default();
                let has_unresolvable = all_unpushed.iter().any(|p| {
                    let path_str = p.to_string_lossy();
                    !path_str.ends_with(".thread") && !path_str.ends_with(".meta.yaml")
                });
                if has_unresolvable {
                    let _ = repo.abort_rebase();
                    warn!(
                        "sync: rebase conflict on non-resolvable files (e.g. cron specs); \
                         preserving local commit. Files: {:?}",
                        all_unpushed
                    );
                    return SyncOutcome::Normal;
                }

                // Rebase failed — use thread-aware + meta conflict resolution
                if local_additions.is_empty() && local_metas.is_empty() {
                    let _ = repo.abort_rebase();
                    warn!("sync: rebase conflict with no resolvable changes, aborted");
                    return SyncOutcome::Normal;
                }

                // SyncLoop manages git state; resolve_content does pure content transform
                if let Err(e) = repo.discard_unpushed() {
                    warn!("sync: discard_unpushed failed: {}", e);
                    return SyncOutcome::Normal;
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
                                    return SyncOutcome::Normal;
                                }
                                modified_paths
                                    .push(resolved.path.to_str().unwrap_or("").to_string());
                            }
                            mappings
                        }
                        Err(e) => {
                            warn!("sync: conflict resolution failed: {}", e);
                            return SyncOutcome::Normal;
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
                                warn!(
                                    "sync: failed to read remote meta {}: {}",
                                    rel_path.display(),
                                    e
                                );
                                continue;
                            }
                        };
                        let local_meta: gitim_core::types::ChannelMeta =
                            match serde_yaml::from_str(local_content) {
                                Ok(m) => m,
                                Err(e) => {
                                    warn!(
                                        "sync: failed to parse local meta {}: {}",
                                        rel_path.display(),
                                        e
                                    );
                                    continue;
                                }
                            };
                        let remote_meta: gitim_core::types::ChannelMeta =
                            match serde_yaml::from_str(&remote_content) {
                                Ok(m) => m,
                                Err(e) => {
                                    warn!(
                                        "sync: failed to parse remote meta {}: {}",
                                        rel_path.display(),
                                        e
                                    );
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
                    // Under normal operation every local commit on this clone
                    // belongs to one handler (the daemon owner), so stamping
                    // the rebase-resolution commit with that handler matches
                    // reality. Committer still comes from git config — only
                    // author is rewritten. `None` preserves the legacy
                    // behaviour (git config picks author too).
                    let commit_result = match rebase_author {
                        Some((name, email)) => {
                            repo.add_and_commit_as(&path_refs, &commit_msg, Some((name, email)))
                        }
                        None => repo.add_and_commit(&path_refs, &commit_msg),
                    };
                    if let Err(e) = commit_result {
                        warn!("sync: commit after conflict resolution failed: {}", e);
                        return SyncOutcome::Normal;
                    }
                }

                for m in &thread_mappings {
                    on_renumbered(m.file.clone(), m.old_line, m.new_line);
                }

                // Resolve committed — commit tree is stable again, release
                // before the network round-trip so a slow push doesn't hold
                // back handler writers waiting on commit_lock.
                drop(rebase_guard);

                let push_after_resolve = repo.push();
                observe_auth(circuit, &push_after_resolve);
                match push_after_resolve {
                    Ok(()) => {
                        on_pushed();
                        info!(
                            "sync: push complete after conflict resolution (attempt {})",
                            attempt
                        );
                        return SyncOutcome::Normal;
                    }
                    Err(GitError::RateLimited) => {
                        warn!(
                            "sync: push rate limited after conflict resolution (attempt {})",
                            attempt
                        );
                        return SyncOutcome::RateLimited;
                    }
                    Err(GitError::AuthFailed(_)) => {
                        warn!(
                            "sync: push auth failed after conflict resolution (attempt {})",
                            attempt
                        );
                        if circuit.is_tripped() {
                            return SyncOutcome::AuthCircuitOpen;
                        }
                        return SyncOutcome::Normal;
                    }
                    Err(_) => {
                        warn!(
                            "sync: push failed after conflict resolution (attempt {}), retrying",
                            attempt
                        );
                        // Blocking sleep OK: run_sync_cycle is already synchronous (git commands)
                        std::thread::sleep(Duration::from_millis(200 * 2u64.pow(attempt as u32)));
                        continue;
                    }
                }
            }
        }
    }

    warn!(
        "sync: push failed after {} retries, giving up",
        MAX_SYNC_RETRIES
    );
    SyncOutcome::Normal
}

/// Pull-only path: fetch remote changes, then fast-forward via rebase.
/// On failure, abort the rebase but preserve local state — next cycle retries.
fn sync_pull_only(
    repo: &GitStorage,
    circuit: &mut AuthCircuit,
    commit_lock: &Mutex<()>,
) -> SyncOutcome {
    let fetch_result = repo.fetch();
    observe_auth(circuit, &fetch_result);
    match fetch_result {
        Err(GitError::RateLimited) => {
            warn!("sync: fetch rate limited (pull-only)");
            return SyncOutcome::RateLimited;
        }
        Err(GitError::AuthFailed(_)) => {
            warn!("sync: fetch auth failed (pull-only)");
            if circuit.is_tripped() {
                return SyncOutcome::AuthCircuitOpen;
            }
            return SyncOutcome::Normal;
        }
        Err(e) => {
            warn!("sync: fetch failed: {}", e);
            return SyncOutcome::Normal;
        }
        Ok(()) => {}
    }

    // Rebase mutates the local commit tree; hold commit_lock so it can't
    // interleave with a handler's read-append-commit window.
    let _rebase_guard = commit_lock.lock().expect("commit_lock poisoned");
    if let Err(e) = repo.rebase_onto_origin() {
        warn!("sync: rebase failed after fetch: {}", e);
        let _ = repo.abort_rebase();
    }

    SyncOutcome::Normal
}
