use async_trait::async_trait;
use axum::{extract::State, routing::{get, post}, Json, Router};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures::stream::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::task::AbortHandle;
use tower_http::cors::CorsLayer;

use crate::agent::{detect_git_config, name_to_handler, provision_agent, provision_human, AgentConfig};
use crate::agent_loop::AgentLoop;
use crate::git_config::{
    mark_excluded_from_backups, validate_workspace_path_from_env, GitConfig, GitProvider,
    WorkspaceConfig,
};
use crate::github::{check_repo_access, parse_github_url, verify_token, GithubError};
use gitim_client::GitimClient;
use gitim_sync::url_redact::redacted_url;

/// Default TCP port for the runtime HTTP server. Shared between
/// `RuntimeState::default()` and `bin/runtime.rs`'s argv parser so the two
/// can't drift. Chosen to sit well above the IANA registered range and out
/// of the ephemeral-port band on macOS / Linux.
pub const DEFAULT_PORT: u16 = 16868;

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
    workspaces_count: usize,
}

/// Real-time agent activity event, broadcast via SSE.
///
/// `workspace_id` always carries the originating workspace's slug so SSE
/// subscribers can route or filter events. Events are published on the
/// workspace-scoped `broadcast::Sender` held in `WorkspaceContext`.
#[derive(Clone, Debug, Serialize)]
pub struct AgentActivityEvent {
    pub agent_id: String,
    pub workspace_id: String,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip)]
    pub loop_handle: Option<AbortHandle>,
}

pub struct RuntimeState {
    /// Epoch seconds of last activity. Used by idle watchdog.
    pub last_activity: std::sync::atomic::AtomicU64,
    pub github_api: Arc<dyn GithubApiClient>,
    /// Tests substitute a `file://` bare so the `git clone` step doesn't need
    /// the real internet. Production must leave this `None`; if it's ever
    /// `Some`, the token verification step has still run against the real API
    /// so we don't accidentally create a "demo mode" path.
    pub clone_url_override: Option<String>,
    pub workspaces: HashMap<String, crate::workspace::WorkspaceContext>,
    /// Canonicalized path to the runtime binary captured at startup. The
    /// update endpoint (self-replace) uses this to (a) validate the install
    /// dir in strict mode, and (b) fork-exec a new runtime after the binary
    /// is swapped. We must capture this BEFORE the binary is replaced on
    /// disk — on Linux `std::env::current_exe()` returns `<path> (deleted)`
    /// for an inode whose dentry has been unlinked.
    pub canonical_exe_path: PathBuf,
    /// Guard against concurrent self-update runs. Set when the sync phase of
    /// `POST /runtime/update-and-restart` begins; cleared when the async phase
    /// finishes or any step fails. A second request arriving while this is
    /// `true` gets a `409 concurrent_update`.
    pub update_in_progress: Arc<std::sync::atomic::AtomicBool>,
    /// Most recent async-phase failure from `/runtime/update-and-restart`.
    /// Written by the async phase on error (replace / fork-exec) so a future
    /// diagnostic endpoint or log export can surface what went wrong. v1 has
    /// no UI that reads this — the WebUI just polls `/health` for the new
    /// version and times out on failure — but we still capture the detail so
    /// it isn't silently lost.
    pub update_last_error: Option<String>,
    /// TCP port the runtime's HTTP server is bound to. Set by `run_shell`
    /// after argument parsing so the async self-update phase knows which
    /// `--port` to pass when fork-exec'ing the replacement binary. Tests that
    /// go through `create_router()` / `create_router_with_exe()` leave the
    /// default; the E2E test overrides it before driving the async phase.
    pub listen_port: u16,
}

impl RuntimeState {
    pub fn get(&self, slug: &str) -> Option<&crate::workspace::WorkspaceContext> {
        self.workspaces.get(slug)
    }

    pub fn get_mut(&mut self, slug: &str) -> Option<&mut crate::workspace::WorkspaceContext> {
        self.workspaces.get_mut(slug)
    }
}

impl Default for RuntimeState {
    fn default() -> Self {
        // E2E test seam: env vars let a compiled binary point at a stub
        // github API + a local `file://` bare repo instead of github.com.
        // Unset in production; Rust integration tests still override directly
        // via `state.lock()`.
        let base_url = std::env::var("GITIM_TEST_GITHUB_API_BASE")
            .unwrap_or_else(|_| "https://api.github.com".to_string());
        let clone_url_override = std::env::var("GITIM_TEST_CLONE_URL_OVERRIDE").ok();
        // Best-effort canonical exe for test constructors. Production boots
        // via `run_shell()` which computes + passes the real path into
        // `create_router_with_exe` — this fallback only matters in unit/IT
        // tests that call `RuntimeState::default()` / `create_router()`
        // directly. A placeholder at `/nonexistent/gitim-runtime` keeps
        // Task 6/7 strict-mode checks safe: the update endpoint will refuse
        // to self-replace a path that doesn't exist.
        let canonical_exe_path = std::env::current_exe()
            .and_then(|p| p.canonicalize())
            .unwrap_or_else(|_| PathBuf::from("/nonexistent/gitim-runtime"));
        Self {
            last_activity: std::sync::atomic::AtomicU64::new(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            ),
            github_api: Arc::new(DefaultGithubApi { base_url }),
            clone_url_override,
            workspaces: HashMap::new(),
            canonical_exe_path,
            update_in_progress: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            update_last_error: None,
            listen_port: DEFAULT_PORT,
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

/// Check if any agent is currently running across all workspaces.
pub fn has_active_agents(state: &SharedRuntimeState) -> bool {
    let s = state.lock().unwrap();
    s.workspaces
        .values()
        .flat_map(|w| w.agents.values())
        .any(|a| a.status == "running")
}

async fn health(State(state): State<SharedRuntimeState>) -> Json<HealthResponse> {
    let s = state.lock().unwrap();
    Json(HealthResponse {
        service: "gitim-runtime",
        version: env!("CARGO_PKG_VERSION"),
        workspaces_count: s.workspaces.len(),
    })
}

pub struct WorkspaceSlug(pub String);

impl<S> axum::extract::FromRequestParts<S> for WorkspaceSlug
where
    S: Send + Sync,
{
    type Rejection = axum::response::Response;
    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        use axum::extract::Path;
        use axum::response::IntoResponse;
        let Path(slug): Path<String> = Path::from_request_parts(parts, state)
            .await
            .map_err(|e| e.into_response())?;
        crate::slug::validate(&slug).map_err(|e| {
            (
                axum::http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "ok": false,
                    "error": format!("invalid slug: {e}")
                })),
            )
                .into_response()
        })?;
        Ok(WorkspaceSlug(slug))
    }
}

fn not_found_workspace() -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        axum::http::StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "ok": false, "error": "unknown workspace" })),
    )
        .into_response()
}

fn with_workspace_snapshot<F, R>(
    state: &SharedRuntimeState,
    slug: &str,
    f: F,
) -> Result<R, axum::response::Response>
where
    F: FnOnce(&crate::workspace::WorkspaceContext) -> R,
{
    let s = state.lock().unwrap();
    let ctx = s.workspaces.get(slug).ok_or_else(not_found_workspace)?;
    Ok(f(ctx))
}

fn with_workspace_mut<F, R>(
    state: &SharedRuntimeState,
    slug: &str,
    f: F,
) -> Result<R, axum::response::Response>
where
    F: FnOnce(&mut crate::workspace::WorkspaceContext) -> R,
{
    let mut s = state.lock().unwrap();
    let ctx = s.workspaces.get_mut(slug).ok_or_else(not_found_workspace)?;
    Ok(f(ctx))
}

