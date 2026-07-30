use thiserror::Error;

use super::{ProposalStatus, RevisionId};

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum SkillError {
    #[error("skill not found")]
    NotFound,
    #[error("skill is archived")]
    Archived,
    #[error("skill already exists")]
    Exists,
    #[error("invalid skill slug")]
    InvalidSlug,
    #[error("invalid skill package")]
    InvalidPackage,
    #[error("skill package is too large")]
    PackageTooLarge,
    #[error("skill revision not found")]
    RevisionNotFound,
    #[error("skill revision is unpublished")]
    RevisionUnpublished,
    #[error("skill revision is corrupted")]
    RevisionCorrupted,
    #[error("skill proposal not found")]
    ProposalNotFound,
    #[error("skill proposal is terminal")]
    ProposalTerminal,
    #[error("skill open proposal limit reached")]
    OpenProposalLimit,
    #[error("skill content revision is stale")]
    StaleContentRevision {
        current_revision: RevisionId,
        control_revision: u64,
        event_revision: u64,
    },
    #[error("skill control revision is stale")]
    StaleControlRevision {
        current_revision: RevisionId,
        control_revision: u64,
        event_revision: u64,
    },
    #[error("skill proposal revision is stale")]
    StaleProposalRevision {
        current_revision: RevisionId,
        control_revision: u64,
        event_revision: u64,
        proposal_status: ProposalStatus,
        proposal_state_revision: u64,
    },
    #[error("actor is not a skill maintainer")]
    NotMaintainer,
    #[error("actor is not a skill owner")]
    NotOwner,
    #[error("workspace skill administrator required")]
    AdminRequired,
    #[error("workspace skill administrator is uninitialized")]
    AdminUninitialized,
    #[error("cannot remove the last workspace skill administrator")]
    LastAdmin,
    #[error("user has a workspace skill administrator role")]
    AdminRolePresent,
    #[error("cannot remove the last skill owner")]
    LastOwner,
    #[error("skill owner is still a maintainer")]
    OwnerIsMaintainer,
    #[error("skill role target is invalid")]
    RoleTargetInvalid,
    #[error("skill role target is inactive")]
    RoleTargetInactive,
    #[error("user has skill roles")]
    RolesPresent,
    #[error("skill mutation requires a remote")]
    RemoteRequired,
    #[error("skill synchronization conflict")]
    SyncConflict,
    #[error("local skill quarantine blocks this operation")]
    LocalQuarantineBlocked,
    #[error("skill epoch validation blocks this operation")]
    EpochValidationBlocked,
    #[error("skill load is unavailable")]
    LoadUnavailable,
    #[error("request identifier conflicts with an existing request")]
    RequestIdConflict,
    #[error("output already exists")]
    OutputExists,
    #[error("cursor is stale")]
    StaleCursor,
}

impl SkillError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "skill_not_found",
            Self::Archived => "skill_archived",
            Self::Exists => "skill_exists",
            Self::InvalidSlug => "skill_invalid_slug",
            Self::InvalidPackage => "skill_invalid_package",
            Self::PackageTooLarge => "skill_package_too_large",
            Self::RevisionNotFound => "skill_revision_not_found",
            Self::RevisionUnpublished => "skill_revision_unpublished",
            Self::RevisionCorrupted => "skill_revision_corrupted",
            Self::ProposalNotFound => "skill_proposal_not_found",
            Self::ProposalTerminal => "skill_proposal_terminal",
            Self::OpenProposalLimit => "skill_open_proposal_limit",
            Self::StaleContentRevision { .. } => "skill_stale_content_revision",
            Self::StaleControlRevision { .. } => "skill_stale_control_revision",
            Self::StaleProposalRevision { .. } => "skill_stale_proposal_revision",
            Self::NotMaintainer => "skill_not_maintainer",
            Self::NotOwner => "skill_not_owner",
            Self::AdminRequired => "skill_admin_required",
            Self::AdminUninitialized => "skill_admin_uninitialized",
            Self::LastAdmin => "skill_last_admin",
            Self::AdminRolePresent => "skill_admin_role_present",
            Self::LastOwner => "skill_last_owner",
            Self::OwnerIsMaintainer => "skill_owner_is_maintainer",
            Self::RoleTargetInvalid => "skill_role_target_invalid",
            Self::RoleTargetInactive => "skill_role_target_inactive",
            Self::RolesPresent => "skill_roles_present",
            Self::RemoteRequired => "skill_remote_required",
            Self::SyncConflict => "skill_sync_conflict",
            Self::LocalQuarantineBlocked => "skill_local_quarantine_blocked",
            Self::EpochValidationBlocked => "skill_epoch_validation_blocked",
            Self::LoadUnavailable => "skill_load_unavailable",
            Self::RequestIdConflict => "request_id_conflict",
            Self::OutputExists => "output_exists",
            Self::StaleCursor => "stale_cursor",
        }
    }
}
