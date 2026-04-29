use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

const COMMON_TOOL_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin"];

pub fn ensure_common_tool_paths() {
    let current = std::env::var_os("PATH").unwrap_or_default();
    let dirs = existing_common_tool_dirs();
    let next = augment_path(&current, dirs);
    if next != current {
        std::env::set_var("PATH", next);
    }
}

fn existing_common_tool_dirs() -> Vec<PathBuf> {
    COMMON_TOOL_DIRS
        .iter()
        .map(PathBuf::from)
        .filter(|dir| dir.is_dir())
        .collect()
}

fn augment_path<I>(current: &OsStr, extra_dirs: I) -> OsString
where
    I: IntoIterator<Item = PathBuf>,
{
    let mut paths: Vec<PathBuf> = std::env::split_paths(current).collect();
    let mut seen: HashSet<PathBuf> = paths.iter().map(|path| normalize_path(path)).collect();

    for dir in extra_dirs {
        let normalized = normalize_path(&dir);
        if seen.insert(normalized) {
            paths.push(dir);
        }
    }

    std::env::join_paths(paths).unwrap_or_else(|_| current.to_os_string())
}

fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn augment_path_appends_missing_tool_dirs() {
        let current = OsStr::new("/usr/bin:/bin");
        let next = augment_path(
            current,
            [
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/local/bin"),
            ],
        );

        assert_eq!(
            next,
            OsString::from("/usr/bin:/bin:/opt/homebrew/bin:/usr/local/bin")
        );
    }

    #[test]
    fn augment_path_does_not_duplicate_existing_dirs() {
        let current = OsStr::new("/usr/bin:/opt/homebrew/bin:/bin");
        let next = augment_path(
            current,
            [
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/local/bin"),
            ],
        );

        assert_eq!(
            next,
            OsString::from("/usr/bin:/opt/homebrew/bin:/bin:/usr/local/bin")
        );
    }
}
