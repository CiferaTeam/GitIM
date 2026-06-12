#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Integration tests for `handle_depart_user` — the composite "burn"
//! operation defined in archive-protocol plan A.4.
//!
//! Pattern mirrors archive_user_test.rs / archive_dm_test.rs:
//! temp git repo + AppState in-process, exercise via `handle_request`.
//! No daemon process spawned.

mod common;

use std::sync::Arc;

use tempfile::TempDir;

use gitim_daemon::api::Request;
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::AppState;

/// Build a temp git repo with alice + bob + carol registered. Caller
/// adds channels / DMs as needed for the specific test.
async fn setup_test_repo() -> (tempfile::TempDir, Arc<AppState>) {
    common::setup_repo_with_users(&["alice", "bob", "carol"]).await
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

fn git_log_subjects(root: &std::path::Path) -> Vec<String> {
    let out = std::process::Command::new("git")
        .args(["log", "--pretty=%s"])
        .current_dir(root)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|s| s.to_string())
        .collect()
}

fn git_log_authors(root: &std::path::Path) -> Vec<(String, String)> {
    let out = std::process::Command::new("git")
        .args(["log", "--pretty=%s||%an"])
        .current_dir(root)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut sp = l.splitn(2, "||");
            let s = sp.next()?.to_string();
            let a = sp.next()?.to_string();
            Some((s, a))
        })
        .collect()
}

fn read_thread(root: &std::path::Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_default()
}

// ─── 1. happy path: alice has spoken in #dev / #ops, has a DM with bob,
//        is in #dev members. Burn alice → all four phases land. ────────────

#[tokio::test]
async fn test_depart_user_happy_path() {
    let (_tmp, state) = setup_test_repo().await;

    // Setup: alice creates #dev (with bob), creates #ops, sends messages
    // in both channels, and bob sends to alice's #dev so the DM exists.
    let resp = create_channel(state.clone(), "dev", "alice", &["bob"]).await;
    assert!(resp.ok, "create #dev failed: {:?}", resp.error);
    let resp = create_channel(state.clone(), "ops", "alice", &[]).await;
    assert!(resp.ok, "create #ops failed: {:?}", resp.error);

    let resp = send_message(state.clone(), "dev", "hi from alice", "alice").await;
    assert!(resp.ok);
    let resp = send_message(state.clone(), "ops", "ops msg", "alice").await;
    assert!(resp.ok);

    // alice DMs bob.
    let resp = send_message(state.clone(), "dm:alice,bob", "hey bob", "alice").await;
    assert!(resp.ok, "alice dm to bob failed: {:?}", resp.error);

    // Pre-check: alice is in #dev members.
    let dev_meta_str =
        std::fs::read_to_string(state.repo_root.join("channels/dev.meta.yaml")).unwrap();
    assert!(
        dev_meta_str.contains("alice"),
        "alice should be in #dev members pre-burn: {}",
        dev_meta_str
    );

    let pre_log = git_log_subjects(&state.repo_root);

    // Burn alice.
    let resp = depart_user(state.clone(), "alice").await;
    assert!(resp.ok, "depart_user failed: {:?}", resp.error);
    let data = resp.data.unwrap();
    assert_eq!(data["handler"].as_str().unwrap(), "alice");
    assert!(!data["already_departed"].as_bool().unwrap());
    let commits = data["commits"].as_u64().unwrap();
    assert!(
        commits >= 4,
        "expected at least 4 commits (1 dev leave + 1 ops leave + 1 DM mv + 1 dev members + 1 user mv); got {}",
        commits
    );

    // Phase 1 verification: each thread alice spoke in has her leave-workspace at the end.
    let dev_thread = read_thread(&state.repo_root, "channels/dev.thread");
    assert!(
        dev_thread.lines().last().unwrap().contains("@alice")
            && dev_thread
                .lines()
                .last()
                .unwrap()
                .contains("[E:leave-workspace]"),
        "dev.thread last line missing alice leave-workspace event:\n{}",
        dev_thread
    );
    let ops_thread = read_thread(&state.repo_root, "channels/ops.thread");
    assert!(
        ops_thread.lines().last().unwrap().contains("@alice")
            && ops_thread
                .lines()
                .last()
                .unwrap()
                .contains("[E:leave-workspace]"),
        "ops.thread last line missing alice leave-workspace event:\n{}",
        ops_thread
    );
    // The DM ends up in archive/dm/ after Phase 2 — leave event lands
    // there too because Phase 1 wrote to dm/ before Phase 2 moved it.
    let dm_archived = read_thread(&state.repo_root, "archive/dm/alice--bob.thread");
    assert!(
        dm_archived.contains("@alice") && dm_archived.contains("[E:leave-workspace]"),
        "alice's leave-workspace event should be in archived DM:\n{}",
        dm_archived
    );

    // Phase 2 verification: dm/alice--bob.thread moved to archive/dm/.
    assert!(
        !state.repo_root.join("dm/alice--bob.thread").exists(),
        "active DM should be gone"
    );
    assert!(
        state
            .repo_root
            .join("archive/dm/alice--bob.thread")
            .exists(),
        "DM should be archived"
    );

    // Phase 3 verification: alice removed from #dev members. The
    // `created_by: alice` field in the YAML is meta-historical and
    // intentionally not rewritten, so a substring check on the whole
    // file would match it; parse and inspect the members list directly.
    let dev_meta_post: gitim_core::types::ChannelMeta = serde_yaml::from_str(
        &std::fs::read_to_string(state.repo_root.join("channels/dev.meta.yaml")).unwrap(),
    )
    .unwrap();
    assert!(
        !dev_meta_post.members.iter().any(|m| m == "alice"),
        "alice should be removed from #dev members: {:?}",
        dev_meta_post.members
    );
    assert!(
        dev_meta_post.members.iter().any(|m| m == "bob"),
        "bob should still be in #dev members: {:?}",
        dev_meta_post.members
    );

    // Phase 4 verification: users/alice.meta.yaml moved to archive/users/.
    assert!(
        !state.repo_root.join("users/alice.meta.yaml").exists(),
        "active user meta should be gone"
    );
    assert!(
        state
            .repo_root
            .join("archive/users/alice.meta.yaml")
            .exists(),
        "archived user meta should exist"
    );

    // In-memory users list updated.
    {
        let users = state.users.read().await;
        assert!(!users.contains(&"alice".to_string()));
        assert!(users.contains(&"bob".to_string()));
    }

    // Commits look right: depart-related commits appear in log.
    let post_log = git_log_subjects(&state.repo_root);
    let new_commits: Vec<&String> = post_log
        .iter()
        .take(post_log.len() - pre_log.len())
        .collect();
    let joined = new_commits
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("archive: depart user @alice"),
        "expected phase 4 commit in log:\n{}",
        joined
    );
    assert!(
        joined.contains("archive: dm alice--bob"),
        "expected phase 2 commit in log:\n{}",
        joined
    );
    assert!(
        joined.contains("event: @alice leave-workspace"),
        "expected phase 1 commit in log:\n{}",
        joined
    );
    assert!(
        joined.contains("channel: remove @alice from #dev members"),
        "expected phase 3 commit in log:\n{}",
        joined
    );
}

