use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::handler::Handler;
use super::labels::{validate_labels, LabelError, BOARD_MAX_LABELS};

pub const BOARD_VERSION: u32 = 1;
pub const BOARD_MAX_BYTES: usize = 64 * 1024;
pub const BOARD_MAX_STATUS_LEN: usize = 80;
pub const BOARD_MAX_SUMMARY_LEN: usize = 280;

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum BoardError {
    #[error("invalid handler: {0}")]
    InvalidHandler(String),
    #[error("handler mismatch: expected {expected}, got {actual}")]
    HandlerMismatch { expected: String, actual: String },
    #[error("unsupported board version: {0}, expected {1}")]
    UnsupportedVersion(u32, u32),
    #[error("invalid timestamp '{0}'")]
    InvalidTimestamp(String),
    #[error("status cannot be empty")]
    EmptyStatus,
    #[error("status exceeds {1} bytes, got {0}")]
    StatusTooLong(usize, usize),
    #[error("summary exceeds {1} bytes, got {0}")]
    SummaryTooLong(usize, usize),
    #[error(transparent)]
    Label(#[from] LabelError),
    #[error("YAML serialization error: {0}")]
    YamlSerialize(String),
    #[error("unknown board field '{0}'")]
    UnknownField(String),
    #[error("invalid section name: {0}")]
    InvalidSection(String),
    #[error("board document exceeds {1} bytes, got {0}")]
    DocumentTooLarge(usize, usize),
}

#[derive(Error, Debug)]
pub enum BoardMarkdownError {
    #[error("board markdown exceeds {1} bytes, got {0}")]
    TooLarge(usize, usize),
    #[error("board markdown must start with frontmatter delimiter")]
    MissingOpeningDelimiter,
    #[error("board markdown missing closing frontmatter delimiter")]
    MissingClosingDelimiter,
    #[error("YAML parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error(transparent)]
    Board(#[from] BoardError),
}

/// Board metadata frontmatter.
///
/// Two compatibility provisions for the unified-labels rollout:
///
/// 1. **`#[serde(deny_unknown_fields)]` removed** (eng-review Issue #1) so a
///    new daemon doesn't reject yaml/JSON with future-unknown fields. This
///    matches `CardMeta` / `UserMeta` policy.
///
/// 2. **Field is serialized as `tags:`** during the v1 transition window
///    (PR #35 review P1): the internal Rust name is `labels` for code clarity,
///    but `#[serde(rename = "tags", alias = "labels")]` means yaml/JSON
///    output continues to use `tags:` (compatible with old daemons that still
///    have `deny_unknown_fields`), while accepting either name on input. v2
///    will switch the output side to `labels:` after enough release cycles
///    that no peer is left running pre-v1.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BoardMeta {
    pub version: u32,
    pub handler: String,
    pub updated_at: String,
    pub status: String,
    pub summary: String,
    #[serde(default, rename = "tags", alias = "labels")]
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BoardDocument {
    pub meta: BoardMeta,
    pub body: String,
}

pub fn board_path(handler: &str) -> Result<PathBuf, BoardError> {
    let handler = Handler::new(handler).map_err(|e| BoardError::InvalidHandler(e.to_string()))?;
    Ok(PathBuf::from("showboards")
        .join(handler.as_str())
        .join("board.md"))
}

pub fn default_board(handler: &str, timestamp: &str) -> Result<BoardDocument, BoardError> {
    let handler = Handler::new(handler).map_err(|e| BoardError::InvalidHandler(e.to_string()))?;
    let doc = BoardDocument {
        meta: BoardMeta {
            version: BOARD_VERSION,
            handler: handler.to_string(),
            updated_at: timestamp.to_string(),
            status: "idle".to_string(),
            summary: String::new(),
            labels: Vec::new(),
        },
        body: default_board_body(),
    };
    validate_board_document(&doc)?;
    Ok(doc)
}

pub fn parse_board_markdown(input: &str) -> Result<BoardDocument, BoardMarkdownError> {
    if input.len() > BOARD_MAX_BYTES {
        return Err(BoardMarkdownError::TooLarge(input.len(), BOARD_MAX_BYTES));
    }

    let (yaml, body) = split_frontmatter(input)?;
    let meta: BoardMeta = serde_yaml::from_str(yaml)?;
    let doc = BoardDocument {
        meta,
        body: body.to_string(),
    };
    validate_board_document(&doc)?;
    Ok(doc)
}

pub fn stringify_board_markdown(doc: &BoardDocument) -> Result<String, BoardMarkdownError> {
    validate_board_document(doc)?;

    let yaml = serde_yaml::to_string(&doc.meta)?;
    let yaml = yaml.strip_prefix("---\n").unwrap_or(&yaml);
    let mut out = String::with_capacity(yaml.len() + doc.body.len() + 8);
    out.push_str("---\n");
    out.push_str(yaml);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("---\n");
    out.push_str(&doc.body);
    if !out.ends_with('\n') {
        out.push('\n');
    }

    if out.len() > BOARD_MAX_BYTES {
        return Err(BoardMarkdownError::Board(BoardError::DocumentTooLarge(
            out.len(),
            BOARD_MAX_BYTES,
        )));
    }
    Ok(out)
}

pub fn validate_board_document(doc: &BoardDocument) -> Result<(), BoardError> {
    validate_board_meta(&doc.meta)?;
    validate_rendered_size(&doc.meta, &doc.body)
}

pub fn validate_board_for_handler(
    doc: &BoardDocument,
    expected_handler: &str,
) -> Result<(), BoardError> {
    let expected =
        Handler::new(expected_handler).map_err(|e| BoardError::InvalidHandler(e.to_string()))?;
    validate_board_document(doc)?;
    if doc.meta.handler != expected.as_str() {
        return Err(BoardError::HandlerMismatch {
            expected: expected.to_string(),
            actual: doc.meta.handler.clone(),
        });
    }
    Ok(())
}

pub fn set_board_field(
    doc: &mut BoardDocument,
    field: &str,
    value: &str,
) -> Result<(), BoardError> {
    match field {
        "status" => {
            let status = value.trim().to_string();
            validate_status(&status)?;
            doc.meta.status = status;
        }
        "summary" => {
            let summary = value.trim().to_string();
            validate_summary(&summary)?;
            doc.meta.summary = summary;
        }
        // "labels" is canonical; "tags" is a backward-compat alias accepted as a
        // field name so existing agent prompts (`gitim board set tags <csv>`)
        // keep working through the v1 rollout.
        "labels" | "tags" => {
            let labels = parse_labels_csv(value)?;
            doc.meta.labels = labels;
        }
        other => return Err(BoardError::UnknownField(other.to_string())),
    }
    validate_board_document(doc)
}

pub fn set_board_section(
    doc: &mut BoardDocument,
    section: &str,
    value: &str,
) -> Result<(), BoardError> {
    let section = validate_section_name(section)?;
    let replacement = section_block(section, value);
    let body = if let Some(range) = find_section(&doc.body, section) {
        let mut body = String::with_capacity(doc.body.len() + replacement.len());
        body.push_str(&doc.body[..range.start]);
        body.push_str(&replacement);
        body.push_str(&doc.body[range.end..]);
        body
    } else {
        body_with_appended_section(&doc.body, &replacement)
    };
    validate_candidate_document(&doc.meta, &body)?;
    doc.body = body;
    Ok(())
}

pub fn append_board_section(
    doc: &mut BoardDocument,
    section: &str,
    value: &str,
) -> Result<(), BoardError> {
    let section = validate_section_name(section)?;
    let value = normalize_section_value(value);
    let body = if let Some(range) = find_section(&doc.body, section) {
        let existing = &doc.body[range.start..range.end];
        let replacement = append_to_section_block(existing, &value);
        let mut body = String::with_capacity(doc.body.len() + replacement.len());
        body.push_str(&doc.body[..range.start]);
        body.push_str(&replacement);
        body.push_str(&doc.body[range.end..]);
        body
    } else {
        body_with_appended_section(&doc.body, &section_block(section, &value))
    };
    validate_candidate_document(&doc.meta, &body)?;
    doc.body = body;
    Ok(())
}

fn default_board_body() -> String {
    "## 我能做什么\n\n## 暂时阻塞\n\n## 最近交付\n\n## 合作前需要知道的\n".to_string()
}

fn split_frontmatter(input: &str) -> Result<(&str, &str), BoardMarkdownError> {
    const DELIMITER: &str = "---\n";
    if !input.starts_with(DELIMITER) {
        return Err(BoardMarkdownError::MissingOpeningDelimiter);
    }

    let rest = &input[DELIMITER.len()..];
    if let Some(idx) = rest.find("\n---\n") {
        let yaml = &rest[..idx + 1];
        let body = &rest[idx + "\n---\n".len()..];
        Ok((yaml, body))
    } else if let Some(body) = rest.strip_prefix(DELIMITER) {
        Ok(("", body))
    } else {
        Err(BoardMarkdownError::MissingClosingDelimiter)
    }
}

fn validate_board_meta(meta: &BoardMeta) -> Result<(), BoardError> {
    if meta.version != BOARD_VERSION {
        return Err(BoardError::UnsupportedVersion(meta.version, BOARD_VERSION));
    }
    Handler::new(&meta.handler).map_err(|e| BoardError::InvalidHandler(e.to_string()))?;
    validate_timestamp(&meta.updated_at)?;
    validate_status(&meta.status)?;
    validate_summary(&meta.summary)?;
    validate_labels(&meta.labels, BOARD_MAX_LABELS)?;
    Ok(())
}

fn validate_candidate_document(meta: &BoardMeta, body: &str) -> Result<(), BoardError> {
    validate_board_meta(meta)?;
    validate_rendered_size(meta, body)
}

fn validate_rendered_size(meta: &BoardMeta, body: &str) -> Result<(), BoardError> {
    let len = rendered_board_len(meta, body)?;
    if len > BOARD_MAX_BYTES {
        return Err(BoardError::DocumentTooLarge(len, BOARD_MAX_BYTES));
    }
    Ok(())
}

fn rendered_board_len(meta: &BoardMeta, body: &str) -> Result<usize, BoardError> {
    let yaml = serde_yaml::to_string(meta).map_err(|e| BoardError::YamlSerialize(e.to_string()))?;
    let yaml = yaml.strip_prefix("---\n").unwrap_or(&yaml);
    let yaml_newline_len = usize::from(!yaml.ends_with('\n'));
    let final_newline_len = usize::from(!body.is_empty() && !body.ends_with('\n'));

    Ok("---\n".len()
        + yaml.len()
        + yaml_newline_len
        + "---\n".len()
        + body.len()
        + final_newline_len)
}

fn validate_timestamp(timestamp: &str) -> Result<(), BoardError> {
    let bytes = timestamp.as_bytes();
    let valid = bytes.len() == 16
        && bytes[8] == b'T'
        && bytes[15] == b'Z'
        && bytes[..8].iter().all(u8::is_ascii_digit)
        && bytes[9..15].iter().all(u8::is_ascii_digit);
    if valid {
        Ok(())
    } else {
        Err(BoardError::InvalidTimestamp(timestamp.to_string()))
    }
}

fn validate_status(status: &str) -> Result<(), BoardError> {
    if status.trim().is_empty() {
        return Err(BoardError::EmptyStatus);
    }
    if status.len() > BOARD_MAX_STATUS_LEN {
        return Err(BoardError::StatusTooLong(
            status.len(),
            BOARD_MAX_STATUS_LEN,
        ));
    }
    Ok(())
}

fn validate_summary(summary: &str) -> Result<(), BoardError> {
    if summary.len() > BOARD_MAX_SUMMARY_LEN {
        return Err(BoardError::SummaryTooLong(
            summary.len(),
            BOARD_MAX_SUMMARY_LEN,
        ));
    }
    Ok(())
}

fn parse_labels_csv(value: &str) -> Result<Vec<String>, BoardError> {
    let labels = value
        .split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    validate_labels(&labels, BOARD_MAX_LABELS)?;
    Ok(labels)
}

fn validate_section_name(section: &str) -> Result<&str, BoardError> {
    let section = section.trim();
    if section.is_empty() {
        return Err(BoardError::InvalidSection("empty".to_string()));
    }
    if section.contains('\n') || section.contains('\r') {
        return Err(BoardError::InvalidSection(
            "must fit on one line".to_string(),
        ));
    }
    Ok(section)
}

fn section_block(section: &str, value: &str) -> String {
    let value = normalize_section_value(value);
    if value.is_empty() {
        format!("## {section}\n\n")
    } else {
        format!("## {section}\n\n{value}\n")
    }
}

fn normalize_section_value(value: &str) -> String {
    value.trim_matches('\n').to_string()
}

fn body_with_appended_section(body: &str, section: &str) -> String {
    if body.is_empty() {
        return section.to_string();
    }

    let mut out = body.trim_end_matches('\n').to_string();
    out.push_str("\n\n");
    out.push_str(section);
    out
}

fn append_to_section_block(existing: &str, value: &str) -> String {
    let mut replacement = existing.trim_end_matches('\n').to_string();
    if !replacement.ends_with('\n') {
        replacement.push('\n');
    }
    if !value.is_empty() {
        replacement.push_str(value);
        replacement.push('\n');
    }
    replacement
}

#[derive(Debug, Clone, Copy)]
struct SectionRange {
    start: usize,
    end: usize,
}

fn find_section(body: &str, section: &str) -> Option<SectionRange> {
    let mut offset = 0;
    let mut start = None;
    let mut content_start = 0;

    for line in body.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();
        if heading_name(line) == Some(section) {
            start = Some(line_start);
            content_start = offset;
            break;
        }
    }

    let start = start?;
    let mut end = body.len();
    let mut offset = content_start;
    for line in body[content_start..].split_inclusive('\n') {
        if heading_name(line).is_some() {
            end = offset;
            break;
        }
        offset += line.len();
    }

    Some(SectionRange { start, end })
}

fn heading_name(line: &str) -> Option<&str> {
    let line = line.trim_end_matches('\n').trim_end_matches('\r');
    line.strip_prefix("## ").map(str::trim)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_board() -> &'static str {
        "---\nversion: 1\nhandler: alice\nupdated_at: 20260509T120000Z\nstatus: working\nsummary: 正在梳理发布风险\nlabels:\n  - release\n---\n## 当前状态\n\n在看 sync 失败。\n\n## 已知事实\n\n- origin/main 可达\n"
    }

    fn board_without_labels() -> &'static str {
        "---\nversion: 1\nhandler: alice\nupdated_at: 20260509T120000Z\nstatus: working\nsummary: 正在梳理发布风险\n---\n## 当前状态\n"
    }

    fn legacy_board_with_tags() -> &'static str {
        // 旧 yaml 用 tags: alias 应能 read
        "---\nversion: 1\nhandler: alice\nupdated_at: 20260509T120000Z\nstatus: working\nsummary: s\ntags:\n  - release\n---\nbody\n"
    }

    #[test]
    fn board_markdown_roundtrips() {
        let parsed = parse_board_markdown(sample_board()).unwrap();
        assert_eq!(parsed.meta.handler, "alice");
        assert_eq!(parsed.meta.status, "working");
        assert_eq!(parsed.meta.labels, vec!["release"]);
        assert!(parsed.body.contains("## 当前状态"));

        let rendered = stringify_board_markdown(&parsed).unwrap();
        let reparsed = parse_board_markdown(&rendered).unwrap();
        assert_eq!(reparsed, parsed);
    }

    #[test]
    fn default_board_includes_standard_headings() {
        let board = default_board("alice", "20260509T120000Z").unwrap();

        assert!(board.body.contains("## 我能做什么"));
        assert!(board.body.contains("## 暂时阻塞"));
        assert!(board.body.contains("## 最近交付"));
        assert!(board.body.contains("## 合作前需要知道的"));
    }

    #[test]
    fn parse_board_without_labels_defaults_to_empty_vec() {
        let parsed = parse_board_markdown(board_without_labels()).unwrap();

        assert!(parsed.meta.labels.is_empty());
    }

    #[test]
    fn legacy_tags_yaml_field_is_read_via_alias() {
        // 兼容旧 yaml: `tags:` 字段经 serde alias 路由到 BoardMeta.labels
        let parsed = parse_board_markdown(legacy_board_with_tags()).unwrap();
        assert_eq!(parsed.meta.labels, vec!["release"]);
    }

    #[test]
    fn rendered_yaml_keeps_tags_field_name_for_v1() {
        // v1 transition window (PR #35 review P1): wire/yaml output stays as
        // `tags:` so old daemons with `deny_unknown_fields` keep working.
        // Internal Rust field is `labels` (serde rename) — only the on-wire
        // name is `tags`. v2 will swap after fleet upgrades.
        let parsed = parse_board_markdown(legacy_board_with_tags()).unwrap();
        let rendered = stringify_board_markdown(&parsed).unwrap();
        assert!(rendered.contains("tags:"), "rendered:\n{rendered}");
        assert!(!rendered.contains("labels:"), "rendered:\n{rendered}");
    }

    #[test]
    fn new_yaml_with_labels_alias_parses_and_round_trips_to_tags() {
        // Caller wrote yaml with `labels:` directly (e.g. hand-edit or future
        // migration tool). v1 daemon reads it via alias, then writes back with
        // canonical `tags:` on next save. This is the passive migration path
        // in reverse — we tolerate `labels:` input but normalize output.
        let yaml = "---\nversion: 1\nhandler: alice\nupdated_at: 20260509T120000Z\nstatus: working\nsummary: s\nlabels:\n  - release\n---\nbody\n";
        let parsed = parse_board_markdown(yaml).unwrap();
        assert_eq!(parsed.meta.labels, vec!["release"]);
        let rendered = stringify_board_markdown(&parsed).unwrap();
        assert!(rendered.contains("tags:"));
    }

    #[test]
    fn invalid_label_characters_are_rejected() {
        let invalid = "---\nversion: 1\nhandler: alice\nupdated_at: 20260509T120000Z\nstatus: working\nsummary: s\nlabels:\n  - release!\n---\nbody\n";

        assert!(parse_board_markdown(invalid).is_err());
    }

    #[test]
    fn unknown_frontmatter_fields_are_silently_dropped() {
        // eng-review Issue #1:`deny_unknown_fields` 移除后,unknown 字段
        // 不再 reject(避免新 daemon 写 `labels:` 时老 daemon fetch 拒收)。
        let with_extra = "---\nversion: 1\nhandler: alice\nupdated_at: 20260509T120000Z\nstatus: working\nsummary: s\nlabels: []\nfuture_field: dropped\n---\nbody\n";

        let parsed = parse_board_markdown(with_extra).expect("should accept unknown field");
        assert_eq!(parsed.meta.handler, "alice");
        // future_field 不存在于 BoardMeta,被静默 drop
    }

    #[test]
    fn set_field_revalidates_existing_metadata() {
        let mut parsed = parse_board_markdown(sample_board()).unwrap();
        parsed.meta.version = 2;

        assert!(set_board_field(&mut parsed, "summary", "等待 CI 结果").is_err());
    }

    #[test]
    fn validate_rejects_rendered_document_over_max_bytes() {
        let mut parsed = parse_board_markdown(sample_board()).unwrap();
        parsed.body = "x".repeat(BOARD_MAX_BYTES - 1);

        assert!(validate_board_document(&parsed).is_err());
    }

    #[test]
    fn section_edit_rejects_rendered_document_over_max_bytes() {
        let mut parsed = parse_board_markdown(sample_board()).unwrap();
        parsed.body.clear();
        let empty_rendered_len = stringify_board_markdown(&parsed).unwrap().len();
        let target_body_len = BOARD_MAX_BYTES - empty_rendered_len + 1;
        let section_shell_len = "## 当前状态\n\n".len() + "\n".len();
        let value = "x".repeat(target_body_len - section_shell_len);

        assert!(target_body_len < BOARD_MAX_BYTES);
        assert!(set_board_section(&mut parsed, "当前状态", &value).is_err());
    }

    #[test]
    fn stringified_board_ends_with_newline() {
        let mut parsed = parse_board_markdown(sample_board()).unwrap();
        parsed.body = "## 当前状态".to_string();

        let rendered = stringify_board_markdown(&parsed).unwrap();

        assert!(rendered.ends_with('\n'));
    }

    #[test]
    fn validate_rejects_handler_mismatch() {
        let parsed = parse_board_markdown(sample_board()).unwrap();
        let err = validate_board_for_handler(&parsed, "bob").unwrap_err();
        assert!(err.to_string().contains("handler mismatch"));
    }

    #[test]
    fn set_field_updates_thin_frontmatter() {
        let mut parsed = parse_board_markdown(sample_board()).unwrap();
        set_board_field(&mut parsed, "summary", "等待 CI 结果").unwrap();
        set_board_field(&mut parsed, "labels", "ci,release").unwrap();

        assert_eq!(parsed.meta.summary, "等待 CI 结果");
        assert_eq!(parsed.meta.labels, vec!["ci", "release"]);
    }

    #[test]
    fn set_field_with_tags_alias_routes_to_labels() {
        // eng-review Issue #2 + P2:`set_board_field` arg name 同时接受
        // 'tags' 和 'labels',两者路由到同一个 meta.labels 字段
        let mut parsed = parse_board_markdown(sample_board()).unwrap();
        set_board_field(&mut parsed, "tags", "ci,release").unwrap();
        assert_eq!(parsed.meta.labels, vec!["ci", "release"]);
    }

    #[test]
    fn section_set_replaces_existing_section() {
        let mut parsed = parse_board_markdown(sample_board()).unwrap();
        set_board_section(&mut parsed, "当前状态", "已定位为 token 过期。").unwrap();

        assert!(parsed.body.contains("## 当前状态\n\n已定位为 token 过期。"));
        assert!(!parsed.body.contains("在看 sync 失败。"));
        assert!(parsed.body.contains("## 已知事实"));
    }

    #[test]
    fn section_append_creates_missing_section() {
        let mut parsed = parse_board_markdown(sample_board()).unwrap();
        append_board_section(&mut parsed, "待确认", "- 是否需要轮换 token").unwrap();

        assert!(parsed.body.contains("## 待确认\n\n- 是否需要轮换 token"));
    }

    #[test]
    fn board_path_is_handler_scoped() {
        assert_eq!(
            board_path("alice").unwrap().to_string_lossy(),
            "showboards/alice/board.md"
        );
        assert!(board_path("Alice").is_err());
    }
}
