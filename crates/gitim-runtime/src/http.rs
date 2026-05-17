use async_trait::async_trait;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::{
    extract::State,
    routing::{delete, get, post},
    Json, Router,
};
use futures::stream::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::task::AbortHandle;
use tower_http::cors::CorsLayer;

use crate::agent::{
    detect_git_config, infer_local_human_identity, name_to_handler, provision_agent,
    provision_human, AgentConfig,
};
use crate::agent_loop::{is_daemon_not_running_poll_error, AgentLoop};
use crate::git_config::{
    mark_excluded_from_backups, validate_workspace_path_from_env, GitConfig, GitProvider,
    WorkspaceConfig,
};
use crate::github::{
    check_repo_access, fetch_user_email, parse_github_url, verify_token, GithubError,
};
use crate::gitignore::ensure_defaults_gitignored;
use gitim_client::{ensure_daemon_with_log, ClientError, GitimClient};
use gitim_core::me_json::MeJson;
use gitim_core::types::{UserMeta, MAX_INTRODUCTION_LEN};
use gitim_sync::url_redact::redacted_url;

/// Default TCP port for the runtime HTTP server. Shared between
/// `RuntimeState::default()` and `bin/runtime.rs`'s argv parser so the two
/// can't drift. Chosen to sit well above the IANA registered range and out
/// of the ephemeral-port band on macOS / Linux.
pub const DEFAULT_PORT: u16 = 16868;

/// Max bytes accepted for the `dotenv` field on `PATCH /agents/{id}`.
/// Typical `.env` is < 1 KB; cap is generous headroom without enabling abuse.
pub const DOTENV_MAX_BYTES: usize = 64 * 1024;

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
    async fn fetch_user_email(&self, token: &str) -> Result<Option<String>, GithubError>;
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
    async fn fetch_user_email(&self, token: &str) -> Result<Option<String>, GithubError> {
        fetch_user_email(token, &self.base_url).await
    }
}

#[derive(Serialize)]
struct HealthResponse {
    service: &'static str,
    version: &'static str,
    workspaces_count: usize,
    runtime_id: String,
    /// Best-effort observability for the token statistics layer. Increments
    /// every time the agent loop fails to persist a usage log. v1 has no
    /// alerting on this; the field exists so a flaky filesystem shows up
    /// without scraping logs.
    usage_save_failures: u64,
}

// -----------------------------------------------------------------------------
// Typed response shapes.
//
// One Response struct per success path; ErrorBody for every failure path
// (the legacy `Json(serde_json::json!({"ok": false, "error": ...}))` shape).
// Renaming a wire field anywhere in this section breaks the build at every
// call site — that's the point.
// -----------------------------------------------------------------------------

/// Shared error response body. `ok` is always `false` — Serialize via a
/// const associated function so callers can't construct an `ok: true` mistake.
///
/// `preflight_detail` is set only by [`Self::with_preflight`] — used by
/// `agents_add` (T6) to nest the full [`crate::preflight::PreflightResult`]
/// under a provisioning failure so the WebUI / CLI can render structured
/// detail (which binary, what error_kind, stdout preview) without a second
/// roundtrip. Skipped from serialization when `None` so every other error
/// path keeps its existing two-field shape.
#[derive(Serialize)]
struct ErrorBody {
    ok: bool,
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    preflight_detail: Option<crate::preflight::PreflightResult>,
}

impl ErrorBody {
    fn new(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: error.into(),
            error_code: None,
            preflight_detail: None,
        }
    }

    fn with_code(error: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: error.into(),
            error_code: Some(code.into()),
            preflight_detail: None,
        }
    }

    /// Error body carrying a nested `PreflightResult`. Used by `agents_add`
    /// when provisioning aborts because the per-agent preflight came back
    /// `available: false` — the caller maps `result.failure_code` through
    /// `classify_preflight_error_code` for the top-level `error_code`, and
    /// threads the raw result through so the UI can show e.g. which binary
    /// was missing or which model failed.
    fn with_preflight(
        error: impl Into<String>,
        code: impl Into<String>,
        detail: crate::preflight::PreflightResult,
    ) -> Self {
        Self {
            ok: false,
            error: error.into(),
            error_code: Some(code.into()),
            preflight_detail: Some(detail),
        }
    }
}

#[derive(Serialize)]
struct WorkspacesListResponse {
    workspaces: Vec<WorkspaceSummary>,
}

/// Single-workspace detail. Differs from `WorkspaceSummary` (which is the
/// list-row shape) by adding `agents_count` and `human_repo`.
#[derive(Serialize)]
struct WorkspaceDetailResponse {
    slug: String,
    workspace_name: String,
    path: String,
    provider: GitProvider,
    initialized: bool,
    agents_count: usize,
    human_repo: Option<String>,
}

/// `{"ok": true}` ack for `DELETE /workspaces/{slug}`.
#[derive(Serialize)]
struct OkAckResponse {
    ok: bool,
}

#[derive(Serialize)]
struct FleetNodesListResponse {
    ok: bool,
    nodes: Vec<crate::user_config::FleetNodeEntry>,
}

#[derive(Serialize)]
struct FleetStatusResponse {
    ok: bool,
    nodes: Vec<crate::fleet::FleetNodeStatus>,
}

#[derive(Serialize)]
struct FleetAgentsResponse {
    ok: bool,
    agents: Vec<crate::fleet::FleetAgentSnapshot>,
}

#[derive(Serialize)]
struct FleetNodeUpsertResponse {
    ok: bool,
    node: crate::user_config::FleetNodeEntry,
}

/// `POST /workspaces` success body. Wire keeps `ok: true` inline because
/// pre-typed callers parse `obj.get("slug")` from the same dict.
#[derive(Serialize)]
struct WorkspaceCreateResponse {
    ok: bool,
    slug: String,
    workspace_name: String,
    path: String,
    provider: GitProvider,
}

#[derive(Serialize)]
struct ImMeData {
    handler: String,
    display_name: String,
    guest: bool,
}

#[derive(Serialize)]
struct ImMeResponse {
    ok: bool,
    data: ImMeData,
}

/// 409 `workspace_path_exists` error — carries the slug of the live
/// workspace already pinned to that path so the caller can show a useful
/// message. Different shape from `ErrorBody` because it has the extra
/// `existing_slug` field.
#[derive(Serialize)]
struct WorkspacePathExistsError {
    ok: bool,
    error_code: &'static str,
    error: String,
    existing_slug: String,
}

#[derive(Serialize)]
struct AgentsListResponse {
    ok: bool,
    agents: Vec<AgentInfo>,
}

#[derive(Serialize)]
struct AgentDetailResponse {
    ok: bool,
    agent: AgentInfo,
}

/// `POST /agents/add` success — `id` is the agent handler that was
/// created (echo of `req.handler`).
#[derive(Serialize)]
struct AgentAddResponse {
    ok: bool,
    id: String,
}

/// Real-time agent activity event, broadcast via SSE.
///
/// `workspace_id` always carries the originating workspace's slug so SSE
/// subscribers can route or filter events. Events are published on the
/// workspace-scoped `broadcast::Sender` held in `WorkspaceContext`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentActivityEvent {
    pub agent_id: String,
    pub workspace_id: String,
    pub event_type: String, // "tool_use", "thinking", "done", "error", "usage", "burned"
    pub detail: String,
    pub timestamp: String, // ISO8601
}

#[derive(Clone, Debug, Serialize, Deserialize)]
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
    /// Free-form blurb the WebUI shows on the agent card and detail page.
    /// Sourced from `users/<handler>.meta.yaml::introduction` — i.e. the
    /// git-synced user metadata file, NOT `.gitim/me.json`. None at the
    /// type level lets recovery paths skip the disk read for legacy
    /// agents whose meta.yaml predates this field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub introduction: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    /// Last-known usage snapshot for the agent's current provider session.
    /// Populated at recovery from `.gitim/agent-state.json` and patched in
    /// place by `AgentLoop::update_session_usage` after every turn — so
    /// `GET /agents/:id` returns fresh data without re-reading disk.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_usage: Option<crate::state::SessionUsageSnapshot>,
    /// Hermes-only: the selected LLM provider id (e.g. "deepseek", "custom:myendpoint").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_provider: Option<String>,
    /// Hermes-only: the selected LLM model id (e.g. "deepseek-chat").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_model: Option<String>,
    /// Cumulative + 30-day breakdown of token usage. Loaded at recovery
    /// from `<workspace>/.gitim-runtime/usage/<handler>.json` and patched in
    /// place by the agent loop after each turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_summary: Option<crate::usage_log::UsageSummary>,
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
    pub fleet_tx: tokio::sync::broadcast::Sender<crate::fleet::FleetEventEnvelope>,
    pub fleet_nodes: HashMap<String, crate::fleet::FleetNodeRuntime>,
    pub fleet_status: HashMap<String, crate::fleet::FleetNodeStatus>,
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
    /// Stable device-bound UUID for this runtime install. Once-write at
    /// startup by `bin/runtime.rs::run_shell()` from
    /// `user_config::ensure_runtime_id`; read-only thereafter. Empty
    /// string when constructed via `Default::default()` for tests that
    /// don't go through the boot path. See docs/plans/runtime-id/00-design.md.
    pub runtime_id: String,
    /// Counter incremented every time `AgentUsageLog::save` returns an
    /// error from the agent loop. Surfaced on `/runtime/health` so a
    /// repeatedly-failing FS shows up without scraping logs. Best-effort
    /// observability — no alerting / threshold logic in v1.
    pub usage_save_failures: std::sync::atomic::AtomicU64,
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
        // directly. A placeholder at `/nonexistent/gitim-runtime` keeps the
        // self-update endpoint safe: it refuses to replace a missing path.
        let canonical_exe_path = std::env::current_exe()
            .and_then(|p| p.canonicalize())
            .unwrap_or_else(|_| PathBuf::from("/nonexistent/gitim-runtime"));
        let (fleet_tx, _) = tokio::sync::broadcast::channel(256);
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
            fleet_tx,
            fleet_nodes: HashMap::new(),
            fleet_status: HashMap::new(),
            canonical_exe_path,
            update_in_progress: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            update_last_error: None,
            listen_port: DEFAULT_PORT,
            runtime_id: String::new(),
            usage_save_failures: std::sync::atomic::AtomicU64::new(0),
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
        runtime_id: s.runtime_id.clone(),
        usage_save_failures: s
            .usage_save_failures
            .load(std::sync::atomic::Ordering::Relaxed),
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
                Json(ErrorBody::new(format!("invalid slug: {e}"))),
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
        Json(ErrorBody::new("unknown workspace")),
    )
        .into_response()
}

/// Used by `/im/*` routes when the workspace exists but `human_repo`
/// isn't wired up yet (initial provisioning never finished). Returns
/// 200 with the standard daemon-error body shape — same convention as
/// `api_response_to_json` for daemon-side `Response::error`s, so the
/// WebUI can branch on `body.ok` without status-code-aware code.
fn human_not_initialized() -> axum::response::Response {
    use axum::response::IntoResponse;
    Json(ErrorBody::new("human daemon not initialized")).into_response()
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

fn persistent_human_repo(workspace: &Path) -> Option<PathBuf> {
    let human_dir = workspace.join(".gitim-runtime").join("human");
    if human_dir.join(".git").is_dir() && human_dir.join(".gitim").join("me.json").is_file() {
        Some(human_dir)
    } else {
        None
    }
}

fn workspace_initialized(ctx: &crate::workspace::WorkspaceContext) -> bool {
    ctx.human_repo.is_some() || persistent_human_repo(&ctx.path).is_some()
}

fn human_repo_path(ctx: &crate::workspace::WorkspaceContext) -> Option<PathBuf> {
    ctx.human_repo
        .clone()
        .or_else(|| persistent_human_repo(&ctx.path))
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
        None if persistent_human_repo(&ctx.path).is_some() => Err((
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorBody::new("human daemon unavailable")),
        )
            .into_response()),
        None => Err(human_not_initialized()),
    }
}

async fn ensure_daemon_with_runtime_log(repo_root: PathBuf) -> Result<(), ClientError> {
    let log_path = crate::daemon_log::daemon_log_path(&repo_root);
    tokio::task::spawn_blocking(move || ensure_daemon_with_log(&repo_root, &log_path))
        .await
        .map_err(|e| ClientError::ConnectionFailed(format!("daemon restart task panicked: {e}")))?
}

fn api_response_to_json(
    result: Result<gitim_client::ApiResponse, gitim_client::ClientError>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    match result {
        // ApiResponse serializes with `skip_serializing_if = is_none` —
        // matches the legacy hand-rolled shape (`null` was never emitted
        // for absent data/error fields, only when callers explicitly set them).
        Ok(resp) => Json(resp).into_response(),
        Err(e) => Json(ErrorBody::new(e.to_string())).into_response(),
    }
}

// -- /im/me --

async fn im_me(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let human_repo = match with_workspace_snapshot(&state, &slug, human_repo_path) {
        Ok(Some(p)) => p,
        Ok(None) => {
            return human_not_initialized();
        }
        Err(r) => return r,
    };

    let me_path = human_repo.join(".gitim/me.json");
    match std::fs::read_to_string(&me_path) {
        Ok(content) => match serde_json::from_str::<MeJson>(&content) {
            Ok(me) => Json(ImMeResponse {
                ok: true,
                data: ImMeData {
                    handler: me.handler.unwrap_or_else(|| "unknown".to_string()),
                    display_name: me.display_name.unwrap_or_else(|| "Unknown".to_string()),
                    guest: me.guest.unwrap_or(false),
                },
            })
            .into_response(),
            Err(e) => Json(ErrorBody::new(format!("failed to parse me.json: {e}"))).into_response(),
        },
        Err(e) => Json(ErrorBody::new(format!("failed to read me.json: {e}"))).into_response(),
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
            .create_channel(
                &req.name,
                req.display_name.as_deref(),
                req.introduction.as_deref(),
                &req.invitees,
            )
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
    api_response_to_json(
        client
            .send(&req.channel, &req.body, None, req.reply_to)
            .await,
    )
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
    let repo_root = match with_workspace_snapshot(&state, &slug, |ctx| human_repo_path(ctx)) {
        Ok(Some(p)) => p,
        Ok(None) => return human_not_initialized(),
        Err(r) => return r,
    };

    let cursor = match with_workspace_snapshot(&state, &slug, |ctx| {
        req.since.clone().or_else(|| ctx.poll_cursor.clone())
    }) {
        Ok(c) => c,
        Err(r) => return r,
    };

    let client = GitimClient::new(&repo_root);
    let mut result = client.poll(cursor.as_deref()).await;
    if matches!(result, Err(ClientError::DaemonNotRunning)) {
        tracing::warn!(
            slug = %slug,
            repo = %repo_root.display(),
            "human daemon missing during poll; attempting restart"
        );
        if let Err(e) = ensure_daemon_with_runtime_log(repo_root.clone()).await {
            tracing::error!(
                slug = %slug,
                repo = %repo_root.display(),
                error = %e,
                "human daemon restart failed"
            );
            result = Err(e);
        } else {
            tracing::info!(
                slug = %slug,
                repo = %repo_root.display(),
                "human daemon restarted after poll failure"
            );
            result = GitimClient::new(&repo_root).poll(cursor.as_deref()).await;
        }
    }

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
    let labels_slice: Option<&[String]> = if q.label.is_empty() {
        None
    } else {
        Some(&q.label)
    };
    api_response_to_json(
        client
            .list_cards(
                q.channel.as_deref(),
                labels_slice,
                q.status.as_deref(),
                q.assignee.as_deref(),
            )
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
            Json(ErrorBody::new(format!("invalid slug: {e}"))),
        )
            .into_response();
    }
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    api_response_to_json(client.read_card(&channel, &card_id, q.limit, q.since).await)
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
            Json(ErrorBody::new(format!("invalid slug: {e}"))),
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
            Json(ErrorBody::new(format!("invalid slug: {e}"))),
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
    // human_handler keeps the explicit 503 status — distinct from the
    // /im/* proxy convention because it's only called from card/channel
    // archive endpoints that want a hard failure if the workspace's daemon
    // never came up. Was that way before the typed sweep; keeping it.
    let human_repo = with_workspace_snapshot(state, slug, human_repo_path)?.ok_or_else(|| {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorBody::new("human daemon not initialized")),
        )
            .into_response()
    })?;
    let me_path = human_repo.join(".gitim/me.json");
    let content = std::fs::read_to_string(&me_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorBody::new(format!("failed to read me.json: {e}"))),
        )
            .into_response()
    })?;
    let me: MeJson = serde_json::from_str(&content).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorBody::new(format!("failed to parse me.json: {e}"))),
        )
            .into_response()
    })?;
    me.handler.ok_or_else(|| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorBody::new("me.json missing handler field")),
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
            Json(ErrorBody::new(format!("invalid slug: {e}"))),
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
            Json(ErrorBody::new(format!("invalid slug: {e}"))),
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
            Json(ErrorBody::new(format!("invalid slug: {e}"))),
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
            Json(ErrorBody::new(format!("invalid slug: {e}"))),
        )
            .into_response();
    }
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    api_response_to_json(client.unarchive_channel(&name).await)
}

