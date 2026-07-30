use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use fs2::FileExt;
use gitim_core::epoch::{EpochFile, EpochStatus};
use gitim_core::skill::{
    validate_package_entries, validate_skill_commit, PackageEntry, PackageEntryKind, ProposalId,
    RequestId, RevisionId, SkillCommitEvidence, SkillConflictCheckpoint, SkillError, SkillMeta,
    SkillObjectSnapshot, SkillOperation, SkillProposalMeta, SkillProposalSnapshot,
    SkillPublicationMeta, SkillReceipt, SkillRepairAcceptedState, SkillRepositorySnapshot,
    SkillRevisionMeta, SkillRevisionSnapshot, SkillSlug, WorkspaceSkillMeta,
};
use gitim_core::types::{Handler, UserMeta};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::git_tree::{list_tree_recursive, tree_oid_at, GitTreeEntry};
use crate::git::{run_git_with_env_and_timeout, GitError, GitStorage};

const CHECKPOINT_SCHEMA_VERSION: u32 = 1;
const SKILL_GIT_TIMEOUT: Duration = Duration::from_secs(60);
const CHECKPOINT_MAX_BYTES: u64 = 4 * 1024 * 1024;
const WORKSPACE_CONFLICT_KEY: &str = "$workspace";
const EPOCH_PATH: &str = "gitim.epoch.yaml";
const MAX_EPOCH_DEPTH: usize = 32;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillValidationCheckpoint {
    pub schema_version: u32,
    pub active_epoch: String,
    pub last_scanned_tip: String,
    pub workspace_tree: Option<AcceptedTree>,
    pub skills: BTreeMap<String, AcceptedSkillState>,
    pub conflicts: BTreeMap<String, SkillConflict>,
}

impl SkillValidationCheckpoint {
    pub fn empty(active_epoch: impl Into<String>) -> Self {
        Self {
            schema_version: CHECKPOINT_SCHEMA_VERSION,
            active_epoch: active_epoch.into(),
            last_scanned_tip: String::new(),
            workspace_tree: None,
            skills: BTreeMap::new(),
            conflicts: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptedTree {
    pub commit_oid: String,
    pub tree_oid: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptedSkillState {
    pub tree: AcceptedTree,
    pub event_revision: u64,
    pub archived: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillConflict {
    pub rejected_commit: String,
    pub code: String,
    pub accepted_tree_oid: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcceptedSkillChange {
    pub slug: String,
    pub event_revision: u64,
    pub control_revision: u64,
}

#[derive(Clone, Debug)]
pub struct SkillCheckpointStore {
    pub path: PathBuf,
    pub lock_path: PathBuf,
}

#[derive(Debug, Error)]
pub enum SkillSyncError {
    #[error("git: {0}")]
    Git(#[from] GitError),
    #[error("skill: {0}")]
    Domain(#[from] SkillError),
    #[error("checkpoint: {0}")]
    Checkpoint(String),
    #[error("local quarantine blocked: {0}")]
    LocalQuarantineBlocked(String),
    #[error("epoch validation blocked: {0}")]
    EpochValidationBlocked(String),
}

impl SkillSyncError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Git(_) | Self::Checkpoint(_) => "skill_sync_conflict",
            Self::Domain(error) => error.code(),
            Self::LocalQuarantineBlocked(_) => "skill_local_quarantine_blocked",
            Self::EpochValidationBlocked(_) => "skill_epoch_validation_blocked",
        }
    }
}

#[derive(Debug)]
pub struct IncomingSkillValidation {
    pub checkpoint: SkillValidationCheckpoint,
    pub accepted_changes: Vec<AcceptedSkillChange>,
}

impl SkillCheckpointStore {
    pub fn new(repository_root: &Path) -> Result<Self, SkillSyncError> {
        let root = repository_root
            .canonicalize()
            .map_err(|error| checkpoint_error("canonicalize repository", error))?;
        let directory = root.join(".gitim");
        match fs::symlink_metadata(&directory) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(SkillSyncError::Checkpoint(
                    ".gitim must be a real directory".to_owned(),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&directory)
                    .map_err(|error| checkpoint_error("create .gitim", error))?;
            }
            Err(error) => return Err(checkpoint_error("inspect .gitim", error)),
        }
        Ok(Self {
            path: directory.join("skill-validation.json"),
            lock_path: directory.join("skill-validation.json.lock"),
        })
    }

    pub fn load(&self) -> Result<Option<SkillValidationCheckpoint>, SkillSyncError> {
        let lock = self.lock()?;
        let result = self.load_locked();
        FileExt::unlock(&lock).map_err(|error| checkpoint_error("unlock checkpoint", error))?;
        result
    }

    pub fn save(&self, checkpoint: &SkillValidationCheckpoint) -> Result<(), SkillSyncError> {
        validate_checkpoint(checkpoint)?;
        let lock = self.lock()?;
        let result = self.save_locked(checkpoint);
        FileExt::unlock(&lock).map_err(|error| checkpoint_error("unlock checkpoint", error))?;
        result
    }

    fn lock(&self) -> Result<File, SkillSyncError> {
        self.validate_paths()?;
        let file = open_regular(&self.lock_path, true)?;
        file.lock_exclusive()
            .map_err(|error| checkpoint_error("lock checkpoint", error))?;
        Ok(file)
    }

    fn load_locked(&self) -> Result<Option<SkillValidationCheckpoint>, SkillSyncError> {
        let mut file = match open_regular(&self.path, false) {
            Ok(file) => file,
            Err(SkillSyncError::Checkpoint(message))
                if message.starts_with("open checkpoint: not found") =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let metadata = file
            .metadata()
            .map_err(|error| checkpoint_error("stat checkpoint", error))?;
        if metadata.len() > CHECKPOINT_MAX_BYTES {
            return Err(SkillSyncError::Checkpoint(
                "checkpoint exceeds size limit".to_owned(),
            ));
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.read_to_end(&mut bytes)
            .map_err(|error| checkpoint_error("read checkpoint", error))?;
        let checkpoint: SkillValidationCheckpoint = serde_json::from_slice(&bytes)
            .map_err(|error| SkillSyncError::Checkpoint(format!("parse checkpoint: {error}")))?;
        validate_checkpoint(&checkpoint)?;
        Ok(Some(checkpoint))
    }

    fn save_locked(&self, checkpoint: &SkillValidationCheckpoint) -> Result<(), SkillSyncError> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| SkillSyncError::Checkpoint("checkpoint has no parent".to_owned()))?;
        let bytes = serde_json::to_vec_pretty(checkpoint).map_err(|error| {
            SkillSyncError::Checkpoint(format!("serialize checkpoint: {error}"))
        })?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)
            .map_err(|error| checkpoint_error("create checkpoint temp file", error))?;
        temporary
            .write_all(&bytes)
            .and_then(|()| temporary.write_all(b"\n"))
            .and_then(|()| temporary.flush())
            .map_err(|error| checkpoint_error("write checkpoint temp file", error))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temporary
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|error| checkpoint_error("chmod checkpoint temp file", error))?;
        }
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| checkpoint_error("sync checkpoint temp file", error))?;
        temporary
            .persist(&self.path)
            .map_err(|error| checkpoint_error("persist checkpoint", error.error))?;
        sync_parent_directory(parent)?;
        Ok(())
    }

