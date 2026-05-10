//! Cross-clone propagation test for `handle_depart_user` (archive-protocol
//! plan A.8, P1.b case 5).
//!
//! Pattern: bare repo (origin) + working clone A. Run depart on clone A,
//! then clone the bare into clone B and verify that B sees the entire
//! depart artifact set — leave-workspace event in the channel thread,
//! DM in archive/dm/, alice meta in archive/users/, dev.meta.yaml with
//! alice removed from members. This is the "remote/origin state
//! verification" form: instead of standing up a second daemon (heavy,
//! cross-process), we read the bare repo through a fresh clone, which
//! is exactly what a second machine would see after `git fetch`.
//!
//! Per the A.8 plan: "the simplest sufficient version: setup two clones
//! using existing test helpers ... [or] just `git clone --bare` of the
//! test repo and clone again, verify file state."

use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::broadcast;

use gitim_core::types::Config;
use gitim_daemon::api::{Event, Request};
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::AppState;

fn run_git(dir: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@test.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@test.com")
        .output()
        .expect("git command failed");
    assert!(
        output.status.success(),
        "git {:?} failed in {}: {}",
        args,
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn make_config() -> Config {
    serde_yaml::from_str("version: 1").unwrap()
}

/// Set up bare repo + working clone with alice + bob registered. Returns
/// (bare_dir, clone_dir, AppState). The clone has a remote pointed at the
/// bare, so depart_user's per-phase pushes propagate to origin.
async fn setup_with_remote_two_users() -> (TempDir, TempDir, Arc<AppState>) {
    let bare_dir = TempDir::new().unwrap();
    let clone_dir = TempDir::new().unwrap();

    run_git(bare_dir.path(), &["init", "--bare"]);

    run_git(
        clone_dir.path().parent().unwrap(),
        &[
            "clone",
            bare_dir.path().to_str().unwrap(),
            clone_dir.path().to_str().unwrap(),
        ],
    );
    run_git(clone_dir.path(), &["config", "user.email", "test@test.com"]);
    run_git(clone_dir.path(), &["config", "user.name", "test"]);

    let root = clone_dir.path().to_path_buf();

    std::fs::create_dir_all(root.join("channels")).unwrap();
    std::fs::create_dir_all(root.join("users")).unwrap();
    std::fs::create_dir_all(root.join(".gitim")).unwrap();
    std::fs::write(root.join(".gitim/config.yaml"), "version: 1").unwrap();
    for h in ["alice", "bob"] {
        std::fs::write(
            root.join(format!("users/{}.meta.yaml", h)),
            format!("display_name: {}\nrole: dev\nintroduction: hi\n", h),
        )
        .unwrap();
    }

    run_git(&root, &["add", "."]);
    run_git(&root, &["commit", "-m", "initial structure"]);
    run_git(&root, &["push", "-u", "origin", "HEAD"]);

    let (event_tx, _) = broadcast::channel::<Event>(256);
    let state = Arc::new(AppState::new(
        root,
        make_config(),
        event_tx,
        Some("alice".to_string()),
    ));
    {
        let mut users = state.users.write().await;
        *users = vec!["alice".to_string(), "bob".to_string()];
    }

    (bare_dir, clone_dir, state)
}

/// Clone the bare repo into a fresh tempdir for verification — simulates
/// what a second machine sees after `git fetch`.
fn clone_bare(bare_path: &Path) -> TempDir {
    let verify = TempDir::new().unwrap();
    run_git(
        verify.path().parent().unwrap(),
        &[
            "clone",
            bare_path.to_str().unwrap(),
            verify.path().to_str().unwrap(),
        ],
    );
    verify
}

async fn create_channel(
    state: Arc<AppState>,
    name: &str,
    author: &str,
    invitees: &[&str],
) -> gitim_daemon::api::Response {
    let invitees_json: Vec<String> = invitees.iter().map(|s| s.to_string()).collect();
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_channel",
        "name": name,
        "author": author,
        "invitees": invitees_json,
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

async fn depart_user(state: Arc<AppState>, handler: &str) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "depart_user",
        "handler": handler,
    }))
    .unwrap();
    handle_request(req, state).await
}

// ─── Cross-clone burn propagation ───────────────────────────────────────────
//
// Setup clone A with alice + bob, alice creates #dev (with bob), alice and
// bob each post, alice DMs bob. Burn alice from A. Then clone the bare repo
// into clone B and verify B sees the complete depart artifact set:
//
//   - channels/dev.thread ends with alice's leave-workspace event
//   - dm/alice--bob.thread is GONE (moved to archive)
//   - archive/dm/alice--bob.thread exists
//   - archive/users/alice.meta.yaml exists
//   - users/alice.meta.yaml is GONE
//   - channels/dev.meta.yaml's members no longer contains alice
//   - git log on clone B contains all four phase commits
//
// This proves the burn pushes are durable across the bare and that a fresh
// fetch yields a workspace where alice has been fully departed. A second
// daemon spawned on clone B would read this state and treat alice as
// departed — that's the contract under test.

