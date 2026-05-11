use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::broadcast;

use gitim_core::types::config::{Config, IndexerConfig};
use gitim_daemon::api::{Event, Request};
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::AppState;

fn make_config_indexer_disabled() -> Config {
    Config {
        indexer: IndexerConfig { enabled: false },
        ..Config::default()
    }
}

fn make_config_indexer_enabled() -> Config {
    Config {
        indexer: IndexerConfig { enabled: true },
        ..Config::default()
    }
}

fn init_git_repo(root: &std::path::Path) {
    let run = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .expect("git command failed");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run(&["init"]);
    run(&["commit", "--allow-empty", "-m", "init"]);
}

async fn setup_repo(config: Config) -> (TempDir, Arc<AppState>) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();

    init_git_repo(&root);

    std::fs::create_dir_all(root.join(".gitim")).unwrap();
    std::fs::create_dir_all(root.join("users")).unwrap();
    std::fs::write(
        root.join("users/alice.meta.yaml"),
        "display_name: Alice\nrole: dev\nintroduction: hi\n",
    )
    .unwrap();

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
    assert!(result.is_ok(), "initialize_index should not fail when disabled");

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

    // Search request must return an error (not assert on message text — Task 3 owns that)
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
    assert!(!resp.ok, "Search must return error when indexer is disabled");
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
