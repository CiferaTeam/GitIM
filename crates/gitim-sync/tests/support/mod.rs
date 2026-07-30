use gitim_sync::git::{GitError, GitStorage};

pub trait TestWorkingBranchPush {
    fn push(&self) -> Result<(), GitError>;
}

impl TestWorkingBranchPush for GitStorage {
    fn push(&self) -> Result<(), GitError> {
        let output = std::process::Command::new("git")
            .args(["push", "-u", "origin", "HEAD"])
            .current_dir(self.root())
            .output()
            .map_err(GitError::Io)?;
        output
            .status
            .success()
            .then_some(())
            .ok_or(GitError::PushConflict)
    }
}
