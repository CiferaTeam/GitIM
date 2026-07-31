#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::net::SocketAddr;

use common::{ensure_daemon_in_path, short_tempdir, HomeGuard};
use gitim_core::skill::WorkspaceSkillMeta;
use gitim_runtime::git_config::{GitProvider, WorkspaceConfig};
use gitim_runtime::http::{create_router, recover_from_config};
use gitim_sync::git::GitStorage;
use gitim_sync::skill::checkpoint::SkillCheckpointStore;
use gitim_sync::skill::git_tree::read_blob_at;

async fn spawn_server() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let (router, _state) = create_router();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (addr, handle)
}

async fn post_json(addr: SocketAddr, path: &str, body: serde_json::Value) -> serde_json::Value {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}{path}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    resp.json().await.unwrap()
}

fn kill_human_daemon(workspace: &std::path::Path) {
    let pid_file = workspace.join(".gitim-runtime/human/.gitim/run/gitim.pid");
    if let Ok(content) = std::fs::read_to_string(pid_file) {
        if let Ok(pid) = content.trim().parse::<u32>() {
            let _ = std::process::Command::new("kill")
                .arg(pid.to_string())
                .output();
        }
    }
}

#[tokio::test]
#[serial_test::serial(home_env)]
async fn git_init_local_creates_bare_and_human_and_config() {
    let _home = HomeGuard::install();
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let (addr, server) = spawn_server().await;

    let workspace_path = tmp.path().join("ws");
    std::fs::create_dir_all(&workspace_path).unwrap();

    let init_resp = post_json(
        addr,
        "/workspaces",
        serde_json::json!({
            "path": workspace_path.to_string_lossy(),
            "git": { "provider": "local" },
        }),
    )
    .await;
    assert_eq!(
        init_resp["ok"], true,
        "workspace create failed: {init_resp:?}"
    );

    assert!(
        workspace_path.join("repo.git").exists(),
        "bare repo.git should exist"
    );
    assert!(
        workspace_path.join(".gitim-runtime/human").exists(),
        "human dir should exist"
    );

    let cfg = WorkspaceConfig::read(&workspace_path).expect("config should be readable");
    assert_eq!(cfg.git.provider, GitProvider::Local);
    assert!(cfg.git.remote_url.is_none());
    assert!(cfg.git.token.is_none());

    let human = workspace_path.join(".gitim-runtime/human");
    let checkpoint = SkillCheckpointStore::new(&human)
        .unwrap()
        .load()
        .unwrap()
        .expect("local provisioning should write a Skill checkpoint");
    let accepted = checkpoint
        .workspace_tree
        .expect("local provisioning should accept workspace Skill metadata");
    let workspace_meta: WorkspaceSkillMeta = serde_yaml::from_slice(
        &read_blob_at(
            &GitStorage::new(&human),
            &accepted.commit_oid,
            "skills/workspace.meta.yaml",
        )
        .unwrap()
        .unwrap(),
    )
    .unwrap();
    assert_eq!(workspace_meta.administrators.len(), 1);

    kill_human_daemon(&workspace_path);
    server.abort();
}

#[tokio::test]
#[serial_test::serial(home_env)]
async fn recovery_bootstraps_a_legacy_local_workspace_before_returning() {
    let _home = HomeGuard::install();
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let (addr, server) = spawn_server().await;
    let workspace = tmp.path().join("legacy");
    std::fs::create_dir_all(&workspace).unwrap();
    let response = post_json(
        addr,
        "/workspaces",
        serde_json::json!({
            "path": workspace.to_string_lossy(),
            "git": { "provider": "local" },
        }),
    )
    .await;
    assert_eq!(response["ok"], true, "{response:?}");

    kill_human_daemon(&workspace);
    server.abort();
    let human = workspace.join(".gitim-runtime/human");
    let first_commit = std::process::Command::new("git")
        .args(["rev-list", "--max-parents=0", "HEAD"])
        .current_dir(&human)
        .output()
        .unwrap();
    assert!(first_commit.status.success());
    let first_commit = String::from_utf8(first_commit.stdout)
        .unwrap()
        .trim()
        .to_owned();
    let branch = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(&human)
        .output()
        .unwrap();
    let branch = String::from_utf8(branch.stdout).unwrap().trim().to_owned();
    let update = std::process::Command::new("git")
        .args([
            "--git-dir",
            workspace.join("repo.git").to_str().unwrap(),
            "update-ref",
            &format!("refs/heads/{branch}"),
            &first_commit,
        ])
        .output()
        .unwrap();
    assert!(update.status.success());
    let reset = std::process::Command::new("git")
        .args(["reset", "--hard", &first_commit])
        .current_dir(&human)
        .output()
        .unwrap();
    assert!(reset.status.success());
    let checkpoint_store = SkillCheckpointStore::new(&human).unwrap();
    if checkpoint_store.path.exists() {
        std::fs::remove_file(&checkpoint_store.path).unwrap();
    }

    let (_router, state) = create_router();
    recover_from_config(state).await;

    let recovered = checkpoint_store
        .load()
        .unwrap()
        .expect("recovery should create a Skill checkpoint");
    assert!(
        recovered.workspace_tree.is_some(),
        "recovery should accept workspace Skill metadata"
    );
    kill_human_daemon(&workspace);
}
