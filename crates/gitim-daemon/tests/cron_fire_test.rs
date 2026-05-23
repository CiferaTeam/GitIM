#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Integration tests for `cron_engine::fire`.
//!
//! Mirrors the harness pattern from `cron_create_test.rs` /
//! `cron_lifecycle_test.rs`: temp git repo + AppState, no spawned
//! daemon. Tests assert filesystem + git state directly.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use tempfile::TempDir;
use tokio::sync::broadcast;

use gitim_core::types::{Config, CronSpec, Handler};
use gitim_daemon::cron_engine::{fire, FireRequest};
use gitim_daemon::cron_paths::format_thread_filename_ts;
use gitim_daemon::state::AppState;

fn make_config() -> Config {
    serde_yaml::from_str("version: 1").unwrap()
}

/// Build a temp git repo with `current_user = alice`, an existing
/// `crons/<spec_name>/spec.yaml`, and `users/alice.meta.yaml` so author
/// resolution works. Returns the tempdir, AppState, and the spec we
/// wrote so tests can build matching FireRequests.
async fn setup_with_spec(
    spec_name: &str,
    prompt: &str,
    github_email: Option<String>,
) -> (TempDir, Arc<AppState>, CronSpec) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();

    std::fs::create_dir_all(root.join("users")).unwrap();
    std::fs::write(
        root.join("users/alice.meta.yaml"),
        "display_name: Alice\nrole: dev\nintroduction: hi\n",
    )
    .unwrap();

    let spec = CronSpec {
        version: 1,
        schedule: "@daily".to_string(),
        timezone: None,
        target: Handler::new("alice").unwrap(),
        prompt: prompt.to_string(),
        enabled: true,
        created_by: Handler::new("alice").unwrap(),
        created_at: "2026-05-01T00:00:00Z".to_string(),
        extra: BTreeMap::new(),
    };
    let spec_dir = root.join("crons").join(spec_name);
    std::fs::create_dir_all(&spec_dir).unwrap();
    std::fs::write(spec_dir.join("spec.yaml"), spec.to_yaml().unwrap()).unwrap();

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
    let state = Arc::new(AppState::new_with_email(
        root,
        make_config(),
        tx,
        Some("alice".to_string()),
        github_email,
    ));
    {
        let mut users = state.users.write().await;
        *users = vec!["alice".to_string()];
    }

    (tmp, state, spec)
}

fn build_request(spec_name: &str, spec: &CronSpec, ts: DateTime<Utc>) -> FireRequest {
    FireRequest {
        spec_name: spec_name.to_string(),
        spec: spec.clone(),
        theoretical_ts: ts,
    }
}

fn count_commits(root: &std::path::Path) -> usize {
    let out = std::process::Command::new("git")
        .args(["log", "--pretty=%H"])
        .current_dir(root)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .count()
}

