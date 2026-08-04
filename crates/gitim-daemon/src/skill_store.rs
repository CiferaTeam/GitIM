use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use chrono::{SecondsFormat, Utc};
use gitim_core::skill::{
    media_type_for_path, reduce_skill, validate_description, validate_display_name,
    validate_package_entries, validate_summary, EventId, PackageEntry, PackageEntryKind,
    ProposalId, ProposalStatus, ResourceDescriptor, RevisionId, SkillError, SkillEvent,
    SkillEventKind, SkillReference, SkillRevisionMeta, SkillSlug, SkillState, MAX_PACKAGE_BYTES,
    MAX_PACKAGE_FILES, MAX_PACKAGE_FILE_BYTES, MAX_SKILL_MD_BYTES, SKILL_SCHEMA_VERSION,
};
use gitim_core::types::Handler;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use thiserror::Error;

use crate::state::AppState;

#[derive(Debug, Error)]
pub enum SkillStoreError {
    #[error("{0}")]
    Protocol(#[from] SkillError),
    #[error("Skill not found")]
    NotFound,
    #[error("Skill already exists")]
    Exists,
    #[error("Skill is archived")]
    Archived,
    #[error("Skill revision not found")]
    RevisionNotFound,
    #[error("Skill revision is not published")]
    RevisionUnpublished,
    #[error("Skill revision content is corrupted")]
    RevisionCorrupted,
    #[error("Skill proposal not found")]
    ProposalNotFound,
    #[error("Skill proposal is terminal")]
    ProposalTerminal,
    #[error("actor is not the proposal author")]
    NotProposalAuthor,
    #[error("actor is not a Skill maintainer")]
    NotMaintainer,
    #[error("actor is not a Skill owner")]
    NotOwner,
    #[error("the final Skill owner cannot be removed")]
    LastOwner,
    #[error("an owner must remain a maintainer")]
    OwnerIsMaintainer,
    #[error("role target is not an active workspace member")]
    RoleTargetInactive,
    #[error("daemon identity is required for Skill writes")]
    IdentityRequired,
    #[error("Skill event ID conflicts with existing history")]
    EventConflict,
    #[error("Skill resource not found")]
    ResourceNotFound,
    #[error("invalid Skill input")]
    InvalidInput,
    #[error("Skill repository read failed: {0}")]
    ReadFailed(String),
    #[error("Skill repository write failed: {0}")]
    WriteFailed(String),
    #[error("Skill commit failed: {0}")]
    CommitFailed(String),
}

impl SkillStoreError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Protocol(error) => error.code(),
            Self::NotFound => "skill_not_found",
            Self::Exists => "skill_exists",
            Self::Archived => "skill_archived",
            Self::RevisionNotFound => "skill_revision_not_found",
            Self::RevisionUnpublished => "skill_revision_unpublished",
            Self::RevisionCorrupted => "skill_revision_corrupted",
            Self::ProposalNotFound => "skill_proposal_not_found",
            Self::ProposalTerminal => "skill_proposal_terminal",
            Self::NotProposalAuthor => "skill_not_proposal_author",
            Self::NotMaintainer => "skill_not_maintainer",
            Self::NotOwner => "skill_not_owner",
            Self::LastOwner => "skill_last_owner",
            Self::OwnerIsMaintainer => "skill_owner_is_maintainer",
            Self::RoleTargetInactive => "skill_role_target_inactive",
            Self::IdentityRequired => "skill_identity_required",
            Self::EventConflict => "skill_event_conflict",
            Self::ResourceNotFound => "skill_resource_not_found",
            Self::InvalidInput => "skill_invalid_input",
            Self::ReadFailed(_) => "skill_read_failed",
            Self::WriteFailed(_) => "skill_write_failed",
            Self::CommitFailed(_) => "skill_commit_failed",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillCatalogEntry {
    pub slug: SkillSlug,
    pub display_name: String,
    pub description: String,
    pub current_revision: RevisionId,
    pub owners: Vec<Handler>,
    pub maintainers: Vec<Handler>,
    pub open_proposal_count: usize,
    pub archived: bool,
    pub last_event_id: EventId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InvalidSkillEntry {
    pub slug: String,
    pub error_code: String,
    pub error: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillCatalog {
    pub skills: Vec<SkillCatalogEntry>,
    pub invalid: Vec<InvalidSkillEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_after: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillLoad {
    pub canonical_ref: SkillReference,
    pub revision: SkillRevisionMeta,
    pub skill_markdown: String,
    pub resources: Vec<ResourceDescriptor>,
    pub archived: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillResource {
    pub canonical_ref: SkillReference,
    pub path: String,
    pub media_type: String,
    pub text: bool,
    pub content_base64: String,
    pub archived: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillMutation {
    pub event_id: EventId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<RevisionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal: Option<ProposalId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_ref: Option<SkillReference>,
    pub state: SkillState,
    pub commit_id: String,
    pub idempotent: bool,
}

pub struct SkillStore<'a> {
    state: &'a AppState,
}

impl<'a> SkillStore<'a> {
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }

    pub fn catalog(
        &self,
        include_archived: bool,
        limit: usize,
        after: Option<&str>,
    ) -> Result<SkillCatalog, SkillStoreError> {
        let root = self.state.repo_root.join("skills");
        if !root.exists() {
            return Ok(SkillCatalog {
                skills: Vec::new(),
                invalid: Vec::new(),
                next_after: None,
            });
        }
        let entries = fs::read_dir(&root).map_err(read_error)?;
        let mut slugs = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                entry
                    .file_type()
                    .ok()
                    .filter(|file_type| file_type.is_dir())
                    .map(|_| entry.file_name().to_string_lossy().to_string())
            })
            .filter(|slug| after.is_none_or(|cursor| slug.as_str() > cursor))
            .collect::<Vec<_>>();
        slugs.sort();

        let bounded_limit = limit.clamp(1, 100);
        let has_more = slugs.len() > bounded_limit;
        slugs.truncate(bounded_limit);
        let next_after = has_more.then(|| slugs.last().cloned()).flatten();
        let mut skills = Vec::new();
        let mut invalid = Vec::new();
        for raw_slug in slugs {
            let Ok(slug) = SkillSlug::new(&raw_slug) else {
                invalid.push(InvalidSkillEntry {
                    slug: raw_slug,
                    error_code: SkillError::InvalidHistory.code().to_string(),
                    error: "invalid Skill directory name".to_string(),
                });
                continue;
            };
            match self.skill_state(&slug) {
                Ok(state) if include_archived || !state.archived => {
                    skills.push(catalog_entry(&state));
                }
                Ok(_) => {}
                Err(error) => invalid.push(InvalidSkillEntry {
                    slug: raw_slug,
                    error_code: error.code().to_string(),
                    error: error.to_string(),
                }),
            }
        }
        Ok(SkillCatalog {
            skills,
            invalid,
            next_after,
        })
    }

    pub fn skill_state(&self, slug: &SkillSlug) -> Result<SkillState, SkillStoreError> {
        let (_, _, state) = self.read_skill(slug)?;
        Ok(state)
    }

    pub fn sole_active_owner_skills(
        &self,
        handler: &Handler,
    ) -> Result<Vec<SkillSlug>, SkillStoreError> {
        let skills_root = self.state.repo_root.join("skills");
        if !skills_root.exists() {
            return Ok(Vec::new());
        }
        let mut blocked = Vec::new();
        for entry in fs::read_dir(skills_root).map_err(read_error)? {
            let entry = entry.map_err(read_error)?;
            if !entry.file_type().map_err(read_error)?.is_dir() {
                continue;
            }
            let raw_slug = entry.file_name().to_string_lossy().to_string();
            let Ok(slug) = SkillSlug::new(&raw_slug) else {
                continue;
            };
            let Ok(state) = self.skill_state(&slug) else {
                continue;
            };
            if !state.owners.contains(handler) {
                continue;
            }
            let active_owners = state
                .owners
                .iter()
                .filter(|owner| {
                    self.state
                        .repo_root
                        .join("users")
                        .join(format!("{}.meta.yaml", owner.as_str()))
                        .is_file()
                })
                .count();
            if active_owners == 1 {
                blocked.push(slug);
            }
        }
        blocked.sort();
        Ok(blocked)
    }

    pub fn revisions(&self, slug: &SkillSlug) -> Result<Vec<SkillRevisionMeta>, SkillStoreError> {
        let (mut revisions, _, _) = self.read_skill(slug)?;
        revisions.sort_by(|left, right| right.id.cmp(&left.id));
        Ok(revisions)
    }

    pub fn load(&self, reference: &SkillReference) -> Result<SkillLoad, SkillStoreError> {
        let (revisions, _, state) = self.read_skill(&reference.slug)?;
        if state.archived && reference.revision.is_none() {
            return Err(SkillStoreError::Archived);
        }
        let revision_id = reference
            .revision
            .as_ref()
            .unwrap_or(&state.current_revision);
        if !state.published_revisions.contains(revision_id) {
            if revisions.iter().any(|meta| meta.id == *revision_id) {
                return Err(SkillStoreError::RevisionUnpublished);
            }
            return Err(SkillStoreError::RevisionNotFound);
        }
        self.load_revision(&state, &revisions, revision_id)
    }

    pub fn load_proposal(
        &self,
        slug: &SkillSlug,
        proposal: &ProposalId,
    ) -> Result<SkillLoad, SkillStoreError> {
        let (revisions, _, state) = self.read_skill(slug)?;
        let proposal = state
            .proposals
            .iter()
            .find(|item| item.id == *proposal)
            .ok_or(SkillStoreError::ProposalNotFound)?;
        self.load_revision(&state, &revisions, &proposal.revision)
    }

    pub fn resource(
        &self,
        reference: &SkillReference,
        path: &str,
    ) -> Result<SkillResource, SkillStoreError> {
        let load = self.load(reference)?;
        let package = self.read_verified_package(&load.revision)?;
        let entry = package
            .entries
            .into_iter()
            .find(|entry| entry.path == path && entry.path != "SKILL.md")
            .ok_or(SkillStoreError::ResourceNotFound)?;
        Ok(SkillResource {
            canonical_ref: load.canonical_ref,
            path: entry.path.clone(),
            media_type: media_type_for_path(&entry.path).to_string(),
            text: std::str::from_utf8(&entry.bytes).is_ok(),
            content_base64: BASE64_STANDARD.encode(entry.bytes),
            archived: load.archived,
        })
    }

    pub fn proposal_resource(
        &self,
        slug: &SkillSlug,
        proposal: &ProposalId,
        path: &str,
    ) -> Result<SkillResource, SkillStoreError> {
        let load = self.load_proposal(slug, proposal)?;
        let package = self.read_verified_package(&load.revision)?;
        let entry = package
            .entries
            .into_iter()
            .find(|entry| entry.path == path && entry.path != "SKILL.md")
            .ok_or(SkillStoreError::ResourceNotFound)?;
        Ok(SkillResource {
            canonical_ref: load.canonical_ref,
            path: entry.path.clone(),
            media_type: media_type_for_path(&entry.path).to_string(),
            text: std::str::from_utf8(&entry.bytes).is_ok(),
            content_base64: BASE64_STANDARD.encode(entry.bytes),
            archived: load.archived,
        })
    }

    pub fn validate_directory(
        &self,
        slug: &SkillSlug,
        source: &Path,
    ) -> Result<gitim_core::skill::ValidatedPackage, SkillStoreError> {
        let entries = collect_package_entries(source)?;
        validate_package_entries(slug, entries).map_err(Into::into)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create(
        &self,
        actor: &Handler,
        slug: &SkillSlug,
        source: &Path,
        display_name: &str,
        description: &str,
        event_id: Option<EventId>,
    ) -> Result<SkillMutation, SkillStoreError> {
        if !validate_display_name(display_name) || !validate_description(description) {
            return Err(SkillStoreError::InvalidInput);
        }
        let package = self.validate_directory(slug, source)?;
        let _guard = self
            .state
            .commit_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ensure_active_member(&self.state.repo_root, actor)?;
        let skill_root = skill_root(&self.state.repo_root, slug);
        if skill_root.exists() {
            if let Some(existing_id) = event_id {
                return self.replay_create(
                    actor,
                    slug,
                    display_name,
                    description,
                    &package,
                    &existing_id,
                );
            }
            return Err(SkillStoreError::Exists);
        }

        let event_id = event_id.unwrap_or(EventId::generate_after(None)?);
        let revision_id = unique_revision_id(&skill_root);
        let timestamp = current_timestamp();
        let revision = SkillRevisionMeta {
            schema_version: SKILL_SCHEMA_VERSION,
            id: revision_id.clone(),
            skill: slug.clone(),
            base_revision: None,
            content_sha256: package.content_sha256.clone(),
            resources: package.resources.clone(),
            created_by: actor.clone(),
            created_at: timestamp.clone(),
        };
        let event = SkillEvent {
            schema_version: SKILL_SCHEMA_VERSION,
            id: event_id.clone(),
            skill: slug.clone(),
            actor: actor.clone(),
            created_at: timestamp,
            kind: SkillEventKind::Created {
                display_name: display_name.to_string(),
                description: description.to_string(),
                revision: revision_id.clone(),
            },
        };
        let revision_root = revision_root(&self.state.repo_root, slug, &revision_id);
        let event_path = event_path(&self.state.repo_root, slug, &event_id);
        if let Err(error) = write_revision(&revision_root, &revision, &package) {
            let _ = fs::remove_dir_all(&skill_root);
            return Err(error);
        }
        if let Err(error) = write_yaml(&event_path, &event) {
            let _ = fs::remove_dir_all(&skill_root);
            return Err(error);
        }
        let commit_id =
            match self.commit_skill(slug, &format!("skill: create {slug} @{actor}"), actor) {
                Ok(commit) => commit,
                Err(error) => {
                    rollback_skill_path(&self.state.repo_root, slug, &skill_root);
                    return Err(error);
                }
            };
        let state = self.skill_state(slug)?;
        Ok(SkillMutation {
            event_id,
            revision: Some(revision_id.clone()),
            proposal: None,
            canonical_ref: Some(SkillReference::pinned(slug.clone(), revision_id)),
            state,
            commit_id,
            idempotent: false,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn propose(
        &self,
        actor: &Handler,
        slug: &SkillSlug,
        source: &Path,
        base_revision: &RevisionId,
        summary: &str,
        event_id: Option<EventId>,
    ) -> Result<SkillMutation, SkillStoreError> {
        if !validate_summary(summary) {
            return Err(SkillStoreError::InvalidInput);
        }
        let package = self.validate_directory(slug, source)?;
        let _guard = self
            .state
            .commit_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ensure_active_member(&self.state.repo_root, actor)?;
        let (mut revisions, mut events, state) = self.read_skill(slug)?;
        if state.archived {
            return Err(SkillStoreError::Archived);
        }
        if let Some(existing_id) = event_id.as_ref() {
            if events.iter().any(|event| event.id == *existing_id) {
                return self.replay_proposal(
                    actor,
                    base_revision,
                    summary,
                    &package,
                    existing_id,
                    &state,
                    &revisions,
                    &events,
                );
            }
        }
        if !revisions
            .iter()
            .any(|revision| revision.id == *base_revision)
        {
            return Err(SkillStoreError::RevisionNotFound);
        }
        if !state.published_revisions.contains(base_revision) {
            return Err(SkillStoreError::RevisionUnpublished);
        }
        let event_id = next_event_id(event_id, &state)?;
        let skill_root = skill_root(&self.state.repo_root, slug);
        let revision_id = unique_revision_id(&skill_root);
        let proposal_id = unique_proposal_id(&state);
        let timestamp = current_timestamp();
        let revision = SkillRevisionMeta {
            schema_version: SKILL_SCHEMA_VERSION,
            id: revision_id.clone(),
            skill: slug.clone(),
            base_revision: Some(base_revision.clone()),
            content_sha256: package.content_sha256.clone(),
            resources: package.resources.clone(),
            created_by: actor.clone(),
            created_at: timestamp.clone(),
        };
        let event = SkillEvent {
            schema_version: SKILL_SCHEMA_VERSION,
            id: event_id.clone(),
            skill: slug.clone(),
            actor: actor.clone(),
            created_at: timestamp,
            kind: SkillEventKind::ProposalOpened {
                proposal: proposal_id.clone(),
                revision: revision_id.clone(),
                base_revision: base_revision.clone(),
                summary: summary.to_string(),
            },
        };
        revisions.push(revision.clone());
        events.push(event.clone());
        ensure_candidate_effective(slug, &revisions, events)?;

        let revision_root = revision_root(&self.state.repo_root, slug, &revision_id);
        let event_path = event_path(&self.state.repo_root, slug, &event_id);
        if let Err(error) = write_revision(&revision_root, &revision, &package) {
            let _ = fs::remove_dir_all(&revision_root);
            return Err(error);
        }
        if let Err(error) = write_yaml(&event_path, &event) {
            let _ = fs::remove_dir_all(&revision_root);
            return Err(error);
        }
        let commit_id =
            match self.commit_skill(slug, &format!("skill: propose {slug} @{actor}"), actor) {
                Ok(commit) => commit,
                Err(error) => {
                    let _ = fs::remove_file(&event_path);
                    let _ = fs::remove_dir_all(&revision_root);
                    restore_index_path(&self.state.repo_root, slug);
                    return Err(error);
                }
            };
        let state = self.skill_state(slug)?;
        Ok(SkillMutation {
            event_id,
            revision: Some(revision_id),
            proposal: Some(proposal_id),
            canonical_ref: None,
            state,
            commit_id,
            idempotent: false,
        })
    }

    pub fn publish(
        &self,
        actor: &Handler,
        slug: &SkillSlug,
        proposal: &ProposalId,
        event_id: Option<EventId>,
    ) -> Result<SkillMutation, SkillStoreError> {
        let state = self.skill_state(slug)?;
        self.append_event(
            actor,
            slug,
            SkillEventKind::ProposalPublished {
                proposal: proposal.clone(),
                expected_current_revision: state.current_revision,
            },
            event_id,
            "publish",
            Some(proposal.clone()),
        )
    }

    pub fn comment(
        &self,
        actor: &Handler,
        slug: &SkillSlug,
        proposal: &ProposalId,
        body: &str,
        event_id: Option<EventId>,
    ) -> Result<SkillMutation, SkillStoreError> {
        self.append_event(
            actor,
            slug,
            SkillEventKind::ProposalCommented {
                proposal: proposal.clone(),
                body: body.to_string(),
            },
            event_id,
            "comment",
            Some(proposal.clone()),
        )
    }

    pub fn reject(
        &self,
        actor: &Handler,
        slug: &SkillSlug,
        proposal: &ProposalId,
        event_id: Option<EventId>,
    ) -> Result<SkillMutation, SkillStoreError> {
        self.append_event(
            actor,
            slug,
            SkillEventKind::ProposalRejected {
                proposal: proposal.clone(),
            },
            event_id,
            "reject",
            Some(proposal.clone()),
        )
    }

    pub fn withdraw(
        &self,
        actor: &Handler,
        slug: &SkillSlug,
        proposal: &ProposalId,
        event_id: Option<EventId>,
    ) -> Result<SkillMutation, SkillStoreError> {
        self.append_event(
            actor,
            slug,
            SkillEventKind::ProposalWithdrawn {
                proposal: proposal.clone(),
            },
            event_id,
            "withdraw",
            Some(proposal.clone()),
        )
    }

    pub fn update_metadata(
        &self,
        actor: &Handler,
        slug: &SkillSlug,
        display_name: Option<String>,
        description: Option<String>,
        event_id: Option<EventId>,
    ) -> Result<SkillMutation, SkillStoreError> {
        self.append_event(
            actor,
            slug,
            SkillEventKind::MetadataUpdated {
                display_name,
                description,
            },
            event_id,
            "update",
            None,
        )
    }

    pub fn owner_add(
        &self,
        actor: &Handler,
        slug: &SkillSlug,
        target: Handler,
        event_id: Option<EventId>,
    ) -> Result<SkillMutation, SkillStoreError> {
        self.append_event(
            actor,
            slug,
            SkillEventKind::OwnerAdded { handler: target },
            event_id,
            "owner-add",
            None,
        )
    }

    pub fn owner_remove(
        &self,
        actor: &Handler,
        slug: &SkillSlug,
        target: Handler,
        remove_maintainer: bool,
        event_id: Option<EventId>,
    ) -> Result<SkillMutation, SkillStoreError> {
        self.append_event(
            actor,
            slug,
            SkillEventKind::OwnerRemoved {
                handler: target,
                remove_maintainer,
            },
            event_id,
            "owner-remove",
            None,
        )
    }

    pub fn maintainer_add(
        &self,
        actor: &Handler,
        slug: &SkillSlug,
        target: Handler,
        event_id: Option<EventId>,
    ) -> Result<SkillMutation, SkillStoreError> {
        self.append_event(
            actor,
            slug,
            SkillEventKind::MaintainerAdded { handler: target },
            event_id,
            "maintainer-add",
            None,
        )
    }

    pub fn maintainer_remove(
        &self,
        actor: &Handler,
        slug: &SkillSlug,
        target: Handler,
        event_id: Option<EventId>,
    ) -> Result<SkillMutation, SkillStoreError> {
        self.append_event(
            actor,
            slug,
            SkillEventKind::MaintainerRemoved { handler: target },
            event_id,
            "maintainer-remove",
            None,
        )
    }

    pub fn archive(
        &self,
        actor: &Handler,
        slug: &SkillSlug,
        event_id: Option<EventId>,
    ) -> Result<SkillMutation, SkillStoreError> {
        self.append_event(
            actor,
            slug,
            SkillEventKind::Archived,
            event_id,
            "archive",
            None,
        )
    }

    pub fn unarchive(
        &self,
        actor: &Handler,
        slug: &SkillSlug,
        event_id: Option<EventId>,
    ) -> Result<SkillMutation, SkillStoreError> {
        self.append_event(
            actor,
            slug,
            SkillEventKind::Unarchived,
            event_id,
            "unarchive",
            None,
        )
    }

    fn append_event(
        &self,
        actor: &Handler,
        slug: &SkillSlug,
        kind: SkillEventKind,
        event_id: Option<EventId>,
        operation: &str,
        proposal: Option<ProposalId>,
    ) -> Result<SkillMutation, SkillStoreError> {
        let _guard = self
            .state
            .commit_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ensure_active_member(&self.state.repo_root, actor)?;
        if let SkillEventKind::OwnerAdded { handler }
        | SkillEventKind::MaintainerAdded { handler } = &kind
        {
            ensure_active_member(&self.state.repo_root, handler)?;
        }
        let (revisions, mut events, state) = self.read_skill(slug)?;
        if let Some(existing_id) = event_id.as_ref() {
            if let Some(existing) = events.iter().find(|event| event.id == *existing_id) {
                if existing.actor != *actor || existing.kind != kind {
                    return Err(SkillStoreError::EventConflict);
                }
                return Ok(replay_mutation(
                    existing_id.clone(),
                    proposal,
                    state,
                    self.current_head()?,
                ));
            }
        }
        let event_id = next_event_id(event_id, &state)?;
        let event = SkillEvent {
            schema_version: SKILL_SCHEMA_VERSION,
            id: event_id.clone(),
            skill: slug.clone(),
            actor: actor.clone(),
            created_at: current_timestamp(),
            kind,
        };
        events.push(event.clone());
        let reduced = reduce_skill(slug, &revisions, events)?;
        let last = reduced.history.last().ok_or(SkillError::InvalidHistory)?;
        if !last.effective {
            return Err(reason_error(last.reason.as_deref()));
        }

        let event_path = event_path(&self.state.repo_root, slug, &event_id);
        write_yaml(&event_path, &event)?;
        let commit_id =
            match self.commit_skill(slug, &format!("skill: {operation} {slug} @{actor}"), actor) {
                Ok(commit) => commit,
                Err(error) => {
                    let _ = fs::remove_file(&event_path);
                    restore_index_path(&self.state.repo_root, slug);
                    return Err(error);
                }
            };
        let state = self.skill_state(slug)?;
        let canonical_ref = Some(SkillReference::pinned(
            slug.clone(),
            state.current_revision.clone(),
        ));
        Ok(SkillMutation {
            event_id,
            revision: None,
            proposal,
            canonical_ref,
            state,
            commit_id,
            idempotent: false,
        })
    }

    fn load_revision(
        &self,
        state: &SkillState,
        revisions: &[SkillRevisionMeta],
        revision_id: &RevisionId,
    ) -> Result<SkillLoad, SkillStoreError> {
        let revision = revisions
            .iter()
            .find(|meta| meta.id == *revision_id)
            .cloned()
            .ok_or(SkillStoreError::RevisionNotFound)?;
        let package = self.read_verified_package(&revision)?;
        let skill_markdown = String::from_utf8(package.skill_markdown)
            .map_err(|_| SkillStoreError::RevisionCorrupted)?;
        Ok(SkillLoad {
            canonical_ref: SkillReference::pinned(state.slug.clone(), revision_id.clone()),
            revision,
            skill_markdown,
            resources: package.resources,
            archived: state.archived,
        })
    }

    fn read_revision_package(
        &self,
        revision: &SkillRevisionMeta,
    ) -> Result<Vec<PackageEntry>, SkillStoreError> {
        let package_root =
            revision_root(&self.state.repo_root, &revision.skill, &revision.id).join("package");
        collect_package_entries(&package_root)
    }

    fn read_verified_package(
        &self,
        revision: &SkillRevisionMeta,
    ) -> Result<gitim_core::skill::ValidatedPackage, SkillStoreError> {
        let entries = self
            .read_revision_package(revision)
            .map_err(|_| SkillStoreError::RevisionCorrupted)?;
        let package = validate_package_entries(&revision.skill, entries)
            .map_err(|_| SkillStoreError::RevisionCorrupted)?;
        if package.content_sha256 != revision.content_sha256
            || package.resources != revision.resources
        {
            return Err(SkillStoreError::RevisionCorrupted);
        }
        Ok(package)
    }

    fn read_skill(
        &self,
        slug: &SkillSlug,
    ) -> Result<(Vec<SkillRevisionMeta>, Vec<SkillEvent>, SkillState), SkillStoreError> {
        let root = skill_root(&self.state.repo_root, slug);
        if !root.is_dir() {
            return Err(SkillStoreError::NotFound);
        }
        let revisions = read_revisions(&root, slug)?;
        let events = read_events(&root, slug)?;
        let state = reduce_skill(slug, &revisions, events.clone())?;
        Ok((revisions, events, state))
    }

    fn replay_create(
        &self,
        actor: &Handler,
        slug: &SkillSlug,
        display_name: &str,
        description: &str,
        package: &gitim_core::skill::ValidatedPackage,
        event_id: &EventId,
    ) -> Result<SkillMutation, SkillStoreError> {
        let (revisions, events, state) = self.read_skill(slug)?;
        let event = events
            .iter()
            .find(|event| event.id == *event_id)
            .ok_or(SkillStoreError::Exists)?;
        let SkillEventKind::Created {
            display_name: existing_name,
            description: existing_description,
            revision,
        } = &event.kind
        else {
            return Err(SkillStoreError::EventConflict);
        };
        if event.actor != *actor
            || existing_name != display_name
            || existing_description != description
        {
            return Err(SkillStoreError::EventConflict);
        }
        let existing = self.load_revision(&state, &revisions, revision)?;
        if existing.revision.content_sha256 != package.content_sha256 {
            return Err(SkillStoreError::EventConflict);
        }
        Ok(SkillMutation {
            event_id: event_id.clone(),
            revision: Some(revision.clone()),
            proposal: None,
            canonical_ref: Some(existing.canonical_ref),
            state,
            commit_id: self.current_head()?,
            idempotent: true,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn replay_proposal(
        &self,
        actor: &Handler,
        base_revision: &RevisionId,
        summary: &str,
        package: &gitim_core::skill::ValidatedPackage,
        event_id: &EventId,
        state: &SkillState,
        revisions: &[SkillRevisionMeta],
        events: &[SkillEvent],
    ) -> Result<SkillMutation, SkillStoreError> {
        let event = events
            .iter()
            .find(|event| event.id == *event_id)
            .ok_or(SkillStoreError::EventConflict)?;
        let SkillEventKind::ProposalOpened {
            proposal,
            revision,
            base_revision: existing_base,
            summary: existing_summary,
        } = &event.kind
        else {
            return Err(SkillStoreError::EventConflict);
        };
        if event.actor != *actor || existing_base != base_revision || existing_summary != summary {
            return Err(SkillStoreError::EventConflict);
        }
        let existing = self.load_revision(state, revisions, revision)?;
        if existing.revision.content_sha256 != package.content_sha256 {
            return Err(SkillStoreError::EventConflict);
        }
        Ok(SkillMutation {
            event_id: event_id.clone(),
            revision: Some(revision.clone()),
            proposal: Some(proposal.clone()),
            canonical_ref: None,
            state: state.clone(),
            commit_id: self.current_head()?,
            idempotent: true,
        })
    }

    fn commit_skill(
        &self,
        slug: &SkillSlug,
        message: &str,
        actor: &Handler,
    ) -> Result<String, SkillStoreError> {
        let relative = format!("skills/{slug}");
        let (author_name, author_email) = self.state.author_for(actor.as_str());
        self.state
            .git_storage
            .add_and_commit_only_as(&relative, message, Some((&author_name, &author_email)))
            .map_err(|error| SkillStoreError::CommitFailed(error.to_string()))
    }

    fn current_head(&self) -> Result<String, SkillStoreError> {
        self.state
            .git_storage
            .rev_parse("HEAD")
            .map_err(|error| SkillStoreError::ReadFailed(error.to_string()))
    }
}

fn catalog_entry(state: &SkillState) -> SkillCatalogEntry {
    SkillCatalogEntry {
        slug: state.slug.clone(),
        display_name: state.display_name.clone(),
        description: state.description.clone(),
        current_revision: state.current_revision.clone(),
        owners: state.owners.clone(),
        maintainers: state.maintainers.clone(),
        open_proposal_count: state
            .proposals
            .iter()
            .filter(|proposal| proposal.status == ProposalStatus::Open)
            .count(),
        archived: state.archived,
        last_event_id: state.last_event_id.clone(),
    }
}

fn ensure_active_member(root: &Path, handler: &Handler) -> Result<(), SkillStoreError> {
    if root
        .join("users")
        .join(format!("{}.meta.yaml", handler.as_str()))
        .is_file()
    {
        Ok(())
    } else {
        Err(SkillStoreError::RoleTargetInactive)
    }
}

fn read_revisions(
    skill_root: &Path,
    slug: &SkillSlug,
) -> Result<Vec<SkillRevisionMeta>, SkillStoreError> {
    let root = skill_root.join("revisions");
    let entries = fs::read_dir(&root).map_err(read_error)?;
    let mut revisions = Vec::new();
    for entry in entries {
        let entry = entry.map_err(read_error)?;
        let file_type = entry.file_type().map_err(read_error)?;
        if !file_type.is_dir() {
            return Err(SkillError::InvalidHistory.into());
        }
        let raw_id = entry.file_name().to_string_lossy().to_string();
        let id = RevisionId::new(&raw_id).map_err(|_| SkillError::InvalidHistory)?;
        let meta_path = entry.path().join("revision.meta.yaml");
        let meta: SkillRevisionMeta = read_yaml(&meta_path)?;
        if meta.id != id
            || meta.skill != *slug
            || meta.schema_version != SKILL_SCHEMA_VERSION
            || !valid_sha256(&meta.content_sha256)
        {
            return Err(SkillError::InvalidHistory.into());
        }
        revisions.push(meta);
    }
    Ok(revisions)
}

fn read_events(skill_root: &Path, slug: &SkillSlug) -> Result<Vec<SkillEvent>, SkillStoreError> {
    let root = skill_root.join("events");
    let entries = fs::read_dir(&root).map_err(read_error)?;
    let mut events = Vec::new();
    for entry in entries {
        let entry = entry.map_err(read_error)?;
        if !entry.file_type().map_err(read_error)?.is_file() {
            return Err(SkillError::InvalidHistory.into());
        }
        let filename = entry.file_name().to_string_lossy().to_string();
        let raw_id = filename
            .strip_suffix(".meta.yaml")
            .ok_or(SkillError::InvalidHistory)?;
        let id = EventId::new(raw_id).map_err(|_| SkillError::InvalidHistory)?;
        let event: SkillEvent = read_yaml(&entry.path())?;
        if event.id != id || event.skill != *slug || event.schema_version != SKILL_SCHEMA_VERSION {
            return Err(SkillError::InvalidHistory.into());
        }
        events.push(event);
    }
    Ok(events)
}

fn collect_package_entries(root: &Path) -> Result<Vec<PackageEntry>, SkillStoreError> {
    let metadata = fs::symlink_metadata(root).map_err(read_error)?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(SkillError::InvalidPackage.into());
    }
    let mut entries = Vec::new();
    let mut total_bytes = 0_u64;
    collect_directory(root, root, &mut entries, &mut total_bytes)?;
    Ok(entries)
}

fn collect_directory(
    root: &Path,
    directory: &Path,
    entries: &mut Vec<PackageEntry>,
    total_bytes: &mut u64,
) -> Result<(), SkillStoreError> {
    for item in fs::read_dir(directory).map_err(read_error)? {
        let item = item.map_err(read_error)?;
        let file_type = item.file_type().map_err(read_error)?;
        let path = item.path();
        if file_type.is_dir() {
            collect_directory(root, &path, entries, total_bytes)?;
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|error| SkillStoreError::ReadFailed(error.to_string()))?
            .to_str()
            .ok_or(SkillError::InvalidPackage)?
            .replace(std::path::MAIN_SEPARATOR, "/");
        if entries.len() >= MAX_PACKAGE_FILES {
            return Err(SkillError::PackageTooLarge.into());
        }
        let kind = if file_type.is_file() {
            PackageEntryKind::Regular
        } else if file_type.is_symlink() {
            PackageEntryKind::Symlink
        } else {
            PackageEntryKind::Socket
        };
        let bytes = if file_type.is_file() {
            let file_limit = if relative == "SKILL.md" {
                MAX_SKILL_MD_BYTES
            } else {
                MAX_PACKAGE_FILE_BYTES
            };
            let bytes = read_regular_file_nofollow(&path, file_limit)?;
            *total_bytes = total_bytes
                .checked_add(bytes.len() as u64)
                .ok_or(SkillError::PackageTooLarge)?;
            if *total_bytes > MAX_PACKAGE_BYTES as u64 {
                return Err(SkillError::PackageTooLarge.into());
            }
            bytes
        } else {
            Vec::new()
        };
        entries.push(PackageEntry::with_kind(relative, bytes, kind));
    }
    Ok(())
}

fn read_regular_file_nofollow(path: &Path, max_bytes: usize) -> Result<Vec<u8>, SkillStoreError> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);

    let file = options.open(path).map_err(|error| {
        #[cfg(unix)]
        if error.raw_os_error() == Some(libc::ELOOP) {
            return SkillError::InvalidPackage.into();
        }
        read_error(error)
    })?;
    let metadata = file.metadata().map_err(read_error)?;
    if !metadata.file_type().is_file() {
        return Err(SkillError::InvalidPackage.into());
    }
    let max_bytes_u64 = u64::try_from(max_bytes).map_err(|_| SkillError::PackageTooLarge)?;
    if metadata.len() > max_bytes_u64 {
        return Err(SkillError::PackageTooLarge.into());
    }
    let read_limit = max_bytes_u64
        .checked_add(1)
        .ok_or(SkillError::PackageTooLarge)?;
    let mut bytes = Vec::with_capacity(max_bytes.min(metadata.len() as usize));
    file.take(read_limit)
        .read_to_end(&mut bytes)
        .map_err(read_error)?;
    if bytes.len() > max_bytes {
        return Err(SkillError::PackageTooLarge.into());
    }
    Ok(bytes)
}

fn write_revision(
    root: &Path,
    meta: &SkillRevisionMeta,
    package: &gitim_core::skill::ValidatedPackage,
) -> Result<(), SkillStoreError> {
    let package_root = root.join("package");
    fs::create_dir_all(&package_root).map_err(write_error)?;
    for entry in &package.entries {
        let destination = package_root.join(&entry.path);
        let parent = destination.parent().ok_or_else(|| {
            SkillStoreError::WriteFailed("package destination has no parent".to_string())
        })?;
        fs::create_dir_all(parent).map_err(write_error)?;
        atomic_write(&destination, &entry.bytes)?;
    }
    write_yaml(&root.join("revision.meta.yaml"), meta)
}

fn read_yaml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, SkillStoreError> {
    let content = fs::read_to_string(path).map_err(read_error)?;
    serde_yaml::from_str(&content).map_err(|error| {
        tracing::warn!(path = %path.display(), %error, "invalid Skill YAML");
        SkillStoreError::Protocol(SkillError::InvalidHistory)
    })
}

fn write_yaml<T: Serialize>(path: &Path, value: &T) -> Result<(), SkillStoreError> {
    let yaml = serde_yaml::to_string(value)
        .map_err(|error| SkillStoreError::WriteFailed(error.to_string()))?;
    atomic_write(path, yaml.as_bytes())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), SkillStoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| SkillStoreError::WriteFailed("path has no parent".to_string()))?;
    fs::create_dir_all(parent).map_err(write_error)?;
    let mut temporary = NamedTempFile::new_in(parent).map_err(write_error)?;
    std::io::Write::write_all(&mut temporary, bytes).map_err(write_error)?;
    temporary
        .persist(path)
        .map_err(|error| SkillStoreError::WriteFailed(error.error.to_string()))?;
    Ok(())
}

fn ensure_candidate_effective(
    slug: &SkillSlug,
    revisions: &[SkillRevisionMeta],
    events: Vec<SkillEvent>,
) -> Result<(), SkillStoreError> {
    let state = reduce_skill(slug, revisions, events)?;
    let last = state.history.last().ok_or(SkillError::InvalidHistory)?;
    if last.effective {
        Ok(())
    } else {
        Err(reason_error(last.reason.as_deref()))
    }
}

fn reason_error(reason: Option<&str>) -> SkillStoreError {
    match reason {
        Some("skill_archived") => SkillStoreError::Archived,
        Some("not_maintainer") => SkillStoreError::NotMaintainer,
        Some("not_owner") => SkillStoreError::NotOwner,
        Some("last_owner") => SkillStoreError::LastOwner,
        Some("owner_is_maintainer") => SkillStoreError::OwnerIsMaintainer,
        Some("proposal_not_found") => SkillStoreError::ProposalNotFound,
        Some("proposal_terminal") => SkillStoreError::ProposalTerminal,
        Some("not_proposal_author") => SkillStoreError::NotProposalAuthor,
        Some("stale_current_revision" | "role_unchanged" | "already_archived" | "not_archived") => {
            SkillStoreError::EventConflict
        }
        Some("invalid_metadata" | "invalid_summary" | "invalid_comment") => {
            SkillStoreError::InvalidInput
        }
        _ => SkillStoreError::Protocol(SkillError::InvalidHistory),
    }
}

fn replay_mutation(
    event_id: EventId,
    proposal: Option<ProposalId>,
    state: SkillState,
    commit_id: String,
) -> SkillMutation {
    SkillMutation {
        event_id,
        revision: None,
        proposal,
        canonical_ref: Some(SkillReference::pinned(
            state.slug.clone(),
            state.current_revision.clone(),
        )),
        state,
        commit_id,
        idempotent: true,
    }
}

fn next_event_id(
    requested: Option<EventId>,
    state: &SkillState,
) -> Result<EventId, SkillStoreError> {
    match requested {
        Some(id) if id > state.last_event_id => Ok(id),
        Some(_) => Err(SkillStoreError::EventConflict),
        None => EventId::generate_after(Some(&state.last_event_id)).map_err(Into::into),
    }
}

fn unique_revision_id(skill_root: &Path) -> RevisionId {
    loop {
        let id = RevisionId::generate();
        if !skill_root.join("revisions").join(id.as_str()).exists() {
            return id;
        }
    }
}

fn unique_proposal_id(state: &SkillState) -> ProposalId {
    loop {
        let id = ProposalId::generate();
        if !state.proposals.iter().any(|proposal| proposal.id == id) {
            return id;
        }
    }
}

fn skill_root(repo_root: &Path, slug: &SkillSlug) -> PathBuf {
    repo_root.join("skills").join(slug.as_str())
}

fn revision_root(repo_root: &Path, slug: &SkillSlug, revision: &RevisionId) -> PathBuf {
    skill_root(repo_root, slug)
        .join("revisions")
        .join(revision.as_str())
}

fn event_path(repo_root: &Path, slug: &SkillSlug, event: &EventId) -> PathBuf {
    skill_root(repo_root, slug)
        .join("events")
        .join(format!("{}.meta.yaml", event.as_str()))
}

fn current_timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn rollback_skill_path(repo_root: &Path, slug: &SkillSlug, path: &Path) {
    let _ = fs::remove_dir_all(path);
    restore_index_path(repo_root, slug);
}

fn restore_index_path(repo_root: &Path, slug: &SkillSlug) {
    let relative = format!("skills/{slug}");
    let _ = Command::new("git")
        .args(["reset", "HEAD", "--", &relative])
        .current_dir(repo_root)
        .output();
    let _ = Command::new("git")
        .args(["checkout", "--", &relative])
        .current_dir(repo_root)
        .output();
}

fn read_error(error: std::io::Error) -> SkillStoreError {
    SkillStoreError::ReadFailed(error.to_string())
}

fn write_error(error: std::io::Error) -> SkillStoreError {
    SkillStoreError::WriteFailed(error.to_string())
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn regular_file_reader_does_not_follow_symlinks() {
        let temporary = tempfile::tempdir().unwrap();
        let target = temporary.path().join("target.txt");
        let link = temporary.path().join("link.txt");
        fs::write(&target, b"private").unwrap();
        symlink(&target, &link).unwrap();

        let error = read_regular_file_nofollow(&link, 1024).unwrap_err();
        assert_eq!(error.code(), "skill_invalid_package");
    }
}
