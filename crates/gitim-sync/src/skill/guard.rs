use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use gitim_core::skill::SkillError;
use serde::{Deserialize, Serialize};

use super::checkpoint::{
    validate_incoming_skill_history, IncomingSkillValidation, SkillCheckpointStore, SkillSyncError,
    SkillValidationCheckpoint,
};
use crate::conflict;
use crate::git::{run_git, GitError, GitStorage};

const JOURNAL_SCHEMA_VERSION: u32 = 1;
const JOURNAL_FILE: &str = "skill-quarantine.json";
const QUARANTINE_REF_PREFIX: &str = "refs/gitim/quarantine/skill-";
const QUARANTINE_TAIL_REF_PREFIX: &str = "refs/gitim/quarantine/tail-";
const ROTATION_RECOVERY_FILE: &str = "rotation-recovery.json";
const ROTATION_TAIL_REF_PREFIX: &str = "refs/gitim/rotation-tail/";

#[derive(Clone)]
pub struct SkillSyncGuard {
    checkpoint: SkillCheckpointStore,
    journal_path: PathBuf,
    rotation_journal_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IntegrationOperation {
    RebaseOntoOrigin { expected_head: String },
    HardDivergenceRecovery { expected_head: String },
    FollowEpochRedirect,
    FollowEpochRedirectAfterDiscard { expected_head: String },
    CleanupFailedFire { orphan_branch: String },
}

#[derive(Debug, Eq, PartialEq)]
pub enum GuardedPushOutcome {
    Pushed,
    NothingToPush,
    RepairedAndPushed { quarantine_ref: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum QuarantinePhase {
    Prepared,
    Replayed,
    Moved,
    Completed,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum QuarantineKind {
    #[default]
    SkillHistory,
    UserArchive,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct QuarantineJournal {
    schema_version: u32,
    operation_id: String,
    branch: String,
    upstream_oid: String,
    original_head: String,
    quarantine_ref: String,
    phase: QuarantinePhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repaired_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    branch_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tail_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tail_base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tail_head: Option<String>,
    #[serde(default, skip_serializing_if = "is_skill_history_quarantine")]
    kind: QuarantineKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    excluded_commits: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    semantic_error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    replay_base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_branch_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_tail_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_tail_base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_tail_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completion_head: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RotationRecoveryPhase {
    Prepared,
    Replayed,
    Moved,
    Completed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RotationRecoveryJournal {
    schema_version: u32,
    operation_id: String,
    branch: String,
    upstream_oid: String,
    active_branch: String,
    active_oid: String,
    seal_oid: String,
    tail_ref: String,
    tail_head: String,
    orphan_branch: String,
    orphan_oid: Option<String>,
    phase: RotationRecoveryPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repaired_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prior_repaired_head: Option<String>,
}

impl SkillSyncGuard {
    pub fn new(repository_root: &Path) -> Result<Self, SkillSyncError> {
        let checkpoint = SkillCheckpointStore::new(repository_root)?;
        ensure_local_metadata_excluded(repository_root)?;
        let parent = checkpoint
            .path
            .parent()
            .ok_or_else(|| SkillSyncError::Checkpoint("checkpoint path has no parent".to_owned()))?
            .to_path_buf();
        Ok(Self {
            checkpoint,
            journal_path: parent.join(JOURNAL_FILE),
            rotation_journal_path: parent.join(ROTATION_RECOVERY_FILE),
        })
    }

    pub fn guarded_push(
        &self,
        repo: &GitStorage,
        commit_lock: &Mutex<()>,
        author: (&str, &str),
    ) -> Result<GuardedPushOutcome, SkillSyncError> {
        if !repo.has_remote() {
            return Ok(GuardedPushOutcome::NothingToPush);
        }
        self.resume_pending_recoveries(repo, commit_lock)?;
        if let Some(error) =
            self.resume_semantic_archive_recovery(repo, commit_lock, Some(author))?
        {
            return Err(error);
        }

        repo.fetch()?;
        let captured_branch = repo.current_branch()?;
        let upstream_ref = format!("origin/{captured_branch}");
        let Some(upstream_oid) = revision_oid(repo, &upstream_ref)? else {
            let head = {
                let _guard = commit_lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if repo.current_branch()? != captured_branch || self.load_journal()?.is_some() {
                    return Err(SkillSyncError::Git(GitError::PushConflict));
                }
                let head = repo.rev_parse("HEAD")?;
                if history_touches_managed_skills(repo, &head)? {
                    return Err(SkillSyncError::LocalQuarantineBlocked(
                        "cannot publish unvalidated Skill history to an empty remote".to_owned(),
                    ));
                }
                head
            };
            repo.push_working_branch_exact(&captured_branch, &head, None)?;
            return Ok(GuardedPushOutcome::Pushed);
        };
        match crate::rotate::epoch_status_at_ref(repo, &upstream_ref)
            .map_err(|error| SkillSyncError::EpochValidationBlocked(error.to_string()))?
        {
            Some(gitim_core::epoch::EpochStatus::Redirected) => {
                let active_branch = crate::rotate::resolve_active_branch(repo, &captured_branch)
                    .map_err(|error| SkillSyncError::EpochValidationBlocked(error.to_string()))?;
                let active_tip = repo.rev_parse(&format!("origin/{active_branch}"))?;
                self.validate_and_store(repo, &active_tip, &active_branch)?;
                return Err(SkillSyncError::Git(GitError::PushConflict));
            }
            Some(gitim_core::epoch::EpochStatus::Active) | None => {}
        }
        self.validate_and_store(repo, &upstream_oid, &captured_branch)?;

        enum PreparedPush {
            Nothing,
            Ordinary { head: String },
            Quarantine(PendingQuarantinePush),
            PublishedQuarantine { quarantine_ref: String },
            RecoveredUserArchive { error: SkillSyncError },
        }

        let prepared = {
            let _guard = commit_lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if repo.current_branch()? != captured_branch {
                return Err(SkillSyncError::Git(GitError::PushConflict));
            }
            ensure_ref_equals(repo, &upstream_ref, &upstream_oid)?;

            if let Some(journal) = self.load_journal()? {
                if journal.kind == QuarantineKind::UserArchive {
                    let error = semantic_archive_error(
                        journal.semantic_error_code.as_deref().ok_or_else(|| {
                            SkillSyncError::LocalQuarantineBlocked(
                                "semantic archive journal is missing its error code".to_owned(),
                            )
                        })?,
                    )?;
                    let pending =
                        self.prepare_quarantine_locked(repo, author, journal, &upstream_oid)?;
                    self.complete_semantic_archive_locked(repo, &pending)?;
                    PreparedPush::RecoveredUserArchive { error }
                } else if self.reconcile_published_quarantine_locked(
                    repo,
                    &journal,
                    &upstream_oid,
                )? {
                    PreparedPush::PublishedQuarantine {
                        quarantine_ref: journal.quarantine_ref,
                    }
                } else {
                    validate_user_archive_preconditions(
                        repo,
                        &upstream_oid,
                        &journal.original_head,
                    )?;
                    PreparedPush::Quarantine(self.prepare_quarantine_locked(
                        repo,
                        author,
                        journal,
                        &upstream_oid,
                    )?)
                }
            } else {
                let original_head = repo.rev_parse("HEAD")?;
                if let Some(violation) =
                    inspect_user_archive_preconditions(repo, &upstream_oid, &original_head)?
                {
                    let journal = QuarantineJournal::user_archive(
                        &captured_branch,
                        &upstream_oid,
                        &original_head,
                        vec![violation.commit],
                        violation.error_code,
                    )?;
                    self.save_journal(&journal)?;
                    ensure_quarantine_ref(repo, &journal)?;
                    let pending =
                        self.prepare_quarantine_locked(repo, author, journal, &upstream_oid)?;
                    self.complete_semantic_archive_locked(repo, &pending)?;
                    PreparedPush::RecoveredUserArchive {
                        error: violation.error,
                    }
                } else if history_touches_managed_skills_between(
                    repo,
                    &upstream_oid,
                    &original_head,
                )? {
                    let journal = QuarantineJournal::prepared(
                        &captured_branch,
                        &upstream_oid,
                        &original_head,
                    )?;
                    self.save_journal(&journal)?;
                    ensure_quarantine_ref(repo, &journal)?;
                    PreparedPush::Quarantine(self.prepare_quarantine_locked(
                        repo,
                        author,
                        journal,
                        &upstream_oid,
                    )?)
                } else if changed_paths(repo, &upstream_oid, &original_head)?.is_empty() {
                    PreparedPush::Nothing
                } else {
                    PreparedPush::Ordinary {
                        head: original_head,
                    }
                }
            }
        };

        match prepared {
            PreparedPush::Nothing => Ok(GuardedPushOutcome::NothingToPush),
            PreparedPush::Ordinary { head } => {
                repo.push_working_branch_exact(&captured_branch, &head, Some(&upstream_oid))?;
                Ok(GuardedPushOutcome::Pushed)
            }
            PreparedPush::Quarantine(pending) => {
                repo.push_working_branch_exact(
                    &pending.branch,
                    &pending.repaired_head,
                    Some(&upstream_oid),
                )?;
                let _guard = commit_lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                self.complete_quarantine_locked(repo, &pending)?;
                Ok(GuardedPushOutcome::RepairedAndPushed {
                    quarantine_ref: pending.quarantine_ref,
                })
            }
            PreparedPush::PublishedQuarantine { quarantine_ref } => {
                Ok(GuardedPushOutcome::RepairedAndPushed { quarantine_ref })
            }
            PreparedPush::RecoveredUserArchive { error } => Err(error),
        }
    }

    pub fn guarded_integrate(
        &self,
        repo: &GitStorage,
        commit_lock: &Mutex<()>,
        operation: IntegrationOperation,
    ) -> Result<IncomingSkillValidation, SkillSyncError> {
        if let Some(error) = self.resume_semantic_archive_recovery(repo, commit_lock, None)? {
            return Err(error);
        }
        let resumed_rotation = self.resume_pending_recoveries(repo, commit_lock)?;
        if resumed_rotation {
            if matches!(&operation, IntegrationOperation::CleanupFailedFire { .. }) {
                repo.fetch()?;
                let branch = repo.current_branch()?;
                let tip = repo.rev_parse(&format!("origin/{branch}"))?;
                return self.validate_and_store(repo, &tip, &branch);
            }
            return Err(SkillSyncError::Git(GitError::PushConflict));
        }
        let captured_branch = repo.current_branch()?;
        let captured_head = repo.rev_parse("HEAD")?;
        if let Some(expected_head) = operation.expected_head() {
            if expected_head != captured_head {
                return Err(SkillSyncError::Git(GitError::PushConflict));
            }
        }
        let captured_orphan = match &operation {
            IntegrationOperation::CleanupFailedFire { orphan_branch } => Some(CapturedLocalRef {
                reference: format!("refs/heads/{orphan_branch}"),
                oid: revision_oid(repo, &format!("refs/heads/{orphan_branch}"))?,
            }),
            _ => None,
        };
        repo.fetch()?;
        let follows_redirect = matches!(
            &operation,
            IntegrationOperation::FollowEpochRedirect
                | IntegrationOperation::FollowEpochRedirectAfterDiscard { .. }
                | IntegrationOperation::CleanupFailedFire { .. }
        );
        let captured_refs = if follows_redirect {
            capture_epoch_chain(repo, &captured_branch)?
        } else {
            vec![capture_remote_ref(repo, &captured_branch)?]
        };
        let current_upstream = captured_refs.first().ok_or_else(|| {
            SkillSyncError::EpochValidationBlocked("empty epoch ref capture".to_owned())
        })?;
        let current_upstream_oid = current_upstream.oid.clone();
        let validation_ref = captured_refs.last().ok_or_else(|| {
            SkillSyncError::EpochValidationBlocked("empty epoch ref capture".to_owned())
        })?;
        let validation_branch = validation_ref.branch.clone();
        let validated_tip = validation_ref.oid.clone();
        let captured_target = if follows_redirect {
            let reference = format!("refs/heads/{validation_branch}");
            Some(CapturedLocalRef {
                oid: revision_oid(repo, &reference)?,
                reference,
            })
        } else {
            None
        };
        let validation = self.validate_and_store(repo, &validated_tip, &validation_branch)?;

        let _guard = commit_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if repo.current_branch()? != captured_branch || repo.rev_parse("HEAD")? != captured_head {
            return Err(SkillSyncError::Git(GitError::PushConflict));
        }
        for captured in &captured_refs {
            ensure_ref_equals(repo, &captured.reference, &captured.oid)?;
        }
        if let Some(captured) = &captured_orphan {
            ensure_optional_ref_equals(repo, &captured.reference, captured.oid.as_deref())?;
        }
        if let Some(captured) = &captured_target {
            ensure_optional_ref_equals(repo, &captured.reference, captured.oid.as_deref())?;
        }
        let journal = self.load_journal()?;
        let rotation_journal = self.load_rotation_journal()?;
        if rotation_journal.is_some()
            && !matches!(&operation, IntegrationOperation::CleanupFailedFire { .. })
        {
            return Err(SkillSyncError::LocalQuarantineBlocked(
                "rotation recovery must finish before another guarded integration".to_owned(),
            ));
        }
        if journal.is_some()
            && matches!(
                &operation,
                IntegrationOperation::HardDivergenceRecovery { .. }
                    | IntegrationOperation::FollowEpochRedirect
                    | IntegrationOperation::FollowEpochRedirectAfterDiscard { .. }
                    | IntegrationOperation::CleanupFailedFire { .. }
            )
        {
            return Err(SkillSyncError::LocalQuarantineBlocked(
                "quarantine replay must finish before destructive integration".to_owned(),
            ));
        }
        if journal
            .as_ref()
            .is_some_and(|journal| journal.phase != QuarantinePhase::Moved)
        {
            return Err(SkillSyncError::LocalQuarantineBlocked(
                "quarantine replay has not moved the working branch".to_owned(),
            ));
        }

        match operation {
            IntegrationOperation::RebaseOntoOrigin { expected_head } => {
                ensure_clean_tracked_worktree(repo)?;
                if let Err(error) = repo.rebase_onto_exact(&current_upstream_oid) {
                    repo.abort_rebase()?;
                    if repo.current_branch()? != captured_branch
                        || repo.rev_parse("HEAD")? != expected_head
                    {
                        return Err(SkillSyncError::LocalQuarantineBlocked(
                            "failed rebase did not restore the expected local head".to_owned(),
                        ));
                    }
                    return Err(error.into());
                }
                if let Some(mut journal) = journal {
                    journal.upstream_oid = current_upstream_oid.clone();
                    journal.repaired_head = Some(repo.rev_parse("HEAD")?);
                    self.save_journal(&journal)?;
                }
            }
            IntegrationOperation::HardDivergenceRecovery { .. } => {
                ensure_clean_tracked_worktree(repo)?;
                repo.discard_unpushed_to(&current_upstream_oid)?;
            }
            IntegrationOperation::FollowEpochRedirect => {
                ensure_clean_tracked_worktree(repo)?;
                crate::rotate::follow_redirect_exact(
                    repo,
                    &captured_branch,
                    &current_upstream_oid,
                    &validation_branch,
                    &validated_tip,
                    captured_target
                        .as_ref()
                        .and_then(|captured| captured.oid.as_deref()),
                )
                .map_err(|error| SkillSyncError::EpochValidationBlocked(error.to_string()))?;
            }
            IntegrationOperation::FollowEpochRedirectAfterDiscard { expected_head } => {
                ensure_clean_tracked_worktree(repo)?;
                repo.discard_unpushed_to(&current_upstream_oid)?;
                let intermediate_head = repo.rev_parse("HEAD")?;
                let follow_result = crate::rotate::follow_redirect_exact(
                    repo,
                    &captured_branch,
                    &current_upstream_oid,
                    &validation_branch,
                    &validated_tip,
                    captured_target
                        .as_ref()
                        .and_then(|captured| captured.oid.as_deref()),
                );
                let followed = match follow_result {
                    Ok(followed) => followed,
                    Err(error) => {
                        if repo.current_branch()? == captured_branch
                            && repo.rev_parse("HEAD")? == intermediate_head
                        {
                            repo.reset_hard_to(&expected_head)?;
                        }
                        return Err(SkillSyncError::EpochValidationBlocked(error.to_string()));
                    }
                };
                if !followed {
                    if repo.current_branch()? == captured_branch
                        && repo.rev_parse("HEAD")? == intermediate_head
                    {
                        repo.reset_hard_to(&expected_head)?;
                    }
                    return Err(SkillSyncError::EpochValidationBlocked(
                        "epoch redirect follow was a no-op after discard".to_owned(),
                    ));
                }
            }
            IntegrationOperation::CleanupFailedFire { orphan_branch } => {
                let ahead = repo.subjects_ahead_of(&captured_branch, &current_upstream_oid)?;
                if ahead
                    .iter()
                    .any(|subject| !subject.starts_with(crate::rotate::SEAL_SUBJECT_PREFIX))
                {
                    self.recover_rotation_tail_locked(
                        repo,
                        rotation_journal,
                        &captured_branch,
                        &captured_head,
                        &current_upstream_oid,
                        &validation_branch,
                        &validated_tip,
                        &orphan_branch,
                        captured_orphan.as_ref(),
                        captured_target.as_ref(),
                    )?;
                    return Ok(validation);
                }
                ensure_clean_tracked_worktree(repo)?;
                repo.discard_unpushed_to(&current_upstream_oid)?;
                if let Some(orphan_oid) = captured_orphan
                    .as_ref()
                    .and_then(|captured| captured.oid.as_deref())
                {
                    repo.delete_local_branch_exact(&orphan_branch, orphan_oid)?;
                }
            }
        }
        Ok(validation)
    }

    pub fn rotation_allowed(&self, repo: &GitStorage) -> Result<(), SkillSyncError> {
        self.quarantine_resolved()?;
        if !repo.has_remote() {
            return Ok(());
        }
        repo.fetch()?;
        let branch = repo.current_branch()?;
        let origin = repo.rev_parse(&format!("origin/{branch}"))?;
        self.validate_and_store(repo, &origin, &branch)?;
        verify_managed_roots(repo, &origin, "HEAD")
    }

    pub fn accepted_skill_root(&self, repo: &GitStorage) -> Result<String, SkillSyncError> {
        if repo.has_remote() {
            repo.fetch()?;
        }
        let branch = repo.current_branch()?;
        let Some(upstream_oid) = revision_oid(repo, &format!("origin/{branch}"))? else {
            return Ok("absent".to_owned());
        };
        let validation = self.validate_and_store(repo, &upstream_oid, &branch)?;
        if validation.checkpoint.last_scanned_tip != upstream_oid {
            return Err(SkillSyncError::Checkpoint(
                "accepted Skill checkpoint is not bound to the current origin tip".to_owned(),
            ));
        }
        Ok(tree_oid(repo, &upstream_oid, "skills")?.unwrap_or_else(|| "absent".to_owned()))
    }

    pub(crate) fn quarantine_resolved(&self) -> Result<(), SkillSyncError> {
        if self.load_journal()?.is_some() || self.load_rotation_journal()?.is_some() {
            return Err(SkillSyncError::LocalQuarantineBlocked(
                "quarantine journal is unresolved".to_owned(),
            ));
        }
        Ok(())
    }

    pub fn quarantine_pending(&self) -> Result<bool, SkillSyncError> {
        Ok(self.load_journal()?.is_some())
    }

    fn validate_and_store(
        &self,
        repo: &GitStorage,
        fetched_tip: &str,
        active_epoch: &str,
    ) -> Result<IncomingSkillValidation, SkillSyncError> {
        self.checkpoint.with_lock(|checkpoint| {
            let previous = checkpoint
                .load()?
                .unwrap_or_else(|| SkillValidationCheckpoint::empty(active_epoch));
            let validation = validate_incoming_skill_history(repo, &previous, fetched_tip)?;
            checkpoint.save(&validation.checkpoint)?;
            if !validation.checkpoint.conflicts.is_empty() {
                return Err(SkillSyncError::Domain(SkillError::SyncConflict));
            }
            Ok(validation)
        })
    }

    fn prepare_quarantine_locked(
        &self,
        repo: &GitStorage,
        author: (&str, &str),
        mut journal: QuarantineJournal,
        captured_upstream_oid: &str,
    ) -> Result<PendingQuarantinePush, SkillSyncError> {
        validate_journal(repo, &journal)?;
        ensure_quarantine_ref(repo, &journal)?;
        ensure_current_branch(repo, &journal.branch)?;
        let upstream_ref = format!("origin/{}", journal.branch);
        ensure_ref_equals(repo, &upstream_ref, captured_upstream_oid)?;

        if journal.upstream_oid != captured_upstream_oid {
            let current = repo.rev_parse(&format!("refs/heads/{}", journal.branch))?;
            let expected = journal.expected_branch_head().to_owned();
            let previous_repaired = journal.repaired_head.clone();
            if current != expected && previous_repaired.as_deref() != Some(current.as_str()) {
                let already_captured = journal.tail_head.as_deref() == Some(current.as_str());
                if !already_captured {
                    let repaired = previous_repaired.as_deref().ok_or_else(|| {
                        SkillSyncError::LocalQuarantineBlocked(
                            "working branch changed before quarantine replay".to_owned(),
                        )
                    })?;
                    let replay_base = journal.upstream_oid.clone();
                    capture_quarantine_tail(repo, &mut journal, repaired, &replay_base, &current)?;
                    self.save_journal(&journal)?;
                    cleanup_quarantine_tail_refs(repo, &journal, journal.tail_ref.as_deref())?;
                }
            }
            journal.upstream_oid = captured_upstream_oid.to_owned();
            journal.branch_head = Some(current);
            journal.repaired_head = None;
            journal.phase = QuarantinePhase::Prepared;
            self.save_journal(&journal)?;
        }

        if journal.phase == QuarantinePhase::Prepared {
            let repaired = replay_without_managed_skills(repo, &journal, Some(author))?;
            journal.repaired_head = Some(repaired);
            journal.phase = QuarantinePhase::Replayed;
            self.save_journal(&journal)?;
        }

        if journal.phase == QuarantinePhase::Replayed {
            let repaired = journal.repaired_head.as_deref().ok_or_else(|| {
                SkillSyncError::LocalQuarantineBlocked(
                    "replayed quarantine is missing repaired head".to_owned(),
                )
            })?;
            verify_replayed_result(repo, &journal, repaired)?;
            let current = repo.rev_parse(&format!("refs/heads/{}", journal.branch))?;
            let expected = journal.expected_branch_head();
            reconcile_update_ref_only_residue(repo, &journal.branch, &current, repaired, expected)?;
            if current == expected {
                ensure_clean_tracked_worktree(repo)?;
                update_working_branch(repo, &journal.branch, repaired, expected)?;
            } else if current == repaired {
                ensure_clean_tracked_worktree(repo)?;
                repo.reset_hard_to(repaired)?;
            } else {
                return Err(SkillSyncError::LocalQuarantineBlocked(
                    "working branch changed during quarantine replay".to_owned(),
                ));
            }
            journal.phase = QuarantinePhase::Moved;
            self.save_journal(&journal)?;
        }

        let repaired = journal.repaired_head.as_deref().ok_or_else(|| {
            SkillSyncError::LocalQuarantineBlocked(
                "moved quarantine is missing repaired head".to_owned(),
            )
        })?;
        let current = repo.rev_parse(&format!("refs/heads/{}", journal.branch))?;
        if current != repaired && !is_ancestor(repo, repaired, &current)? {
            return Err(SkillSyncError::LocalQuarantineBlocked(
                "working branch no longer contains repaired quarantine head".to_owned(),
            ));
        }
        verify_managed_roots(repo, &journal.upstream_oid, repaired)?;
        Ok(PendingQuarantinePush {
            operation_id: journal.operation_id,
            branch: journal.branch,
            repaired_head: repaired.to_owned(),
            quarantine_ref: journal.quarantine_ref,
        })
    }

    fn reconcile_published_quarantine_locked(
        &self,
        repo: &GitStorage,
        journal: &QuarantineJournal,
        validated_upstream_oid: &str,
    ) -> Result<bool, SkillSyncError> {
        validate_journal(repo, journal)?;
        ensure_quarantine_ref(repo, journal)?;
        if journal.phase == QuarantinePhase::Completed {
            self.finalize_completed_quarantine(repo, journal)?;
            return Ok(true);
        }
        if journal.phase != QuarantinePhase::Moved {
            return Ok(false);
        }
        let repaired = journal.repaired_head.as_deref().ok_or_else(|| {
            SkillSyncError::LocalQuarantineBlocked(
                "moved quarantine is missing repaired head".to_owned(),
            )
        })?;
        if repaired != validated_upstream_oid
            && !is_ancestor(repo, repaired, validated_upstream_oid)?
        {
            return Ok(false);
        }

        let current = repo.rev_parse(&format!("refs/heads/{}", journal.branch))?;
        let mut cleanup_head = current.clone();
        if current != repaired && !is_ancestor(repo, repaired, &current)? {
            let expected = journal.expected_branch_head();
            if current != expected {
                return Err(SkillSyncError::Git(GitError::PushConflict));
            }
            ensure_clean_tracked_worktree(repo)?;
            update_working_branch(repo, &journal.branch, validated_upstream_oid, expected)?;
            cleanup_head = validated_upstream_oid.to_owned();
        }
        ensure_exact_checkout(repo, &journal.branch, &cleanup_head)?;
        let mut completed = journal.clone();
        self.transition_quarantine_to_completed(repo, &mut completed, &cleanup_head)?;
        Ok(true)
    }

    fn complete_quarantine_locked(
        &self,
        repo: &GitStorage,
        pending: &PendingQuarantinePush,
    ) -> Result<(), SkillSyncError> {
        match self.load_journal()? {
            Some(mut journal)
                if journal.operation_id == pending.operation_id
                    && matches!(
                        journal.phase,
                        QuarantinePhase::Moved | QuarantinePhase::Completed
                    )
                    && journal.repaired_head.as_deref() == Some(pending.repaired_head.as_str()) =>
            {
                validate_journal(repo, &journal)?;
                match journal.phase {
                    QuarantinePhase::Moved => self.transition_quarantine_to_completed(
                        repo,
                        &mut journal,
                        &pending.repaired_head,
                    ),
                    QuarantinePhase::Completed => {
                        self.finalize_completed_quarantine(repo, &journal)
                    }
                    _ => unreachable!("phase was matched above"),
                }
            }
            Some(_) => Ok(()),
            None => Ok(()),
        }
    }

    fn complete_semantic_archive_locked(
        &self,
        repo: &GitStorage,
        pending: &PendingQuarantinePush,
    ) -> Result<(), SkillSyncError> {
        let mut journal = self.load_journal()?.ok_or_else(|| {
            SkillSyncError::LocalQuarantineBlocked(
                "semantic archive recovery journal disappeared".to_owned(),
            )
        })?;
        if journal.kind != QuarantineKind::UserArchive
            || journal.operation_id != pending.operation_id
            || !matches!(
                journal.phase,
                QuarantinePhase::Moved | QuarantinePhase::Completed
            )
            || journal.repaired_head.as_deref() != Some(pending.repaired_head.as_str())
        {
            return Err(SkillSyncError::Git(GitError::PushConflict));
        }
        validate_journal(repo, &journal)?;
        match journal.phase {
            QuarantinePhase::Moved => {
                self.transition_quarantine_to_completed(repo, &mut journal, &pending.repaired_head)
            }
            QuarantinePhase::Completed => self.finalize_completed_quarantine(repo, &journal),
            _ => unreachable!("phase was matched above"),
        }
    }

    fn resume_semantic_archive_recovery(
        &self,
        repo: &GitStorage,
        commit_lock: &Mutex<()>,
        author: Option<(&str, &str)>,
    ) -> Result<Option<SkillSyncError>, SkillSyncError> {
        let Some(captured_journal) = self.load_journal()? else {
            return Ok(None);
        };
        if captured_journal.kind != QuarantineKind::UserArchive {
            return Ok(None);
        }
        if self.load_rotation_journal()?.is_some() {
            return Err(SkillSyncError::LocalQuarantineBlocked(
                "rotation recovery must finish before semantic archive recovery".to_owned(),
            ));
        }

        let captured_checkout_branch = repo.current_branch()?;
        let captured_checkout_head = repo.rev_parse("HEAD")?;
        repo.fetch()?;
        let captured_refs = capture_epoch_chain(repo, &captured_journal.branch)?;
        let original_remote = captured_refs.first().ok_or_else(|| {
            SkillSyncError::EpochValidationBlocked("empty semantic recovery epoch chain".to_owned())
        })?;
        let active_remote = captured_refs.last().ok_or_else(|| {
            SkillSyncError::EpochValidationBlocked("empty semantic recovery epoch chain".to_owned())
        })?;
        self.validate_and_store(repo, &active_remote.oid, &active_remote.branch)?;
        let captured_active_local =
            revision_oid(repo, &format!("refs/heads/{}", active_remote.branch))?;

        let _guard = commit_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for captured in &captured_refs {
            ensure_ref_equals(repo, &captured.reference, &captured.oid)?;
        }
        if self.load_rotation_journal()?.is_some() {
            return Err(SkillSyncError::LocalQuarantineBlocked(
                "rotation recovery appeared during semantic archive recovery".to_owned(),
            ));
        }
        let mut journal = self
            .load_journal()?
            .ok_or(SkillSyncError::Git(GitError::PushConflict))?;
        if journal != captured_journal {
            return Err(SkillSyncError::Git(GitError::PushConflict));
        }
        ensure_exact_checkout(repo, &captured_checkout_branch, &captured_checkout_head)?;
        validate_journal(repo, &journal)?;
        ensure_quarantine_ref(repo, &journal)?;
        ensure_semantic_recovery_branch(repo, &journal.branch, &active_remote.branch)?;
        let error =
            semantic_archive_error(journal.semantic_error_code.as_deref().ok_or_else(|| {
                SkillSyncError::LocalQuarantineBlocked(
                    "semantic archive journal is missing its error code".to_owned(),
                )
            })?)?;
        if active_remote.branch != journal.branch {
            validate_semantic_active_tail(
                repo,
                &journal,
                &active_remote.oid,
                captured_active_local.as_deref(),
            )?;
        }
        if journal.phase == QuarantinePhase::Completed {
            self.finalize_completed_quarantine(repo, &journal)?;
            return Ok(Some(error));
        }
        if journal.phase == QuarantinePhase::Replayed && captured_checkout_branch == journal.branch
        {
            let repaired = journal.repaired_head.as_deref().ok_or_else(|| {
                SkillSyncError::LocalQuarantineBlocked(
                    "semantic archive recovery is missing its repaired head".to_owned(),
                )
            })?;
            let current = repo.rev_parse(&format!("refs/heads/{}", journal.branch))?;
            reconcile_update_ref_only_residue(
                repo,
                &journal.branch,
                &current,
                repaired,
                journal.expected_branch_head(),
            )?;
        } else if journal.phase == QuarantinePhase::Replayed
            && active_remote.branch != journal.branch
            && captured_checkout_branch == active_remote.branch
            && journal.active_branch.as_deref() == Some(active_remote.branch.as_str())
        {
            let repaired = journal.repaired_head.as_deref().ok_or_else(|| {
                SkillSyncError::LocalQuarantineBlocked(
                    "semantic archive recovery is missing its repaired head".to_owned(),
                )
            })?;
            let expected = journal.active_branch_head.as_deref().ok_or_else(|| {
                SkillSyncError::LocalQuarantineBlocked(
                    "checked-out semantic destination has no expected head".to_owned(),
                )
            })?;
            let current = repo.rev_parse(&format!("refs/heads/{}", active_remote.branch))?;
            reconcile_update_ref_only_residue(
                repo,
                &active_remote.branch,
                &current,
                repaired,
                expected,
            )?;
        }
        ensure_clean_tracked_worktree(repo)?;

        if journal.upstream_oid != active_remote.oid {
            if journal.phase == QuarantinePhase::Replayed {
                let prior_repaired = journal.repaired_head.as_deref().ok_or_else(|| {
                    SkillSyncError::LocalQuarantineBlocked(
                        "semantic archive recovery is missing its repaired head".to_owned(),
                    )
                })?;
                verify_replayed_result(repo, &journal, prior_repaired)?;
                let branch_head = repo.rev_parse(&format!("refs/heads/{}", journal.branch))?;
                if branch_head != journal.expected_branch_head() && branch_head != prior_repaired {
                    return Err(SkillSyncError::Git(GitError::PushConflict));
                }
                journal.branch_head = Some(branch_head);
                journal.repaired_head = None;
                journal.phase = QuarantinePhase::Prepared;
            } else if journal.phase != QuarantinePhase::Prepared {
                return Err(SkillSyncError::Git(GitError::PushConflict));
            }
            if journal.replay_base.is_none() {
                journal.replay_base = Some(merge_base(
                    repo,
                    &journal.upstream_oid,
                    &journal.original_head,
                )?);
            }
            journal.upstream_oid = active_remote.oid.clone();
            journal.repaired_head = None;
            self.save_journal(&journal)?;
        }

        if active_remote.branch != journal.branch {
            bind_semantic_active_destination(
                repo,
                &mut journal,
                &active_remote.branch,
                &active_remote.oid,
                captured_active_local.as_deref(),
            )?;
            self.save_journal(&journal)?;
        }

        if journal.phase == QuarantinePhase::Prepared {
            let repaired = replay_without_managed_skills(repo, &journal, author)?;
            journal.repaired_head = Some(repaired);
            journal.phase = QuarantinePhase::Replayed;
            self.save_journal(&journal)?;
        }

        if journal.phase == QuarantinePhase::Replayed {
            let repaired = journal.repaired_head.as_deref().ok_or_else(|| {
                SkillSyncError::LocalQuarantineBlocked(
                    "semantic archive recovery is missing its repaired head".to_owned(),
                )
            })?;
            verify_replayed_result(repo, &journal, repaired)?;
            let expected = journal.expected_branch_head();
            if active_remote.branch == journal.branch {
                let current = repo.rev_parse(&format!("refs/heads/{}", journal.branch))?;
                if current == expected {
                    update_working_branch(repo, &journal.branch, repaired, expected)?;
                } else if current == repaired {
                    repo.reset_hard_to(repaired)?;
                } else {
                    return Err(SkillSyncError::Git(GitError::PushConflict));
                }
            } else {
                let active_ref = format!("refs/heads/{}", active_remote.branch);
                if journal.active_branch.as_deref() != Some(active_remote.branch.as_str()) {
                    return Err(SkillSyncError::Git(GitError::PushConflict));
                }
                let expected_active = journal.active_branch_head.as_deref();
                let mut active_ref_changed = false;
                match revision_oid(repo, &active_ref)? {
                    Some(oid) if oid == repaired => {}
                    Some(oid) if Some(oid.as_str()) == expected_active => {
                        repo.create_or_repoint_branch_to_exact(
                            &active_remote.branch,
                            repaired,
                            Some(&oid),
                        )?;
                        active_ref_changed = true;
                    }
                    None if expected_active.is_none() => {
                        repo.create_or_repoint_branch_to_exact(
                            &active_remote.branch,
                            repaired,
                            None,
                        )?;
                        active_ref_changed = true;
                    }
                    Some(_) | None => return Err(SkillSyncError::Git(GitError::PushConflict)),
                }
                repo.set_upstream_to_origin(&active_remote.branch)?;
                match repo.current_branch()?.as_str() {
                    branch if branch == journal.branch => {
                        repo.checkout_branch(&active_remote.branch)?;
                    }
                    branch if branch == active_remote.branch => {
                        if active_ref_changed {
                            repo.reset_hard_to(repaired)?;
                        }
                    }
                    _ => return Err(SkillSyncError::Git(GitError::PushConflict)),
                }
                let old_ref = format!("refs/heads/{}", journal.branch);
                match revision_oid(repo, &old_ref)? {
                    Some(oid) if oid == expected => {
                        repo.reset_without_checkout_to_exact(
                            &journal.branch,
                            &original_remote.oid,
                            expected,
                        )?;
                    }
                    Some(oid) if oid == original_remote.oid => {}
                    _ => return Err(SkillSyncError::Git(GitError::PushConflict)),
                }
            }
            ensure_exact_checkout(repo, &active_remote.branch, repaired)?;
            journal.phase = QuarantinePhase::Moved;
            self.save_journal(&journal)?;
        }

        if journal.phase == QuarantinePhase::Moved {
            let repaired = journal.repaired_head.clone().ok_or_else(|| {
                SkillSyncError::LocalQuarantineBlocked(
                    "moved semantic archive recovery is missing its repaired head".to_owned(),
                )
            })?;
            ensure_exact_checkout(repo, &active_remote.branch, &repaired)?;
            self.transition_quarantine_to_completed(repo, &mut journal, &repaired)?;
        }

        Ok(Some(error))
    }

    fn transition_quarantine_to_completed(
        &self,
        repo: &GitStorage,
        journal: &mut QuarantineJournal,
        completion_head: &str,
    ) -> Result<(), SkillSyncError> {
        if journal.phase != QuarantinePhase::Moved {
            return Err(SkillSyncError::LocalQuarantineBlocked(
                "only a moved quarantine can enter cleanup".to_owned(),
            ));
        }
        ensure_exact_checkout(repo, journal.completion_branch(), completion_head)?;
        journal.completion_head = Some(completion_head.to_owned());
        journal.phase = QuarantinePhase::Completed;
        self.save_journal(journal)?;
        self.finalize_completed_quarantine(repo, journal)
    }

    fn finalize_completed_quarantine(
        &self,
        repo: &GitStorage,
        journal: &QuarantineJournal,
    ) -> Result<(), SkillSyncError> {
        let completion_head = journal.completion_head.as_deref().ok_or_else(|| {
            SkillSyncError::LocalQuarantineBlocked(
                "completed quarantine is missing its cleanup head".to_owned(),
            )
        })?;
        ensure_exact_checkout(repo, journal.completion_branch(), completion_head)?;
        cleanup_quarantine_tail_refs(repo, journal, None)?;
        self.remove_journal()
    }

    #[allow(clippy::too_many_arguments)]
    fn recover_rotation_tail_locked(
        &self,
        repo: &GitStorage,
        existing: Option<RotationRecoveryJournal>,
        captured_branch: &str,
        captured_head: &str,
        upstream_oid: &str,
        active_branch: &str,
        active_oid: &str,
        orphan_branch: &str,
        captured_orphan: Option<&CapturedLocalRef>,
        captured_target: Option<&CapturedLocalRef>,
    ) -> Result<(), SkillSyncError> {
        let journal = match existing {
            Some(journal) => journal,
            None => {
                ensure_clean_tracked_worktree(repo)?;
                let seal_oid = find_rotation_seal(repo, upstream_oid, captured_head)?;
                if seal_oid == captured_head
                    || !is_ancestor(repo, &seal_oid, captured_head)?
                    || history_touches_managed_skills_between(repo, &seal_oid, captured_head)?
                    || history_touches_epoch_file_between(repo, &seal_oid, captured_head)?
                {
                    return Err(SkillSyncError::LocalQuarantineBlocked(
                        "failed rotation tail is unsafe or unrelated".to_owned(),
                    ));
                }
                verify_managed_roots(repo, &seal_oid, captured_head)?;
                let operation_id = seal_oid.clone();
                let tail_ref = format!("{ROTATION_TAIL_REF_PREFIX}{operation_id}");
                let zero_oid = "0".repeat(captured_head.len());
                match revision_oid(repo, &tail_ref)? {
                    Some(existing) if existing == captured_head => {}
                    Some(_) => {
                        return Err(SkillSyncError::LocalQuarantineBlocked(
                            "rotation tail ref collides with another recovery".to_owned(),
                        ));
                    }
                    None => {
                        run_git(
                            &["update-ref", &tail_ref, captured_head, &zero_oid],
                            repo.root(),
                        )?;
                    }
                }
                let journal = RotationRecoveryJournal {
                    schema_version: JOURNAL_SCHEMA_VERSION,
                    operation_id,
                    branch: captured_branch.to_owned(),
                    upstream_oid: upstream_oid.to_owned(),
                    active_branch: active_branch.to_owned(),
                    active_oid: active_oid.to_owned(),
                    seal_oid,
                    tail_ref,
                    tail_head: captured_head.to_owned(),
                    orphan_branch: orphan_branch.to_owned(),
                    orphan_oid: captured_orphan.and_then(|captured| captured.oid.clone()),
                    phase: RotationRecoveryPhase::Prepared,
                    repaired_head: None,
                    expected_head: None,
                    prior_repaired_head: None,
                };
                self.save_rotation_journal(&journal)?;
                journal
            }
        };

        validate_rotation_journal(repo, &journal)?;
        if journal.branch != captured_branch
            || journal.upstream_oid != upstream_oid
            || journal.active_branch != active_branch
            || journal.active_oid != active_oid
            || journal.orphan_branch != orphan_branch
        {
            return Err(SkillSyncError::Git(GitError::PushConflict));
        }
        let captured_orphan_oid = captured_orphan.and_then(|captured| captured.oid.as_deref());
        match journal.phase {
            RotationRecoveryPhase::Prepared | RotationRecoveryPhase::Replayed
                if journal.orphan_oid.as_deref() != captured_orphan_oid =>
            {
                return Err(SkillSyncError::Git(GitError::PushConflict));
            }
            RotationRecoveryPhase::Moved
                if captured_orphan_oid.is_some()
                    && journal.orphan_oid.as_deref() != captured_orphan_oid =>
            {
                return Err(SkillSyncError::Git(GitError::PushConflict));
            }
            _ => {}
        }
        if active_branch != captured_branch
            && captured_target.and_then(|captured| captured.oid.as_deref())
                != journal.orphan_oid.as_deref()
            && journal.phase != RotationRecoveryPhase::Moved
            && journal.phase != RotationRecoveryPhase::Completed
        {
            return Err(SkillSyncError::Git(GitError::PushConflict));
        }

        self.resume_rotation_journal_locked(repo, journal)
    }

    fn resume_rotation_journal_locked(
        &self,
        repo: &GitStorage,
        mut journal: RotationRecoveryJournal,
    ) -> Result<(), SkillSyncError> {
        if journal.phase == RotationRecoveryPhase::Prepared && journal.repaired_head.is_some() {
            let repaired = journal.repaired_head.as_deref().ok_or_else(|| {
                SkillSyncError::LocalQuarantineBlocked(
                    "prepared rotation recovery lost its repaired head".to_owned(),
                )
            })?;
            let repaired_is_current =
                revision_oid(repo, &format!("refs/heads/{}", journal.active_branch))?.as_deref()
                    == Some(repaired);
            if repaired_is_current {
                journal.phase = RotationRecoveryPhase::Replayed;
                self.save_rotation_journal(&journal)?;
            } else {
                journal.repaired_head = None;
                self.save_rotation_journal(&journal)?;
            }
        }

        if journal.phase == RotationRecoveryPhase::Prepared {
            ensure_clean_tracked_worktree(repo)?;
            let repaired = replay_rotation_tail(repo, &journal)?;
            journal.repaired_head = Some(repaired);
            journal.phase = RotationRecoveryPhase::Replayed;
            self.save_rotation_journal(&journal)?;
        }

        if journal.phase == RotationRecoveryPhase::Replayed {
            let repaired = journal.repaired_head.as_deref().ok_or_else(|| {
                SkillSyncError::LocalQuarantineBlocked(
                    "rotation recovery is missing its repaired head".to_owned(),
                )
            })?;
            if journal.active_branch == journal.branch {
                let current = repo.rev_parse(&format!("refs/heads/{}", journal.branch))?;
                let expected = journal
                    .expected_head
                    .as_deref()
                    .unwrap_or(journal.tail_head.as_str());
                reconcile_update_ref_only_residue(
                    repo,
                    &journal.branch,
                    &current,
                    repaired,
                    expected,
                )?;
                ensure_clean_tracked_worktree(repo)?;
                if current == expected {
                    update_working_branch(repo, &journal.branch, repaired, expected)?;
                } else if current == repaired {
                    repo.reset_hard_to(repaired)?;
                } else {
                    return Err(SkillSyncError::Git(GitError::PushConflict));
                }
            } else {
                ensure_clean_tracked_worktree(repo)?;
                let active_ref = format!("refs/heads/{}", journal.active_branch);
                match revision_oid(repo, &active_ref)? {
                    Some(oid) if oid == repaired => {}
                    Some(oid) if Some(oid.as_str()) == journal.orphan_oid.as_deref() => {
                        repo.create_or_repoint_branch_to_exact(
                            &journal.active_branch,
                            repaired,
                            Some(&oid),
                        )?;
                    }
                    Some(_) => return Err(SkillSyncError::Git(GitError::PushConflict)),
                    None if journal.orphan_oid.is_none() => {
                        repo.create_or_repoint_branch_to_exact(
                            &journal.active_branch,
                            repaired,
                            None,
                        )?;
                    }
                    None => return Err(SkillSyncError::Git(GitError::PushConflict)),
                }
                repo.set_upstream_to_origin(&journal.active_branch)?;
                match repo.current_branch()?.as_str() {
                    branch if branch == journal.branch => {
                        repo.checkout_branch(&journal.active_branch)?;
                    }
                    branch if branch == journal.active_branch => {}
                    _ => return Err(SkillSyncError::Git(GitError::PushConflict)),
                }
                let old_ref = format!("refs/heads/{}", journal.branch);
                let expected = journal
                    .expected_head
                    .as_deref()
                    .unwrap_or(journal.tail_head.as_str());
                match revision_oid(repo, &old_ref)? {
                    Some(oid) if oid == expected => {
                        repo.reset_without_checkout_to_exact(
                            &journal.branch,
                            &journal.upstream_oid,
                            expected,
                        )?;
                    }
                    Some(oid) if oid == journal.upstream_oid => {}
                    _ => return Err(SkillSyncError::Git(GitError::PushConflict)),
                }
            }
            journal.phase = RotationRecoveryPhase::Moved;
            self.save_rotation_journal(&journal)?;
        }

        if journal.phase == RotationRecoveryPhase::Moved {
            let repaired = journal.repaired_head.as_deref().ok_or_else(|| {
                SkillSyncError::LocalQuarantineBlocked(
                    "moved rotation recovery is missing its repaired head".to_owned(),
                )
            })?;
            ensure_exact_checkout(repo, &journal.active_branch, repaired)?;
            if journal.orphan_branch != journal.active_branch {
                match revision_oid(repo, &format!("refs/heads/{}", journal.orphan_branch))? {
                    Some(oid) if Some(oid.as_str()) == journal.orphan_oid.as_deref() => {
                        repo.delete_local_branch_exact(&journal.orphan_branch, &oid)?;
                    }
                    Some(_) => return Err(SkillSyncError::Git(GitError::PushConflict)),
                    None => {}
                }
            }
            journal.phase = RotationRecoveryPhase::Completed;
            self.save_rotation_journal(&journal)?;
        }

        if journal.phase == RotationRecoveryPhase::Completed {
            let repaired = journal.repaired_head.as_deref().ok_or_else(|| {
                SkillSyncError::LocalQuarantineBlocked(
                    "completed rotation recovery is missing its repaired head".to_owned(),
                )
            })?;
            ensure_exact_checkout(repo, &journal.active_branch, repaired)?;
            cleanup_rotation_tail_refs(repo, &journal)?;
            self.remove_rotation_journal()?;
        }
        Ok(())
    }

    pub fn resume_pending_recoveries(
        &self,
        repo: &GitStorage,
        commit_lock: &Mutex<()>,
    ) -> Result<bool, SkillSyncError> {
        let Some(captured_journal) = self.load_rotation_journal()? else {
            return Ok(false);
        };
        repo.fetch()?;
        let captured_refs = capture_epoch_chain(repo, &captured_journal.branch)?;
        let original_remote = captured_refs.first().ok_or_else(|| {
            SkillSyncError::EpochValidationBlocked("empty rotation recovery epoch chain".to_owned())
        })?;
        let active_remote = captured_refs.last().ok_or_else(|| {
            SkillSyncError::EpochValidationBlocked("empty rotation recovery epoch chain".to_owned())
        })?;
        self.validate_and_store(repo, &active_remote.oid, &active_remote.branch)?;

        let _guard = commit_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for captured in &captured_refs {
            ensure_ref_equals(repo, &captured.reference, &captured.oid)?;
        }
        if self.load_journal()?.is_some() {
            return Err(SkillSyncError::LocalQuarantineBlocked(
                "Skill quarantine must finish before rotation recovery".to_owned(),
            ));
        }
        let journal = self
            .load_rotation_journal()?
            .ok_or(SkillSyncError::Git(GitError::PushConflict))?;
        if journal != captured_journal {
            return Err(SkillSyncError::Git(GitError::PushConflict));
        }
        validate_rotation_journal(repo, &journal)?;
        let mut journal = self.capture_rotation_descendant_locked(repo, journal)?;
        if matches!(
            journal.phase,
            RotationRecoveryPhase::Prepared | RotationRecoveryPhase::Replayed
        ) && (journal.upstream_oid != original_remote.oid
            || journal.active_branch != active_remote.branch
            || journal.active_oid != active_remote.oid)
        {
            ensure_clean_tracked_worktree(repo)?;
            if journal.upstream_oid != original_remote.oid
                && !is_ancestor(repo, &journal.upstream_oid, &original_remote.oid)?
            {
                return Err(SkillSyncError::LocalQuarantineBlocked(
                    "rotation recovery remote branch diverged from its captured tip".to_owned(),
                ));
            }
            if journal.active_branch == active_remote.branch
                && journal.active_oid != active_remote.oid
                && !is_ancestor(repo, &journal.active_oid, &active_remote.oid)?
            {
                return Err(SkillSyncError::LocalQuarantineBlocked(
                    "rotation recovery active branch was rewritten".to_owned(),
                ));
            }
            let current_branch = repo.current_branch()?;
            let current_head = repo.rev_parse("HEAD")?;
            if current_branch != journal.branch && current_branch != journal.active_branch {
                return Err(SkillSyncError::Git(GitError::PushConflict));
            }
            let current_is_safe = current_head == journal.tail_head
                || journal.repaired_head.as_deref() == Some(current_head.as_str())
                || journal.expected_head.as_deref() == Some(current_head.as_str());
            if !current_is_safe {
                return Err(SkillSyncError::LocalQuarantineBlocked(
                    "rotation recovery working branch moved outside its durable tail".to_owned(),
                ));
            }
            journal.upstream_oid = original_remote.oid.clone();
            journal.active_branch = active_remote.branch.clone();
            journal.active_oid = active_remote.oid.clone();
            journal.expected_head = Some(current_head);
            if journal.repaired_head.is_some() {
                journal.prior_repaired_head = journal.repaired_head.clone();
            }
            journal.repaired_head = None;
            journal.phase = RotationRecoveryPhase::Prepared;
            self.save_rotation_journal(&journal)?;
        }
        self.resume_rotation_journal_locked(repo, journal)?;
        Ok(true)
    }

    fn capture_rotation_descendant_locked(
        &self,
        repo: &GitStorage,
        mut journal: RotationRecoveryJournal,
    ) -> Result<RotationRecoveryJournal, SkillSyncError> {
        if !matches!(
            journal.phase,
            RotationRecoveryPhase::Prepared | RotationRecoveryPhase::Replayed
        ) {
            return Ok(journal);
        }
        let current_branch = repo.current_branch()?;
        if current_branch != journal.branch && current_branch != journal.active_branch {
            return Err(SkillSyncError::Git(GitError::PushConflict));
        }
        let current_head = repo.rev_parse("HEAD")?;
        if current_head == journal.tail_head
            || journal.repaired_head.as_deref() == Some(current_head.as_str())
            || journal.expected_head.as_deref() == Some(current_head.as_str())
        {
            return Ok(journal);
        }

        ensure_clean_tracked_worktree(repo)?;
        let (combined_head, expected_head, prior_repaired_head) =
            if is_ancestor(repo, &journal.tail_head, &current_head)? {
                validate_appended_rotation_range(repo, &journal.tail_head, &current_head)?;
                (current_head.clone(), None, None)
            } else {
                let mut replay_base = None;
                for candidate in [
                    journal.repaired_head.as_deref(),
                    journal.expected_head.as_deref(),
                    journal.prior_repaired_head.as_deref(),
                ]
                .into_iter()
                .flatten()
                {
                    if is_ancestor(repo, candidate, &current_head)? {
                        replay_base = Some(candidate.to_owned());
                        break;
                    }
                }
                let replay_base = replay_base.ok_or_else(|| {
                    SkillSyncError::LocalQuarantineBlocked(
                        "rotation recovery working branch diverged from its durable tail"
                            .to_owned(),
                    )
                })?;
                validate_appended_rotation_range(repo, &replay_base, &current_head)?;
                let combined = combine_rotation_tail(repo, &journal, &replay_base, &current_head)?;
                (combined, Some(current_head.clone()), Some(replay_base))
            };

        let capture_ref = format!(
            "{ROTATION_TAIL_REF_PREFIX}{}-capture-{combined_head}",
            journal.operation_id
        );
        ensure_exact_ref(repo, &capture_ref, &combined_head)?;
        journal.tail_ref = capture_ref;
        journal.tail_head = combined_head;
        journal.expected_head = expected_head;
        journal.prior_repaired_head = prior_repaired_head;
        journal.repaired_head = None;
        journal.phase = RotationRecoveryPhase::Prepared;
        self.save_rotation_journal(&journal)?;
        Ok(journal)
    }

    pub(crate) fn resume_rotation_recovery(
        &self,
        repo: &GitStorage,
        commit_lock: &Mutex<()>,
        old_branch: &str,
        orphan_branch: &str,
    ) -> Result<bool, SkillSyncError> {
        let Some(journal) = self.load_rotation_journal()? else {
            return Ok(false);
        };
        if journal.branch != old_branch || journal.orphan_branch != orphan_branch {
            return Err(SkillSyncError::Git(GitError::PushConflict));
        }
        self.resume_pending_recoveries(repo, commit_lock)
    }

    fn load_rotation_journal(&self) -> Result<Option<RotationRecoveryJournal>, SkillSyncError> {
        let bytes = match fs::read(&self.rotation_journal_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(quarantine_error("read rotation journal", error)),
        };
        serde_json::from_slice(&bytes).map(Some).map_err(|error| {
            SkillSyncError::LocalQuarantineBlocked(format!(
                "parse rotation recovery journal: {error}"
            ))
        })
    }

    fn save_rotation_journal(
        &self,
        journal: &RotationRecoveryJournal,
    ) -> Result<(), SkillSyncError> {
        let parent = self.rotation_journal_path.parent().ok_or_else(|| {
            SkillSyncError::LocalQuarantineBlocked(
                "rotation recovery journal has no parent".to_owned(),
            )
        })?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)
            .map_err(|error| quarantine_error("create rotation journal temp file", error))?;
        serde_json::to_writer_pretty(&mut temporary, journal).map_err(|error| {
            SkillSyncError::LocalQuarantineBlocked(format!(
                "serialize rotation recovery journal: {error}"
            ))
        })?;
        temporary
            .write_all(b"\n")
            .and_then(|()| temporary.flush())
            .map_err(|error| quarantine_error("write rotation journal", error))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| quarantine_error("sync rotation journal", error))?;
        temporary
            .persist(&self.rotation_journal_path)
            .map_err(|error| quarantine_error("persist rotation journal", error.error))?;
        sync_directory(parent)?;
        Ok(())
    }

    fn remove_rotation_journal(&self) -> Result<(), SkillSyncError> {
        match fs::remove_file(&self.rotation_journal_path) {
            Ok(()) => sync_directory(self.rotation_journal_path.parent().ok_or_else(|| {
                SkillSyncError::LocalQuarantineBlocked(
                    "rotation recovery journal has no parent".to_owned(),
                )
            })?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(quarantine_error("remove rotation journal", error)),
        }
    }

    fn load_journal(&self) -> Result<Option<QuarantineJournal>, SkillSyncError> {
        let bytes = match fs::read(&self.journal_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(quarantine_error("read journal", error)),
        };
        let journal: QuarantineJournal = serde_json::from_slice(&bytes).map_err(|error| {
            SkillSyncError::LocalQuarantineBlocked(format!("parse quarantine journal: {error}"))
        })?;
        Ok(Some(journal))
    }

    fn save_journal(&self, journal: &QuarantineJournal) -> Result<(), SkillSyncError> {
        let parent = self.journal_path.parent().ok_or_else(|| {
            SkillSyncError::LocalQuarantineBlocked("journal has no parent".to_owned())
        })?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)
            .map_err(|error| quarantine_error("create journal temp file", error))?;
        serde_json::to_writer_pretty(&mut temporary, journal).map_err(|error| {
            SkillSyncError::LocalQuarantineBlocked(format!("serialize journal: {error}"))
        })?;
        temporary
            .write_all(b"\n")
            .and_then(|()| temporary.flush())
            .map_err(|error| quarantine_error("write journal", error))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| quarantine_error("sync journal", error))?;
        temporary
            .persist(&self.journal_path)
            .map_err(|error| quarantine_error("persist journal", error.error))?;
        sync_directory(parent)?;
        Ok(())
    }

    fn remove_journal(&self) -> Result<(), SkillSyncError> {
        match fs::remove_file(&self.journal_path) {
            Ok(()) => sync_directory(self.journal_path.parent().ok_or_else(|| {
                SkillSyncError::LocalQuarantineBlocked("journal has no parent".to_owned())
            })?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(quarantine_error("remove journal", error)),
        }
    }
}

impl IntegrationOperation {
    fn expected_head(&self) -> Option<&str> {
        match self {
            Self::RebaseOntoOrigin { expected_head }
            | Self::HardDivergenceRecovery { expected_head }
            | Self::FollowEpochRedirectAfterDiscard { expected_head } => Some(expected_head),
            Self::FollowEpochRedirect | Self::CleanupFailedFire { .. } => None,
        }
    }
}

fn ensure_local_metadata_excluded(repository_root: &Path) -> Result<(), SkillSyncError> {
    let output = run_git(
        &["rev-parse", "--git-path", "info/exclude"],
        repository_root,
    )?;
    let raw_path = String::from_utf8(output.stdout).map_err(|error| {
        SkillSyncError::Checkpoint(format!("git exclude path is not UTF-8: {error}"))
    })?;
    let raw_path = raw_path.trim();
    let exclude_path = if Path::new(raw_path).is_absolute() {
        PathBuf::from(raw_path)
    } else {
        repository_root.join(raw_path)
    };
    let existing = match fs::read_to_string(&exclude_path) {
        Ok(existing) => existing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(quarantine_error("read git exclude", error)),
    };
    if existing.lines().any(|line| line.trim() == ".gitim/") {
        return Ok(());
    }
    let parent = exclude_path
        .parent()
        .ok_or_else(|| SkillSyncError::Checkpoint("git exclude path has no parent".to_owned()))?;
    fs::create_dir_all(parent).map_err(|error| quarantine_error("create git info", error))?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&exclude_path)
        .map_err(|error| quarantine_error("open git exclude", error))?;
    if !existing.is_empty() && !existing.ends_with('\n') {
        file.write_all(b"\n")
            .map_err(|error| quarantine_error("separate git exclude", error))?;
    }
    file.write_all(b".gitim/\n")
        .and_then(|()| file.flush())
        .map_err(|error| quarantine_error("write git exclude", error))
}

impl QuarantineJournal {
    fn prepared(
        branch: &str,
        upstream_oid: &str,
        original_head: &str,
    ) -> Result<Self, SkillSyncError> {
        validate_oid(upstream_oid)?;
        validate_oid(original_head)?;
        validate_branch(branch)?;
        let quarantine_ref = format!("{QUARANTINE_REF_PREFIX}{original_head}");
        Ok(Self {
            schema_version: JOURNAL_SCHEMA_VERSION,
            operation_id: original_head.to_owned(),
            branch: branch.to_owned(),
            upstream_oid: upstream_oid.to_owned(),
            original_head: original_head.to_owned(),
            quarantine_ref,
            phase: QuarantinePhase::Prepared,
            repaired_head: None,
            branch_head: Some(original_head.to_owned()),
            tail_ref: None,
            tail_base: None,
            tail_head: None,
            kind: QuarantineKind::SkillHistory,
            excluded_commits: Vec::new(),
            semantic_error_code: None,
            replay_base: None,
            active_branch: None,
            active_branch_head: None,
            active_tail_ref: None,
            active_tail_base: None,
            active_tail_head: None,
            completion_head: None,
        })
    }

    fn user_archive(
        branch: &str,
        upstream_oid: &str,
        original_head: &str,
        excluded_commits: Vec<String>,
        semantic_error_code: String,
    ) -> Result<Self, SkillSyncError> {
        let mut journal = Self::prepared(branch, upstream_oid, original_head)?;
        journal.quarantine_ref = format!("refs/gitim/quarantine/user-archive-{original_head}");
        journal.kind = QuarantineKind::UserArchive;
        journal.excluded_commits = excluded_commits;
        journal.semantic_error_code = Some(semantic_error_code);
        Ok(journal)
    }

    fn expected_branch_head(&self) -> &str {
        self.branch_head
            .as_deref()
            .unwrap_or(self.original_head.as_str())
    }

    fn completion_branch(&self) -> &str {
        self.active_branch.as_deref().unwrap_or(&self.branch)
    }
}

fn is_skill_history_quarantine(kind: &QuarantineKind) -> bool {
    *kind == QuarantineKind::SkillHistory
}

fn validate_journal(repo: &GitStorage, journal: &QuarantineJournal) -> Result<(), SkillSyncError> {
    if journal.schema_version != JOURNAL_SCHEMA_VERSION {
        return Err(SkillSyncError::LocalQuarantineBlocked(
            "unsupported quarantine journal schema".to_owned(),
        ));
    }
    validate_oid(&journal.operation_id)?;
    validate_oid(&journal.upstream_oid)?;
    validate_oid(&journal.original_head)?;
    if journal.operation_id != journal.original_head {
        return Err(SkillSyncError::LocalQuarantineBlocked(
            "quarantine operation identity does not match original head".to_owned(),
        ));
    }
    validate_branch(&journal.branch)?;
    let expected_ref = match journal.kind {
        QuarantineKind::SkillHistory => {
            format!("{QUARANTINE_REF_PREFIX}{}", journal.original_head)
        }
        QuarantineKind::UserArchive => {
            format!(
                "refs/gitim/quarantine/user-archive-{}",
                journal.original_head
            )
        }
    };
    if journal.quarantine_ref != expected_ref {
        return Err(SkillSyncError::LocalQuarantineBlocked(
            "quarantine ref does not match operation identity".to_owned(),
        ));
    }
    if let Some(repaired) = &journal.repaired_head {
        validate_oid(repaired)?;
        repo.rev_parse(&format!("{repaired}^{{commit}}"))?;
    }
    match (&journal.phase, &journal.completion_head) {
        (QuarantinePhase::Completed, Some(completion_head)) => {
            validate_oid(completion_head)?;
            repo.rev_parse(&format!("{completion_head}^{{commit}}"))?;
            let repaired = journal.repaired_head.as_deref().ok_or_else(|| {
                SkillSyncError::LocalQuarantineBlocked(
                    "completed quarantine is missing its repaired head".to_owned(),
                )
            })?;
            if !is_ancestor(repo, repaired, completion_head)? {
                return Err(SkillSyncError::LocalQuarantineBlocked(
                    "completed quarantine head does not contain its repaired head".to_owned(),
                ));
            }
        }
        (QuarantinePhase::Completed, None) => {
            return Err(SkillSyncError::LocalQuarantineBlocked(
                "completed quarantine is missing its cleanup head".to_owned(),
            ));
        }
        (_, Some(_)) => {
            return Err(SkillSyncError::LocalQuarantineBlocked(
                "in-flight quarantine contains a cleanup head".to_owned(),
            ));
        }
        (_, None) => {}
    }
    if let Some(branch_head) = &journal.branch_head {
        validate_oid(branch_head)?;
        if journal.phase != QuarantinePhase::Completed {
            repo.rev_parse(&format!("{branch_head}^{{commit}}"))?;
        }
    }
    if let Some(replay_base) = &journal.replay_base {
        validate_oid(replay_base)?;
        repo.rev_parse(&format!("{replay_base}^{{commit}}"))?;
        if !is_ancestor(repo, replay_base, &journal.original_head)? {
            return Err(SkillSyncError::LocalQuarantineBlocked(
                "quarantine replay base is outside quarantined history".to_owned(),
            ));
        }
    }
    if let Some(active_branch) = &journal.active_branch {
        validate_branch(active_branch)?;
    } else if journal.active_branch_head.is_some() {
        return Err(SkillSyncError::LocalQuarantineBlocked(
            "semantic archive active branch metadata is incomplete".to_owned(),
        ));
    }
    if let Some(active_branch_head) = &journal.active_branch_head {
        validate_oid(active_branch_head)?;
        if journal.phase != QuarantinePhase::Completed {
            repo.rev_parse(&format!("{active_branch_head}^{{commit}}"))?;
        }
    }
    match (
        &journal.active_tail_ref,
        &journal.active_tail_base,
        &journal.active_tail_head,
    ) {
        (None, None, None) => {}
        (Some(tail_ref), Some(tail_base), Some(tail_head)) => {
            validate_oid(tail_base)?;
            validate_oid(tail_head)?;
            let expected_tail_ref = format!(
                "{QUARANTINE_TAIL_REF_PREFIX}{}-active-{tail_head}",
                journal.operation_id
            );
            let observed_tail = revision_oid(repo, tail_ref)?;
            let tail_ref_retained = observed_tail.as_deref() == Some(tail_head.as_str());
            let tail_ref_cleaned =
                journal.phase == QuarantinePhase::Completed && observed_tail.is_none();
            if tail_ref != &expected_tail_ref
                || (!tail_ref_retained && !tail_ref_cleaned)
                || journal.active_branch_head.as_deref() != Some(tail_head.as_str())
            {
                return Err(SkillSyncError::LocalQuarantineBlocked(
                    "semantic archive active tail metadata is invalid".to_owned(),
                ));
            }
            if tail_ref_retained {
                if !is_ancestor(repo, tail_base, tail_head)? {
                    return Err(SkillSyncError::LocalQuarantineBlocked(
                        "semantic archive active tail ancestry is invalid".to_owned(),
                    ));
                }
                verify_managed_roots(repo, tail_base, tail_head)?;
                if history_touches_managed_skills_between(repo, tail_base, tail_head)? {
                    return Err(SkillSyncError::LocalQuarantineBlocked(
                        "semantic archive active tail touches managed Skill paths".to_owned(),
                    ));
                }
                if history_touches_epoch_file_between(repo, tail_base, tail_head)? {
                    return Err(SkillSyncError::LocalQuarantineBlocked(
                        "semantic archive active tail touches epoch metadata".to_owned(),
                    ));
                }
            }
        }
        _ => {
            return Err(SkillSyncError::LocalQuarantineBlocked(
                "semantic archive active tail metadata is incomplete".to_owned(),
            ));
        }
    }
    match journal.kind {
        QuarantineKind::SkillHistory => {
            if !journal.excluded_commits.is_empty()
                || journal.semantic_error_code.is_some()
                || journal.active_branch.is_some()
                || journal.active_branch_head.is_some()
                || journal.active_tail_ref.is_some()
                || journal.active_tail_base.is_some()
                || journal.active_tail_head.is_some()
            {
                return Err(SkillSyncError::LocalQuarantineBlocked(
                    "Skill quarantine contains semantic archive metadata".to_owned(),
                ));
            }
        }
        QuarantineKind::UserArchive => {
            if journal.excluded_commits.is_empty() || journal.semantic_error_code.is_none() {
                return Err(SkillSyncError::LocalQuarantineBlocked(
                    "semantic archive quarantine metadata is incomplete".to_owned(),
                ));
            }
            for commit in &journal.excluded_commits {
                validate_oid(commit)?;
                if !is_ancestor(repo, commit, &journal.original_head)? {
                    return Err(SkillSyncError::LocalQuarantineBlocked(
                        "semantic archive exclusion is outside quarantined history".to_owned(),
                    ));
                }
            }
        }
    }
    match (&journal.tail_ref, &journal.tail_base, &journal.tail_head) {
        (None, None, None) => {}
        (Some(tail_ref), Some(tail_base), Some(tail_head)) => {
            validate_oid(tail_base)?;
            validate_oid(tail_head)?;
            let expected_tail_ref = format!(
                "{QUARANTINE_TAIL_REF_PREFIX}{}-{tail_head}",
                journal.operation_id
            );
            if tail_ref != &expected_tail_ref {
                return Err(SkillSyncError::LocalQuarantineBlocked(
                    "quarantine tail ref does not match operation identity".to_owned(),
                ));
            }
            let observed_tail = revision_oid(repo, tail_ref)?;
            let tail_ref_retained = observed_tail.as_deref() == Some(tail_head.as_str());
            let tail_ref_cleaned =
                journal.phase == QuarantinePhase::Completed && observed_tail.is_none();
            if !tail_ref_retained && !tail_ref_cleaned {
                return Err(SkillSyncError::LocalQuarantineBlocked(
                    "quarantine tail ref or ancestry is invalid".to_owned(),
                ));
            }
            if tail_ref_retained {
                if !is_ancestor(repo, tail_base, tail_head)? {
                    return Err(SkillSyncError::LocalQuarantineBlocked(
                        "quarantine tail ancestry is invalid".to_owned(),
                    ));
                }
                verify_managed_roots(repo, tail_base, tail_head)?;
                if history_touches_managed_skills_between(repo, tail_base, tail_head)? {
                    return Err(SkillSyncError::LocalQuarantineBlocked(
                        "quarantine tail touches managed Skill paths".to_owned(),
                    ));
                }
                if history_touches_epoch_file_between(repo, tail_base, tail_head)? {
                    return Err(SkillSyncError::LocalQuarantineBlocked(
                        "quarantine tail touches epoch metadata".to_owned(),
                    ));
                }
            }
        }
        _ => {
            return Err(SkillSyncError::LocalQuarantineBlocked(
                "quarantine tail metadata is incomplete".to_owned(),
            ));
        }
    }
    repo.rev_parse(&format!("{}^{{commit}}", journal.original_head))?;
    repo.rev_parse(&format!("{}^{{commit}}", journal.upstream_oid))?;
    Ok(())
}

fn validate_oid(oid: &str) -> Result<(), SkillSyncError> {
    if !matches!(oid.len(), 40 | 64) || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(SkillSyncError::LocalQuarantineBlocked(
            "quarantine contains an invalid object id".to_owned(),
        ));
    }
    Ok(())
}

fn validate_branch(branch: &str) -> Result<(), SkillSyncError> {
    if branch.is_empty()
        || branch.starts_with('-')
        || branch.bytes().any(|byte| {
            byte.is_ascii_control()
                || byte.is_ascii_whitespace()
                || matches!(byte, b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\')
        })
        || branch.contains("..")
        || branch.contains("@{")
        || branch.ends_with('.')
        || branch.ends_with('/')
        || branch.contains("//")
    {
        return Err(SkillSyncError::LocalQuarantineBlocked(
            "quarantine contains an invalid branch".to_owned(),
        ));
    }
    Ok(())
}

fn validate_rotation_journal(
    repo: &GitStorage,
    journal: &RotationRecoveryJournal,
) -> Result<(), SkillSyncError> {
    if journal.schema_version != JOURNAL_SCHEMA_VERSION {
        return Err(SkillSyncError::LocalQuarantineBlocked(
            "unsupported rotation recovery journal schema".to_owned(),
        ));
    }
    for branch in [
        &journal.branch,
        &journal.active_branch,
        &journal.orphan_branch,
    ] {
        validate_branch(branch)?;
    }
    for oid in [
        &journal.operation_id,
        &journal.upstream_oid,
        &journal.active_oid,
        &journal.seal_oid,
        &journal.tail_head,
    ] {
        validate_oid(oid)?;
    }
    if let Some(orphan_oid) = journal.orphan_oid.as_deref() {
        validate_oid(orphan_oid)?;
    }
    if let Some(prior_repaired) = journal.prior_repaired_head.as_deref() {
        validate_oid(prior_repaired)?;
    }
    if let Some(expected_head) = journal.expected_head.as_deref() {
        validate_oid(expected_head)?;
    }
    if let Some(repaired) = journal.repaired_head.as_deref() {
        validate_oid(repaired)?;
    }
    let initial_tail_ref = format!("{ROTATION_TAIL_REF_PREFIX}{}", journal.operation_id);
    let captured_tail_ref = format!(
        "{ROTATION_TAIL_REF_PREFIX}{}-capture-{}",
        journal.operation_id, journal.tail_head
    );
    if journal.operation_id != journal.seal_oid
        || (journal.tail_ref != initial_tail_ref && journal.tail_ref != captured_tail_ref)
    {
        return Err(SkillSyncError::LocalQuarantineBlocked(
            "rotation recovery journal identity is invalid".to_owned(),
        ));
    }
    if journal.phase == RotationRecoveryPhase::Completed {
        if revision_oid(repo, &journal.tail_ref)?.is_some_and(|oid| oid != journal.tail_head) {
            return Err(SkillSyncError::LocalQuarantineBlocked(
                "completed rotation recovery tail ref changed ownership".to_owned(),
            ));
        }
        let repaired = journal.repaired_head.as_deref().ok_or_else(|| {
            SkillSyncError::LocalQuarantineBlocked(
                "completed rotation recovery is missing its repaired head".to_owned(),
            )
        })?;
        for oid in [&journal.upstream_oid, &journal.active_oid, repaired] {
            repo.rev_parse(&format!("{oid}^{{commit}}"))?;
        }
        verify_managed_roots(repo, &journal.active_oid, repaired)?;
        if repo.show_file_at_ref(&journal.active_oid, crate::rotate::EPOCH_FILE)?
            != repo.show_file_at_ref(repaired, crate::rotate::EPOCH_FILE)?
        {
            return Err(SkillSyncError::LocalQuarantineBlocked(
                "rotation recovery changed active epoch metadata".to_owned(),
            ));
        }
        return Ok(());
    }
    for oid in [
        &journal.operation_id,
        &journal.upstream_oid,
        &journal.active_oid,
        &journal.seal_oid,
        &journal.tail_head,
    ] {
        repo.rev_parse(&format!("{oid}^{{commit}}"))?;
    }
    if let Some(prior_repaired) = journal.prior_repaired_head.as_deref() {
        repo.rev_parse(&format!("{prior_repaired}^{{commit}}"))?;
    }
    if let Some(expected_head) = journal.expected_head.as_deref() {
        repo.rev_parse(&format!("{expected_head}^{{commit}}"))?;
        if expected_head != journal.tail_head {
            let prior_repaired = journal.prior_repaired_head.as_deref().ok_or_else(|| {
                SkillSyncError::LocalQuarantineBlocked(
                    "rotation recovery expected head has no prior replay".to_owned(),
                )
            })?;
            if expected_head != prior_repaired {
                validate_appended_rotation_range(repo, prior_repaired, expected_head)?;
            }
        }
    }
    if !is_ancestor(repo, &journal.seal_oid, &journal.tail_head)?
        || history_touches_managed_skills_between(repo, &journal.seal_oid, &journal.tail_head)?
        || history_touches_epoch_file_between(repo, &journal.seal_oid, &journal.tail_head)?
    {
        return Err(SkillSyncError::LocalQuarantineBlocked(
            "rotation recovery journal has an unsafe tail".to_owned(),
        ));
    }
    verify_managed_roots(repo, &journal.seal_oid, &journal.tail_head)?;
    if revision_oid(repo, &journal.tail_ref)?.as_deref() != Some(journal.tail_head.as_str()) {
        return Err(SkillSyncError::LocalQuarantineBlocked(
            "rotation recovery tail ref moved or disappeared".to_owned(),
        ));
    }
    if let Some(repaired) = journal.repaired_head.as_deref() {
        repo.rev_parse(&format!("{repaired}^{{commit}}"))?;
        verify_managed_roots(repo, &journal.active_oid, repaired)?;
        if repo.show_file_at_ref(&journal.active_oid, crate::rotate::EPOCH_FILE)?
            != repo.show_file_at_ref(repaired, crate::rotate::EPOCH_FILE)?
        {
            return Err(SkillSyncError::LocalQuarantineBlocked(
                "rotation recovery changed active epoch metadata".to_owned(),
            ));
        }
    } else if journal.phase != RotationRecoveryPhase::Prepared {
        return Err(SkillSyncError::LocalQuarantineBlocked(
            "rotation recovery phase is missing its repaired head".to_owned(),
        ));
    }
    Ok(())
}

fn find_rotation_seal(
    repo: &GitStorage,
    upstream_oid: &str,
    head: &str,
) -> Result<String, SkillSyncError> {
    let range = format!("{upstream_oid}..{head}");
    let commits = repo.run_git_capture(&["rev-list", "--reverse", "--first-parent", &range])?;
    let mut seals = Vec::new();
    for commit in commits.lines().filter(|commit| !commit.is_empty()) {
        let subject = repo.run_git_capture(&["show", "-s", "--format=%s", commit])?;
        if subject.starts_with(crate::rotate::SEAL_SUBJECT_PREFIX) {
            seals.push(commit.to_owned());
        }
    }
    if seals.len() != 1 {
        return Err(SkillSyncError::LocalQuarantineBlocked(
            "failed rotation history must contain exactly one local seal".to_owned(),
        ));
    }
    Ok(seals.remove(0))
}

fn replay_rotation_tail(
    repo: &GitStorage,
    journal: &RotationRecoveryJournal,
) -> Result<String, SkillSyncError> {
    let worktrees = repo
        .root()
        .join(".gitim")
        .join("rotation-recovery-worktrees");
    fs::create_dir_all(&worktrees)
        .map_err(|error| quarantine_error("create rotation recovery worktree directory", error))?;
    let worktree = worktrees.join(&journal.operation_id);
    cleanup_worktree(repo, &worktree)?;
    let worktree_string = path_string(&worktree)?;
    run_git(
        &[
            "worktree",
            "add",
            "--detach",
            "--force",
            &worktree_string,
            &journal.active_oid,
        ],
        repo.root(),
    )?;
    let replay_repo = GitStorage::new(&worktree);
    let result = (|| {
        let replayed = replay_linear_range(
            repo,
            &replay_repo,
            &journal.seal_oid,
            &journal.tail_head,
            &[],
            None,
        )?;
        if replayed == 0
            && changed_paths(repo, &journal.seal_oid, &journal.tail_head)?
                .iter()
                .any(|path| !is_managed_skill_path(path) && path != crate::rotate::EPOCH_FILE)
        {
            return Err(SkillSyncError::LocalQuarantineBlocked(
                "rotation tail replay omitted ordinary changes".to_owned(),
            ));
        }
        let repaired = replay_repo.rev_parse("HEAD")?;
        verify_managed_roots(&replay_repo, &journal.active_oid, &repaired)?;
        if replay_repo.show_file_at_ref(&journal.active_oid, crate::rotate::EPOCH_FILE)?
            != replay_repo.show_file_at_ref(&repaired, crate::rotate::EPOCH_FILE)?
        {
            return Err(SkillSyncError::LocalQuarantineBlocked(
                "rotation tail replay changed active epoch metadata".to_owned(),
            ));
        }
        Ok(repaired)
    })();
    let cleanup = cleanup_worktree(repo, &worktree);
    match (result, cleanup) {
        (Ok(repaired), Ok(())) => Ok(repaired),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
}

fn validate_appended_rotation_range(
    repo: &GitStorage,
    base: &str,
    head: &str,
) -> Result<(), SkillSyncError> {
    if !is_ancestor(repo, base, head)?
        || history_touches_managed_skills_between(repo, base, head)?
        || history_touches_epoch_file_between(repo, base, head)?
    {
        return Err(SkillSyncError::LocalQuarantineBlocked(
            "rotation recovery descendant is unsafe or unrelated".to_owned(),
        ));
    }
    verify_managed_roots(repo, base, head)?;
    if repo.show_file_at_ref(base, crate::rotate::EPOCH_FILE)?
        != repo.show_file_at_ref(head, crate::rotate::EPOCH_FILE)?
    {
        return Err(SkillSyncError::LocalQuarantineBlocked(
            "rotation recovery descendant changed epoch metadata".to_owned(),
        ));
    }
    Ok(())
}

fn combine_rotation_tail(
    repo: &GitStorage,
    journal: &RotationRecoveryJournal,
    appended_base: &str,
    appended_head: &str,
) -> Result<String, SkillSyncError> {
    let worktrees = repo
        .root()
        .join(".gitim")
        .join("rotation-tail-capture-worktrees");
    fs::create_dir_all(&worktrees)
        .map_err(|error| quarantine_error("create rotation tail worktree directory", error))?;
    let worktree = worktrees.join(&journal.operation_id);
    cleanup_worktree(repo, &worktree)?;
    let worktree_string = path_string(&worktree)?;
    run_git(
        &[
            "worktree",
            "add",
            "--detach",
            "--force",
            &worktree_string,
            &journal.tail_head,
        ],
        repo.root(),
    )?;
    let replay_repo = GitStorage::new(&worktree);
    let result = (|| {
        let replayed =
            replay_linear_range(repo, &replay_repo, appended_base, appended_head, &[], None)?;
        if replayed == 0
            && changed_paths(repo, appended_base, appended_head)?
                .iter()
                .any(|path| !is_managed_skill_path(path) && path != crate::rotate::EPOCH_FILE)
        {
            return Err(SkillSyncError::LocalQuarantineBlocked(
                "rotation recovery omitted an appended ordinary change".to_owned(),
            ));
        }
        let combined = replay_repo.rev_parse("HEAD")?;
        verify_managed_roots(&replay_repo, &journal.tail_head, &combined)?;
        if replay_repo.show_file_at_ref(&journal.tail_head, crate::rotate::EPOCH_FILE)?
            != replay_repo.show_file_at_ref(&combined, crate::rotate::EPOCH_FILE)?
        {
            return Err(SkillSyncError::LocalQuarantineBlocked(
                "combined rotation tail changed epoch metadata".to_owned(),
            ));
        }
        Ok(combined)
    })();
    let cleanup = cleanup_worktree(repo, &worktree);
    match (result, cleanup) {
        (Ok(combined), Ok(())) => Ok(combined),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
}

fn ensure_quarantine_ref(
    repo: &GitStorage,
    journal: &QuarantineJournal,
) -> Result<(), SkillSyncError> {
    match repo.rev_parse(&journal.quarantine_ref) {
        Ok(existing) if existing == journal.original_head => Ok(()),
        Ok(_) => Err(SkillSyncError::LocalQuarantineBlocked(
            "quarantine ref collides with another operation".to_owned(),
        )),
        Err(_) => {
            let zero_oid = "0".repeat(journal.original_head.len());
            run_git(
                &[
                    "update-ref",
                    &journal.quarantine_ref,
                    &journal.original_head,
                    &zero_oid,
                ],
                repo.root(),
            )?;
            Ok(())
        }
    }
}

fn capture_quarantine_tail(
    repo: &GitStorage,
    journal: &mut QuarantineJournal,
    ancestry_base: &str,
    replay_base: &str,
    tail_head: &str,
) -> Result<(), SkillSyncError> {
    if !is_ancestor(repo, ancestry_base, tail_head)?
        || !is_ancestor(repo, replay_base, tail_head)?
        || history_touches_managed_skills_between(repo, ancestry_base, tail_head)?
        || history_touches_managed_skills_between(repo, replay_base, tail_head)?
        || history_touches_epoch_file_between(repo, ancestry_base, tail_head)?
        || history_touches_epoch_file_between(repo, replay_base, tail_head)?
    {
        return Err(SkillSyncError::LocalQuarantineBlocked(
            "working branch tail is unrelated or touches managed Skill paths".to_owned(),
        ));
    }
    verify_managed_roots(repo, ancestry_base, tail_head)?;
    verify_managed_roots(repo, replay_base, tail_head)?;

    let tail_ref = format!(
        "{QUARANTINE_TAIL_REF_PREFIX}{}-{tail_head}",
        journal.operation_id
    );
    match revision_oid(repo, &tail_ref)? {
        Some(existing) if existing == tail_head => {}
        Some(_) => {
            return Err(SkillSyncError::LocalQuarantineBlocked(
                "quarantine tail ref collides with another operation".to_owned(),
            ));
        }
        None => {
            let zero_oid = "0".repeat(tail_head.len());
            run_git(
                &["update-ref", &tail_ref, tail_head, &zero_oid],
                repo.root(),
            )?;
        }
    }
    journal.tail_ref = Some(tail_ref);
    journal.tail_base = Some(replay_base.to_owned());
    journal.tail_head = Some(tail_head.to_owned());
    Ok(())
}

fn bind_semantic_active_destination(
    repo: &GitStorage,
    journal: &mut QuarantineJournal,
    active_branch: &str,
    active_oid: &str,
    captured_active_head: Option<&str>,
) -> Result<(), SkillSyncError> {
    let active_ref = format!("refs/heads/{active_branch}");
    ensure_optional_ref_equals(repo, &active_ref, captured_active_head)?;
    validate_semantic_active_tail(repo, journal, active_oid, captured_active_head)?;

    if journal.active_branch.as_deref() == Some(active_branch) {
        let current = revision_oid(repo, &active_ref)?;
        if current.as_deref() != journal.active_branch_head.as_deref()
            && current.as_deref() != journal.repaired_head.as_deref()
        {
            return Err(SkillSyncError::Git(GitError::PushConflict));
        }
        return Ok(());
    }

    if let Some(active_head) = captured_active_head {
        if active_head != active_oid && journal.repaired_head.as_deref() != Some(active_head) {
            match journal.active_tail_head.as_deref() {
                Some(existing) if existing == active_head => {}
                Some(_) => {
                    return Err(SkillSyncError::LocalQuarantineBlocked(
                        "semantic archive already owns a different active tail".to_owned(),
                    ));
                }
                None => {
                    let tail_ref = format!(
                        "{QUARANTINE_TAIL_REF_PREFIX}{}-active-{active_head}",
                        journal.operation_id
                    );
                    match revision_oid(repo, &tail_ref)? {
                        Some(existing) if existing == active_head => {}
                        Some(_) => {
                            return Err(SkillSyncError::LocalQuarantineBlocked(
                                "semantic archive active tail ref collides with another operation"
                                    .to_owned(),
                            ));
                        }
                        None => {
                            let zero_oid = "0".repeat(active_head.len());
                            run_git(
                                &["update-ref", &tail_ref, active_head, &zero_oid],
                                repo.root(),
                            )?;
                        }
                    }
                    journal.active_tail_ref = Some(tail_ref);
                    journal.active_tail_base = Some(active_oid.to_owned());
                    journal.active_tail_head = Some(active_head.to_owned());
                }
            }
        }
    }

    journal.active_branch = Some(active_branch.to_owned());
    journal.active_branch_head = captured_active_head.map(str::to_owned);
    Ok(())
}

fn validate_semantic_active_tail(
    repo: &GitStorage,
    journal: &QuarantineJournal,
    active_oid: &str,
    captured_active_head: Option<&str>,
) -> Result<(), SkillSyncError> {
    let Some(active_head) = captured_active_head else {
        return Ok(());
    };
    if active_head == active_oid || journal.repaired_head.as_deref() == Some(active_head) {
        return Ok(());
    }
    if !is_ancestor(repo, active_oid, active_head)?
        || history_touches_managed_skills_between(repo, active_oid, active_head)?
        || history_touches_epoch_file_between(repo, active_oid, active_head)?
    {
        return Err(SkillSyncError::LocalQuarantineBlocked(
            "active epoch branch has an unsafe local tail".to_owned(),
        ));
    }
    verify_managed_roots(repo, active_oid, active_head)
}

fn cleanup_quarantine_tail_refs(
    repo: &GitStorage,
    journal: &QuarantineJournal,
    keep: Option<&str>,
) -> Result<(), SkillSyncError> {
    let prefix = format!("{QUARANTINE_TAIL_REF_PREFIX}{}-", journal.operation_id);
    let refs = repo.run_git_capture(&["for-each-ref", "--format=%(refname)", &prefix])?;
    for reference in refs.lines().filter(|reference| !reference.is_empty()) {
        if keep == Some(reference) {
            continue;
        }
        let oid = repo.rev_parse(reference)?;
        run_git(&["update-ref", "-d", reference, &oid], repo.root())?;
    }
    Ok(())
}

fn cleanup_rotation_tail_refs(
    repo: &GitStorage,
    journal: &RotationRecoveryJournal,
) -> Result<(), SkillSyncError> {
    let prefix = format!("{ROTATION_TAIL_REF_PREFIX}{}", journal.operation_id);
    let refs = repo.run_git_capture(&["for-each-ref", "--format=%(refname)", &prefix])?;
    for reference in refs.lines().filter(|reference| !reference.is_empty()) {
        let oid = repo.rev_parse(reference)?;
        run_git(&["update-ref", "-d", reference, &oid], repo.root())?;
    }
    Ok(())
}

fn replay_without_managed_skills(
    repo: &GitStorage,
    journal: &QuarantineJournal,
    author: Option<(&str, &str)>,
) -> Result<String, SkillSyncError> {
    let worktrees = repo
        .root()
        .join(".gitim")
        .join("skill-quarantine-worktrees");
    fs::create_dir_all(&worktrees)
        .map_err(|error| quarantine_error("create quarantine worktree directory", error))?;
    let worktree = worktrees.join(&journal.operation_id);
    cleanup_worktree(repo, &worktree)?;
    let worktree_string = path_string(&worktree)?;
    run_git(
        &[
            "worktree",
            "add",
            "--detach",
            "--force",
            &worktree_string,
            &journal.upstream_oid,
        ],
        repo.root(),
    )?;
    let replay_repo = GitStorage::new(&worktree);
    let result = (|| {
        if journal.tail_head.is_some() {
            replay_quarantine_tail(repo, &replay_repo, journal, author)?;
        } else {
            replay_commits(repo, &replay_repo, journal, author)?;
        }
        replay_semantic_active_tail(repo, &replay_repo, journal, author)
    })();
    let cleanup = cleanup_worktree(repo, &worktree);
    match (result, cleanup) {
        (Ok(head), Ok(())) => Ok(head),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
}

fn replay_commits(
    source: &GitStorage,
    target: &GitStorage,
    journal: &QuarantineJournal,
    author: Option<(&str, &str)>,
) -> Result<String, SkillSyncError> {
    let merge_base = match journal.replay_base.as_deref() {
        Some(replay_base) => replay_base.to_owned(),
        None => merge_base(source, &journal.upstream_oid, &journal.original_head)?,
    };
    let replayed_non_skill_commits = replay_linear_range(
        source,
        target,
        &merge_base,
        &journal.original_head,
        &journal.excluded_commits,
        author,
    )?;
    let repaired = target.rev_parse("HEAD")?;
    if journal.kind == QuarantineKind::SkillHistory
        && replayed_non_skill_commits == 0
        && changed_paths(source, &merge_base, &journal.original_head)?
            .iter()
            .any(|path| !is_managed_skill_path(path))
    {
        return Err(SkillSyncError::LocalQuarantineBlocked(
            "quarantine replay omitted ordinary changes".to_owned(),
        ));
    }
    verify_managed_roots(target, &journal.upstream_oid, &repaired)?;
    if merge_base == journal.upstream_oid && journal.kind == QuarantineKind::SkillHistory {
        verify_non_skill_tree_equivalence(source, &journal.original_head, target, &repaired)?;
    }
    Ok(repaired)
}

fn replay_quarantine_tail(
    source: &GitStorage,
    target: &GitStorage,
    journal: &QuarantineJournal,
    author: Option<(&str, &str)>,
) -> Result<String, SkillSyncError> {
    let (Some(tail_base), Some(tail_head)) =
        (journal.tail_base.as_deref(), journal.tail_head.as_deref())
    else {
        return target.rev_parse("HEAD").map_err(Into::into);
    };
    let replayed = replay_linear_range(
        source,
        target,
        tail_base,
        tail_head,
        &journal.excluded_commits,
        author,
    )?;
    if replayed == 0
        && changed_paths(source, tail_base, tail_head)?
            .iter()
            .any(|path| !is_managed_skill_path(path))
    {
        return Err(SkillSyncError::LocalQuarantineBlocked(
            "quarantine tail replay omitted ordinary changes".to_owned(),
        ));
    }
    let repaired = target.rev_parse("HEAD")?;
    verify_managed_roots(target, &journal.upstream_oid, &repaired)?;
    Ok(repaired)
}

fn replay_semantic_active_tail(
    source: &GitStorage,
    target: &GitStorage,
    journal: &QuarantineJournal,
    author: Option<(&str, &str)>,
) -> Result<String, SkillSyncError> {
    let (Some(tail_base), Some(tail_head)) = (
        journal.active_tail_base.as_deref(),
        journal.active_tail_head.as_deref(),
    ) else {
        return target.rev_parse("HEAD").map_err(Into::into);
    };
    let replayed = replay_linear_range(source, target, tail_base, tail_head, &[], author)?;
    if replayed == 0
        && changed_paths(source, tail_base, tail_head)?
            .iter()
            .any(|path| !is_managed_skill_path(path))
    {
        return Err(SkillSyncError::LocalQuarantineBlocked(
            "semantic archive active tail replay omitted ordinary changes".to_owned(),
        ));
    }
    let repaired = target.rev_parse("HEAD")?;
    verify_managed_roots(target, &journal.upstream_oid, &repaired)?;
    Ok(repaired)
}

fn replay_linear_range(
    source: &GitStorage,
    target: &GitStorage,
    base: &str,
    head: &str,
    excluded_commits: &[String],
    author: Option<(&str, &str)>,
) -> Result<usize, SkillSyncError> {
    if history_touches_epoch_file_between(source, base, head)? {
        return Err(SkillSyncError::LocalQuarantineBlocked(
            "replay range touches epoch metadata".to_owned(),
        ));
    }
    let range = format!("{base}..{head}");
    let commits = source.run_git_capture(&[
        "rev-list",
        "--reverse",
        "--topo-order",
        "--first-parent",
        &range,
    ])?;
    let mut replayed_non_skill_commits = 0_usize;
    for commit in commits.lines().filter(|line| !line.is_empty()) {
        if excluded_commits.iter().any(|excluded| excluded == commit) {
            continue;
        }
        let parents = source.run_git_capture(&["rev-list", "--parents", "-n", "1", commit])?;
        let fields: Vec<&str> = parents.split_whitespace().collect();
        if fields.len() != 2 {
            return Err(SkillSyncError::LocalQuarantineBlocked(format!(
                "cannot replay non-linear bypass commit {commit}"
            )));
        }
        let parent = fields[1];
        let patch = run_git(
            &[
                "diff",
                "--binary",
                "--full-index",
                "--no-renames",
                parent,
                commit,
                "--",
                ".",
                ":(exclude)skills",
                ":(exclude)skills/**",
                ":(exclude)archive/skills",
                ":(exclude)archive/skills/**",
            ],
            source.root(),
        )?
        .stdout;
        if patch.is_empty() {
            continue;
        }
        apply_replay_patch(source, target, parent, commit, &patch)?;
        if index_is_clean(target)? {
            continue;
        }
        let short = commit.get(..12).unwrap_or(commit);
        let original_author;
        let replay_author = match author {
            Some(author) => author,
            None => {
                original_author = commit_author(source, commit)?;
                (original_author.0.as_str(), original_author.1.as_str())
            }
        };
        target.add_and_commit_as(
            &["."],
            &format!("sync: replay quarantined commit {short}"),
            Some(replay_author),
        )?;
        replayed_non_skill_commits = replayed_non_skill_commits.saturating_add(1);
    }
    Ok(replayed_non_skill_commits)
}

fn commit_author(repo: &GitStorage, commit: &str) -> Result<(String, String), SkillSyncError> {
    let output = repo.run_git_capture(&["show", "-s", "--format=%an%x00%ae", commit])?;
    let (name, email) = output.trim_end().split_once('\0').ok_or_else(|| {
        SkillSyncError::LocalQuarantineBlocked(format!(
            "commit {commit} has malformed author metadata"
        ))
    })?;
    Ok((name.to_owned(), email.to_owned()))
}

fn apply_replay_patch(
    source: &GitStorage,
    target: &GitStorage,
    parent: &str,
    commit: &str,
    patch: &[u8],
) -> Result<(), SkillSyncError> {
    let mut patch_file = tempfile::NamedTempFile::new_in(target.root())
        .map_err(|error| quarantine_error("create replay patch", error))?;
    patch_file
        .write_all(patch)
        .and_then(|()| patch_file.flush())
        .map_err(|error| quarantine_error("write replay patch", error))?;
    let patch_path = path_string(patch_file.path())?;
    match run_git(
        &[
            "apply",
            "--3way",
            "--index",
            "--whitespace=nowarn",
            "--",
            &patch_path,
        ],
        target.root(),
    ) {
        Ok(_) => Ok(()),
        Err(apply_error) => {
            let conflicts =
                target.run_git_capture(&["diff", "--name-only", "--diff-filter=U", "--"])?;
            let conflict_paths: Vec<&str> =
                conflicts.lines().filter(|line| !line.is_empty()).collect();
            if conflict_paths.is_empty()
                || conflict_paths.iter().any(|path| {
                    is_managed_skill_path(path)
                        || (!path.ends_with(".thread") && !path.ends_with(".meta.yaml"))
                })
            {
                return Err(SkillSyncError::LocalQuarantineBlocked(format!(
                    "ordinary replay conflict for commit {commit}: {apply_error}"
                )));
            }
            let thread_conflicts: BTreeSet<&str> = conflict_paths
                .iter()
                .copied()
                .filter(|path| path.ends_with(".thread"))
                .collect();
            let additions = source
                .diff_range(parent, commit)?
                .into_iter()
                .filter(|(path, _)| {
                    thread_conflicts
                        .iter()
                        .any(|conflict| path == Path::new(conflict))
                })
                .collect();
            for path in &conflict_paths {
                if path.ends_with(".thread") {
                    let content = target.show_file_at_ref("HEAD", path)?.ok_or_else(|| {
                        SkillSyncError::LocalQuarantineBlocked(format!(
                            "thread conflict removed existing path {path}"
                        ))
                    })?;
                    fs::write(target.root().join(path), content)
                        .map_err(|error| quarantine_error("restore thread conflict base", error))?;
                } else {
                    resolve_replay_meta(source, target, commit, path)?;
                }
            }
            if !thread_conflicts.is_empty() {
                let (resolved, _) = conflict::resolve_content(&additions, target.root())
                    .map_err(|error| SkillSyncError::LocalQuarantineBlocked(error.to_string()))?;
                for file in resolved {
                    fs::write(target.root().join(&file.path), file.content)
                        .map_err(|error| quarantine_error("write resolved replay thread", error))?;
                    let path = path_string(&file.path)?;
                    run_git(&["add", "--", &path], target.root())?;
                }
            }
            if !target
                .run_git_capture(&["diff", "--name-only", "--diff-filter=U", "--"])?
                .trim()
                .is_empty()
            {
                return Err(SkillSyncError::LocalQuarantineBlocked(
                    "thread replay left unresolved paths".to_owned(),
                ));
            }
            Ok(())
        }
    }
}

fn resolve_replay_meta(
    source: &GitStorage,
    target: &GitStorage,
    commit: &str,
    path: &str,
) -> Result<(), SkillSyncError> {
    let local = source.show_file_at_ref(commit, path)?.ok_or_else(|| {
        SkillSyncError::LocalQuarantineBlocked(format!("meta conflict removed local path {path}"))
    })?;
    let content = if path.starts_with("channels/") {
        let remote = target.show_file_at_ref("HEAD", path)?.ok_or_else(|| {
            SkillSyncError::LocalQuarantineBlocked(format!(
                "meta conflict removed upstream path {path}"
            ))
        })?;
        let local_meta: gitim_core::types::ChannelMeta =
            serde_yaml::from_str(&local).map_err(|error| {
                SkillSyncError::LocalQuarantineBlocked(format!(
                    "parse local channel metadata {path}: {error}"
                ))
            })?;
        let remote_meta: gitim_core::types::ChannelMeta =
            serde_yaml::from_str(&remote).map_err(|error| {
                SkillSyncError::LocalQuarantineBlocked(format!(
                    "parse upstream channel metadata {path}: {error}"
                ))
            })?;
        serde_yaml::to_string(&conflict::merge_channel_meta(&local_meta, &remote_meta)).map_err(
            |error| {
                SkillSyncError::LocalQuarantineBlocked(format!(
                    "serialize merged channel metadata {path}: {error}"
                ))
            },
        )?
    } else {
        local
    };
    fs::write(target.root().join(path), content)
        .map_err(|error| quarantine_error("write resolved replay metadata", error))?;
    run_git(&["add", "--", path], target.root())?;
    Ok(())
}

fn verify_replayed_result(
    repo: &GitStorage,
    journal: &QuarantineJournal,
    repaired: &str,
) -> Result<(), SkillSyncError> {
    verify_managed_roots(repo, &journal.upstream_oid, repaired)?;
    if repo.show_file_at_ref(&journal.upstream_oid, crate::rotate::EPOCH_FILE)?
        != repo.show_file_at_ref(repaired, crate::rotate::EPOCH_FILE)?
    {
        return Err(SkillSyncError::LocalQuarantineBlocked(
            "replayed result changed accepted epoch metadata".to_owned(),
        ));
    }
    let merge_base = match journal.replay_base.as_deref() {
        Some(replay_base) => replay_base.to_owned(),
        None => merge_base(repo, &journal.upstream_oid, &journal.original_head)?,
    };
    if merge_base == journal.upstream_oid && journal.kind == QuarantineKind::SkillHistory {
        verify_non_skill_tree_equivalence(repo, &journal.original_head, repo, repaired)?;
    }
    Ok(())
}

fn verify_managed_roots(
    repo: &GitStorage,
    accepted: &str,
    candidate: &str,
) -> Result<(), SkillSyncError> {
    for root in ["skills", "archive/skills"] {
        let accepted_oid = tree_oid(repo, accepted, root)?;
        let candidate_oid = tree_oid(repo, candidate, root)?;
        if accepted_oid != candidate_oid {
            return Err(SkillSyncError::LocalQuarantineBlocked(format!(
                "replayed {root} tree differs from accepted upstream"
            )));
        }
    }
    Ok(())
}

fn verify_non_skill_tree_equivalence(
    expected_repo: &GitStorage,
    expected: &str,
    actual_repo: &GitStorage,
    actual: &str,
) -> Result<(), SkillSyncError> {
    let expected_files = non_skill_tree(expected_repo, expected)?;
    let actual_files = non_skill_tree(actual_repo, actual)?;
    if expected_files != actual_files {
        return Err(SkillSyncError::LocalQuarantineBlocked(
            "replayed ordinary tree differs from quarantined history".to_owned(),
        ));
    }
    Ok(())
}

fn non_skill_tree(
    repo: &GitStorage,
    revision: &str,
) -> Result<BTreeMap<String, (String, String)>, SkillSyncError> {
    let output = run_git(
        &["ls-tree", "-r", "-z", "--full-tree", revision],
        repo.root(),
    )?;
    let mut files = BTreeMap::new();
    for record in output.stdout.split(|byte| *byte == 0) {
        if record.is_empty() {
            continue;
        }
        let tab = record
            .iter()
            .position(|byte| *byte == b'\t')
            .ok_or_else(|| {
                SkillSyncError::LocalQuarantineBlocked("malformed replay tree".to_owned())
            })?;
        let metadata = std::str::from_utf8(&record[..tab]).map_err(|error| {
            SkillSyncError::LocalQuarantineBlocked(format!("non-UTF-8 replay tree: {error}"))
        })?;
        let path = std::str::from_utf8(&record[tab + 1..]).map_err(|error| {
            SkillSyncError::LocalQuarantineBlocked(format!("non-UTF-8 replay path: {error}"))
        })?;
        if is_managed_skill_path(path) {
            continue;
        }
        let mut fields = metadata.split_whitespace();
        let mode = fields.next().unwrap_or_default().to_owned();
        let _kind = fields.next();
        let oid = fields.next().unwrap_or_default().to_owned();
        files.insert(path.to_owned(), (mode, oid));
    }
    Ok(files)
}

fn changed_paths(repo: &GitStorage, from: &str, to: &str) -> Result<Vec<String>, SkillSyncError> {
    Ok(repo
        .changed_files_range(from, to)?
        .into_iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect())
}

fn is_managed_skill_path(path: &str) -> bool {
    path == "skills"
        || path.starts_with("skills/")
        || path == "archive/skills"
        || path.starts_with("archive/skills/")
}

fn tree_oid(
    repo: &GitStorage,
    revision: &str,
    path: &str,
) -> Result<Option<String>, SkillSyncError> {
    let spec = format!("{revision}:{path}");
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "--end-of-options", &spec])
        .current_dir(repo.root())
        .output()
        .map_err(GitError::Io)?;
    if output.status.success() {
        Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        ))
    } else {
        Ok(None)
    }
}

fn revision_oid(repo: &GitStorage, revision: &str) -> Result<Option<String>, SkillSyncError> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "--end-of-options", revision])
        .current_dir(repo.root())
        .output()
        .map_err(GitError::Io)?;
    if output.status.success() {
        Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        ))
    } else {
        Ok(None)
    }
}