// ─── 2. idempotent: second depart_user is a no-op. ──────────────────────────

#[tokio::test]
async fn test_depart_user_idempotent() {
    let (_tmp, state) = setup_test_repo().await;

    // Minimal scenario — alice has no DMs / channels / messages, just exists.
    // Phases 1-3 do nothing; Phase 4 runs once.

    let resp = depart_user(state.clone(), "alice").await;
    assert!(resp.ok, "first depart failed: {:?}", resp.error);
    let first_commits = resp.data.unwrap()["commits"].as_u64().unwrap();
    assert_eq!(
        first_commits, 1,
        "no channels/DMs — only Phase 4 should commit once"
    );

    let log_after_first = git_log_subjects(&state.repo_root);

    // Second call must short-circuit at the terminal-state check.
    let resp = depart_user(state.clone(), "alice").await;
    assert!(resp.ok, "idempotent retry failed: {:?}", resp.error);
    let data = resp.data.unwrap();
    assert!(data["already_departed"].as_bool().unwrap());
    assert_eq!(data["commits"].as_u64().unwrap(), 0);

    // No new commits.
    let log_after_second = git_log_subjects(&state.repo_root);
    assert_eq!(
        log_after_first, log_after_second,
        "idempotent retry must not add commits"
    );
}

// ─── 3. partial-completion recovery: simulate Phase 1 finishing only some
//        threads, then retry → resumes from where it left off. ──────────────

