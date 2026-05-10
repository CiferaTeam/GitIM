//! Integration tests for `handle_archive_dm`, `handle_unarchive_dm`,
//! and `handle_list_archived_dms`.
//!
//! Pattern mirrors `archive_user_test.rs`: temp git repo + AppState
//! in-process, exercise via `handle_request`. No daemon process spawned.

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

/// Build a temp git repo with alice + bob + charlie registered, plus
/// an `dm/alice--bob.thread` populated with one line so archive has
/// something to move. Returns (_tmp, state) — keep _tmp alive for the
/// duration of the test.
async fn setup_test_repo() -> (TempDir, Arc<AppState>) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    std::fs::create_dir_all(root.join("users")).unwrap();
    for h in ["alice", "bob", "charlie"] {
        std::fs::write(
            root.join(format!("users/{}.meta.yaml", h)),
            format!("display_name: {}\nrole: dev\nintroduction: hi\n", h),
        )
        .unwrap();
    }

    // dm/alice--bob.thread — one message so the file is non-empty (archive
    // operation moves an existing file; `git mv` on missing file errors out).
    let dm_dir = root.join("dm");
    std::fs::create_dir_all(&dm_dir).unwrap();
    std::fs::write(
        dm_dir.join("alice--bob.thread"),
        "[L000001][P000000][@alice][20260509T100000Z] hey bob\n",
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
        *users = vec![
            "alice".to_string(),
            "bob".to_string(),
            "charlie".to_string(),
        ];
    }

    (tmp, state)
}

async fn archive_dm(state: Arc<AppState>, peer: &str, author: &str) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "archive_dm",
        "peer": peer,
        "author": author,
    }))
    .unwrap();
    handle_request(req, state).await
}

async fn unarchive_dm(
    state: Arc<AppState>,
    peer: &str,
    author: &str,
) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "unarchive_dm",
        "peer": peer,
        "author": author,
    }))
    .unwrap();
    handle_request(req, state).await
}

async fn list_archived_dms(state: Arc<AppState>, author: &str) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "list_archived_dms",
        "author": author,
    }))
    .unwrap();
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

async fn read_thread(state: Arc<AppState>, channel: &str) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "read",
        "channel": channel,
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
async fn test_archive_dm_round_trip() {
    let (_tmp, state) = setup_test_repo().await;

    // Pre-condition: dm/alice--bob.thread exists, no archive entry.
    assert!(state.repo_root.join("dm/alice--bob.thread").exists());
    assert!(!state
        .repo_root
        .join("archive/dm/alice--bob.thread")
        .exists());

    // Archive (alice initiating).
    let resp = archive_dm(state.clone(), "bob", "alice").await;
    assert!(resp.ok, "archive failed: {:?}", resp.error);
    let data = resp.data.unwrap();
    assert_eq!(data["archived_by"].as_str().unwrap(), "alice");
    assert_eq!(data["dm_pair_stem"].as_str().unwrap(), "alice--bob");

    // File moved.
    assert!(
        !state.repo_root.join("dm/alice--bob.thread").exists(),
        "active dm should be gone"
    );
    assert!(
        state
            .repo_root
            .join("archive/dm/alice--bob.thread")
            .exists(),
        "archive dm should exist"
    );

    // Commit recorded.
    let log = git_log_subjects(&state.repo_root);
    assert!(log.contains("archive: dm with @bob"), "log: {}", log);

    // list_archived_dms — alice sees her DM with bob.
    let la = list_archived_dms(state.clone(), "alice").await;
    assert!(la.ok);
    let dms = la.data.unwrap()["dms"].clone();
    let arr = dms.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["peer"].as_str().unwrap(), "bob");
    assert_eq!(arr[0]["dm_pair_stem"].as_str().unwrap(), "alice--bob");

    // bob also sees the archived DM (sym sort) — peer is alice from his POV.
    let lb = list_archived_dms(state.clone(), "bob").await;
    assert!(lb.ok);
    let dms = lb.data.unwrap()["dms"].clone();
    let arr = dms.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["peer"].as_str().unwrap(), "alice");
    assert_eq!(arr[0]["dm_pair_stem"].as_str().unwrap(), "alice--bob");

    // Now restore.
    let resp = unarchive_dm(state.clone(), "bob", "alice").await;
    assert!(resp.ok, "unarchive failed: {:?}", resp.error);
    let data = resp.data.unwrap();
    assert_eq!(data["unarchived_by"].as_str().unwrap(), "alice");
    assert_eq!(data["dm_pair_stem"].as_str().unwrap(), "alice--bob");

    assert!(state.repo_root.join("dm/alice--bob.thread").exists());
    assert!(!state
        .repo_root
        .join("archive/dm/alice--bob.thread")
        .exists());

    // Both archive and restore commits present.
    let log = git_log_subjects(&state.repo_root);
    assert!(log.contains("archive: dm with @bob"), "log: {}", log);
    assert!(
        log.contains("archive: restore dm with @bob"),
        "log: {}",
        log
    );

    // Working tree clean.
    assert!(
        git_status_clean(&state.repo_root).trim().is_empty(),
        "working tree should stay clean"
    );

    // After restore, list_archived_dms is empty for both participants.
    let la = list_archived_dms(state.clone(), "alice").await;
    assert!(la.data.unwrap()["dms"].as_array().unwrap().is_empty());
    let lb = list_archived_dms(state.clone(), "bob").await;
    assert!(lb.data.unwrap()["dms"].as_array().unwrap().is_empty());
}

