//! Per-agent hermes profile management.
//!
//! Each gitim agent is paired 1:1 with a hermes profile at
//! `~/.hermes/profiles/gitim-<handler>/`. This module owns the naming
//! convention, path resolution, and shell-out wrappers around the
//! `hermes profile create / delete` CLI.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum HermesProfileError {
    #[error("home directory not found")]
    HomeDirNotFound,

    #[error("hermes CLI not found in PATH; install hermes or run `hermes setup` first")]
    CliNotFound,

    #[error("{0}")]
    Other(String),
}

/// Returns the hermes profile name for a given agent handler.
/// Profile name format is `gitim-<handler>`.
pub fn profile_name(handler: &str) -> String {
    format!("gitim-{handler}")
}

/// Returns the hermes profile directory path for a given agent handler.
/// Returns `<home>/.hermes/profiles/gitim-<handler>`.
pub fn profile_dir(handler: &str) -> Result<PathBuf, HermesProfileError> {
    let home = dirs::home_dir().ok_or(HermesProfileError::HomeDirNotFound)?;
    Ok(home.join(".hermes/profiles").join(profile_name(handler)))
}

/// Returns true when the user's default hermes profile (the source of
/// `--clone` for new agent profiles) appears to have been set up — i.e.
/// at least one of `.env` (API keys) or `auth.json` (OAuth state) is
/// present. False when the user has installed hermes but never run
/// `hermes setup`, in which case cloning would yield an unusable agent.
///
/// Respects `HERMES_HOME` env override; falls back to `~/.hermes`.
pub fn default_profile_ready() -> bool {
    let home = std::env::var_os("HERMES_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".hermes")));

    match home {
        Some(h) => h.join(".env").is_file() || h.join("auth.json").is_file(),
        None => false,
    }
}

/// Outcome of an `ensure_profile` call.
#[derive(Debug, PartialEq, Eq)]
pub enum EnsureOutcome {
    /// A new profile was created (and bundled skills synced).
    Created,
    /// The profile already existed; nothing was changed.
    AlreadyExists,
}

/// Idempotently create a hermes profile for `handler` by clone-from-active.
///
/// Calls `hermes profile create gitim-<handler> --clone --no-alias`. The
/// `--clone` flag copies `config.yaml` / `.env` / `SOUL.md` / `memories/`
/// from the user's currently active profile (typically `~/.hermes`) so the
/// new agent inherits LLM provider configuration without manual setup.
/// `--no-alias` skips wrapper-script creation under `~/.local/bin/`.
pub async fn ensure_profile(handler: &str) -> Result<EnsureOutcome, HermesProfileError> {
    ensure_profile_with(handler, "hermes").await
}

/// Same as [`ensure_profile`] but with a configurable hermes binary path
/// (used by tests to inject a non-existent path and verify `CliNotFound`).
pub async fn ensure_profile_with(
    handler: &str,
    bin: &str,
) -> Result<EnsureOutcome, HermesProfileError> {
    let name = profile_name(handler);
    let output = tokio::process::Command::new(bin)
        .args(["profile", "create", &name, "--clone", "--no-alias"])
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                HermesProfileError::CliNotFound
            } else {
                HermesProfileError::Other(format!("spawn {bin}: {e}"))
            }
        })?;

    if output.status.success() {
        return Ok(EnsureOutcome::Created);
    }

    let combined = combined_output(&output.stdout, &output.stderr);
    if combined.contains("already exists") {
        Ok(EnsureOutcome::AlreadyExists)
    } else {
        Err(HermesProfileError::Other(format!(
            "hermes profile create failed: {}",
            combined.trim()
        )))
    }
}

fn combined_output(stdout: &[u8], stderr: &[u8]) -> String {
    let mut s = String::from_utf8_lossy(stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(stderr));
    s
}

/// Best-effort delete of the agent's hermes profile.
///
/// Calls `hermes profile delete gitim-<handler> -y`. Idempotent: returns
/// `Ok(())` when the profile is already gone. When the hermes CLI itself is
/// missing (user uninstalled hermes but agents remain), logs a warning and
/// returns `Ok(())` so this never blocks `hard_delete_agent` callers.
pub async fn delete_profile(handler: &str) -> Result<(), HermesProfileError> {
    delete_profile_with(handler, "hermes").await
}

/// Same as [`delete_profile`] but with a configurable hermes binary path.
pub async fn delete_profile_with(
    handler: &str,
    bin: &str,
) -> Result<(), HermesProfileError> {
    let name = profile_name(handler);
    let output = match tokio::process::Command::new(bin)
        .args(["profile", "delete", &name, "-y"])
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::warn!(
                "hermes CLI not found while deleting profile {name}; \
                 leaving profile dir on disk"
            );
            return Ok(());
        }
        Err(e) => {
            return Err(HermesProfileError::Other(format!("spawn {bin}: {e}")));
        }
    };

    if output.status.success() {
        return Ok(());
    }

    let combined = combined_output(&output.stdout, &output.stderr);
    if combined.contains("does not exist") {
        Ok(())
    } else {
        Err(HermesProfileError::Other(format!(
            "hermes profile delete failed: {}",
            combined.trim()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_name_for_alice() {
        assert_eq!(profile_name("alice"), "gitim-alice");
    }

    #[test]
    fn profile_dir_for_alice() {
        let result = profile_dir("alice").unwrap();
        let expected = dirs::home_dir()
            .unwrap()
            .join(".hermes/profiles/gitim-alice");
        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn ensure_profile_with_nonexistent_binary_returns_cli_not_found() {
        let err = ensure_profile_with("alice", "/nonexistent/binary/xyz")
            .await
            .expect_err("expected CliNotFound");
        assert!(matches!(err, HermesProfileError::CliNotFound));
    }

    #[tokio::test]
    async fn delete_profile_with_nonexistent_binary_is_ok() {
        // Best-effort: if hermes is gone (uninstalled mid-life), hard_delete
        // must still succeed instead of leaving the agent half-deleted.
        delete_profile_with("alice", "/nonexistent/binary/xyz")
            .await
            .expect("delete should be best-effort when CLI is missing");
    }

    // `default_profile_ready` reads HERMES_HOME, a process-global env var.
    // serial_test prevents these from running concurrently with each other
    // or with other tests that touch HERMES_HOME.
    mod default_profile_ready_tests {
        use super::super::default_profile_ready;
        use serial_test::serial;
        use tempfile::TempDir;

        #[test]
        #[serial(hermes_home_env)]
        fn ready_when_env_file_exists() {
            let tmp = TempDir::new().unwrap();
            std::fs::write(tmp.path().join(".env"), "FOO=bar").unwrap();
            std::env::set_var("HERMES_HOME", tmp.path());
            assert!(default_profile_ready());
            std::env::remove_var("HERMES_HOME");
        }

        #[test]
        #[serial(hermes_home_env)]
        fn ready_when_authjson_exists() {
            let tmp = TempDir::new().unwrap();
            std::fs::write(tmp.path().join("auth.json"), "{}").unwrap();
            std::env::set_var("HERMES_HOME", tmp.path());
            assert!(default_profile_ready());
            std::env::remove_var("HERMES_HOME");
        }

        #[test]
        #[serial(hermes_home_env)]
        fn not_ready_when_empty() {
            let tmp = TempDir::new().unwrap();
            std::env::set_var("HERMES_HOME", tmp.path());
            assert!(!default_profile_ready());
            std::env::remove_var("HERMES_HOME");
        }
    }
}
