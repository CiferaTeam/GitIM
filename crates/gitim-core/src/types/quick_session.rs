use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::handler::Handler;

pub const QUICK_SESSION_ID_PREFIX: &str = "qs-";
pub const QUICK_SESSION_ID_LEN: usize = 29; // "qs-" + 26-char ULID
pub const MAX_QUICK_SESSION_TITLE_LEN: usize = 80;

#[derive(Error, Debug)]
pub enum QuickSessionError {
    #[error("quick session id must start with 'qs-' and be 29 characters")]
    InvalidId,
    #[error("title cannot be empty")]
    EmptyTitle,
    #[error("title too long (max {0})")]
    TitleTooLong(usize),
    #[error("invalid status '{0}'")]
    InvalidStatus(String),
    #[error("invalid title source '{0}'")]
    InvalidTitleSource(String),
    #[error("invalid handler: {0}")]
    InvalidHandler(String),
    #[error("title not set before assistant content")]
    TitleRequired,
    #[error("session id collision after {0} retries")]
    IdCollision(u32),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuickSessionStatus {
    NeedsTitle,
    Active,
    Running,
    Error,
    Archived,
}

impl QuickSessionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            QuickSessionStatus::NeedsTitle => "needs_title",
            QuickSessionStatus::Active => "active",
            QuickSessionStatus::Running => "running",
            QuickSessionStatus::Error => "error",
            QuickSessionStatus::Archived => "archived",
        }
    }
}

impl std::str::FromStr for QuickSessionStatus {
    type Err = QuickSessionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "needs_title" => Ok(QuickSessionStatus::NeedsTitle),
            "active" => Ok(QuickSessionStatus::Active),
            "running" => Ok(QuickSessionStatus::Running),
            "error" => Ok(QuickSessionStatus::Error),
            "archived" => Ok(QuickSessionStatus::Archived),
            _ => Err(QuickSessionError::InvalidStatus(s.to_string())),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuickSessionTitleSource {
    None,
    ApiSet,
}

impl QuickSessionTitleSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            QuickSessionTitleSource::None => "none",
            QuickSessionTitleSource::ApiSet => "api_set",
        }
    }
}

impl std::str::FromStr for QuickSessionTitleSource {
    type Err = QuickSessionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "none" => Ok(QuickSessionTitleSource::None),
            "api_set" => Ok(QuickSessionTitleSource::ApiSet),
            _ => Err(QuickSessionError::InvalidTitleSource(s.to_string())),
        }
    }
}

/// Durable metadata stored in `quick-sessions/<id>/session.meta.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuickSessionMeta {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default = "default_title_source")]
    pub title_source: QuickSessionTitleSource,
    pub agent_id: String,
    pub created_by: Handler,
    #[serde(default = "default_status")]
    pub status: QuickSessionStatus,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message_preview: Option<String>,
    /// Stable ref: `session:qs-<ulid>`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_: Option<String>,
}

fn default_status() -> QuickSessionStatus {
    QuickSessionStatus::NeedsTitle
}

fn default_title_source() -> QuickSessionTitleSource {
    QuickSessionTitleSource::None
}

impl QuickSessionMeta {
    pub fn ref_string(&self) -> String {
        format!("session:{}", self.id)
    }
}

/// Runtime-local state stored in `.gitim-runtime/quick-sessions/<id>.state.json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuickSessionRuntimeState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_usage: Option<serde_json::Value>,
    #[serde(default)]
    pub estimated_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_session_usage: Option<serde_json::Value>,
    #[serde(default)]
    pub usage_notice_pending: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_compaction_at: Option<String>,
}

/// Request to set the session title (title API gate).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetQuickSessionTitleRequest {
    pub title: String,
}

/// Response after creating a quick session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateQuickSessionResponse {
    pub id: String,
    pub ref_: String,
    pub status: QuickSessionStatus,
    pub meta: QuickSessionMeta,
}