/// Assemble the token-carrying clone URL. Caller owns url parsing — we only
/// stamp `x-access-token:{token}` and restore the `.git` suffix that
/// `parse_github_url` stripped. Suffix is always singular because `parse_github_url`
/// also strips any explicit `.git` the user provided.
pub(crate) fn build_token_url(owner: &str, repo: &str, token: &str) -> String {
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

// -- IM helpers --

fn human_client(
    state: &SharedRuntimeState,
    slug: &str,
) -> Result<GitimClient, axum::response::Response> {
    use axum::response::IntoResponse;
    let s = state.lock().unwrap();
    let ctx = s.workspaces.get(slug).ok_or_else(not_found_workspace)?;
    match &ctx.human_repo {
        Some(p) => Ok(GitimClient::new(p)),
        None => Err(Json(serde_json::json!({
            "ok": false,
            "error": "human daemon not initialized"
        }))
        .into_response()),
    }
}

fn api_response_to_json(
    result: Result<gitim_client::ApiResponse, gitim_client::ClientError>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    match result {
        Ok(resp) => Json(serde_json::json!({
            "ok": resp.ok,
            "data": resp.data,
            "error": resp.error,
        }))
        .into_response(),
        Err(e) => Json(serde_json::json!({
            "ok": false,
            "error": e.to_string(),
        }))
        .into_response(),
    }
}

// -- /im/me --

async fn im_me(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let human_repo = match with_workspace_snapshot(&state, &slug, |ctx| ctx.human_repo.clone()) {
        Ok(Some(p)) => p,
        Ok(None) => {
            return Json(serde_json::json!({
                "ok": false,
                "error": "human daemon not initialized"
            }))
            .into_response();
        }
        Err(r) => return r,
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
            }))
            .into_response(),
            Err(e) => Json(serde_json::json!({
                "ok": false,
                "error": format!("failed to parse me.json: {e}")
            }))
            .into_response(),
        },
        Err(e) => Json(serde_json::json!({
            "ok": false,
            "error": format!("failed to read me.json: {e}")
        }))
        .into_response(),
    }
}

// -- /im/channels --

async fn im_channels(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
) -> axum::response::Response {
    let client = match human_client(&state, &slug) {
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
    WorkspaceSlug(slug): WorkspaceSlug,
    Json(req): Json<CreateChannelRequest>,
) -> axum::response::Response {
    let client = match human_client(&state, &slug) {
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
    WorkspaceSlug(slug): WorkspaceSlug,
    Json(req): Json<JoinRequest>,
) -> axum::response::Response {
    let client = match human_client(&state, &slug) {
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
    WorkspaceSlug(slug): WorkspaceSlug,
    Json(req): Json<SendRequest>,
) -> axum::response::Response {
    let client = match human_client(&state, &slug) {
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
    WorkspaceSlug(slug): WorkspaceSlug,
    Json(req): Json<ReadRequest>,
) -> axum::response::Response {
    let client = match human_client(&state, &slug) {
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
    WorkspaceSlug(slug): WorkspaceSlug,
    Json(req): Json<PollRequest>,
) -> axum::response::Response {
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(e) => return e,
    };

    let cursor = match with_workspace_snapshot(&state, &slug, |ctx| {
        req.since.clone().or_else(|| ctx.poll_cursor.clone())
    }) {
        Ok(c) => c,
        Err(r) => return r,
    };

    let result = client.poll(cursor.as_deref()).await;

    if let Ok(ref resp) = result {
        if resp.ok {
            if let Some(commit_id) = resp
                .data
                .as_ref()
                .and_then(|d| d.get("commit_id"))
                .and_then(|v| v.as_str())
            {
                let _ = with_workspace_mut(&state, &slug, |ctx| {
                    ctx.poll_cursor = Some(commit_id.to_string());
                });
            }
        }
    }

    api_response_to_json(result)
}

// -- /im/users --

async fn im_users(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
) -> axum::response::Response {
    let client = match human_client(&state, &slug) {
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
    WorkspaceSlug(slug): WorkspaceSlug,
    Json(req): Json<ThreadRequest>,
) -> axum::response::Response {
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(e) => return e,
    };
    api_response_to_json(client.get_thread(&req.channel, req.line).await)
}

// -- /im/cards --

#[derive(Deserialize)]
struct CreateCardRequest {
    channel: String,
    title: String,
    #[serde(default)]
    labels: Option<Vec<String>>,
    #[serde(default)]
    assignee: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

async fn im_create_card(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
    Json(req): Json<CreateCardRequest>,
) -> axum::response::Response {
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    let labels_slice = req.labels.as_deref();
    api_response_to_json(
        client
            .create_card(
                &req.channel,
                &req.title,
                labels_slice,
                req.assignee.as_deref(),
                req.status.as_deref(),
            )
            .await,
    )
}

#[derive(Deserialize)]
struct ListCardsQuery {
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    label: Vec<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    assignee: Option<String>,
}

async fn im_list_cards(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
    axum::extract::Query(q): axum::extract::Query<ListCardsQuery>,
) -> axum::response::Response {
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    let labels_slice: Option<&[String]> = if q.label.is_empty() { None } else { Some(&q.label) };
    api_response_to_json(
        client
            .list_cards(q.channel.as_deref(), labels_slice, q.status.as_deref(), q.assignee.as_deref())
            .await,
    )
}

#[derive(Deserialize)]
struct ReadCardQuery {
    #[serde(default)]
    limit: Option<u64>,
    #[serde(default)]
    since: Option<u64>,
}

async fn im_read_card(
    State(state): State<SharedRuntimeState>,
    axum::extract::Path((slug, channel, card_id)): axum::extract::Path<(String, String, String)>,
    axum::extract::Query(q): axum::extract::Query<ReadCardQuery>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if let Err(e) = crate::slug::validate(&slug) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": format!("invalid slug: {e}") })),
        )
            .into_response();
    }
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    api_response_to_json(
        client.read_card(&channel, &card_id, q.limit, q.since).await,
    )
}

#[derive(Deserialize)]
struct SendCardMessageRequest {
    body: String,
    #[serde(default)]
    reply_to: Option<u64>,
}

async fn im_send_card_message(
    State(state): State<SharedRuntimeState>,
    axum::extract::Path((slug, channel, card_id)): axum::extract::Path<(String, String, String)>,
    Json(req): Json<SendCardMessageRequest>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if let Err(e) = crate::slug::validate(&slug) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": format!("invalid slug: {e}") })),
        )
            .into_response();
    }
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    api_response_to_json(
        client
            .send_card_message(&channel, &card_id, &req.body, req.reply_to)
            .await,
    )
}

#[derive(Deserialize)]
struct UpdateCardRequest {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    labels: Option<Vec<String>>,
    #[serde(default)]
    assignee: Option<String>,
}

async fn im_update_card(
    State(state): State<SharedRuntimeState>,
    axum::extract::Path((slug, channel, card_id)): axum::extract::Path<(String, String, String)>,
    Json(req): Json<UpdateCardRequest>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if let Err(e) = crate::slug::validate(&slug) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": format!("invalid slug: {e}") })),
        )
            .into_response();
    }
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    let labels_slice = req.labels.as_deref();
    api_response_to_json(
        client
            .update_card(
                &channel,
                &card_id,
                req.status.as_deref(),
                labels_slice,
                req.assignee.as_deref(),
            )
            .await,
    )
}

// -- /im/cards archive/unarchive + /im/channels archive/unarchive --
//
// Cards carry an explicit `author` in the daemon API (creator/assignee check +
// commit attribution), so these handlers read the workspace's `.gitim/me.json`
// the same way the CLI does. Channel archive doesn't need an author.

/// Read the human's handler from `$human_repo/.gitim/me.json`. Mirrors the
/// CLI's `read_my_handler` — returns a structured JSON error the route can
/// short-circuit on when the workspace isn't provisioned or the file is
/// unreadable.
fn human_handler(
    state: &SharedRuntimeState,
    slug: &str,
) -> Result<String, axum::response::Response> {
    use axum::response::IntoResponse;
    let human_repo = with_workspace_snapshot(state, slug, |ctx| ctx.human_repo.clone())?
        .ok_or_else(|| {
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "ok": false,
                    "error": "human daemon not initialized"
                })),
            )
                .into_response()
        })?;
    let me_path = human_repo.join(".gitim/me.json");
    let content = std::fs::read_to_string(&me_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "ok": false,
                "error": format!("failed to read me.json: {e}")
            })),
        )
            .into_response()
    })?;
    let me: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "ok": false,
                "error": format!("failed to parse me.json: {e}")
            })),
        )
            .into_response()
    })?;
    me.get("handler")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "ok": false,
                    "error": "me.json missing handler field"
                })),
            )
                .into_response()
        })
}

