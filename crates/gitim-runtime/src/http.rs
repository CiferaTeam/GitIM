use async_trait::async_trait;
use axum::{extract::State, routing::{get, post}, Json, Router};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures::stream::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use tokio::task::AbortHandle;
use tower_http::cors::CorsLayer;

use crate::agent::{detect_git_config, name_to_handler, provision_agent, provision_human, AgentConfig};
use crate::agent_loop::AgentLoop;
use crate::git_config::{
    mark_excluded_from_backups, validate_workspace_path_from_env, GitConfig, GitProvider,
    WorkspaceConfig, WorkspacePathError,
};
use crate::github::{check_repo_access, parse_github_url, verify_token, GithubError};
use crate::url_redact::redacted_url;
use gitim_client::GitimClient;

/// Seam for tests: production hits github.com, tests hit a mockito server.
/// Kept inside the runtime crate so the call sites in `git_init` don't care
/// which backing impl is wired up — they just ask the injected client.
#[async_trait]
pub trait GithubApiClient: Send + Sync {
    async fn verify_token(&self, token: &str) -> Result<(), GithubError>;
    async fn check_repo_access(
        &self,
        owner: &str,
        repo: &str,
        token: &str,
    ) -> Result<(), GithubError>;
}

pub struct DefaultGithubApi {
    pub base_url: String,
}

#[async_trait]
impl GithubApiClient for DefaultGithubApi {
    async fn verify_token(&self, token: &str) -> Result<(), GithubError> {
        verify_token(token, &self.base_url).await
    }
    async fn check_repo_access(
        &self,
        owner: &str,
        repo: &str,
        token: &str,
    ) -> Result<(), GithubError> {
        check_repo_access(owner, repo, token, &self.base_url).await
    }
}

#[derive(Serialize)]
struct HealthResponse {
    service: &'static str,
    version: &'static str,
    initialized: bool,
    workspace: Option<String>,
}

#[derive(Deserialize)]
struct WorkspaceRequest {
    path: String,
    #[serde(default)]
    confirm: bool,
}

/// Real-time agent activity event, broadcast via SSE.
#[derive(Clone, Debug, Serialize)]
pub struct AgentActivityEvent {
    pub agent_id: String,
    pub event_type: String, // "tool_use", "thinking", "done", "error"
    pub detail: String,
    pub timestamp: String, // ISO8601
}

#[derive(Clone, Serialize)]
pub struct AgentInfo {
    pub id: String,
    pub handler: String,
    pub display_name: String,
    pub status: String, // "idle", "running", "error"
    pub last_activity: Option<String>,
    pub messages_processed: u64,
    pub repo_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    #[serde(skip)]
    pub loop_handle: Option<AbortHandle>,
}

pub struct RuntimeState {
    pub workspace: Option<PathBuf>,
    pub human_repo: Option<PathBuf>,
    pub poll_cursor: Option<String>,
    pub agents: HashMap<String, AgentInfo>,
    pub activity_tx: broadcast::Sender<AgentActivityEvent>,
    /// Epoch seconds of last activity. Used by idle watchdog.
    pub last_activity: std::sync::atomic::AtomicU64,
    pub github_api: Arc<dyn GithubApiClient>,
    /// Tests substitute a `file://` bare so the `git clone` step doesn't need
    /// the real internet. Production must leave this `None`; if it's ever
    /// `Some`, the token verification step has still run against the real API
    /// so we don't accidentally create a "demo mode" path.
    pub clone_url_override: Option<String>,
}

impl Default for RuntimeState {
    fn default() -> Self {
        let (activity_tx, _) = broadcast::channel(128);
        Self {
            workspace: None,
            human_repo: None,
            poll_cursor: None,
            agents: HashMap::new(),
            activity_tx,
            last_activity: std::sync::atomic::AtomicU64::new(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            ),
            github_api: Arc::new(DefaultGithubApi {
                base_url: "https://api.github.com".to_string(),
            }),
            clone_url_override: None,
        }
    }
}

pub type SharedRuntimeState = Arc<Mutex<RuntimeState>>;

