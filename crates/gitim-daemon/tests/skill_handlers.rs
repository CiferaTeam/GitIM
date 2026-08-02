#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use gitim_core::skill::EventId;
use gitim_daemon::api::Request;
use gitim_daemon::handlers::handle_request;

fn write_package(root: &Path, slug: &str, marker: &str) -> PathBuf {
    let source = root.join(format!("handler-source-{marker}"));
    std::fs::create_dir_all(source.join("references")).unwrap();
    std::fs::write(
        source.join("SKILL.md"),
        format!("---\nname: {slug}\ndescription: Shared release checks.\n---\n\n# {marker}\n"),
    )
    .unwrap();
    std::fs::write(source.join("references/checklist.md"), marker).unwrap();
    source
}

async fn set_actor(state: &gitim_daemon::state::SharedState, actor: &str) {
    *state.current_user.write().await = Some(actor.to_string());
}

#[test]
fn skill_requests_deserialize_with_stable_method_names() {
    let create: Request = serde_json::from_str(
        r#"{"method":"skill_create","slug":"release-check","source_directory":"/tmp/package","display_name":"Release Check","description":"Verify releases."}"#,
    )
    .unwrap();
    assert!(matches!(create, Request::SkillCreate { .. }));

    let load: Request = serde_json::from_str(
        r#"{"method":"skill_load","reference":"skill:release-check@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N"}"#,
    )
    .unwrap();
    assert!(matches!(load, Request::SkillLoad { .. }));

    let publish: Request = serde_json::from_str(
        r#"{"method":"skill_proposal_publish","slug":"release-check","proposal":"p-01K1D8QG2S8RX4T9M9BDKQ9Z7N"}"#,
    )
    .unwrap();
    assert!(matches!(publish, Request::SkillProposalPublish { .. }));
}

#[tokio::test]
async fn invalid_skill_write_input_is_not_reported_as_corrupt_history() {
    let (tmp, state) = common::setup_repo_alice().await;
    let source = write_package(tmp.path(), "release-check", "v1");
    let response = handle_request(
        Request::SkillCreate {
            slug: "release-check".to_string(),
            source_directory: source.to_string_lossy().to_string(),
            display_name: " ".to_string(),
            description: "Verify releases.".to_string(),
            event_id: None,
        },
        state,
    )
    .await;

    assert!(!response.ok);
    assert_eq!(response.error_code.as_deref(), Some("skill_invalid_input"));
}

#[tokio::test]
async fn invalid_role_target_is_reported_as_invalid_input() {
    let (tmp, state) = common::setup_repo_alice().await;
    common::run_git(tmp.path(), &["config", "user.name", "Test"]);
    common::run_git(tmp.path(), &["config", "user.email", "test@example.com"]);
    let source = write_package(tmp.path(), "release-check", "v1");
    let created = handle_request(
        Request::SkillCreate {
            slug: "release-check".to_string(),
            source_directory: source.to_string_lossy().to_string(),
            display_name: "Release Check".to_string(),
            description: "Verify releases.".to_string(),
            event_id: None,
        },
        state.clone(),
    )
    .await;
    assert!(created.ok, "{:?}", created.error);

    let response = handle_request(
        Request::SkillMaintainerAdd {
            slug: "release-check".to_string(),
            handler: "INVALID HANDLER".to_string(),
            event_id: None,
        },
        state,
    )
    .await;

    assert!(!response.ok);
    assert_eq!(response.error_code.as_deref(), Some("skill_invalid_input"));
}

