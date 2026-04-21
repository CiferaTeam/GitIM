//! Verifies .gitignore contains .env after workspace init (local mode).

mod common;

use std::net::SocketAddr;

use common::{ensure_daemon_in_path, short_tempdir};
use gitim_runtime::http::create_router;
use serial_test::serial;

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

fn kill_daemon_in(workspace_path: &std::path::Path) {
    let pid_file = workspace_path.join(".gitim-runtime/human/.gitim/run/gitim.pid");
    if let Ok(content) = std::fs::read_to_string(&pid_file) {
        if let Ok(pid) = content.trim().parse::<u32>() {
            let _ = std::process::Command::new("kill")
                .arg(pid.to_string())
                .output();
        }
    }
}

#[tokio::test]
#[serial]
async fn local_git_init_adds_env_to_gitignore_and_commits() {
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
    assert_eq!(init_resp["ok"], true, "workspace create failed: {init_resp:?}");

    let human_root = workspace_path.join(".gitim-runtime/human");
    let gi = human_root.join(".gitignore");
    assert!(gi.exists(), ".gitignore missing at {gi:?}");
    let content = std::fs::read_to_string(&gi).unwrap();
    assert!(content.contains(".env"), ".gitignore should contain .env, got: {content}");

    let out = std::process::Command::new("git")
        .args(["log", "--oneline", "--", ".gitignore"])
        .current_dir(&human_root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git log failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let log = String::from_utf8_lossy(&out.stdout);
    assert!(!log.trim().is_empty(), ".gitignore was never committed; log: {log}");

    kill_daemon_in(&workspace_path);
    server.abort();
}

#[tokio::test]
#[serial]
async fn local_git_init_commits_env_gitignore_exactly_once() {
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
    assert_eq!(init_resp["ok"], true, "workspace create failed: {init_resp:?}");

    let human_root = workspace_path.join(".gitim-runtime/human");
    let out = std::process::Command::new("git")
        .args(["log", "--oneline", "--", ".gitignore"])
        .current_dir(&human_root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git log failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let log = String::from_utf8_lossy(&out.stdout);
    // Count only our specific commit — ensure_repo also touches .gitignore (adds .gitim/ rule),
    // so the total commit count may be > 1. Idempotence means our rule was added exactly once.
    let our_commits = log
        .lines()
        .filter(|l| l.contains("gitignore .env"))
        .count();
    assert_eq!(
        our_commits,
        1,
        "expected exactly one 'chore: gitignore .env' commit, got {our_commits}: {log}"
    );

    kill_daemon_in(&workspace_path);
    server.abort();
}
