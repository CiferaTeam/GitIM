mod error;
mod id;
mod package;
mod portable_path;
mod reference;
mod transition;
mod types;

pub use error::SkillError;
pub use id::{ProposalId, RequestId, RevisionId, SkillSlug};
pub use package::{
    canonical_package_sha256, media_type_for_path, truncate_utf8_bytes, validate_package_entries,
    PackageEntry, PackageEntryKind, ValidatedPackage, MAX_PACKAGE_BYTES, MAX_PACKAGE_FILES,
    MAX_PACKAGE_FILE_BYTES, MAX_SKILL_MD_BYTES,
};
pub use reference::{parse_skill_reference, scan_skill_references, SkillReference};
pub use transition::{
    plan_skill_mutation, validate_skill_commit, SkillCommitEvidence, SkillConflictCheckpoint,
    SkillMutationContext, SkillMutationPlan, SkillObjectSnapshot, SkillProposalSnapshot,
    SkillRepairAcceptedState, SkillRepositorySnapshot, SkillRevisionSnapshot,
    SkillTransitionOutcome, SkillTreeEdit,
};
pub use types::{
    ProposalStatus, ResourceDescriptor, SkillCatalogEntry, SkillCreateRequest,
    SkillHistoryResponse, SkillListQuery, SkillListResponse, SkillLoadResponse, SkillMeta,
    SkillMutationRequest, SkillMutationResult, SkillOperation, SkillPageQuery, SkillProposalDiff,
    SkillProposalListQuery, SkillProposalListResponse, SkillProposalMeta,
    SkillProposalResourceQuery, SkillProposalResourceResponse, SkillProposalShowQuery,
    SkillProposalShowResponse, SkillProposalTransitionRequest, SkillProposeRequest,
    SkillPublicationMeta, SkillReceipt, SkillReceiptRequest, SkillReceiptScope, SkillRepairRequest,
    SkillRepairScope, SkillResourceQuery, SkillResourceResponse, SkillRevisionListResponse,
    SkillRevisionMeta, SkillShowQuery, SkillShowResponse, SkillWorkspaceBootstrapRequest,
    WorkspaceSkillMeta,
};

pub const SKILL_SCHEMA_VERSION: u32 = 1;