#[tokio::test]
async fn handler_lifecycle_creates_proposes_and_publishes() {
    let (tmp, state) = common::setup_repo_alice_bob().await;
    common::run_git(tmp.path(), &["config", "user.name", "Test"]);
    common::run_git(tmp.path(), &["config", "user.email", "test@example.com"]);
    let v1 = write_package(tmp.path(), "release-check", "v1");

    let created = handle_request(
        Request::SkillCreate {
            slug: "release-check".to_string(),
            source_directory: v1.to_string_lossy().to_string(),
            display_name: "Release Check".to_string(),
            description: "Verify releases.".to_string(),
            event_id: None,
        },
        state.clone(),
    )
    .await;
    assert!(created.ok, "{:?}", created.error);
    let created_data = created.data.unwrap();
    let initial_revision = created_data["revision"].as_str().unwrap().to_string();
    assert_eq!(
        created_data["canonical_ref"].as_str().unwrap(),
        format!("skill:release-check@{initial_revision}")
    );

    let listed = handle_request(
        Request::SkillList {
            archived: false,
            limit: Some(50),
            after: None,
        },
        state.clone(),
    )
    .await;
    assert!(listed.ok);
    assert_eq!(listed.data.unwrap()["skills"].as_array().unwrap().len(), 1);

    set_actor(&state, "bob").await;
    let v2 = write_package(tmp.path(), "release-check", "v2");
    let proposed = handle_request(
        Request::SkillPropose {
            slug: "release-check".to_string(),
            source_directory: v2.to_string_lossy().to_string(),
            base_revision: initial_revision,
            summary: "Add rollback verification.".to_string(),
            event_id: None,
        },
        state.clone(),
    )
    .await;
    assert!(proposed.ok, "{:?}", proposed.error);
    let proposed_data = proposed.data.unwrap();
    let proposal = proposed_data["proposal"].as_str().unwrap().to_string();
    let candidate_revision = proposed_data["revision"].as_str().unwrap().to_string();

    let denied = handle_request(
        Request::SkillProposalPublish {
            slug: "release-check".to_string(),
            proposal: proposal.clone(),
            event_id: None,
        },
        state.clone(),
    )
    .await;
    assert!(!denied.ok);
    assert_eq!(denied.error_code.as_deref(), Some("skill_not_maintainer"));

    let proposal_detail = handle_request(
        Request::SkillProposalShow {
            slug: "release-check".to_string(),
            proposal: proposal.clone(),
        },
        state.clone(),
    )
    .await;
    assert!(proposal_detail.ok);
    assert!(proposal_detail.data.unwrap()["skill_markdown"]
        .as_str()
        .unwrap()
        .contains("# v2"));

    set_actor(&state, "alice").await;
    let published = handle_request(
        Request::SkillProposalPublish {
            slug: "release-check".to_string(),
            proposal,
            event_id: None,
        },
        state.clone(),
    )
    .await;
    assert!(published.ok, "{:?}", published.error);
    assert_eq!(
        published.data.unwrap()["current_revision"].as_str(),
        Some(candidate_revision.as_str())
    );

    let loaded = handle_request(
        Request::SkillLoad {
            reference: format!("skill:release-check@{candidate_revision}"),
        },
        state,
    )
    .await;
    assert!(loaded.ok);
    assert!(loaded.data.unwrap()["skill_markdown"]
        .as_str()
        .unwrap()
        .contains("# v2"));
}

#[tokio::test]
async fn handler_supports_comments_roles_metadata_and_archive_history() {
    let (tmp, state) = common::setup_repo_alice_bob().await;
    common::run_git(tmp.path(), &["config", "user.name", "Test"]);
    common::run_git(tmp.path(), &["config", "user.email", "test@example.com"]);
    let v1 = write_package(tmp.path(), "release-check", "v1");
    let created = handle_request(
        Request::SkillCreate {
            slug: "release-check".to_string(),
            source_directory: v1.to_string_lossy().to_string(),
            display_name: "Release Check".to_string(),
            description: "Verify releases.".to_string(),
            event_id: None,
        },
        state.clone(),
    )
    .await;
    let initial = created.data.unwrap()["revision"]
        .as_str()
        .unwrap()
        .to_string();

    let role = handle_request(
        Request::SkillOwnerAdd {
            slug: "release-check".to_string(),
            handler: "bob".to_string(),
            event_id: None,
        },
        state.clone(),
    )
    .await;
    assert!(role.ok, "{:?}", role.error);

    set_actor(&state, "bob").await;
    let update = handle_request(
        Request::SkillMetadataUpdate {
            slug: "release-check".to_string(),
            display_name: Some("Release Gate".to_string()),
            description: None,
            event_id: None,
        },
        state.clone(),
    )
    .await;
    assert!(update.ok, "{:?}", update.error);

    let v2 = write_package(tmp.path(), "release-check", "v2");
    let proposed = handle_request(
        Request::SkillPropose {
            slug: "release-check".to_string(),
            source_directory: v2.to_string_lossy().to_string(),
            base_revision: initial,
            summary: "Improve checks.".to_string(),
            event_id: None,
        },
        state.clone(),
    )
    .await;
    let proposal = proposed.data.unwrap()["proposal"]
        .as_str()
        .unwrap()
        .to_string();

    set_actor(&state, "alice").await;
    let comment = handle_request(
        Request::SkillProposalComment {
            slug: "release-check".to_string(),
            proposal: proposal.clone(),
            body: "Please cover rollback.".to_string(),
            event_id: None,
        },
        state.clone(),
    )
    .await;
    assert!(comment.ok, "{:?}", comment.error);

    let proposals = handle_request(
        Request::SkillProposalList {
            slug: "release-check".to_string(),
            status: None,
            limit: None,
            after: None,
        },
        state.clone(),
    )
    .await;
    let listed = &proposals.data.unwrap()["proposals"][0];
    assert!(listed.get("comments").is_none());

    let proposal_detail = handle_request(
        Request::SkillProposalShow {
            slug: "release-check".to_string(),
            proposal: proposal.clone(),
        },
        state.clone(),
    )
    .await;
    assert_eq!(proposal_detail.data.unwrap()["comments_truncated"], false);

    let reject = handle_request(
        Request::SkillProposalReject {
            slug: "release-check".to_string(),
            proposal,
            event_id: None,
        },
        state.clone(),
    )
    .await;
    assert!(reject.ok, "{:?}", reject.error);

    set_actor(&state, "bob").await;
    let archived = handle_request(
        Request::SkillArchive {
            slug: "release-check".to_string(),
            event_id: None,
        },
        state.clone(),
    )
    .await;
    assert!(archived.ok, "{:?}", archived.error);

    let show = handle_request(
        Request::SkillShow {
            slug: "release-check".to_string(),
        },
        state.clone(),
    )
    .await;
    let show_data = show.data.unwrap();
    assert_eq!(show_data["display_name"], "Release Gate");
    assert_eq!(show_data["archived"], true);

    let history = handle_request(
        Request::SkillHistory {
            slug: "release-check".to_string(),
            limit: Some(100),
            after: None,
        },
        state,
    )
    .await;
    let entries = history.data.unwrap()["events"].as_array().unwrap().clone();
    assert_eq!(entries.len(), 7);
    assert!(entries.iter().all(|entry| entry["effective"] == true));
}

