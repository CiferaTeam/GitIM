//! Integration tests for the provisioning preflight gate wired into
//! `POST /workspaces/{slug}/agents/add`.
//!
//! These tests exercise the boundary between request validation (handler
//! conflicts, archive checks) and the actual provision_agent call. They
//! verify that the gate:
//!   1. Short-circuits success for the mock provider (no spawn).
//!   2. Blocks provisioning with a structured ErrorBody + preflight_detail
//!      when the provider binary is missing or fails.
//!   3. Threads through the hermes resolution logic (explicit dual-LLM and
//!      default-profile resolution) end-to-end via HTTP, including the
//!      `hermes_default_profile_no_llm` tagged failure code.
//!
//! The outer-timeout flavor (provider preflight exceeds the 90s cap) is
//! exercised directly via `preflight_for_add_request_with_overrides` in
//! `tests/preflight_for_add_request.rs::outer_timeout_fires_with_slow_binary`.
//! `agents_add` calls the production `preflight_for_add_request` which uses
//! the default cap, so reproducing it through HTTP would require a 90s
//! sleep — covered by the lower-level test instead.
//!
//! The post-preflight failure path (preflight passes, then hermes profile
//! creation or apply_model_config fails) is unchanged from before this
//! task. Existing coverage in github_add_agent.rs (handler_conflict /
//! hermes_not_setup / hermes_profile_create_failed paths) continues to
//! pass; adding a dedicated test here would duplicate that surface.

mod common;

use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{ensure_daemon_in_path, short_tempdir, stop_daemon};
use gitim_runtime::git_config::{GitConfig, GitProvider, WorkspaceConfig};
use gitim_runtime::http::{create_router, SharedRuntimeState};
use gitim_runtime::workspace::WorkspaceContext;
use serial_test::serial;
use tempfile::TempDir;

// ── Shared helpers ──────────────────────────────────────────────────────────

async fn spawn_server() -> (SocketAddr, SharedRuntimeState, tokio::task::JoinHandle<()>) {
    let (router, state) = create_router();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (addr, state, handle)
}

async fn post_json(addr: SocketAddr, path: &str, body: serde_json::Value) -> serde_json::Value {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}{path}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    resp.json().await.unwrap()
}

fn inject_workspace(state: &SharedRuntimeState, slug: &str, path: &Path) {
    let ctx = WorkspaceContext::new(slug.to_string(), slug.to_string(), path.to_path_buf());
    state
        .lock()
        .unwrap()
        .workspaces
        .insert(slug.to_string(), ctx);
}

fn write_local_workspace_config(workspace: &Path) {
    let config = WorkspaceConfig {
        workspace: workspace.to_string_lossy().into_owned(),
        created_at: chrono::Utc::now().to_rfc3339(),
        git: GitConfig {
            provider: GitProvider::Local,
            remote_url: None,
            token: None,
            github_email: None,
        },
    };
    config.write(workspace).unwrap();
}

/// Initialise a bare repo at `workspace/repo.git` with one seed commit so
/// the runtime's local-mode clone (`workspace.join("repo.git")`) succeeds.
/// Mirrors what `/git/init` local-mode would have produced.
fn init_local_repo_git(workspace: &Path) {
    let repo_git = workspace.join("repo.git");
    Command::new("git")
        .args(["init", "--bare", repo_git.to_str().unwrap()])
        .output()
        .unwrap();

    let seed = workspace.join("__seed__");
    Command::new("git")
        .args(["clone", repo_git.to_str().unwrap(), "__seed__"])
        .current_dir(workspace)
        .output()
        .unwrap();
    for (k, v) in [("user.email", "t@t.com"), ("user.name", "Seed")] {
        Command::new("git")
            .args(["config", k, v])
            .current_dir(&seed)
            .output()
            .unwrap();
    }
    std::fs::write(seed.join(".gitkeep"), "").unwrap();
    Command::new("git")
        .args(["add", ".gitkeep"])
        .current_dir(&seed)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "initial"])
        .current_dir(&seed)
        .output()
        .unwrap();
    Command::new("git")
        .args(["push"])
        .current_dir(&seed)
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&seed);
}

/// RAII guard that swaps `PATH` to a caller-supplied directory (which
/// must contain whatever fake binaries the test needs) and restores the
/// prior value on drop. Pairs with `#[serial(path_env)]` so only one
/// `PathGuard` is live at a time.
///
/// We prepend rather than replace so the real `git` / `gitim-daemon`
/// binaries (added by `ensure_daemon_in_path`) remain resolvable when the
/// gate passes and the downstream provision_agent path runs.
struct PathGuard {
    original: Option<std::ffi::OsString>,
}

