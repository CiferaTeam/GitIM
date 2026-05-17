use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RunIdError {
    #[error("run id is empty")]
    Empty,
    #[error("run id does not match YYYYMMDDTHHMMSS-XXXXXX pattern")]
    Format,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RunId(String);

impl RunId {
    pub fn new(s: &str) -> Result<Self, RunIdError> {
        if s.is_empty() {
            return Err(RunIdError::Empty);
        }
        if !is_valid_run_id(s) {
            return Err(RunIdError::Format);
        }
        Ok(Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 生成新的 run id:`YYYYMMDDTHHMMSS-XXXXXX`(XXXXXX = 6 lowercase hex chars)
    pub fn generate() -> Self {
        let now = chrono::Utc::now();
        let timestamp = now.format("%Y%m%dT%H%M%S");
        let mut hash_bytes = [0u8; 3];
        getrandom::getrandom(&mut hash_bytes).expect("getrandom failed");
        let hash = hex::encode(hash_bytes);
        Self(format!("{}-{}", timestamp, hash))
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

fn is_valid_run_id(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 22 {
        return false;
    }
    for (i, &b) in bytes.iter().enumerate() {
        let ok = match i {
            0..=7 => b.is_ascii_digit(),
            8 => b == b'T',
            9..=14 => b.is_ascii_digit(),
            15 => b == b'-',
            16..=21 => b.is_ascii_hexdigit() && (b.is_ascii_digit() || b.is_ascii_lowercase()),
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    InProgress,
    Done,
    Failed,
    Cancelled,
}

impl RunStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            RunStatus::Done | RunStatus::Failed | RunStatus::Cancelled
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    Pending,
    InProgress,
    Done,
    Failed,
    Skipped,
}

impl NodeStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            NodeStatus::Done | NodeStatus::Failed | NodeStatus::Skipped
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_id_generate_is_valid() {
        let id = RunId::generate();
        let parsed = RunId::new(id.as_str()).unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn test_run_id_pattern_accepts_valid() {
        for s in &[
            "20260517T103045-a1b2c3",
            "00000000T000000-000000",
            "99991231T235959-ffffff",
        ] {
            assert!(RunId::new(s).is_ok(), "expected {} ok", s);
        }
    }

    #[test]
    fn test_run_id_pattern_rejects_invalid() {
        for s in &[
            "",
            "not-a-run-id",
            "20260517T103045a1b2c3",   // missing dash
            "20260517T103045-A1B2C3",  // uppercase hex
            "20260517T103045-a1b2c",   // too short hash
            "20260517T103045-a1b2c3d", // too long hash
            "20260517 103045-a1b2c3",  // space instead of T
        ] {
            assert!(RunId::new(s).is_err(), "expected {} err", s);
        }
    }

    #[test]
    fn test_run_status_terminal() {
        assert!(!RunStatus::InProgress.is_terminal());
        assert!(RunStatus::Done.is_terminal());
        assert!(RunStatus::Failed.is_terminal());
        assert!(RunStatus::Cancelled.is_terminal());
    }

    #[test]
    fn test_node_status_terminal() {
        assert!(!NodeStatus::Pending.is_terminal());
        assert!(!NodeStatus::InProgress.is_terminal());
        assert!(NodeStatus::Done.is_terminal());
        assert!(NodeStatus::Failed.is_terminal());
        assert!(NodeStatus::Skipped.is_terminal());
    }

    #[test]
    fn test_serde_snake_case() {
        let json = serde_json::to_string(&RunStatus::InProgress).unwrap();
        assert_eq!(json, "\"in_progress\"");
        let json = serde_json::to_string(&NodeStatus::Pending).unwrap();
        assert_eq!(json, "\"pending\"");
    }
}
