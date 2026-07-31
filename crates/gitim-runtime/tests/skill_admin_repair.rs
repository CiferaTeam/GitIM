#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use axum::routing::{get, post};
use axum::{Json, Router};
use gitim_runtime::cli::cmd_skill_repair::{build_request, run, Args};
use gitim_runtime::cli::{CliError, Client};
use serde_json::json;

#[test]
fn repair_request_requires_explicit_confirmation() {
    let error = build_request(
        "workspace",
        &Args {
            workspace: Some("workspace".to_owned()),
            skill: None,
            conflict_tip: "a".repeat(40),
            accepted_tree: "b".repeat(40),
            confirm: false,
        },
    )
    .unwrap_err();
    assert!(matches!(error, CliError::InvalidConfig(_)));
}

#[test]
fn repair_request_carries_checkpoint_values_and_optional_scope() {
    let (path, body) = build_request(
        "workspace",
        &Args {
            workspace: Some("workspace".to_owned()),
            skill: Some("release-check".to_owned()),
            conflict_tip: "a".repeat(40),
            accepted_tree: "b".repeat(40),
            confirm: true,
        },
    )
    .unwrap();
    assert_eq!(path, "/workspaces/workspace/admin/repair-skill-state");
    assert_eq!(body["skill"], "release-check");
    assert_eq!(body["conflict_tip"], "a".repeat(40));
    assert_eq!(body["accepted_tree"], "b".repeat(40));
    assert_eq!(body["confirm"], true);
}

#[tokio::test]
async fn repair_cli_maps_runtime_checkpoint_rejection() {
    let router = Router::new()
        .route(
            "/workspaces",
            get(|| async { Json(json!({"workspaces": [{"slug": "workspace"}]})) }),
        )
        .route(
            "/workspaces/workspace/admin/repair-skill-state",
            post(|| async {
                Json(json!({
                    "ok": false,
                    "error": "checkpoint values do not match",
                    "error_code": "skill_sync_conflict"
                }))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let client = Client::new(format!("http://{address}"));

    let error = run(
        &client,
        Args {
            workspace: Some("workspace".to_owned()),
            skill: None,
            conflict_tip: "a".repeat(40),
            accepted_tree: "b".repeat(40),
            confirm: true,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        CliError::ResponseErrorCode { ref code, .. } if code == "skill_sync_conflict"
    ));
    server.abort();
}

#[test]
fn runtime_sources_gate_bootstrap_and_private_repair_route() {
    let source = include_str!("../src/http.rs");
    let recovery = source.find("recover_single_workspace").unwrap();
    let agent_recovery = source[recovery..]
        .find("recover_agents_for_workspace")
        .map(|offset| recovery + offset)
        .unwrap();
    let bootstrap = source[recovery..agent_recovery]
        .find("ensure_workspace_skill_bootstrap")
        .map(|offset| recovery + offset)
        .expect("recovery bootstraps Skills before agents");
    assert!(bootstrap < agent_recovery);
    assert!(source.matches("ensure_workspace_skill_bootstrap").count() >= 4);
    assert!(source.contains("recovered workspace without Skill administration bootstrap"));
    assert!(source.contains("/admin/repair-skill-state"));
    assert!(!source.contains("/im/repair-skill-state"));
    assert!(source.contains("peer.ip().is_loopback()"));
}

#[test]
fn repair_uses_long_request_timeout() {
    assert!(Duration::from_secs(180) < gitim_runtime::cli::cmd_skill_repair::REQUEST_TIMEOUT);
}