#[derive(serde::Deserialize)]
struct ArchivedChannelsQuery {
    #[serde(default)]
    prefix: Option<String>,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

async fn im_list_archived_channels(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
    axum::extract::Query(q): axum::extract::Query<ArchivedChannelsQuery>,
) -> axum::response::Response {
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    let prefix = q.prefix.as_deref();
    let offset = q.offset.unwrap_or(0);
    let limit = q.limit.unwrap_or(10).clamp(1, 100);
    api_response_to_json(client.list_archived_channels(prefix, offset, limit).await)
}

async fn im_dm_archive(
    State(state): State<SharedRuntimeState>,
    axum::extract::Path((slug, peer)): axum::extract::Path<(String, String)>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if let Err(e) = crate::slug::validate(&slug) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(ErrorBody::new(format!("invalid slug: {e}"))),
        )
            .into_response();
    }
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    api_response_to_json(client.archive_dm(&peer).await)
}

async fn im_dm_unarchive(
    State(state): State<SharedRuntimeState>,
    axum::extract::Path((slug, peer)): axum::extract::Path<(String, String)>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if let Err(e) = crate::slug::validate(&slug) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(ErrorBody::new(format!("invalid slug: {e}"))),
        )
            .into_response();
    }
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    api_response_to_json(client.unarchive_dm(&peer).await)
}

/// Query params for `GET /im/dm/archived`. All optional; missing values
/// resolve to `prefix=None`, `offset=0`, `limit=5` (page-size matches the
/// WebUI default). `limit` is clamped to `[1,100]` before forwarding so the
/// daemon — which also enforces the same bound (Task 3) — never sees an
/// out-of-range value. Defence in depth: the runtime layer is the front
/// door from the browser, the daemon is the last word.
#[derive(serde::Deserialize)]
struct ArchivedDmsQuery {
    #[serde(default)]
    prefix: Option<String>,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

async fn im_list_archived_dms(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
    axum::extract::Query(q): axum::extract::Query<ArchivedDmsQuery>,
) -> axum::response::Response {
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    let prefix = q.prefix.as_deref();
    let offset = q.offset.unwrap_or(0);
    let limit = q.limit.unwrap_or(5).clamp(1, 100);
    api_response_to_json(client.list_archived_dms(prefix, offset, limit).await)
}

// -- /users/archived + /users/{handler}/unarchive --
//
// Used by WebUI's E.3 "show archived" toggle on the agent list, and by
// the unarchive recovery action on archived agents. Both proxy directly
// to the human-clone daemon — no runtime-side state is involved, since
// archived users are an artifact of `archive/users/<handler>.meta.yaml`
// in the shared repo. The runtime keeps no metadata for archived
// agents (provider / model / messages_processed are gone with the
// daemon's clone delete), so the WebUI renders the list with only
// handler / display_name from the daemon response.

async fn users_list_archived(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
) -> axum::response::Response {
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    api_response_to_json(client.list_archived_users().await)
}

async fn users_unarchive(
    State(state): State<SharedRuntimeState>,
    axum::extract::Path((slug, handler)): axum::extract::Path<(String, String)>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if let Err(e) = crate::slug::validate(&slug) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(ErrorBody::new(format!("invalid slug: {e}"))),
        )
            .into_response();
    }
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    api_response_to_json(client.unarchive_user(&handler).await)
}

// -- /im/boards --

async fn im_list_boards(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
) -> axum::response::Response {
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    api_response_to_json(client.board_list().await)
}

async fn im_show_board(
    State(state): State<SharedRuntimeState>,
    axum::extract::Path((slug, handler)): axum::extract::Path<(String, String)>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if let Err(e) = crate::slug::validate(&slug) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(ErrorBody::new(format!("invalid slug: {e}"))),
        )
            .into_response();
    }
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    api_response_to_json(client.board_show(&handler).await)
}

async fn im_board_init(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
) -> axum::response::Response {
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    api_response_to_json(client.board_init().await)
}

#[derive(Deserialize)]
struct BoardPublishRequest {
    #[serde(default)]
    content: Option<String>,
}

async fn im_board_publish(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
    Json(req): Json<BoardPublishRequest>,
) -> axum::response::Response {
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    api_response_to_json(client.board_publish(req.content.as_deref()).await)
}

#[derive(Deserialize)]
struct BoardFieldRequest {
    field: String,
    value: String,
}

async fn im_board_field(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
    Json(req): Json<BoardFieldRequest>,
) -> axum::response::Response {
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    api_response_to_json(client.board_set(&req.field, &req.value).await)
}

#[derive(Deserialize)]
struct BoardSectionRequest {
    section: String,
    value: String,
}

async fn im_board_section_set(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
    Json(req): Json<BoardSectionRequest>,
) -> axum::response::Response {
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    api_response_to_json(client.board_section_set(&req.section, &req.value).await)
}

async fn im_board_section_append(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
    Json(req): Json<BoardSectionRequest>,
) -> axum::response::Response {
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    api_response_to_json(client.board_section_append(&req.section, &req.value).await)
}

// -- /workspaces/{slug}/crons (read endpoints) --
//
// These routes proxy to the workspace's human-clone daemon for spec metadata
// (list / show / runs lists). The single-run body endpoint reads the thread
// file straight off disk — there is no per-thread-path daemon IPC, and the
// runtime already trusts the workspace path with full read access. Reading
// `crons/<name>/<ts>.thread` directly keeps the path simple without forcing
// a daemon-side change.
//
// Error mapping: daemon-side `error_code: "not_found"` (cron name unknown)
// becomes HTTP 404. Other daemon errors travel through `ErrorBody` with the
// daemon's `error_code` preserved for the WebUI to branch on.

/// Validate the `<ts>` URL parameter shape: filesystem-safe ISO 8601 UTC
/// with `:` swapped for `-` (matches the on-disk `<ts>.thread` filename).
/// Hand-rolled against `YYYY-MM-DDTHH-MM-SSZ` — 20 ASCII chars, fixed-width
/// digits + literal separators. Cheaper and clearer than pulling a regex
/// crate for one validation site.
fn cron_ts_is_valid(ts: &str) -> bool {
    if ts.len() != 20 {
        return false;
    }
    let bytes = ts.as_bytes();
    let digit_positions = [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18];
    for i in digit_positions {
        if !bytes[i].is_ascii_digit() {
            return false;
        }
    }
    bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b'-'
        && bytes[16] == b'-'
        && bytes[19] == b'Z'
}

#[derive(Serialize)]
struct CronListResponse {
    crons: Vec<gitim_core::responses::CronSummary>,
}

#[derive(Serialize)]
struct CronRunsListResponse {
    runs: Vec<gitim_core::responses::CronRunEntry>,
}

#[derive(Serialize)]
struct CronRunBodyResponse {
    body: String,
}

/// Map a `ClientError::Api` with `error_code = "not_found"` to an HTTP 404,
/// other daemon errors to a 200 with `ok: false` payload (matching the rest
/// of `api_response_to_json`'s convention so WebUI can branch on `body.ok`),
/// and unrelated transport errors to a 200 `ok: false`.
fn cron_client_error_to_response(err: gitim_client::ClientError) -> axum::response::Response {
    use axum::response::IntoResponse;
    use gitim_client::ClientError;
    match err {
        ClientError::Api {
            ref message,
            code: Some(ref code),
        } if code == "not_found" => (
            axum::http::StatusCode::NOT_FOUND,
            Json(ErrorBody::with_code(message.clone(), code.clone())),
        )
            .into_response(),
        ClientError::Api { message, code } => {
            let body = match code {
                Some(c) => ErrorBody::with_code(message, c),
                None => ErrorBody::new(message),
            };
            Json(body).into_response()
        }
        other => Json(ErrorBody::new(other.to_string())).into_response(),
    }
}

async fn crons_list(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    match client.list_crons().await {
        Ok(crons) => Json(CronListResponse { crons }).into_response(),
        Err(e) => cron_client_error_to_response(e),
    }
}

async fn crons_show(
    State(state): State<SharedRuntimeState>,
    axum::extract::Path((slug, name)): axum::extract::Path<(String, String)>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if let Err(e) = crate::slug::validate(&slug) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(ErrorBody::new(format!("invalid slug: {e}"))),
        )
            .into_response();
    }
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    match client.show_cron(&name).await {
        Ok(detail) => Json(detail).into_response(),
        Err(e) => cron_client_error_to_response(e),
    }
}

async fn crons_runs_list(
    State(state): State<SharedRuntimeState>,
    axum::extract::Path((slug, name)): axum::extract::Path<(String, String)>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if let Err(e) = crate::slug::validate(&slug) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(ErrorBody::new(format!("invalid slug: {e}"))),
        )
            .into_response();
    }
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    // `None` limit means "daemon default" (50, capped at 1000) — same
    // ceiling history_cron applies. Explicit caps live behind the
    // /timeline endpoint.
    match client.history_cron(&name, None).await {
        Ok(runs) => Json(CronRunsListResponse { runs }).into_response(),
        Err(e) => cron_client_error_to_response(e),
    }
}

async fn crons_run_body(
    State(state): State<SharedRuntimeState>,
    axum::extract::Path((slug, name, ts)): axum::extract::Path<(String, String, String)>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if let Err(e) = crate::slug::validate(&slug) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(ErrorBody::new(format!("invalid slug: {e}"))),
        )
            .into_response();
    }
    if !cron_ts_is_valid(&ts) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(ErrorBody::with_code(
                "invalid run timestamp; expected YYYY-MM-DDTHH-MM-SSZ",
                "invalid_ts",
            )),
        )
            .into_response();
    }

    // Same validator the daemon runs on the IPC surface, lifted to
    // gitim-core so a regex drift can't open a path-traversal hole here.
    // Charset rule (`^[a-z0-9][a-z0-9-]{0,62}$`) means `..`, `/`, `\`,
    // null bytes, percent-encoded escapes — none of them survive past
    // this point, so the `crons/<name>/<ts>.thread` join below cannot
    // escape `crons/`.
    if let Err(e) = gitim_core::types::validate_cron_name(&name) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(ErrorBody::with_code(e.to_string(), "invalid_name")),
        )
            .into_response();
    }

    let human_repo = match with_workspace_snapshot(&state, &slug, human_repo_path) {
        Ok(Some(p)) => p,
        Ok(None) => return human_not_initialized(),
        Err(r) => return r,
    };

    // Only look in the active path; archived crons are out of v1 scope for
    // the run viewer (they don't appear in /crons/list anyway).
    let thread_path = human_repo
        .join("crons")
        .join(&name)
        .join(format!("{ts}.thread"));

    // Defense in depth: the charset check above already rejects every
    // shape that could escape, but if the runtime's view of the
    // workspace ever races with a symlink swap (e.g. an attacker with
    // local fs access points `crons/<name>` at `/etc`) the canonical
    // path check still catches it. The cost — one extra stat — only
    // pays out on an existing file path, so the happy-path overhead is
    // a single syscall per request.
    let crons_root = human_repo.join("crons");
    if let (Ok(canon_path), Ok(canon_root)) = (
        std::fs::canonicalize(&thread_path),
        std::fs::canonicalize(&crons_root),
    ) {
        if !canon_path.starts_with(&canon_root) {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(ErrorBody::with_code(
                    "cron name resolved outside workspace cron root",
                    "invalid_name",
                )),
            )
                .into_response();
        }
    }

    if !thread_path.is_file() {
        return (
            axum::http::StatusCode::NOT_FOUND,
            Json(ErrorBody::with_code(
                format!("run '{ts}' for cron '{name}' not found"),
                "not_found",
            )),
        )
            .into_response();
    }
    match std::fs::read_to_string(&thread_path) {
        Ok(body) => Json(CronRunBodyResponse { body }).into_response(),
        Err(e) => {
            // Status code matches the failure mode:
            //   NotFound  → 404 (TOCTOU disappear between is_file and read)
            //   anything else → 500 (perm denied, IO error, etc.)
            // Bare `Json(ErrorBody...)` without an explicit status maps to
            // 200 OK on the wire, so the status tuple is load-bearing.
            let (status, code) = match e.kind() {
                std::io::ErrorKind::NotFound => (axum::http::StatusCode::NOT_FOUND, "not_found"),
                _ => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "read_failed"),
            };
            (
                status,
                Json(ErrorBody::with_code(
                    format!("failed to read run body: {e}"),
                    code,
                )),
            )
                .into_response()
        }
    }
}

// -- /workspaces/{slug}/crons/timeline --
//
// Merged past/future/missed view across every active spec in the workspace,
// computed on the runtime side. The daemon only owns spec metadata; the
// timeline algorithm needs (a) the schedule + timezone + created_at from
// `list_crons`, and (b) the actual `<ts>.thread` filenames glob'd straight
// off disk in the human clone. No new daemon IPC was needed.
//
// Future-fire iteration is bounded per cron to prevent a runaway schedule
// (e.g. `* * * * *` over a month = 43 200 entries) from DoSing the endpoint:
// the cap is `MAX_TIMELINE_ENTRIES_PER_CRON`. When any single cron hits the
// cap, the response carries `truncated: true` so the WebUI can surface a
// hint without denying the rest of the data.

/// Per-cron iteration ceiling for `next_fire_after` walks. Picked as a
/// reasonable upper bound for a typical month view: even `* * * * *` over
/// 30 days only emits 43 200 entries, so 10 000 is enough for ~7 days of
/// minute-level granularity or a full month of hourly-or-coarser. A spec
/// that exceeds this in the requested window is almost always a misconfig
/// (or a deliberate DoS attempt) — partial response with a truncated flag
/// is the safer default than unbounded iteration.
const MAX_TIMELINE_ENTRIES_PER_CRON: usize = 10_000;

#[derive(Deserialize)]
struct TimelineQuery {
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
}

#[derive(Serialize)]
struct TimelineEntry {
    /// RFC 3339 UTC with seconds + trailing `Z`, matching `next_fire` on
    /// `CronSummary` and `CronDetail` (both call `to_rfc3339_opts` with
    /// `SecondsFormat::Secs`).
    ts: String,
    /// `"past"` | `"future"` | `"missed"` — kept as string so the wire
    /// stays language-agnostic and the frontend can switch on it.
    kind: &'static str,
    cron_name: String,
    /// Handler the cron message goes to when it fires — i.e. the agent
    /// that "does" this task. Copied straight off the owning `CronSummary`
    /// so the calendar can disambiguate "alice ran e2e-check at 13:00"
    /// from "bob ran e2e-check at 13:00" without a follow-up `show_cron`
    /// per entry. Required (never empty) because `CronSpec` validation
    /// rejects specs without a target.
    target: String,
    /// Populated for `kind == "past"` so the calendar UI can deep-link
    /// directly to the run body endpoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_url: Option<String>,
    /// Populated for `kind == "missed"` with a short human reason. The
    /// WebUI shows this in tooltip / detail panel.
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Serialize)]
struct TimelineResponse {
    entries: Vec<TimelineEntry>,
    /// `true` when at least one cron's iteration hit
    /// `MAX_TIMELINE_ENTRIES_PER_CRON` and the rest of its theoretical
    /// fires were dropped. Absent (skipped on the wire) on the typical
    /// healthy path.
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    truncated: bool,
}

/// Default window when neither `from` nor `to` is given: the calendar's
/// current month in UTC. Picked over "rolling 30d" because the WebUI is
/// month-grid-driven; matching the natural frontend default keeps the
/// boundary behavior intuitive.
fn default_window_now(
    now: chrono::DateTime<chrono::Utc>,
) -> (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>) {
    use chrono::{Datelike, NaiveDate, TimeZone};
    let year = now.year();
    let month = now.month();
    // First day of current month, 00:00:00 UTC.
    let from_date = NaiveDate::from_ymd_opt(year, month, 1).expect("valid month start");
    let from = chrono::Utc.from_utc_datetime(
        &from_date
            .and_hms_opt(0, 0, 0)
            .expect("00:00:00 always valid"),
    );
    // First day of NEXT month, then minus one second → end of current month.
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let next_date = NaiveDate::from_ymd_opt(next_year, next_month, 1).expect("valid next month");
    let next_start = chrono::Utc.from_utc_datetime(
        &next_date
            .and_hms_opt(0, 0, 0)
            .expect("00:00:00 always valid"),
    );
    let to = next_start - chrono::Duration::seconds(1);
    (from, to)
}

/// Format a UTC instant as `YYYY-MM-DDTHH-MM-SSZ` — the filesystem-safe
/// stem used for `<ts>.thread` filenames AND the URL-safe `<ts>` segment
/// in the runs endpoints. Single source of truth so the two consumers
/// can't drift.
fn ts_to_filename_stem(dt: chrono::DateTime<chrono::Utc>) -> String {
    let canonical = dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    canonical.replace(':', "-")
}