/// Update the last-activity timestamp to now.
pub fn touch_activity(state: &SharedRuntimeState) {
    let s = state.lock().unwrap();
    s.last_activity.store(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// Check if any agent is currently running.
pub fn has_active_agents(state: &SharedRuntimeState) -> bool {
    let s = state.lock().unwrap();
    s.agents.values().any(|a| a.status == "running")
}

async fn health(State(state): State<SharedRuntimeState>) -> Json<HealthResponse> {
    let s = state.lock().unwrap();
    let initialized = s.workspace.is_some() && s.human_repo.is_some();
    let workspace = s.workspace.as_ref().map(|p| p.to_string_lossy().into_owned());
    Json(HealthResponse {
        service: "gitim-runtime",
        version: env!("CARGO_PKG_VERSION"),
        initialized,
        workspace,
    })
}

async fn set_workspace(
    State(state): State<SharedRuntimeState>,
    Json(req): Json<WorkspaceRequest>,
) -> Json<serde_json::Value> {
    let path = PathBuf::from(&req.path);

    if path.exists() {
        if !path.is_dir() {
            return Json(serde_json::json!({
                "ok": false,
                "error": format!("path exists but is not a directory: {}", req.path)
            }));
        }
        // Non-empty directory: ask for confirmation
        let is_empty = match std::fs::read_dir(&path) {
            Ok(mut entries) => entries.next().is_none(),
            Err(e) => {
                return Json(serde_json::json!({
                    "ok": false,
                    "error": format!("cannot read directory: {e}")
                }));
            }
        };
        if !is_empty && !req.confirm {
            return Json(serde_json::json!({
                "ok": false,
                "needs_confirm": true,
                "error": format!("directory is not empty: {}", req.path)
            }));
        }
    } else {
        // Create directory if it doesn't exist
        if let Err(e) = std::fs::create_dir_all(&path) {
            return Json(serde_json::json!({
                "ok": false,
                "error": format!("failed to create directory: {e}")
            }));
        }
    }

    // Create marker directory and write config
    let marker_dir = path.join(".gitim-runtime");
    if let Err(e) = std::fs::create_dir_all(&marker_dir) {
        return Json(serde_json::json!({
            "ok": false,
            "error": format!("failed to create .gitim-runtime: {e}")
        }));
    }

    let config = serde_json::json!({
        "workspace": req.path,
        "created_at": chrono::Utc::now().to_rfc3339(),
    });
    let config_path = marker_dir.join("config.json");
    if let Err(e) = std::fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()) {
        return Json(serde_json::json!({
            "ok": false,
            "error": format!("failed to write config: {e}")
        }));
    }

    let mut s = state.lock().unwrap();
    s.workspace = Some(path);

    Json(serde_json::json!({ "ok": true }))
}

#[derive(Deserialize)]
struct GitInitRequest {
    provider: String,
    #[serde(default)]
    remote_url: Option<String>,
    #[serde(default)]
    token: Option<String>,
}

async fn git_init(
    State(state): State<SharedRuntimeState>,
    Json(req): Json<GitInitRequest>,
) -> Json<serde_json::Value> {
    let workspace = {
        let s = state.lock().unwrap();
        match &s.workspace {
            Some(p) => p.clone(),
            None => {
                return Json(serde_json::json!({
                    "ok": false,
                    "error_code": "workspace_not_set",
                    "error": "workspace not set"
                }));
            }
        }
    };

    match req.provider.as_str() {
        "local" => git_init_local(&state, &workspace).await,
        "github" => git_init_github(&state, &workspace, req.remote_url, req.token).await,
        other => Json(serde_json::json!({
            "ok": false,
            "error_code": "provider_not_supported",
            "error": format!("provider not supported: {other}")
        })),
    }
}

async fn git_init_local(
    state: &SharedRuntimeState,
    workspace: &Path,
) -> Json<serde_json::Value> {
    let repo_path = workspace.join("repo.git");
    if let Err(e) = std::fs::create_dir_all(&repo_path) {
        return Json(serde_json::json!({
            "ok": false,
            "error_code": "clone_failed",
            "error": redacted_url(&format!("failed to create repo directory: {e}"))
        }));
    }

    let output = std::process::Command::new("git")
        .args(["init", "--bare"])
        .current_dir(&repo_path)
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let remote_url = repo_path.to_string_lossy().into_owned();
            let display_name = detect_git_config("user.name", workspace)
                .unwrap_or_else(|| "human".to_string());
            let handler = {
                let h = name_to_handler(&display_name);
                if h.is_empty() { "human".to_string() } else { h }
            };
            let auth = serde_json::json!({
                "type": "git",
                "handler": handler,
                "display_name": display_name,
            });
            match provision_human(workspace, &remote_url, "git", auth).await {
                Ok(human_dir) => {
                    {
                        let mut s = state.lock().unwrap();
                        s.human_repo = Some(human_dir.clone());
                    }
                    save_runtime_config(workspace);
                    let config = WorkspaceConfig {
                        workspace: workspace.to_string_lossy().into_owned(),
                        created_at: chrono::Utc::now().to_rfc3339(),
                        git: GitConfig {
                            provider: GitProvider::Local,
                            remote_url: None,
                            token: None,
                        },
                    };
                    if let Err(e) = config.write(workspace) {
                        return Json(serde_json::json!({
                            "ok": false,
                            "error_code": "config_write_failed",
                            "error": redacted_url(&format!("failed to write config: {e}"))
                        }));
                    }
                    let _ = mark_excluded_from_backups(&workspace.join(".gitim-runtime"));
                    Json(serde_json::json!({
                        "ok": true,
                        "repo_path": repo_path.to_string_lossy(),
                        "human_repo": human_dir.to_string_lossy()
                    }))
                }
                Err(e) => Json(serde_json::json!({
                    "ok": false,
                    "error_code": "onboard_failed",
                    "error": redacted_url(&format!("provision_human failed: {e}"))
                })),
            }
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            Json(serde_json::json!({
                "ok": false,
                "error_code": "clone_failed",
                "error": redacted_url(&format!("git init failed: {stderr}"))
            }))
        }
        Err(e) => {
            Json(serde_json::json!({
                "ok": false,
                "error_code": "clone_failed",
                "error": redacted_url(&format!("failed to run git: {e}"))
            }))
        }
    }
}

