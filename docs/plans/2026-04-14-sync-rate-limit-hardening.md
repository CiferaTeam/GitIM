# Sync Loop Rate-Limit Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix sync loop rate-limit risks — add jitter, 429 detection, push retry backoff, and unify pull-only path to prevent data loss.

**Architecture:** `run_sync_cycle` returns a `SyncOutcome` enum (`Normal` / `RateLimited`). The async loop uses `tokio::time::sleep` with jitter instead of a fixed-interval ticker. On `RateLimited`, exponential backoff kicks in while `push_notify` remains responsive via `tokio::select!`. Pull-only path switches from `pull --rebase` to `fetch` + `rebase_onto_origin` with abort-only failure handling.

**Tech Stack:** Rust, tokio, rand 0.9

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `crates/gitim-core/src/types/config.rs` | Modify | Default sync_interval 1→3 |
| `crates/gitim-sync/Cargo.toml` | Modify | Add `rand = "0.9"` dependency |
| `crates/gitim-sync/src/git.rs` | Modify | `RateLimited` variant, `is_rate_limited()`, `abort_rebase()`, detection in `fetch()`/`push()` |
| `crates/gitim-sync/src/sync_loop.rs` | Modify | `SyncOutcome` enum, jitter loop, backoff, retry sleep, pull-only path fix |

---

## Task 1: Default sync_interval 1→3s

**Files:**
- Modify: `crates/gitim-core/src/types/config.rs`

- [ ] **Step 1: Update test assertion**

In `config_default_values` test (line 62), change the assertion:

```rust
assert_eq!(c.daemon.sync_interval, 3);
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p gitim-core config_default_values`
Expected: FAIL — `assertion failed: 1 != 3`

- [ ] **Step 3: Change default values**

Change `default_sync_interval()` (line 38):

```rust
fn default_sync_interval() -> u32 { 3 }
```

Change `DaemonConfig::default()` (line 31):

```rust
sync_interval: 3,
```

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p gitim-core`
Expected: all pass

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-core/src/types/config.rs
git commit -m "fix(sync): change default sync_interval from 1s to 3s

Reduces baseline request rate against remote. 10 agents at 1s = 10 req/s,
at 3s = ~3.3 req/s before jitter."
```

---

## Task 2: RateLimited variant + detection helper

**Files:**
- Modify: `crates/gitim-sync/src/git.rs`

- [ ] **Step 1: Write test for is_rate_limited detection**

Add `#[cfg(test)] mod tests` at the bottom of `git.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_detection_matches_known_patterns() {
        // GitHub HTTPS
        assert!(is_rate_limited("fatal: unable to access '...': The requested URL returned error: 429"));
        // Generic rate limit message
        assert!(is_rate_limited("fatal: rate limit exceeded for this endpoint"));
        // Case insensitive
        assert!(is_rate_limited("Rate Limit Exceeded"));
        assert!(is_rate_limited("Too Many Requests"));
        // SecondaryRateLimit (GitHub API)
        assert!(is_rate_limited("SecondaryRateLimit"));
    }

    #[test]
    fn rate_limit_detection_no_false_positives() {
        assert!(!is_rate_limited("fatal: authentication failed"));
        assert!(!is_rate_limited("error: failed to push some refs"));
        assert!(!is_rate_limited("[rejected] main -> main (non-fast-forward)"));
        assert!(!is_rate_limited(""));
    }
}
```