    fn validate_paths(&self) -> Result<(), SkillSyncError> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| SkillSyncError::Checkpoint("checkpoint has no parent".to_owned()))?;
        if self.path.file_name().and_then(|name| name.to_str()) != Some("skill-validation.json")
            || self.lock_path != parent.join("skill-validation.json.lock")
        {
            return Err(SkillSyncError::Checkpoint(
                "checkpoint paths are not canonical".to_owned(),
            ));
        }
        let metadata = fs::symlink_metadata(parent)
            .map_err(|error| checkpoint_error("inspect checkpoint directory", error))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(SkillSyncError::Checkpoint(
                "checkpoint parent must be a real directory".to_owned(),
            ));
        }
        Ok(())
    }
}

pub fn validate_incoming_skill_history(
    repo: &GitStorage,
    previous: &SkillValidationCheckpoint,
    fetched_tip: &str,
) -> Result<IncomingSkillValidation, SkillSyncError> {
    validate_checkpoint(previous)?;
    let fetched_tip = resolve_commit(repo, fetched_tip)?;
    let marker = (!previous.last_scanned_tip.is_empty())
        .then(|| resolve_commit(repo, &previous.last_scanned_tip))
        .transpose()?;
    let mut replay = Replay::new(repo, marker.as_deref(), &previous.active_epoch);
    replay.segment(&fetched_tip, 0)?;

    let top_epoch = epoch_at(repo, &fetched_tip)?;
    if top_epoch
        .as_ref()
        .is_some_and(|epoch| epoch.status == EpochStatus::Redirected)
    {
        return Err(SkillSyncError::EpochValidationBlocked(
            "fetched tip is a sealed epoch".to_owned(),
        ));
    }
    if let Some(epoch) = &top_epoch {
        let active_ref = format!("refs/remotes/origin/{}", epoch.branch);
        let authoritative_tip = resolve_commit(repo, &active_ref).map_err(|error| {
            SkillSyncError::EpochValidationBlocked(format!(
                "active epoch ref {} is unavailable: {error}",
                epoch.branch
            ))
        })?;
        if authoritative_tip != fetched_tip {
            return Err(SkillSyncError::EpochValidationBlocked(
                "fetched tip does not match the active epoch ref".to_owned(),
            ));
        }
    }

    if let Some(ref marker) = marker {
        let observed = replay
            .marker_checkpoint
            .ok_or(SkillSyncError::Domain(SkillError::SyncConflict))?;
        if observed != *previous || *marker != previous.last_scanned_tip {
            return Err(SkillSyncError::Checkpoint(
                "checkpoint does not match retained history".to_owned(),
            ));
        }
    } else if previous.workspace_tree.is_some()
        || !previous.skills.is_empty()
        || !previous.conflicts.is_empty()
    {
        return Err(SkillSyncError::Checkpoint(
            "unanchored checkpoint contains accepted state".to_owned(),
        ));
    }

    let accepted_changes = replay
        .changes
        .into_iter()
        .filter(|record| {
            previous.last_scanned_tip.is_empty()
                || replay
                    .new_commits
                    .as_ref()
                    .is_some_and(|commits| commits.contains(&record.commit))
        })
        .map(|record| record.change)
        .collect();
    Ok(IncomingSkillValidation {
        checkpoint: replay.checkpoint,
        accepted_changes,
    })
}

struct Replay<'a> {
    repo: &'a GitStorage,
    marker: Option<&'a str>,
    marker_seen: bool,
    marker_checkpoint: Option<SkillValidationCheckpoint>,
    checkpoint: SkillValidationCheckpoint,
    accepted: MaterializedSnapshot,
    changes: Vec<RecordedChange>,
    new_commits: Option<BTreeSet<String>>,
    visited_roots: BTreeSet<String>,
}

struct RecordedChange {
    commit: String,
    change: AcceptedSkillChange,
}

#[derive(Clone)]
struct MaterializedSnapshot {
    snapshot: SkillRepositorySnapshot,
    modes: BTreeMap<String, String>,
}

#[derive(Clone)]
struct TreeMaterial {
    files: BTreeMap<String, Vec<u8>>,
    modes: BTreeMap<String, String>,
    active_users: BTreeSet<String>,
}

struct CommitMetadata {
    oid: String,
    parents: Vec<String>,
    author: String,
    message: String,
}

impl<'a> Replay<'a> {
    fn new(repo: &'a GitStorage, marker: Option<&'a str>, active_epoch: &str) -> Self {
        Self {
            repo,
            marker,
            marker_seen: marker.is_none(),
            marker_checkpoint: None,
            checkpoint: SkillValidationCheckpoint::empty(active_epoch),
            accepted: MaterializedSnapshot {
                snapshot: SkillRepositorySnapshot::default(),
                modes: BTreeMap::new(),
            },
            changes: Vec::new(),
            new_commits: marker.map(|_| BTreeSet::new()),
            visited_roots: BTreeSet::new(),
        }
    }

