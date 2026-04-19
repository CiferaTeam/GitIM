//! Multi-workspace integration tests (Task 11).
//!
//! Covers the pieces that only matter once the runtime hosts more than one
//! workspace at a time:
//!   * `recover_from_config` reads the v2 user-config schema and spawns a task
//!     per entry, tolerating entries whose path has disappeared.
//!   * SSE broadcast channels are per-workspace — one workspace's events must
//!     not leak to another workspace's subscriber, even when dispatched
//!     through the live router.
//!   * `AgentActivityEvent`s emitted from the recovery error path carry the
//!     owning workspace's slug.
//!   * Deleting a workspace drops its broadcast Sender, which closes any
//!     in-flight subscriber stream.
//!
//! We deliberately avoid starting real daemons or CLI subprocesses. Happy-path
//! provisioning is already covered by provision/poller tests; here we target
//! the multi-workspace wiring alone.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures::StreamExt;
use serial_test::serial;
use tempfile::TempDir;
use tower::ServiceExt;

use gitim_runtime::http::{
    create_router, recover_agents_for_workspace, recover_from_config, AgentActivityEvent,
    RuntimeState, SharedRuntimeState,
};
use gitim_runtime::user_config::{UserConfig, WorkspaceEntry};
use gitim_runtime::workspace::WorkspaceContext;

/// Swap `HOME` to a tempdir for the duration of a test and restore on drop.
/// Paired with `#[serial(home_env)]` so the process-global var can't race.
struct HomeGuard {
    original: Option<std::ffi::OsString>,
    _tmp: TempDir,
}

impl HomeGuard {
    fn install() -> (Self, std::path::PathBuf) {
        let tmp = TempDir::new().expect("tempdir for HOME");
        let path = tmp.path().to_path_buf();
        let original = std::env::var_os("HOME");
        std::env::set_var("HOME", &path);
        (
            Self {
                original,
                _tmp: tmp,
            },
            path,
        )
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match self.original.take() {
            Some(val) => std::env::set_var("HOME", val),
            None => std::env::remove_var("HOME"),
        }
    }
}

/// Write `~/.gitim/runtime.json` with the v2 schema. Mirrors the on-disk shape
/// that `user_config::write` produces, so we exercise the read path end-to-end.
fn write_runtime_json(home: &Path, cfg: &UserConfig) {
    let dir = home.join(".gitim");
    std::fs::create_dir_all(&dir).expect("create ~/.gitim");
    let path = dir.join("runtime.json");
    std::fs::write(&path, serde_json::to_string_pretty(cfg).unwrap())
        .expect("write runtime.json");
}

fn entry(slug: &str, name: &str, path: &Path) -> WorkspaceEntry {
    WorkspaceEntry {
        slug: slug.to_string(),
        workspace_name: name.to_string(),
        path: path.to_string_lossy().into_owned(),
    }
}

fn empty_state() -> SharedRuntimeState {
    Arc::new(Mutex::new(RuntimeState::default()))
}

fn insert_ws(state: &SharedRuntimeState, slug: &str, path: &Path) {
    let mut s = state.lock().unwrap();
    s.workspaces.insert(
        slug.to_string(),
        WorkspaceContext::new(slug.to_string(), slug.to_string(), path.to_path_buf()),
    );
}

// -- 1. Multi-entry recovery --

#[tokio::test]
#[serial(home_env)]
async fn recover_multiple_workspaces_from_user_config() {
    let (_home_guard, home) = HomeGuard::install();

    // Three workspace dirs on disk. No `.gitim-runtime/human` → the expensive
    // provisioning branch short-circuits and recovery populates ctx only.
    let ws_tmp = TempDir::new().unwrap();
    let ws_a = ws_tmp.path().join("ws-a");
    let ws_b = ws_tmp.path().join("ws-b");
    let ws_c = ws_tmp.path().join("ws-c");
    for p in [&ws_a, &ws_b, &ws_c] {
        std::fs::create_dir_all(p).unwrap();
    }

    let cfg = UserConfig {
        workspaces: vec![
            entry("ws-a", "A", &ws_a),
            entry("ws-b", "B", &ws_b),
            entry("ws-c", "C", &ws_c),
        ],
    };
    write_runtime_json(&home, &cfg);

    let state = empty_state();
    recover_from_config(state.clone()).await;

    let s = state.lock().unwrap();
    assert_eq!(s.workspaces.len(), 3, "all three ws should be inserted");
    assert!(s.workspaces.contains_key("ws-a"));
    assert!(s.workspaces.contains_key("ws-b"));
    assert!(s.workspaces.contains_key("ws-c"));
    // workspace_name preserved on the way through.
    assert_eq!(s.workspaces.get("ws-a").unwrap().workspace_name, "A");
    assert_eq!(s.workspaces.get("ws-b").unwrap().workspace_name, "B");
    assert_eq!(s.workspaces.get("ws-c").unwrap().workspace_name, "C");
    // No human_repo provisioned because .gitim-runtime/human is absent.
    for slug in ["ws-a", "ws-b", "ws-c"] {
        assert!(
            s.workspaces.get(slug).unwrap().human_repo.is_none(),
            "human_repo should stay None when human_dir doesn't exist for {slug}"
        );
    }
}

