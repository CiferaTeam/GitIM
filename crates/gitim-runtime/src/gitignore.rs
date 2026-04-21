//! Idempotent .gitignore management for the `.env` secrets convention.
//!
//! Called from workspace provisioning so every agent clone inherits the rule
//! via its shared remote. Separate module to keep workspace.rs / http.rs
//! focused on orchestration.

use std::path::Path;

/// Append `.env` to the repo's `.gitignore` if not already matched. Returns
/// `Ok(true)` if a change was made (caller should commit), `Ok(false)` if
/// already present.
pub fn ensure_env_gitignored(clone_root: &Path) -> std::io::Result<bool> {
    let path = clone_root.join(".gitignore");
    let current = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };

    // Match any of: ".env", "/.env", ".env*", "/.env*" as standalone lines.
    // Comments and blank lines don't count. We deliberately don't parse
    // negation (`!...`) or more complex glob forms — if a user has something
    // fancier in their .gitignore, they already know what they're doing.
    for line in current.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        if matches!(trimmed, ".env" | "/.env" | ".env*" | "/.env*") {
            return Ok(false);
        }
    }

    let mut next = current;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(".env\n");
    std::fs::write(&path, next)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmpdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn appends_to_empty_gitignore() {
        let dir = tmpdir();
        let changed = ensure_env_gitignored(dir.path()).unwrap();
        assert!(changed);
        let content = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(content.contains(".env"));
    }

    #[test]
    fn idempotent_when_already_present() {
        let dir = tmpdir();
        fs::write(dir.path().join(".gitignore"), ".env\n").unwrap();
        let changed = ensure_env_gitignored(dir.path()).unwrap();
        assert!(!changed);
    }

    #[test]
    fn recognizes_slash_dot_env_form() {
        let dir = tmpdir();
        fs::write(dir.path().join(".gitignore"), "/.env\n").unwrap();
        let changed = ensure_env_gitignored(dir.path()).unwrap();
        assert!(!changed);
    }

    #[test]
    fn recognizes_dot_env_star_form() {
        let dir = tmpdir();
        fs::write(dir.path().join(".gitignore"), ".env*\n").unwrap();
        let changed = ensure_env_gitignored(dir.path()).unwrap();
        assert!(!changed);
    }

    #[test]
    fn appends_with_trailing_newline_to_existing() {
        let dir = tmpdir();
        fs::write(dir.path().join(".gitignore"), "node_modules\ntarget").unwrap();
        let changed = ensure_env_gitignored(dir.path()).unwrap();
        assert!(changed);
        let content = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(content.contains("node_modules"));
        assert!(content.contains("target"));
        assert!(content.ends_with('\n'));
        assert!(content.contains(".env"));
    }
}
