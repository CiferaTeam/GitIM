//! Integration tests for `handle_list_crons`, `handle_show_cron`, and
//! `handle_history_cron`.
//!
//! Same temp-repo + AppState pattern as `cron_create_test.rs`. We reuse
//! `handle_create_cron` to populate specs (its happy-path is already
//! covered in 2.2's tests) and seed `<ts>.thread` files manually for run
//! history.

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
) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_cron",
        "name": name,
        "schedule": schedule,
        "target": target,
        "prompt": prompt,
        "author": "alice",
    }))
    .unwrap();
    handle_request(req, state).await
}

async fn list_crons(state: Arc<AppState>) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "list_crons",
    }))
    .unwrap();
    handle_request(req, state).await
}

async fn show_cron(state: Arc<AppState>, name: &str) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "show_cron",
        "name": name,
    }))
    .unwrap();
    handle_request(req, state).await
}

async fn history_cron(
    state: Arc<AppState>,
    name: &str,
    limit: Option<u32>,
) -> gitim_daemon::api::Response {
    let mut payload = serde_json::json!({
        "method": "history_cron",
        "name": name,
    });
    if let Some(l) = limit {
        payload["limit"] = serde_json::Value::Number(l.into());
    }
    let req: Request = serde_json::from_value(payload).unwrap();
    handle_request(req, state).await
}

/// Seed an `<ts>.thread` file. ts is the filename stem ISO 8601 UTC with
/// `:` → `-` (e.g. `2026-05-11T09-00-00Z`).
fn seed_thread_file(state: &Arc<AppState>, cron_name: &str, ts: &str) {
    let dir = state.repo_root.join("crons").join(cron_name);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{}.thread", ts));
    std::fs::write(
        &path,
        format!(
            "[L000001][P000000][@system][{}] cron({}): hi\n",
            ts, cron_name
        ),
    )
    .unwrap();
}

// ─── list ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_empty() {
    let (_tmp, state) = setup_test_repo().await;
    let resp = list_crons(state.clone()).await;
    assert!(resp.ok, "list failed: {:?}", resp.error);
    let crons = resp.data.unwrap()["crons"].as_array().unwrap().clone();
    assert!(crons.is_empty(), "expected empty, got {:?}", crons);
}

#[tokio::test]
async fn list_with_active_and_archived() {
    let (_tmp, state) = setup_test_repo().await;

    // Active cron via create handler.
    let r1 = create_cron(state.clone(), "active-job", "@daily", "alice", "hi").await;
    assert!(r1.ok);

    // Archived cron seeded under archive/crons/ — should be excluded.
    let archive_dir = state.repo_root.join("archive/crons/old-job");
    std::fs::create_dir_all(&archive_dir).unwrap();
    std::fs::write(
        archive_dir.join("spec.yaml"),
        "version: 1\nschedule: \"@daily\"\ntarget: alice\nprompt: x\ncreated_by: alice\ncreated_at: \"2026-04-01T00:00:00Z\"\n",
    )
    .unwrap();

    let resp = list_crons(state.clone()).await;
    assert!(resp.ok);
    let arr = resp.data.unwrap()["crons"].as_array().unwrap().clone();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["name"], "active-job");
}

