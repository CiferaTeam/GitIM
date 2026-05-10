//! Integration tests for `handle_create_cron`.
//!
//! Pattern mirrors `archive_dm_test.rs` / `archive_user_test.rs`: temp git
//! repo + AppState in-process, exercise via `handle_request`. No daemon
//! process spawned.

use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::broadcast;

use gitim_core::types::{Config, CronSpec};
use gitim_daemon::api::Request;
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::AppState;

fn make_config() -> Config {
    serde_yaml::from_str("version: 1").unwrap()
}

/// Build a temp git repo with alice + bob registered. `current_user =
/// alice` so dispatch resolves "no author" to alice. Same shape as the
/// other archive_*_test fixtures.
async fn setup_test_repo() -> (TempDir, Arc<AppState>) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();

    std::fs::create_dir_all(root.join("users")).unwrap();
    for h in ["alice", "bob"] {
        std::fs::write(
            root.join(format!("users/{}.meta.yaml", h)),
            format!("display_name: {}\nrole: dev\nintroduction: hi\n", h),
        )
        .unwrap();
    }

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

// ─── 2. Name validation ──────────────────────────────────────────────────────

#[tokio::test]
async fn create_name_invalid_uppercase() {
    let (_tmp, state) = setup_test_repo().await;
    let resp = create_cron(
        state.clone(),
        "WeeklyReport",
        "0 9 * * 1",
        "alice",
        "x",
        None,
        Some("alice"),
    )
    .await;
    assert!(!resp.ok);
    assert_eq!(resp.error_code.as_deref(), Some("invalid_name"));
}

#[tokio::test]
async fn create_name_invalid_empty() {
    let (_tmp, state) = setup_test_repo().await;
    let resp = create_cron(
        state.clone(),
        "",
        "0 9 * * 1",
        "alice",
        "x",
        None,
        Some("alice"),
    )
    .await;
    assert!(!resp.ok);
    assert_eq!(resp.error_code.as_deref(), Some("invalid_name"));
}

#[tokio::test]
async fn create_name_invalid_too_long() {
    let (_tmp, state) = setup_test_repo().await;
    let name = "a".repeat(64);
    let resp = create_cron(
        state.clone(),
        &name,
        "0 9 * * 1",
        "alice",
        "x",
        None,
        Some("alice"),
    )
    .await;
    assert!(!resp.ok);
    assert_eq!(resp.error_code.as_deref(), Some("invalid_name"));
}

#[tokio::test]
async fn create_name_invalid_reserved() {
    let (_tmp, state) = setup_test_repo().await;
    for reserved in ["archive", "crons"] {
        let resp = create_cron(
            state.clone(),
            reserved,
            "0 9 * * 1",
            "alice",
            "x",
            None,
            Some("alice"),
        )
        .await;
        assert!(!resp.ok, "name '{}' should be rejected", reserved);
        assert_eq!(resp.error_code.as_deref(), Some("invalid_name"));
    }
}

#[tokio::test]
async fn create_name_invalid_dotfile() {
    let (_tmp, state) = setup_test_repo().await;
    let resp = create_cron(
        state.clone(),
        ".hidden",
        "0 9 * * 1",
        "alice",
        "x",
        None,
        Some("alice"),
    )
    .await;
    assert!(!resp.ok);
    assert_eq!(resp.error_code.as_deref(), Some("invalid_name"));
}

#[tokio::test]
async fn create_name_invalid_leading_hyphen() {
    let (_tmp, state) = setup_test_repo().await;
    let resp = create_cron(
        state.clone(),
        "-leading",
        "0 9 * * 1",
        "alice",
        "x",
        None,
        Some("alice"),
    )
    .await;
    assert!(!resp.ok);
    assert_eq!(resp.error_code.as_deref(), Some("invalid_name"));
}

// ─── 3. Name conflict ────────────────────────────────────────────────────────

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

// ─── 4. Schedule + timezone validation ───────────────────────────────────────

#[tokio::test]
async fn create_invalid_schedule() {
    let (_tmp, state) = setup_test_repo().await;
    let resp = create_cron(
        state.clone(),
        "weekly",
        "totally bogus",
        "alice",
        "x",
        None,
        Some("alice"),
    )
    .await;
    assert!(!resp.ok);
    assert_eq!(resp.error_code.as_deref(), Some("invalid_schedule"));
}

#[tokio::test]
async fn create_invalid_timezone() {
    let (_tmp, state) = setup_test_repo().await;
    let resp = create_cron(
        state.clone(),
        "weekly",
        "0 9 * * 1",
        "alice",
        "x",
        Some("Mars/Olympus_Mons"),
        Some("alice"),
    )
    .await;
    assert!(!resp.ok);
    assert_eq!(resp.error_code.as_deref(), Some("invalid_timezone"));
}

// ─── 5. Target resolution ────────────────────────────────────────────────────

#[tokio::test]
async fn create_self_target_resolves() {
    let (_tmp, state) = setup_test_repo().await;
    let resp = create_cron(
        state.clone(),
        "self-checkin",
        "@daily",
        "@self",
        "x",
        None,
        Some("alice"),
    )
    .await;
    assert!(resp.ok, "create failed: {:?}", resp.error);
    assert_eq!(resp.data.as_ref().unwrap()["target"], "alice");

    let body =
        std::fs::read_to_string(state.repo_root.join("crons/self-checkin/spec.yaml")).unwrap();
    let spec: CronSpec = CronSpec::from_yaml(&body).unwrap();
    assert_eq!(spec.target.as_str(), "alice");
}

#[tokio::test]
async fn create_resolves_self_case_insensitive() {
    // `@SELF`, `@Self`, `@self`, `SELF` (no leading @) all alias the
    // author handler. Without the eq_ignore_ascii_case path, anything
    // but lowercase `@self` would fall through to `Handler::new("SELF")`
    // — which rejects on uppercase and surfaces a confusing
    // "InvalidChar" error from a layer the user isn't trying to interact
    // with.
    for variant in ["@SELF", "@Self", "@self", "SELF", "Self"] {
        let (_tmp, state) = setup_test_repo().await;
        let resp = create_cron(
            state.clone(),
            "self-test",
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

// ─── 6. Prompt validation ────────────────────────────────────────────────────

#[tokio::test]
async fn create_empty_prompt() {
    let (_tmp, state) = setup_test_repo().await;
    let resp = create_cron(
        state.clone(),
        "noop",
        "@daily",
        "alice",
        "",
        None,
        Some("alice"),
    )
    .await;
    assert!(!resp.ok);
    assert_eq!(resp.error_code.as_deref(), Some("prompt_empty"));
}

#[tokio::test]
async fn create_oversized_prompt() {
    let (_tmp, state) = setup_test_repo().await;
    let huge = "a".repeat(8 * 1024 + 1);
    let resp = create_cron(
        state.clone(),
        "spam",
        "@daily",
        "alice",
        &huge,
        None,
        Some("alice"),
    )
    .await;
    assert!(!resp.ok);
    assert_eq!(resp.error_code.as_deref(), Some("prompt_too_large"));
}

// ─── 7. Author resolution from dispatch ──────────────────────────────────────

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