#[tokio::test]
async fn test_depart_user_partial_completion_recovery() {
    let (_tmp, state) = setup_test_repo().await;

    // Setup: alice in 3 channels with messages.
    create_channel(state.clone(), "ch1", "alice", &[]).await;
    create_channel(state.clone(), "ch2", "alice", &[]).await;
    create_channel(state.clone(), "ch3", "alice", &[]).await;
    send_message(state.clone(), "ch1", "1", "alice").await;
    send_message(state.clone(), "ch2", "2", "alice").await;
    send_message(state.clone(), "ch3", "3", "alice").await;

    // Manually pre-write a leave-workspace event on ch1 so Phase 1 will
    // skip it. We append it as a real event line + commit it to mirror
    // what a partial run would have produced.
    let ch1_thread = state.repo_root.join("channels/ch1.thread");
    let cur = std::fs::read_to_string(&ch1_thread).unwrap();
    let next_line: u64 = cur
        .lines()
        .filter_map(|l| {
            l.strip_prefix("[L")
                .and_then(|s| s.split(']').next())
                .and_then(|s| s.parse::<u64>().ok())
        })
        .max()
        .unwrap_or(0)
        + 1;
    let pre_event = format!(
        "[L{:06}][P{:06}][@alice][20260509T120000Z][E:leave-workspace] {{}}\n",
        next_line, 0
    );
    std::fs::write(&ch1_thread, format!("{}{}", cur, pre_event).as_bytes()).unwrap();
    std::process::Command::new("git")
        .args(["add", "channels/ch1.thread"])
        .current_dir(&state.repo_root)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "event: @alice leave-workspace"])
        .env("GIT_AUTHOR_NAME", "alice")
        .env("GIT_AUTHOR_EMAIL", "alice@gitim")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@test.com")
        .current_dir(&state.repo_root)
        .output()
        .unwrap();

    let pre_log = git_log_subjects(&state.repo_root);

    // Now run depart_user. Phase 1 should skip ch1 (already has the event),
    // commit ch2 + ch3, then run Phases 2-4.
    let resp = depart_user(state.clone(), "alice").await;
    assert!(resp.ok, "depart_user failed: {:?}", resp.error);

    // ch1's last line is still alice's pre-existing leave-workspace event
    // (no duplicate appended).
    let ch1_post = read_thread(&state.repo_root, "channels/ch1.thread");
    let leave_count = ch1_post
        .lines()
        .filter(|l| l.contains("[E:leave-workspace]") && l.contains("@alice"))
        .count();
    assert_eq!(
        leave_count, 1,
        "ch1 must have exactly one leave-workspace event (no duplicate), got:\n{}",
        ch1_post
    );

    // ch2 + ch3 each have a leave event now.
    let ch2_post = read_thread(&state.repo_root, "channels/ch2.thread");
    assert!(
        ch2_post.contains("[E:leave-workspace]") && ch2_post.contains("@alice"),
        "ch2 missing alice leave event:\n{}",
        ch2_post
    );
    let ch3_post = read_thread(&state.repo_root, "channels/ch3.thread");
    assert!(
        ch3_post.contains("[E:leave-workspace]") && ch3_post.contains("@alice"),
        "ch3 missing alice leave event:\n{}",
        ch3_post
    );

    // Phase 4 ran.
    assert!(
        state
            .repo_root
            .join("archive/users/alice.meta.yaml")
            .exists(),
        "phase 4 should have completed"
    );

    // Verify only 2 leave-workspace commits added (for ch2 + ch3), not 3.
    let post_log = git_log_subjects(&state.repo_root);
    let new = post_log[..post_log.len() - pre_log.len()].to_vec();
    let leave_commits = new
        .iter()
        .filter(|s| s.contains("event: @alice leave-workspace"))
        .count();
    assert_eq!(
        leave_commits,
        2,
        "expected exactly 2 NEW leave-workspace commits (ch2, ch3); got {} in:\n{}",
        leave_commits,
        new.join("\n")
    );
}

// ─── 4. zero-message agent: alice never wrote anything → Phase 1 has no
//        threads to touch; Phases 2-4 still run. ──────────────────────────────

