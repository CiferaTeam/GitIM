//! Integration tests for PATCH /workspaces/{slug}/agents/{id}
//!
//! Follows the `tests/http_workspaces.rs` pattern: tower::ServiceExt::oneshot
//! + create_router + direct WorkspaceContext injection.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use gitim_runtime::git_config::{GitConfig, GitProvider, WorkspaceConfig};
use gitim_runtime::http::{create_router, SharedRuntimeState, DOTENV_MAX_BYTES};
use gitim_runtime::workspace::WorkspaceContext;

async fn send(
    router: axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(b) => builder
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&b).unwrap()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

fn inject_workspace(state: &SharedRuntimeState, slug_str: &str) {
    use std::path::PathBuf;
    let mut ctx = WorkspaceContext::new(
        slug_str.to_string(),
        slug_str.to_string(),
        PathBuf::from("/tmp/test-ws"),
    );
    ctx.git_config = Some(WorkspaceConfig {
        workspace: "/tmp/test-ws".to_string(),
        created_at: "2026-04-21T00:00:00Z".to_string(),
        git: GitConfig {
            provider: GitProvider::Local,
            remote_url: None,
            token: None,
            github_email: None,
        },
    });
    state
        .lock()
        .unwrap()
        .workspaces
        .insert(slug_str.to_string(), ctx);
}

// -- Seeding helper -----------------------------------------------------------

/// Create a fake agent directory with a `.gitim/me.json` and inject an
/// `AgentInfo` into the workspace's agents map.  Returns the `tempdir` so the
/// caller holds it alive for the duration of the test.
///
/// `system_prompt` and `env` fields on the injected `AgentInfo` are derived
/// from `me_json` so in-memory state matches on-disk state out of the box.
fn seed_agent_in_workspace(
    state: &SharedRuntimeState,
    slug_str: &str,
    agent_id: &str,
    me_json: serde_json::Value,
) -> tempfile::TempDir {
    use gitim_runtime::http::AgentInfo;
    use std::collections::HashMap;

    let dir = tempfile::TempDir::new().expect("tempdir");
    let gitim_dir = dir.path().join(".gitim");
    std::fs::create_dir_all(&gitim_dir).unwrap();
    std::fs::write(
        gitim_dir.join("me.json"),
        serde_json::to_string_pretty(&me_json).unwrap(),
    )
    .unwrap();

    let env: HashMap<String, String> = me_json
        .get("env")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();

    let info = AgentInfo {
        id: agent_id.to_string(),
        handler: agent_id.to_string(),
        display_name: "Test Agent".to_string(),
        status: "idle".to_string(),
        last_activity: None,
        messages_processed: 0,
        repo_path: dir.path().display().to_string(),
        provider: me_json
            .get("provider")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        model: me_json
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        system_prompt: me_json
            .get("system_prompt")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        introduction: None,
        env,
        error_message: None,
        session_usage: None,
        llm_provider: None,
        llm_model: None,
        loop_handle: None,
    };

    state
        .lock()
        .unwrap()
        .workspaces
        .get_mut(slug_str)
        .expect("workspace must be injected first")
        .agents
        .insert(agent_id.to_string(), info);

    dir
}

// Helper shortcut for PATCH requests (used by env tests; kept separate from
// `send` so the router can be reused across multiple calls in one test).
async fn send_patch(
    router: &axum::Router,
    slug: &str,
    agent_id: &str,
    body: Value,
) -> (StatusCode, Value) {
    send(
        router.clone(),
        "PATCH",
        &format!("/workspaces/{slug}/agents/{agent_id}"),
        Some(body),
    )
    .await
}

// -- 1. PATCH on nonexistent agent returns 404 --------------------------------

#[tokio::test]
async fn patch_nonexistent_agent_returns_404() {
    let (router, state) = create_router();
    inject_workspace(&state, "ws1");

    let (status, body) = send(
        router,
        "PATCH",
        "/workspaces/ws1/agents/nonexistent",
        Some(json!({ "system_prompt": "hi" })),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["ok"], json!(false));
}

// -- 2. PATCH system_prompt writes me.json (merge semantics) ------------------

