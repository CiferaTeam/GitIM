//! Flow run handler integration tests. Tempdir repo + in-process daemon state.
//! Follows the same setup pattern as flow_handlers.rs.

use std::path::Path;
use std::sync::Arc;

use gitim_core::types::Config;
use gitim_daemon::api::Event;
use gitim_daemon::state::AppState;
use tempfile::TempDir;
use tokio::sync::broadcast;

fn git(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .expect("git command failed");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn make_config() -> Config {
    serde_yaml::from_str("version: 1").unwrap()
}

async fn setup() -> (TempDir, Arc<AppState>) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git(root, &["init"]);
    git(root, &["config", "user.name", "test"]);
    git(root, &["config", "user.email", "test@example.com"]);

    std::fs::create_dir_all(root.join(".gitim")).unwrap();
    std::fs::create_dir_all(root.join("users")).unwrap();
    std::fs::create_dir_all(root.join("channels")).unwrap();
    std::fs::write(root.join(".gitim/config.yaml"), "version: 1").unwrap();
    std::fs::write(
        root.join("users/lewis.meta.yaml"),
        "display_name: Lewis\nrole: dev\nintroduction: hi\n",
    )
    .unwrap();
    // channel used by run tests
    std::fs::write(
        root.join("channels/release-discuss.meta.yaml"),
        "name: release-discuss\ndisplay_name: Release\nintroduction: x\nmembers: [lewis]\ncreated_at: 2026-05-17T10:00:00Z\n",
    )
    .unwrap();
    std::fs::write(root.join("channels/release-discuss.thread"), "").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "init"]);

    let (event_tx, _) = broadcast::channel::<Event>(64);
    let state = Arc::new(AppState::new(
        root.to_path_buf(),
        make_config(),
        event_tx,
        Some("lewis".to_string()),
    ));
    {
        let mut users = state.users.write().await;
        *users = vec!["lewis".to_string()];
    }

    // create a flow template with 2 nodes (changelog -> e2e)
    let r = gitim_daemon::flow_handlers::handle_flow_create(
        state.clone(),
        "release".into(),
        "Release Flow".into(),
        "test".into(),
        "lewis".into(),
    )
    .await;
    assert!(r.ok, "flow create pre-condition failed: {:?}", r.error);

    // overwrite the stub template to add 2 nodes
    let template_yaml = "---
schema_version: 1
slug: release
name: Release Flow
description: test
created_by: lewis
created_at: 2026-05-17T10:00:00Z
updated_at: 2026-05-17T10:00:00Z
nodes:
  - id: changelog
    type: agent_mention
    owner: alice
    needs: []
  - id: e2e
    type: agent_mention
    owner: bob
    needs: [changelog]
---

## changelog

generate changelog

## e2e

run tests
";
    std::fs::write(root.join("flows/release/index.md"), template_yaml).unwrap();
    git(root, &["add", "flows/release/index.md"]);
    git(root, &["commit", "-m", "add nodes"]);

    (tmp, state)
}

#[tokio::test]
async fn run_start_then_node_set_then_auto_complete() {
    let (_tmp, state) = setup().await;

    // start run
    let r = gitim_daemon::flow_run_handlers::handle_flow_run_start(
        state.clone(),
        "release".into(),
        "release-discuss".into(),
        "lewis".into(),
    )
    .await;
    assert!(r.ok, "run start failed: {:?}", r.error);
    let data = r.data.unwrap();
    let run_id = data["run_id"].as_str().unwrap().to_string();
    assert_eq!(data["flow_slug"], "release");
    assert_eq!(data["channel"], "release-discuss");

    // node-set changelog -> in_progress
    let r = gitim_daemon::flow_run_handlers::handle_flow_node_set(
        state.clone(),
        run_id.clone(),
        "changelog".into(),
        "in_progress".into(),
        Some("alice".into()),
        None,
        "alice".into(),
    )
    .await;
    assert!(r.ok, "node_set in_progress failed: {:?}", r.error);

    // node-set changelog -> done
    let r = gitim_daemon::flow_run_handlers::handle_flow_node_set(
        state.clone(),
        run_id.clone(),
        "changelog".into(),
        "done".into(),
        Some("alice".into()),
        None,
        "alice".into(),
    )
    .await;
    assert!(r.ok, "node_set changelog done failed: {:?}", r.error);
    // e2e not yet done, so run is still in_progress
    assert_eq!(
        r.data.as_ref().unwrap()["run_status"],
        "in_progress",
        "run should still be in_progress after one node done"
    );

    // node-set e2e -> done
    let r = gitim_daemon::flow_run_handlers::handle_flow_node_set(
        state.clone(),
        run_id.clone(),
        "e2e".into(),
        "done".into(),
        Some("bob".into()),
        None,
        "bob".into(),
    )
    .await;
    assert!(r.ok, "node_set e2e done failed: {:?}", r.error);
    // all nodes done -> run auto-completes
    assert_eq!(
        r.data.as_ref().unwrap()["run_status"],
        "done",
        "run should be done after all nodes done"
    );

    // show confirms run.status = done
    let r =
        gitim_daemon::flow_run_handlers::handle_flow_run_show(state.clone(), run_id.clone()).await;
    assert!(r.ok, "run show failed: {:?}", r.error);
    assert_eq!(r.data.unwrap()["status"], "done");
}

