//! Epoch rotation: fire (orphan + redirect + atomic push arbitration),
//! follow (origin-authoritative, multi-hop), fence (pre-push guard) and
//! migrate (rebase --onto). Protocol invariants + race matrix:
//! docs/plans/git-history-snapshot-pack/03-phase-b-v2-design.md

use crate::git::{GitError, GitStorage};
use gitim_core::epoch::{EpochFile, EpochStatus};
use std::path::Path;
use std::sync::Mutex;

pub const EPOCH_FILE: &str = "gitim.epoch.yaml";
/// Multi-hop follow guard (design scenario 6). 32 is unreachable in
/// practice — it exists to turn a metadata cycle into an error, not a hang.
pub const MAX_FOLLOW_HOPS: u32 = 32;
/// Subject prefix of the seal (redirect) commit. Shared by the producer
/// (`seal_commit_message`) and `cleanup_failed_fire`'s self-produced-commit
/// checker — the zero-loss refusal logic keys on this exact prefix.
pub const SEAL_SUBJECT_PREFIX: &str = "seal: redirect";

/// The seal commit's message. Single producer so the checker prefix above
/// can never drift from what fire actually writes.
fn seal_commit_message(current_epoch: u32, new_branch: &str, orphan_short: &str) -> String {
    format!("{SEAL_SUBJECT_PREFIX} epoch {current_epoch} -> {new_branch}@{orphan_short}")
}

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
/// Zero-loss guard (review I3): reset only when everything ahead of origin
/// is rotation-self-produced. A foreign commit in that range means messages
/// would die — leave the residue in place (the push fence keeps it
/// unpublished; delayed, never lost) and let a human look.
pub fn cleanup_failed_fire(
    storage: &GitStorage,
    commit_lock: &Mutex<()>,
    old_branch: &str,
    orphan_branch: &str,
) -> Result<(), RotationError> {
    if storage.current_branch()? != old_branch {
        return Err(RotationError::Epoch(format!(
            "cleanup branch changed before guarded reset: expected {old_branch}"
        )));
    }
    crate::skill::guard::SkillSyncGuard::new(storage.root())
        .map_err(|error| RotationError::Epoch(error.to_string()))?
        .guarded_integrate(
            storage,
            commit_lock,
            crate::skill::guard::IntegrationOperation::CleanupFailedFire {
                orphan_branch: orphan_branch.to_owned(),
            },
        )
        .map_err(|error| RotationError::Epoch(error.to_string()))?;
    Ok(())
}

