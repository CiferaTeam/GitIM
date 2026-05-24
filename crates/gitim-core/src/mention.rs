use crate::types::Handler;
use regex::Regex;
use std::sync::LazyLock;

static MENTION_RE: LazyLock<Regex> =
    LazyLock::new(|| crate::preconditions::regex_literal(r"<@([a-z0-9]([a-z0-9-]*[a-z0-9])?)>"));

/// 从消息 body 中提取协议级 mention，去重，按首次出现顺序返回。
pub fn extract_mentions(body: &str) -> Vec<Handler> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for caps in MENTION_RE.captures_iter(body) {
        let raw = &caps[1];
        if seen.contains(raw) {
            continue;
        }
        if let Ok(handler) = Handler::new(raw) {
            seen.insert(raw.to_string());
            result.push(handler);
        }
    }
    result
}
