use std::path::{Path, PathBuf};
use std::process::Command;

use tracing::info;

use gitim_client::{ensure_daemon_with_log, GitimClient};
use gitim_core::auth_payload::AuthPayload;
use gitim_core::config_patch::ensure_config_indexer_enabled;
use gitim_sync::url_redact::redacted_url;

use crate::daemon_log::daemon_log_path;
use crate::error::RuntimeError;

const GIT_HTTP_TIMEOUT_ARGS: &[&str] = &[
    "-c",
    "http.lowSpeedLimit=1000",
    "-c",
    "http.lowSpeedTime=10",
];

/// Read a git config key from the given directory, allowing the usual git
/// fallback through repo → global → system. Used for first-time onboard
/// where the user's global identity is a sensible default.
pub(crate) fn detect_git_config(key: &str, cwd: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["config", "--get", key])
        .current_dir(cwd)
        .output()
        .ok()?;
    if output.status.success() {
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if value.is_empty() {
            None
        } else {
            Some(value)
        }
    } else {
        None
    }
}

/// Read a git config key from the given directory's **local repo config
/// only** — does not fall back to `~/.gitconfig` or the system config.
/// Returns None if `cwd` is not a repo or the key is not set locally.
pub(crate) fn detect_git_config_local(key: &str, cwd: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["config", "--local", "--get", key])
        .current_dir(cwd)
        .output()
        .ok()?;
    if output.status.success() {
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if value.is_empty() {
            None
        } else {
            Some(value)
        }
    } else {
        None
    }
}

