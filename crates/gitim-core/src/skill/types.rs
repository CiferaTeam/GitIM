use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::types::{Handler, ThreadEntry};

use super::{ProposalId, RequestId, RevisionId, SkillReference, SkillSlug, SKILL_SCHEMA_VERSION};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceSkillMeta {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    pub administrators: Vec<Handler>,
    pub control_revision: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillMeta {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    pub slug: SkillSlug,
    pub display_name: String,
    pub description: String,
    pub created_by: Handler,
    pub owners: Vec<Handler>,
    pub maintainers: Vec<Handler>,
    pub current_revision: RevisionId,
    pub open_proposal_count: u16,
    #[serde(default)]
    pub open_proposal_ids: Vec<ProposalId>,
    pub control_revision: u64,
    pub event_revision: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillRevisionMeta {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    pub id: RevisionId,
    pub skill: SkillSlug,
    #[serde(default)]
    pub base_revision: Option<RevisionId>,
    pub content_sha256: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<ResourceDescriptor>,
    pub created_by: Handler,
    pub created_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillPublicationMeta {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    pub skill: SkillSlug,
    pub revision: RevisionId,
    pub content_sha256: String,
    #[serde(default)]
    pub base_revision: Option<RevisionId>,
    #[serde(default)]
    pub proposal: Option<ProposalId>,
    pub published_by: Handler,
    pub published_at: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Open,
    Published,
    Rejected,
    Withdrawn,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillProposalMeta {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    pub id: ProposalId,
    pub skill: SkillSlug,
    pub candidate_revision: RevisionId,
    pub base_revision: RevisionId,
    pub summary: String,
    pub status: ProposalStatus,
    pub created_by: Handler,
    pub created_at: String,
    pub updated_at: String,
    pub state_revision: u64,
    #[serde(default)]
    pub resolved_by: Option<Handler>,
    #[serde(default)]
    pub resolved_at: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillReceiptScope {
    Workspace,
    Skill,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillOperation {
    WorkspaceBootstrap,
    WorkspaceAdminAdd,
    WorkspaceAdminRemove,
    SkillCreate,
    ProposalCreate,
    ProposalComment,
    ProposalPublish,
    ProposalReject,
    ProposalWithdraw,
    MetadataUpdate,
    OwnerAdd,
    OwnerRemove,
    MaintainerAdd,
    MaintainerRemove,
    Archive,
    Unarchive,
    OwnerRecovered,
    RepairSkillState,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillReceiptRequest {
    pub payload_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<SkillSlug>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<RevisionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_revision: Option<RevisionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_revision: Option<RevisionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal: Option<ProposalId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Handler>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_content_revision: Option<RevisionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_control_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_proposal_revision: Option<u64>,
    #[serde(default)]
    pub remove_maintainer: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_tip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_tree: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillReceipt {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    pub id: RequestId,
    pub scope: SkillReceiptScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<SkillSlug>,
    pub actor: Handler,
    pub operation: SkillOperation,
    pub request: SkillReceiptRequest,
    pub result: SkillMutationResult,
    pub created_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillMutationResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_ref: Option<SkillReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_revision: Option<RevisionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_state_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_status: Option<ProposalStatus>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SkillMutationRequest {
    WorkspaceBootstrap(SkillWorkspaceBootstrapRequest),
    Create(SkillCreateRequest),
    Propose(SkillProposeRequest),
    ProposalTransition(SkillProposalTransitionRequest),
    MetadataUpdate(SkillMetadataUpdateRequest),
    RoleUpdate(SkillRoleUpdateRequest),
    ArchiveTransition(SkillArchiveTransitionRequest),
    Repair(SkillRepairRequest),
}

impl SkillMutationRequest {
    pub fn request_id(&self) -> &RequestId {
        match self {
            Self::WorkspaceBootstrap(request) => &request.request_id,
            Self::Create(request) => &request.request_id,
            Self::Propose(request) => &request.request_id,
            Self::ProposalTransition(request) => &request.request_id,
            Self::MetadataUpdate(request) => &request.request_id,
            Self::RoleUpdate(request) => &request.request_id,
            Self::ArchiveTransition(request) => &request.request_id,
            Self::Repair(request) => &request.request_id,
        }
    }

    pub const fn operation(&self) -> SkillOperation {
        match self {
            Self::WorkspaceBootstrap(_) => SkillOperation::WorkspaceBootstrap,
            Self::Create(_) => SkillOperation::SkillCreate,
            Self::Propose(_) => SkillOperation::ProposalCreate,
            Self::ProposalTransition(request) => request.operation,
            Self::MetadataUpdate(_) => SkillOperation::MetadataUpdate,
            Self::RoleUpdate(request) => request.operation,
            Self::ArchiveTransition(request) => request.operation,
            Self::Repair(_) => SkillOperation::RepairSkillState,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillWorkspaceBootstrapRequest {
    pub request_id: RequestId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillRepairScope {
    Workspace,
    Skill(SkillSlug),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillRepairRequest {
    pub request_id: RequestId,
    pub scope: SkillRepairScope,
    pub conflict_tip: String,
    pub accepted_tree: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillListQuery {
    pub archived: bool,
    pub limit: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillCatalogEntry {
    pub slug: SkillSlug,
    pub display_name: String,
    pub description: String,
    pub current_revision: RevisionId,
    pub owners: Vec<Handler>,
    pub maintainers: Vec<Handler>,
    pub open_proposal_count: u16,
    pub archived: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillListResponse {
    pub skills: Vec<SkillCatalogEntry>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResourceDescriptor {
    pub path: String,
    pub byte_size: u64,
    pub media_type: String,
    pub text: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillShowQuery {
    pub slug: SkillSlug,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<RevisionId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillShowResponse {
    pub meta: SkillMeta,
    pub revision: SkillRevisionMeta,
    pub canonical_ref: SkillReference,
    pub archived: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillLoadResponse {
    pub canonical_ref: SkillReference,
    pub revision: SkillRevisionMeta,
    pub skill_markdown: String,
    pub resources: Vec<ResourceDescriptor>,
    pub archived: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillResourceQuery {
    pub reference: SkillReference,
    pub path: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillResourceResponse {
    pub canonical_ref: SkillReference,
    pub path: String,
    pub media_type: String,
    pub text: bool,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillPageQuery {
    pub slug: SkillSlug,
    pub limit: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillRevisionListResponse {
    pub revisions: Vec<SkillRevisionMeta>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkillHistoryResponse {
    pub entries: Vec<ThreadEntry>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillProposalListQuery {
    pub slug: SkillSlug,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ProposalStatus>,
    pub limit: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillProposalListResponse {
    pub proposals: Vec<SkillProposalMeta>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillProposalShowQuery {
    pub proposal_id: ProposalId,
    pub diff: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillProposalDiff {
    pub text: String,
    pub changed_resources: Vec<ResourceDescriptor>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillProposalShowResponse {
    pub proposal: SkillProposalMeta,
    pub candidate_revision: SkillRevisionMeta,
    pub base_revision: SkillRevisionMeta,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<SkillProposalDiff>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillProposalResourceQuery {
    pub proposal_id: ProposalId,
    pub path: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillProposalResourceResponse {
    pub proposal_id: ProposalId,
    pub candidate_revision: RevisionId,
    pub path: String,
    pub media_type: String,
    pub text: bool,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillCreateRequest {
    pub request_id: RequestId,
    pub slug: SkillSlug,
    pub display_name: String,
    pub description: String,
    pub source_directory: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillProposeRequest {
    pub request_id: RequestId,
    pub slug: SkillSlug,
    pub base_revision: RevisionId,
    pub summary: String,
    pub source_directory: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillProposalTransitionRequest {
    pub request_id: RequestId,
    pub proposal_id: ProposalId,
    pub operation: SkillOperation,
    pub expected_state_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_control_revision: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillMetadataUpdateRequest {
    pub request_id: RequestId,
    pub slug: SkillSlug,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub expected_control_revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillRoleUpdateRequest {
    pub request_id: RequestId,
    pub slug: SkillSlug,
    pub operation: SkillOperation,
    pub target: Handler,
    #[serde(default)]
    pub remove_maintainer: bool,
    pub expected_control_revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillArchiveTransitionRequest {
    pub request_id: RequestId,
    pub slug: SkillSlug,
    pub operation: SkillOperation,
    pub expected_control_revision: u64,
}

const fn schema_version() -> u32 {
    SKILL_SCHEMA_VERSION
}
