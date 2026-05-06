use std::path::Path;

use gitim_runtime::email_propagation::backfill_github_email;
use gitim_runtime::git_config::{GitConfig, GitProvider, WorkspaceConfig};
use mockito::{Mock, Server, ServerGuard};
use serde_json::{json, Value};
use tempfile::TempDir;

fn write_config(workspace: &Path, cfg: WorkspaceConfig) {
    std::fs::create_dir_all(workspace.join(".gitim-runtime")).unwrap();
    cfg.write(workspace).unwrap();
}

fn github_config(token: Option<&str>, email: Option<&str>) -> WorkspaceConfig {
    WorkspaceConfig {
        workspace: ".".to_string(),
        created_at: "2026-04-17T00:00:00Z".to_string(),
        git: GitConfig {
            provider: GitProvider::Github,
            remote_url: Some("https://github.com/owner/repo".to_string()),
            token: token.map(str::to_string),
            github_email: email.map(str::to_string),
        },
    }
}

fn write_me_json(clone_dir: &Path, initial: &Value) {
    let gitim_dir = clone_dir.join(".gitim");
    std::fs::create_dir_all(&gitim_dir).unwrap();
    std::fs::write(
        gitim_dir.join("me.json"),
        serde_json::to_string_pretty(initial).unwrap(),
    )
    .unwrap();
}

fn read_me_json(clone_dir: &Path) -> Value {
    let content = std::fs::read_to_string(clone_dir.join(".gitim").join("me.json")).unwrap();
    serde_json::from_str(&content).unwrap()
}

async fn mock_user_endpoint(server: &mut ServerGuard, body: &str) -> Mock {
    server
        .mock("GET", "/user")
        .with_status(200)
        .with_body(body)
        .create_async()
        .await
}

#[tokio::test]
async fn backfill_writes_public_email_into_config_and_clones() {
    let mut server = Server::new_async().await;
    let mock = mock_user_endpoint(
        &mut server,
        r#"{"id":42,"login":"octo","email":"octo@example.com"}"#,
    )
    .await;

    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path();
    write_config(workspace, github_config(Some("tok"), None));

    let human = workspace.join(".gitim-runtime").join("human");
    let agent = workspace.join("agent-a");
    write_me_json(&human, &json!({"handler": "owner"}));
    write_me_json(&agent, &json!({"handler": "agent-a"}));

    let changed = backfill_github_email(workspace, &server.url())
        .await
        .unwrap();
    assert!(changed, "first run should report backfill happened");

    let cfg = WorkspaceConfig::read(workspace).unwrap();
    assert_eq!(
        cfg.git.github_email.as_deref(),
        Some("octo@example.com"),
        "config.json should carry the new email"
    );

    assert_eq!(
        read_me_json(&human)
            .get("github_email")
            .and_then(|v| v.as_str()),
        Some("octo@example.com")
    );
    assert_eq!(
        read_me_json(&agent)
            .get("github_email")
            .and_then(|v| v.as_str()),
        Some("octo@example.com")
    );
    mock.assert_async().await;
}

#[tokio::test]
async fn backfill_derives_noreply_when_user_email_null() {
    let mut server = Server::new_async().await;
    let mock = mock_user_endpoint(&mut server, r#"{"id":42,"login":"octo","email":null}"#).await;

    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path();
    write_config(workspace, github_config(Some("tok"), None));

    let human = workspace.join(".gitim-runtime").join("human");
    write_me_json(&human, &json!({"handler": "owner"}));

    let changed = backfill_github_email(workspace, &server.url())
        .await
        .unwrap();
    assert!(changed);

    let cfg = WorkspaceConfig::read(workspace).unwrap();
    assert_eq!(
        cfg.git.github_email.as_deref(),
        Some("42+octo@users.noreply.github.com")
    );
    mock.assert_async().await;
}

#[tokio::test]
async fn backfill_is_idempotent_when_email_already_set() {
    // No mock — if we hit the network, the request fails and the test fails.
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path();
    write_config(
        workspace,
        github_config(Some("tok"), Some("already@example.com")),
    );

    let human = workspace.join(".gitim-runtime").join("human");
    write_me_json(
        &human,
        &json!({"handler": "owner", "github_email": "already@example.com"}),
    );

    let changed = backfill_github_email(workspace, "http://127.0.0.1:1")
        .await
        .unwrap();
    assert!(!changed, "no-op when email already populated");
}

#[tokio::test]
async fn backfill_uses_existing_config_email_for_missing_clone_fields() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path();
    write_config(
        workspace,
        github_config(Some("tok"), Some("already@example.com")),
    );

    let human = workspace.join(".gitim-runtime").join("human");
    let agent = workspace.join("agent-a");
    write_me_json(&human, &json!({"handler": "owner"}));
    write_me_json(&agent, &json!({"handler": "agent-a"}));

    let changed = backfill_github_email(workspace, "http://127.0.0.1:1")
        .await
        .unwrap();
    assert!(
        changed,
        "config email should still be propagated into clone me.json files"
    );

    assert_eq!(
        read_me_json(&human)
            .get("github_email")
            .and_then(|v| v.as_str()),
        Some("already@example.com")
    );
    assert_eq!(
        read_me_json(&agent)
            .get("github_email")
            .and_then(|v| v.as_str()),
        Some("already@example.com")
    );
}

