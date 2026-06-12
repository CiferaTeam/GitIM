#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;

use gitim_daemon::api::Request;
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::AppState;

async fn setup_test_repo() -> (tempfile::TempDir, Arc<AppState>) {
    let (tmp, state) = common::setup_repo_alice_bob().await;
    // Create the "dev" channel thread file that card tests write into.
    std::fs::create_dir_all(state.repo_root.join("channels")).unwrap();
    std::fs::write(state.repo_root.join("channels/dev.thread"), "").unwrap();
    common::run_git(&state.repo_root, &["add", "."]);
    common::run_git(&state.repo_root, &["commit", "-m", "add dev channel"]);
    (tmp, state)
}

async fn create_card(
    state: Arc<AppState>,
    channel: &str,
    title: &str,
) -> (gitim_daemon::api::Response, Option<String>) {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_card",
        "channel": channel,
        "title": title,
        "author": "alice",
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    let card_id = resp
        .data
        .as_ref()
        .and_then(|d| d["card_id"].as_str())
        .map(|s| s.to_string());
    (resp, card_id)
}

#[tokio::test]
async fn test_create_card_happy_path() {
    let (_t, state) = setup_test_repo().await;
    let (resp, card_id) = create_card(state.clone(), "dev", "Implement X").await;
    assert!(resp.ok, "create should succeed: {:?}", resp.error);
    let card_id = card_id.unwrap();
    let meta_path = state
        .repo_root
        .join("channels/dev/cards")
        .join(&card_id)
        .join("card.meta.yaml");
    assert!(meta_path.exists());
    let content = std::fs::read_to_string(&meta_path).unwrap();
    let meta: gitim_core::types::CardMeta = serde_yaml::from_str(&content).unwrap();
    assert_eq!(meta.status, gitim_core::types::CardStatus::Todo);
    assert_eq!(meta.channel, "dev");
    assert_eq!(meta.title, "Implement X");
}

#[tokio::test]
async fn test_create_card_channel_missing() {
    let (_t, state) = setup_test_repo().await;
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_card",
        "channel": "ghost",
        "title": "T",
        "author": "alice",
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("does not exist"));
}

#[tokio::test]
async fn test_create_card_invalid_status() {
    let (_t, state) = setup_test_repo().await;
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_card",
        "channel": "dev",
        "title": "T",
        "status": "review",
        "author": "alice",
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(!resp.ok);
    let err = resp.error.unwrap();
    assert!(
        err.contains("invalid status"),
        "expected 'invalid status' in: {}",
        err
    );
    assert!(
        err.contains("review"),
        "expected 'review' in error: {}",
        err
    );
}

#[tokio::test]
async fn test_create_card_with_labels() {
    let (_t, state) = setup_test_repo().await;
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_card",
        "channel": "dev",
        "title": "T",
        "labels": ["v2", "agent-task"],
        "author": "alice",
    }))
    .unwrap();
    let resp = handle_request(req, state.clone()).await;
    assert!(resp.ok);
    let card_id = resp.data.as_ref().unwrap()["card_id"]
        .as_str()
        .unwrap()
        .to_string();
    let content = std::fs::read_to_string(
        state
            .repo_root
            .join("channels/dev/cards")
            .join(&card_id)
            .join("card.meta.yaml"),
    )
    .unwrap();
    let meta: gitim_core::types::CardMeta = serde_yaml::from_str(&content).unwrap();
    assert_eq!(
        meta.labels,
        vec!["v2".to_string(), "agent-task".to_string()]
    );
}

#[tokio::test]
async fn test_create_card_too_many_labels() {
    let (_t, state) = setup_test_repo().await;
    let labels: Vec<String> = (0..11).map(|i| format!("l{}", i)).collect();
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_card",
        "channel": "dev",
        "title": "T",
        "labels": labels,
        "author": "alice",
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("too many labels"));
}

#[tokio::test]
async fn test_list_cards_empty() {
    let (_t, state) = setup_test_repo().await;
    let req: Request = serde_json::from_value(serde_json::json!({"method": "list_cards"})).unwrap();
    let resp = handle_request(req, state).await;
    assert!(resp.ok);
    assert_eq!(resp.data.unwrap()["cards"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_list_cards_filter_by_channel() {
    let (_t, state) = setup_test_repo().await;
    std::fs::write(state.repo_root.join("channels/docs.thread"), "").unwrap();
    let (_, _) = create_card(state.clone(), "dev", "A").await;
    let (_, _) = create_card(state.clone(), "docs", "B").await;
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "list_cards",
        "channel": "dev",
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(resp.ok);
    let cards = resp.data.unwrap()["cards"].as_array().unwrap().clone();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0]["title"].as_str().unwrap(), "A");
}

#[tokio::test]
async fn test_list_cards_filter_by_status() {
    let (_t, state) = setup_test_repo().await;
    let (_, id_a) = create_card(state.clone(), "dev", "A").await;
    let (_, _) = create_card(state.clone(), "dev", "B").await;
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "update_card",
        "channel": "dev",
        "card_id": id_a.unwrap(),
        "status": "doing",
        "author": "alice",
    }))
    .unwrap();
    let resp = handle_request(req, state.clone()).await;
    assert!(resp.ok);
    let req2: Request = serde_json::from_value(serde_json::json!({
        "method": "list_cards",
        "status": "doing",
    }))
    .unwrap();
    let resp2 = handle_request(req2, state).await;
    assert!(resp2.ok);
    let cards = resp2.data.unwrap()["cards"].as_array().unwrap().clone();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0]["title"].as_str().unwrap(), "A");
}

