//! Conventions for where a runtime-managed daemon writes its log.
//!
//! All runtime logs live under `~/.gitim/logs/`. Each per-workspace daemon
//! gets its own file named `<workspace>-<handler>.log` so tailing one
//! daemon's output never pulls in another agent's lines. The runtime's own
//! shell log is `runtime.log` in the same directory.
//!
//! Workspace and handler are derived from the daemon's `repo_root`. The
//! runtime lays daemons out at `<workspace>/.gitim-runtime/<handler>/`, so
//! the workspace label is the grandparent's basename and the handler is the
//! directory's own basename. For the human clone the directory is always
//! `human`, which means the human daemon's log is `<workspace>-human.log`
//! regardless of the onboarded handle — deterministic at spawn time (before
//! the daemon has resolved identity).

use std::path::{Path, PathBuf};

/// Root directory for all runtime-owned logs. Created lazily by the callers
/// that actually write (daemonize / ensure_daemon_with_log).
pub fn logs_dir() -> PathBuf {
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
/// "unknown" for either component if the path doesn't match the expected
/// layout — a best-effort label, never an error.
pub fn daemon_log_path(repo_root: &Path) -> PathBuf {
    let handler = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    let workspace = repo_root
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    logs_dir().join(format!("{workspace}-{handler}.log"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_log_uses_workspace_and_handler() {
        let p = daemon_log_path(Path::new("/tmp/d1/.gitim-runtime/codex-01"));
        assert_eq!(p.file_name().unwrap(), "d1-codex-01.log");
    }

    #[test]
    fn human_daemon_log_uses_human_suffix() {
        let p = daemon_log_path(Path::new("/tmp/d1/.gitim-runtime/human"));
        assert_eq!(p.file_name().unwrap(), "d1-human.log");
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
}