#[tokio::test]
async fn test_depart_user_zero_message_agent() {
    let (_tmp, state) = setup_test_repo().await;

    // alice exists, never speaks. Carol creates a channel that alice is
    // in, but alice never posts.
    let resp = create_channel(state.clone(), "team", "carol", &["alice", "bob"]).await;
    assert!(resp.ok, "create #team failed: {:?}", resp.error);

    // bob DMs alice — alice doesn't reply, so the active dm exists with
    // bob as the sole author.
    send_message(state.clone(), "dm:alice,bob", "you there?", "bob").await;

    let resp = depart_user(state.clone(), "alice").await;
    assert!(resp.ok, "depart_user failed: {:?}", resp.error);
    let data = resp.data.unwrap();
    assert!(!data["already_departed"].as_bool().unwrap());

    // Phase 1: alice never authored anything in #team → no leave-workspace
    // event should be written there.
    let team_thread = read_thread(&state.repo_root, "channels/team.thread");
    let alice_leave_count = team_thread
        .lines()
        .filter(|l| l.contains("@alice") && l.contains("[E:leave-workspace]"))
        .count();
    assert_eq!(
        alice_leave_count, 0,
        "no leave-workspace expected (alice never spoke):\n{}",
        team_thread
    );

    // Phase 1 should also skip the DM — alice never wrote in it. But it
    // gets archived in Phase 2 regardless (alice is a participant by
    // filename). Verify no leave event landed in the archived thread.
    let dm_archived = read_thread(&state.repo_root, "archive/dm/alice--bob.thread");
    let dm_leave_count = dm_archived
        .lines()
        .filter(|l| l.contains("@alice") && l.contains("[E:leave-workspace]"))
        .count();
    assert_eq!(
        dm_leave_count, 0,
        "no leave-workspace expected in DM (alice never authored there):\n{}",
        dm_archived
    );

    // Phase 2: DM archived.
    assert!(
        state
            .repo_root
            .join("archive/dm/alice--bob.thread")
            .exists(),
        "DM should be archived"
    );

    // Phase 3: alice removed from #team members. (Carol creates the
    // channel so `created_by` is carol — but parse the YAML rather than
    // string-match to avoid the same trap as test_depart_user_happy_path.)
    let team_meta: gitim_core::types::ChannelMeta = serde_yaml::from_str(
        &std::fs::read_to_string(state.repo_root.join("channels/team.meta.yaml")).unwrap(),
    )
    .unwrap();
    assert!(
        !team_meta.members.iter().any(|m| m == "alice"),
        "alice should be removed from #team members: {:?}",
        team_meta.members
    );

    // Phase 4: terminal state met.
    assert!(state
        .repo_root
        .join("archive/users/alice.meta.yaml")
        .exists());
}

// ─── 5. alice has no DMs and is in 0 channel members lists. Phases 2/3
//        should clean-skip; only Phase 4 commits. ─────────────────────────────

#[tokio::test]
async fn test_depart_user_minimal_state() {
    let (_tmp, state) = setup_test_repo().await;

    // alice is registered, never joins anything, never DMs. depart_user
    // should be a single Phase 4 commit.
    let resp = depart_user(state.clone(), "alice").await;
    assert!(resp.ok, "depart_user failed: {:?}", resp.error);
    let data = resp.data.unwrap();
    assert_eq!(
        data["commits"].as_u64().unwrap(),
        1,
        "minimal alice — only Phase 4 should commit"
    );
    assert!(state
        .repo_root
        .join("archive/users/alice.meta.yaml")
        .exists());
}

// ─── 6. commit author for leave-workspace events = handler herself.
//        The depart op is daemon-driven but signed as the agent — mirrors
//        the leave-channel author convention. ───────────────────────────────

#[tokio::test]
async fn test_depart_user_leave_event_commit_author() {
    let (_tmp, state) = setup_test_repo().await;

    create_channel(state.clone(), "dev", "alice", &[]).await;
    send_message(state.clone(), "dev", "speaking", "alice").await;

    let resp = depart_user(state.clone(), "alice").await;
    assert!(resp.ok, "depart failed: {:?}", resp.error);

    let log = git_log_authors(&state.repo_root);
    // Find the leave-workspace commit and check its author.
    let leave_entry = log
        .iter()
        .find(|(s, _)| s.contains("event: @alice leave-workspace"));
    assert!(
        leave_entry.is_some(),
        "no leave-workspace commit in log: {:?}",
        log
    );
    let (_, author) = leave_entry.unwrap();
    assert_eq!(
        author, "alice",
        "leave-workspace commit author should be alice"
    );

    let mv_entry = log
        .iter()
        .find(|(s, _)| s.contains("archive: depart user @alice"));
    assert!(mv_entry.is_some(), "no Phase 4 commit in log");
    let (_, mv_author) = mv_entry.unwrap();
    assert_eq!(
        mv_author, "alice",
        "Phase 4 commit author should be alice (handler self-departs)"
    );
}

// ─── 7. depart on never-registered handler → clean error, no commits. ───────

#[tokio::test]
async fn test_depart_unknown_handler() {
    let (_tmp, state) = setup_test_repo().await;

    let pre_log = git_log_subjects(&state.repo_root);

    let resp = depart_user(state.clone(), "ghost").await;
    assert!(!resp.ok, "depart of unregistered should fail");
    let err = resp.error.unwrap();
    assert!(
        err.contains("not found"),
        "expected 'not found' error, got: {}",
        err
    );

    // No git side-effects.
    let post_log = git_log_subjects(&state.repo_root);
    assert_eq!(pre_log, post_log, "no commits should be created");
}

// ─── 8. archived DMs are skipped in Phase 1 (decision (a) in the plan).
//        An already-archived DM where alice has spoken does NOT receive a
//        leave-workspace event. ──────────────────────────────────────────────

