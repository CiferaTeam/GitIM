use serde::{Deserialize, Serialize};
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

/// 节点类型。v1 落地 agent_mention + channel_thread;human_review / wait_for_signal 留位。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeType {
    AgentMention,
    ChannelThread,
    HumanReview,
    WaitForSignal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowNode {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: NodeType,

    /// agent_mention 必填:派给谁
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,

    /// channel_thread 必填:参与者
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub participants: Vec<String>,

    /// wait_for_signal 必填:信号名
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,

    /// 上游依赖。空数组 = 入口节点。
    #[serde(default)]
    pub needs: Vec<String>,

    /// v2 conditional 留位:节点可能的退出 label。v1 解析但不读。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exits: Vec<String>,

    /// 节点能力需求(unified labels space)。仅信息位 —— daemon 不强制 routing,
    /// 不计算"谁满足"也不阻塞 flow 推进。Coordinator agent 自行用
    /// `agents_with_labels` IPC 查候选,决定拉谁。详见
    /// `docs/plans/unified-labels/00-requirements.md` (P6)。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_labels: Vec<String>,

    /// 节点 prompt body(由 body section parser 注入,frontmatter 里不读)。
    #[serde(skip)]
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowMeta {
    pub schema_version: u32,
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub created_by: String,
    pub created_at: String,
    #[serde(default)]
    pub updated_at: Option<String>,
    pub nodes: Vec<FlowNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowDocument {
    pub meta: FlowMeta,
}

#[derive(Error, Debug)]
pub enum FlowError {
    #[error("invalid slug: {0}")]
    InvalidSlug(#[from] FlowSlugError),
    #[error("missing frontmatter delimiter")]
    MissingFrontmatter,
    #[error("frontmatter yaml: {0}")]
    YamlParse(String),
    #[error("schema mismatch: expected schema_version 1, got {0}")]
    SchemaVersion(u32),
    #[error("slug in frontmatter ({frontmatter}) != path slug ({path})")]
    SlugMismatch { frontmatter: String, path: String },
    #[error("invalid node id `{id}`: {reason}")]
    InvalidNodeId { id: String, reason: String },
    #[error("duplicate node id: {0}")]
    DuplicateNodeId(String),
    #[error("node {node} references unknown id in needs: {missing}")]
    UnknownNeed { node: String, missing: String },
    #[error("cycle detected in flow DAG")]
    Cycle,
    #[error("node {0} type {1:?} missing required field: {2}")]
    MissingRequiredField(String, NodeType, &'static str),
    #[error("node {node} field {field}: {inner}")]
    InvalidNodeField {
        node: String,
        field: &'static str,
        inner: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowWarning {
    /// frontmatter 有 id 但 body 缺 `## id` section
    BodySectionMissing(String),
    /// body 有 `## id` 但 frontmatter 没声明
    OrphanBodySection(String),
    /// 文件 size 超过 256KB
    OversizedFile { actual: usize, limit: usize },
    /// 节点数超过 50
    TooManyNodes { count: usize, limit: usize },
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

    #[test]
    fn test_node_type_serialize_snake_case() {
        let json = serde_json::to_string(&NodeType::AgentMention).unwrap();
        assert_eq!(json, "\"agent_mention\"");
        let json2 = serde_json::to_string(&NodeType::ChannelThread).unwrap();
        assert_eq!(json2, "\"channel_thread\"");
    }

    #[test]
    fn test_flow_node_default_fields_omitted() {
        let node = FlowNode {
            id: "n1".into(),
            node_type: NodeType::AgentMention,
            owner: Some("alice".into()),
            participants: vec![],
            signal: None,
            needs: vec![],
            exits: vec![],
            required_labels: vec![],
            prompt: String::new(),
        };
        let yaml = serde_yaml::to_string(&node).unwrap();
        assert!(yaml.contains("id: n1"), "yaml={yaml}");
        assert!(yaml.contains("owner: alice"), "yaml={yaml}");
        assert!(!yaml.contains("participants"), "yaml={yaml}");
        assert!(!yaml.contains("signal"), "yaml={yaml}");
        assert!(!yaml.contains("exits"), "yaml={yaml}");
        assert!(!yaml.contains("required_labels"), "yaml={yaml}");
    }

    #[test]
    fn old_flow_yaml_without_required_labels_defaults_to_empty() {
        let yaml = "id: n1\ntype: agent_mention\nowner: alice\n";
        let node: FlowNode = serde_yaml::from_str(yaml).unwrap();
        assert!(node.required_labels.is_empty());
    }

    #[test]
    fn flow_node_required_labels_roundtrip() {
        let node = FlowNode {
            id: "n1".into(),
            node_type: NodeType::AgentMention,
            owner: Some("alice".into()),
            participants: vec![],
            signal: None,
            needs: vec![],
            exits: vec![],
            required_labels: vec!["rust".into(), "backend".into()],
            prompt: String::new(),
        };
        let yaml = serde_yaml::to_string(&node).unwrap();
        assert!(yaml.contains("required_labels:"));
        assert!(yaml.contains("- rust"));
        let parsed: FlowNode = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.required_labels, vec!["rust", "backend"]);
    }
}