/// Inverse of `ts_to_filename_stem`. Returns `None` for shapes that don't
/// match `YYYY-MM-DDTHH-MM-SSZ` (the validator already gates the URL
/// path; this is for internal use against on-disk filenames).
fn filename_stem_to_ts(stem: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    if !cron_ts_is_valid(stem) {
        return None;
    }
    // Restore colons so chrono's RFC 3339 parser can read it.
    let restored: String = stem
        .char_indices()
        .map(|(i, c)| {
            // Positions 13 and 16 are the time-component separators.
            if (i == 13 || i == 16) && c == '-' {
                ':'
            } else {
                c
            }
        })
        .collect();
    chrono::DateTime::parse_from_rfc3339(&restored)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

/// Build a synthetic `CronSpec` from a `CronSummary` good enough for
/// `next_fire_after` to operate on. Synthetic fields (`prompt`,
/// `created_by`, `version`) get placeholder values that the cron-spec
/// validator never sees because we skip validate() here — we trust the
/// daemon's own validation that ran on create. Avoids round-tripping
/// through `show_cron` once per spec just to call `next_fire_after`.
fn synthesize_spec_for_iteration(
    summary: &gitim_core::responses::CronSummary,
) -> Result<gitim_core::types::cron::CronSpec, String> {
    use gitim_core::types::cron::CronSpec;
    use gitim_core::types::handler::Handler;
    use std::collections::BTreeMap;

    let target = Handler::new(&summary.target).map_err(|e| format!("invalid target: {e}"))?;
    let created_by =
        Handler::new(&summary.created_by).map_err(|e| format!("invalid created_by: {e}"))?;
    Ok(CronSpec {
        version: 1,
        schedule: summary.schedule.clone(),
        timezone: summary.timezone.clone(),
        target,
        prompt: "_".to_string(),
        enabled: summary.enabled,
        created_by,
        created_at: summary.created_at.clone(),
        extra: BTreeMap::new(),
    })
}

async fn crons_timeline(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
    axum::extract::Query(q): axum::extract::Query<TimelineQuery>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    use chrono::{DateTime, Utc};
    use gitim_core::types::cron::next_fire_after;

    let now = Utc::now();
    let (default_from, default_to) = default_window_now(now);
    let from = match q.from.as_deref() {
        None => default_from,
        Some(s) => match DateTime::parse_from_rfc3339(s) {
            Ok(dt) => dt.with_timezone(&Utc),
            Err(e) => {
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    Json(ErrorBody::with_code(
                        format!("invalid from timestamp '{s}': {e}"),
                        "invalid_timestamp",
                    )),
                )
                    .into_response()
            }
        },
    };
    let to = match q.to.as_deref() {
        None => default_to,
        Some(s) => match DateTime::parse_from_rfc3339(s) {
            Ok(dt) => dt.with_timezone(&Utc),
            Err(e) => {
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    Json(ErrorBody::with_code(
                        format!("invalid to timestamp '{s}': {e}"),
                        "invalid_timestamp",
                    )),
                )
                    .into_response()
            }
        },
    };
    if from > to {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(ErrorBody::with_code("from must be <= to", "invalid_window")),
        )
            .into_response();
    }

    let human_repo = match with_workspace_snapshot(&state, &slug, human_repo_path) {
        Ok(Some(p)) => p,
        Ok(None) => return human_not_initialized(),
        Err(r) => return r,
    };

    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    let summaries = match client.list_crons().await {
        Ok(s) => s,
        Err(e) => return cron_client_error_to_response(e),
    };

    let mut entries: Vec<TimelineEntry> = Vec::new();
    let mut any_truncated = false;

    for summary in &summaries {
        // -- Past entries: glob the on-disk thread files --
        // Disabled specs still surface past runs here so historical
        // context isn't lost on disable (matches the daemon's
        // list_thread_runs semantics — run files persist after disable).
        // The future/missed arm below skips disabled specs at the
        // live `if !summary.enabled { continue }` check.
        let cron_dir = human_repo.join("crons").join(&summary.name);
        let mut past_ts_set: std::collections::HashSet<String> = std::collections::HashSet::new();
        if let Ok(rd) = std::fs::read_dir(&cron_dir) {
            for entry in rd.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                let stem = match fname.strip_suffix(".thread") {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                let ts_dt = match filename_stem_to_ts(&stem) {
                    Some(dt) => dt,
                    None => continue,
                };
                if ts_dt < from || ts_dt > to {
                    continue;
                }
                past_ts_set.insert(stem.clone());
                let canonical = ts_dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
                entries.push(TimelineEntry {
                    ts: canonical,
                    kind: "past",
                    cron_name: summary.name.clone(),
                    target: summary.target.clone(),
                    thread_url: Some(format!(
                        "/workspaces/{}/crons/{}/runs/{}",
                        slug, summary.name, stem
                    )),
                    reason: None,
                });
            }
        }

        // -- Future / missed: iterate next_fire_after for active specs --
        if !summary.enabled {
            continue;
        }
        let spec = match synthesize_spec_for_iteration(summary) {
            Ok(s) => s,
            Err(e) => {
                // Synthesis can only fail if the daemon's CronSummary
                // shape drifts from what `synthesize_spec_for_iteration`
                // expects (a missing/renamed `target` / `created_by` /
                // `created_at` / etc.). Past entries already emitted from
                // disk above; the cron's future + missed projections will
                // be silently absent. Logging here is the only signal a
                // future debugger gets — without it, "this cron's future
                // entries vanished" requires source reading to diagnose.
                tracing::warn!(
                    cron_name = %summary.name,
                    error = %e,
                    "timeline: failed to synthesize spec for future-fire iteration; cron's future/missed entries omitted"
                );
                continue;
            }
        };
        let created_at_dt = match DateTime::parse_from_rfc3339(&spec.created_at) {
            Ok(dt) => dt.with_timezone(&Utc),
            Err(e) => {
                // Same failure mode as the synthesis branch — RFC 3339
                // parse should already be guaranteed by daemon-side
                // CronSpec::validate, but a hand-edited spec.yaml could
                // regress. Log with the same shape so dashboards can
                // group them.
                tracing::warn!(
                    cron_name = %summary.name,
                    created_at = %spec.created_at,
                    error = %e,
                    "timeline: created_at failed RFC 3339 parse; cron's future/missed entries omitted"
                );
                continue;
            }
        };
        // Anchor selection mirrors the engine's strictly-after-created_at
        // contract:
        //   - When the requested `from` is *after* `created_at`, we walk
        //     `from - 1s` so a fire that lands exactly on `from` still
        //     surfaces (boundary recovery — `next_fire_after` returns
        //     strictly later than its argument, so we shift one second
        //     left to keep an on-`from` instant in scope).
        //   - When `from <= created_at`, we MUST anchor at `created_at`
        //     itself, NOT `created_at - 1s`. The engine never fires *at*
        //     `created_at` (Bootstrap invariant: `last_fire = created_at`,
        //     and `next_fire_after(spec, created_at)` returns the next
        //     scheduled instant strictly later). Subtracting a second
        //     here would let the timeline emit a `future`/`missed` entry
        //     at `created_at` that the engine can never produce — the
        //     calendar would lie about the cron's first run.
        let mut anchor = if from > created_at_dt {
            from - chrono::Duration::seconds(1)
        } else {
            created_at_dt
        };
        let mut iters = 0usize;
        loop {
            if iters >= MAX_TIMELINE_ENTRIES_PER_CRON {
                any_truncated = true;
                break;
            }
            let next = match next_fire_after(&spec, anchor) {
                Ok(dt) => dt,
                Err(_) => break,
            };
            if next > to {
                break;
            }
            iters += 1;
            anchor = next;

            let stem = ts_to_filename_stem(next);
            if past_ts_set.contains(&stem) {
                // Theoretical fire that already happened — already emitted
                // as `past` from the disk glob above.
                continue;
            }
            let canonical = next.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            if next > now {
                entries.push(TimelineEntry {
                    ts: canonical,
                    kind: "future",
                    cron_name: summary.name.clone(),
                    target: summary.target.clone(),
                    thread_url: None,
                    reason: None,
                });
            } else {
                entries.push(TimelineEntry {
                    ts: canonical,
                    kind: "missed",
                    cron_name: summary.name.clone(),
                    target: summary.target.clone(),
                    thread_url: None,
                    reason: Some("no thread file present".to_string()),
                });
            }
        }
    }

    // Stable sort by `ts` ascending. `ts` strings are RFC 3339 UTC with
    // fixed-width fields, so lexicographic sort == chronological.
    entries.sort_by(|a, b| a.ts.cmp(&b.ts));

    Json(TimelineResponse {
        entries,
        truncated: any_truncated,
    })
    .into_response()
}

// -- /workspaces/{slug}/im/flows --
//
// These routes proxy to the workspace's human-clone daemon for flow management.
// Read endpoints (list / show / validate) return raw JSON bodies — the frontend
// wraps them in `{ok: true, data}` via `cronRequest`. Write endpoints (create /
// remove) use HTTP status codes for success/failure detection; the frontend
// checks `res.ok` rather than the body `ok` field.
//
// Error mapping: daemon-side `error_code: "not_found"` → HTTP 404. Other
// daemon errors travel through `ErrorBody` with the error_code preserved.

fn flow_client_error_to_response(err: gitim_client::ClientError) -> axum::response::Response {
    use axum::response::IntoResponse;
    use gitim_client::ClientError;
    match err {
        ClientError::Api {
            ref message,
            code: Some(ref code),
        } if code == "not_found" => (
            axum::http::StatusCode::NOT_FOUND,
            Json(ErrorBody::with_code(message.clone(), code.clone())),
        )
            .into_response(),
        ClientError::Api { message, code } => {
            let body = match code {
                Some(c) => ErrorBody::with_code(message, c),
                None => ErrorBody::new(message),
            };
            Json(body).into_response()
        }
        other => Json(ErrorBody::new(other.to_string())).into_response(),
    }
}

/// Extract the `data` field from a successful `ApiResponse` and return it
/// as a raw JSON body. The frontend's `cronRequest` wraps this in
/// `{ok: true, data: body}` on the client side.
fn flow_raw_data_response(
    result: Result<gitim_client::ApiResponse, gitim_client::ClientError>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    match result {
        Ok(resp) if resp.ok => {
            let data = resp.data.unwrap_or(serde_json::Value::Null);
            Json(data).into_response()
        }
        Ok(resp) => {
            let body = match resp.error_code {
                Some(ref code) if code == "not_found" => (
                    axum::http::StatusCode::NOT_FOUND,
                    Json(ErrorBody::with_code(
                        resp.error.unwrap_or_default(),
                        code.clone(),
                    )),
                )
                    .into_response(),
                Some(c) => {
                    Json(ErrorBody::with_code(resp.error.unwrap_or_default(), c)).into_response()
                }
                None => Json(ErrorBody::new(resp.error.unwrap_or_default())).into_response(),
            };
            body
        }
        Err(e) => flow_client_error_to_response(e),
    }
}

async fn flows_list(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
) -> axum::response::Response {
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    flow_raw_data_response(client.flow_list().await)
}

async fn flows_show(
    State(state): State<SharedRuntimeState>,
    axum::extract::Path((slug, flow_slug)): axum::extract::Path<(String, String)>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if let Err(e) = crate::slug::validate(&slug) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(ErrorBody::new(format!("invalid slug: {e}"))),
        )
            .into_response();
    }
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    flow_raw_data_response(client.flow_show(&flow_slug).await)
}

#[derive(Deserialize)]
struct FlowCreateRequest {
    slug: String,
    name: String,
    description: String,
}

async fn flows_create(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
    Json(req): Json<FlowCreateRequest>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    match client
        .flow_create(&req.slug, &req.name, &req.description)
        .await
    {
        Ok(resp) if resp.ok => Json(serde_json::json!({ "ok": true })).into_response(),
        Ok(resp) => {
            let body = match resp.error_code {
                Some(ref code) if code == "not_found" => (
                    axum::http::StatusCode::NOT_FOUND,
                    Json(ErrorBody::with_code(
                        resp.error.unwrap_or_default(),
                        code.clone(),
                    )),
                )
                    .into_response(),
                Some(c) => (
                    axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                    Json(ErrorBody::with_code(resp.error.unwrap_or_default(), c)),
                )
                    .into_response(),
                None => (
                    axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                    Json(ErrorBody::new(resp.error.unwrap_or_default())),
                )
                    .into_response(),
            };
            body
        }
        Err(e) => {
            let status = axum::http::StatusCode::INTERNAL_SERVER_ERROR;
            (status, Json(ErrorBody::new(e.to_string()))).into_response()
        }
    }
}

async fn flows_remove(
    State(state): State<SharedRuntimeState>,
    axum::extract::Path((slug, flow_slug)): axum::extract::Path<(String, String)>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if let Err(e) = crate::slug::validate(&slug) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(ErrorBody::new(format!("invalid slug: {e}"))),
        )
            .into_response();
    }
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    match client.flow_remove(&flow_slug).await {
        Ok(resp) if resp.ok => Json(serde_json::json!({ "ok": true })).into_response(),
        Ok(resp) => {
            let body = match resp.error_code {
                Some(ref code) if code == "not_found" => (
                    axum::http::StatusCode::NOT_FOUND,
                    Json(ErrorBody::with_code(
                        resp.error.unwrap_or_default(),
                        code.clone(),
                    )),
                )
                    .into_response(),
                Some(c) => (
                    axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                    Json(ErrorBody::with_code(resp.error.unwrap_or_default(), c)),
                )
                    .into_response(),
                None => (
                    axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                    Json(ErrorBody::new(resp.error.unwrap_or_default())),
                )
                    .into_response(),
            };
            body
        }
        Err(e) => {
            let status = axum::http::StatusCode::INTERNAL_SERVER_ERROR;
            (status, Json(ErrorBody::new(e.to_string()))).into_response()
        }
    }
}