- [ ] **Step 2: Run test to verify it fails (function doesn't exist)**

Run: `cargo test -p gitim-sync --lib rate_limit_detection`
Expected: FAIL — `cannot find function is_rate_limited`

- [ ] **Step 3: Add RateLimited variant and is_rate_limited()**

Add to `GitError` enum (after `PushConflict`):

```rust
#[error("rate limited by remote")]
RateLimited,
```

Add the helper function after the `GitStorage` impl block:

```rust
fn is_rate_limited(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("rate limit")
        || lower.contains("too many requests")
        || lower.contains("429")
        || lower.contains("secondaryratelimit")
}
```

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p gitim-sync --lib rate_limit_detection`
Expected: both tests pass

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-sync/src/git.rs
git commit -m "feat(sync): add RateLimited error variant and stderr detection

Detects GitHub/Gitea/GitLab rate limit responses by matching stderr
patterns: 429 status, 'rate limit', 'too many requests',
'SecondaryRateLimit'."
```

---

## Task 3: abort_rebase() method

**Files:**
- Modify: `crates/gitim-sync/src/git.rs`

- [ ] **Step 1: Write test for abort_rebase**

Add to the `#[cfg(test)] mod tests` in `git.rs`:

```rust
#[test]
fn abort_rebase_is_safe_when_no_rebase_in_progress() {
    let dir = tempfile::TempDir::new().unwrap();
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let repo = GitStorage::new(dir.path());
    // Should not error even when no rebase is in progress
    repo.abort_rebase().unwrap();
}
```

Add `tempfile` to dev-deps if not already present — it's already in `[dev-dependencies]` of Cargo.toml.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p gitim-sync --lib abort_rebase`
Expected: FAIL — `no method named abort_rebase`

- [ ] **Step 3: Implement abort_rebase**

Add to `impl GitStorage` (after `discard_unpushed`):

```rust
/// Best-effort abort any in-progress rebase. Always succeeds.
pub fn abort_rebase(&self) -> Result<(), GitError> {
    let _ = Command::new("git")
        .args(["rebase", "--abort"])
        .current_dir(&self.root)
        .output();
    Ok(())
}
```

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p gitim-sync --lib abort_rebase`
Expected: PASS

- [ ] **Step 5: Write test — abort_rebase preserves local commits after failed rebase**

Add to the `#[cfg(test)] mod tests` in `git.rs`:

```rust
#[test]
fn abort_rebase_preserves_local_commit() {
    // Setup: bare repo + two clones
    let bare_dir = tempfile::TempDir::new().unwrap();
    let clone_a = tempfile::TempDir::new().unwrap();
    let clone_b = tempfile::TempDir::new().unwrap();

    std::process::Command::new("git").args(["init", "--bare"]).current_dir(bare_dir.path()).output().unwrap();

    std::process::Command::new("git").args(["clone", bare_dir.path().to_str().unwrap(), clone_a.path().to_str().unwrap()]).current_dir(bare_dir.path().parent().unwrap()).output().unwrap();
    std::process::Command::new("git").args(["config", "user.email", "a@test.com"]).current_dir(clone_a.path()).output().unwrap();
    std::process::Command::new("git").args(["config", "user.name", "A"]).current_dir(clone_a.path()).output().unwrap();

    // Initial commit
    std::fs::write(clone_a.path().join("init.txt"), "init").unwrap();
    std::process::Command::new("git").args(["add", "."]).current_dir(clone_a.path()).output().unwrap();
    std::process::Command::new("git").args(["commit", "-m", "initial"]).current_dir(clone_a.path()).output().unwrap();
    std::process::Command::new("git").args(["push", "-u", "origin", "main"]).current_dir(clone_a.path()).output().unwrap();

    // Clone B
    std::process::Command::new("git").args(["clone", bare_dir.path().to_str().unwrap(), clone_b.path().to_str().unwrap()]).current_dir(bare_dir.path().parent().unwrap()).output().unwrap();
    std::process::Command::new("git").args(["config", "user.email", "b@test.com"]).current_dir(clone_b.path()).output().unwrap();
    std::process::Command::new("git").args(["config", "user.name", "B"]).current_dir(clone_b.path()).output().unwrap();

    // Clone A: modify init.txt and push
    std::fs::write(clone_a.path().join("init.txt"), "A's version").unwrap();
    std::process::Command::new("git").args(["add", "init.txt"]).current_dir(clone_a.path()).output().unwrap();
    std::process::Command::new("git").args(["commit", "-m", "A change"]).current_dir(clone_a.path()).output().unwrap();
    std::process::Command::new("git").args(["push"]).current_dir(clone_a.path()).output().unwrap();

    // Clone B: conflicting change + commit (not pushed)
    std::fs::write(clone_b.path().join("init.txt"), "B's version").unwrap();
    std::process::Command::new("git").args(["add", "init.txt"]).current_dir(clone_b.path()).output().unwrap();
    std::process::Command::new("git").args(["commit", "-m", "B change"]).current_dir(clone_b.path()).output().unwrap();

    let repo_b = GitStorage::new(clone_b.path());

    // Fetch (simulating the new pull-only path)
    repo_b.fetch().unwrap();

    // Rebase fails due to conflict
    let result = repo_b.rebase_onto_origin();
    assert!(result.is_err(), "rebase should fail due to conflict");

    // abort_rebase (NOT discard_unpushed)
    repo_b.abort_rebase().unwrap();

    // KEY ASSERTION: local commit is preserved
    let content = std::fs::read_to_string(clone_b.path().join("init.txt")).unwrap();
    assert_eq!(content, "B's version", "local commit should be preserved after abort_rebase");

    // Also: repo is NOT in rebase state
    let rebase_merge = clone_b.path().join(".git/rebase-merge");
    let rebase_apply = clone_b.path().join(".git/rebase-apply");
    assert!(!rebase_merge.exists() && !rebase_apply.exists(), "repo should be clean after abort");

    // Also: has_unpushed_commits still returns true
    assert!(repo_b.has_unpushed_commits().unwrap(), "local commit should still be unpushed");
}
```

- [ ] **Step 6: Run all git.rs tests**

Run: `cargo test -p gitim-sync --lib`
Expected: all pass

- [ ] **Step 7: Commit**

```bash
git add crates/gitim-sync/src/git.rs
git commit -m "feat(sync): add abort_rebase() method

Best-effort rebase abort without hard reset. Used by the pull-only
sync path so failed rebases don't discard local commits.
Includes test verifying local commits survive abort_rebase."
```

---

## Task 4: Wire rate-limit detection into fetch() and push()

**Files:**
- Modify: `crates/gitim-sync/src/git.rs`

- [ ] **Step 1: Add rate-limit check to push()**

In `push()` (line 83-96), add the rate-limit check BEFORE the PushConflict check. The order matters — rate limit should take priority:

```rust
pub fn push(&self) -> Result<(), GitError> {
    let output = Command::new("git")
        .args(["push", "-u", "origin", "HEAD"])
        .current_dir(&self.root)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if is_rate_limited(&stderr) {
            return Err(GitError::RateLimited);
        }
        if stderr.contains("rejected") || stderr.contains("non-fast-forward") {
            return Err(GitError::PushConflict);
        }
        return Err(GitError::CommandFailed(stderr));
    }
    Ok(())
}
```

- [ ] **Step 2: Add rate-limit check to fetch()**

Replace `fetch()` (line 107-118):

```rust
pub fn fetch(&self) -> Result<(), GitError> {
    let output = Command::new("git")
        .args(["fetch", "origin"])
        .current_dir(&self.root)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if is_rate_limited(&stderr) {
            return Err(GitError::RateLimited);
        }
        return Err(GitError::CommandFailed(stderr));
    }
    Ok(())
}
```

- [ ] **Step 3: Verify compilation and tests**

Run: `cargo test -p gitim-sync`
Expected: all pass (no behavior change for existing tests — rate limit only triggers on specific stderr)

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-sync/src/git.rs
git commit -m "feat(sync): detect rate limiting in fetch() and push()

Both methods now check stderr for rate-limit patterns before
classifying errors. RateLimited takes priority over PushConflict."
```

---

## Task 5: SyncOutcome enum + jitter + backoff in async loop

**Files:**
- Modify: `crates/gitim-sync/Cargo.toml`
- Modify: `crates/gitim-sync/src/sync_loop.rs`

- [ ] **Step 1: Add rand dependency**

Add to `[dependencies]` in `crates/gitim-sync/Cargo.toml`:

```toml
rand = "0.9"
```

- [ ] **Step 2: Add SyncOutcome enum and update imports**

Replace the imports section at the top of `sync_loop.rs`:

```rust
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use rand::Rng;
use tokio::sync::Notify;
use tracing::{info, warn};

use crate::conflict::{self, build_rebase_commit_msg};
use crate::git::GitStorage;

/// Outcome of a single sync cycle, used to determine backoff.
pub enum SyncOutcome {
    Normal,
    RateLimited,
}
```

(Remove `use tokio::time;` — no longer needed since we switch from `interval` to `sleep`.)

- [ ] **Step 3: Refactor start_sync_loop to use jitter + backoff**

Replace the entire `start_sync_loop` function body (lines 21-61):

```rust
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

    let base_ms = interval_secs as u64 * 1000;
    let jitter_range = base_ms / 3;
    let mut consecutive_rate_limits: u32 = 0;

    info!("sync loop started, interval={}s (jitter ±{}ms)", interval_secs, jitter_range);

    // Initial delay before first cycle (skip immediate fire)
    let mut next_delay = Duration::from_millis(base_ms);

    loop {
        tokio::select! {
            _ = tokio::time::sleep(next_delay) => {}
            _ = push_notify.notified() => {}
        }

        let outcome = run_sync_cycle(&repo, &on_pushed, &on_renumbered, &on_synced, &on_cycle_done);

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
                warn!(
                    "sync: rate limited, backing off {}ms (consecutive: {})",
                    capped_ms, consecutive_rate_limits
                );
                Duration::from_millis(capped_ms)
            }
        };
    }
}
```

- [ ] **Step 4: Make run_sync_cycle return SyncOutcome**

Replace `run_sync_cycle` signature and body — for now, just propagate the return value without actually detecting rate limits yet (wired in Task 6):

```rust
fn run_sync_cycle<F1, F2, F3, F4>(
    repo: &GitStorage,
    on_pushed: &F1,
    on_renumbered: &F2,
    on_synced: &F3,
    on_cycle_done: &F4,
) -> SyncOutcome
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
            return SyncOutcome::Normal;
        }
    };

    let outcome = if has_unpushed {
        sync_with_push(repo, on_pushed, on_renumbered)
    } else {
        // Fetch + rebase (replaces pull --rebase, wired in Task 7)
        match repo.pull_rebase() {
            Ok(()) => info!("sync: pull complete"),
            Err(e) => {
                warn!("sync: pull failed: {}", e);
                let _ = repo.discard_unpushed();
            }
        }
        SyncOutcome::Normal
    };

    match repo.rev_parse("HEAD") {
        Ok(head) => on_synced(head),
        Err(e) => warn!("sync: failed to get HEAD for on_synced: {}", e),
    }

    on_cycle_done();
    outcome
}
```

- [ ] **Step 5: Make sync_with_push return SyncOutcome**

Change `sync_with_push` signature to return `SyncOutcome`. Add `-> SyncOutcome` to the signature. Replace all `return;` with `return SyncOutcome::Normal;`. The final warn line becomes:

```rust
warn!("sync: push failed after {} retries, giving up", MAX_SYNC_RETRIES);
SyncOutcome::Normal
```

(Rate limit detection inside sync_with_push is wired in Task 6.)

- [ ] **Step 6: Verify compilation and tests**

Run: `cargo test -p gitim-sync`
Expected: all pass (behavioral change: jitter added to timing, but logic is identical)

- [ ] **Step 7: Commit**

```bash
git add crates/gitim-sync/Cargo.toml crates/gitim-sync/src/sync_loop.rs
git commit -m "feat(sync): add SyncOutcome, jitter, and rate-limit backoff to sync loop