async fn im_card_archive(
    State(state): State<SharedRuntimeState>,
    axum::extract::Path((slug, channel, card_id)): axum::extract::Path<(String, String, String)>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if let Err(e) = crate::slug::validate(&slug) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": format!("invalid slug: {e}") })),
        )
            .into_response();
    }
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    let author = match human_handler(&state, &slug) {
        Ok(h) => h,
        Err(j) => return j,
    };
    api_response_to_json(client.archive_card(&channel, &card_id, &author).await)
}

async fn im_card_unarchive(
    State(state): State<SharedRuntimeState>,
    axum::extract::Path((slug, channel, card_id)): axum::extract::Path<(String, String, String)>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if let Err(e) = crate::slug::validate(&slug) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": format!("invalid slug: {e}") })),
        )
            .into_response();
    }
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    let author = match human_handler(&state, &slug) {
        Ok(h) => h,
        Err(j) => return j,
    };
    api_response_to_json(client.unarchive_card(&channel, &card_id, &author).await)
}

#[derive(Deserialize)]
struct ListArchivedCardsQuery {
    #[serde(default)]
    channel: Option<String>,
}

async fn im_list_archived_cards(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
    axum::extract::Query(q): axum::extract::Query<ListArchivedCardsQuery>,
) -> axum::response::Response {
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    api_response_to_json(client.list_archived_cards(q.channel.as_deref()).await)
}

async fn im_channel_archive(
    State(state): State<SharedRuntimeState>,
    axum::extract::Path((slug, name)): axum::extract::Path<(String, String)>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if let Err(e) = crate::slug::validate(&slug) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": format!("invalid slug: {e}") })),
        )
            .into_response();
    }
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    api_response_to_json(client.archive_channel(&name).await)
}

async fn im_channel_unarchive(
    State(state): State<SharedRuntimeState>,
    axum::extract::Path((slug, name)): axum::extract::Path<(String, String)>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if let Err(e) = crate::slug::validate(&slug) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": format!("invalid slug: {e}") })),
        )
            .into_response();
    }
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    api_response_to_json(client.unarchive_channel(&name).await)
}

async fn im_list_archived_channels(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
) -> axum::response::Response {
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    api_response_to_json(client.list_archived_channels().await)
}

// -- /agents/add --

#[derive(Deserialize)]
struct AgentAddRequest {
    handler: String,
    display_name: String,
    // `provider` is required. Omitting it triggers serde's "missing field"
    // error, which axum's Json extractor reports as a 4xx before the handler
    // body runs — the WebUI can no longer silently fall back to Claude.
    provider: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    system_prompt: Option<String>,
    #[serde(default)]
    env: HashMap<String, String>,
}

async fn agents_add(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
    Json(req): Json<AgentAddRequest>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    // Provider whitelist runs before workspace lookup so invalid input is
    // rejected even when the runtime has no workspaces yet.
    // `mock` is permitted because existing E2E tests (agent-interaction.spec.ts)
    // provision an agent with provider=mock; the UI still only offers
    // claude/codex per Q1 scope.
    match req.provider.as_str() {
        "claude" | "codex" | "mock" => {}
        other => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "ok": false,
                    "error": format!("unsupported provider: {other}"),
                })),
            )
                .into_response();
        }
    }

    let (workspace, human_repo, already_exists) = {
        let s = state.lock().unwrap();
        let ctx = match s.workspaces.get(&slug) {
            Some(c) => c,
            None => return not_found_workspace(),
        };
        let human = ctx.human_repo.clone();
        let exists = ctx.agents.contains_key(&req.handler);
        (ctx.path.clone(), human, exists)
    };

    if already_exists {
        return Json(serde_json::json!({
            "ok": false,
            "error_code": "handler_conflict",
            "error": format!("agent already exists: {}", req.handler)
        }))
        .into_response();
    }

    // Read workspace config; treat a missing/legacy file as local mode so
    // workspaces from before the github schema still work.
    let workspace_config = WorkspaceConfig::read(&workspace).ok();
    let git_provider = workspace_config
        .as_ref()
        .map(|c| c.git.provider)
        .unwrap_or(GitProvider::Local);

    // For github mode, refresh the human clone first so a concurrent remote
    // registration of the same handler is visible before we decide to reject.
    // Best-effort: network flakes degrade to the local file check rather than
    // blocking new agent creation.
    let human_dir = human_repo
        .unwrap_or_else(|| workspace.join(".gitim-runtime").join("human"));
    if git_provider == GitProvider::Github && human_dir.exists() {
        let fetch = std::process::Command::new("git")
            .args([
                "-c", "http.lowSpeedLimit=1000",
                "-c", "http.lowSpeedTime=10",
                "fetch", "origin",
            ])
            .current_dir(&human_dir)
            .output();
        if let Ok(o) = &fetch {
            if !o.status.success() {
                let stderr = String::from_utf8_lossy(&o.stderr);
                tracing::warn!(
                    handler = %req.handler,
                    stderr = %redacted_url(&stderr),
                    "git fetch before add_agent failed; proceeding with local state",
                );
            }
        } else if let Err(e) = &fetch {
            tracing::warn!(
                handler = %req.handler,
                error = %e,
                "git fetch before add_agent failed to spawn; proceeding with local state",
            );
        }
    }

    let meta_path = human_dir
        .join("users")
        .join(format!("{}.meta.yaml", req.handler));
    if meta_path.exists() {
        return Json(serde_json::json!({
            "ok": false,
            "error_code": "handler_conflict",
            "error": format!(
                "handler @{} already registered in this workspace",
                req.handler
            )
        }))
        .into_response();
    }

    let agents_dir = workspace.clone();
    if let Err(e) = std::fs::create_dir_all(&agents_dir) {
        return Json(serde_json::json!({
            "ok": false,
            "error": format!("failed to create agents dir: {e}")
        }))
        .into_response();
    }

    let remote_url = match git_provider {
        GitProvider::Local => workspace.join("repo.git").to_string_lossy().into_owned(),
        GitProvider::Github => {
            let cfg = match workspace_config.as_ref() {
                Some(c) => c,
                None => {
                    return Json(serde_json::json!({
                        "ok": false,
                        "error_code": "config_missing",
                        "error": "github mode requires workspace config with remote_url + token"
                    }))
                    .into_response();
                }
            };
            let remote = match cfg.git.remote_url.as_deref() {
                Some(u) if !u.is_empty() => u,
                _ => {
                    return Json(serde_json::json!({
                        "ok": false,
                        "error_code": "missing_remote_url",
                        "error": "workspace config lacks remote_url"
                    }))
                    .into_response();
                }
            };
            let token = match cfg.git.token.as_deref() {
                Some(t) if !t.is_empty() => t,
                _ => {
                    return Json(serde_json::json!({
                        "ok": false,
                        "error_code": "missing_token",
                        "error": "workspace config lacks token"
                    }))
                    .into_response();
                }
            };
            let (owner, repo_name) = match parse_github_url(remote) {
                Ok(t) => t,
                Err(e) => {
                    return Json(serde_json::json!({
                        "ok": false,
                        "error_code": github_error_code(&e),
                        "error": redacted_url(&e.to_string())
                    }))
                    .into_response();
                }
            };
            build_token_url(&owner, &repo_name, token)
        }
    };

    tracing::info!(
        handler = %req.handler,
        remote = %redacted_url(&remote_url),
        "provisioning agent",
    );

    let config = AgentConfig {
        handler: req.handler.clone(),
        display_name: req.display_name.clone(),
        remote_url,
    };

    match provision_agent(&agents_dir, &config).await {
        Ok(handle) => {
            // Recheck after async provision to prevent duplicate loops from concurrent requests
            {
                let s = state.lock().unwrap();
                if let Some(ctx) = s.workspaces.get(&slug) {
                    if ctx.agents.contains_key(&req.handler) {
                        return Json(serde_json::json!({
                            "ok": true,
                            "id": req.handler,
                        }))
                        .into_response();
                    }
                }
            }

            // Persist config to me.json
            let me_path = handle.repo_root.join(".gitim/me.json");
            if let Ok(content) = std::fs::read_to_string(&me_path) {
                if let Ok(mut me) = serde_json::from_str::<serde_json::Value>(&content) {
                    me["provider"] = serde_json::Value::String(req.provider.clone());
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
                // AgentInfo.provider stays Option<String> for back-compat with
                // old me.json files recovered at startup; new agents always
                // get Some(req.provider).
                provider: Some(req.provider.clone()),
                model: req.model.clone(),
                system_prompt: req.system_prompt.clone(),
                env: req.env.clone(),
                error_message: None,
                loop_handle: None,
            };
            {
                let mut s = state.lock().unwrap();
                if let Some(ctx) = s.workspaces.get_mut(&slug) {
                    ctx.agents.insert(req.handler.clone(), info);
                } else {
                    cleanup_agent_dir(&workspace, &req.handler);
                    return not_found_workspace();
                }
            }

            // Defensive: provision already stamped the new clone with the
            // current token, but resyncing here guarantees a single consistent
            // state if config.json was edited mid-provision.
            if let Err(e) = crate::token_propagation::propagate_token(&workspace) {
                tracing::warn!(error = %e, "token propagation after add_agent failed");
            }

            if let Err(e) = start_agent_loop(&state, &slug, &req.handler) {
                tracing::warn!("agent @{} created but auto-start failed: {e}", req.handler);
            }

            Json(serde_json::json!({ "ok": true, "id": req.handler })).into_response()
        }
        Err(e) => {
            cleanup_agent_dir(&workspace, &req.handler);
            Json(serde_json::json!({
                "ok": false,
                "error": redacted_url(&format!("provision_agent failed: {e}"))
            }))
            .into_response()
        }
    }
}