async fn flows_validate(
    State(state): State<SharedRuntimeState>,
    axum::extract::Path((slug, flow_slug)): axum::extract::Path<(String, String)>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if let Err(e) = crate::slug::validate(&slug) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(ErrorBody::new(format!("invalid slug: {e}"))),
        )
            .into_response();
    }
    let client = match human_client(&state, &slug) {
        Ok(c) => c,
        Err(j) => return j,
    };
    flow_raw_data_response(client.flow_validate(&flow_slug).await)
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
    /// Optional human blurb (≤ MAX_INTRODUCTION_LEN). Surfaced on the agent
    /// card and detail page; not fed to the LLM. Empty / missing keeps the
    /// daemon's onboard default ("GitIM user").
    #[serde(default)]
    introduction: Option<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    /// Opt the new agent out of #general auto-join. `None` (field omitted) =
    /// preserve historical default of joining. `Some(false)` skips the
    /// auto_join_general step inside the daemon's onboard handler.
    #[serde(default)]
    join_general: Option<bool>,
    /// Hermes-only: the LLM provider id to configure in the agent's hermes
    /// profile (e.g. "anthropic", "kimi-coding", "custom:foo"). When omitted
    /// together with `llm_model`, the cloned profile keeps its default model.
    /// Explicit values are validated against BUILTIN_PROVIDERS + config.yaml.
    #[serde(default)]
    llm_provider: Option<String>,
    /// Hermes-only: the model to set as `model.default` in the hermes profile
    /// (e.g. "claude-opus-4-5"). Required with an explicit `llm_provider`.
    #[serde(default)]
    llm_model: Option<String>,
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
    // `mock` is permitted because E2E tests (agent-interaction.spec.ts)
    // provision an agent with provider=mock.
    match req.provider.as_str() {
        "claude" | "codex" | "opencode" | "pi" | "hermes" | "cursor" | "kimi" | "mock" => {}
        other => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorBody::new(format!("unsupported provider: {other}"))),
            )
                .into_response();
        }
    }

    // ── Hermes LLM provider early validation ─────────────────────────────────
    // Runs before workspace lookup so invalid hermes config is rejected cheaply
    // without touching git or the daemon. Must mirror the whitelist logic in the
    // hermes branch below (apply_model_config + rollback).
    if req.provider == "hermes" {
        let llm_provider = req
            .llm_provider
            .as_deref()
            .and_then(|value| (!value.is_empty()).then_some(value));
        let llm_model = req
            .llm_model
            .as_deref()
            .and_then(|value| (!value.is_empty()).then_some(value));

        match (llm_provider, llm_model) {
            (None, None) => {}
            (Some(llm_provider_str), Some(_)) => {
                let is_builtin = crate::hermes_llm::BUILTIN_PROVIDERS
                    .iter()
                    .any(|p| p.id == llm_provider_str);

                if !is_builtin && llm_provider_str.starts_with("custom:") {
                    let custom_name = &llm_provider_str["custom:".len()..];
                    let hermes_home = std::env::var_os("HERMES_HOME")
                        .map(PathBuf::from)
                        .unwrap_or_else(|| {
                            dirs::home_dir()
                                .unwrap_or_else(|| PathBuf::from("/"))
                                .join(".hermes")
                        });
                    let providers = crate::hermes_llm::list_providers(&hermes_home);
                    if !providers.iter().any(|p| p.id == llm_provider_str) {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(ErrorBody::with_code(
                                format!(
                                    "custom provider {custom_name} not found in hermes config.yaml"
                                ),
                                "custom_provider_not_found",
                            )),
                        )
                            .into_response();
                    }
                } else if !is_builtin {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorBody::with_code(
                            format!("unknown llm_provider: {llm_provider_str}"),
                            "unknown_llm_provider",
                        )),
                    )
                        .into_response();
                }
            }
            _ => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorBody::with_code(
                        "missing llm_provider/llm_model for hermes",
                        "missing_llm_provider",
                    )),
                )
                    .into_response();
            }
        }
    }

    // Length-check the optional introduction up front so we never start
    // provisioning (clone + daemon spawn + onboard) just to bounce on a
    // 400 at the post-onboard update_user step. Daemon enforces the same
    // ceiling as a defense-in-depth.
    if let Some(intro) = req.introduction.as_deref() {
        if intro.len() > MAX_INTRODUCTION_LEN {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorBody::new(format!(
                    "introduction exceeds {} byte limit",
                    MAX_INTRODUCTION_LEN
                ))),
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
        return Json(ErrorBody::with_code(
            format!("agent already exists: {}", req.handler),
            "handler_conflict",
        ))
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
    let human_dir = human_repo.unwrap_or_else(|| workspace.join(".gitim-runtime").join("human"));
    if git_provider == GitProvider::Github && human_dir.exists() {
        let fetch = std::process::Command::new("git")
            .args([
                "-c",
                "http.lowSpeedLimit=1000",
                "-c",
                "http.lowSpeedTime=10",
                "fetch",
                "origin",
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
        return Json(ErrorBody::with_code(
            format!(
                "handler @{} already registered in this workspace",
                req.handler
            ),
            "handler_conflict",
        ))
        .into_response();
    }

    // Archive Contract 2: handlers are terminally unique once departed.
    // The daemon enforces this at register_user / onboard time, but the
    // runtime checks it here too — we'd otherwise let the user pick a
    // handler the daemon will reject after we've already kicked off
    // provisioning. Distinct error_code so the WebUI can surface a clear
    // "previously departed" message instead of conflating it with a live
    // conflict.
    let archived_meta_path = human_dir
        .join("archive/users")
        .join(format!("{}.meta.yaml", req.handler));
    if archived_meta_path.exists() {
        return Json(ErrorBody::with_code(
            format!(
                "handler @{} is reserved (previously departed in this workspace)",
                req.handler
            ),
            "handler_reserved",
        ))
        .into_response();
    }

    // Provisioning preflight gate. All conflict checks have passed; before we
    // touch disk (create agent dir, clone remote, spawn daemon, write me.json),
    // verify that the agent's chosen provider can actually be invoked with the
    // env/model/llm config the request specifies. A `false` result tags the
    // failure with a stable error_code and carries the full `PreflightResult`
    // back so the WebUI / CLI can render structured detail (which binary, what
    // error_kind, stdout preview) without a second roundtrip.
    let preflight = crate::preflight::preflight_for_add_request(
        &req.provider,
        Some(&req.env),
        req.model.as_deref(),
        req.llm_provider.as_deref(),
        req.llm_model.as_deref(),
    )
    .await;
    if !preflight.available {
        let code = crate::preflight::classify_preflight_error_code(&preflight);
        let message = preflight
            .error
            .clone()
            .unwrap_or_else(|| "provisioning preflight failed".to_string());
        // Operator observability: failures here are user-facing AND infrequent,
        // so log them so `tail -f ~/.gitim/logs/*.log` surfaces patterns
        // (e.g. spike in `hermes_default_profile_no_llm` after a hermes
        // upgrade). Doesn't replace the structured response — that still
        // carries the full preflight_detail.
        tracing::warn!(
            handler = %req.handler,
            provider = %req.provider,
            error_code = %code,
            error_kind = ?preflight.error_kind,
            "agents_add aborted at provisioning preflight gate"
        );
        return Json(ErrorBody::with_preflight(message, code, preflight)).into_response();
    }

    let agents_dir = workspace.clone();
    if let Err(e) = std::fs::create_dir_all(&agents_dir) {
        return Json(ErrorBody::new(format!("failed to create agents dir: {e}"))).into_response();
    }

    let remote_url = match git_provider {
        GitProvider::Local => workspace.join("repo.git").to_string_lossy().into_owned(),
        GitProvider::Github => {
            let cfg = match workspace_config.as_ref() {
                Some(c) => c,
                None => {
                    return Json(ErrorBody::with_code(
                        "github mode requires workspace config with remote_url + token",
                        "config_missing",
                    ))
                    .into_response();
                }
            };
            let remote = match cfg.git.remote_url.as_deref() {
                Some(u) if !u.is_empty() => u,
                _ => {
                    return Json(ErrorBody::with_code(
                        "workspace config lacks remote_url",
                        "missing_remote_url",
                    ))
                    .into_response();
                }
            };
            let token = match cfg.git.token.as_deref() {
                Some(t) if !t.is_empty() => t,
                _ => {
                    return Json(ErrorBody::with_code(
                        "workspace config lacks token",
                        "missing_token",
                    ))
                    .into_response();
                }
            };
            let (owner, repo_name) = match parse_github_url(remote) {
                Ok(t) => t,
                Err(e) => {
                    return Json(ErrorBody::with_code(
                        redacted_url(&e.to_string()),
                        github_error_code(&e),
                    ))
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

    // Pull the workspace-level GitHub email so it rides into the agent's
    // git-mode onboard and lands in the agent me.json + AppState. None in
    // local mode or when the owner's email is private.
    let workspace_github_email = workspace_config
        .as_ref()
        .and_then(|c| c.git.github_email.clone());

    let config = AgentConfig {
        handler: req.handler.clone(),
        display_name: req.display_name.clone(),
        remote_url,
        github_email: workspace_github_email,
    };

    match provision_agent(&agents_dir, &config, req.join_general.unwrap_or(true)).await {
        Ok(handle) => {
            // Recheck after async provision to prevent duplicate loops from concurrent requests
            {
                let s = state.lock().unwrap();
                if let Some(ctx) = s.workspaces.get(&slug) {
                    if ctx.agents.contains_key(&req.handler) {
                        return Json(AgentAddResponse {
                            ok: true,
                            id: req.handler.clone(),
                        })
                        .into_response();
                    }
                }
            }

            // Persist config to me.json. Empty env doesn't overwrite an
            // existing env (None patch field = preserve).
            let me_path = handle.repo_root.join(".gitim/me.json");
            if let Ok(content) = std::fs::read_to_string(&me_path) {
                if let Ok(existing) = serde_json::from_str::<MeJson>(&content) {
                    let env_patch = if req.env.is_empty() {
                        None
                    } else {
                        Some(req.env.clone().into_iter().collect())
                    };
                    let patch = MeJson {
                        provider: Some(req.provider.clone()),
                        model: req.model.clone(),
                        system_prompt: req.system_prompt.clone(),
                        env: env_patch,
                        // Hermes-only: persist llm_provider/llm_model chosen at
                        // add-agent time so the agent loop can introspect them.
                        llm_provider: req
                            .llm_provider
                            .as_ref()
                            .filter(|value| !value.is_empty())
                            .cloned(),
                        llm_model: req
                            .llm_model
                            .as_ref()
                            .filter(|value| !value.is_empty())
                            .cloned(),
                        ..Default::default()
                    };
                    let merged = existing.merged_with(patch);
                    let _ =
                        std::fs::write(&me_path, serde_json::to_string_pretty(&merged).unwrap());
                }
            }

            // Hermes profile bootstrap: each agent gets its own
            // ~/.hermes/profiles/gitim-<handler> cloned from the user's
            // active profile so LLM config / auth / sessions stay isolated.
            // No-op for other providers.
            if req.provider == "hermes" {
                if !crate::hermes_profile::default_profile_ready() {
                    cleanup_agent_dir(&workspace, &req.handler);
                    return Json(ErrorBody::with_code(
                        "Hermes default profile is not configured. \
                            Run `hermes setup` in a terminal first to set up \
                            an LLM provider, then add the agent again.",
                        "hermes_not_setup",
                    ))
                    .into_response();
                }
                // Use ensure_profile_with so tests can inject a fake binary.
                let hermes_bin =
                    std::env::var("GITIM_TEST_HERMES_BIN").unwrap_or_else(|_| "hermes".to_string());
                if let Err(e) =
                    crate::hermes_profile::ensure_profile_with(&req.handler, &hermes_bin).await
                {
                    cleanup_agent_dir(&workspace, &req.handler);
                    return Json(ErrorBody::with_code(
                        format!("hermes profile create failed: {e}"),
                        "hermes_profile_create_failed",
                    ))
                    .into_response();
                }

                // ── Hermes LLM provider validation + model config write ────────
                // Explicit llm_provider/llm_model values were validated early
                // (before workspace lookup). Resolve base_url for custom
                // providers; builtin providers pass None so hermes uses its
                // registry default.
                let llm_provider = req
                    .llm_provider
                    .as_deref()
                    .and_then(|value| (!value.is_empty()).then_some(value));
                let llm_model = req
                    .llm_model
                    .as_deref()
                    .and_then(|value| (!value.is_empty()).then_some(value));

                if let (Some(llm_provider_str), Some(llm_model_str)) = (llm_provider, llm_model) {
                    let base_url: Option<String> = if llm_provider_str.starts_with("custom:") {
                        let hermes_home = std::env::var_os("HERMES_HOME")
                            .map(PathBuf::from)
                            .unwrap_or_else(|| {
                                dirs::home_dir()
                                    .unwrap_or_else(|| PathBuf::from("/"))
                                    .join(".hermes")
                            });
                        crate::hermes_llm::list_providers(&hermes_home)
                            .into_iter()
                            .find(|p| p.id == llm_provider_str)
                            .and_then(|p| p.base_url)
                    } else {
                        None
                    };

                    if let Err(e) = crate::hermes_profile::apply_model_config_with(
                        &req.handler,
                        llm_provider_str,
                        llm_model_str,
                        base_url.as_deref(),
                        &hermes_bin,
                    )
                    .await
                    {
                        // Best-effort profile cleanup before full agent dir removal.
                        let _ =
                            crate::hermes_profile::delete_profile_with(&req.handler, &hermes_bin)
                                .await;
                        cleanup_agent_dir(&workspace, &req.handler);
                        return (
                            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                            Json(ErrorBody::with_code(
                                format!("apply_model_config failed: {e}"),
                                "apply_model_config_failed",
                            )),
                        )
                            .into_response();
                    }
                }

                // ── Install GitIM-managed SOUL.md ───────────────────────────
                // The hermes profile was just clone-created from the user's
                // active profile, so its SOUL.md is still the hermes-shipped
                // template (`# Hermes Agent Persona` + comment). Replace it
                // with our hermes-tailored system prompt so hermes loads
                // GitIM identity / operating rules into its frozen system-
                // prompt slot at every session start (and rebuilds after
                // each in-loop compression event). Failure here is non-
                // fatal: the agent is still usable, just with hermes'
                // default persona — log a warning so it's visible.
                let body = crate::hermes_profile::build_hermes_soul_body(
                    &req.handler,
                    req.model.as_deref(),
                    req.system_prompt.as_deref(),
                );
                match crate::hermes_profile::write_soul_md(
                    &req.handler,
                    &body,
                    crate::hermes_profile::SoulWriteMode::Force,
                ) {
                    Ok(crate::hermes_profile::SoulWriteOutcome::Wrote) => {
                        tracing::info!(
                            handler = %req.handler,
                            "installed gitim-managed SOUL.md"
                        );
                    }
                    Ok(crate::hermes_profile::SoulWriteOutcome::SkippedUnchanged) => {
                        tracing::debug!(
                            handler = %req.handler,
                            "SOUL.md already up to date"
                        );
                    }
                    Ok(crate::hermes_profile::SoulWriteOutcome::RefusedUserEdited) => {
                        // Unreachable in Force mode but kept as a typed
                        // branch — defensive in case the variant grows.
                        tracing::warn!(
                            handler = %req.handler,
                            "SOUL.md write skipped unexpectedly under Force mode"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            handler = %req.handler,
                            error = %e,
                            "failed to install SOUL.md; agent will run with hermes default persona"
                        );
                    }
                }
            }

            // Apply the user-supplied introduction blurb (if any) to the
            // freshly-registered user meta.yaml. The daemon is still running
            // from provision_agent, so we reach it via the IPC client. Empty
            // / missing keeps the onboard default ("GitIM user"). Failure
            // here is intentionally non-fatal: the agent is fully usable
            // without the blurb, and the user can retry via PATCH.
            if let Some(intro) = req.introduction.as_deref() {
                if !intro.is_empty() {
                    let client = GitimClient::new(&handle.repo_root);
                    match client.update_user(&req.handler, intro).await {
                        Ok(resp) if resp.ok => {}
                        Ok(resp) => {
                            tracing::warn!(
                                handler = %req.handler,
                                error = ?resp.error,
                                "update_user during add_agent failed; continuing with default introduction",
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                handler = %req.handler,
                                error = %e,
                                "update_user during add_agent IPC error; continuing with default introduction",
                            );
                        }
                    }
                }
            }

            // Read introduction back from disk so AgentInfo reflects what
            // actually got committed (covers update_user no-op, partial
            // failure, and the default-introduction case uniformly).
            let introduction = read_user_introduction(&handle.repo_root, &req.handler);

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
                introduction,
                env: req.env.clone(),
                error_message: None,
                session_usage: None,
                llm_provider: if req.provider == "hermes" {
                    req.llm_provider
                        .as_ref()
                        .filter(|value| !value.is_empty())
                        .cloned()
                } else {
                    None
                },
                llm_model: if req.provider == "hermes" {
                    req.llm_model
                        .as_ref()
                        .filter(|value| !value.is_empty())
                        .cloned()
                } else {
                    None
                },
                usage_summary: None,
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

            Json(AgentAddResponse {
                ok: true,
                id: req.handler.clone(),
            })
            .into_response()
        }
        Err(e) => {
            cleanup_agent_dir(&workspace, &req.handler);
            Json(ErrorBody::new(redacted_url(&format!(
                "provision_agent failed: {e}"
            ))))
            .into_response()
        }
    }
}

/// Read `introduction` out of `users/<handler>.meta.yaml` for the agent's
/// own clone. Returns `None` for legacy agents where the file is missing
/// or malformed — recovery should not fail just because a meta file drifted,
/// the WebUI just won't have a blurb to display until the next PATCH.
fn read_user_introduction(repo_root: &Path, handler: &str) -> Option<String> {
    let meta_path = repo_root
        .join("users")
        .join(format!("{}.meta.yaml", handler));
    let content = std::fs::read_to_string(&meta_path).ok()?;
    let meta: UserMeta = serde_yaml::from_str(&content).ok()?;
    Some(meta.introduction)
}

/// Recovery-time loader for `<workspace>/.gitim-runtime/usage/<handler>.json`.
///
/// Returns `None` when the file is missing — that's the lazy-init posture for
/// agents created before token statistics shipped. We only synthesize a
/// `UsageSummary` when the file actually exists, so a fresh install renders
/// as "no data yet" rather than as a fully-zeroed sparkline.
///
/// Provider/model are stamped into the file when the agent loop first writes
/// it; recovery doesn't need them, so we pass empty strings — load_or_default
/// will only enter the load branch when the file is present and parseable.
fn load_usage_summary_for_recovery(
    workspace: &Path,
    handler: &str,
) -> Option<crate::usage_log::UsageSummary> {
    let path = crate::usage_log::AgentUsageLog::path(workspace, handler);
    if !path.exists() {
        return None;
    }
    let log = crate::usage_log::AgentUsageLog::load_or_default(workspace, handler, "", "", true);
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    Some(log.summary(&today))
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
        Json(AgentsListResponse { ok: true, agents })
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

#[derive(Deserialize)]
struct AgentRemoveRequest {
    id: String,
    #[serde(default)]
    hard_delete: bool,
}

/// Start the agent loop for a given agent ID. Shared by add, start, and recover.
fn start_agent_loop(state: &SharedRuntimeState, slug: &str, agent_id: &str) -> Result<(), String> {
    let (repo_root, handler, provider, model, system_prompt, env, activity_tx, workspace_root) = {
        let s = state.lock().unwrap();
        let ctx = s
            .workspaces
            .get(slug)
            .ok_or_else(|| format!("unknown workspace: {slug}"))?;
        let workspace_root = ctx.path.clone();
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
                workspace_root,
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
    agent_loop.set_runtime_state(state.clone());
    agent_loop.set_workspace_root(workspace_root);

    let owned_id = agent_id.to_string();
    let owned_slug = slug.to_string();
    let state_clone = state.clone();

    let handle = tokio::spawn(async move {
        match agent_loop.init().await {
            Ok(()) => {}
            Err(crate::error::RuntimeError::SelfDeparted) => {
                // Edge case: runtime restarted AFTER this agent self-burned.
                // Recovery walked the agent back into ctx.agents and
                // start_agent_loop spawned us — but the daemon's first
                // poll trips the self_departed gate immediately. Mirror
                // the run_once SelfDeparted arm: drive cleanup once, then
                // exit. Without this, the agent would sit in ctx.agents
                // with status="error" forever (or until the user
                // manually clicked burn in the WebUI).
                tracing::info!(
                    agent = %owned_id,
                    slug = %owned_slug,
                    "agent self-departed before runtime startup, triggering cleanup"
                );
                let cleanup_inputs = {
                    let s = state_clone.lock().unwrap();
                    s.workspaces.get(&owned_slug).and_then(|ctx| {
                        ctx.agents.get(&owned_id).map(|info| {
                            (
                                ctx.path.clone(),
                                PathBuf::from(&info.repo_path),
                                info.provider.clone(),
                                ctx.activity_tx.clone(),
                            )
                        })
                    })
                };
                if let Some((workspace_path, repo_path, provider, activity_tx)) = cleanup_inputs {
                    if let Err(e) = cleanup_agent_runtime_side(
                        &state_clone,
                        &owned_slug,
                        &owned_id,
                        &workspace_path,
                        &repo_path,
                        provider.as_deref(),
                        &activity_tx,
                    )
                    .await
                    {
                        tracing::error!(
                            agent = %owned_id,
                            slug = %owned_slug,
                            error = %e,
                            "self-departed cleanup failed during init"
                        );
                    }
                } else {
                    tracing::warn!(
                        agent = %owned_id,
                        slug = %owned_slug,
                        "self-departed at init but agent already removed from state — nothing to clean"
                    );
                }
                return;
            }
            Err(e) => {
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
                                info.last_activity = Some(chrono::Utc::now().to_rfc3339());
                            }
                        }
                    }
                    touch_activity(&state_clone);
                }
                Ok(false) => {
                    consecutive_errors = 0;
                }
                Err(crate::error::RuntimeError::SelfDeparted) => {
                    // Archive-protocol B.4: this agent's own user.meta.yaml
                    // landed in archive/users/, so the daemon refuses
                    // further polls. Don't back off — drive the same
                    // cleanup the WebUI burn endpoint runs (kill daemon,
                    // rm clone, hermes profile, ctx.agents removal, SSE)
                    // and exit the loop. Any further iteration would just
                    // re-trip the same self_departed gate.
                    tracing::info!(
                        agent = %owned_id,
                        slug = %owned_slug,
                        "agent self-departed, triggering runtime cleanup"
                    );
                    let cleanup_inputs = {
                        let s = state_clone.lock().unwrap();
                        s.workspaces.get(&owned_slug).and_then(|ctx| {
                            ctx.agents.get(&owned_id).map(|info| {
                                (
                                    ctx.path.clone(),
                                    PathBuf::from(&info.repo_path),
                                    info.provider.clone(),
                                    ctx.activity_tx.clone(),
                                )
                            })
                        })
                    };
                    if let Some((workspace_path, repo_path, provider, activity_tx)) = cleanup_inputs
                    {
                        if let Err(e) = cleanup_agent_runtime_side(
                            &state_clone,
                            &owned_slug,
                            &owned_id,
                            &workspace_path,
                            &repo_path,
                            provider.as_deref(),
                            &activity_tx,
                        )
                        .await
                        {
                            tracing::error!(
                                agent = %owned_id,
                                slug = %owned_slug,
                                error = %e,
                                "self-departed cleanup failed"
                            );
                        }
                    } else {
                        tracing::warn!(
                            agent = %owned_id,
                            slug = %owned_slug,
                            "self-departed but agent already removed from state — nothing to clean"
                        );
                    }
                    return;
                }
                Err(e) => {
                    if is_daemon_not_running_poll_error(&e) {
                        tracing::warn!(
                            agent = %owned_id,
                            slug = %owned_slug,
                            "agent daemon missing during poll; attempting restart"
                        );
                        match agent_loop.ensure_daemon_running().await {
                            Ok(()) => {
                                consecutive_errors = 0;
                                tracing::info!(
                                    agent = %owned_id,
                                    slug = %owned_slug,
                                    "agent daemon restarted after poll failure"
                                );
                                continue;
                            }
                            Err(restart_err) => {
                                tracing::error!(
                                    agent = %owned_id,
                                    slug = %owned_slug,
                                    error = %restart_err,
                                    "agent daemon restart failed"
                                );
                            }
                        }
                    }
                    consecutive_errors += 1;
                    let backoff = std::time::Duration::from_secs(
                        (2u64.saturating_pow(consecutive_errors)).min(MAX_BACKOFF_SECS),
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
) -> axum::response::Response {
    use axum::response::IntoResponse;
    match start_agent_loop(&state, &slug, &req.id) {
        Ok(()) => Json(OkAckResponse { ok: true }).into_response(),
        Err(e) => Json(ErrorBody::new(e)).into_response(),
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
            Json(ErrorBody::new(format!("invalid slug: {e}"))),
        )
            .into_response();
    }
    let s = state.lock().unwrap();
    let ctx = match s.workspaces.get(&slug) {
        Some(c) => c,
        None => return not_found_workspace(),
    };
    match ctx.agents.get(&id) {
        Some(info) => Json(AgentDetailResponse {
            ok: true,
            agent: info.clone(),
        })
        .into_response(),
        None => Json(ErrorBody::new("agent not found")).into_response(),
    }
}

// -- /agents PATCH --

/// Deserialize a JSON field that has three distinct states:
///   - absent → `None`           (caller should treat as "no-op")
///   - `null`  → `Some(None)`    (caller should clear the field)
///   - `"s"`   → `Some(Some(s))` (caller should set the field to `s`)
///
/// Standard serde maps both absent and `null` to `None` for `Option<T>`,
/// which loses the distinction we need.  This helper uses a raw `Value` round-
/// trip on the existing serde infrastructure instead of pulling in `serde_with`.
fn deser_triple_option<'de, D>(d: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v: serde_json::Value = serde::Deserialize::deserialize(d)?;
    match v {
        serde_json::Value::Null => Ok(Some(None)),
        serde_json::Value::String(s) => Ok(Some(Some(s))),
        _ => Err(serde::de::Error::custom("expected string or null")),
    }
}

#[derive(Deserialize, Default)]
struct AgentUpdateRequest {
    #[serde(default, deserialize_with = "deser_triple_option")]
    system_prompt: Option<Option<String>>,
    #[serde(default, deserialize_with = "deser_triple_option")]
    model: Option<Option<String>>,
    /// Three-state: absent → no-op, null → clear (empty introduction),
    /// "s" → set to s. Goes to daemon via `update_user`, which writes
    /// `users/<handler>.meta.yaml` and commits.
    #[serde(default, deserialize_with = "deser_triple_option")]
    introduction: Option<Option<String>>,
    #[serde(default)]
    env: Option<HashMap<String, String>>,
    #[serde(default)]
    dotenv: Option<String>,
}

async fn agents_patch(
    State(state): State<SharedRuntimeState>,
    axum::extract::Path((slug, agent_id)): axum::extract::Path<(String, String)>,
    Json(req): Json<AgentUpdateRequest>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    // Match the validation convention for multi-path-param handlers
    // (`im_card_archive`, `im_channel_archive`, `agents_get`): combining the
    // `WorkspaceSlug` extractor with a `Path<(String, String)>` tuple fails
    // in axum because both consume from the same cached url-params extension.
    if let Err(e) = crate::slug::validate(&slug) {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorBody::new(format!("invalid slug: {e}"))),
        )
            .into_response();
    }

    // 1. Look up agent; clone repo_path so we can release the lock before I/O.
    let repo_root = {
        let s = state.lock().unwrap();
        let ctx = match s.workspaces.get(&slug) {
            Some(c) => c,
            None => return not_found_workspace(),
        };
        match ctx.agents.get(&agent_id) {
            Some(info) => {
                if req.model.is_some() && info.status == "running" {
                    return (
                        StatusCode::CONFLICT,
                        Json(ErrorBody::new("stop the agent before changing model")),
                    )
                        .into_response();
                }
                PathBuf::from(&info.repo_path)
            }
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(ErrorBody::new(format!("agent not found: {agent_id}"))),
                )
                    .into_response();
            }
        }
    };

    // 2. Read + merge me.json (preserves untouched fields like github_email).
    let me_path = repo_root.join(".gitim/me.json");
    let me_content = match std::fs::read_to_string(&me_path) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorBody::new(format!("read me.json failed: {e}"))),
            )
                .into_response();
        }
    };
    let mut me: MeJson = match serde_json::from_str(&me_content) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorBody::new(format!("parse me.json failed: {e}"))),
            )
                .into_response();
        }
    };

    // Three-state semantics for system_prompt:
    //   absent (None)       → no-op
    //   Some(None)          → remove field
    //   Some(Some(""))      → remove field
    //   Some(Some(s))       → set to s
    if let Some(sp_opt) = &req.system_prompt {
        me.system_prompt = match sp_opt {
            Some(s) if !s.is_empty() => Some(s.clone()),
            _ => None,
        };
    }

    let old_model = me.model.clone();
    let mut model_changed = false;

    // Three-state semantics for model mirror system_prompt:
    //   absent (None)       → no-op
    //   Some(None)          → remove field and use provider default
    //   Some(Some(""))      → remove field and use provider default
    //   Some(Some(s))       → set to s
    if let Some(model_opt) = &req.model {
        let new_model = match model_opt {
            Some(s) if !s.is_empty() => Some(s.clone()),
            _ => None,
        };
        model_changed = old_model != new_model;
        me.model = new_model;
    }

    // Env validation + whole-map replacement.
    // absent (None)       → no-op
    // Some({})            → remove "env" field entirely
    // Some({k: v, ...})   → validate keys, then replace wholesale
    if let Some(env_map) = &req.env {
        for key in env_map.keys() {
            if !is_valid_env_key(key) {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorBody::new(format!("invalid env var name: {key}"))),
                )
                    .into_response();
            }
        }
        me.env = if env_map.is_empty() {
            None
        } else {
            Some(env_map.clone().into_iter().collect())
        };
    }

    // dotenv size cap — validated before any disk write for fail-fast.
    if let Some(contents) = &req.dotenv {
        if contents.len() > DOTENV_MAX_BYTES {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorBody::new("dotenv exceeds 64 KB limit")),
            )
                .into_response();
        }
    }

    // Introduction length validation — runs before any disk write so a
    // malformed payload doesn't leave the rest of the patch half-applied.
    // Empty / null both clear the field; the daemon will write "" to the
    // YAML on our behalf.
    let introduction_patch: Option<String> = match &req.introduction {
        None => None,
        Some(opt) => Some(opt.clone().unwrap_or_default()),
    };
    if let Some(intro) = introduction_patch.as_deref() {
        if intro.len() > MAX_INTRODUCTION_LEN {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorBody::new(format!(
                    "introduction exceeds {} byte limit",
                    MAX_INTRODUCTION_LEN
                ))),
            )
                .into_response();
        }
    }

    if let Err(e) = std::fs::write(&me_path, serde_json::to_string_pretty(&me).unwrap()) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorBody::new(format!("write me.json failed: {e}"))),
        )
            .into_response();
    }

    // Sync SOUL.md if the user touched system_prompt and the agent is
    // hermes. We use PreserveUserEdits — if the user has hand-edited
    // SOUL.md since provisioning (no marker), the PATCH succeeds for
    // me.json but the SOUL.md update is refused with a warning. The
    // user can reconcile manually. SOUL.md is hermes-only; other
    // providers don't have one.
    let provider_is_hermes = me.provider.as_deref() == Some("hermes");
    let system_prompt_patched = req.system_prompt.is_some();
    if provider_is_hermes && system_prompt_patched {
        let body = crate::hermes_profile::build_hermes_soul_body(
            &agent_id,
            me.model.as_deref(),
            me.system_prompt.as_deref(),
        );
        match crate::hermes_profile::write_soul_md(
            &agent_id,
            &body,
            crate::hermes_profile::SoulWriteMode::PreserveUserEdits,
        ) {
            Ok(crate::hermes_profile::SoulWriteOutcome::Wrote) => {
                tracing::info!(handler = %agent_id, "SOUL.md synced with patched system_prompt");
            }
            Ok(crate::hermes_profile::SoulWriteOutcome::SkippedUnchanged) => {}
            Ok(crate::hermes_profile::SoulWriteOutcome::RefusedUserEdited) => {
                tracing::warn!(
                    handler = %agent_id,
                    "system_prompt patched but SOUL.md has been hand-edited; \
                     manual reconciliation required"
                );
            }
            Err(e) => {
                tracing::warn!(
                    handler = %agent_id,
                    error = %e,
                    "failed to sync SOUL.md after system_prompt patch"
                );
            }
        }
    }

    if model_changed {
        let state_path = crate::state::AgentState::state_path(&repo_root);
        if state_path.exists() {
            let mut agent_state = match crate::state::AgentState::load(&repo_root) {
                Ok(s) => s,
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorBody::new(format!("read agent state failed: {e}"))),
                    )
                        .into_response();
                }
            };
            agent_state.clear_session();
            if let Err(e) = agent_state.save(&repo_root) {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorBody::new(format!("write agent state failed: {e}"))),
                )
                    .into_response();
            }
        }
    }

    // Write or delete <repo_root>/.env based on dotenv field.
    // File-only: dotenv is kept out of in-memory AgentInfo to avoid secrets
    // leaking into API responses or process memory beyond what's needed.
    //
    // Partial-failure contract: me.json has already been written by this point.
    // If the .env write/delete below fails, the caller sees 500 but me.json is
    // already updated on disk. Accepted trade-off: system_prompt/env updates are
    // idempotent and the client can retry the full PATCH. True atomicity across
    // two files would require a WAL and is out of scope for v1.
    if let Some(contents) = &req.dotenv {
        let env_path = repo_root.join(".env");
        if contents.is_empty() {
            if env_path.exists() {
                if let Err(e) = std::fs::remove_file(&env_path) {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorBody::new(format!("delete .env failed: {e}"))),
                    )
                        .into_response();
                }
            }
        } else {
            if let Err(e) = std::fs::write(&env_path, contents) {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorBody::new(format!("write .env failed: {e}"))),
                )
                    .into_response();
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                // File now contains live secrets. chmod failure must be observable:
                // silent fallthrough leaves 0o644 (world-readable on typical umask)
                // while caller sees 200 OK, defeating the security guarantee.
                match std::fs::metadata(&env_path) {
                    Ok(meta) => {
                        let mut perm = meta.permissions();
                        perm.set_mode(0o600);
                        if let Err(e) = std::fs::set_permissions(&env_path, perm) {
                            tracing::warn!(
                                path = %env_path.display(),
                                error = %e,
                                "failed to set 0600 on .env — file may be world-readable"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            path = %env_path.display(),
                            error = %e,
                            "stat .env after write failed — mode not set"
                        );
                    }
                }
            }
        }
    }

    // Introduction lives in the git-tracked `users/<handler>.meta.yaml`,
    // not me.json — we route it through the daemon so the commit + push
    // flow goes through the same lock/sync paths as register_user. The
    // daemon stays alive even when the agent loop is offline (stop_agent
    // only aborts the loop task), so the IPC connection is reliable.
    if let Some(intro) = introduction_patch.as_deref() {
        let client = GitimClient::new(&repo_root);
        match client.update_user(&agent_id, intro).await {
            Ok(resp) if resp.ok => {}
            Ok(resp) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorBody::new(format!(
                        "update_user failed: {}",
                        resp.error.unwrap_or_else(|| "unknown".into())
                    ))),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorBody::new(format!("update_user IPC failed: {e}"))),
                )
                    .into_response();
            }
        }
    }

    // 3+4. Update in-memory AgentInfo + take fresh snapshot under one lock.
    // Folding these into one acquisition prevents a TOCTOU panic when
    // `agents_remove` lands between the update and the snapshot.  If the agent
    // (or workspace) disappeared mid-flight, caller gets 404 — the on-disk
    // me.json write is harmless residual since the agent dir gets cleaned up
    // on remove anyway.
    let response = {
        let mut s = state.lock().unwrap();
        if let Some(ctx) = s.workspaces.get_mut(&slug) {
            if let Some(info) = ctx.agents.get_mut(&agent_id) {
                if let Some(sp_opt) = &req.system_prompt {
                    info.system_prompt = match sp_opt {
                        Some(s) if !s.is_empty() => Some(s.clone()),
                        _ => None,
                    };
                }
                if let Some(model_opt) = &req.model {
                    info.model = match model_opt {
                        Some(s) if !s.is_empty() => Some(s.clone()),
                        _ => None,
                    };
                    if model_changed {
                        info.session_usage = None;
                    }
                }
                if let Some(env_map) = &req.env {
                    info.env = env_map.clone();
                }
                if introduction_patch.is_some() {
                    info.introduction = introduction_patch.clone();
                }
                Some(info.clone())
            } else {
                None
            }
        } else {
            None
        }
    };

    match response {
        Some(info) => Json(AgentDetailResponse {
            ok: true,
            agent: info,
        })
        .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(ErrorBody::new(format!("agent not found: {agent_id}"))),
        )
            .into_response(),
    }
}