#[tokio::test]
async fn backfill_skips_local_provider() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path();
    let cfg = WorkspaceConfig {
        workspace: ".".to_string(),
        created_at: "2026-04-17T00:00:00Z".to_string(),
        git: GitConfig {
            provider: GitProvider::Local,
            remote_url: None,
            token: None,
            github_email: None,
        },
    };
    write_config(workspace, cfg);

    let changed = backfill_github_email(workspace, "http://127.0.0.1:1")
        .await
        .unwrap();
    assert!(!changed);
}

#[tokio::test]
async fn backfill_skips_when_token_missing() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path();
    write_config(workspace, github_config(None, None));

    let changed = backfill_github_email(workspace, "http://127.0.0.1:1")
        .await
        .unwrap();
    assert!(!changed);
}

#[tokio::test]
async fn backfill_preserves_existing_me_json_fields() {
    let mut server = Server::new_async().await;
    let _mock = mock_user_endpoint(
        &mut server,
        r#"{"id":42,"login":"octo","email":"octo@example.com"}"#,
    )
    .await;

    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path();
    write_config(workspace, github_config(Some("tok"), None));

    let human = workspace.join(".gitim-runtime").join("human");
    write_me_json(
        &human,
        &json!({
            "handler": "owner",
            "display_name": "Owner",
            "provider": "github",
        }),
    );

    backfill_github_email(workspace, &server.url())
        .await
        .unwrap();

    let me = read_me_json(&human);
    assert_eq!(me.get("handler").and_then(|v| v.as_str()), Some("owner"));
    assert_eq!(
        me.get("display_name").and_then(|v| v.as_str()),
        Some("Owner")
    );
    assert_eq!(me.get("provider").and_then(|v| v.as_str()), Some("github"));
    assert_eq!(
        me.get("github_email").and_then(|v| v.as_str()),
        Some("octo@example.com")
    );
}

#[tokio::test]
async fn backfill_ignores_subdirectories_without_me_json() {
    // A workspace may contain non-clone directories (notes, docs, scratch).
    // They must be tolerated silently — no panic, no unrelated writes.
    let mut server = Server::new_async().await;
    let _mock = mock_user_endpoint(
        &mut server,
        r#"{"id":42,"login":"octo","email":"octo@example.com"}"#,
    )
    .await;

    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path();
    write_config(workspace, github_config(Some("tok"), None));

    let human = workspace.join(".gitim-runtime").join("human");
    write_me_json(&human, &json!({"handler": "owner"}));

    let scratch = workspace.join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::write(scratch.join("note.txt"), "just a note").unwrap();

    backfill_github_email(workspace, &server.url())
        .await
        .unwrap();

    assert!(scratch.join("note.txt").exists());
    assert!(!scratch.join(".gitim").exists());
}

#[tokio::test]
async fn backfill_second_run_is_noop_after_success() {
    let mut server = Server::new_async().await;
    // First call consumes one /user response; after the config is updated,
    // we should never hit the endpoint again.
    let mock = server
        .mock("GET", "/user")
        .with_status(200)
        .with_body(r#"{"id":42,"login":"octo","email":"octo@example.com"}"#)
        .expect(1)
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path();
    write_config(workspace, github_config(Some("tok"), None));

    let human = workspace.join(".gitim-runtime").join("human");
    write_me_json(&human, &json!({"handler": "owner"}));

    let first = backfill_github_email(workspace, &server.url())
        .await
        .unwrap();
    assert!(first);

    let second = backfill_github_email(workspace, &server.url())
        .await
        .unwrap();
    assert!(
        !second,
        "second run should short-circuit before the API call"
    );

    mock.assert_async().await;
}