Replace fixed-interval ticker with sleep+jitter (base ± base/3).
On RateLimited outcome, exponential backoff up to 2min while
push_notify remains responsive via tokio::select!."
```

---

## Task 6: Wire rate-limit detection + retry backoff into sync_with_push

**Files:**
- Modify: `crates/gitim-sync/src/sync_loop.rs`

- [ ] **Step 1: Add rate-limit detection and retry backoff to sync_with_push**

Replace the `sync_with_push` function entirely:

```rust
fn sync_with_push<F1, F2>(repo: &GitStorage, on_pushed: &F1, on_renumbered: &F2) -> SyncOutcome
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
                return SyncOutcome::Normal;
            }
            Err(crate::git::GitError::RateLimited) => {
                warn!("sync: push rate limited (attempt {})", attempt);
                return SyncOutcome::RateLimited;
            }
            Err(crate::git::GitError::PushConflict) => {
                // Remote has diverged, need to sync
            }
            Err(e) => {
                warn!("sync: push failed (non-conflict): {}", e);
                return SyncOutcome::Normal;
            }
        }

        // Fetch remote changes
        match repo.fetch() {
            Err(crate::git::GitError::RateLimited) => {
                warn!("sync: fetch rate limited (attempt {})", attempt);
                return SyncOutcome::RateLimited;
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
                        return SyncOutcome::Normal;
                    }
                    Err(crate::git::GitError::RateLimited) => {
                        warn!("sync: push rate limited after rebase (attempt {})", attempt);
                        return SyncOutcome::RateLimited;
                    }
                    Err(_) => {
                        warn!("sync: push failed after rebase (attempt {}), retrying", attempt);
                        // Backoff before retry
                        std::thread::sleep(Duration::from_millis(200 * 2u64.pow(attempt as u32)));
                        continue;
                    }
                }
            }
            Err(_) => {
                // Rebase failed — use thread-aware + meta conflict resolution
                if local_additions.is_empty() && local_metas.is_empty() {
                    let _ = repo.discard_unpushed();
                    warn!("sync: non-thread/meta rebase conflict, aborted");
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
                                modified_paths.push(resolved.path.to_str().unwrap_or("").to_string());
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
                        return SyncOutcome::Normal;
                    }
                }

                for m in &thread_mappings {
                    on_renumbered(m.file.clone(), m.old_line, m.new_line);
                }

                match repo.push() {
                    Ok(()) => {
                        on_pushed();
                        info!("sync: push complete after conflict resolution (attempt {})", attempt);
                        return SyncOutcome::Normal;
                    }
                    Err(crate::git::GitError::RateLimited) => {
                        warn!("sync: push rate limited after conflict resolution (attempt {})", attempt);
                        return SyncOutcome::RateLimited;
                    }
                    Err(_) => {
                        warn!("sync: push failed after conflict resolution (attempt {}), retrying", attempt);
                        // Backoff before retry
                        std::thread::sleep(Duration::from_millis(200 * 2u64.pow(attempt as u32)));
                        continue;
                    }
                }
            }
        }
    }

    warn!("sync: push failed after {} retries, giving up", MAX_SYNC_RETRIES);
    SyncOutcome::Normal
}
```

- [ ] **Step 2: Verify compilation and tests**

Run: `cargo test -p gitim-sync`
Expected: all pass

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-sync/src/sync_loop.rs
git commit -m "feat(sync): wire rate-limit detection and retry backoff into push path

push() and fetch() RateLimited errors propagate as SyncOutcome::RateLimited
for exponential backoff. Push retries now sleep 400ms/800ms/1600ms between
attempts to prevent conflict storms."
```

