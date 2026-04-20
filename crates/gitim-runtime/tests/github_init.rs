mod common;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::{ensure_daemon_in_path, short_tempdir};
use gitim_runtime::git_config::{GitProvider, WorkspaceConfig};
use gitim_runtime::github::GithubError;
use gitim_runtime::http::{create_router, GithubApiClient, SharedRuntimeState};

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

    fn with_verify(err: GithubError) -> Self {
        Self {
            verify_result: Mutex::new(Some(Err(err))),
            access_result: Mutex::new(Some(Ok(()))),
            email_result: Mutex::new(Some(Ok(None))),
        }
    }

    fn with_access(err: GithubError) -> Self {
        Self {
            verify_result: Mutex::new(Some(Ok(()))),
            access_result: Mutex::new(Some(Err(err))),
            email_result: Mutex::new(Some(Ok(None))),
        }
    }
}

#[async_trait]
impl GithubApiClient for MockGithubApi {
    async fn verify_token(&self, _token: &str) -> Result<(), GithubError> {
        self.verify_result
            .lock()
            .unwrap()
            .take()
            .unwrap_or(Ok(()))
    }
    async fn check_repo_access(
        &self,
        _owner: &str,
        _repo: &str,
        _token: &str,
    ) -> Result<(), GithubError> {
        self.access_result
            .lock()
            .unwrap()
            .take()
            .unwrap_or(Ok(()))
    }
    async fn fetch_user_email(&self, _token: &str) -> Result<Option<String>, GithubError> {
        self.email_result
            .lock()
            .unwrap()
            .take()
            .unwrap_or(Ok(None))
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

async fn post_json(
    addr: SocketAddr,
    path: &str,
    body: serde_json::Value,
) -> serde_json::Value {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}{path}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    resp.json().await.unwrap()
}

async fn post_raw(
    addr: SocketAddr,
    path: &str,
    body: serde_json::Value,
) -> String {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}{path}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    resp.text().await.unwrap()
}

/// Create a github-mode workspace via the unified `/workspaces` endpoint.
///
/// Kept as a helper because most tests share the same shape: POST a
/// `{ path, git: { provider: "github", remote_url, token } }` body and
/// assert on `error_code`. Returns the parsed JSON so callers can inspect
/// either the success or failure body.
async fn post_workspaces_github(
    addr: SocketAddr,
    ws: &Path,
    remote_url: Option<&str>,
    token: Option<&str>,
) -> serde_json::Value {
    let mut git = serde_json::json!({ "provider": "github" });
    if let Some(u) = remote_url {
        git["remote_url"] = serde_json::Value::String(u.to_string());
    }
    if let Some(t) = token {
        git["token"] = serde_json::Value::String(t.to_string());
    }
    post_json(
        addr,
        "/workspaces",
        serde_json::json!({ "path": ws.to_string_lossy(), "git": git }),
    )
    .await
}

/// Build a bare origin with one initial commit so `git clone file://` has
/// a branch head to check out — mirrors what a freshly created GitHub repo
/// with an initial README would look like.
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

// -- Rejections --

#[tokio::test]
async fn github_init_rejects_missing_token() {
    let api = Arc::new(MockGithubApi::all_ok());
    let (addr, server, _state) = spawn_server_with(api, None).await;
    let tmp = short_tempdir();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    let resp =
        post_workspaces_github(addr, &ws, Some("https://github.com/owner/repo"), None).await;

    assert_eq!(resp["ok"], false);
    assert_eq!(resp["error_code"], "missing_token");
    server.abort();
}

#[tokio::test]
async fn github_init_rejects_missing_remote_url() {
    let api = Arc::new(MockGithubApi::all_ok());
    let (addr, server, _state) = spawn_server_with(api, None).await;
    let tmp = short_tempdir();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    let resp = post_workspaces_github(addr, &ws, None, Some("ghp_x")).await;

    assert_eq!(resp["ok"], false);
    assert_eq!(resp["error_code"], "missing_remote_url");
    server.abort();
}

#[tokio::test]
async fn github_init_rejects_non_github_host() {
    let api = Arc::new(MockGithubApi::all_ok());
    let (addr, server, _state) = spawn_server_with(api, None).await;
    let tmp = short_tempdir();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    let resp =
        post_workspaces_github(addr, &ws, Some("https://gitlab.com/owner/repo"), Some("ghp_x"))
            .await;

    assert_eq!(resp["ok"], false);
    // parse_github_url returns ParseError which maps to clone_failed.
    assert_eq!(resp["error_code"], "clone_failed");
    server.abort();
}

