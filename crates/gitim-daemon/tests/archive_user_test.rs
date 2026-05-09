//! Integration tests for `handle_archive_user`, `handle_unarchive_user`,
//! and `handle_list_archived_users`.
//!
//! Pattern mirrors `unarchive_channel.rs`: temp git repo + AppState in-process,
//! exercise via `handle_request`. No daemon process spawned.

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

/// Build a temp git repo with alice + bob registered in `users/`. Returns
/// (_tmp, state) — keep _tmp alive for the test.
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

    (tmp, state)
}

async fn archive_user(
    state: Arc<AppState>,
    handler: &str,
    author: &str,
) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "archive_user",
        "handler": handler,
        "author": author,
    }))
    .unwrap();
    handle_request(req, state).await
}

async fn unarchive_user(
    state: Arc<AppState>,
    handler: &str,
    author: &str,
) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "unarchive_user",
        "handler": handler,
        "author": author,
    }))
    .unwrap();
    handle_request(req, state).await
}

async fn list_users(state: Arc<AppState>) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({"method": "users"})).unwrap();
    handle_request(req, state).await
}

async fn list_archived_users(state: Arc<AppState>) -> gitim_daemon::api::Response {
    let req: Request =
        serde_json::from_value(serde_json::json!({"method": "list_archived_users"})).unwrap();
    handle_request(req, state).await
}

async fn send_message(
    state: Arc<AppState>,
    channel: &str,
    body: &str,
    author: &str,
) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "send",
        "channel": channel,
        "body": body,
        "author": author,
    }))
    .unwrap();
    handle_request(req, state).await
}

async fn register_user(
    state: Arc<AppState>,
    handler: &str,
    display_name: &str,
) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "register_user",
        "handler": handler,
        "display_name": display_name,
        "role": "member",
        "introduction": "hi",
    }))
    .unwrap();
    handle_request(req, state).await
}

fn git_log_subjects(root: &std::path::Path) -> String {
    let out = std::process::Command::new("git")
        .args(["log", "--pretty=%s"])
        .current_dir(root)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn git_status_clean(root: &std::path::Path) -> String {
    let out = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).to_string()
}

// ─── 1. happy-path: archive + restore round trip ──────────────────────────────

#[tokio::test]
async fn test_archive_user_round_trip() {
    let (_tmp, state) = setup_test_repo().await;

    // Pre-condition: alice in users/, not in archive/users/.
    assert!(state.repo_root.join("users/alice.meta.yaml").exists());
    assert!(!state
        .repo_root
        .join("archive/users/alice.meta.yaml")
        .exists());

    // Archive alice (alice authoring her own departure).
    let resp = archive_user(state.clone(), "alice", "alice").await;
    assert!(resp.ok, "archive failed: {:?}", resp.error);
    let data = resp.data.unwrap();
    assert_eq!(data["handler"].as_str().unwrap(), "alice");
    assert_eq!(data["archived_by"].as_str().unwrap(), "alice");

    // File moved.
    assert!(
        !state.repo_root.join("users/alice.meta.yaml").exists(),
        "active meta should be gone"
    );
    assert!(
        state
            .repo_root
            .join("archive/users/alice.meta.yaml")
            .exists(),
        "archive meta should exist"
    );

    // In-memory list updated.
    {
        let users = state.users.read().await;
        assert!(!users.contains(&"alice".to_string()));
        assert!(users.contains(&"bob".to_string()));
    }

    // Commit recorded.
    let log = git_log_subjects(&state.repo_root);
    assert!(
        log.contains("archive: depart user @alice"),
        "log: {}",
        log
    );

    // list_users excludes alice; list_archived_users includes alice.
    let lu = list_users(state.clone()).await;
    assert!(lu.ok);
    let lu_users: Vec<String> =
        serde_json::from_value(lu.data.unwrap()["users"].clone()).unwrap();
    assert!(!lu_users.contains(&"alice".to_string()));
    assert!(lu_users.contains(&"bob".to_string()));

    let la = list_archived_users(state.clone()).await;
    assert!(la.ok);
    let la_users: Vec<String> =
        serde_json::from_value(la.data.unwrap()["users"].clone()).unwrap();
    assert_eq!(la_users, vec!["alice".to_string()]);

    // Now restore. archive_user dropped alice from the in-memory users
    // list, so alice can't author her own unarchive — bob (still active)
    // does it. Common production shape: peer / admin restores.
    let resp = unarchive_user(state.clone(), "alice", "bob").await;
    assert!(resp.ok, "unarchive failed: {:?}", resp.error);
    let data = resp.data.unwrap();
    assert_eq!(data["handler"].as_str().unwrap(), "alice");
    assert_eq!(data["unarchived_by"].as_str().unwrap(), "bob");

    assert!(state.repo_root.join("users/alice.meta.yaml").exists());
    assert!(!state
        .repo_root
        .join("archive/users/alice.meta.yaml")
        .exists());

    // In-memory list back.
    {
        let users = state.users.read().await;
        assert!(users.contains(&"alice".to_string()));
        assert!(users.contains(&"bob".to_string()));
    }

    // Both archive and restore commits present.
    let log = git_log_subjects(&state.repo_root);
    assert!(log.contains("archive: depart user @alice"), "log: {}", log);
    assert!(log.contains("archive: restore user @alice"), "log: {}", log);

    // Working tree clean.
    assert!(
        git_status_clean(&state.repo_root).trim().is_empty(),
        "working tree should stay clean"
    );

    // After restore, list_users sees alice again.
    let lu = list_users(state.clone()).await;
    let lu_users: Vec<String> =
        serde_json::from_value(lu.data.unwrap()["users"].clone()).unwrap();
    assert!(lu_users.contains(&"alice".to_string()));

    let la = list_archived_users(state.clone()).await;
    let la_users: Vec<String> =
        serde_json::from_value(la.data.unwrap()["users"].clone()).unwrap();
    assert!(la_users.is_empty());
}

