use std::path::Path;

use serde_json::Value;

use crate::git_config::{ConfigError, GitProvider, WorkspaceConfig};
use crate::github::{fetch_user_email, GithubError};

pub const GITHUB_API_BASE: &str = "https://api.github.com";

#[derive(Debug, thiserror::Error)]
pub enum PropagationError {
    #[error("config read: {0}")]
    Config(#[from] ConfigError),
    #[error("github api: {0}")]
    Github(#[from] GithubError),
}

// Ride-along with token_propagation: some clones were provisioned before the
// email feature shipped (or before the user granted a PAT with email scope),
// so config.json + every clone's `.gitim/me.json` may be missing
// `github_email`. Without it, daemons fall back to `<handler>@gitim` and the
// commits don't count toward the owner's contribution graph. Backfilling at
// startup fixes the owner's intent without requiring workspace re-init.
//
// Caveat: existing agent daemons only read me.json at process start. A
// restart is still required for in-flight daemons to pick up the new value.
pub async fn backfill_github_email(
    workspace: &Path,
    api_base: &str,
) -> Result<bool, PropagationError> {
    let mut config = WorkspaceConfig::read(workspace)?;
    if config.git.provider != GitProvider::Github {
        return Ok(false);
    }
    if config.git.github_email.is_some() {
        return Ok(false);
    }
    let Some(token) = config.git.token.clone().filter(|t| !t.is_empty()) else {
        return Ok(false);
    };
    let email = match fetch_user_email(&token, api_base).await? {
        Some(e) => e,
        None => return Ok(false),
    };

    config.git.github_email = Some(email.clone());
    config.write(workspace).map_err(PropagationError::Config)?;

    merge_email_into_clone(&workspace.join(".gitim-runtime").join("human"), &email);
    if let Ok(entries) = std::fs::read_dir(workspace) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == ".gitim-runtime" {
                continue;
            }
            merge_email_into_clone(&path, &email);
        }
    }
    tracing::info!(
        workspace = %workspace.display(),
        "email_propagation: backfilled github_email (agent daemons need a restart to pick it up)",
    );
    Ok(true)
}

fn merge_email_into_clone(clone_dir: &Path, email: &str) {
    let me_path = clone_dir.join(".gitim").join("me.json");
    if !me_path.exists() {
        return;
    }
    let content = match std::fs::read_to_string(&me_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                dir = %clone_dir.display(),
                error = %e,
                "email_propagation: read me.json failed",
            );
            return;
        }
    };
    let mut value: Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                dir = %clone_dir.display(),
                error = %e,
                "email_propagation: parse me.json failed",
            );
            return;
        }
    };
    // Idempotent: skip the write if nothing changes. Keeps this cheap to
    // re-run on every runtime boot.
    if value.get("github_email").and_then(|v| v.as_str()) == Some(email) {
        return;
    }
    let Some(obj) = value.as_object_mut() else {
        return;
    };
    obj.insert("github_email".to_string(), Value::String(email.to_string()));
    let serialized = match serde_json::to_string_pretty(&value) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                dir = %clone_dir.display(),
                error = %e,
                "email_propagation: serialize me.json failed",
            );
            return;
        }
    };
    if let Err(e) = std::fs::write(&me_path, serialized) {
        tracing::warn!(
            dir = %clone_dir.display(),
            error = %e,
            "email_propagation: write me.json failed",
        );
    }
}