fn cleanup_agent_dir(workspace: &Path, handler: &str) {
    let agent_dir = workspace.join(handler);
    let pid_file = agent_dir.join(".gitim/run/gitim.pid");
    if let Ok(content) = std::fs::read_to_string(&pid_file) {
        if let Ok(pid) = content.trim().parse::<u32>() {
            let _ = std::process::Command::new("kill")
                .arg(pid.to_string())
                .output();
            std::thread::sleep(std::time::Duration::from_millis(500));
            let _ = std::process::Command::new("kill")
                .args(["-9", &pid.to_string()])
                .output();
        }
    }
    let _ = std::fs::remove_dir_all(&agent_dir);
}

// -- /agents --

async fn agents_list(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    match with_workspace_snapshot(&state, &slug, |ctx| {
        let agents: Vec<AgentInfo> = ctx.agents.values().cloned().collect();
        Json(serde_json::json!({ "ok": true, "agents": agents }))
    }) {
        Ok(j) => j.into_response(),
        Err(r) => r,
    }
}

// -- /agents/start --

#[derive(Deserialize)]
struct AgentIdRequest {
    id: String,
}

/// Start the agent loop for a given agent ID. Shared by add, start, and recover.
fn start_agent_loop(
    state: &SharedRuntimeState,
    slug: &str,
    agent_id: &str,
) -> Result<(), String> {
    let (repo_root, handler, provider, model, system_prompt, env, activity_tx) = {
        let s = state.lock().unwrap();
        let ctx = s
            .workspaces
            .get(slug)
            .ok_or_else(|| format!("unknown workspace: {slug}"))?;
        match ctx.agents.get(agent_id) {
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
                ctx.activity_tx.clone(),
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

    agent_loop.set_activity_tx_with_workspace(activity_tx, slug.to_string());

    let owned_id = agent_id.to_string();
    let owned_slug = slug.to_string();
    let state_clone = state.clone();

    let handle = tokio::spawn(async move {
        if let Err(e) = agent_loop.init().await {
            tracing::error!(error = %e, "agent loop init failed");
            let mut s = state_clone.lock().unwrap();
            if let Some(ctx) = s.workspaces.get_mut(&owned_slug) {
                if let Some(info) = ctx.agents.get_mut(&owned_id) {
                    info.loop_handle = None;
                    info.status = "error".to_string();
                }
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
                        if let Some(ctx) = s.workspaces.get_mut(&owned_slug) {
                            if let Some(info) = ctx.agents.get_mut(&owned_id) {
                                info.messages_processed += 1;
                                info.last_activity =
                                    Some(chrono::Utc::now().to_rfc3339());
                            }
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
        if let Some(ctx) = s.workspaces.get_mut(slug) {
            if let Some(info) = ctx.agents.get_mut(agent_id) {
                info.loop_handle = Some(abort_handle);
                info.status = "running".to_string();
            }
        }
    }

    Ok(())
}

async fn agents_start(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
    Json(req): Json<AgentIdRequest>,
) -> Json<serde_json::Value> {
    match start_agent_loop(&state, &slug, &req.id) {
        Ok(()) => Json(serde_json::json!({ "ok": true })),
        Err(e) => Json(serde_json::json!({ "ok": false, "error": e })),
    }
}

// -- /agents/:id --

async fn agents_get(
    State(state): State<SharedRuntimeState>,
    axum::extract::Path((slug, id)): axum::extract::Path<(String, String)>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if let Err(e) = crate::slug::validate(&slug) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": format!("invalid slug: {e}") })),
        )
            .into_response();
    }
    let s = state.lock().unwrap();
    let ctx = match s.workspaces.get(&slug) {
        Some(c) => c,
        None => return not_found_workspace(),
    };
    match ctx.agents.get(&id) {
        Some(info) => Json(serde_json::json!({ "ok": true, "agent": info })).into_response(),
        None => Json(serde_json::json!({ "ok": false, "error": "agent not found" })).into_response(),
    }
}

// -- /agents/remove --

async fn agents_remove(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
    Json(req): Json<AgentIdRequest>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let mut s = state.lock().unwrap();
    let ctx = match s.workspaces.get_mut(&slug) {
        Some(c) => c,
        None => return not_found_workspace(),
    };
    match ctx.agents.remove(&req.id) {
        Some(info) => {
            if let Some(handle) = &info.loop_handle {
                handle.abort();
            }
            let pid_file = PathBuf::from(&info.repo_path).join(".gitim/run/gitim.pid");
            if let Ok(content) = std::fs::read_to_string(&pid_file) {
                if let Ok(pid) = content.trim().parse::<u32>() {
                    let _ = std::process::Command::new("kill").arg(pid.to_string()).output();
                }
            }
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        None => Json(serde_json::json!({ "ok": false, "error": "agent not found" })).into_response(),
    }
}

// -- /agents/stop --

async fn agents_stop(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
    Json(req): Json<AgentIdRequest>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let abort_handle = {
        let mut s = state.lock().unwrap();
        let ctx = match s.workspaces.get_mut(&slug) {
            Some(c) => c,
            None => return not_found_workspace(),
        };
        match ctx.agents.get_mut(&req.id) {
            None => {
                return Json(serde_json::json!({
                    "ok": false,
                    "error": format!("agent not found: {}", req.id)
                }))
                .into_response();
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

    Json(serde_json::json!({ "ok": true })).into_response()
}

// -- /agents/events (SSE) --

async fn agents_events(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
) -> Result<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>, axum::response::Response> {
    let rx = with_workspace_snapshot(&state, &slug, |ctx| ctx.activity_tx.subscribe())?;

    let stream = tokio_stream::wrappers::BroadcastStream::new(rx).filter_map(|result| {
        futures::future::ready(match result {
            Ok(event) => {
                let data = serde_json::to_string(&event).unwrap_or_default();
                Some(Ok(SseEvent::default().data(data)))
            }
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(_)) => None,
        })
    });

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// Recover all workspaces listed in `~/.gitim/runtime.json` on startup. Each
/// workspace is recovered in its own task so one slow daemon doesn't stall
/// the rest.
pub async fn recover_from_config(state: SharedRuntimeState) {
    let cfg = crate::user_config::read();
    if cfg.workspaces.is_empty() {
        return;
    }
    tracing::info!("recovering {} workspace(s)", cfg.workspaces.len());

    let mut tasks = Vec::new();
    for entry in cfg.workspaces {
        let workspace = PathBuf::from(&entry.path);
        if !workspace.exists() {
            tracing::warn!(slug=%entry.slug, path=%entry.path, "workspace path missing; skip");
            continue;
        }
        let state = state.clone();
        tasks.push(tokio::spawn(async move {
            recover_single_workspace(state, entry.slug, entry.workspace_name, workspace).await;
        }));
    }
    for t in tasks {
        let _ = t.await;
    }
}

async fn recover_single_workspace(
    state: SharedRuntimeState,
    slug: String,
    workspace_name: String,
    workspace: PathBuf,
) {
    {
        let mut s = state.lock().unwrap();
        if s.workspaces.contains_key(&slug) {
            tracing::warn!(slug=%slug, "slug already present; skipping duplicate recovery");
            return;
        }
        let mut ctx = crate::workspace::WorkspaceContext::new(
            slug.clone(),
            workspace_name,
            workspace.clone(),
        );
        ctx.git_config = WorkspaceConfig::read(&workspace).ok();
        s.workspaces.insert(slug.clone(), ctx);
    }

    let human_dir = workspace.join(".gitim-runtime/human");
    if human_dir.exists() {
        let workspace_cfg = WorkspaceConfig::read(&workspace).ok();
        let (remote_url, git_server, auth) = match workspace_cfg.as_ref().map(|c| &c.git) {
            Some(GitConfig {
                provider: GitProvider::Github,
                remote_url: Some(url),
                token: Some(token),
            }) => {
                let token_url = match parse_github_url(url) {
                    Ok((owner, repo)) => build_token_url(&owner, &repo, token),
                    Err(_) => url.clone(),
                };
                (
                    token_url,
                    "github".to_string(),
                    serde_json::json!({ "type": "github", "token": token }),
                )
            }
            _ => {
                let remote = workspace.join("repo.git").to_string_lossy().into_owned();
                let display_name = detect_git_config("user.name", &workspace)
                    .unwrap_or_else(|| "human".to_string());
                let handler = {
                    let h = name_to_handler(&display_name);
                    if h.is_empty() {
                        "human".to_string()
                    } else {
                        h
                    }
                };
                (
                    remote,
                    "git".to_string(),
                    serde_json::json!({
                        "type": "git",
                        "handler": handler,
                        "display_name": display_name,
                    }),
                )
            }
        };
        match provision_human(&workspace, &remote_url, &git_server, auth).await {
            Ok(dir) => {
                let mut s = state.lock().unwrap();
                if let Some(ctx) = s.workspaces.get_mut(&slug) {
                    ctx.human_repo = Some(dir);
                }
                tracing::info!(slug=%slug, "human daemon recovered");
            }
            Err(e) => tracing::warn!(slug=%slug, error=%e, "failed to recover human daemon"),
        }
    }

    recover_agents_for_workspace(state, &slug, &workspace).await;
}

/// Scan a workspace directory for agent sub-dirs and recover each into
/// the workspace context for `slug`. Agents with a missing or unsupported
/// `provider` field in `me.json` are inserted in `status = "error"` and skip
/// daemon startup + loop auto-start — so broken configs don't stall the
/// recovery loop. The workspace context must already exist in state (the
/// caller inserts it before calling us).
pub async fn recover_agents_for_workspace(
    state: SharedRuntimeState,
    slug: &str,
    workspace: &Path,
) {
    let entries = match std::fs::read_dir(workspace) {
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

        let model = me["model"].as_str().map(|s| s.to_string());
        let custom_system_prompt = me["system_prompt"].as_str().map(|s| s.to_string());
        let env: HashMap<String, String> = me.get("env")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let provider_raw = me["provider"].as_str();
        let provider_error = match provider_raw {
            None => Some(format!(
                "Missing \"provider\" in {}. Add \"provider\": \"claude\" or \"provider\": \"codex\" to the file and restart the runtime.",
                me_path.display()
            )),
            Some(p) if p != "claude" && p != "codex" => Some(format!(
                "Unsupported provider \"{}\" in {}. Expected \"claude\" or \"codex\".",
                p,
                me_path.display()
            )),
            Some(_) => None,
        };

        if let Some(msg) = provider_error {
            tracing::warn!("agent @{handler} recovered in error state: {msg}");
            let activity_tx = {
                let s = state.lock().unwrap();
                s.workspaces.get(slug).expect("ws exists").activity_tx.clone()
            };
            let _ = activity_tx.send(AgentActivityEvent {
                agent_id: handler.clone(),
                workspace_id: slug.to_string(),
                event_type: "error".to_string(),
                detail: msg.clone(),
                timestamp: chrono::Utc::now().to_rfc3339(),
            });
            let mut s = state.lock().unwrap();
            s.workspaces.get_mut(slug).expect("ws exists").agents.insert(handler.clone(), AgentInfo {
                id: handler.clone(),
                handler: handler.clone(),
                display_name,
                status: "error".to_string(),
                last_activity: None,
                messages_processed: 0,
                repo_path: dir.display().to_string(),
                provider: provider_raw.map(|s| s.to_string()),
                model,
                system_prompt: custom_system_prompt,
                env,
                error_message: Some(msg),
                loop_handle: None,
            });
            continue;
        }

        let root = dir.clone();
        let log_path = crate::daemon_log::daemon_log_path(&dir);
        match tokio::task::spawn_blocking(move || {
            gitim_client::ensure_daemon_with_log(&root, &log_path)
        })
        .await
        {
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
            s.workspaces.get_mut(slug).expect("ws exists").agents.insert(handler.clone(), AgentInfo {
                id: handler.clone(),
                handler: handler.clone(),
                display_name,
                status: "idle".to_string(),
                last_activity: None,
                messages_processed: 0,
                repo_path: dir.display().to_string(),
                provider: provider_raw.map(|s| s.to_string()),
                model,
                system_prompt: custom_system_prompt,
                env,
                error_message: None,
                loop_handle: None,
            });
        }

        match start_agent_loop(&state, slug, &handler) {
            Ok(()) => tracing::info!("agent @{handler} recovered and started"),
            Err(e) => tracing::warn!("agent @{handler} recovered but auto-start failed: {e}"),
        }
    }
}

/// HTTP handler for `GET /preflight/{provider}`.
///
/// Dispatches to the matching provider's real-hello preflight. Unknown
/// providers return 400 with a stable `{"ok": false, "error": ...}` shape so
/// the WebUI can branch without parsing provider-specific fields.
async fn preflight_handler(
    axum::extract::Path(provider): axum::extract::Path<String>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    match provider.as_str() {
        "claude" => {
            let result = crate::preflight::preflight_claude().await;
            (StatusCode::OK, Json(result)).into_response()
        }
        "codex" => {
            let result = crate::preflight::preflight_codex().await;
            (StatusCode::OK, Json(result)).into_response()
        }
        _ => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "ok": false,
                "error": "unknown provider",
            })),
        )
            .into_response(),
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

// -- Global /workspaces CRUD --
//
// Multi-workspace entry points: list, create, read, delete. Writes
// `RuntimeState.workspaces` + `~/.gitim/runtime.json`.

#[derive(Deserialize)]
struct WorkspacesCreateGit {
    provider: String,
    #[serde(default)]
    remote_url: Option<String>,
    #[serde(default)]
    token: Option<String>,
}

#[derive(Deserialize)]
struct WorkspacesCreateRequest {
    path: String,
    #[serde(default)]
    workspace_name: Option<String>,
    git: WorkspacesCreateGit,
}

#[derive(Serialize)]
struct WorkspaceSummary {
    slug: String,
    workspace_name: String,
    path: String,
    provider: GitProvider,
    initialized: bool,
}

fn workspace_summary(ctx: &crate::workspace::WorkspaceContext) -> WorkspaceSummary {
    let provider = ctx
        .git_config
        .as_ref()
        .map(|c| c.git.provider)
        .unwrap_or(GitProvider::Local);
    WorkspaceSummary {
        slug: ctx.slug.clone(),
        workspace_name: ctx.workspace_name.clone(),
        path: ctx.path.to_string_lossy().into_owned(),
        provider,
        initialized: ctx.human_repo.is_some(),
    }
}

async fn workspaces_list(State(state): State<SharedRuntimeState>) -> Json<serde_json::Value> {
    let s = state.lock().unwrap();
    let mut list: Vec<WorkspaceSummary> = s.workspaces.values().map(workspace_summary).collect();
    // Deterministic order makes the response stable for tests and WebUI.
    list.sort_by(|a, b| a.slug.cmp(&b.slug));
    Json(serde_json::json!({ "workspaces": list }))
}

async fn workspaces_get(
    State(state): State<SharedRuntimeState>,
    axum::extract::Path(slug): axum::extract::Path<String>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    let s = state.lock().unwrap();
    match s.workspaces.get(&slug) {
        Some(ctx) => {
            let provider = ctx
                .git_config
                .as_ref()
                .map(|c| c.git.provider)
                .unwrap_or(GitProvider::Local);
            let body = serde_json::json!({
                "slug": ctx.slug,
                "workspace_name": ctx.workspace_name,
                "path": ctx.path.to_string_lossy(),
                "provider": provider,
                "initialized": ctx.human_repo.is_some(),
                "agents_count": ctx.agents.len(),
                "human_repo": ctx.human_repo.as_ref().map(|p| p.to_string_lossy().into_owned()),
            });
            Json(body).into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "ok": false, "error": "unknown workspace" })),
        )
            .into_response(),
    }
}

/// Remove the workspace entry from memory and the user config file.
/// On-disk user files at `workspace` root are preserved — only `.gitim-runtime/`
/// artifacts are cleaned by the daemon-kill path. If the config file write
/// fails the caller is told (500), because the API would otherwise lie about
/// durable state (workspace gone from memory, resurrects on restart).
async fn workspaces_delete(
    State(state): State<SharedRuntimeState>,
    axum::extract::Path(slug): axum::extract::Path<String>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    let mut removed = {
        let mut s = state.lock().unwrap();
        match s.workspaces.remove(&slug) {
            Some(ctx) => ctx,
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({ "ok": false, "error": "unknown workspace" })),
                )
                    .into_response();
            }
        }
    };

    // Abort in-process agent loop tasks before killing their daemons. Mirrors
    // the cleanup `/agents/remove` and `/agents/stop` already perform — without
    // this the tokio tasks survive workspace removal and keep polling repos
    // whose daemons are gone (silently erroring forever).
    for agent in removed.agents.values_mut() {
        if let Some(handle) = agent.loop_handle.take() {
            handle.abort();
        }
    }

    crate::workspace::kill_daemons(&removed).await;

    let mut cfg = crate::user_config::read();
    if cfg.remove(&slug) {
        if let Err(e) = crate::user_config::write(&cfg) {
            tracing::error!(slug = %slug, error = %e, "failed to persist workspace removal");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "ok": false,
                    "error_code": "config_write_failed",
                    "error": format!(
                        "workspace removed from memory and daemons stopped, but ~/.gitim/runtime.json write failed: {e}. Next runtime start will try to recover this workspace.",
                    ),
                })),
            )
                .into_response();
        }
    }

    Json(serde_json::json!({ "ok": true })).into_response()
}

