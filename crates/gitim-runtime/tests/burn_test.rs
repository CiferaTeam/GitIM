//! Integration tests for `POST /workspaces/{slug}/agents/burn` (Task B.3).
//!
//! Per `docs/plans/2026-05-09-archive-protocol/03-runtime.md`, the burn endpoint
//! orchestrates daemon `depart_user` + runtime-side cleanup (clone delete +
//! hermes profile delete + ctx.agents removal + SSE broadcast). These tests
//! exercise:
//!
//! 1. burn nonexistent agent id → 4xx + `error_code: "not_an_agent"`
//! 2. burn a `users/<h>.meta.yaml` that exists but is NOT in `ctx.agents`
//!    (i.e. a human user) → same 4xx (P1.c — verify the type guard)
//! 3. happy path: real provisioned agent + daemon → end-to-end departure,
//!    archive entry on remote, clone removed, ctx.agents cleared, SSE event
//! 4. daemon `depart_user` returns ok=false (achieved by removing the
//!    user file before burn so the daemon rejects with "user not found")
//!    → 5xx with `error_code: "daemon_depart_failed"` and runtime preserves
//!    the clone for retry
//! 5. hermes provider on an agent whose profile dir doesn't exist → cleanup
//!    still succeeds (best-effort, only warns)
//!
//! Tests that spawn real daemons are `#[serial]` to keep daemon socket paths
//! and bare-remote pushes from racing each other.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serial_test::serial;
use tower::ServiceExt;

use gitim_runtime::http::{create_router, AgentInfo, SharedRuntimeState};
use gitim_runtime::workspace::WorkspaceContext;
use gitim_runtime::{provision_agent, AgentConfig};

use common::{ensure_daemon_in_path, setup_bare_remote, short_tempdir, stop_daemon};

// -- Shared helpers --

async fn body_to_json(resp: axum::response::Response) -> serde_json::Value {
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).expect("response body is JSON")
}

fn burn_request(slug: &str, id: &str) -> Request<Body> {
    Request::builder()
        .uri(format!("/workspaces/{slug}/agents/burn"))
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::json!({ "id": id }).to_string()))
        .unwrap()
}

fn inject_workspace(state: &SharedRuntimeState, slug: &str, ws: &Path) {
    let mut s = state.lock().unwrap();
    let ctx = WorkspaceContext::new(slug.to_string(), slug.to_string(), ws.to_path_buf());
    s.workspaces.insert(slug.to_string(), ctx);
}

/// Insert a fully-formed `AgentInfo` into the workspace's `ctx.agents`.
/// Mirrors what `agents_add` does after provision_agent succeeds, minus the
/// agent loop spawn — burn doesn't depend on a running loop.
fn insert_agent(
    state: &SharedRuntimeState,
    slug: &str,
    handler: &str,
    repo_path: &Path,
    provider: &str,
) {
    let mut s = state.lock().unwrap();
    let ctx = s.workspaces.get_mut(slug).expect("workspace exists");
    ctx.agents.insert(
        handler.to_string(),
        AgentInfo {
            id: handler.to_string(),
            handler: handler.to_string(),
            display_name: handler.to_string(),
            status: "idle".to_string(),
            last_activity: None,
            messages_processed: 0,
            repo_path: repo_path.display().to_string(),
            provider: Some(provider.to_string()),
            model: None,
            system_prompt: None,
            introduction: None,
            env: std::collections::HashMap::new(),
            error_message: None,
            session_usage: None,
            llm_provider: None,
            llm_model: None,
            usage_summary: None,
            loop_handle: None,
        },
    );
}

/// Provision a real agent (clone + daemon + onboard) under the given workspace
/// directory. Returns the absolute clone path. The agent's daemon stays alive
/// until burn (or the test's cleanup) kills it — burn relies on this so it can
/// RPC `depart_user`.
async fn provision_test_agent(workspace: &Path, handler: &str, remote: &Path) -> PathBuf {
    std::fs::create_dir_all(workspace).unwrap();
    let config = AgentConfig {
        handler: handler.to_string(),
        display_name: handler.to_string(),
        remote_url: remote.to_str().unwrap().into(),
        github_email: None,
    };
    let handle = provision_agent(workspace, &config, true).await.unwrap();
    handle.repo_root
}

