//! Integration tests for `cli::cmd_list_agents::run`.
//!
//! Pattern mirrors `cli_status` / `cli_workspaces`: spin up the real runtime
//! router on an ephemeral loopback port, inject `WorkspaceContext` /
//! `AgentInfo` directly into shared state, then point a `cli::Client` at
//! that port and call the handler. The redaction-focused tests rely on the
//! handler's projection (`AgentView` vs `AgentDetail`) rather than the
//! runtime's wire shape — so we double-check by issuing the raw GET and
//! confirming what the handler would have seen, then explicitly assert the
//! projection drops or redacts the sensitive fields.
//!
//! Why inject `AgentInfo` directly instead of going through `/agents/add`?
//! The provision flow requires a real Claude/Codex binary + provider preflight
//! and writes a `me.json` to disk; that's a lot of moving parts for a test
//! whose subject is the CLI's serialization filter, not the provision flow.
//! Direct state injection keeps the test boundary tight on what we're
//! actually testing — the redaction policy.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;

use gitim_runtime::cli::{cmd_list_agents, CliError, Client};
use gitim_runtime::http::{create_router, AgentInfo, SharedRuntimeState};
use gitim_runtime::workspace::WorkspaceContext;

/// Spin up the runtime router on `127.0.0.1:0` and return the bound address,
/// the shared state (for direct workspace / agent injection), and the join
/// handle so callers can abort at test end.
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

/// Inject a barebones WorkspaceContext into state. Mirrors
/// `cli_workspaces::inject_workspace`. No git_config / human_repo because
/// `agents_list` only consults the agents map.
fn inject_workspace(state: &SharedRuntimeState, slug: &str, name: &str) {
    // Path doesn't have to exist for the agents-list route — the handler
    // only reads `ctx.agents`. Use a deterministic placeholder so the test
    // assertion can hit `repo_path` predictably.
    let ctx = WorkspaceContext::new(
        slug.to_string(),
        name.to_string(),
        PathBuf::from(format!("/tmp/cli-list-agents-{slug}")),
    );
    state
        .lock()
        .unwrap()
        .workspaces
        .insert(slug.to_string(), ctx);
}

/// Build a fully-populated `AgentInfo` so projection tests can confirm
/// both "sensitive fields exist on the wire" and "the CLI projection drops
/// or redacts them". Defaulting the operationally-internal fields
/// (`loop_handle`, `last_activity`) lets the test stay focused on the
/// fields we actually project.
fn make_agent_with_env(
    id: &str,
    handler: &str,
    repo_path: &str,
    system_prompt: &str,
    env: HashMap<String, String>,
) -> AgentInfo {
    AgentInfo {
        id: id.to_string(),
        handler: handler.to_string(),
        display_name: handler.to_string(),
        status: "idle".to_string(),
        last_activity: None,
        messages_processed: 0,
        repo_path: repo_path.to_string(),
        provider: Some("claude".to_string()),
        model: None,
        system_prompt: Some(system_prompt.to_string()),
        introduction: None,
        env,
        error_message: None,
        session_usage: None,
        llm_provider: None,
        llm_model: None,
        usage_summary: None,
        loop_handle: None,
    }
}

/// Insert an agent into the workspace's agents map. Workspace must exist.
fn insert_agent(state: &SharedRuntimeState, slug: &str, agent: AgentInfo) {
    let mut s = state.lock().unwrap();
    let ctx = s
        .workspaces
        .get_mut(slug)
        .expect("workspace must be injected before agents");
    ctx.agents.insert(agent.id.clone(), agent);
}

// -- Redaction tests --

