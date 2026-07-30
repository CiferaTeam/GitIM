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

#[derive(Clone)]
pub struct SkillSyncGuard {
    checkpoint: SkillCheckpointStore,
    journal_path: PathBuf,
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
            repo.push_working_branch_unchecked(&captured_branch, &head)?;
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
                if self.reconcile_published_quarantine_locked(repo, &journal, &upstream_oid)? {
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
                validate_user_archive_preconditions(repo, &upstream_oid, &original_head)?;
                let touched_managed =
                    history_touches_managed_skills_between(repo, &upstream_oid, &original_head)?;
                let changed_paths = changed_paths(repo, &upstream_oid, &original_head)?;
                if touched_managed {
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
                } else if changed_paths.is_empty() {
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
                repo.push_working_branch_unchecked(&captured_branch, &head)?;
                Ok(GuardedPushOutcome::Pushed)
            }
            PreparedPush::Quarantine(pending) => {
                repo.push_working_branch_unchecked(&pending.branch, &pending.repaired_head)?;
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
        }
    }

    pub fn guarded_integrate(
        &self,
        repo: &GitStorage,
        commit_lock: &Mutex<()>,
        operation: IntegrationOperation,
    ) -> Result<IncomingSkillValidation, SkillSyncError> {
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
        if self.load_journal()?.is_some() {
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
        let previous = self
            .checkpoint
            .load()?
            .unwrap_or_else(|| SkillValidationCheckpoint::empty(active_epoch));
        let validation = validate_incoming_skill_history(repo, &previous, fetched_tip)?;
        self.checkpoint.save(&validation.checkpoint)?;
        if !validation.checkpoint.conflicts.is_empty() {
            return Err(SkillSyncError::Domain(SkillError::SyncConflict));
        }
        Ok(validation)
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
            let repaired = replay_without_managed_skills(repo, &journal, author)?;
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
        if current != repaired && !is_ancestor(repo, repaired, &current)? {
            let expected = journal.expected_branch_head();
            if current != expected {
                return Err(SkillSyncError::Git(GitError::PushConflict));
            }
            ensure_clean_tracked_worktree(repo)?;
            update_working_branch(repo, &journal.branch, validated_upstream_oid, expected)?;
        }
        cleanup_quarantine_tail_refs(repo, journal, None)?;
        self.remove_journal()?;
        Ok(true)
    }

    fn complete_quarantine_locked(
        &self,
        repo: &GitStorage,
        pending: &PendingQuarantinePush,
    ) -> Result<(), SkillSyncError> {
        match self.load_journal()? {
            Some(journal)
                if journal.operation_id == pending.operation_id
                    && journal.phase == QuarantinePhase::Moved
                    && journal.repaired_head.as_deref() == Some(pending.repaired_head.as_str()) =>
            {
                cleanup_quarantine_tail_refs(repo, &journal, None)?;
                self.remove_journal()
            }
            Some(_) => Ok(()),
            None => Ok(()),
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
        })
    }