#[tokio::test]
async fn run_start_for_unknown_channel_returns_not_found() {
    let (_tmp, state) = setup().await;
    let r = gitim_daemon::flow_run_handlers::handle_flow_run_start(
        state,
        "release".into(),
        "no-such-channel".into(),
        "lewis".into(),
    )
    .await;
    assert!(!r.ok, "expected failure for unknown channel");
    assert_eq!(
        r.error_code.as_deref(),
        Some("not_found"),
        "expected error_code=not_found, got: {:?}",
        r.error_code
    );
}

#[tokio::test]
async fn run_node_set_rejects_invalid_transition() {
    let (_tmp, state) = setup().await;

    let r = gitim_daemon::flow_run_handlers::handle_flow_run_start(
        state.clone(),
        "release".into(),
        "release-discuss".into(),
        "lewis".into(),
    )
    .await;
    assert!(r.ok, "run start pre-condition failed: {:?}", r.error);
    let run_id = r.data.unwrap()["run_id"].as_str().unwrap().to_string();

    // set changelog -> done
    let r = gitim_daemon::flow_run_handlers::handle_flow_node_set(
        state.clone(),
        run_id.clone(),
        "changelog".into(),
        "done".into(),
        Some("alice".into()),
        None,
        "alice".into(),
    )
    .await;
    assert!(r.ok, "node_set done pre-condition failed: {:?}", r.error);

    // try to set back to in_progress -> must be rejected
    let r = gitim_daemon::flow_run_handlers::handle_flow_node_set(
        state,
        run_id,
        "changelog".into(),
        "in_progress".into(),
        Some("alice".into()),
        None,
        "alice".into(),
    )
    .await;
    assert!(!r.ok, "expected rejection for invalid transition");
    assert!(
        r.error.as_deref().unwrap_or("").contains("transition"),
        "expected 'transition' in error, got: {:?}",
        r.error
    );
}

#[tokio::test]
async fn run_cancel_then_node_set_rejected() {
    let (_tmp, state) = setup().await;

    let r = gitim_daemon::flow_run_handlers::handle_flow_run_start(
        state.clone(),
        "release".into(),
        "release-discuss".into(),
        "lewis".into(),
    )
    .await;
    assert!(r.ok, "run start pre-condition failed: {:?}", r.error);
    let run_id = r.data.unwrap()["run_id"].as_str().unwrap().to_string();

    let r = gitim_daemon::flow_run_handlers::handle_flow_run_cancel(
        state.clone(),
        run_id.clone(),
        "lewis".into(),
    )
    .await;
    assert!(r.ok, "run cancel failed: {:?}", r.error);

    // node_set on a cancelled run -> rejected
    let r = gitim_daemon::flow_run_handlers::handle_flow_node_set(
        state,
        run_id,
        "changelog".into(),
        "done".into(),
        Some("alice".into()),
        None,
        "alice".into(),
    )
    .await;
    assert!(!r.ok, "expected rejection after cancel");
    assert!(
        r.error.as_deref().unwrap_or("").contains("terminal"),
        "expected 'terminal' in error, got: {:?}",
        r.error
    );
}

