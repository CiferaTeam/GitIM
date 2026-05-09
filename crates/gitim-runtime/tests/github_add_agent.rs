mod common;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::{ensure_daemon_in_path, short_tempdir};
use gitim_runtime::git_config::{GitConfig, GitProvider, WorkspaceConfig};
use gitim_runtime::github::GithubError;
use gitim_runtime::http::{create_router, GithubApiClient, SharedRuntimeState};
use gitim_runtime::workspace::WorkspaceContext;

struct MockGithubApi {
    verify_result: Mutex<Option<Result<(), GithubError>>>,
    access_result: Mutex<Option<Result<(), GithubError>>>,
    email_result: Mutex<Option<Result<Option<String>, GithubError>>>,
}

impl MockGithubApi {
    fn all_ok() -> Self {
        Self {
            verify_result: Mutex::new(Some(Ok(()))),
            access_result: Mutex::new(Some(Ok(()))),
            email_result: Mutex::new(Some(Ok(Some("octo@example.com".to_string())))),
        }
    }
}

#[async_trait]
impl GithubApiClient for MockGithubApi {
    async fn verify_token(&self, _token: &str) -> Result<(), GithubError> {
        self.verify_result.lock().unwrap().take().unwrap_or(Ok(()))
    }
    async fn check_repo_access(
        &self,
        _owner: &str,
        _repo: &str,
        _token: &str,
    ) -> Result<(), GithubError> {
        self.access_result.lock().unwrap().take().unwrap_or(Ok(()))
    }
    async fn fetch_user_email(&self, _token: &str) -> Result<Option<String>, GithubError> {
        self.email_result.lock().unwrap().take().unwrap_or(Ok(None))
    }
}

