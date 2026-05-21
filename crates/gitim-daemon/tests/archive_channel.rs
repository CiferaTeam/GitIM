#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Integration tests for `handle_archive_channel` — specifically the cards-
//! subtree behaviour introduced in Task 3.1.

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

/// Minimal repo: alice + bob registered, channel "general" created by alice.
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
    run_git(&["commit", "-m", "init"]);

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

    // Create channel "general" as alice.
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_channel",
        "name": "general",
        "author": "alice",
    }))
    .unwrap();
    let resp = handle_request(req, state.clone()).await;
    assert!(resp.ok, "setup: create_channel failed: {:?}", resp.error);

    (tmp, state)
}

/// Creates a card in `channel` as alice, returns the card_id.
async fn create_card(state: Arc<AppState>, channel: &str, title: &str) -> String {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_card",
        "channel": channel,
        "title": title,
        "author": "alice",
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(resp.ok, "create_card failed: {:?}", resp.error);
    resp.data.unwrap()["card_id"].as_str().unwrap().to_string()
}

/// Archives a card as alice.
async fn archive_card(state: Arc<AppState>, channel: &str, card_id: &str) {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "archive_card",
        "channel": channel,
        "card_id": card_id,
        "author": "alice",
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(resp.ok, "archive_card failed: {:?}", resp.error);
}

/// Archives a channel as alice.
async fn archive_channel(state: Arc<AppState>, channel: &str) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "archive_channel",
        "channel": channel,
        "author": "alice",
    }))
    .unwrap();
    handle_request(req, state).await
}

/// Unarchives a channel as alice.
async fn unarchive_channel(state: Arc<AppState>, channel: &str) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "unarchive_channel",
        "channel": channel,
        "author": "alice",
    }))
    .unwrap();
    handle_request(req, state).await
}

// ─── 1. active cards follow the channel ──────────────────────────────────────

#[tokio::test]
async fn archive_channel_moves_active_cards_with_archived_via_channel() {
    let (_tmp, state) = setup_test_repo().await;

    let card1 = create_card(state.clone(), "general", "a").await;
    let card2 = create_card(state.clone(), "general", "b").await;

    let resp = archive_channel(state.clone(), "general").await;
    assert!(resp.ok, "archive_channel failed: {:?}", resp.error);

    // Cards should be at archive/channels/general/cards/<id>/card.meta.yaml
    for card_id in [&card1, &card2] {
        let path = state.repo_root.join(format!(
            "archive/channels/general/cards/{}/card.meta.yaml",
            card_id
        ));
        let yaml = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("archived card meta should exist: {}", path.display()));
        let meta: gitim_core::types::CardMeta = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(
            meta.archived_via,
            Some(gitim_core::types::card::ArchivedVia::Channel),
            "card {} should be archived_via=Channel",
            card_id
        );
    }

    // Original active paths must be gone.
    let active_cards_dir = state.repo_root.join("channels/general/cards");
    assert!(
        !active_cards_dir.exists() || active_cards_dir.read_dir().unwrap().count() == 0,
        "channels/general/cards should be empty or absent after archive"
    );

    // Single commit covers everything (no extra commit for cards).
    let log = std::process::Command::new("git")
        .args(["log", "--pretty=%s"])
        .current_dir(&state.repo_root)
        .output()
        .unwrap();
    let log_str = String::from_utf8_lossy(&log.stdout);
    assert!(
        log_str.contains("archive: #general by @alice"),
        "expected archive commit in log: {}",
        log_str
    );

    // Working tree must be clean.
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&state.repo_root)
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&status.stdout).trim().is_empty(),
        "working tree should be clean after archive"
    );
}

// ─── 2. already-manual-archived card keeps its stamp ─────────────────────────

