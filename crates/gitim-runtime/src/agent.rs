use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use tracing::info;

use gitim_client::{ensure_daemon, GitimClient};

use crate::error::RuntimeError;

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
            return Err(RuntimeError::GitCloneFailed(stderr.to_string()));
        }
        info!(handler = %config.handler, "cloned repo");
    }

    // Ensure .gitim/ exists (idempotent)
    std::fs::create_dir_all(repo_root.join(".gitim"))?;

    // Start daemon (idempotent — skips if already running)
    let root = repo_root.clone();
    tokio::task::spawn_blocking(move || ensure_daemon(&root)).await.unwrap()?;
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
