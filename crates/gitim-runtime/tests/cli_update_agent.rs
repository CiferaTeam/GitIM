#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Integration tests for `cli::cmd_update_agent::run`.
//!
//! Two-layer coverage strategy:
//!
//! - **Wire shape** — the pure body builder is fully unit-tested inside
//!   `cmd_update_agent::tests::build_body_*`. We don't replicate every
//!   permutation here.
//! - **End-to-end behavior** — this file spins up the real runtime router
//!   on an ephemeral loopback port, optionally seeds workspace state, then
//!   exercises the failure paths that exit `run()` before / through HTTP:
//!     * no update fields → exit 1 (no HTTP issued)
//!     * malformed `--env` → exit 1 (no HTTP issued)
//!     * `--system-prompt-file` over 64 KB → exit 1 (no HTTP issued)
//!     * agent not found on the runtime → exit 2 (HTTP 404, permanent)
//!
//! The happy-path PATCH that successfully writes me.json is covered by
//! `tests/agent_patch.rs` against the same handler. We don't dual-stage it
//! here because there's no incremental signal — once the body builder is
//! verified and the dispatch wiring is verified, the HTTP layer is a thin
//! `client.patch(path, body)`.

mod common;

use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;

use common::short_tempdir;
use gitim_runtime::cli::{cmd_update_agent, from_cli_error, CliError, Client};
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

/// Seed an agent with a valid `.gitim/me.json` so the PATCH handler reaches
/// the write path successfully. Returns the tempdir so the caller can keep
/// it alive for the duration of the test.
fn seed_agent(
    state: &SharedRuntimeState,
    slug: &str,
    agent_id: &str,
    provider: &str,
) -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let gitim = dir.path().join(".gitim");
    std::fs::create_dir_all(&gitim).unwrap();
    let me = serde_json::json!({
        "handler": agent_id,
        "provider": provider,
    });
    std::fs::write(gitim.join("me.json"), serde_json::to_string(&me).unwrap()).unwrap();

    let info = AgentInfo {
        id: agent_id.to_string(),
        handler: agent_id.to_string(),
        display_name: agent_id.to_string(),
        status: "idle".to_string(),
        last_activity: None,
        messages_processed: 0,
        repo_path: dir.path().display().to_string(),
        provider: Some(provider.to_string()),
        model: None,
        effort: None,
        system_prompt: None,
        introduction: None,
        env: Default::default(),
        error_message: None,
        session_usage: None,
        llm_provider: None,
        llm_model: None,
        usage_summary: None,
        saturation_summary: None,
        is_working: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        loop_handle: None,
    };
    state
        .lock()
        .unwrap()
        .workspaces
        .get_mut(slug)
        .expect("inject workspace first")
        .agents
        .insert(agent_id.to_string(), info);
    dir
}

/// Builder for `cmd_update_agent::Args` — tests only fill in the fields
/// they're exercising. All other fields default to "unset".
fn baseline_args(workspace: Option<String>, id: &str) -> cmd_update_agent::Args {
    cmd_update_agent::Args {
        workspace,
        id: id.to_string(),
        system_prompt: None,
        system_prompt_file: None,
        model: None,
        effort: None,
        introduction: None,
        env: Vec::new(),
        dotenv_file: None,
        clear_session: false,
    }
}

// ── Validation tests — exit 1, no HTTP call ─────────────────────────────────

/// Empty patch (only `--id`, no update flags) must fail at the CLI boundary
/// without issuing a request. The runtime would happily accept it as a
/// no-op, but the user clearly didn't mean to call update with no diff.
#[tokio::test]
async fn test_update_no_fields_errors() {
    let (addr, _state, server) = spawn_server().await;
    let client = client_for(addr);

    let args = baseline_args(Some("ws".to_string()), "alice");

    let err = cmd_update_agent::run(&client, args)
        .await
        .expect_err("empty patch must error");

    assert!(matches!(err, CliError::InvalidConfig(_)));
    assert_eq!(from_cli_error(&err), 1, "validation errors exit 1");
    let msg = err.to_string();
    assert!(
        msg.contains("no update fields"),
        "msg should explain the failure: {msg}"
    );
    // Hint the user toward the available flags.
    assert!(
        msg.contains("--system-prompt"),
        "msg should list flags: {msg}"
    );
    assert!(msg.contains("--model"), "msg should list flags: {msg}");
    assert!(
        msg.contains("--introduction"),
        "msg should list flags: {msg}"
    );
    assert!(msg.contains("--env"), "msg should list flags: {msg}");
    assert!(
        msg.contains("--dotenv-file"),
        "msg should list flags: {msg}"
    );

    server.abort();
}

