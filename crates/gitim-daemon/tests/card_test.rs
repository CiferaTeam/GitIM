use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::broadcast;

use gitim_core::types::Config;
use gitim_daemon::api::Request;
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::AppState;

fn make_config() -> Config {
    serde_yaml::from_str("version: 1").unwrap()
}

async fn setup_test_repo() -> (TempDir, Arc<AppState>) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    std::fs::create_dir_all(root.join("users")).unwrap();
    std::fs::create_dir_all(root.join("channels")).unwrap();
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
    std::fs::write(root.join("channels/dev.thread"), "").unwrap();
    let run_git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .unwrap()
    };
    run_git(&["init"]);
    run_git(&["add", "."]);
    run_git(&[
        "commit", "-m", "init",
    ]);
    let (tx, _) = broadcast::channel(100);
    let state = Arc::new(AppState::new(
        root,
        make_config(),
        tx,
        Some("alice".to_string()),
    ));
    {
        let mut users = state.users.write().await;
        *users = vec!["alice".to_string(), "bob".to_string()];
    }
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
    assert!(err.contains("invalid status"), "expected 'invalid status' in: {}", err);
    assert!(err.contains("review"), "expected 'review' in error: {}", err);
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
    let card_id = resp.data.as_ref().unwrap()["card_id"].as_str().unwrap().to_string();
    let content = std::fs::read_to_string(
        state.repo_root.join("channels/dev/cards").join(&card_id).join("card.meta.yaml"),
    )
    .unwrap();
    let meta: gitim_core::types::CardMeta = serde_yaml::from_str(&content).unwrap();
    assert_eq!(meta.labels, vec!["v2".to_string(), "agent-task".to_string()]);
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
        gitim_daemon::api::Event::CardStatusChanged { old_status, new_status, .. } => {
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
