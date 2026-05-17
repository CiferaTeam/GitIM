use crate::flow::types::{FlowDocument, FlowError, FlowNode, FlowSlug, FlowWarning, NodeType};

const MAX_FILE_SIZE: usize = 256 * 1024;
const MAX_NODE_COUNT: usize = 50;

pub fn validate_flow_document(doc: &FlowDocument, slug_in_path: &str) -> Result<(), FlowError> {
    FlowSlug::new(&doc.meta.slug).map_err(FlowError::InvalidSlug)?;
    FlowSlug::new(slug_in_path).map_err(FlowError::InvalidSlug)?;

    if doc.meta.slug != slug_in_path {
        return Err(FlowError::SlugMismatch {
            frontmatter: doc.meta.slug.clone(),
            path: slug_in_path.to_string(),
        });
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
}
