#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use gitim_runtime::git_config::{
    validate_workspace_path, ConfigError, GitConfig, GitProvider, WorkspaceConfig,
    WorkspacePathError,
};
use gitim_runtime::user_config::{self, UserConfig};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const MUTATION_CHILD_PATH: &str = "GITIM_CONFIG_MUTATION_CHILD_PATH";
const MUTATION_CHILD_FIELD: &str = "GITIM_CONFIG_MUTATION_CHILD_FIELD";
const MUTATION_CHILD_MARKER: &str = "GITIM_CONFIG_MUTATION_CHILD_MARKER";

#[test]
fn runtime_config_mutation_child() {
    let Ok(path) = std::env::var(MUTATION_CHILD_PATH) else {
        return;
    };
    let field = std::env::var(MUTATION_CHILD_FIELD).expect("child mutation field");
    let marker = std::env::var(MUTATION_CHILD_MARKER).expect("child marker path");

    user_config::mutate_at(Path::new(&path), |cfg| match field.as_str() {
        "runtime_id" => {
            cfg.runtime_id = "11111111-1111-4111-8111-111111111111".to_string();
            std::fs::write(marker, b"ready").expect("write child marker");
            std::thread::sleep(Duration::from_millis(500));
        }
        "listen_port" => cfg.listen_port = Some(19090),
        other => panic!("unknown child mutation field: {other}"),
    })
    .expect("child config mutation");
}

#[test]
fn legacy_fleet_node_without_runtime_id_roundtrips_omitted() {
    let legacy = r#"{
        "fleet_nodes": [{
            "node_id": "studio",
            "base_url": "http://127.0.0.1:16868",
            "workspaces": ["room"]
        }]
    }"#;

    let config: UserConfig = serde_json::from_str(legacy).expect("legacy config");
    assert_eq!(config.fleet_nodes[0].runtime_id, None);

    let roundtrip = serde_json::to_value(config).expect("serialize config");
    assert!(roundtrip["fleet_nodes"][0].get("runtime_id").is_none());
}

#[test]
fn concurrent_thread_mutations_preserve_both_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.json");
    user_config::write_to(&UserConfig::default(), &path).unwrap();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();

    std::thread::scope(|scope| {
        let first_path = path.clone();
        let first = scope.spawn(move || {
            user_config::mutate_at(&first_path, |cfg| {
                cfg.runtime_id = "22222222-2222-4222-8222-222222222222".to_string();
                entered_tx.send(()).unwrap();
                std::thread::sleep(Duration::from_millis(250));
            })
            .unwrap();
        });
        entered_rx.recv().unwrap();
        let second_path = path.clone();
        let second = scope.spawn(move || {
            user_config::mutate_at(&second_path, |cfg| cfg.listen_port = Some(18080)).unwrap();
        });
        first.join().unwrap();
        second.join().unwrap();
    });

    let config = user_config::read_from(Some(&path));
    assert_eq!(config.runtime_id, "22222222-2222-4222-8222-222222222222");
    assert_eq!(config.listen_port, Some(18080));
}

#[test]
fn concurrent_process_mutations_preserve_both_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.json");
    let marker = dir.path().join("first-entered");
    user_config::write_to(&UserConfig::default(), &path).unwrap();
    let current_test = std::env::current_exe().expect("current test binary");

    let mut first = Command::new(&current_test)
        .args(["--exact", "runtime_config_mutation_child", "--nocapture"])
        .env(MUTATION_CHILD_PATH, &path)
        .env(MUTATION_CHILD_FIELD, "runtime_id")
        .env(MUTATION_CHILD_MARKER, &marker)
        .spawn()
        .expect("spawn first mutation child");

    let deadline = Instant::now() + Duration::from_secs(3);
    while !marker.exists() {
        assert!(
            Instant::now() < deadline,
            "first mutation child did not enter closure"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let second = Command::new(&current_test)
        .args(["--exact", "runtime_config_mutation_child", "--nocapture"])
        .env(MUTATION_CHILD_PATH, &path)
        .env(MUTATION_CHILD_FIELD, "listen_port")
        .env(MUTATION_CHILD_MARKER, dir.path().join("unused"))
        .status()
        .expect("run second mutation child");
    assert!(second.success(), "second mutation child failed: {second}");

    let first_status = first.wait().expect("wait for first mutation child");
    assert!(
        first_status.success(),
        "first mutation child failed: {first_status}"
    );

    let config = user_config::read_from(Some(&path));
    assert_eq!(config.runtime_id, "11111111-1111-4111-8111-111111111111");
    assert_eq!(config.listen_port, Some(19090));
}

#[cfg(unix)]
#[test]
fn atomic_runtime_config_write_keeps_valid_json_and_0600_mode() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.json");
    user_config::mutate_at(&path, |cfg| {
        cfg.runtime_id = "33333333-3333-4333-8333-333333333333".to_string();
        cfg.listen_port = Some(16868);
    })
    .unwrap();

    let bytes = std::fs::read(&path).unwrap();
    let parsed: UserConfig = serde_json::from_slice(&bytes).expect("valid runtime config JSON");
    assert_eq!(parsed.listen_port, Some(16868));
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn mutation_rejects_corrupt_existing_config_without_replacing_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.json");
    let corrupt = b"{ token-bearing config is truncated";
    std::fs::write(&path, corrupt).unwrap();

    let error = user_config::mutate_at(&path, |cfg| cfg.listen_port = Some(16868))
        .expect_err("corrupt config must reject mutation");

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(std::fs::read(&path).unwrap(), corrupt);
}

#[test]
fn local_mode_roundtrip() {
    let cfg = WorkspaceConfig {
        workspace: "/tmp/ws".to_string(),
        created_at: "2026-04-17T00:00:00Z".to_string(),
        git: GitConfig {
            provider: GitProvider::Local,
            remote_url: None,
            token: None,
            github_email: None,
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
            github_email: None,
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
            github_email: None,
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

// Real-fs test: a symlink that points into a blacklisted cloud-sync folder
// should be caught via canonicalization. Lexical `starts_with` alone would miss
// this — symlinks and `..` segments both bypass prefix matching until paths are
// resolved.
#[cfg(unix)]
#[test]
fn reject_symlink_pointing_into_cloud_sync_dir() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().unwrap();
    let fake_home = tmp.path();
    let dropbox = fake_home.join("Dropbox");
    std::fs::create_dir_all(&dropbox).unwrap();

    let sneaky = fake_home.join("not-dropbox-symlink");
    symlink(&dropbox, &sneaky).unwrap();

    let ws = sneaky.join("whatever");
    let err = validate_workspace_path(&ws, fake_home).expect_err("symlink should be caught");
    match err {
        WorkspacePathError::CloudSyncDetected(name) => assert_eq!(name, "Dropbox"),
    }
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