impl PathGuard {
    fn install_prepend(dir: &Path) -> Self {
        let original = std::env::var_os("PATH");
        let new_path = match &original {
            Some(prev) => {
                let mut s = std::ffi::OsString::new();
                s.push(dir);
                s.push(":");
                s.push(prev);
                s
            }
            None => dir.as_os_str().to_os_string(),
        };
        std::env::set_var("PATH", new_path);
        Self { original }
    }
}

impl Drop for PathGuard {
    fn drop(&mut self) {
        match self.original.take() {
            Some(val) => std::env::set_var("PATH", val),
            None => std::env::remove_var("PATH"),
        }
    }
}

/// RAII guard for `HERMES_HOME` env. Restores the prior value on drop.
struct HermesHomeGuard {
    original: Option<std::ffi::OsString>,
}

impl HermesHomeGuard {
    fn install(path: &Path) -> Self {
        let original = std::env::var_os("HERMES_HOME");
        std::env::set_var("HERMES_HOME", path);
        Self { original }
    }
}

impl Drop for HermesHomeGuard {
    fn drop(&mut self) {
        match self.original.take() {
            Some(val) => std::env::set_var("HERMES_HOME", val),
            None => std::env::remove_var("HERMES_HOME"),
        }
    }
}

/// Drop a shell script with the given filename in `dir` and chmod +x it.
fn write_executable(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn state_has_agent(state: &SharedRuntimeState, slug: &str, handler: &str) -> bool {
    state
        .lock()
        .unwrap()
        .workspaces
        .get(slug)
        .map(|ctx| ctx.agents.contains_key(handler))
        .unwrap_or(false)
}

// ── Test 1: mock provider — preflight short-circuits and provision succeeds ─

/// Mock provider's preflight branch is the only zero-spawn success path.
/// Verifies the gate doesn't block and the request flows through to the
/// existing provision_agent + state.insert + spawn_agent_loop chain.
///
/// Serialised against the env-mutating tests below because provision_agent
/// shells out to `git` (PATH-resolved), and a concurrent test that has
/// replaced PATH would break the clone.
#[tokio::test]
#[serial(provision_preflight_env)]
async fn test_mock_provider_short_circuits_to_success() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    write_local_workspace_config(&ws);
    init_local_repo_git(&ws);

    let (addr, state, server) = spawn_server().await;
    inject_workspace(&state, "ws", &ws);

    let resp = post_json(
        addr,
        "/workspaces/ws/agents/add",
        serde_json::json!({
            "handler": "mockbot",
            "display_name": "Mock Bot",
            "provider": "mock",
        }),
    )
    .await;

    assert_eq!(
        resp["ok"], true,
        "mock provider should pass preflight + provision, got: {resp}"
    );
    assert_eq!(resp["id"], "mockbot");
    assert!(
        state_has_agent(&state, "ws", "mockbot"),
        "state should have the new agent after provision"
    );

    // Cleanup: stop the daemon spawned by provision_agent. Best-effort.
    let agent_dir = ws.join("mockbot");
    stop_daemon(&agent_dir).await;
    server.abort();
}

// ── Test 2: claude with failing binary — preflight blocks provision ─────────

/// Fake claude binary that exits 1. Preflight should return `available: false`,
/// the gate aborts before provision_agent, and the response carries
/// `error_code: provision_preflight_failed` + a populated `preflight_detail`.
///
/// Serial lock name `provision_preflight_env` is shared by every test in this
/// file that mutates `PATH` or `HERMES_HOME`. Distinct lock names would allow
/// these tests to race on process-global env between them.
#[tokio::test]
#[serial(provision_preflight_env)]
async fn test_claude_with_failing_binary_returns_preflight_failed() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    write_local_workspace_config(&ws);
    // No init_local_repo_git — preflight must abort first; if the gate
    // leaked, provision_agent would fail with a clone error and we'd see
    // a different error_code.

    let bin_dir = TempDir::new().unwrap();
    write_executable(
        bin_dir.path(),
        "claude",
        "#!/bin/sh\necho 'fake claude failure' 1>&2\nexit 1\n",
    );
    let _path_guard = PathGuard::install_prepend(bin_dir.path());

    let (addr, state, server) = spawn_server().await;
    inject_workspace(&state, "ws", &ws);

    let resp = post_json(
        addr,
        "/workspaces/ws/agents/add",
        serde_json::json!({
            "handler": "broken",
            "display_name": "Broken Bot",
            "provider": "claude",
            "model": "any-model",
        }),
    )
    .await;

    assert_eq!(
        resp["ok"], false,
        "preflight failure should set ok=false: {resp}"
    );
    assert_eq!(
        resp["error_code"], "provision_preflight_failed",
        "expected provision_preflight_failed error_code, got: {resp}"
    );
    let detail = resp
        .get("preflight_detail")
        .expect("preflight_detail must be present");
    assert!(
        detail.is_object(),
        "preflight_detail must be an object: {detail}"
    );
    assert_eq!(detail["provider"], "claude");
    assert_eq!(detail["available"], false);
    // Provider was invoked and failed → error_kind populated.
    assert!(
        detail.get("error_kind").is_some(),
        "preflight_detail.error_kind missing: {detail}"
    );

    // Gate worked: no agent in state, no agent dir on disk.
    assert!(
        !state_has_agent(&state, "ws", "broken"),
        "preflight failure must not insert agent into state"
    );
    assert!(
        !ws.join("broken").exists(),
        "preflight failure must not leave an agent dir behind"
    );

    server.abort();
}

