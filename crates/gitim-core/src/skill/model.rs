use serde::{Deserialize, Serialize};

use crate::types::Handler;

use super::{EventId, ProposalId, ResourceDescriptor, RevisionId, SkillSlug};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillRevisionMeta {
    pub schema_version: u32,
    pub id: RevisionId,
    pub skill: SkillSlug,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_revision: Option<RevisionId>,
    pub content_sha256: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<ResourceDescriptor>,
    pub created_by: Handler,
    pub created_at: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Open,
    Published,
    Rejected,
    Withdrawn,
}

impl ProposalStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Published => "published",
            Self::Rejected => "rejected",
            Self::Withdrawn => "withdrawn",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillComment {
    pub event_id: EventId,
    pub actor: Handler,
    pub body: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillProposal {
    pub id: ProposalId,
    pub revision: RevisionId,
    pub base_revision: RevisionId,
    pub summary: String,
    pub status: ProposalStatus,
    pub created_by: Handler,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<SkillComment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_by: Option<Handler>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillEvent {
    pub schema_version: u32,
    pub id: EventId,
    pub skill: SkillSlug,
    pub actor: Handler,
    pub created_at: String,
    #[serde(flatten)]
    pub kind: SkillEventKind,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SkillEventKind {
    Created {
        display_name: String,
        description: String,
        revision: RevisionId,
    },
    ProposalOpened {
        proposal: ProposalId,
        revision: RevisionId,
        base_revision: RevisionId,
        summary: String,
    },
    ProposalCommented {
        proposal: ProposalId,
        body: String,
    },
    ProposalPublished {
        proposal: ProposalId,
        expected_current_revision: RevisionId,
    },
    ProposalRejected {
        proposal: ProposalId,
    },
    ProposalWithdrawn {
        proposal: ProposalId,
    },
    MetadataUpdated {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    OwnerAdded {
        handler: Handler,
    },
    OwnerRemoved {
        handler: Handler,
        #[serde(default)]
        remove_maintainer: bool,
    },
    MaintainerAdded {
        handler: Handler,
    },
    MaintainerRemoved {
        handler: Handler,
    },
    Archived,
    Unarchived,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReducedEvent {
    pub event: SkillEvent,
    pub effective: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillState {
    pub slug: SkillSlug,
    pub display_name: String,
    pub description: String,
    pub created_by: Handler,
    pub owners: Vec<Handler>,
    pub maintainers: Vec<Handler>,
    pub current_revision: RevisionId,
    pub published_revisions: Vec<RevisionId>,
    pub proposals: Vec<SkillProposal>,
    pub archived: bool,
    pub created_at: String,
    pub updated_at: String,
    pub last_event_id: EventId,
    pub history: Vec<ReducedEvent>,
}

pub fn validate_display_name(value: &str) -> bool {
    bounded_nonblank(value, 80)
}

pub fn validate_description(value: &str) -> bool {
    bounded_nonblank(value, 1024)
}

pub fn validate_summary(value: &str) -> bool {
    bounded_nonblank(value, 500)
}

pub fn validate_comment(value: &str) -> bool {
    bounded_nonblank(value, 10_000)
}

fn bounded_nonblank(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty() && value.chars().count() <= maximum
}
