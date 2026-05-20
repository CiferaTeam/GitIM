// crates/gitim-core/src/timer.rs

//! Oneshot timer: per-agent file-backed pending wake-ups.
//!
//! Design: `docs/plans/oneshot-timer/00-requirements.md`
//! Constraints: `docs/plans/oneshot-timer/01-eng-review-findings.md`

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MIN_DURATION_SECS: i64 = 10;
pub const MAX_DURATION_SECS: i64 = 24 * 60 * 60;
pub const MAX_PENDING_PER_AGENT: usize = 3;
pub const TIMERS_FILENAME: &str = "timers.json";
pub const LOCK_FILENAME: &str = "timers.json.lock";
pub const GITIM_DIR: &str = ".gitim";

#[derive(Error, Debug)]
pub enum TimerError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid duration: {0}")]
    InvalidDuration(String),
    #[error("anchor cannot be empty")]
    EmptyAnchor,
    #[error("cap reached ({MAX_PENDING_PER_AGENT} pending timers); cancel one first")]
    CapReached,
    #[error("no timer matches \"{0}\"")]
    NoMatch(String),
    #[error("prefix \"{prefix}\" matches {count} timers: {ids}")]
    AmbiguousPrefix {
        prefix: String,
        count: usize,
        ids: String,
    },
    #[error("not in a gitim agent clone (no .gitim/ directory)")]
    NotInClone,
    #[error("random: {0}")]
    Random(#[from] getrandom::Error),
    #[error("timers.json corrupted: {0}")]
    Corrupted(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timer {
    pub id: String,
    pub fire_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub anchor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimersFile {
    pub version: u32,
    #[serde(default)]
    pub timers: Vec<Timer>,
}

impl TimersFile {
    pub fn empty() -> Self {
        Self {
            version: 1,
            timers: Vec::new(),
        }
    }
}

pub fn parse_duration(s: &str) -> Result<ChronoDuration, TimerError> {
    // Reject whitespace-bearing strings ("30 minutes" etc) — humantime accepts
    // them but we want compact CLI-friendly forms only.
    if s.chars().any(char::is_whitespace) {
        return Err(TimerError::InvalidDuration(format!(
            "{s:?}: whitespace not allowed; use compact form like 30m or 1h30m"
        )));
    }
    let std_dur = humantime::parse_duration(s)
        .map_err(|e| TimerError::InvalidDuration(format!("{s:?}: {e}")))?;
    let secs = std_dur.as_secs() as i64;
    if !(MIN_DURATION_SECS..=MAX_DURATION_SECS).contains(&secs) {
        return Err(TimerError::InvalidDuration(format!(
            "{s:?}: must be {MIN_DURATION_SECS}s..{MAX_DURATION_SECS}s ({}s given)",
            secs
        )));
    }
    Ok(ChronoDuration::seconds(secs))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn timers_file_serde_round_trip() {
        let original = TimersFile {
            version: 1,
            timers: vec![Timer {
                id: "20260520T143055-a3f4c2".into(),
                fire_at: "2026-05-20T15:00:55Z"
                    .parse()
                    .unwrap_or_else(|_| Utc::now()),
                created_at: "2026-05-20T14:30:55Z"
                    .parse()
                    .unwrap_or_else(|_| Utc::now()),
                anchor: "<#product:L000042>".into(),
                note: Some("check deploy".into()),
            }],
        };
        let json = serde_json::to_string(&original).unwrap_or_default();
        let parsed: TimersFile = serde_json::from_str(&json).unwrap_or(TimersFile::empty());
        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.timers.len(), 1);
        assert_eq!(parsed.timers[0].id, "20260520T143055-a3f4c2");
        assert_eq!(parsed.timers[0].anchor, "<#product:L000042>");
        assert_eq!(parsed.timers[0].note.as_deref(), Some("check deploy"));
    }

    #[test]
    fn timers_file_empty_default() {
        let f = TimersFile::empty();
        assert_eq!(f.version, 1);
        assert!(f.timers.is_empty());
    }

    #[test]
    fn timers_file_missing_note_parses() {
        let json = r#"{"version":1,"timers":[{"id":"20260520T143055-a3f4c2","fire_at":"2026-05-20T15:00:55Z","created_at":"2026-05-20T14:30:55Z","anchor":"<#x>"}]}"#;
        let f: TimersFile = serde_json::from_str(json).unwrap();
        assert!(f.timers[0].note.is_none());
    }

    #[test]
    fn parse_duration_humantime_ok() {
        assert_eq!(parse_duration("30s").unwrap().num_seconds(), 30);
        assert_eq!(parse_duration("5m").unwrap().num_seconds(), 300);
        assert_eq!(parse_duration("2h").unwrap().num_seconds(), 7200);
        assert_eq!(parse_duration("1h30m").unwrap().num_seconds(), 5400);
    }

    #[test]
    fn parse_duration_min_bound() {
        let err = parse_duration("5s").unwrap_err();
        assert!(matches!(err, TimerError::InvalidDuration(_)));
    }

    #[test]
    fn parse_duration_max_bound() {
        let err = parse_duration("25h").unwrap_err();
        assert!(matches!(err, TimerError::InvalidDuration(_)));
    }

    #[test]
    fn parse_duration_garbage() {
        assert!(parse_duration("30 minutes").is_err());
        assert!(parse_duration("").is_err());
        assert!(parse_duration("abc").is_err());
    }

    #[test]
    fn parse_duration_at_bounds_ok() {
        assert!(parse_duration("10s").is_ok());
        assert!(parse_duration("24h").is_ok());
    }
}