// -- /agents/remove --

/// **DEPRECATED**: replaced by `POST /workspaces/{slug}/agents/burn` (archive-protocol).
///
/// `agents/remove` only deletes the agent's clone directory; it does NOT remove the
/// agent's user.meta.yaml from the shared repo, archive their DMs, write leave-workspace
/// events, or clean their channels meta members. The agent's footprint persists in every
/// other clone of the workspace, defeating the user-facing intent of "remove".
///
/// Use `agents/burn` for the full workspace-wide departure (writes audit events,
/// archives DMs, archives user entry, then physically deletes clone). Use `agents/stop`
/// for non-destructive pause.
///
/// Retained for backward compatibility with older WebUI builds — the current
/// WebUI calls `agents/burn` directly.
async fn agents_remove(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
    Json(req): Json<AgentRemoveRequest>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    tracing::warn!(
        agent_id = %req.id,
        slug = %slug,
        "agents/remove is deprecated; use POST /agents/burn (archive-protocol) or POST /agents/stop (pause)"
    );

    let (workspace_path, repo_path, loop_handle, provider) = {
        let mut s = state.lock().unwrap();
        let ctx = match s.workspaces.get_mut(&slug) {
            Some(c) => c,
            None => return not_found_workspace(),
        };
        match ctx.agents.get_mut(&req.id) {
            Some(info) => {
                let loop_handle = info.loop_handle.take();
                info.status = "idle".to_string();
                (
                    ctx.path.clone(),
                    PathBuf::from(&info.repo_path),
                    loop_handle,
                    info.provider.clone(),
                )
            }
            None => {
                return Json(ErrorBody::new("agent not found")).into_response();
            }
        }
    };

    if let Some(handle) = loop_handle {
        handle.abort();
    }
    kill_agent_daemon(&repo_path);

    if req.hard_delete {
        if let Err(e) = hard_delete_agent_dir(&workspace_path, &req.id, &repo_path) {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorBody::new(e))).into_response();
        }
        // Best-effort: same posture as hermes profile cleanup below.
        // Statistics file failure must not block the user-visible delete.
        if let Err(e) = crate::usage_log::AgentUsageLog::delete(&workspace_path, &req.id) {
            tracing::warn!(
                agent = %req.id,
                error = %e,
                "failed to delete usage log during hard_delete"
            );
        }
        // Best-effort hermes profile cleanup: failures only warn so a
        // missing/broken hermes CLI never blocks the user-facing delete.
        if provider.as_deref() == Some("hermes") {
            if let Err(e) = crate::hermes_profile::delete_profile(&req.id).await {
                tracing::warn!(
                    agent = %req.id,
                    error = %e,
                    "failed to delete hermes profile during hard_delete"
                );
            }
        }
    }

    let mut s = state.lock().unwrap();
    let ctx = match s.workspaces.get_mut(&slug) {
        Some(c) => c,
        None => return not_found_workspace(),
    };
    ctx.agents.remove(&req.id);
    Json(OkAckResponse { ok: true }).into_response()
}

