//! Integration tests for the cron-fire branch of `handle_poll`.
//!
//! Without this branch, `crons/<name>/<ts>.thread` files committed by
//! `cron_engine::fire` would never reach the runtime poller. The runtime
//! drives agent_loop wake-ups off `PollChange` entries; a missed branch
//! here means the entire cron feature is non-functional even though
//! every other layer (engine, fs writes, git commits) works correctly.
//!
//! Pattern mirrors `poll_archive_test.rs` — temp git repo + AppState,
//! exercised via `handle_request(Request::Poll {...})`.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::TimeZone;
use tempfile::TempDir;
use tokio::sync::broadcast;

use gitim_core::types::{Config, CronSpec, Handler};
use gitim_daemon::api::Request;
use gitim_daemon::cron_paths::format_thread_filename_ts;
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::AppState;

fn make_config() -> Config {
    serde_yaml::from_str("version: 1").unwrap()
}

/// Build temp repo with alice as `current_user` and a single committed
/// `users/alice.meta.yaml`. The cron poll branch keys ownership off
/// `state.current_user`, so this is the minimum identity setup the
/// branch needs.
async fn setup_state_with_alice() -> (TempDir, Arc<AppState>) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    std::fs::create_dir_all(root.join("users")).unwrap();
    std::fs::write(
        root.join("users/alice.meta.yaml"),
        "display_name: Alice\nrole: dev\nintroduction: hi\n",
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
        *users = vec!["alice".to_string()];
    }
    (tmp, state)
}

/// Helper: write `crons/<name>/spec.yaml`, write a thread file via
/// `cron_engine::format_cron_body` semantics, then commit both. Returns
/// the cursor commit before the fire so a subsequent poll diff sees only
/// the fire write.
fn write_spec_and_fire(
    root: &std::path::Path,
    cron_name: &str,
    target: &str,
    fire_ts: chrono::DateTime<chrono::Utc>,
) {
    let spec = CronSpec {
        version: 1,
        schedule: "@daily".to_string(),
        timezone: None,
        target: Handler::new(target).unwrap(),
        prompt: "scan logs".to_string(),
        enabled: true,
        created_by: Handler::new(target).unwrap(),
        created_at: "2026-01-01T00:00:00Z".to_string(),
        extra: BTreeMap::new(),
    };
    let spec_dir = root.join("crons").join(cron_name);
    std::fs::create_dir_all(&spec_dir).unwrap();
    std::fs::write(spec_dir.join("spec.yaml"), spec.to_yaml().unwrap()).unwrap();

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
    // First commit: just the spec.
    run_git(&["add", "crons"]);
    run_git(&["commit", "-m", "cron: create spec"]);

    // Second commit: the fire thread file.
    let stem = format_thread_filename_ts(fire_ts);
    let thread_path = spec_dir.join(format!("{stem}.thread"));
    let body = "[L000001][P000000][@system][20260102T090000Z] cron(weekly): scan logs\n";
    std::fs::write(&thread_path, body).unwrap();
    run_git(&["add", "crons"]);
    run_git(&["commit", "-m", "cron: fire weekly"]);
}

