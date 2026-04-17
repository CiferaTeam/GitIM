use std::path::Path;
use std::process::Command;

use crate::git_config::{ConfigError, GitProvider, WorkspaceConfig};
use crate::github::parse_github_url;
use crate::http::build_token_url;

#[derive(Debug, thiserror::Error)]
pub enum PropagationError {
    #[error("config read: {0}")]
    Config(#[from] ConfigError),
}

// config.json is the token source of truth; every clone's `.git/config`
// mirrors it. Stale URLs silently burn auth quota, so we resync on startup
// and after provisioning rather than trusting that nothing drifted.
pub fn propagate_token(workspace: &Path) -> Result<(), PropagationError> {
    let config = WorkspaceConfig::read(workspace)?;
    if config.git.provider != GitProvider::Github {
        return Ok(());
    }

    let Some(remote_url) = config.git.remote_url.as_deref().filter(|u| !u.is_empty()) else {
        return Ok(());
    };
    let Some(token) = config.git.token.as_deref().filter(|t| !t.is_empty()) else {
        return Ok(());
    };

    let Ok((owner, repo)) = parse_github_url(remote_url) else {
        return Ok(());
    };

    let new_url = build_token_url(&owner, &repo, token);

    propagate_to(&workspace.join(".gitim-runtime").join("human"), &new_url);

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
            propagate_to(&path, &new_url);
        }
    }

    Ok(())
}

fn propagate_to(clone_dir: &Path, new_url: &str) {
    if !clone_dir.join(".git").exists() {
        return;
    }
    let result = Command::new("git")
        .arg("-C")
        .arg(clone_dir)
        .args(["config", "remote.origin.url", new_url])
        .output();
    match result {
        Ok(out) if !out.status.success() => {
            // new_url carries the token; log stderr only, never the url.
            tracing::warn!(
                dir = %clone_dir.display(),
                stderr = %String::from_utf8_lossy(&out.stderr).trim(),
                "propagate_token: git config returned non-zero",
            );
        }
        Err(e) => {
            tracing::warn!(
                dir = %clone_dir.display(),
                error = %e,
                "propagate_token: git config failed to spawn",
            );
        }
        Ok(_) => {}
    }
}