fn ensure_ref_equals(
    repo: &GitStorage,
    reference: &str,
    expected_oid: &str,
) -> Result<(), SkillSyncError> {
    if revision_oid(repo, reference)?.as_deref() != Some(expected_oid) {
        return Err(SkillSyncError::Git(GitError::PushConflict));
    }
    Ok(())
}

fn ensure_exact_ref(repo: &GitStorage, reference: &str, oid: &str) -> Result<(), SkillSyncError> {
    match revision_oid(repo, reference)? {
        Some(existing) if existing == oid => Ok(()),
        Some(_) => Err(SkillSyncError::Git(GitError::PushConflict)),
        None => {
            let zero_oid = "0".repeat(oid.len());
            run_git(&["update-ref", reference, oid, &zero_oid], repo.root())?;
            Ok(())
        }
    }
}

struct CapturedRemoteRef {
    branch: String,
    reference: String,
    oid: String,
}

struct CapturedLocalRef {
    reference: String,
    oid: Option<String>,
}

struct PendingQuarantinePush {
    operation_id: String,
    branch: String,
    repaired_head: String,
    quarantine_ref: String,
}

fn ensure_optional_ref_equals(
    repo: &GitStorage,
    reference: &str,
    expected_oid: Option<&str>,
) -> Result<(), SkillSyncError> {
    if revision_oid(repo, reference)?.as_deref() != expected_oid {
        return Err(SkillSyncError::Git(GitError::PushConflict));
    }
    Ok(())
}