// ─── 2. archive non-existent DM ───────────────────────────────────────────────

#[tokio::test]
async fn test_archive_nonexistent_dm() {
    let (_tmp, state) = setup_test_repo().await;

    let before_log = git_log_subjects(&state.repo_root);

    // alice has no DM with charlie — archive should error.
    let resp = archive_dm(state.clone(), "charlie", "alice").await;
    assert!(!resp.ok);
    let err = resp.error.unwrap();
    assert!(err.contains("DM with @charlie not found"), "err: {}", err);

    // No git side-effects.
    let after_log = git_log_subjects(&state.repo_root);
    assert_eq!(before_log, after_log);
    assert!(git_status_clean(&state.repo_root).trim().is_empty());
}

// ─── 3. archive an already-archived DM ────────────────────────────────────────

#[tokio::test]
async fn test_archive_already_archived_dm() {
    let (_tmp, state) = setup_test_repo().await;

    // First archive succeeds.
    let resp = archive_dm(state.clone(), "bob", "alice").await;
    assert!(resp.ok, "first archive failed: {:?}", resp.error);

    let before_log = git_log_subjects(&state.repo_root);

    // Second archive must error cleanly.
    let resp = archive_dm(state.clone(), "bob", "alice").await;
    assert!(!resp.ok);
    let err = resp.error.unwrap();
    assert!(
        err.contains("DM with @bob is already archived"),
        "err: {}",
        err
    );

    // No new git operations.
    let after_log = git_log_subjects(&state.repo_root);
    assert_eq!(before_log, after_log);
    assert!(git_status_clean(&state.repo_root).trim().is_empty());
}

// ─── 4. list_archived_dms filters by caller participation ─────────────────────

#[tokio::test]
async fn test_list_archived_dms_filters_by_caller() {
    let (_tmp, state) = setup_test_repo().await;

    // alice archives her DM with bob.
    let resp = archive_dm(state.clone(), "bob", "alice").await;
    assert!(resp.ok, "archive failed: {:?}", resp.error);

    // alice (participant) sees it.
    let la = list_archived_dms(state.clone(), "alice").await;
    assert!(la.ok);
    let arr_a = la.data.unwrap()["dms"].as_array().unwrap().clone();
    assert_eq!(arr_a.len(), 1);

    // bob (participant) sees it.
    let lb = list_archived_dms(state.clone(), "bob").await;
    assert!(lb.ok);
    let arr_b = lb.data.unwrap()["dms"].as_array().unwrap().clone();
    assert_eq!(arr_b.len(), 1);

    // charlie (third party) does NOT see it.
    let lc = list_archived_dms(state.clone(), "charlie").await;
    assert!(lc.ok);
    let arr_c = lc.data.unwrap()["dms"].as_array().unwrap().clone();
    assert!(
        arr_c.is_empty(),
        "charlie should not see alice<->bob DM, got {:?}",
        arr_c
    );
}

