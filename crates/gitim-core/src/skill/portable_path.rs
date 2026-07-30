use std::collections::BTreeSet;

pub(super) fn valid_portable_relative_paths<'a>(paths: impl IntoIterator<Item = &'a str>) -> bool {
    let mut case_folded_paths = BTreeSet::new();
    paths.into_iter().all(|path| {
        valid_portable_relative_path(path) && case_folded_paths.insert(path.to_ascii_lowercase())
    })
}

fn valid_portable_relative_path(path: &str) -> bool {
    if path.is_empty() || path.len() > 240 || path.starts_with('/') || path.contains('\\') {
        return false;
    }
    path.split('/').all(valid_portable_segment)
}

fn valid_portable_segment(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    if bytes.is_empty() || bytes.len() > 80 || segment.ends_with(['.', ' ']) {
        return false;
    }
    if !matches!(bytes.first(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'))
        || !bytes.iter().all(
            |byte| matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'),
        )
    {
        return false;
    }

    let stem = segment.split('.').next().unwrap_or_default();
    let upper = stem.to_ascii_uppercase();
    !matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | ".GIT" | ".GITIM"
    ) && !matches!(
        upper.as_str(),
        "COM1" | "COM2" | "COM3" | "COM4" | "COM5" | "COM6" | "COM7" | "COM8" | "COM9"
    ) && !matches!(
        upper.as_str(),
        "LPT1" | "LPT2" | "LPT3" | "LPT4" | "LPT5" | "LPT6" | "LPT7" | "LPT8" | "LPT9"
    )
}
