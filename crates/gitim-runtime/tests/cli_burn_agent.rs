//! Integration tests for `cli::cmd_burn_agent::run`.
//!
//! Pattern: spin up the real runtime router on an ephemeral loopback port,
//! inject `WorkspaceContext` / `AgentInfo` directly into shared state, then
//! point a `cli::Client` at that port and call the handler. Identical
//! template to `cli_list_agents` / `cli_add_agent`.
//!
//! Two endpoints back this subcommand and the routing demux is the
//! interesting bit:
//!   * `hard=false` → `POST /workspaces/{slug}/agents/burn` (archive protocol)
//!   * `hard=true`  → `POST /workspaces/{slug}/agents/remove` with
//!                    `hard_delete: true` (quiet local-only delete)
//!
//! The pure `build_burn_request` helper has unit-test coverage in
//! `cmd_burn_agent::tests`. These integration tests verify the *dispatched*
//! request actually reaches the right runtime handler — caught by observing
//! the endpoint-specific structured error semantics:
//!
//!   * `/burn` with a missing agent → 404 + `error_code: "not_an_agent"`
//!     (structured). The CLI surfaces this as `CliError::ResponseErrorCode`.
//!   * `/burn` with an agent whose daemon can't be spawned → 500 +
//!     `error_code: "daemon_unreachable"`. We exploit this by injecting an
//!     agent whose `repo_path` doesn't point at a real clone — daemon spawn
//!     fails fast, and the structured error_code proves the request reached
//!     the burn handler (not remove).
//!   * `/remove` with `hard_delete: true` over a real clone dir → 200 + raw
//!     `{ok: true}` body. Asserting the agent is gone from `ctx.agents`
//!     afterward proves the remove handler ran end-to-end.

mod common;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use common::{ensure_daemon_in_path, short_tempdir, stop_daemon};
use gitim_runtime::cli::{cmd_burn_agent, from_cli_error, CliError, Client};
use gitim_runtime::http::{create_router, AgentInfo, SharedRuntimeState};
use gitim_runtime::workspace::WorkspaceContext;

async fn spawn_server() -> (SocketAddr, SharedRuntimeState, tokio::task::JoinHandle<()>) {
    let (router, state) = create_router();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (addr, state, handle)
}

fn client_for(addr: SocketAddr) -> Client {
    Client::new(format!("http://{addr}"))
}

fn inject_workspace(state: &SharedRuntimeState, slug: &str, path: &Path) {
    let ctx = WorkspaceContext::new(slug.to_string(), slug.to_string(), path.to_path_buf());
    state
        .lock()
        .unwrap()
        .workspaces
        .insert(slug.to_string(), ctx);
}

