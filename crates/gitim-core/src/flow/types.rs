use thiserror::Error;

#[derive(Error, Debug)]
pub enum FlowSlugError {
    #[error("flow slug is empty")]
    Empty,
    #[error("flow slug exceeds 39 characters")]
    TooLong,
    #[error("flow slug contains invalid character: {0}")]
    InvalidChar(char),
    #[error("flow slug must not start or end with hyphen")]
    HyphenBoundary,
    #[error("flow slug must not contain consecutive hyphens")]
    ConsecutiveHyphens,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FlowSlug(String);

impl FlowSlug {
    pub fn new(s: &str) -> Result<Self, FlowSlugError> {
        if s.is_empty() {
            return Err(FlowSlugError::Empty);
        }
        if s.len() > 39 {
            return Err(FlowSlugError::TooLong);
        }
        for ch in s.chars() {
            if !matches!(ch, 'a'..='z' | '0'..='9' | '-') {
                return Err(FlowSlugError::InvalidChar(ch));
            }
        }
        if s.starts_with('-') || s.ends_with('-') {
            return Err(FlowSlugError::HyphenBoundary);
        }
        if s.contains("--") {
            return Err(FlowSlugError::ConsecutiveHyphens);
        }
        Ok(Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for FlowSlug {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub fn flow_path(slug: &FlowSlug) -> std::path::PathBuf {
    std::path::PathBuf::from("flows")
        .join(slug.as_str())
        .join("index.md")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_slugs() {
        for name in &["release", "kickoff", "weekly-retro", "a", "a1b2"] {
            assert!(
                FlowSlug::new(name).is_ok(),
                "expected '{}' to be valid",
                name
            );
        }
    }

    #[test]
    fn test_empty_slug() {
        let err = FlowSlug::new("").unwrap_err();
        assert!(matches!(err, FlowSlugError::Empty));
    }

    #[test]
    fn test_too_long() {
        let name = "a".repeat(40);
        let err = FlowSlug::new(&name).unwrap_err();
        assert!(matches!(err, FlowSlugError::TooLong));
    }

    #[test]
    fn test_invalid_chars() {
        for name in &["UPPER", "under_score", "space name", "../etc", "x/y"] {
            let err = FlowSlug::new(name).unwrap_err();
            assert!(
                matches!(err, FlowSlugError::InvalidChar(_)),
                "for '{}', got {:?}",
                name,
                err
            );
        }
    }

    #[test]
    fn test_hyphen_boundary() {
        for name in &["-start", "end-"] {
            let err = FlowSlug::new(name).unwrap_err();
            assert!(matches!(err, FlowSlugError::HyphenBoundary));
        }
    }

    #[test]
    fn test_consecutive_hyphens() {
        let err = FlowSlug::new("a--b").unwrap_err();
        assert!(matches!(err, FlowSlugError::ConsecutiveHyphens));
    }

    #[test]
    fn test_flow_path() {
        let slug = FlowSlug::new("release").unwrap();
        assert_eq!(
            flow_path(&slug),
            std::path::PathBuf::from("flows/release/index.md")
        );
    }
}