/// A cron fire whose `target` matches the daemon's own handler must
/// surface in `handle_poll` as a `cron_thread`-kind change keyed by
/// `cron:<name>`.
#[tokio::test]
async fn poll_surfaces_cron_fire_for_self_target() {
    let (_tmp, state) = setup_state_with_alice().await;

    // Cursor: HEAD after the init commit, before any cron writes.
    let cursor = handle_request(Request::Poll { since: None }, state.clone())
        .await
        .data
        .unwrap()["commit_id"]
        .as_str()
        .unwrap()
        .to_string();

    let fire_ts = chrono::Utc.with_ymd_and_hms(2026, 1, 2, 9, 0, 0).unwrap();
    write_spec_and_fire(&state.repo_root, "weekly", "alice", fire_ts);

    let poll_resp = handle_request(
        Request::Poll {
            since: Some(cursor),
        },
        state.clone(),
    )
    .await;
    assert!(poll_resp.ok, "poll failed: {:?}", poll_resp.error);

    let changes = poll_resp.data.unwrap()["changes"]
        .as_array()
        .cloned()
        .unwrap();
    let cron_change = changes
        .iter()
        .find(|c| c["kind"] == "cron_thread")
        .unwrap_or_else(|| panic!("expected a cron_thread change, got: {:#?}", changes));
    assert_eq!(cron_change["channel"], "cron:weekly");
    let entries = cron_change["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "exactly one entry per fire write");
    let entry = &entries[0];
    assert_eq!(entry["author"], "system");
    assert!(
        entry["body"]
            .as_str()
            .unwrap_or("")
            .contains("cron(weekly)"),
        "body should contain the cron-fire signature: {:#?}",
        entry
    );
}

/// A cron fire whose `target` is a different handler MUST be silently
/// dropped by this clone's poll branch — otherwise multi-clone workspaces
/// would wake every agent on every cron in the repo.
#[tokio::test]
async fn poll_drops_cron_fire_for_other_target() {
    let (_tmp, state) = setup_state_with_alice().await;

    // Add bob as a known user so the spec target points somewhere real.
    std::fs::write(
        state.repo_root.join("users/bob.meta.yaml"),
        "display_name: Bob\nrole: dev\nintroduction: hi\n",
    )
    .unwrap();
    let run_git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&state.repo_root)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .unwrap()
    };
    run_git(&["add", "users"]);
    run_git(&["commit", "-m", "add bob"]);

    let cursor = handle_request(Request::Poll { since: None }, state.clone())
        .await
        .data
        .unwrap()["commit_id"]
        .as_str()
        .unwrap()
        .to_string();

    let fire_ts = chrono::Utc.with_ymd_and_hms(2026, 1, 2, 9, 0, 0).unwrap();
    write_spec_and_fire(&state.repo_root, "for-bob", "bob", fire_ts);

    let poll_resp = handle_request(
        Request::Poll {
            since: Some(cursor),
        },
        state.clone(),
    )
    .await;
    assert!(poll_resp.ok, "poll failed: {:?}", poll_resp.error);

    let changes = poll_resp.data.unwrap()["changes"]
        .as_array()
        .cloned()
        .unwrap();
    let cron_changes: Vec<_> = changes
        .iter()
        .filter(|c| c["kind"] == "cron_thread")
        .collect();
    assert!(
        cron_changes.is_empty(),
        "alice's clone must drop bob's cron fire, got: {:#?}",
        cron_changes
    );
}

/// Archived cron threads (`archive/crons/<name>/<ts>.thread`) must NOT
/// surface as cron_thread changes — the archive tree is frozen audit
/// state, not a live trigger surface. Mirrors how archive/channels and
/// archive/dm get separate handling.
#[tokio::test]
async fn poll_drops_archived_cron_threads() {
    let (_tmp, state) = setup_state_with_alice().await;

    let cursor = handle_request(Request::Poll { since: None }, state.clone())
        .await
        .data
        .unwrap()["commit_id"]
        .as_str()
        .unwrap()
        .to_string();

    // Create the archived thread directly — simulates a fire that was
    // already moved to archive (or arrived via sync from a clone that
    // archived it). No corresponding active spec on disk.
    let archive_dir = state.repo_root.join("archive/crons/old");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let stem = format_thread_filename_ts(
        chrono::Utc
            .with_ymd_and_hms(2025, 12, 31, 23, 59, 0)
            .unwrap(),
    );
    std::fs::write(
        archive_dir.join(format!("{stem}.thread")),
        "[L000001][P000000][@system][20251231T235900Z] cron(old): legacy\n",
    )
    .unwrap();

    let run_git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&state.repo_root)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .unwrap()
    };
    run_git(&["add", "archive"]);
    run_git(&["commit", "-m", "import archived cron"]);

    let poll_resp = handle_request(
        Request::Poll {
            since: Some(cursor),
        },
        state.clone(),
    )
    .await;
    assert!(poll_resp.ok);
    let changes = poll_resp.data.unwrap()["changes"]
        .as_array()
        .cloned()
        .unwrap();
    let cron_changes: Vec<_> = changes
        .iter()
        .filter(|c| c["kind"] == "cron_thread")
        .collect();
    assert!(
        cron_changes.is_empty(),
        "archived cron threads must not surface, got: {:#?}",
        cron_changes
    );
}
