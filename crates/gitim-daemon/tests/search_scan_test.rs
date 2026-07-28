#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::path::Path;

use gitim_daemon::api::Request;
use gitim_daemon::handlers::handle_request;

fn write_thread(path: &Path, messages: &[(u64, &str, &str, &str)]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let content = messages
        .iter()
        .map(|(line, author, timestamp, body)| {
            format!("[L{line:06}][P000000][@{author}][{timestamp}] {body}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(path, content).unwrap();
}

fn search_request(query: &str) -> Request {
    Request::Search {
        query: query.to_string(),
    }
}

fn messages(response: &gitim_daemon::api::Response) -> Vec<serde_json::Value> {
    response
        .data
        .as_ref()
        .and_then(|data| data.get("messages"))
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap()
}

#[tokio::test]
async fn search_matches_continuation_and_returns_message_line() {
    let (tmp, state) = common::setup_repo_alice().await;
    write_thread(
        &tmp.path().join("channels/general.thread"),
        &[(
            42,
            "alice",
            "20260728T100000Z",
            "first physical line\nneedle appears in the continuation",
        )],
    );

    let response = handle_request(search_request("needle"), state).await;

    assert!(response.ok, "{:?}", response.error);
    let hits = messages(&response);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["line_number"], 42);
    assert_eq!(
        hits[0]["body"],
        "first physical line\nneedle appears in the continuation"
    );
}

#[tokio::test]
async fn search_returns_latest_ten_across_active_threads() {
    let (tmp, state) = common::setup_repo_alice().await;
    let channel_messages = (1..=8)
        .map(|line| {
            (
                line,
                "alice",
                format!("20260728T10{line:02}00Z"),
                format!("needle channel {line}"),
            )
        })
        .collect::<Vec<_>>();
    let channel_refs = channel_messages
        .iter()
        .map(|(line, author, timestamp, body)| (*line, *author, timestamp.as_str(), body.as_str()))
        .collect::<Vec<_>>();
    write_thread(&tmp.path().join("channels/general.thread"), &channel_refs);

    let card_messages = (9..=12)
        .map(|line| {
            (
                line,
                "alice",
                format!("20260728T10{line:02}00Z"),
                format!("needle card {line}"),
            )
        })
        .collect::<Vec<_>>();
    let card_refs = card_messages
        .iter()
        .map(|(line, author, timestamp, body)| (*line, *author, timestamp.as_str(), body.as_str()))
        .collect::<Vec<_>>();
    write_thread(
        &tmp.path()
            .join("channels/general/cards/20260728-demo/discussion.thread"),
        &card_refs,
    );

    let response = handle_request(search_request("needle"), state).await;

    assert!(response.ok, "{:?}", response.error);
    let hits = messages(&response);
    assert_eq!(hits.len(), 10);
    let lines = hits
        .iter()
        .map(|hit| hit["line_number"].as_u64().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(lines, vec![12, 11, 10, 9, 8, 7, 6, 5, 4, 3]);
}

#[tokio::test]
async fn search_only_reads_direct_messages_visible_to_current_user() {
    let (tmp, state) = common::setup_repo_alice().await;
    write_thread(
        &tmp.path().join("dm/alice--bob.thread"),
        &[(1, "bob", "20260728T100000Z", "visible needle")],
    );
    write_thread(
        &tmp.path().join("dm/bob--carol.thread"),
        &[(1, "bob", "20260728T100100Z", "hidden needle")],
    );

    let response = handle_request(search_request("needle"), state).await;

    assert!(response.ok, "{:?}", response.error);
    let hits = messages(&response);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["channel"], "alice--bob");
}
