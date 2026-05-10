//! Performance baseline for `handle_depart_user` — archive-protocol plan A.9.
//!
//! Measures cold-cache, end-to-end latency of `depart_user("alice")` against
//! a synthetic 1000-thread workspace where alice has authored 1-3 messages
//! in ~50% of channels. The remaining setup state exercises Phase 2
//! (5 active DMs involving alice) and Phase 3 (alice present in a few
//! channel meta `members` lists).
//!
//! **Run command**:
//! ```bash
//! cargo test -p gitim-daemon --test depart_user_perf -- --ignored --nocapture
//! ```
//!
//! **Pass criteria** (per A.9 plan):
//! - ≤ 500ms → prints "PASS" and exits silently
//! - >  500ms → prints "ABOVE BASELINE" with measured latency. The test
//!   still passes (returns `Ok`) — A.9 is explicitly non-blocking for v1
//!   merge. If above baseline, an optimization follow-up plan should be
//!   filed at `docs/plans/<date>-archive-perf-optimization/`. Likely
//!   candidate: replace per-thread parse with a `gitim-index` author
//!   reverse-lookup so Phase 1 only opens threads alice actually wrote in.
//!
//! Setup itself is NOT measured — only the `depart_user` invocation.
//! The setup uses one bulk `git add . && git commit` to keep prep time
//! reasonable; depart_user's per-phase commit count (1k threads × ~50%
//! authored ≈ 500 commits) is what we're actually timing.

use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::broadcast;

use gitim_core::types::Config;
use gitim_daemon::api::Request;
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::AppState;

const NUM_THREADS: usize = 1000;
const ALICE_PARTICIPATION: f64 = 0.5;
const NUM_DMS: usize = 5;
const NUM_MEMBER_CHANNELS: usize = 10;
const BASELINE_MS: u128 = 500;

fn make_config() -> Config {
    serde_yaml::from_str("version: 1").unwrap()
}

fn run_git(dir: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@test.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@test.com")
        .output()
        .expect("git command failed");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Build the synthetic 1k-thread workspace in one bulk `git add . &&
/// git commit`. Returns (tempdir, AppState). Setup time is intentionally
/// outside the measured region.
async fn build_synthetic_workspace() -> (TempDir, Arc<AppState>) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();

    // Users.
    std::fs::create_dir_all(root.join("users")).unwrap();
    for h in ["alice", "bob", "carol"] {
        std::fs::write(
            root.join(format!("users/{}.meta.yaml", h)),
            format!("display_name: {}\nrole: dev\nintroduction: hi\n", h),
        )
        .unwrap();
    }

    // Channel threads + minimal meta. Deterministic alice participation:
    // every other channel (i % 2 == 0) gets 1-3 alice messages, varied by
    // i % 3 to spread across the 1/2/3 buckets.
    let channels_dir = root.join("channels");
    std::fs::create_dir_all(&channels_dir).unwrap();
    for i in 0..NUM_THREADS {
        let ch_name = format!("ch_{:04}", i);
        let alice_speaks = (i as f64) < (NUM_THREADS as f64 * ALICE_PARTICIPATION) * 2.0
            && i % 2 == 0;
        let alice_msg_count = if alice_speaks { 1 + (i % 3) } else { 0 };

        let mut thread_content = String::new();
        let mut line: u64 = 1;
        // Always seed with one bob message so the thread isn't empty.
        thread_content.push_str(&format!(
            "[L{:06}][P{:06}][@bob][20260101T000000Z] seed message in {}\n",
            line, 0, ch_name
        ));
        line += 1;
        for j in 0..alice_msg_count {
            thread_content.push_str(&format!(
                "[L{:06}][P{:06}][@alice][20260102T00{:04}Z] alice msg {} in {}\n",
                line, 0, j, j, ch_name
            ));
            line += 1;
        }

        std::fs::write(channels_dir.join(format!("{}.thread", ch_name)), thread_content)
            .unwrap();

        // Channel meta — alice is a member of the first NUM_MEMBER_CHANNELS
        // channels so Phase 3 has work to do.
        let in_members = i < NUM_MEMBER_CHANNELS;
        let members_yaml = if in_members {
            "  - alice\n  - bob\n"
        } else {
            "  - bob\n"
        };
        let meta = format!(
            "display_name: {ch}\ncreated_by: bob\ncreated_at: 20260101T000000Z\nintroduction: ''\nmembers:\n{members}",
            ch = ch_name,
            members = members_yaml,
        );
        std::fs::write(channels_dir.join(format!("{}.meta.yaml", ch_name)), meta).unwrap();
    }

    // Active DMs involving alice (Phase 2 work).
    let dm_dir = root.join("dm");
    std::fs::create_dir_all(&dm_dir).unwrap();
    for i in 0..NUM_DMS {
        // Alphabetical pair → "alice--peerN".
        let peer = format!("peer{}", i);
        let stem = if "alice" <= peer.as_str() {
            format!("alice--{}", peer)
        } else {
            format!("{}--alice", peer)
        };
        let content = format!(
            "[L{:06}][P{:06}][@alice][20260101T000000Z] hi {}\n",
            1, 0, peer
        );
        std::fs::write(dm_dir.join(format!("{}.thread", stem)), content).unwrap();
    }

    // Bulk commit. One `git add` + one `git commit` covers the whole
    // synthetic state in O(1) git operations regardless of NUM_THREADS.
    run_git(&root, &["init"]);
    run_git(&root, &["add", "."]);
    run_git(&root, &["commit", "-m", "synthetic workspace"]);

    let (tx, _) = broadcast::channel(256);
    let state = Arc::new(AppState::new(
        root,
        make_config(),
        tx,
        Some("alice".to_string()),
    ));
    {
        let mut users = state.users.write().await;
        *users = vec!["alice".to_string(), "bob".to_string(), "carol".to_string()];
    }

    (tmp, state)
}