// ── Test 3: hermes with explicit (llm_provider, llm_model) ──────────────────

/// Body specifies both `llm_provider` and `llm_model`, so the dispatcher
/// should fire the chat-mode hermes preflight directly (no default-profile
/// YAML lookup). Verified by a fake hermes binary that echoes GITIM_OK on
/// successful invocation.
#[tokio::test]
#[serial(provision_preflight_env)]
async fn test_hermes_dual_llm_specified_dispatches_chat_mode() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    write_local_workspace_config(&ws);
    // No init_local_repo_git — we expect preflight to pass and provisioning
    // to fail downstream (hermes profile cleanup). That's fine: we only
    // assert what preflight did, which is to NOT short-circuit with
    // `provision_preflight_failed`.

    let bin_dir = TempDir::new().unwrap();
    // Hermes preflight expects "GITIM_OK" in stdout for the chat-mode success
    // branch. argv inspection: must include --provider <id> + --model <m>.
    let argv_capture = bin_dir.path().join("hermes_argv.txt");
    write_executable(
        bin_dir.path(),
        "hermes",
        &format!(
            "#!/bin/sh\necho \"ARGV=$*\" >> \"{capture}\"\necho 'GITIM_OK'\nexit 0\n",
            capture = argv_capture.display()
        ),
    );
    let _path_guard = PathGuard::install_prepend(bin_dir.path());

    let (addr, state, server) = spawn_server().await;
    inject_workspace(&state, "ws", &ws);

    let resp = post_json(
        addr,
        "/workspaces/ws/agents/add",
        serde_json::json!({
            "handler": "alice",
            "display_name": "Alice",
            "provider": "hermes",
            "llm_provider": "anthropic",
            "llm_model": "claude-opus-4-7",
        }),
    )
    .await;

    // Preflight should have passed (gate didn't block). Whatever downstream
    // failure path fires (hermes_not_setup / clone / etc.) is not
    // `provision_preflight_failed`.
    let err_code = resp["error_code"].as_str().unwrap_or("");
    assert_ne!(
        err_code, "provision_preflight_failed",
        "preflight should not have blocked dual-LLM hermes request: {resp}"
    );

    // The fake hermes binary must have been invoked with the explicit pair.
    let captured = std::fs::read_to_string(&argv_capture)
        .unwrap_or_else(|_| panic!("hermes argv capture file missing — preflight didn't spawn"));
    assert!(
        captured.contains("--provider"),
        "expected --provider in hermes argv, got: {captured}"
    );
    assert!(
        captured.contains("anthropic"),
        "expected anthropic in hermes argv, got: {captured}"
    );
    assert!(
        captured.contains("--model"),
        "expected --model in hermes argv, got: {captured}"
    );
    assert!(
        captured.contains("claude-opus-4-7"),
        "expected claude-opus-4-7 in hermes argv, got: {captured}"
    );

    server.abort();
}

// ── Test 4: hermes with no LLM but default profile has one ──────────────────

