//! Idempotent .gitignore management for workspace clones.
//!
//! Called from workspace provisioning so every agent clone inherits the rule
//! set via its shared remote. Separate module to keep workspace.rs / http.rs
//! focused on orchestration.
//!
//! ## Why we curate a default pattern set
//!
//! Every agent provider (Codex, Claude Code, opencode, aider, Cursor, ...)
//! writes its own local memory file in the daemon's cwd — AGENTS.md,
//! CLAUDE.md, .cursorrules, .aider*, etc. These are intentionally per-clone:
//! they capture *that* agent's running notes, not shared protocol.
//!
//! If one of them slips into a commit, the next time another clone tries to
//! fast-forward through that commit, git refuses to checkout because the
//! incoming tracked version would overwrite the receiving clone's own
//! untracked file. sync_loop's pull-only path has no recovery for that
//! conflict, so the clone gets stuck N commits behind origin, indefinitely.
//! Once that happens, `handle_poll`'s membership cache reads stale meta from
//! a working tree that never advanced, falls into the read-failure fallback,
//! and starts broadcasting channel changes to non-members.
//!
//! The fix is upstream of all of that: never let those files become tracked
//! in the first place. Curate them in `DEFAULT_PATTERNS` and inject at
//! /git/init time.

use std::path::Path;

/// Patterns we always want excluded from a workspace clone.
///
/// Grouped by intent; the order matches what gets appended to .gitignore when
/// a fresh workspace is initialized. Adding to this list is safe — existing
/// workspaces will pick up new entries the next time `/git/init` runs against
/// their human clone, and each pattern is idempotent.
///
/// Negation rules in the existing .gitignore (`!foo`) are respected per
/// pattern: if a user explicitly wants something tracked, we bail on that
/// pattern only, not the whole list.
pub const DEFAULT_PATTERNS: &[&str] = &[
    // Per-clone secrets (the original protected pattern).
    ".env",
    // Agent provider root-level memory files. Each provider writes its own
    // in the daemon cwd as the agent runs.
    "AGENTS.md",
    "CLAUDE.md",
    "GEMINI.md",
    "SOUL.md",
    ".cursorrules",
    ".aider*",
    // Agent provider config / session directories.
    ".claude/",
    ".codex/",
    ".opencode/",
    ".cursor/",
    // Per-agent scratch / memory space. Two parallel directories both
    // gitignored, both per-clone — they exist so agent-generated files
    // (scripts, intermediate artifacts, fetched content, debug dumps,
    // session notes) live in a stable place that never enters the shared
    // tree. Without this, every random file an agent writes risks
    // colliding with an incoming tracked file and wedging rebase.
    //
    // The agent prompt teaches the split:
    //   - `notes/` → persistent notes that survive across sessions
    //   - `workspace/` → throwaway working files for the current task
    "notes/",
    "workspace/",
    // OS / editor noise.
    ".DS_Store",
    "._*",
    "Thumbs.db",
    ".vscode/",
    ".idea/",
    "*.swp",
    "*.swo",
    "*~",
    // Language artifacts — defensive. The daemon itself only commits
    // whitelisted IM paths, but an AI agent shelling out `git add -A` in
    // a fit of helpfulness can drag these in.
    "__pycache__/",
    "*.py[cod]",
    "*.egg-info/",
    ".pytest_cache/",
    ".ruff_cache/",
    ".mypy_cache/",
    ".coverage",
    "node_modules/",
    "target/",
    "*.log",
    ".cache/",
];

/// Append any of `patterns` that aren't already covered by the clone's
/// `.gitignore`. Returns `Ok(true)` if a change was made (caller should
/// commit), `Ok(false)` if nothing was appended.
///
/// "Already covered" is line-literal: we compare against each non-comment,
/// non-blank line in the existing file, also accepting the rooted form
/// (`/pattern`) as equivalent to the unrooted form. We do not try to be
/// clever about glob equivalence — if the user wrote `**/AGENTS.md` we
/// will still append `AGENTS.md`, which is harmless (git tolerates
/// redundant lines) but cosmetically duplicative.
///
/// Negation safety: for each pattern `P`, if the .gitignore contains any
/// line of the form `!<...P...>`, we skip that pattern. This preserves the
/// user's explicit "I want this tracked" intent — same policy the original
/// `.env`-only implementation used.
pub fn ensure_patterns_gitignored(clone_root: &Path, patterns: &[&str]) -> std::io::Result<bool> {
    let path = clone_root.join(".gitignore");
    let current = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };

    let mut existing_lines: Vec<&str> = Vec::new();
    let mut negations: Vec<&str> = Vec::new();
    for line in current.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('!') {
            negations.push(rest);
        } else {
            existing_lines.push(trimmed);
        }
    }

    let mut to_append: Vec<&str> = Vec::new();
    for &pat in patterns {
        if negations.iter().any(|n| n.contains(pat)) {
            continue;
        }
        let rooted = format!("/{pat}");
        let already = existing_lines
            .iter()
            .any(|line| *line == pat || *line == rooted.as_str());
        if !already {
            to_append.push(pat);
        }
    }

    if to_append.is_empty() {
        return Ok(false);
    }

    let mut next = current;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    for pat in &to_append {
        next.push_str(pat);
        next.push('\n');
    }
    std::fs::write(&path, next)?;
    Ok(true)
}