/// Best-effort rollback for a failed `POST /workspaces`. Kills any daemon the
/// partial provisioning started, then removes `.gitim-runtime/` (which holds
/// `human/` + any token-carrying config). We do NOT delete user-owned files
/// at `workspace` root (e.g. the local bare `repo.git`) — those existed before
/// we touched the directory or were created by us but are safe to leave; the
/// plan's "file hygiene" rule is to preserve local files.
fn cleanup_partial_workspace(workspace: &Path) {
    cleanup_human_dir(workspace);
    let runtime_dir = workspace.join(".gitim-runtime");
    let _ = std::fs::remove_dir_all(&runtime_dir);
}

/// Provision a local-mode workspace: init bare at `{path}/repo.git` and run
/// `provision_human`. Mirrors `git_init_local` but operates on an arbitrary
/// workspace path (not the legacy singleton) and returns the provisioned
/// `human_dir` + a `WorkspaceConfig` instead of mutating `RuntimeState`
/// directly. Returns `Err((error_code, message))` — the HTTP layer maps those
/// into the standard `{ ok: false, error_code, error }` body.
async fn provision_local_workspace(
    workspace: &Path,
) -> Result<(PathBuf, WorkspaceConfig), (&'static str, String)> {
    let repo_path = workspace.join("repo.git");
    std::fs::create_dir_all(&repo_path).map_err(|e| {
        (
            "clone_failed",
            redacted_url(&format!("failed to create repo directory: {e}")),
        )
    })?;

    let output = std::process::Command::new("git")
        .args(["init", "--bare"])
        .current_dir(&repo_path)
        .output()
        .map_err(|e| ("clone_failed", redacted_url(&format!("failed to run git: {e}"))))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err((
            "clone_failed",
            redacted_url(&format!("git init failed: {stderr}")),
        ));
    }

    let remote_url = repo_path.to_string_lossy().into_owned();
    let display_name =
        detect_git_config("user.name", workspace).unwrap_or_else(|| "human".to_string());
    let handler = {
        let h = name_to_handler(&display_name);
        if h.is_empty() { "human".to_string() } else { h }
    };
    let auth = serde_json::json!({
        "type": "git",
        "handler": handler,
        "display_name": display_name,
    });

    let human_dir = provision_human(workspace, &remote_url, "git", auth)
        .await
        .map_err(|e| {
            (
                "onboard_failed",
                redacted_url(&format!("provision_human failed: {e}")),
            )
        })?;

    let config = WorkspaceConfig {
        workspace: workspace.to_string_lossy().into_owned(),
        created_at: chrono::Utc::now().to_rfc3339(),
        git: GitConfig {
            provider: GitProvider::Local,
            remote_url: None,
            token: None,
        },
    };
    config.write(workspace).map_err(|e| {
        (
            "config_write_failed",
            redacted_url(&format!("failed to write config: {e}")),
        )
    })?;
    let _ = mark_excluded_from_backups(&workspace.join(".gitim-runtime"));

    Ok((human_dir, config))
}

