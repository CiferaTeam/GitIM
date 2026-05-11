//! HTTP integration tests for `GET /workspaces/{slug}/crons/timeline`.
//!
//! Same scripted-fake-daemon harness as `cron_http.rs`, plus on-disk seeding
//! for past `<ts>.thread` files. Each test pins explicit `from` / `to` query
//! parameters relative to `Utc::now()` so the past / future / missed split
//! is deterministic without needing to mock the clock.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{DateTime, Duration, Timelike, Utc};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tower::ServiceExt;

use gitim_runtime::http::{create_router, SharedRuntimeState};
use gitim_runtime::workspace::WorkspaceContext;

async fn send(router: axum::Router, method: &str, uri: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

fn inject_human_workspace(
    state: &SharedRuntimeState,
    slug: &str,
    workspace_path: PathBuf,
    human_repo: PathBuf,
) {
    let mut ctx = WorkspaceContext::new(slug.to_string(), slug.to_string(), workspace_path);
    ctx.human_repo = Some(human_repo);
    state
        .lock()
        .unwrap()
        .workspaces
        .insert(slug.to_string(), ctx);
}

type ResponseTable = std::collections::HashMap<String, Value>;

struct ScriptedDaemon {
    task: JoinHandle<()>,
}

impl ScriptedDaemon {
    fn spawn(repo_root: &Path, table: Arc<Mutex<ResponseTable>>) -> Self {
        let run_dir = repo_root.join(".gitim/run");
        std::fs::create_dir_all(&run_dir).unwrap();
        let socket_path = run_dir.join("gitim.sock");
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path).unwrap();

        let task = tokio::spawn(async move {
            while let Ok((stream, _addr)) = listener.accept().await {
                let table = table.clone();
                tokio::spawn(async move {
                    let (reader, mut writer) = stream.into_split();
                    let mut reader = BufReader::new(reader);
                    let mut line = String::new();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let request: Value = match serde_json::from_str(&line) {
                        Ok(v) => v,
                        Err(_) => return,
                    };
                    let method = request
                        .get("method")
                        .and_then(|m| m.as_str())
                        .unwrap_or("")
                        .to_string();
                    let resp_value = {
                        let map = table.lock().await;
                        map.get(&method)
                            .cloned()
                            .unwrap_or_else(|| json!({"ok": false, "error": format!("no scripted response for method {method}")}))
                    };
                    let mut serialized = resp_value.to_string();
                    serialized.push('\n');
                    let _ = writer.write_all(serialized.as_bytes()).await;
                });
            }
        });
        Self { task }
    }
}