async fn spawn_server_with(
    api: Arc<dyn GithubApiClient>,
    clone_override: Option<String>,
) -> (SocketAddr, tokio::task::JoinHandle<()>, SharedRuntimeState) {
    let (router, state) = create_router();
    {
        let mut s = state.lock().unwrap();
        s.github_api = api;
        s.clone_url_override = clone_override;
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (addr, handle, state)
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

/// Inject a WorkspaceContext directly so add_agent has a slug to look up.
/// Does not run real provisioning; the tests seed .gitim-runtime/ separately.
fn inject_workspace(state: &SharedRuntimeState, slug: &str, ws: &Path) {
    let mut s = state.lock().unwrap();
    let ctx = WorkspaceContext::new(slug.to_string(), slug.to_string(), ws.to_path_buf());
    s.workspaces.insert(slug.to_string(), ctx);
}

fn setup_fake_bare(tmp_dir: &Path) -> PathBuf {
    let bare = tmp_dir.join("fake-github.git");
    Command::new("git")
        .args(["init", "--bare", bare.to_str().unwrap()])
        .output()
        .unwrap();

    let seed = tmp_dir.join("seed");
    Command::new("git")
        .args(["clone", bare.to_str().unwrap(), "seed"])
        .current_dir(tmp_dir)
        .output()
        .unwrap();
    for (k, v) in [("user.email", "t@t.com"), ("user.name", "Seed")] {
        Command::new("git")
            .args(["config", k, v])
            .current_dir(&seed)
            .output()
            .unwrap();
    }
    std::fs::write(seed.join("README.md"), "seed\n").unwrap();
    Command::new("git")
        .args(["add", "README.md"])
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
    bare
}

fn kill_human_daemon(workspace: &Path) {
    let pid_file = workspace.join(".gitim-runtime/human/.gitim/run/gitim.pid");
    if let Ok(content) = std::fs::read_to_string(&pid_file) {
        if let Ok(pid) = content.trim().parse::<u32>() {
            let _ = Command::new("kill").arg(pid.to_string()).output();
        }
    }
}

fn kill_agent_daemon(workspace: &Path, handler: &str) {
    let pid_file = workspace.join(handler).join(".gitim/run/gitim.pid");
    if let Ok(content) = std::fs::read_to_string(&pid_file) {
        if let Ok(pid) = content.trim().parse::<u32>() {
            let _ = Command::new("kill").arg(pid.to_string()).output();
        }
    }
}

/// Seed a fake-human clone under `<workspace>/.gitim-runtime/human/` so that
/// handler-conflict detection has a real `users/<handler>.meta.yaml` to find.
/// Mimics what `/git/init` github would leave behind after a successful onboard
/// — without running the real onboard (which requires daemon identity inference).
fn seed_human_clone(workspace: &Path, existing_handlers: &[&str]) {
    let runtime_dir = workspace.join(".gitim-runtime");
    let human_dir = runtime_dir.join("human");
    std::fs::create_dir_all(human_dir.join("users")).unwrap();
    std::fs::create_dir_all(human_dir.join(".git")).unwrap();
    for h in existing_handlers {
        let path = human_dir.join("users").join(format!("{h}.meta.yaml"));
        std::fs::write(&path, format!("handler: {h}\ndisplay_name: {h}\n")).unwrap();
    }
}

/// Seed `archive/users/<h>.meta.yaml` in the human clone — mimics a
/// post-departure state where the daemon has moved a meta file under
/// archive/ but the workspace still exists. add_agent should reject
/// reuse with `error_code: "handler_reserved"`.
fn seed_archived_handler(workspace: &Path, departed_handlers: &[&str]) {
    let archive_dir = workspace.join(".gitim-runtime/human/archive/users");
    std::fs::create_dir_all(&archive_dir).unwrap();
    for h in departed_handlers {
        let path = archive_dir.join(format!("{h}.meta.yaml"));
        std::fs::write(&path, format!("handler: {h}\ndisplay_name: {h}\n")).unwrap();
    }
}

fn write_workspace_config(
    workspace: &Path,
    provider: GitProvider,
    remote_url: Option<String>,
    token: Option<String>,
) {
    let config = WorkspaceConfig {
        workspace: workspace.to_string_lossy().into_owned(),
        created_at: chrono::Utc::now().to_rfc3339(),
        git: GitConfig {
            provider,
            remote_url,
            token,
            github_email: None,
        },
    };
    config.write(workspace).unwrap();
}

#[tokio::test]
async fn add_agent_rejects_existing_handler_in_github_mode() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    let api = Arc::new(MockGithubApi::all_ok());
    let (addr, server, state) = spawn_server_with(api, None).await;
    inject_workspace(&state, "test-ws", &ws);

    // Simulate the post-provision state: github config + a seeded human clone
    // containing an existing agent's user file. The daemon wasn't run, so
    // add_agent has to reject based purely on the file presence check.
    write_workspace_config(
        &ws,
        GitProvider::Github,
        Some("https://github.com/fake/fake".to_string()),
        Some("ghp_TESTSENTINEL_xyz".to_string()),
    );
    seed_human_clone(&ws, &["agent-a"]);

    let resp = post_json(
        addr,
        "/workspaces/test-ws/agents/add",
        serde_json::json!({
            "handler": "agent-a",
            "display_name": "Agent A",
            "provider": "mock"
        }),
    )
    .await;

    assert_eq!(
        resp["ok"], false,
        "should reject duplicate handler: {resp:?}"
    );
    assert_eq!(resp["error_code"], "handler_conflict");
    let raw = serde_json::to_string(&resp).unwrap();
    assert!(
        !raw.contains("ghp_TESTSENTINEL_xyz"),
        "response leaked token: {raw}"
    );
    server.abort();
}

#[tokio::test]
async fn add_agent_rejects_existing_handler_in_local_mode() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    let api = Arc::new(MockGithubApi::all_ok());
    let (addr, server, state) = spawn_server_with(api, None).await;
    inject_workspace(&state, "test-ws", &ws);

    write_workspace_config(&ws, GitProvider::Local, None, None);
    seed_human_clone(&ws, &["taken"]);

    let resp = post_json(
        addr,
        "/workspaces/test-ws/agents/add",
        serde_json::json!({
            "handler": "taken",
            "display_name": "Taken",
            "provider": "mock"
        }),
    )
    .await;

    assert_eq!(resp["ok"], false);
    assert_eq!(resp["error_code"], "handler_conflict");
    server.abort();
}