---

## Task 7: Pull-only path — fetch + rebase with abort-only failure

**Files:**
- Modify: `crates/gitim-sync/src/sync_loop.rs`

- [ ] **Step 1: Replace pull-only path in run_sync_cycle**

In `run_sync_cycle`, replace the `else` branch (the pull-only path). Change:

```rust
    } else {
        // Fetch + rebase (replaces pull --rebase, wired in Task 7)
        match repo.pull_rebase() {
            Ok(()) => info!("sync: pull complete"),
            Err(e) => {
                warn!("sync: pull failed: {}", e);
                let _ = repo.discard_unpushed();
            }
        }
        SyncOutcome::Normal
    };
```

To:

```rust
    } else {
        sync_pull_only(repo)
    };
```

- [ ] **Step 2: Add sync_pull_only function**

Add after `sync_with_push`:

```rust
/// Pull-only path: fetch remote changes, then fast-forward via rebase.
/// On failure, abort the rebase but preserve local state — next cycle retries.
fn sync_pull_only(repo: &GitStorage) -> SyncOutcome {
    match repo.fetch() {
        Err(crate::git::GitError::RateLimited) => {
            warn!("sync: fetch rate limited (pull-only)");
            return SyncOutcome::RateLimited;
        }
        Err(e) => {
            warn!("sync: fetch failed: {}", e);
            return SyncOutcome::Normal;
        }
        Ok(()) => {}
    }

    if let Err(e) = repo.rebase_onto_origin() {
        warn!("sync: rebase failed after fetch: {}", e);
        // Only abort rebase — do NOT hard reset. Preserves any local commits
        // that appeared between has_unpushed_commits check and now.
        let _ = repo.abort_rebase();
    }

    SyncOutcome::Normal
}
```