// -- /agents/burn --

/// `POST /workspaces/{slug}/agents/burn { id }` — full archive-protocol
/// departure. See `docs/plans/2026-05-09-archive-protocol/01-plan.md`
/// "Agent burn 工作流" for the contract.
///
/// Steps:
///   1. type-check `id` is in `ctx.agents` — burn is strictly for agents
///      (humans are out of v1 scope; daemon is type-agnostic but the
///      runtime entry point gates on agent-membership here)
///   2. abort the agent loop (so it stops polling/sending)
///   3. ensure the target's daemon is alive, then RPC `depart_user` to it
///      — daemon walks A.4's idempotent multi-commit chain and uses
///      `archive/users/<h>.meta.yaml` as the single source of truth for
///      "depart complete". A daemon RPC failure short-circuits steps
///      4-7; the user retries and the daemon resumes from the first
///      incomplete phase
///   4-7. delegate to [`cleanup_agent_runtime_side`]: kill daemon, rm -rf
///        clone, best-effort hermes profile delete, drop from `ctx.agents`,
///        broadcast `AgentActivityEvent::burned` SSE
///
/// Error codes:
/// - `not_an_agent` — `id` is not an agent in `ctx.agents` (404)
/// - `daemon_unreachable` — couldn't reach daemon: spawn failed,
///   `ensure_daemon_with_log` errored, `spawn_blocking` panicked, or the
///   `depart_user` RPC IO failed (500)
/// - `daemon_depart_failed` — daemon was reachable and replied `ok=false`
///   from `depart_user` (e.g. partial commit chain failure mid-depart);
///   client may retry safely, daemon resumes from first incomplete phase (500)
/// - (filesystem cleanup errors return 500 with no error_code; rare and
///   typically fatal — a permission issue or a stale handle on the clone)
async fn agents_burn(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
    Json(req): Json<AgentIdRequest>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    // Step 1: type-check the target id is an agent in this workspace.
    // The daemon's depart_user is type-agnostic (so unarchive_user/
    // archive_user can serve future "human leaves workspace" needs), but
    // the burn endpoint is strictly for agents. Returning 404 +
    // not_an_agent makes WebUI's "Burn" button safe even if the operator
    // somehow types a human handler — it can't accidentally archive a
    // human user via this path.
    let (workspace_path, repo_path, loop_handle, provider, activity_tx) = {
        let mut s = state.lock().unwrap();
        let ctx = match s.workspaces.get_mut(&slug) {
            Some(c) => c,
            None => return not_found_workspace(),
        };
        match ctx.agents.get_mut(&req.id) {
            Some(info) => {
                let loop_handle = info.loop_handle.take();
                info.status = "idle".to_string();
                (
                    ctx.path.clone(),
                    PathBuf::from(&info.repo_path),
                    loop_handle,
                    info.provider.clone(),
                    ctx.activity_tx.clone(),
                )
            }
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(ErrorBody::with_code(
                        format!("agent not found: {}", req.id),
                        "not_an_agent",
                    )),
                )
                    .into_response();
            }
        }
    };

    // Step 2: abort the in-process agent loop. Stops the agent from
    // sending or polling while the daemon performs its multi-commit
    // depart sequence. We deliberately leave the daemon process
    // running for now — depart_user needs a live socket.
    if let Some(handle) = loop_handle {
        handle.abort();
    }

    // Step 3: ensure the daemon is up, then RPC depart_user.
    //
    // The agent's daemon may have been stopped by a prior `agents/stop`
    // call. We respawn it so the depart_user RPC has a target — it
    // will be killed in step 4 immediately afterward, so this is a
    // brief, scoped revival.
    {
        let repo_root = repo_path.clone();
        let log_path = crate::daemon_log::daemon_log_path(&repo_path);
        let spawn_result = tokio::task::spawn_blocking(move || {
            gitim_client::ensure_daemon_with_log(&repo_root, &log_path)
        })
        .await;
        match spawn_result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorBody::with_code(
                        format!("failed to start agent daemon for burn: {e}"),
                        "daemon_unreachable",
                    )),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorBody::with_code(
                        format!("agent daemon spawn task panicked: {e}"),
                        "daemon_unreachable",
                    )),
                )
                    .into_response();
            }
        }
    }

    let client = GitimClient::new(&repo_path);
    match client.depart_user(&req.id).await {
        Ok(resp) if resp.ok => {}
        Ok(resp) => {
            // Daemon was reachable but replied `ok=false`. Semantically
            // distinct from RPC IO failure: daemon is up, we got a
            // response, the depart logic itself refused or failed
            // partway. We don't execute steps 4-7 — leaving the clone +
            // ctx.agents intact lets the user retry, and the daemon's
            // terminal-state judgment (archive/users/<h>) makes the
            // retry idempotent.
            let detail = resp
                .error
                .unwrap_or_else(|| "daemon returned ok=false without error message".to_string());
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorBody::with_code(
                    format!("daemon depart_user failed: {detail}"),
                    "daemon_depart_failed",
                )),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorBody::with_code(
                    format!("daemon depart_user RPC failed: {e}"),
                    "daemon_unreachable",
                )),
            )
                .into_response();
        }
    }

    // Steps 4-7: shared cleanup with the self-departed self-heal path
    // (B.4). Both call sites must produce identical end state — see
    // `cleanup_agent_runtime_side` for the contract.
    match cleanup_agent_runtime_side(
        &state,
        &slug,
        &req.id,
        &workspace_path,
        &repo_path,
        provider.as_deref(),
        &activity_tx,
    )
    .await
    {
        Ok(()) => Json(OkAckResponse { ok: true }).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorBody::new(e))).into_response(),
    }
}

/// Steps 4-7 of the burn workflow, factored out so the self-departed
/// self-heal path (B.4) can reuse it. Both call sites must converge on
/// identical end state: agent daemon dead, clone dir gone, hermes profile
/// gone (if applicable), `ctx.agents` does not contain `agent_id`, and a
/// `burned` `AgentActivityEvent` was broadcast on `activity_tx`.
///
/// Hard-delete failure is the only condition that returns `Err` — hermes
/// cleanup is best-effort (warn on failure) and `ctx.agents.remove` /
/// SSE broadcast cannot fail in a way the caller could recover from.
///
/// Concurrency: re-locks `SharedRuntimeState` at the end to remove the
/// agent. If the workspace was dropped mid-flight, the remove is a no-op
/// — this matches the pre-extraction behavior, which assumed the
/// workspace would still be present (it almost always is, since the
/// caller already verified it before this point).
pub(crate) async fn cleanup_agent_runtime_side(
    state: &SharedRuntimeState,
    slug: &str,
    agent_id: &str,
    workspace_path: &Path,
    repo_path: &Path,
    provider: Option<&str>,
    activity_tx: &tokio::sync::broadcast::Sender<AgentActivityEvent>,
) -> Result<(), String> {
    // Step 4: kill the agent's daemon process.
    kill_agent_daemon(repo_path);

    // Step 5: rm -rf the clone dir. Daemon already wrote the depart
    // commits to the shared remote (in the burn path) or self-burn
    // already wrote them (in the B.4 self-heal path), so losing the
    // local clone's working tree is safe.
    hard_delete_agent_dir(workspace_path, agent_id, repo_path)?;

    // Step 6: best-effort hermes profile cleanup. Failures only warn —
    // a missing/broken hermes CLI must not block cleanup since the
    // workspace-side depart already succeeded.
    if provider == Some("hermes") {
        if let Err(e) = crate::hermes_profile::delete_profile(agent_id).await {
            tracing::warn!(
                agent = %agent_id,
                error = %e,
                "failed to delete hermes profile during cleanup"
            );
        }
    }

    // Step 7: drop from in-memory ctx.agents + emit SSE so the WebUI
    // refreshes its agent list without polling. Workspace-not-found at
    // this stage is a no-op (workspace was dropped concurrently — rare,
    // and the agent is already gone from a user-visible standpoint).
    {
        let mut s = state.lock().unwrap();
        if let Some(ctx) = s.workspaces.get_mut(slug) {
            ctx.agents.remove(agent_id);
        }
    }

    let _ = activity_tx.send(AgentActivityEvent {
        agent_id: agent_id.to_string(),
        workspace_id: slug.to_string(),
        event_type: "burned".to_string(),
        detail: format!("agent @{agent_id} departed the workspace"),
        timestamp: chrono::Utc::now().to_rfc3339(),
    });

    Ok(())
}

fn kill_agent_daemon(repo_path: &Path) {
    let pid_file = repo_path.join(".gitim/run/gitim.pid");
    if let Ok(content) = std::fs::read_to_string(&pid_file) {
        if let Ok(pid) = content.trim().parse::<u32>() {
            let _ = std::process::Command::new("kill")
                .arg(pid.to_string())
                .output();
        }
    }
}

fn hard_delete_agent_dir(workspace: &Path, agent_id: &str, repo_path: &Path) -> Result<(), String> {
    if !repo_path.is_absolute() {
        return Err("agent repo path is not absolute".to_string());
    }

    let workspace = std::fs::canonicalize(workspace)
        .map_err(|e| format!("failed to resolve workspace path: {e}"))?;
    let parent = repo_path
        .parent()
        .ok_or_else(|| "agent repo path has no parent".to_string())?;
    let parent = std::fs::canonicalize(parent)
        .map_err(|e| format!("failed to resolve agent parent path: {e}"))?;
    if parent != workspace {
        return Err("agent repo path is outside the workspace".to_string());
    }

    let Some(name) = repo_path.file_name().and_then(|s| s.to_str()) else {
        return Err("agent repo path has no valid directory name".to_string());
    };
    if name != agent_id {
        return Err("agent repo path does not match the agent id".to_string());
    }

    if !repo_path.exists() {
        return Ok(());
    }

    let target = std::fs::canonicalize(repo_path)
        .map_err(|e| format!("failed to resolve agent repo path: {e}"))?;
    if target == workspace || !target.starts_with(&workspace) {
        return Err("agent repo path is outside the workspace".to_string());
    }
    if !target.is_dir() {
        return Err("agent repo path is not a directory".to_string());
    }

    // Retry on ENOTEMPTY / EBUSY — the SIGTERM-vs-rm race.
    //
    // `cleanup_agent_runtime_side` SIGTERMs the agent's daemon
    // (`kill_agent_daemon`) without waiting for exit, then immediately
    // calls into here. On macOS (and occasionally Linux under load) the
    // daemon's signal handler is still tearing down `.gitim/run/` while
    // `remove_dir_all` walks it, surfacing as `ENOTEMPTY` (errno 66) on
    // an intermediate directory. The daemon finishes exiting within
    // ~hundreds of ms, so a few retries with short backoff is enough to
    // converge — and `remove_dir_all` itself is idempotent over partial
    // state, so the second pass picks up where the first left off.
    //
    // 50 / 100 / 150 ms backoff fits inside a normal cleanup turn (the
    // `tests/burn_test.rs::burn_with_idempotent_retry` helper waits 500 ms
    // between attempts, so we're well under that even in the worst case).
    // Past 3 attempts: bubble the error up. The caller sees the same 5xx
    // it would have without retry, plus we've at least proven this isn't
    // the daemon-shutdown race (which never takes >450 ms in practice).
    const MAX_ATTEMPTS: u32 = 3;
    const BACKOFF_MS: u64 = 50;
    let mut last_err: Option<String> = None;
    for attempt in 1..=MAX_ATTEMPTS {
        match std::fs::remove_dir_all(&target) {
            Ok(()) => return Ok(()),
            Err(e) => {
                let retriable = matches!(
                    e.raw_os_error(),
                    // ENOTEMPTY (Linux 39, macOS 66) and EBUSY (Linux/macOS 16):
                    // the daemon is still releasing handles or its signal
                    // handler is mid-flight. ENOENT means a parallel cleanup
                    // beat us to it; we treat that as success below.
                    Some(39) | Some(66) | Some(16)
                );
                let already_gone = e.kind() == std::io::ErrorKind::NotFound;
                if already_gone {
                    return Ok(());
                }
                if retriable && attempt < MAX_ATTEMPTS {
                    let backoff = std::time::Duration::from_millis(BACKOFF_MS * attempt as u64);
                    std::thread::sleep(backoff);
                    last_err = Some(format!("{e}"));
                    continue;
                }
                return Err(format!("failed to delete agent directory: {e}"));
            }
        }
    }
    Err(format!(
        "failed to delete agent directory after {MAX_ATTEMPTS} attempts: {}",
        last_err.unwrap_or_else(|| "unknown".to_string()),
    ))
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
                return Json(ErrorBody::new(format!("agent not found: {}", req.id)))
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

    Json(OkAckResponse { ok: true }).into_response()
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

// -- /fleet/* --

async fn fleet_events(
    State(state): State<SharedRuntimeState>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let rx = {
        let s = state.lock().unwrap();
        s.fleet_tx.subscribe()
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

async fn fleet_nodes_list(State(state): State<SharedRuntimeState>) -> Json<FleetNodesListResponse> {
    let nodes = {
        let s = state.lock().unwrap();
        s.fleet_nodes
            .values()
            .map(|node| node.entry.clone())
            .collect()
    };
    Json(FleetNodesListResponse { ok: true, nodes })
}

async fn fleet_status(State(state): State<SharedRuntimeState>) -> Json<FleetStatusResponse> {
    let mut nodes: Vec<_> = {
        let s = state.lock().unwrap();
        s.fleet_status.values().cloned().collect()
    };
    nodes.sort_by(|a, b| {
        a.node_id
            .cmp(&b.node_id)
            .then_with(|| a.workspace_id.cmp(&b.workspace_id))
    });
    Json(FleetStatusResponse { ok: true, nodes })
}

async fn fleet_agents(State(state): State<SharedRuntimeState>) -> Json<FleetAgentsResponse> {
    let agents = crate::fleet::fetch_agent_snapshots(&state).await;
    Json(FleetAgentsResponse { ok: true, agents })
}

async fn fleet_nodes_upsert(
    State(state): State<SharedRuntimeState>,
    Json(req): Json<crate::user_config::FleetNodeEntry>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    let entry = crate::fleet::normalize_node(req);
    if let Err(err) = crate::fleet::validate_node(&entry) {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorBody::with_code(err, "invalid_fleet_node")),
        )
            .into_response();
    }
    let entry = match crate::fleet::resolve_workspace_mappings(&state, entry).await {
        Ok(entry) => entry,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorBody::with_code(err, "no_matching_fleet_workspace")),
            )
                .into_response()
        }
    };

    let mut cfg = crate::user_config::read();
    cfg.upsert_fleet_node(entry.clone());
    if let Err(err) = crate::user_config::write(&cfg) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorBody::with_code(
                format!("failed to persist fleet node: {err}"),
                "fleet_config_write_failed",
            )),
        )
            .into_response();
    }

    crate::fleet::activate_node(state, entry.clone());
    Json(FleetNodeUpsertResponse {
        ok: true,
        node: entry,
    })
    .into_response()
}

