//! Cron trigger handlers.
//!
//! See `docs/plans/2026-05-09-cron-trigger/design.md` for the protocol-level
//! framing — `crons/<name>/spec.yaml` + `crons/<name>/<theoretical_ts>.thread`.
//! The engine that scans + fires lives in `cron_engine.rs`; this module only
//! covers the IPC surface (create, list, show, history, enable, disable,
//! delete).
//!
//! Task 2.1 scope: scaffolding only — every handler returns a typed
//! `not_implemented` error so the wire shape is observable end-to-end before
//! the real logic lands in 2.2/2.3/2.4.

use crate::api::Response;
use crate::state::SharedState;

/// Stub for [`Request::CreateCron`]. Real implementation lands in Task 2.2.
pub async fn handle_create_cron(
    _state: SharedState,
    _name: String,
    _schedule: String,
    _timezone: Option<String>,
    _target: String,
    _prompt: String,
    _author: String,
) -> Response {
    not_implemented("create_cron")
}

/// Stub for [`Request::ListCrons`]. Real implementation lands in Task 2.3.
pub async fn handle_list_crons(_state: SharedState) -> Response {
    not_implemented("list_crons")
}

/// Stub for [`Request::ShowCron`]. Real implementation lands in Task 2.3.
pub async fn handle_show_cron(_state: SharedState, _name: String) -> Response {
    not_implemented("show_cron")
}

/// Stub for [`Request::HistoryCron`]. Real implementation lands in Task 2.3.
pub async fn handle_history_cron(
    _state: SharedState,
    _name: String,
    _limit: Option<u32>,
) -> Response {
    not_implemented("history_cron")
}

/// Stub for [`Request::EnableCron`]. Real implementation lands in Task 2.4.
pub async fn handle_enable_cron(
    _state: SharedState,
    _name: String,
    _author: String,
) -> Response {
    not_implemented("enable_cron")
}

/// Stub for [`Request::DisableCron`]. Real implementation lands in Task 2.4.
pub async fn handle_disable_cron(
    _state: SharedState,
    _name: String,
    _author: String,
) -> Response {
    not_implemented("disable_cron")
}

/// Stub for [`Request::DeleteCron`]. Real implementation lands in Task 2.4.
pub async fn handle_delete_cron(
    _state: SharedState,
    _name: String,
    _author: String,
) -> Response {
    not_implemented("delete_cron")
}

/// Tagged error helper. The `error_code: "not_implemented"` lets the client
/// short-circuit on unfinished daemon endpoints without parsing the human
/// message.
fn not_implemented(method: &str) -> Response {
    Response::error_with_code(
        format!("{method}: not implemented yet (cron Wave 2 in progress)"),
        "not_implemented",
    )
}

#[cfg(test)]
mod tests {
    //! Task 2.1 scope tests: roundtrip every cron `Request` variant
    //! through `handle_request`'s dispatch and confirm we land on the
    //! cron stub, not some other handler.

    use crate::api::{Request, Response};
    use crate::handlers::handle_request;
    use crate::state::AppState;
    use gitim_core::types::Config;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::broadcast;

    fn make_config() -> Config {
        serde_yaml::from_str("version: 1").unwrap()
    }

    /// Minimal AppState with no users registered. Sufficient for stub
    /// dispatch tests — the stubs short-circuit before touching state.
    async fn make_state() -> (TempDir, Arc<AppState>) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        // git init so any future handler that reaches GitStorage doesn't
        // panic on missing repo. Stubs don't need it but a future test
        // that promotes into 2.2/2.3/2.4 will reuse this fixture.
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(&root)
            .output()
            .unwrap();
        let (tx, _) = broadcast::channel(16);
        let state = Arc::new(AppState::new(
            root,
            make_config(),
            tx,
            Some("alice".to_string()),
        ));
        (tmp, state)
    }

    fn assert_not_implemented(resp: &Response, method: &str) {
        assert!(!resp.ok, "{method}: expected error, got success");
        assert_eq!(
            resp.error_code.as_deref(),
            Some("not_implemented"),
            "{method}: expected error_code=not_implemented, got {:?}",
            resp.error_code
        );
        let msg = resp.error.as_deref().unwrap_or("");
        assert!(
            msg.contains(method),
            "{method}: error message should mention method name, got {msg}",
        );
    }

    #[tokio::test]
    async fn create_cron_dispatches_to_stub() {
        let (_tmp, state) = make_state().await;
        let req: Request = serde_json::from_value(serde_json::json!({
            "method": "create_cron",
            "name": "weekly",
            "schedule": "0 9 * * 1",
            "target": "@self",
            "prompt": "weekly checkin",
        }))
        .unwrap();
        let resp = handle_request(req, state).await;
        assert_not_implemented(&resp, "create_cron");
    }

    #[tokio::test]
    async fn list_crons_dispatches_to_stub() {
        let (_tmp, state) = make_state().await;
        let req: Request = serde_json::from_value(serde_json::json!({
            "method": "list_crons",
        }))
        .unwrap();
        let resp = handle_request(req, state).await;
        assert_not_implemented(&resp, "list_crons");
    }

    #[tokio::test]
    async fn show_cron_dispatches_to_stub() {
        let (_tmp, state) = make_state().await;
        let req: Request = serde_json::from_value(serde_json::json!({
            "method": "show_cron",
            "name": "weekly",
        }))
        .unwrap();
        let resp = handle_request(req, state).await;
        assert_not_implemented(&resp, "show_cron");
    }

    #[tokio::test]
    async fn history_cron_dispatches_to_stub() {
        let (_tmp, state) = make_state().await;
        let req: Request = serde_json::from_value(serde_json::json!({
            "method": "history_cron",
            "name": "weekly",
            "limit": 5,
        }))
        .unwrap();
        let resp = handle_request(req, state).await;
        assert_not_implemented(&resp, "history_cron");
    }

    #[tokio::test]
    async fn enable_cron_dispatches_to_stub() {
        let (_tmp, state) = make_state().await;
        let req: Request = serde_json::from_value(serde_json::json!({
            "method": "enable_cron",
            "name": "weekly",
        }))
        .unwrap();
        let resp = handle_request(req, state).await;
        assert_not_implemented(&resp, "enable_cron");
    }

    #[tokio::test]
    async fn disable_cron_dispatches_to_stub() {
        let (_tmp, state) = make_state().await;
        let req: Request = serde_json::from_value(serde_json::json!({
            "method": "disable_cron",
            "name": "weekly",
        }))
        .unwrap();
        let resp = handle_request(req, state).await;
        assert_not_implemented(&resp, "disable_cron");
    }

    #[tokio::test]
    async fn delete_cron_dispatches_to_stub() {
        let (_tmp, state) = make_state().await;
        let req: Request = serde_json::from_value(serde_json::json!({
            "method": "delete_cron",
            "name": "weekly",
        }))
        .unwrap();
        let resp = handle_request(req, state).await;
        assert_not_implemented(&resp, "delete_cron");
    }
}