#[tokio::test]
async fn test_depart_user_skips_already_archived_dms_in_phase1() {
    let (_tmp, state) = setup_test_repo().await;

    // alice DMs bob, then bob archives the DM (so it's frozen audit data
    // before the burn).
    send_message(state.clone(), "dm:alice,bob", "from alice", "alice").await;
    let resp: gitim_daemon::api::Response = handle_request(
        serde_json::from_value(serde_json::json!({
            "method": "archive_dm",
            "peer": "alice",
            "author": "bob",
        }))
        .unwrap(),
        state.clone(),
    )
    .await;
    assert!(resp.ok, "pre-archive of DM failed: {:?}", resp.error);

    // Confirm the DM is in archive/.
    assert!(state
        .repo_root
        .join("archive/dm/alice--bob.thread")
        .exists());
    let pre_archived_thread = read_thread(&state.repo_root, "archive/dm/alice--bob.thread");

    // Burn alice. The archived DM should NOT get a leave-workspace event.
    let resp = depart_user(state.clone(), "alice").await;
    assert!(resp.ok, "depart failed: {:?}", resp.error);

    let post_archived_thread = read_thread(&state.repo_root, "archive/dm/alice--bob.thread");
    assert_eq!(
        pre_archived_thread, post_archived_thread,
        "already-archived DM must remain untouched by depart_user"
    );

    // Phase 4 still landed.
    assert!(state
        .repo_root
        .join("archive/users/alice.meta.yaml")
        .exists());
}

// ─── 9. concurrent depart-vs-send (P1.b case 2): a non-burning user's send
//        runs while alice is in mid-burn. Both must complete cleanly because
//        the gate (`ensure_author_not_departed`) keys on the burning handler,
//        not on others, and `commit_lock` serializes the underlying writes.
//
//        This exercises the same retry/serialization path that production
//        relies on. We use a multi-thread tokio runtime so the two tasks
//        actually interleave on the executor; commit_lock then arbitrates
//        atomically between the leave-workspace appends and carol's send.
//
//        Note: the contract under test is "neither side errors". The
//        ordering of carol's send vs alice's leave-workspace events is
//        non-deterministic — both must exist in the final thread, and the
//        thread must parse cleanly with strictly increasing line numbers.
//
//        Per A.8 plan: this is the "scaled-back" version — full process-
//        level concurrent depart vs concurrent send is exercised in
//        production; here we cover the in-process surface that the lock +
//        retry logic actually controls. ────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_depart_user_concurrent_with_send() {
    let (_tmp, state) = setup_test_repo().await;

    // Setup: alice + bob + carol all in #dev. alice has spoken so Phase 1
    // will write a leave-workspace event for #dev. carol will fire a send
    // to #dev concurrently with alice's burn.
    let resp = create_channel(state.clone(), "dev", "alice", &["bob", "carol"]).await;
    assert!(resp.ok, "create #dev failed: {:?}", resp.error);
    let resp = send_message(state.clone(), "dev", "hi from alice", "alice").await;
    assert!(resp.ok, "alice's pre-burn send failed: {:?}", resp.error);
    let resp = send_message(state.clone(), "dev", "bob here", "bob").await;
    assert!(resp.ok, "bob's pre-burn send failed: {:?}", resp.error);

    // Snapshot the line count before the concurrent dispatch — we expect
    // carol's message + alice's leave-workspace event on top of these.
    // (create_channel emits a join event so the line count reflects events
    // + messages combined.)
    let pre_dev_thread = read_thread(&state.repo_root, "channels/dev.thread");
    let pre_line_count = pre_dev_thread.lines().count();
    assert!(
        pre_line_count >= 2,
        "pre-burn dev should have at least 2 lines, got {}:\n{}",
        pre_line_count,
        pre_dev_thread
    );

    // Spawn the concurrent operations. depart_user walks all 4 phases;
    // carol's send hits the same commit_lock at the channel write step.
    let s_depart = state.clone();
    let s_send = state.clone();
    let h_depart = tokio::spawn(async move { depart_user(s_depart, "alice").await });
    let h_send = tokio::spawn(async move {
        // Tiny stagger so the depart call has a chance to enter Phase 1
        // first; without it, on some schedulers the send always lands
        // before depart_user even reads the thread, which would degrade
        // the test to a sequential one. The stagger doesn't pin the order
        // — it just biases against the trivial case.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        send_message(s_send, "dev", "carol during burn", "carol").await
    });

    let depart_resp = h_depart.await.unwrap();
    let send_resp = h_send.await.unwrap();

    assert!(
        depart_resp.ok,
        "depart_user should complete cleanly under concurrency: {:?}",
        depart_resp.error
    );
    assert!(
        send_resp.ok,
        "carol's send should not be rejected — gate keys on burning user, not others: {:?}",
        send_resp.error
    );

    // Final state checks: alice in archive/users, dev.thread carries
    // (a) every original message, (b) carol's "during burn" send, and
    // (c) alice's leave-workspace event. Order between (b) and (c) is
    // non-deterministic and both orderings are valid.
    assert!(
        state
            .repo_root
            .join("archive/users/alice.meta.yaml")
            .exists(),
        "Phase 4 should have completed"
    );
    assert!(
        !state.repo_root.join("users/alice.meta.yaml").exists(),
        "active user meta should be gone"
    );

    let post_dev = read_thread(&state.repo_root, "channels/dev.thread");
    let post_parsed =
        gitim_core::parser::parse_thread(&post_dev).expect("post-burn dev.thread must still parse");
    let post_line_numbers: Vec<u64> = post_parsed
        .entries
        .iter()
        .map(|e| e.line_number())
        .collect();
    // Strictly increasing line numbers is the duplicate-key invariant we
    // care about under concurrency (mirrors concurrent_write_test.rs).
    for w in post_line_numbers.windows(2) {
        assert!(
            w[0] < w[1],
            "line numbers must be strictly increasing under concurrency: {:?}",
            post_line_numbers
        );
    }

    // Charlie's message present.
    assert!(
        post_dev.contains("carol during burn") && post_dev.contains("@carol"),
        "carol's concurrent send must have landed:\n{}",
        post_dev
    );
    // Alice's leave-workspace event present.
    let leave_count = post_dev
        .lines()
        .filter(|l| l.contains("@alice") && l.contains("[E:leave-workspace]"))
        .count();
    assert_eq!(
        leave_count, 1,
        "exactly one alice leave-workspace event expected, got:\n{}",
        post_dev
    );
}