/// Attempt to fire an epoch rotation.
pub fn try_fire_rotation(
    storage: &GitStorage,
    commit_lock: &Mutex<()>,
    current_branch: &str,
    threshold: u64,
    archive_dir: &Path,
    author: (&str, &str),
    created_at: &str,
) -> Result<RotationOutcome, RotationError> {
    let skill_guard = crate::skill::guard::SkillSyncGuard::new(storage.root())
        .map_err(|error| RotationError::Epoch(error.to_string()))?;
    skill_guard
        .quarantine_resolved()
        .map_err(|error| RotationError::Epoch(error.to_string()))?;
    storage.fetch()?;
    let origin_ref = format!("origin/{current_branch}");
    let fetched_origin_oid = storage.rev_parse(&origin_ref)?;
    if matches!(
        epoch_file_at_ref(storage, &fetched_origin_oid)?,
        Some(file) if file.status == EpochStatus::Redirected
    ) {
        return Ok(RotationOutcome::Lost);
    }
    skill_guard
        .rotation_allowed(storage)
        .map_err(|error| RotationError::Epoch(error.to_string()))?;
    let captured_origin_oid = storage.rev_parse(&origin_ref)?;
    let origin_epoch = epoch_file_at_ref(storage, &captured_origin_oid)?;
    if matches!(&origin_epoch, Some(f) if f.status == EpochStatus::Redirected) {
        return Ok(RotationOutcome::Lost);
    }

    let mutation_guard = commit_lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    skill_guard
        .quarantine_resolved()
        .map_err(|error| RotationError::Epoch(error.to_string()))?;
    if storage.rev_parse(&origin_ref)? != captured_origin_oid {
        return Ok(RotationOutcome::Lost);
    }
    // Zero-loss guard (review I3): the Lost path resets hard onto origin, so
    // fire may only proceed from a clean local == origin state. Any backlog
    // (messages committed between push-success and our lock acquisition)
    // defers rotation to the next push.
    //
    // Probed against `origin/<branch>..<branch>` rather than `@{upstream}`:
    // fire-created epoch branches (update-ref + checkout -f) carry no
    // upstream config until sync_loop's first `push -u`, but their
    // remote-tracking ref always exists (the atomic push that created the
    // branch updates it) — so this works on the second rotation too.
    if !storage.subjects_ahead_of_origin(current_branch)?.is_empty() {
        return Ok(RotationOutcome::NotReady);
    }

    // Zero-loss guard (review R-I2): a dirty tracked file is a deferred
    // send — send.rs leaves the message on disk when its `git commit`
    // fails, for sync_loop to commit later. That content exists nowhere
    // but this working tree, and Won's `checkout -f` / Lost's
    // `reset --hard` would destroy it permanently. Defer rotation; the
    // next handler/sync activity commits it and a later push re-fires.
    if storage.has_dirty_tracked_files()? {
        return Ok(RotationOutcome::NotReady);
    }

    let n = storage.count_commits_on_branch(current_branch)?;
    if n < threshold {
        return Ok(RotationOutcome::NotReady);
    }

    let current_epoch = origin_epoch.as_ref().map(|f| f.epoch).unwrap_or(1);
    let new_epoch = current_epoch + 1;
    let new_branch = format!("main-epoch-{new_epoch}");

    let sealed_commit_sha = storage.rev_parse(current_branch)?.trim().to_string();
    let sealed_short = sealed_commit_sha.get(..7).unwrap_or(&sealed_commit_sha);
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
    let orphan_short = orphan_commit_sha.get(..7).unwrap_or(&orphan_commit_sha);
    storage.write_redirect_commit(
        EPOCH_FILE,
        &redirect_yaml,
        &seal_commit_message(current_epoch, &new_branch, orphan_short),
        author,
    )?;
    let redirect_commit_sha = storage.rev_parse(current_branch)?;
    drop(mutation_guard);

    match storage.atomic_push_two_refs_exact(
        current_branch,
        &redirect_commit_sha,
        &new_branch,
        &orphan_commit_sha,
    ) {
        Ok(()) => {
            let local_guard = commit_lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let local_old_tip = storage.rev_parse(&format!("refs/heads/{current_branch}"))?;
            if local_old_tip == redirect_commit_sha {
                storage.checkout_branch(&new_branch)?;
            }
            // The orphan branch was born via update-ref and carries no
            // upstream config; run_sync_cycle's TOP-of-cycle `@{upstream}`
            // probe bails the whole cycle on an upstream-less branch —
            // before sync_with_push's `push -u` could ever run — so there
            // is NO downstream self-heal: a persistent failure here wedges
            // publishing. Hence the retry and the loud error. Failure
            // still returns Won — the rotation is already durable on
            // origin, and an Err would misreport it as failed.
            if let Err(first) = storage.set_upstream_to_origin(&new_branch) {
                tracing::warn!(
                    "rotation: set upstream for {new_branch} failed ({first}); retrying once"
                );
                if let Err(second) = storage.set_upstream_to_origin(&new_branch) {
                    tracing::error!(
                        "rotation: set upstream for {new_branch} failed twice ({second}); \
                         sync cycles will bail at the @{{upstream}} probe — publishing is \
                         wedged until daemon restart or a manual \
                         `git branch --set-upstream-to=origin/{new_branch} {new_branch}`"
                    );
                }
            }
            let tag_result = storage.tag_archive(&archive_tag, &sealed_commit_sha);
            drop(local_guard);
            // Best-effort archive: tag + push + bundle. Failure warns, never
            // blocks — the rotation itself is already durable on origin.
            if let Err(e) = tag_result {
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
            cleanup_failed_fire(storage, commit_lock, current_branch, &new_branch)?;
            Ok(RotationOutcome::Lost)
        }
        Err(e) => {
            // Auth / rate-limit / network (review C1 follow-through): nobody
            // won; restore the clean state and let a later push retry.
            cleanup_failed_fire(storage, commit_lock, current_branch, &new_branch)?;
            Err(RotationError::Git(e))
        }
    }
}

/// Walk the redirect chain from `start_branch` (reading each
/// `origin/<b>:gitim.epoch.yaml`) until an active/absent epoch file.
/// Returns the final branch. Errors after MAX_FOLLOW_HOPS (cycle guard).
pub fn resolve_active_branch(
    storage: &GitStorage,
    start_branch: &str,
) -> Result<String, RotationError> {
    let mut branch = start_branch.to_string();
    for _ in 0..MAX_FOLLOW_HOPS {
        let origin_ref = format!("origin/{branch}");
        match epoch_file_at_ref(storage, &origin_ref)? {
            Some(f) if f.status == EpochStatus::Redirected => {
                branch = f
                    .redirect
                    .as_ref()
                    .ok_or_else(|| RotationError::Epoch("redirected but no redirect block".into()))?
                    .target_branch
                    .clone();
            }
            _ => return Ok(branch),
        }
    }
    Err(RotationError::Epoch(format!(
        "redirect chain exceeded {MAX_FOLLOW_HOPS} hops from {start_branch}"
    )))
}

/// True iff HEAD's committed tree carries a redirected epoch.yaml — i.e. a
/// redirect commit R is in the local chain. O(1) and complete: R writes the
/// redirected yaml and no message commit ever touches that file (invariant 1).
pub fn check_push_fence(storage: &GitStorage) -> Result<bool, RotationError> {
    Ok(matches!(
        epoch_status_at_ref(storage, "HEAD")?,
        Some(EpochStatus::Redirected)
    ))
}

/// Transplant unpushed commits of `from_branch` onto `target_branch`
/// (design migrate, scenarios 3/4). Precondition: HEAD must be attached to
/// `from_branch`'s tip — the rebase transplants `origin/<from>..HEAD`.
/// The snapshot carries the full tree, so thread appends apply cleanly; a
/// conflict surfaces as Err (mid-rebase state included — the caller is
/// responsible for aborting) and the caller falls back to sync_loop's
/// capture-and-replay.
pub fn migrate_unpushed(
    storage: &GitStorage,
    from_branch: &str,
    target_branch: &str,
) -> Result<(), RotationError> {
    let origin_from = format!("origin/{from_branch}");
    let origin_target = format!("origin/{target_branch}");
    storage.rebase_onto(&origin_target, &origin_from)?;
    Ok(())
}

/// Follow a redirect published on origin: resolve the final active branch
/// (multi-hop), carry any unpushed local commits over, and switch the
/// checkout. Caller must hold `commit_lock`. Returns true if a switch
/// happened. Decisions read origin state only (invariant 3) — a no-op when
/// origin says active, regardless of local residue.
///
/// # Errors
/// On Err the switch did not happen: HEAD remains on `current_branch` with
/// all local commits intact (a conflicted migrate rebase is aborted back
/// before propagating), so the caller can simply retry next cycle.
///
/// Dirty tracked files at follow time (a mid-write or deferred send) are an
/// accepted residual risk window: unlike fire, follow cannot defer
/// indefinitely — the old branch is sealed — so the `checkout -f` switch
/// applies over them. Do not auto-commit here.
pub fn follow_redirect(storage: &GitStorage, current_branch: &str) -> Result<bool, RotationError> {
    storage.fetch()?;
    let target = resolve_active_branch(storage, current_branch)?;
    if target == current_branch {
        return Ok(false);
    }

    // Zero-loss: probe against `origin/<branch>..<branch>` (epoch branches
    // may lack upstream config, see try_fire_rotation), and propagate errors
    // rather than guessing — a swallowed "false" here would skip migrate and
    // the final origin-align below would orphan the unpushed commits. The
    // tracking ref provably resolves: resolve_active_branch just read
    // `origin/<current_branch>` to decide we're redirected.
    let has_unpushed = !storage.subjects_ahead_of_origin(current_branch)?.is_empty();

    // Make the target branch exist locally, tracking origin.
    storage.create_or_repoint_branch(&target)?;
    // Bind upstream explicitly — `branch -f` only sets it when git's
    // ambient branch.autoSetupMerge config allows, and sync_loop's
    // `@{upstream}` probes wedge permanently on an upstream-less branch.
    // Unlike the Won arm this propagates: nothing durable happened yet
    // (we are still on the old branch), so an Err is an honest "follow
    // didn't happen" and the next cycle retries the whole switch. That
    // retry guarantee is also why this runs BEFORE the checkout — once
    // HEAD sits on `target`, a re-entered follow no-ops at
    // `target == current_branch` and would never repair the upstream.
    storage.set_upstream_to_origin(&target)?;

    if has_unpushed {
        // HEAD is on current_branch; transplant <origin/current>..HEAD onto
        // origin/target. The rebase drags the current branch's ref along to
        // the migrated tip (HEAD stays attached to it) — stamp the target
        // branch there before checkout; the origin-align below then puts the
        // old branch ref back where origin says it belongs.
        if let Err(e) = migrate_unpushed(storage, current_branch, &target) {
            // Err contract (review R-I3): a conflicted rebase leaves
            // `.git/rebase-merge` + detached HEAD — abort restores the
            // pre-rebase state (HEAD on current_branch, commits intact)
            // so the caller can retry cleanly. If even the abort fails,
            // sync_loop's top-of-cycle stale-rebase recovery is the net.
            if let Err(abort_err) = storage.abort_rebase() {
                tracing::error!("follow: abort after failed migrate also failed: {abort_err}");
            }
            return Err(e);
        }
        storage.repoint_branch_to_head(&target)?;
    }
    storage.checkout_branch(&target)?;
    tracing::info!("follow: switched {current_branch} -> {target}");

    // The old local branch may still carry pre-redirect commits; align it to
    // origin (ref-only — we must stay on `target`) so nothing resurrects
    // content onto the sealed branch.
    if let Err(e) = storage.reset_to_origin_without_checkout(current_branch) {
        tracing::warn!("follow: aligning {current_branch} to origin failed (non-fatal): {e}");
    }

    Ok(true)
}

pub(crate) fn follow_redirect_exact(
    storage: &GitStorage,
    current_branch: &str,
    current_upstream_oid: &str,
    target_branch: &str,
    target_oid: &str,
    expected_target_oid: Option<&str>,
) -> Result<bool, RotationError> {
    if target_branch == current_branch {
        return Ok(false);
    }

    let has_unpushed = !storage
        .subjects_ahead_of(current_branch, current_upstream_oid)?
        .is_empty();
    storage.create_or_repoint_branch_to_exact(target_branch, target_oid, expected_target_oid)?;
    storage.set_upstream_to_origin(target_branch)?;

    let old_branch_after_migrate;
    if has_unpushed {
        if let Err(error) = storage.rebase_onto(target_oid, current_upstream_oid) {
            if let Err(abort_error) = storage.abort_rebase() {
                tracing::error!(
                    "follow: abort after failed exact migrate also failed: {abort_error}"
                );
            }
            return Err(error.into());
        }
        old_branch_after_migrate = storage.rev_parse("HEAD")?;
        storage.create_or_repoint_branch_to_exact(
            target_branch,
            &old_branch_after_migrate,
            Some(target_oid),
        )?;
    } else {
        old_branch_after_migrate = storage.rev_parse("HEAD")?;
    }
    storage.checkout_branch(target_branch)?;
    storage.reset_without_checkout_to_exact(
        current_branch,
        current_upstream_oid,
        &old_branch_after_migrate,
    )?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_message_matches_cleanup_checker_prefix() {
        // cleanup_failed_fire's refusal logic keys on this prefix; the
        // producer and checker must never drift apart.
        let msg = seal_commit_message(1, "main-epoch-2", "abc1234");
        assert!(
            msg.starts_with(SEAL_SUBJECT_PREFIX),
            "producer message {msg:?} must start with checker prefix {SEAL_SUBJECT_PREFIX:?}"
        );
    }
}
