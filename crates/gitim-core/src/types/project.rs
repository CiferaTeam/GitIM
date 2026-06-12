use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug, PartialEq, Eq)]
pub enum ProjectSlugError {
    #[error("project slug is empty")]
    Empty,
    #[error("project slug exceeds 32 characters")]
    TooLong,
    #[error("project slug contains invalid character: {0:?}")]
    InvalidChar(char),
    #[error("project slug must not start or end with hyphen")]
    HyphenBoundary,
    #[error("project slug must not contain consecutive hyphens")]
    ConsecutiveHyphens,
    #[error("project slug '{0}' is reserved")]
    Reserved(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectSlug(String);

/// Reserved set covers top-level directory names + system handler.
/// Keep in sync with channel reserved expectations and the `reserved` test below.
pub const RESERVED_PROJECT_SLUGS: &[&str] = &[
    "archive", "channels", "projects", "users", "dms", "cards", "flows", "system",
];

impl ProjectSlug {
    pub fn new(s: &str) -> Result<Self, ProjectSlugError> {
        if s.is_empty() {
            return Err(ProjectSlugError::Empty);
        }
        if s.len() > 32 {
            return Err(ProjectSlugError::TooLong);
        }
        for ch in s.chars() {
            if !matches!(ch, 'a'..='z' | '0'..='9' | '-') {
                return Err(ProjectSlugError::InvalidChar(ch));
            }
        }
        if s.starts_with('-') || s.ends_with('-') {
            return Err(ProjectSlugError::HyphenBoundary);
        }
        if s.contains("--") {
            return Err(ProjectSlugError::ConsecutiveHyphens);
        }
        if RESERVED_PROJECT_SLUGS.contains(&s) {
            return Err(ProjectSlugError::Reserved(s.to_string()));
        }
        Ok(Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProjectSlug {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// NOTE: ProjectMeta 跟 ChannelMeta 共享 display_name / created_by / created_at / introduction
// 4 个字段。v1 不抽象 (YAGNI)。修改 ChannelMeta 共享字段时记得检查这里。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectMeta {
    pub display_name: String,
    pub created_by: String,
    pub created_at: String,
    pub introduction: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_slugs() {
        for s in &["design", "infra", "team-a", "ml-x9"] {
            assert!(ProjectSlug::new(s).is_ok(), "{s}");
        }
    }

    #[test]
    fn empty() {
        assert_eq!(ProjectSlug::new(""), Err(ProjectSlugError::Empty));
    }

    #[test]
    fn too_long() {
        let s = "a".repeat(33);
        assert_eq!(ProjectSlug::new(&s), Err(ProjectSlugError::TooLong));
    }

    #[test]
    fn invalid_chars() {
        for s in &[
            "UPPER",
            "with_underscore",
            "with space",
            "slash/here",
            "with.dot",
        ] {
            assert!(
                matches!(ProjectSlug::new(s), Err(ProjectSlugError::InvalidChar(_))),
                "{s}"
            );
        }
    }

    #[test]
    fn hyphen_boundary() {
        for s in &["-leading", "trailing-"] {
            assert_eq!(ProjectSlug::new(s), Err(ProjectSlugError::HyphenBoundary));
        }
    }

    #[test]
    fn consecutive_hyphens() {
        assert_eq!(
            ProjectSlug::new("a--b"),
            Err(ProjectSlugError::ConsecutiveHyphens)
        );
    }

    #[test]
    fn reserved() {
        for s in RESERVED_PROJECT_SLUGS {
            assert_eq!(
                ProjectSlug::new(s),
                Err(ProjectSlugError::Reserved(s.to_string()))
            );
        }
    }
}

#[cfg(test)]
mod meta_tests {
    use super::*;

    #[test]
    fn project_meta_roundtrip() {
        let meta = ProjectMeta {
            display_name: "Design Sprint".to_string(),
            created_by: "lewisliu".to_string(),
            created_at: "2026-05-21T08:00:00Z".to_string(),
            introduction: "All UX work for v2".to_string(),
        };
        let yaml = serde_yaml::to_string(&meta).expect("ser");
        let back: ProjectMeta = serde_yaml::from_str(&yaml).expect("de");
        assert_eq!(meta, back);
    }

    #[test]
    fn project_meta_missing_required_field_fails() {
        // display_name 缺失
        let yaml = r#"
created_by: lewisliu
created_at: "2026-05-21T08:00:00Z"
introduction: hi
"#;
        let res: Result<ProjectMeta, _> = serde_yaml::from_str(yaml);
        assert!(res.is_err());
    }
}