impl Drop for ScriptedDaemon {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct TestEnv {
    router: axum::Router,
    table: Arc<Mutex<ResponseTable>>,
    human_repo: PathBuf,
    _daemon: ScriptedDaemon,
    _tmp: TempDir,
}

fn setup() -> TestEnv {
    let tmp = TempDir::new().unwrap();
    let workspace_path = tmp.path().join("workspace");
    let human_repo = tmp.path().join("human");
    std::fs::create_dir_all(&workspace_path).unwrap();
    std::fs::create_dir_all(&human_repo).unwrap();
    let table: Arc<Mutex<ResponseTable>> = Arc::new(Mutex::new(ResponseTable::new()));
    let daemon = ScriptedDaemon::spawn(&human_repo, table.clone());
    let (router, state) = create_router();
    inject_human_workspace(&state, "test-ws", workspace_path, human_repo.clone());
    TestEnv {
        router,
        table,
        human_repo,
        _daemon: daemon,
        _tmp: tmp,
    }
}

async fn set_list_crons(table: &Arc<Mutex<ResponseTable>>, summaries: Value) {
    table.lock().await.insert(
        "list_crons".to_string(),
        json!({"ok": true, "data": {"crons": summaries}}),
    );
}

/// Format a UTC instant the way `to_rfc3339_opts(SecondsFormat::Secs, true)`
/// would — matches what the timeline implementation emits and the daemon's
/// `created_at` accepts.
fn rfc3339_secs(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// `<ts>` filename stem with `:` swapped for `-`. Same shape the engine
/// writes to disk; mirroring it here keeps the test fixtures honest.
fn filename_stem(dt: DateTime<Utc>) -> String {
    rfc3339_secs(dt).replace(':', "-")
}

fn write_thread(human_repo: &Path, name: &str, ts_dt: DateTime<Utc>) {
    let dir = human_repo.join("crons").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let stem = filename_stem(ts_dt);
    let body = format!(
        "[L1][@system][{}] cron({}): test prompt\n",
        rfc3339_secs(ts_dt),
        name
    );
    std::fs::write(dir.join(format!("{stem}.thread")), body).unwrap();
}

#[tokio::test]
async fn timeline_empty_workspace() {
    let env = setup();
    set_list_crons(&env.table, json!([])).await;
    let (status, body) = send(env.router, "GET", "/workspaces/test-ws/crons/timeline").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body.get("entries").and_then(|v| v.as_array()).map(|a| a.len()),
        Some(0)
    );
    // `truncated` should be omitted on the wire when false (skip_serializing_if).
    assert!(body.get("truncated").is_none());
}

#[tokio::test]
async fn timeline_past_only() {
    let env = setup();
    let now = Utc::now();
    let from = now - Duration::days(20);
    let to = now - Duration::days(5);
    // Hourly schedule, created comfortably before `from` so all theoretical
    // fires sit inside the window. Disk state has a single thread file at
    // a known past instant, expected to surface as `kind: past`.
    let created_at = from - Duration::hours(2);
    set_list_crons(
        &env.table,
        json!([{
            "name": "hourly-job",
            "schedule": "0 * * * *",
            "target": "alice",
            "enabled": true,
            "created_by": "alice",
            "created_at": rfc3339_secs(created_at),
        }]),
    )
    .await;

    let past_fire = from + Duration::hours(3);
    write_thread(&env.human_repo, "hourly-job", past_fire);

    let from_q = rfc3339_secs(from);
    let to_q = rfc3339_secs(to);
    let uri = format!(
        "/workspaces/test-ws/crons/timeline?from={}&to={}",
        from_q, to_q
    );
    let (status, body) = send(env.router, "GET", &uri).await;
    assert_eq!(status, StatusCode::OK);
    let entries = body.get("entries").and_then(|v| v.as_array()).unwrap();
    let pasts: Vec<_> = entries
        .iter()
        .filter(|e| e.get("kind").and_then(|v| v.as_str()) == Some("past"))
        .collect();
    let futures: Vec<_> = entries
        .iter()
        .filter(|e| e.get("kind").and_then(|v| v.as_str()) == Some("future"))
        .collect();
    assert_eq!(pasts.len(), 1, "exactly one past file in window");
    assert_eq!(futures.len(), 0, "to is in the past — no future entries");
    let p = &pasts[0];
    assert_eq!(p.get("cron_name").and_then(|v| v.as_str()), Some("hourly-job"));
    assert_eq!(
        p.get("target").and_then(|v| v.as_str()),
        Some("alice"),
        "past entry must carry the cron's target handler",
    );
    assert!(p.get("thread_url").is_some(), "past entry must carry a thread_url");
}

#[tokio::test]
async fn timeline_future_only() {
    let env = setup();
    let now = Utc::now();
    // Window starts well in the future — every theoretical fire is in the
    // future, no thread files exist on disk yet.
    let from = now + Duration::days(5);
    let to = now + Duration::days(8);
    let created_at = now;
    set_list_crons(
        &env.table,
        json!([{
            "name": "daily-standup",
            "schedule": "@daily",
            "target": "bob",
            "enabled": true,
            "created_by": "bob",
            "created_at": rfc3339_secs(created_at),
        }]),
    )
    .await;

    let from_q = rfc3339_secs(from);
    let to_q = rfc3339_secs(to);
    let uri = format!(
        "/workspaces/test-ws/crons/timeline?from={}&to={}",
        from_q, to_q
    );
    let (status, body) = send(env.router, "GET", &uri).await;
    assert_eq!(status, StatusCode::OK);
    let entries = body.get("entries").and_then(|v| v.as_array()).unwrap();
    assert!(!entries.is_empty(), "@daily over 3 days emits at least one fire");
    for e in entries {
        assert_eq!(e.get("kind").and_then(|v| v.as_str()), Some("future"));
        assert_eq!(
            e.get("cron_name").and_then(|v| v.as_str()),
            Some("daily-standup")
        );
        assert_eq!(
            e.get("target").and_then(|v| v.as_str()),
            Some("bob"),
            "future entry must carry the cron's target handler",
        );
        assert!(e.get("thread_url").is_none());
    }
}

#[tokio::test]
async fn timeline_mixed_past_future() {
    let env = setup();
    let now = Utc::now();
    let from = now - Duration::hours(48);
    let to = now + Duration::hours(48);
    let created_at = from - Duration::hours(1);
    set_list_crons(
        &env.table,
        json!([{
            "name": "hourly-job",
            "schedule": "0 * * * *",
            "target": "alice",
            "enabled": true,
            "created_by": "alice",
            "created_at": rfc3339_secs(created_at),
        }]),
    )
    .await;

    // Drop a few past thread files at known hourly slots so they're guaranteed
    // to land in the iteration sequence.
    let aligned_past = (from + Duration::hours(2))
        .with_minute(0)
        .unwrap()
        .with_second(0)
        .unwrap()
        .with_nanosecond(0)
        .unwrap();
    write_thread(&env.human_repo, "hourly-job", aligned_past);

    let from_q = rfc3339_secs(from);
    let to_q = rfc3339_secs(to);
    let uri = format!(
        "/workspaces/test-ws/crons/timeline?from={}&to={}",
        from_q, to_q
    );
    let (status, body) = send(env.router, "GET", &uri).await;
    assert_eq!(status, StatusCode::OK);
    let entries = body.get("entries").and_then(|v| v.as_array()).unwrap();
    let counts = kind_counts(entries);
    assert!(counts.past >= 1, "at least one past file written");
    assert!(counts.future >= 1, "future part of window has fires too");
    // Sorting invariant: ts strings should be sorted ascending.
    let tss: Vec<&str> = entries
        .iter()
        .map(|e| e.get("ts").and_then(|v| v.as_str()).unwrap_or(""))
        .collect();
    let mut sorted = tss.clone();
    sorted.sort();
    assert_eq!(tss, sorted, "entries must be sorted by ts asc");
}

#[tokio::test]
async fn timeline_includes_missed() {
    let env = setup();
    let now = Utc::now();
    // Spec created in the past, window also entirely in the past, no thread
    // files on disk → every theoretical fire becomes "missed" because
    // theoretical_ts <= now and no file is present.
    let from = now - Duration::days(5);
    let to = now - Duration::days(2);
    let created_at = from - Duration::hours(2);
    set_list_crons(
        &env.table,
        json!([{
            "name": "hourly-job",
            "schedule": "0 * * * *",
            "target": "alice",
            "enabled": true,
            "created_by": "alice",
            "created_at": rfc3339_secs(created_at),
        }]),
    )
    .await;

    let from_q = rfc3339_secs(from);
    let to_q = rfc3339_secs(to);
    let uri = format!(
        "/workspaces/test-ws/crons/timeline?from={}&to={}",
        from_q, to_q
    );
    let (status, body) = send(env.router, "GET", &uri).await;
    assert_eq!(status, StatusCode::OK);
    let entries = body.get("entries").and_then(|v| v.as_array()).unwrap();
    assert!(!entries.is_empty(), "hourly schedule over 3 days has many entries");
    for e in entries {
        assert_eq!(
            e.get("kind").and_then(|v| v.as_str()),
            Some("missed"),
            "no files present for any theoretical fire"
        );
        assert_eq!(
            e.get("reason").and_then(|v| v.as_str()),
            Some("no thread file present")
        );
        assert_eq!(
            e.get("target").and_then(|v| v.as_str()),
            Some("alice"),
            "missed entry must carry the cron's target handler",
        );
    }
}

#[tokio::test]
async fn timeline_window_filter_excludes_outside_entries() {
    let env = setup();
    let now = Utc::now();
    let from = now - Duration::days(2);
    let to = now + Duration::days(2);
    let created_at = from - Duration::days(30);
    set_list_crons(
        &env.table,
        json!([{
            "name": "hourly-job",
            "schedule": "0 * * * *",
            "target": "alice",
            "enabled": true,
            "created_by": "alice",
            "created_at": rfc3339_secs(created_at),
        }]),
    )
    .await;

    // Write a thread file far before `from` — should NOT show up.
    let outside = from - Duration::days(10);
    write_thread(&env.human_repo, "hourly-job", outside);

    let from_q = rfc3339_secs(from);
    let to_q = rfc3339_secs(to);
    let uri = format!(
        "/workspaces/test-ws/crons/timeline?from={}&to={}",
        from_q, to_q
    );
    let (status, body) = send(env.router, "GET", &uri).await;
    assert_eq!(status, StatusCode::OK);
    let entries = body.get("entries").and_then(|v| v.as_array()).unwrap();
    let outside_stem = filename_stem(outside);
    for e in entries {
        let url = e.get("thread_url").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            !url.contains(&outside_stem),
            "out-of-window thread file leaked: {url}"
        );
        let ts = e.get("ts").and_then(|v| v.as_str()).unwrap_or("");
        let parsed = DateTime::parse_from_rfc3339(ts).unwrap().with_timezone(&Utc);
        assert!(parsed >= from && parsed <= to, "ts outside window: {ts}");
    }
}