// ─── 10. unarchive_user preserves leave-workspace audit trail (P1.b case 4):
//         depart writes leave-workspace events into channel threads as a
//         permanent audit record. unarchive_user only restores
//         `users/<handler>.meta.yaml` from `archive/users/` — it does NOT
//         rewrite history. The leave events stay; subsequent messages from
//         the now-restored handler get fresh line numbers AFTER them. ─────

#[tokio::test]
async fn test_unarchive_user_preserves_leave_events() {
    let (_tmp, state) = setup_test_repo().await;

    // Setup: alice creates #dev (with bob), sends a message, departs.
    let resp = create_channel(state.clone(), "dev", "alice", &["bob"]).await;
    assert!(resp.ok, "create #dev failed: {:?}", resp.error);
    let resp = send_message(state.clone(), "dev", "first from alice", "alice").await;
    assert!(resp.ok, "alice's pre-burn send failed: {:?}", resp.error);

    let resp = depart_user(state.clone(), "alice").await;
    assert!(resp.ok, "depart_user failed: {:?}", resp.error);

    // Capture #dev.thread snapshot — it should contain alice's leave-workspace event.
    let pre_unarchive_thread = read_thread(&state.repo_root, "channels/dev.thread");
    assert!(
        pre_unarchive_thread.contains("@alice")
            && pre_unarchive_thread.contains("[E:leave-workspace]"),
        "pre-condition: dev.thread should have alice's leave-workspace event:\n{}",
        pre_unarchive_thread
    );
    let pre_unarchive_lines: Vec<String> = pre_unarchive_thread
        .lines()
        .map(|s| s.to_string())
        .collect();

    // Pre-condition: alice in archive, not active.
    assert!(state
        .repo_root
        .join("archive/users/alice.meta.yaml")
        .exists());
    assert!(!state.repo_root.join("users/alice.meta.yaml").exists());

    // Action: bob (still active) unarchives alice. Restoration always reachable
    // by an active actor; alice herself is no longer in state.users.
    let req: gitim_daemon::api::Request = serde_json::from_value(serde_json::json!({
        "method": "unarchive_user",
        "handler": "alice",
        "author": "bob",
    }))
    .unwrap();
    let resp = handle_request(req, state.clone()).await;
    assert!(resp.ok, "unarchive_user failed: {:?}", resp.error);

    // Verify file moves: users/alice.meta.yaml restored, archive entry gone.
    assert!(
        state.repo_root.join("users/alice.meta.yaml").exists(),
        "alice should be back in users/ after unarchive"
    );
    assert!(
        !state
            .repo_root
            .join("archive/users/alice.meta.yaml")
            .exists(),
        "archive entry should be gone after unarchive"
    );

    // Critical: dev.thread contents UNCHANGED — the leave-workspace event is
    // a permanent audit record, not erased on restoration.
    let post_unarchive_thread = read_thread(&state.repo_root, "channels/dev.thread");
    let post_unarchive_lines: Vec<String> = post_unarchive_thread
        .lines()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        pre_unarchive_lines, post_unarchive_lines,
        "thread contents must be unchanged by unarchive_user (audit-trail decision):\n--- pre ---\n{}\n--- post ---\n{}",
        pre_unarchive_thread, post_unarchive_thread
    );

    // Last line must still be alice's leave-workspace event.
    let last = post_unarchive_thread.lines().last().unwrap();
    assert!(
        last.contains("@alice") && last.contains("[E:leave-workspace]"),
        "last line should still be alice's leave-workspace event after unarchive: {}",
        last
    );

    // Now alice can write again — but Phase 3 of depart removed her from
    // #dev members, so an immediate send to #dev would be rejected as a
    // membership violation. Add her back to in-memory users (mirror what
    // unarchive_user does) and verify the channel send guard.
    //
    // Pinning the audit-trail decision: when alice DOES post again
    // (after a re-join, separate flow), her new message must land at a
    // line number strictly greater than the leave-workspace event's,
    // because the line numbers are file-positional. Send to a fresh
    // channel where she isn't blocked by the prior membership cleanup.
    let resp = create_channel(state.clone(), "ops", "alice", &["bob"]).await;
    assert!(
        resp.ok,
        "alice creating fresh #ops post-unarchive failed: {:?}",
        resp.error
    );
    let resp = send_message(state.clone(), "ops", "alice is back", "alice").await;
    assert!(
        resp.ok,
        "alice's post-unarchive send failed: {:?}",
        resp.error
    );

    // Sanity: dev.thread STILL hasn't changed despite alice's new activity
    // elsewhere. Cross-channel writes don't disturb the audit row.
    let final_dev_thread = read_thread(&state.repo_root, "channels/dev.thread");
    assert_eq!(
        pre_unarchive_lines,
        final_dev_thread
            .lines()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
        "dev.thread must remain stable across alice's later activity"
    );
}

