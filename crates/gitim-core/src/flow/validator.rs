use crate::flow::types::{FlowDocument, FlowError, FlowNode, FlowSlug, FlowWarning, NodeType};
use crate::types::labels::{validate_labels, FLOW_NODE_MAX_LABELS};

const MAX_FILE_SIZE: usize = 256 * 1024;
const MAX_NODE_COUNT: usize = 50;

/// Validate a node ID: a-z, 0-9, `-`, `_`; length 1-39;
/// no leading/trailing hyphen; no consecutive hyphens.
fn validate_node_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("must not be empty".into());
    }
    if id.len() > 39 {
        return Err(format!("exceeds 39 characters (len={})", id.len()));
    }
    for ch in id.chars() {
        if !matches!(ch, 'a'..='z' | '0'..='9' | '-' | '_') {
            return Err(format!("contains invalid character {:?}", ch));
        }
    }
    if id.starts_with('-') || id.ends_with('-') {
        return Err("must not start or end with hyphen".into());
    }
    if id.contains("--") {
        return Err("must not contain consecutive hyphens".into());
    }
    Ok(())
}

pub fn validate_flow_document(doc: &FlowDocument, slug_in_path: &str) -> Result<(), FlowError> {
    FlowSlug::new(&doc.meta.slug).map_err(FlowError::InvalidSlug)?;
    FlowSlug::new(slug_in_path).map_err(FlowError::InvalidSlug)?;

    if doc.meta.slug != slug_in_path {
        return Err(FlowError::SlugMismatch {
            frontmatter: doc.meta.slug.clone(),
            path: slug_in_path.to_string(),
        });
    }

    for n in &doc.meta.nodes {
        if let Err(reason) = validate_node_id(&n.id) {
            return Err(FlowError::InvalidNodeId {
                id: n.id.clone(),
                reason,
            });
        }
    }

    let mut seen = std::collections::HashSet::new();
    for n in &doc.meta.nodes {
        if !seen.insert(n.id.clone()) {
            return Err(FlowError::DuplicateNodeId(n.id.clone()));
        }
    }

    for n in &doc.meta.nodes {
        for need in &n.needs {
            if !seen.contains(need) {
                return Err(FlowError::UnknownNeed {
                    node: n.id.clone(),
                    missing: need.clone(),
                });
            }
        }
    }

    for n in &doc.meta.nodes {
        match n.node_type {
            NodeType::AgentMention => {
                if n.owner.is_none() {
                    return Err(FlowError::MissingRequiredField(
                        n.id.clone(),
                        n.node_type.clone(),
                        "owner",
                    ));
                }
            }
            NodeType::ChannelThread => {
                if n.participants.is_empty() {
                    return Err(FlowError::MissingRequiredField(
                        n.id.clone(),
                        n.node_type.clone(),
                        "participants",
                    ));
                }
            }
            NodeType::HumanReview => {}
            NodeType::WaitForSignal => {
                if n.signal.is_none() {
                    return Err(FlowError::MissingRequiredField(
                        n.id.clone(),
                        n.node_type.clone(),
                        "signal",
                    ));
                }
            }
        }

        // required_labels 不是 node_type-required,但若存在必须合法
        if let Err(inner) = validate_labels(&n.required_labels, FLOW_NODE_MAX_LABELS) {
            return Err(FlowError::InvalidNodeField {
                node: n.id.clone(),
                field: "required_labels",
                inner: inner.to_string(),
            });
        }
    }

    if has_cycle(&doc.meta.nodes) {
        return Err(FlowError::Cycle);
    }

    Ok(())
}

pub fn validate_flow_for_storage(doc: &FlowDocument, file_size: usize) -> Vec<FlowWarning> {
    let mut w = Vec::new();
    if file_size > MAX_FILE_SIZE {
        w.push(FlowWarning::OversizedFile {
            actual: file_size,
            limit: MAX_FILE_SIZE,
        });
    }
    if doc.meta.nodes.len() > MAX_NODE_COUNT {
        w.push(FlowWarning::TooManyNodes {
            count: doc.meta.nodes.len(),
            limit: MAX_NODE_COUNT,
        });
    }
    for n in &doc.meta.nodes {
        if matches!(n.node_type, NodeType::HumanReview | NodeType::WaitForSignal) {
            w.push(FlowWarning::Phase2NodeType {
                node_id: n.id.clone(),
                node_type: n.node_type.clone(),
            });
        }
    }
    w
}