#[tokio::test]
async fn timeline_cap_truncates_runaway_cron() {
    let env = setup();
    let now = Utc::now();
    // `* * * * *` over a ~10-day window = 14 400 entries — well over the
    // 10 000 cap. Created at far in past so future + past arms together
    // would emit > cap; we expect a truncated flag.
    let from = now;
    let to = now + Duration::days(10);
    let created_at = now - Duration::days(30);
    set_list_crons(
        &env.table,
        json!([{
            "name": "minute-burst",
            "schedule": "* * * * *",
            "target": "alice",
            "enabled": true,
            "created_by": "alice",
            "created_at": rfc3339_secs(created_at),
        }]),
    )
    .await;

    let from_q = rfc3339_secs(from);
    let to_q = rfc3339_secs(to);
    let uri = format!(
        "/workspaces/test-ws/crons/timeline?from={}&to={}",
        from_q, to_q
    );
    let (status, body) = send(env.router, "GET", &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body.get("truncated").and_then(|v| v.as_bool()),
        Some(true),
        "runaway schedule must surface truncated flag"
    );
    let entries = body.get("entries").and_then(|v| v.as_array()).unwrap();
    // The cap is 10 000; we should see exactly that many entries (every
    // iteration produces one entry until the cap fires). All future kind.
    assert_eq!(entries.len(), 10_000);
    for e in entries.iter().take(10) {
        assert_eq!(e.get("kind").and_then(|v| v.as_str()), Some("future"));
    }
}

