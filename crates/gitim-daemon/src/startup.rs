use std::path::PathBuf;

use gitim_sync::git::GitStorage;
use gitim_sync::skill::checkpoint::SkillSyncError;
use gitim_sync::skill::guard::SkillSyncGuard;
use gitim_sync::skill::transaction::{
    recover_remote_skill_transactions, RemoteSkillTransactionResult,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StartupRecoveryError {
    #[error("skill transaction recovery worker failed: {0}")]
    Worker(#[from] tokio::task::JoinError),
    #[error("skill transaction recovery failed: {0}")]
    Skill(#[from] SkillSyncError),
}

pub async fn recover_skill_transactions_before_serving(
    repo_root: PathBuf,
) -> Result<Vec<RemoteSkillTransactionResult>, StartupRecoveryError> {
    tokio::task::spawn_blocking(move || {
        let storage = GitStorage::new(&repo_root);
        if !storage.has_remote() {
            return Ok(Vec::new());
        }
        let guard = SkillSyncGuard::new(&repo_root)?;
        recover_remote_skill_transactions(&storage, &guard)
    })
    .await?
    .map_err(Into::into)
}