/// Provision a github-mode workspace: verify token → check repo access →
/// clone → `provision_human`. Mirrors `git_init_github` but targets an
/// arbitrary workspace path. Windows is rejected consistent with the
/// workspace-github-mode scope.
async fn provision_github_workspace(
    state: &SharedRuntimeState,
    workspace: &Path,
    remote_url: String,
    token: String,
) -> Result<(PathBuf, WorkspaceConfig), (&'static str, String)> {
    #[cfg(windows)]
    {
        let _ = (state, workspace, remote_url, token);
        return Err((
            "provider_not_supported",
            "github mode is not supported on Windows".to_string(),
        ));
    }
    #[cfg(not(windows))]
    {
        let (github_api, clone_override) = {
            let s = state.lock().unwrap();
            (s.github_api.clone(), s.clone_url_override.clone())
        };

        github_api
            .verify_token(&token)
            .await
            .map_err(|e| (github_error_code(&e), redacted_url(&e.to_string())))?;

        let (owner, repo) = parse_github_url(&remote_url)
            .map_err(|e| (github_error_code(&e), redacted_url(&e.to_string())))?;

        github_api
            .check_repo_access(&owner, &repo, &token)
            .await
            .map_err(|e| (github_error_code(&e), redacted_url(&e.to_string())))?;

        let clone_url = clone_override
            .clone()
            .unwrap_or_else(|| build_token_url(&owner, &repo, &token));

        let runtime_dir = workspace.join(".gitim-runtime");
        std::fs::create_dir_all(&runtime_dir).map_err(|e| {
            (
                "clone_failed",
                redacted_url(&format!("failed to create runtime dir: {e}")),
            )
        })?;

        let human_dir = runtime_dir.join("human");
        if human_dir.exists() {
            // Prior failed provisioning may have left a half-built clone;
            // `provision_human` is not re-entrant over partial state.
            cleanup_human_dir(workspace);
        }

        let clone_output = std::process::Command::new("git")
            .args(["clone", &clone_url, "human"])
            .current_dir(&runtime_dir)
            .output()
            .map_err(|e| {
                cleanup_human_dir(workspace);
                ("clone_failed", redacted_url(&format!("failed to run git: {e}")))
            })?;
        if !clone_output.status.success() {
            let stderr = String::from_utf8_lossy(&clone_output.stderr);
            cleanup_human_dir(workspace);
            return Err((
                "clone_failed",
                redacted_url(&format!("git clone failed: {stderr}")),
            ));
        }

        // Scrub the token from the clone's origin URL so `git remote -v`
        // and any diagnostic dump stop leaking it. When an override is
        // active (e2e tests point at a `file://` bare) skip this — that URL
        // never carried a token to begin with.
        if clone_override.is_none() {
            let _ = std::process::Command::new("git")
                .args(["remote", "set-url", "origin", &remote_url])
                .current_dir(&human_dir)
                .output();
        }

        let auth = serde_json::json!({
            "type": "github",
            "token": token,
        });
        let final_human = provision_human(workspace, &remote_url, "github", auth)
            .await
            .map_err(|e| {
                cleanup_human_dir(workspace);
                (
                    "onboard_failed",
                    redacted_url(&format!("provision_human failed: {e}")),
                )
            })?;

        let config = WorkspaceConfig {
            workspace: workspace.to_string_lossy().into_owned(),
            created_at: chrono::Utc::now().to_rfc3339(),
            git: GitConfig {
                provider: GitProvider::Github,
                remote_url: Some(remote_url.clone()),
                token: Some(token.clone()),
            },
        };
        config.write(workspace).map_err(|e| {
            cleanup_human_dir(workspace);
            (
                "config_write_failed",
                redacted_url(&format!("failed to write config: {e}")),
            )
        })?;
        let _ = mark_excluded_from_backups(&runtime_dir);

        Ok((final_human, config))
    }
}

