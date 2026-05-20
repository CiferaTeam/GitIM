// crates/gitim-core/src/timer.rs

//! Oneshot timer: per-agent file-backed pending wake-ups.
//!
//! Design: `docs/plans/oneshot-timer/00-requirements.md`
//! Constraints: `docs/plans/oneshot-timer/01-eng-review-findings.md`

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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

#[cfg(test)]
mod tests {
    // Tests come in subsequent tasks.
}