/// Assemble the token-carrying clone URL. Caller owns url parsing — we only
/// stamp `x-access-token:{token}` and restore the `.git` suffix that
/// `parse_github_url` stripped. Suffix is always singular because `parse_github_url`
/// also strips any explicit `.git` the user provided.
fn build_token_url(owner: &str, repo: &str, token: &str) -> String {
    format!("https://x-access-token:{token}@github.com/{owner}/{repo}.git")
}

fn github_error_code(err: &GithubError) -> &'static str {
    match err {
        GithubError::InvalidToken => "invalid_token",
        GithubError::InsufficientScope => "insufficient_scope",
        GithubError::RepoNotFoundOrNoAccess => "token_lacks_repo_access",
        GithubError::RateLimited => "rate_limited",
        GithubError::NetworkError(_) => "network_error",
        GithubError::UnexpectedStatus(_) => "network_error",
        GithubError::ParseError(_) => "clone_failed",
    }
}

fn cleanup_human_dir(workspace: &Path) {
    let human_dir = workspace.join(".gitim-runtime").join("human");
    let pid_file = human_dir.join(".gitim/run/gitim.pid");
    if let Ok(content) = std::fs::read_to_string(&pid_file) {
        if let Ok(pid) = content.trim().parse::<u32>() {
            // SIGTERM → grace → SIGKILL matches `kill_managed_daemons` in the
            // shell binary. Shelling out to `kill(1)` keeps us off a libc dep
            // for a single-platform-niche code path.
            let _ = std::process::Command::new("kill")
                .arg(pid.to_string())
                .output();
            std::thread::sleep(std::time::Duration::from_millis(500));
            let _ = std::process::Command::new("kill")
                .args(["-9", &pid.to_string()])
                .output();
        }
    }
    // Ignore NotFound — the directory may never have existed if clone failed
    // before reaching provision_human.
    let _ = std::fs::remove_dir_all(&human_dir);
}