#[tokio::test]
async fn list_sort_by_name() {
    let (_tmp, state) = setup_test_repo().await;

    // Create out of order to make sure the daemon resorts.
    for n in ["zeta", "alpha", "mu"] {
        let r = create_cron(state.clone(), n, "@daily", "alice", "hi").await;
        assert!(r.ok, "create '{}' failed", n);
    }

    let resp = list_crons(state.clone()).await;
    let arr = resp.data.unwrap()["crons"].as_array().unwrap().clone();
    let names: Vec<String> = arr
        .iter()
        .map(|e| e["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, vec!["alpha", "mu", "zeta"]);
}

#[tokio::test]
async fn list_includes_next_fire() {
    let (_tmp, state) = setup_test_repo().await;
    let r = create_cron(state.clone(), "weekly", "0 9 * * 1", "alice", "hi").await;
    assert!(r.ok);

    let resp = list_crons(state.clone()).await;
    let arr = resp.data.unwrap()["crons"].as_array().unwrap().clone();
    assert_eq!(arr.len(), 1);
    let nf = arr[0]["next_fire"].as_str();
    assert!(
        nf.is_some(),
        "next_fire should be present, got {:?}",
        arr[0]
    );
    let nf = nf.unwrap();
    // Whatever the absolute value, it must be a UTC ISO 8601 with `Z`.
    assert!(nf.ends_with('Z'), "next_fire should be UTC: {nf}");
    // And must parse cleanly.
    chrono::DateTime::parse_from_rfc3339(nf).expect("next_fire should parse");
}

// ─── show ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn show_existing_with_next_fire() {
    let (_tmp, state) = setup_test_repo().await;
    let r = create_cron(state.clone(), "weekly", "0 9 * * 1", "alice", "hi").await;
    assert!(r.ok);

    let resp = show_cron(state.clone(), "weekly").await;
    assert!(resp.ok, "show failed: {:?}", resp.error);
    let data = resp.data.unwrap();
    assert_eq!(data["name"], "weekly");
    assert!(data["spec"].is_object(), "spec should be a yaml object");
    assert_eq!(data["spec"]["schedule"], "0 9 * * 1");
    assert_eq!(data["spec"]["target"], "alice");
    assert_eq!(data["spec"]["prompt"], "hi");

    // recent_runs empty for fresh spec.
    assert!(data["recent_runs"].as_array().unwrap().is_empty());

    // next_fire present + UTC.
    let nf = data["next_fire"].as_str().unwrap();
    assert!(nf.ends_with('Z'));
}

#[tokio::test]
async fn show_missing() {
    let (_tmp, state) = setup_test_repo().await;
    let resp = show_cron(state.clone(), "ghost").await;
    assert!(!resp.ok);
    assert_eq!(resp.error_code.as_deref(), Some("not_found"));
}

#[tokio::test]
async fn show_invalid_name() {
    let (_tmp, state) = setup_test_repo().await;
    let resp = show_cron(state.clone(), "WeeklyReport").await;
    assert!(!resp.ok);
    assert_eq!(resp.error_code.as_deref(), Some("invalid_name"));
}

#[tokio::test]
async fn show_no_runs_yet() {
    let (_tmp, state) = setup_test_repo().await;
    let r = create_cron(state.clone(), "fresh", "@daily", "alice", "hi").await;
    assert!(r.ok);

    let resp = show_cron(state.clone(), "fresh").await;
    assert!(resp.ok);
    let arr = resp.data.unwrap()["recent_runs"]
        .as_array()
        .unwrap()
        .clone();
    assert!(arr.is_empty(), "fresh cron should have no recent runs");
}

#[tokio::test]
async fn show_returns_recent_runs_newest_first_capped_at_5() {
    let (_tmp, state) = setup_test_repo().await;
    let r = create_cron(state.clone(), "frequent", "@hourly", "alice", "hi").await;
    assert!(r.ok);

    // Seed 7 thread files; show should return the 5 newest.
    let timestamps = [
        "2026-05-01T09-00-00Z",
        "2026-05-02T09-00-00Z",
        "2026-05-03T09-00-00Z",
        "2026-05-04T09-00-00Z",
        "2026-05-05T09-00-00Z",
        "2026-05-06T09-00-00Z",
        "2026-05-07T09-00-00Z",
    ];
    for ts in &timestamps {
        seed_thread_file(&state, "frequent", ts);
    }

    let resp = show_cron(state.clone(), "frequent").await;
    assert!(resp.ok);
    let runs = resp.data.unwrap()["recent_runs"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(runs.len(), 5);

    // Newest first.
    let returned: Vec<String> = runs
        .iter()
        .map(|r| r["ts"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        returned,
        vec![
            "2026-05-07T09-00-00Z",
            "2026-05-06T09-00-00Z",
            "2026-05-05T09-00-00Z",
            "2026-05-04T09-00-00Z",
            "2026-05-03T09-00-00Z",
        ]
    );
    assert_eq!(runs[0]["filename"], "2026-05-07T09-00-00Z.thread");
}

// ─── history ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn history_empty() {
    let (_tmp, state) = setup_test_repo().await;
    let r = create_cron(state.clone(), "fresh", "@daily", "alice", "hi").await;
    assert!(r.ok);

    let resp = history_cron(state.clone(), "fresh", None).await;
    assert!(resp.ok);
    let runs = resp.data.unwrap()["runs"].as_array().unwrap().clone();
    assert!(runs.is_empty());
}

#[tokio::test]
async fn history_pagination_limit() {
    let (_tmp, state) = setup_test_repo().await;
    let r = create_cron(state.clone(), "frequent", "@hourly", "alice", "hi").await;
    assert!(r.ok);

    for ts in [
        "2026-05-01T09-00-00Z",
        "2026-05-02T09-00-00Z",
        "2026-05-03T09-00-00Z",
        "2026-05-04T09-00-00Z",
    ] {
        seed_thread_file(&state, "frequent", ts);
    }

    let resp = history_cron(state.clone(), "frequent", Some(2)).await;
    assert!(resp.ok);
    let runs = resp.data.unwrap()["runs"].as_array().unwrap().clone();
    assert_eq!(runs.len(), 2);
    // Newest two.
    let ts: Vec<String> = runs
        .iter()
        .map(|r| r["ts"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ts, vec!["2026-05-04T09-00-00Z", "2026-05-03T09-00-00Z"]);
}

#[tokio::test]
async fn history_default_limit_50() {
    let (_tmp, state) = setup_test_repo().await;
    let r = create_cron(state.clone(), "frequent", "@hourly", "alice", "hi").await;
    assert!(r.ok);

    // Seed 60 thread files; default limit 50 should cap.
    for i in 1..=60 {
        // Pad to fixed width so lex sort matches numeric.
        let ts = format!("2026-05-01T09-{:02}-00Z", i % 60);
        seed_thread_file(&state, "frequent", &ts);
    }

    let resp = history_cron(state.clone(), "frequent", None).await;
    assert!(resp.ok);
    let runs = resp.data.unwrap()["runs"].as_array().unwrap().clone();
    assert!(
        runs.len() <= 50,
        "default cap should be ≤ 50, got {}",
        runs.len()
    );
}

#[tokio::test]
async fn history_missing_cron() {
    let (_tmp, state) = setup_test_repo().await;
    let resp = history_cron(state.clone(), "ghost", None).await;
    assert!(!resp.ok);
    assert_eq!(resp.error_code.as_deref(), Some("not_found"));
}