#[tokio::test]
async fn guest_mode_rejects_skill_writes_but_allows_catalog_reads() {
    let (tmp, state) = common::setup_repo_alice().await;
    state
        .is_guest
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let source = write_package(tmp.path(), "release-check", "v1");
    let denied = handle_request(
        Request::SkillCreate {
            slug: "release-check".to_string(),
            source_directory: source.to_string_lossy().to_string(),
            display_name: "Release Check".to_string(),
            description: "Verify releases.".to_string(),
            event_id: None,
        },
        state.clone(),
    )
    .await;
    assert!(!denied.ok);

    let listed = handle_request(
        Request::SkillList {
            archived: false,
            limit: None,
            after: None,
        },
        state,
    )
    .await;
    assert!(listed.ok);
}

#[tokio::test]
async fn skill_write_waits_for_identity_read_lock() {
    let (tmp, state) = common::setup_repo_alice().await;
    common::run_git(tmp.path(), &["config", "user.name", "Test"]);
    common::run_git(tmp.path(), &["config", "user.email", "test@example.com"]);
    let source = write_package(tmp.path(), "release-check", "v1");
    let identity_guard = state.current_user.write().await;
    let request_state = state.clone();
    let source_directory = source.to_string_lossy().to_string();
    let request = tokio::spawn(async move {
        handle_request(
            Request::SkillCreate {
                slug: "release-check".to_string(),
                source_directory,
                display_name: "Release Check".to_string(),
                description: "Verify releases.".to_string(),
                event_id: None,
            },
            request_state,
        )
        .await
    });

    tokio::task::yield_now().await;
    drop(identity_guard);

    let response = request.await.unwrap();
    assert!(response.ok, "{:?}", response.error);
}

#[tokio::test]
async fn idempotent_skill_write_does_not_emit_a_second_change_event() {
    let (tmp, state) = common::setup_repo_alice().await;
    common::run_git(tmp.path(), &["config", "user.name", "Test"]);
    common::run_git(tmp.path(), &["config", "user.email", "test@example.com"]);
    let source = write_package(tmp.path(), "release-check", "v1");
    let event_id = EventId::new("e-01K1D8QG2S8RX4T9M9BDKQ9Z7N").unwrap();
    let mut events = state.event_tx.subscribe();

    for expected_idempotent in [false, true] {
        let response = handle_request(
            Request::SkillCreate {
                slug: "release-check".to_string(),
                source_directory: source.to_string_lossy().to_string(),
                display_name: "Release Check".to_string(),
                description: "Verify releases.".to_string(),
                event_id: Some(event_id.to_string()),
            },
            state.clone(),
        )
        .await;
        assert!(response.ok, "{:?}", response.error);
        assert_eq!(response.data.unwrap()["idempotent"], expected_idempotent);

        if expected_idempotent {
            assert!(
                tokio::time::timeout(Duration::from_millis(50), events.recv())
                    .await
                    .is_err()
            );
        } else {
            events.recv().await.unwrap();
        }
    }
}