    fn segment(&mut self, tip: &str, depth: usize) -> Result<(), SkillSyncError> {
        if depth >= MAX_EPOCH_DEPTH {
            return Err(SkillSyncError::EpochValidationBlocked(
                "epoch lineage exceeds maximum depth".to_owned(),
            ));
        }
        let chain = first_parent_chain(self.repo, tip)?;
        let relevant = relevant_commits(self.repo, tip)?;
        let root = chain.first().ok_or_else(|| {
            SkillSyncError::EpochValidationBlocked("empty commit lineage".to_owned())
        })?;
        if !self.visited_roots.insert(root.clone()) {
            return Err(SkillSyncError::EpochValidationBlocked(
                "epoch lineage cycle".to_owned(),
            ));
        }
        let root_epoch = epoch_at(self.repo, root)?;
        match root_epoch {
            None => {
                if depth != 0 && !self.checkpoint.last_scanned_tip.is_empty() {
                    return Err(SkillSyncError::EpochValidationBlocked(
                        "epoch predecessor root lacks active metadata".to_owned(),
                    ));
                }
                let material = load_tree_material(self.repo, root)?;
                let snapshot = parse_snapshot(&material)?;
                if !snapshot.repository_files.is_empty() {
                    return Err(SkillSyncError::EpochValidationBlocked(
                        "initial root already contains Skill state".to_owned(),
                    ));
                }
                self.accepted = MaterializedSnapshot {
                    snapshot,
                    modes: material.modes,
                };
                self.advance(root);
            }
            Some(epoch) if epoch.status == EpochStatus::Active => {
                let snapshot = epoch.snapshot.as_ref().ok_or_else(|| {
                    SkillSyncError::EpochValidationBlocked(
                        "active epoch lacks snapshot metadata".to_owned(),
                    )
                })?;
                validate_branch_name(&epoch.branch)?;
                validate_branch_name(&snapshot.source_branch)?;
                let source_commit = resolve_commit(self.repo, &snapshot.source_commit)?;
                self.segment(&source_commit, depth + 1)?;
                let seal_ref = format!("refs/remotes/origin/{}", snapshot.source_branch);
                let seal_oid = resolve_commit(self.repo, &seal_ref).map_err(|error| {
                    SkillSyncError::EpochValidationBlocked(format!(
                        "retained predecessor {} is unavailable: {error}",
                        snapshot.source_branch
                    ))
                })?;
                let seal = commit_metadata(self.repo, &seal_oid)?;
                if seal.parents.as_slice() != [source_commit.as_str()]
                    || changed_paths(self.repo, &seal)? != BTreeSet::from([EPOCH_PATH.to_owned()])
                {
                    return Err(SkillSyncError::EpochValidationBlocked(
                        "predecessor seal is malformed".to_owned(),
                    ));
                }
                let redirect = epoch_at(self.repo, &seal_oid)?
                    .filter(|value| value.status == EpochStatus::Redirected)
                    .and_then(|value| value.redirect)
                    .ok_or_else(|| {
                        SkillSyncError::EpochValidationBlocked(
                            "predecessor seal lacks redirect".to_owned(),
                        )
                    })?;
                if redirect.target_epoch != epoch.epoch
                    || redirect.target_branch != epoch.branch
                    || resolve_commit(self.repo, &redirect.snapshot_of)? != source_commit
                    || redirect.target_commit != snapshot.commit
                {
                    return Err(SkillSyncError::EpochValidationBlocked(
                        "epoch redirect does not match active snapshot".to_owned(),
                    ));
                }
                let snapshot_commit = resolve_commit(self.repo, &snapshot.commit)?;
                if snapshot_commit != source_commit && snapshot_commit != *root {
                    return Err(SkillSyncError::EpochValidationBlocked(
                        "active snapshot commit is unrelated".to_owned(),
                    ));
                }
                self.advance(&seal_oid);

                let material = load_tree_material(self.repo, root)?;
                let root_snapshot = parse_snapshot(&material)?;
                if !self.checkpoint.conflicts.is_empty()
                    || root_snapshot.repository_files != self.accepted.snapshot.repository_files
                    || root_snapshot.active_users != self.accepted.snapshot.active_users
                {
                    return Err(SkillSyncError::EpochValidationBlocked(
                        "orphan snapshot differs from accepted predecessor".to_owned(),
                    ));
                }
                self.accepted = MaterializedSnapshot {
                    snapshot: root_snapshot,
                    modes: material.modes,
                };
                self.checkpoint.active_epoch = epoch.branch;
                self.rollover(root)?;
                self.advance(root);
            }
            Some(_) => {
                return Err(SkillSyncError::EpochValidationBlocked(
                    "epoch root is redirected".to_owned(),
                ));
            }
        }

        for commit in chain.iter().skip(1) {
            if relevant.contains(commit) {
                self.process_commit(commit)?;
            } else {
                self.advance(commit);
            }
        }
        if let Some(root_epoch) = epoch_at(self.repo, root)? {
            if epoch_at(self.repo, tip)? != Some(root_epoch) {
                return Err(SkillSyncError::EpochValidationBlocked(
                    "active epoch metadata changed inside an epoch".to_owned(),
                ));
            }
        } else if epoch_at(self.repo, tip)?.is_some() {
            return Err(SkillSyncError::EpochValidationBlocked(
                "legacy lineage contains epoch metadata".to_owned(),
            ));
        }
        Ok(())
    }

    fn process_commit(&mut self, commit: &str) -> Result<(), SkillSyncError> {
        let metadata = commit_metadata(self.repo, commit)?;
        let paths = changed_paths(self.repo, &metadata)?;
        if paths.contains(EPOCH_PATH) {
            return Err(SkillSyncError::EpochValidationBlocked(
                "epoch metadata changed outside a seal".to_owned(),
            ));
        }
        let skill_affecting = paths.iter().any(|path| managed_skill_path(path));
        if !skill_affecting {
            let material = load_tree_material(self.repo, commit)?;
            self.accepted.snapshot.active_users = material.active_users;
            self.advance(commit);
            return Ok(());
        }

        let actual_after = load_tree_material(self.repo, commit)?;
        let receipt = changed_receipt(&paths, &actual_after).ok();
        let affected = affected_scopes(&paths, receipt.as_ref());
        let result = receipt
            .as_ref()
            .ok_or(SkillError::SyncConflict)
            .and_then(|receipt| {
                if receipt.operation == SkillOperation::RepairSkillState {
                    self.validate_repair(&metadata, &paths, receipt, &actual_after)
                } else {
                    self.validate_normal(&metadata, &paths, receipt, &actual_after)
                }
            });
        match result {
            Ok((projected, outcome)) => {
                self.accepted = projected;
                self.checkpoint.last_scanned_tip = commit.to_owned();
                if let Some(slug) = outcome.changed_skill {
                    let slug_text = slug.as_str().to_owned();
                    self.checkpoint.conflicts.remove(&slug_text);
                    self.update_skill_pointer(commit, &slug)?;
                    if let (Some(event_revision), Some(control_revision)) =
                        (outcome.event_revision, outcome.control_revision)
                    {
                        self.changes.push(RecordedChange {
                            commit: commit.to_owned(),
                            change: AcceptedSkillChange {
                                slug: slug_text,
                                event_revision,
                                control_revision,
                            },
                        });
                    }
                } else {
                    self.checkpoint.conflicts.remove(WORKSPACE_CONFLICT_KEY);
                    self.update_workspace_pointer(commit)?;
                }
                self.capture_marker(commit);
            }
            Err(error) => {
                for key in affected {
                    let accepted_tree_oid = if key == WORKSPACE_CONFLICT_KEY {
                        self.checkpoint
                            .workspace_tree
                            .as_ref()
                            .map(|tree| tree.tree_oid.clone())
                    } else {
                        self.checkpoint
                            .skills
                            .get(&key)
                            .map(|state| state.tree.tree_oid.clone())
                    };
                    self.checkpoint
                        .conflicts
                        .entry(key)
                        .or_insert_with(|| SkillConflict {
                            rejected_commit: commit.to_owned(),
                            code: error.code().to_owned(),
                            accepted_tree_oid,
                        });
                }
                self.advance(commit);
            }
        }
        Ok(())
    }