/// Response for quick session list items (hub view).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuickSessionListItem {
    pub id: String,
    pub title: String,
    pub agent_id: String,
    pub status: QuickSessionStatus,
    pub updated_at: String,
    pub ref_: String,
    pub last_message_preview: Option<String>,
}

/// Request to create a quick session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateQuickSessionRequest {
    pub agent_id: String,
    pub first_message: String,
}

/// Request to send a message to a quick session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendQuickSessionMessageRequest {
    pub body: String,
}

/// Validate a quick session ID.
///
/// Format: `qs-` followed by 26 characters of Crockford base32
/// (digits 0-9 and uppercase letters excluding I, L, O, U).
pub fn validate_quick_session_id(id: &str) -> Result<(), QuickSessionError> {
    if id.len() != QUICK_SESSION_ID_LEN || !id.starts_with(QUICK_SESSION_ID_PREFIX) {
        return Err(QuickSessionError::InvalidId);
    }
    let ulid_part = &id[3..];
    for ch in ulid_part.chars() {
        if !matches!(ch, '0'..='9' | 'A'..='H' | 'J' | 'K' | 'M' | 'N' | 'P'..='T' | 'V'..='Z') {
            return Err(QuickSessionError::InvalidId);
        }
    }
    Ok(())
}

pub fn validate_quick_session_title(title: &str) -> Result<(), QuickSessionError> {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return Err(QuickSessionError::EmptyTitle);
    }
    if trimmed.chars().count() > MAX_QUICK_SESSION_TITLE_LEN {
        return Err(QuickSessionError::TitleTooLong(MAX_QUICK_SESSION_TITLE_LEN));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_quick_session_id_valid() {
        // Example ULID: 01ARZ3NDEKTSV4RRFFQ69G5FAV
        assert!(validate_quick_session_id("qs-01ARZ3NDEKTSV4RRFFQ69G5FAV").is_ok());
    }

    #[test]
    fn validate_quick_session_id_invalid_too_short() {
        assert!(validate_quick_session_id("qs-01ARZ3ND").is_err());
    }

    #[test]
    fn validate_quick_session_id_invalid_chars() {
        assert!(validate_quick_session_id("qs-01ARZ3NDEKTSV4RRFFQ69G5ILO").is_err()); // I,L,O
        assert!(validate_quick_session_id("qs-01ARZ3NDEKTSV4RRFFQ69G5FAu").is_err());
        // lowercase
    }

    #[test]
    fn validate_quick_session_id_no_prefix() {
        assert!(validate_quick_session_id("01ARZ3NDEKTSV4RRFFQ69G5FAV").is_err());
    }

    #[test]
    fn validate_title_valid() {
        assert!(validate_quick_session_title("Fix login bug").is_ok());
    }

    #[test]
    fn validate_title_empty() {
        assert!(validate_quick_session_title("").is_err());
        assert!(validate_quick_session_title("   ").is_err());
    }

    #[test]
    fn validate_title_too_long() {
        let long = "a".repeat(81);
        assert!(validate_quick_session_title(&long).is_err());
    }

    #[test]
    fn validate_title_counts_chars_not_bytes() {
        let title = "短".repeat(80);
        assert!(validate_quick_session_title(&title).is_ok());

        let long = "短".repeat(81);
        assert!(validate_quick_session_title(&long).is_err());
    }

    #[test]
    fn status_roundtrip() {
        assert_eq!(
            "needs_title".parse::<QuickSessionStatus>().unwrap(),
            QuickSessionStatus::NeedsTitle
        );
        assert_eq!(
            "active".parse::<QuickSessionStatus>().unwrap(),
            QuickSessionStatus::Active
        );
    }

    #[test]
    fn title_source_roundtrip() {
        assert_eq!(
            "none".parse::<QuickSessionTitleSource>().unwrap(),
            QuickSessionTitleSource::None
        );
        assert_eq!(
            "api_set".parse::<QuickSessionTitleSource>().unwrap(),
            QuickSessionTitleSource::ApiSet
        );
    }
}
