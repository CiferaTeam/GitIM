mod error;
mod id;
mod model;
mod package;
mod reducer;
mod reference;

pub use error::SkillError;
pub use id::{EventId, ProposalId, RevisionId, SkillSlug};
pub use model::{
    validate_comment, validate_description, validate_display_name, validate_summary,
    ProposalStatus, ReducedEvent, SkillComment, SkillEvent, SkillEventKind, SkillProposal,
    SkillRevisionMeta, SkillState,
};
pub use package::{
    canonical_package_sha256, media_type_for_path, validate_package_entries, PackageEntry,
    PackageEntryKind, ResourceDescriptor, ValidatedPackage, MAX_PACKAGE_BYTES, MAX_PACKAGE_FILES,
    MAX_PACKAGE_FILE_BYTES, MAX_SKILL_MD_BYTES,
};
pub use reducer::reduce_skill;
pub use reference::{parse_skill_reference, parse_skill_reference_or_shorthand, SkillReference};

pub const SKILL_SCHEMA_VERSION: u32 = 1;
