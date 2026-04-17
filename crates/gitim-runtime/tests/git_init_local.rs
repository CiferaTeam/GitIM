mod common;

use std::net::SocketAddr;

use common::{ensure_daemon_in_path, short_tempdir};
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

async fn post_json(
    addr: SocketAddr,
    path: &str,
    body: serde_json::Value,
) -> serde_json::Value {
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
async fn git_init_local_creates_bare_and_human_and_config() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let (addr, server) = spawn_server().await;

    let workspace_path = tmp.path().join("ws");
    std::fs::create_dir_all(&workspace_path).unwrap();

    let ws_resp = post_json(
        addr,
        "/workspace",
        serde_json::json!({ "path": workspace_path.to_string_lossy(), "confirm": true }),
    )
    .await;
    assert_eq!(ws_resp["ok"], true, "workspace setup failed: {ws_resp:?}");

    let init_resp = post_json(
        addr,
        "/git/init",
        serde_json::json!({ "provider": "local" }),
    )
    .await;
    assert_eq!(init_resp["ok"], true, "git_init failed: {init_resp:?}");

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
