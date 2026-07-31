#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;

use gitim_core::skill::{
    validate_package_entries, PackageEntry, ProposalId, RequestId, RevisionId, SkillListQuery,
    SkillLoadResponse, SkillMutationRequest, SkillMutationResult, SkillOperation,
    SkillProposeRequest, SkillReference, SkillResourceQuery, SkillSlug,
};
use gitim_daemon::api::Request;
use gitim_daemon::handlers::handle_request;
use gitim_daemon::skill_import::snapshot_skill_directory;
use gitim_daemon::skill_store::SkillStore;
use gitim_daemon::state::AppState;
use gitim_sync::skill::checkpoint::SkillConflict;
use tempfile::TempDir;

fn git<I, S>(root: &Path, args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git failed in {}: {}",
        root.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn configure(root: &Path) {
    git(root, ["config", "user.name", "Alice"]);
    git(root, ["config", "user.email", "alice@example.com"]);
    git(root, ["config", "commit.gpgsign", "false"]);
}

struct Fixture {
    _root: TempDir,
    _remote: PathBuf,
    repo: PathBuf,
    state: Arc<AppState>,
}

impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let remote = root.path().join("origin.git");
        git(
            root.path(),
            ["init", "--bare", "-b", "main", remote.to_str().unwrap()],
        );
        let seed = root.path().join("seed");
        fs::create_dir(&seed).unwrap();
        git(&seed, ["init", "-b", "main"]);
        configure(&seed);
        fs::create_dir_all(seed.join("users")).unwrap();
        fs::write(
            seed.join("users/alice.meta.yaml"),
            "display_name: Alice\nrole: human\nintroduction: Owner\n",
        )
        .unwrap();
        fs::write(seed.join(".gitignore"), ".gitim/\n").unwrap();
        git(&seed, ["add", "."]);
        git(&seed, ["commit", "-m", "initialize workspace"]);
        git(
            &seed,
            [
                OsStr::new("remote"),
                OsStr::new("add"),
                OsStr::new("origin"),
                remote.as_os_str(),
            ],
        );
        git(&seed, ["push", "-u", "origin", "main"]);

        let repo = root.path().join("repo");
        git(
            root.path(),
            [OsStr::new("clone"), remote.as_os_str(), repo.as_os_str()],
        );
        configure(&repo);
        let (events, _) = tokio::sync::broadcast::channel(32);
        let state = Arc::new(AppState::new(
            repo.clone(),
            gitim_core::types::Config::default(),
            events,
            Some("alice".to_owned()),
        ));
        *state.users.write().await = vec!["alice".to_owned()];
        Self {
            _root: root,
            _remote: remote,
            repo,
            state,
        }
    }

    async fn request(&self, value: serde_json::Value) -> gitim_daemon::api::Response {
        let request: Request = serde_json::from_value(value).unwrap();
        handle_request(request, Arc::clone(&self.state)).await
    }

    async fn bootstrap(&self) {
        let response = self
            .request(serde_json::json!({
                "method": "skill_workspace_bootstrap",
                "request": { "request_id": RequestId::generate() }
            }))
            .await;
        assert!(response.ok, "{:?}", response.error);
    }

    fn package_dir(&self, slug: &str, body: &str) -> PathBuf {
        let directory = self
            ._root
            .path()
            .join(format!("source-{slug}-{}", RequestId::generate().as_str()));
        fs::create_dir_all(directory.join("references")).unwrap();
        fs::write(
            directory.join("SKILL.md"),
            format!("---\nname: {slug}\ndescription: Test package\n---\n\n{body}\n"),
        )
        .unwrap();
        fs::write(directory.join("references/note.md"), "resource\n").unwrap();
        fs::write(
            directory.join("references/pixel.bin"),
            [0_u8, 159, 146, 150],
        )
        .unwrap();
        fs::write(directory.join("references/raw.txt"), [0_u8, 159, 146, 150]).unwrap();
        fs::write(
            directory.join("references/utf8.unknown-extension"),
            "plain utf-8\n",
        )
        .unwrap();
        directory
    }

    async fn create(&self, slug: &str) -> SkillMutationResult {
        let source = self.package_dir(slug, "initial");
        let response = self
            .request(serde_json::json!({
                "method": "skill_create",
                "request": {
                    "request_id": RequestId::generate(),
                    "slug": slug,
                    "display_name": format!("{slug} display"),
                    "description": "Lifecycle fixture",
                    "source_directory": source,
                }
            }))
            .await;
        assert!(response.ok, "{:?}", response.error);
        assert_eq!(
            response.data.as_ref().unwrap()["local_state"],
            "pending_sync",
            "remote publication should be readable before local worktree integration"
        );
        serde_json::from_value(response.data.unwrap()["result"].clone()).unwrap()
    }

    fn mark_skill_archived_in_accepted_view(&self, slug: &str) {
        git(&self.repo, ["fetch", "origin"]);
        git(&self.repo, ["reset", "--hard", "origin/main"]);
        fs::create_dir_all(self.repo.join("archive/skills")).unwrap();
        git(
            &self.repo,
            [
                "mv",
                &format!("skills/{slug}"),
                &format!("archive/skills/{slug}"),
            ],
        );
        git(&self.repo, ["commit", "-m", "archive accepted fixture"]);
        let commit = String::from_utf8(git(&self.repo, ["rev-parse", "HEAD"]).stdout)
            .unwrap()
            .trim()
            .to_owned();
        let tree = gitim_sync::skill::git_tree::tree_oid_at(
            &gitim_sync::git::GitStorage::new(&self.repo),
            &commit,
            &format!("archive/skills/{slug}"),
        )
        .unwrap()
        .unwrap();
        let store = gitim_sync::skill::checkpoint::SkillCheckpointStore::new(&self.repo).unwrap();
        let mut checkpoint = store.load().unwrap().unwrap();
        checkpoint.last_scanned_tip = commit.clone();
        let accepted = checkpoint.skills.get_mut(slug).unwrap();
        accepted.archived = true;
        accepted.tree.commit_oid = commit;
        accepted.tree.tree_oid = tree;
        store.save(&checkpoint).unwrap();
        self.state.skill_store.invalidate();
    }
}

