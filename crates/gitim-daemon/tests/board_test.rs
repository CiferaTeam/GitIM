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
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(&root)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(&root)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@test.com",
            "commit",
            "-m",
            "init",
        ])
        .current_dir(&root)
        .output()
        .unwrap();
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

// Helper: create a board and return the response
async fn create_board(state: Arc<AppState>, name: &str) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_board",
        "name": name,
        "author": "alice"
    }))
    .unwrap();
    handle_request(req, state).await
}

// Helper: create a card and return (response, card_id)
async fn create_card(
    state: Arc<AppState>,
    board: &str,
    title: &str,
) -> (gitim_daemon::api::Response, Option<String>) {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_card",
        "board": board,
        "title": title,
        "author": "alice"
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

// ---------- 1. test_create_board ----------
#[tokio::test]
async fn test_create_board() {
    let (_tmp, state) = setup_test_repo().await;
    let resp = create_board(state.clone(), "dev-board").await;
    assert!(resp.ok, "create_board should succeed: {:?}", resp.error);

    let data = resp.data.unwrap();
    assert_eq!(data["board"], "dev-board");
    assert_eq!(data["created_by"], "alice");

    // Verify file exists
    assert!(state
        .repo_root
        .join("boards/dev-board/board.meta.yaml")
        .exists());
}

// ---------- 2. test_create_board_duplicate ----------
#[tokio::test]
async fn test_create_board_duplicate() {
    let (_tmp, state) = setup_test_repo().await;
    let resp = create_board(state.clone(), "dup-board").await;
    assert!(resp.ok);

    let resp2 = create_board(state.clone(), "dup-board").await;
    assert!(!resp2.ok, "duplicate board should fail");
    assert!(
        resp2.error.as_deref().unwrap().contains("already exists"),
        "error should mention 'already exists': {:?}",
        resp2.error
    );
}

// ---------- 3. test_create_board_invalid_name ----------
#[tokio::test]
async fn test_create_board_invalid_name() {
    let (_tmp, state) = setup_test_repo().await;
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_board",
        "name": "BadName",
        "author": "alice"
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(!resp.ok, "invalid name should fail");
    assert!(resp.error.as_deref().unwrap().contains("invalid board name"));
}

// ---------- 4. test_list_boards_empty ----------
#[tokio::test]
async fn test_list_boards_empty() {
    let (_tmp, state) = setup_test_repo().await;
    let req: Request = serde_json::from_str(r#"{"method": "list_boards"}"#).unwrap();
    let resp = handle_request(req, state).await;
    assert!(resp.ok);

    let boards = resp.data.unwrap()["boards"].as_array().unwrap().clone();
    assert!(boards.is_empty(), "should be empty: {:?}", boards);
}

// ---------- 5. test_list_boards ----------
#[tokio::test]
async fn test_list_boards() {
    let (_tmp, state) = setup_test_repo().await;
    create_board(state.clone(), "alpha").await;
    create_board(state.clone(), "beta").await;

    let req: Request = serde_json::from_str(r#"{"method": "list_boards"}"#).unwrap();
    let resp = handle_request(req, state).await;
    assert!(resp.ok);

    let boards = resp.data.unwrap()["boards"].as_array().unwrap().clone();
    assert_eq!(boards.len(), 2);
    assert_eq!(boards[0]["name"], "alpha");
    assert_eq!(boards[1]["name"], "beta");
}

// ---------- 6. test_create_card ----------
#[tokio::test]
async fn test_create_card() {
    let (_tmp, state) = setup_test_repo().await;
    create_board(state.clone(), "proj").await;

    let (resp, card_id) = create_card(state.clone(), "proj", "Fix bug #1").await;
    assert!(resp.ok, "create_card should succeed: {:?}", resp.error);
    assert!(card_id.is_some(), "card_id should be present");

    let cid = card_id.unwrap();
    assert!(state
        .repo_root
        .join(format!("boards/proj/{}/card.meta.yaml", cid))
        .exists());
    assert!(state
        .repo_root
        .join(format!("boards/proj/{}/discussion.thread", cid))
        .exists());
}

// ---------- 7. test_create_card_board_not_found ----------
#[tokio::test]
async fn test_create_card_board_not_found() {
    let (_tmp, state) = setup_test_repo().await;
    let (resp, _) = create_card(state.clone(), "nonexistent", "title").await;
    assert!(!resp.ok, "should fail for missing board");
    assert!(resp
        .error
        .as_deref()
        .unwrap()
        .contains("does not exist"));
}

// ---------- 8. test_create_card_invalid_status ----------
#[tokio::test]
async fn test_create_card_invalid_status() {
    let (_tmp, state) = setup_test_repo().await;
    create_board(state.clone(), "proj").await;

    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_card",
        "board": "proj",
        "title": "bad status card",
        "status": "invalid-status",
        "author": "alice"
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(!resp.ok, "invalid status should fail");
    assert!(resp.error.as_deref().unwrap().contains("invalid status"));
}

// ---------- 9. test_list_cards ----------
#[tokio::test]
async fn test_list_cards() {
    let (_tmp, state) = setup_test_repo().await;
    create_board(state.clone(), "proj").await;
    create_card(state.clone(), "proj", "Card A").await;
    create_card(state.clone(), "proj", "Card B").await;

    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "list_cards",
        "board": "proj"
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(resp.ok);

    let cards = resp.data.unwrap()["cards"].as_array().unwrap().clone();
    assert_eq!(cards.len(), 2);
}

// ---------- 10. test_list_cards_filter_status ----------
#[tokio::test]
async fn test_list_cards_filter_status() {
    let (_tmp, state) = setup_test_repo().await;
    create_board(state.clone(), "proj").await;

    // Create two cards (default status = "todo")
    let (_, cid1) = create_card(state.clone(), "proj", "Card A").await;
    create_card(state.clone(), "proj", "Card B").await;

    // Update one card to "done"
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "update_card",
        "board": "proj",
        "card_id": cid1.unwrap(),
        "status": "done",
        "author": "alice"
    }))
    .unwrap();
    let resp = handle_request(req, state.clone()).await;
    assert!(resp.ok);

    // Filter for "todo" — should get only 1
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "list_cards",
        "board": "proj",
        "status": "todo"
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(resp.ok);

    let cards = resp.data.unwrap()["cards"].as_array().unwrap().clone();
    assert_eq!(cards.len(), 1, "only 1 card should be 'todo': {:?}", cards);
}