- [ ] **Step 3: Verify compilation and tests**

Run: `cargo test -p gitim-sync`
Expected: all pass

- [ ] **Step 4: Run full workspace test**

Run: `cargo test`
Expected: all pass (including integration tests in gitim-daemon)

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-sync/src/sync_loop.rs
git commit -m "fix(sync): replace pull --rebase with fetch+rebase in pull-only path

Old path: pull_rebase() failure → hard reset → loses local commits.
New path: fetch() + rebase_onto_origin() failure → abort_rebase() only.
Preserves local commits if a handler writes between has_unpushed check
and the fetch. Next cycle detects unpushed and takes the push path."
```

---

## Task 8: E2E test — pull-only path via fetch + rebase

**Files:**
- Modify: `crates/gitim-sync/tests/sync_e2e_test.rs`

- [ ] **Step 1: Add test for pull-only path using fetch + rebase**

Add after `test_sync_pulls_when_nothing_to_push`:

```rust
#[test]
fn test_pull_only_via_fetch_rebase() {
    // Mirrors test_sync_pulls_when_nothing_to_push but uses the new
    // fetch + rebase_onto_origin path instead of pull_rebase
    let (_bare_dir, clone_a_dir, clone_b_dir) = setup_two_clones();

    let thread_a = clone_a_dir.path().join("channels/general.thread");
    let thread_b = clone_b_dir.path().join("channels/general.thread");

    let alice = Handler::new("alice").unwrap();

    // Clone A: add content, commit, push
    let content = format_message(1, 0, &alice, "20260317T100000Z", "hello from alice");
    std::fs::write(&thread_a, &content).unwrap();
    let repo_a = GitStorage::new(clone_a_dir.path());
    repo_a.add_and_commit(&["channels/general.thread"], "alice msg").unwrap();
    repo_a.push().unwrap();

    // Clone B: no local changes — use fetch + rebase (new pull-only path)
    let repo_b = GitStorage::new(clone_b_dir.path());
    assert!(!repo_b.has_unpushed_commits().unwrap());

    repo_b.fetch().expect("fetch should succeed");
    repo_b.rebase_onto_origin().expect("rebase should succeed (fast-forward)");

    // Verify: clone_b now has the content from clone_a
    let b_content = std::fs::read_to_string(&thread_b).unwrap();
    let file = parse_thread(&b_content).unwrap();
    assert_eq!(file.messages().len(), 1);
    assert_eq!(file.messages()[0].author.as_str(), "alice");
    assert_eq!(file.messages()[0].body, "hello from alice");
}