#[tokio::test]
async fn timeline_dst_boundary_iterates_consistently() {
    let env = setup();
    // Pin the window across the US 2026-03-08 spring-forward boundary in
    // America/Los_Angeles. The schedule "30 2 * * *" snaps to 03:00 LA on
    // the gap day per croner's behavior — the timeline must reflect that
    // and not emit a duplicate entry on either side of the transition.
    let from = "2026-03-07T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
    let to = "2026-03-10T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
    let created_at = "2026-03-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
    set_list_crons(
        &env.table,
        json!([{
            "name": "morning-task",
            "schedule": "30 2 * * *",
            "timezone": "America/Los_Angeles",
            "target": "alice",
            "enabled": true,
            "created_by": "alice",
            "created_at": rfc3339_secs(created_at),
        }]),
    )
    .await;

    let from_q = rfc3339_secs(from);
    let to_q = rfc3339_secs(to);
    let uri = format!(
        "/workspaces/test-ws/crons/timeline?from={}&to={}",
        from_q, to_q
    );
    let (status, body) = send(env.router, "GET", &uri).await;
    assert_eq!(status, StatusCode::OK);
    let entries = body.get("entries").and_then(|v| v.as_array()).unwrap();
    let tss: Vec<&str> = entries
        .iter()
        .map(|e| e.get("ts").and_then(|v| v.as_str()).unwrap_or(""))
        .collect();
    // Dedup + sort assertions: every ts is unique, sorted ascending.
    let mut sorted = tss.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        tss, sorted,
        "DST boundary must not emit duplicate or out-of-order entries"
    );
    // 3 days of "30 2 LA" ≈ 3 distinct fires (one snapped on 2026-03-08
    // to 03:00 LA = UTC 10:00). Asserting on the count keeps the test
    // brittle in a useful way — if croner's DST behavior changes the
    // count would shift and we'd need to revisit `next_fire_after`.
    assert_eq!(
        tss.len(),
        3,
        "expected exactly 3 fires across the DST window (got {})",
        tss.len()
    );
}