    fn validate_normal(
        &self,
        metadata: &CommitMetadata,
        paths: &BTreeSet<String>,
        receipt: &SkillReceipt,
        actual_after: &TreeMaterial,
    ) -> Result<
        (
            MaterializedSnapshot,
            gitim_core::skill::SkillTransitionOutcome,
        ),
        SkillError,
    > {
        let mut before = self.accepted.snapshot.clone();
        if receipt
            .skill
            .as_ref()
            .is_some_and(|slug| self.checkpoint.conflicts.contains_key(slug.as_str()))
            || (receipt.skill.is_none()
                && self
                    .checkpoint
                    .conflicts
                    .contains_key(WORKSPACE_CONFLICT_KEY))
        {
            return Err(SkillError::SyncConflict);
        }
        before.conflict_checkpoint = None;
        let projected = project_after(&self.accepted, actual_after, paths)?;
        let evidence = evidence(metadata, paths, receipt)?;
        let outcome = validate_skill_commit(&before, &projected.snapshot, &evidence)?;
        Ok((projected, outcome))
    }

    fn validate_repair(
        &self,
        metadata: &CommitMetadata,
        paths: &BTreeSet<String>,
        receipt: &SkillReceipt,
        actual_after: &TreeMaterial,
    ) -> Result<
        (
            MaterializedSnapshot,
            gitim_core::skill::SkillTransitionOutcome,
        ),
        SkillError,
    > {
        let key = receipt
            .skill
            .as_ref()
            .map_or(WORKSPACE_CONFLICT_KEY, SkillSlug::as_str);
        let conflict = self
            .checkpoint
            .conflicts
            .get(key)
            .ok_or(SkillError::SyncConflict)?;
        let parent = metadata.parents.first().ok_or(SkillError::SyncConflict)?;
        let actual_parent =
            load_tree_material(self.repo, parent).map_err(|_| SkillError::SyncConflict)?;
        let mut before = self.accepted.clone();
        let accepted_state = repair_accepted_state(&before.snapshot, receipt)?;
        let accepted_files = scope_files(&before.snapshot.repository_files, receipt.skill.as_ref());
        overlay_scope(
            &mut before.snapshot.repository_files,
            &mut before.modes,
            &actual_parent,
            receipt.skill.as_ref(),
        );
        let changed_paths = raw_scope_diff(
            &before.snapshot.repository_files,
            &accepted_files,
            receipt.skill.as_ref(),
        );
        before.snapshot.conflict_checkpoint = Some(SkillConflictCheckpoint {
            conflict_tip: conflict.rejected_commit.clone(),
            accepted_tree: conflict
                .accepted_tree_oid
                .clone()
                .ok_or(SkillError::SyncConflict)?,
            accepted_state,
            accepted_files,
            changed_paths,
        });
        let projected = project_after(&self.accepted, actual_after, paths)?;
        let evidence = evidence(metadata, paths, receipt)?;
        let outcome = validate_skill_commit(&before.snapshot, &projected.snapshot, &evidence)?;
        Ok((projected, outcome))
    }

    fn rollover(&mut self, commit: &str) -> Result<(), SkillSyncError> {
        if self.accepted.snapshot.workspace.is_some() {
            self.update_workspace_pointer(commit)?;
        }
        let skills: Vec<_> = self
            .accepted
            .snapshot
            .active_skills
            .keys()
            .chain(self.accepted.snapshot.archived_skills.keys())
            .cloned()
            .collect();
        for slug in skills {
            self.update_skill_pointer(commit, &slug)?;
        }
        Ok(())
    }

    fn update_workspace_pointer(&mut self, commit: &str) -> Result<(), SkillSyncError> {
        self.checkpoint.workspace_tree =
            tree_oid_at(self.repo, commit, "skills/workspace.meta.yaml")?.map(|tree_oid| {
                AcceptedTree {
                    commit_oid: commit.to_owned(),
                    tree_oid,
                }
            });
        Ok(())
    }

    fn update_skill_pointer(
        &mut self,
        commit: &str,
        slug: &SkillSlug,
    ) -> Result<(), SkillSyncError> {
        let (skill, archived, root) =
            if let Some(skill) = self.accepted.snapshot.active_skills.get(slug) {
                (skill, false, format!("skills/{}", slug.as_str()))
            } else if let Some(skill) = self.accepted.snapshot.archived_skills.get(slug) {
                (skill, true, format!("archive/skills/{}", slug.as_str()))
            } else {
                self.checkpoint.skills.remove(slug.as_str());
                return Ok(());
            };
        let tree_oid = tree_oid_at(self.repo, commit, &root)?.ok_or_else(|| {
            SkillSyncError::Checkpoint(format!("accepted Skill tree {root} is missing"))
        })?;
        self.checkpoint.skills.insert(
            slug.as_str().to_owned(),
            AcceptedSkillState {
                tree: AcceptedTree {
                    commit_oid: commit.to_owned(),
                    tree_oid,
                },
                event_revision: skill.meta.event_revision,
                archived,
            },
        );
        Ok(())
    }