/// Default mode (`--detailed` = false) must drop sensitive fields entirely.
/// Asserts both the positive shape (id/handler/display_name/status present)
/// and the negative shape (no repo_path/system_prompt/env/etc.). We can't
/// capture the handler's println output from inside the process, so we
/// instead inspect what the handler *would* have seen by replaying the
/// projection logic against the raw `/agents` response. The handler call
/// itself proves the projection path doesn't error.
#[tokio::test]
async fn test_list_agents_redacted_default() {
    let (addr, state, server) = spawn_server().await;
    let client = client_for(addr);

    inject_workspace(&state, "alpha", "Alpha");
    let mut env = HashMap::new();
    env.insert("API_KEY".to_string(), "secret123".to_string());
    env.insert("DEBUG".to_string(), "1".to_string());
    let agent = make_agent_with_env(
        "agent-1",
        "alice",
        "/abs/repo/alice",
        "You are a helpful agent.",
        env,
    );
    insert_agent(&state, "alpha", agent);

    // Workspace omitted → auto-pick since only one workspace exists. detailed=false.
    let exit_code = cmd_list_agents::run(&client, None, false)
        .await
        .expect("list-agents returns Ok");
    assert_eq!(exit_code, 0);

    // Replay the projection: raw GET then deserialize through AgentView.
    // This is the same code path the handler exercised; we're just looking
    // at the result here.
    let raw = client
        .get("/workspaces/alpha/agents")
        .await
        .expect("agents responds");
    let arr = raw
        .get("agents")
        .and_then(|v| v.as_array())
        .expect("agents array");
    assert_eq!(arr.len(), 1);

    // Each agent JSON object run through AgentView serde drops everything
    // not in the AgentView struct.
    let view: gitim_runtime::cli::AgentView =
        serde_json::from_value(arr[0].clone()).expect("AgentView parse");
    assert_eq!(view.id, "agent-1");
    assert_eq!(view.handler, "alice");
    assert_eq!(view.display_name, "alice");
    assert_eq!(view.status, "idle");
    assert_eq!(view.messages_processed, 0);

    // Round-trip back to JSON; sensitive keys must be absent on the wire.
    let serialized = serde_json::to_value(&view).expect("serialize");
    let obj = serialized.as_object().expect("object");
    for forbidden in [
        "repo_path",
        "system_prompt",
        "env",
        "session_usage",
        "usage_summary",
        "introduction",
        "error_message",
    ] {
        assert!(
            !obj.contains_key(forbidden),
            "AgentView leaked sensitive field {forbidden}: {obj:?}",
        );
    }

    server.abort();
}

/// `--detailed` includes the sensitive fields, but env values still pass
/// through `redact_env_secrets`. The redaction happens inside the CLI before
/// printing, so the on-disk runtime state (and the raw `/agents` response)
/// still carries the real secret — that's the whole point of the CLI being
/// the redaction boundary.
#[tokio::test]
async fn test_list_agents_detailed_redacts_secrets() {
    let (addr, state, server) = spawn_server().await;
    let client = client_for(addr);

    inject_workspace(&state, "alpha", "Alpha");
    let mut env = HashMap::new();
    env.insert("API_KEY".to_string(), "secret123".to_string());
    env.insert("DEBUG".to_string(), "1".to_string());
    let agent = make_agent_with_env(
        "agent-1",
        "alice",
        "/abs/repo/alice",
        "You are a helpful agent.",
        env,
    );
    insert_agent(&state, "alpha", agent);

    // detailed=true.
    let exit_code = cmd_list_agents::run(&client, None, true)
        .await
        .expect("list-agents detailed returns Ok");
    assert_eq!(exit_code, 0);

    // Sanity-check the raw runtime response carries the real secret so we
    // know the redaction is happening on the CLI side, not somehow upstream.
    let raw = client
        .get("/workspaces/alpha/agents")
        .await
        .expect("agents responds");
    let raw_agent = &raw.get("agents").and_then(|v| v.as_array()).unwrap()[0];
    assert_eq!(
        raw_agent["env"]["API_KEY"], "secret123",
        "runtime side should NOT pre-redact — that's the CLI's job",
    );

    // Run the same projection the handler used and confirm the output of
    // that step swaps secret-shaped values to "<redacted>".
    let detail =
        gitim_runtime::cli::agent_detail_from_value(raw_agent).expect("agent_detail_from_value");

    // Sensitive fields surface in --detailed mode.
    assert_eq!(detail.repo_path, "/abs/repo/alice");
    assert_eq!(
        detail.system_prompt.as_deref(),
        Some("You are a helpful agent.")
    );

    // Env: secret-shaped key redacted, benign key untouched.
    assert_eq!(
        detail.env.get("API_KEY"),
        Some(&"<redacted>".to_string()),
        "API_KEY value should be redacted, not '{}'",
        detail
            .env
            .get("API_KEY")
            .map(|s| s.as_str())
            .unwrap_or("<missing>"),
    );
    assert_eq!(detail.env.get("DEBUG"), Some(&"1".to_string()));

    server.abort();
}