async fn workspaces_create(
    State(state): State<SharedRuntimeState>,
    Json(req): Json<WorkspacesCreateRequest>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    let workspace = PathBuf::from(&req.path);

    // Path validation: only cloud-sync rejection today. Do this before
    // touching `state` so concurrent callers with bad paths fail fast and
    // don't race for a slug.
    if let Err(crate::git_config::WorkspacePathError::CloudSyncDetected(service)) =
        validate_workspace_path_from_env(&workspace)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "ok": false,
                "error_code": "cloud_sync_path_rejected",
                "error": format!("workspace is inside {service} — refusing to store a token there"),
            })),
        )
            .into_response();
    }

    // `workspace_name` defaults to the basename *as-is* (case/spaces/unicode
    // preserved) so the UI can show a human-friendly label even when the slug
    // is the ASCII-only normalized form.
    let basename_raw = workspace
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(String::new);
    let workspace_name = req
        .workspace_name
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| basename_raw.clone());

    // TOCTOU-safe slug reservation: path-uniqueness check + slug derivation +
    // placeholder insertion all happen under the same lock. Without this, a
    // second POST for an already-registered path would allocate a fresh slug,
    // and a provisioning failure would run `cleanup_partial_workspace` against
    // the shared directory — killing the live workspace's daemon and deleting
    // its `.gitim-runtime/` tree.
    let slug = {
        let mut s = state.lock().unwrap();

        if let Some(existing) = s.workspaces.values().find(|w| w.path == workspace) {
            let existing_slug = existing.slug.clone();
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "ok": false,
                    "error_code": "workspace_path_exists",
                    "error": format!(
                        "workspace at {} already registered as slug \"{}\"",
                        workspace.display(), existing_slug,
                    ),
                    "existing_slug": existing_slug,
                })),
            )
                .into_response();
        }

        let candidate = crate::slug::normalize(&basename_raw);
        let existing: std::collections::HashSet<String> = s.workspaces.keys().cloned().collect();
        let slug = crate::slug::resolve(&candidate, &existing);

        if s.workspaces.contains_key(&slug) {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "ok": false,
                    "error_code": "slug_conflict_unexpected",
                    "error": format!("slug collision not resolved: {slug}"),
                })),
            )
                .into_response();
        }
        let placeholder = crate::workspace::WorkspaceContext::new(
            slug.clone(),
            workspace_name.clone(),
            workspace.clone(),
        );
        s.workspaces.insert(slug.clone(), placeholder);
        slug
    };

    // Async provisioning runs without the state lock held. On any failure
    // below we must re-lock and drop the placeholder so a retry can succeed.
    let provider_str = req.git.provider.as_str();
    let provisioned = match provider_str {
        "local" => provision_local_workspace(&workspace).await,
        "github" => {
            let token = match req.git.token.as_ref() {
                Some(t) if !t.is_empty() => t.clone(),
                _ => {
                    state.lock().unwrap().workspaces.remove(&slug);
                    cleanup_partial_workspace(&workspace);
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "ok": false,
                            "error_code": "missing_token",
                            "error": "github mode requires a personal access token",
                        })),
                    )
                        .into_response();
                }
            };
            let remote_url = match req.git.remote_url.as_ref() {
                Some(u) if !u.is_empty() => u.clone(),
                _ => {
                    state.lock().unwrap().workspaces.remove(&slug);
                    cleanup_partial_workspace(&workspace);
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "ok": false,
                            "error_code": "missing_remote_url",
                            "error": "github mode requires remote_url",
                        })),
                    )
                        .into_response();
                }
            };
            provision_github_workspace(&state, &workspace, remote_url, token).await
        }
        other => {
            state.lock().unwrap().workspaces.remove(&slug);
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "ok": false,
                    "error_code": "provider_not_supported",
                    "error": format!("provider not supported: {other}"),
                })),
            )
                .into_response();
        }
    };

    let (human_dir, config) = match provisioned {
        Ok(x) => x,
        Err((error_code, message)) => {
            state.lock().unwrap().workspaces.remove(&slug);
            cleanup_partial_workspace(&workspace);
            // All provisioning failures surface as 400: they're all "your input
            // or environment caused this" (bad token, bad URL, clone failed).
            // None are 500-class — the runtime itself is still fine.
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "ok": false,
                    "error_code": error_code,
                    "error": message,
                })),
            )
                .into_response();
        }
    };

    // Success: fill the placeholder with real provisioning results, then
    // persist to ~/.gitim/runtime.json so the workspace survives a restart.
    let provider_for_response;
    {
        let mut s = state.lock().unwrap();
        match s.workspaces.get_mut(&slug) {
            Some(ctx) => {
                ctx.human_repo = Some(human_dir);
                ctx.git_config = Some(config.clone());
            }
            None => {
                // Extremely unlikely — would mean a DELETE raced in during
                // provisioning. Roll back the filesystem side and fail.
                cleanup_partial_workspace(&workspace);
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "ok": false,
                        "error_code": "slug_conflict_unexpected",
                        "error": "workspace slot disappeared during provisioning",
                    })),
                )
                    .into_response();
            }
        }
        provider_for_response = config.git.provider;
    }

    let mut user_cfg = crate::user_config::read();
    user_cfg.upsert(crate::user_config::WorkspaceEntry {
        slug: slug.clone(),
        workspace_name: workspace_name.clone(),
        path: workspace.to_string_lossy().into_owned(),
    });
    if let Err(e) = crate::user_config::write(&user_cfg) {
        tracing::error!(slug = %slug, error = %e, "failed to persist workspace entry");
        state.lock().unwrap().workspaces.remove(&slug);
        cleanup_partial_workspace(&workspace);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "ok": false,
                "error_code": "config_write_failed",
                "error": format!("workspace provisioned but ~/.gitim/runtime.json write failed: {e}"),
            })),
        )
            .into_response();
    }

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "ok": true,
            "slug": slug,
            "workspace_name": workspace_name,
            "path": workspace.to_string_lossy(),
            "provider": provider_for_response,
        })),
    )
        .into_response()
}