fn is_ancestor(
    repo: &GitStorage,
    possible_ancestor: &str,
    descendant: &str,
) -> Result<bool, SkillSyncError> {
    let output = std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", possible_ancestor, descendant])
        .current_dir(repo.root())
        .output()
        .map_err(GitError::Io)?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(SkillSyncError::Git(GitError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ))),
    }
}

fn merge_base(repo: &GitStorage, left: &str, right: &str) -> Result<String, SkillSyncError> {
    let base = repo
        .run_git_capture(&["merge-base", left, right])?
        .trim()
        .to_owned();
    validate_oid(&base)?;
    Ok(base)
}

fn ensure_clean_tracked_worktree(repo: &GitStorage) -> Result<(), SkillSyncError> {
    if repo.has_dirty_tracked_files()? {
        return Err(SkillSyncError::LocalQuarantineBlocked(
            "tracked working-tree changes block a guarded rewrite".to_owned(),
        ));
    }
    Ok(())
}

fn ensure_current_branch(repo: &GitStorage, expected_branch: &str) -> Result<(), SkillSyncError> {
    if repo.current_branch()? != expected_branch {
        return Err(SkillSyncError::Git(GitError::PushConflict));
    }
    Ok(())
}

fn ensure_exact_checkout(
    repo: &GitStorage,
    expected_branch: &str,
    expected_head: &str,
) -> Result<(), SkillSyncError> {
    if repo.current_branch()? != expected_branch || repo.rev_parse("HEAD")? != expected_head {
        return Err(SkillSyncError::Git(GitError::PushConflict));
    }
    Ok(())
}