async fn git_init_github(
    state: &SharedRuntimeState,
    workspace: &Path,
    remote_url: Option<String>,
    token: Option<String>,
) -> Json<serde_json::Value> {
    let token = match token {
        Some(t) if !t.is_empty() => t,
        _ => {
            return Json(serde_json::json!({
                "ok": false,
                "error_code": "missing_token",
                "error": "github mode requires a personal access token"
            }));
        }
    };
    let remote_url = match remote_url {
        Some(u) if !u.is_empty() => u,
        _ => {
            return Json(serde_json::json!({
                "ok": false,
                "error_code": "missing_remote_url",
                "error": "github mode requires remote_url"
            }));
        }
    };

    // On Windows the config file can't be made 0600 — reject before we spend
    // a network round-trip or touch the filesystem.
    #[cfg(windows)]
    {
        return Json(serde_json::json!({
            "ok": false,
            "error_code": "provider_not_supported",
            "error": "github mode is not supported on Windows"
        }));
    }

    if let Err(WorkspacePathError::CloudSyncDetected(service)) =
        validate_workspace_path_from_env(workspace)
    {
        return Json(serde_json::json!({
            "ok": false,
            "error_code": "cloud_sync_path_rejected",
            "error": format!("workspace is inside {service} — refusing to store a token there")
        }));
    }

    let (github_api, clone_override) = {
        let s = state.lock().unwrap();
        (s.github_api.clone(), s.clone_url_override.clone())
    };

    if let Err(e) = github_api.verify_token(&token).await {
        return Json(serde_json::json!({
            "ok": false,
            "error_code": github_error_code(&e),
            "error": redacted_url(&e.to_string())
        }));
    }

    let (owner, repo) = match parse_github_url(&remote_url) {
        Ok(t) => t,
        Err(e) => {
            return Json(serde_json::json!({
                "ok": false,
                "error_code": github_error_code(&e),
                "error": redacted_url(&e.to_string())
            }));
        }
    };

    if let Err(e) = github_api.check_repo_access(&owner, &repo, &token).await {
        return Json(serde_json::json!({
            "ok": false,
            "error_code": github_error_code(&e),
            "error": redacted_url(&e.to_string())
        }));
    }

    let clone_url = clone_override.unwrap_or_else(|| build_token_url(&owner, &repo, &token));

    let runtime_dir = workspace.join(".gitim-runtime");
    if let Err(e) = std::fs::create_dir_all(&runtime_dir) {
        return Json(serde_json::json!({
            "ok": false,
            "error_code": "clone_failed",
            "error": redacted_url(&format!("failed to create runtime dir: {e}"))
        }));
    }

    let human_dir = runtime_dir.join("human");
    if human_dir.exists() {
        // A prior failed init may have left a partial clone behind. Clean it
        // fully before retrying — provisioning is not re-entrant on partial state.
        cleanup_human_dir(workspace);
    }

    let clone_output = std::process::Command::new("git")
        .args(["clone", &clone_url, "human"])
        .current_dir(&runtime_dir)
        .output();

    match clone_output {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            cleanup_human_dir(workspace);
            return Json(serde_json::json!({
                "ok": false,
                "error_code": "clone_failed",
                "error": redacted_url(&format!("git clone failed: {stderr}"))
            }));
        }
        Err(e) => {
            cleanup_human_dir(workspace);
            return Json(serde_json::json!({
                "ok": false,
                "error_code": "clone_failed",
                "error": redacted_url(&format!("failed to run git: {e}"))
            }));
        }
    }

    // The remote URL stored with the clone would carry the token in
    // `.git/config` — rewrite it to the token-less public URL so any future
    // git operation (status inspection, debug dump) doesn't leak it. Token
    // injection for push/fetch happens through sync_loop's credential helper
    // in Task 7, not through the origin URL.
    let _ = std::process::Command::new("git")
        .args(["remote", "set-url", "origin", &remote_url])
        .current_dir(&human_dir)
        .output();

    let auth = serde_json::json!({
        "type": "github",
        "token": token,
    });

    match provision_human(workspace, &remote_url, "github", auth).await {
        Ok(final_human) => {
            {
                let mut s = state.lock().unwrap();
                s.human_repo = Some(final_human.clone());
            }
            save_runtime_config(workspace);
            let config = WorkspaceConfig {
                workspace: workspace.to_string_lossy().into_owned(),
                created_at: chrono::Utc::now().to_rfc3339(),
                git: GitConfig {
                    provider: GitProvider::Github,
                    remote_url: Some(remote_url.clone()),
                    token: Some(token.clone()),
                },
            };
            if let Err(e) = config.write(workspace) {
                cleanup_human_dir(workspace);
                return Json(serde_json::json!({
                    "ok": false,
                    "error_code": "config_write_failed",
                    "error": redacted_url(&format!("failed to write config: {e}"))
                }));
            }
            let _ = mark_excluded_from_backups(&runtime_dir);
            Json(serde_json::json!({
                "ok": true,
                "human_repo": final_human.to_string_lossy(),
                "remote_url": remote_url,
            }))
        }
        Err(e) => {
            cleanup_human_dir(workspace);
            Json(serde_json::json!({
                "ok": false,
                "error_code": "onboard_failed",
                "error": redacted_url(&format!("provision_human failed: {e}"))
            }))
        }
    }
}

// -- IM helpers --

fn human_client(state: &SharedRuntimeState) -> Result<GitimClient, Json<serde_json::Value>> {
    let s = state.lock().unwrap();
    match &s.human_repo {
        Some(p) => Ok(GitimClient::new(p)),
        None => Err(Json(serde_json::json!({
            "ok": false,
            "error": "human daemon not initialized"
        }))),
    }
}

fn api_response_to_json(result: Result<gitim_client::ApiResponse, gitim_client::ClientError>) -> Json<serde_json::Value> {
    match result {
        Ok(resp) => Json(serde_json::json!({
            "ok": resp.ok,
            "data": resp.data,
            "error": resp.error,
        })),
        Err(e) => Json(serde_json::json!({
            "ok": false,
            "error": e.to_string(),
        })),
    }
}

// -- /im/me --

async fn im_me(State(state): State<SharedRuntimeState>) -> Json<serde_json::Value> {
    let human_repo = {
        let s = state.lock().unwrap();
        match &s.human_repo {
            Some(p) => p.clone(),
            None => {
                return Json(serde_json::json!({
                    "ok": false,
                    "error": "human daemon not initialized"
                }));
            }
        }
    };

    let me_path = human_repo.join(".gitim/me.json");
    match std::fs::read_to_string(&me_path) {
        Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(me) => Json(serde_json::json!({
                "ok": true,
                "data": {
                    "handler": me.get("handler").and_then(|v| v.as_str()).unwrap_or("unknown"),
                    "display_name": me.get("display_name").and_then(|v| v.as_str()).unwrap_or("Unknown"),
                    "guest": me.get("guest").and_then(|v| v.as_bool()).unwrap_or(false),
                }
            })),
            Err(e) => Json(serde_json::json!({
                "ok": false,
                "error": format!("failed to parse me.json: {e}")
            })),
        },
        Err(e) => Json(serde_json::json!({
            "ok": false,
            "error": format!("failed to read me.json: {e}")
        })),
    }
}

