use axum::{extract::State, routing::post, Json, Router};
use crate::api::{Request, Response};
use crate::state::SharedState;

pub fn create_router(state: SharedState) -> Router {
    Router::new()
        .route("/api", post(handle_api))
        .with_state(state)
}

async fn handle_api(
    State(state): State<SharedState>,
    Json(req): Json<Request>,
) -> Json<Response> {
    Json(crate::handlers::handle_request(req, state).await)
}
