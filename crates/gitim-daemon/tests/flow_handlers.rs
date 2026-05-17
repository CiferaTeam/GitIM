//! Flow handler integration tests. Tempdir repo + in-process daemon state.
//! Follows the same setup pattern as board_test.rs.

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
    std::fs::write(root.join(".gitim/config.yaml"), "version: 1").unwrap();
    std::fs::write(
        root.join("users/lewis.meta.yaml"),
        "display_name: Lewis\nrole: dev\nintroduction: hi\n",
    )
    .unwrap();
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

    (tmp, state)
}

#[tokio::test]
async fn flow_create_then_list_then_show_then_validate_then_remove() {
    let (_tmp, state) = setup().await;

    // 1. list flows on empty repo → []
    let r = gitim_daemon::flow_handlers::handle_flow_list(state.clone()).await;
    assert!(r.ok, "flow_list failed: {:?}", r.error);
    let flows = r.data.unwrap();
    assert_eq!(
        flows["flows"].as_array().unwrap().len(),
        0,
        "expected empty flows list"
    );

    // 2. create stub flow
    let r = gitim_daemon::flow_handlers::handle_flow_create(
        state.clone(),
        "release".into(),
        "Release Flow".into(),
        "test".into(),
        "lewis".into(),
    )
    .await;
    assert!(r.ok, "flow_create failed: {:?}", r.error);
    assert_eq!(r.data.as_ref().unwrap()["slug"], "release");
    assert_eq!(r.data.as_ref().unwrap()["status"], "committed");

    // 3. list now contains it
    let r = gitim_daemon::flow_handlers::handle_flow_list(state.clone()).await;
    assert!(r.ok, "flow_list after create failed: {:?}", r.error);
    let flows = r.data.unwrap();
    let arr = flows["flows"].as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["slug"], "release");
    assert_eq!(arr[0]["name"], "Release Flow");

    // 4. show returns raw_markdown + 0 nodes
    let r = gitim_daemon::flow_handlers::handle_flow_show(state.clone(), "release".into()).await;
    assert!(r.ok, "flow_show failed: {:?}", r.error);
    let data = r.data.unwrap();
    assert_eq!(data["slug"], "release");
    assert_eq!(data["name"], "Release Flow");
    let raw = data["raw_markdown"].as_str().unwrap();
    assert!(
        raw.starts_with("---\n"),
        "raw_markdown should start with YAML frontmatter"
    );
    assert_eq!(
        data["nodes"].as_array().unwrap().len(),
        0,
        "stub flow has no nodes"
    );

    // 5. validate → ok with no errors
    let r =
        gitim_daemon::flow_handlers::handle_flow_validate(state.clone(), "release".into()).await;
    assert!(r.ok, "flow_validate response failed: {:?}", r.error);
    let data = r.data.unwrap();
    assert!(
        data["ok"].as_bool().unwrap(),
        "validate should report ok=true"
    );

    // 6. remove
    let r = gitim_daemon::flow_handlers::handle_flow_remove(
        state.clone(),
        "release".into(),
        "lewis".into(),
    )
    .await;
    assert!(r.ok, "flow_remove failed: {:?}", r.error);
    assert_eq!(r.data.as_ref().unwrap()["status"], "removed");
    assert_eq!(r.data.as_ref().unwrap()["slug"], "release");

    // 7. list again — empty
    let r = gitim_daemon::flow_handlers::handle_flow_list(state.clone()).await;
    assert!(r.ok, "flow_list after remove failed: {:?}", r.error);
    let flows = r.data.unwrap();
    assert_eq!(
        flows["flows"].as_array().unwrap().len(),
        0,
        "expected empty after remove"
    );
}

