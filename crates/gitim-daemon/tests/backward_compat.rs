#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Backward-compatibility tests for the `project` field on channel meta.
//!
//! Task 11: guards two invariants —
//!   1. Old channel meta YAML (no `project` field) is parsed as `None` by the
//!      daemon and does not break IPC responses (list_channels, set_channel_project,
//!      list_projects).
//!   2. New channel meta with `project` set does not pollute old-style consumers:
//!      `project` is skipped on serialization when `None`, and old callers that
//!      read the raw response JSON see no unexpected `project` key.

use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::broadcast;

use gitim_core::types::Config;
use gitim_daemon::api::Request;
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::AppState;

fn make_config() -> Config {
    serde_yaml::from_str("version: 1").unwrap()
}

/// Build a repo that already contains a channel whose meta.yaml was written by
/// an older daemon that did NOT know about the `project` field.
async fn setup_repo_with_legacy_channel() -> (TempDir, Arc<AppState>) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();

    std::fs::create_dir_all(root.join("users")).unwrap();
    std::fs::create_dir_all(root.join("channels")).unwrap();

    // Register alice using the old-style user meta (no newer fields).
    std::fs::write(
        root.join("users/alice.meta.yaml"),
        "display_name: Alice\nrole: dev\nintroduction: hi\n",
    )
    .unwrap();

    // Write a channel meta WITHOUT the `project` field — simulating an old repo.
    std::fs::write(
        root.join("channels/general.meta.yaml"),
        "display_name: General\ncreated_by: alice\ncreated_at: \"2026-01-01T00:00:00Z\"\nintroduction: General chat\nmembers:\n- alice\n",
    )
    .unwrap();
    // A minimal thread file is required for the channel to be valid.
    std::fs::write(root.join("channels/general.thread"), "").unwrap();

    let run_git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .unwrap()
    };
    run_git(&["init"]);
    run_git(&["add", "."]);
    run_git(&["commit", "-m", "init"]);

    let (tx, _) = broadcast::channel(100);
    let state = Arc::new(AppState::new(
        root,
        make_config(),
        tx,
        Some("alice".to_string()),
    ));
    {
        let mut users = state.users.write().await;
        *users = vec!["alice".to_string()];
    }

    (tmp, state)
}

// ─── 1. Old meta without project field: list_channels IPC does not error ─────

#[tokio::test]
async fn old_channel_meta_without_project_list_channels_ok() {
    let (_tmp, state) = setup_repo_with_legacy_channel().await;

    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "channels",
    }))
    .unwrap();
    let resp = handle_request(req, state.clone()).await;

    assert!(
        resp.ok,
        "list_channels must succeed against old meta (no project field): {:?}",
        resp.error
    );

    // The general channel must appear in the response.
    let channels = resp.data.unwrap()["channels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap_or("").to_string())
        .collect::<Vec<_>>();
    assert!(
        channels.contains(&"general".to_string()),
        "general channel must appear in list_channels response: {:?}",
        channels
    );
}

// ─── 2. Old meta without project field: set_channel_project can assign it ────

#[tokio::test]
async fn old_channel_meta_without_project_can_be_assigned_to_project() {
    let (_tmp, state) = setup_repo_with_legacy_channel().await;

    // Create a project first.
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_project",
        "slug": "eng",
        "display_name": "Engineering",
        "introduction": "All eng work",
        "author": "alice",
    }))
    .unwrap();
    let resp = handle_request(req, state.clone()).await;
    assert!(resp.ok, "create_project failed: {:?}", resp.error);

    // Assign the legacy channel (no project field in meta) to the new project.
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "set_channel_project",
        "channel": "general",
        "project": "eng",
        "author": "alice",
    }))
    .unwrap();
    let resp = handle_request(req, state.clone()).await;
    assert!(
        resp.ok,
        "set_channel_project must succeed against old meta: {:?}",
        resp.error
    );

    // Verify the project field is now written.
    let meta_str =
        std::fs::read_to_string(state.repo_root.join("channels/general.meta.yaml")).unwrap();
    assert!(
        meta_str.contains("project: eng"),
        "project field must appear after assignment:\n{meta_str}"
    );
}

// ─── 3. Old meta without project field: list_projects channel_count is correct

#[tokio::test]
async fn old_channel_meta_without_project_list_projects_channel_count_zero() {
    let (_tmp, state) = setup_repo_with_legacy_channel().await;

    // Create a project.
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_project",
        "slug": "infra",
        "display_name": "Infra",
        "introduction": "Infrastructure",
        "author": "alice",
    }))
    .unwrap();
    let resp = handle_request(req, state.clone()).await;
    assert!(resp.ok, "create_project failed: {:?}", resp.error);

    // list_projects should return channel_count=0 for this project because the
    // legacy channel (no project field → None) is not counted.
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "list_projects",
    }))
    .unwrap();
    let resp = handle_request(req, state.clone()).await;
    assert!(resp.ok, "list_projects failed: {:?}", resp.error);

    let projects = resp.data.unwrap()["projects"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["slug"].as_str() == Some("infra"))
        .map(|p| p["channel_count"].as_u64().unwrap_or(999))
        .collect::<Vec<_>>();
    assert_eq!(
        projects,
        vec![0u64],
        "legacy channel without project field must not be counted in channel_count"
    );
}

// ─── 4. New meta with project: None → project key absent from serialized YAML ─

#[test]
fn new_channel_meta_none_project_absent_from_yaml() {
    // Guards that ChannelMeta with project=None serializes without a `project:` key,
    // so old consumers that don't know about the field see a clean YAML.
    let meta = gitim_core::types::ChannelMeta {
        display_name: "General".to_string(),
        created_by: "alice".to_string(),
        created_at: "2026-01-01T00:00:00Z".to_string(),
        introduction: "hi".to_string(),
        members: vec!["alice".to_string()],
        project: None,
    };
    let yaml = serde_yaml::to_string(&meta).expect("serialize");
    assert!(
        !yaml.contains("project"),
        "project field must be absent from YAML when None; got:\n{yaml}"
    );
}

// ─── 5. Old YAML without project field → ChannelMeta.project == None ─────────

#[test]
fn old_yaml_without_project_parses_to_none() {
    // Guards the serde default for `project` at the daemon crate level —
    // confirms that any YAML parsed by daemon code (e.g. in archive/unarchive
    // handlers that call serde_yaml::from_str::<ChannelMeta>) will correctly
    // treat a missing field as None.
    let yaml = "display_name: General\ncreated_by: alice\ncreated_at: \"2026-01-01T00:00:00Z\"\nintroduction: hi\nmembers:\n- alice\n";
    let meta: gitim_core::types::ChannelMeta = serde_yaml::from_str(yaml).expect("parse old meta");
    assert_eq!(
        meta.project, None,
        "old YAML without project field must parse as None"
    );
}
