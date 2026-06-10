//! Epoch rotation: fire (orphan + redirect + atomic push arbitration),
//! follow (origin-authoritative, multi-hop), fence (pre-push guard) and
//! migrate (rebase --onto). Protocol invariants + race matrix:
//! docs/plans/git-history-snapshot-pack/03-phase-b-v2-design.md

use crate::git::{GitError, GitStorage};
use gitim_core::epoch::{EpochFile, EpochStatus};
use std::path::Path;

pub const EPOCH_FILE: &str = "gitim.epoch.yaml";
/// Multi-hop follow guard (design scenario 6). 32 is unreachable in
/// practice — it exists to turn a metadata cycle into an error, not a hang.
pub const MAX_FOLLOW_HOPS: u32 = 32;

#[derive(Debug)]
pub enum RotationOutcome {
    NotReady,
    Won {
        sealed_branch: String,
        new_branch: String,
        new_epoch: u32,
        sealed_commit_sha: String,
        orphan_commit_sha: String,
    },
    Lost,
}

#[derive(Debug, thiserror::Error)]
pub enum RotationError {
    #[error("git: {0}")]
    Git(#[from] GitError),
    #[error("epoch: {0}")]
    Epoch(String),
}

/// Parse `gitim.epoch.yaml` as committed at `<ref>` (not the working tree —
/// design invariant 3: decisions trust origin, never local residue).
pub fn epoch_file_at_ref(
    storage: &GitStorage,
    reference: &str,
) -> Result<Option<EpochFile>, RotationError> {
    let Some(content) = storage.show_file_at_ref(reference, EPOCH_FILE)? else {
        return Ok(None);
    };
    let file: EpochFile = serde_yaml::from_str(&content)
        .map_err(|e| RotationError::Epoch(format!("parse {reference}:{EPOCH_FILE}: {e}")))?;
    file.validate()
        .map_err(|e| RotationError::Epoch(format!("validate {reference}:{EPOCH_FILE}: {e}")))?;
    Ok(Some(file))
}

/// Status-only convenience for fence checks in sync_loop.
pub fn epoch_status_at_ref(
    storage: &GitStorage,
    reference: &str,
) -> Result<Option<EpochStatus>, RotationError> {
    Ok(epoch_file_at_ref(storage, reference)?.map(|f| f.status))
}

/// Remove every local trace of a failed fire so the next cycle starts
/// clean: reset old branch onto origin, drop the never-published orphan
/// branch. Also the boot-time cleanup for crash residue (scenario 7).
///
/// Zero-loss guard (review I3): reset only when everything ahead of origin
/// is rotation-self-produced. A foreign commit in that range means messages
/// would die — leave the residue in place (the push fence keeps it
/// unpublished; delayed, never lost) and let a human look.
pub fn cleanup_failed_fire(
    storage: &GitStorage,
    old_branch: &str,
    orphan_branch: &str,
) -> Result<(), RotationError> {
    let ahead = storage.subjects_ahead_of_origin(old_branch)?;
    if ahead.iter().any(|s| !s.starts_with("seal: redirect")) {
        tracing::warn!(
            "cleanup_failed_fire: non-rotation commits ahead of origin/{old_branch} \
             ({ahead:?}); refusing to reset — residue stays fenced until resolved"
        );
        return Ok(());
    }
    storage.reset_branch_to_origin(old_branch)?;
    // Branch may not exist if we crashed before creating it — best-effort.
    let _ = storage.delete_local_branch(orphan_branch);
    Ok(())
}

/// Attempt to fire an epoch rotation. Caller must hold `commit_lock`.
pub fn try_fire_rotation(
    storage: &GitStorage,
    current_branch: &str,
    threshold: u64,
    archive_dir: &Path,
    author: (&str, &str),
    created_at: &str,
) -> Result<RotationOutcome, RotationError> {
    // Zero-loss guard (review I3): the Lost path resets hard onto origin, so
    // fire may only proceed from a clean local == origin state. Any backlog
    // (messages committed between push-success and our lock acquisition)
    // defers rotation to the next push.
    if storage.has_unpushed_commits()? {
        return Ok(RotationOutcome::NotReady);
    }

    let n = storage.count_commits_on_branch(current_branch)?;
    if n < threshold {
        return Ok(RotationOutcome::NotReady);
    }

    // Best-effort fetch so the origin checks below see fresh state. If it
    // fails (offline) the atomic push will arbitrate anyway.
    let _ = storage.fetch();

    // Invariant 3: read epoch state from origin, not the working tree.
    let origin_ref = format!("origin/{current_branch}");
    let origin_epoch = epoch_file_at_ref(storage, &origin_ref)?;
    if matches!(&origin_epoch, Some(f) if f.status == EpochStatus::Redirected) {
        // Someone already rotated this branch — we are a follower, not a firer.
        return Ok(RotationOutcome::Lost);
    }
    let current_epoch = origin_epoch.as_ref().map(|f| f.epoch).unwrap_or(1);
    let new_epoch = current_epoch + 1;
    let new_branch = format!("main-epoch-{new_epoch}");

    let sealed_commit_sha = storage.rev_parse(current_branch)?.trim().to_string();
    let sealed_short = &sealed_commit_sha[..7];
    let archive_tag = format!("archive/epoch-{current_epoch}/{sealed_short}");

    let active = EpochFile::new_active(
        new_epoch,
        new_branch.clone(),
        current_branch.to_string(),
        sealed_commit_sha.clone(),
        // snapshot.commit: v1 fills the sealed SHA (the orphan SHA doesn't
        // exist until after this YAML is committed) — precision is a
        // documented design non-goal, patched in a later phase.
        sealed_commit_sha.clone(),
        created_at.to_string(),
        None,
    );
    let active_yaml = serde_yaml::to_string(&active)
        .map_err(|e| RotationError::Epoch(format!("serialize active: {e}")))?;
    let redirect = EpochFile::new_redirect(
        current_epoch,
        current_branch.to_string(),
        new_epoch,
        new_branch.clone(),
        sealed_commit_sha.clone(),
        sealed_commit_sha.clone(),
        created_at.to_string(),
        None,
    );
    let redirect_yaml = serde_yaml::to_string(&redirect)
        .map_err(|e| RotationError::Epoch(format!("serialize redirect: {e}")))?;

    let orphan_commit_sha = storage.create_orphan_commit(
        &new_branch,
        EPOCH_FILE,
        &active_yaml,
        &format!("snapshot: open epoch {new_epoch} from {current_branch}@{sealed_short}"),
        author,
    )?;
    storage.write_redirect_commit(
        EPOCH_FILE,
        &redirect_yaml,
        &format!(
            "seal: redirect epoch {current_epoch} -> {new_branch}@{}",
            &orphan_commit_sha[..7]
        ),
        author,
    )?;

    match storage.atomic_push_two_refs(current_branch, &new_branch) {
        Ok(()) => {
            storage.checkout_branch(&new_branch)?;
            // Best-effort archive: tag + push + bundle. Failure warns, never
            // blocks — the rotation itself is already durable on origin.
            if let Err(e) = storage.tag_archive(&archive_tag, &sealed_commit_sha) {
                tracing::warn!("rotation: tag_archive failed (non-fatal): {e}");
            } else if let Err(e) = storage.push_tag(&archive_tag) {
                tracing::warn!("rotation: push_tag failed (non-fatal): {e}");
            }
            let bundle_path = archive_dir.join(format!("epoch-{current_epoch}.bundle"));
            if let Err(e) = storage.bundle_to_path(&bundle_path, &archive_tag) {
                tracing::warn!("rotation: bundle failed (non-fatal): {e}");
            }
            Ok(RotationOutcome::Won {
                sealed_branch: current_branch.to_string(),
                new_branch,
                new_epoch,
                sealed_commit_sha,
                orphan_commit_sha,
            })
        }
        Err(GitError::PushConflict) => {
            // Lost the race (to another firer OR to a plain message push —
            // design scenarios 1 and 2; we don't need to know which).
            cleanup_failed_fire(storage, current_branch, &new_branch)?;
            Ok(RotationOutcome::Lost)
        }
        Err(e) => {
            // Auth / rate-limit / network (review C1 follow-through): nobody
            // won; restore the clean state and let a later push retry.
            cleanup_failed_fire(storage, current_branch, &new_branch)?;
            Err(RotationError::Git(e))
        }
    }
}