// -- 2. Missing path is skipped, siblings survive --

#[tokio::test]
#[serial(home_env)]
async fn recover_skips_missing_workspace_path() {
    let (_home_guard, home) = HomeGuard::install();

    let ws_tmp = TempDir::new().unwrap();
    let ws_real = ws_tmp.path().join("real");
    std::fs::create_dir_all(&ws_real).unwrap();
    let ws_missing = ws_tmp.path().join("does-not-exist");
    // Intentionally not created.

    let cfg = UserConfig {
        workspaces: vec![
            entry("real", "Real", &ws_real),
            entry("ghost", "Ghost", &ws_missing),
        ],
    };
    write_runtime_json(&home, &cfg);

    let state = empty_state();
    recover_from_config(state.clone()).await;

    let s = state.lock().unwrap();
    assert_eq!(
        s.workspaces.len(),
        1,
        "only the real workspace should be recovered"
    );
    assert!(s.workspaces.contains_key("real"));
    assert!(
        !s.workspaces.contains_key("ghost"),
        "ghost path missing → must be skipped"
    );
}

// -- 3. Parallel recovery still populates every workspace --
//
// Recovery dispatches one tokio task per entry. A serial loop would also pass
// this assertion, so the test doesn't prove parallelism by itself — but it
// does prove the join-all step waits for every task before returning, which
// is the property the HashMap invariant depends on.

#[tokio::test]
#[serial(home_env)]
async fn recover_populates_all_workspaces_even_without_human_dir() {
    let (_home_guard, home) = HomeGuard::install();

    let ws_tmp = TempDir::new().unwrap();
    let a = ws_tmp.path().join("alpha");
    let b = ws_tmp.path().join("beta");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();

    let cfg = UserConfig {
        workspaces: vec![entry("alpha", "Alpha", &a), entry("beta", "Beta", &b)],
    };
    write_runtime_json(&home, &cfg);

    let state = empty_state();
    recover_from_config(state.clone()).await;

    let s = state.lock().unwrap();
    assert_eq!(s.workspaces.len(), 2);
    assert!(s.workspaces.contains_key("alpha"));
    assert!(s.workspaces.contains_key("beta"));
}

// -- 4. SSE broadcast isolation through the live router --
//
// Sends on B's broadcast Sender must not reach A's /agents/events subscriber.
// We dispatch the SSE request via oneshot, then drive the response body as a
// stream with a timeout — the timeout firing is what proves nothing crossed
// over. A second pass publishes on A's own Sender and asserts that stream
// wakes up, confirming the wiring is live (not deadlocked).