fn has_cycle(nodes: &[FlowNode]) -> bool {
    use std::collections::HashMap;

    let adj: HashMap<&str, Vec<&str>> = nodes
        .iter()
        .map(|n| (n.id.as_str(), n.needs.iter().map(String::as_str).collect()))
        .collect();

    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        White,
        Gray,
        Black,
    }

    let mut marks: HashMap<&str, Mark> =
        nodes.iter().map(|n| (n.id.as_str(), Mark::White)).collect();

    fn dfs<'a>(
        id: &'a str,
        adj: &HashMap<&'a str, Vec<&'a str>>,
        marks: &mut HashMap<&'a str, Mark>,
    ) -> bool {
        match marks.get(id).copied().unwrap_or(Mark::Black) {
            Mark::Gray => return true,
            Mark::Black => return false,
            Mark::White => {}
        }
        marks.insert(id, Mark::Gray);
        if let Some(needs) = adj.get(id) {
            for n in needs {
                if dfs(n, adj, marks) {
                    return true;
                }
            }
        }
        marks.insert(id, Mark::Black);
        false
    }

    for n in nodes {
        if dfs(n.id.as_str(), &adj, &mut marks) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::types::{FlowMeta, FlowNode, NodeType};

    fn doc_with_nodes(nodes: Vec<FlowNode>) -> FlowDocument {
        FlowDocument {
            meta: FlowMeta {
                schema_version: 1,
                slug: "test".into(),
                name: "Test".into(),
                description: String::new(),
                created_by: "lewis".into(),
                created_at: "2026-05-12T10:00:00Z".into(),
                updated_at: None,
                nodes,
            },
        }
    }

    fn node(id: &str, needs: &[&str]) -> FlowNode {
        FlowNode {
            id: id.into(),
            node_type: NodeType::AgentMention,
            owner: Some("alice".into()),
            participants: vec![],
            signal: None,
            needs: needs.iter().map(|s| s.to_string()).collect(),
            exits: vec![],
            required_labels: vec![],
            prompt: String::new(),
        }
    }

    #[test]
    fn test_validate_happy_path() {
        let d = doc_with_nodes(vec![node("a", &[]), node("b", &["a"])]);
        assert!(validate_flow_document(&d, "test").is_ok());
    }

    #[test]
    fn test_slug_mismatch() {
        let d = doc_with_nodes(vec![node("a", &[])]);
        let err = validate_flow_document(&d, "different-slug").unwrap_err();
        assert!(matches!(err, FlowError::SlugMismatch { .. }));
    }

    #[test]
    fn test_duplicate_node_id() {
        let d = doc_with_nodes(vec![node("a", &[]), node("a", &[])]);
        let err = validate_flow_document(&d, "test").unwrap_err();
        assert!(matches!(err, FlowError::DuplicateNodeId(id) if id == "a"));
    }

    #[test]
    fn test_unknown_need() {
        let d = doc_with_nodes(vec![node("a", &["ghost"])]);
        let err = validate_flow_document(&d, "test").unwrap_err();
        assert!(matches!(err, FlowError::UnknownNeed { .. }));
    }

    #[test]
    fn test_cycle_detection() {
        let d = doc_with_nodes(vec![node("a", &["b"]), node("b", &["a"])]);
        let err = validate_flow_document(&d, "test").unwrap_err();
        assert!(matches!(err, FlowError::Cycle));
    }

    #[test]
    fn test_self_cycle() {
        let d = doc_with_nodes(vec![node("a", &["a"])]);
        let err = validate_flow_document(&d, "test").unwrap_err();
        assert!(matches!(err, FlowError::Cycle));
    }

    #[test]
    fn test_agent_mention_missing_owner() {
        let mut bad = node("a", &[]);
        bad.owner = None;
        let d = doc_with_nodes(vec![bad]);
        let err = validate_flow_document(&d, "test").unwrap_err();
        assert!(matches!(
            err,
            FlowError::MissingRequiredField(_, _, "owner")
        ));
    }

    #[test]
    fn test_channel_thread_missing_participants() {
        let mut n = node("a", &[]);
        n.node_type = NodeType::ChannelThread;
        n.owner = None;
        n.participants = vec![];
        let d = doc_with_nodes(vec![n]);
        let err = validate_flow_document(&d, "test").unwrap_err();
        assert!(matches!(
            err,
            FlowError::MissingRequiredField(_, _, "participants")
        ));
    }

    #[test]
    fn test_oversized_warning() {
        let d = doc_with_nodes(vec![node("a", &[])]);
        let warnings = validate_flow_for_storage(&d, 300_000);
        assert!(warnings
            .iter()
            .any(|w| matches!(w, FlowWarning::OversizedFile { .. })));
    }

    #[test]
    fn test_too_many_nodes_warning() {
        let nodes = (0..51).map(|i| node(&format!("n{i}"), &[])).collect();
        let d = doc_with_nodes(nodes);
        let warnings = validate_flow_for_storage(&d, 1000);
        assert!(warnings
            .iter()
            .any(|w| matches!(w, FlowWarning::TooManyNodes { .. })));
    }

    #[test]
    fn test_node_id_invalid_chars() {
        // space in node id → InvalidNodeId
        let d = doc_with_nodes(vec![node("bad name", &[])]);
        let err = validate_flow_document(&d, "test").unwrap_err();
        assert!(
            matches!(&err, FlowError::InvalidNodeId { id, .. } if id == "bad name"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_node_id_uppercase_rejected() {
        let d = doc_with_nodes(vec![node("UPPER", &[])]);
        let err = validate_flow_document(&d, "test").unwrap_err();
        assert!(
            matches!(&err, FlowError::InvalidNodeId { id, .. } if id == "UPPER"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_node_id_empty_rejected() {
        let d = doc_with_nodes(vec![node("", &[])]);
        let err = validate_flow_document(&d, "test").unwrap_err();
        assert!(
            matches!(&err, FlowError::InvalidNodeId { id, .. } if id.is_empty()),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_node_id_too_long() {
        let long_id = "a".repeat(40);
        let d = doc_with_nodes(vec![node(&long_id, &[])]);
        let err = validate_flow_document(&d, "test").unwrap_err();
        assert!(
            matches!(&err, FlowError::InvalidNodeId { .. }),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_required_labels_invalid_char_rejected() {
        let mut n = node("n1", &[]);
        n.required_labels = vec!["Rust!".into()];
        let d = doc_with_nodes(vec![n]);
        let err = validate_flow_document(&d, "test").unwrap_err();
        match err {
            FlowError::InvalidNodeField { node, field, .. } => {
                assert_eq!(node, "n1");
                assert_eq!(field, "required_labels");
            }
            e => panic!("unexpected error: {e:?}"),
        }
    }

    #[test]
    fn test_required_labels_too_many_rejected() {
        let mut n = node("n1", &[]);
        n.required_labels = (0..11).map(|i| format!("l{i}")).collect();
        let d = doc_with_nodes(vec![n]);
        let err = validate_flow_document(&d, "test").unwrap_err();
        assert!(matches!(
            err,
            FlowError::InvalidNodeField {
                field: "required_labels",
                ..
            }
        ));
    }

    #[test]
    fn test_required_labels_valid_accepted() {
        let mut n = node("n1", &[]);
        n.required_labels = vec!["rust".into(), "backend".into()];
        let d = doc_with_nodes(vec![n]);
        assert!(validate_flow_document(&d, "test").is_ok());
    }

    #[test]
    fn test_human_review_node_produces_phase2_warning() {
        let mut n = node("approval", &[]);
        n.node_type = NodeType::HumanReview;
        n.owner = None;
        let d = doc_with_nodes(vec![n]);
        // validate_flow_document accepts it (no error)
        assert!(validate_flow_document(&d, "test").is_ok());
        // validate_flow_for_storage emits a Phase2NodeType warning
        let warnings = validate_flow_for_storage(&d, 100);
        assert!(
            warnings.iter().any(|w| matches!(
                w,
                FlowWarning::Phase2NodeType { node_id, node_type }
                if node_id == "approval" && *node_type == NodeType::HumanReview
            )),
            "expected Phase2NodeType warning, got: {warnings:?}"
        );
    }

    #[test]
    fn test_wait_for_signal_node_produces_phase2_warning() {
        let mut n = node("gate", &[]);
        n.node_type = NodeType::WaitForSignal;
        n.owner = None;
        n.signal = Some("deploy-approved".into());
        let d = doc_with_nodes(vec![n]);
        assert!(validate_flow_document(&d, "test").is_ok());
        let warnings = validate_flow_for_storage(&d, 100);
        assert!(
            warnings.iter().any(|w| matches!(
                w,
                FlowWarning::Phase2NodeType { node_id, node_type }
                if node_id == "gate" && *node_type == NodeType::WaitForSignal
            )),
            "expected Phase2NodeType warning, got: {warnings:?}"
        );
    }

    #[test]
    fn test_v1_node_types_produce_no_phase2_warning() {
        let d = doc_with_nodes(vec![node("a", &[])]);
        let warnings = validate_flow_for_storage(&d, 100);
        assert!(
            !warnings
                .iter()
                .any(|w| matches!(w, FlowWarning::Phase2NodeType { .. })),
            "unexpected Phase2NodeType warning for v1 node: {warnings:?}"
        );
    }
}