// ─── 5. unarchive non-existent / not-archived DM ──────────────────────────────

#[tokio::test]
async fn test_unarchive_dm_not_archived() {
    let (_tmp, state) = setup_test_repo().await;

    let before_log = git_log_subjects(&state.repo_root);

    // alice<->bob DM is still active — unarchive should error.
    let resp = unarchive_dm(state.clone(), "bob", "alice").await;
    assert!(!resp.ok);
    let err = resp.error.unwrap();
    assert!(err.contains("DM with @bob is not archived"), "err: {}", err);

    let after_log = git_log_subjects(&state.repo_root);
    assert_eq!(before_log, after_log);
    assert!(git_status_clean(&state.repo_root).trim().is_empty());
}

#[tokio::test]
async fn test_unarchive_dm_missing_in_archive() {
    let (_tmp, state) = setup_test_repo().await;

    // alice has no DM with charlie at all (active or archived).
    let resp = unarchive_dm(state.clone(), "charlie", "alice").await;
    assert!(!resp.ok);
    let err = resp.error.unwrap();
    // Active path doesn't exist → falls through to archive check, which
    // also doesn't exist → "not found in archive".
    assert!(err.contains("not found in archive"), "err: {}", err);
}

// ─── 6. archive commit failure rolls back git mv ──────────────────────────────

#[tokio::test]
async fn test_archive_dm_rolls_back_on_commit_failure() {
    let (_tmp, state) = setup_test_repo().await;

    // Pre-commit hook that always rejects, forcing commit failure after
    // git mv has moved the thread file.
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

    let resp = archive_dm(state.clone(), "bob", "alice").await;
    assert!(!resp.ok, "archive should fail when commit is rejected");
    let err = resp.error.unwrap();
    assert!(err.contains("rolled back"), "err: {}", err);

    // File still in active location after rollback.
    assert!(
        state.repo_root.join("dm/alice--bob.thread").exists(),
        "active dm must be back after rollback"
    );
    assert!(
        !state
            .repo_root
            .join("archive/dm/alice--bob.thread")
            .exists(),
        "archive dm must not remain after rollback"
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
    let resp = archive_dm(state.clone(), "bob", "alice").await;
    assert!(
        resp.ok,
        "retry after hook removal should succeed: {:?}",
        resp.error
    );
    assert!(state
        .repo_root
        .join("archive/dm/alice--bob.thread")
        .exists());
    assert!(!state.repo_root.join("dm/alice--bob.thread").exists());
}

// ─── 7. either party can archive (decision B1: single-party archive) ──────────

#[tokio::test]
async fn test_either_party_can_archive_dm() {
    let (_tmp, state) = setup_test_repo().await;

    // alice archives, then unarchives.
    let resp = archive_dm(state.clone(), "bob", "alice").await;
    assert!(resp.ok, "alice archive failed: {:?}", resp.error);
    assert!(state
        .repo_root
        .join("archive/dm/alice--bob.thread")
        .exists());

    let resp = unarchive_dm(state.clone(), "alice", "bob").await;
    assert!(resp.ok, "bob unarchive failed: {:?}", resp.error);
    assert!(state.repo_root.join("dm/alice--bob.thread").exists());

    // Now bob archives the same DM independently.
    let resp = archive_dm(state.clone(), "alice", "bob").await;
    assert!(resp.ok, "bob archive failed: {:?}", resp.error);
    assert!(state
        .repo_root
        .join("archive/dm/alice--bob.thread")
        .exists());
    assert!(!state.repo_root.join("dm/alice--bob.thread").exists());

    // Commit log shows both authors.
    let log = git_log_subjects(&state.repo_root);
    assert!(log.contains("archive: dm with @bob"), "log: {}", log);
    assert!(
        log.contains("archive: restore dm with @alice"),
        "log: {}",
        log
    );
    assert!(log.contains("archive: dm with @alice"), "log: {}", log);
}

// ─── 8. list_archived_dms empty + sorted ──────────────────────────────────────

#[tokio::test]
async fn test_list_archived_dms_empty_then_sorted() {
    let (_tmp, state) = setup_test_repo().await;

    // Empty before any archive.
    let resp = list_archived_dms(state.clone(), "alice").await;
    assert!(resp.ok);
    let arr = resp.data.unwrap()["dms"].as_array().unwrap().clone();
    assert!(arr.is_empty());

    // Add a second DM file alice<->charlie so we can verify sort order.
    std::fs::write(
        state.repo_root.join("dm/alice--charlie.thread"),
        "[L000001][P000000][@alice][20260509T101000Z] hey charlie\n",
    )
    .unwrap();
    std::process::Command::new("git")
        .args(["add", "dm/alice--charlie.thread"])
        .current_dir(&state.repo_root)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@test.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@test.com")
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "add charlie dm"])
        .current_dir(&state.repo_root)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@test.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@test.com")
        .output()
        .unwrap();

    // Archive bob first, then charlie (insertion order != alphabetical).
    let resp = archive_dm(state.clone(), "bob", "alice").await;
    assert!(resp.ok, "archive bob failed: {:?}", resp.error);
    let resp = archive_dm(state.clone(), "charlie", "alice").await;
    assert!(resp.ok, "archive charlie failed: {:?}", resp.error);

    let resp = list_archived_dms(state.clone(), "alice").await;
    assert!(resp.ok);
    let arr = resp.data.unwrap()["dms"].as_array().unwrap().clone();
    assert_eq!(arr.len(), 2);
    // Sorted by peer alphabetically: bob < charlie.
    assert_eq!(arr[0]["peer"].as_str().unwrap(), "bob");
    assert_eq!(arr[1]["peer"].as_str().unwrap(), "charlie");
}