// -- Workspace selection tests --

/// Multiple workspaces, explicit `--workspace` picks the right one. We seed
/// two distinct agent IDs so a "wrong workspace picked" bug would mis-route.
#[tokio::test]
async fn test_list_agents_explicit_workspace() {
    let (addr, state, server) = spawn_server().await;
    let client = client_for(addr);

    inject_workspace(&state, "ws-a", "WS A");
    inject_workspace(&state, "ws-b", "WS B");
    insert_agent(
        &state,
        "ws-a",
        make_agent_with_env("agent-a", "alice", "/a", "", HashMap::new()),
    );
    insert_agent(
        &state,
        "ws-b",
        make_agent_with_env("agent-b", "bob", "/b", "", HashMap::new()),
    );

    let exit_code = cmd_list_agents::run(&client, Some("ws-b".to_string()), false)
        .await
        .expect("explicit ws-b ok");
    assert_eq!(exit_code, 0);

    // Replay: confirm only ws-b's agents come back when we ask for ws-b.
    let raw = client
        .get("/workspaces/ws-b/agents")
        .await
        .expect("agents responds");
    let arr = raw.get("agents").and_then(|v| v.as_array()).unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["id"], "agent-b");
    assert_eq!(arr[0]["handler"], "bob");

    server.abort();
}

/// Multiple workspaces without `--workspace` must error before issuing the
/// agents request. Asserts `InvalidConfig` variant (exit 1 per
/// `from_cli_error`) and that the error message lists both candidate slugs
/// so the user can pick one without re-running.
#[tokio::test]
async fn test_list_agents_ambiguous_workspace() {
    let (addr, state, server) = spawn_server().await;
    let client = client_for(addr);

    inject_workspace(&state, "ws-a", "WS A");
    inject_workspace(&state, "ws-b", "WS B");

    let err = cmd_list_agents::run(&client, None, false)
        .await
        .expect_err("ambiguous workspace must error");
    assert!(
        matches!(err, CliError::InvalidConfig(_)),
        "expected InvalidConfig, got: {err:?}",
    );
    let msg = err.to_string();
    assert!(msg.contains("ws-a"), "error should list ws-a: {msg}");
    assert!(msg.contains("ws-b"), "error should list ws-b: {msg}");
    assert!(
        msg.contains("--workspace"),
        "error should mention --workspace: {msg}",
    );

    server.abort();
}

/// Zero workspaces, no flag → `InvalidConfig` with the "no workspace
/// configured" hint that points the user at `/git/init` or the WebUI.
#[tokio::test]
async fn test_list_agents_empty_workspace() {
    let (addr, _state, server) = spawn_server().await;
    let client = client_for(addr);

    // No workspaces injected — fresh state.
    let err = cmd_list_agents::run(&client, None, false)
        .await
        .expect_err("empty list must error");
    assert!(
        matches!(err, CliError::InvalidConfig(_)),
        "expected InvalidConfig, got: {err:?}",
    );
    let msg = err.to_string();
    assert!(
        msg.contains("no workspace configured"),
        "error should hint user: {msg}",
    );

    server.abort();
}
