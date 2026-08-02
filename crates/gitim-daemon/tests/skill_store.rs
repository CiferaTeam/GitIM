#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use gitim_core::skill::{EventId, RevisionId, SkillReference, SkillSlug};
use gitim_core::types::Handler;
use gitim_daemon::skill_store::SkillStore;

fn actor(value: &str) -> Handler {
    Handler::new(value).expect("handler")
}

fn write_package(root: &Path, slug: &str, marker: &str, binary: bool) -> PathBuf {
    let source = root.join(format!("source-{marker}"));
    std::fs::create_dir_all(source.join("references")).unwrap();
    std::fs::create_dir_all(source.join("assets")).unwrap();
    std::fs::write(
        source.join("SKILL.md"),
        format!("---\nname: {slug}\ndescription: Shared release checks.\n---\n\n# {marker}\n"),
    )
    .unwrap();
    std::fs::write(
        source.join("references/checklist.md"),
        format!("check {marker}\n"),
    )
    .unwrap();
    if binary {
        std::fs::write(source.join("assets/blob.bin"), [0, 159, 146, 150]).unwrap();
    }
    source
}

fn git_output(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("git command");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf8")
}

#[tokio::test]
async fn create_commits_and_loads_an_immutable_revision() {
    let (tmp, state) = common::setup_repo_alice_bob().await;
    common::run_git(tmp.path(), &["config", "user.name", "Test"]);
    common::run_git(tmp.path(), &["config", "user.email", "test@example.com"]);
    let source = write_package(tmp.path(), "release-check", "v1", true);
    let store = SkillStore::new(state.as_ref());

    let created = store
        .create(
            &actor("alice"),
            &SkillSlug::new("release-check").unwrap(),
            &source,
            "Release Check",
            "Verify a release candidate.",
            None,
        )
        .expect("create");

    assert!(!created.idempotent);
    assert_eq!(
        created.state.current_revision,
        created.revision.clone().unwrap()
    );
    assert!(tmp
        .path()
        .join(format!(
            "skills/release-check/events/{}.meta.yaml",
            created.event_id
        ))
        .is_file());
    assert!(tmp
        .path()
        .join(format!(
            "skills/release-check/revisions/{}/package/SKILL.md",
            created.revision.as_ref().unwrap()
        ))
        .is_file());

    let loaded = store
        .load(&SkillReference::pinned(
            SkillSlug::new("release-check").unwrap(),
            created.revision.clone().unwrap(),
        ))
        .expect("load");
    assert!(loaded.skill_markdown.contains("# v1"));
    assert_eq!(loaded.resources.len(), 2);
    assert_eq!(
        loaded.canonical_ref.to_string(),
        created.canonical_ref.unwrap().to_string()
    );
    assert!(!loaded.archived);

    let catalog = store.catalog(false, 50, None).expect("catalog");
    assert_eq!(catalog.skills.len(), 1);
    assert!(catalog.invalid.is_empty());
    assert_eq!(catalog.skills[0].slug.as_str(), "release-check");
    assert_eq!(catalog.skills[0].open_proposal_count, 0);

    let log = git_output(tmp.path(), &["log", "-1", "--format=%s"]);
    assert_eq!(log.trim(), "skill: create release-check @alice");

    std::fs::remove_file(tmp.path().join(format!(
        "skills/release-check/revisions/{}/package/SKILL.md",
        created.revision.as_ref().unwrap()
    )))
    .unwrap();
    let corrupted = store.load(&SkillReference::pinned(
        SkillSlug::new("release-check").unwrap(),
        created.revision.unwrap(),
    ));
    assert_eq!(corrupted.unwrap_err().code(), "skill_revision_corrupted");
}