// ─── 9. A.5: write interception — send to archived DM is rejected ─────────────

#[tokio::test]
async fn test_send_to_archived_dm_fails() {
    let (_tmp, state) = setup_test_repo().await;

    // alice archives her DM with bob.
    let resp = archive_dm(state.clone(), "bob", "alice").await;
    assert!(resp.ok, "archive failed: {:?}", resp.error);
    assert!(state
        .repo_root
        .join("archive/dm/alice--bob.thread")
        .exists());

    // alice tries to send to bob via the canonical "dm:" channel form —
    // contract under test: handle_send stats archive/dm/<sorted>.thread
    // before any write. Use both ordering variants so we know the check
    // doesn't depend on caller-provided handler order.
    let resp = send_message(state.clone(), "dm:alice,bob", "hello again", "alice").await;
    assert!(!resp.ok, "send to archived DM should be rejected");
    let err = resp.error.unwrap();
    assert!(
        err.contains("is archived"),
        "err should mention archived: {}",
        err
    );

    // Reverse handler order — same archive file, must still reject. Author
    // is now bob; the implementation derives `peer` from "the participant
    // that isn't the author", so we expect "@alice is archived".
    let resp = send_message(state.clone(), "dm:bob,alice", "ping", "bob").await;
    assert!(!resp.ok, "reversed-order send should also reject");
    let err = resp.error.unwrap();
    assert!(
        err.contains("is archived"),
        "err should mention archived: {}",
        err
    );

    // Sanity: third party charlie can still DM bob (no archive file for
    // bob<->charlie). Send to charlie's DM with bob — should succeed.
    let resp = send_message(state.clone(), "dm:bob,charlie", "side channel", "charlie").await;
    assert!(
        resp.ok,
        "unrelated DM should still be writable: {:?}",
        resp.error
    );
}

// ─── 10. A.5: unarchive_dm works even when called by an active third party ────