    fn advance(&mut self, commit: &str) {
        self.checkpoint.last_scanned_tip = commit.to_owned();
        self.capture_marker(commit);
    }

    fn capture_marker(&mut self, commit: &str) {
        if !self.marker_seen && self.marker == Some(commit) {
            self.marker_seen = true;
            self.marker_checkpoint = Some(self.checkpoint.clone());
            return;
        }
        if self.marker_seen {
            if let Some(commits) = &mut self.new_commits {
                commits.insert(commit.to_owned());
            }
        }
    }
}

fn project_after(
    accepted: &MaterializedSnapshot,
    actual_after: &TreeMaterial,
    changed_paths: &BTreeSet<String>,
) -> Result<MaterializedSnapshot, SkillError> {
    let mut files = accepted.snapshot.repository_files.clone();
    let mut modes = accepted.modes.clone();
    for path in changed_paths.iter().filter(|path| managed_skill_path(path)) {
        if let Some(bytes) = actual_after.files.get(path) {
            files.insert(path.clone(), bytes.clone());
            if let Some(mode) = actual_after.modes.get(path) {
                modes.insert(path.clone(), mode.clone());
            }
        } else {
            files.remove(path);
            modes.remove(path);
        }
    }
    let material = TreeMaterial {
        files,
        modes,
        active_users: actual_after.active_users.clone(),
    };
    let snapshot = parse_snapshot(&material)?;
    Ok(MaterializedSnapshot {
        snapshot,
        modes: material.modes,
    })
}

fn evidence(
    metadata: &CommitMetadata,
    changed_paths: &BTreeSet<String>,
    receipt: &SkillReceipt,
) -> Result<SkillCommitEvidence, SkillError> {
    let trailer = request_trailer(&metadata.message)?;
    Ok(SkillCommitEvidence {
        commit_author: metadata.author.clone(),
        request_trailer: trailer,
        parent_count: metadata.parents.len(),
        receipt: receipt.clone(),
        changed_paths: changed_paths.clone(),
    })
}

fn changed_receipt(
    changed_paths: &BTreeSet<String>,
    material: &TreeMaterial,
) -> Result<SkillReceipt, SkillError> {
    let paths: Vec<_> = changed_paths
        .iter()
        .filter(|path| receipt_id_from_path(path).is_some())
        .collect();
    if paths.len() != 1 {
        return Err(SkillError::SyncConflict);
    }
    let path = paths[0];
    let bytes = material.files.get(path).ok_or(SkillError::SyncConflict)?;
    let receipt: SkillReceipt =
        serde_yaml::from_slice(bytes).map_err(|_| SkillError::SyncConflict)?;
    if receipt_id_from_path(path).as_ref() != Some(&receipt.id) {
        return Err(SkillError::SyncConflict);
    }
    Ok(receipt)
}

fn request_trailer(message: &str) -> Result<RequestId, SkillError> {
    let values: Vec<_> = message
        .lines()
        .filter_map(|line| line.strip_prefix("Gitim-Request-Id: "))
        .collect();
    if values.len() != 1 {
        return Err(SkillError::SyncConflict);
    }
    RequestId::new(values[0]).map_err(|_| SkillError::SyncConflict)
}

fn affected_scopes(paths: &BTreeSet<String>, receipt: Option<&SkillReceipt>) -> BTreeSet<String> {
    let mut affected = BTreeSet::new();
    if let Some(slug) = receipt.and_then(|receipt| receipt.skill.as_ref()) {
        affected.insert(slug.as_str().to_owned());
    }
    for path in paths {
        let components: Vec<_> = path.split('/').collect();
        let slug = match components.as_slice() {
            ["skills", slug, ..] if *slug != "receipts" && *slug != "workspace.meta.yaml" => {
                Some(*slug)
            }
            ["archive", "skills", slug, ..] => Some(*slug),
            _ => None,
        };
        if let Some(slug) = slug.and_then(|value| SkillSlug::new(value).ok()) {
            affected.insert(slug.as_str().to_owned());
        }
        if path == "skills/workspace.meta.yaml" {
            affected.insert(WORKSPACE_CONFLICT_KEY.to_owned());
        }
    }
    if affected.is_empty() {
        affected.insert(WORKSPACE_CONFLICT_KEY.to_owned());
    }
    affected
}

fn repair_accepted_state(
    snapshot: &SkillRepositorySnapshot,
    receipt: &SkillReceipt,
) -> Result<SkillRepairAcceptedState, SkillError> {
    match &receipt.skill {
        None => snapshot
            .workspace
            .clone()
            .map(SkillRepairAcceptedState::Workspace)
            .ok_or(SkillError::SyncConflict),
        Some(slug) => snapshot
            .active_skills
            .get(slug)
            .cloned()
            .map(|skill| SkillRepairAcceptedState::ActiveSkill {
                slug: slug.clone(),
                skill,
            })
            .or_else(|| {
                snapshot.archived_skills.get(slug).cloned().map(|skill| {
                    SkillRepairAcceptedState::ArchivedSkill {
                        slug: slug.clone(),
                        skill,
                    }
                })
            })
            .ok_or(SkillError::SyncConflict),
    }
}

fn overlay_scope(
    files: &mut BTreeMap<String, Vec<u8>>,
    modes: &mut BTreeMap<String, String>,
    source: &TreeMaterial,
    slug: Option<&SkillSlug>,
) {
    let belongs = |path: &str| scope_contains(path, slug);
    files.retain(|path, _| !belongs(path));
    modes.retain(|path, _| !belongs(path));
    files.extend(
        source
            .files
            .iter()
            .filter(|(path, _)| belongs(path))
            .map(|(path, bytes)| (path.clone(), bytes.clone())),
    );
    modes.extend(
        source
            .modes
            .iter()
            .filter(|(path, _)| belongs(path))
            .map(|(path, mode)| (path.clone(), mode.clone())),
    );
}

fn scope_files(
    files: &BTreeMap<String, Vec<u8>>,
    slug: Option<&SkillSlug>,
) -> BTreeMap<String, Vec<u8>> {
    files
        .iter()
        .filter(|(path, _)| scope_contains(path, slug))
        .map(|(path, bytes)| (path.clone(), bytes.clone()))
        .collect()
}

fn raw_scope_diff(
    actual: &BTreeMap<String, Vec<u8>>,
    accepted: &BTreeMap<String, Vec<u8>>,
    slug: Option<&SkillSlug>,
) -> BTreeSet<String> {
    actual
        .keys()
        .chain(accepted.keys())
        .filter(|path| scope_contains(path, slug) && actual.get(*path) != accepted.get(*path))
        .cloned()
        .collect()
}