/// Insert a barebones `AgentInfo` into the workspace's `ctx.agents`. The
/// `repo_path` is the only field that matters operationally for burn /
/// remove — every other field is just metadata that gets dropped during
/// cleanup. Mirrors the same helper in `tests/burn_test.rs::insert_agent`,
/// kept local so this file stands alone.
fn insert_agent(state: &SharedRuntimeState, slug: &str, id: &str, repo_path: &Path) {
    let mut s = state.lock().unwrap();
    let ctx = s.workspaces.get_mut(slug).expect("workspace exists");
    ctx.agents.insert(
        id.to_string(),
        AgentInfo {
            id: id.to_string(),
            handler: id.to_string(),
            display_name: id.to_string(),
            status: "idle".to_string(),
            last_activity: None,
            messages_processed: 0,
            repo_path: repo_path.display().to_string(),
            provider: Some("mock".to_string()),
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

/// Returns `true` if the named agent is still in `ctx.agents`. Used after a
/// remove call to confirm the in-memory state actually got cleaned up.
fn workspace_has_agent(state: &SharedRuntimeState, slug: &str, id: &str) -> bool {
    let s = state.lock().unwrap();
    s.workspaces
        .get(slug)
        .map(|ctx| ctx.agents.contains_key(id))
        .unwrap_or(false)
}

// ── Endpoint demux: hard=false routes to /burn ───────────────────────────────

/// With `hard=false` the CLI must hit `/agents/burn`. We prove that by
/// injecting an agent without ever provisioning a real user.meta.yaml on
/// the daemon side — the burn handler then spawns the daemon, calls
/// `depart_user`, and the daemon replies `ok=false` with "user not found".
/// The runtime wraps that as a 500 + `error_code: "daemon_depart_failed"`,
/// which is unique to the burn endpoint (`/remove` never invokes daemon
/// RPC).
///
/// We observe `CliError::ResponseErrorCode { code: "daemon_depart_failed",
/// http_status: 500, .. }`. The CLI's `process_response` parses the body
/// regardless of HTTP status and the structured `error_code` takes
/// precedence over status-based classification — see `cli/http.rs`. The
/// agent's exit-code mapper then classifies this as **permanent** (exit
/// code 2): the daemon was reachable but the depart logic refused, so
/// retrying without operator intervention won't help.
///
/// This test spawns a real `gitim-daemon` process; we install
/// `ensure_daemon_in_path` to redirect its logs out of `~/.gitim/logs/`,
/// and `stop_daemon` it at the end to keep the test process clean.
#[tokio::test]
async fn test_burn_default_calls_burn_endpoint() {
    ensure_daemon_in_path();
    let (addr, state, server) = spawn_server().await;
    let client = client_for(addr);

    let tmp = short_tempdir();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    inject_workspace(&state, "ws", &ws);

    // The burn handler's `hard_delete_agent_dir` later in the cleanup
    // path requires the agent dir to live directly under the workspace
    // and to be named after the agent id. Set both up so the test
    // failure mode is the daemon RPC (which we want to fingerprint),
    // not a path-shape rejection upstream of it.
    let agent_dir = ws.join("alice");
    std::fs::create_dir_all(&agent_dir).unwrap();
    insert_agent(&state, "ws", "alice", &agent_dir);

    let err = cmd_burn_agent::run(
        &client,
        Some("ws".to_string()),
        "alice".to_string(),
        false, // default: ritual burn
    )
    .await
    .expect_err("agent without provisioned user.meta.yaml must surface an error");

    match &err {
        CliError::ResponseErrorCode {
            code, http_status, ..
        } => {
            assert_eq!(*http_status, 500, "burn daemon failure must be 5xx");
            // `daemon_depart_failed` and `daemon_unreachable` are both
            // exclusive to the burn handler — either one proves we hit
            // `/burn` rather than `/remove`.
            assert!(
                code == "daemon_depart_failed" || code == "daemon_unreachable",
                "expected /burn-exclusive error_code, got: {code}",
            );
        }
        other => panic!("expected ResponseErrorCode from /burn daemon RPC, got: {other:?}",),
    }
    // Structured `error_code` → permanent (exit 2) per
    // `exit_code::from_cli_error`. The daemon was reachable and gave a
    // structured rejection; the agent should NOT auto-retry. Lock this
    // contract so future tweaks to `process_response` don't silently flip
    // burn failures back to transient (3) on a body that has a clear code.
    assert_eq!(from_cli_error(&err), 2);

    // Best-effort: kill the daemon `ensure_daemon_with_log` spawned for
    // the burn attempt. The test process exits soon after, but stopping
    // explicitly avoids a stray daemon lingering past `cargo test`.
    stop_daemon(&agent_dir).await;

    server.abort();
}

// ── Endpoint demux: hard=true routes to /remove ──────────────────────────────

/// With `hard=true` the CLI must hit `/agents/remove` and send
/// `hard_delete: true`. Confirmed by observing the side effects unique to
/// the remove handler:
///   1. Response is `200 {ok: true}` (no structured error)
///   2. The agent's clone dir on disk gets `rm -rf`'d (only happens when
///      `hard_delete: true` is set — `hard_delete: false` is a no-op for
///      the filesystem)
///   3. The agent is removed from `ctx.agents`
///
/// If the CLI routed `hard=true` to `/burn` instead, step 1 would fail
/// with `daemon_unreachable` (burn requires a live daemon).
#[tokio::test]
async fn test_burn_hard_calls_remove_endpoint() {
    let (addr, state, server) = spawn_server().await;
    let client = client_for(addr);

    let tmp = short_tempdir();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    inject_workspace(&state, "ws", &ws);

    // The remove handler's `hard_delete_agent_dir` requires the clone
    // dir to exist under the workspace and have the agent id as its
    // last path component. Set both up.
    let agent_dir = ws.join("alice");
    std::fs::create_dir_all(&agent_dir).unwrap();
    insert_agent(&state, "ws", "alice", &agent_dir);

    let exit_code = cmd_burn_agent::run(
        &client,
        Some("ws".to_string()),
        "alice".to_string(),
        true, // --hard
    )
    .await
    .expect("hard remove of state-only agent must succeed");
    assert_eq!(exit_code, 0);

    // Side effect 1: ctx.agents no longer contains the agent. This is
    // observable only via the remove handler's final mutation step; the
    // burn handler does the same on success but couldn't have reached it
    // (no daemon). So presence-after = remove handler ran end-to-end.
    assert!(
        !workspace_has_agent(&state, "ws", "alice"),
        "remove handler must drop agent from ctx.agents",
    );

    // Side effect 2: the clone dir got `rm -rf`'d. The hard_delete branch
    // is the only place that touches disk; absence here doubles as a
    // signal that hard_delete: true made it onto the wire.
    assert!(
        !agent_dir.exists(),
        "hard_delete: true must remove the agent dir; still present: {}",
        agent_dir.display(),
    );

    server.abort();
}

// ── Nonexistent agent → structured error_code → exit 2 ──────────────────────

/// Burn on an id that doesn't exist in the workspace. The burn handler
/// returns `404 not_an_agent` and the CLI surfaces it as a permanent
/// failure (`exit 2`). The runtime's wire shape is the canonical signal
/// — we don't try to interpret the code beyond that.
#[tokio::test]
async fn test_burn_nonexistent_agent_returns_exit_2() {
    let (addr, state, server) = spawn_server().await;
    let client = client_for(addr);

    let tmp = short_tempdir();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    inject_workspace(&state, "ws", &ws);
    // No agents injected — workspace is empty.

    let err = cmd_burn_agent::run(&client, Some("ws".to_string()), "ghost".to_string(), false)
        .await
        .expect_err("burn of nonexistent agent must error");

    match &err {
        CliError::ResponseErrorCode { code, .. } => {
            // Per `agents_burn` step 1 of the archive protocol. If the
            // runtime ever renames this code we want to see the failure
            // here so docs and CLI exit-code mapping stay in sync.
            assert_eq!(code, "not_an_agent", "unexpected code: {err:?}");
        }
        other => panic!("expected ResponseErrorCode, got: {other:?}"),
    }
    assert_eq!(from_cli_error(&err), 2, "structured error_code → exit 2");

    server.abort();
}

// ── Workspace selection: ambiguous without --workspace ───────────────────────

/// Two workspaces, no `--workspace` flag → `resolve_workspace` refuses to
/// auto-pick and the CLI exits 1. Mirrors the parallel test in
/// `cli_add_agent` / `cli_list_agents` so a future regression in the shared
/// helper is caught from this subcommand's perspective too.
#[tokio::test]
async fn test_burn_workspace_resolution() {
    let (addr, state, server) = spawn_server().await;
    let client = client_for(addr);

    let tmp = short_tempdir();
    let ws_a = tmp.path().join("a");
    let ws_b = tmp.path().join("b");
    std::fs::create_dir_all(&ws_a).unwrap();
    std::fs::create_dir_all(&ws_b).unwrap();
    inject_workspace(&state, "alpha", &ws_a);
    inject_workspace(&state, "beta", &ws_b);

    let err = cmd_burn_agent::run(
        &client,
        None, // no --workspace
        "alice".to_string(),
        false,
    )
    .await
    .expect_err("ambiguous workspace must error");

    assert!(
        matches!(err, CliError::InvalidConfig(_)),
        "expected InvalidConfig, got: {err:?}",
    );
    assert_eq!(from_cli_error(&err), 1, "InvalidConfig → exit 1");

    let msg = err.to_string();
    assert!(msg.contains("alpha"), "message must list alpha: {msg}");
    assert!(msg.contains("beta"), "message must list beta: {msg}");
    assert!(
        msg.contains("--workspace"),
        "message must mention --workspace flag: {msg}",
    );

    server.abort();
}

// ── Sanity: handler signature covers what bin/runtime.rs passes ──────────────

/// Surface check — the bin/runtime.rs dispatch passes
/// `(client, workspace, id, hard)` as four positional args. Lock the
/// signature here so a refactor that adds a parameter trips this test
/// instead of breaking the binary build silently.
#[test]
fn handler_signature_smoke() {
    fn _assert_signature() {
        // Compile-time-only check: this never runs.
        async fn _check(client: &Client) {
            let _: Result<i32, CliError> =
                cmd_burn_agent::run(client, Some("ws".to_string()), "alice".to_string(), false)
                    .await;
        }
        let _ = _check;
    }
    // The compile is the test.
    let _: PathBuf = PathBuf::new();
}