/// Normalise a display name into a valid handler: lowercase, spaces → hyphens.
pub(crate) fn name_to_handler(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c == ' ' { '-' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect()
}

/// Infer the `(handler, display_name)` for a local-mode workspace's human
/// daemon during recovery.
///
/// Source-of-truth order:
/// 1. `me.json` — what the daemon wrote at the previous onboard; the
///    authoritative record for this workspace.
/// 2. The human clone's **local** git config (`git config --local`) — set
///    only if someone explicitly bound a workspace-scoped identity.
/// 3. Literal `"human"` — explicit unknown.
///
/// Deliberately does **not** fall back to `~/.gitconfig`. A workspace's
/// identity must not be tied to whatever the user's global git config
/// happens to say at restart time — that lets a global-config change
/// silently re-onboard the daemon under a different handler and overwrite
/// the workspace's me.json / channel memberships.
pub(crate) fn infer_local_human_identity(human_dir: &Path) -> (String, String) {
    if let Some(identity) = read_me_json_identity(human_dir) {
        return identity;
    }
    let display_name =
        detect_git_config_local("user.name", human_dir).unwrap_or_else(|| "human".to_string());
    let h = name_to_handler(&display_name);
    let handler = if h.is_empty() { "human".to_string() } else { h };
    (handler, display_name)
}

fn read_me_json_identity(human_dir: &Path) -> Option<(String, String)> {
    let raw = std::fs::read_to_string(human_dir.join(".gitim/me.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let handler = v
        .get("handler")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())?;
    let display_name = v
        .get("display_name")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(handler);
    Some((handler.to_string(), display_name.to_string()))
}

/// Provision the human clone: clone remote → start daemon → onboard → verify.
///
/// The caller owns `remote_url` and `auth`: local mode passes the bare-repo
/// path and `AuthPayload::Git { handler, display_name, .. }`; github mode
/// passes an https URL and `AuthPayload::GitHub { token }`. Identity
/// inference lives in the daemon — runtime forwards the auth payload
/// unchanged.
///
/// Idempotent: if `.gitim-runtime/human/` already exists, skip the clone step.
pub async fn provision_human(
    workspace: &Path,
    remote_url: &str,
    git_server: &str,
    auth: AuthPayload,
) -> Result<PathBuf, RuntimeError> {
    let runtime_dir = workspace.join(".gitim-runtime");
    std::fs::create_dir_all(&runtime_dir)?;

    let human_dir = runtime_dir.join("human");

    if human_dir.exists() {
        info!("human dir exists, skipping clone");
    } else {
        let output = Command::new("git")
            .args(GIT_HTTP_TIMEOUT_ARGS)
            .args(["clone", remote_url, "human"])
            .current_dir(&runtime_dir)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(RuntimeError::GitCloneFailed(redacted_url(&stderr)));
        }
        info!("cloned remote into human/");
    }

    std::fs::create_dir_all(human_dir.join(".gitim"))?;

    // Write before daemon spawn so initialize_index sees enabled=true on first startup.
    // Daemon reads config.yaml at launch; if indexer.enabled is absent or false at that
    // moment, state.index stays None for the entire session (no hot-reload).
    ensure_config_indexer_enabled(&human_dir, true)
        .map_err(|e| RuntimeError::OnboardFailed(format!("indexer config: {e}")))?;
    info!("indexer enabled in human config");

    let root = human_dir.clone();
    let log_path = daemon_log_path(&human_dir);
    tokio::task::spawn_blocking(move || ensure_daemon_with_log(&root, &log_path))
        .await
        .map_err(|e| {
            RuntimeError::DaemonStartFailed(gitim_client::ClientError::ConnectionFailed(format!(
                "task panicked: {e}"
            )))
        })??;
    info!("human daemon running");

    let client = GitimClient::new(&human_dir);
    let onboard_resp = client
        .onboard(git_server, Some(auth), true, false, true)
        .await
        .map_err(|e| RuntimeError::OnboardFailed(e.to_string()))?;

    if !onboard_resp.ok {
        let msg = onboard_resp
            .error
            .unwrap_or_else(|| "unknown onboard error".into());
        return Err(RuntimeError::OnboardFailed(msg));
    }
    info!("human onboarded");

    client
        .status()
        .await
        .map_err(|e| RuntimeError::OnboardFailed(format!("status check failed: {e}")))?;

    Ok(human_dir)
}

#[derive(Debug)]
pub struct AgentConfig {
    pub handler: String,
    pub display_name: String,
    pub remote_url: String,
    /// Workspace-level GitHub email propagated into the agent's me.json so
    /// its commits attribute to the owner's contribution graph. `None` for
    /// local-mode workspaces or github-mode workspaces where the owner's
    /// email is private.
    pub github_email: Option<String>,
}

#[derive(Debug)]
pub struct AgentHandle {
    pub repo_root: PathBuf,
    pub handler: String,
}

/// Provision an agent directory: clone (if needed) → start daemon → onboard.
///
/// Idempotent: if the directory already exists, skips clone and re-starts daemon.
///
/// `join_general` rides through to the daemon's onboard so the caller can
/// opt the new agent out of #general auto-join. Persisted config lives in
/// AgentConfig; this is a one-shot provisioning decision so it stays as a
/// function arg rather than a struct field.
pub async fn provision_agent(
    agents_dir: &Path,
    config: &AgentConfig,
    join_general: bool,
) -> Result<AgentHandle, RuntimeError> {
    let repo_root = agents_dir.join(&config.handler);

    // Clone only if directory doesn't exist
    if repo_root.exists() {
        info!(handler = %config.handler, "directory exists, skipping clone");
    } else {
        let output = Command::new("git")
            .args(GIT_HTTP_TIMEOUT_ARGS)
            .args(["clone", &config.remote_url, &config.handler])
            .current_dir(agents_dir)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(RuntimeError::GitCloneFailed(redacted_url(&stderr)));
        }
        info!(handler = %config.handler, "cloned repo");
    }

    // Ensure .gitim/ exists (idempotent)
    std::fs::create_dir_all(repo_root.join(".gitim"))?;

    // Start daemon (idempotent — skips if already running)
    let root = repo_root.clone();
    let log_path = daemon_log_path(&repo_root);
    tokio::task::spawn_blocking(move || ensure_daemon_with_log(&root, &log_path))
        .await
        .map_err(|e| {
            RuntimeError::DaemonStartFailed(gitim_client::ClientError::ConnectionFailed(format!(
                "task panicked: {e}"
            )))
        })??;
    info!(handler = %config.handler, "daemon running");

    // Onboard (idempotent — daemon handles repeat calls).
    //
    // `github_email`, if present, rides alongside handler + display_name in
    // the git-mode auth payload. Daemon identity inference surfaces it into
    // `InferredIdentity.email`, which write_me_json persists to the agent's
    // me.json, which the daemon's commit path reads via `author_for`. The
    // chain is how workspace-owner email reaches agent commits even though
    // the agent onboards via `git` rather than `github`.
    let auth = AuthPayload::Git {
        handler: config.handler.clone(),
        display_name: config.display_name.clone(),
        github_email: config.github_email.clone(),
    };

    let client = GitimClient::new(&repo_root);
    let onboard_resp = client
        .onboard("git", Some(auth), false, false, join_general)
        .await
        .map_err(|e| RuntimeError::OnboardFailed(e.to_string()))?;

    if !onboard_resp.ok {
        let msg = onboard_resp
            .error
            .unwrap_or_else(|| "unknown onboard error".into());
        return Err(RuntimeError::OnboardFailed(msg));
    }
    info!(handler = %config.handler, "onboarded");

    // Verify daemon is responsive
    client
        .status()
        .await
        .map_err(|e| RuntimeError::OnboardFailed(format!("status check failed: {e}")))?;

    Ok(AgentHandle {
        repo_root,
        handler: config.handler.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_me_json(human_dir: &Path, body: &str) {
        let gitim = human_dir.join(".gitim");
        std::fs::create_dir_all(&gitim).unwrap();
        std::fs::write(gitim.join("me.json"), body).unwrap();
    }

    #[test]
    fn infer_uses_me_json_handler_and_display_name() {
        let tmp = tempfile::tempdir().unwrap();
        let human = tmp.path().join("human");
        std::fs::create_dir_all(&human).unwrap();
        write_me_json(
            &human,
            r#"{"handler":"flame4","display_name":"Lewis Liu","git_server":"github"}"#,
        );
        let (h, d) = infer_local_human_identity(&human);
        assert_eq!(h, "flame4");
        assert_eq!(d, "Lewis Liu");
    }

    #[test]
    fn infer_falls_back_display_name_to_handler_when_field_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let human = tmp.path().join("human");
        std::fs::create_dir_all(&human).unwrap();
        write_me_json(&human, r#"{"handler":"flame4"}"#);
        let (h, d) = infer_local_human_identity(&human);
        assert_eq!(h, "flame4");
        assert_eq!(d, "flame4");
    }

    #[test]
    fn infer_ignores_me_json_with_empty_handler() {
        let tmp = tempfile::tempdir().unwrap();
        let human = tmp.path().join("human");
        std::fs::create_dir_all(&human).unwrap();
        write_me_json(&human, r#"{"handler":"","display_name":"x"}"#);
        let (h, d) = infer_local_human_identity(&human);
        assert_eq!(h, "human");
        assert_eq!(d, "human");
    }

    #[test]
    fn infer_ignores_malformed_me_json() {
        let tmp = tempfile::tempdir().unwrap();
        let human = tmp.path().join("human");
        std::fs::create_dir_all(human.join(".gitim")).unwrap();
        std::fs::write(human.join(".gitim/me.json"), "{not json").unwrap();
        let (h, d) = infer_local_human_identity(&human);
        assert_eq!(h, "human");
        assert_eq!(d, "human");
    }

    #[test]
    fn infer_uses_human_clone_local_user_name_when_me_json_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let human = tmp.path().join("human");
        std::fs::create_dir_all(&human).unwrap();
        // Make human/ a real git repo with a workspace-scoped user.name set
        // in its local config. Recovery should pick this up while ignoring
        // whatever ~/.gitconfig says.
        for args in [
            &["init", "--quiet"][..],
            &["config", "--local", "user.name", "Workspace Owner"][..],
            // Use --local user.email too so a global override of email
            // doesn't accidentally make the test depend on host config.
            &["config", "--local", "user.email", "owner@example.test"][..],
        ] {
            Command::new("git")
                .args(args)
                .current_dir(&human)
                .output()
                .unwrap();
        }
        let (h, d) = infer_local_human_identity(&human);
        assert_eq!(d, "Workspace Owner");
        assert_eq!(h, "workspace-owner");
    }

    #[test]
    fn infer_does_not_leak_global_git_config_when_clone_has_no_local_name() {
        // The whole point of detect_git_config_local: if the human clone has
        // no workspace-scoped user.name, we MUST NOT silently fall back to
        // ~/.gitconfig. Otherwise a global-config change can shift this
        // workspace's identity at the next restart.
        let tmp = tempfile::tempdir().unwrap();
        let human = tmp.path().join("human");
        std::fs::create_dir_all(&human).unwrap();
        // Real git repo but no local user.name set; any value from the
        // host's ~/.gitconfig would leak in if we used `--get` without
        // `--local`.
        Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&human)
            .output()
            .unwrap();
        let (h, d) = infer_local_human_identity(&human);
        assert_eq!(h, "human");
        assert_eq!(d, "human");
    }
}
