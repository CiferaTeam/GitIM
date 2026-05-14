use crate::types::Handler;

/// DM filename for a handler pair: `{first}--{second}`, where the two
/// handlers are sorted in lexicographic order.
pub fn dm_filename(a: &Handler, b: &Handler) -> String {
    let (first, second) = if a.as_str() <= b.as_str() {
        (a.as_str(), b.as_str())
    } else {
        (b.as_str(), a.as_str())
    };
    format!("{}--{}", first, second)
}

/// Split a DM filename stem back into its two handlers, or `None`
/// if the stem isn't a valid `{first}--{second}` shape.
pub fn parse_dm_filename(stem: &str) -> Option<(&str, &str)> {
    let idx = stem.find("--")?;
    let first = &stem[..idx];
    let second = &stem[idx + 2..];
    if first.is_empty() || second.is_empty() {
        return None;
    }
    Some((first, second))
}