#[tokio::test]
async fn patch_system_prompt_writes_me_json() {
    let (router, state) = create_router();
    inject_workspace(&state, "ws2");
    let _dir = seed_agent_in_workspace(
        &state,
        "ws2",
        "alice",
        json!({ "provider": "claude", "system_prompt": "old prompt" }),
    );
    let agent_dir = state
        .lock()
        .unwrap()
        .workspaces
        .get("ws2")
        .unwrap()
        .agents
        .get("alice")
        .unwrap()
        .repo_path
        .clone();

    let (status, body) = send(
        router,
        "PATCH",
        "/workspaces/ws2/agents/alice",
        Some(json!({ "system_prompt": "new prompt" })),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["agent"]["system_prompt"], json!("new prompt"));

    // On-disk me.json must be updated.
    let me_path = std::path::PathBuf::from(&agent_dir).join(".gitim/me.json");
    let me: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&me_path).unwrap()).unwrap();
    assert_eq!(
        me["system_prompt"],
        json!("new prompt"),
        "disk must reflect new value"
    );
    // Merge semantics: provider field must be preserved.
    assert_eq!(
        me["provider"],
        json!("claude"),
        "provider must be preserved"
    );
}

// -- 3. PATCH system_prompt null clears the field ------------------------------

#[tokio::test]
async fn patch_system_prompt_null_clears_field() {
    let (router, state) = create_router();
    inject_workspace(&state, "ws3");
    let _dir = seed_agent_in_workspace(
        &state,
        "ws3",
        "bob",
        json!({ "provider": "claude", "system_prompt": "some prompt" }),
    );
    let agent_dir = state
        .lock()
        .unwrap()
        .workspaces
        .get("ws3")
        .unwrap()
        .agents
        .get("bob")
        .unwrap()
        .repo_path
        .clone();

    let (status, body) = send(
        router,
        "PATCH",
        "/workspaces/ws3/agents/bob",
        Some(json!({ "system_prompt": null })),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["ok"], json!(true));
    // system_prompt absent from serialized AgentInfo (skip_serializing_if = None)
    assert!(
        body["agent"]["system_prompt"].is_null(),
        "system_prompt should be null/absent in response; got body: {body}"
    );

    // On-disk: field removed.
    let me_path = std::path::PathBuf::from(&agent_dir).join(".gitim/me.json");
    let me: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&me_path).unwrap()).unwrap();
    assert!(
        me.get("system_prompt").is_none(),
        "system_prompt must be removed from me.json; got: {me}"
    );
    // Merge: provider still there.
    assert_eq!(me["provider"], json!("claude"));
}

// -- 4. PATCH empty body does not touch system_prompt -------------------------

#[tokio::test]
async fn patch_missing_field_does_not_touch_it() {
    let (router, state) = create_router();
    inject_workspace(&state, "ws4");
    let _dir = seed_agent_in_workspace(
        &state,
        "ws4",
        "carol",
        json!({ "provider": "claude", "system_prompt": "keep me" }),
    );
    let agent_dir = state
        .lock()
        .unwrap()
        .workspaces
        .get("ws4")
        .unwrap()
        .agents
        .get("carol")
        .unwrap()
        .repo_path
        .clone();

    let (status, body) = send(
        router,
        "PATCH",
        "/workspaces/ws4/agents/carol",
        Some(json!({})),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["ok"], json!(true));

    // system_prompt must be unchanged in me.json.
    let me_path = std::path::PathBuf::from(&agent_dir).join(".gitim/me.json");
    let me: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&me_path).unwrap()).unwrap();
    assert_eq!(
        me["system_prompt"],
        json!("keep me"),
        "system_prompt must be untouched when field is absent from request"
    );
}

// -- 5. PATCH env replaces full map (A and B gone after PATCH {C: 3}) ---------

#[tokio::test]
async fn patch_env_replaces_full_map() {
    let (router, state) = create_router();
    inject_workspace(&state, "ws5");
    let _dir = seed_agent_in_workspace(
        &state,
        "ws5",
        "alice",
        json!({ "provider": "claude", "env": { "A": "1", "B": "2" } }),
    );

    let agent_dir = state
        .lock()
        .unwrap()
        .workspaces
        .get("ws5")
        .unwrap()
        .agents
        .get("alice")
        .unwrap()
        .repo_path
        .clone();

    let (status, body) = send_patch(&router, "ws5", "alice", json!({ "env": { "C": "3" } })).await;

    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["ok"], json!(true));
    // Response env should contain only C.
    let resp_env = body["agent"]["env"]
        .as_object()
        .expect("env should be object");
    assert_eq!(
        resp_env.get("C").and_then(|v| v.as_str()),
        Some("3"),
        "C must be present"
    );
    assert!(
        resp_env.get("A").is_none(),
        "A must be gone — replacement, not merge"
    );
    assert!(
        resp_env.get("B").is_none(),
        "B must be gone — replacement, not merge"
    );

    // On-disk me.json must also reflect replacement.
    let me_path = std::path::PathBuf::from(&agent_dir).join(".gitim/me.json");
    let me: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&me_path).unwrap()).unwrap();
    let disk_env = me["env"].as_object().expect("env should be object on disk");
    assert_eq!(disk_env.get("C").and_then(|v| v.as_str()), Some("3"));
    assert!(
        disk_env.get("A").is_none(),
        "A must be absent from me.json after replacement"
    );
    assert!(
        disk_env.get("B").is_none(),
        "B must be absent from me.json after replacement"
    );
    // Other fields survive (merge semantics for non-env fields).
    assert_eq!(me["provider"], json!("claude"));
}

