use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;
use thiserror::Error;

use crate::url_redact::redacted_url;

/// Process-level timeout for all git subprocess invocations.
/// Prevents `Command::output()` from blocking indefinitely when git hangs
/// (e.g. disk full, credential prompt, NFS stall, lock contention).
const GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

const GIT_HTTP_TIMEOUT_ARGS: &[&str] = &[
    "-c",
    "http.lowSpeedLimit=1000",
    "-c",
    "http.lowSpeedTime=10",
];

#[derive(Error, Debug)]
pub enum GitError {
    #[error("git command failed: {0}")]
    CommandFailed(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("push rejected: remote has diverged")]
    PushConflict,
    #[error("rate limited by remote")]
    RateLimited,
    #[error("authentication failed: {0}")]
    AuthFailed(String),
    /// HEAD is detached from any branch. Operations that resolve
    /// `@{upstream}` (push, fetch, rev-list, diff against upstream)
    /// cannot proceed until HEAD is reattached.
    #[error("HEAD is detached: not pointing to a branch")]
    DetachedHead,
    /// Git subprocess did not finish within the process-level timeout.
    /// The child process has been killed.
    #[error("git command timed out after {0:?}")]
    Timeout(Duration),
    /// Disk-full condition detected in git output (ENOSPC / No space left on device).
    #[error("disk full: {0}")]
    DiskFull(String),
}

/// Run a git subprocess with a process-level timeout.
///
/// Spawns `git` as a child process and waits up to `GIT_COMMAND_TIMEOUT` for
/// it to finish. If the deadline expires, the child is killed and
/// `GitError::Timeout` is returned. On success, stderr is checked for
/// ENOSPC patterns before returning the output.
fn run_git_command(args: &[&str], current_dir: &Path) -> Result<Output, GitError> {
    run_git_command_with_env(args, current_dir, &[])
}

/// Like `run_git_command`, but with caller-supplied environment overrides
/// (`GIT_AUTHOR_*` / `GIT_COMMITTER_*`). Every git subprocess in this file —
/// env-ful or not — must route through here so all share the timeout + kill
/// plumbing.
fn run_git_command_with_env(
    args: &[&str],
    current_dir: &Path,
    envs: &[(&str, &str)],
) -> Result<Output, GitError> {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(current_dir)
        // stderr classification throughout this file (auth / rate-limit /
        // disk-full / path-missing matchers) depends on untranslated git
        // messages — pin the locale so gettext-localized gits don't break it.
        .env("LC_ALL", "C")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let child = cmd.spawn()?;

    // Wait with timeout using a thread + channel.
    // We keep the Child's pid so we can kill it on timeout.
    let pid = child.id();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });

    match rx.recv_timeout(GIT_COMMAND_TIMEOUT) {
        Ok(Ok(output)) => {
            // Check for disk-full even on "success" — git sometimes exits 0
            // with ENOSPC warnings in stderr, and some commands (e.g. fetch)
            // may partially succeed.
            let stderr_str = String::from_utf8_lossy(&output.stderr).to_string();
            if is_disk_full(&stderr_str) {
                return Err(GitError::DiskFull(stderr_str));
            }
            Ok(output)
        }
        Ok(Err(e)) => Err(GitError::Io(e)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            // Child process is hung — try to kill it.
            // SAFETY: pid belongs to our child process. On Unix, pids are
            // recycled slowly, and the window between spawn and timeout
            // (120s) makes a pid recycle race practically impossible.
            #[cfg(unix)]
            {
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGKILL);
                }
            }
            #[cfg(not(unix))]
            {
                // On non-Unix, try the portable kill. This is best-effort;
                // the child will be reaped when it eventually exits.
                let _ = Command::new("kill").args([&pid.to_string()]).output();
            }
            Err(GitError::Timeout(GIT_COMMAND_TIMEOUT))
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            // Thread panicked before sending — treat as I/O error.
            Err(GitError::CommandFailed(
                "git subprocess thread panicked".to_string(),
            ))
        }
    }
}

/// Run a git subprocess with timeout, returning `Result<Output, GitError>`
/// where non-zero exit is converted to `CommandFailed` (with ENOSPC check).
fn run_git(args: &[&str], current_dir: &Path) -> Result<Output, GitError> {
    run_git_with_env(args, current_dir, &[])
}

/// Env-accepting sibling of `run_git`: same non-zero-exit mapping, with
/// caller-supplied environment overrides.
fn run_git_with_env(
    args: &[&str],
    current_dir: &Path,
    envs: &[(&str, &str)],
) -> Result<Output, GitError> {
    let output = run_git_command_with_env(args, current_dir, envs)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if is_disk_full(&stderr) {
            return Err(GitError::DiskFull(stderr));
        }
        return Err(GitError::CommandFailed(stderr));
    }
    Ok(output)
}

/// Run a git subprocess with timeout, for best-effort calls that discard
/// the result. Returns the output if the command succeeded within the
/// timeout, or `None` on any error (timeout, non-zero exit, I/O).
fn run_git_best_effort(args: &[&str], current_dir: &Path) -> Option<Output> {
    run_git_command(args, current_dir).ok().and_then(|o| {
        if o.status.success() {
            Some(o)
        } else {
            None
        }
    })
}

fn is_disk_full(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("no space left on device")
        || lower.contains("enospc")
        || lower.contains("cannot write: no space left on device")
        || lower.contains("disk full")
}

#[derive(Clone)]
pub struct GitStorage {
    root: PathBuf,
}

impl GitStorage {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn pull_rebase(&self) -> Result<(), GitError> {
        let args = [
            GIT_HTTP_TIMEOUT_ARGS[0],
            GIT_HTTP_TIMEOUT_ARGS[1],
            GIT_HTTP_TIMEOUT_ARGS[2],
            GIT_HTTP_TIMEOUT_ARGS[3],
            "pull",
            "--rebase",
        ];
        let output = run_git_command(&args, &self.root)?;
        if !output.status.success() {
            return Err(classify_remote_error(&String::from_utf8_lossy(
                &output.stderr,
            )));
        }
        Ok(())
    }

    pub fn add_and_commit(&self, paths: &[&str], message: &str) -> Result<(), GitError> {
        self.add_and_commit_as(paths, message, None)
    }