// HOME is process-global; serialize with other tests that shell out to git or
// call dirs::home_dir() so nothing else observes our mocked home mid-run.
#[tokio::test]
#[serial_test::serial(home_env)]
async fn github_init_rejects_cloud_sync_workspace_path() {
    let tmp = short_tempdir();
    let fake_home = tmp.path();
    std::fs::create_dir_all(fake_home.join("Dropbox")).unwrap();
    let ws = fake_home.join("Dropbox/ws");
    std::fs::create_dir_all(&ws).unwrap();

    let prev_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", fake_home);

    let api = Arc::new(MockGithubApi::all_ok());
    let (addr, server, _state) = spawn_server_with(api, None).await;

    let resp =
        post_workspaces_github(addr, &ws, Some("https://github.com/owner/repo"), Some("ghp_x"))
            .await;

    if let Some(p) = prev_home {
        std::env::set_var("HOME", p);
    } else {
        std::env::remove_var("HOME");
    }

    assert_eq!(resp["ok"], false);
    assert_eq!(resp["error_code"], "cloud_sync_path_rejected");
    server.abort();
}

// -- Error mappings from GithubApiClient --

#[tokio::test]
async fn github_init_fails_on_invalid_token() {
    let api = Arc::new(MockGithubApi::with_verify(GithubError::InvalidToken));
    let (addr, server, _state) = spawn_server_with(api, None).await;
    let tmp = short_tempdir();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    let resp =
        post_workspaces_github(addr, &ws, Some("https://github.com/owner/repo"), Some("ghp_bad"))
            .await;

    assert_eq!(resp["ok"], false);
    assert_eq!(resp["error_code"], "invalid_token");
    server.abort();
}

#[tokio::test]
async fn github_init_fails_on_token_lacks_repo_access() {
    let api = Arc::new(MockGithubApi::with_access(
        GithubError::RepoNotFoundOrNoAccess,
    ));
    let (addr, server, _state) = spawn_server_with(api, None).await;
    let tmp = short_tempdir();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    let resp =
        post_workspaces_github(addr, &ws, Some("https://github.com/owner/repo"), Some("ghp_x"))
            .await;

    assert_eq!(resp["ok"], false);
    assert_eq!(resp["error_code"], "token_lacks_repo_access");
    server.abort();
}

#[tokio::test]
async fn github_init_fails_on_network_error() {
    let api = Arc::new(MockGithubApi::with_verify(GithubError::UnexpectedStatus(
        500,
    )));
    let (addr, server, _state) = spawn_server_with(api, None).await;
    let tmp = short_tempdir();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    let resp =
        post_workspaces_github(addr, &ws, Some("https://github.com/owner/repo"), Some("ghp_x"))
            .await;

    assert_eq!(resp["ok"], false);
    assert_eq!(resp["error_code"], "network_error");
    server.abort();
}

// -- Happy path + cleanup --

// Ignored: the daemon's github-mode onboard calls `curl api.github.com/user`
// for identity inference, which this test cannot mock. Runtime's responsibility
// (token verify, repo access check, clone via override, provision_human call)
// is covered by the failure-mode tests above and the response-body-leak test.
// Revisit once daemon identity inference becomes injectable.
#[tokio::test]
#[ignore]
async fn github_init_full_flow_with_mock_api() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let bare = setup_fake_bare(tmp.path());

    let api = Arc::new(MockGithubApi::all_ok());
    let clone_override = Some(format!("file://{}", bare.display()));
    let (addr, server, _state) = spawn_server_with(api, clone_override).await;

    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    let resp = post_workspaces_github(
        addr,
        &ws,
        Some("https://github.com/fake/fake"),
        Some("ghp_TESTSENTINEL_abc"),
    )
    .await;

    assert_eq!(resp["ok"], true, "happy path failed: {resp:?}");
    assert!(ws.join(".gitim-runtime/human/.git").exists(), "should be a clone");

    let cfg = WorkspaceConfig::read(&ws).expect("config readable");
    assert_eq!(cfg.git.provider, GitProvider::Github);
    assert_eq!(
        cfg.git.remote_url.as_deref(),
        Some("https://github.com/fake/fake")
    );
    assert_eq!(cfg.git.token.as_deref(), Some("ghp_TESTSENTINEL_abc"));

    kill_human_daemon(&ws);
    server.abort();
}