#[tokio::test]
async fn flow_validate_reports_orphan_section_warning() {
    let (_tmp, state) = setup().await;

    // Manually drop a flow with an orphan body section (node "ghost" has no
    // corresponding entry in the nodes list).
    let flow_dir = state.repo_root.join("flows").join("test");
    std::fs::create_dir_all(&flow_dir).unwrap();
    let md = "---\nschema_version: 1\nslug: test\nname: Test\ncreated_by: lewis\ncreated_at: 2026-05-12T10:00:00Z\nnodes:\n  - id: a\n    type: agent_mention\n    owner: alice\n    needs: []\n---\n\n## a\n\nprompt for a\n\n## ghost\n\nunrelated section\n";
    std::fs::write(flow_dir.join("index.md"), md).unwrap();

    let r = gitim_daemon::flow_handlers::handle_flow_validate(state.clone(), "test".into()).await;
    assert!(r.ok, "validate response itself should be ok: {:?}", r.error);
    let data = r.data.unwrap();
    // The document-level validate flag: may be true (orphan is a warning not an error)
    let items = data["items"].as_array().unwrap();
    let has_orphan_warning = items.iter().any(|item| {
        item["kind"].as_str() == Some("warning")
            && item["message"]
                .as_str()
                .map(|m| m.contains("orphan"))
                .unwrap_or(false)
    });
    assert!(
        has_orphan_warning,
        "expected an orphan-section warning, items: {:?}",
        items
    );
}

#[tokio::test]
async fn flow_create_invalid_slug_rejected() {
    let (_tmp, state) = setup().await;
    let r = gitim_daemon::flow_handlers::handle_flow_create(
        state.clone(),
        "INVALID_UPPER".into(),
        "x".into(),
        "".into(),
        "lewis".into(),
    )
    .await;
    assert!(
        !r.ok,
        "invalid slug should be rejected, got ok=true with data: {:?}",
        r.data
    );
    assert!(r.error.is_some(), "expected error message for invalid slug");
}

/// Depart a user by moving their meta.yaml to archive/users/ and committing,
/// mirroring what handle_depart_user does under the hood.
fn depart_user_fs(root: &Path, handler: &str) {
    let archive_dir = root.join("archive").join("users");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let src = root.join("users").join(format!("{}.meta.yaml", handler));
    let dst = archive_dir.join(format!("{}.meta.yaml", handler));
    std::fs::rename(&src, &dst).unwrap();
    git(root, &["add", "."]);
    git(
        root,
        &[
            "commit",
            "-m",
            &format!("archive: depart user @{}", handler),
        ],
    );
}

#[tokio::test]
async fn flow_create_rejected_for_departed_user() {
    let (_tmp, state) = setup().await;

    // Depart lewis before attempting to create a flow.
    depart_user_fs(&state.repo_root, "lewis");

    let r = gitim_daemon::flow_handlers::handle_flow_create(
        state.clone(),
        "my-flow".into(),
        "My Flow".into(),
        "A test flow".into(),
        "lewis".into(),
    )
    .await;
    assert!(
        !r.ok,
        "departed user should not be able to create a flow, got ok=true with data: {:?}",
        r.data
    );
    assert!(
        r.error.as_deref().unwrap_or("").contains("departed"),
        "expected 'departed' in error message, got: {:?}",
        r.error
    );
}

#[tokio::test]
async fn flow_remove_rejected_for_departed_user() {
    let (_tmp, state) = setup().await;

    // Create a flow as lewis while still active.
    let r = gitim_daemon::flow_handlers::handle_flow_create(
        state.clone(),
        "to-remove".into(),
        "To Remove".into(),
        "Will be rejected".into(),
        "lewis".into(),
    )
    .await;
    assert!(r.ok, "flow_create pre-condition failed: {:?}", r.error);

    // Now depart lewis.
    depart_user_fs(&state.repo_root, "lewis");

    // Attempt to remove the flow as the departed user — must be rejected.
    let r = gitim_daemon::flow_handlers::handle_flow_remove(
        state.clone(),
        "to-remove".into(),
        "lewis".into(),
    )
    .await;
    assert!(
        !r.ok,
        "departed user should not be able to remove a flow, got ok=true with data: {:?}",
        r.data
    );
    assert!(
        r.error.as_deref().unwrap_or("").contains("departed"),
        "expected 'departed' in error message, got: {:?}",
        r.error
    );

    // Verify the flow still exists (not deleted).
    let flow_path = state
        .repo_root
        .join("flows")
        .join("to-remove")
        .join("index.md");
    assert!(
        flow_path.exists(),
        "flow should still exist after rejected remove"
    );
}