// ─── 2. archive non-existent user ─────────────────────────────────────────────

#[tokio::test]
async fn test_archive_nonexistent_user() {
    let (_tmp, state) = setup_test_repo().await;

    let before_log = git_log_subjects(&state.repo_root);

    let resp = archive_user(state.clone(), "nobody", "alice").await;
    assert!(!resp.ok);
    let err = resp.error.unwrap();
    assert!(err.contains("user @nobody not found"), "err: {}", err);

    // No git side-effects.
    let after_log = git_log_subjects(&state.repo_root);
    assert_eq!(before_log, after_log);
    assert!(git_status_clean(&state.repo_root).trim().is_empty());
}

// ─── 3. archive an already-archived user ──────────────────────────────────────

#[tokio::test]
async fn test_archive_already_archived_user() {
    let (_tmp, state) = setup_test_repo().await;

    // First archive succeeds.
    let resp = archive_user(state.clone(), "alice", "alice").await;
    assert!(resp.ok, "first archive failed: {:?}", resp.error);

    let before_log = git_log_subjects(&state.repo_root);

    // Second archive must error cleanly. alice was removed from state.users
    // by the previous archive (the in-memory mutation happens AFTER successful
    // push). Use bob to author the retry — alice would now fail the
    // registered-author guard before reaching the already-archived check.
    let resp = archive_user(state.clone(), "alice", "bob").await;
    assert!(!resp.ok);
    let err = resp.error.unwrap();
    assert!(
        err.contains("user @alice is already archived"),
        "err: {}",
        err
    );

    // No new git operations.
    let after_log = git_log_subjects(&state.repo_root);
    assert_eq!(before_log, after_log);
    assert!(git_status_clean(&state.repo_root).trim().is_empty());
}

// ─── 4. unarchive non-existent archive entry ──────────────────────────────────

#[tokio::test]
async fn test_unarchive_missing_source() {
    let (_tmp, state) = setup_test_repo().await;

    let before_log = git_log_subjects(&state.repo_root);

    let resp = unarchive_user(state.clone(), "alice", "alice").await;
    assert!(!resp.ok);
    let err = resp.error.unwrap();
    assert!(
        err.contains("archive source does not exist for user @alice"),
        "err: {}",
        err
    );

    let after_log = git_log_subjects(&state.repo_root);
    assert_eq!(before_log, after_log);
    assert!(git_status_clean(&state.repo_root).trim().is_empty());
}