#[tokio::test]
async fn timeline_invalid_from_returns_400() {
    let env = setup();
    let (status, body) = send(
        env.router,
        "GET",
        "/workspaces/test-ws/crons/timeline?from=garbage&to=2026-05-31T23:59:59Z",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body.get("error_code").and_then(|v| v.as_str()),
        Some("invalid_timestamp")
    );
}

#[tokio::test]
async fn timeline_invalid_window_from_after_to_returns_400() {
    let env = setup();
    let (status, body) = send(
        env.router,
        "GET",
        "/workspaces/test-ws/crons/timeline?from=2026-05-31T23:59:59Z&to=2026-05-01T00:00:00Z",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body.get("error_code").and_then(|v| v.as_str()),
        Some("invalid_window")
    );
}

#[tokio::test]
async fn timeline_workspace_not_found() {
    let env = setup();
    let (status, _body) = send(
        env.router,
        "GET",
        "/workspaces/missing/crons/timeline",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ─── Boundary correctness around `created_at` ────────────────────────────────
//
// The engine fires strictly *after* `last_fire`, where the bootstrap value
// of `last_fire` is `created_at`. The timeline endpoint must mirror that:
// when the requested window covers (or starts before) `created_at`, no
// theoretical fire may land *at* `created_at`. Otherwise the calendar
// over-promises an instant the engine will never produce.

#[tokio::test]
async fn timeline_does_not_emit_fire_at_created_at_when_window_covers_creation() {
    // Spec created exactly on a scheduled instant: `0 0 * * *` with
    // `created_at = 2025-01-01T00:00:00Z`. The first real fire is the
    // NEXT day at 00:00:00Z, not the creation moment itself. Use a
    // pre-now-anchored absolute date so the `missed` arm produces
    // deterministic output (all theoretical fires ≤ `now` → "missed"
    // unless caught as "past" by an on-disk file).
    let env = setup();
    let from = "2025-01-01T00:00:00Z"
        .parse::<DateTime<Utc>>()
        .unwrap();
    let to = "2025-01-05T00:00:00Z"
        .parse::<DateTime<Utc>>()
        .unwrap();
    let created_at = "2025-01-01T00:00:00Z"
        .parse::<DateTime<Utc>>()
        .unwrap();
    set_list_crons(
        &env.table,
        json!([{
            "name": "midnight-daily",
            "schedule": "0 0 * * *",
            "target": "alice",
            "enabled": true,
            "created_by": "alice",
            "created_at": rfc3339_secs(created_at),
        }]),
    )
    .await;

    let from_q = rfc3339_secs(from);
    let to_q = rfc3339_secs(to);
    let uri = format!(
        "/workspaces/test-ws/crons/timeline?from={}&to={}",
        from_q, to_q
    );
    let (status, body) = send(env.router, "GET", &uri).await;
    assert_eq!(status, StatusCode::OK);
    let entries = body.get("entries").and_then(|v| v.as_array()).unwrap();

    // The bug: an entry at `2025-01-01T00:00:00Z` would be emitted
    // because `next_fire_after(spec, created_at - 1s)` returns
    // `created_at` itself. After the fix, the first emitted ts must be
    // `2025-01-02T00:00:00Z`.
    let timestamps: Vec<&str> = entries
        .iter()
        .map(|e| e.get("ts").and_then(|v| v.as_str()).unwrap_or(""))
        .collect();
    assert!(
        !timestamps.contains(&"2025-01-01T00:00:00Z"),
        "engine never fires at created_at; timeline must not emit one. got: {:?}",
        timestamps
    );
    // Sanity: at least one fire exists in the window (Jan 2 / 3 / 4 / 5).
    assert!(
        !timestamps.is_empty(),
        "expected daily fires in 4-day window"
    );
    // First entry must be the day AFTER created_at.
    assert_eq!(
        timestamps.first().copied(),
        Some("2025-01-02T00:00:00Z"),
        "first fire must be one schedule period after created_at"
    );
}

#[tokio::test]
async fn timeline_does_emit_fire_at_window_start_when_after_created_at() {
    // The complementary case: created_at well before the window, and
    // `from` lands exactly on a scheduled instant. The 1-second slack
    // applies (because `from > created_at_dt`), so the on-`from`
    // theoretical fire surfaces rather than being lost to strict-after
    // semantics.
    let env = setup();
    let created_at = "2025-01-01T00:00:00Z"
        .parse::<DateTime<Utc>>()
        .unwrap();
    let from = "2025-02-10T09:00:00Z"
        .parse::<DateTime<Utc>>()
        .unwrap();
    let to = "2025-02-15T00:00:00Z"
        .parse::<DateTime<Utc>>()
        .unwrap();
    set_list_crons(
        &env.table,
        json!([{
            "name": "morning",
            "schedule": "0 9 * * *",
            "target": "alice",
            "enabled": true,
            "created_by": "alice",
            "created_at": rfc3339_secs(created_at),
        }]),
    )
    .await;

    let from_q = rfc3339_secs(from);
    let to_q = rfc3339_secs(to);
    let uri = format!(
        "/workspaces/test-ws/crons/timeline?from={}&to={}",
        from_q, to_q
    );
    let (status, body) = send(env.router, "GET", &uri).await;
    assert_eq!(status, StatusCode::OK);
    let entries = body.get("entries").and_then(|v| v.as_array()).unwrap();
    let timestamps: Vec<&str> = entries
        .iter()
        .map(|e| e.get("ts").and_then(|v| v.as_str()).unwrap_or(""))
        .collect();
    assert!(
        timestamps.contains(&"2025-02-10T09:00:00Z"),
        "boundary fire on `from` must be present; got: {:?}",
        timestamps
    );
}

#[tokio::test]
async fn timeline_cap_one_cron_other_unaffected() {
    // Cross-cron isolation: when cron A's iteration trips the
    // per-cron cap, cron B's expected entries must still appear in the
    // response. The code structurally iterates per-cron — this test
    // locks that invariant against future refactors that might share a
    // global counter.
    let env = setup();
    let now = Utc::now();
    let from = now;
    let to = now + Duration::days(30);
    let created_at = now - Duration::days(30);
    set_list_crons(
        &env.table,
        json!([
            {
                "name": "minute-burst",
                "schedule": "* * * * *",
                "target": "alice",
                "enabled": true,
                "created_by": "alice",
                "created_at": rfc3339_secs(created_at),
            },
            {
                "name": "morning-daily",
                "schedule": "0 9 * * *",
                "target": "bob",
                "enabled": true,
                "created_by": "bob",
                "created_at": rfc3339_secs(created_at),
            }
        ]),
    )
    .await;

    let from_q = rfc3339_secs(from);
    let to_q = rfc3339_secs(to);
    let uri = format!(
        "/workspaces/test-ws/crons/timeline?from={}&to={}",
        from_q, to_q
    );
    let (status, body) = send(env.router, "GET", &uri).await;
    assert_eq!(status, StatusCode::OK);

    // A trips the cap → truncated must be true.
    assert_eq!(
        body.get("truncated").and_then(|v| v.as_bool()),
        Some(true),
        "minute-burst is far over the cap; truncated must surface"
    );

    // B's entries (~30 daily fires) must still be present despite A
    // exhausting iteration. Count by cron_name to filter cleanly.
    let entries = body.get("entries").and_then(|v| v.as_array()).unwrap();
    let b_count = entries
        .iter()
        .filter(|e| {
            e.get("cron_name").and_then(|v| v.as_str()) == Some("morning-daily")
        })
        .count();
    assert!(
        b_count >= 25,
        "morning-daily should produce ~30 entries over 30 days; got {} (cap on cron A bled into B?)",
        b_count
    );
}

struct KindCounts {
    past: usize,
    future: usize,
    #[allow(dead_code)]
    missed: usize,
}

fn kind_counts(entries: &[Value]) -> KindCounts {
    let mut past = 0;
    let mut future = 0;
    let mut missed = 0;
    for e in entries {
        match e.get("kind").and_then(|v| v.as_str()) {
            Some("past") => past += 1,
            Some("future") => future += 1,
            Some("missed") => missed += 1,
            _ => {}
        }
    }
    KindCounts {
        past,
        future,
        missed,
    }
}