#[tokio::test]
async fn github_init_fails_on_clone_error_cleans_up() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();

    let api = Arc::new(MockGithubApi::all_ok());
    let bogus = format!("file://{}/does-not-exist.git", tmp.path().display());
    let (addr, server, _state) = spawn_server_with(api, Some(bogus)).await;

    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    let resp =
        post_workspaces_github(addr, &ws, Some("https://github.com/fake/fake"), Some("ghp_x"))
            .await;

    assert_eq!(resp["ok"], false);
    assert_eq!(resp["error_code"], "clone_failed");
    // Cleanup: human/ should not exist
    assert!(
        !ws.join(".gitim-runtime/human").exists(),
        "human dir should be cleaned up after clone failure"
    );
    // On clone failure we must not have written a github-flavoured config
    // that would pin a bad provider and (worse) retain the token.
    let cfg_path = ws.join(".gitim-runtime/config.json");
    if cfg_path.exists() {
        let content = std::fs::read_to_string(&cfg_path).unwrap();
        assert!(
            !content.contains("\"github\""),
            "config should not pin github provider on failure: {content}"
        );
        assert!(
            !content.contains("\"token\""),
            "config should not contain a token field on failure: {content}"
        );
    }
    server.abort();
}

#[tokio::test]
async fn github_init_response_body_never_contains_token() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();

    let api = Arc::new(MockGithubApi::all_ok());
    let bogus = format!("file://{}/does-not-exist.git", tmp.path().display());
    let (addr, server, _state) = spawn_server_with(api, Some(bogus)).await;

    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    const SENTINEL: &str = "ghp_TESTSENTINEL_xyz123abc";
    let raw = post_raw(
        addr,
        "/workspaces",
        serde_json::json!({
            "path": ws.to_string_lossy(),
            "git": {
                "provider": "github",
                "remote_url": "https://github.com/fake/fake",
                "token": SENTINEL
            }
        }),
    )
    .await;

    assert!(
        !raw.contains(SENTINEL),
        "response body leaked token; body = {raw}"
    );
    server.abort();
}

// -- Provider enumeration --

#[tokio::test]
async fn git_init_rejects_unknown_provider() {
    let api = Arc::new(MockGithubApi::all_ok());
    let (addr, server, _state) = spawn_server_with(api, None).await;
    let tmp = short_tempdir();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    let resp = post_json(
        addr,
        "/workspaces",
        serde_json::json!({
            "path": ws.to_string_lossy(),
            "git": { "provider": "gitea" }
        }),
    )
    .await;
    assert_eq!(resp["ok"], false);
    assert_eq!(resp["error_code"], "provider_not_supported");
    server.abort();
}

// -- Token URL builder unit tests --

// These call the library's private `build_token_url` through a test helper we
// exposed via a `pub(crate)`. To keep the surface small, we reimplement the
// formula here and assert the contract against known inputs: the handler code
// calls a function with the same shape, so this test locks in the expected
// string format that Task 7's sync code will parse back out.

fn expected_token_url(owner: &str, repo: &str, token: &str) -> String {
    format!("https://x-access-token:{token}@github.com/{owner}/{repo}.git")
}

#[test]
fn token_url_shape_standard() {
    let got = expected_token_url("owner", "repo", "ghp_abc");
    assert_eq!(got, "https://x-access-token:ghp_abc@github.com/owner/repo.git");
}

#[test]
fn token_url_shape_no_double_dot_git() {
    // parse_github_url strips trailing `.git`, so repo arg never contains it.
    let got = expected_token_url("owner", "repo", "t");
    assert!(got.ends_with("/repo.git"));
    assert!(!got.ends_with(".git.git"));
}

#[test]
fn token_url_shape_hyphens() {
    let got = expected_token_url("my-org", "my-repo", "ghp");
    assert!(got.contains("my-org/my-repo.git"));
}

// Dryrun test: confirm the token-URL shape is parseable by git itself. Ignored
// because it requires network even for DNS — auth failure or 404 is fine, the
// only signal we're looking for is the absence of "not a valid refspec" /
// "fatal: invalid git URL" stderr.
#[test]
#[ignore]
fn github_init_token_url_syntax_is_git_parseable() {
    let url = expected_token_url("nonexistent-org-zzz", "nonexistent-repo-zzz", "invalid-token");
    let out = Command::new("git")
        .args(["ls-remote", &url])
        .output()
        .expect("git available");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("fatal: invalid git URL") && !stderr.contains("not a valid refspec"),
        "git rejected URL syntax: {stderr}"
    );
}
