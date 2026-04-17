use gitim_runtime::git_config::{
    validate_workspace_path, ConfigError, GitConfig, GitProvider, WorkspaceConfig,
    WorkspacePathError,
};
use std::path::PathBuf;

#[test]
fn local_mode_roundtrip() {
    let cfg = WorkspaceConfig {
        workspace: "/tmp/ws".to_string(),
        created_at: "2026-04-17T00:00:00Z".to_string(),
        git: GitConfig {
            provider: GitProvider::Local,
            remote_url: None,
            token: None,
        },
    };
    let json = serde_json::to_string(&cfg).unwrap();
    let back: WorkspaceConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(cfg, back);
}

#[test]
fn github_mode_roundtrip() {
    let cfg = WorkspaceConfig {
        workspace: "/tmp/ws".to_string(),
        created_at: "2026-04-17T00:00:00Z".to_string(),
        git: GitConfig {
            provider: GitProvider::Github,
            remote_url: Some("https://github.com/owner/repo.git".to_string()),
            token: Some("ghp_example".to_string()),
        },
    };
    let json = serde_json::to_string(&cfg).unwrap();
    let back: WorkspaceConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(cfg, back);
    assert!(json.contains("\"provider\":\"github\""));
}

#[test]
fn legacy_config_without_git_field_loads_as_local() {
    let legacy = r#"{"workspace":"/tmp/ws","created_at":"2026-04-17T00:00:00Z"}"#;
    let cfg: WorkspaceConfig = serde_json::from_str(legacy).unwrap();
    assert_eq!(cfg.workspace, "/tmp/ws");
    assert_eq!(cfg.git.provider, GitProvider::Local);
    assert!(cfg.git.remote_url.is_none());
    assert!(cfg.git.token.is_none());
}

#[cfg(unix)]
#[test]
fn write_config_sets_0600_perms() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let cfg = WorkspaceConfig {
        workspace: dir.path().to_string_lossy().into_owned(),
        created_at: "2026-04-17T00:00:00Z".to_string(),
        git: GitConfig {
            provider: GitProvider::Github,
            remote_url: Some("https://github.com/owner/repo.git".to_string()),
            token: Some("ghp_example".to_string()),
        },
    };
    cfg.write(dir.path()).expect("write");

    let written = dir.path().join(".gitim-runtime/config.json");
    let meta = std::fs::metadata(&written).unwrap();
    let mode = meta.permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "expected 0600, got {mode:o}");
}

#[test]
fn read_config_from_nonexistent_returns_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let err = WorkspaceConfig::read(dir.path()).expect_err("should fail");
    assert!(matches!(err, ConfigError::NotFound(_)), "got {err:?}");
}

#[test]
fn read_config_from_fresh_workspace_returns_valid_struct() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = WorkspaceConfig {
        workspace: dir.path().to_string_lossy().into_owned(),
        created_at: "2026-04-17T00:00:00Z".to_string(),
        git: GitConfig::default(),
    };
    cfg.write(dir.path()).expect("write");
    let read_back = WorkspaceConfig::read(dir.path()).expect("read");
    assert_eq!(cfg, read_back);
}

#[test]
fn reject_icloud_drive_path() {
    let home = PathBuf::from("/Users/alice");
    let ws = home.join("Library/Mobile Documents/com~apple~CloudDocs/test-workspace");
    let err = validate_workspace_path(&ws, &home).expect_err("should reject");
    match err {
        WorkspacePathError::CloudSyncDetected(name) => assert_eq!(name, "iCloud Drive"),
    }
}

#[test]
fn reject_dropbox_path() {
    let home = PathBuf::from("/Users/alice");
    let ws = home.join("Dropbox/test");
    let err = validate_workspace_path(&ws, &home).expect_err("should reject");
    match err {
        WorkspacePathError::CloudSyncDetected(name) => assert_eq!(name, "Dropbox"),
    }
}

#[test]
fn reject_google_drive_path() {
    let home = PathBuf::from("/Users/alice");
    let ws = home.join("Google Drive/test");
    let err = validate_workspace_path(&ws, &home).expect_err("should reject");
    match err {
        WorkspacePathError::CloudSyncDetected(name) => assert_eq!(name, "Google Drive"),
    }
}

#[test]
fn reject_onedrive_path() {
    let home = PathBuf::from("/Users/alice");
    let ws = home.join("OneDrive/test");
    let err = validate_workspace_path(&ws, &home).expect_err("should reject");
    match err {
        WorkspacePathError::CloudSyncDetected(name) => assert_eq!(name, "OneDrive"),
    }
}

#[test]
fn accept_normal_path() {
    let home = PathBuf::from("/Users/alice");
    let ws = home.join("projects/test-workspace");
    validate_workspace_path(&ws, &home).expect("should accept");
}

#[cfg(target_os = "macos")]
#[test]
fn exclude_from_time_machine_sets_xattr() {
    use gitim_runtime::git_config::mark_excluded_from_backups;

    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join(".gitim-runtime");
    std::fs::create_dir_all(&marker).unwrap();

    mark_excluded_from_backups(&marker).expect("mark");

    let value = xattr::get(&marker, "com.apple.metadata:com_apple_backup_excludeItem")
        .expect("read xattr")
        .expect("xattr present");
    let as_text = String::from_utf8_lossy(&value);
    assert!(
        as_text.contains("<true/>") || value.starts_with(b"bplist"),
        "unexpected xattr payload: {as_text}"
    );
}
