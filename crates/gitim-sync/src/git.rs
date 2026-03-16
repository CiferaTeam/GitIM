use std::path::Path;
use std::process::Command;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum GitError {
    #[error("git command failed: {0}")]
    CommandFailed(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("push failed after {0} retries")]
    PushRetriesExhausted(u32),
}

pub struct GitRepo {
    root: std::path::PathBuf,
}

impl GitRepo {
    pub fn new(root: &Path) -> Self {
        Self { root: root.to_path_buf() }
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
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(())
    }

    pub fn push_with_retry(&self, max_retries: u32) -> Result<(), GitError> {
        for attempt in 0..=max_retries {
            match self.push() {
                Ok(()) => return Ok(()),
                Err(_) if attempt < max_retries => {
                    self.pull_rebase()?;
                }
                Err(_) => return Err(GitError::PushRetriesExhausted(max_retries)),
            }
        }
        Err(GitError::PushRetriesExhausted(max_retries))
    }

    pub fn has_remote(&self) -> bool {
        Command::new("git")
            .args(["remote", "get-url", "origin"])
            .current_dir(&self.root)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}