#[tokio::test]
async fn archive_channel_does_not_touch_existing_manual_archived_cards() {
    let (_tmp, state) = setup_test_repo().await;

    // Archive one card manually before archiving the channel.
    let manual_card = create_card(state.clone(), "general", "manual").await;
    archive_card(state.clone(), "general", &manual_card).await;

    // Create a second card that is still active when the channel is archived.
    let auto_card = create_card(state.clone(), "general", "auto").await;

    let resp = archive_channel(state.clone(), "general").await;
    assert!(resp.ok, "archive_channel failed: {:?}", resp.error);

    // Manual card: already in archive before channel archive — stamp must remain Manual.
    let manual_yaml = std::fs::read_to_string(state.repo_root.join(format!(
        "archive/channels/general/cards/{}/card.meta.yaml",
        manual_card
    )))
    .unwrap();
    let manual_meta: gitim_core::types::CardMeta = serde_yaml::from_str(&manual_yaml).unwrap();
    assert_eq!(
        manual_meta.archived_via,
        Some(gitim_core::types::card::ArchivedVia::Manual),
        "previously-manual card must keep Manual stamp (got {:?})",
        manual_meta.archived_via
    );

    // Auto card: was active, now archived — stamp must be Channel.
    let auto_yaml = std::fs::read_to_string(state.repo_root.join(format!(
        "archive/channels/general/cards/{}/card.meta.yaml",
        auto_card
    )))
    .unwrap();
    let auto_meta: gitim_core::types::CardMeta = serde_yaml::from_str(&auto_yaml).unwrap();
    assert_eq!(
        auto_meta.archived_via,
        Some(gitim_core::types::card::ArchivedVia::Channel),
        "active card should be archived_via=Channel"
    );
}

// ─── 3. unarchive_channel restores only archived_via=channel cards ────────────

#[tokio::test]
async fn unarchive_channel_restores_only_channel_archived_cards() {
    let (_tmp, state) = setup_test_repo().await;

    // Archive one card manually first.
    let manual_card = create_card(state.clone(), "general", "manual").await;
    archive_card(state.clone(), "general", &manual_card).await;

    // Create a second card still active when the channel is archived.
    let auto_card = create_card(state.clone(), "general", "auto").await;

    // Archive the channel — auto_card moves to archive with archived_via=Channel.
    let resp = archive_channel(state.clone(), "general").await;
    assert!(resp.ok, "archive_channel failed: {:?}", resp.error);

    // Now unarchive the channel.
    let resp = unarchive_channel(state.clone(), "general").await;
    assert!(resp.ok, "unarchive_channel failed: {:?}", resp.error);

    // auto_card should be restored to the active location with archived_via cleared.
    let active_meta = state.repo_root.join(format!(
        "channels/general/cards/{}/card.meta.yaml",
        auto_card
    ));
    let auto_yaml =
        std::fs::read_to_string(&active_meta).expect("auto card should be back in active location");
    let auto: gitim_core::types::CardMeta = serde_yaml::from_str(&auto_yaml).unwrap();
    assert_eq!(
        auto.archived_via, None,
        "auto card archived_via should be None after unarchive"
    );
    assert!(
        !auto_yaml.contains("archived_via"),
        "archived_via field should be absent from yaml (skipped_serializing_if)"
    );

    // manual_card should remain in the archive with its Manual stamp intact.
    let manual_archived_meta = state.repo_root.join(format!(
        "archive/channels/general/cards/{}/card.meta.yaml",
        manual_card
    ));
    let manual_yaml = std::fs::read_to_string(&manual_archived_meta)
        .expect("manual card should still be in archive");
    let manual: gitim_core::types::CardMeta = serde_yaml::from_str(&manual_yaml).unwrap();
    assert_eq!(
        manual.archived_via,
        Some(gitim_core::types::card::ArchivedVia::Manual),
        "manual card must keep Manual stamp"
    );

    // Working tree must be clean after the operation.
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&state.repo_root)
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&status.stdout).trim().is_empty(),
        "working tree should be clean after unarchive"
    );
}
