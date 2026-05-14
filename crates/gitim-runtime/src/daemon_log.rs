//! Conventions for where a runtime-managed daemon writes its log.
//!
//! All per-daemon logs live under `~/.gitim/logs/` so a single `tail -f`
//! over the directory surfaces every agent at once. Each daemon's file is
//! named `<workspace>-<handler>.log` to keep them distinguishable. The
//! runtime's own shell log shares the directory at `runtime.log`.
//!
//! Workspace and handler are derived from the daemon's repo root, which
//! takes one of two layouts:
//!   - `<workspace>/.gitim-runtime/human/` for the human clone (handler =
//!     "human")
//!   - `<workspace>/<handler>/`            for each agent clone
//!
//! Tests opt out of writing to the real `~/.gitim/logs/` by setting
//! `GITIM_LOG_DIR` to a TempDir; see `tests/common/mod.rs`. Production
//! must not set this env — leaving it unset is what gives users a single
//! grep-able directory across daemons.
//!
//! The "human" name for the human daemon's handler comes from the
//! directory it's cloned into, not from any identity resolution. That's
//! intentional: spawn time happens before the daemon has read its
//! `me.json`, so this label has to be derivable from the path alone.

use std::path::{Path, PathBuf};

/// Root directory for all runtime-owned logs. Production: `~/.gitim/logs/`.
/// Test infra sets `GITIM_LOG_DIR` to redirect into a TempDir.
pub fn logs_dir() -> PathBuf {
    if let Some(over) = std::env::var_os("GITIM_LOG_DIR") {
        return PathBuf::from(over);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".gitim")
        .join("logs")
}

/// Absolute path to the runtime shell's own log.
pub fn runtime_log_path() -> PathBuf {
    logs_dir().join("runtime.log")
}

/// Derive the log path for the daemon serving `repo_root`. Falls back to
/// "unknown" for either component when the path doesn't match a known
/// layout — a best-effort label, never an error.
pub fn daemon_log_path(repo_root: &Path) -> PathBuf {
    let handler = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    // Two layouts to recognize:
    //   <workspace>/.gitim-runtime/<handler> → workspace is two levels up
    //   <workspace>/<handler>                → workspace is the parent
    // Anything else falls back to "unknown".
    let workspace = repo_root
        .parent()
        .and_then(|parent| {
            if parent.file_name().and_then(|n| n.to_str()) == Some(".gitim-runtime") {
                parent.parent()
            } else {
                Some(parent)
            }
        })
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    logs_dir().join(format!("{workspace}-{handler}.log"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_daemon_log_uses_workspace_grandparent() {
        let p = daemon_log_path(Path::new("/tmp/d1/.gitim-runtime/human"));
        assert_eq!(p.file_name().unwrap(), "d1-human.log");
    }

    #[test]
    fn agent_daemon_log_uses_workspace_parent() {
        let p = daemon_log_path(Path::new("/tmp/d1/codex-01"));
        assert_eq!(p.file_name().unwrap(), "d1-codex-01.log");
    }

    #[test]
    fn log_path_falls_back_on_unexpected_layout() {
        let p = daemon_log_path(Path::new("/just-a-repo"));
        assert_eq!(p.file_name().unwrap(), "unknown-just-a-repo.log");
    }

    #[test]
    fn runtime_log_path_sits_in_logs_dir() {
        let p = runtime_log_path();
        assert!(p.ends_with("logs/runtime.log"));
    }

    #[test]
    fn env_override_redirects_logs_dir() {
        // Serial guard would be ideal but this test runs in isolation
        // (no other test mutates GITIM_LOG_DIR at the same nesting); use
        // a unique-enough sentinel to detect cross-test leakage.
        let sentinel = std::env::temp_dir().join("gitim-daemon-log-override-test");
        std::env::set_var("GITIM_LOG_DIR", &sentinel);
        let got = logs_dir();
        std::env::remove_var("GITIM_LOG_DIR");
        assert_eq!(got, sentinel);
    }
}