    /// `author` is `Option<(name, email)>`. `None` → git picks author from
    /// local git config (committer == author); `Some` → `name <email>`
    /// becomes the `author` line, committer still comes from git config.
    pub fn add_and_commit_as(
        &self,
        paths: &[&str],
        message: &str,
        author: Option<(&str, &str)>,
    ) -> Result<(), GitError> {
        let mut args = vec!["add"];
        args.extend(paths);
        run_git(&args, &self.root)?;

        let mut commit_args = vec!["commit", "-m", message];
        let author_str;
        if let Some((name, email)) = author {
            author_str = format!("{} <{}>", name, email);
            commit_args.push("--author");
            commit_args.push(&author_str);
        }
        run_git(&commit_args, &self.root)?;
        Ok(())
    }

    pub fn add_and_commit_only_as(
        &self,
        path: &str,
        message: &str,
        author: Option<(&str, &str)>,
    ) -> Result<String, GitError> {
        run_git(&["add", "--", path], &self.root)?;

        let mut commit_args = vec!["commit", "--only", "-m", message];
        let author_str;
        if let Some((name, email)) = author {
            author_str = format!("{} <{}>", name, email);
            commit_args.push("--author");
            commit_args.push(&author_str);
        }
        commit_args.push("--");
        commit_args.push(path);

        run_git(&commit_args, &self.root)?;

        self.rev_parse("HEAD")
    }

    pub fn push(&self) -> Result<(), GitError> {
        let args = [
            GIT_HTTP_TIMEOUT_ARGS[0],
            GIT_HTTP_TIMEOUT_ARGS[1],
            GIT_HTTP_TIMEOUT_ARGS[2],
            GIT_HTTP_TIMEOUT_ARGS[3],
            "push",
            "-u",
            "origin",
            "HEAD",
        ];
        let output = run_git_command(&args, &self.root)?;
        if !output.status.success() {
            return Err(classify_remote_error(&String::from_utf8_lossy(
                &output.stderr,
            )));
        }
        Ok(())
    }

    pub fn has_remote(&self) -> bool {
        run_git_best_effort(&["remote", "get-url", "origin"], &self.root).is_some()
    }

    pub fn fetch(&self) -> Result<(), GitError> {
        let args = [
            GIT_HTTP_TIMEOUT_ARGS[0],
            GIT_HTTP_TIMEOUT_ARGS[1],
            GIT_HTTP_TIMEOUT_ARGS[2],
            GIT_HTTP_TIMEOUT_ARGS[3],
            "fetch",
            "origin",
        ];
        let output = run_git_command(&args, &self.root)?;
        if !output.status.success() {
            return Err(classify_remote_error(&String::from_utf8_lossy(
                &output.stderr,
            )));
        }
        Ok(())
    }

    pub fn has_unpushed_commits(&self) -> Result<bool, GitError> {
        // `@{upstream}` requires HEAD to be on a branch (it's per-branch
        // config). Probe HEAD first via `symbolic-ref --quiet` so the caller
        // gets a typed DetachedHead error instead of a generic CommandFailed
        // — sync_cycle uses this to decide whether to auto-recover vs. bail.
        if !self.head_is_on_branch()? {
            return Err(GitError::DetachedHead);
        }

        let output = run_git(&["rev-list", "--count", "@{upstream}..HEAD"], &self.root)?;
        let count: u64 = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .unwrap_or(0);
        Ok(count > 0)
    }

    /// Returns `(behind, ahead)`: how many commits HEAD is behind / ahead of
    /// `@{upstream}`. Computed against the local remote-tracking ref, so the
    /// caller must `fetch()` first for the numbers to reflect the actual
    /// remote state.
    ///
    /// Used by sync_loop's divergence safety net: when an untracked working
    /// tree file collides with an incoming tracked file, `git rebase` refuses
    /// to checkout and the pull-only path has no recovery — behind grows
    /// every cycle. Crossing a threshold triggers a hard reset to upstream.
    pub fn divergence_from_upstream(&self) -> Result<(u64, u64), GitError> {
        if !self.head_is_on_branch()? {
            return Err(GitError::DetachedHead);
        }
        let output = run_git(
            &["rev-list", "--left-right", "--count", "@{upstream}...HEAD"],
            &self.root,
        )?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut parts = stdout.split_whitespace();
        let behind: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let ahead: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        Ok((behind, ahead))
    }

    /// True iff `.git/HEAD` resolves to a symbolic ref (i.e. a branch),
    /// false on detached HEAD. Used as a precondition probe for any
    /// `@{upstream}`-dependent operation.
    pub fn head_is_on_branch(&self) -> Result<bool, GitError> {
        let output = run_git_command(&["symbolic-ref", "--quiet", "HEAD"], &self.root)?;
        // `--quiet` makes detached HEAD exit non-zero with no output and no
        // stderr. We treat that specifically as detached; any other
        // non-zero exit is a real error.
        if output.status.success() {
            return Ok(true);
        }
        if output.stderr.is_empty() && output.stdout.is_empty() {
            return Ok(false);
        }
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if is_disk_full(&stderr) {
            return Err(GitError::DiskFull(stderr));
        }
        Err(GitError::CommandFailed(stderr))
    }

    pub fn rev_parse(&self, reference: &str) -> Result<String, GitError> {
        let output = run_git(&["rev-parse", reference], &self.root)?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Return the number of commits reachable from `branch` head.
    /// Equivalent to `git rev-list --count <branch>`. Missing or unborn branch
    /// (no commits yet) → `Err`. The trailing `--` separator forces git to
    /// treat `branch` as a revision, not a flag, so a caller-side mistake
    /// like `--foo` produces a clean "bad revision" error rather than git's
    /// multi-line usage screen.
    pub fn count_commits_on_branch(&self, branch: &str) -> Result<u64, GitError> {
        let output = run_git(&["rev-list", "--count", branch, "--"], &self.root)?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let trimmed = stdout.trim();
        trimmed
            .parse::<u64>()
            .map_err(|e| GitError::CommandFailed(format!("parse count {trimmed:?}: {e}")))
    }

    pub fn diff_range(&self, from: &str, to: &str) -> Result<HashMap<PathBuf, String>, GitError> {
        let range = format!("{}..{}", from, to);
        // `--no-renames` is load-bearing: `git mv` (how we archive channels
        // and cards) produces a pure rename that git happily reports as
        // `rename from/to` with no `---`/`+++` headers — which parse_diff_output
        // would silently skip. Forcing rename decomposition turns every
        // archival into a delete + add pair, and the new path's full
        // content lands in the returned map.
        let output = run_git(&["diff", "--no-renames", &range], &self.root)?;
        Ok(Self::parse_diff_output(&String::from_utf8_lossy(
            &output.stdout,
        )))
    }

    pub fn changed_files_range(&self, from: &str, to: &str) -> Result<Vec<PathBuf>, GitError> {
        let range = format!("{}..{}", from, to);
        let output = run_git(&["diff", "--name-only", "--no-renames", &range], &self.root)?;
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .collect())
    }