/// Burn through the router with an idempotent-retry tolerance for the
/// known SIGTERM-vs-rm race in `cleanup_agent_runtime_side` (the kill is
/// non-blocking, so the daemon may still hold `.gitim/run/` when
/// `remove_dir_all` walks it on macOS). Returns the final response body.
///
/// Note: as of the final-review fix, production `hard_delete_agent_dir`
/// retries internally up to 3× on ENOTEMPTY / EBUSY with short backoff,
/// so the burn endpoint should converge on its own. We keep this helper
/// as defense-in-depth — under heavy parallel `serial_test` load on macOS
/// the daemon's signal handler can still spike past the 50/100/150 ms
/// internal backoff, and the test-side retry tolerates that without
/// hiding real burn-protocol regressions (it only retries on the
/// canonical race signature; any other 5xx panics).
async fn burn_with_idempotent_retry(
    router: &axum::Router,
    slug: &str,
    handler: &str,
) -> serde_json::Value {
    for attempt in 0..3 {
        let response = router
            .clone()
            .oneshot(burn_request(slug, handler))
            .await
            .unwrap();
        let status = response.status();
        let body = body_to_json(response).await;
        if status == StatusCode::OK {
            return body;
        }
        // Only retry on the known race signature. Any other 5xx is a real
        // bug worth surfacing, so panic with the diagnostic body.
        let err = body["error"].as_str().unwrap_or("");
        if !err.contains("Directory not empty") && !err.contains("Resource busy") {
            panic!("burn returned unexpected error on attempt {attempt}: {body:?}");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("burn never converged after 3 attempts (daemon shutdown race)");
}

/// `git ls-tree -r origin/main` on the bare remote — the cheapest, least
/// flaky way to check the post-depart commit landed without spinning up a
/// human clone with its own sync loop.
fn bare_has_path(bare: &Path, path: &str) -> bool {
    let head_branch = match Command::new("git")
        .args(["symbolic-ref", "--short", "HEAD"])
        .current_dir(bare)
        .output()
    {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => "main".to_string(),
    };
    let output = Command::new("git")
        .args(["ls-tree", "-r", "--name-only", &head_branch])
        .current_dir(bare)
        .output()
        .expect("git ls-tree on bare");
    let listing = String::from_utf8_lossy(&output.stdout);
    listing.lines().any(|l| l == path)
}

// -- Test 1: burn nonexistent agent id --

/// Calling burn with an `id` not in `ctx.agents` and not anywhere else in the
/// workspace must return `not_an_agent`. No daemon is spawned, no filesystem
/// is touched. This is the cheapest of the five tests; no `#[serial]` needed.
#[tokio::test]
async fn test_burn_nonexistent_agent_returns_not_an_agent() {
    let tmp = tempfile::tempdir().expect("workspace tempdir");
    let (router, state) = create_router();
    inject_workspace(&state, "test-ws", tmp.path());

    let response = router
        .oneshot(burn_request("test-ws", "ghost"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = body_to_json(response).await;
    assert_eq!(body["ok"], serde_json::Value::Bool(false));
    assert_eq!(
        body["error_code"],
        serde_json::Value::String("not_an_agent".to_string()),
    );
}

// -- Test 2: burn a human user (P1.c type guard) --

/// The burn endpoint is for agents only — humans are out of v1 scope. If the
/// caller passes a handler that exists in `users/<h>.meta.yaml` but was never
/// registered as an agent in `ctx.agents`, we must reject with
/// `not_an_agent` and leave the user file alone. The daemon's `depart_user`
/// is type-agnostic, so this guard sits at the runtime layer.
#[tokio::test]
async fn test_burn_human_user_returns_not_an_agent() {
    let tmp = tempfile::tempdir().expect("workspace tempdir");
    let (router, state) = create_router();
    inject_workspace(&state, "test-ws", tmp.path());

    // Mimic a workspace containing a human user file. We don't need a full
    // human clone — burn's gate runs purely off `ctx.agents`. The file's job
    // is to be present after burn so we can prove cleanup didn't run.
    let users_dir = tmp.path().join(".gitim-runtime/human/users");
    std::fs::create_dir_all(&users_dir).unwrap();
    let user_file = users_dir.join("charlie.meta.yaml");
    std::fs::write(&user_file, "handler: charlie\ndisplay_name: Charlie\n").unwrap();

    let response = router
        .oneshot(burn_request("test-ws", "charlie"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = body_to_json(response).await;
    assert_eq!(body["ok"], serde_json::Value::Bool(false));
    assert_eq!(
        body["error_code"],
        serde_json::Value::String("not_an_agent".to_string()),
    );
    assert!(
        user_file.exists(),
        "burn must not touch the human user's meta file when rejecting on the type guard"
    );
}

// -- Test 3: happy path full e2e --

/// The headline test: provision a real agent + daemon → burn → assert the
/// daemon ran the full archive-protocol depart, the clone is gone, the in-
/// memory agent entry is gone, the SSE broadcast fired. Reads the bare
/// remote with `git ls-tree` to verify Phase 4's `archive/users/<h>.meta.yaml`
/// got pushed — much more reliable than waiting on a separate clone's sync
/// loop, and it proves the daemon's commit chain reached the canonical
/// remote (the same place a human clone would later fetch from).
#[tokio::test]
#[serial]
async fn test_burn_happy_path_full_e2e() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let bare = setup_bare_remote(&tmp);
    let workspace = tmp.path().join("ws");
    let handler = "alice";

    let agent_path = provision_test_agent(&workspace, handler, &bare).await;

    // Sanity: provision should land users/alice.meta.yaml in the agent clone
    // (and push it to the bare remote). If this fails the test below would
    // fail for the wrong reason — fail fast here.
    assert!(
        agent_path.join("users/alice.meta.yaml").exists(),
        "agent provision should have created users/alice.meta.yaml"
    );

    let (router, state) = create_router();
    let slug = "test-ws";
    inject_workspace(&state, slug, &workspace);
    insert_agent(&state, slug, handler, &agent_path, "mock");

    // Subscribe to the activity broadcast BEFORE burn so we don't race the
    // send. `subscribe()` only catches messages dispatched after the call.
    let mut activity_rx = {
        let s = state.lock().unwrap();
        s.workspaces[slug].activity_tx.subscribe()
    };

    // Have the agent author a message so Phase 1 has something to do —
    // the leave-workspace event has to land in `general` because that's
    // where alice posted. Without this, Phase 1 is a no-op and the test
    // wouldn't notice if Phase 1 silently broke.
    let agent_client = gitim_client::GitimClient::new(&agent_path);
    let send_resp = agent_client
        .send("general", "hi from alice", None, None)
        .await
        .expect("send should succeed");
    assert!(send_resp.ok, "send: {:?}", send_resp.error);

    // Give the agent's sync loop a moment to push the message commit. Without
    // this, depart_user may run before the channel commit hits the remote
    // and Phase 1 wouldn't see alice as an author. 3s mirrors the cadence
    // poller tests use for the same reason.
    tokio::time::sleep(Duration::from_secs(3)).await;

    // POST burn. The cleanup helper SIGTERMs the daemon then immediately
    // `remove_dir_all`s the clone (B.1 doesn't wait for the daemon to fully
    // exit). On macOS that occasionally races into ENOTEMPTY when the
    // daemon's signal handler is still tearing down `.gitim/run/`. The
    // documented recovery path is exactly the same as the daemon-failure
    // retry: a follow-up burn — depart_user is idempotent and the second
    // call short-circuits to cleanup-only via `archive/users/<h>.meta.yaml`.
    // Two attempts is plenty: the daemon has had hundreds of milliseconds
    // to finish dying by the time the retry fires.
    let body = burn_with_idempotent_retry(&router, slug, handler).await;
    assert_eq!(body["ok"], serde_json::Value::Bool(true));

    // Step 4 — clone is gone.
    assert!(
        !agent_path.exists(),
        "burn should hard-delete the agent clone directory"
    );

    // Step 7 — agent entry removed.
    {
        let s = state.lock().unwrap();
        assert!(
            !s.workspaces[slug].agents.contains_key(handler),
            "burn should remove the agent from ctx.agents"
        );
    }

    // SSE broadcast — must surface a `burned` event for this agent.
    let event = tokio::time::timeout(Duration::from_secs(1), activity_rx.recv())
        .await
        .expect("burn SSE should arrive within 1s")
        .expect("activity_tx broadcast");
    assert_eq!(event.event_type, "burned");
    assert_eq!(event.agent_id, handler);
    assert_eq!(event.workspace_id, slug);

    // Phase 4 verification: archive/users/alice.meta.yaml committed and
    // pushed to the bare remote. This is the daemon's terminal state — its
    // presence proves all four depart_user phases ran. The original
    // users/alice.meta.yaml must also be gone (git mv, not git cp).
    assert!(
        bare_has_path(&bare, "archive/users/alice.meta.yaml"),
        "depart_user Phase 4 should have pushed archive/users/alice.meta.yaml"
    );
    assert!(
        !bare_has_path(&bare, "users/alice.meta.yaml"),
        "depart_user Phase 4 should have removed users/alice.meta.yaml (git mv)"
    );

    // No stop_daemon — the burn already kills the agent's daemon. The clone
    // is gone, so there's no socket to talk to anyway.
}

// -- Test 4: daemon depart_user fails → runtime preserves clone for retry --

/// When the daemon replies `ok=false` to `depart_user`, the runtime must
/// short-circuit and leave the clone + ctx.agents intact so the user can
/// retry. The plan emphasises this: depart_user is idempotent, so retrying
/// after a partial failure resumes from the first incomplete phase.
///
/// We can't easily inject a controlled push failure into the daemon's
/// internals from here, so we provoke `ok=false` the cleanest way available:
/// after a successful provision, delete the agent's `users/<h>.meta.yaml`.
/// `handle_depart_user` checks for the active user file before phase 1 and
/// returns `error: "user @<h> not found"` (ok=false) when it's missing. The
/// runtime must surface this as `daemon_depart_failed` and skip cleanup.
#[tokio::test]
#[serial]
async fn test_burn_daemon_depart_failed_preserves_clone() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let bare = setup_bare_remote(&tmp);
    let workspace = tmp.path().join("ws");
    let handler = "broken-bot";

    let agent_path = provision_test_agent(&workspace, handler, &bare).await;

    // Yank the active user file. This is the file `handle_depart_user`
    // gates on (line ~73 of daemon depart.rs) — without it the handler
    // returns `ok=false` with "user @<h> not found", which is exactly the
    // shape of failure the runtime must treat as `daemon_depart_failed`.
    std::fs::remove_file(
        agent_path
            .join("users")
            .join(format!("{handler}.meta.yaml")),
    )
    .expect("remove users/<h>.meta.yaml");

    let (router, state) = create_router();
    let slug = "test-ws";
    inject_workspace(&state, slug, &workspace);
    insert_agent(&state, slug, handler, &agent_path, "mock");

    let response = router.oneshot(burn_request(slug, handler)).await.unwrap();
    let status = response.status();
    let body = body_to_json(response).await;
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "expected 500 for daemon failure: {body:?}"
    );
    assert_eq!(body["ok"], serde_json::Value::Bool(false));
    assert_eq!(
        body["error_code"],
        serde_json::Value::String("daemon_depart_failed".to_string()),
        "body: {body:?}"
    );

    // The clone is the proof of "no cleanup ran" — the burn handler
    // contract is steps 4-7 are skipped on daemon error, leaving the clone
    // for an idempotent retry. ctx.agents likewise stays intact.
    assert!(
        agent_path.exists(),
        "burn must leave the clone intact when daemon returns ok=false"
    );
    {
        let s = state.lock().unwrap();
        assert!(
            s.workspaces[slug].agents.contains_key(handler),
            "burn must leave ctx.agents intact when daemon returns ok=false"
        );
    }

    // The agent's daemon is still alive after a daemon-error burn (we only
    // skip the cleanup steps, not the daemon revival). Stop it cleanly so
    // the next test's daemon can bind a fresh socket.
    stop_daemon(&agent_path).await;
}

// -- Test 5: hermes provider, profile delete is best-effort --

/// `cleanup_agent_runtime_side` calls `hermes_profile::delete_profile` for
/// agents tagged `provider == "hermes"`. The plan says this is best-effort
/// (failures only warn). We provoke the missing-profile case by not setting
/// up any hermes profile at all and verifying burn still succeeds. Whether
/// the host has hermes installed or not, both branches in
/// `delete_profile_with` already return `Ok(())` for "no such profile" — so
/// the burn must complete regardless of host setup.
#[tokio::test]
#[serial]
async fn test_burn_hermes_profile_delete_failure_warns_only() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let bare = setup_bare_remote(&tmp);
    let workspace = tmp.path().join("ws");
    let handler = "hermes-bot";

    let agent_path = provision_test_agent(&workspace, handler, &bare).await;

    let (router, state) = create_router();
    let slug = "test-ws";
    inject_workspace(&state, slug, &workspace);
    // Tag the agent as hermes-provided so `cleanup_agent_runtime_side`
    // takes the profile-delete branch. We don't actually run hermes
    // provisioning — the point of this test is to prove cleanup is robust
    // when the profile dir is missing or hermes is unreachable.
    insert_agent(&state, slug, handler, &agent_path, "hermes");

    // Same SIGTERM-vs-rm race concern as test 3 — the cleanup path is
    // identical, just with the hermes-profile delete tacked on. Reuse the
    // retry helper so the test stays deterministic on macOS.
    let body = burn_with_idempotent_retry(&router, slug, handler).await;
    assert_eq!(body["ok"], serde_json::Value::Bool(true));

    // Cleanup ran end-to-end — clone and ctx.agents both gone.
    assert!(
        !agent_path.exists(),
        "burn should hard-delete the clone even when hermes profile is missing"
    );
    {
        let s = state.lock().unwrap();
        assert!(
            !s.workspaces[slug].agents.contains_key(handler),
            "burn should drop the agent entry even when hermes profile is missing"
        );
    }

    // No stop_daemon — burn killed the agent daemon and removed the clone.
}