// ---------- 11. test_list_cards_board_not_found ----------
#[tokio::test]
async fn test_list_cards_board_not_found() {
    let (_tmp, state) = setup_test_repo().await;
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "list_cards",
        "board": "nope"
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(!resp.ok);
    assert!(resp.error.as_deref().unwrap().contains("does not exist"));
}

// ---------- 12. test_send_card_message_and_read ----------
#[tokio::test]
async fn test_send_card_message_and_read() {
    let (_tmp, state) = setup_test_repo().await;
    create_board(state.clone(), "proj").await;
    let (_, cid) = create_card(state.clone(), "proj", "Task 1").await;
    let card_id = cid.unwrap();

    // Send a message
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "send_card_message",
        "board": "proj",
        "card_id": card_id,
        "body": "Hello from test",
        "author": "alice"
    }))
    .unwrap();
    let send_resp = handle_request(req, state.clone()).await;
    assert!(
        send_resp.ok,
        "send_card_message should succeed: {:?}",
        send_resp.error
    );
    assert_eq!(send_resp.data.as_ref().unwrap()["line_number"], 1);

    // Read card — should have 1 entry
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "read_card",
        "board": "proj",
        "card_id": card_id
    }))
    .unwrap();
    let read_resp = handle_request(req, state).await;
    assert!(read_resp.ok);

    let data = read_resp.data.unwrap();
    let entries = data["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "should have 1 entry: {:?}", entries);
    assert!(data["meta"]["title"].as_str().unwrap() == "Task 1");
}