    pub fn diff_unpushed(&self, pattern: &str) -> Result<HashMap<PathBuf, String>, GitError> {
        let output = run_git(
            &["diff", "--no-renames", "@{upstream}..HEAD", "--", pattern],
            &self.root,
        )?;
        Ok(Self::parse_diff_output(&String::from_utf8_lossy(
            &output.stdout,
        )))
    }

    fn parse_diff_output(stdout: &str) -> HashMap<PathBuf, String> {
        let mut result: HashMap<PathBuf, String> = HashMap::new();
        let mut current_path: Option<PathBuf> = None;
        let mut prev_was_minus_header = false;

        for line in stdout.lines() {
            if line.starts_with("--- a/") || line == "--- /dev/null" {
                prev_was_minus_header = true;
                continue;
            }
            if let Some(path_str) = line.strip_prefix("+++ b/") {
                if prev_was_minus_header {
                    current_path = Some(PathBuf::from(path_str));
                }
                prev_was_minus_header = false;
            } else if line.starts_with('+') && !line.starts_with("+++") {
                prev_was_minus_header = false;
                if let Some(ref path) = current_path {
                    let added_line = &line[1..];
                    let entry = result.entry(path.clone()).or_default();
                    if !entry.is_empty() {
                        entry.push('\n');
                    }
                    entry.push_str(added_line);
                }
            } else {
                prev_was_minus_header = false;
            }
        }

        result
    }

    pub fn rebase_onto_origin(&self) -> Result<(), GitError> {
        run_git(&["rebase", "@{upstream}"], &self.root)?;
        Ok(())
    }

