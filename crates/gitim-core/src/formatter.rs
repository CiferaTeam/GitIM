use crate::types::Handler;
use regex::Regex;
use std::sync::LazyLock;

static MSG_PREFIX_RE: LazyLock<Regex> =
    LazyLock::new(|| crate::preconditions::regex_literal(r"^\[L\d{6,}\]"));

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