// ---------- 13. test_send_card_message_card_not_found ----------
#[tokio::test]
async fn test_send_card_message_card_not_found() {
    let (_tmp, state) = setup_test_repo().await;
    create_board(state.clone(), "proj").await;

    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "send_card_message",
        "board": "proj",
        "card_id": "00000000-000000-000",
        "body": "hi",
        "author": "alice"
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(!resp.ok);
    assert!(resp.error.as_deref().unwrap().contains("not found"));
}

// ---------- 14. test_read_card_not_found ----------
#[tokio::test]
async fn test_read_card_not_found() {
    let (_tmp, state) = setup_test_repo().await;
    create_board(state.clone(), "proj").await;

    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "read_card",
        "board": "proj",
        "card_id": "00000000-000000-001"
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(!resp.ok);
    assert!(resp.error.as_deref().unwrap().contains("not found"));
}

// ---------- 15. test_update_card_status ----------
#[tokio::test]
async fn test_update_card_status() {
    let (_tmp, state) = setup_test_repo().await;
    create_board(state.clone(), "proj").await;
    let (_, cid) = create_card(state.clone(), "proj", "Status card").await;
    let card_id = cid.unwrap();

    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "update_card",
        "board": "proj",
        "card_id": card_id,
        "status": "in-progress",
        "author": "alice"
    }))
    .unwrap();
    let resp = handle_request(req, state.clone()).await;
    assert!(resp.ok, "update_card should succeed: {:?}", resp.error);

    let data = resp.data.unwrap();
    assert_eq!(data["status"], "in-progress");

    // Verify via read_card
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "read_card",
        "board": "proj",
        "card_id": card_id
    }))
    .unwrap();
    let read_resp = handle_request(req, state).await;
    assert!(read_resp.ok);
    assert_eq!(
        read_resp.data.unwrap()["meta"]["status"],
        "in-progress"
    );
}

// ---------- 16. test_update_card_assignee ----------
#[tokio::test]
async fn test_update_card_assignee() {
    let (_tmp, state) = setup_test_repo().await;
    create_board(state.clone(), "proj").await;
    let (_, cid) = create_card(state.clone(), "proj", "Assign card").await;
    let card_id = cid.unwrap();

    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "update_card",
        "board": "proj",
        "card_id": card_id,
        "assignee": "bob",
        "author": "alice"
    }))
    .unwrap();
    let resp = handle_request(req, state.clone()).await;
    assert!(resp.ok, "update assignee should succeed: {:?}", resp.error);

    let data = resp.data.unwrap();
    assert_eq!(data["assignee"], "bob");

    // Verify via read_card
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "read_card",
        "board": "proj",
        "card_id": card_id
    }))
    .unwrap();
    let read_resp = handle_request(req, state).await;
    assert!(read_resp.ok);
    assert_eq!(read_resp.data.unwrap()["meta"]["assignee"], "bob");
}

// ---------- 17. test_update_card_invalid_status ----------
#[tokio::test]
async fn test_update_card_invalid_status() {
    let (_tmp, state) = setup_test_repo().await;
    create_board(state.clone(), "proj").await;
    let (_, cid) = create_card(state.clone(), "proj", "Bad update").await;
    let card_id = cid.unwrap();

    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "update_card",
        "board": "proj",
        "card_id": card_id,
        "status": "nonexistent-status",
        "author": "alice"
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(!resp.ok);
    assert!(resp.error.as_deref().unwrap().contains("invalid status"));
}

// ---------- 18. test_update_card_not_found ----------
#[tokio::test]
async fn test_update_card_not_found() {
    let (_tmp, state) = setup_test_repo().await;
    create_board(state.clone(), "proj").await;

    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "update_card",
        "board": "proj",
        "card_id": "00000000-000000-002",
        "status": "done",
        "author": "alice"
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(!resp.ok);
    assert!(resp.error.as_deref().unwrap().contains("not found"));
}