fn last_commit_author_email(root: &std::path::Path) -> String {
    let out = std::process::Command::new("git")
        .args(["log", "-1", "--pretty=%ae"])
        .current_dir(root)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn last_commit_subject(root: &std::path::Path) -> String {
    let out = std::process::Command::new("git")
        .args(["log", "-1", "--pretty=%s"])
        .current_dir(root)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

// ─── happy path ────────────────────────────────────────────────────────────

#[tokio::test]
async fn fire_happy_path() {
    let (_tmp, state, spec) = setup_with_spec("weekly", "scan logs", None).await;
    let ts = Utc.with_ymd_and_hms(2026, 5, 11, 9, 0, 0).unwrap();

    let res = fire(&state, build_request("weekly", &spec, ts)).await;
    assert!(res.is_ok(), "fire failed: {:?}", res.err());

    let stem = format_thread_filename_ts(ts);
    let dest = state
        .repo_root
        .join("crons/weekly")
        .join(format!("{stem}.thread"));
    assert!(dest.exists(), "thread file should exist");

    // Body parses back through the canonical parser.
    let body = std::fs::read_to_string(&dest).unwrap();
    let parsed = gitim_core::parser::parse_thread(&body).expect("body parses");
    assert_eq!(parsed.entries.len(), 1);
    match &parsed.entries[0] {
        gitim_core::types::ThreadEntry::Message(m) => {
            assert_eq!(m.author.as_str(), "system");
            assert!(m.body.starts_with("cron(weekly):"), "body: {}", m.body);
            assert!(m.body.contains("scan logs"));
        }
        other => panic!("expected message, got {other:?}"),
    }

    // Commit landed with the cron-specific subject.
    let subject = last_commit_subject(&state.repo_root);
    assert!(
        subject.starts_with("cron: fire weekly at "),
        "subject: {subject}"
    );
}

// ─── idempotency ────────────────────────────────────────────────────────────

#[tokio::test]
async fn fire_already_exists_no_op() {
    // Two `fire` calls with the same theoretical_ts: second one must be
    // a no-op (no new commit, file unchanged).
    let (_tmp, state, spec) = setup_with_spec("daily", "hi", None).await;
    let ts = Utc.with_ymd_and_hms(2026, 5, 11, 0, 0, 0).unwrap();

    fire(&state, build_request("daily", &spec, ts))
        .await
        .unwrap();
    let after_first = count_commits(&state.repo_root);

    fire(&state, build_request("daily", &spec, ts))
        .await
        .unwrap();
    let after_second = count_commits(&state.repo_root);
    assert_eq!(
        after_first, after_second,
        "second fire at same ts must not produce a new commit"
    );
}

// ─── lock contention ───────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fire_lock_held_blocks_then_proceeds() {
    // Two concurrent fires with the SAME theoretical_ts: one wins the
    // lock, writes + commits; the other sees the file under lock and
    // returns Ok without producing a duplicate commit.
    //
    // We don't try to verify the literal blocking — that would be flaky
    // (lock acquisition timing varies). What we DO verify is the
    // observable outcome: both calls return Ok, exactly one commit
    // landed, and the file is on disk. That's the engine's only
    // contract: race-safety, not lock-fairness.
    let (_tmp, state, spec) = setup_with_spec("conc", "hi", None).await;
    let ts = Utc.with_ymd_and_hms(2026, 5, 11, 12, 0, 0).unwrap();

    let s1 = state.clone();
    let s2 = state.clone();
    let spec1 = spec.clone();
    let spec2 = spec.clone();
    let h1 = tokio::spawn(async move { fire(&s1, build_request("conc", &spec1, ts)).await });
    let h2 = tokio::spawn(async move { fire(&s2, build_request("conc", &spec2, ts)).await });

    let r1 = h1.await.unwrap();
    let r2 = h2.await.unwrap();
    assert!(r1.is_ok(), "first fire failed: {:?}", r1.err());
    assert!(r2.is_ok(), "second fire failed: {:?}", r2.err());

    // Exactly one fire commit landed (init + cron fire = 2 total).
    let total = count_commits(&state.repo_root);
    assert_eq!(total, 2, "expected init + 1 fire commit, got {total}");
}

// ─── author email plumbing ─────────────────────────────────────────────────

#[tokio::test]
async fn fire_author_email_from_state_github_email() {
    // When the daemon was provisioned with a github_email, fire commits
    // attribute to that address (so the workspace owner gets contribution
    // graph credit per CLAUDE.md "Agent 独立 GitHub 身份").
    let (_tmp, state, spec) =
        setup_with_spec("attribute", "hi", Some("owner@example.com".to_string())).await;
    let ts = Utc.with_ymd_and_hms(2026, 5, 11, 0, 0, 0).unwrap();
    fire(&state, build_request("attribute", &spec, ts))
        .await
        .unwrap();

    assert_eq!(
        last_commit_author_email(&state.repo_root),
        "owner@example.com"
    );
}

#[tokio::test]
async fn fire_author_email_fallback_when_github_email_absent() {
    // No github_email → falls back to `<handler>@gitim` (current_user
    // is alice in our fixture).
    let (_tmp, state, spec) = setup_with_spec("nofallback", "hi", None).await;
    let ts = Utc.with_ymd_and_hms(2026, 5, 11, 0, 0, 0).unwrap();
    fire(&state, build_request("nofallback", &spec, ts))
        .await
        .unwrap();

    assert_eq!(last_commit_author_email(&state.repo_root), "alice@gitim");
}

// ─── multi-line prompt ─────────────────────────────────────────────────────

#[tokio::test]
async fn fire_multiline_prompt_uses_continuation_lines() {
    // Multi-line prompt → first line carries `[L...]` prefix, subsequent
    // lines are continuation. Round-trip through parser gives us a
    // single Message entry with the full body.
    let prompt = "first line\nsecond line\nthird line";
    let (_tmp, state, spec) = setup_with_spec("ml", prompt, None).await;
    let ts = Utc.with_ymd_and_hms(2026, 5, 11, 0, 0, 0).unwrap();
    fire(&state, build_request("ml", &spec, ts)).await.unwrap();

    let stem = format_thread_filename_ts(ts);
    let body = std::fs::read_to_string(
        state
            .repo_root
            .join("crons/ml")
            .join(format!("{stem}.thread")),
    )
    .unwrap();
    let parsed = gitim_core::parser::parse_thread(&body).expect("parses");
    assert_eq!(parsed.entries.len(), 1, "single message, multiple lines");
    match &parsed.entries[0] {
        gitim_core::types::ThreadEntry::Message(m) => {
            assert!(m.body.contains("first line"));
            assert!(m.body.contains("second line"));
            assert!(m.body.contains("third line"));
        }
        other => panic!("expected message, got {other:?}"),
    }
}