    fn expected_branch_head(&self) -> &str {
        self.branch_head
            .as_deref()
            .unwrap_or(self.original_head.as_str())
    }
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
    let expected_ref = format!("{QUARANTINE_REF_PREFIX}{}", journal.original_head);
    if journal.quarantine_ref != expected_ref {
        return Err(SkillSyncError::LocalQuarantineBlocked(
            "quarantine ref does not match operation identity".to_owned(),
        ));
    }
    if let Some(repaired) = &journal.repaired_head {
        validate_oid(repaired)?;
        repo.rev_parse(&format!("{repaired}^{{commit}}"))?;
    }
    if let Some(branch_head) = &journal.branch_head {
        validate_oid(branch_head)?;
        repo.rev_parse(&format!("{branch_head}^{{commit}}"))?;
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
            if repo.rev_parse(tail_ref)? != *tail_head || !is_ancestor(repo, tail_base, tail_head)?
            {
                return Err(SkillSyncError::LocalQuarantineBlocked(
                    "quarantine tail ref or ancestry is invalid".to_owned(),
                ));
            }
            verify_managed_roots(repo, tail_base, tail_head)?;
            if history_touches_managed_skills_between(repo, tail_base, tail_head)? {
                return Err(SkillSyncError::LocalQuarantineBlocked(
                    "quarantine tail touches managed Skill paths".to_owned(),
                ));
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

fn replay_without_managed_skills(
    repo: &GitStorage,
    journal: &QuarantineJournal,
    author: (&str, &str),
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
    let result = if journal.tail_head.is_some() {
        replay_quarantine_tail(repo, &replay_repo, journal, author)
    } else {
        replay_commits(repo, &replay_repo, journal, author)
    };
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
    author: (&str, &str),
) -> Result<String, SkillSyncError> {
    let merge_base = source
        .run_git_capture(&["merge-base", &journal.upstream_oid, &journal.original_head])?
        .trim()
        .to_owned();
    validate_oid(&merge_base)?;
    let replayed_non_skill_commits =
        replay_linear_range(source, target, &merge_base, &journal.original_head, author)?;
    let repaired = target.rev_parse("HEAD")?;
    if replayed_non_skill_commits == 0
        && changed_paths(source, &merge_base, &journal.original_head)?
            .iter()
            .any(|path| !is_managed_skill_path(path))
    {
        return Err(SkillSyncError::LocalQuarantineBlocked(
            "quarantine replay omitted ordinary changes".to_owned(),
        ));
    }
    verify_managed_roots(target, &journal.upstream_oid, &repaired)?;
    if merge_base == journal.upstream_oid {
        verify_non_skill_tree_equivalence(source, &journal.original_head, target, &repaired)?;
    }
    Ok(repaired)
}

fn replay_quarantine_tail(
    source: &GitStorage,
    target: &GitStorage,
    journal: &QuarantineJournal,
    author: (&str, &str),
) -> Result<String, SkillSyncError> {
    let (Some(tail_base), Some(tail_head)) =
        (journal.tail_base.as_deref(), journal.tail_head.as_deref())
    else {
        return target.rev_parse("HEAD").map_err(Into::into);
    };
    let replayed = replay_linear_range(source, target, tail_base, tail_head, author)?;
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

fn replay_linear_range(
    source: &GitStorage,
    target: &GitStorage,
    base: &str,
    head: &str,
    author: (&str, &str),
) -> Result<usize, SkillSyncError> {
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
        target.add_and_commit_as(
            &["."],
            &format!("sync: replay quarantined commit {short}"),
            Some(author),
        )?;
        replayed_non_skill_commits = replayed_non_skill_commits.saturating_add(1);
    }
    Ok(replayed_non_skill_commits)
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
    let merge_base = repo
        .run_git_capture(&["merge-base", &journal.upstream_oid, &journal.original_head])?
        .trim()
        .to_owned();
    if merge_base == journal.upstream_oid {
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

fn ensure_clean_tracked_worktree(repo: &GitStorage) -> Result<(), SkillSyncError> {
    if repo.has_dirty_tracked_files()? {
        return Err(SkillSyncError::LocalQuarantineBlocked(
            "tracked working-tree changes block a guarded rewrite".to_owned(),
        ));
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
        let observed = message
            .lines()
            .find_map(|line| line.strip_prefix("Gitim-Skills-Tree: "))
            .ok_or_else(|| {
                SkillSyncError::LocalQuarantineBlocked(
                    "user archive commit is missing its Skill semantic precondition".to_owned(),
                )
            })?;
        let actual = tree_oid(repo, upstream, "skills")?.unwrap_or_else(|| "absent".to_owned());
        if observed != actual {
            return Err(SkillSyncError::Git(GitError::PushConflict));
        }
        for handler in archived_handlers {
            ensure_handler_has_no_skill_role(repo, upstream, &handler)?;
        }
    }
    Ok(())
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
