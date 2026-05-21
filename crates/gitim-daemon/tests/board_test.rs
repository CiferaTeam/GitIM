#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::sync::Arc;

use gitim_core::types::Config;
use gitim_daemon::api::{Event, Request};
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::AppState;
use tempfile::TempDir;
use tokio::sync::broadcast;

fn git(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .expect("git command failed");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn make_config() -> Config {
    serde_yaml::from_str("version: 1").unwrap()
}

async fn setup() -> (TempDir, Arc<AppState>) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git(root, &["init"]);
    git(root, &["config", "user.name", "test"]);
    git(root, &["config", "user.email", "test@example.com"]);

    std::fs::create_dir_all(root.join(".gitim")).unwrap();
    std::fs::create_dir_all(root.join("users")).unwrap();
    std::fs::write(root.join(".gitim/config.yaml"), "version: 1").unwrap();
    std::fs::write(
        root.join("users/alice.meta.yaml"),
        "display_name: Alice\nrole: dev\nintroduction: hi\n",
    )
    .unwrap();
    std::fs::write(
        root.join("users/bob.meta.yaml"),
        "display_name: Bob\nrole: dev\nintroduction: hello\n",
    )
    .unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "init"]);

    let (event_tx, _) = broadcast::channel::<Event>(64);
    let state = Arc::new(AppState::new(
        root.to_path_buf(),
        make_config(),
        event_tx,
        Some("alice".to_string()),
    ));
    {
        let mut users = state.users.write().await;
        *users = vec!["alice".to_string(), "bob".to_string()];
    }

    (tmp, state)
}