fn scope_contains(path: &str, slug: Option<&SkillSlug>) -> bool {
    match slug {
        None => path == "skills/workspace.meta.yaml",
        Some(slug) => {
            path.starts_with(&format!("skills/{}/", slug.as_str()))
                || path.starts_with(&format!("archive/skills/{}/", slug.as_str()))
        }
    }
}

fn load_tree_material(repo: &GitStorage, commit: &str) -> Result<TreeMaterial, SkillSyncError> {
    let mut files = BTreeMap::new();
    let mut modes = BTreeMap::new();
    let mut active_users = BTreeSet::new();
    for entry in list_tree_recursive(repo, commit, "")? {
        validate_repository_path(&entry.path)?;
        let managed = managed_skill_path(&entry.path);
        let user = active_user_path(&entry.path);
        if !managed && user.is_none() {
            continue;
        }
        if entry.object_type != "blob" {
            return Err(SkillSyncError::Domain(SkillError::SyncConflict));
        }
        let bytes = read_blob_oid(repo, &entry)?;
        if managed {
            modes.insert(entry.path.clone(), entry.mode.clone());
            files.insert(entry.path, bytes);
        } else if let Some(handler) = user {
            if !matches!(entry.mode.as_str(), "100644" | "100755")
                || serde_yaml::from_slice::<UserMeta>(&bytes).is_err()
            {
                return Err(SkillSyncError::Domain(SkillError::SyncConflict));
            }
            active_users.insert(handler);
        }
    }
    Ok(TreeMaterial {
        files,
        modes,
        active_users,
    })
}

fn parse_snapshot(material: &TreeMaterial) -> Result<SkillRepositorySnapshot, SkillError> {
    let workspace = material
        .files
        .get("skills/workspace.meta.yaml")
        .map(|bytes| serde_yaml::from_slice::<WorkspaceSkillMeta>(bytes))
        .transpose()
        .map_err(|_| SkillError::SyncConflict)?;
    let mut receipts = BTreeMap::new();
    let mut active_slugs = BTreeSet::new();
    let mut archived_slugs = BTreeSet::new();
    for (path, bytes) in &material.files {
        if let Some(id) = receipt_id_from_path(path) {
            let receipt: SkillReceipt =
                serde_yaml::from_slice(bytes).map_err(|_| SkillError::SyncConflict)?;
            receipts.insert(id, receipt);
            continue;
        }
        let components: Vec<_> = path.split('/').collect();
        match components.as_slice() {
            ["skills", slug, ..] if *slug != "workspace.meta.yaml" && *slug != "receipts" => {
                active_slugs.insert(SkillSlug::new(slug).map_err(|_| SkillError::SyncConflict)?);
            }
            ["archive", "skills", slug, ..] => {
                archived_slugs.insert(SkillSlug::new(slug).map_err(|_| SkillError::SyncConflict)?);
            }
            _ => {}
        }
    }
    let active_skills = active_slugs
        .into_iter()
        .map(|slug| parse_skill_object(material, &slug, false).map(|skill| (slug, skill)))
        .collect::<Result<_, _>>()?;
    let archived_skills = archived_slugs
        .into_iter()
        .map(|slug| parse_skill_object(material, &slug, true).map(|skill| (slug, skill)))
        .collect::<Result<_, _>>()?;
    Ok(SkillRepositorySnapshot {
        workspace,
        active_skills,
        archived_skills,
        receipts,
        active_users: material.active_users.clone(),
        conflict_checkpoint: None,
        repository_files: material.files.clone(),
    })
}

fn parse_skill_object(
    material: &TreeMaterial,
    slug: &SkillSlug,
    archived: bool,
) -> Result<SkillObjectSnapshot, SkillError> {
    let root = if archived {
        format!("archive/skills/{}", slug.as_str())
    } else {
        format!("skills/{}", slug.as_str())
    };
    let meta: SkillMeta = parse_yaml_file(material, &format!("{root}/skill.meta.yaml"))?;
    let history = String::from_utf8(
        material
            .files
            .get(&format!("{root}/history.thread"))
            .cloned()
            .ok_or(SkillError::SyncConflict)?,
    )
    .map_err(|_| SkillError::SyncConflict)?;
    let mut revision_ids = BTreeSet::new();
    let mut publication_ids = BTreeSet::new();
    let mut proposal_ids = BTreeSet::new();
    for path in material.files.keys().filter(|path| path.starts_with(&root)) {
        let suffix = path
            .strip_prefix(&format!("{root}/"))
            .ok_or(SkillError::SyncConflict)?;
        let parts: Vec<_> = suffix.split('/').collect();
        match parts.as_slice() {
            ["revisions", id, ..] => {
                revision_ids.insert(RevisionId::new(id).map_err(|_| SkillError::SyncConflict)?);
            }
            ["publications", file] => {
                let id = file
                    .strip_suffix(".meta.yaml")
                    .ok_or(SkillError::SyncConflict)?;
                publication_ids.insert(RevisionId::new(id).map_err(|_| SkillError::SyncConflict)?);
            }
            ["proposals", id, ..] => {
                proposal_ids.insert(ProposalId::new(id).map_err(|_| SkillError::SyncConflict)?);
            }
            _ => {}
        }
    }
    let revisions = revision_ids
        .into_iter()
        .map(|id| {
            let revision_root = format!("{root}/revisions/{}", id.as_str());
            let meta: SkillRevisionMeta =
                parse_yaml_file(material, &format!("{revision_root}/revision.meta.yaml"))?;
            let package_prefix = format!("{revision_root}/package/");
            let entries = material
                .files
                .iter()
                .filter_map(|(path, bytes)| {
                    path.strip_prefix(&package_prefix).map(|relative| {
                        let kind = match material.modes.get(path).map(String::as_str) {
                            Some("100644" | "100755") => PackageEntryKind::Regular,
                            Some("120000") => PackageEntryKind::Symlink,
                            _ => PackageEntryKind::BlockDevice,
                        };
                        PackageEntry::with_kind(relative, bytes.clone(), kind)
                    })
                })
                .collect();
            let package = validate_package_entries(slug, entries)?;
            Ok((id, SkillRevisionSnapshot { meta, package }))
        })
        .collect::<Result<_, SkillError>>()?;
    let publications = publication_ids
        .into_iter()
        .map(|id| {
            let path = format!("{root}/publications/{}.meta.yaml", id.as_str());
            parse_yaml_file(material, &path).map(|publication| (id, publication))
        })
        .collect::<Result<BTreeMap<_, SkillPublicationMeta>, _>>()?;
    let proposals = proposal_ids
        .into_iter()
        .map(|id| {
            let proposal_root = format!("{root}/proposals/{}", id.as_str());
            let meta: SkillProposalMeta =
                parse_yaml_file(material, &format!("{proposal_root}/proposal.meta.yaml"))?;
            let discussion = String::from_utf8(
                material
                    .files
                    .get(&format!("{proposal_root}/discussion.thread"))
                    .cloned()
                    .ok_or(SkillError::SyncConflict)?,
            )
            .map_err(|_| SkillError::SyncConflict)?;
            Ok((id, SkillProposalSnapshot { meta, discussion }))
        })
        .collect::<Result<_, SkillError>>()?;
    Ok(SkillObjectSnapshot {
        meta,
        revisions,
        publications,
        proposals,
        history,
    })
}

