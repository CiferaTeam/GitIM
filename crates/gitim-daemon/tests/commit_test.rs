#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::broadcast;

use gitim_core::types::Config;
use gitim_daemon::api::{Event, Request};
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::AppState;

fn make_config() -> Config {
    serde_yaml::from_str("version: 1").unwrap()
}

fn init_git_repo(root: &std::path::Path) {
    let run = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .expect("git command failed");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    };

    run(&["init"]);
    run(&["commit", "--allow-empty", "-m", "init"]);
}

async fn setup_git_test_repo() -> (TempDir, Arc<AppState>) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();

    // Initialize git repo with an initial commit
    init_git_repo(&root);

    // Create required directory structure
    std::fs::create_dir_all(root.join("channels")).unwrap();
    std::fs::create_dir_all(root.join("users")).unwrap();
    std::fs::create_dir_all(root.join(".gitim")).unwrap();
    std::fs::write(root.join(".gitim/config.yaml"), "version: 1").unwrap();
    std::fs::write(
        root.join("users/alice.meta.yaml"),
        "display_name: Alice\nrole: dev\nintroduction: hi\n",
    )
    .unwrap();
    // Create "general" channel meta (required by handle_send)
    std::fs::write(
        root.join("channels/general.meta.yaml"),
        "display_name: general\ncreated_by: alice\ncreated_at: \"20260323T000000Z\"\nintroduction: general channel\nmembers: []\n",
    )
    .unwrap();

    // Git add and commit the initial structure
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .expect("git command failed");
    };
    run(&["add", "."]);
    run(&["commit", "-m", "add initial structure"]);

    let (event_tx, _) = broadcast::channel::<Event>(256);
    let state = Arc::new(AppState::new(
        root,
        make_config(),
        event_tx,
        Some("alice".to_string()),
    ));

    {
        let mut users = state.users.write().await;
        users.push("alice".to_string());
    }

    (tmp, state)
}

#[tokio::test]
async fn test_handle_send_creates_git_commit() {
    let (_tmp, state) = setup_git_test_repo().await;

    let req = Request::Send {
        channel: "general".to_string(),
        body: "hello world".to_string(),
        reply_to: None,
        author: Some("alice".to_string()),
    };
    let resp = handle_request(req, state.clone()).await;
    assert!(resp.ok, "send should succeed");

    // Verify the response contains status: "committed"
    let data = resp.data.unwrap();
    assert_eq!(data["status"], "committed", "status should be committed");
    assert_eq!(data["line_number"], 1);
    assert_eq!(data["channel"], "general");

    // Verify git log contains the expected commit message
    let output = std::process::Command::new("git")
        .args(["log", "--oneline"])
        .current_dir(&state.repo_root)
        .output()
        .expect("git log failed");
    let log = String::from_utf8_lossy(&output.stdout);
    assert!(
        log.contains("msg: @alice -> general L000001"),
        "git log should contain the commit message, got: {}",
        log
    );

    // Verify pending_push was recorded
    let pending = state.pending_push.read().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].channel, "general");
    assert_eq!(pending[0].line_number, 1);
}

async fn setup_git_test_repo_with_email(email: Option<String>) -> (TempDir, Arc<AppState>) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();

    init_git_repo(&root);
    std::fs::create_dir_all(root.join("channels")).unwrap();
    std::fs::create_dir_all(root.join("users")).unwrap();
    std::fs::create_dir_all(root.join(".gitim")).unwrap();
    std::fs::write(root.join(".gitim/config.yaml"), "version: 1").unwrap();
    std::fs::write(
        root.join("users/alice.meta.yaml"),
        "display_name: Alice\nrole: dev\nintroduction: hi\n",
    )
    .unwrap();
    std::fs::write(
        root.join("channels/general.meta.yaml"),
        "display_name: general\ncreated_by: alice\ncreated_at: \"20260323T000000Z\"\nintroduction: general channel\nmembers: []\n",
    )
    .unwrap();

    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .expect("git command failed");
    };
    run(&["add", "."]);
    run(&["commit", "-m", "add initial structure"]);

    let (event_tx, _) = broadcast::channel::<Event>(256);
    let state = Arc::new(AppState::new_with_email(
        root,
        make_config(),
        event_tx,
        Some("alice".to_string()),
        email,
    ));

    {
        let mut users = state.users.write().await;
        users.push("alice".to_string());
    }

    (tmp, state)
}

fn read_head_author(repo_root: &std::path::Path) -> (String, String) {
    let out = std::process::Command::new("git")
        .args(["log", "-1", "--format=%an%n%ae"])
        .current_dir(repo_root)
        .output()
        .expect("git log failed");
    let lines: Vec<&str> = std::str::from_utf8(&out.stdout)
        .unwrap()
        .trim_end()
        .split('\n')
        .collect();
    (lines[0].to_string(), lines[1].to_string())
}

#[tokio::test]
async fn send_uses_configured_github_email_as_author() {
    let (_tmp, state) = setup_git_test_repo_with_email(Some("flame0743@gmail.com".into())).await;

    let req = Request::Send {
        channel: "general".to_string(),
        body: "contribution-graph worthy".to_string(),
        reply_to: None,
        author: Some("alice".to_string()),
    };
    let resp = handle_request(req, state.clone()).await;
    assert!(resp.ok, "send should succeed: {:?}", resp.error);

    let (name, email) = read_head_author(&state.repo_root);
    assert_eq!(name, "alice", "author name stays as handler (audit)");
    assert_eq!(
        email, "flame0743@gmail.com",
        "author email should come from AppState.github_email"
    );
}

#[tokio::test]
async fn send_falls_back_to_gitim_domain_when_email_not_configured() {
    let (_tmp, state) = setup_git_test_repo_with_email(None).await;

    let req = Request::Send {
        channel: "general".to_string(),
        body: "legacy behavior".to_string(),
        reply_to: None,
        author: Some("alice".to_string()),
    };
    let resp = handle_request(req, state.clone()).await;
    assert!(resp.ok, "send should succeed: {:?}", resp.error);

    let (name, email) = read_head_author(&state.repo_root);
    assert_eq!(name, "alice");
    assert_eq!(
        email, "alice@gitim",
        "author email falls back to <handler>@gitim when github_email absent"
    );
}