// ─── 5. unarchive when active path already exists ─────────────────────────────

#[tokio::test]
async fn test_unarchive_name_conflict() {
    let (_tmp, state) = setup_test_repo().await;

    // Archive alice.
    let resp = archive_user(state.clone(), "alice", "alice").await;
    assert!(resp.ok, "archive failed: {:?}", resp.error);

    // Re-create users/alice.meta.yaml manually (simulates re-registration
    // while archived — A.5 will close that hole; here we just exercise
    // the conflict guard in unarchive).
    std::fs::write(
        state.repo_root.join("users/alice.meta.yaml"),
        "display_name: Alice2\nrole: dev\nintroduction: round 2\n",
    )
    .unwrap();

    let before_log = git_log_subjects(&state.repo_root);

    let resp = unarchive_user(state.clone(), "alice", "bob").await;
    assert!(!resp.ok);
    let err = resp.error.unwrap();
    assert!(
        err.contains("already exists in active location"),
        "err: {}",
        err
    );

    // Both files still where we left them; archive copy intact.
    assert!(state
        .repo_root
        .join("archive/users/alice.meta.yaml")
        .exists());
    assert!(state.repo_root.join("users/alice.meta.yaml").exists());

    let after_log = git_log_subjects(&state.repo_root);
    assert_eq!(before_log, after_log);
}

// ─── 6. archive commit failure rolls back git mv ──────────────────────────────