#[tokio::test]
async fn test_unarchive_dm_works_after_departure() {
    let (_tmp, state) = setup_test_repo().await;

    // alice archives her DM with bob.
    let resp = archive_dm(state.clone(), "bob", "alice").await;
    assert!(resp.ok, "archive failed: {:?}", resp.error);

    // Now archive bob (so bob is "departed"). alice (active) unarchives
    // the DM — contract: unarchive_dm does NOT gate on archived-author
    // semantics for the *target*, only on the caller's own state. alice
    // is active, so this must succeed.
    //
    // Note: archiving bob removes bob from state.users in-memory, so bob
    // himself can't author the unarchive RPC — this is the realistic
    // production shape. Just verify alice can.
    let resp = archive_user(state.clone(), "bob", "alice").await;
    assert!(resp.ok, "archive bob failed: {:?}", resp.error);

    let resp = unarchive_dm(state.clone(), "bob", "alice").await;
    assert!(
        resp.ok,
        "unarchive_dm by active alice should succeed even with bob departed: {:?}",
        resp.error
    );
    assert!(state.repo_root.join("dm/alice--bob.thread").exists());
}

// ─── 11. A.6: read fallback on archived DM returns content + archived flag ────

#[tokio::test]
async fn test_read_archived_dm_returns_content_with_flag() {
    let (_tmp, state) = setup_test_repo().await;

    // Add a second message so we have multi-line content to verify on
    // read; the setup helper put one. Send via the canonical API path so
    // the message lands in dm/alice--bob.thread with proper formatting.
    let resp = send_message(state.clone(), "dm:alice,bob", "second line", "bob").await;
    assert!(resp.ok, "second send failed: {:?}", resp.error);

    // Sanity-read active DM — must return entries, archived=false.
    let resp = read_thread(state.clone(), "dm:alice,bob").await;
    assert!(resp.ok, "active read failed: {:?}", resp.error);
    let data = resp.data.unwrap();
    assert_eq!(
        data["archived"].as_bool(),
        Some(false),
        "active DM read should report archived=false"
    );
    let entries_active = data["entries"].as_array().unwrap().clone();
    assert!(
        !entries_active.is_empty(),
        "active DM must return entries before archive"
    );

    // Archive the DM. File moves to archive/dm/alice--bob.thread.
    let resp = archive_dm(state.clone(), "bob", "alice").await;
    assert!(resp.ok, "archive failed: {:?}", resp.error);
    assert!(!state.repo_root.join("dm/alice--bob.thread").exists());
    assert!(state
        .repo_root
        .join("archive/dm/alice--bob.thread")
        .exists());

    // Read again — must transparently fall back to the archive, return
    // the same entries, and flip `archived` to true.
    let resp = read_thread(state.clone(), "dm:alice,bob").await;
    assert!(resp.ok, "archived read failed: {:?}", resp.error);
    let data = resp.data.unwrap();
    assert_eq!(
        data["archived"].as_bool(),
        Some(true),
        "archived DM read should report archived=true"
    );
    let entries_archived = data["entries"].as_array().unwrap().clone();
    assert_eq!(
        entries_archived.len(),
        entries_active.len(),
        "archived read should return the same number of entries as active"
    );
    // Tighter than length-only: assert each entry's body / author /
    // line_number match. Catches a regression where the fallback reads
    // a DIFFERENT file with coincidentally the same number of entries.
    for (i, (active_entry, archived_entry)) in entries_active
        .iter()
        .zip(entries_archived.iter())
        .enumerate()
    {
        assert_eq!(
            active_entry.get("body").and_then(|v| v.as_str()),
            archived_entry.get("body").and_then(|v| v.as_str()),
            "entry {} body mismatch between active and archived read",
            i,
        );
        assert_eq!(
            active_entry.get("author").and_then(|v| v.as_str()),
            archived_entry.get("author").and_then(|v| v.as_str()),
            "entry {} author mismatch between active and archived read",
            i,
        );
        assert_eq!(
            active_entry.get("line_number").and_then(|v| v.as_u64()),
            archived_entry.get("line_number").and_then(|v| v.as_u64()),
            "entry {} line_number mismatch between active and archived read",
            i,
        );
    }

    // Reverse handler order in the channel arg — same archive file, same
    // result (resolve_thread_path produces the canonical sorted stem).
    let resp = read_thread(state.clone(), "dm:bob,alice").await;
    assert!(
        resp.ok,
        "archived read with reversed-order DM failed: {:?}",
        resp.error
    );
    let data = resp.data.unwrap();
    assert_eq!(data["archived"].as_bool(), Some(true));
    let entries_reversed = data["entries"].as_array().unwrap().clone();
    assert_eq!(
        entries_reversed.len(),
        entries_archived.len(),
        "reversed-order read should resolve to the same archived thread"
    );
    // Same body-by-body equality as above — proves resolve_thread_path
    // canonicalizes to the identical archive file regardless of arg order.
    for (i, (forward, reversed)) in entries_archived
        .iter()
        .zip(entries_reversed.iter())
        .enumerate()
    {
        assert_eq!(
            forward.get("body").and_then(|v| v.as_str()),
            reversed.get("body").and_then(|v| v.as_str()),
            "entry {} body mismatch between forward and reversed-order read",
            i,
        );
    }
}