/// Assemble the axum router with a fresh `RuntimeState`. The canonical exe
/// path is resolved from `RuntimeState::default()` — fine for tests, but
/// production must call `create_router_with_exe` so the pre-replacement
/// binary path is captured before any self-update can happen.
pub fn create_router() -> (Router, SharedRuntimeState) {
    build_router(Arc::new(Mutex::new(RuntimeState::default())))
}

/// Production entry point: caller supplies the canonical exe path captured
/// at startup (before any binary self-replace). Task 6/7 self-update reads
/// this from `state.canonical_exe_path`.
pub fn create_router_with_exe(canonical_exe_path: PathBuf) -> (Router, SharedRuntimeState) {
    let inner = RuntimeState {
        canonical_exe_path,
        ..RuntimeState::default()
    };
    build_router(Arc::new(Mutex::new(inner)))
}

fn build_router(state: SharedRuntimeState) -> (Router, SharedRuntimeState) {
    let ws_router = Router::new()
        .route("/im/me", get(im_me))
        .route("/im/channels", get(im_channels))
        .route("/im/create-channel", post(im_create))
        .route("/im/join", post(im_join))
        .route("/im/send", post(im_send))
        .route("/im/read", post(im_read))
        .route("/im/poll", post(im_poll))
        .route("/im/users", get(im_users))
        .route("/im/thread", post(im_thread))
        .route("/im/cards", post(im_create_card).get(im_list_cards))
        // `/im/cards/archived` must come before `/im/cards/{channel}/{card_id}`
        // so axum doesn't try to match "archived" as a channel segment.
        .route("/im/cards/archived", get(im_list_archived_cards))
        .route(
            "/im/cards/{channel}/{card_id}",
            get(im_read_card).patch(im_update_card),
        )
        .route(
            "/im/cards/{channel}/{card_id}/messages",
            post(im_send_card_message),
        )
        .route("/im/cards/{channel}/{card_id}/archive", post(im_card_archive))
        .route("/im/cards/{channel}/{card_id}/unarchive", post(im_card_unarchive))
        .route("/im/channels/archived", get(im_list_archived_channels))
        .route("/im/channels/{name}/archive", post(im_channel_archive))
        .route("/im/channels/{name}/unarchive", post(im_channel_unarchive))
        .route("/agents", get(agents_list))
        .route("/agents/events", get(agents_events))
        .route("/agents/add", post(agents_add))
        .route("/agents/start", post(agents_start))
        .route("/agents/stop", post(agents_stop))
        .route("/agents/remove", post(agents_remove))
        .route("/agents/{id}", get(agents_get));

    let router = Router::new()
        .route("/health", get(health))
        .route(
            "/workspaces",
            get(workspaces_list).post(workspaces_create),
        )
        .route(
            "/workspaces/{slug}",
            get(workspaces_get).delete(workspaces_delete),
        )
        .nest("/workspaces/{slug}", ws_router)
        .route("/preflight/{provider}", get(preflight_handler))
        .route(
            "/runtime/update-and-restart",
            post(crate::update::update_and_restart),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            activity_middleware,
        ))
        .layer(CorsLayer::permissive())
        .with_state(state.clone());

    (router, state)
}

#[cfg(test)]
mod tests {
    //! Unit tests for the `/workspaces` request/response types (Task 5).
    //! Full HTTP integration coverage — lifecycle with real filesystem,
    //! slug collisions, 404s, error bodies — lives in
    //! `tests/http_workspaces.rs` (Task 10).

    use super::*;

    #[test]
    fn workspaces_create_request_deserializes_local() {
        let body = serde_json::json!({
            "path": "/tmp/ws",
            "workspace_name": "My Workspace",
            "git": { "provider": "local" }
        });
        let req: WorkspacesCreateRequest = serde_json::from_value(body).unwrap();
        assert_eq!(req.path, "/tmp/ws");
        assert_eq!(req.workspace_name.as_deref(), Some("My Workspace"));
        assert_eq!(req.git.provider, "local");
        assert!(req.git.token.is_none());
        assert!(req.git.remote_url.is_none());
    }

    #[test]
    fn workspaces_create_request_defaults_workspace_name() {
        let body = serde_json::json!({
            "path": "/tmp/ws",
            "git": { "provider": "local" }
        });
        let req: WorkspacesCreateRequest = serde_json::from_value(body).unwrap();
        assert!(req.workspace_name.is_none());
    }

    #[test]
    fn workspaces_create_request_deserializes_github() {
        let body = serde_json::json!({
            "path": "/tmp/ws",
            "git": {
                "provider": "github",
                "remote_url": "https://github.com/org/repo",
                "token": "ghp_x"
            }
        });
        let req: WorkspacesCreateRequest = serde_json::from_value(body).unwrap();
        assert_eq!(req.git.provider, "github");
        assert_eq!(req.git.remote_url.as_deref(), Some("https://github.com/org/repo"));
        assert_eq!(req.git.token.as_deref(), Some("ghp_x"));
    }

    #[test]
    fn workspace_summary_round_trips() {
        let summary = WorkspaceSummary {
            slug: "frontend".to_string(),
            workspace_name: "Frontend".to_string(),
            path: "/ws/frontend".to_string(),
            provider: GitProvider::Local,
            initialized: false,
        };
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["slug"], "frontend");
        assert_eq!(json["workspace_name"], "Frontend");
        assert_eq!(json["path"], "/ws/frontend");
        assert_eq!(json["provider"], "local");
        assert_eq!(json["initialized"], false);
    }

    #[test]
    fn workspace_summary_derives_provider_from_git_config() {
        let mut ctx = crate::workspace::WorkspaceContext::new(
            "fe".to_string(),
            "FE".to_string(),
            PathBuf::from("/ws/fe"),
        );
        ctx.git_config = Some(WorkspaceConfig {
            workspace: "/ws/fe".to_string(),
            created_at: "2026-04-18T00:00:00Z".to_string(),
            git: GitConfig {
                provider: GitProvider::Github,
                remote_url: Some("https://github.com/o/r".to_string()),
                token: Some("tok".to_string()),
            },
        });
        ctx.human_repo = Some(PathBuf::from("/ws/fe/.gitim-runtime/human"));
        let summary = workspace_summary(&ctx);
        assert_eq!(summary.slug, "fe");
        assert_eq!(summary.provider, GitProvider::Github);
        assert!(summary.initialized);
    }

    #[test]
    fn workspace_summary_defaults_provider_when_config_missing() {
        let ctx = crate::workspace::WorkspaceContext::new(
            "fe".to_string(),
            "FE".to_string(),
            PathBuf::from("/ws/fe"),
        );
        let summary = workspace_summary(&ctx);
        assert_eq!(summary.provider, GitProvider::Local);
        assert!(!summary.initialized);
    }

    #[tokio::test]
    async fn workspaces_get_returns_404_for_unknown_slug() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let (router, _state) = create_router();
        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/workspaces/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["ok"], false);
        assert_eq!(body["error"], "unknown workspace");
    }

    #[tokio::test]
    async fn workspaces_delete_returns_404_for_unknown_slug() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let (router, _state) = create_router();
        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/workspaces/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["ok"], false);
        assert_eq!(body["error"], "unknown workspace");
    }

    #[tokio::test]
    async fn workspaces_list_empty() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let (router, _state) = create_router();
        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/workspaces")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["workspaces"], serde_json::json!([]));
    }
}