#[test]
fn import_copies_exact_regular_files_into_a_private_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let request = root.path().join("request");
    fs::create_dir_all(source.join("references")).unwrap();
    let markdown = b"---\nname: release-check\ndescription: Release checks\n---\n\nBody\n";
    fs::write(source.join("SKILL.md"), markdown).unwrap();
    fs::write(source.join("references/raw.bin"), [0_u8, 255, 1]).unwrap();

    let package = snapshot_skill_directory(&source, &request).unwrap();

    assert_eq!(package.skill_markdown, markdown);
    assert_eq!(fs::read(request.join("SKILL.md")).unwrap(), markdown);
    assert_eq!(
        fs::read(request.join("references/raw.bin")).unwrap(),
        [0_u8, 255, 1]
    );
}

#[cfg(unix)]
#[test]
fn import_rejects_directory_and_file_symlinks() {
    use std::os::unix::fs::symlink;

    for symlink_directory in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(
            source.join("SKILL.md"),
            "---\nname: safe\ndescription: Safe\n---\n",
        )
        .unwrap();
        let outside = root.path().join("outside");
        if symlink_directory {
            fs::create_dir(&outside).unwrap();
            fs::write(outside.join("secret.txt"), "secret").unwrap();
            symlink(&outside, source.join("references")).unwrap();
        } else {
            fs::write(&outside, "secret").unwrap();
            symlink(&outside, source.join("secret.txt")).unwrap();
        }

        let error = snapshot_skill_directory(&source, &root.path().join("request")).unwrap_err();
        assert_eq!(error.code(), "skill_invalid_package");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handlers_bootstrap_create_list_load_resource_and_reject_candidates() {
    let fixture = Fixture::new().await;
    fixture.bootstrap().await;
    let created = fixture.create("release-check").await;
    let current = created.current_revision.clone().unwrap();

    let store = SkillStore::new(&fixture.repo);
    let first = store
        .list(SkillListQuery {
            archived: false,
            limit: 1,
            cursor: None,
        })
        .unwrap();
    assert_eq!(first.skills.len(), 1);
    assert_eq!(first.skills[0].slug.as_str(), "release-check");
    assert!(first.next_cursor.is_none());

    let loaded: SkillLoadResponse = store
        .load(&SkillReference {
            slug: SkillSlug::new("release-check").unwrap(),
            revision: None,
        })
        .unwrap();
    assert_eq!(loaded.canonical_ref.revision.as_ref(), Some(&current));
    assert_eq!(loaded.resources.len(), 4);
    let note = loaded
        .resources
        .iter()
        .find(|resource| resource.path == "references/note.md")
        .unwrap();
    assert_eq!(note.byte_size, b"resource\n".len() as u64);
    assert!(note.text);
    let pixel = loaded
        .resources
        .iter()
        .find(|resource| resource.path == "references/pixel.bin")
        .unwrap();
    assert_eq!(pixel.byte_size, 4);
    assert!(!pixel.text);
    let raw = loaded
        .resources
        .iter()
        .find(|resource| resource.path == "references/raw.txt")
        .unwrap();
    let unknown = loaded
        .resources
        .iter()
        .find(|resource| resource.path == "references/utf8.unknown-extension")
        .unwrap();

    let binary = store
        .resource(SkillResourceQuery {
            reference: loaded.canonical_ref.clone(),
            path: "references/pixel.bin".to_owned(),
        })
        .unwrap();
    assert!(!binary.text);
    assert_eq!(binary.media_type, "application/octet-stream");
    assert_eq!(binary.bytes, [0_u8, 159, 146, 150]);

    let raw_resource = store
        .resource(SkillResourceQuery {
            reference: loaded.canonical_ref.clone(),
            path: raw.path.clone(),
        })
        .unwrap();
    let unknown_resource = store
        .resource(SkillResourceQuery {
            reference: loaded.canonical_ref,
            path: unknown.path.clone(),
        })
        .unwrap();
    assert_eq!(raw.text, raw_resource.text);
    assert!(!raw.text);
    assert_eq!(unknown.text, unknown_resource.text);
    assert!(unknown.text);

    let source = fixture.package_dir("release-check", "candidate");
    let proposal_request = RequestId::generate();
    let proposal = fixture
        .request(serde_json::json!({
            "method": "skill_propose",
            "request": {
                "request_id": proposal_request,
                "slug": "release-check",
                "base_revision": current,
                "summary": "Candidate",
                "source_directory": source,
            }
        }))
        .await;
    assert!(proposal.ok, "{:?}", proposal.error);
    let candidate = RevisionId::new(&format!("r-{}", &proposal_request.as_str()[2..])).unwrap();
    let unpublished = store
        .load(&SkillReference {
            slug: SkillSlug::new("release-check").unwrap(),
            revision: Some(candidate),
        })
        .unwrap_err();
    assert_eq!(unpublished.code(), "skill_revision_unpublished");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_catalog_paginates_and_pinned_archived_revision_loads_exactly() {
    let fixture = Fixture::new().await;
    fixture.bootstrap().await;
    let release = fixture.create("release-check").await;
    fixture.create("deploy-check").await;
    let store = &fixture.state.skill_store;

    let first = store
        .list(SkillListQuery {
            archived: false,
            limit: 1,
            cursor: None,
        })
        .unwrap();
    assert_eq!(first.skills[0].slug.as_str(), "deploy-check");
    let second = store
        .list(SkillListQuery {
            archived: false,
            limit: 1,
            cursor: first.next_cursor,
        })
        .unwrap();
    assert_eq!(second.skills[0].slug.as_str(), "release-check");
    assert!(second.next_cursor.is_none());

    fixture.mark_skill_archived_in_accepted_view("release-check");
    let unpinned = store
        .load(&SkillReference {
            slug: SkillSlug::new("release-check").unwrap(),
            revision: None,
        })
        .unwrap_err();
    assert_eq!(unpinned.code(), "skill_archived");
    let pinned = store
        .load(&SkillReference {
            slug: SkillSlug::new("release-check").unwrap(),
            revision: release.current_revision,
        })
        .unwrap();
    assert!(pinned.archived);
    assert!(pinned.skill_markdown.contains("initial"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_tip_does_not_stale_an_accepted_catalog_cursor() {
    let fixture = Fixture::new().await;
    fixture.bootstrap().await;
    fixture.create("release-check").await;
    fixture.create("deploy-check").await;
    let store = &fixture.state.skill_store;

    let first = store
        .list(SkillListQuery {
            archived: false,
            limit: 1,
            cursor: None,
        })
        .unwrap();
    let cursor = first.next_cursor.unwrap();
    let checkpoint_store =
        gitim_sync::skill::checkpoint::SkillCheckpointStore::new(&fixture.repo).unwrap();
    let mut checkpoint = checkpoint_store.load().unwrap().unwrap();
    git(&fixture.repo, ["fetch", "origin"]);
    git(&fixture.repo, ["reset", "--hard", "origin/main"]);
    fs::write(fixture.repo.join("rejected-marker"), "rejected\n").unwrap();
    git(&fixture.repo, ["add", "rejected-marker"]);
    git(&fixture.repo, ["commit", "-m", "rejected candidate"]);
    checkpoint.last_scanned_tip =
        String::from_utf8(git(&fixture.repo, ["rev-parse", "HEAD"]).stdout)
            .unwrap()
            .trim()
            .to_owned();
    checkpoint.conflicts.insert(
        "$workspace".to_owned(),
        SkillConflict {
            rejected_commit: checkpoint.last_scanned_tip.clone(),
            code: "skill_invalid_workspace_meta".to_owned(),
            accepted_tree_oid: checkpoint
                .workspace_tree
                .as_ref()
                .map(|tree| tree.tree_oid.clone()),
            rejected_receipt_paths: BTreeSet::new(),
        },
    );
    checkpoint_store.save(&checkpoint).unwrap();
    store.invalidate();

    let second = store
        .list(SkillListQuery {
            archived: false,
            limit: 1,
            cursor: Some(cursor),
        })
        .unwrap();
    assert_eq!(second.skills[0].slug.as_str(), "release-check");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_reads_use_immutable_snapshots_when_head_skill_tree_matches() {
    let fixture = Fixture::new().await;
    fixture.bootstrap().await;
    let created = fixture.create("release-check").await;
    let revision = created.current_revision.unwrap();
    let checkpoint_store =
        gitim_sync::skill::checkpoint::SkillCheckpointStore::new(&fixture.repo).unwrap();
    let checkpoint = checkpoint_store.load().unwrap().unwrap();
    let accepted = checkpoint.skills.get("release-check").unwrap();
    git(
        &fixture.repo,
        ["reset", "--hard", &accepted.tree.commit_oid],
    );

    let markdown = fixture.repo.join(format!(
        "skills/release-check/revisions/{}/package/SKILL.md",
        revision.as_str()
    ));
    fs::write(
        &markdown,
        "---\nname: release-check\ndescription: Worktree\n---\n\nworktree-equal\n",
    )
    .unwrap();
    fixture.state.skill_store.invalidate();
    let equal = fixture
        .state
        .skill_store
        .load(&SkillReference {
            slug: SkillSlug::new("release-check").unwrap(),
            revision: None,
        })
        .unwrap();
    assert!(equal.skill_markdown.contains("initial"));
    assert!(!equal.skill_markdown.contains("worktree-equal"));

    git(&fixture.repo, ["add", "."]);
    git(&fixture.repo, ["commit", "-m", "diverge skill tree"]);
    fixture.state.skill_store.invalidate();
    let diverged = fixture
        .state
        .skill_store
        .load(&SkillReference {
            slug: SkillSlug::new("release-check").unwrap(),
            revision: None,
        })
        .unwrap();
    assert!(diverged.skill_markdown.contains("initial"));
    assert!(!diverged.skill_markdown.contains("worktree-equal"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proposal_publish_reject_and_withdraw_emit_ordered_events() {
    let fixture = Fixture::new().await;
    fixture.bootstrap().await;
    let created = fixture.create("release-check").await;
    let base = created.current_revision.unwrap();
    let mut events = fixture.state.event_tx.subscribe();

    for (index, operation) in [
        SkillOperation::ProposalReject,
        SkillOperation::ProposalWithdraw,
        SkillOperation::ProposalPublish,
    ]
    .into_iter()
    .enumerate()
    {
        let request_id = RequestId::generate();
        let proposal_id = ProposalId::new(&format!("p-{}", &request_id.as_str()[2..])).unwrap();
        let response = fixture
            .request(serde_json::json!({
                "method": "skill_propose",
                "request": {
                    "request_id": request_id,
                    "slug": "release-check",
                    "base_revision": base,
                    "summary": format!("Candidate {index}"),
                    "source_directory": fixture.package_dir("release-check", &format!("candidate {index}")),
                }
            }))
            .await;
        assert!(response.ok, "{:?}", response.error);
        let event = events.recv().await.unwrap();
        assert!(matches!(
            event,
            gitim_daemon::api::Event::SkillChanged { ref kind, .. }
                if kind == "proposal_created"
        ));

        let response = fixture
            .request(serde_json::json!({
                "method": "skill_proposal_transition",
                "request": {
                    "request_id": RequestId::generate(),
                    "proposal_id": proposal_id,
                    "operation": operation,
                    "expected_state_revision": 1,
                    "expected_control_revision":
                        (operation == SkillOperation::ProposalPublish).then_some(1),
                }
            }))
            .await;
        assert!(response.ok, "{:?}", response.error);
        let event = events.recv().await.unwrap();
        let expected = match operation {
            SkillOperation::ProposalPublish => "proposal_published",
            SkillOperation::ProposalReject => "proposal_rejected",
            SkillOperation::ProposalWithdraw => "proposal_withdrawn",
            _ => unreachable!(),
        };
        assert!(matches!(
            event,
            gitim_daemon::api::Event::SkillChanged {
                ref kind,
                proposal_id: Some(ref id),
                ..
            } if kind == expected && id == proposal_id.as_str()
        ));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_remote_change_emits_one_deduplicated_invalidation() {
    let fixture = Fixture::new().await;
    fixture.bootstrap().await;
    let created = fixture.create("release-check").await;
    fixture.state.prime_skill_event_revisions();
    let mut events = fixture.state.event_tx.subscribe();
    let slug = SkillSlug::new("release-check").unwrap();
    let package = validate_package_entries(
        &slug,
        vec![PackageEntry::new(
            "SKILL.md",
            b"---\nname: release-check\ndescription: Remote\n---\n\nremote\n".to_vec(),
        )],
    )
    .unwrap();
    let repo = gitim_sync::git::GitStorage::new(&fixture.repo);
    let guard = gitim_sync::skill::guard::SkillSyncGuard::new(&fixture.repo).unwrap();
    gitim_sync::skill::transaction::execute_remote_skill_transaction(
        &repo,
        &guard,
        gitim_sync::skill::transaction::RemoteSkillTransactionRequest {
            request: SkillMutationRequest::Propose(SkillProposeRequest {
                request_id: RequestId::generate(),
                slug,
                base_revision: created.current_revision.unwrap(),
                summary: "remote proposal".to_owned(),
                source_directory: fixture.repo.join("unused"),
            }),
            actor: "alice".to_owned(),
            author_email: "alice@example.com".to_owned(),
            now: "2026-07-31T12:00:00Z".to_owned(),
            package: Some(package),
            active_users: BTreeSet::from(["alice".to_owned()]),
        },
    )
    .unwrap();

    fixture.state.refresh_synced_skill_events();
    let event = events.recv().await.unwrap();
    assert!(matches!(
        event,
        gitim_daemon::api::Event::SkillChanged {
            ref slug,
            ref kind,
            event_revision: 2,
            ..
        } if slug == "release-check" && kind == "synced"
    ));
    fixture.state.refresh_synced_skill_events();
    assert!(matches!(
        events.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
}

#[test]
fn empty_store_and_resource_path_bounds_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    git(root.path(), ["init", "-b", "main"]);
    configure(root.path());
    fs::write(root.path().join("README.md"), "empty\n").unwrap();
    git(root.path(), ["add", "."]);
    git(root.path(), ["commit", "-m", "empty"]);
    let store = SkillStore::new(root.path());
    let response = store
        .list(SkillListQuery {
            archived: true,
            limit: 20,
            cursor: None,
        })
        .unwrap();
    assert!(response.skills.is_empty());

    let error = store
        .resource(SkillResourceQuery {
            reference: SkillReference {
                slug: SkillSlug::new("missing").unwrap(),
                revision: None,
            },
            path: "../secret".to_owned(),
        })
        .unwrap_err();
    assert_eq!(error.code(), "skill_invalid_package");
}

#[test]
fn lifecycle_request_types_retain_nested_typed_payloads() {
    let request_id = RequestId::generate();
    let request: Request = serde_json::from_value(serde_json::json!({
        "method": "skill_workspace_bootstrap",
        "request": { "request_id": request_id }
    }))
    .unwrap();
    assert!(matches!(request, Request::SkillWorkspaceBootstrap { .. }));

    let transition: Request = serde_json::from_value(serde_json::json!({
        "method": "skill_proposal_transition",
        "request": {
            "request_id": RequestId::generate(),
            "proposal_id": ProposalId::generate(),
            "operation": "proposal_reject",
            "expected_state_revision": 1
        }
    }))
    .unwrap();
    assert!(matches!(
        transition,
        Request::SkillProposalTransition { .. }
    ));
}