#[test]
fn test_pull_only_abort_rebase_preserves_racing_commit() {
    // Simulates the race condition: handler writes a commit between
    // has_unpushed_commits (false) and the fetch+rebase. When rebase
    // conflicts, abort_rebase must preserve the racing local commit.
    let (_bare_dir, clone_a_dir, clone_b_dir) = setup_two_clones();

    let thread_a = clone_a_dir.path().join("channels/general.thread");
    let thread_b = clone_b_dir.path().join("channels/general.thread");

    let alice = Handler::new("alice").unwrap();
    let bob = Handler::new("bob").unwrap();

    // Clone A: add a message, push
    let a_content = format_message(1, 0, &alice, "20260317T100000Z", "alice msg");
    std::fs::write(&thread_a, &a_content).unwrap();
    let repo_a = GitStorage::new(clone_a_dir.path());
    repo_a.add_and_commit(&["channels/general.thread"], "alice msg").unwrap();
    repo_a.push().unwrap();

    // Clone B: "racing" local commit (simulating handler write during pull-only window)
    let b_content = format_message(1, 0, &bob, "20260317T100100Z", "bob msg");
    std::fs::write(&thread_b, &b_content).unwrap();
    let repo_b = GitStorage::new(clone_b_dir.path());
    repo_b.add_and_commit(&["channels/general.thread"], "bob msg").unwrap();

    // Clone B: fetch succeeds
    repo_b.fetch().unwrap();

    // Clone B: rebase fails (both wrote L000001 to same file)
    let rebase_result = repo_b.rebase_onto_origin();
    assert!(rebase_result.is_err(), "rebase should fail due to conflict");

    // Clone B: abort_rebase (NOT discard_unpushed!)
    repo_b.abort_rebase().unwrap();

    // KEY: bob's local commit is preserved
    let final_content = std::fs::read_to_string(&thread_b).unwrap();
    assert!(final_content.contains("bob msg"), "local commit must survive abort_rebase");

    // KEY: unpushed commits still detected — next cycle will use push path
    assert!(repo_b.has_unpushed_commits().unwrap(), "unpushed commit should still exist");
}
```

- [ ] **Step 2: Run sync e2e tests**

Run: `cargo test -p gitim-sync --test sync_e2e_test`
Expected: all pass (including existing tests + 2 new tests)

- [ ] **Step 3: Run full workspace test**

Run: `cargo test`
Expected: all 236+ tests pass

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-sync/tests/sync_e2e_test.rs
git commit -m "test(sync): add e2e tests for fetch+rebase pull-only path

Two new tests:
- test_pull_only_via_fetch_rebase: verifies fetch+rebase pulls remote
  changes correctly (equivalent to existing pull_rebase test)
- test_pull_only_abort_rebase_preserves_racing_commit: verifies the
  key safety property — a local commit written during the pull-only
  window survives abort_rebase and is picked up by the next push cycle"
```
