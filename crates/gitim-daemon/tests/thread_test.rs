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

async fn send(state: Arc<AppState>, body: &str, reply_to: Option<u64>) -> u64 {
    let req = Request::Send {
        channel: "general".to_string(),
        body: body.to_string(),
        reply_to,
        author: Some("alice".to_string()),
    };
    let resp = handle_request(req, state).await;
    assert!(resp.ok, "send failed: {:?}", resp.error);
    resp.data.unwrap()["line_number"].as_u64().unwrap()
}

// Builds a 3-level parent chain: L1 <- L2 <- L3.
// Asking for the thread of L3 (a leaf) must return the whole chain rooted at L1,
// because the true root is reached by walking `point_to` upward.
#[tokio::test]
async fn get_thread_from_leaf_walks_up_to_true_root() {
    let (_tmp, state) = setup_git_test_repo().await;

    let l1 = send(state.clone(), "root message", None).await;
    let l2 = send(state.clone(), "reply to root", Some(l1)).await;
    let l3 = send(state.clone(), "reply to reply", Some(l2)).await;
    assert_eq!((l1, l2, l3), (1, 2, 3));

    let resp = handle_request(
        Request::GetThread {
            channel: "general".to_string(),
            line_number: l3,
        },
        state,
    )
    .await;
    assert!(resp.ok, "thread failed: {:?}", resp.error);

    let data = resp.data.unwrap();
    assert_eq!(
        data["root_line"].as_u64().unwrap(),
        l1,
        "root_line must be the topmost ancestor"
    );

    let entries = data["entries"].as_array().unwrap();
    let lines: Vec<u64> = entries
        .iter()
        .map(|e| e["line_number"].as_u64().unwrap())
        .collect();
    assert_eq!(
        lines,
        vec![l1, l2, l3],
        "entries must include the true root and every descendant"
    );
}

// Clicking thread on a middle-of-chain message must still resolve up to the top.
// Also verifies a sibling branch under the root is included (BFS from true root).
#[tokio::test]
async fn get_thread_from_middle_includes_siblings() {
    let (_tmp, state) = setup_git_test_repo().await;

    let root = send(state.clone(), "root", None).await;
    let child_a = send(state.clone(), "child a", Some(root)).await;
    let grandchild = send(state.clone(), "grandchild of a", Some(child_a)).await;
    let child_b = send(state.clone(), "child b (sibling)", Some(root)).await;

    let resp = handle_request(
        Request::GetThread {
            channel: "general".to_string(),
            line_number: child_a,
        },
        state,
    )
    .await;
    assert!(resp.ok);

    let data = resp.data.unwrap();
    assert_eq!(data["root_line"].as_u64().unwrap(), root);

    let entries = data["entries"].as_array().unwrap();
    let lines: Vec<u64> = entries
        .iter()
        .map(|e| e["line_number"].as_u64().unwrap())
        .collect();
    assert_eq!(lines, vec![root, child_a, grandchild, child_b]);
}
