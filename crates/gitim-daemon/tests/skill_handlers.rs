#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;

use gitim_core::skill::{
    validate_package_entries, PackageEntry, ProposalId, ProposalStatus, RequestId, RevisionId,
    SkillListQuery, SkillLoadResponse, SkillMutationRequest, SkillMutationResult, SkillOperation,
    SkillPageQuery, SkillProposalDiff, SkillProposalListQuery, SkillProposalResourceQuery,
    SkillProposalShowQuery, SkillProposeRequest, SkillReference, SkillResourceQuery, SkillSlug,
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
        self.state
            .is_admin
            .store(true, std::sync::atomic::Ordering::SeqCst);
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
        self.create_from(slug, &source).await
    }

    async fn create_from(&self, slug: &str, source: &Path) -> SkillMutationResult {
        let request_id = RequestId::generate();
        let response = self
            .request(serde_json::json!({
                "method": "skill_create",
                "request": {
                    "request_id": request_id,
                    "slug": slug,
                    "display_name": format!("{slug} display"),
                    "description": "Lifecycle fixture",
                    "source_directory": source,
                }
            }))
            .await;
        assert!(response.ok, "{:?}", response.error);
        assert_eq!(
            response.data.as_ref().unwrap()["request_id"],
            request_id.as_str()
        );
        assert_eq!(response.data.as_ref().unwrap()["operation"], "skill_create");
        assert!(response.data.as_ref().unwrap().get("target").is_none());
        assert_eq!(
            response.data.as_ref().unwrap()["local_state"],
            "pending_sync",
            "remote publication should be readable before local worktree integration"
        );
        serde_json::from_value(response.data.unwrap()["result"].clone()).unwrap()
    }

    async fn proposal_diff(
        &self,
        slug: &str,
        base_revision: &RevisionId,
        source: &Path,
    ) -> SkillProposalDiff {
        let request_id = RequestId::generate();
        let response = self
            .request(serde_json::json!({
                "method": "skill_propose",
                "request": {
                    "request_id": request_id,
                    "slug": slug,
                    "base_revision": base_revision,
                    "summary": "Compare package paths",
                    "source_directory": source,
                }
            }))
            .await;
        assert!(response.ok, "{:?}", response.error);
        let proposal_id = ProposalId::new(&format!("p-{}", &request_id.as_str()[2..])).unwrap();
        self.state
            .skill_store
            .proposal_show(SkillProposalShowQuery {
                proposal_id,
                diff: true,
            })
            .unwrap()
            .diff
            .unwrap()
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
async fn workspace_bootstrap_rejects_non_admin_daemon_identity() {
    let fixture = Fixture::new().await;
    let response = fixture
        .request(serde_json::json!({
            "method": "skill_workspace_bootstrap",
            "request": { "request_id": RequestId::generate() }
        }))
        .await;

    assert!(!response.ok);
    assert_eq!(response.error_code.as_deref(), Some("skill_admin_required"));
    assert!(!fixture.repo.join("skills/workspace.meta.yaml").exists());
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
    let proposal_id = ProposalId::new(&format!("p-{}", &proposal_request.as_str()[2..])).unwrap();
    assert_eq!(
        proposal.data.as_ref().unwrap()["proposal_id"],
        proposal_id.as_str()
    );
    assert_eq!(
        proposal.data.as_ref().unwrap()["operation"],
        "proposal_create"
    );
    let candidate = RevisionId::new(&format!("r-{}", &proposal_request.as_str()[2..])).unwrap();
    let unpublished = store
        .load(&SkillReference {
            slug: SkillSlug::new("release-check").unwrap(),
            revision: Some(candidate),
        })
        .unwrap_err();
    assert_eq!(unpublished.code(), "skill_revision_unpublished");

    let revisions = store
        .revisions(SkillPageQuery {
            slug: SkillSlug::new("release-check").unwrap(),
            limit: 50,
            cursor: None,
        })
        .unwrap();
    assert_eq!(revisions.revisions.len(), 1);
    assert_eq!(revisions.revisions[0].id, current);

    let history = store
        .history(SkillPageQuery {
            slug: SkillSlug::new("release-check").unwrap(),
            limit: 100,
            cursor: None,
        })
        .unwrap();
    assert!(history.entries.len() >= 2);

    let proposals = store
        .proposal_list(SkillProposalListQuery {
            slug: SkillSlug::new("release-check").unwrap(),
            status: Some(ProposalStatus::Open),
            limit: 50,
            cursor: None,
        })
        .unwrap();
    assert_eq!(proposals.proposals.len(), 1);
    assert_eq!(proposals.proposals[0].id, proposal_id);

    let shown = store
        .proposal_show(SkillProposalShowQuery {
            proposal_id: proposal_id.clone(),
            diff: true,
        })
        .unwrap();
    assert_eq!(shown.proposal.id, proposal_id);
    assert!(shown.diff.unwrap().text.contains("candidate"));

    let proposal_resource = store
        .proposal_resource(SkillProposalResourceQuery {
            proposal_id,
            path: "references/pixel.bin".to_owned(),
        })
        .unwrap();
    assert!(!proposal_resource.text);
    assert_eq!(proposal_resource.bytes, [0_u8, 159, 146, 150]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proposal_diff_compares_every_package_path_by_content() {
    let fixture = Fixture::new().await;
    fixture.bootstrap().await;
    let base_source = fixture.package_dir("release-check", "initial");
    fs::write(
        base_source.join("references/note.md"),
        "zero\none\ntwo\nthree\nold\nfive\nsix\nseven\neight\n",
    )
    .unwrap();
    let created = fixture.create_from("release-check", &base_source).await;
    let base_revision = created.current_revision.unwrap();

    let source = fixture.package_dir("release-check", "candidate");
    fs::write(
        source.join("references/note.md"),
        "zero\none\ntwo\nthree\nnew\nfive\nsix\nseven\neight\n",
    )
    .unwrap();
    fs::remove_file(source.join("references/raw.txt")).unwrap();
    fs::write(source.join("references/pixel.bin"), [1_u8, 2, 3, 4]).unwrap();
    fs::write(source.join("references/added.bin"), [9_u8, 8, 7]).unwrap();

    let diff = fixture
        .proposal_diff("release-check", &base_revision, &source)
        .await;
    let diff = serde_json::to_value(diff).unwrap();
    let text = diff["text"].as_str().unwrap();
    assert!(text.contains("--- a/SKILL.md"), "{text}");
    assert!(text.contains("+++ b/SKILL.md"), "{text}");
    assert!(text.contains("--- a/references/note.md"), "{text}");
    assert!(text.contains("+++ b/references/note.md"), "{text}");
    assert!(text.contains("-old"), "{text}");
    assert!(text.contains("+new"), "{text}");
    assert!(text.contains(" one\n two\n three\n"), "{text}");
    assert!(text.contains(" five\n six\n seven\n"), "{text}");
    assert!(
        !text.contains(" zero\n"),
        "fourth leading context line leaked: {text}"
    );
    assert!(
        !text.contains(" eight\n"),
        "fourth trailing context line leaked: {text}"
    );
    assert!(
        !text.contains("pixel.bin"),
        "binary bytes must not be rendered"
    );

    let changes = diff["changed_resources"].as_array().unwrap();
    let find = |path: &str| {
        changes
            .iter()
            .find(|change| change["path"] == path)
            .unwrap_or_else(|| panic!("missing change record for {path}: {changes:?}"))
    };
    let added = find("references/added.bin");
    assert_eq!(added["change_kind"], "added");
    assert!(added["before_byte_size"].is_null());
    assert_eq!(added["after_byte_size"], 3);
    assert!(added["before_sha256"].is_null());
    assert_eq!(added["after_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(added["media_type"], "application/octet-stream");

    let removed = find("references/raw.txt");
    assert_eq!(removed["change_kind"], "removed");
    assert_eq!(removed["before_byte_size"], 4);
    assert!(removed["after_byte_size"].is_null());
    assert_eq!(removed["before_sha256"].as_str().unwrap().len(), 64);
    assert!(removed["after_sha256"].is_null());
    assert_eq!(removed["media_type"], "text/plain");

    let binary = find("references/pixel.bin");
    assert_eq!(binary["change_kind"], "modified");
    assert_eq!(binary["before_byte_size"], 4);
    assert_eq!(binary["after_byte_size"], 4);
    assert_ne!(binary["before_sha256"], binary["after_sha256"]);
    assert_eq!(binary["media_type"], "application/octet-stream");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proposal_diff_describes_large_text_without_rendering_its_bytes() {
    let fixture = Fixture::new().await;
    fixture.bootstrap().await;
    let base_source = fixture.package_dir("release-check", "same index");
    fs::write(
        base_source.join("references/large.txt"),
        vec![b'a'; 1024 * 1024 + 1],
    )
    .unwrap();
    let created = fixture.create_from("release-check", &base_source).await;
    let base_revision = created.current_revision.unwrap();

    let candidate_source = fixture.package_dir("release-check", "same index");
    fs::write(
        candidate_source.join("references/large.txt"),
        vec![b'b'; 1024 * 1024 + 1],
    )
    .unwrap();
    let diff = fixture
        .proposal_diff("release-check", &base_revision, &candidate_source)
        .await;
    assert!(!diff.text.contains("large.txt"));
    let value = serde_json::to_value(diff).unwrap();
    let change = value["changed_resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|change| change["path"] == "references/large.txt")
        .unwrap();
    assert_eq!(change["change_kind"], "modified");
    assert_eq!(change["before_byte_size"], 1024 * 1024 + 1);
    assert_eq!(change["after_byte_size"], 1024 * 1024 + 1);
    assert_ne!(change["before_sha256"], change["after_sha256"]);
    assert_eq!(change["media_type"], "text/plain");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proposal_diff_truncates_a_utf8_unified_diff_deterministically() {
    let fixture = Fixture::new().await;
    fixture.bootstrap().await;
    let base_source = fixture.package_dir("release-check", "same index");
    let candidate_source = fixture.package_dir("release-check", "same index");
    let mut before = String::new();
    let mut after = String::new();
    for index in 0..4_500 {
        before.push_str(&format!("anchor-{index}\nold-{index}-界界界界界\n"));
        after.push_str(&format!("anchor-{index}\nnew-{index}-界界界界界\n"));
    }
    fs::write(base_source.join("references/diff.txt"), before).unwrap();
    fs::write(candidate_source.join("references/diff.txt"), after).unwrap();
    let created = fixture.create_from("release-check", &base_source).await;
    let base_revision = created.current_revision.unwrap();

    let first = fixture
        .proposal_diff("release-check", &base_revision, &candidate_source)
        .await;
    assert!(first.truncated);
    assert!(first.text.len() <= 256 * 1024);
    assert!(first.text.contains("--- a/references/diff.txt"));
    assert!(first.text.contains("界"));

    let proposal_id = fixture
        .state
        .skill_store
        .proposal_list(SkillProposalListQuery {
            slug: SkillSlug::new("release-check").unwrap(),
            status: Some(ProposalStatus::Open),
            limit: 1,
            cursor: None,
        })
        .unwrap()
        .proposals[0]
        .id
        .clone();
    let second = fixture
        .state
        .skill_store
        .proposal_show(SkillProposalShowQuery {
            proposal_id,
            diff: true,
        })
        .unwrap()
        .diff
        .unwrap();
    assert_eq!(second.text, first.text);
    assert_eq!(second.truncated, first.truncated);
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
        assert_eq!(
            response.data.as_ref().unwrap()["proposal_id"],
            proposal_id.as_str()
        );
        assert_eq!(
            response.data.as_ref().unwrap()["operation"],
            "proposal_create"
        );
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
        assert_eq!(
            response.data.as_ref().unwrap()["proposal_id"],
            proposal_id.as_str()
        );
        assert_eq!(
            response.data.as_ref().unwrap()["operation"],
            serde_json::to_value(operation).unwrap()
        );
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
async fn metadata_role_and_archive_requests_run_shared_transitions() {
    let fixture = Fixture::new().await;
    fs::write(
        fixture.repo.join("users/bob.meta.yaml"),
        "display_name: Bob\nrole: agent\nintroduction: Collaborator\n",
    )
    .unwrap();
    git(&fixture.repo, ["add", "users/bob.meta.yaml"]);
    git(&fixture.repo, ["commit", "-m", "add collaborator"]);
    git(&fixture.repo, ["push", "origin", "main"]);
    *fixture.state.users.write().await = vec!["alice".to_owned(), "bob".to_owned()];
    fixture.bootstrap().await;
    let created = fixture.create("release-check").await;

    let metadata = fixture
        .request(serde_json::json!({
            "method":"skill_metadata_update",
            "request":{
                "request_id":RequestId::generate(),
                "slug":"release-check",
                "display_name":"Release Gate",
                "expected_control_revision":created.control_revision.unwrap()
            }
        }))
        .await;
    assert!(metadata.ok, "{:?}", metadata.error);
    assert_eq!(
        metadata.data.as_ref().unwrap()["result"]["control_revision"],
        2
    );
    assert_eq!(
        metadata.data.as_ref().unwrap()["operation"],
        "metadata_update"
    );
    assert!(metadata.data.as_ref().unwrap().get("target").is_none());

    let role = fixture
        .request(serde_json::json!({
            "method":"skill_role_update",
            "request":{
                "request_id":RequestId::generate(),
                "slug":"release-check",
                "operation":"owner_add",
                "target":"bob",
                "expected_control_revision":2
            }
        }))
        .await;
    assert!(role.ok, "{:?}", role.error);
    assert_eq!(role.data.as_ref().unwrap()["result"]["control_revision"], 3);
    assert_eq!(role.data.as_ref().unwrap()["operation"], "owner_add");
    assert_eq!(role.data.as_ref().unwrap()["target"], "bob");

    let archive = fixture
        .request(serde_json::json!({
            "method":"skill_archive_transition",
            "request":{
                "request_id":RequestId::generate(),
                "slug":"release-check",
                "operation":"archive",
                "expected_control_revision":3
            }
        }))
        .await;
    assert!(archive.ok, "{:?}", archive.error);
    assert_eq!(
        archive.data.as_ref().unwrap()["result"]["control_revision"],
        4
    );
    assert_eq!(archive.data.as_ref().unwrap()["operation"], "archive");
    assert!(archive.data.as_ref().unwrap().get("target").is_none());
    git(&fixture.repo, ["fetch", "origin"]);
    git(
        &fixture.repo,
        [
            "show",
            "origin/main:archive/skills/release-check/skill.meta.yaml",
        ],
    );
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