// ---------- 19. test_update_card_no_fields ----------
#[tokio::test]
async fn test_update_card_no_fields() {
    let (_tmp, state) = setup_test_repo().await;
    create_board(state.clone(), "proj").await;
    let (_, cid) = create_card(state.clone(), "proj", "No update").await;
    let card_id = cid.unwrap();

    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "update_card",
        "board": "proj",
        "card_id": card_id,
        "author": "alice"
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(!resp.ok, "no fields should fail");
    assert!(
        resp.error
            .as_deref()
            .unwrap()
            .contains("at least one field"),
        "error should mention 'at least one field': {:?}",
        resp.error
    );
}

// ---------- 20. test_guest_mode_blocks_board_writes ----------
#[tokio::test]
async fn test_guest_mode_blocks_board_writes() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    std::fs::create_dir_all(root.join("users")).unwrap();
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(&root)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(&root)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&root)
        .output()
        .unwrap();

    let (tx, _) = broadcast::channel(16);
    let state = Arc::new(AppState::new(root, make_config(), tx, None));
    state
        .is_guest
        .store(true, std::sync::atomic::Ordering::SeqCst);

    // create_board
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_board",
        "name": "test",
        "author": "alice"
    }))
    .unwrap();
    let resp = handle_request(req, state.clone()).await;
    assert!(!resp.ok, "guest create_board should fail");
    assert!(resp.error.as_deref().unwrap().contains("guest"));

    // create_card
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_card",
        "board": "test",
        "title": "t",
        "author": "alice"
    }))
    .unwrap();
    let resp = handle_request(req, state.clone()).await;
    assert!(!resp.ok, "guest create_card should fail");
    assert!(resp.error.as_deref().unwrap().contains("guest"));

    // send_card_message
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "send_card_message",
        "board": "test",
        "card_id": "x",
        "body": "hi",
        "author": "alice"
    }))
    .unwrap();
    let resp = handle_request(req, state.clone()).await;
    assert!(!resp.ok, "guest send_card_message should fail");
    assert!(resp.error.as_deref().unwrap().contains("guest"));

    // update_card
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "update_card",
        "board": "test",
        "card_id": "x",
        "status": "done",
        "author": "alice"
    }))
    .unwrap();
    let resp = handle_request(req, state.clone()).await;
    assert!(!resp.ok, "guest update_card should fail");
    assert!(resp.error.as_deref().unwrap().contains("guest"));

    // read-only ops should work
    let req: Request = serde_json::from_str(r#"{"method": "list_boards"}"#).unwrap();
    let resp = handle_request(req, state.clone()).await;
    assert!(resp.ok, "guest list_boards should succeed");
}

// ---------- 21. test_card_status_changed_event ----------
#[tokio::test]
async fn test_card_status_changed_event() {
    let (_tmp, state) = setup_test_repo().await;
    create_board(state.clone(), "proj").await;
    let (_, cid) = create_card(state.clone(), "proj", "Event card").await;
    let card_id = cid.unwrap();

    let mut rx = state.event_tx.subscribe();

    // Drain any CardCreated event from create_card
    while rx.try_recv().is_ok() {}

    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "update_card",
        "board": "proj",
        "card_id": card_id,
        "status": "done",
        "author": "alice"
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(resp.ok);

    let event = rx.try_recv().unwrap();
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["event"], "card_status_changed");
    assert_eq!(json["board"], "proj");
    assert_eq!(json["card_id"], card_id);
    assert_eq!(json["old_status"], "todo");
    assert_eq!(json["new_status"], "done");
    assert_eq!(json["author"], "alice");
}

