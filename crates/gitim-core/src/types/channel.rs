use thiserror::Error;

#[derive(Error, Debug)]
pub enum ChannelNameError {
    #[error("channel name is empty")]
    Empty,
    #[error("channel name exceeds 32 characters")]
    TooLong,
    #[error("channel name contains invalid character: {0}")]
    InvalidChar(char),
    #[error("channel name must not start or end with hyphen")]
    HyphenBoundary,
    #[error("channel name must not contain consecutive hyphens")]
    ConsecutiveHyphens,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChannelName(String);

impl ChannelName {
    pub fn new(s: &str) -> Result<Self, ChannelNameError> {
        if s.is_empty() {
            return Err(ChannelNameError::Empty);
        }
        if s.len() > 32 {
            return Err(ChannelNameError::TooLong);
        }
        for ch in s.chars() {
            if !matches!(ch, 'a'..='z' | '0'..='9' | '-') {
                return Err(ChannelNameError::InvalidChar(ch));
            }
        }
        if s.starts_with('-') || s.ends_with('-') {
            return Err(ChannelNameError::HyphenBoundary);
        }
        if s.contains("--") {
            return Err(ChannelNameError::ConsecutiveHyphens);
        }
        Ok(Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ChannelName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_channel_names() {
        for name in &["general", "random", "dev-chat", "a", "a1b2"] {
            assert!(
                ChannelName::new(name).is_ok(),
                "expected '{}' to be valid",
                name
            );
        }
    }

    #[test]
    fn test_empty_name() {
        let err = ChannelName::new("").unwrap_err();
        assert!(matches!(err, ChannelNameError::Empty));
    }

    #[test]
    fn test_too_long() {
        let name = "a".repeat(33);
        let err = ChannelName::new(&name).unwrap_err();
        assert!(matches!(err, ChannelNameError::TooLong));
    }

    #[test]
    fn test_invalid_chars() {
        for name in &["/", "..", "../../etc", "UPPER", "under_score", "space name"] {
            let err = ChannelName::new(name).unwrap_err();
            assert!(
                matches!(err, ChannelNameError::InvalidChar(_)),
                "expected InvalidChar for '{}', got: {:?}",
                name,
                err
            );
        }
    }

    #[test]
    fn test_hyphen_boundary() {
        for name in &["-start", "end-"] {
            let err = ChannelName::new(name).unwrap_err();
            assert!(
                matches!(err, ChannelNameError::HyphenBoundary),
                "expected HyphenBoundary for '{}', got: {:?}",
                name,
                err
            );
        }
    }

    #[test]
    fn test_consecutive_hyphens() {
        let err = ChannelName::new("a--b").unwrap_err();
        assert!(matches!(err, ChannelNameError::ConsecutiveHyphens));
    }
}
