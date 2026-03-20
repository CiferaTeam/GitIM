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

        let output = Command::new("git")
            .args(["commit", "-m", message])
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
            .args(["push"])
            .current_dir(&self.root)
            .output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
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
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(())
    }

    pub fn has_unpushed_commits(&self) -> Result<bool, GitError> {
        let output = Command::new("git")
            .args(["rev-list", "--count", "origin/main..HEAD"])
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

    pub fn diff_unpushed(&self, pattern: &str) -> Result<HashMap<PathBuf, String>, GitError> {
        let output = Command::new("git")
            .args(["diff", "origin/main..HEAD", "--", pattern])
            .current_dir(&self.root)
            .output()?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
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
            } else if line.starts_with("+") && !line.starts_with("+++") {
                prev_was_minus_header = false;
                if let Some(ref path) = current_path {
                    let added_line = &line[1..]; // strip leading '+'
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

        Ok(result)
    }

    pub fn rebase_onto_origin(&self) -> Result<(), GitError> {
        let output = Command::new("git")
            .args(["rebase", "origin/main"])
            .current_dir(&self.root)
            .output()?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(())
    }

    /// Discard all unpushed local changes, reset to remote state.
    /// Encapsulates rebase_abort + reset_hard_origin.
    pub fn discard_unpushed(&self) -> Result<(), GitError> {
        // Best-effort abort any in-progress rebase
        let _ = Command::new("git")
            .args(["rebase", "--abort"])
            .current_dir(&self.root)
            .output();

        // Reset to remote state
        let output = Command::new("git")
            .args(["reset", "--hard", "origin/main"])
            .current_dir(&self.root)
            .output()?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(())
    }
}