async fn fleet_nodes_delete(
    State(state): State<SharedRuntimeState>,
    axum::extract::Path(node_id): axum::extract::Path<String>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    let mut cfg = crate::user_config::read();
    let config_existed = cfg.remove_fleet_node(&node_id);
    let runtime_existed = {
        let s = state.lock().unwrap();
        s.fleet_nodes.contains_key(&node_id)
    };
    if !config_existed && !runtime_existed {
        return (
            StatusCode::NOT_FOUND,
            Json(ErrorBody::with_code("fleet node not found", "not_found")),
        )
            .into_response();
    }

    if let Err(err) = crate::user_config::write(&cfg) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorBody::with_code(
                format!("failed to persist fleet node removal: {err}"),
                "fleet_config_write_failed",
            )),
        )
            .into_response();
    }

    crate::fleet::remove_node(&state, &node_id);
    Json(OkAckResponse { ok: true }).into_response()
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
    let mut seen_paths: HashSet<PathBuf> = HashSet::new();
    for entry in cfg.workspaces {
        let workspace = PathBuf::from(&entry.path);
        if !workspace.exists() {
            tracing::warn!(slug=%entry.slug, path=%entry.path, "workspace path missing; skip");
            continue;
        }
        let recovered_path = match workspace.canonicalize() {
            Ok(path) => path,
            Err(e) => {
                tracing::warn!(slug=%entry.slug, path=%entry.path, error=%e, "workspace path canonicalization failed; using configured path");
                workspace.clone()
            }
        };
        if !seen_paths.insert(recovered_path) {
            tracing::warn!(slug=%entry.slug, path=%entry.path, "workspace path already recovered; skipping duplicate entry");
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
                ..
            }) => {
                let token_url = match parse_github_url(url) {
                    Ok((owner, repo)) => build_token_url(&owner, &repo, token),
                    Err(_) => url.clone(),
                };
                (
                    token_url,
                    "github".to_string(),
                    gitim_core::auth_payload::AuthPayload::GitHub {
                        token: token.clone(),
                    },
                )
            }
            _ => {
                let remote = workspace.join("repo.git").to_string_lossy().into_owned();
                let (handler, display_name) = infer_local_human_identity(&human_dir);
                (
                    remote,
                    "git".to_string(),
                    gitim_core::auth_payload::AuthPayload::Git {
                        handler,
                        display_name,
                        github_email: None,
                    },
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
pub async fn recover_agents_for_workspace(state: SharedRuntimeState, slug: &str, workspace: &Path) {
    {
        let s = state.lock().unwrap();
        if !s.workspaces.contains_key(slug) {
            tracing::warn!(slug=%slug, "workspace missing during agent recovery; skipping scan");
            return;
        }
    }

    let entries = match std::fs::read_dir(workspace) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "repo.git" || name.starts_with('.') {
            continue;
        }

        let me_path = dir.join(".gitim/me.json");
        if !me_path.exists() {
            continue;
        }

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
        let display_name = me["display_name"].as_str().unwrap_or(&handler).to_string();

        let model = me["model"].as_str().map(|s| s.to_string());
        let custom_system_prompt = me["system_prompt"].as_str().map(|s| s.to_string());
        let env: HashMap<String, String> = me
            .get("env")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let llm_provider_val = me["llm_provider"].as_str().map(|s| s.to_string());
        let llm_model_val = me["llm_model"].as_str().map(|s| s.to_string());

        let provider_raw = me["provider"].as_str();
        let provider_error = match provider_raw {
            None => Some(format!(
                "Missing \"provider\" in {}. Add \"provider\": \"claude\", \"codex\", \"opencode\", \"pi\", \"hermes\", \"cursor\", or \"kimi\" to the file and restart the runtime.",
                me_path.display()
            )),
            Some(p) if p != "claude" && p != "codex" && p != "opencode" && p != "pi" && p != "hermes" && p != "cursor" && p != "kimi" => {
                Some(format!(
                    "Unsupported provider \"{}\" in {}. Expected \"claude\", \"codex\", \"opencode\", \"pi\", \"hermes\", \"cursor\", or \"kimi\".",
                    p,
                    me_path.display()
                ))
            }
            Some(_) => None,
        };

        if let Some(msg) = provider_error {
            tracing::warn!("agent @{handler} recovered in error state: {msg}");
            let activity_tx = {
                let s = state.lock().unwrap();
                match s.workspaces.get(slug) {
                    Some(ctx) => ctx.activity_tx.clone(),
                    None => {
                        tracing::warn!(
                            slug=%slug,
                            handler=%handler,
                            "workspace missing during agent recovery; skipping agent"
                        );
                        continue;
                    }
                }
            };
            let _ = activity_tx.send(AgentActivityEvent {
                agent_id: handler.clone(),
                workspace_id: slug.to_string(),
                event_type: "error".to_string(),
                detail: msg.clone(),
                timestamp: chrono::Utc::now().to_rfc3339(),
            });
            let mut s = state.lock().unwrap();
            match s.workspaces.get_mut(slug) {
                Some(ctx) => {
                    ctx.agents.insert(
                        handler.clone(),
                        AgentInfo {
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
                            introduction: read_user_introduction(&dir, &handler),
                            env,
                            error_message: Some(msg),
                            session_usage: crate::state::AgentState::load(&dir)
                                .ok()
                                .and_then(|s| s.session_usage),
                            llm_provider: llm_provider_val.clone(),
                            llm_model: llm_model_val.clone(),
                            usage_summary: load_usage_summary_for_recovery(workspace, &handler),
                            loop_handle: None,
                        },
                    );
                }
                None => {
                    tracing::warn!(
                        slug=%slug,
                        handler=%handler,
                        "workspace missing during agent recovery; skipping agent"
                    );
                }
            }
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
            match s.workspaces.get_mut(slug) {
                Some(ctx) => {
                    ctx.agents.insert(
                        handler.clone(),
                        AgentInfo {
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
                            introduction: read_user_introduction(&dir, &handler),
                            env,
                            error_message: None,
                            session_usage: crate::state::AgentState::load(&dir)
                                .ok()
                                .and_then(|s| s.session_usage),
                            llm_provider: llm_provider_val,
                            llm_model: llm_model_val,
                            usage_summary: load_usage_summary_for_recovery(workspace, &handler),
                            loop_handle: None,
                        },
                    );
                }
                None => {
                    tracing::warn!(
                        slug=%slug,
                        handler=%handler,
                        "workspace missing during agent recovery; skipping agent"
                    );
                    continue;
                }
            }
        }

        match start_agent_loop(&state, slug, &handler) {
            Ok(()) => tracing::info!("agent @{handler} recovered and started"),
            Err(e) => tracing::warn!("agent @{handler} recovered but auto-start failed: {e}"),
        }
    }
}

/// Query parameters for `GET /preflight/{provider}`.
///
/// Currently only consumed by the `hermes` branch, which forwards them to
/// `preflight_hermes_with`. All other providers silently ignore the fields.
#[derive(Deserialize, Default)]
struct PreflightQuery {
    llm_provider: Option<String>,
    llm_model: Option<String>,
}

/// HTTP handler for `GET /preflight/{provider}`.
///
/// Dispatches to the matching provider's real-hello preflight. Unknown
/// providers return 400 with a stable `{"ok": false, "error": ...}` shape so
/// the WebUI can branch without parsing provider-specific fields.
///
/// The `hermes` branch accepts optional `llm_provider` and `llm_model` query
/// parameters to override the LLM used for the preflight hello.
async fn preflight_handler(
    axum::extract::Path(provider): axum::extract::Path<String>,
    axum::extract::Query(params): axum::extract::Query<PreflightQuery>,
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
        "opencode" => {
            let result = crate::preflight::preflight_opencode().await;
            (StatusCode::OK, Json(result)).into_response()
        }
        "pi" => {
            let result = crate::preflight::preflight_pi().await;
            (StatusCode::OK, Json(result)).into_response()
        }
        "hermes" => {
            let result = crate::preflight::preflight_hermes_with(
                "hermes",
                std::time::Duration::from_secs(30),
                None,
                params.llm_provider.as_deref(),
                params.llm_model.as_deref(),
                None,
            )
            .await;
            (StatusCode::OK, Json(result)).into_response()
        }
        "cursor" => {
            let result = crate::preflight::preflight_cursor().await;
            (StatusCode::OK, Json(result)).into_response()
        }
        "kimi" => {
            let result = crate::preflight::preflight_kimi().await;
            (StatusCode::OK, Json(result)).into_response()
        }
        _ => (
            StatusCode::BAD_REQUEST,
            Json(ErrorBody::new("unknown provider")),
        )
            .into_response(),
    }
}

/// HTTP handler for `GET /providers/{provider}/models`.
///
/// Returns the provider's runtime-visible model catalog when the backing CLI
/// exposes one. Catalog lookup failures stay inside the JSON `error` field so
/// clients can still offer "Use CLI default" and custom model input.
async fn provider_models_handler(
    axum::extract::Path(provider): axum::extract::Path<String>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    let result = crate::model_catalog::list_provider_models(&provider).await;
    (StatusCode::OK, Json(result)).into_response()
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
    #[serde(skip_serializing_if = "Option::is_none")]
    remote_identity: Option<String>,
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
        remote_identity: ctx
            .git_config
            .as_ref()
            .and_then(|config| config.git.remote_identity()),
        initialized: workspace_initialized(ctx),
    }
}

async fn workspaces_list(State(state): State<SharedRuntimeState>) -> Json<WorkspacesListResponse> {
    let s = state.lock().unwrap();
    let mut workspaces: Vec<WorkspaceSummary> =
        s.workspaces.values().map(workspace_summary).collect();
    // Deterministic order makes the response stable for tests and WebUI.
    workspaces.sort_by(|a, b| a.slug.cmp(&b.slug));
    Json(WorkspacesListResponse { workspaces })
}

async fn workspaces_get(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
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
            let body = WorkspaceDetailResponse {
                slug: ctx.slug.clone(),
                workspace_name: ctx.workspace_name.clone(),
                path: ctx.path.to_string_lossy().into_owned(),
                provider,
                initialized: workspace_initialized(ctx),
                agents_count: ctx.agents.len(),
                human_repo: ctx
                    .human_repo
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned()),
            };
            Json(body).into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(ErrorBody::new("unknown workspace")),
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
    WorkspaceSlug(slug): WorkspaceSlug,
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
                    Json(ErrorBody::new("unknown workspace")),
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
                Json(ErrorBody::with_code(
                    format!(
                        "workspace removed from memory and daemons stopped, but ~/.gitim/runtime.json write failed: {e}. Next runtime start will try to recover this workspace.",
                    ),
                    "config_write_failed",
                )),
            )
                .into_response();
        }
    }

    Json(OkAckResponse { ok: true }).into_response()
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
        .map_err(|e| {
            (
                "clone_failed",
                redacted_url(&format!("failed to run git: {e}")),
            )
        })?;
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
        if h.is_empty() {
            "human".to_string()
        } else {
            h
        }
    };
    let auth = gitim_core::auth_payload::AuthPayload::Git {
        handler,
        display_name,
        github_email: None,
    };

    let human_dir = provision_human(workspace, &remote_url, "git", auth)
        .await
        .map_err(|e| {
            (
                "onboard_failed",
                redacted_url(&format!("provision_human failed: {e}")),
            )
        })?;

    apply_default_gitignore(&human_dir);

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
                (
                    "clone_failed",
                    redacted_url(&format!("failed to run git: {e}")),
                )
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

        let auth = gitim_core::auth_payload::AuthPayload::GitHub {
            token: token.clone(),
        };
        let final_human = provision_human(workspace, &remote_url, "github", auth)
            .await
            .map_err(|e| {
                cleanup_human_dir(workspace);
                (
                    "onboard_failed",
                    redacted_url(&format!("provision_human failed: {e}")),
                )
            })?;

        apply_default_gitignore(&final_human);

        // Best-effort email fetch: a failure or null email (private account)
        // falls back to the `<handler>@gitim` sentinel. Never blocks init —
        // the workspace is already usable without it.
        let github_email = match github_api.fetch_user_email(&token).await {
            Ok(email) => email,
            Err(e) => {
                tracing::warn!(
                    "fetch_user_email failed, agent commits will fallback: {}",
                    redacted_url(&e.to_string())
                );
                None
            }
        };

        let config = WorkspaceConfig {
            workspace: workspace.to_string_lossy().into_owned(),
            created_at: chrono::Utc::now().to_rfc3339(),
            git: GitConfig {
                provider: GitProvider::Github,
                remote_url: Some(remote_url.clone()),
                token: Some(token.clone()),
                github_email,
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
            Json(ErrorBody::with_code(
                format!("workspace is inside {service} — refusing to store a token there"),
                "cloud_sync_path_rejected",
            )),
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
                Json(WorkspacePathExistsError {
                    ok: false,
                    error_code: "workspace_path_exists",
                    error: format!(
                        "workspace at {} already registered as slug \"{}\"",
                        workspace.display(),
                        existing_slug,
                    ),
                    existing_slug,
                }),
            )
                .into_response();
        }

        let candidate = crate::slug::normalize(&basename_raw);
        let existing: std::collections::HashSet<String> = s.workspaces.keys().cloned().collect();
        let slug = crate::slug::resolve(&candidate, &existing);

        if s.workspaces.contains_key(&slug) {
            return (
                StatusCode::CONFLICT,
                Json(ErrorBody::with_code(
                    format!("slug collision not resolved: {slug}"),
                    "slug_conflict_unexpected",
                )),
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
                        Json(ErrorBody::with_code(
                            "github mode requires a personal access token",
                            "missing_token",
                        )),
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
                        Json(ErrorBody::with_code(
                            "github mode requires remote_url",
                            "missing_remote_url",
                        )),
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
                Json(ErrorBody::with_code(
                    format!("provider not supported: {other}"),
                    "provider_not_supported",
                )),
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
                Json(ErrorBody::with_code(message, error_code)),
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
                    Json(ErrorBody::with_code(
                        "workspace slot disappeared during provisioning",
                        "slug_conflict_unexpected",
                    )),
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
            Json(ErrorBody::with_code(
                format!("workspace provisioned but ~/.gitim/runtime.json write failed: {e}"),
                "config_write_failed",
            )),
        )
            .into_response();
    }

    (
        StatusCode::CREATED,
        Json(WorkspaceCreateResponse {
            ok: true,
            slug,
            workspace_name,
            path: workspace.to_string_lossy().into_owned(),
            provider: provider_for_response,
        }),
    )
        .into_response()
}

/// HTTP handler for `GET /hermes/llm/providers`.
///
/// Resolves the hermes home directory from `HERMES_HOME` (or `~/.hermes`),
/// delegates to `hermes_llm::list_selectable_providers`, and returns the
/// result as `{"providers": [...]}`. Always 200 so the WebUI degrades
/// gracefully.
async fn list_hermes_llm_providers() -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    let hermes_home = std::env::var_os("HERMES_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/"))
                .join(".hermes")
        });

    let providers = crate::hermes_llm::list_selectable_providers(&hermes_home);
    (
        StatusCode::OK,
        Json(serde_json::json!({ "providers": providers })),
    )
        .into_response()
}

/// HTTP handler for `GET /hermes/llm/providers/{id}/models`.
///
/// Resolution order for `provider_id`:
///
/// 1. Call `hermes_llm::list_providers` (reads `.env` + `config.yaml`).  If
///    the id is found there, use the fully-resolved `LlmProvider` (correct
///    kimi-coding URL, custom entries, etc.) and call `fetch_models`.
/// 2. If not found but the id matches a `BUILTIN_PROVIDERS` entry, construct a
///    minimal `LlmProvider` from the static registry and call `fetch_models` —
///    which will return `error: "missing api key …"` (the provider is known but
///    not configured).  HTTP status is still 200.
/// 3. If the id starts with `"custom:"` but isn't in `list_providers`, return
///    400 — the user asked for a named custom provider that doesn't exist in
///    their `config.yaml`.
/// 4. Otherwise (completely unknown id) → 400.
///
/// All upstream failures (missing key, network error, HTTP 5xx, etc.) produce
/// HTTP 200 with the error embedded in the `error` field.  HTTP 400 is
/// reserved exclusively for unrecognisable provider ids.
async fn list_hermes_llm_models(
    axum::extract::Path(provider_id): axum::extract::Path<String>,
) -> axum::response::Response {
    use crate::hermes_llm::{LlmProvider, ProviderKind, BUILTIN_PROVIDERS};
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    let hermes_home = std::env::var_os("HERMES_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/"))
                .join(".hermes")
        });

    // Step 1: try the live-configured provider list first (correct URLs, custom
    // entries read from config.yaml).
    let live_providers = crate::hermes_llm::list_providers(&hermes_home);
    if let Some(provider) = live_providers.iter().find(|p| p.id == provider_id) {
        let result = crate::hermes_llm::fetch_models(provider, &hermes_home).await;
        return (StatusCode::OK, Json(result)).into_response();
    }

    // Step 2: id matches a builtin but user hasn't configured a key yet.
    if let Some(bp) = BUILTIN_PROVIDERS.iter().find(|p| p.id == provider_id) {
        let provider = LlmProvider {
            id: bp.id.to_owned(),
            label: bp.label.to_owned(),
            kind: ProviderKind::ApiKey,
            base_url: Some(bp.base_url.to_owned()),
            api_protocol: bp.api_protocol,
        };
        let result = crate::hermes_llm::fetch_models(&provider, &hermes_home).await;
        return (StatusCode::OK, Json(result)).into_response();
    }

    // Step 3 & 4: unknown or unreachable custom provider → 400.
    let msg = if provider_id.starts_with("custom:") {
        let name = &provider_id["custom:".len()..];
        format!("custom provider '{name}' not found in config.yaml")
    } else {
        format!("unknown provider id '{provider_id}'")
    };
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": msg })),
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
/// at startup (before any binary self-replace). Self-update reads this
/// from `state.canonical_exe_path`.
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
        .route("/im/boards", get(im_list_boards))
        .route("/im/boards/{handler}", get(im_show_board))
        .route("/im/board/init", post(im_board_init))
        .route("/im/board/publish", post(im_board_publish))
        .route("/im/board/field", post(im_board_field))
        .route("/im/board/section/set", post(im_board_section_set))
        .route("/im/board/section/append", post(im_board_section_append))
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
        .route(
            "/im/cards/{channel}/{card_id}/archive",
            post(im_card_archive),
        )
        .route(
            "/im/cards/{channel}/{card_id}/unarchive",
            post(im_card_unarchive),
        )
        .route("/im/channels/archived", get(im_list_archived_channels))
        .route("/im/channels/{name}/archive", post(im_channel_archive))
        .route("/im/channels/{name}/unarchive", post(im_channel_unarchive))
        .route("/im/dm/archived", get(im_list_archived_dms))
        .route("/im/dm/{peer}/archive", post(im_dm_archive))
        .route("/im/dm/{peer}/unarchive", post(im_dm_unarchive))
        .route("/users/archived", get(users_list_archived))
        .route("/users/{handler}/unarchive", post(users_unarchive))
        // Cron read endpoints — list / timeline / detail / run history /
        // single-run body. The fixed-prefix `timeline` route MUST come
        // before `/crons/{name}` so axum doesn't try to match the literal
        // word as a cron name (which would 404 for any populated workspace).
        .route("/crons", get(crons_list))
        .route("/crons/timeline", get(crons_timeline))
        .route("/crons/{name}", get(crons_show))
        .route("/crons/{name}/runs", get(crons_runs_list))
        .route("/crons/{name}/runs/{ts}", get(crons_run_body))
        // Flow routes — list/show/validate return raw JSON (cronRequest pattern);
        // create/remove use HTTP status codes for success/failure detection.
        // Fixed-prefix `validate` route must come before `/{slug}` to avoid
        // axum matching the literal word as a flow slug.
        .route("/im/flows", get(flows_list).post(flows_create))
        .route("/im/flows/{flow_slug}/validate", get(flows_validate))
        .route(
            "/im/flows/{flow_slug}",
            get(flows_show).delete(flows_remove),
        )
        .route("/agents", get(agents_list))
        .route("/agents/events", get(agents_events))
        .route("/agents/add", post(agents_add))
        .route("/agents/start", post(agents_start))
        .route("/agents/stop", post(agents_stop))
        .route("/agents/remove", post(agents_remove))
        .route("/agents/burn", post(agents_burn))
        .route("/agents/{id}", get(agents_get).patch(agents_patch));

    let router = Router::new()
        .route("/health", get(health))
        .route("/workspaces", get(workspaces_list).post(workspaces_create))
        .route(
            "/workspaces/{slug}",
            get(workspaces_get).delete(workspaces_delete),
        )
        .nest("/workspaces/{slug}", ws_router)
        .route("/fleet/events", get(fleet_events))
        .route("/fleet/status", get(fleet_status))
        .route("/fleet/agents", get(fleet_agents))
        .route(
            "/fleet/nodes",
            get(fleet_nodes_list).post(fleet_nodes_upsert),
        )
        .route("/fleet/nodes/{node_id}", delete(fleet_nodes_delete))
        .route("/preflight/{provider}", get(preflight_handler))
        .route("/providers/{provider}/models", get(provider_models_handler))
        .route("/hermes/llm/providers", get(list_hermes_llm_providers))
        .route(
            "/hermes/llm/providers/{id}/models",
            get(list_hermes_llm_models),
        )
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