/// llm_provider/llm_model both omitted, but `HERMES_HOME/config.yaml`
/// supplies a default model — preflight should resolve from the YAML and
/// dispatch chat-mode with that pair. The fake hermes binary captures argv
/// so we can verify the resolved pair was used.
#[tokio::test]
#[serial(provision_preflight_env)]
async fn test_hermes_no_llm_with_default_profile_having_llm() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    write_local_workspace_config(&ws);

    let hermes_home_tmp = TempDir::new().unwrap();
    let hermes_home = hermes_home_tmp.path();
    std::fs::write(
        hermes_home.join("config.yaml"),
        "model:\n  default: claude-haiku-4-5\n  provider: anthropic\n",
    )
    .unwrap();

    let bin_dir = TempDir::new().unwrap();
    let argv_capture = bin_dir.path().join("hermes_argv.txt");
    write_executable(
        bin_dir.path(),
        "hermes",
        &format!(
            "#!/bin/sh\necho \"ARGV=$*\" >> \"{capture}\"\necho 'GITIM_OK'\nexit 0\n",
            capture = argv_capture.display()
        ),
    );
    let _path_guard = PathGuard::install_prepend(bin_dir.path());
    let _hermes_guard = HermesHomeGuard::install(hermes_home);

    let (addr, state, server) = spawn_server().await;
    inject_workspace(&state, "ws", &ws);

    let resp = post_json(
        addr,
        "/workspaces/ws/agents/add",
        serde_json::json!({
            "handler": "carol",
            "display_name": "Carol",
            "provider": "hermes",
            // llm_provider + llm_model intentionally omitted → triggers
            // default-profile resolution path.
        }),
    )
    .await;

    // Preflight should have passed via default-profile resolution.
    let err_code = resp["error_code"].as_str().unwrap_or("");
    assert_ne!(
        err_code, "provision_preflight_failed",
        "default-profile-resolve should have succeeded: {resp}"
    );
    assert_ne!(
        err_code, "hermes_default_profile_no_llm",
        "config.yaml had a model — should not fall to no_llm: {resp}"
    );

    let captured = std::fs::read_to_string(&argv_capture)
        .unwrap_or_else(|_| panic!("hermes argv capture missing — no spawn occurred"));
    assert!(
        captured.contains("anthropic"),
        "expected resolved provider 'anthropic' in argv: {captured}"
    );
    assert!(
        captured.contains("claude-haiku-4-5"),
        "expected resolved model 'claude-haiku-4-5' in argv: {captured}"
    );

    server.abort();
}

// ── Test 5: hermes with no LLM and default profile lacks one ────────────────

/// Both llm params omitted AND `HERMES_HOME/config.yaml` doesn't define
/// model.default/model.provider — preflight returns failure tagged with
/// `hermes_default_profile_no_llm`. No spawn, no agent dir.
#[tokio::test]
#[serial(provision_preflight_env)]
async fn test_hermes_no_llm_default_profile_missing_llm_returns_default_profile_no_llm() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    write_local_workspace_config(&ws);

    let hermes_home_tmp = TempDir::new().unwrap();
    let hermes_home = hermes_home_tmp.path();
    // config.yaml exists but has no model.* keys → preflight should tag
    // the failure as hermes_default_profile_no_llm.
    std::fs::write(
        hermes_home.join("config.yaml"),
        "auth:\n  provider: anthropic\n",
    )
    .unwrap();
    let _hermes_guard = HermesHomeGuard::install(hermes_home);

    let (addr, state, server) = spawn_server().await;
    inject_workspace(&state, "ws", &ws);

    let resp = post_json(
        addr,
        "/workspaces/ws/agents/add",
        serde_json::json!({
            "handler": "dora",
            "display_name": "Dora",
            "provider": "hermes",
        }),
    )
    .await;

    assert_eq!(resp["ok"], false);
    assert_eq!(
        resp["error_code"], "hermes_default_profile_no_llm",
        "expected hermes_default_profile_no_llm error_code, got: {resp}"
    );

    let detail = resp
        .get("preflight_detail")
        .expect("preflight_detail must be present for hermes no-llm failure");
    assert_eq!(detail["provider"], "hermes");
    assert_eq!(detail["available"], false);
    assert_eq!(
        detail["failure_code"], "hermes_default_profile_no_llm",
        "preflight_detail.failure_code must match top-level error_code: {detail}"
    );

    // Gate worked: no agent in state, no agent dir on disk.
    assert!(!state_has_agent(&state, "ws", "dora"));
    assert!(!ws.join("dora").exists());

    server.abort();
}

// ── Outer-timeout flavor ────────────────────────────────────────────────────
//
// `agents_add` calls the production `preflight_for_add_request` which uses
// the default `PROVIDER_PREFLIGHT_TIMEOUT` (90s). Reproducing the timeout
// path here would require a 90s sleep. The behavior is covered directly in
// `tests/preflight_for_add_request.rs::outer_timeout_fires_with_slow_binary`
// against the `_with_overrides` variant — agents_add inherits the same
// dispatcher, so the wire shape (error_kind: timeout, no failure_code,
// classified as provision_preflight_failed) is the same here.

// ── Post-preflight failure paths (unchanged behavior) ───────────────────────
//
// Preflight passing + downstream failure (hermes profile clone, apply
// model config, daemon spawn timeout, etc.) is unchanged from before this
// task. The existing test suite already exercises these paths:
//   - `github_add_agent::add_agent_rejects_existing_handler_in_*` covers
//     handler_conflict short-circuiting (runs BEFORE the gate).
//   - `cli_add_agent::test_add_agent_hermes_with_llm_flags_reaches_runtime`
//     covers a hermes request that reaches the runtime and gets a
//     structured error code back.
// Adding a dedicated test here would duplicate that coverage without
// adding a new boundary the gate introduces.