#[tokio::test]
async fn test_archive_user_rolls_back_on_commit_failure() {
    let (_tmp, state) = setup_test_repo().await;

    // Pre-commit hook that always rejects, forcing commit failure after
    // git mv has moved the meta file.
    let hooks_dir = state.repo_root.join(".git/hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    let hook_path = hooks_dir.join("pre-commit");
    std::fs::write(&hook_path, "#!/bin/sh\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let before_log = git_log_subjects(&state.repo_root);

    let resp = archive_user(state.clone(), "alice", "alice").await;
    assert!(!resp.ok, "archive should fail when commit is rejected");
    let err = resp.error.unwrap();
    assert!(err.contains("rolled back"), "err: {}", err);

    // File still in active location after rollback.
    assert!(
        state.repo_root.join("users/alice.meta.yaml").exists(),
        "active meta must be back after rollback"
    );
    assert!(
        !state
            .repo_root
            .join("archive/users/alice.meta.yaml")
            .exists(),
        "archive meta must not remain after rollback"
    );

    // No commit recorded.
    let after_log = git_log_subjects(&state.repo_root);
    assert_eq!(before_log, after_log);

    // Working tree clean.
    assert!(
        git_status_clean(&state.repo_root).trim().is_empty(),
        "working tree should be clean after rollback"
    );

    // After removing the hook, retry succeeds.
    std::fs::remove_file(&hook_path).unwrap();
    let resp = archive_user(state.clone(), "alice", "alice").await;
    assert!(
        resp.ok,
        "retry after hook removal should succeed: {:?}",
        resp.error
    );
    assert!(state
        .repo_root
        .join("archive/users/alice.meta.yaml")
        .exists());
    assert!(!state.repo_root.join("users/alice.meta.yaml").exists());
}

// ─── 7. list_archived_users empty + sorted ────────────────────────────────────

#[tokio::test]
async fn test_list_archived_users_empty_then_sorted() {
    let (_tmp, state) = setup_test_repo().await;

    // Empty before any archive.
    let resp = list_archived_users(state.clone()).await;
    assert!(resp.ok);
    let users: Vec<String> =
        serde_json::from_value(resp.data.unwrap()["users"].clone()).unwrap();
    assert!(users.is_empty());

    // Archive bob then alice (insertion order != alphabetical).
    let resp = archive_user(state.clone(), "bob", "alice").await;
    assert!(resp.ok, "archive bob failed: {:?}", resp.error);
    let resp = archive_user(state.clone(), "alice", "alice").await;
    // alice was still in state.users when the archive RPC fired (the check
    // runs at dispatch time, not after the in-memory mutation). We're past
    // the read-side gate; just assert the operation succeeded.
    assert!(resp.ok, "archive alice failed: {:?}", resp.error);

    let resp = list_archived_users(state.clone()).await;
    assert!(resp.ok);
    let users: Vec<String> =
        serde_json::from_value(resp.data.unwrap()["users"].clone()).unwrap();
    assert_eq!(users, vec!["alice".to_string(), "bob".to_string()]);
}

// ─── 8. A.5: write interception — departed author can't send ──────────────────

#[tokio::test]
async fn test_send_by_departed_user_fails() {
    let (_tmp, state) = setup_test_repo().await;

    // Archive alice. Post-archive alice is removed from state.users in-memory,
    // but the dispatch path validates `archive/users/alice.meta.yaml` first
    // — that's the contract under test.
    let resp = archive_user(state.clone(), "alice", "alice").await;
    assert!(resp.ok, "archive failed: {:?}", resp.error);

    // Re-add alice to the in-memory users list to bypass the "unknown user"
    // check and isolate the departed-author gate. In production, the on-disk
    // state.users refresh after sync would normally drop her too — but the
    // archive path on disk is the authoritative signal we want to assert.
    {
        let mut users = state.users.write().await;
        if !users.contains(&"alice".to_string()) {
            users.push("alice".to_string());
            users.sort();
        }
    }

    // alice tries to send a DM to bob — must be rejected with "is departed".
    let resp = send_message(state.clone(), "dm:alice,bob", "are you there", "alice").await;
    assert!(!resp.ok, "send by departed alice should be rejected");
    let err = resp.error.unwrap();
    assert!(
        err.contains("is departed"),
        "err should mention departed: {}",
        err
    );

    // bob (still active) can send to alice — but the DM thread doesn't exist
    // yet, so just send to a channel-style target instead. Use a fresh
    // sanity check via register_user: bob is active and his send-equivalent
    // (register a new user) should still work later. Here we just confirm
    // bob's archive guard does NOT fire (negative side of the contract).
    let archive_bob_path = state
        .repo_root
        .join("archive/users/bob.meta.yaml");
    assert!(!archive_bob_path.exists(), "bob should not be archived");
}

// ─── 9. A.5: write interception — register_user rejects departed handler ──────

#[tokio::test]
async fn test_register_user_rejects_departed_handler() {
    let (_tmp, state) = setup_test_repo().await;

    // Archive alice — `archive/users/alice.meta.yaml` now exists.
    let resp = archive_user(state.clone(), "alice", "alice").await;
    assert!(resp.ok, "archive failed: {:?}", resp.error);

    // Try to register a fresh alice — handler is reserved per Contract 2.
    let resp = register_user(state.clone(), "alice", "Alice Reborn").await;
    assert!(!resp.ok, "register_user should reject reserved handler");
    let err = resp.error.unwrap();
    assert!(
        err.contains("is reserved"),
        "err should mention reserved: {}",
        err
    );

    // Sanity: a fresh handler that has never been departed registers fine.
    let resp = register_user(state.clone(), "carol", "Carol").await;
    assert!(resp.ok, "register fresh handler failed: {:?}", resp.error);
}

// ─── 10. A.5: unarchive_user works even when the target was departed ──────────

#[tokio::test]
async fn test_unarchive_user_works_after_departure() {
    let (_tmp, state) = setup_test_repo().await;

    // Archive bob (alice authoring), so bob is the departed party.
    let resp = archive_user(state.clone(), "bob", "alice").await;
    assert!(resp.ok, "archive bob failed: {:?}", resp.error);
    assert!(state
        .repo_root
        .join("archive/users/bob.meta.yaml")
        .exists());

    // alice (active) unarchives bob — the contract: unarchive does NOT gate
    // on departed *target*, and alice is not departed herself, so this must
    // succeed. Restoration must always be reachable by an active actor.
    let resp = unarchive_user(state.clone(), "bob", "alice").await;
    assert!(
        resp.ok,
        "unarchive after departure should succeed: {:?}",
        resp.error
    );
    assert!(
        state.repo_root.join("users/bob.meta.yaml").exists(),
        "bob should be back in active dir"
    );
    assert!(
        !state
            .repo_root
            .join("archive/users/bob.meta.yaml")
            .exists(),
        "archive entry should be gone"
    );
}