/// `--env MALFORMED` (no `=`) parses fail before any body is built.
/// Exit 1 (CLI/config class), no HTTP.
#[tokio::test]
async fn test_update_env_parse_error() {
    let (addr, _state, server) = spawn_server().await;
    let client = client_for(addr);

    let mut args = baseline_args(Some("ws".to_string()), "alice");
    args.env = vec!["MALFORMED".to_string()];

    let err = cmd_update_agent::run(&client, args)
        .await
        .expect_err("malformed --env must error");

    assert!(matches!(err, CliError::InvalidConfig(_)));
    assert_eq!(from_cli_error(&err), 1);
    let msg = err.to_string();
    assert!(msg.contains("MALFORMED"), "msg includes entry: {msg}");
    assert!(msg.contains("KEY=VALUE"), "msg hints expected shape: {msg}");

    server.abort();
}

/// `--system-prompt-file` over 64 KB rejects before any HTTP. Real system
/// prompts are a few KB at most; a 65 KB file is almost certainly a
/// wrong-path mistake (transcript / log).
#[tokio::test]
async fn test_update_system_prompt_file_too_large() {
    let (addr, _state, server) = spawn_server().await;
    let client = client_for(addr);

    let tmp = short_tempdir();
    let huge = tmp.path().join("huge.txt");
    std::fs::write(&huge, vec![b'a'; 65 * 1024]).unwrap();

    let mut args = baseline_args(Some("ws".to_string()), "alice");
    args.system_prompt_file = Some(huge);

    let err = cmd_update_agent::run(&client, args)
        .await
        .expect_err("oversize file must error");

    assert!(matches!(err, CliError::InvalidConfig(_)));
    assert_eq!(from_cli_error(&err), 1);
    let msg = err.to_string();
    assert!(msg.contains("64KB"), "msg must mention the cap: {msg}");
    assert!(
        msg.contains("system_prompt_file"),
        "msg must identify the field: {msg}"
    );

    server.abort();
}

/// Same cap applies to `--dotenv-file`. The runtime also enforces 64 KB
/// on the dotenv body, but the CLI catches it locally so we don't burn an
/// HTTP round-trip.
#[tokio::test]
async fn test_update_dotenv_file_too_large() {
    let (addr, _state, server) = spawn_server().await;
    let client = client_for(addr);

    let tmp = short_tempdir();
    let huge = tmp.path().join("huge.env");
    std::fs::write(&huge, vec![b'a'; 65 * 1024]).unwrap();

    let mut args = baseline_args(Some("ws".to_string()), "alice");
    args.dotenv_file = Some(huge);

    let err = cmd_update_agent::run(&client, args)
        .await
        .expect_err("oversize dotenv must error");

    assert!(matches!(err, CliError::InvalidConfig(_)));
    assert_eq!(from_cli_error(&err), 1);
    assert!(err.to_string().contains("dotenv_file"));

    server.abort();
}

// ── Runtime-level rejection — exit 2 (permanent) ────────────────────────────

/// Agent doesn't exist in the workspace. Runtime's `agents_patch` returns
/// 404 with `{ok: false}` (no `error_code`). With body-first classification,
/// 4xx without `error_code` falls through to `CliError::HttpStatus(404, _)` —
/// the synthesis sentinel only fires for 2xx, so 5xx still maps to transient
/// (see http.rs `process_response_inner`). The downstream contract is exit
/// code 2 (permanent), preserved by the 4xx → permanent mapping in
/// `from_cli_error`.
#[tokio::test]
async fn test_update_agent_not_found() {
    let (addr, state, server) = spawn_server().await;
    let client = client_for(addr);

    let tmp = short_tempdir();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    inject_workspace(&state, "ws", &ws);
    // No agent seeded — the lookup in `agents_patch` will hit the 404 path.

    let mut args = baseline_args(Some("ws".to_string()), "ghost");
    args.system_prompt = Some("hi".to_string());

    let err = cmd_update_agent::run(&client, args)
        .await
        .expect_err("missing agent must error");

    match &err {
        CliError::HttpStatus(status, body) => {
            assert_eq!(*status, 404);
            assert!(
                body.contains("agent not found"),
                "body excerpt must include server's error text: {body}",
            );
        }
        other => panic!("expected HttpStatus(404, _), got: {other:?}"),
    }
    assert_eq!(
        from_cli_error(&err),
        2,
        "4xx without structured error_code → permanent (exit 2)"
    );

    server.abort();
}