fn ensure_semantic_recovery_branch(
    repo: &GitStorage,
    journal_branch: &str,
    active_branch: &str,
) -> Result<(), SkillSyncError> {
    let current_branch = repo.current_branch()?;
    if current_branch == journal_branch
        || (active_branch != journal_branch && current_branch == active_branch)
    {
        return Ok(());
    }
    Err(SkillSyncError::Git(GitError::PushConflict))
}

fn tracked_worktree_matches_commit(
    repo: &GitStorage,
    commit: &str,
) -> Result<bool, SkillSyncError> {
    let index_tree = repo.run_git_capture(&["write-tree"])?;
    let commit_tree = repo.rev_parse(&format!("{commit}^{{tree}}"))?;
    if index_tree.trim() != commit_tree {
        return Ok(false);
    }
    let output = std::process::Command::new("git")
        .args(["diff", "--quiet", "--exit-code", "--"])
        .current_dir(repo.root())
        .output()
        .map_err(GitError::Io)?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(SkillSyncError::Git(GitError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ))),
    }
}

fn reconcile_update_ref_only_residue(
    repo: &GitStorage,
    expected_branch: &str,
    current: &str,
    repaired: &str,
    expected: &str,
) -> Result<(), SkillSyncError> {
    ensure_current_branch(repo, expected_branch)?;
    if current == repaired
        && repo.has_dirty_tracked_files()?
        && tracked_worktree_matches_commit(repo, expected)?
    {
        repo.reset_hard_to(repaired)?;
    }
    Ok(())
}