// -- 6. PATCH env empty map clears all env vars --------------------------------

#[tokio::test]
async fn patch_env_empty_clears_all() {
    let (router, state) = create_router();
    inject_workspace(&state, "ws6");
    let _dir = seed_agent_in_workspace(
        &state,
        "ws6",
        "alice",
        json!({ "provider": "claude", "env": { "A": "1" } }),
    );

    let agent_dir = state
        .lock()
        .unwrap()
        .workspaces
        .get("ws6")
        .unwrap()
        .agents
        .get("alice")
        .unwrap()
        .repo_path
        .clone();

    let (status, body) = send_patch(&router, "ws6", "alice", json!({ "env": {} })).await;

    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["ok"], json!(true));

    // On-disk: env field must be removed outright (not just emptied).  If a
    // future refactor starts writing `{}` instead, this assertion fails and we
    // catch the behavior change deliberately.
    let me_path = std::path::PathBuf::from(&agent_dir).join(".gitim/me.json");
    let me: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&me_path).unwrap()).unwrap();
    assert!(
        me.get("env").is_none(),
        "me.json should have env field removed, got: {me}"
    );
}

// -- 7. PATCH env with illegal key returns 400 --------------------------------

#[tokio::test]
async fn patch_env_rejects_illegal_key() {
    let (router, state) = create_router();
    inject_workspace(&state, "ws7");
    let _dir = seed_agent_in_workspace(&state, "ws7", "alice", json!({ "provider": "claude" }));

    let (status, body) =
        send_patch(&router, "ws7", "alice", json!({ "env": { "1bad": "x" } })).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert_eq!(body["ok"], json!(false));
    assert!(
        body["error"]
            .as_str()
            .unwrap_or("")
            .contains("invalid env var"),
        "error should mention 'invalid env var'; got: {body}"
    );
}

// -- 8. PATCH dotenv writes .env file with mode 0600 --------------------------

#[tokio::test]
async fn patch_dotenv_writes_file_with_mode_600() {
    let (router, state) = create_router();
    inject_workspace(&state, "ws8");
    let _dir = seed_agent_in_workspace(&state, "ws8", "alice", json!({ "provider": "claude" }));
    let repo_root = state
        .lock()
        .unwrap()
        .workspaces
        .get("ws8")
        .unwrap()
        .agents
        .get("alice")
        .unwrap()
        .repo_path
        .clone();

    let (status, _body) = send_patch(
        &router,
        "ws8",
        "alice",
        json!({ "dotenv": "OPENAI_KEY=sk-xxx\nDB=postgres://..." }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {_body}");

    let env_path = std::path::PathBuf::from(&repo_root).join(".env");
    let contents = std::fs::read_to_string(&env_path).unwrap();
    assert!(
        contents.contains("OPENAI_KEY=sk-xxx"),
        "file must contain written value"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&env_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "expected 0600, got {:o}", mode & 0o777);
    }
}

// -- 9. PATCH dotenv empty string deletes the .env file -----------------------

#[tokio::test]
async fn patch_dotenv_empty_deletes_file() {
    let (router, state) = create_router();
    inject_workspace(&state, "ws9");
    let _dir = seed_agent_in_workspace(&state, "ws9", "alice", json!({ "provider": "claude" }));
    let repo_root = state
        .lock()
        .unwrap()
        .workspaces
        .get("ws9")
        .unwrap()
        .agents
        .get("alice")
        .unwrap()
        .repo_path
        .clone();

    // First: write a non-empty dotenv.
    let (status, body) =
        send_patch(&router, "ws9", "alice", json!({ "dotenv": "SECRET=abc" })).await;
    assert_eq!(status, StatusCode::OK, "setup write failed: {body}");
    let env_path = std::path::PathBuf::from(&repo_root).join(".env");
    assert!(env_path.exists(), ".env should exist after initial write");

    // Then: send empty string → file must be deleted.
    let (status, body) = send_patch(&router, "ws9", "alice", json!({ "dotenv": "" })).await;
    assert_eq!(status, StatusCode::OK, "delete failed: {body}");
    assert!(
        !env_path.exists(),
        ".env should be deleted after empty-string patch"
    );

    // Also verify: empty-string patch when .env was never created is a no-op 200.
    let (status, body) = send_patch(&router, "ws9", "alice", json!({ "dotenv": "" })).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "second empty patch (no-op) failed: {body}"
    );
}