#[tokio::test]
async fn board_init_creates_current_handler_board() {
    let (_tmp, state) = setup().await;

    let resp = handle_request(
        Request::BoardInit {
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(resp.ok, "board_init failed: {:?}", resp.error);

    let path = state.repo_root.join("showboards/alice/board.md");
    let content = std::fs::read_to_string(path).unwrap();
    assert!(content.contains("handler: alice"));
    assert!(content.contains("## 我能做什么"));
}

#[tokio::test]
async fn board_init_refuses_to_overwrite_existing_board() {
    let (_tmp, state) = setup().await;

    let first = handle_request(
        Request::BoardInit {
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(first.ok, "first board_init failed: {:?}", first.error);

    let path = state.repo_root.join("showboards/alice/board.md");
    let original = std::fs::read_to_string(&path).unwrap();
    let edited = original.replace("## 我能做什么", "## 我能做什么\n\nKeep this board");
    std::fs::write(&path, &edited).unwrap();

    let second = handle_request(
        Request::BoardInit {
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;

    assert!(!second.ok);
    assert!(second.error.unwrap().contains("already exists"));
    assert_eq!(std::fs::read_to_string(path).unwrap(), edited);
}

#[tokio::test]
async fn board_init_rejects_author_mismatch_with_current_user() {
    let (_tmp, state) = setup().await;

    let resp = handle_request(
        Request::BoardInit {
            author: Some("bob".to_string()),
        },
        state.clone(),
    )
    .await;

    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("current user"));
    assert!(!state.repo_root.join("showboards/bob/board.md").exists());
}

#[tokio::test]
async fn board_section_set_commits_only_board_file() {
    let (_tmp, state) = setup().await;
    let resp = handle_request(
        Request::BoardInit {
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(resp.ok, "board_init failed: {:?}", resp.error);

    std::fs::write(state.repo_root.join("unrelated.txt"), "staged\n").unwrap();
    git(&state.repo_root, &["add", "unrelated.txt"]);

    let resp = handle_request(
        Request::BoardSectionSet {
            section: "我能做什么".to_string(),
            value: "正在验证 board 协议。".to_string(),
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(resp.ok, "section set failed: {:?}", resp.error);

    let committed_files = git(
        &state.repo_root,
        &["show", "--name-only", "--format=", "HEAD"],
    );
    assert_eq!(committed_files.trim(), "showboards/alice/board.md");

    let staged_files = git(&state.repo_root, &["diff", "--cached", "--name-only"]);
    assert_eq!(staged_files.trim(), "unrelated.txt");
}

#[tokio::test]
async fn board_publish_rejects_handler_mismatch() {
    let (_tmp, state) = setup().await;
    let content = "---\nversion: 1\nhandler: bob\nupdated_at: 20260509T120000Z\nstatus: working\nsummary: x\ntags: []\n---\n## 当前状态\n\nx\n";

    let resp = handle_request(
        Request::BoardPublish {
            content: Some(content.to_string()),
            author: Some("alice".to_string()),
        },
        state,
    )
    .await;

    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("handler mismatch"));
}

#[tokio::test]
async fn board_publish_stdin_refreshes_updated_at() {
    let (_tmp, state) = setup().await;
    let content = "---\nversion: 1\nhandler: alice\nupdated_at: 20200101T000000Z\nstatus: working\nsummary: stale\ntags: []\n---\n## 当前状态\n\nfrom stdin\n";

    let resp = handle_request(
        Request::BoardPublish {
            content: Some(content.to_string()),
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;

    assert!(resp.ok, "board_publish failed: {:?}", resp.error);
    let persisted =
        std::fs::read_to_string(state.repo_root.join("showboards/alice/board.md")).unwrap();
    assert!(persisted.contains("handler: alice"));
    assert!(persisted.contains("from stdin"));
    assert!(
        !persisted.contains("updated_at: 20200101T000000Z"),
        "publish --stdin should stamp the protocol update time"
    );
}

#[tokio::test]
async fn board_show_reads_other_handlers() {
    let (_tmp, state) = setup().await;
    let resp = handle_request(
        Request::BoardInit {
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(resp.ok, "board_init failed: {:?}", resp.error);

    {
        let mut current_user = state.current_user.write().await;
        *current_user = Some("bob".to_string());
    }

    let resp = handle_request(
        Request::BoardShow {
            handler: "alice".to_string(),
        },
        state,
    )
    .await;
    assert!(resp.ok, "show failed: {:?}", resp.error);
    let data = resp.data.unwrap();
    assert_eq!(data["handler"], "alice");
    assert_eq!(data["path"], "showboards/alice/board.md");
    assert_eq!(data["meta"]["handler"], "alice");
}

#[tokio::test]
async fn board_commit_is_visible_in_poll_catchup() {
    let (_tmp, state) = setup().await;
    let previous_head = git(&state.repo_root, &["rev-parse", "HEAD"]);

    let resp = handle_request(
        Request::BoardInit {
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(resp.ok, "board_init failed: {:?}", resp.error);

    let resp = handle_request(
        Request::Poll {
            since: Some(previous_head.trim().to_string()),
        },
        state,
    )
    .await;
    assert!(resp.ok, "poll failed: {:?}", resp.error);
    let data = resp.data.unwrap();
    let changes = data["changes"].as_array().unwrap();
    let change = changes
        .iter()
        .find(|change| change["kind"] == "board" && change["channel"] == "alice")
        .unwrap_or_else(|| panic!("expected board change for alice, got: {changes:?}"));
    assert_eq!(change["entries"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn board_deletion_only_publish_is_visible_in_poll_catchup() {
    let (_tmp, state) = setup().await;

    let resp = handle_request(
        Request::BoardInit {
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(resp.ok, "board_init failed: {:?}", resp.error);

    let previous_head = git(&state.repo_root, &["rev-parse", "HEAD"]);
    let board_path = state.repo_root.join("showboards/alice/board.md");
    let content = std::fs::read_to_string(&board_path).unwrap();
    let modified = content.replace("## 合作前需要知道的\n", "");
    assert_ne!(modified, content);

    let resp = handle_request(
        Request::BoardPublish {
            content: Some(modified),
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(resp.ok, "board_publish failed: {:?}", resp.error);

    let resp = handle_request(
        Request::Poll {
            since: Some(previous_head.trim().to_string()),
        },
        state,
    )
    .await;
    assert!(resp.ok, "poll failed: {:?}", resp.error);
    let data = resp.data.unwrap();
    let changes = data["changes"].as_array().unwrap();
    let change = changes
        .iter()
        .find(|change| change["kind"] == "board" && change["channel"] == "alice")
        .unwrap_or_else(|| panic!("expected board change for alice, got: {changes:?}"));
    assert_eq!(change["entries"].as_array().unwrap().len(), 0);
}