fn capture_remote_ref(
    repo: &GitStorage,
    branch: &str,
) -> Result<CapturedRemoteRef, SkillSyncError> {
    let reference = format!("origin/{branch}");
    let oid = repo.rev_parse(&reference)?;
    Ok(CapturedRemoteRef {
        branch: branch.to_owned(),
        reference,
        oid,
    })
}

fn capture_epoch_chain(
    repo: &GitStorage,
    start_branch: &str,
) -> Result<Vec<CapturedRemoteRef>, SkillSyncError> {
    let mut branch = start_branch.to_owned();
    let mut captured = Vec::new();
    for _ in 0..crate::rotate::MAX_FOLLOW_HOPS {
        let remote_ref = capture_remote_ref(repo, &branch)?;
        let epoch = crate::rotate::epoch_file_at_ref(repo, &remote_ref.oid)
            .map_err(|error| SkillSyncError::EpochValidationBlocked(error.to_string()))?;
        let next = match epoch {
            Some(file) if file.status == gitim_core::epoch::EpochStatus::Redirected => Some(
                file.redirect
                    .ok_or_else(|| {
                        SkillSyncError::EpochValidationBlocked(
                            "redirected epoch has no redirect block".to_owned(),
                        )
                    })?
                    .target_branch,
            ),
            _ => None,
        };
        captured.push(remote_ref);
        match next {
            Some(target) => branch = target,
            None => return Ok(captured),
        }
    }
    Err(SkillSyncError::EpochValidationBlocked(format!(
        "redirect chain exceeded {} hops from {start_branch}",
        crate::rotate::MAX_FOLLOW_HOPS
    )))
}