#[tokio::test]
#[ignore = "perf baseline; run manually with --ignored --nocapture"]
async fn depart_user_baseline_1k_threads() {
    let setup_start = std::time::Instant::now();
    let (_tmp, state) = build_synthetic_workspace().await;
    let setup_elapsed = setup_start.elapsed();
    eprintln!(
        "[setup] {} threads, {} DMs, {} member channels — built in {:?}",
        NUM_THREADS, NUM_DMS, NUM_MEMBER_CHANNELS, setup_elapsed
    );

    // The actual measurement.
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "depart_user",
        "handler": "alice",
    }))
    .unwrap();

    let start = std::time::Instant::now();
    let resp = handle_request(req, state.clone()).await;
    let elapsed = start.elapsed();

    assert!(
        resp.ok,
        "depart_user must succeed for the timing to be meaningful: {:?}",
        resp.error
    );
    let data = resp.data.unwrap();
    let commits = data["commits"].as_u64().unwrap();
    let already = data["already_departed"].as_bool().unwrap();
    assert!(!already);

    eprintln!("[result] depart_user latency: {:?}", elapsed);
    eprintln!("[result] commits produced:  {}", commits);
    eprintln!(
        "[result] avg per commit:    {:?}",
        elapsed / commits.max(1) as u32
    );

    if elapsed.as_millis() <= BASELINE_MS {
        eprintln!(
            "[verdict] PASS — {:?} ≤ {}ms baseline",
            elapsed, BASELINE_MS
        );
    } else {
        eprintln!(
            "[verdict] ABOVE BASELINE — measured {:?} > {}ms target.",
            elapsed, BASELINE_MS
        );
        eprintln!(
            "[verdict] Per A.9: non-blocking for v1 merge. Open follow-up:"
        );
        eprintln!(
            "[verdict]   docs/plans/<date>-archive-perf-optimization/"
        );
        eprintln!(
            "[verdict] Likely cause: Phase 1 parses every thread file to"
        );
        eprintln!(
            "[verdict] check authorship; cost dominated by git subprocess"
        );
        eprintln!(
            "[verdict] fork per commit (~10ms × {} commits).",
            commits
        );
        eprintln!(
            "[verdict] Candidate optimization: gitim-index author reverse-"
        );
        eprintln!(
            "[verdict] lookup so Phase 1 only opens threads alice authored."
        );
    }
}