fn parse_yaml_file<T: serde::de::DeserializeOwned>(
    material: &TreeMaterial,
    path: &str,
) -> Result<T, SkillError> {
    let bytes = material.files.get(path).ok_or(SkillError::SyncConflict)?;
    serde_yaml::from_slice(bytes).map_err(|_| SkillError::SyncConflict)
}

fn first_parent_chain(repo: &GitStorage, tip: &str) -> Result<Vec<String>, SkillSyncError> {
    let output = run_skill_git(repo, &["rev-list", "--first-parent", "--reverse", tip])?;
    let text = std::str::from_utf8(&output.stdout)
        .map_err(|_| SkillSyncError::Checkpoint("non-UTF-8 commit list".to_owned()))?;
    text.lines()
        .map(|line| validate_oid(line).map(|()| line.to_owned()))
        .collect()
}

fn relevant_commits(repo: &GitStorage, tip: &str) -> Result<BTreeSet<String>, SkillSyncError> {
    let output = run_skill_git(
        repo,
        &[
            "rev-list",
            "--first-parent",
            tip,
            "--",
            "skills",
            "archive/skills",
            "users",
            EPOCH_PATH,
        ],
    )?;
    let text = std::str::from_utf8(&output.stdout)
        .map_err(|_| SkillSyncError::Checkpoint("non-UTF-8 commit list".to_owned()))?;
    text.lines()
        .map(|line| validate_oid(line).map(|()| line.to_owned()))
        .collect()
}

fn commit_metadata(repo: &GitStorage, commit: &str) -> Result<CommitMetadata, SkillSyncError> {
    let format = "%H%x00%T%x00%P%x00%an%x00%B";
    let output = run_skill_git(repo, &["show", "-s", &format!("--format={format}"), commit])?;
    let fields: Vec<_> = output.stdout.splitn(5, |byte| *byte == 0).collect();
    if fields.len() != 5 {
        return Err(SkillSyncError::Checkpoint(
            "malformed commit metadata".to_owned(),
        ));
    }
    let field = |index: usize| {
        std::str::from_utf8(fields[index])
            .map(str::trim_end)
            .map(str::to_owned)
            .map_err(|_| SkillSyncError::Checkpoint("non-UTF-8 commit metadata".to_owned()))
    };
    let oid = field(0)?;
    let tree_oid = field(1)?;
    validate_oid(&oid)?;
    validate_oid(&tree_oid)?;
    let parents = field(2)?
        .split_whitespace()
        .map(|parent| validate_oid(parent).map(|()| parent.to_owned()))
        .collect::<Result<_, _>>()?;
    Ok(CommitMetadata {
        oid,
        parents,
        author: field(3)?,
        message: field(4)?,
    })
}

fn changed_paths(
    repo: &GitStorage,
    metadata: &CommitMetadata,
) -> Result<BTreeSet<String>, SkillSyncError> {
    let output = if let Some(parent) = metadata.parents.first() {
        run_skill_git(
            repo,
            &[
                "diff",
                "--name-only",
                "-z",
                "--no-renames",
                parent,
                &metadata.oid,
                "--",
            ],
        )?
    } else {
        run_skill_git(
            repo,
            &[
                "diff-tree",
                "--root",
                "--no-commit-id",
                "--name-only",
                "-r",
                "-z",
                "--no-renames",
                &metadata.oid,
                "--",
            ],
        )?
    };
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            let path = std::str::from_utf8(path)
                .map_err(|_| SkillSyncError::Checkpoint("non-UTF-8 changed path".to_owned()))?;
            validate_repository_path(path)?;
            Ok(path.to_owned())
        })
        .collect()
}

fn epoch_at(repo: &GitStorage, commit: &str) -> Result<Option<EpochFile>, SkillSyncError> {
    let entries = list_tree_recursive(repo, commit, EPOCH_PATH)?;
    let Some(entry) = entries.into_iter().find(|entry| entry.path == EPOCH_PATH) else {
        return Ok(None);
    };
    if entry.object_type != "blob" {
        return Err(SkillSyncError::EpochValidationBlocked(
            "epoch metadata is not a blob".to_owned(),
        ));
    }
    let bytes = read_blob_oid(repo, &entry)?;
    let epoch: EpochFile = serde_yaml::from_slice(&bytes).map_err(|error| {
        SkillSyncError::EpochValidationBlocked(format!("parse epoch metadata: {error}"))
    })?;
    epoch.validate().map_err(|error| {
        SkillSyncError::EpochValidationBlocked(format!("validate epoch metadata: {error}"))
    })?;
    Ok(Some(epoch))
}

fn resolve_commit(repo: &GitStorage, revision: &str) -> Result<String, SkillSyncError> {
    if revision.is_empty() || revision.starts_with('-') || revision.contains(['\0', '\n', '\r']) {
        return Err(SkillSyncError::Checkpoint(
            "invalid commit revision".to_owned(),
        ));
    }
    let object = format!("{revision}^{{commit}}");
    let output = run_skill_git(
        repo,
        &["rev-parse", "--verify", "--end-of-options", &object],
    )?;
    let oid = std::str::from_utf8(&output.stdout)
        .map_err(|_| SkillSyncError::Checkpoint("non-UTF-8 object ID".to_owned()))?
        .trim()
        .to_owned();
    validate_oid(&oid)?;
    Ok(oid)
}

