use crate::api::{Request, Response};
use crate::state::SharedState;
use axum::response::sse::{Event as SseEvent, Sse};
use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use futures::stream::Stream;
use std::convert::Infallible;

pub fn create_router(state: SharedState) -> Router {
    Router::new()
        .route("/api", post(handle_api))
        .route("/api/events", get(handle_sse))
        .with_state(state)
}

async fn handle_api(State(state): State<SharedState>, Json(req): Json<Request>) -> Json<Response> {
    Json(crate::handlers::handle_request(req, state).await)
}

async fn handle_sse(
    State(state): State<SharedState>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let mut rx = state.event_tx.subscribe();

    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let data = serde_json::to_string(&event).unwrap_or_default();
                    yield Ok(SseEvent::default().data(data));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    };

    Sse::new(stream)
}
