use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use tracing::info;

use gitim_client::{ensure_daemon, GitimClient};
use gitim_sync::url_redact::redacted_url;

use crate::error::RuntimeError;

/// Read a git config key from the given directory.
/// Returns None if the key is not set.
pub(crate) fn detect_git_config(key: &str, cwd: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["config", "--get", key])
        .current_dir(cwd)
        .output()
        .ok()?;
    if output.status.success() {
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if value.is_empty() { None } else { Some(value) }
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

/// Provision the human clone: clone remote → start daemon → onboard → verify.
///
/// The caller owns `remote_url` and `auth`: local mode passes the bare-repo
/// path and `{type:"git", handler, display_name}`; github mode will pass an
/// https URL and `{type:"github", token}`. Identity inference lives in the
/// daemon — runtime forwards the auth payload unchanged.
///
/// Idempotent: if `.gitim-runtime/human/` already exists, skip the clone step.
pub async fn provision_human(
    workspace: &Path,
    remote_url: &str,
    git_server: &str,
    auth: serde_json::Value,
) -> Result<PathBuf, RuntimeError> {
    let runtime_dir = workspace.join(".gitim-runtime");
    std::fs::create_dir_all(&runtime_dir)?;

    let human_dir = runtime_dir.join("human");

    if human_dir.exists() {
        info!("human dir exists, skipping clone");
    } else {
        let output = Command::new("git")
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

    let root = human_dir.clone();
    tokio::task::spawn_blocking(move || ensure_daemon(&root))
        .await
        .map_err(|e| RuntimeError::DaemonStartFailed(
            gitim_client::ClientError::ConnectionFailed(format!("task panicked: {e}"))
        ))??;
    info!("human daemon running");

    let client = GitimClient::new(&human_dir);
    let onboard_resp = client
        .onboard(git_server, auth, true, false)
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
}

#[derive(Debug)]
pub struct AgentHandle {
    pub repo_root: PathBuf,
    pub handler: String,
}

/// Provision an agent directory: clone (if needed) → start daemon → onboard.
///
/// Idempotent: if the directory already exists, skips clone and re-starts daemon.
pub async fn provision_agent(
    agents_dir: &Path,
    config: &AgentConfig,
) -> Result<AgentHandle, RuntimeError> {
    let repo_root = agents_dir.join(&config.handler);

    // Clone only if directory doesn't exist
    if repo_root.exists() {
        info!(handler = %config.handler, "directory exists, skipping clone");
    } else {
        let output = Command::new("git")
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
    tokio::task::spawn_blocking(move || ensure_daemon(&root))
        .await
        .map_err(|e| RuntimeError::DaemonStartFailed(
            gitim_client::ClientError::ConnectionFailed(format!("task panicked: {e}"))
        ))??;
    info!(handler = %config.handler, "daemon running");

    // Onboard (idempotent — daemon handles repeat calls)
    let client = GitimClient::new(&repo_root);
    let onboard_resp = client
        .onboard(
            "git",
            json!({
                "type": "git",
                "handler": config.handler,
                "display_name": config.display_name,
            }),
            false,
            false,
        )
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
