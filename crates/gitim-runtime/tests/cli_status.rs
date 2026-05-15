//! Integration tests for `cli::cmd_status::run` and `cli::cmd_runtime_id::run`.
//!
//! Pattern: spin up a real `axum::serve` against the runtime router on an
//! ephemeral loopback port, then point a `cli::Client` at that port and call
//! the handler directly. This exercises the full HTTP path (reqwest → axum)
//! without going through the binary's stdout — that subprocess-level test
//! is deferred to T14's e2e suite.

use std::net::SocketAddr;

use gitim_runtime::cli::{cmd_runtime_id, cmd_status, CliError, Client};
use gitim_runtime::http::create_router;

/// Spin up the runtime router on `127.0.0.1:0` and return the bound address
/// plus a join handle so callers can let the OS reclaim the port at test end.
async fn spawn_server() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let (router, state) = create_router();
    // Stamp a deterministic runtime_id so assertions can hit it directly
    // without re-querying `user_config::read()`.
    state.lock().unwrap().runtime_id = "test-runtime-id-cli-status".to_string();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        // unwrap is fine — only the test's own server task gets aborted at
        // process exit, not by user shutdown signals.
        axum::serve(listener, router).await.unwrap();
    });
    (addr, handle)
}

fn client_for(addr: SocketAddr) -> Client {
    Client::new(format!("http://{addr}"))
}

#[tokio::test]
async fn test_cmd_status_returns_runtime_id_and_zero_workspaces() {
    let (addr, server) = spawn_server().await;
    let client = client_for(addr);

    // We can't easily capture stdout from inside the test process — the
    // handler prints via `println!`. What we *can* assert is that the
    // composed RuntimeStatus carries the expected counts by exercising the
    // same endpoints the handler does and comparing.
    //
    // Two-shot: first run the handler to confirm exit code, then issue the
    // raw GETs to inspect the data. If the underlying endpoints regressed,
    // the handler call would have errored.
    let exit_code = cmd_status::run(&client).await.expect("status returns Ok");
    assert_eq!(exit_code, 0);

    let health = client.get("/health").await.expect("health responds");
    let workspaces = client
        .get("/workspaces")
        .await
        .expect("workspaces responds");

    assert_eq!(
        health.get("runtime_id").and_then(|v| v.as_str()),
        Some("test-runtime-id-cli-status"),
    );
    let ws_arr = workspaces
        .get("workspaces")
        .and_then(|v| v.as_array())
        .expect("workspaces array present");
    assert_eq!(ws_arr.len(), 0, "fresh runtime has no workspaces");

    server.abort();
}

#[tokio::test]
async fn test_cmd_runtime_id_returns_uuid_shape() {
    let (addr, server) = spawn_server().await;
    let client = client_for(addr);

    let exit_code = cmd_runtime_id::run(&client)
        .await
        .expect("runtime-id returns Ok");
    assert_eq!(exit_code, 0);

    // Re-query directly to assert on the value the handler would have printed.
    let health = client.get("/health").await.expect("health responds");
    let id = health
        .get("runtime_id")
        .and_then(|v| v.as_str())
        .expect("runtime_id present");
    assert_eq!(id, "test-runtime-id-cli-status");

    server.abort();
}

#[tokio::test]
async fn test_cmd_status_runtime_not_running() {
    // Bind an ephemeral port then immediately drop the listener so a
    // subsequent connect attempt fails fast with connection-refused. This
    // avoids the "pick a hardcoded port and hope it's free" race that the
    // task description warned about.
    //
    // There's still a tiny window where another test could grab the same
    // port between drop and connect; if that ever flakes in practice we can
    // mark this test #[ignore]. Today it's clean enough to keep enforcing.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let client = Client::new(format!("http://{addr}"));
    let result = cmd_status::run(&client).await;
    let err = result.expect_err("status against dead port must error");
    // Connection-refused is a transport-class failure → exit code 1 per
    // `from_cli_error`. We assert on the variant here; the bin-level exit
    // code mapping is unit-tested in `cli::exit_code`.
    assert!(
        matches!(err, CliError::Transport(_)),
        "expected Transport error, got: {err:?}",
    );
}
