use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GitProvider {
    Local,
    Github,
}

impl Default for GitProvider {
    fn default() -> Self {
        GitProvider::Local
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GitConfig {
    #[serde(default)]
    pub provider: GitProvider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    pub workspace: String,
    pub created_at: String,
    #[serde(default)]
    pub git: GitConfig,
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspacePathError {
    #[error("workspace is inside a cloud-sync directory: {0}")]
    CloudSyncDetected(String),
}

const CLOUD_SYNC_PREFIXES: &[(&str, &str)] = &[
    ("Library/Mobile Documents", "iCloud Drive"),
    ("Dropbox", "Dropbox"),
    ("Google Drive", "Google Drive"),
    ("OneDrive", "OneDrive"),
];

// Resolve to a canonical path if possible; otherwise fall back to the canonical
// parent plus the literal filename. Matters for pre-existing symlinks and `..`
// segments — lexical `starts_with` misses both.
fn canonicalize_or_parent(path: &Path) -> PathBuf {
    if let Ok(p) = path.canonicalize() {
        return p;
    }
    if let Some(parent) = path.parent() {
        if let Ok(canon_parent) = parent.canonicalize() {
            if let Some(name) = path.file_name() {
                return canon_parent.join(name);
            }
            return canon_parent;
        }
    }
    path.to_path_buf()
}

pub fn validate_workspace_path(path: &Path, home: &Path) -> Result<(), WorkspacePathError> {
    let canon_path = canonicalize_or_parent(path);
    let canon_home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    for (suffix, service) in CLOUD_SYNC_PREFIXES {
        let blacklisted = canon_home.join(suffix);
        let canon_blacklisted = blacklisted
            .canonicalize()
            .unwrap_or_else(|_| blacklisted.clone());
        if canon_path.starts_with(&canon_blacklisted) {
            return Err(WorkspacePathError::CloudSyncDetected((*service).to_string()));
        }
    }
    Ok(())
}

pub fn validate_workspace_path_from_env(path: &Path) -> Result<(), WorkspacePathError> {
    // `dirs::home_dir` is cross-platform — critically, it reads `USERPROFILE`
    // on Windows where `HOME` is usually unset, so OneDrive detection actually
    // fires there.
    let Some(home) = dirs::home_dir() else {
        return Ok(());
    };
    validate_workspace_path(path, &home)
}

#[cfg(target_os = "macos")]
pub fn mark_excluded_from_backups(dir: &Path) -> std::io::Result<()> {
    // Time Machine recognises this xattr regardless of payload format, but a text
    // plist keeps the bytes auditable by `xattr -p`.
    const XATTR_KEY: &str = "com.apple.metadata:com_apple_backup_excludeItem";
    const PAYLOAD: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><true/></plist>"#;
    xattr::set(dir, XATTR_KEY, PAYLOAD)
}

#[cfg(not(target_os = "macos"))]
pub fn mark_excluded_from_backups(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("config not found at {0}")]
    NotFound(String),

    #[error("unsupported platform for github mode")]
    UnsupportedPlatform,

    #[error(transparent)]
    InvalidPath(#[from] WorkspacePathError),
}

fn config_dir(workspace: &Path) -> PathBuf {
    workspace.join(".gitim-runtime")
}

fn config_path(workspace: &Path) -> PathBuf {
    config_dir(workspace).join("config.json")
}

// Windows cannot enforce chmod-style perms on the token file, so Github mode
// is refused up front rather than silently writing a world-readable secret.
// Keep the platform gate as conditional compilation (matches the style used
// for `mark_excluded_from_backups`).
#[cfg(windows)]
fn check_platform_supports(provider: GitProvider) -> Result<(), ConfigError> {
    if provider == GitProvider::Github {
        return Err(ConfigError::UnsupportedPlatform);
    }
    Ok(())
}

#[cfg(not(windows))]
fn check_platform_supports(_: GitProvider) -> Result<(), ConfigError> {
    Ok(())
}

impl WorkspaceConfig {
    pub fn read(workspace: &Path) -> Result<Self, ConfigError> {
        let path = config_path(workspace);
        if !path.exists() {
            return Err(ConfigError::NotFound(path.to_string_lossy().into_owned()));
        }
        let content = std::fs::read_to_string(&path)?;
        let cfg: Self = serde_json::from_str(&content)?;
        Ok(cfg)
    }

    pub fn write(&self, workspace: &Path) -> Result<(), ConfigError> {
        check_platform_supports(self.git.provider)?;

        let dir = config_dir(workspace);
        std::fs::create_dir_all(&dir)?;

        let final_path = config_path(workspace);
        let tmp_path = dir.join("config.json.tmp");

        // Any leftover tmp from a previous crashed write would otherwise linger
        // until the next successful rename shadowed it — and it may contain a
        // token. Best-effort unlink; errors here (including NotFound) are fine.
        let _ = std::fs::remove_file(&tmp_path);

        let serialized = serde_json::to_string_pretty(self)?;
        std::fs::write(&tmp_path, serialized)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&tmp_path, perms)?;
        }

        std::fs::rename(&tmp_path, &final_path)?;
        Ok(())
    }
}