// ── Happy-path wire-shape spot-checks ───────────────────────────────────────
//
// The pure body builder is exhaustively unit-tested in
// `cmd_update_agent::tests::build_body_*`. The two integration tests below
// confirm that:
//   * `run()` actually reaches the HTTP layer with the body we expect
//   * the runtime side accepts the body and writes me.json
// We read the agent's `me.json` after the call to verify the patch landed,
// rather than installing a request-capturing intermediary. That keeps the
// test against the real router + handler combo.

#[tokio::test]
async fn test_update_system_prompt_lands_via_real_handler() {
    let (addr, state, server) = spawn_server().await;
    let client = client_for(addr);

    let tmp = short_tempdir();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    inject_workspace(&state, "ws", &ws);
    let agent_dir = seed_agent(&state, "ws", "alice", "claude");

    let mut args = baseline_args(Some("ws".to_string()), "alice");
    args.system_prompt = Some("new system prompt".to_string());

    let rc = cmd_update_agent::run(&client, args)
        .await
        .expect("update should succeed");
    assert_eq!(rc, 0);

    // Verify me.json picked up the new value. If the body shape were
    // wrong (e.g. wrapped as null or omitted), the runtime would have
    // skipped the write and the field would still be missing.
    let me_path = agent_dir.path().join(".gitim/me.json");
    let me_content = std::fs::read_to_string(&me_path).expect("read me.json");
    let me: serde_json::Value = serde_json::from_str(&me_content).expect("parse me.json");
    assert_eq!(
        me["system_prompt"], "new system prompt",
        "patched system_prompt must land in me.json; got: {me:?}"
    );

    server.abort();
}

/// Multiple fields in one call. After PATCH succeeds, the runtime should
/// have written every patched field — confirms our omission rules don't
/// accidentally drop a field on the wire.
///
/// We deliberately leave `--introduction` out of this multi-field test.
/// The runtime's introduction path goes through `daemon.update_user` IPC,
/// which can't be exercised in-process without spawning the agent's
/// per-clone daemon. The wire shape for introduction is verified by the
/// pure unit tests in `cmd_update_agent::tests::build_body_all_fields_present`.
#[tokio::test]
async fn test_update_multiple_fields_lands_via_real_handler() {
    let (addr, state, server) = spawn_server().await;
    let client = client_for(addr);

    let tmp = short_tempdir();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    inject_workspace(&state, "ws", &ws);
    let agent_dir = seed_agent(&state, "ws", "bob", "claude");

    let dotenv_path = tmp.path().join("agent.env");
    std::fs::write(&dotenv_path, "FOO=bar\nBAZ=qux\n").unwrap();

    let mut args = baseline_args(Some("ws".to_string()), "bob");
    args.system_prompt = Some("sp".to_string());
    args.model = Some("claude-opus-4-7".to_string());
    args.env = vec!["KEY=VAL".to_string()];
    args.dotenv_file = Some(dotenv_path);

    let rc = cmd_update_agent::run(&client, args)
        .await
        .expect("update should succeed");
    assert_eq!(rc, 0);

    // Verify each patched field lands as expected.
    let me_path = agent_dir.path().join(".gitim/me.json");
    let me_content = std::fs::read_to_string(&me_path).expect("read me.json");
    let me: serde_json::Value = serde_json::from_str(&me_content).expect("parse me.json");
    assert_eq!(me["system_prompt"], "sp");
    assert_eq!(me["model"], "claude-opus-4-7");
    // env field on me.json mirrors the request map.
    let env = me["env"].as_object().expect("env in me.json");
    assert_eq!(env["KEY"], "VAL");

    // Dotenv lands at <repo>/.env (not under .gitim/).
    let env_file = agent_dir.path().join(".env");
    let env_content = std::fs::read_to_string(&env_file).expect("read .env");
    assert_eq!(env_content, "FOO=bar\nBAZ=qux\n");

    server.abort();
}

// ── Sanity-check the args wrapper ───────────────────────────────────────────

/// Confirms the field set on `cmd_update_agent::Args` round-trips and the
/// types stay compatible. A clap → Args plumbing bug in `bin/runtime.rs`
/// wouldn't be caught by field-level unit tests; this is the boundary.
#[test]
fn args_struct_smoke() {
    let args = cmd_update_agent::Args {
        workspace: Some("ws".to_string()),
        id: "alice".to_string(),
        system_prompt: Some("p".to_string()),
        system_prompt_file: None,
        model: Some("m".to_string()),
        effort: Some("high".to_string()),
        introduction: Some("i".to_string()),
        env: vec!["A=1".to_string()],
        dotenv_file: Some(PathBuf::from("/tmp/.env")),
        clear_session: false,
    };
    assert_eq!(args.id, "alice");
    assert_eq!(args.env.len(), 1);
    let _: Option<PathBuf> = args.dotenv_file;
}
