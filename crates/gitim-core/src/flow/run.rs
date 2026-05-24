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
        crate::preconditions::random_bytes(&mut hash_bytes);
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

    pub fn as_str(self) -> &'static str {
        match self {
            RunStatus::InProgress => "in_progress",
            RunStatus::Done => "done",
            RunStatus::Failed => "failed",
            RunStatus::Cancelled => "cancelled",
        }
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

    pub fn as_str(self) -> &'static str {
        match self {
            NodeStatus::Pending => "pending",
            NodeStatus::InProgress => "in_progress",
            NodeStatus::Done => "done",
            NodeStatus::Failed => "failed",
            NodeStatus::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowRunNode {
    pub id: String,
    pub status: NodeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowRun {
    pub schema_version: u32,
    pub run_id: String,
    pub flow_slug: String,
    pub channel: String,
    pub started_at: String,
    pub started_by: String,
    pub status: RunStatus,
    pub nodes: Vec<FlowRunNode>,
    pub updated_at: String,
}

#[derive(Error, Debug)]
pub enum FlowRunError {
    #[error("invalid run id: {0}")]
    InvalidRunId(#[from] RunIdError),
    #[error("yaml parse: {0}")]
    YamlParse(String),
    #[error("schema mismatch: expected schema_version 1, got {0}")]
    SchemaVersion(u32),
    #[error("unknown node id `{0}`")]
    UnknownNodeId(String),
    #[error("invalid status transition: {from:?} → {to:?}")]
    InvalidTransition { from: NodeStatus, to: NodeStatus },
    #[error("run is terminal ({status:?}); refuse to mutate")]
    RunTerminal { status: RunStatus },
}

pub fn run_path(slug: &str, run_id: &RunId) -> std::path::PathBuf {
    std::path::PathBuf::from("flows")
        .join(slug)
        .join("runs")
        .join(run_id.as_str())
        .join("state.yaml")
}

pub fn parse_run_state(content: &str) -> Result<FlowRun, FlowRunError> {
    let run: FlowRun =
        serde_yaml::from_str(content).map_err(|e| FlowRunError::YamlParse(e.to_string()))?;
    if run.schema_version != 1 {
        return Err(FlowRunError::SchemaVersion(run.schema_version));
    }
    Ok(run)
}

pub fn stringify_run_state(run: &FlowRun) -> Result<String, FlowRunError> {
    serde_yaml::to_string(run).map_err(|e| FlowRunError::YamlParse(e.to_string()))
}

/// 5-state machine: pending → in_progress → done|failed|skipped.
/// pending → done|failed|skipped 直接跳也允许(adjacent skip)。
/// Once terminal, no further changes.
pub fn validate_node_transition(from: NodeStatus, to: NodeStatus) -> Result<(), FlowRunError> {
    if from == to {
        return Ok(()); // no-op allowed
    }
    use NodeStatus::*;
    let allowed = match from {
        Pending => matches!(to, InProgress | Done | Failed | Skipped),
        InProgress => matches!(to, Done | Failed | Skipped),
        Done | Failed | Skipped => false, // terminal
    };
    if allowed {
        Ok(())
    } else {
        Err(FlowRunError::InvalidTransition { from, to })
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

    #[test]
    fn test_run_path() {
        let id = RunId::new("20260517T103045-a1b2c3").unwrap();
        assert_eq!(
            run_path("release", &id),
            std::path::PathBuf::from("flows/release/runs/20260517T103045-a1b2c3/state.yaml")
        );
    }

    #[test]
    fn test_parse_round_trip() {
        let yaml = r#"schema_version: 1
run_id: 20260517T103045-a1b2c3
flow_slug: release
channel: release-discuss
started_at: 2026-05-17T10:30:45Z
started_by: lewis
status: in_progress
nodes:
  - id: changelog
    status: done
    actor: alice
    started_at: 2026-05-17T10:31:00Z
    completed_at: 2026-05-17T11:15:00Z
  - id: e2e
    status: pending
updated_at: 2026-05-17T11:15:00Z
"#;
        let run = parse_run_state(yaml).unwrap();
        assert_eq!(run.run_id, "20260517T103045-a1b2c3");
        assert_eq!(run.nodes.len(), 2);
        assert_eq!(run.nodes[0].status, NodeStatus::Done);
        assert_eq!(run.nodes[0].actor.as_deref(), Some("alice"));
        assert_eq!(run.nodes[1].status, NodeStatus::Pending);
        let back = stringify_run_state(&run).unwrap();
        let again = parse_run_state(&back).unwrap();
        assert_eq!(again, run);
    }

    #[test]
    fn test_parse_schema_version_mismatch() {
        let yaml = "schema_version: 2\nrun_id: 20260517T103045-a1b2c3\nflow_slug: r\nchannel: c\nstarted_at: x\nstarted_by: l\nstatus: in_progress\nnodes: []\nupdated_at: x\n";
        let err = parse_run_state(yaml).unwrap_err();
        assert!(matches!(err, FlowRunError::SchemaVersion(2)));
    }

    #[test]
    fn test_validate_transition_forward() {
        use NodeStatus::*;
        assert!(validate_node_transition(Pending, InProgress).is_ok());
        assert!(validate_node_transition(Pending, Done).is_ok());
        assert!(validate_node_transition(Pending, Skipped).is_ok());
        assert!(validate_node_transition(InProgress, Done).is_ok());
        assert!(validate_node_transition(InProgress, Failed).is_ok());
        assert!(validate_node_transition(InProgress, Skipped).is_ok());
        // no-op
        assert!(validate_node_transition(Done, Done).is_ok());
    }

    #[test]
    fn test_validate_transition_backward_rejected() {
        use NodeStatus::*;
        for (f, t) in &[
            (InProgress, Pending),
            (Done, Pending),
            (Done, InProgress),
            (Done, Failed),
            (Failed, Done),
            (Skipped, Done),
        ] {
            let err = validate_node_transition(*f, *t).unwrap_err();
            assert!(matches!(err, FlowRunError::InvalidTransition { .. }));
        }
    }

    #[test]
    fn test_status_as_str_matches_serde() {
        // Critical: event wire format relies on as_str() == serde representation
        for s in [
            NodeStatus::Pending,
            NodeStatus::InProgress,
            NodeStatus::Done,
            NodeStatus::Failed,
            NodeStatus::Skipped,
        ] {
            let serde_str = serde_json::to_value(s)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string();
            assert_eq!(s.as_str(), serde_str, "NodeStatus::{:?} as_str mismatch", s);
        }
        for s in [
            RunStatus::InProgress,
            RunStatus::Done,
            RunStatus::Failed,
            RunStatus::Cancelled,
        ] {
            let serde_str = serde_json::to_value(s)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string();
            assert_eq!(s.as_str(), serde_str, "RunStatus::{:?} as_str mismatch", s);
        }
    }

    #[test]
    fn test_skip_optional_fields_serialize() {
        let node = FlowRunNode {
            id: "n".into(),
            status: NodeStatus::Pending,
            actor: None,
            started_at: None,
            completed_at: None,
            result_ref: None,
        };
        let yaml = serde_yaml::to_string(&node).unwrap();
        assert!(yaml.contains("id: n"), "yaml={yaml}");
        assert!(yaml.contains("status: pending"), "yaml={yaml}");
        assert!(!yaml.contains("actor"), "yaml={yaml}");
        assert!(!yaml.contains("started_at"), "yaml={yaml}");
        assert!(!yaml.contains("completed_at"), "yaml={yaml}");
        assert!(!yaml.contains("result_ref"), "yaml={yaml}");
    }
}