// ---------- 22. test_create_board_custom_statuses ----------
#[tokio::test]
async fn test_create_board_custom_statuses() {
    let (_tmp, state) = setup_test_repo().await;

    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_board",
        "name": "custom",
        "statuses": ["backlog", "wip", "review", "shipped"],
        "author": "alice"
    }))
    .unwrap();
    let resp = handle_request(req, state.clone()).await;
    assert!(resp.ok);

    // list_boards and check statuses
    let req: Request = serde_json::from_str(r#"{"method": "list_boards"}"#).unwrap();
    let resp = handle_request(req, state.clone()).await;
    assert!(resp.ok);
    let boards = resp.data.unwrap()["boards"].as_array().unwrap().clone();
    assert_eq!(boards.len(), 1);
    let statuses = boards[0]["statuses"].as_array().unwrap();
    assert_eq!(statuses.len(), 4);
    assert_eq!(statuses[0], "backlog");
    assert_eq!(statuses[3], "shipped");

    // Create a card with one of the custom statuses
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_card",
        "board": "custom",
        "title": "Custom status card",
        "status": "wip",
        "author": "alice"
    }))
    .unwrap();
    let resp = handle_request(req, state.clone()).await;
    assert!(resp.ok, "card with custom status should succeed: {:?}", resp.error);

    // Try default statuses — should fail
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_card",
        "board": "custom",
        "title": "Bad card",
        "status": "todo",
        "author": "alice"
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(!resp.ok, "'todo' should not be valid for custom board");
}

// ---------- 23. test_read_card_with_limit_and_since ----------
#[tokio::test]
async fn test_read_card_with_limit_and_since() {
    let (_tmp, state) = setup_test_repo().await;
    create_board(state.clone(), "proj").await;
    let (_, cid) = create_card(state.clone(), "proj", "Pagination card").await;
    let card_id = cid.unwrap();

    // Send 3 messages
    for i in 1..=3 {
        let req: Request = serde_json::from_value(serde_json::json!({
            "method": "send_card_message",
            "board": "proj",
            "card_id": card_id,
            "body": format!("Message {}", i),
            "author": "alice"
        }))
        .unwrap();
        let resp = handle_request(req, state.clone()).await;
        assert!(resp.ok, "send {} failed: {:?}", i, resp.error);
    }

    // Read with limit=2
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "read_card",
        "board": "proj",
        "card_id": card_id,
        "limit": 2
    }))
    .unwrap();
    let resp = handle_request(req, state.clone()).await;
    assert!(resp.ok);
    let entries = resp.data.unwrap()["entries"].as_array().unwrap().clone();
    assert_eq!(entries.len(), 2, "limit=2 should return 2 entries");

    // Read with since=2 (should return entries with line_number > 2, i.e. line 3)
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "read_card",
        "board": "proj",
        "card_id": card_id,
        "since": 2
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(resp.ok);
    let entries = resp.data.unwrap()["entries"].as_array().unwrap().clone();
    assert_eq!(
        entries.len(),
        1,
        "since=2 should return 1 entry (line 3): {:?}",
        entries
    );
}

#[tokio::test]
async fn test_card_id_path_traversal_rejected() {
    let (_tmp, state) = setup_test_repo().await;
    let req = serde_json::from_str::<Request>(r#"{"method":"create_board","name":"sprint-1"}"#).unwrap();
    handle_request(req, state.clone()).await;
    let req = serde_json::from_str::<Request>(r#"{"method":"read_card","board":"sprint-1","card_id":"../../users"}"#).unwrap();
    let resp = handle_request(req, state.clone()).await;
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("invalid card_id"));
}

#[tokio::test]
async fn test_create_board_empty_statuses_rejected() {
    let (_tmp, state) = setup_test_repo().await;
    let req = serde_json::from_value::<Request>(serde_json::json!({
        "method": "create_board",
        "name": "sprint-1",
        "statuses": [],
    })).unwrap();
    let resp = handle_request(req, state.clone()).await;
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("cannot be empty"));
}
