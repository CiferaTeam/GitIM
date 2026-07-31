#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::{ensure_daemon_in_path, setup_bare_remote, short_tempdir, HomeGuard};
use gitim_runtime::git_config::{GitProvider, WorkspaceConfig};
use gitim_runtime::github::GithubError;
use gitim_runtime::http::{create_router, GithubApiClient};

const TOKEN: &str = "ghp_PRIVATE_BOOTSTRAP_SENTINEL";
const PUBLIC_URL: &str = "https://github.com/private-owner/private-repo";
const AUTH_URL: &str =
    "https://x-access-token:ghp_PRIVATE_BOOTSTRAP_SENTINEL@github.com/private-owner/private-repo.git";

struct MockGithubApi {
    email_result: Mutex<Option<String>>,
}

#[async_trait]
impl GithubApiClient for MockGithubApi {
    async fn verify_token(&self, _token: &str) -> Result<(), GithubError> {
        Ok(())
    }

    async fn check_repo_access(
        &self,
        _owner: &str,
        _repo: &str,
        _token: &str,
    ) -> Result<(), GithubError> {
        Ok(())
    }

    async fn fetch_user_email(&self, _token: &str) -> Result<Option<String>, GithubError> {
        Ok(self.email_result.lock().unwrap().take())
    }
}

struct EnvironmentGuard {
    path: Option<std::ffi::OsString>,
    api_base: Option<std::ffi::OsString>,
}

impl EnvironmentGuard {
    fn install(shim_dir: &Path, api_base: &str) -> Self {
        let path = std::env::var_os("PATH");
        let api_base_before = std::env::var_os("GITIM_GITHUB_API_BASE");
        let existing = path
            .as_deref()
            .map(|value| value.to_string_lossy())
            .unwrap_or_default();
        std::env::set_var("PATH", format!("{}:{existing}", shim_dir.display()));
        std::env::set_var("GITIM_GITHUB_API_BASE", api_base);
        Self {
            path,
            api_base: api_base_before,
        }
    }
}

impl Drop for EnvironmentGuard {
    fn drop(&mut self) {
        match self.path.take() {
            Some(path) => std::env::set_var("PATH", path),
            None => std::env::remove_var("PATH"),
        }
        match self.api_base.take() {
            Some(base) => std::env::set_var("GITIM_GITHUB_API_BASE", base),
            None => std::env::remove_var("GITIM_GITHUB_API_BASE"),
        }
    }
}

fn command_path(command: &str) -> PathBuf {
    let output = Command::new("which").arg(command).output().unwrap();
    assert!(output.status.success());
    PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
}

fn install_private_remote_git_shim(
    root: &Path,
    bare: &Path,
    allowed_origin: &str,
) -> (PathBuf, PathBuf) {
    let shim_dir = root.join("shims");
    std::fs::create_dir_all(&shim_dir).unwrap();
    let log_path = root.join("private-remote.log");
    let real_git = command_path("git");
    let bare_url = format!("file://{}", bare.display());
    let script = format!(
        r#"#!/bin/sh
set -eu
operation=""
skip_config_value=0
for argument in "$@"; do
  if [ "$skip_config_value" -eq 1 ]; then
    skip_config_value=0
    continue
  fi
  if [ "$argument" = "-c" ]; then
    skip_config_value=1
    continue
  fi
  case "$argument" in
    clone|fetch|push)
      operation="$argument"
      break
      ;;
  esac
done

if [ "$operation" = "fetch" ] || [ "$operation" = "push" ]; then
  origin="$("{real_git}" config --get remote.origin.url 2>/dev/null || true)"
  printf '%s %s\n' "$operation" "$origin" >> "{log_path}"
  if [ "$origin" != "{allowed_origin}" ]; then
    echo "private remote authentication required" >&2
    exit 73
  fi
fi

exec "{real_git}" \
  -c protocol.file.allow=always \
  -c "url.{bare_url}.insteadOf={auth_url}" \
  "$@"
"#,
        real_git = real_git.display(),
        log_path = log_path.display(),
        auth_url = AUTH_URL,
        allowed_origin = allowed_origin,
    );
    let shim = shim_dir.join("git");
    std::fs::write(&shim, script).unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    (shim_dir, log_path)
}

fn serve_github_user() -> String {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request);
        let body = r#"{"id":42,"login":"private-owner","name":"Private Owner","email":"private-owner@example.com"}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
    });
    format!("http://{addr}")
}

async fn spawn_server() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let (router, state) = create_router();
    {
        let mut runtime = state.lock().unwrap();
        runtime.github_api = Arc::new(MockGithubApi {
            email_result: Mutex::new(Some("private-owner@example.com".to_owned())),
        });
        assert!(
            runtime.clone_url_override.is_none(),
            "private remote coverage must use the production clone URL path"
        );
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (addr, handle)
}

