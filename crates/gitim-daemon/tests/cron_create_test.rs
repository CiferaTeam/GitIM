#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Integration tests for `handle_create_cron`.
//!
//! Pattern mirrors `archive_dm_test.rs` / `archive_user_test.rs`: temp git
//! repo + AppState in-process, exercise via `handle_request`. No daemon
//! process spawned.

mod common;

use std::sync::Arc;

use gitim_core::types::CronSpec;
use gitim_daemon::api::Request;
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::AppState;

/// Build a temp git repo with alice + bob registered. `current_user =
/// alice` so dispatch resolves "no author" to alice. Same shape as the
/// other archive_*_test fixtures.
///
/// Note: uses `display_name = handler` (lowercase) via `setup_repo_with_users`.
async fn setup_test_repo() -> (tempfile::TempDir, Arc<AppState>) {
    common::setup_repo_with_users(&["alice", "bob"]).await
}

async fn create_cron(
    state: Arc<AppState>,
    name: &str,
    schedule: &str,
    target: &str,
    prompt: &str,
    timezone: Option<&str>,
    author: Option<&str>,
) -> gitim_daemon::api::Response {
    let mut payload = serde_json::json!({
        "method": "create_cron",
        "name": name,
        "schedule": schedule,
        "target": target,
        "prompt": prompt,
    });
    if let Some(tz) = timezone {
        payload["timezone"] = serde_json::Value::String(tz.to_string());
    }
    if let Some(a) = author {
        payload["author"] = serde_json::Value::String(a.to_string());
    }
    let req: Request = serde_json::from_value(payload).unwrap();
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

fn git_log_authors(root: &std::path::Path) -> String {
    let out = std::process::Command::new("git")
        .args(["log", "--pretty=%an <%ae>"])
        .current_dir(root)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).to_string()
}

// ─── 1. Happy path ────────────────────────────────────────────────────────────

#[tokio::test]
async fn create_happy_path() {
    let (_tmp, state) = setup_test_repo().await;

    let resp = create_cron(
        state.clone(),
        "weekly-report",
        "0 9 * * 1",
        "alice",
        "weekly checkin",
        None,
        Some("alice"),
    )
    .await;

    assert!(resp.ok, "create failed: {:?}", resp.error);
    let data = resp.data.unwrap();
    assert_eq!(data["name"], "weekly-report");
    assert_eq!(data["created_by"], "alice");
    assert_eq!(data["target"], "alice");

    // spec.yaml exists and parses cleanly.
    let spec_path = state.repo_root.join("crons/weekly-report/spec.yaml");
    assert!(spec_path.exists(), "spec.yaml should exist");
    let body = std::fs::read_to_string(&spec_path).unwrap();
    let spec: CronSpec = CronSpec::from_yaml(&body).unwrap();
    assert_eq!(spec.schedule, "0 9 * * 1");
    assert_eq!(spec.target.as_str(), "alice");
    assert_eq!(spec.prompt, "weekly checkin");
    assert!(spec.enabled);
    assert_eq!(spec.created_by.as_str(), "alice");

    // Commit recorded with the convention message + author.
    let log = git_log_subjects(&state.repo_root);
    assert!(
        log.contains("cron: create weekly-report by @alice"),
        "log: {log}"
    );
    let authors = git_log_authors(&state.repo_root);
    assert!(authors.contains("alice"), "authors: {authors}");
}

#[tokio::test]
async fn create_validation_errors_map_to_stable_codes() {
    let (_tmp, state) = setup_test_repo().await;
    let oversized_prompt = "a".repeat(8 * 1024 + 1);
    let cases = [
        (
            "invalid name",
            "WeeklyReport",
            "0 9 * * 1",
            "x",
            None,
            "invalid_name",
        ),
        (
            "invalid schedule",
            "invalid-schedule",
            "totally bogus",
            "x",
            None,
            "invalid_schedule",
        ),
        (
            "invalid timezone",
            "invalid-timezone",
            "0 9 * * 1",
            "x",
            Some("Mars/Olympus_Mons"),
            "invalid_timezone",
        ),
        (
            "empty prompt",
            "empty-prompt",
            "@daily",
            "",
            None,
            "prompt_empty",
        ),
        (
            "oversized prompt",
            "oversized-prompt",
            "@daily",
            oversized_prompt.as_str(),
            None,
            "prompt_too_large",
        ),
    ];

    for (case, name, schedule, prompt, timezone, expected_code) in cases {
        let response = create_cron(
            state.clone(),
            name,
            schedule,
            "alice",
            prompt,
            timezone,
            Some("alice"),
        )
        .await;
        assert!(!response.ok, "{case} should fail");
        assert_eq!(
            response.error_code.as_deref(),
            Some(expected_code),
            "wrong error code for {case}"
        );
    }
}