#[tokio::test]
async fn test_update_card_status_and_emit_event() {
    let (_t, state) = setup_test_repo().await;
    let mut rx = state.event_tx.subscribe();
    let (_, card_id) = create_card(state.clone(), "dev", "T").await;
    let id = card_id.unwrap();
    let _ = rx.recv().await;

    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "update_card",
        "channel": "dev",
        "card_id": id,
        "status": "done",
        "author": "alice",
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(resp.ok);

    let ev = rx.recv().await.unwrap();
    match ev {
        gitim_daemon::api::Event::CardStatusChanged {
            old_status,
            new_status,
            ..
        } => {
            assert_eq!(old_status, "todo");
            assert_eq!(new_status, "done");
        }
        other => panic!("unexpected event: {:?}", other),
    }
}

#[tokio::test]
async fn test_send_card_message_emits_event() {
    let (_t, state) = setup_test_repo().await;
    let mut rx = state.event_tx.subscribe();
    let (_, card_id) = create_card(state.clone(), "dev", "T").await;
    let id = card_id.unwrap();
    let _ = rx.recv().await;

    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "send_card_message",
        "channel": "dev",
        "card_id": id,
        "body": "started work",
        "author": "alice",
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(resp.ok);
    let response_line = resp.data.as_ref().unwrap()["line_number"].as_u64().unwrap();

    let ev = rx.recv().await.unwrap();
    match ev {
        gitim_daemon::api::Event::CardMessageAppended { line_numbers, .. } => {
            assert_eq!(line_numbers, vec![response_line]);
            assert!(response_line >= 1);
        }
        other => panic!("unexpected event: {:?}", other),
    }
}

#[tokio::test]
async fn test_read_card_roundtrip() {
    let (_t, state) = setup_test_repo().await;
    let (_, card_id) = create_card(state.clone(), "dev", "T").await;
    let id = card_id.unwrap();
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "send_card_message",
        "channel": "dev",
        "card_id": id.clone(),
        "body": "progress line",
        "author": "bob",
    }))
    .unwrap();
    let _ = handle_request(req, state.clone()).await;

    let req2: Request = serde_json::from_value(serde_json::json!({
        "method": "read_card",
        "channel": "dev",
        "card_id": id,
    }))
    .unwrap();
    let resp = handle_request(req2, state).await;
    assert!(resp.ok);
    let data = resp.data.unwrap();
    assert_eq!(data["meta"]["title"].as_str().unwrap(), "T");
    let entries = data["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["body"].as_str().unwrap(), "progress line");
    assert_eq!(entries[0]["author"].as_str().unwrap(), "bob");
}

#[tokio::test]
async fn archive_card_sets_archived_via_manual_in_yaml() {
    let (_t, state) = setup_test_repo().await;
    let (_, card_id) = create_card(state.clone(), "dev", "task").await;
    let id = card_id.unwrap();

    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "archive_card",
        "channel": "dev",
        "card_id": id,
        "author": "alice",
    }))
    .unwrap();
    let resp = handle_request(req, state.clone()).await;
    assert!(resp.ok, "archive_card should succeed: {:?}", resp.error);

    // After archive, meta file lives at archive/channels/dev/cards/<id>/card.meta.yaml
    let path = state
        .repo_root
        .join(format!("archive/channels/dev/cards/{}/card.meta.yaml", id));
    let yaml = std::fs::read_to_string(&path).expect("archived card.meta.yaml must exist");
    let meta: gitim_core::types::CardMeta = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(
        meta.archived_via,
        Some(gitim_core::types::card::ArchivedVia::Manual),
        "archived_via should be Manual after archive_card"
    );
}

/// Manual test: simulate commit failure by chmod 0444 on .git/, then archive.
/// Verify that card.meta.yaml archived_via reverts to None after rollback.
#[tokio::test]
#[ignore]
async fn archive_card_rolls_back_yaml_when_commit_fails() {
    // No way to inject commit failure deterministically via the current test fixture.
    // Manual verification steps:
    //   1. Create a card in a test repo.
    //   2. chmod 0444 .git/objects to make commit fail.
    //   3. Call archive_card — expect Response::error containing "rolled back git mv".
    //   4. Read channels/<ch>/cards/<id>/card.meta.yaml — archived_via must be absent.
    //   5. chmod 0755 .git/objects to restore.
}