/// Apply the curated default .gitignore pattern set to the human clone and
/// commit if anything was added. Best-effort — failures are logged, not
/// propagated; missing rules are cosmetic in isolation but matter in
/// aggregate (see crate::gitignore for the rationale).
fn apply_default_gitignore(human_clone: &Path) {
    match ensure_defaults_gitignored(human_clone) {
        Ok(false) => {}
        Ok(true) => {
            let add = std::process::Command::new("git")
                .args(["add", ".gitignore"])
                .current_dir(human_clone)
                .output();
            match &add {
                Ok(o) if !o.status.success() => {
                    tracing::warn!(
                        stderr = %String::from_utf8_lossy(&o.stderr),
                        "git add .gitignore failed"
                    );
                    return;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "git add .gitignore spawn failed");
                    return;
                }
                _ => {}
            }
            let commit = std::process::Command::new("git")
                .args([
                    "-c",
                    "user.email=system@gitim",
                    "-c",
                    "user.name=system",
                    "commit",
                    "-m",
                    "chore: gitignore agent memory + local artifact patterns (runtime init)",
                ])
                .current_dir(human_clone)
                .output();
            match &commit {
                Ok(o) if !o.status.success() => {
                    tracing::warn!(
                        stderr = %String::from_utf8_lossy(&o.stderr),
                        "git commit .gitignore failed"
                    );
                }
                Err(e) => {
                    tracing::warn!(error = %e, "git commit .gitignore spawn failed");
                }
                _ => {}
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "ensure_defaults_gitignored failed");
        }
    }
}

/// Validate an environment variable key name.
///
/// Rejects empty strings, keys starting with a digit or non-ASCII character,
/// and keys containing anything other than ASCII alphanumerics or underscores.
/// (POSIX convention: `[A-Za-z_][A-Za-z0-9_]*`.)
fn is_valid_env_key(k: &str) -> bool {
    if k.is_empty() {
        return false;
    }
    let bytes = k.as_bytes();
    let first = bytes[0];
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return false;
    }
    bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

#[cfg(test)]
mod tests {
    //! Unit tests for the `/workspaces` request/response types.
    //! Full HTTP integration coverage — lifecycle with real filesystem,
    //! slug collisions, 404s, error bodies — lives in
    //! `tests/http_workspaces.rs`.

    use super::*;

    fn write_persistent_human_repo(workspace: &Path) {
        let human = workspace.join(".gitim-runtime").join("human");
        std::fs::create_dir_all(human.join(".git")).unwrap();
        std::fs::create_dir_all(human.join(".gitim")).unwrap();
        std::fs::write(human.join(".gitim").join("me.json"), "{}").unwrap();
    }

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
        assert_eq!(
            req.git.remote_url.as_deref(),
            Some("https://github.com/org/repo")
        );
        assert_eq!(req.git.token.as_deref(), Some("ghp_x"));
    }

    #[test]
    fn workspace_summary_round_trips() {
        let summary = WorkspaceSummary {
            slug: "frontend".to_string(),
            workspace_name: "Frontend".to_string(),
            path: "/ws/frontend".to_string(),
            provider: GitProvider::Local,
            remote_identity: None,
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
                github_email: None,
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

    #[test]
    fn workspace_summary_treats_persistent_human_repo_as_initialized() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("room");
        write_persistent_human_repo(&workspace);

        let ctx = crate::workspace::WorkspaceContext::new(
            "room".to_string(),
            "Room".to_string(),
            workspace,
        );

        let summary = workspace_summary(&ctx);
        assert!(ctx.human_repo.is_none());
        assert!(summary.initialized);
    }

    #[tokio::test]
    async fn im_channels_reports_unavailable_when_human_daemon_is_not_recovered() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("room");
        write_persistent_human_repo(&workspace);

        let (router, state) = create_router();
        {
            let mut s = state.lock().unwrap();
            s.workspaces.insert(
                "room".to_string(),
                crate::workspace::WorkspaceContext::new(
                    "room".to_string(),
                    "Room".to_string(),
                    workspace,
                ),
            );
        }

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/workspaces/room/im/channels")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["ok"], false);
        assert_eq!(body["error"], "human daemon unavailable");
    }

    #[tokio::test]
    async fn im_boards_route_reaches_workspace_lookup() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let (router, _state) = create_router();
        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/workspaces/missing/im/boards")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["ok"], false);
        assert_eq!(body["error"], "unknown workspace");
    }

    #[tokio::test]
    async fn workspaces_get_separates_initialized_from_recovered_human_daemon() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("room");
        write_persistent_human_repo(&workspace);

        let (router, state) = create_router();
        {
            let mut s = state.lock().unwrap();
            s.workspaces.insert(
                "room".to_string(),
                crate::workspace::WorkspaceContext::new(
                    "room".to_string(),
                    "Room".to_string(),
                    workspace,
                ),
            );
        }

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/workspaces/room")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["initialized"], true);
        assert_eq!(body["human_repo"], serde_json::Value::Null);
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

    #[tokio::test]
    async fn health_response_includes_runtime_id() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let (router, state) = create_router();
        // 模拟启动期注入
        state.lock().unwrap().runtime_id = "test-runtime-id-1234".to_string();

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            body.get("runtime_id").and_then(|v| v.as_str()),
            Some("test-runtime-id-1234")
        );
        // 现存字段不能被破坏
        assert_eq!(body["service"], "gitim-runtime");
    }

    // -- introduction wire format coverage --
    //
    // The triple-option semantics on `AgentUpdateRequest::introduction` are
    // shared with `system_prompt` and `model`, but each one is exposed on the
    // wire as a separate JSON key — a regression in the deserializer or rename
    // would silently drop introduction patches without these tests catching it.

    #[test]
    fn agent_add_request_accepts_introduction() {
        let body = serde_json::json!({
            "handler": "alice",
            "display_name": "Alice",
            "provider": "claude",
            "introduction": "Senior code reviewer"
        });
        let req: AgentAddRequest = serde_json::from_value(body).unwrap();
        assert_eq!(req.introduction.as_deref(), Some("Senior code reviewer"));
    }

    #[test]
    fn agent_add_request_introduction_omitted_is_none() {
        let body = serde_json::json!({
            "handler": "alice",
            "display_name": "Alice",
            "provider": "claude"
        });
        let req: AgentAddRequest = serde_json::from_value(body).unwrap();
        assert!(req.introduction.is_none());
    }

    #[test]
    fn agent_add_request_join_general_omitted_is_none() {
        let body = serde_json::json!({
            "handler": "alice",
            "display_name": "Alice",
            "provider": "claude"
        });
        let req: AgentAddRequest = serde_json::from_value(body).unwrap();
        assert!(req.join_general.is_none());
    }

    #[test]
    fn agent_add_request_join_general_explicit_false_deserializes() {
        let body = serde_json::json!({
            "handler": "alice",
            "display_name": "Alice",
            "provider": "claude",
            "join_general": false
        });
        let req: AgentAddRequest = serde_json::from_value(body).unwrap();
        assert_eq!(req.join_general, Some(false));
    }

    #[test]
    fn agent_update_request_introduction_absent_is_noop() {
        let body = serde_json::json!({});
        let req: AgentUpdateRequest = serde_json::from_value(body).unwrap();
        assert!(
            req.introduction.is_none(),
            "missing key should preserve the existing value"
        );
    }

    #[test]
    fn agent_update_request_introduction_null_clears() {
        let body = serde_json::json!({ "introduction": null });
        let req: AgentUpdateRequest = serde_json::from_value(body).unwrap();
        assert_eq!(
            req.introduction,
            Some(None),
            "null must be distinguishable from absent so the daemon clears the blurb"
        );
    }

    #[test]
    fn agent_update_request_introduction_string_sets() {
        let body = serde_json::json!({ "introduction": "AI assistant" });
        let req: AgentUpdateRequest = serde_json::from_value(body).unwrap();
        assert_eq!(req.introduction, Some(Some("AI assistant".to_string())));
    }

    // ─── Cron timeline coupling guard ────────────────────────────────────────
    //
    // `synthesize_spec_for_iteration` is the load-bearing bridge from
    // `CronSummary` (daemon-owned wire shape) to `CronSpec` (engine
    // contract). The doc comment on `CronSummary` enumerates the fields
    // marked "timeline: required". This test locks the relationship: it
    // builds a default-shaped summary, runs synthesis, and proves the
    // result is usable by `next_fire_after`. If a future refactor drops
    // or renames any of those fields, this test fails — surfacing the
    // breakage at compile/test time rather than as a silent drop in
    // future / missed entries on the calendar.

    #[test]
    fn cron_synthesis_succeeds_for_default_summary() {
        // Minimum-viable summary: every field the synthesizer reads is
        // populated with the simplest valid value. Any future drift in
        // `CronSummary`'s required-for-timeline fields fails this test.
        let summary = gitim_core::responses::CronSummary {
            name: "test-job".to_string(),
            schedule: "0 9 * * *".to_string(),
            timezone: None,
            target: "alice".to_string(),
            enabled: true,
            created_by: "alice".to_string(),
            created_at: "2026-05-09T10:00:00Z".to_string(),
            next_fire: None,
        };
        let spec = synthesize_spec_for_iteration(&summary)
            .expect("default-shaped summary must synthesize");
        assert_eq!(spec.schedule, "0 9 * * *");
        assert_eq!(spec.target.as_str(), "alice");
        assert_eq!(spec.created_by.as_str(), "alice");
        assert_eq!(spec.created_at, "2026-05-09T10:00:00Z");
        assert_eq!(spec.timezone, None);
        assert!(spec.enabled);
    }

    #[test]
    fn synthesize_spec_for_iteration_locks_summary_contract() {
        // End-to-end: feed a CronSummary into synthesis, then into
        // `next_fire_after`, and assert the predicted instant matches
        // the schedule. This proves the synthesized spec is more than
        // just well-formed — it's actually usable by the engine's
        // iteration entry point. A refactor that, say, drops `timezone`
        // from CronSummary would break the synthesis call AND any
        // future-fire iteration that hangs off it.
        use chrono::{DateTime, Utc};
        let summary = gitim_core::responses::CronSummary {
            name: "morning-task".to_string(),
            schedule: "0 9 * * *".to_string(),
            timezone: Some("America/Los_Angeles".to_string()),
            target: "alice".to_string(),
            enabled: true,
            created_by: "alice".to_string(),
            created_at: "2026-05-09T10:00:00Z".to_string(),
            next_fire: None,
        };
        let spec = synthesize_spec_for_iteration(&summary).expect("synthesize OK");
        // 9am LA on a non-DST day = UTC 17:00 (PDT, UTC-7) or 16:00 (PST,
        // UTC-8). Pick a known PDT date and assert the predicted next
        // fire lands on that instant.
        let after: DateTime<Utc> = "2026-05-10T00:00:00Z".parse().unwrap();
        let next = gitim_core::types::cron::next_fire_after(&spec, after)
            .expect("next_fire_after on synthesized spec");
        let expected: DateTime<Utc> = "2026-05-10T16:00:00Z".parse().unwrap();
        assert_eq!(
            next, expected,
            "synthesized spec must drive next_fire_after the same way a parsed spec.yaml would"
        );
    }

    // ------------------------------------------------------------------
    // ErrorBody — preflight_detail (T5)
    //
    // Every other ErrorBody path must keep its old two-key shape (ok, error
    // [+ error_code]) — frontend / CLI consumers branch on absence of keys,
    // so `skip_serializing_if` must actually elide the field for `None`.
    // Only `with_preflight` carries the nested PreflightResult.
    // ------------------------------------------------------------------

    #[test]
    fn error_body_new_serializes_without_preflight_detail() {
        let body = ErrorBody::new("plain failure");
        let v = serde_json::to_value(&body).expect("serialize ErrorBody::new");
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], "plain failure");
        assert!(
            v.get("error_code").is_none(),
            "error_code must be elided when None: {v:?}"
        );
        assert!(
            v.get("preflight_detail").is_none(),
            "preflight_detail must be elided when None: {v:?}"
        );
    }

    #[test]
    fn error_body_with_code_serializes_without_preflight_detail() {
        let body = ErrorBody::with_code("oops", "handler_conflict");
        let v = serde_json::to_value(&body).expect("serialize ErrorBody::with_code");
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], "oops");
        assert_eq!(v["error_code"], "handler_conflict");
        assert!(
            v.get("preflight_detail").is_none(),
            "preflight_detail must be elided when None: {v:?}"
        );
    }

    #[test]
    fn error_body_with_preflight_serializes_with_nested_detail() {
        let detail = crate::preflight::PreflightResult::failure(
            "claude",
            crate::preflight::ErrorKind::Other,
            "fake",
            123,
        );
        let body = ErrorBody::with_preflight("oops", "provision_preflight_failed", detail);
        let v = serde_json::to_value(&body).expect("serialize ErrorBody::with_preflight");

        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], "oops");
        assert_eq!(v["error_code"], "provision_preflight_failed");

        let pd = v
            .get("preflight_detail")
            .expect("preflight_detail key present");
        assert_eq!(pd["available"], false);
        assert_eq!(pd["provider"], "claude");
        assert_eq!(pd["error_kind"], "other");
        assert_eq!(pd["error"], "fake");
        assert_eq!(pd["duration_ms"], 123);
    }

    #[test]
    fn error_body_round_trip_via_serde_json_value() {
        // Round-trip: ErrorBody -> JSON Value -> parse back as PreflightResult
        // on the nested branch. Confirms the nested struct survives JSON
        // serialization with all its fields preserved (esp. error_kind
        // snake_case rename and skip_serializing_if behavior).
        let detail = crate::preflight::PreflightResult::failure_with_code(
            "hermes",
            crate::preflight::ErrorKind::NotInstalled,
            "binary missing",
            7,
            "provider_cli_not_found",
        );
        let body =
            ErrorBody::with_preflight("preflight failed", "provision_preflight_failed", detail);
        let v = serde_json::to_value(&body).expect("serialize");

        // Parse the nested branch back into a PreflightResult — this proves
        // wire compat for downstream consumers (CLI typed DTO) that may
        // deserialize the nested object directly.
        let pd_value = v.get("preflight_detail").cloned().expect("nested present");
        let parsed: crate::preflight::PreflightResult =
            serde_json::from_value(pd_value).expect("nested deserializes as PreflightResult");
        assert!(!parsed.available);
        assert_eq!(parsed.provider, "hermes");
        assert_eq!(
            parsed.error_kind,
            Some(crate::preflight::ErrorKind::NotInstalled)
        );
        assert_eq!(parsed.error.as_deref(), Some("binary missing"));
        assert_eq!(parsed.duration_ms, 7);
        assert_eq!(
            parsed.failure_code.as_deref(),
            Some("provider_cli_not_found"),
            "failure_code survives JSON round-trip"
        );
    }
}