#[tokio::test]
async fn create_name_conflict_active() {
    let (_tmp, state) = setup_test_repo().await;
    // First create succeeds.
    let r1 = create_cron(
        state.clone(),
        "daily",
        "@daily",
        "alice",
        "x",
        None,
        Some("alice"),
    )
    .await;
    assert!(r1.ok);
    // Second with same name → name_conflict.
    let r2 = create_cron(
        state.clone(),
        "daily",
        "@daily",
        "alice",
        "y",
        None,
        Some("alice"),
    )
    .await;
    assert!(!r2.ok);
    assert_eq!(r2.error_code.as_deref(), Some("name_conflict"));
}

#[tokio::test]
async fn create_name_conflict_archived() {
    let (_tmp, state) = setup_test_repo().await;
    // Pre-populate the archive path manually — Task 2.4 ships the real
    // delete handler; for now we just stage what the conflict check
    // looks for.
    let archive_dir = state.repo_root.join("archive/crons/daily");
    std::fs::create_dir_all(&archive_dir).unwrap();
    std::fs::write(
        archive_dir.join("spec.yaml"),
        "version: 1\nschedule: \"@daily\"\ntarget: alice\nprompt: x\ncreated_by: alice\ncreated_at: \"2026-05-01T00:00:00Z\"\n",
    )
    .unwrap();

    let resp = create_cron(
        state.clone(),
        "daily",
        "@daily",
        "alice",
        "x",
        None,
        Some("alice"),
    )
    .await;
    assert!(!resp.ok);
    assert_eq!(resp.error_code.as_deref(), Some("name_conflict"));
}

#[tokio::test]
async fn create_resolves_self_aliases() {
    let (_tmp, state) = setup_test_repo().await;

    for (index, variant) in ["@SELF", "@Self", "@self", "SELF", "Self"]
        .into_iter()
        .enumerate()
    {
        let name = format!("self-test-{index}");
        let resp = create_cron(
            state.clone(),
            &name,
            "@daily",
            variant,
            "x",
            None,
            Some("alice"),
        )
        .await;
        assert!(
            resp.ok,
            "create with target='{}' failed: {:?}",
            variant, resp.error
        );
        assert_eq!(
            resp.data.as_ref().unwrap()["target"],
            "alice",
            "target='{}' did not resolve to author handler",
            variant
        );

        let body = std::fs::read_to_string(state.repo_root.join(format!("crons/{name}/spec.yaml")))
            .unwrap();
        let spec = CronSpec::from_yaml(&body).unwrap();
        assert_eq!(spec.target.as_str(), "alice");
    }
}

#[tokio::test]
async fn create_target_with_at_prefix_strips() {
    let (_tmp, state) = setup_test_repo().await;
    let resp = create_cron(
        state.clone(),
        "ping-bob",
        "@daily",
        "@bob",
        "x",
        None,
        Some("alice"),
    )
    .await;
    assert!(resp.ok, "create failed: {:?}", resp.error);
    let body = std::fs::read_to_string(state.repo_root.join("crons/ping-bob/spec.yaml")).unwrap();
    let spec: CronSpec = CronSpec::from_yaml(&body).unwrap();
    assert_eq!(spec.target.as_str(), "bob");
}

#[tokio::test]
async fn create_target_not_found() {
    let (_tmp, state) = setup_test_repo().await;
    let resp = create_cron(
        state.clone(),
        "ghost",
        "@daily",
        "ghosthandle",
        "x",
        None,
        Some("alice"),
    )
    .await;
    assert!(!resp.ok);
    assert_eq!(resp.error_code.as_deref(), Some("target_not_found"));
}

#[tokio::test]
async fn create_author_resolved_from_state_when_omitted() {
    let (_tmp, state) = setup_test_repo().await;
    // current_user is alice; omitting `author` should resolve to alice.
    let resp = create_cron(
        state.clone(),
        "default-author",
        "@daily",
        "@self",
        "x",
        None,
        None,
    )
    .await;
    assert!(resp.ok, "create failed: {:?}", resp.error);
    let body =
        std::fs::read_to_string(state.repo_root.join("crons/default-author/spec.yaml")).unwrap();
    let spec: CronSpec = CronSpec::from_yaml(&body).unwrap();
    assert_eq!(spec.created_by.as_str(), "alice");
    assert_eq!(spec.target.as_str(), "alice");
}