#[tokio::test]
async fn propose_is_unpublished_until_a_maintainer_publishes_it() {
    let (tmp, state) = common::setup_repo_alice_bob().await;
    common::run_git(tmp.path(), &["config", "user.name", "Test"]);
    common::run_git(tmp.path(), &["config", "user.email", "test@example.com"]);
    let store = SkillStore::new(state.as_ref());
    let initial_source = write_package(tmp.path(), "release-check", "v1", false);
    let created = store
        .create(
            &actor("alice"),
            &SkillSlug::new("release-check").unwrap(),
            &initial_source,
            "Release Check",
            "Verify releases.",
            None,
        )
        .unwrap();
    let proposal_source = write_package(tmp.path(), "release-check", "v2", false);
    let unknown_base = RevisionId::new("r-01K1D8QG2S8RX4T9M9BDKQ9Z7N").unwrap();
    let invalid_base = store.propose(
        &actor("bob"),
        &SkillSlug::new("release-check").unwrap(),
        &proposal_source,
        &unknown_base,
        "Add rollback verification.",
        None,
    );
    assert_eq!(invalid_base.unwrap_err().code(), "skill_revision_not_found");

    let proposed = store
        .propose(
            &actor("bob"),
            &SkillSlug::new("release-check").unwrap(),
            &proposal_source,
            created.revision.as_ref().unwrap(),
            "Add rollback verification.",
            None,
        )
        .expect("propose");

    let candidate_ref = SkillReference::pinned(
        SkillSlug::new("release-check").unwrap(),
        proposed.revision.clone().unwrap(),
    );
    assert_eq!(
        store.load(&candidate_ref).unwrap_err().code(),
        "skill_revision_unpublished"
    );
    let candidate = store
        .load_proposal(
            &SkillSlug::new("release-check").unwrap(),
            proposed.proposal.as_ref().unwrap(),
        )
        .expect("candidate");
    assert!(candidate.skill_markdown.contains("# v2"));

    let denied = store.publish(
        &actor("bob"),
        &SkillSlug::new("release-check").unwrap(),
        proposed.proposal.as_ref().unwrap(),
        None,
    );
    assert_eq!(denied.unwrap_err().code(), "skill_not_maintainer");

    let published = store
        .publish(
            &actor("alice"),
            &SkillSlug::new("release-check").unwrap(),
            proposed.proposal.as_ref().unwrap(),
            None,
        )
        .expect("publish");
    assert_eq!(
        published.state.current_revision,
        proposed.revision.clone().unwrap()
    );
    assert!(store
        .load(&candidate_ref)
        .expect("published load")
        .skill_markdown
        .contains("# v2"));
}

#[tokio::test]
async fn archived_skill_requires_a_pinned_reference() {
    let (tmp, state) = common::setup_repo_alice().await;
    common::run_git(tmp.path(), &["config", "user.name", "Test"]);
    common::run_git(tmp.path(), &["config", "user.email", "test@example.com"]);
    let store = SkillStore::new(state.as_ref());
    let source = write_package(tmp.path(), "release-check", "v1", false);
    let created = store
        .create(
            &actor("alice"),
            &SkillSlug::new("release-check").unwrap(),
            &source,
            "Release Check",
            "Verify releases.",
            None,
        )
        .unwrap();
    let unchanged_role = store.owner_add(
        &actor("alice"),
        &SkillSlug::new("release-check").unwrap(),
        actor("alice"),
        None,
    );
    assert_eq!(unchanged_role.unwrap_err().code(), "skill_event_conflict");
    store
        .archive(
            &actor("alice"),
            &SkillSlug::new("release-check").unwrap(),
            None,
        )
        .expect("archive");

    let unpinned = SkillReference {
        slug: SkillSlug::new("release-check").unwrap(),
        revision: None,
    };
    assert_eq!(store.load(&unpinned).unwrap_err().code(), "skill_archived");

    let pinned = SkillReference::pinned(
        SkillSlug::new("release-check").unwrap(),
        created.revision.unwrap(),
    );
    assert!(store.load(&pinned).expect("pinned load").archived);
    assert!(store.catalog(false, 50, None).unwrap().skills.is_empty());
    assert_eq!(store.catalog(true, 50, None).unwrap().skills.len(), 1);
}