fn kill_human_daemon(workspace: &Path) {
    let pid_file = workspace.join(".gitim-runtime/human/.gitim/run/gitim.pid");
    if let Ok(content) = std::fs::read_to_string(pid_file) {
        if let Ok(pid) = content.trim().parse::<u32>() {
            let _ = Command::new("kill").arg(pid.to_string()).output();
        }
    }
}

#[tokio::test]
#[serial_test::serial(home_env)]
async fn github_private_remote_authenticates_skill_bootstrap_from_workspace_config() {
    let _home = HomeGuard::install();
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let bare = setup_bare_remote(&tmp);
    let (shim_dir, transport_log) = install_private_remote_git_shim(tmp.path(), &bare, AUTH_URL);
    let api_base = serve_github_user();
    let _environment = EnvironmentGuard::install(&shim_dir, &api_base);
    let (addr, server) = spawn_server().await;
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let response = reqwest::Client::new()
        .post(format!("http://{addr}/workspaces"))
        .json(&serde_json::json!({
            "path": workspace,
            "git": {
                "provider": "github",
                "remote_url": PUBLIC_URL,
                "token": TOKEN,
            }
        }))
        .send()
        .await
        .unwrap();
    let body = response.text().await.unwrap();

    assert!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap()["ok"] == true,
        "private remote provisioning failed: {body}"
    );
    assert!(
        !body.contains(TOKEN),
        "HTTP response leaked the PAT: {body}"
    );

    let config = WorkspaceConfig::read(&workspace).unwrap();
    assert_eq!(config.git.provider, GitProvider::Github);
    assert_eq!(config.git.remote_url.as_deref(), Some(PUBLIC_URL));
    assert_eq!(config.git.token.as_deref(), Some(TOKEN));
    let config_path = workspace.join(".gitim-runtime/config.json");
    assert_eq!(
        std::fs::metadata(&config_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(!workspace.join(".gitim-runtime/config.json.tmp").exists());

    let human = workspace.join(".gitim-runtime/human");
    let origin = Command::new("git")
        .args(["config", "--get", "remote.origin.url"])
        .current_dir(&human)
        .output()
        .unwrap();
    assert!(origin.status.success());
    assert_eq!(String::from_utf8(origin.stdout).unwrap().trim(), AUTH_URL);

    let remote_operations = std::fs::read_to_string(transport_log).unwrap();
    assert!(
        remote_operations
            .lines()
            .any(|line| line == format!("fetch {AUTH_URL}")),
        "Skill bootstrap never fetched with authenticated origin:\n{remote_operations}"
    );
    assert!(
        remote_operations
            .lines()
            .any(|line| line == format!("push {AUTH_URL}")),
        "Skill bootstrap never pushed with authenticated origin:\n{remote_operations}"
    );
    assert!(
        !remote_operations.contains(PUBLIC_URL),
        "private remote observed an unauthenticated origin:\n{remote_operations}"
    );

    kill_human_daemon(&workspace);
    server.abort();
}

#[tokio::test]
#[serial_test::serial(home_env)]
async fn github_private_remote_auth_failure_rolls_back_provisional_secret_config() {
    let _home = HomeGuard::install();
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let bare = setup_bare_remote(&tmp);
    let denied_origin = AUTH_URL.replace(TOKEN, "ghp_DIFFERENT_TOKEN");
    let (shim_dir, _transport_log) =
        install_private_remote_git_shim(tmp.path(), &bare, &denied_origin);
    let api_base = serve_github_user();
    let _environment = EnvironmentGuard::install(&shim_dir, &api_base);
    let (addr, server) = spawn_server().await;
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let response = reqwest::Client::new()
        .post(format!("http://{addr}/workspaces"))
        .json(&serde_json::json!({
            "path": workspace,
            "git": {
                "provider": "github",
                "remote_url": PUBLIC_URL,
                "token": TOKEN,
            }
        }))
        .send()
        .await
        .unwrap();
    let body = response.text().await.unwrap();
    let parsed = serde_json::from_str::<serde_json::Value>(&body).unwrap();

    assert_eq!(parsed["ok"], false);
    assert_eq!(parsed["error_code"], "onboard_failed");
    assert!(
        !body.contains(TOKEN),
        "HTTP response leaked the PAT: {body}"
    );
    assert!(
        !workspace.join(".gitim-runtime/config.json").exists(),
        "failed provisioning retained the provisional token source"
    );
    assert!(
        !workspace.join(".gitim-runtime/human").exists(),
        "failed provisioning retained the partial clone"
    );

    server.abort();
}
