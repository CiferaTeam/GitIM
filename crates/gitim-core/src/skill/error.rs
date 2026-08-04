use thiserror::Error;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum SkillError {
    #[error("invalid Skill slug")]
    InvalidSlug,
    #[error("invalid Skill reference")]
    InvalidReference,
    #[error("invalid Skill package")]
    InvalidPackage,
    #[error("Skill package exceeds its size limit")]
    PackageTooLarge,
    #[error("invalid Skill event history")]
    InvalidHistory,
}

impl SkillError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidSlug => "skill_invalid_slug",
            Self::InvalidReference => "skill_invalid_ref",
            Self::InvalidPackage => "skill_invalid_package",
            Self::PackageTooLarge => "skill_package_too_large",
            Self::InvalidHistory => "skill_invalid_history",
        }
    }
}
