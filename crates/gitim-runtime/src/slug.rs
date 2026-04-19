use std::collections::HashSet;

const RESERVED: &[&str] = &["default", "system", "active", "current"];
const MAX_LEN: usize = 32;
const FALLBACK: &str = "workspace";

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum SlugError {
    #[error("slug empty")]
    Empty,
    #[error("slug too long (max {MAX_LEN})")]
    TooLong,
    #[error("slug contains invalid characters (allowed: a-z 0-9 -)")]
    InvalidChars,
}

/// Normalize a raw string (e.g. directory basename) into slug form.
/// Rules: lowercase -> replace non-[a-z0-9-] with `-` -> collapse repeats ->
/// trim `-` -> truncate to 32 -> empty falls back to "workspace".
pub fn normalize(raw: &str) -> String {
    let lower = raw.to_lowercase();
    let replaced: String = lower
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect();
    let mut collapsed = String::with_capacity(replaced.len());
    let mut prev_dash = false;
    for c in replaced.chars() {
        if c == '-' {
            if !prev_dash {
                collapsed.push(c);
            }
            prev_dash = true;
        } else {
            collapsed.push(c);
            prev_dash = false;
        }
    }
    let trimmed = collapsed.trim_matches('-');
    let truncated: String = trimmed.chars().take(MAX_LEN).collect();
    let truncated = truncated.trim_end_matches('-').to_string();
    if truncated.is_empty() {
        FALLBACK.to_string()
    } else {
        truncated
    }
}

/// Resolve a slug collision by appending `-2`, `-3`, ... Reserved keywords
/// always collide — so `default` becomes `default-2`.
pub fn resolve(candidate: &str, existing: &HashSet<String>) -> String {
    let reserved = RESERVED.contains(&candidate);
    if !reserved && !existing.contains(candidate) {
        return candidate.to_string();
    }
    let mut n = 2u32;
    loop {
        let suffixed = format!("{candidate}-{n}");
        if !existing.contains(&suffixed) && !RESERVED.contains(&suffixed.as_str()) {
            return suffixed;
        }
        n += 1;
    }
}

pub fn validate(slug: &str) -> Result<(), SlugError> {
    if slug.is_empty() {
        return Err(SlugError::Empty);
    }
    if slug.len() > MAX_LEN {
        return Err(SlugError::TooLong);
    }
    if !slug
        .chars()
        .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase() || c == '-')
    {
        return Err(SlugError::InvalidChars);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_basic() {
        assert_eq!(normalize("Frontend"), "frontend");
    }

    #[test]
    fn normalize_spaces() {
        assert_eq!(normalize("My workspace"), "my-workspace");
    }

    #[test]
    fn normalize_unicode() {
        assert_eq!(normalize("前端"), "workspace");
    }

    #[test]
    fn normalize_collapses() {
        assert_eq!(normalize("a---b"), "a-b");
    }

    #[test]
    fn normalize_trims() {
        assert_eq!(normalize("-foo-"), "foo");
    }

    #[test]
    fn normalize_truncates() {
        assert_eq!(normalize(&"x".repeat(100)).len(), 32);
    }

    #[test]
    fn normalize_empty_fallback() {
        assert_eq!(normalize(""), "workspace");
    }

    #[test]
    fn resolve_conflict_no_conflict() {
        let existing: HashSet<String> = HashSet::new();
        assert_eq!(resolve("foo", &existing), "foo");
    }

    #[test]
    fn resolve_conflict_appends_2() {
        let existing: HashSet<String> = ["foo"].into_iter().map(String::from).collect();
        assert_eq!(resolve("foo", &existing), "foo-2");
    }

    #[test]
    fn resolve_conflict_skips_taken_suffixes() {
        let existing = ["foo", "foo-2", "foo-3"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(resolve("foo", &existing), "foo-4");
    }

    #[test]
    fn resolve_reserved_keyword() {
        let existing: HashSet<String> = HashSet::new();
        assert_eq!(resolve("default", &existing), "default-2");
    }

    #[test]
    fn validate_accepts() {
        assert!(validate("foo-bar").is_ok());
    }

    #[test]
    fn validate_rejects_uppercase() {
        assert!(validate("Foo").is_err());
    }

    #[test]
    fn validate_rejects_slash() {
        assert!(validate("foo/bar").is_err());
    }

    #[test]
    fn validate_rejects_dotdot() {
        assert!(validate("..").is_err());
    }

    #[test]
    fn validate_rejects_empty() {
        assert!(validate("").is_err());
    }

    #[test]
    fn validate_rejects_over_32() {
        assert!(validate(&"x".repeat(33)).is_err());
    }
}