// ─── B.4: handle_poll surfaces self_departed when the daemon's own
//          handler has been departed. ─────────────────────────────────────

/// After alice's daemon's own user.meta.yaml lands in archive/users/,
/// any poll request must short-circuit with the typed `self_departed`
/// error_code so the runtime agent_loop can drive self-cleanup instead
/// of looping on a corpse. This is the daemon side of archive-protocol
/// B.4 — pairs with the runtime-side `RuntimeError::SelfDeparted` arm
/// in `start_agent_loop`.
#[tokio::test]
async fn test_poll_returns_self_departed_when_handler_archived() {
    let (_tmp, state) = setup_test_repo().await;

    // Run the full depart sequence on alice (she is the daemon's
    // current_user — see setup_test_repo). Phase 4 places
    // archive/users/alice.meta.yaml; that's the only file the
    // self-departed gate stats.
    let resp = depart_user(state.clone(), "alice").await;
    assert!(resp.ok, "depart_user failed: {:?}", resp.error);

    // Poll — must trip the self-departed gate.
    let poll = handle_request(Request::Poll { since: None }, state.clone()).await;
    assert!(!poll.ok, "expected error response, got ok=true");
    assert_eq!(
        poll.error_code.as_deref(),
        Some("self_departed"),
        "expected error_code=self_departed, got {:?}",
        poll.error_code
    );
    assert!(
        poll.error
            .as_deref()
            .unwrap_or("")
            .contains("self-departed"),
        "expected human message to mention self-departed, got {:?}",
        poll.error
    );
}

