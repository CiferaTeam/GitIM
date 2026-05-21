#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
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

async fn list_users_include_archived(state: Arc<AppState>) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "users",
        "include_archived": true,
    }))
    .unwrap();
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
    assert!(log.contains("archive: depart user @alice"), "log: {}", log);

    // list_users excludes alice; list_archived_users includes alice.
    let lu = list_users(state.clone()).await;
    assert!(lu.ok);
    let lu_users: Vec<String> = serde_json::from_value(lu.data.unwrap()["users"].clone()).unwrap();
    assert!(!lu_users.contains(&"alice".to_string()));
    assert!(lu_users.contains(&"bob".to_string()));

    let la = list_archived_users(state.clone()).await;
    assert!(la.ok);
    let la_users = la.data.unwrap()["users"].as_array().cloned().unwrap();
    let la_handlers: Vec<String> = la_users
        .iter()
        .map(|v| v["handler"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(la_handlers, vec!["alice".to_string()]);
    // display_name is parsed best-effort from the archived meta.yaml. The
    // setup wrote `display_name: Alice` for alice, so the response must
    // surface it on the wire — proves the daemon actually reads the file
    // (not just the filename stem).
    assert_eq!(
        la_users[0]["display_name"].as_str(),
        Some("Alice"),
        "list_archived_users must parse display_name from archive meta",
    );

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
    let lu_users: Vec<String> = serde_json::from_value(lu.data.unwrap()["users"].clone()).unwrap();
    assert!(lu_users.contains(&"alice".to_string()));

    let la = list_archived_users(state.clone()).await;
    let la_users = la.data.unwrap()["users"].as_array().cloned().unwrap();
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
    let users = resp.data.unwrap()["users"].as_array().cloned().unwrap();
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
    let users = resp.data.unwrap()["users"].as_array().cloned().unwrap();
    let handlers: Vec<String> = users
        .iter()
        .map(|v| v["handler"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(handlers, vec!["alice".to_string(), "bob".to_string()]);
    // Both setup_test_repo entries had `display_name: Alice` / `Bob` —
    // verify the parse worked for every row, not just the alphabetically
    // first one.
    assert_eq!(users[0]["display_name"].as_str(), Some("Alice"));
    assert_eq!(users[1]["display_name"].as_str(), Some("Bob"));
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
    let archive_bob_path = state.repo_root.join("archive/users/bob.meta.yaml");
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

// ─── 10. A.5: card handlers gate departed authors ────────────────────────────
//
// Locks in `ensure_author_not_departed` wiring inside
// `crates/gitim-daemon/src/card_handlers.rs` so a future refactor that drops
// the gate from any of the four mutating card handlers (`handle_create_card`,
// `handle_send_card_message`, `handle_update_card`, `handle_archive_card`)
// trips this test instead of silently violating Contract 2.
//
// Coverage:
//   - alice (departed) → each of the four handlers must reject with "is departed"
//   - bob (active)     → the gate must NOT fire (positive control). For
//     handlers without other permission constraints (`handle_create_card`,
//     `handle_send_card_message`, `handle_update_card`) bob's call succeeds.
//     `handle_archive_card` carries a creator/assignee permission check; the
//     positive control there is simply that bob's rejection message is the
//     permission error, not "is departed".

#[tokio::test]
async fn test_card_writes_by_departed_user_fail() {
    let (_tmp, state) = setup_test_repo().await;

    // The shared archive_user_test setup_test_repo doesn't create a channel
    // thread; add one + commit so card handlers can find it.
    std::fs::create_dir_all(state.repo_root.join("channels")).unwrap();
    std::fs::write(state.repo_root.join("channels").join("dev.thread"), "").unwrap();
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
    run_git(&["add", "channels/dev.thread"]);
    run_git(&["commit", "-m", "add dev channel"]);

    // Helper: invoke each card handler via the dispatch path so we exercise
    // the same surface end-to-end.
    let create_card = |author: &str| {
        let req: Request = serde_json::from_value(serde_json::json!({
            "method": "create_card",
            "channel": "dev",
            "title": format!("card by {}", author),
            "author": author,
        }))
        .unwrap();
        req
    };
    let send_card_msg = |author: &str, card_id: &str, body: &str| {
        let req: Request = serde_json::from_value(serde_json::json!({
            "method": "send_card_message",
            "channel": "dev",
            "card_id": card_id,
            "body": body,
            "author": author,
        }))
        .unwrap();
        req
    };
    let update_card = |author: &str, card_id: &str| {
        let req: Request = serde_json::from_value(serde_json::json!({
            "method": "update_card",
            "channel": "dev",
            "card_id": card_id,
            "status": "doing",
            "author": author,
        }))
        .unwrap();
        req
    };
    let archive_card = |author: &str, card_id: &str| {
        let req: Request = serde_json::from_value(serde_json::json!({
            "method": "archive_card",
            "channel": "dev",
            "card_id": card_id,
            "author": author,
        }))
        .unwrap();
        req
    };

    // Step 1 (control): alice (active) creates a card — must succeed.
    let resp = handle_request(create_card("alice"), state.clone()).await;
    assert!(
        resp.ok,
        "control: alice's create_card should succeed: {:?}",
        resp.error
    );
    let alice_card_id = resp
        .data
        .as_ref()
        .and_then(|d| d["card_id"].as_str())
        .unwrap()
        .to_string();

    // Step 2: archive alice. Post-archive the meta yaml lives at
    // `archive/users/alice.meta.yaml` — that's the gate's signal.
    let resp = archive_user(state.clone(), "alice", "alice").await;
    assert!(resp.ok, "archive alice failed: {:?}", resp.error);

    // Restore alice in the in-memory users list so the dispatch path's
    // "unknown user" check doesn't preempt the departed gate. The on-disk
    // archive marker is what we're verifying against.
    {
        let mut users = state.users.write().await;
        if !users.contains(&"alice".to_string()) {
            users.push("alice".to_string());
            users.sort();
        }
    }

    // Step 3: each card handler must reject alice with "is departed".
    // (Request is not Clone, so each case rebuilds its own request.)
    let assert_departed = |label: &str, resp: gitim_daemon::api::Response| {
        assert!(
            !resp.ok,
            "{}: departed alice should be rejected, got ok",
            label
        );
        let err = resp.error.unwrap();
        assert!(
            err.contains("is departed"),
            "{}: error should mention 'is departed', got: {}",
            label,
            err
        );
    };
    let resp = handle_request(create_card("alice"), state.clone()).await;
    assert_departed("create_card", resp);
    let resp = handle_request(
        send_card_msg("alice", &alice_card_id, "ping"),
        state.clone(),
    )
    .await;
    assert_departed("send_card_message", resp);
    let resp = handle_request(update_card("alice", &alice_card_id), state.clone()).await;
    assert_departed("update_card", resp);
    let resp = handle_request(archive_card("alice", &alice_card_id), state.clone()).await;
    assert_departed("archive_card", resp);

    // Step 4 (positive control): bob is active, gate must NOT fire on him.
    //   - create_card: bob's call succeeds.
    //   - send_card_message on alice's card: bob's call succeeds (no
    //     creator/assignee gate on this handler).
    //   - update_card on alice's card: bob's call succeeds (same).
    //   - archive_card on alice's card: bob hits the permission error, not
    //     the departed gate — that's the assertion.

    let resp = handle_request(create_card("bob"), state.clone()).await;
    assert!(
        resp.ok,
        "positive control: bob's create_card should succeed: {:?}",
        resp.error
    );

    let resp = handle_request(
        send_card_msg("bob", &alice_card_id, "hello from bob"),
        state.clone(),
    )
    .await;
    assert!(
        resp.ok,
        "positive control: bob's send_card_message should succeed: {:?}",
        resp.error
    );

    let resp = handle_request(update_card("bob", &alice_card_id), state.clone()).await;
    assert!(
        resp.ok,
        "positive control: bob's update_card should succeed: {:?}",
        resp.error
    );

    let resp = handle_request(archive_card("bob", &alice_card_id), state.clone()).await;
    assert!(
        !resp.ok,
        "positive control: bob's archive_card lacks permission, expected failure"
    );
    let err = resp.error.unwrap();
    assert!(
        !err.contains("is departed"),
        "bob is active — error must NOT be 'is departed', got: {}",
        err
    );
    assert!(
        err.contains("only creator or assignee"),
        "bob's archive_card should fail with permission error, got: {}",
        err
    );
}

// ─── 11. A.5: unarchive_user works even when the target was departed ──────────

#[tokio::test]
async fn test_unarchive_user_works_after_departure() {
    let (_tmp, state) = setup_test_repo().await;

    // Archive bob (alice authoring), so bob is the departed party.
    let resp = archive_user(state.clone(), "bob", "alice").await;
    assert!(resp.ok, "archive bob failed: {:?}", resp.error);
    assert!(state.repo_root.join("archive/users/bob.meta.yaml").exists());

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
        !state.repo_root.join("archive/users/bob.meta.yaml").exists(),
        "archive entry should be gone"
    );
}

// ─── 12. A.6: list_users default omits archived; include_archived returns both ─

#[tokio::test]
async fn test_list_users_default_excludes_archived() {
    let (_tmp, state) = setup_test_repo().await;

    // Archive bob — alice (still active) authors.
    let resp = archive_user(state.clone(), "bob", "alice").await;
    assert!(resp.ok, "archive bob failed: {:?}", resp.error);

    // Default list_users — must omit `archived_users` and exclude bob from `users`.
    let resp = list_users(state.clone()).await;
    assert!(resp.ok);
    let data = resp.data.unwrap();
    let obj = data.as_object().unwrap();

    let users: Vec<String> = serde_json::from_value(data["users"].clone()).unwrap();
    assert_eq!(
        users,
        vec!["alice".to_string()],
        "default list_users should only show active handlers"
    );

    // Wire-additive contract: `archived_users` is omitted on the default-call
    // wire, not present-but-empty.
    assert!(
        !obj.contains_key("archived_users"),
        "default response must omit `archived_users` field, got: {:?}",
        obj.keys().collect::<Vec<_>>()
    );
    // Disambiguation guard: bare `archived` (the bool semantic on
    // `ReadResponse`) must never appear here either.
    assert!(
        !obj.contains_key("archived"),
        "default response must not surface bare `archived` (would clash with ReadResponse.archived: bool), got: {:?}",
        obj.keys().collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn test_list_users_include_archived_returns_both() {
    let (_tmp, state) = setup_test_repo().await;

    // Archive bob — `archive/users/bob.meta.yaml` now exists, alice stays active.
    let resp = archive_user(state.clone(), "bob", "alice").await;
    assert!(resp.ok, "archive bob failed: {:?}", resp.error);

    // include_archived=true must return both lists, sorted, mutually exclusive.
    let resp = list_users_include_archived(state.clone()).await;
    assert!(resp.ok, "include_archived list failed: {:?}", resp.error);
    let data = resp.data.unwrap();

    let users: Vec<String> = serde_json::from_value(data["users"].clone()).unwrap();
    assert_eq!(
        users,
        vec!["alice".to_string()],
        "active list should not contain archived bob"
    );

    let archived: Vec<String> = serde_json::from_value(data["archived_users"].clone()).unwrap();
    assert_eq!(
        archived,
        vec!["bob".to_string()],
        "archived_users list should contain bob"
    );

    // No overlap — Contract 2 / write interception keeps a handler in
    // exactly one place at a time. Check both directions: an archived
    // handler must not appear in `users`, AND an active handler must not
    // appear in `archived_users`. A regression that double-lists in one
    // direction would slip past a single-direction loop.
    for h in &archived {
        assert!(
            !users.contains(h),
            "archived handler @{} appeared in active users",
            h
        );
    }
    for h in &users {
        assert!(
            !archived.contains(h),
            "active handler @{} appeared in archived_users",
            h
        );
    }
}

// ─── 13. A.7: poll surfaces `user_archived` for a departed handler ───────────

#[tokio::test]
async fn test_poll_emits_user_archived_event() {
    let (_tmp, state) = setup_test_repo().await;

    // Cursor before the archive — captures HEAD as it stands.
    let cursor = handle_request(Request::Poll { since: None }, state.clone())
        .await
        .data
        .unwrap()["commit_id"]
        .as_str()
        .unwrap()
        .to_string();

    // alice archives bob — workspace-wide event, alice as the actor sees it.
    let resp = archive_user(state.clone(), "bob", "alice").await;
    assert!(resp.ok, "archive failed: {:?}", resp.error);

    // Poll since the pre-archive cursor — must emit user_archived for bob.
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

    // The `channel` field carries the bare handler — PollChange has no
    // dedicated handler slot, the wire reuses the existing field. Clients
    // discriminate by `kind`.
    let archived_hit = changes
        .iter()
        .any(|c| c["kind"] == "user_archived" && c["channel"] == "bob");
    assert!(
        archived_hit,
        "expected user_archived event for bob, got: {:#?}",
        changes,
    );

    // Path-shaped event — no entries on a meta-only file move.
    for c in &changes {
        if c["kind"] == "user_archived" {
            assert!(
                c["entries"].as_array().unwrap().is_empty(),
                "user_archived event must have empty entries, got: {:?}",
                c["entries"],
            );
            // Sanity: handler must not be a path or carry a slash — would
            // signal we leaked the on-disk path through.
            let h = c["channel"].as_str().unwrap_or("");
            assert!(
                !h.contains('/'),
                "user_archived `channel` must be the bare handler, got: {:?}",
                h,
            );
            assert!(
                !h.starts_with("archive"),
                "user_archived `channel` must not start with 'archive', got: {:?}",
                h,
            );
        }
    }
}

// ─── 14. A.7 (P2.a verify): poll does NOT filter old messages from a now-
//        archived author. The archive marker is a forward-looking write
//        gate (Contract 2 / A.5), not a retroactive content filter. Agents
//        polling a channel must see the departed author's history exactly
//        as it stood — silencing those rows would amount to retroactive
//        history rewriting, which the design explicitly rejects (decision
//        A2 in the archive-protocol plan).
//
// Sequence: alice + bob both write to #dev → alice gets archived → bob
// polls and must still see alice's earlier messages.

#[tokio::test]
async fn test_poll_does_not_filter_archived_authors_old_messages() {
    let (_tmp, state) = setup_test_repo().await;

    // Set up #dev as a channel both alice and bob can post to. The shared
    // setup_test_repo helper creates only users/, not channels/, so we
    // drive the real handler to keep the on-disk shape production-shaped.
    // Inviting bob at create time makes him an `allowed_sender` for
    // compliance — that's what create_channel writes into channel meta.
    let resp = handle_request(
        serde_json::from_value(serde_json::json!({
            "method": "create_channel",
            "name": "dev",
            "author": "alice",
            "invitees": ["bob"],
        }))
        .unwrap(),
        state.clone(),
    )
    .await;
    assert!(resp.ok, "create #dev failed: {:?}", resp.error);

    // Cursor BEFORE alice writes anything — anchors the diff so the poll
    // covers her messages, the archive op, and bob's post-archive write
    // in a single pass. The contract is: a single poll diff that crosses
    // the archive boundary must surface the pre-archive messages from the
    // now-archived author. Anything narrower (cursor mid-stream) wouldn't
    // exercise the no-filter contract — it would just be a normal poll.
    let cursor = handle_request(Request::Poll { since: None }, state.clone())
        .await
        .data
        .unwrap()["commit_id"]
        .as_str()
        .unwrap()
        .to_string();

    // alice writes 2 messages.
    let resp = send_message(state.clone(), "dev", "first from alice", "alice").await;
    assert!(resp.ok, "alice send 1 failed: {:?}", resp.error);
    let resp = send_message(state.clone(), "dev", "second from alice", "alice").await;
    assert!(resp.ok, "alice send 2 failed: {:?}", resp.error);

    // Archive alice (bob's perspective driving). Post-archive alice is no
    // longer in state.users, but the messages she already wrote are
    // committed in channels/dev.thread — they MUST stay visible.
    let resp = archive_user(state.clone(), "alice", "bob").await;
    assert!(resp.ok, "archive alice failed: {:?}", resp.error);

    // bob writes one more message after alice is archived. This proves the
    // channel keeps accepting writes from active users while the archived
    // author's history persists alongside.
    {
        let mut me = state.current_user.write().await;
        *me = Some("bob".to_string());
    }
    let resp = send_message(state.clone(), "dev", "third from bob", "bob").await;
    assert!(resp.ok, "bob send post-archive failed: {:?}", resp.error);

    // Poll from cursor — must include both alice's messages (pre-archive)
    // AND bob's post-archive message. The diff is computed over the
    // channels/dev.thread file additions only; the test asserts no
    // author-state-aware filter has been applied.
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

    // Aggregate every entry across `kind: "channel"` for #dev. We don't
    // care if it spans one or many PollChange rows — the contract is
    // about content presence.
    let mut bodies: Vec<String> = Vec::new();
    let mut authors: Vec<String> = Vec::new();
    for c in &changes {
        if c["kind"] == "channel" && c["channel"] == "dev" {
            for e in c["entries"].as_array().unwrap() {
                if let Some(b) = e.get("body").and_then(|v| v.as_str()) {
                    bodies.push(b.to_string());
                }
                if let Some(a) = e.get("author").and_then(|v| v.as_str()) {
                    authors.push(a.to_string());
                }
            }
        }
    }

    // alice's two messages must be present despite her now-archived state.
    assert!(
        bodies.iter().any(|b| b == "first from alice"),
        "alice's first message must not be filtered, bodies: {:?}",
        bodies,
    );
    assert!(
        bodies.iter().any(|b| b == "second from alice"),
        "alice's second message must not be filtered, bodies: {:?}",
        bodies,
    );
    // bob's post-archive message rounds out the sequence — proves the
    // diff range correctly spanned both pre- and post-archive commits.
    assert!(
        bodies.iter().any(|b| b == "third from bob"),
        "bob's post-archive message must appear, bodies: {:?}",
        bodies,
    );
    // Author column must surface "alice" — not redacted, not rewritten.
    assert!(
        authors.iter().any(|a| a == "alice"),
        "archived author handle must remain on her old messages, authors: {:?}",
        authors,
    );
}
