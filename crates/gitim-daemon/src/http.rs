use axum::{routing::post, Json, Router};
use crate::api::{Request, Response};

pub fn create_router() -> Router {
    Router::new()
        .route("/api", post(handle_api))
}

async fn handle_api(Json(req): Json<Request>) -> Json<Response> {
    let response = match req {
        Request::Status => Response::success(serde_json::json!({
            "version": "0.1.0",
            "status": "running",
        })),
        _ => Response::error("not implemented yet"),
    };
    Json(response)
}