// A.5: handlers are terminally unique across the depart/restore frontier.
// If `archive/users/<h>.meta.yaml` exists in the human clone, the runtime
// must reject reuse before kicking off provisioning — daemon would reject
// it anyway, but we want the early signal so the WebUI can surface a
// distinct "previously departed" message.
#[tokio::test]
async fn add_agent_rejects_departed_handler_local_mode() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    let api = Arc::new(MockGithubApi::all_ok());
    let (addr, server, state) = spawn_server_with(api, None).await;
    inject_workspace(&state, "test-ws", &ws);

    write_workspace_config(&ws, GitProvider::Local, None, None);
    // Seed an empty active users/ dir alongside the archive entry so the
    // active-conflict check passes and the archive check fires.
    seed_human_clone(&ws, &[]);
    seed_archived_handler(&ws, &["departed-bot"]);

    let resp = post_json(
        addr,
        "/workspaces/test-ws/agents/add",
        serde_json::json!({
            "handler": "departed-bot",
            "display_name": "Departed Bot",
            "provider": "mock"
        }),
    )
    .await;

    assert_eq!(resp["ok"], false, "should reject departed handler: {resp:?}");
    assert_eq!(resp["error_code"], "handler_reserved");
    let err_msg = resp["error"].as_str().unwrap_or_default();
    assert!(
        err_msg.contains("reserved"),
        "error should mention reserved: {err_msg}"
    );
    server.abort();
}

#[tokio::test]
#[ignore]
async fn add_agent_github_mode_clones_with_token_url() {
    // Ignored for the same reason github_init_full_flow_with_mock_api is:
    // provision_agent drives the real daemon through onboard, which relies on
    // identity inference that can't be mocked here. We cover the clone-URL
    // construction logic through the unit-test-equivalent URL-shape tests
    // in github_init.rs and through the handler-conflict path above.
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let bare = setup_fake_bare(tmp.path());

    let api = Arc::new(MockGithubApi::all_ok());
    let clone_override = Some(format!("file://{}", bare.display()));
    let (addr, server, state) = spawn_server_with(api, clone_override).await;

    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    inject_workspace(&state, "test-ws", &ws);

    write_workspace_config(
        &ws,
        GitProvider::Github,
        Some("https://github.com/fake/fake".to_string()),
        Some("ghp_TESTSENTINEL_xyz".to_string()),
    );

    // Seed an empty human clone (no users/) so handler-conflict check passes.
    let human_dir = ws.join(".gitim-runtime/human");
    std::fs::create_dir_all(human_dir.join(".git")).unwrap();
    std::fs::create_dir_all(human_dir.join("users")).unwrap();

    let resp = post_json(
        addr,
        "/workspaces/test-ws/agents/add",
        serde_json::json!({
            "handler": "agent-b",
            "display_name": "Agent B",
            "provider": "mock"
        }),
    )
    .await;

    assert_eq!(resp["ok"], true, "happy path failed: {resp:?}");
    assert!(ws.join("agent-b").exists(), "agent clone directory missing");

    kill_agent_daemon(&ws, "agent-b");
    kill_human_daemon(&ws);
    server.abort();
}

#[tokio::test]
async fn add_agent_github_mode_without_workspace_config_fails_gracefully() {
    // Safety net: if someone calls add_agent without a workspace config,
    // we should fail cleanly, not panic on unwrap().
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    let api = Arc::new(MockGithubApi::all_ok());
    let (addr, server, state) = spawn_server_with(api, None).await;
    inject_workspace(&state, "test-ws", &ws);
    // Intentionally don't call write_workspace_config — no workspace config
    // file present. add_agent should still work (fall back to local mode).

    seed_human_clone(&ws, &["existing"]);

    let resp = post_json(
        addr,
        "/workspaces/test-ws/agents/add",
        serde_json::json!({
            "handler": "existing",
            "display_name": "Existing",
            "provider": "mock"
        }),
    )
    .await;

    assert_eq!(resp["ok"], false);
    assert_eq!(resp["error_code"], "handler_conflict");
    server.abort();
}