#[tokio::test]
async fn unarchive_card_clears_archived_via_in_yaml() {
    let (_t, state) = setup_test_repo().await;
    let (_, card_id) = create_card(state.clone(), "dev", "task").await;
    let id = card_id.unwrap();

    // Archive the card first
    let archive_req: Request = serde_json::from_value(serde_json::json!({
        "method": "archive_card",
        "channel": "dev",
        "card_id": id,
        "author": "alice",
    }))
    .unwrap();
    let resp = handle_request(archive_req, state.clone()).await;
    assert!(resp.ok, "archive_card should succeed: {:?}", resp.error);

    // Now unarchive
    let unarchive_req: Request = serde_json::from_value(serde_json::json!({
        "method": "unarchive_card",
        "channel": "dev",
        "card_id": id,
        "author": "alice",
    }))
    .unwrap();
    let resp = handle_request(unarchive_req, state.clone()).await;
    assert!(resp.ok, "unarchive_card should succeed: {:?}", resp.error);

    // After unarchive, meta file is back at channels/dev/cards/<id>/card.meta.yaml
    let path = state
        .repo_root
        .join(format!("channels/dev/cards/{}/card.meta.yaml", id));
    let yaml = std::fs::read_to_string(&path).expect("active card meta exists");
    let meta: gitim_core::types::CardMeta = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(meta.archived_via, None);
    // skip_serializing_if = "Option::is_none" means the field must not appear in yaml
    assert!(
        !yaml.contains("archived_via"),
        "expected archived_via field absent, got:\n{yaml}"
    );
}

/// Manual test: simulate create_dir_all failure (step 7), verify yaml stamp is rolled back.
/// Failure mode: make the archive directory unwritable so create_dir_all fails before git mv.
#[tokio::test]
#[ignore]
async fn archive_card_rolls_back_yaml_when_create_dir_fails() {
    // No way to inject create_dir_all failure deterministically via the current test fixture.
    // Manual verification steps:
    //   1. Create a card in a test repo (channel "dev", card id <id>).
    //   2. mkdir -p archive/channels/dev && chmod 0555 archive/channels/dev
    //      so create_dir_all("archive/channels/dev/cards") fails with EACCES.
    //   3. Call archive_card — expect Response::error containing "failed to create archive dir".
    //   4. Read channels/dev/cards/<id>/card.meta.yaml — archived_via must be absent (yaml restored).
    //   5. chmod 0755 archive/channels/dev to restore.
}

/// Manual test: simulate git mv failure (step 8), verify yaml stamp is rolled back.
/// Failure mode: make the source directory read-only so git mv cannot rename it.
#[tokio::test]
#[ignore]
async fn archive_card_rolls_back_yaml_when_git_mv_fails() {
    // No way to inject git mv failure deterministically via the current test fixture.
    // Manual verification steps:
    //   1. Create a card in a test repo (channel "dev", card id <id>).
    //   2. chmod 0555 channels/dev/cards/<id> so git mv cannot move the directory.
    //   3. Call archive_card — expect Response::error containing "git mv failed".
    //   4. Read channels/dev/cards/<id>/card.meta.yaml — archived_via must be absent (yaml restored).
    //   5. chmod 0755 channels/dev/cards/<id> to restore.
}

/// Manual test: simulate create_dir_all failure during unarchive_card, verify yaml clear is rolled back.
/// Failure mode: make channels/<ch> unwritable so create_dir_all("channels/<ch>/cards") fails.
#[tokio::test]
#[ignore]
async fn unarchive_card_rolls_back_yaml_when_create_dir_fails() {
    // No way to inject create_dir_all failure deterministically via the current test fixture.
    // Manual verification steps:
    //   1. Create and archive a card in a test repo (channel "dev", card id <id>).
    //   2. chmod 0555 channels/dev so create_dir_all("channels/dev/cards") fails with EACCES.
    //   3. Call unarchive_card — expect Response::error containing "failed to create cards dir".
    //   4. Read archive/channels/dev/cards/<id>/card.meta.yaml — archived_via must still be Manual
    //      (yaml cleared then restored by restore_card_yaml).
    //   5. chmod 0755 channels/dev to restore.
}

/// Manual test: simulate git mv failure during unarchive_card, verify yaml clear is rolled back.
/// Failure mode: make the archive source directory read-only so git mv cannot move it.
#[tokio::test]
#[ignore]
async fn unarchive_card_rolls_back_yaml_when_git_mv_fails() {
    // No way to inject git mv failure deterministically via the current test fixture.
    // Manual verification steps:
    //   1. Create and archive a card in a test repo (channel "dev", card id <id>).
    //   2. chmod 0555 archive/channels/dev/cards/<id> so git mv cannot move the directory.
    //   3. Call unarchive_card — expect Response::error containing "git mv failed".
    //   4. Read archive/channels/dev/cards/<id>/card.meta.yaml — archived_via must still be Manual
    //      (yaml cleared then restored by restore_card_yaml).
    //   5. chmod 0755 archive/channels/dev/cards/<id> to restore.
}