#[tokio::test]
async fn sse_isolation_between_workspaces() {
    let (router, state) = create_router();
    let tmp = TempDir::new().unwrap();
    insert_ws(&state, "ws-a", tmp.path());
    insert_ws(&state, "ws-b", tmp.path());

    // Grab both senders up front. Cloning the Sender keeps A's subscriber
    // count pinned even if other code drops its handle on ctx.
    let (tx_a, tx_b) = {
        let s = state.lock().unwrap();
        (
            s.workspaces.get("ws-a").unwrap().activity_tx.clone(),
            s.workspaces.get("ws-b").unwrap().activity_tx.clone(),
        )
    };

    // Subscribe via the real HTTP route — this exercises the `with_workspace_snapshot`
    // lookup and `BroadcastStream` wrapping inside `agents_events`.
    let resp = router
        .oneshot(
            Request::builder()
                .uri("/workspaces/ws-a/agents/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let mut body = resp.into_body().into_data_stream();

    // Give the handler a beat to install its subscriber before we publish.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Publish on B only. A's stream must stay silent.
    let _ = tx_b.send(AgentActivityEvent {
        agent_id: "bot-b".to_string(),
        workspace_id: "ws-b".to_string(),
        event_type: "tool_use".to_string(),
        detail: "leak-probe".to_string(),
        timestamp: "2026-04-18T00:00:00Z".to_string(),
    });

    // 250ms is enough that a cross-channel leak would have landed but short
    // enough to keep the test snappy. BroadcastStream delivers eagerly.
    let leaked = tokio::time::timeout(Duration::from_millis(250), body.next()).await;
    assert!(
        leaked.is_err(),
        "A-subscriber must not observe B-published events, got: {leaked:?}"
    );

    // Now publish on A and confirm the same stream delivers it. This rules out
    // a trivial pass where both channels were silently broken.
    let _ = tx_a.send(AgentActivityEvent {
        agent_id: "bot-a".to_string(),
        workspace_id: "ws-a".to_string(),
        event_type: "tool_use".to_string(),
        detail: "own-event".to_string(),
        timestamp: "2026-04-18T00:00:01Z".to_string(),
    });

    let frame = tokio::time::timeout(Duration::from_secs(1), body.next())
        .await
        .expect("A-subscriber should receive A-published events within 1s")
        .expect("stream should yield at least one frame")
        .expect("frame result should be Ok");
    let text = std::str::from_utf8(&frame).expect("SSE body is utf-8");
    assert!(
        text.contains("own-event"),
        "A-subscriber frame should carry its own event, got: {text}"
    );
    assert!(
        !text.contains("leak-probe"),
        "A-subscriber frame must not contain B's event payload, got: {text}"
    );
}

// -- 5. Error-path AgentActivityEvent carries workspace_id --
//
// Seeds a workspace where one agent dir is missing the provider field.
// `recover_agents_for_workspace` broadcasts an "error" event while inserting
// the agent in error state — the event's workspace_id must match the slug it
// was broadcast to, not (say) the default "" the struct has at rest elsewhere.

#[tokio::test]
async fn recover_agent_error_event_carries_workspace_id() {
    let tmp = TempDir::new().unwrap();
    let agent_dir = tmp.path().join("broken-bot");
    std::fs::create_dir_all(agent_dir.join(".gitim")).unwrap();
    std::fs::write(
        agent_dir.join(".gitim/me.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "handler": "broken-bot",
            "display_name": "Broken",
            // provider intentionally omitted -> triggers the error branch.
        }))
        .unwrap(),
    )
    .unwrap();

    let state = empty_state();
    let slug = "my-ws-slug";
    insert_ws(&state, slug, tmp.path());

    // Subscribe BEFORE calling recover so we don't race the send.
    let mut rx = {
        let s = state.lock().unwrap();
        s.workspaces.get(slug).unwrap().activity_tx.subscribe()
    };

    recover_agents_for_workspace(state.clone(), slug, tmp.path()).await;

    let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("error event should arrive within 1s")
        .expect("broadcast channel should deliver the event");

    assert_eq!(event.event_type, "error");
    assert_eq!(event.agent_id, "broken-bot");
    assert_eq!(
        event.workspace_id, slug,
        "workspace_id on the emitted event must echo the owning ws slug"
    );
}

// -- 6. DELETE /workspaces/{slug} closes outstanding broadcast subscribers --
//
// The subscriber holds a `broadcast::Receiver` whose lifetime is tied to the
// Sender. Deleting the workspace drops the ctx (and its Sender), so any live
// subscriber should observe a `RecvError::Closed` on the next recv.

#[tokio::test]
async fn delete_workspace_closes_broadcast() {
    let (router, state) = create_router();
    let tmp = TempDir::new().unwrap();
    insert_ws(&state, "dying-ws", tmp.path());

    let mut rx = {
        let s = state.lock().unwrap();
        s.workspaces.get("dying-ws").unwrap().activity_tx.subscribe()
    };

    let resp = router
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/workspaces/dying-ws")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // With the ctx gone, the Sender is dropped. recv() on a closed broadcast
    // channel returns Err(Closed) once the buffer drains (it was already empty).
    let recv = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("recv should return quickly once Sender drops");
    match recv {
        Err(tokio::sync::broadcast::error::RecvError::Closed) => {}
        other => panic!("expected RecvError::Closed after DELETE, got {other:?}"),
    }
}