/// Setup variant: creates a flow with *no* nodes (the default stub state).
async fn setup_zero_node() -> (TempDir, Arc<AppState>) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git(root, &["init"]);
    git(root, &["config", "user.name", "test"]);
    git(root, &["config", "user.email", "test@example.com"]);

    std::fs::create_dir_all(root.join(".gitim")).unwrap();
    std::fs::create_dir_all(root.join("users")).unwrap();
    std::fs::create_dir_all(root.join("channels")).unwrap();
    std::fs::write(root.join(".gitim/config.yaml"), "version: 1").unwrap();
    std::fs::write(
        root.join("users/lewis.meta.yaml"),
        "display_name: Lewis\nrole: dev\nintroduction: hi\n",
    )
    .unwrap();
    std::fs::write(
        root.join("channels/release-discuss.meta.yaml"),
        "name: release-discuss\ndisplay_name: Release\nintroduction: x\nmembers: [lewis]\ncreated_at: 2026-05-17T10:00:00Z\n",
    )
    .unwrap();
    std::fs::write(root.join("channels/release-discuss.thread"), "").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "init"]);

    let (event_tx, _) = broadcast::channel::<Event>(64);
    let state = Arc::new(AppState::new(
        root.to_path_buf(),
        make_config(),
        event_tx,
        Some("lewis".to_string()),
    ));
    {
        let mut users = state.users.write().await;
        *users = vec!["lewis".to_string()];
    }

    // Create a flow with no nodes (empty nodes list — the default stub state).
    let flow_dir = root.join("flows").join("empty");
    std::fs::create_dir_all(&flow_dir).unwrap();
    let template_yaml = "---\nschema_version: 1\nslug: empty\nname: Empty\ndescription: zero nodes\ncreated_by: lewis\ncreated_at: 20260517T100000Z\nupdated_at: 20260517T100000Z\nnodes: []\n---\n";
    std::fs::write(flow_dir.join("index.md"), template_yaml).unwrap();
    git(root, &["add", "flows/empty/index.md"]);
    git(root, &["commit", "-m", "add empty flow"]);

    (tmp, state)
}

#[tokio::test]
async fn run_start_with_zero_node_template_auto_completes() {
    let (_dir, state) = setup_zero_node().await;
    let r = gitim_daemon::flow_run_handlers::handle_flow_run_start(
        state.clone(),
        "empty".into(),
        "release-discuss".into(),
        "lewis".into(),
    )
    .await;
    assert!(r.ok, "start: {:?}", r.error);
    let run_id = r.data.unwrap()["run_id"].as_str().unwrap().to_string();

    // show should immediately report done (no nodes to execute)
    let r = gitim_daemon::flow_run_handlers::handle_flow_run_show(state.clone(), run_id).await;
    assert!(r.ok, "show failed: {:?}", r.error);
    assert_eq!(
        r.data.unwrap()["status"],
        "done",
        "zero-node run should start as done"
    );
}

#[tokio::test]
async fn flow_remove_cleans_up_runs() {
    let (_dir, state) = setup().await;

    // start a run
    let r = gitim_daemon::flow_run_handlers::handle_flow_run_start(
        state.clone(),
        "release".into(),
        "release-discuss".into(),
        "lewis".into(),
    )
    .await;
    assert!(r.ok, "run start failed: {:?}", r.error);
    let run_id = r.data.unwrap()["run_id"].as_str().unwrap().to_string();

    // remove the flow
    let r = gitim_daemon::flow_handlers::handle_flow_remove(
        state.clone(),
        "release".into(),
        "lewis".into(),
    )
    .await;
    assert!(r.ok, "flow remove: {:?}", r.error);

    // run should no longer be reachable
    let r =
        gitim_daemon::flow_run_handlers::handle_flow_run_show(state.clone(), run_id.clone()).await;
    assert!(!r.ok, "show should fail after flow remove");

    // list should return 0 runs (orphan runs/ tree is gone with the flow dir)
    let r = gitim_daemon::flow_run_handlers::handle_flow_run_list(state, None, None, None).await;
    let runs = r.data.unwrap()["runs"].as_array().unwrap().clone();
    assert_eq!(runs.len(), 0, "runs list should be empty after flow remove");
}

#[tokio::test]
async fn run_list_filters_by_channel() {
    let (_tmp, state) = setup().await;

    // start 2 runs in release-discuss
    for _ in 0..2 {
        let r = gitim_daemon::flow_run_handlers::handle_flow_run_start(
            state.clone(),
            "release".into(),
            "release-discuss".into(),
            "lewis".into(),
        )
        .await;
        assert!(r.ok, "run start failed: {:?}", r.error);
    }

    // list by channel -> 2 results
    let r = gitim_daemon::flow_run_handlers::handle_flow_run_list(
        state.clone(),
        None,
        Some("release-discuss".into()),
        None,
    )
    .await;
    assert!(r.ok, "run list failed: {:?}", r.error);
    let runs = r.data.unwrap()["runs"].as_array().unwrap().clone();
    assert_eq!(runs.len(), 2, "expected 2 runs for release-discuss");

    // list by unknown channel -> 0 results
    let r = gitim_daemon::flow_run_handlers::handle_flow_run_list(
        state,
        None,
        Some("no-such-channel".into()),
        None,
    )
    .await;
    assert!(r.ok, "run list for empty channel failed: {:?}", r.error);
    let runs = r.data.unwrap()["runs"].as_array().unwrap().clone();
    assert_eq!(runs.len(), 0, "expected 0 runs for unknown channel");
}
