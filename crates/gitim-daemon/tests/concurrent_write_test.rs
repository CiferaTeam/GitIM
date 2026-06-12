#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Regression test: concurrent thread writers must never produce duplicate
//! line numbers in a .thread file.
//!
//! The daemon used to do read → compute next_line → append without any lock,
//! so two tokio tasks could both see `last_line = N`, both compute `N+1`, and
//! both append the same line number. This surfaced in production as two
//! `[L000002][...][E:join]` lines side-by-side after a single agent fired two
//! `gitim join-channel -t <user>` invocations back-to-back.
//!
//! These tests spawn multiple concurrent writers and assert the resulting
//! thread file has strictly increasing, unique line numbers.

mod common;

use std::sync::Arc;
use tempfile::TempDir;

use gitim_core::parser::parse_thread;
use gitim_daemon::api::Request;
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::AppState;

/// Build a repo with the given users registered, plus a "general" channel
/// whose `members` list contains `users[0]` (so send/join work immediately).
async fn setup_repo_with_users(users: &[&str]) -> (TempDir, Arc<AppState>) {
    let (tmp, state) = common::setup_repo_with_users(users).await;
    let root = &state.repo_root;

    // Add a "general" channel where users[0] is already a member.
    let members_yaml = format!("- {}", users[0]);
    std::fs::create_dir_all(root.join("channels")).unwrap();
    std::fs::write(
        root.join("channels/general.meta.yaml"),
        format!(
            "display_name: general\ncreated_by: {}\ncreated_at: \"20260323T000000Z\"\nintroduction: general channel\nmembers:\n{}\n",
            users[0], members_yaml
        ),
    )
    .unwrap();
    std::fs::write(root.join("channels/general.thread"), "").unwrap();
    common::run_git(root, &["add", "."]);
    common::run_git(root, &["commit", "-m", "add general channel"]);

    (tmp, state)
}

/// Twelve concurrent Send requests to the same channel must produce
/// twelve distinct, strictly-sequential line numbers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_sends_assign_unique_line_numbers() {
    let (tmp, state) = setup_repo_with_users(&["alice"]).await;

    const N: u64 = 12;
    let mut handles = Vec::with_capacity(N as usize);
    for i in 0..N {
        let state = state.clone();
        handles.push(tokio::spawn(async move {
            let req = Request::Send {
                channel: "general".to_string(),
                body: format!("msg {i}"),
                reply_to: None,
                author: Some("alice".to_string()),
            };
            handle_request(req, state).await
        }));
    }
    let mut returned_lines: Vec<u64> = Vec::with_capacity(N as usize);
    for h in handles {
        let resp = h.await.unwrap();
        assert!(resp.ok, "send failed: {:?}", resp.error);
        returned_lines.push(
            resp.data
                .unwrap()
                .get("line_number")
                .and_then(|v| v.as_u64())
                .expect("response missing line_number"),
        );
    }

    // File must parse and contain exactly N entries with line numbers 1..=N.
    let content = std::fs::read_to_string(tmp.path().join("channels/general.thread")).unwrap();
    let parsed = parse_thread(&content).expect("thread parsed");
    let file_lines: Vec<u64> = parsed.entries.iter().map(|e| e.line_number()).collect();
    assert_eq!(
        file_lines,
        (1..=N).collect::<Vec<_>>(),
        "thread file must contain sequential line numbers 1..={N}, got {file_lines:?}"
    );

    // Each handler's returned line_number must match the file and be unique.
    let mut sorted_returned = returned_lines.clone();
    sorted_returned.sort();
    assert_eq!(
        sorted_returned,
        (1..=N).collect::<Vec<_>>(),
        "returned line_numbers must be unique and cover 1..={N}, got {returned_lines:?}"
    );
}

/// Two concurrent join-channel requests (simulating a codex agent firing
/// `join -t lewis` and `join -t claude01` in parallel) must not produce two
/// events with the same line number.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_joins_assign_unique_line_numbers() {
    let (tmp, state) = setup_repo_with_users(&["codex-01", "lewis", "claude01"]).await;

    let req_lewis = Request::JoinChannel {
        channel: "general".to_string(),
        targets: vec!["lewis".to_string()],
        author: Some("codex-01".to_string()),
    };
    let req_claude = Request::JoinChannel {
        channel: "general".to_string(),
        targets: vec!["claude01".to_string()],
        author: Some("codex-01".to_string()),
    };

    let s1 = state.clone();
    let s2 = state.clone();
    let h1 = tokio::spawn(async move { handle_request(req_lewis, s1).await });
    let h2 = tokio::spawn(async move { handle_request(req_claude, s2).await });
    let r1 = h1.await.unwrap();
    let r2 = h2.await.unwrap();

    // Both should succeed — distinct targets, no business-level conflict.
    assert!(r1.ok, "join lewis failed: {:?}", r1.error);
    assert!(r2.ok, "join claude01 failed: {:?}", r2.error);

    let content = std::fs::read_to_string(tmp.path().join("channels/general.thread")).unwrap();
    let parsed = parse_thread(&content).expect("thread parsed");
    let file_lines: Vec<u64> = parsed.entries.iter().map(|e| e.line_number()).collect();

    // Must be exactly 2 events at L1 and L2 — no duplicate line numbers.
    assert_eq!(
        file_lines,
        vec![1, 2],
        "concurrent joins must land at L1, L2 (no duplicate), got {file_lines:?}\n---\n{content}"
    );
}