// -- 10. PATCH dotenv > 64KB is rejected with 400 -----------------------------

#[tokio::test]
async fn patch_dotenv_rejects_oversize() {
    let (router, state) = create_router();
    inject_workspace(&state, "ws10");
    let _dir = seed_agent_in_workspace(&state, "ws10", "alice", json!({ "provider": "claude" }));

    let big = "A".repeat(DOTENV_MAX_BYTES + 1); // one byte over the cap
    let (status, body) = send_patch(&router, "ws10", "alice", json!({ "dotenv": big })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert!(
        body["error"].as_str().unwrap_or("").contains("64 KB"),
        "error should mention '64 KB'; got: {body}"
    );
}

// -- 11. PATCH model updates me.json and clears model-bound session state -----

#[tokio::test]
async fn patch_model_writes_me_json_and_clears_session_state() {
    let (router, state) = create_router();
    inject_workspace(&state, "ws11");
    let dir = seed_agent_in_workspace(
        &state,
        "ws11",
        "alice",
        json!({ "provider": "codex", "model": "gpt-5.4" }),
    );

    let state_path = dir.path().join(".gitim/agent-state.json");
    std::fs::write(
        &state_path,
        r#"{
          "cursor": "000123",
          "session_token": "old-session",
          "session_usage": {
            "session_id": "old-session",
            "max_tokens": 272000,
            "used_percent": 70.0,
            "source": "runtime_estimated",
            "updated_at": "2026-04-24T00:00:00Z"
          },
          "estimated_tokens": 190400,
          "usage_notice_pending": true
        }"#,
    )
    .unwrap();

    let (status, body) = send_patch(&router, "ws11", "alice", json!({ "model": "gpt-5.5" })).await;

    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["agent"]["model"], json!("gpt-5.5"));

    let me_path = dir.path().join(".gitim/me.json");
    let me: Value = serde_json::from_str(&std::fs::read_to_string(&me_path).unwrap()).unwrap();
    assert_eq!(me["model"], json!("gpt-5.5"));

    let agent_state: Value =
        serde_json::from_str(&std::fs::read_to_string(&state_path).unwrap()).unwrap();
    assert_eq!(agent_state["cursor"], json!("000123"));
    assert!(agent_state.get("session_token").is_none());
    assert!(agent_state.get("session_usage").is_none());
    assert!(agent_state.get("estimated_tokens").is_none());
    assert!(
        body["agent"].get("session_usage").is_none() || body["agent"]["session_usage"].is_null()
    );
}

#[tokio::test]
async fn patch_model_null_clears_me_json_field() {
    let (router, state) = create_router();
    inject_workspace(&state, "ws12");
    let dir = seed_agent_in_workspace(
        &state,
        "ws12",
        "alice",
        json!({ "provider": "codex", "model": "gpt-5.4" }),
    );

    let (status, body) = send_patch(&router, "ws12", "alice", json!({ "model": null })).await;

    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(body["agent"].get("model").is_none() || body["agent"]["model"].is_null());

    let me_path = dir.path().join(".gitim/me.json");
    let me: Value = serde_json::from_str(&std::fs::read_to_string(&me_path).unwrap()).unwrap();
    assert!(me.get("model").is_none());
}

#[tokio::test]
async fn patch_model_rejects_running_agent() {
    let (router, state) = create_router();
    inject_workspace(&state, "ws13");
    let _dir = seed_agent_in_workspace(
        &state,
        "ws13",
        "alice",
        json!({ "provider": "codex", "model": "gpt-5.4" }),
    );
    state
        .lock()
        .unwrap()
        .workspaces
        .get_mut("ws13")
        .unwrap()
        .agents
        .get_mut("alice")
        .unwrap()
        .status = "running".to_string();

    let (status, body) = send_patch(&router, "ws13", "alice", json!({ "model": "gpt-5.5" })).await;

    assert_eq!(status, StatusCode::CONFLICT, "body: {body}");
    assert_eq!(body["ok"], json!(false));
    assert!(body["error"]
        .as_str()
        .unwrap_or("")
        .contains("stop the agent before changing model"));
}
