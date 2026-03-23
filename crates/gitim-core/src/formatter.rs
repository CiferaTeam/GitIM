use std::sync::LazyLock;
use regex::Regex;
use crate::types::Handler;

static MSG_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\[L\d{6,}\]").unwrap()
});

pub fn format_message(
    line_number: u64,
    point_to: u64,
    author: &Handler,
    timestamp: &str,
    body: &str,
) -> String {
    let width = format!("{}", line_number).len().max(6);
    let mut output = format!(
        "[L{:0>width$}][P{:0>width$}][@{}][{}] ",
        line_number,
        point_to,
        author.as_str(),
        timestamp,
        width = width,
    );

    let mut lines = body.lines().peekable();
    if let Some(first) = lines.next() {
        output.push_str(first);
        output.push('\n');

        for line in lines {
            // Escape continuation lines that look like message prefixes
            if MSG_PREFIX_RE.is_match(line) {
                output.push(' ');
            }
            output.push_str(line);
            output.push('\n');
        }
    } else {
        output.push('\n');
    }

    output
}

pub fn format_event(
    line_number: u64,
    author: &Handler,
    timestamp: &str,
    event_type: &str,
    meta: &serde_json::Value,
) -> String {
    let width = format!("{}", line_number).len().max(6);
    let meta_str = serde_json::to_string(meta).unwrap_or_else(|_| "{}".to_string());
    format!(
        "[L{:0>width$}][P{:0>width$}][@{}][{}][E:{}] {}\n",
        line_number,
        0,
        author.as_str(),
        timestamp,
        event_type,
        meta_str,
        width = width,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_event_self_join() {
        let author = Handler::new("nexus").unwrap();
        let result = format_event(1, &author, "20250316T120000Z", "join", &serde_json::json!({}));
        assert_eq!(
            result,
            "[L000001][P000000][@nexus][20250316T120000Z][E:join] {}\n"
        );
    }

    #[test]
    fn test_format_event_with_targets() {
        let author = Handler::new("nexus").unwrap();
        let meta = serde_json::json!({"targets": ["lewis", "coder"]});
        let result = format_event(5, &author, "20250316T120000Z", "leave", &meta);
        assert!(result.starts_with("[L000005][P000000][@nexus][20250316T120000Z][E:leave] "));
        assert!(result.contains("\"targets\""));
        assert!(result.ends_with('\n'));
    }
}
