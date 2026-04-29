use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

const SYSTEM_TOOL_DIRS: &[&str] = &[
    "/opt/homebrew/bin",
    "/opt/homebrew/sbin",
    "/usr/local/bin",
    "/usr/local/sbin",
];

const HOME_TOOL_DIRS: &[&str] = &[
    ".gitim/bin",
    ".local/bin",
    ".cargo/bin",
    ".hermes/bin",
    ".hermes/hermes-agent",
    ".hermes/hermes-agent/venv/bin",
    ".claude/local",
    ".codex/bin",
    ".opencode/bin",
    ".gemini/bin",
    ".gstack/bin",
    ".agents/bin",
    ".bun/bin",
    ".deno/bin",
    "go/bin",
    "Library/pnpm",
    ".local/share/pnpm",
    ".local/state/pnpm",
    ".npm/bin",
    ".npm-global/bin",
    ".yarn/bin",
    ".volta/bin",
    ".n/bin",
    ".asdf/shims",
    ".mise/shims",
    ".local/share/mise/shims",
    ".pyenv/shims",
    ".rbenv/shims",
];

pub fn ensure_common_tool_paths() {
    let current = std::env::var_os("PATH").unwrap_or_default();
    let dirs = existing_common_tool_dirs();
    let next = augment_path(&current, dirs);
    if next != current {
        std::env::set_var("PATH", next);
    }
}

fn existing_common_tool_dirs() -> Vec<PathBuf> {
    let home = dirs::home_dir();
    candidate_tool_dirs(home.as_deref(), SYSTEM_TOOL_DIRS.iter().map(PathBuf::from))
        .into_iter()
        .filter(|dir| dir.is_dir())
        .collect()
}

fn candidate_tool_dirs<I>(home: Option<&Path>, system_dirs: I) -> Vec<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
{
    let mut dirs: Vec<PathBuf> = system_dirs.into_iter().collect();
    if let Some(home) = home {
        dirs.extend(HOME_TOOL_DIRS.iter().map(|dir| home.join(dir)));
    }
    dirs
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

    #[test]
    fn candidate_tool_dirs_include_user_and_agent_bins() {
        let dirs = candidate_tool_dirs(
            Some(Path::new("/Users/example")),
            [PathBuf::from("/opt/homebrew/bin")],
        );

        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/Users/example/.gitim/bin"),
                PathBuf::from("/Users/example/.local/bin"),
                PathBuf::from("/Users/example/.cargo/bin"),
                PathBuf::from("/Users/example/.hermes/bin"),
                PathBuf::from("/Users/example/.hermes/hermes-agent"),
                PathBuf::from("/Users/example/.hermes/hermes-agent/venv/bin"),
                PathBuf::from("/Users/example/.claude/local"),
                PathBuf::from("/Users/example/.codex/bin"),
                PathBuf::from("/Users/example/.opencode/bin"),
                PathBuf::from("/Users/example/.gemini/bin"),
                PathBuf::from("/Users/example/.gstack/bin"),
                PathBuf::from("/Users/example/.agents/bin"),
                PathBuf::from("/Users/example/.bun/bin"),
                PathBuf::from("/Users/example/.deno/bin"),
                PathBuf::from("/Users/example/go/bin"),
                PathBuf::from("/Users/example/Library/pnpm"),
                PathBuf::from("/Users/example/.local/share/pnpm"),
                PathBuf::from("/Users/example/.local/state/pnpm"),
                PathBuf::from("/Users/example/.npm/bin"),
                PathBuf::from("/Users/example/.npm-global/bin"),
                PathBuf::from("/Users/example/.yarn/bin"),
                PathBuf::from("/Users/example/.volta/bin"),
                PathBuf::from("/Users/example/.n/bin"),
                PathBuf::from("/Users/example/.asdf/shims"),
                PathBuf::from("/Users/example/.mise/shims"),
                PathBuf::from("/Users/example/.local/share/mise/shims"),
                PathBuf::from("/Users/example/.pyenv/shims"),
                PathBuf::from("/Users/example/.rbenv/shims"),
            ]
        );
    }
}
