mod error;
mod id;
mod reference;
mod types;

pub use error::SkillError;
pub use id::{ProposalId, RequestId, RevisionId, SkillSlug};
pub use reference::{parse_skill_reference, scan_skill_references, SkillReference};
pub use types::{
    ProposalStatus, ResourceDescriptor, SkillCatalogEntry, SkillCreateRequest, SkillListQuery,
    SkillListResponse, SkillLoadResponse, SkillMeta, SkillMutationRequest, SkillMutationResult,
    SkillOperation, SkillProposalMeta, SkillProposalTransitionRequest, SkillProposeRequest,
    SkillPublicationMeta, SkillReceipt, SkillReceiptRequest, SkillReceiptScope, SkillResourceQuery,
    SkillResourceResponse, SkillRevisionMeta, SkillShowQuery, SkillShowResponse,
    WorkspaceSkillMeta,
};

pub const SKILL_SCHEMA_VERSION: u32 = 1;
