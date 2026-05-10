use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum HandlerError {
    #[error("handler is empty")]
    Empty,
    #[error("handler exceeds 39 characters")]
    TooLong,
    #[error("handler contains invalid character: {0}")]
    InvalidChar(char),
    #[error("handler must not start or end with hyphen")]
    HyphenBoundary,
    #[error("handler must not contain consecutive hyphens")]
    ConsecutiveHyphens,
    #[error("handler 'system' is reserved")]
    Reserved,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Handler(String);

impl Handler {
    pub fn new(s: &str) -> Result<Self, HandlerError> {
        if s.is_empty() {
            return Err(HandlerError::Empty);
        }
        if s.len() > 39 {
            return Err(HandlerError::TooLong);
        }
        if s == "system" {
            return Err(HandlerError::Reserved);
        }
        for ch in s.chars() {
            if !matches!(ch, 'a'..='z' | '0'..='9' | '-') {
                return Err(HandlerError::InvalidChar(ch));
            }
        }
        if s.starts_with('-') || s.ends_with('-') {
            return Err(HandlerError::HyphenBoundary);
        }
        if s.contains("--") {
            return Err(HandlerError::ConsecutiveHyphens);
        }
        Ok(Self(s.to_string()))
    }

    /// Construct the reserved `system` handler.
    ///
    /// `Handler::new` rejects `"system"` because users cannot register or
    /// claim that handle — it's the protocol-level voice for daemon-emitted
    /// content (cron fires, future broadcast events). Daemon code that
    /// needs to format such a line goes through this constructor instead.
    /// Keeping the rejection in `new()` while still exposing this factory
    /// preserves the user-facing invariant: every parsed `[@system]` line
    /// originated from daemon code, not from any user clone, because no
    /// CLI / user-input path can construct one.
    pub fn system() -> Self {
        Self("system".to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Handler {
    type Error = HandlerError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Handler::new(&s)
    }
}

impl From<Handler> for String {
    fn from(h: Handler) -> Self {
        h.0
    }
}

impl std::fmt::Display for Handler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
