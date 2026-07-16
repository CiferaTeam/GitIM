use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::Handler;

pub const QUICK_SESSION_ID_PREFIX: &str = "qs-";
pub const QUICK_SESSION_ATTEMPT_ID_PREFIX: &str = "qa-";
pub const QUICK_SESSION_TITLE_MAX_CHARS: usize = 80;
pub const QUICK_SESSION_SUMMARY_MAX_CHARS: usize = 4_000;
pub const QUICK_SESSION_PREVIEW_MAX_CHARS: usize = 160;

const OPAQUE_ID_CHARS: usize = 26;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum QuickSessionError {
    #[error("invalid quick session id")]
    InvalidSessionId,
    #[error("invalid quick session attempt id")]
    InvalidAttemptId,
    #[error("quick session title cannot be empty")]
    EmptyTitle,
    #[error("quick session title exceeds {QUICK_SESSION_TITLE_MAX_CHARS} characters")]
    TitleTooLong,
    #[error("quick session summary cannot be empty")]
    EmptySummary,
    #[error("quick session summary exceeds {QUICK_SESSION_SUMMARY_MAX_CHARS} characters")]
    SummaryTooLong,
    #[error("quick session actor is not authorized for this transition")]
    UnauthorizedActor,
    #[error("quick session transition is not valid from the current state")]
    InvalidState,
    #[error("quick session attempt does not match the active claim")]
    StaleAttempt,
    #[error("quick session input line is not newer than the completed input")]
    StaleInputLine,
    #[error("quick session reply must target the claimed input line")]
    InputLineMismatch,
    #[error("quick session title is required before an agent reply")]
    TitleRequired,
    #[error("quick session line number is invalid")]
    InvalidLineNumber,
    #[error("quick session error message cannot be empty")]
    EmptyError,
    #[error("invalid quick session metadata: {0}")]
    InvalidMeta(String),
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuickSessionStatus {
    #[default]
    NeedsTitle,
    Running,
    Active,
    Error,
    Archived,
}

impl QuickSessionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NeedsTitle => "needs_title",
            Self::Running => "running",
            Self::Active => "active",
            Self::Error => "error",
            Self::Archived => "archived",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuickSessionMeta {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub agent_id: String,
    pub created_by: String,
    #[serde(default)]
    pub status: QuickSessionStatus,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_from: Option<QuickSessionStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_updated_at: Option<String>,
    #[serde(default)]
    pub last_message_preview: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub processing_input_line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub processing_started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_completed_attempt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_completed_input_line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_completed_line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failed_attempt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_human_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_human_line: Option<u64>,
    #[serde(default = "default_revision")]
    pub revision: u64,
}

impl QuickSessionMeta {
    pub fn new(id: String, agent_id: String, created_by: String, created_at: String) -> Self {
        Self {
            id,
            title: None,
            agent_id,
            created_by,
            status: QuickSessionStatus::NeedsTitle,
            created_at: created_at.clone(),
            updated_at: created_at,
            archived_at: None,
            archived_from: None,
            summary: None,
            summary_updated_at: None,
            last_message_preview: String::new(),
            error: None,
            processing_input_line: None,
            processing_started_at: None,
            attempt_id: None,
            last_completed_attempt_id: None,
            last_completed_input_line: None,
            last_completed_line: None,
            last_failed_attempt_id: None,
            last_human_request_id: None,
            last_human_line: None,
            revision: default_revision(),
        }
    }

    pub fn ref_string(&self) -> String {
        format!("session:{}", self.id)
    }
}

const fn default_revision() -> u64 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuickSessionTransition {
    HumanMessage {
        actor: String,
        line_number: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        preview: String,
        now: String,
    },
    Claim {
        actor: String,
        input_line: u64,
        attempt_id: String,
        now: String,
    },
    SetTitle {
        actor: String,
        attempt_id: String,
        title: String,
        now: String,
    },
    SetSummary {
        actor: String,
        attempt_id: String,
        summary: String,
        now: String,
    },
    AgentReply {
        actor: String,
        input_line: u64,
        attempt_id: String,
        output_line: u64,
        preview: String,
        now: String,
    },
    MarkError {
        actor: String,
        attempt_id: String,
        error: String,
        now: String,
    },
    Archive {
        actor: String,
        now: String,
    },
    Unarchive {
        actor: String,
        now: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransitionOutcome {
    Applied,
    Duplicate {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line_number: Option<u64>,
    },
}

pub fn validate_quick_session_id(id: &str) -> Result<(), QuickSessionError> {
    validate_opaque_id(id, QUICK_SESSION_ID_PREFIX)
        .then_some(())
        .ok_or(QuickSessionError::InvalidSessionId)
}

pub fn validate_quick_session_attempt_id(id: &str) -> Result<(), QuickSessionError> {
    validate_opaque_id(id, QUICK_SESSION_ATTEMPT_ID_PREFIX)
        .then_some(())
        .ok_or(QuickSessionError::InvalidAttemptId)
}

fn validate_opaque_id(id: &str, prefix: &str) -> bool {
    let Some(value) = id.strip_prefix(prefix) else {
        return false;
    };
    value.len() == OPAQUE_ID_CHARS
        && value.bytes().all(|byte| {
            matches!(
                byte,
                b'0'..=b'9'
                    | b'A'..=b'H'
                    | b'J'
                    | b'K'
                    | b'M'
                    | b'N'
                    | b'P'..=b'T'
                    | b'V'..=b'Z'
            )
        })
}

pub fn validate_quick_session_title(title: &str) -> Result<(), QuickSessionError> {
    validate_text_limit(
        title,
        QUICK_SESSION_TITLE_MAX_CHARS,
        QuickSessionError::EmptyTitle,
        QuickSessionError::TitleTooLong,
    )
}

pub fn validate_quick_session_summary(summary: &str) -> Result<(), QuickSessionError> {
    validate_text_limit(
        summary,
        QUICK_SESSION_SUMMARY_MAX_CHARS,
        QuickSessionError::EmptySummary,
        QuickSessionError::SummaryTooLong,
    )
}

fn validate_text_limit(
    value: &str,
    limit: usize,
    empty: QuickSessionError,
    too_long: QuickSessionError,
) -> Result<(), QuickSessionError> {
    if value.trim().is_empty() {
        return Err(empty);
    }
    if value.chars().count() > limit {
        return Err(too_long);
    }
    Ok(())
}

pub fn truncate_quick_session_preview(preview: &str) -> String {
    preview
        .chars()
        .take(QUICK_SESSION_PREVIEW_MAX_CHARS)
        .collect()
}

pub fn validate_quick_session_meta(meta: &QuickSessionMeta) -> Result<(), QuickSessionError> {
    validate_quick_session_id(&meta.id)?;
    Handler::new(&meta.agent_id)
        .map_err(|error| QuickSessionError::InvalidMeta(format!("invalid agent_id: {error}")))?;
    Handler::new(&meta.created_by)
        .map_err(|error| QuickSessionError::InvalidMeta(format!("invalid created_by: {error}")))?;
    if meta.revision == 0 {
        return invalid_meta("revision must be positive");
    }
    if let Some(title) = meta.title.as_deref() {
        validate_quick_session_title(title)?;
    }
    if meta.last_message_preview.chars().count() > QUICK_SESSION_PREVIEW_MAX_CHARS {
        return invalid_meta("last_message_preview exceeds its limit");
    }
    match (meta.summary.as_deref(), meta.summary_updated_at.as_deref()) {
        (None, None) => {}
        (Some(summary), Some(_)) => validate_quick_session_summary(summary)?,
        _ => return invalid_meta("summary and summary_updated_at must be set together"),
    }

    let has_complete_claim = meta.processing_input_line.is_some()
        && meta.processing_started_at.is_some()
        && meta.attempt_id.is_some();
    let has_any_claim = meta.processing_input_line.is_some()
        || meta.processing_started_at.is_some()
        || meta.attempt_id.is_some();
    if meta.status == QuickSessionStatus::Running {
        if !has_complete_claim || meta.processing_input_line == Some(0) {
            return invalid_meta("running status requires a complete claim");
        }
        if let Some(attempt_id) = meta.attempt_id.as_deref() {
            validate_quick_session_attempt_id(attempt_id)?;
        }
    } else if has_any_claim {
        return invalid_meta("only running status may retain a claim");
    }

    if meta.status == QuickSessionStatus::Archived {
        if meta.archived_at.is_none() {
            return invalid_meta("archived status requires archived_at");
        }
        match (meta.archived_from, meta.title.is_some()) {
            (Some(QuickSessionStatus::Active), true)
            | (Some(QuickSessionStatus::NeedsTitle), false)
            | (Some(QuickSessionStatus::Error), _) => {}
            (None, _) => return invalid_meta("archived status requires archived_from"),
            _ => return invalid_meta("archived_from does not match the stable title state"),
        }
    } else if meta.archived_at.is_some() || meta.archived_from.is_some() {
        return invalid_meta("active metadata cannot retain archive fields");
    }

    match meta.status {
        QuickSessionStatus::NeedsTitle if meta.title.is_some() => {
            return invalid_meta("needs_title status cannot have a title");
        }
        QuickSessionStatus::Active if meta.title.is_none() => {
            return invalid_meta("active status requires a title");
        }
        _ => {}
    }

    if meta
        .error
        .as_deref()
        .is_some_and(|error| error.trim().is_empty())
    {
        return invalid_meta("error message cannot be empty");
    }
    let requires_error = meta.status == QuickSessionStatus::Error
        || matches!(
            (meta.status, meta.archived_from),
            (
                QuickSessionStatus::Archived,
                Some(QuickSessionStatus::Error)
            )
        );
    if requires_error && meta.error.is_none() {
        return invalid_meta("error state requires an error message");
    }
    if meta.status == QuickSessionStatus::Running && meta.error.is_some() {
        return invalid_meta("running status cannot retain error diagnostics");
    }
    if meta.error.is_some() && !requires_error && meta.last_failed_attempt_id.is_none() {
        return invalid_meta("actionable error diagnostics require a failed attempt id");
    }

    let completed_count = usize::from(meta.last_completed_attempt_id.is_some())
        + usize::from(meta.last_completed_input_line.is_some())
        + usize::from(meta.last_completed_line.is_some());
    if completed_count != 0 && completed_count != 3 {
        return invalid_meta("completion fields must be set together");
    }
    if let (Some(attempt_id), Some(input_line), Some(output_line)) = (
        meta.last_completed_attempt_id.as_deref(),
        meta.last_completed_input_line,
        meta.last_completed_line,
    ) {
        validate_quick_session_attempt_id(attempt_id)?;
        if input_line == 0 || output_line <= input_line {
            return invalid_meta("completion line numbers are invalid");
        }
        if meta
            .last_human_line
            .is_none_or(|last_human_line| input_line > last_human_line)
        {
            return invalid_meta("completed input must be a known human line");
        }
    }
    if meta.last_human_line == Some(0) {
        return invalid_meta("last_human_line must be positive");
    }
    if meta.last_human_request_id.is_some() && meta.last_human_line.is_none() {
        return invalid_meta("human request id requires a line number");
    }
    if meta.processing_input_line.is_some_and(|input_line| {
        meta.last_human_line
            .is_none_or(|last_human_line| input_line > last_human_line)
    }) {
        return invalid_meta("processing input must be a known human line");
    }
    if let Some(attempt_id) = meta.last_failed_attempt_id.as_deref() {
        validate_quick_session_attempt_id(attempt_id)?;
        if meta.error.is_none() || meta.status == QuickSessionStatus::Running {
            return invalid_meta("failed attempt id requires retained error diagnostics");
        }
    }
    Ok(())
}

fn invalid_meta<T>(message: &str) -> Result<T, QuickSessionError> {
    Err(QuickSessionError::InvalidMeta(message.to_string()))
}

pub fn apply_quick_session_transition(
    meta: &mut QuickSessionMeta,
    transition: QuickSessionTransition,
) -> Result<TransitionOutcome, QuickSessionError> {
    validate_quick_session_meta(meta)?;
    let mut next = meta.clone();
    let outcome = apply_transition(&mut next, transition)?;
    if outcome == TransitionOutcome::Applied {
        validate_quick_session_meta(&next)?;
        *meta = next;
    }
    Ok(outcome)
}

fn apply_transition(
    meta: &mut QuickSessionMeta,
    transition: QuickSessionTransition,
) -> Result<TransitionOutcome, QuickSessionError> {
    match transition {
        QuickSessionTransition::HumanMessage {
            actor,
            line_number,
            request_id,
            preview,
            now,
        } => {
            require_creator(meta, &actor)?;
            if let Some(request_id) = request_id.as_deref() {
                if meta.last_human_request_id.as_deref() == Some(request_id) {
                    return Ok(TransitionOutcome::Duplicate {
                        line_number: meta.last_human_line,
                    });
                }
            }
            if meta.status == QuickSessionStatus::Archived {
                return Err(QuickSessionError::InvalidState);
            }
            if line_number == 0 {
                return Err(QuickSessionError::InvalidLineNumber);
            }
            if meta
                .last_human_line
                .is_some_and(|last_human_line| line_number <= last_human_line)
            {
                return Err(QuickSessionError::StaleInputLine);
            }
            if meta.status == QuickSessionStatus::Error {
                meta.status = title_derived_status(meta);
                meta.error = None;
                meta.last_failed_attempt_id = None;
            }
            meta.last_human_request_id = request_id;
            meta.last_human_line = Some(line_number);
            meta.last_message_preview = truncate_quick_session_preview(&preview);
            finish(meta, now)?;
        }
        QuickSessionTransition::Claim {
            actor,
            input_line,
            attempt_id,
            now,
        } => {
            require_agent(meta, &actor)?;
            validate_quick_session_attempt_id(&attempt_id)?;
            if meta.status == QuickSessionStatus::Running
                && meta.processing_input_line == Some(input_line)
                && meta.attempt_id.as_deref() == Some(&attempt_id)
            {
                return Ok(TransitionOutcome::Duplicate { line_number: None });
            }
            if !matches!(
                meta.status,
                QuickSessionStatus::NeedsTitle | QuickSessionStatus::Active
            ) {
                return Err(QuickSessionError::InvalidState);
            }
            if meta.last_human_line != Some(input_line) {
                return Err(QuickSessionError::InputLineMismatch);
            }
            if input_line == 0
                || meta
                    .last_completed_input_line
                    .is_some_and(|line| input_line <= line)
            {
                return Err(QuickSessionError::StaleInputLine);
            }
            meta.status = QuickSessionStatus::Running;
            meta.processing_input_line = Some(input_line);
            meta.processing_started_at = Some(now.clone());
            meta.attempt_id = Some(attempt_id);
            meta.error = None;
            meta.last_failed_attempt_id = None;
            finish(meta, now)?;
        }
        QuickSessionTransition::SetTitle {
            actor,
            attempt_id,
            title,
            now,
        } => {
            require_agent(meta, &actor)?;
            require_attempt(meta, &attempt_id)?;
            let title = title.trim().to_string();
            validate_quick_session_title(&title)?;
            if meta.title.as_deref() == Some(&title) {
                return Ok(TransitionOutcome::Duplicate { line_number: None });
            }
            meta.title = Some(title);
            finish(meta, now)?;
        }
        QuickSessionTransition::SetSummary {
            actor,
            attempt_id,
            summary,
            now,
        } => {
            require_agent(meta, &actor)?;
            require_attempt(meta, &attempt_id)?;
            let summary = summary.trim().to_string();
            validate_quick_session_summary(&summary)?;
            if meta.summary.as_deref() == Some(&summary) {
                return Ok(TransitionOutcome::Duplicate { line_number: None });
            }
            meta.summary = Some(summary);
            meta.summary_updated_at = Some(now.clone());
            finish(meta, now)?;
        }
        QuickSessionTransition::AgentReply {
            actor,
            input_line,
            attempt_id,
            output_line,
            preview,
            now,
        } => {
            require_agent(meta, &actor)?;
            validate_quick_session_attempt_id(&attempt_id)?;
            if meta.last_completed_attempt_id.as_deref() == Some(&attempt_id) {
                return Ok(TransitionOutcome::Duplicate {
                    line_number: meta.last_completed_line,
                });
            }
            require_attempt(meta, &attempt_id)?;
            if meta.processing_input_line != Some(input_line) {
                return Err(QuickSessionError::InputLineMismatch);
            }
            if meta.title.is_none() {
                return Err(QuickSessionError::TitleRequired);
            }
            if output_line <= input_line {
                return Err(QuickSessionError::InvalidLineNumber);
            }
            meta.status = QuickSessionStatus::Active;
            meta.last_completed_attempt_id = Some(attempt_id);
            meta.last_completed_input_line = Some(input_line);
            meta.last_completed_line = Some(output_line);
            meta.last_message_preview = truncate_quick_session_preview(&preview);
            meta.error = None;
            meta.last_failed_attempt_id = None;
            clear_claim(meta);
            finish(meta, now)?;
        }
        QuickSessionTransition::MarkError {
            actor,
            attempt_id,
            error,
            now,
        } => {
            require_agent(meta, &actor)?;
            validate_quick_session_attempt_id(&attempt_id)?;
            if meta.last_failed_attempt_id.as_deref() == Some(&attempt_id) {
                return Ok(TransitionOutcome::Duplicate { line_number: None });
            }
            require_attempt(meta, &attempt_id)?;
            let error = error.trim().to_string();
            if error.is_empty() {
                return Err(QuickSessionError::EmptyError);
            }
            let claimed_input_line = meta
                .processing_input_line
                .ok_or(QuickSessionError::InvalidState)?;
            let has_queued_input = meta
                .last_human_line
                .is_some_and(|line_number| line_number > claimed_input_line);
            meta.status = if has_queued_input {
                title_derived_status(meta)
            } else {
                QuickSessionStatus::Error
            };
            meta.error = Some(error);
            meta.last_failed_attempt_id = Some(attempt_id);
            clear_claim(meta);
            finish(meta, now)?;
        }
        QuickSessionTransition::Archive { actor, now } => {
            require_creator(meta, &actor)?;
            if meta.status == QuickSessionStatus::Archived {
                return Err(QuickSessionError::InvalidState);
            }
            meta.archived_from = Some(if meta.status == QuickSessionStatus::Running {
                title_derived_status(meta)
            } else {
                meta.status
            });
            meta.status = QuickSessionStatus::Archived;
            meta.archived_at = Some(now.clone());
            clear_claim(meta);
            finish(meta, now)?;
        }
        QuickSessionTransition::Unarchive { actor, now } => {
            require_creator(meta, &actor)?;
            if meta.status != QuickSessionStatus::Archived {
                return Err(QuickSessionError::InvalidState);
            }
            meta.status = meta
                .archived_from
                .unwrap_or_else(|| title_derived_status(meta));
            if meta.status != QuickSessionStatus::Error {
                meta.error = None;
                meta.last_failed_attempt_id = None;
            }
            meta.archived_at = None;
            meta.archived_from = None;
            clear_claim(meta);
            finish(meta, now)?;
        }
    }
    Ok(TransitionOutcome::Applied)
}

fn require_creator(meta: &QuickSessionMeta, actor: &str) -> Result<(), QuickSessionError> {
    if actor == meta.created_by {
        Ok(())
    } else {
        Err(QuickSessionError::UnauthorizedActor)
    }
}

fn require_agent(meta: &QuickSessionMeta, actor: &str) -> Result<(), QuickSessionError> {
    if actor == meta.agent_id {
        Ok(())
    } else {
        Err(QuickSessionError::UnauthorizedActor)
    }
}

fn require_attempt(meta: &QuickSessionMeta, attempt_id: &str) -> Result<(), QuickSessionError> {
    validate_quick_session_attempt_id(attempt_id)?;
    if meta.status != QuickSessionStatus::Running {
        return Err(QuickSessionError::InvalidState);
    }
    if meta.attempt_id.as_deref() == Some(attempt_id) {
        Ok(())
    } else {
        Err(QuickSessionError::StaleAttempt)
    }
}

fn title_derived_status(meta: &QuickSessionMeta) -> QuickSessionStatus {
    if meta.title.is_some() {
        QuickSessionStatus::Active
    } else {
        QuickSessionStatus::NeedsTitle
    }
}

fn clear_claim(meta: &mut QuickSessionMeta) {
    meta.processing_input_line = None;
    meta.processing_started_at = None;
    meta.attempt_id = None;
}

fn finish(meta: &mut QuickSessionMeta, now: String) -> Result<(), QuickSessionError> {
    meta.updated_at = now;
    meta.revision = meta
        .revision
        .checked_add(1)
        .ok_or_else(|| QuickSessionError::InvalidMeta("revision overflow".to_string()))?;
    Ok(())
}