// ─── 12. A.6: read on a never-existed DM returns the empty / not-found path ───

#[tokio::test]
async fn test_read_nonexistent_dm_returns_empty() {
    let (_tmp, state) = setup_test_repo().await;

    // alice<->charlie DM never existed, never archived.
    assert!(!state.repo_root.join("dm/alice--charlie.thread").exists());
    assert!(!state
        .repo_root
        .join("archive/dm/alice--charlie.thread")
        .exists());

    // Current handle_read semantics: missing thread file is treated as
    // empty (read_to_string defaults to "" when not present), so the
    // response is ok with zero entries and archived=false. The contract
    // is "no archive fallback can hide content that doesn't exist
    // anywhere" — assert the read does not spuriously claim archived=true.
    let resp = read_thread(state.clone(), "dm:alice,charlie").await;
    assert!(
        resp.ok,
        "read of nonexistent DM should not error: {:?}",
        resp.error
    );
    let data = resp.data.unwrap();
    assert_eq!(
        data["archived"].as_bool(),
        Some(false),
        "missing DM must not report archived=true"
    );
    assert!(
        data["entries"].as_array().unwrap().is_empty(),
        "missing DM should return zero entries"
    );
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

// ─── 13. A.7: poll surfaces `dm_archived` for the alice<->bob DM ─────────────

#[tokio::test]
async fn test_poll_emits_dm_archived_event() {
    let (_tmp, state) = setup_test_repo().await;

    // Cursor before the archive — captures HEAD as it stands. The setup
    // helper already commits the initial DM file; archive will produce a
    // single new commit that the diff range below should capture.
    let cursor = handle_request(Request::Poll { since: None }, state.clone())
        .await
        .data
        .unwrap()["commit_id"]
        .as_str()
        .unwrap()
        .to_string();

    // alice archives the DM with bob.
    let resp = archive_dm(state.clone(), "bob", "alice").await;
    assert!(resp.ok, "archive failed: {:?}", resp.error);

    // Poll since the pre-archive cursor — must see a `dm_archived` event
    // keyed off the canonical `dm:alice,bob` channel form.
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
    let archived_hit = changes
        .iter()
        .any(|c| c["kind"] == "dm_archived" && c["channel"] == "dm:alice,bob");
    assert!(
        archived_hit,
        "expected dm_archived event for dm:alice,bob, got: {:#?}",
        changes,
    );

    // The event should carry no entries — it's a path-shaped notification,
    // mirror of `channel_meta`. Clients refetch; they don't read the
    // archived thread inline.
    for c in &changes {
        if c["kind"] == "dm_archived" {
            assert!(
                c["entries"].as_array().unwrap().is_empty(),
                "dm_archived event must have empty entries, got: {:?}",
                c["entries"],
            );
        }
    }

    // No spurious `dm` event for the same archive operation — the active
    // path is being deleted, the only re-appearing path is in archive/dm/,
    // so the active `dm/` branch must not fire.
    let active_dm_hit = changes
        .iter()
        .any(|c| c["kind"] == "dm" && c["channel"] == "dm:alice,bob");
    assert!(
        !active_dm_hit,
        "archive must not produce a phantom `dm` event for the same DM",
    );
}

// ─── 14. A.7: dm_archived visibility — third party must NOT see it ───────────

#[tokio::test]
async fn test_poll_dm_archived_visibility_respects_participants() {
    let (_tmp, state) = setup_test_repo().await;

    // Add a second DM file alice<->charlie so we have a non-bob DM to
    // archive (charlie is a third party from bob's perspective).
    std::fs::write(
        state.repo_root.join("dm/alice--charlie.thread"),
        "[L000001][P000000][@alice][20260509T101000Z] hey charlie\n",
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
    run_git(&["add", "dm/alice--charlie.thread"]);
    run_git(&["commit", "-m", "add charlie dm"]);

    // Switch the daemon's "current user" to bob — he's NOT a participant
    // in the alice<->charlie DM and must not see its archived event.
    {
        let mut me = state.current_user.write().await;
        *me = Some("bob".to_string());
    }

    let cursor = handle_request(Request::Poll { since: None }, state.clone())
        .await
        .data
        .unwrap()["commit_id"]
        .as_str()
        .unwrap()
        .to_string();

    // alice archives her DM with charlie. bob should not see this event.
    let resp = archive_dm(state.clone(), "charlie", "alice").await;
    assert!(resp.ok, "archive failed: {:?}", resp.error);

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

    // bob is a third party — must not see the dm_archived event for
    // alice<->charlie.
    let leak = changes
        .iter()
        .any(|c| c["kind"] == "dm_archived" && c["channel"] == "dm:alice,charlie");
    assert!(
        !leak,
        "third-party bob must not see dm_archived for alice<->charlie, got: {:#?}",
        changes,
    );
}

// ─── 15. A.7: dm unarchive emits naturally via the active `dm/` branch ───────
//
// Channel unarchive surfaces as a `kind: "channel"` event with the thread
// content — there's no dedicated `channel_unarchived`, the active path
// re-appearing in the diff is enough. DM unarchive must do the same:
// `dm/<sorted>.thread` re-appears, the existing `dm/` branch picks it up,
// emits `kind: "dm"` with entries. This test pins that contract.

#[tokio::test]
async fn test_poll_emits_dm_unarchived_event() {
    let (_tmp, state) = setup_test_repo().await;

    // Archive first.
    let resp = archive_dm(state.clone(), "bob", "alice").await;
    assert!(resp.ok, "archive failed: {:?}", resp.error);

    // Cursor between archive and unarchive — anchors the diff to a state
    // where the active path is gone.
    let cursor = handle_request(Request::Poll { since: None }, state.clone())
        .await
        .data
        .unwrap()["commit_id"]
        .as_str()
        .unwrap()
        .to_string();

    // Now unarchive. Active `dm/alice--bob.thread` re-appears in the diff.
    let resp = unarchive_dm(state.clone(), "bob", "alice").await;
    assert!(resp.ok, "unarchive failed: {:?}", resp.error);

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

    // Symmetric to channel unarchive: a `kind: "dm"` event for the
    // canonical `dm:alice,bob` channel, carrying the thread entries.
    let dm_hit = changes
        .iter()
        .find(|c| c["kind"] == "dm" && c["channel"] == "dm:alice,bob");
    assert!(
        dm_hit.is_some(),
        "expected dm event for dm:alice,bob after unarchive, got: {:#?}",
        changes,
    );
    let entries = dm_hit.unwrap()["entries"].as_array().unwrap();
    assert!(
        !entries.is_empty(),
        "unarchive should surface thread content (active path re-appears with full file)",
    );
}