    /// List files changed between upstream and HEAD, matching a pattern.
    /// Returns relative paths (e.g. "channels/general.meta.yaml").
    pub fn changed_files_unpushed(&self, pattern: &str) -> Result<Vec<PathBuf>, GitError> {
        let output = run_git(
            &["diff", "--name-only", "@{upstream}..HEAD", "--", pattern],
            &self.root,
        )?;
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(PathBuf::from)
            .collect())
    }

    /// List ALL files changed between upstream and HEAD (no pathspec filter).
    /// Used by sync_loop's rebase-conflict path to detect local edits that
    /// fall outside the resolvable set (`*.thread`, `*.meta.yaml`,
    /// `showboards/*/board.md`) so they aren't silently destroyed by a
    /// `git reset --hard @{upstream}`.
    pub fn changed_files_unpushed_all(&self) -> Result<Vec<PathBuf>, GitError> {
        let output = run_git(&["diff", "--name-only", "@{upstream}..HEAD"], &self.root)?;
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(PathBuf::from)
            .collect())
    }

    pub fn changed_files_since_merge_base(&self, pattern: &str) -> Result<Vec<PathBuf>, GitError> {
        let merge_base_output = run_git(&["merge-base", "@{upstream}", "HEAD"], &self.root)?;
        let merge_base = String::from_utf8_lossy(&merge_base_output.stdout)
            .trim()
            .to_string();
        let range = format!("{}..HEAD", merge_base);
        let output = run_git(
            &["diff", "--name-only", "--no-renames", &range, "--", pattern],
            &self.root,
        )?;
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(PathBuf::from)
            .collect())
    }

    pub fn mv(&self, from: &str, to: &str) -> Result<(), GitError> {
        run_git(&["mv", from, to], &self.root)?;
        Ok(())
    }

    /// Discard all unpushed local changes, reset to upstream state.
    /// Encapsulates rebase_abort + reset_hard_upstream.
    pub fn discard_unpushed(&self) -> Result<(), GitError> {
        // Best-effort abort any in-progress rebase
        let _ = run_git_best_effort(&["rebase", "--abort"], &self.root);

        // Reset to upstream state
        run_git(&["reset", "--hard", "@{upstream}"], &self.root)?;
        Ok(())
    }

    /// `git reset --hard <rev>` — restore a previously snapshotted state.
    /// The undo half of capture-discard-replay flows: when a step between
    /// the discard and the re-apply fails, the caller rolls HEAD back here
    /// so "any failure mode = delay, never loss" keeps holding.
    pub fn reset_hard_to(&self, rev: &str) -> Result<(), GitError> {
        run_git(&["reset", "--hard", rev], &self.root).map(|_| ())
    }

    /// Abort any in-progress rebase and verify the on-disk markers are gone.
    ///
    /// Idempotent: returns Ok when no rebase exists. Returns Err when rebase
    /// state (`.git/rebase-merge` or `.git/rebase-apply`) persists after the
    /// abort attempt — callers rely on Ok meaning "HEAD is back on a branch
    /// and `@{upstream}` is usable again", which is false while those
    /// directories remain.
    pub fn abort_rebase(&self) -> Result<(), GitError> {
        let _ = run_git_best_effort(&["rebase", "--abort"], &self.root);

        if self.has_stale_rebase_state() {
            return Err(GitError::CommandFailed(
                "rebase state persisted after abort".to_string(),
            ));
        }
        Ok(())
    }

    /// True when the working tree has leftover rebase markers (`.git/rebase-merge`
    /// or `.git/rebase-apply`). Used both by `abort_rebase`'s post-check and by
    /// the sync loop's top-of-cycle stale-rebase recovery probe.
    pub fn has_stale_rebase_state(&self) -> bool {
        self.root.join(".git/rebase-merge").exists() || self.root.join(".git/rebase-apply").exists()
    }

    /// Best-effort recovery from a wedged rebase / detached HEAD state.
    /// Idempotent: safe to call when nothing is wrong (returns Ok with no
    /// side effects).
    ///
    /// Sequence:
    ///   1. `git rebase --abort` (verified by Layer A — Ok means markers gone)
    ///   2. If markers persist (abort couldn't recognise it), force-remove
    ///      `.git/rebase-merge` / `.git/rebase-apply` directly
    ///   3. If HEAD is still detached, reattach by force-creating the local
    ///      default branch at the current SHA — preserves any unpushed work
    ///      that lived on the detached commit
    pub fn recover_from_stale_rebase(&self) -> Result<(), GitError> {
        let _ = self.abort_rebase();

        if self.has_stale_rebase_state() {
            let merge_dir = self.root.join(".git/rebase-merge");
            if merge_dir.exists() {
                std::fs::remove_dir_all(&merge_dir)?;
            }
            let apply_dir = self.root.join(".git/rebase-apply");
            if apply_dir.exists() {
                std::fs::remove_dir_all(&apply_dir)?;
            }
        }

        if !self.head_is_on_branch()? {
            let branch = self.default_branch_from_origin_head()?;
            run_git(&["checkout", "-B", &branch, "HEAD"], &self.root)?;
        }

        Ok(())
    }

    /// Resolve the default branch name via `refs/remotes/origin/HEAD`.
    /// Used by stale-rebase recovery to know where to reattach. Returns
    /// `CommandFailed` if origin/HEAD is unset (caller must decide whether
    /// to escalate or pick a fallback).
    fn default_branch_from_origin_head(&self) -> Result<String, GitError> {
        let output = run_git_command(&["symbolic-ref", "refs/remotes/origin/HEAD"], &self.root)?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if is_disk_full(&stderr) {
                return Err(GitError::DiskFull(stderr));
            }
            return Err(GitError::CommandFailed(format!(
                "origin/HEAD not set: {}",
                stderr
            )));
        }
        let full = String::from_utf8_lossy(&output.stdout).trim().to_string();
        full.strip_prefix("refs/remotes/origin/")
            .map(str::to_string)
            .ok_or_else(|| GitError::CommandFailed(format!("unexpected origin/HEAD: {full}")))
    }

    /// Create an orphan commit on `new_branch` whose tree is the current
    /// working-tree HEAD's tree, with `epoch_yaml_path` overwritten by
    /// `epoch_yaml_content`. Returns the new commit's SHA.
    ///
    /// Caller must hold the workspace `commit_lock` to serialize index writes.
    ///
    /// The branch ref is created or clobbered if pre-existing (`update-ref`
    /// semantics). The clobber is load-bearing: a refused cleanup (foreign
    /// commits ahead of origin) deliberately leaves a stale orphan ref
    /// behind, and the next fire attempt overwrites it here.
    pub fn create_orphan_commit(
        &self,
        new_branch: &str,
        epoch_yaml_path: &str,
        epoch_yaml_content: &str,
        message: &str,
        author: (&str, &str),
    ) -> Result<String, GitError> {
        // 1. Write epoch.yaml content to working tree.
        let yaml_path = self.root.join(epoch_yaml_path);
        std::fs::write(&yaml_path, epoch_yaml_content)
            .map_err(|e| GitError::CommandFailed(format!("write epoch.yaml: {e}")))?;

        // 2. Stage.
        run_git(&["add", epoch_yaml_path], &self.root)?;

        // 3. Write-tree.
        let tree = self.run_git_capture(&["write-tree"])?;
        let tree = tree.trim().to_string();

        // 4. Commit-tree (orphan — no -p flag).
        let (name, email) = author;
        let commit = self.run_git_capture_with_env(
            &["commit-tree", &tree, "-m", message],
            &[
                ("GIT_AUTHOR_NAME", name),
                ("GIT_AUTHOR_EMAIL", email),
                ("GIT_COMMITTER_NAME", name),
                ("GIT_COMMITTER_EMAIL", email),
            ],
        )?;
        let commit = commit.trim().to_string();

        // 5. Update ref.
        run_git(
            &["update-ref", &format!("refs/heads/{new_branch}"), &commit],
            &self.root,
        )?;

        // 6. Reset index back to HEAD so the OLD branch's working tree is clean
        //    (the OLD branch will get its own redirect commit separately).
        run_git(&["reset", "--mixed", "HEAD"], &self.root)?;
        // If the file existed in HEAD, `checkout` restores it. If it did NOT exist,
        // `checkout` fails (path not in HEAD's tree) and we must remove the
        // orphan-only file we placed in the working tree at step 1.
        match run_git(&["checkout", "HEAD", "--", epoch_yaml_path], &self.root) {
            Ok(_) => {
                // File restored from HEAD — leave it.
            }
            Err(_) => {
                // File was not in HEAD — delete our working-tree copy.
                let _ = std::fs::remove_file(&yaml_path);
            }
        }

        Ok(commit)
    }

    /// Append a single commit to the current branch that overwrites
    /// `epoch_yaml_path` with `epoch_yaml_content`. Returns new commit SHA.
    ///
    /// Caller must hold the workspace `commit_lock` to serialize index writes.
    pub fn write_redirect_commit(
        &self,
        epoch_yaml_path: &str,
        epoch_yaml_content: &str,
        message: &str,
        author: (&str, &str),
    ) -> Result<String, GitError> {
        let yaml_path = self.root.join(epoch_yaml_path);
        std::fs::write(&yaml_path, epoch_yaml_content)
            .map_err(|e| GitError::CommandFailed(format!("write epoch.yaml: {e}")))?;
        run_git(&["add", epoch_yaml_path], &self.root)?;

        // `--only -- <path>`: the redirect commit must structurally contain
        // nothing but the epoch.yaml flip (protocol invariant 1), even if
        // unrelated changes happen to be staged at rotation time.
        let (name, email) = author;
        self.run_git_capture_with_env(
            &["commit", "--only", "-m", message, "--", epoch_yaml_path],
            &[
                ("GIT_AUTHOR_NAME", name),
                ("GIT_AUTHOR_EMAIL", email),
                ("GIT_COMMITTER_NAME", name),
                ("GIT_COMMITTER_EMAIL", email),
            ],
        )?;
        let sha = self.run_git_capture(&["rev-parse", "HEAD"])?;
        Ok(sha.trim().to_string())
    }

    /// Return the current branch name (`git symbolic-ref --short HEAD`).
    /// Returns `GitError::DetachedHead` if HEAD is not on a branch — caller
    /// must handle this case before assuming a branch name is available.
    pub fn current_branch(&self) -> Result<String, GitError> {
        if !self.head_is_on_branch()? {
            return Err(GitError::DetachedHead);
        }
        let out = self.run_git_capture(&["symbolic-ref", "--short", "HEAD"])?;
        Ok(out.trim().to_string())
    }

    /// `git show <ref>:<path>` — read a file's committed content without
    /// touching the working tree. Returns Ok(None) when the path does not
    /// exist at that ref (including when the ref itself is unborn); other
    /// failures map to GitError.
    pub fn show_file_at_ref(
        &self,
        reference: &str,
        path: &str,
    ) -> Result<Option<String>, GitError> {
        let spec = format!("{reference}:{path}");
        match run_git(&["show", &spec], &self.root) {
            Ok(out) => Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned())),
            // Path-missing classification, per git's stderr wording:
            //   "fatal: path 'x' does not exist in 'REF'"
            //   "fatal: path 'x' exists on disk, but not in 'REF'"
            //   "fatal: invalid object name 'REF'"   (ref not born yet)
            Err(GitError::CommandFailed(stderr))
                if stderr.contains("does not exist")
                    || stderr.contains("exists on disk, but not in")
                    || stderr.contains("invalid object name") =>
            {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// `git push --atomic origin <new>:refs/heads/<new> <old>:refs/heads/<old>`.
    /// Both refs update or neither does — this is the rotation arbiter.
    /// A reject classifies through `classify_remote_error` like every other
    /// remote op (stderr embeds the credential-bearing remote URL, so it must
    /// be redacted before entering the error value): non-fast-forward →
    /// `PushConflict`, the caller's "lost the rotation race" signal.
    pub fn atomic_push_two_refs(&self, old_branch: &str, new_branch: &str) -> Result<(), GitError> {
        let new_spec = format!("{new_branch}:refs/heads/{new_branch}");
        let old_spec = format!("{old_branch}:refs/heads/{old_branch}");
        let args = [
            GIT_HTTP_TIMEOUT_ARGS[0],
            GIT_HTTP_TIMEOUT_ARGS[1],
            GIT_HTTP_TIMEOUT_ARGS[2],
            GIT_HTTP_TIMEOUT_ARGS[3],
            "push",
            "--atomic",
            "origin",
            &new_spec,
            &old_spec,
        ];
        let output = run_git_command(&args, &self.root)?;
        if !output.status.success() {
            return Err(classify_remote_error(&String::from_utf8_lossy(
                &output.stderr,
            )));
        }
        Ok(())
    }

    /// `git rebase --onto <new_base> <old_base>` — transplant the commits in
    /// `<old_base>..HEAD` onto `<new_base>`. The migrate primitive: snapshot
    /// carries the full tree, so thread appends apply cleanly; conflicts
    /// surface as Err and the caller falls back to capture-and-replay.
    pub fn rebase_onto(&self, new_base: &str, old_base: &str) -> Result<(), GitError> {
        run_git(&["rebase", "--onto", new_base, old_base], &self.root).map(|_| ())
    }

    /// Force-align a local branch to origin: checkout -f + reset --hard.
    /// Lost/crash cleanup primitive.
    pub fn reset_branch_to_origin(&self, branch: &str) -> Result<(), GitError> {
        run_git(&["checkout", "-f", branch], &self.root)?;
        let origin_ref = format!("origin/{branch}");
        run_git(&["reset", "--hard", &origin_ref], &self.root).map(|_| ())
    }

    pub fn delete_local_branch(&self, branch: &str) -> Result<(), GitError> {
        run_git(&["branch", "-D", branch], &self.root).map(|_| ())
    }

    pub fn checkout_branch(&self, branch: &str) -> Result<(), GitError> {
        // -f: rotation holds commit_lock. Dirty tracked state is not
        // necessarily crash residue — it can be a deferred send (send.rs
        // leaves the message on disk when its `git commit` fails). The fire
        // path refuses to rotate over it (`has_dirty_tracked_files` gate);
        // the follow path accepts a small residual window, documented on
        // `rotate::follow_redirect`.
        run_git(&["checkout", "-f", branch], &self.root).map(|_| ())
    }

    /// True when tracked files carry uncommitted changes (worktree or
    /// index): `git status --porcelain --untracked-files=no` is non-empty.
    /// A dirty tracked file is typically a deferred send (send.rs leaves
    /// the message on disk when `git commit` fails, for sync_loop to commit
    /// later) — content that exists nowhere but this working tree, which
    /// any `-f` / `--hard` operation would destroy permanently.
    pub fn has_dirty_tracked_files(&self) -> Result<bool, GitError> {
        let out = run_git(
            &["status", "--porcelain", "--untracked-files=no"],
            &self.root,
        )?;
        Ok(!out.stdout.is_empty())
    }

    pub fn tag_archive(&self, tag: &str, sha: &str) -> Result<(), GitError> {
        run_git(&["tag", tag, sha], &self.root).map(|_| ())
    }

    /// Push a tag to origin. Remote failures classify through
    /// `classify_remote_error` (credential-redacting); an already-existing
    /// tag rejects as `PushConflict`.
    pub fn push_tag(&self, tag: &str) -> Result<(), GitError> {
        let args = [
            GIT_HTTP_TIMEOUT_ARGS[0],
            GIT_HTTP_TIMEOUT_ARGS[1],
            GIT_HTTP_TIMEOUT_ARGS[2],
            GIT_HTTP_TIMEOUT_ARGS[3],
            "push",
            "origin",
            tag,
        ];
        let output = run_git_command(&args, &self.root)?;
        if !output.status.success() {
            return Err(classify_remote_error(&String::from_utf8_lossy(
                &output.stderr,
            )));
        }
        Ok(())
    }

    pub fn bundle_to_path(&self, path: &Path, reference: &str) -> Result<(), GitError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let p = path.to_string_lossy();
        run_git(&["bundle", "create", &p, reference], &self.root).map(|_| ())
    }

    /// `git branch -f <branch> origin/<branch>` — create or re-point a local
    /// branch at its origin counterpart without checkout.
    pub fn create_or_repoint_branch(&self, branch: &str) -> Result<(), GitError> {
        let origin_ref = format!("origin/{branch}");
        run_git(&["branch", "-f", branch, &origin_ref], &self.root).map(|_| ())
    }

    /// `git branch -f <branch> HEAD` — after a rebase leaves HEAD detached,
    /// stamp the branch there.
    pub fn repoint_branch_to_head(&self, branch: &str) -> Result<(), GitError> {
        run_git(&["branch", "-f", branch, "HEAD"], &self.root).map(|_| ())
    }

    /// `git branch --set-upstream-to=origin/<branch> <branch>` — make a
    /// freshly created/switched branch publishable by the `@{upstream}`
    /// probes and pushes the sync loop runs. The remote-tracking ref
    /// already exists in every rotation context (atomic push / fetch
    /// created it).
    pub fn set_upstream_to_origin(&self, branch: &str) -> Result<(), GitError> {
        let upstream = format!("origin/{branch}");
        run_git(
            &["branch", "--set-upstream-to", &upstream, branch],
            &self.root,
        )
        .map(|_| ())
    }

    /// `git update-ref refs/heads/<branch> <origin sha>` — align a NON-checked-out
    /// branch to origin without touching the working tree.
    pub fn reset_to_origin_without_checkout(&self, branch: &str) -> Result<(), GitError> {
        let origin_sha = self.rev_parse(&format!("origin/{branch}"))?;
        let refname = format!("refs/heads/{branch}");
        run_git(&["update-ref", &refname, &origin_sha], &self.root).map(|_| ())
    }

    /// Subjects of commits in `origin/<branch>..<branch>` (oldest first).
    /// Empty when the branch is in sync with origin. Cleanup logic gates
    /// `reset --hard` on what these unpushed commits actually are.
    pub fn subjects_ahead_of_origin(&self, branch: &str) -> Result<Vec<String>, GitError> {
        let range = format!("origin/{branch}..{branch}");
        let out = run_git(&["log", "--reverse", "--format=%s", &range], &self.root)?;
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect())
    }

    pub(crate) fn run_git_capture(&self, args: &[&str]) -> Result<String, GitError> {
        let output = run_git(args, &self.root)?;
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn run_git_capture_with_env(
        &self,
        args: &[&str],
        envs: &[(&str, &str)],
    ) -> Result<String, GitError> {
        let output = run_git_with_env(args, &self.root, envs)?;
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

fn is_rate_limited(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("rate limit")
        || lower.contains("too many requests")
        || lower.contains("429")
        || lower.contains("secondaryratelimit")
}

fn is_auth_failed(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("authentication failed")
        || lower.contains("invalid username or password")
        || lower.contains("invalid username or token")
        || lower.contains("could not read username")
        || lower.contains("could not read password")
        || lower.contains("http 401")
        || lower.contains("http 403")
        || lower.contains("error: 401")
        || lower.contains("error: 403")
        || lower.contains("permission denied (publickey)")
        || lower.contains("bad credentials")
}

/// Classify git push/fetch stderr into a structured error. Rate-limit takes
/// precedence over auth (HTTP 429 from an authed request shouldn't look like
/// credential decay), and divergence is push-only but harmless to detect for fetch.
/// Credentials are redacted before the stderr enters the error value — anything
/// that exits this function is safe to log.
pub(crate) fn classify_remote_error(raw_stderr: &str) -> GitError {
    let stderr = redacted_url(raw_stderr);
    if is_disk_full(&stderr) {
        return GitError::DiskFull(stderr);
    }
    if is_rate_limited(&stderr) {
        return GitError::RateLimited;
    }
    if is_auth_failed(&stderr) {
        return GitError::AuthFailed(stderr);
    }
    if stderr.contains("rejected") || stderr.contains("non-fast-forward") {
        return GitError::PushConflict;
    }
    GitError::CommandFailed(stderr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_detection_matches_known_patterns() {
        assert!(is_rate_limited(
            "fatal: unable to access '...': The requested URL returned error: 429"
        ));
        assert!(is_rate_limited(
            "fatal: rate limit exceeded for this endpoint"
        ));
        assert!(is_rate_limited("Rate Limit Exceeded"));
        assert!(is_rate_limited("Too Many Requests"));
        assert!(is_rate_limited("SecondaryRateLimit"));
    }

    #[test]
    fn rate_limit_detection_no_false_positives() {
        assert!(!is_rate_limited("fatal: authentication failed"));
        assert!(!is_rate_limited("error: failed to push some refs"));
        assert!(!is_rate_limited(
            "[rejected] main -> main (non-fast-forward)"
        ));
        assert!(!is_rate_limited(""));
    }

    #[test]
    fn auth_failed_detection_matches_known_patterns() {
        assert!(is_auth_failed(
            "fatal: Authentication failed for 'https://github.com/x/y.git/'"
        ));
        assert!(is_auth_failed(
            "remote: Invalid username or token. Password authentication is not supported"
        ));
        assert!(is_auth_failed(
            "fatal: could not read Username for 'https://github.com': terminal prompts disabled"
        ));
        assert!(is_auth_failed(
            "fatal: could not read Password for 'https://x@gitlab.com'"
        ));
        assert!(is_auth_failed(
            "error: The requested URL returned error: 401"
        ));
        assert!(is_auth_failed(
            "error: The requested URL returned error: 403"
        ));
        assert!(is_auth_failed("fatal: unable to access '...': HTTP 401"));
        assert!(is_auth_failed(
            "git@github.com: Permission denied (publickey)."
        ));
        assert!(is_auth_failed("remote: Bad credentials"));
        assert!(is_auth_failed("remote: invalid username or password"));
    }

    #[test]
    fn auth_failed_detection_no_false_positives() {
        assert!(!is_auth_failed(""));
        assert!(!is_auth_failed("fatal: rate limit exceeded"));
        assert!(!is_auth_failed(
            "[rejected] main -> main (non-fast-forward)"
        ));
        assert!(!is_auth_failed(
            "fatal: '/tmp/missing.git' does not appear to be a git repository"
        ));
    }

    #[test]
    fn classify_remote_error_prioritizes_rate_limit_over_auth() {
        let stderr = "HTTP 429 rate limit exceeded, auth token invalid";
        assert!(matches!(
            classify_remote_error(stderr),
            GitError::RateLimited
        ));
    }

    #[test]
    fn classify_remote_error_auth_case() {
        let stderr = "fatal: Authentication failed for 'https://github.com/x/y.git/'";
        match classify_remote_error(stderr) {
            GitError::AuthFailed(msg) => assert!(msg.contains("Authentication failed")),
            other => panic!("expected AuthFailed, got {:?}", other),
        }
    }

    #[test]
    fn classify_remote_error_redacts_credentials_in_stderr() {
        let stderr = "fatal: Authentication failed for 'https://x:secrettoken@github.com/x/y.git/'";
        match classify_remote_error(stderr) {
            GitError::AuthFailed(msg) => {
                assert!(
                    !msg.contains("secrettoken"),
                    "token should be redacted: {}",
                    msg
                );
                assert!(msg.contains("<REDACTED>"));
            }
            other => panic!("expected AuthFailed, got {:?}", other),
        }
    }

    #[test]
    fn classify_remote_error_push_conflict_case() {
        let stderr = "! [rejected]        main -> main (non-fast-forward)";
        assert!(matches!(
            classify_remote_error(stderr),
            GitError::PushConflict
        ));
    }

    #[test]
    fn classify_remote_error_falls_through_to_command_failed() {
        let stderr = "fatal: '/tmp/nope' does not appear to be a git repository";
        assert!(matches!(
            classify_remote_error(stderr),
            GitError::CommandFailed(_)
        ));
    }

    #[test]
    fn remote_git_operations_use_low_speed_timeout() {
        assert_eq!(
            GIT_HTTP_TIMEOUT_ARGS,
            [
                "-c",
                "http.lowSpeedLimit=1000",
                "-c",
                "http.lowSpeedTime=10",
            ]
        );
    }

    #[test]
    fn disk_full_detection_matches_known_patterns() {
        assert!(is_disk_full("fatal: cannot write: No space left on device"));
        assert!(is_disk_full("error: No space left on device (os error 28)"));
        assert!(is_disk_full("ENOSPC: write failed"));
        assert!(is_disk_full("disk full: cannot allocate"));
    }

    #[test]
    fn disk_full_detection_no_false_positives() {
        assert!(!is_disk_full(""));
        assert!(!is_disk_full("fatal: authentication failed"));
        assert!(!is_disk_full("error: failed to push some refs"));
    }

    #[test]
    fn classify_remote_error_prioritizes_disk_full_over_rate_limit() {
        let stderr = "No space left on device (rate limit also hit)";
        assert!(matches!(
            classify_remote_error(stderr),
            GitError::DiskFull(_)
        ));
    }

    #[test]
    fn git_command_timeout_is_120_seconds() {
        assert_eq!(GIT_COMMAND_TIMEOUT, Duration::from_secs(120));
    }

    #[test]
    fn abort_rebase_is_safe_when_no_rebase_in_progress() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let repo = GitStorage::new(dir.path());
        repo.abort_rebase().unwrap();
    }

    #[test]
    fn has_unpushed_commits_returns_detached_head_error_when_detached() {
        let bare_dir = tempfile::TempDir::new().unwrap();
        let clone_dir = tempfile::TempDir::new().unwrap();

        std::process::Command::new("git")
            .args(["init", "--bare"])
            .current_dir(bare_dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args([
                "clone",
                bare_dir.path().to_str().unwrap(),
                clone_dir.path().to_str().unwrap(),
            ])
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "a@test.com"])
            .current_dir(clone_dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "A"])
            .current_dir(clone_dir.path())
            .output()
            .unwrap();
        std::fs::write(clone_dir.path().join("seed.txt"), "seed").unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(clone_dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "seed"])
            .current_dir(clone_dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["push", "-u", "origin", "HEAD"])
            .current_dir(clone_dir.path())
            .output()
            .unwrap();

        // Detach HEAD by checking out the current commit by SHA. Mirrors
        // the production failure: rebase mid-flight leaves HEAD pointing
        // at a commit rather than refs/heads/main, and every `@{upstream}`
        // lookup then errors with "HEAD does not point to a branch".
        let sha_out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(clone_dir.path())
            .output()
            .unwrap();
        let sha = String::from_utf8_lossy(&sha_out.stdout).trim().to_string();
        std::process::Command::new("git")
            .args(["checkout", &sha])
            .current_dir(clone_dir.path())
            .output()
            .unwrap();

        let repo = GitStorage::new(clone_dir.path());
        let result = repo.has_unpushed_commits();
        assert!(
            matches!(result, Err(GitError::DetachedHead)),
            "expected DetachedHead, got {:?}",
            result
        );
    }

    #[test]
    fn recover_from_stale_rebase_clears_orphan_state_dir() {
        // Stale state that `git rebase --abort` cannot recognise as a
        // real rebase: a `.git/rebase-merge` directory left over from a
        // killed daemon mid-cleanup. abort_rebase returns Err (Layer A);
        // recover_from_stale_rebase must finish the job by force-removing
        // the dir.
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        // Seed a commit so HEAD resolves to something — otherwise
        // symbolic-ref on a freshly-init repo is its own edge case.
        std::process::Command::new("git")
            .args(["config", "user.email", "t@test.com"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "T"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::fs::write(dir.path().join("seed"), "seed").unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "seed"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        std::fs::create_dir_all(dir.path().join(".git/rebase-merge")).unwrap();
        let repo = GitStorage::new(dir.path());
        assert!(repo.has_stale_rebase_state());

        repo.recover_from_stale_rebase().unwrap();
        assert!(
            !repo.has_stale_rebase_state(),
            "stale rebase dir must be gone after recovery"
        );
        assert!(
            repo.head_is_on_branch().unwrap(),
            "HEAD must be on a branch after recovery"
        );
    }

    #[test]
    fn recover_from_stale_rebase_reattaches_detached_head() {
        let bare_dir = tempfile::TempDir::new().unwrap();
        let clone_dir = tempfile::TempDir::new().unwrap();

        std::process::Command::new("git")
            .args(["init", "--bare", "-b", "main"])
            .current_dir(bare_dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args([
                "clone",
                bare_dir.path().to_str().unwrap(),
                clone_dir.path().to_str().unwrap(),
            ])
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "t@test.com"])
            .current_dir(clone_dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "T"])
            .current_dir(clone_dir.path())
            .output()
            .unwrap();
        std::fs::write(clone_dir.path().join("seed"), "seed").unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(clone_dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "seed"])
            .current_dir(clone_dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["push", "-u", "origin", "main"])
            .current_dir(clone_dir.path())
            .output()
            .unwrap();
        // Production clones from a GitHub remote get origin/HEAD set
        // automatically; a `git init --bare` test fixture does not, so
        // do it explicitly to match the real-world precondition.
        std::process::Command::new("git")
            .args(["remote", "set-head", "origin", "main"])
            .current_dir(clone_dir.path())
            .output()
            .unwrap();

        // Add one more local commit so we can verify recovery preserves
        // unpushed work.
        std::fs::write(clone_dir.path().join("local"), "local").unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(clone_dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "local-only"])
            .current_dir(clone_dir.path())
            .output()
            .unwrap();
        let pre_sha = String::from_utf8_lossy(
            &std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(clone_dir.path())
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_string();

        // Detach + simulate stale rebase state.
        std::process::Command::new("git")
            .args(["checkout", &pre_sha])
            .current_dir(clone_dir.path())
            .output()
            .unwrap();
        std::fs::create_dir_all(clone_dir.path().join(".git/rebase-merge")).unwrap();

        let repo = GitStorage::new(clone_dir.path());
        repo.recover_from_stale_rebase().unwrap();

        assert!(!repo.has_stale_rebase_state());
        assert!(
            repo.head_is_on_branch().unwrap(),
            "HEAD must be re-attached"
        );
        let post_sha = repo.rev_parse("HEAD").unwrap();
        assert_eq!(post_sha, pre_sha, "recovery must preserve current SHA");
        assert!(
            repo.has_unpushed_commits().unwrap(),
            "unpushed local commit must survive recovery"
        );
    }

    #[test]
    fn abort_rebase_returns_err_when_state_persists() {
        // The motivating case: a real rebase got into a state where
        // `git rebase --abort` cannot clean it up (file lock, partial fs
        // failure, daemon killed mid-cleanup). We simulate this by creating
        // a `.git/rebase-merge` directory that git won't recognise as a
        // valid rebase to abort — git will report "No rebase in progress"
        // and leave the directory in place. The contract: abort_rebase MUST
        // refuse to claim success while the on-disk rebase markers remain,
        // because every downstream caller treats Ok as "you can safely use
        // @{upstream} again" and that's false here.
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        let rebase_dir = dir.path().join(".git/rebase-merge");
        std::fs::create_dir_all(&rebase_dir).unwrap();

        let repo = GitStorage::new(dir.path());
        let result = repo.abort_rebase();
        assert!(
            result.is_err(),
            "abort_rebase must Err when rebase state persists on disk"
        );
    }

    #[test]
    fn abort_rebase_preserves_local_commit() {
        let bare_dir = tempfile::TempDir::new().unwrap();
        let clone_a = tempfile::TempDir::new().unwrap();
        let clone_b = tempfile::TempDir::new().unwrap();

        std::process::Command::new("git")
            .args(["init", "--bare"])
            .current_dir(bare_dir.path())
            .output()
            .unwrap();

        std::process::Command::new("git")
            .args([
                "clone",
                bare_dir.path().to_str().unwrap(),
                clone_a.path().to_str().unwrap(),
            ])
            .current_dir(bare_dir.path().parent().unwrap())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "a@test.com"])
            .current_dir(clone_a.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "A"])
            .current_dir(clone_a.path())
            .output()
            .unwrap();

        std::fs::write(clone_a.path().join("init.txt"), "init").unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(clone_a.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(clone_a.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["push", "-u", "origin", "HEAD"])
            .current_dir(clone_a.path())
            .output()
            .unwrap();

        std::process::Command::new("git")
            .args([
                "clone",
                bare_dir.path().to_str().unwrap(),
                clone_b.path().to_str().unwrap(),
            ])
            .current_dir(bare_dir.path().parent().unwrap())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "b@test.com"])
            .current_dir(clone_b.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "B"])
            .current_dir(clone_b.path())
            .output()
            .unwrap();

        std::fs::write(clone_a.path().join("init.txt"), "A's version").unwrap();
        std::process::Command::new("git")
            .args(["add", "init.txt"])
            .current_dir(clone_a.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "A change"])
            .current_dir(clone_a.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["push"])
            .current_dir(clone_a.path())
            .output()
            .unwrap();

        std::fs::write(clone_b.path().join("init.txt"), "B's version").unwrap();
        std::process::Command::new("git")
            .args(["add", "init.txt"])
            .current_dir(clone_b.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "B change"])
            .current_dir(clone_b.path())
            .output()
            .unwrap();

        let repo_b = GitStorage::new(clone_b.path());
        repo_b.fetch().unwrap();
        let result = repo_b.rebase_onto_origin();
        assert!(result.is_err(), "rebase should fail due to conflict");

        repo_b.abort_rebase().unwrap();

        let content = std::fs::read_to_string(clone_b.path().join("init.txt")).unwrap();
        assert_eq!(
            content, "B's version",
            "local commit should be preserved after abort_rebase"
        );

        let rebase_merge = clone_b.path().join(".git/rebase-merge");
        let rebase_apply = clone_b.path().join(".git/rebase-apply");
        assert!(
            !rebase_merge.exists() && !rebase_apply.exists(),
            "repo should be clean after abort"
        );

        assert!(
            repo_b.has_unpushed_commits().unwrap(),
            "local commit should still be unpushed"
        );
    }

    use crate::test_util::{commit_file, configure_git_identity, seed_bare_with_clone};

    #[test]
    fn divergence_reports_zero_when_in_sync() {
        let (_bare, clone_a) = seed_bare_with_clone("A", "a@test.com");
        let repo = GitStorage::new(clone_a.path());
        assert_eq!(repo.divergence_from_upstream().unwrap(), (0, 0));
    }

    #[test]
    fn divergence_reports_behind_after_remote_advances() {
        let (bare, clone_a) = seed_bare_with_clone("A", "a@test.com");

        // Second clone pushes 3 new commits to the same remote.
        let clone_b = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args([
                "clone",
                bare.path().to_str().unwrap(),
                clone_b.path().to_str().unwrap(),
            ])
            .output()
            .unwrap();
        configure_git_identity(clone_b.path(), "B", "b@test.com");
        for i in 1..=3 {
            commit_file(
                clone_b.path(),
                &format!("b-{i}.txt"),
                "x",
                &format!("B commit {i}"),
            );
        }
        std::process::Command::new("git")
            .args(["push"])
            .current_dir(clone_b.path())
            .output()
            .unwrap();

        // Clone A: before fetch, divergence still reads stale (0, 0).
        let repo_a = GitStorage::new(clone_a.path());
        assert_eq!(repo_a.divergence_from_upstream().unwrap(), (0, 0));

        // After fetch, behind reflects the 3 new commits on remote.
        repo_a.fetch().unwrap();
        assert_eq!(repo_a.divergence_from_upstream().unwrap(), (3, 0));
    }

    #[test]
    fn divergence_reports_both_when_diverged() {
        let (bare, clone_a) = seed_bare_with_clone("A", "a@test.com");

        // B pushes 4 commits.
        let clone_b = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args([
                "clone",
                bare.path().to_str().unwrap(),
                clone_b.path().to_str().unwrap(),
            ])
            .output()
            .unwrap();
        configure_git_identity(clone_b.path(), "B", "b@test.com");
        for i in 1..=4 {
            commit_file(
                clone_b.path(),
                &format!("b-{i}.txt"),
                "x",
                &format!("B commit {i}"),
            );
        }
        std::process::Command::new("git")
            .args(["push"])
            .current_dir(clone_b.path())
            .output()
            .unwrap();

        // A makes 2 local commits without pushing.
        commit_file(clone_a.path(), "a-1.txt", "x", "A commit 1");
        commit_file(clone_a.path(), "a-2.txt", "x", "A commit 2");

        let repo_a = GitStorage::new(clone_a.path());
        repo_a.fetch().unwrap();
        let (behind, ahead) = repo_a.divergence_from_upstream().unwrap();
        assert_eq!(behind, 4, "should be 4 behind (B's pushes)");
        assert_eq!(ahead, 2, "should be 2 ahead (A's unpushed)");
    }

    #[test]
    fn divergence_returns_detached_head_error_when_detached() {
        let (_bare, clone_a) = seed_bare_with_clone("A", "a@test.com");
        let sha_out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(clone_a.path())
            .output()
            .unwrap();
        let sha = String::from_utf8_lossy(&sha_out.stdout).trim().to_string();
        std::process::Command::new("git")
            .args(["checkout", &sha])
            .current_dir(clone_a.path())
            .output()
            .unwrap();
        let repo = GitStorage::new(clone_a.path());
        assert!(matches!(
            repo.divergence_from_upstream(),
            Err(GitError::DetachedHead)
        ));
    }
}