fn read_blob_oid(repo: &GitStorage, entry: &GitTreeEntry) -> Result<Vec<u8>, SkillSyncError> {
    validate_oid(&entry.oid)?;
    Ok(run_skill_git(repo, &["cat-file", "blob", &entry.oid])?.stdout)
}

fn run_skill_git(repo: &GitStorage, args: &[&str]) -> Result<std::process::Output, SkillSyncError> {
    Ok(run_git_with_env_and_timeout(
        args,
        repo.root(),
        &[],
        SKILL_GIT_TIMEOUT,
    )?)
}

fn managed_skill_path(path: &str) -> bool {
    path.starts_with("skills/") || path.starts_with("archive/skills/")
}

fn active_user_path(path: &str) -> Option<String> {
    let components: Vec<_> = path.split('/').collect();
    match components.as_slice() {
        ["users", file] => {
            let handler = file.strip_suffix(".meta.yaml")?;
            Handler::new(handler)
                .ok()
                .map(|value| value.as_str().to_owned())
        }
        _ => None,
    }
}

fn receipt_id_from_path(path: &str) -> Option<RequestId> {
    let file = path.strip_prefix("skills/receipts/")?;
    if file.contains('/') {
        return None;
    }
    RequestId::new(file.strip_suffix(".meta.yaml")?).ok()
}

fn validate_repository_path(path: &str) -> Result<(), SkillSyncError> {
    let candidate = Path::new(path);
    if path.is_empty()
        || path.contains('\0')
        || candidate.is_absolute()
        || candidate
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(SkillSyncError::Checkpoint(
            "invalid repository path".to_owned(),
        ));
    }
    Ok(())
}

fn validate_branch_name(branch: &str) -> Result<(), SkillSyncError> {
    if !branch_name_valid(branch) {
        return Err(SkillSyncError::EpochValidationBlocked(
            "invalid epoch branch name".to_owned(),
        ));
    }
    Ok(())
}

fn validate_checkpoint(checkpoint: &SkillValidationCheckpoint) -> Result<(), SkillSyncError> {
    if checkpoint.schema_version != CHECKPOINT_SCHEMA_VERSION || checkpoint.active_epoch.is_empty()
    {
        return Err(SkillSyncError::Checkpoint(
            "unsupported or incomplete checkpoint".to_owned(),
        ));
    }
    if checkpoint.active_epoch != "legacy" && !branch_name_valid(&checkpoint.active_epoch) {
        return Err(SkillSyncError::Checkpoint(
            "checkpoint active epoch is invalid".to_owned(),
        ));
    }
    if !checkpoint.last_scanned_tip.is_empty() {
        validate_oid(&checkpoint.last_scanned_tip)?;
    }
    if let Some(tree) = &checkpoint.workspace_tree {
        validate_tree(tree)?;
    }
    for (slug, state) in &checkpoint.skills {
        SkillSlug::new(slug).map_err(|_| {
            SkillSyncError::Checkpoint(format!("invalid checkpoint Skill slug {slug:?}"))
        })?;
        validate_tree(&state.tree)?;
    }
    for (scope, conflict) in &checkpoint.conflicts {
        if scope != WORKSPACE_CONFLICT_KEY && SkillSlug::new(scope).is_err() {
            return Err(SkillSyncError::Checkpoint(format!(
                "invalid checkpoint conflict scope {scope:?}"
            )));
        }
        validate_oid(&conflict.rejected_commit)?;
        if conflict.code.is_empty() {
            return Err(SkillSyncError::Checkpoint(
                "checkpoint conflict code is empty".to_owned(),
            ));
        }
        if let Some(oid) = &conflict.accepted_tree_oid {
            validate_oid(oid)?;
        }
        let expected_tree_oid = if scope == WORKSPACE_CONFLICT_KEY {
            checkpoint
                .workspace_tree
                .as_ref()
                .map(|tree| tree.tree_oid.as_str())
        } else {
            checkpoint
                .skills
                .get(scope)
                .map(|state| state.tree.tree_oid.as_str())
        };
        if conflict.accepted_tree_oid.as_deref() != expected_tree_oid {
            return Err(SkillSyncError::Checkpoint(format!(
                "conflict scope {scope:?} does not match its accepted tree"
            )));
        }
    }
    Ok(())
}

fn branch_name_valid(branch: &str) -> bool {
    !branch.is_empty()
        && !branch.starts_with(['-', '/', '.'])
        && !branch.ends_with(['/', '.'])
        && !branch.ends_with(".lock")
        && !branch.starts_with("refs/")
        && !branch.contains("..")
        && !branch.contains("//")
        && !branch.contains("@{")
        && !branch.contains(['\0', '\n', '\r', '\\', ' ', '~', '^', ':', '?', '*', '['])
        && branch
            .split('/')
            .all(|component| !component.is_empty() && !component.starts_with('.'))
}

fn validate_tree(tree: &AcceptedTree) -> Result<(), SkillSyncError> {
    validate_oid(&tree.commit_oid)?;
    validate_oid(&tree.tree_oid)
}

fn validate_oid(oid: &str) -> Result<(), SkillSyncError> {
    if !matches!(oid.len(), 40 | 64) || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(SkillSyncError::Checkpoint(format!(
            "invalid object ID {oid:?}"
        )));
    }
    Ok(())
}

fn open_regular(path: &Path, create: bool) -> Result<File, SkillSyncError> {
    let mut options = OpenOptions::new();
    options.read(true).write(create).create(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
        if create {
            options.mode(0o600);
        }
    }
    match options.open(path) {
        Ok(file) => {
            if !file
                .metadata()
                .map_err(|error| checkpoint_error("stat checkpoint", error))?
                .is_file()
            {
                return Err(SkillSyncError::Checkpoint(
                    "checkpoint path is not a regular file".to_owned(),
                ));
            }
            Ok(file)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(
            SkillSyncError::Checkpoint("open checkpoint: not found".to_owned()),
        ),
        Err(error) => Err(checkpoint_error("open checkpoint", error)),
    }
}

fn sync_parent_directory(path: &Path) -> Result<(), SkillSyncError> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| checkpoint_error("sync checkpoint directory", error))?;
    }
    Ok(())
}

fn checkpoint_error(context: &str, error: std::io::Error) -> SkillSyncError {
    SkillSyncError::Checkpoint(format!("{context}: {error}"))
}
