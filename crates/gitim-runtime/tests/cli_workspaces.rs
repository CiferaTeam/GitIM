//! Integration tests for `cli::cmd_workspaces::run`.
//!
//! Pattern mirrors `cli_status`: spin up the real runtime router on an
//! ephemeral loopback port, point a `cli::Client` at it, call the handler
//! directly. Stdout assertions are deferred to T14's subprocess-level e2e
//! suite; here we assert on the handler return value plus the raw `/workspaces`
//! response shape (the same data the handler would have printed).

use std::net::SocketAddr;
use std::path::PathBuf;

use gitim_runtime::cli::{cmd_workspaces, Client, CliError};
use gitim_runtime::http::{create_router, SharedRuntimeState};
use gitim_runtime::workspace::WorkspaceContext;

/// Spin up the runtime router on `127.0.0.1:0` and return the bound address,
/// the shared state (for direct workspace injection), and the join handle.
async fn spawn_server() -> (
    SocketAddr,
    SharedRuntimeState,
    tokio::task::JoinHandle<()>,
) {
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

/// Inject a WorkspaceContext directly so the runtime's GET /workspaces serves
/// it. Mirrors the approach in `tests/http_workspaces.rs::inject_workspace`
/// minus the git_config — we only need the slug/name/path that
/// workspace_summary projects into the wire response.
fn inject_workspace(state: &SharedRuntimeState, slug: &str, name: &str, path: PathBuf) {
    let ctx = WorkspaceContext::new(slug.to_string(), name.to_string(), path);
    state
        .lock()
        .unwrap()
        .workspaces
        .insert(slug.to_string(), ctx);
}

#[tokio::test]
async fn test_workspaces_empty_returns_empty_array() {
    let (addr, _state, server) = spawn_server().await;
    let client = client_for(addr);

    let exit_code = cmd_workspaces::run(&client)
        .await
        .expect("workspaces returns Ok");
    assert_eq!(exit_code, 0);

    // Validate the underlying response shape the handler printed.
    let body = client.get("/workspaces").await.expect("workspaces responds");
    let arr = body
        .get("workspaces")
        .and_then(|v| v.as_array())
        .expect("workspaces array");
    assert!(arr.is_empty(), "fresh runtime has no workspaces");

    server.abort();
}

#[tokio::test]
async fn test_workspaces_lists_known_workspace() {
    let (addr, state, server) = spawn_server().await;
    let client = client_for(addr);

    inject_workspace(
        &state,
        "alpha",
        "Alpha",
        PathBuf::from("/tmp/cli-workspaces-alpha"),
    );

    let exit_code = cmd_workspaces::run(&client)
        .await
        .expect("workspaces returns Ok");
    assert_eq!(exit_code, 0);

    let body = client.get("/workspaces").await.expect("workspaces responds");
    let arr = body
        .get("workspaces")
        .and_then(|v| v.as_array())
        .expect("workspaces array");
    assert_eq!(arr.len(), 1, "exactly one workspace injected");
    assert_eq!(arr[0]["slug"], "alpha");
    assert_eq!(arr[0]["workspace_name"], "Alpha");
    // workspace_summary projects path + provider too; assert at least
    // the slug + name pair the test relies on.
    assert!(arr[0].get("path").is_some(), "path field present");
    assert!(arr[0].get("provider").is_some(), "provider field present");

    server.abort();
}

#[tokio::test]
async fn test_workspaces_runtime_not_running() {
    // Ephemeral-port-drop pattern: bind then immediately drop so connect
    // attempts hit connection-refused fast. Tiny window where another test
    // could grab the port — same caveat as cli_status::test_cmd_status_runtime_not_running.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let client = Client::new(format!("http://{addr}"));
    let result = cmd_workspaces::run(&client).await;
    let err = result.expect_err("workspaces against dead port must error");
    // Connection-refused → transport-class failure → exit 1. Variant check;
    // exit-code mapping is unit-tested in cli::exit_code.
    assert!(
        matches!(err, CliError::Transport(_)),
        "expected Transport error, got: {err:?}",
    );
}
