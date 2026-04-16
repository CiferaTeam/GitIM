use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;

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
}

pub struct GitStorage {
    root: PathBuf,
}

impl GitStorage {
    pub fn new(root: &Path) -> Self {
        Self { root: root.to_path_buf() }
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
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(())
    }

    pub fn add_and_commit(&self, paths: &[&str], message: &str) -> Result<(), GitError> {
        self.add_and_commit_as(paths, message, None)
    }

    pub fn add_and_commit_as(
        &self,
        paths: &[&str],
        message: &str,
        author: Option<&str>,
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
        if let Some(handler) = author {
            author_str = format!("{} <{}@gitim>", handler, handler);
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
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            if is_rate_limited(&stderr) {
                return Err(GitError::RateLimited);
            }
            return Err(GitError::CommandFailed(stderr));
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
        let output = Command::new("git")
            .args(["diff", &range])
            .current_dir(&self.root)
            .output()?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(Self::parse_diff_output(&String::from_utf8_lossy(&output.stdout)))
    }

    pub fn diff_unpushed(&self, pattern: &str) -> Result<HashMap<PathBuf, String>, GitError> {
        let output = Command::new("git")
            .args(["diff", "@{upstream}..HEAD", "--", pattern])
            .current_dir(&self.root)
            .output()?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(Self::parse_diff_output(&String::from_utf8_lossy(&output.stdout)))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_detection_matches_known_patterns() {
        assert!(is_rate_limited("fatal: unable to access '...': The requested URL returned error: 429"));
        assert!(is_rate_limited("fatal: rate limit exceeded for this endpoint"));
        assert!(is_rate_limited("Rate Limit Exceeded"));
        assert!(is_rate_limited("Too Many Requests"));
        assert!(is_rate_limited("SecondaryRateLimit"));
    }

    #[test]
    fn rate_limit_detection_no_false_positives() {
        assert!(!is_rate_limited("fatal: authentication failed"));
        assert!(!is_rate_limited("error: failed to push some refs"));
        assert!(!is_rate_limited("[rejected] main -> main (non-fast-forward)"));
        assert!(!is_rate_limited(""));
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

        std::process::Command::new("git").args(["init", "--bare"]).current_dir(bare_dir.path()).output().unwrap();

        std::process::Command::new("git").args(["clone", bare_dir.path().to_str().unwrap(), clone_a.path().to_str().unwrap()]).current_dir(bare_dir.path().parent().unwrap()).output().unwrap();
        std::process::Command::new("git").args(["config", "user.email", "a@test.com"]).current_dir(clone_a.path()).output().unwrap();
        std::process::Command::new("git").args(["config", "user.name", "A"]).current_dir(clone_a.path()).output().unwrap();

        std::fs::write(clone_a.path().join("init.txt"), "init").unwrap();
        std::process::Command::new("git").args(["add", "."]).current_dir(clone_a.path()).output().unwrap();
        std::process::Command::new("git").args(["commit", "-m", "initial"]).current_dir(clone_a.path()).output().unwrap();
        std::process::Command::new("git").args(["push", "-u", "origin", "HEAD"]).current_dir(clone_a.path()).output().unwrap();

        std::process::Command::new("git").args(["clone", bare_dir.path().to_str().unwrap(), clone_b.path().to_str().unwrap()]).current_dir(bare_dir.path().parent().unwrap()).output().unwrap();
        std::process::Command::new("git").args(["config", "user.email", "b@test.com"]).current_dir(clone_b.path()).output().unwrap();
        std::process::Command::new("git").args(["config", "user.name", "B"]).current_dir(clone_b.path()).output().unwrap();

        std::fs::write(clone_a.path().join("init.txt"), "A's version").unwrap();
        std::process::Command::new("git").args(["add", "init.txt"]).current_dir(clone_a.path()).output().unwrap();
        std::process::Command::new("git").args(["commit", "-m", "A change"]).current_dir(clone_a.path()).output().unwrap();
        std::process::Command::new("git").args(["push"]).current_dir(clone_a.path()).output().unwrap();

        std::fs::write(clone_b.path().join("init.txt"), "B's version").unwrap();
        std::process::Command::new("git").args(["add", "init.txt"]).current_dir(clone_b.path()).output().unwrap();
        std::process::Command::new("git").args(["commit", "-m", "B change"]).current_dir(clone_b.path()).output().unwrap();

        let repo_b = GitStorage::new(clone_b.path());
        repo_b.fetch().unwrap();
        let result = repo_b.rebase_onto_origin();
        assert!(result.is_err(), "rebase should fail due to conflict");

        repo_b.abort_rebase().unwrap();

        let content = std::fs::read_to_string(clone_b.path().join("init.txt")).unwrap();
        assert_eq!(content, "B's version", "local commit should be preserved after abort_rebase");

        let rebase_merge = clone_b.path().join(".git/rebase-merge");
        let rebase_apply = clone_b.path().join(".git/rebase-apply");
        assert!(!rebase_merge.exists() && !rebase_apply.exists(), "repo should be clean after abort");

        assert!(repo_b.has_unpushed_commits().unwrap(), "local commit should still be unpushed");
    }
}
