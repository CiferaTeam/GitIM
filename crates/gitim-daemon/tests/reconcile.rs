#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Integration tests for `reconcile_orphan_cards`.

mod common;

use std::sync::Arc;

use gitim_daemon::api::Request;
use gitim_daemon::handlers::handle_request;
use gitim_daemon::reconcile::reconcile_orphan_cards;
use gitim_daemon::state::AppState;

/// Shared test repo setup: alice registered, a git repo initialized.
async fn setup_repo() -> (tempfile::TempDir, Arc<AppState>) {
    common::setup_repo_alice().await
}

fn head_commit(root: &std::path::Path) -> String {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Creates a channel via the daemon handler, returns void (asserts success).
async fn create_channel(state: Arc<AppState>, channel: &str) {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_channel",
        "name": channel,
        "author": "alice",
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(resp.ok, "create_channel failed: {:?}", resp.error);
}

/// Creates a card via the daemon handler, returns the card_id.
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

// ─── Test 1: orphan cards get migrated ───────────────────────────────────────

#[tokio::test]
async fn reconcile_moves_orphan_card_dir_to_archive() {
    let (_tmp, state) = setup_repo().await;
    create_channel(state.clone(), "general").await;
    let card_id = create_card(state.clone(), "general", "task").await;

    // Simulate legacy archive_channel: only mv channel meta+thread, leaving
    // channels/general/cards/<id>/ as orphan.
    let root = &state.repo_root;
    let active_meta = root.join("channels/general.meta.yaml");
    let active_thread = root.join("channels/general.thread");
    let archive_meta = root.join("archive/channels/general.meta.yaml");
    let archive_thread = root.join("archive/channels/general.thread");
    std::fs::create_dir_all(root.join("archive/channels")).unwrap();

    // Use raw fs rename (bypass git) — we're simulating what old code did before
    // the git mv support was added. The git index will show these as unstaged
    // changes, which reconcile_orphan_cards must handle on its own.
    std::fs::rename(&active_meta, &archive_meta).unwrap();
    std::fs::rename(&active_thread, &archive_thread).unwrap();

    // Commit the partial rename so git index is clean (mimics what old code
    // actually did: it called git mv for meta+thread but not cards).
    let run_git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .unwrap()
    };
    run_git(&["add", "-A"]);
    run_git(&["commit", "-m", "legacy archive_channel (no cards mv)"]);

    // channels/general/cards/<id>/ is now an orphan (channel archived, cards not moved).
    assert!(
        root.join(format!("channels/general/cards/{}", card_id))
            .exists(),
        "orphan card dir should exist before reconcile"
    );

    let n_migrated = reconcile_orphan_cards(state.clone()).await.unwrap();
    assert_eq!(n_migrated, 1, "expected 1 card migrated");

    // Card should now live in archive.
    let migrated_yaml = root.join(format!(
        "archive/channels/general/cards/{}/card.meta.yaml",
        card_id
    ));
    assert!(
        migrated_yaml.exists(),
        "card meta migrated to archive: {}",
        migrated_yaml.display()
    );

    let yaml = std::fs::read_to_string(&migrated_yaml).unwrap();
    let meta: gitim_core::types::CardMeta = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(
        meta.archived_via,
        Some(gitim_core::types::card::ArchivedVia::Channel),
        "card should be stamped archived_via=Channel"
    );

    // Orphan card directory should be gone.
    let orphan_card = root.join(format!("channels/general/cards/{}", card_id));
    assert!(
        !orphan_card.exists(),
        "orphan card dir should have been removed"
    );

    // The now-empty channels/general/cards/ and channels/general/ dirs should
    // also have been cleaned up from disk (reconcile removes empty dirs).
    assert!(
        std::fs::metadata(root.join("channels/general/cards")).is_err(),
        "channels/general/cards/ should have been removed after all cards moved"
    );
    assert!(
        std::fs::metadata(root.join("channels/general")).is_err(),
        "channels/general/ should have been removed after all cards moved"
    );

    // Working tree must be clean after reconcile.
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&status.stdout).trim().is_empty(),
        "working tree should be clean after reconcile"
    );
}

// ─── Test 2: idempotent when no orphans ──────────────────────────────────────

#[tokio::test]
async fn reconcile_is_idempotent_when_no_orphans() {
    let (_tmp, state) = setup_repo().await;
    create_channel(state.clone(), "general").await;
    create_card(state.clone(), "general", "task").await;

    let head_before = head_commit(&state.repo_root);

    let n = reconcile_orphan_cards(state.clone()).await.unwrap();
    assert_eq!(n, 0, "no cards to migrate");

    let head_after = head_commit(&state.repo_root);
    assert_eq!(
        head_before, head_after,
        "reconcile must not create a commit when there are no orphans"
    );
}

// ─── Test 3: active channels with cards are untouched ────────────────────────

#[tokio::test]
async fn reconcile_skips_active_channels_with_cards() {
    let (_tmp, state) = setup_repo().await;
    create_channel(state.clone(), "general").await;
    let card_id = create_card(state.clone(), "general", "task").await;

    let head_before = head_commit(&state.repo_root);

    let n = reconcile_orphan_cards(state.clone()).await.unwrap();
    assert_eq!(n, 0, "active channel should not be touched");

    // Card must still be in the active location.
    assert!(
        state
            .repo_root
            .join(format!("channels/general/cards/{}/card.meta.yaml", card_id))
            .exists(),
        "active card must not be moved"
    );

    assert_eq!(
        head_commit(&state.repo_root),
        head_before,
        "no commit should be created for active channels"
    );
}
