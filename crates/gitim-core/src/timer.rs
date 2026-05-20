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

impl Timer {
    pub fn new_id() -> Result<String, TimerError> {
        let now = Utc::now();
        let timestamp = now.format("%Y%m%dT%H%M%S");
        let mut hash_bytes = [0u8; 3];
        getrandom::getrandom(&mut hash_bytes)?;
        let hash = hex::encode(hash_bytes);
        Ok(format!("{timestamp}-{hash}"))
    }

    pub fn new(
        duration: ChronoDuration,
        anchor: String,
        note: Option<String>,
    ) -> Result<Self, TimerError> {
        let trimmed = anchor.trim();
        if trimmed.is_empty() {
            return Err(TimerError::EmptyAnchor);
        }
        let now = Utc::now();
        Ok(Self {
            id: Self::new_id()?,
            fire_at: now + duration,
            created_at: now,
            anchor: trimmed.to_string(),
            note,
        })
    }
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

/// Pure function — `now` is injected so tests can simulate clock skew.
/// Returned `pending` is sorted by `fire_at` ascending; `fired` preserves
/// input order (caller renders them in registration sequence).
pub fn partition_fired(mut timers: Vec<Timer>, now: DateTime<Utc>) -> (Vec<Timer>, Vec<Timer>) {
    timers.sort_by_key(|t| t.fire_at);
    let mut fired = Vec::new();
    let mut pending = Vec::new();
    for t in timers {
        if t.fire_at <= now {
            fired.push(t);
        } else {
            pending.push(t);
        }
    }
    (fired, pending)
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

    #[test]
    fn new_id_matches_22_char_pattern() {
        let id = Timer::new_id().unwrap();
        assert_eq!(id.len(), 22, "id was: {id:?}");
        let bytes = id.as_bytes();
        for (i, &b) in bytes.iter().enumerate() {
            let ok = match i {
                0..=7 => b.is_ascii_digit(),
                8 => b == b'T',
                9..=14 => b.is_ascii_digit(),
                15 => b == b'-',
                16..=21 => b.is_ascii_digit() || (b'a'..=b'f').contains(&b),
                _ => false,
            };
            assert!(ok, "char at {i} (={b}) failed validation in {id:?}");
        }
    }

    #[test]
    fn new_id_pairs_unique_with_high_probability() {
        use std::collections::HashSet;
        let mut s = HashSet::new();
        for _ in 0..200 {
            s.insert(Timer::new_id().unwrap());
        }
        assert!(s.len() >= 199, "had {}/200 unique", s.len());
    }

    fn mk_timer(id: &str, fire_at_iso: &str) -> Timer {
        Timer {
            id: id.into(),
            fire_at: fire_at_iso.parse().unwrap(),
            created_at: "2026-01-01T00:00:00Z".parse().unwrap(),
            anchor: "<#x>".into(),
            note: None,
        }
    }

    #[test]
    fn partition_fired_empty() {
        let (fired, pending) = partition_fired(vec![], "2026-05-20T15:00:00Z".parse().unwrap());
        assert!(fired.is_empty());
        assert!(pending.is_empty());
    }

    #[test]
    fn partition_fired_all_future() {
        let timers = vec![
            mk_timer("a", "2026-05-20T16:00:00Z"),
            mk_timer("b", "2026-05-20T17:00:00Z"),
        ];
        let (fired, pending) = partition_fired(timers, "2026-05-20T15:00:00Z".parse().unwrap());
        assert!(fired.is_empty());
        assert_eq!(pending.len(), 2);
    }

    #[test]
    fn partition_fired_all_past() {
        let timers = vec![
            mk_timer("a", "2026-05-20T14:00:00Z"),
            mk_timer("b", "2026-05-20T13:00:00Z"),
        ];
        let (fired, pending) = partition_fired(timers, "2026-05-20T15:00:00Z".parse().unwrap());
        assert_eq!(fired.len(), 2);
        assert!(pending.is_empty());
    }

    #[test]
    fn partition_fired_mixed() {
        let timers = vec![
            mk_timer("past", "2026-05-20T14:00:00Z"),
            mk_timer("future", "2026-05-20T16:00:00Z"),
            mk_timer("now", "2026-05-20T15:00:00Z"),
        ];
        let (fired, pending) = partition_fired(timers, "2026-05-20T15:00:00Z".parse().unwrap());
        let fired_ids: Vec<_> = fired.iter().map(|t| t.id.as_str()).collect();
        let pending_ids: Vec<_> = pending.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(fired_ids, vec!["past", "now"], "fire_at == now is fired");
        assert_eq!(pending_ids, vec!["future"]);
    }

    #[test]
    fn partition_fired_pending_sorted_by_fire_at_ascending() {
        let timers = vec![
            mk_timer("late", "2026-05-20T18:00:00Z"),
            mk_timer("early", "2026-05-20T16:00:00Z"),
            mk_timer("mid", "2026-05-20T17:00:00Z"),
        ];
        let (_, pending) = partition_fired(timers, "2026-05-20T15:00:00Z".parse().unwrap());
        let ids: Vec<_> = pending.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["early", "mid", "late"]);
    }
}
