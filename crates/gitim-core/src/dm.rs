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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dm_filename_ordering() {
        let a = Handler::new("lewis").unwrap();
        let b = Handler::new("nexus").unwrap();
        assert_eq!(dm_filename(&a, &b), "lewis--nexus");
        assert_eq!(dm_filename(&b, &a), "lewis--nexus");
    }

    #[test]
    fn test_dm_filename_with_hyphens() {
        let a = Handler::new("cifera-nexus").unwrap();
        let b = Handler::new("lewis").unwrap();
        assert_eq!(dm_filename(&a, &b), "cifera-nexus--lewis");
    }

    #[test]
    fn test_dm_filename_prefix_match() {
        let a = Handler::new("alice").unwrap();
        let b = Handler::new("alice2").unwrap();
        assert_eq!(dm_filename(&a, &b), "alice--alice2");
    }

    #[test]
    fn test_parse_dm_filename_valid() {
        let (first, second) = parse_dm_filename("lewis--nexus").unwrap();
        assert_eq!(first, "lewis");
        assert_eq!(second, "nexus");
    }

    #[test]
    fn test_parse_dm_filename_with_hyphens() {
        let (first, second) = parse_dm_filename("cifera-nexus--lewis").unwrap();
        assert_eq!(first, "cifera-nexus");
        assert_eq!(second, "lewis");
    }

    #[test]
    fn test_parse_dm_filename_invalid_no_separator() {
        assert!(parse_dm_filename("lewis").is_none());
    }

    #[test]
    fn test_parse_dm_filename_invalid_empty_first() {
        assert!(parse_dm_filename("--nexus").is_none());
    }

    #[test]
    fn test_parse_dm_filename_invalid_empty_second() {
        assert!(parse_dm_filename("lewis--").is_none());
    }
}