/// Convenience wrapper that applies `DEFAULT_PATTERNS`. Called from
/// `/git/init` so every freshly provisioned workspace inherits the full
/// curated set; existing workspaces pick up new entries the next time a
/// daemon-side init runs against the human clone.
pub fn ensure_defaults_gitignored(clone_root: &Path) -> std::io::Result<bool> {
    ensure_patterns_gitignored(clone_root, DEFAULT_PATTERNS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmpdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn appends_all_defaults_to_empty_gitignore() {
        let dir = tmpdir();
        let changed = ensure_defaults_gitignored(dir.path()).unwrap();
        assert!(changed);
        let content = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        for pat in DEFAULT_PATTERNS {
            assert!(
                content.lines().any(|l| l.trim() == *pat),
                "expected pattern {pat} in:\n{content}"
            );
        }
        assert!(content.ends_with('\n'));
    }

    #[test]
    fn idempotent_when_all_present() {
        let dir = tmpdir();
        let first = ensure_defaults_gitignored(dir.path()).unwrap();
        assert!(first);
        let after_first = fs::read_to_string(dir.path().join(".gitignore")).unwrap();

        let second = ensure_defaults_gitignored(dir.path()).unwrap();
        assert!(!second, "second call must be a no-op");
        let after_second = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert_eq!(
            after_first, after_second,
            "no-op call must not modify the file"
        );
    }

    #[test]
    fn partial_existing_appends_only_missing() {
        let dir = tmpdir();
        fs::write(dir.path().join(".gitignore"), ".env\nAGENTS.md\n").unwrap();
        let changed = ensure_defaults_gitignored(dir.path()).unwrap();
        assert!(changed);
        let content = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        // .env and AGENTS.md only appear once each
        assert_eq!(content.lines().filter(|l| l.trim() == ".env").count(), 1);
        assert_eq!(
            content.lines().filter(|l| l.trim() == "AGENTS.md").count(),
            1
        );
        // Other defaults got appended
        assert!(content.lines().any(|l| l.trim() == "CLAUDE.md"));
        assert!(content.lines().any(|l| l.trim() == "notes/"));
    }

    #[test]
    fn recognizes_rooted_form_as_equivalent() {
        let dir = tmpdir();
        fs::write(dir.path().join(".gitignore"), "/AGENTS.md\n/.env\n").unwrap();
        let changed = ensure_patterns_gitignored(dir.path(), &["AGENTS.md", ".env"]).unwrap();
        assert!(
            !changed,
            "rooted form /pattern must count as covering pattern"
        );
    }

    #[test]
    fn negation_per_pattern_is_respected() {
        let dir = tmpdir();
        // User explicitly wants AGENTS.md tracked. They don't have an opinion
        // on the other defaults.
        fs::write(dir.path().join(".gitignore"), "*\n!AGENTS.md\n").unwrap();
        let changed = ensure_patterns_gitignored(dir.path(), &["AGENTS.md", "CLAUDE.md"]).unwrap();
        assert!(changed, "CLAUDE.md should still get appended");
        let content = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(content.lines().any(|l| l.trim() == "CLAUDE.md"));
        // AGENTS.md must NOT have been re-added (would conflict with !AGENTS.md)
        assert_eq!(
            content.lines().filter(|l| l.trim() == "AGENTS.md").count(),
            0,
            "must not append AGENTS.md when !AGENTS.md is present"
        );
    }

    #[test]
    fn appends_with_trailing_newline_to_unterminated_file() {
        let dir = tmpdir();
        fs::write(dir.path().join(".gitignore"), "node_modules\ntarget").unwrap();
        let changed = ensure_patterns_gitignored(dir.path(), &[".env"]).unwrap();
        assert!(changed);
        let content = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(content.starts_with("node_modules\ntarget\n"));
        assert!(content.ends_with(".env\n"));
    }

    #[test]
    fn comments_and_blank_lines_dont_block_append() {
        let dir = tmpdir();
        fs::write(
            dir.path().join(".gitignore"),
            "# project ignores\n\n# secrets\n",
        )
        .unwrap();
        let changed = ensure_patterns_gitignored(dir.path(), &[".env"]).unwrap();
        assert!(changed);
        let content = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(content.contains(".env"));
    }
}
