#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::broadcast;

use gitim_core::types::Config;
use gitim_daemon::api::{Event, Request};
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::AppState;

fn make_config_indexer_disabled() -> Config {
    serde_yaml::from_str("version: 1\nindexer:\n  enabled: false\n").unwrap()
}

fn make_config_indexer_enabled() -> Config {
    serde_yaml::from_str("version: 1\nindexer:\n  enabled: true\n").unwrap()
}

/// Setup with a custom config (different indexer settings).
/// Writes alice.meta.yaml, inits git, builds AppState with the given config.
async fn setup_repo(config: Config) -> (TempDir, Arc<AppState>) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();

    std::fs::create_dir_all(root.join(".gitim")).unwrap();
    std::fs::create_dir_all(root.join("users")).unwrap();
    common::write_alice(&root);

    // init after files so the initial commit has a real tree
    common::run_git(&root, &["init"]);
    common::run_git(&root, &["add", "users/alice.meta.yaml"]);
    common::run_git(&root, &["commit", "-m", "init"]);

    let (event_tx, _) = broadcast::channel::<Event>(256);
    let state = Arc::new(AppState::new(
        root,
        config,
        event_tx,
        Some("alice".to_string()),
    ));
    {
        let mut users = state.users.write().await;
        users.push("alice".to_string());
    }

    (tmp, state)
}

#[tokio::test]
async fn indexer_disabled_skips_index_creation() {
    let (tmp, state) = setup_repo(make_config_indexer_disabled()).await;

    // Mimic what main.rs does at startup
    let result = AppState::initialize_index(&state);
    assert!(
        result.is_ok(),
        "initialize_index should not fail when disabled"
    );

    // state.index must remain None
    assert!(
        state.index.read().unwrap().is_none(),
        "state.index must be None when indexer is disabled"
    );

    // index.db must not exist on disk
    let db_path = tmp.path().join(".gitim").join("index.db");
    assert!(
        !db_path.exists(),
        "index.db must not be created when indexer is disabled"
    );

    // Search request must return an error with actionable message
    let resp = handle_request(
        Request::Search {
            query: Some("anything".to_string()),
            author: None,
            channel: None,
            channel_type: None,
            limit: 20,
            offset: 0,
            include_cards: false,
        },
        state.clone(),
    )
    .await;
    assert!(
        !resp.ok,
        "Search must return error when indexer is disabled"
    );
    let error_msg = resp.error.as_deref().unwrap_or("");
    assert!(
        error_msg.contains("disabled"),
        "Search error must mention 'disabled', got: {:?}",
        error_msg
    );

    // Reindex request must also return an error with actionable message
    let reindex_resp = handle_request(Request::Reindex, state.clone()).await;
    assert!(
        !reindex_resp.ok,
        "Reindex must return error when indexer is disabled"
    );
    let reindex_error_msg = reindex_resp.error.as_deref().unwrap_or("");
    assert!(
        reindex_error_msg.contains("disabled"),
        "Reindex error must mention 'disabled', got: {:?}",
        reindex_error_msg
    );
}

#[tokio::test]
async fn indexer_enabled_creates_index() {
    let (tmp, state) = setup_repo(make_config_indexer_enabled()).await;

    let result = AppState::initialize_index(&state);
    assert!(
        result.is_ok(),
        "initialize_index should succeed when enabled: {:?}",
        result
    );

    // state.index must be Some
    assert!(
        state.index.read().unwrap().is_some(),
        "state.index must be Some when indexer is enabled"
    );

    // index.db must exist on disk
    let db_path = tmp.path().join(".gitim").join("index.db");
    assert!(
        db_path.exists(),
        "index.db must be created when indexer is enabled"
    );
}
