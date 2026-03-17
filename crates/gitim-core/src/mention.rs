use regex::Regex;
use std::sync::LazyLock;
use crate::types::Handler;

static MENTION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<@([a-z0-9]([a-z0-9-]*[a-z0-9])?)>").unwrap()
});

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
