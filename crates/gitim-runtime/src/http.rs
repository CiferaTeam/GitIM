use axum::{extract::State, routing::{get, post}, Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::task::AbortHandle;
use tower_http::cors::CorsLayer;

use crate::agent::{provision_agent, provision_human, AgentConfig};
use crate::agent_loop::AgentLoop;
use gitim_client::GitimClient;

#[derive(Serialize)]
struct HealthResponse {
    service: &'static str,
    version: &'static str,
}

#[derive(Deserialize)]
struct WorkspaceRequest {
    path: String,
    #[serde(default)]
    confirm: bool,
}

#[derive(Clone, Serialize)]
pub struct AgentInfo {
    pub id: String,
    pub handler: String,
    pub display_name: String,
    pub status: String, // "idle", "running", "error"
    #[serde(skip)]
    pub repo_root: PathBuf,
    #[serde(skip)]
    pub loop_handle: Option<AbortHandle>,
}

#[derive(Default)]
pub struct RuntimeState {
    pub workspace: Option<PathBuf>,
    pub human_repo: Option<PathBuf>,
    pub poll_cursor: Option<String>,
    pub agents: HashMap<String, AgentInfo>,
}

pub type SharedRuntimeState = Arc<Mutex<RuntimeState>>;

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        service: "gitim-runtime",
        version: env!("CARGO_PKG_VERSION"),
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
}

async fn git_init(
    State(state): State<SharedRuntimeState>,
    Json(req): Json<GitInitRequest>,
) -> Json<serde_json::Value> {
    if req.provider != "local" {
        return Json(serde_json::json!({
            "ok": false,
            "error": format!("provider not supported yet: {}", req.provider)
        }));
    }

    let workspace = {
        let s = state.lock().unwrap();
        match &s.workspace {
            Some(p) => p.clone(),
            None => {
                return Json(serde_json::json!({
                    "ok": false,
                    "error": "workspace not set"
                }));
            }
        }
    };

    let repo_path = workspace.join("repo.git");
    if let Err(e) = std::fs::create_dir_all(&repo_path) {
        return Json(serde_json::json!({
            "ok": false,
            "error": format!("failed to create repo directory: {e}")
        }));
    }

    let output = std::process::Command::new("git")
        .args(["init", "--bare"])
        .current_dir(&repo_path)
        .output();

    match output {
        Ok(o) if o.status.success() => {
            // Provision the human daemon after bare repo is ready
            match provision_human(&workspace).await {
                Ok(human_dir) => {
                    let mut s = state.lock().unwrap();
                    s.human_repo = Some(human_dir.clone());
                    Json(serde_json::json!({
                        "ok": true,
                        "repo_path": repo_path.to_string_lossy(),
                        "human_repo": human_dir.to_string_lossy()
                    }))
                }
                Err(e) => Json(serde_json::json!({
                    "ok": false,
                    "error": format!("provision_human failed: {e}")
                })),
            }
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            Json(serde_json::json!({
                "ok": false,
                "error": format!("git init failed: {stderr}")
            }))
        }
        Err(e) => {
            Json(serde_json::json!({
                "ok": false,
                "error": format!("failed to run git: {e}")
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
    let client = match human_client(&state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    api_response_to_json(client.status().await)
}

// -- /im/channels --

async fn im_channels(State(state): State<SharedRuntimeState>) -> Json<serde_json::Value> {
    let client = match human_client(&state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    api_response_to_json(client.list_channels().await)
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

// -- /agents/add --

#[derive(Deserialize)]
struct AgentAddRequest {
    handler: String,
    display_name: String,
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
            let info = AgentInfo {
                id: req.handler.clone(),
                handler: req.handler.clone(),
                display_name: req.display_name.clone(),
                status: "idle".to_string(),
                repo_root: handle.repo_root,
                loop_handle: None,
            };
            let mut s = state.lock().unwrap();
            s.agents.insert(req.handler.clone(), info);
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

async fn agents_start(
    State(state): State<SharedRuntimeState>,
    Json(req): Json<AgentIdRequest>,
) -> Json<serde_json::Value> {
    let (repo_root, handler) = {
        let s = state.lock().unwrap();
        match s.agents.get(&req.id) {
            None => {
                return Json(serde_json::json!({
                    "ok": false,
                    "error": format!("agent not found: {}", req.id)
                }));
            }
            Some(info) if info.status == "running" => {
                return Json(serde_json::json!({
                    "ok": false,
                    "error": format!("agent already running: {}", req.id)
                }));
            }
            Some(info) => (info.repo_root.clone(), info.handler.clone()),
        }
    };

    let agent_loop = match AgentLoop::with_provider(&repo_root, "claude", &handler) {
        Ok(al) => al,
        Err(e) => {
            return Json(serde_json::json!({
                "ok": false,
                "error": format!("failed to create agent loop: {e}")
            }));
        }
    };

    let agent_id = req.id.clone();
    let state_clone = state.clone();

    let handle = tokio::spawn(async move {
        let mut agent_loop = agent_loop;
        let result = agent_loop.run().await;

        // Update status when loop exits
        let mut s = state_clone.lock().unwrap();
        if let Some(info) = s.agents.get_mut(&agent_id) {
            info.loop_handle = None;
            info.status = match result {
                Ok(()) => "idle".to_string(),
                Err(_) => "error".to_string(),
            };
        }
    });

    let abort_handle = handle.abort_handle();

    {
        let mut s = state.lock().unwrap();
        if let Some(info) = s.agents.get_mut(&req.id) {
            info.loop_handle = Some(abort_handle);
            info.status = "running".to_string();
        }
    }

    Json(serde_json::json!({ "ok": true }))
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
            let pid_file = info.repo_root.join(".gitim/run/gitim.pid");
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

pub fn create_router() -> (Router, SharedRuntimeState) {
    let state: SharedRuntimeState = Arc::new(Mutex::new(RuntimeState::default()));

    let router = Router::new()
        .route("/health", get(health))
        .route("/workspace", post(set_workspace))
        .route("/git/init", post(git_init))
        .route("/im/me", get(im_me))
        .route("/im/channels", get(im_channels))
        .route("/im/send", post(im_send))
        .route("/im/read", post(im_read))
        .route("/im/poll", post(im_poll))
        .route("/agents", get(agents_list))
        .route("/agents/add", post(agents_add))
        .route("/agents/start", post(agents_start))
        .route("/agents/stop", post(agents_stop))
        .route("/agents/remove", post(agents_remove))
        .route("/agents/{id}", get(agents_get))
        .layer(CorsLayer::permissive())
        .with_state(state.clone());

    (router, state)
}