#[tokio::test]
async fn corrupt_skill_is_reported_without_blocking_catalog() {
    let (tmp, state) = common::setup_repo_alice().await;
    common::run_git(tmp.path(), &["config", "user.name", "Test"]);
    common::run_git(tmp.path(), &["config", "user.email", "test@example.com"]);
    let store = SkillStore::new(state.as_ref());
    for slug in ["healthy", "broken"] {
        let source = write_package(tmp.path(), slug, slug, false);
        store
            .create(
                &actor("alice"),
                &SkillSlug::new(slug).unwrap(),
                &source,
                slug,
                "Shared checks.",
                None,
            )
            .unwrap();
    }
    let event = std::fs::read_dir(tmp.path().join("skills/broken/events"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(event, "not: [valid").unwrap();

    let catalog = store.catalog(false, 50, None).expect("catalog");
    assert_eq!(catalog.skills.len(), 1);
    assert_eq!(catalog.skills[0].slug.as_str(), "healthy");
    assert_eq!(catalog.invalid.len(), 1);
    assert_eq!(catalog.invalid[0].slug, "broken");
    assert_eq!(catalog.invalid[0].error_code, "skill_invalid_history");
}

#[tokio::test]
async fn caller_supplied_event_id_is_idempotent() {
    let (tmp, state) = common::setup_repo_alice().await;
    common::run_git(tmp.path(), &["config", "user.name", "Test"]);
    common::run_git(tmp.path(), &["config", "user.email", "test@example.com"]);
    let store = SkillStore::new(state.as_ref());
    let source = write_package(tmp.path(), "release-check", "v1", false);
    let event_id = EventId::new("e-01K1D8QG2S8RX4T9M9BDKQ9Z7N").unwrap();
    let first = store
        .create(
            &actor("alice"),
            &SkillSlug::new("release-check").unwrap(),
            &source,
            "Release Check",
            "Verify releases.",
            Some(event_id.clone()),
        )
        .unwrap();
    let commits_before = git_output(tmp.path(), &["rev-list", "--count", "HEAD"]);
    let replay = store
        .create(
            &actor("alice"),
            &SkillSlug::new("release-check").unwrap(),
            &source,
            "Release Check",
            "Verify releases.",
            Some(event_id),
        )
        .expect("replay");

    assert!(replay.idempotent);
    assert_eq!(replay.revision, first.revision);
    assert_eq!(
        git_output(tmp.path(), &["rev-list", "--count", "HEAD"]),
        commits_before
    );

    let conflict = store.create(
        &actor("alice"),
        &SkillSlug::new("release-check").unwrap(),
        &source,
        "Different Name",
        "Verify releases.",
        Some(replay.event_id),
    );
    assert_eq!(conflict.unwrap_err().code(), "skill_event_conflict");
}

#[tokio::test]
async fn failed_commit_rolls_back_files_and_index() {
    let (tmp, state) = common::setup_repo_alice().await;
    common::run_git(tmp.path(), &["config", "user.name", "Test"]);
    common::run_git(tmp.path(), &["config", "user.email", "test@example.com"]);
    let hook = tmp.path().join(".git/hooks/pre-commit");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let source = write_package(tmp.path(), "release-check", "v1", false);
    let store = SkillStore::new(state.as_ref());

    let result = store.create(
        &actor("alice"),
        &SkillSlug::new("release-check").unwrap(),
        &source,
        "Release Check",
        "Verify releases.",
        None,
    );
    assert_eq!(result.unwrap_err().code(), "skill_commit_failed");
    assert!(!tmp.path().join("skills/release-check").exists());
    assert!(git_output(
        tmp.path(),
        &["status", "--porcelain", "--", "skills/release-check"]
    )
    .is_empty());
}