fn history_touches_managed_skills(
    repo: &GitStorage,
    revision: &str,
) -> Result<bool, SkillSyncError> {
    let output = run_git(
        &[
            "log",
            "--format=",
            "--name-only",
            revision,
            "--",
            "skills",
            "archive/skills",
        ],
        repo.root(),
    )?;
    Ok(output.stdout.iter().any(|byte| !byte.is_ascii_whitespace()))
}

fn history_touches_managed_skills_between(
    repo: &GitStorage,
    upstream_oid: &str,
    validated_head: &str,
) -> Result<bool, SkillSyncError> {
    validate_oid(upstream_oid)?;
    validate_oid(validated_head)?;
    let range = format!("{upstream_oid}..{validated_head}");
    let commits = repo.run_git_capture(&["rev-list", "--topo-order", &range])?;
    for commit in commits.lines().filter(|line| !line.is_empty()) {
        validate_oid(commit)?;
        let paths = run_git(
            &[
                "diff-tree",
                "--root",
                "-m",
                "--no-commit-id",
                "--name-only",
                "-r",
                "-z",
                commit,
                "--",
                "skills",
                "archive/skills",
            ],
            repo.root(),
        )?;
        if paths.stdout.iter().any(|byte| *byte != 0) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn history_touches_epoch_file_between(
    repo: &GitStorage,
    base: &str,
    head: &str,
) -> Result<bool, SkillSyncError> {
    validate_oid(base)?;
    validate_oid(head)?;
    let range = format!("{base}..{head}");
    let commits = repo.run_git_capture(&["rev-list", "--topo-order", &range])?;
    for commit in commits.lines().filter(|line| !line.is_empty()) {
        validate_oid(commit)?;
        let paths = run_git(
            &[
                "diff-tree",
                "--root",
                "-m",
                "--no-commit-id",
                "--name-only",
                "-r",
                "-z",
                commit,
                "--",
                crate::rotate::EPOCH_FILE,
            ],
            repo.root(),
        )?;
        if paths.stdout.iter().any(|byte| *byte != 0) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn index_is_clean(repo: &GitStorage) -> Result<bool, SkillSyncError> {
    let output = std::process::Command::new("git")
        .args(["diff", "--cached", "--quiet", "--exit-code"])
        .current_dir(repo.root())
        .output()
        .map_err(GitError::Io)?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(SkillSyncError::Git(GitError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ))),
    }
}

fn update_working_branch(
    repo: &GitStorage,
    branch: &str,
    repaired: &str,
    expected_old: &str,
) -> Result<(), SkillSyncError> {
    ensure_current_branch(repo, branch)?;
    let branch_ref = format!("refs/heads/{branch}");
    run_git(
        &["update-ref", &branch_ref, repaired, expected_old],
        repo.root(),
    )?;
    run_git(&["reset", "--hard", repaired], repo.root())?;
    Ok(())
}

fn cleanup_worktree(repo: &GitStorage, worktree: &Path) -> Result<(), SkillSyncError> {
    if worktree.exists() {
        let worktree_string = path_string(worktree)?;
        let _ = run_git(
            &["worktree", "remove", "--force", "--", &worktree_string],
            repo.root(),
        );
        if worktree.exists() {
            fs::remove_dir_all(worktree)
                .map_err(|error| quarantine_error("remove quarantine worktree", error))?;
        }
    }
    run_git(&["worktree", "prune"], repo.root())?;
    Ok(())
}

fn path_string(path: &Path) -> Result<String, SkillSyncError> {
    path.to_str().map(str::to_owned).ok_or_else(|| {
        SkillSyncError::LocalQuarantineBlocked("quarantine path is not UTF-8".to_owned())
    })
}

fn validate_user_archive_preconditions(
    repo: &GitStorage,
    upstream: &str,
    local_head: &str,
) -> Result<(), SkillSyncError> {
    match inspect_user_archive_preconditions(repo, upstream, local_head)? {
        Some(violation) => Err(violation.error),
        None => Ok(()),
    }
}

struct UserArchiveViolation {
    commit: String,
    error_code: String,
    error: SkillSyncError,
}

fn inspect_user_archive_preconditions(
    repo: &GitStorage,
    upstream: &str,
    local_head: &str,
) -> Result<Option<UserArchiveViolation>, SkillSyncError> {
    let range = format!("{upstream}..{local_head}");
    let commits = repo.run_git_capture(&["rev-list", "--reverse", "--first-parent", &range])?;
    for commit in commits.lines().filter(|line| !line.is_empty()) {
        let parent = format!("{commit}^");
        let changes = repo.changed_files_range(&parent, commit)?;
        let archive_paths: Vec<(String, String)> = changes
            .iter()
            .filter_map(|path| {
                let path = path.to_string_lossy().into_owned();
                path.strip_prefix("archive/users/")
                    .and_then(|name| name.strip_suffix(".meta.yaml"))
                    .map(str::to_owned)
                    .map(|handler| (handler, path))
            })
            .collect();
        let mut archived_handlers = BTreeSet::new();
        for (handler, path) in archive_paths {
            if repo.show_file_at_ref(commit, &path)?.is_some() {
                archived_handlers.insert(handler);
            }
        }
        if archived_handlers.is_empty() {
            continue;
        }
        let message = repo.run_git_capture(&["show", "-s", "--format=%B", commit])?;
        let Some(observed) = message
            .lines()
            .find_map(|line| line.strip_prefix("Gitim-Skills-Tree: "))
        else {
            return Ok(Some(UserArchiveViolation {
                commit: commit.to_owned(),
                error_code: "missing_precondition".to_owned(),
                error: SkillSyncError::LocalQuarantineBlocked(
                    "user archive commit is missing its Skill semantic precondition".to_owned(),
                ),
            }));
        };
        let actual = tree_oid(repo, upstream, "skills")?.unwrap_or_else(|| "absent".to_owned());
        if observed != actual {
            return Ok(Some(UserArchiveViolation {
                commit: commit.to_owned(),
                error_code: "skill_tree_changed".to_owned(),
                error: SkillSyncError::Git(GitError::PushConflict),
            }));
        }
        for handler in archived_handlers {
            match ensure_handler_has_no_skill_role(repo, upstream, &handler) {
                Ok(()) => {}
                Err(SkillSyncError::Domain(SkillError::AdminRolePresent)) => {
                    return Ok(Some(UserArchiveViolation {
                        commit: commit.to_owned(),
                        error_code: "admin_role_present".to_owned(),
                        error: SkillSyncError::Domain(SkillError::AdminRolePresent),
                    }));
                }
                Err(SkillSyncError::Domain(SkillError::RolesPresent)) => {
                    return Ok(Some(UserArchiveViolation {
                        commit: commit.to_owned(),
                        error_code: "roles_present".to_owned(),
                        error: SkillSyncError::Domain(SkillError::RolesPresent),
                    }));
                }
                Err(error) => return Err(error),
            }
        }
    }
    Ok(None)
}

fn semantic_archive_error(code: &str) -> Result<SkillSyncError, SkillSyncError> {
    match code {
        "missing_precondition" => Ok(SkillSyncError::LocalQuarantineBlocked(
            "user archive commit is missing its Skill semantic precondition".to_owned(),
        )),
        "skill_tree_changed" => Ok(SkillSyncError::Git(GitError::PushConflict)),
        "admin_role_present" => Ok(SkillSyncError::Domain(SkillError::AdminRolePresent)),
        "roles_present" => Ok(SkillSyncError::Domain(SkillError::RolesPresent)),
        _ => Err(SkillSyncError::LocalQuarantineBlocked(
            "semantic archive journal has an unknown error code".to_owned(),
        )),
    }
}

fn ensure_handler_has_no_skill_role(
    repo: &GitStorage,
    revision: &str,
    handler: &str,
) -> Result<(), SkillSyncError> {
    if let Some(workspace) = repo.show_file_at_ref(revision, "skills/workspace.meta.yaml")? {
        let value: serde_yaml::Value = serde_yaml::from_str(&workspace).map_err(|error| {
            SkillSyncError::LocalQuarantineBlocked(format!(
                "parse workspace Skill metadata: {error}"
            ))
        })?;
        if yaml_sequence_contains(&value, "administrators", handler) {
            return Err(SkillSyncError::Domain(SkillError::AdminRolePresent));
        }
    }
    let output = run_git(
        &[
            "ls-tree",
            "-r",
            "--name-only",
            revision,
            "--",
            "skills",
            "archive/skills",
        ],
        repo.root(),
    )?;
    for path in String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|path| path.ends_with("/skill.meta.yaml"))
    {
        let Some(content) = repo.show_file_at_ref(revision, path)? else {
            continue;
        };
        let value: serde_yaml::Value = serde_yaml::from_str(&content).map_err(|error| {
            SkillSyncError::LocalQuarantineBlocked(format!("parse {path}: {error}"))
        })?;
        if yaml_sequence_contains(&value, "owners", handler)
            || yaml_sequence_contains(&value, "maintainers", handler)
        {
            return Err(SkillSyncError::Domain(SkillError::RolesPresent));
        }
    }
    Ok(())
}

fn yaml_sequence_contains(value: &serde_yaml::Value, key: &str, handler: &str) -> bool {
    value
        .get(key)
        .and_then(serde_yaml::Value::as_sequence)
        .is_some_and(|values| values.iter().any(|value| value.as_str() == Some(handler)))
}

fn quarantine_error(context: &str, error: impl std::fmt::Display) -> SkillSyncError {
    SkillSyncError::LocalQuarantineBlocked(format!("{context}: {error}"))
}

fn sync_directory(path: &Path) -> Result<(), SkillSyncError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| quarantine_error("sync journal directory", error))
}
