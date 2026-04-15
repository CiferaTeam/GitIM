use crate::types::Handler;

/// Generate a normalized DM filename from two handlers.
///
/// The filename format is: `{first}--{second}` where first and second are
/// the two handlers sorted in lexicographic order.
pub fn dm_filename(a: &Handler, b: &Handler) -> String {
    let (first, second) = if a.as_str() <= b.as_str() {
        (a.as_str(), b.as_str())
    } else {
        (b.as_str(), a.as_str())
    };
    format!("{}--{}", first, second)
}

/// Parse a DM filename into the two participant handlers.
///
/// Returns `None` if the filename is not a valid DM filename.
pub fn parse_dm_filename(stem: &str) -> Option<(&str, &str)> {
    let idx = stem.find("--")?;
    let first = &stem[..idx];
    let second = &stem[idx + 2..];
    if first.is_empty() || second.is_empty() {
        return None;
    }
    Some((first, second))
}