// ─── 11. Codex P1: terminal-state branch must push pending commits.
//         Regression for the case where a previous attempt's Phase 4 commit
//         landed locally but never reached origin (e.g. transient network /
//         auth failure during push). On retry, the terminal-state check
//         (`archive/users/<h>.meta.yaml` exists locally) fired and the
//         daemon returned ok=true — runtime then `rm -rf`'d the clone, the
//         only place the unpushed audit commits lived. Workspace footprint
//         stayed visible on origin forever.
//
//         Fix: terminal-state branch now calls push_with_retry. If origin
//         already has everything, the push fast-forwards (cheap no-op). If
//         not, it catches origin up before reporting success. ─────────────
#[tokio::test]
async fn test_depart_terminal_state_pushes_pending_commits_to_origin() {
    use std::process::Command;

    // 1) Bare origin + working clone with alice + bob registered.
    let bare = TempDir::new().unwrap();
    let clone_dir = TempDir::new().unwrap();
    common::init_bare_and_clone(bare.path(), clone_dir.path());

    let root = clone_dir.path().to_path_buf();
    std::fs::create_dir_all(root.join("users")).unwrap();
    for h in ["alice", "bob"] {
        common::write_user(&root, h, h, "dev", "hi");
    }
    common::run_git(&root, &["add", "."]);
    common::run_git(&root, &["commit", "-m", "init"]);
    common::run_git(&root, &["push", "-u", "origin", "HEAD"]);

    // Capture the SHA of the pre-depart tip so we can rewind origin to it
    // later — simulating "Phase 4's commit never reached origin".
    let pre_depart_sha = {
        let out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&root)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };

    let state = common::make_state(root.clone(), Some("alice"), &["alice", "bob"]).await;

    // 2) Normal depart — all phases land locally and on origin.
    let resp = depart_user(state.clone(), "alice").await;
    assert!(resp.ok, "first depart failed: {:?}", resp.error);
    assert!(state
        .repo_root
        .join("archive/users/alice.meta.yaml")
        .exists());

    // Pre-condition for the regression: origin has the archive/ entry now.
    let bare_default_branch = {
        let out = Command::new("git")
            .args(["symbolic-ref", "--short", "HEAD"])
            .current_dir(bare.path())
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    let bare_has_archive_user = |branch: &str| -> bool {
        let out = Command::new("git")
            .args([
                "ls-tree",
                "-r",
                "--name-only",
                branch,
                "archive/users/alice.meta.yaml",
            ])
            .current_dir(bare.path())
            .output()
            .unwrap();
        !out.stdout.is_empty()
    };
    assert!(
        bare_has_archive_user(&bare_default_branch),
        "pre-condition: origin should have archive/users/alice.meta.yaml after first depart"
    );

    // 3) Rewind origin to before the depart commits. Mirrors a real-world
    //    scenario where Phase 4's local commit succeeded but `git push` to
    //    origin failed (network blip, auth flake, bare went read-only). The
    //    LOCAL clone still has every archive/ commit; origin does not.
    //    `update-ref` on the bare repo's branch is the cleanest way to
    //    achieve this without running an interactive rewind on the live
    //    clone.
    common::run_git(
        bare.path(),
        &[
            "update-ref",
            &format!("refs/heads/{}", bare_default_branch),
            &pre_depart_sha,
        ],
    );
    assert!(
        !bare_has_archive_user(&bare_default_branch),
        "post-rewind: origin should no longer have archive/users/alice.meta.yaml"
    );

    // 4) Retry depart_user. Local archive/users/alice.meta.yaml exists →
    //    terminal-state branch fires. Pre-fix: returns ok=true with no push,
    //    runtime would then nuke the clone. Post-fix: push_with_retry runs
    //    first, catches origin up, then returns success.
    let resp = depart_user(state.clone(), "alice").await;
    assert!(
        resp.ok,
        "retry depart should succeed (terminal-state push): {:?}",
        resp.error
    );
    let data = resp.data.unwrap();
    assert!(
        data["already_departed"].as_bool().unwrap(),
        "retry should hit terminal-state branch (already_departed=true)"
    );
    assert_eq!(
        data["commits"].as_u64().unwrap(),
        0,
        "no new commits — only the pending push catches up"
    );

    // 5) The fix's contract: origin now has archive/users/alice.meta.yaml.
    assert!(
        bare_has_archive_user(&bare_default_branch),
        "FIX REGRESSION: terminal-state branch must push pending commits to origin"
    );
}

/// Without a current_user (guest / pre-onboard), the gate is a no-op —
/// poll falls through to the normal path. This guards against accidental
/// regressions where the gate might trip on archived rows for *other*
/// handlers, or where empty current_user is misread as a hit.
#[tokio::test]
async fn test_poll_self_departed_no_op_when_no_current_user() {
    let (_tmp, state) = setup_test_repo().await;

    // Drop current_user so the gate has no handler to compare against.
    {
        let mut cu = state.current_user.write().await;
        *cu = None;
    }

    // Even after some other handler departs, a daemon with no own
    // identity must not surface self_departed.
    let resp = depart_user(state.clone(), "bob").await;
    assert!(resp.ok, "depart_user(bob) failed: {:?}", resp.error);

    let poll = handle_request(Request::Poll { since: None }, state.clone()).await;
    assert!(
        poll.ok,
        "poll should succeed for guest-like daemon, got error: {:?} code: {:?}",
        poll.error, poll.error_code
    );
    assert!(
        poll.error_code.is_none(),
        "expected no error_code, got {:?}",
        poll.error_code
    );
}
