use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;

use crate::url_redact::redacted_url;

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
}

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
        let output = Command::new("git")
            .args(["pull", "--rebase"])
            .current_dir(&self.root)
            .output()?;
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
        let output = Command::new("git")
            .args(&args)
            .current_dir(&self.root)
            .output()?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let mut commit_args = vec!["commit", "-m", message];
        let author_str;
        if let Some((name, email)) = author {
            author_str = format!("{} <{}>", name, email);
            commit_args.push("--author");
            commit_args.push(&author_str);
        }
        let output = Command::new("git")
            .args(&commit_args)
            .current_dir(&self.root)
            .output()?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(())
    }

    pub fn add_and_commit_only_as(
        &self,
        path: &str,
        message: &str,
        author: Option<(&str, &str)>,
    ) -> Result<String, GitError> {
        let output = Command::new("git")
            .args(["add", "--", path])
            .current_dir(&self.root)
            .output()?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let mut commit_args = vec!["commit", "--only", "-m", message];
        let author_str;
        if let Some((name, email)) = author {
            author_str = format!("{} <{}>", name, email);
            commit_args.push("--author");
            commit_args.push(&author_str);
        }
        commit_args.push("--");
        commit_args.push(path);

        let output = Command::new("git")
            .args(&commit_args)
            .current_dir(&self.root)
            .output()?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        self.rev_parse("HEAD")
    }

    pub fn push(&self) -> Result<(), GitError> {
        let output = Command::new("git")
            .args(["push", "-u", "origin", "HEAD"])
            .current_dir(&self.root)
            .output()?;
        if !output.status.success() {
            return Err(classify_remote_error(&String::from_utf8_lossy(
                &output.stderr,
            )));
        }
        Ok(())
    }

    pub fn has_remote(&self) -> bool {
        Command::new("git")
            .args(["remote", "get-url", "origin"])
            .current_dir(&self.root)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    pub fn fetch(&self) -> Result<(), GitError> {
        let output = Command::new("git")
            .args(["fetch", "origin"])
            .current_dir(&self.root)
            .output()?;
        if !output.status.success() {
            return Err(classify_remote_error(&String::from_utf8_lossy(
                &output.stderr,
            )));
        }
        Ok(())
    }

    pub fn has_unpushed_commits(&self) -> Result<bool, GitError> {
        let output = Command::new("git")
            .args(["rev-list", "--count", "@{upstream}..HEAD"])
            .current_dir(&self.root)
            .output()?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        let count: u64 = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .unwrap_or(0);
        Ok(count > 0)
    }

    pub fn rev_parse(&self, reference: &str) -> Result<String, GitError> {
        let output = Command::new("git")
            .args(["rev-parse", reference])
            .current_dir(&self.root)
            .output()?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    pub fn diff_range(&self, from: &str, to: &str) -> Result<HashMap<PathBuf, String>, GitError> {
        let range = format!("{}..{}", from, to);
        // `--no-renames` is load-bearing: `git mv` (how we archive channels
        // and cards) produces a pure rename that git happily reports as
        // `rename from/to` with no `---`/`+++` headers — which parse_diff_output
        // would silently skip. Forcing rename decomposition turns every
        // archival into a delete + add pair, and the new path's full
        // content lands in the returned map.
        let output = Command::new("git")
            .args(["diff", "--no-renames", &range])
            .current_dir(&self.root)
            .output()?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(Self::parse_diff_output(&String::from_utf8_lossy(
            &output.stdout,
        )))
    }

    pub fn diff_unpushed(&self, pattern: &str) -> Result<HashMap<PathBuf, String>, GitError> {
        let output = Command::new("git")
            .args(["diff", "--no-renames", "@{upstream}..HEAD", "--", pattern])
            .current_dir(&self.root)
            .output()?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
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
        let output = Command::new("git")
            .args(["rebase", "@{upstream}"])
            .current_dir(&self.root)
            .output()?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(())
    }

    /// List files changed between upstream and HEAD, matching a pattern.
    /// Returns relative paths (e.g. "channels/general.meta.yaml").
    pub fn changed_files_unpushed(&self, pattern: &str) -> Result<Vec<PathBuf>, GitError> {
        let output = Command::new("git")
            .args(["diff", "--name-only", "@{upstream}..HEAD", "--", pattern])
            .current_dir(&self.root)
            .output()?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(PathBuf::from)
            .collect())
    }

    pub fn mv(&self, from: &str, to: &str) -> Result<(), GitError> {
        let output = Command::new("git")
            .args(["mv", from, to])
            .current_dir(&self.root)
            .output()?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(())
    }

    /// Discard all unpushed local changes, reset to upstream state.
    /// Encapsulates rebase_abort + reset_hard_upstream.
    pub fn discard_unpushed(&self) -> Result<(), GitError> {
        // Best-effort abort any in-progress rebase
        let _ = Command::new("git")
            .args(["rebase", "--abort"])
            .current_dir(&self.root)
            .output();

        // Reset to upstream state
        let output = Command::new("git")
            .args(["reset", "--hard", "@{upstream}"])
            .current_dir(&self.root)
            .output()?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(())
    }

    /// Best-effort abort any in-progress rebase. Always succeeds.
    pub fn abort_rebase(&self) -> Result<(), GitError> {
        let _ = Command::new("git")
            .args(["rebase", "--abort"])
            .current_dir(&self.root)
            .output();
        Ok(())
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
}