#[tokio::test]
async fn test_cross_clone_burn_propagation() {
    let (bare_dir, _clone_a_dir, state) = setup_with_remote_two_users().await;

    // Spawn the sync loop so depart_user's per-phase pushes have a
    // committed remote tip to land against. Without it the pushes would
    // still go through (each phase calls push_with_retry directly), but
    // the loop catches the "should be a no-op" later sanity passes.
    AppState::spawn_sync_loop(state.clone());
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Setup: alice creates #dev with bob, both post, alice DMs bob.
    let resp = create_channel(state.clone(), "dev", "alice", &["bob"]).await;
    assert!(resp.ok, "create #dev failed: {:?}", resp.error);
    let resp = send_message(state.clone(), "dev", "hi from alice", "alice").await;
    assert!(resp.ok, "alice's send failed: {:?}", resp.error);
    let resp = send_message(state.clone(), "dev", "bob here", "bob").await;
    assert!(resp.ok, "bob's send failed: {:?}", resp.error);
    let resp = send_message(state.clone(), "dm:alice,bob", "hey bob", "alice").await;
    assert!(resp.ok, "alice's DM to bob failed: {:?}", resp.error);

    // Burn alice on clone A.
    let resp = depart_user(state.clone(), "alice").await;
    assert!(resp.ok, "depart_user failed on clone A: {:?}", resp.error);

    // Give any in-flight sync loop work a moment to settle. depart_user
    // pushes synchronously per phase, so the bare should already be in
    // its terminal state — but a brief wait insulates against scheduler
    // jitter on heavily loaded CI.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    // Now clone the bare into B and verify the full depart artifact set
    // is visible. This is exactly what a second daemon on a second
    // machine would see after `git fetch`.
    let clone_b = clone_bare(bare_dir.path());
    let b_root = clone_b.path();

    // 1. Phase 1: channels/dev.thread carries alice's leave-workspace event.
    let dev_thread = std::fs::read_to_string(b_root.join("channels/dev.thread"))
        .expect("clone B should have channels/dev.thread");
    let last = dev_thread.lines().last().expect("dev.thread non-empty");
    assert!(
        last.contains("@alice") && last.contains("[E:leave-workspace]"),
        "clone B's dev.thread should end with alice's leave-workspace event:\nlast: {}\nfull:\n{}",
        last,
        dev_thread,
    );

    // 2. Phase 2: dm/alice--bob.thread moved to archive/dm/.
    assert!(
        !b_root.join("dm/alice--bob.thread").exists(),
        "clone B should NOT have active dm/alice--bob.thread"
    );
    assert!(
        b_root.join("archive/dm/alice--bob.thread").exists(),
        "clone B SHOULD have archive/dm/alice--bob.thread"
    );

    // 3. Phase 3: alice removed from #dev members.
    let dev_meta_str = std::fs::read_to_string(b_root.join("channels/dev.meta.yaml")).unwrap();
    let dev_meta: gitim_core::types::ChannelMeta = serde_yaml::from_str(&dev_meta_str).unwrap();
    assert!(
        !dev_meta.members.iter().any(|m| m == "alice"),
        "clone B's dev.meta.yaml should not have alice in members: {:?}",
        dev_meta.members
    );
    assert!(
        dev_meta.members.iter().any(|m| m == "bob"),
        "clone B's dev.meta.yaml should still have bob: {:?}",
        dev_meta.members
    );

    // 4. Phase 4: users/alice.meta.yaml moved to archive/users/.
    assert!(
        !b_root.join("users/alice.meta.yaml").exists(),
        "clone B should NOT have active users/alice.meta.yaml"
    );
    assert!(
        b_root.join("archive/users/alice.meta.yaml").exists(),
        "clone B SHOULD have archive/users/alice.meta.yaml"
    );

    // 5. git log on clone B contains the burn commit subjects in the
    //    expected order — Phase 1 (leave events) precede Phase 2 (DM mv)
    //    precede Phase 3 (member cleanup) precede Phase 4 (user mv).
    //    `git log` returns newest first, so the depart commit is at the
    //    top and the early Phase 1 events are deeper.
    let log = std::process::Command::new("git")
        .args(["log", "--pretty=%s"])
        .current_dir(b_root)
        .output()
        .unwrap();
    let log_text = String::from_utf8_lossy(&log.stdout).to_string();
    assert!(
        log_text.contains("event: @alice leave-workspace"),
        "clone B's log should contain Phase 1 commit:\n{}",
        log_text
    );
    assert!(
        log_text.contains("archive: dm alice--bob"),
        "clone B's log should contain Phase 2 commit:\n{}",
        log_text
    );
    assert!(
        log_text.contains("channel: remove @alice from #dev members"),
        "clone B's log should contain Phase 3 commit:\n{}",
        log_text
    );
    assert!(
        log_text.contains("archive: depart user @alice"),
        "clone B's log should contain Phase 4 commit:\n{}",
        log_text
    );

    // Phase ordering: index of Phase 4 (newest, smallest line index in `git
    // log` reverse-chrono) must be smaller than index of Phase 1.
    let phase4_idx = log_text
        .lines()
        .position(|l| l.contains("archive: depart user @alice"))
        .unwrap();
    let phase1_idx = log_text
        .lines()
        .position(|l| l.contains("event: @alice leave-workspace"))
        .unwrap();
    assert!(
        phase4_idx < phase1_idx,
        "Phase 4 must be newer than Phase 1 in git log (lower line idx = newer):\nP4 idx={}, P1 idx={}\n{}",
        phase4_idx,
        phase1_idx,
        log_text
    );
}
