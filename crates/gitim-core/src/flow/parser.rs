use crate::flow::types::{FlowDocument, FlowError, FlowMeta, FlowWarning};

const FRONTMATTER_DELIM: &str = "---";

pub fn parse_flow_markdown(content: &str) -> Result<FlowDocument, FlowError> {
    let (doc, _warnings) = parse_flow_markdown_with_warnings(content)?;
    Ok(doc)
}

pub fn parse_flow_markdown_with_warnings(
    content: &str,
) -> Result<(FlowDocument, Vec<FlowWarning>), FlowError> {
    let mut warnings = Vec::new();

    // Strip BOM if present
    let trimmed = content.trim_start_matches('\u{FEFF}');

    if !trimmed.starts_with(FRONTMATTER_DELIM) {
        return Err(FlowError::MissingFrontmatter);
    }

    let after_open = &trimmed[FRONTMATTER_DELIM.len()..];
    // The closing --- must appear on its own line
    let end = after_open
        .find(&format!("\n{}", FRONTMATTER_DELIM))
        .ok_or(FlowError::MissingFrontmatter)?;
    let yaml_body = &after_open[..end];
    // Skip "\n---" (4 chars) to get content after closing delimiter
    let body_after = &after_open[end + 1 + FRONTMATTER_DELIM.len()..];

    let mut meta: FlowMeta =
        serde_yaml::from_str(yaml_body.trim()).map_err(|e| FlowError::YamlParse(e.to_string()))?;

    if meta.schema_version != 1 {
        return Err(FlowError::SchemaVersion(meta.schema_version));
    }

    let section_map = split_body_sections(body_after);

    // Collect owned IDs before mutating nodes to satisfy borrow checker
    let frontmatter_ids: std::collections::HashSet<String> =
        meta.nodes.iter().map(|n| n.id.clone()).collect();

    for node in meta.nodes.iter_mut() {
        match section_map.get(node.id.as_str()) {
            Some(text) => node.prompt = text.clone(),
            None => warnings.push(FlowWarning::BodySectionMissing(node.id.clone())),
        }
    }

    for section_id in section_map.keys() {
        if !frontmatter_ids.contains(section_id.as_str()) {
            warnings.push(FlowWarning::OrphanBodySection(section_id.clone()));
        }
    }

    Ok((FlowDocument { meta }, warnings))
}

fn split_body_sections(body: &str) -> std::collections::BTreeMap<String, String> {
    let mut sections = std::collections::BTreeMap::new();
    let mut current_id: Option<String> = None;
    let mut buf = String::new();

    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            if let Some(id) = current_id.take() {
                sections.insert(id, buf.trim().to_string());
                buf.clear();
            }
            current_id = Some(rest.trim().to_string());
        } else if current_id.is_some() {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    if let Some(id) = current_id.take() {
        sections.insert(id, buf.trim().to_string());
    }
    sections
}

pub fn stringify_flow_markdown(doc: &FlowDocument) -> Result<String, FlowError> {
    let frontmatter =
        serde_yaml::to_string(&doc.meta).map_err(|e| FlowError::YamlParse(e.to_string()))?;
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(frontmatter.trim_end());
    out.push_str("\n---\n");
    for node in &doc.meta.nodes {
        out.push_str("\n## ");
        out.push_str(&node.id);
        out.push_str("\n\n");
        if !node.prompt.is_empty() {
            out.push_str(node.prompt.trim());
            out.push('\n');
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::types::NodeType;

    const SAMPLE: &str = r#"---
schema_version: 1
slug: release
name: Release Flow
description: 用于一次正式版本发布
created_by: lewis
created_at: 2026-05-12T10:00:00Z
nodes:
  - id: changelog
    type: agent_mention
    owner: alice
    needs: []
  - id: e2e
    type: agent_mention
    owner: bob
    needs: [changelog]
---

## changelog

请基于 `git log v0.7..HEAD` 生成 changelog。

## e2e

跑 `cargo test --workspace`。
"#;

    #[test]
    fn test_parse_happy_path() {
        let doc = parse_flow_markdown(SAMPLE).unwrap();
        assert_eq!(doc.meta.slug, "release");
        assert_eq!(doc.meta.schema_version, 1);
        assert_eq!(doc.meta.nodes.len(), 2);
        assert_eq!(doc.meta.nodes[0].id, "changelog");
        assert_eq!(doc.meta.nodes[0].node_type, NodeType::AgentMention);
        assert_eq!(doc.meta.nodes[0].owner.as_deref(), Some("alice"));
        assert!(doc.meta.nodes[0].prompt.contains("changelog"));
        assert_eq!(doc.meta.nodes[1].needs, vec!["changelog"]);
        assert!(doc.meta.nodes[1].prompt.contains("cargo test"));
    }

    #[test]
    fn test_parse_missing_frontmatter() {
        let err = parse_flow_markdown("## changelog\n\nfoo\n").unwrap_err();
        assert!(matches!(err, FlowError::MissingFrontmatter));
    }

    #[test]
    fn test_parse_schema_version_mismatch() {
        let bad = SAMPLE.replace("schema_version: 1", "schema_version: 2");
        let err = parse_flow_markdown(&bad).unwrap_err();
        assert!(matches!(err, FlowError::SchemaVersion(2)));
    }

    #[test]
    fn test_parse_body_section_missing_warning() {
        let body_stripped = r#"---
schema_version: 1
slug: r
name: r
created_by: lewis
created_at: 2026-05-12T10:00:00Z
nodes:
  - id: a
    type: agent_mention
    owner: alice
    needs: []
---
"#;
        let (_doc, warnings) = parse_flow_markdown_with_warnings(body_stripped).unwrap();
        assert!(
            warnings
                .iter()
                .any(|w| matches!(w, FlowWarning::BodySectionMissing(s) if s == "a")),
            "warnings={warnings:?}",
        );
    }

    #[test]
    fn test_parse_orphan_body_section_warning() {
        let with_orphan = format!(
            "{}\n## extra\n\nthis section has no frontmatter id\n",
            SAMPLE
        );
        let (_doc, warnings) = parse_flow_markdown_with_warnings(&with_orphan).unwrap();
        assert!(
            warnings
                .iter()
                .any(|w| matches!(w, FlowWarning::OrphanBodySection(s) if s == "extra")),
            "warnings={warnings:?}",
        );
    }

    #[test]
    fn test_stringify_round_trip() {
        let doc = parse_flow_markdown(SAMPLE).unwrap();
        let rendered = stringify_flow_markdown(&doc).unwrap();
        let parsed_back = parse_flow_markdown(&rendered).unwrap();
        assert_eq!(parsed_back.meta.slug, doc.meta.slug);
        assert_eq!(parsed_back.meta.nodes.len(), doc.meta.nodes.len());
        for (a, b) in parsed_back.meta.nodes.iter().zip(doc.meta.nodes.iter()) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.node_type, b.node_type);
            assert_eq!(a.owner, b.owner);
            assert_eq!(a.needs, b.needs);
            assert_eq!(a.prompt.trim(), b.prompt.trim());
        }
    }

    #[test]
    fn test_stringify_excludes_prompt_from_frontmatter() {
        let doc = parse_flow_markdown(SAMPLE).unwrap();
        let rendered = stringify_flow_markdown(&doc).unwrap();
        let (frontmatter_block, _body) = rendered
            .strip_prefix("---\n")
            .unwrap()
            .split_once("\n---\n")
            .unwrap();
        assert!(
            !frontmatter_block.contains("prompt:"),
            "frontmatter should not contain prompt field\n{frontmatter_block}",
        );
    }
}
