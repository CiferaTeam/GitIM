#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;

use gitim_daemon::api::Request;
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::AppState;

async fn setup_git_test_repo() -> (tempfile::TempDir, Arc<AppState>) {
    common::setup_repo_with_channel("general").await
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
