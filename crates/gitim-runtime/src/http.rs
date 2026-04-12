use axum::{extract::State, routing::{get, post}, Json, Router};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tower_http::cors::CorsLayer;

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

pub fn create_router() -> Router {
    let state: SharedRuntimeState = Arc::new(Mutex::new(RuntimeState::default()));

    Router::new()
        .route("/health", get(health))
        .route("/workspace", post(set_workspace))
        .layer(CorsLayer::permissive())
        .with_state(state)
}