// -- /im/channels --

async fn im_channels(State(state): State<SharedRuntimeState>) -> Json<serde_json::Value> {
    let client = match human_client(&state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    api_response_to_json(client.list_channels().await)
}

// -- /im/create-channel --

#[derive(Deserialize)]
struct CreateChannelRequest {
    name: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    introduction: Option<String>,
    #[serde(default)]
    invitees: Vec<String>,
}

async fn im_create(
    State(state): State<SharedRuntimeState>,
    Json(req): Json<CreateChannelRequest>,
) -> Json<serde_json::Value> {
    let client = match human_client(&state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    api_response_to_json(
        client
            .create_channel(&req.name, req.display_name.as_deref(), req.introduction.as_deref(), &req.invitees)
            .await,
    )
}

// -- /im/join --

#[derive(Deserialize)]
struct JoinRequest {
    channel: String,
    #[serde(default)]
    targets: Vec<String>,
}

async fn im_join(
    State(state): State<SharedRuntimeState>,
    Json(req): Json<JoinRequest>,
) -> Json<serde_json::Value> {
    let client = match human_client(&state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    api_response_to_json(client.join_channel(&req.channel, &req.targets).await)
}

// -- /im/send --

#[derive(Deserialize)]
struct SendRequest {
    channel: String,
    body: String,
    reply_to: Option<u64>,
}

async fn im_send(
    State(state): State<SharedRuntimeState>,
    Json(req): Json<SendRequest>,
) -> Json<serde_json::Value> {
    let client = match human_client(&state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    api_response_to_json(client.send(&req.channel, &req.body, None, req.reply_to).await)
}

// -- /im/read --

#[derive(Deserialize)]
struct ReadRequest {
    channel: String,
    limit: Option<u64>,
    since: Option<u64>,
}

async fn im_read(
    State(state): State<SharedRuntimeState>,
    Json(req): Json<ReadRequest>,
) -> Json<serde_json::Value> {
    let client = match human_client(&state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    api_response_to_json(client.read(&req.channel, req.limit, req.since).await)
}

// -- /im/poll --

#[derive(Deserialize)]
struct PollRequest {
    since: Option<String>,
}

async fn im_poll(
    State(state): State<SharedRuntimeState>,
    Json(req): Json<PollRequest>,
) -> Json<serde_json::Value> {
    let client = match human_client(&state) {
        Ok(c) => c,
        Err(e) => return e,
    };

    let cursor = {
        let s = state.lock().unwrap();
        req.since.clone().or_else(|| s.poll_cursor.clone())
    };

    let result = client.poll(cursor.as_deref()).await;

    // Update poll_cursor from response commit_id if present
    if let Ok(ref resp) = result {
        if resp.ok {
            if let Some(commit_id) = resp.data.as_ref().and_then(|d| d.get("commit_id")).and_then(|v| v.as_str()) {
                let mut s = state.lock().unwrap();
                s.poll_cursor = Some(commit_id.to_string());
            }
        }
    }

    api_response_to_json(result)
}

// -- /im/users --

async fn im_users(State(state): State<SharedRuntimeState>) -> Json<serde_json::Value> {
    let client = match human_client(&state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    api_response_to_json(client.list_users().await)
}

// -- /im/thread --

#[derive(Deserialize)]
struct ThreadRequest {
    channel: String,
    line: u64,
}

async fn im_thread(
    State(state): State<SharedRuntimeState>,
    Json(req): Json<ThreadRequest>,
) -> Json<serde_json::Value> {
    let client = match human_client(&state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    api_response_to_json(client.get_thread(&req.channel, req.line).await)
}

// -- /agents/add --

#[derive(Deserialize)]
struct AgentAddRequest {
    handler: String,
    display_name: String,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    system_prompt: Option<String>,
    #[serde(default)]
    env: HashMap<String, String>,
}

async fn agents_add(
    State(state): State<SharedRuntimeState>,
    Json(req): Json<AgentAddRequest>,
) -> Json<serde_json::Value> {
    let (workspace, already_exists) = {
        let s = state.lock().unwrap();
        let ws = match &s.workspace {
            Some(p) => p.clone(),
            None => {
                return Json(serde_json::json!({
                    "ok": false,
                    "error": "workspace not set"
                }));
            }
        };
        let exists = s.agents.contains_key(&req.handler);
        (ws, exists)
    };

    if already_exists {
        return Json(serde_json::json!({
            "ok": false,
            "error": format!("agent already exists: {}", req.handler)
        }));
    }

    let agents_dir = workspace.clone();
    if let Err(e) = std::fs::create_dir_all(&agents_dir) {
        return Json(serde_json::json!({
            "ok": false,
            "error": format!("failed to create agents dir: {e}")
        }));
    }

    let bare_repo = workspace.join("repo.git");
    let config = AgentConfig {
        handler: req.handler.clone(),
        display_name: req.display_name.clone(),
        remote_url: bare_repo.to_string_lossy().to_string(),
    };

    match provision_agent(&agents_dir, &config).await {
        Ok(handle) => {
            // Recheck after async provision to prevent duplicate loops from concurrent requests
            {
                let s = state.lock().unwrap();
                if s.agents.contains_key(&req.handler) {
                    return Json(serde_json::json!({
                        "ok": true,
                        "id": req.handler,
                    }));
                }
            }

            // Persist config to me.json
            let me_path = handle.repo_root.join(".gitim/me.json");
            if let Ok(content) = std::fs::read_to_string(&me_path) {
                if let Ok(mut me) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(provider) = &req.provider {
                        me["provider"] = serde_json::Value::String(provider.clone());
                    }
                    if let Some(model) = &req.model {
                        me["model"] = serde_json::Value::String(model.clone());
                    }
                    if let Some(sp) = &req.system_prompt {
                        me["system_prompt"] = serde_json::Value::String(sp.clone());
                    }
                    if !req.env.is_empty() {
                        me["env"] = serde_json::to_value(&req.env).unwrap_or_default();
                    }
                    let _ = std::fs::write(&me_path, serde_json::to_string_pretty(&me).unwrap());
                }
            }

            let info = AgentInfo {
                id: req.handler.clone(),
                handler: req.handler.clone(),
                display_name: req.display_name.clone(),
                status: "idle".to_string(),
                last_activity: None,
                messages_processed: 0,
                repo_path: handle.repo_root.display().to_string(),
                provider: req.provider.clone(),
                model: req.model.clone(),
                system_prompt: req.system_prompt.clone(),
                env: req.env.clone(),
                loop_handle: None,
            };
            {
                let mut s = state.lock().unwrap();
                s.agents.insert(req.handler.clone(), info);
            }

            // Auto-start the agent loop
            if let Err(e) = start_agent_loop(&state, &req.handler) {
                tracing::warn!("agent @{} created but auto-start failed: {e}", req.handler);
            }

            Json(serde_json::json!({ "ok": true, "id": req.handler }))
        }
        Err(e) => Json(serde_json::json!({
            "ok": false,
            "error": format!("provision_agent failed: {e}")
        })),
    }
}

// -- /agents --

async fn agents_list(State(state): State<SharedRuntimeState>) -> Json<serde_json::Value> {
    let s = state.lock().unwrap();
    let agents: Vec<&AgentInfo> = s.agents.values().collect();
    Json(serde_json::json!({ "ok": true, "agents": agents }))
}

// -- /agents/start --

#[derive(Deserialize)]
struct AgentIdRequest {
    id: String,
}

/// Start the agent loop for a given agent ID. Shared by add, start, and recover.
fn start_agent_loop(state: &SharedRuntimeState, agent_id: &str) -> Result<(), String> {
    let (repo_root, handler, provider, model, system_prompt, env) = {
        let s = state.lock().unwrap();
        match s.agents.get(agent_id) {
            None => return Err(format!("agent not found: {agent_id}")),
            Some(info) if info.status == "running" => {
                return Ok(()); // idempotent: already running is ok
            }
            Some(info) => (
                PathBuf::from(&info.repo_path),
                info.handler.clone(),
                info.provider.clone(),
                info.model.clone(),
                info.system_prompt.clone(),
                info.env.clone(),
            ),
        }
    };

    let loop_config = crate::agent_loop::AgentLoopConfig {
        provider_type: provider.unwrap_or_else(|| "claude".to_string()),
        handler,
        model,
        system_prompt,
        env,
    };
    let mut agent_loop = AgentLoop::with_config(&repo_root, &loop_config)
        .map_err(|e| format!("failed to create agent loop: {e}"))?;

    // Wire up activity broadcast
    {
        let s = state.lock().unwrap();
        agent_loop.set_activity_tx(s.activity_tx.clone());
    }

    let owned_id = agent_id.to_string();
    let state_clone = state.clone();

    let handle = tokio::spawn(async move {
        // Initialize poller cursor (same as run() does)
        if let Err(e) = agent_loop.init().await {
            tracing::error!(error = %e, "agent loop init failed");
            let mut s = state_clone.lock().unwrap();
            if let Some(info) = s.agents.get_mut(&owned_id) {
                info.loop_handle = None;
                info.status = "error".to_string();
            }
            return;
        }

        let poll_interval = agent_loop.poll_interval;
        let mut consecutive_errors: u32 = 0;
        const MAX_BACKOFF_SECS: u64 = 60;

        loop {
            match agent_loop.run_once().await {
                Ok(true) => {
                    consecutive_errors = 0;
                    if let Ok(mut s) = state_clone.try_lock() {
                        if let Some(info) = s.agents.get_mut(&owned_id) {
                            info.messages_processed += 1;
                            info.last_activity =
                                Some(chrono::Utc::now().to_rfc3339());
                        }
                    }
                    touch_activity(&state_clone);
                }
                Ok(false) => {
                    consecutive_errors = 0;
                }
                Err(e) => {
                    consecutive_errors += 1;
                    let backoff = std::time::Duration::from_secs(
                        (2u64.saturating_pow(consecutive_errors))
                            .min(MAX_BACKOFF_SECS),
                    );
                    tracing::error!(
                        error = %e,
                        consecutive = consecutive_errors,
                        backoff_secs = backoff.as_secs(),
                        "agent loop error, backing off"
                    );
                    tokio::time::sleep(backoff).await;
                    continue;
                }
            }
            tokio::time::sleep(poll_interval).await;
        }
    });

    let abort_handle = handle.abort_handle();
    {
        let mut s = state.lock().unwrap();
        if let Some(info) = s.agents.get_mut(agent_id) {
            info.loop_handle = Some(abort_handle);
            info.status = "running".to_string();
        }
    }

    Ok(())
}

async fn agents_start(
    State(state): State<SharedRuntimeState>,
    Json(req): Json<AgentIdRequest>,
) -> Json<serde_json::Value> {
    match start_agent_loop(&state, &req.id) {
        Ok(()) => Json(serde_json::json!({ "ok": true })),
        Err(e) => Json(serde_json::json!({ "ok": false, "error": e })),
    }
}

// -- /agents/:id --

async fn agents_get(
    State(state): State<SharedRuntimeState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    let s = state.lock().unwrap();
    match s.agents.get(&id) {
        Some(info) => Json(serde_json::json!({ "ok": true, "agent": info })),
        None => Json(serde_json::json!({ "ok": false, "error": "agent not found" })),
    }
}

// -- /agents/remove --

async fn agents_remove(
    State(state): State<SharedRuntimeState>,
    Json(req): Json<AgentIdRequest>,
) -> Json<serde_json::Value> {
    let mut s = state.lock().unwrap();
    match s.agents.remove(&req.id) {
        Some(info) => {
            if let Some(handle) = &info.loop_handle {
                handle.abort();
            }
            // Kill the agent's daemon process
            let pid_file = PathBuf::from(&info.repo_path).join(".gitim/run/gitim.pid");
            if let Ok(content) = std::fs::read_to_string(&pid_file) {
                if let Ok(pid) = content.trim().parse::<u32>() {
                    let _ = std::process::Command::new("kill").arg(pid.to_string()).output();
                }
            }
            Json(serde_json::json!({ "ok": true }))
        }
        None => Json(serde_json::json!({ "ok": false, "error": "agent not found" })),
    }
}

// -- /agents/stop --

async fn agents_stop(
    State(state): State<SharedRuntimeState>,
    Json(req): Json<AgentIdRequest>,
) -> Json<serde_json::Value> {
    let abort_handle = {
        let mut s = state.lock().unwrap();
        match s.agents.get_mut(&req.id) {
            None => {
                return Json(serde_json::json!({
                    "ok": false,
                    "error": format!("agent not found: {}", req.id)
                }));
            }
            Some(info) => {
                let handle = info.loop_handle.take();
                info.status = "idle".to_string();
                handle
            }
        }
    };

    if let Some(handle) = abort_handle {
        handle.abort();
    }

    Json(serde_json::json!({ "ok": true }))
}

// -- /agents/events (SSE) --

async fn agents_events(
    State(state): State<SharedRuntimeState>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let rx = {
        let s = state.lock().unwrap();
        s.activity_tx.subscribe()
    };

    let stream = tokio_stream::wrappers::BroadcastStream::new(rx).filter_map(|result| {
        futures::future::ready(match result {
            Ok(event) => {
                let data = serde_json::to_string(&event).unwrap_or_default();
                Some(Ok(SseEvent::default().data(data)))
            }
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(_)) => None,
        })
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

// -- persistence helpers --

fn runtime_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".gitim/runtime.json"))
}

fn save_runtime_config(workspace: &Path) {
    if let Some(config_path) = runtime_config_path() {
        if let Some(parent) = config_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let config = serde_json::json!({ "workspace": workspace.to_string_lossy() });
        let _ = std::fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap());
    }
}

/// Recover workspace state from `~/.gitim/runtime.json` on startup.
/// Restores workspace path, human daemon, and agent daemons.
pub async fn recover_from_config(state: SharedRuntimeState) {
    let config_path = match runtime_config_path() {
        Some(p) if p.exists() => p,
        _ => return,
    };

    let content = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let config: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return,
    };
    let workspace_str = match config["workspace"].as_str() {
        Some(s) => s,
        None => return,
    };

    let workspace = PathBuf::from(workspace_str);
    if !workspace.exists() {
        tracing::warn!("saved workspace {} no longer exists, skipping recovery", workspace_str);
        return;
    }

    tracing::info!("recovering workspace from {}", workspace_str);
    {
        let mut s = state.lock().unwrap();
        s.workspace = Some(workspace.clone());
    }

    // Recover human daemon
    let human_dir = workspace.join(".gitim-runtime/human");
    if human_dir.exists() {
        let remote_url = workspace.join("repo.git").to_string_lossy().into_owned();
        let display_name = detect_git_config("user.name", &workspace)
            .unwrap_or_else(|| "human".to_string());
        let handler = {
            let h = name_to_handler(&display_name);
            if h.is_empty() { "human".to_string() } else { h }
        };
        let auth = serde_json::json!({
            "type": "git",
            "handler": handler,
            "display_name": display_name,
        });
        match provision_human(&workspace, &remote_url, "git", auth).await {
            Ok(dir) => {
                let mut s = state.lock().unwrap();
                s.human_repo = Some(dir);
                tracing::info!("human daemon recovered");
            }
            Err(e) => tracing::warn!("failed to recover human daemon: {e}"),
        }
    }

    // Scan for agent directories (have .gitim/me.json, not human or repo.git)
    let entries = match std::fs::read_dir(&workspace) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() { continue; }
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "repo.git" || name.starts_with('.') { continue; }

        let me_path = dir.join(".gitim/me.json");
        if !me_path.exists() { continue; }

        let me_content = match std::fs::read_to_string(&me_path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let me: serde_json::Value = match serde_json::from_str(&me_content) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let handler = match me["handler"].as_str() {
            Some(h) => h.to_string(),
            None => continue,
        };
        let display_name = me["display_name"]
            .as_str()
            .unwrap_or(&handler)
            .to_string();

        // Ensure agent daemon is running
        let root = dir.clone();
        match tokio::task::spawn_blocking(move || gitim_client::ensure_daemon(&root)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::warn!("failed to start daemon for @{handler}: {e}");
                continue;
            }
            Err(e) => {
                tracing::warn!("task panicked for @{handler}: {e}");
                continue;
            }
        }

        {
            let mut s = state.lock().unwrap();
            let provider = me["provider"].as_str().map(|s| s.to_string());
            let model = me["model"].as_str().map(|s| s.to_string());
            let custom_system_prompt = me["system_prompt"].as_str().map(|s| s.to_string());
            let env: HashMap<String, String> = me.get("env")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();

            s.agents.insert(handler.clone(), AgentInfo {
                id: handler.clone(),
                handler: handler.clone(),
                display_name,
                status: "idle".to_string(),
                last_activity: None,
                messages_processed: 0,
                repo_path: dir.display().to_string(),
                provider,
                model,
                system_prompt: custom_system_prompt,
                env,
                loop_handle: None,
            });
        }

        // Auto-start the agent loop on recovery
        match start_agent_loop(&state, &handler) {
            Ok(()) => tracing::info!("agent @{handler} recovered and started"),
            Err(e) => tracing::warn!("agent @{handler} recovered but auto-start failed: {e}"),
        }
    }
}

async fn preflight_claude() -> impl axum::response::IntoResponse {
    match crate::preflight::check_claude().await {
        Ok(version) => Json(serde_json::json!({
            "available": true,
            "version": version,
        })),
        Err(error) => Json(serde_json::json!({
            "available": false,
            "error": error,
        })),
    }
}

async fn activity_middleware(
    State(state): State<SharedRuntimeState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    touch_activity(&state);
    next.run(request).await
}

pub fn create_router() -> (Router, SharedRuntimeState) {
    let state: SharedRuntimeState = Arc::new(Mutex::new(RuntimeState::default()));

    let router = Router::new()
        .route("/health", get(health))
        .route("/workspace", post(set_workspace))
        .route("/git/init", post(git_init))
        .route("/im/me", get(im_me))
        .route("/im/channels", get(im_channels))
        .route("/im/create-channel", post(im_create))
        .route("/im/join", post(im_join))
        .route("/im/send", post(im_send))
        .route("/im/read", post(im_read))
        .route("/im/poll", post(im_poll))
        .route("/im/users", get(im_users))
        .route("/im/thread", post(im_thread))
        .route("/agents", get(agents_list))
        .route("/agents/events", get(agents_events))
        .route("/agents/add", post(agents_add))
        .route("/agents/start", post(agents_start))
        .route("/agents/stop", post(agents_stop))
        .route("/agents/remove", post(agents_remove))
        .route("/agents/{id}", get(agents_get))
        .route("/preflight/claude", get(preflight_claude))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            activity_middleware,
        ))
        .layer(CorsLayer::permissive())
        .with_state(state.clone());

    (router, state)
}
