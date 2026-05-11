mod common;

use std::net::SocketAddr;

use common::{ensure_daemon_in_path, short_tempdir, HomeGuard};
use gitim_core::types::config::Config;
use gitim_runtime::git_config::{GitProvider, WorkspaceConfig};
use gitim_runtime::http::create_router;

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

    // Cleanup: stop the daemon that provision_human spawned.
    let pid_file = workspace_path.join(".gitim-runtime/human/.gitim/run/gitim.pid");
    if let Ok(content) = std::fs::read_to_string(&pid_file) {
        if let Ok(pid) = content.trim().parse::<u32>() {
            let _ = std::process::Command::new("kill")
                .arg(pid.to_string())
                .output();
        }
    }
    server.abort();
}

#[tokio::test]
#[serial_test::serial(home_env)]
async fn git_init_local_writes_indexer_enabled_true() {
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

    let config_path = workspace_path.join(".gitim-runtime/human/.gitim/config.yaml");
    let content = std::fs::read_to_string(&config_path)
        .expect("human config.yaml should exist after /git/init");
    let config: Config = serde_yaml::from_str(&content).expect("config.yaml should be valid yaml");
    assert!(
        config.indexer.enabled,
        "indexer.enabled must be true after human /git/init"
    );

    // Cleanup
    let pid_file = workspace_path.join(".gitim-runtime/human/.gitim/run/gitim.pid");
    if let Ok(content) = std::fs::read_to_string(&pid_file) {
        if let Ok(pid) = content.trim().parse::<u32>() {
            let _ = std::process::Command::new("kill")
                .arg(pid.to_string())
                .output();
        }
    }
    server.abort();
}
