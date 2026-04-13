use axum::{extract::State, routing::{get, post}, Json, Router};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tower_http::cors::CorsLayer;

use crate::agent::provision_human;
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

#[derive(Default)]
pub struct RuntimeState {
    pub workspace: Option<PathBuf>,
    pub human_repo: Option<PathBuf>,
    pub poll_cursor: Option<String>,
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

pub fn create_router() -> Router {
    let state: SharedRuntimeState = Arc::new(Mutex::new(RuntimeState::default()));

    Router::new()
        .route("/health", get(health))
        .route("/workspace", post(set_workspace))
        .route("/git/init", post(git_init))
        .route("/im/me", get(im_me))
        .route("/im/channels", get(im_channels))
        .route("/im/send", post(im_send))
        .route("/im/read", post(im_read))
        .route("/im/poll", post(im_poll))
        .layer(CorsLayer::permissive())
        .with_state(state)
}
