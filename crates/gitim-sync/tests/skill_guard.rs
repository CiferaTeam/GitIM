#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gitim_core::epoch::EpochFile;
use gitim_core::skill::{
    plan_skill_mutation, validate_package_entries, PackageEntry, RequestId,
    SkillConflictCheckpoint, SkillCreateRequest, SkillMutationContext, SkillMutationPlan,
    SkillMutationRequest, SkillProposeRequest, SkillReceipt, SkillRepairAcceptedState,
    SkillRepairRequest, SkillRepairScope, SkillRepositorySnapshot, SkillWorkspaceBootstrapRequest,
};
use gitim_sync::git::GitStorage;
use gitim_sync::rotate::{
    try_fire_rotation as try_fire_rotation_impl, RotationError, RotationOutcome,
};
use gitim_sync::skill::checkpoint::{
    validate_incoming_skill_history, SkillCheckpointStore, SkillConflict, SkillValidationCheckpoint,
};
use gitim_sync::skill::git_tree::{
    build_private_index_commit, tree_oid_at, PrivateIndexCommitRequest,
};
use gitim_sync::skill::guard::{GuardedPushOutcome, IntegrationOperation, SkillSyncGuard};
use gitim_sync::sync_loop::{run_sync_cycle, AuthCircuit};
use tempfile::TempDir;

const ALICE: &str = "alice";
const NOW: &str = "2026-07-30T10:00:00Z";

fn try_fire_rotation(
    storage: &GitStorage,
    current_branch: &str,
    threshold: u64,
    archive_dir: &Path,
    author: (&str, &str),
    created_at: &str,
) -> Result<RotationOutcome, RotationError> {
    try_fire_rotation_impl(
        storage,
        &Mutex::new(()),
        current_branch,
        threshold,
        archive_dir,
        author,
        created_at,
    )
}

struct Repository {
    directory: TempDir,
    storage: GitStorage,
}

impl Repository {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        git(directory.path(), &["init", "-b", "main"]);
        git(directory.path(), &["config", "user.name", ALICE]);
        git(
            directory.path(),
            &["config", "user.email", "alice@example.com"],
        );
        fs::create_dir_all(directory.path().join("users")).unwrap();
        fs::write(
            directory.path().join("users/alice.meta.yaml"),
            "display_name: Alice\nrole: human\nintroduction: Owner\n",
        )
        .unwrap();
        commit_all(directory.path(), "seed", ALICE);
        let storage = GitStorage::new(directory.path());
        Self { directory, storage }
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }

    fn tip(&self) -> String {
        git_output(self.root(), &["rev-parse", "HEAD"])
    }

    fn snapshot(&self) -> SkillValidationCheckpoint {
        validate_incoming_skill_history(
            &self.storage,
            &SkillValidationCheckpoint::empty("main"),
            &self.tip(),
        )
        .unwrap()
        .checkpoint
    }

    fn commit_plan(&self, plan: &SkillMutationPlan) -> String {
        apply_plan(self.root(), plan);
        commit_all(self.root(), &plan.commit_message, ALICE);
        self.tip()
    }
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn commit_all(root: &Path, message: &str, author: &str) {
    git(root, &["add", "-A"]);
    commit_index(root, message, author);
}

fn commit_paths(root: &Path, message: &str, author: &str, paths: &[&str]) {
    let mut add = vec!["add", "--"];
    add.extend_from_slice(paths);
    git(root, &add);
    commit_index(root, message, author);
}

fn commit_index(root: &Path, message: &str, author: &str) {
    let author = format!("{author} <{author}@example.com>");
    git(root, &["commit", "--author", &author, "-m", message]);
}

fn apply_plan(root: &Path, plan: &SkillMutationPlan) {
    for edit in &plan.edits {
        match edit {
            gitim_core::skill::SkillTreeEdit::Upsert { path, bytes } => {
                let target = root.join(path);
                fs::create_dir_all(target.parent().unwrap()).unwrap();
                fs::write(target, bytes).unwrap();
            }
            gitim_core::skill::SkillTreeEdit::Delete { path } => {
                let target = root.join(path);
                if target.is_dir() {
                    fs::remove_dir_all(target).unwrap();
                } else if target.exists() {
                    fs::remove_file(target).unwrap();
                }
            }
        }
    }
}

fn commit_plan_with_private_index(repository: &Repository, plan: &SkillMutationPlan) -> String {
    let transaction = tempfile::tempdir().unwrap();
    let built = build_private_index_commit(
        &repository.storage,
        &PrivateIndexCommitRequest {
            base_commit: repository.tip(),
            private_index: transaction.path().join("index"),
            edits: plan.edits.clone(),
            message: plan.commit_message.lines().next().unwrap().to_owned(),
            author_name: ALICE.to_owned(),
            author_email: "alice@example.com".to_owned(),
            request_id: plan.receipt.id.clone(),
        },
    )
    .unwrap();
    git(repository.root(), &["reset", "--hard", &built.commit_oid]);
    assert_eq!(repository.tip(), built.commit_oid);
    built.commit_oid
}

fn skill_path_matches(path: &str, slug: &str) -> bool {
    let active = format!("skills/{slug}");
    let archived = format!("archive/skills/{slug}");
    path == active
        || path.starts_with(&format!("{active}/"))
        || path == archived
        || path.starts_with(&format!("{archived}/"))
}

fn skill_scope_files(snapshot: &SkillRepositorySnapshot, slug: &str) -> BTreeMap<String, Vec<u8>> {
    snapshot
        .repository_files
        .iter()
        .filter(|(path, _)| skill_path_matches(path, slug))
        .map(|(path, bytes)| (path.clone(), bytes.clone()))
        .collect()
}

fn plan_repair_for_skill(
    accepted: &SkillRepositorySnapshot,
    conflict: &SkillConflict,
    slug: &str,
    actual_scope_files: BTreeMap<String, Vec<u8>>,
    entry_changed_paths: BTreeSet<String>,
    rejected_receipts: &[SkillReceipt],
    suffix: char,
) -> SkillMutationPlan {
    let slug = gitim_core::skill::SkillSlug::new(slug).unwrap();
    let accepted_files = skill_scope_files(accepted, slug.as_str());
    let accepted_state = accepted
        .active_skills
        .get(&slug)
        .cloned()
        .map(|skill| SkillRepairAcceptedState::ActiveSkill {
            slug: slug.clone(),
            skill,
        })
        .or_else(|| {
            accepted.archived_skills.get(&slug).cloned().map(|skill| {
                SkillRepairAcceptedState::ArchivedSkill {
                    slug: slug.clone(),
                    skill,
                }
            })
        })
        .unwrap_or_else(|| SkillRepairAcceptedState::AbsentSkill { slug: slug.clone() });
    let mut before = accepted.clone();
    before
        .repository_files
        .retain(|path, _| !skill_path_matches(path, slug.as_str()));
    before.repository_files.extend(actual_scope_files);
    for receipt in rejected_receipts {
        let path = format!("skills/receipts/{}.meta.yaml", receipt.id.as_str());
        if conflict.rejected_receipt_paths.contains(&path) {
            before
                .repository_files
                .insert(path, serde_yaml::to_string(receipt).unwrap().into_bytes());
            before.receipts.insert(receipt.id.clone(), receipt.clone());
        }
    }
    let rejected_receipt_paths = conflict.rejected_receipt_paths.clone();
    let mut changed_paths: BTreeSet<_> = before
        .repository_files
        .keys()
        .chain(accepted_files.keys())
        .filter(|path| skill_path_matches(path, slug.as_str()))
        .filter(|path| before.repository_files.get(*path) != accepted_files.get(*path))
        .cloned()
        .collect();
    changed_paths.extend(entry_changed_paths.iter().cloned());
    changed_paths.extend(rejected_receipt_paths.iter().cloned());
    let accepted_tree = conflict.accepted_tree_oid.clone().unwrap();
    before.conflict_checkpoint = Some(SkillConflictCheckpoint {
        conflict_tip: conflict.rejected_commit.clone(),
        accepted_tree: accepted_tree.clone(),
        accepted_state,
        accepted_files,
        entry_changed_paths,
        rejected_receipt_paths,
        changed_paths,
    });
    plan_skill_mutation(
        &before,
        &context(None),
        &SkillMutationRequest::Repair(SkillRepairRequest {
            request_id: request_id(suffix),
            scope: SkillRepairScope::Skill(slug),
            conflict_tip: conflict.rejected_commit.clone(),
            accepted_tree,
        }),
    )
    .unwrap()
}

fn hash_blob(root: &Path, bytes: &[u8]) -> String {
    let path = root.join(".git/gitim-test-object");
    fs::write(&path, bytes).unwrap();
    git_output(root, &["hash-object", "-w", "--", path.to_str().unwrap()])
}

fn replace_index_scope_root(root: &Path, path: &str, mode: &str, oid: &str) {
    git(
        root,
        &["rm", "-r", "--cached", "--ignore-unmatch", "--", path],
    );
    let cache_info = format!("{mode},{oid},{path}");
    git(root, &["update-index", "--add", "--cacheinfo", &cache_info]);
}

fn request_id(suffix: char) -> RequestId {
    RequestId::new(&format!("q-01K1D8QG2S8RX4T9M9BDKQ9Z7{suffix}")).unwrap()
}

fn context(package: Option<gitim_core::skill::ValidatedPackage>) -> SkillMutationContext {
    SkillMutationContext {
        actor: ALICE.to_owned(),
        now: NOW.to_owned(),
        package,
    }
}

fn package_for(slug: &str) -> gitim_core::skill::ValidatedPackage {
    let slug = gitim_core::skill::SkillSlug::new(slug).unwrap();
    validate_package_entries(
        &slug,
        vec![
            PackageEntry::new(
                "SKILL.md",
                format!(
                    "---\nname: {}\ndescription: Verify releases.\n---\n\nRun it.\n",
                    slug.as_str()
                )
                .into_bytes(),
            ),
            PackageEntry::new("scripts/check.sh", b"#!/bin/sh\nexit 0\n".to_vec()),
        ],
    )
    .unwrap()
}

fn bootstrap_plan(
    before: &gitim_core::skill::SkillRepositorySnapshot,
    suffix: char,
) -> SkillMutationPlan {
    plan_skill_mutation(
        before,
        &context(None),
        &SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
            request_id: request_id(suffix),
        }),
    )
    .unwrap()
}

fn create_plan(
    before: &gitim_core::skill::SkillRepositorySnapshot,
    suffix: char,
) -> SkillMutationPlan {
    create_plan_for(before, suffix, "release-check")
}

fn create_plan_for(
    before: &gitim_core::skill::SkillRepositorySnapshot,
    suffix: char,
    slug: &str,
) -> SkillMutationPlan {
    plan_skill_mutation(
        before,
        &context(Some(package_for(slug))),
        &SkillMutationRequest::Create(SkillCreateRequest {
            request_id: request_id(suffix),
            slug: gitim_core::skill::SkillSlug::new(slug).unwrap(),
            display_name: slug.to_owned(),
            description: "Verify releases.".to_owned(),
            source_directory: "/unused".into(),
        }),
    )
    .unwrap()
}

#[test]
fn validates_each_real_git_transition_and_uses_commit_tree_bytes() {
    let repository = Repository::new();
    let initial = repository.snapshot();
    assert_eq!(initial.active_epoch, "main");
    let bootstrap = bootstrap_plan(
        &gitim_core::skill::SkillRepositorySnapshot {
            active_users: BTreeSet::from([ALICE.to_owned()]),
            ..Default::default()
        },
        'A',
    );
    let bootstrap_tip = repository.commit_plan(&bootstrap);
    let accepted_bootstrap =
        validate_incoming_skill_history(&repository.storage, &initial, &bootstrap_tip).unwrap();
    let create = create_plan(&bootstrap.after, 'B');
    let create_tip = repository.commit_plan(&create);

    fs::write(
        repository
            .root()
            .join("skills/release-check/skill.meta.yaml"),
        "this worktree byte must not be trusted\n",
    )
    .unwrap();
    let accepted = validate_incoming_skill_history(
        &repository.storage,
        &accepted_bootstrap.checkpoint,
        &create_tip,
    )
    .unwrap();

    assert_eq!(accepted.accepted_changes.len(), 1);
    assert_eq!(accepted.accepted_changes[0].slug, "release-check");
    assert_eq!(accepted.checkpoint.last_scanned_tip, create_tip);
    assert!(accepted.checkpoint.conflicts.is_empty());
    assert!(accepted.checkpoint.skills.contains_key("release-check"));
}

#[test]
fn missing_receipt_never_replaces_the_last_accepted_skill_view() {
    let repository = Repository::new();
    let initial = repository.snapshot();
    let bootstrap = bootstrap_plan(
        &gitim_core::skill::SkillRepositorySnapshot {
            active_users: BTreeSet::from([ALICE.to_owned()]),
            ..Default::default()
        },
        'C',
    );
    let bootstrap_tip = repository.commit_plan(&bootstrap);
    let accepted_bootstrap =
        validate_incoming_skill_history(&repository.storage, &initial, &bootstrap_tip).unwrap();
    let create = create_plan(&bootstrap.after, 'D');
    for edit in create.edits.iter().filter(|edit| match edit {
        gitim_core::skill::SkillTreeEdit::Upsert { path, .. }
        | gitim_core::skill::SkillTreeEdit::Delete { path } => !path.contains("/receipts/"),
    }) {
        match edit {
            gitim_core::skill::SkillTreeEdit::Upsert { path, bytes } => {
                let target = repository.root().join(path);
                fs::create_dir_all(target.parent().unwrap()).unwrap();
                fs::write(target, bytes).unwrap();
            }
            gitim_core::skill::SkillTreeEdit::Delete { .. } => unreachable!(),
        }
    }
    commit_all(repository.root(), "invalid create", ALICE);
    let rejected_tip = repository.tip();

    let rejected = validate_incoming_skill_history(
        &repository.storage,
        &accepted_bootstrap.checkpoint,
        &rejected_tip,
    )
    .unwrap();

    assert_eq!(rejected.checkpoint.last_scanned_tip, rejected_tip);
    assert!(!rejected.checkpoint.skills.contains_key("release-check"));
    let conflict = &rejected.checkpoint.conflicts["release-check"];
    assert_eq!(conflict.rejected_commit, rejected_tip);
    assert!(conflict
        .accepted_tree_oid
        .as_ref()
        .is_some_and(|oid| !oid.is_empty()));
    assert!(conflict.rejected_receipt_paths.is_empty());
    assert!(rejected.accepted_changes.is_empty());
}

#[test]
fn mismatched_root_receipt_path_is_rejected() {
    let repository = Repository::new();
    let initial = repository.snapshot();
    let bootstrap = bootstrap_plan(
        &gitim_core::skill::SkillRepositorySnapshot {
            active_users: BTreeSet::from([ALICE.to_owned()]),
            ..Default::default()
        },
        'V',
    );
    let bootstrap_tip = repository.commit_plan(&bootstrap);
    let accepted = validate_incoming_skill_history(&repository.storage, &initial, &bootstrap_tip)
        .unwrap()
        .checkpoint;
    let create = create_plan(&bootstrap.after, 'W');
    apply_plan(repository.root(), &create);
    let original = repository.root().join(format!(
        "skills/receipts/{}.meta.yaml",
        create.receipt.id.as_str()
    ));
    let mismatch = repository
        .root()
        .join("skills/receipts/q-01K1D8QG2S8RX4T9M9BDKQ9Z7X.meta.yaml");
    fs::rename(original, mismatch).unwrap();
    commit_all(repository.root(), &create.commit_message, ALICE);

    let rejected =
        validate_incoming_skill_history(&repository.storage, &accepted, &repository.tip()).unwrap();

    assert!(!rejected.checkpoint.skills.contains_key("release-check"));
    assert_eq!(
        rejected.checkpoint.conflicts["release-check"].rejected_commit,
        repository.tip()
    );
    assert!(rejected.checkpoint.conflicts["release-check"]
        .rejected_receipt_paths
        .is_empty());
}

#[test]
fn malformed_root_receipt_creates_no_cleanup_authority() {
    let repository = Repository::new();
    let initial = repository.snapshot();
    let bootstrap = bootstrap_plan(
        &gitim_core::skill::SkillRepositorySnapshot {
            active_users: BTreeSet::from([ALICE.to_owned()]),
            ..Default::default()
        },
        'X',
    );
    let bootstrap_tip = repository.commit_plan(&bootstrap);
    let accepted = validate_incoming_skill_history(&repository.storage, &initial, &bootstrap_tip)
        .unwrap()
        .checkpoint;
    let create = create_plan(&bootstrap.after, 'Y');
    apply_plan(repository.root(), &create);
    fs::write(
        repository.root().join(format!(
            "skills/receipts/{}.meta.yaml",
            create.receipt.id.as_str()
        )),
        "not: [valid\n",
    )
    .unwrap();
    commit_all(repository.root(), &create.commit_message, ALICE);

    let rejected =
        validate_incoming_skill_history(&repository.storage, &accepted, &repository.tip()).unwrap();

    assert!(rejected.checkpoint.conflicts["release-check"]
        .rejected_receipt_paths
        .is_empty());
}

#[test]
fn commit_author_must_match_the_receipt_actor() {
    let repository = Repository::new();
    let initial = repository.snapshot();
    let bootstrap = bootstrap_plan(
        &gitim_core::skill::SkillRepositorySnapshot {
            active_users: BTreeSet::from([ALICE.to_owned()]),
            ..Default::default()
        },
        'Y',
    );
    let bootstrap_tip = repository.commit_plan(&bootstrap);
    let accepted = validate_incoming_skill_history(&repository.storage, &initial, &bootstrap_tip)
        .unwrap()
        .checkpoint;
    let create = create_plan(&bootstrap.after, 'Z');
    apply_plan(repository.root(), &create);
    commit_all(repository.root(), &create.commit_message, "bob");

    let rejected =
        validate_incoming_skill_history(&repository.storage, &accepted, &repository.tip()).unwrap();

    assert_eq!(
        rejected.checkpoint.conflicts["release-check"].code,
        "skill_sync_conflict"
    );
}

#[test]
fn non_fast_forward_rewrite_does_not_reset_checkpoint_trust() {
    let repository = Repository::new();
    let initial = repository.snapshot();
    let trusted_tip = initial.last_scanned_tip.clone();
    fs::write(repository.root().join("ordinary.txt"), "first\n").unwrap();
    commit_all(repository.root(), "ordinary", ALICE);
    let advanced =
        validate_incoming_skill_history(&repository.storage, &initial, &repository.tip())
            .unwrap()
            .checkpoint;

    git(repository.root(), &["reset", "--hard", &trusted_tip]);
    fs::write(repository.root().join("rewritten.txt"), "rewrite\n").unwrap();
    commit_all(repository.root(), "rewrite", ALICE);

    let error = validate_incoming_skill_history(&repository.storage, &advanced, &repository.tip())
        .unwrap_err();
    assert_eq!(error.code(), "skill_sync_conflict");
    assert!(advanced.last_scanned_tip != repository.tip());
}

#[test]
fn checkpoint_store_round_trips_atomically_and_rejects_corruption() {
    let repository = Repository::new();
    let checkpoint = repository.snapshot();
    let store = SkillCheckpointStore::new(repository.root()).unwrap();

    store.save(&checkpoint).unwrap();
    assert_eq!(store.load().unwrap(), Some(checkpoint.clone()));
    fs::write(&store.path, b"{\"schema_version\":999}").unwrap();
    assert!(store.load().is_err());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        store.save(&checkpoint).unwrap();
        assert_eq!(
            fs::metadata(&store.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn invalid_checkpoint_save_preserves_the_last_good_file() {
    let repository = Repository::new();
    let checkpoint = repository.snapshot();
    let store = SkillCheckpointStore::new(repository.root()).unwrap();
    store.save(&checkpoint).unwrap();
    let good_bytes = fs::read(&store.path).unwrap();
    let mut invalid = checkpoint;
    invalid.schema_version = 999;

    assert!(store.save(&invalid).is_err());
    assert_eq!(fs::read(&store.path).unwrap(), good_bytes);
}

#[test]
fn oversized_checkpoint_save_preserves_the_last_good_file() {
    let repository = Repository::new();
    let checkpoint = repository.snapshot();
    let store = SkillCheckpointStore::new(repository.root()).unwrap();
    store.save(&checkpoint).unwrap();
    let good_bytes = fs::read(&store.path).unwrap();
    let mut oversized = checkpoint;
    let repository_tree = git_output(repository.root(), &["rev-parse", "HEAD^{tree}"]);
    oversized.workspace_tree = Some(gitim_sync::skill::checkpoint::AcceptedTree {
        commit_oid: oversized.last_scanned_tip.clone(),
        tree_oid: repository_tree.clone(),
    });
    oversized.conflicts.insert(
        "$workspace".to_owned(),
        gitim_sync::skill::checkpoint::SkillConflict {
            rejected_commit: oversized.last_scanned_tip.clone(),
            code: "x".repeat(4 * 1024 * 1024),
            accepted_tree_oid: Some(repository_tree),
            rejected_receipt_paths: BTreeSet::new(),
        },
    );

    assert!(store.save(&oversized).is_err());
    assert_eq!(fs::read(&store.path).unwrap(), good_bytes);
}

#[test]
fn checkpoint_rejects_conflict_that_names_a_different_accepted_tree() {
    let repository = Repository::new();
    let mut checkpoint = repository.snapshot();
    checkpoint.workspace_tree = Some(gitim_sync::skill::checkpoint::AcceptedTree {
        commit_oid: checkpoint.last_scanned_tip.clone(),
        tree_oid: git_output(repository.root(), &["rev-parse", "HEAD^{tree}"]),
    });
    checkpoint.conflicts.insert(
        "$workspace".to_owned(),
        gitim_sync::skill::checkpoint::SkillConflict {
            rejected_commit: checkpoint.last_scanned_tip.clone(),
            code: "skill_sync_conflict".to_owned(),
            accepted_tree_oid: Some("0000000000000000000000000000000000000000".to_owned()),
            rejected_receipt_paths: BTreeSet::new(),
        },
    );
    let store = SkillCheckpointStore::new(repository.root()).unwrap();

    assert!(store.save(&checkpoint).is_err());
}

#[test]
fn checkpoint_rejects_a_noncanonical_absent_skill_tree_oid() {
    let repository = Repository::new();
    let mut checkpoint = repository.snapshot();
    checkpoint.conflicts.insert(
        "release-check".to_owned(),
        gitim_sync::skill::checkpoint::SkillConflict {
            rejected_commit: checkpoint.last_scanned_tip.clone(),
            code: "skill_sync_conflict".to_owned(),
            accepted_tree_oid: Some("0000000000000000000000000000000000000000".to_owned()),
            rejected_receipt_paths: BTreeSet::new(),
        },
    );
    let store = SkillCheckpointStore::new(repository.root()).unwrap();

    assert!(store.save(&checkpoint).is_err());
}

#[test]
fn active_and_archived_skill_prefixes_are_slash_delimited() {
    let repository = Repository::new();
    let initial = repository.snapshot();
    let bootstrap = bootstrap_plan(
        &gitim_core::skill::SkillRepositorySnapshot {
            active_users: BTreeSet::from([ALICE.to_owned()]),
            ..Default::default()
        },
        '1',
    );
    let bootstrap_tip = repository.commit_plan(&bootstrap);
    let accepted_bootstrap =
        validate_incoming_skill_history(&repository.storage, &initial, &bootstrap_tip)
            .unwrap()
            .checkpoint;
    let release = create_plan_for(&bootstrap.after, '2', "release");
    let release_tip = repository.commit_plan(&release);
    let accepted_release =
        validate_incoming_skill_history(&repository.storage, &accepted_bootstrap, &release_tip)
            .unwrap()
            .checkpoint;
    let release_check = create_plan_for(&release.after, '3', "release-check");
    let release_check_tip = repository.commit_plan(&release_check);

    let accepted =
        validate_incoming_skill_history(&repository.storage, &accepted_release, &release_check_tip)
            .unwrap();

    assert!(accepted.checkpoint.conflicts.is_empty());
    assert!(accepted.checkpoint.skills.contains_key("release"));
    assert!(accepted.checkpoint.skills.contains_key("release-check"));
}

#[test]
fn illegal_extra_path_in_a_skill_commit_is_rejected() {
    let repository = Repository::new();
    let initial = repository.snapshot();
    let bootstrap = bootstrap_plan(
        &gitim_core::skill::SkillRepositorySnapshot {
            active_users: BTreeSet::from([ALICE.to_owned()]),
            ..Default::default()
        },
        'E',
    );
    let bootstrap_tip = repository.commit_plan(&bootstrap);
    let accepted = validate_incoming_skill_history(&repository.storage, &initial, &bootstrap_tip)
        .unwrap()
        .checkpoint;
    let create = create_plan(&bootstrap.after, 'F');
    apply_plan(repository.root(), &create);
    fs::write(repository.root().join("ordinary.txt"), "smuggled\n").unwrap();
    commit_all(repository.root(), &create.commit_message, ALICE);

    let rejected =
        validate_incoming_skill_history(&repository.storage, &accepted, &repository.tip()).unwrap();

    assert!(!rejected.checkpoint.skills.contains_key("release-check"));
    assert_eq!(
        rejected.checkpoint.conflicts["release-check"].code,
        "skill_sync_conflict"
    );
}

#[test]
fn merge_commit_that_touches_skill_paths_is_rejected() {
    let repository = Repository::new();
    let initial = repository.snapshot();
    let bootstrap = bootstrap_plan(
        &gitim_core::skill::SkillRepositorySnapshot {
            active_users: BTreeSet::from([ALICE.to_owned()]),
            ..Default::default()
        },
        'G',
    );
    let bootstrap_tip = repository.commit_plan(&bootstrap);
    let accepted = validate_incoming_skill_history(&repository.storage, &initial, &bootstrap_tip)
        .unwrap()
        .checkpoint;
    let create = create_plan(&bootstrap.after, 'H');
    git(repository.root(), &["checkout", "-b", "skill-side"]);
    repository.commit_plan(&create);
    git(repository.root(), &["checkout", "main"]);
    fs::write(repository.root().join("ordinary.txt"), "main\n").unwrap();
    commit_all(repository.root(), "main work", ALICE);
    let merge_message = format!(
        "merge skill side\n\nGitim-Request-Id: {}",
        create.receipt.id.as_str()
    );
    git(
        repository.root(),
        &["merge", "--no-ff", "skill-side", "-m", &merge_message],
    );

    let rejected =
        validate_incoming_skill_history(&repository.storage, &accepted, &repository.tip()).unwrap();

    assert_eq!(
        rejected.checkpoint.conflicts["release-check"].rejected_commit,
        repository.tip()
    );
    assert!(!rejected.checkpoint.skills.contains_key("release-check"));
}

#[test]
fn corrupted_revision_hash_is_never_accepted() {
    let repository = Repository::new();
    let initial = repository.snapshot();
    let bootstrap = bootstrap_plan(
        &gitim_core::skill::SkillRepositorySnapshot {
            active_users: BTreeSet::from([ALICE.to_owned()]),
            ..Default::default()
        },
        'Q',
    );
    let bootstrap_tip = repository.commit_plan(&bootstrap);
    let accepted = validate_incoming_skill_history(&repository.storage, &initial, &bootstrap_tip)
        .unwrap()
        .checkpoint;
    let create = create_plan(&bootstrap.after, 'R');
    apply_plan(repository.root(), &create);
    let revision = create.receipt.request.revision.as_ref().unwrap();
    let path = repository.root().join(format!(
        "skills/release-check/revisions/{}/revision.meta.yaml",
        revision.as_str()
    ));
    let mut yaml = fs::read_to_string(&path).unwrap();
    let digest = create.receipt.request.payload_sha256.as_str();
    yaml = yaml.replace(digest, &"0".repeat(64));
    fs::write(path, yaml).unwrap();
    commit_all(repository.root(), &create.commit_message, ALICE);

    let rejected =
        validate_incoming_skill_history(&repository.storage, &accepted, &repository.tip()).unwrap();

    assert!(!rejected.checkpoint.skills.contains_key("release-check"));
    assert!(rejected.accepted_changes.is_empty());
}

#[test]
fn authorized_repair_clears_only_the_validated_skill_conflict() {
    let repository = Repository::new();
    let initial = repository.snapshot();
    let bootstrap = bootstrap_plan(
        &gitim_core::skill::SkillRepositorySnapshot {
            active_users: BTreeSet::from([ALICE.to_owned()]),
            ..Default::default()
        },
        'K',
    );
    let bootstrap_tip = repository.commit_plan(&bootstrap);
    let accepted_bootstrap =
        validate_incoming_skill_history(&repository.storage, &initial, &bootstrap_tip)
            .unwrap()
            .checkpoint;
    let create = create_plan(&bootstrap.after, 'T');
    let create_tip = repository.commit_plan(&create);
    let accepted_create =
        validate_incoming_skill_history(&repository.storage, &accepted_bootstrap, &create_tip)
            .unwrap()
            .checkpoint;
    let meta_path = "skills/release-check/skill.meta.yaml";
    let mut corrupt_meta = fs::read_to_string(repository.root().join(meta_path)).unwrap();
    corrupt_meta = corrupt_meta.replace("Verify releases.", "Rejected bytes.");
    fs::write(repository.root().join(meta_path), &corrupt_meta).unwrap();
    commit_all(repository.root(), "bypass skill state", ALICE);
    let corrupt_tip = repository.tip();
    let conflicted =
        validate_incoming_skill_history(&repository.storage, &accepted_create, &corrupt_tip)
            .unwrap()
            .checkpoint;
    let accepted_tree = conflicted.skills["release-check"].tree.tree_oid.clone();

    let mut repair_before = create.after.clone();
    repair_before
        .repository_files
        .insert(meta_path.to_owned(), corrupt_meta.into_bytes());
    let accepted_files: BTreeMap<_, _> = create
        .after
        .repository_files
        .iter()
        .filter(|(path, _)| path.starts_with("skills/release-check/"))
        .map(|(path, bytes)| (path.clone(), bytes.clone()))
        .collect();
    repair_before.conflict_checkpoint = Some(SkillConflictCheckpoint {
        conflict_tip: corrupt_tip.clone(),
        accepted_tree: accepted_tree.clone(),
        accepted_state: SkillRepairAcceptedState::ActiveSkill {
            slug: gitim_core::skill::SkillSlug::new("release-check").unwrap(),
            skill: create.after.active_skills
                [&gitim_core::skill::SkillSlug::new("release-check").unwrap()]
                .clone(),
        },
        accepted_files,
        entry_changed_paths: BTreeSet::new(),
        rejected_receipt_paths: BTreeSet::new(),
        changed_paths: BTreeSet::from([meta_path.to_owned()]),
    });
    let repair = plan_skill_mutation(
        &repair_before,
        &context(None),
        &SkillMutationRequest::Repair(SkillRepairRequest {
            request_id: request_id('M'),
            scope: SkillRepairScope::Skill(
                gitim_core::skill::SkillSlug::new("release-check").unwrap(),
            ),
            conflict_tip: corrupt_tip,
            accepted_tree,
        }),
    )
    .unwrap();
    let repair_tip = repository.commit_plan(&repair);

    let repaired =
        validate_incoming_skill_history(&repository.storage, &conflicted, &repair_tip).unwrap();

    assert!(repaired.checkpoint.conflicts.is_empty());
    assert_eq!(
        repaired.checkpoint.skills["release-check"].tree.commit_oid,
        repair_tip
    );
    assert_eq!(repaired.accepted_changes.len(), 1);
}

#[test]
fn receipt_only_repair_does_not_clear_a_mode_only_skill_conflict() {
    let repository = Repository::new();
    let initial = repository.snapshot();
    let bootstrap = bootstrap_plan(
        &gitim_core::skill::SkillRepositorySnapshot {
            active_users: BTreeSet::from([ALICE.to_owned()]),
            ..Default::default()
        },
        'D',
    );
    let bootstrap_tip = repository.commit_plan(&bootstrap);
    let accepted_bootstrap =
        validate_incoming_skill_history(&repository.storage, &initial, &bootstrap_tip)
            .unwrap()
            .checkpoint;
    let create = create_plan(&bootstrap.after, 'E');
    let create_tip = repository.commit_plan(&create);
    let accepted_create =
        validate_incoming_skill_history(&repository.storage, &accepted_bootstrap, &create_tip)
            .unwrap()
            .checkpoint;
    let script_path = format!(
        "skills/release-check/revisions/{}/package/scripts/check.sh",
        create.receipt.request.revision.as_ref().unwrap().as_str()
    );
    git(
        repository.root(),
        &["update-index", "--chmod=+x", &script_path],
    );
    git(repository.root(), &["commit", "-m", "mode-only bypass"]);
    let rejected_tip = repository.tip();
    let conflicted =
        validate_incoming_skill_history(&repository.storage, &accepted_create, &rejected_tip)
            .unwrap()
            .checkpoint;
    let accepted_tree = conflicted.skills["release-check"].tree.tree_oid.clone();
    let slug = gitim_core::skill::SkillSlug::new("release-check").unwrap();
    let accepted_files: BTreeMap<_, _> = create
        .after
        .repository_files
        .iter()
        .filter(|(path, _)| path.starts_with("skills/release-check/"))
        .map(|(path, bytes)| (path.clone(), bytes.clone()))
        .collect();
    let mut repair_before = create.after.clone();
    repair_before.conflict_checkpoint = Some(SkillConflictCheckpoint {
        conflict_tip: rejected_tip.clone(),
        accepted_tree: accepted_tree.clone(),
        accepted_state: SkillRepairAcceptedState::ActiveSkill {
            slug: slug.clone(),
            skill: create.after.active_skills[&slug].clone(),
        },
        accepted_files,
        entry_changed_paths: BTreeSet::from([script_path.clone()]),
        rejected_receipt_paths: BTreeSet::new(),
        changed_paths: BTreeSet::from([script_path.clone()]),
    });
    let repair = plan_skill_mutation(
        &repair_before,
        &context(None),
        &SkillMutationRequest::Repair(SkillRepairRequest {
            request_id: request_id('F'),
            scope: SkillRepairScope::Skill(slug),
            conflict_tip: rejected_tip,
            accepted_tree,
        }),
    )
    .unwrap();
    assert!(repair.changed_paths.contains(&script_path));
    for edit in &repair.edits {
        if matches!(
            edit,
            gitim_core::skill::SkillTreeEdit::Upsert { path, .. }
                if path.starts_with("skills/receipts/")
        ) {
            match edit {
                gitim_core::skill::SkillTreeEdit::Upsert { path, bytes } => {
                    fs::write(repository.root().join(path), bytes).unwrap();
                }
                gitim_core::skill::SkillTreeEdit::Delete { .. } => unreachable!(),
            }
        }
    }
    let repair_receipt_path = format!("skills/receipts/{}.meta.yaml", repair.receipt.id.as_str());
    commit_paths(
        repository.root(),
        &repair.commit_message,
        ALICE,
        &[&repair_receipt_path],
    );

    let rejected_repair =
        validate_incoming_skill_history(&repository.storage, &conflicted, &repository.tip())
            .unwrap();

    assert!(rejected_repair
        .checkpoint
        .conflicts
        .contains_key("release-check"));
}

#[test]
fn repair_with_the_wrong_post_repair_tree_oid_keeps_the_conflict() {
    let repository = Repository::new();
    let initial = repository.snapshot();
    let bootstrap = bootstrap_plan(
        &gitim_core::skill::SkillRepositorySnapshot {
            active_users: BTreeSet::from([ALICE.to_owned()]),
            ..Default::default()
        },
        'G',
    );
    let bootstrap_tip = repository.commit_plan(&bootstrap);
    let accepted_bootstrap =
        validate_incoming_skill_history(&repository.storage, &initial, &bootstrap_tip)
            .unwrap()
            .checkpoint;
    let create = create_plan(&bootstrap.after, 'H');
    let create_tip = repository.commit_plan(&create);
    let accepted_create =
        validate_incoming_skill_history(&repository.storage, &accepted_bootstrap, &create_tip)
            .unwrap()
            .checkpoint;
    let meta_path = "skills/release-check/skill.meta.yaml";
    let mut corrupt_meta = fs::read_to_string(repository.root().join(meta_path)).unwrap();
    corrupt_meta = corrupt_meta.replace("Verify releases.", "Rejected bytes.");
    fs::write(repository.root().join(meta_path), &corrupt_meta).unwrap();
    commit_all(repository.root(), "bypass skill state", ALICE);
    let rejected_tip = repository.tip();
    let conflicted =
        validate_incoming_skill_history(&repository.storage, &accepted_create, &rejected_tip)
            .unwrap()
            .checkpoint;
    let accepted_tree = conflicted.skills["release-check"].tree.tree_oid.clone();
    let slug = gitim_core::skill::SkillSlug::new("release-check").unwrap();
    let accepted_files: BTreeMap<_, _> = create
        .after
        .repository_files
        .iter()
        .filter(|(path, _)| path.starts_with("skills/release-check/"))
        .map(|(path, bytes)| (path.clone(), bytes.clone()))
        .collect();
    let mut repair_before = create.after.clone();
    repair_before
        .repository_files
        .insert(meta_path.to_owned(), corrupt_meta.into_bytes());
    repair_before.conflict_checkpoint = Some(SkillConflictCheckpoint {
        conflict_tip: rejected_tip.clone(),
        accepted_tree: accepted_tree.clone(),
        accepted_state: SkillRepairAcceptedState::ActiveSkill {
            slug: slug.clone(),
            skill: create.after.active_skills[&slug].clone(),
        },
        accepted_files,
        entry_changed_paths: BTreeSet::new(),
        rejected_receipt_paths: BTreeSet::new(),
        changed_paths: BTreeSet::from([meta_path.to_owned()]),
    });
    let repair = plan_skill_mutation(
        &repair_before,
        &context(None),
        &SkillMutationRequest::Repair(SkillRepairRequest {
            request_id: request_id('J'),
            scope: SkillRepairScope::Skill(slug),
            conflict_tip: rejected_tip,
            accepted_tree,
        }),
    )
    .unwrap();
    apply_plan(repository.root(), &repair);
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(repository.root().join(meta_path))
        .unwrap()
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(repository.root().join(meta_path), permissions).unwrap();
    commit_all(repository.root(), &repair.commit_message, ALICE);

    let rejected_repair =
        validate_incoming_skill_history(&repository.storage, &conflicted, &repository.tip())
            .unwrap();

    assert!(rejected_repair
        .checkpoint
        .conflicts
        .contains_key("release-check"));
}

#[test]
fn otherwise_valid_create_rejects_an_executable_managed_leaf() {
    let repository = Repository::new();
    let initial = repository.snapshot();
    let bootstrap = bootstrap_plan(
        &SkillRepositorySnapshot {
            active_users: BTreeSet::from([ALICE.to_owned()]),
            ..Default::default()
        },
        'A',
    );
    let bootstrap_tip = repository.commit_plan(&bootstrap);
    let accepted = validate_incoming_skill_history(&repository.storage, &initial, &bootstrap_tip)
        .unwrap()
        .checkpoint;
    let create = create_plan(&bootstrap.after, 'B');
    apply_plan(repository.root(), &create);
    git(repository.root(), &["add", "-A"]);
    let script_path = format!(
        "skills/release-check/revisions/{}/package/scripts/check.sh",
        create.receipt.request.revision.as_ref().unwrap().as_str()
    );
    git(
        repository.root(),
        &["update-index", "--chmod=+x", "--", &script_path],
    );
    commit_index(repository.root(), &create.commit_message, ALICE);

    let rejected =
        validate_incoming_skill_history(&repository.storage, &accepted, &repository.tip()).unwrap();

    assert!(!rejected.checkpoint.skills.contains_key("release-check"));
    assert!(rejected.checkpoint.conflicts.contains_key("release-check"));
}

#[test]
fn symlink_and_gitlink_managed_leaves_never_become_accepted() {
    for object_kind in ["symlink", "gitlink"] {
        let repository = Repository::new();
        let initial = repository.snapshot();
        let bootstrap = bootstrap_plan(
            &SkillRepositorySnapshot {
                active_users: BTreeSet::from([ALICE.to_owned()]),
                ..Default::default()
            },
            'A',
        );
        let bootstrap_tip = repository.commit_plan(&bootstrap);
        let accepted =
            validate_incoming_skill_history(&repository.storage, &initial, &bootstrap_tip)
                .unwrap()
                .checkpoint;
        let create = create_plan(&bootstrap.after, 'B');
        apply_plan(repository.root(), &create);
        git(repository.root(), &["add", "-A"]);
        let script_path = format!(
            "skills/release-check/revisions/{}/package/scripts/check.sh",
            create.receipt.request.revision.as_ref().unwrap().as_str()
        );
        let (mode, oid) = if object_kind == "symlink" {
            ("120000", hash_blob(repository.root(), b"../SKILL.md"))
        } else {
            ("160000", bootstrap_tip.clone())
        };
        replace_index_scope_root(repository.root(), &script_path, mode, &oid);
        commit_index(repository.root(), &create.commit_message, ALICE);

        let rejected =
            validate_incoming_skill_history(&repository.storage, &accepted, &repository.tip())
                .unwrap();

        assert!(
            !rejected.checkpoint.skills.contains_key("release-check"),
            "{object_kind}"
        );
        assert!(
            rejected.checkpoint.conflicts.contains_key("release-check"),
            "{object_kind}"
        );
    }
}

#[test]
fn private_index_repair_normalizes_a_mode_only_conflict() {
    let repository = Repository::new();
    let initial = repository.snapshot();
    let bootstrap = bootstrap_plan(
        &SkillRepositorySnapshot {
            active_users: BTreeSet::from([ALICE.to_owned()]),
            ..Default::default()
        },
        'A',
    );
    let bootstrap_tip = repository.commit_plan(&bootstrap);
    let accepted_bootstrap =
        validate_incoming_skill_history(&repository.storage, &initial, &bootstrap_tip)
            .unwrap()
            .checkpoint;
    let create = create_plan(&bootstrap.after, 'B');
    let create_tip = repository.commit_plan(&create);
    let accepted =
        validate_incoming_skill_history(&repository.storage, &accepted_bootstrap, &create_tip)
            .unwrap()
            .checkpoint;
    let script_path = format!(
        "skills/release-check/revisions/{}/package/scripts/check.sh",
        create.receipt.request.revision.as_ref().unwrap().as_str()
    );
    git(
        repository.root(),
        &["update-index", "--chmod=+x", "--", &script_path],
    );
    git(repository.root(), &["commit", "-m", "mode-only corruption"]);
    let rejected_tip = repository.tip();
    let conflicted = validate_incoming_skill_history(&repository.storage, &accepted, &rejected_tip)
        .unwrap()
        .checkpoint;
    let conflict = conflicted.conflicts["release-check"].clone();
    let accepted_tree = conflict.accepted_tree_oid.clone().unwrap();
    let repair = plan_repair_for_skill(
        &create.after,
        &conflict,
        "release-check",
        skill_scope_files(&create.after, "release-check"),
        BTreeSet::from([script_path.clone()]),
        &[],
        'C',
    );
    assert!(repair.edits.iter().any(|edit| {
        matches!(
            edit,
            gitim_core::skill::SkillTreeEdit::Upsert { path, .. }
                if path == &script_path
        )
    }));
    let repair_tip = commit_plan_with_private_index(&repository, &repair);

    let repaired =
        validate_incoming_skill_history(&repository.storage, &conflicted, &repair_tip).unwrap();

    assert!(repaired.checkpoint.conflicts.is_empty());
    assert_eq!(
        tree_oid_at(&repository.storage, &repair_tip, "skills/release-check").unwrap(),
        Some(accepted_tree)
    );
    assert_eq!(
        git_output(
            repository.root(),
            &["ls-tree", &repair_tip, "--", &script_path]
        )
        .split_whitespace()
        .next(),
        Some("100644")
    );
}

#[test]
fn absent_skill_repair_deletes_exact_scope_root_collisions() {
    for object_kind in ["blob", "symlink", "gitlink"] {
        let repository = Repository::new();
        let initial = repository.snapshot();
        let bootstrap = bootstrap_plan(
            &SkillRepositorySnapshot {
                active_users: BTreeSet::from([ALICE.to_owned()]),
                ..Default::default()
            },
            'A',
        );
        let bootstrap_tip = repository.commit_plan(&bootstrap);
        let accepted =
            validate_incoming_skill_history(&repository.storage, &initial, &bootstrap_tip)
                .unwrap()
                .checkpoint;
        let create = create_plan(&bootstrap.after, 'B');
        let receipt_path = format!("skills/receipts/{}.meta.yaml", create.receipt.id.as_str());
        let receipt_edit = create
            .edits
            .iter()
            .find(|edit| {
                matches!(
                    edit,
                    gitim_core::skill::SkillTreeEdit::Upsert { path, .. }
                        if path == &receipt_path
                )
            })
            .unwrap();
        if let gitim_core::skill::SkillTreeEdit::Upsert { path, bytes } = receipt_edit {
            fs::write(repository.root().join(path), bytes).unwrap();
        }
        git(repository.root(), &["add", "--", &receipt_path]);
        let root_path = "skills/release-check";
        let collision_bytes = format!("{object_kind} collision\n").into_bytes();
        let (mode, oid) = match object_kind {
            "blob" => ("100644", hash_blob(repository.root(), &collision_bytes)),
            "symlink" => ("120000", hash_blob(repository.root(), &collision_bytes)),
            "gitlink" => ("160000", bootstrap_tip.clone()),
            _ => unreachable!(),
        };
        replace_index_scope_root(repository.root(), root_path, mode, &oid);
        commit_index(repository.root(), &create.commit_message, ALICE);
        let rejected_tip = repository.tip();
        let conflicted =
            validate_incoming_skill_history(&repository.storage, &accepted, &rejected_tip)
                .unwrap()
                .checkpoint;
        let conflict = conflicted.conflicts["release-check"].clone();
        let actual_scope_files = if object_kind == "gitlink" {
            BTreeMap::new()
        } else {
            BTreeMap::from([(root_path.to_owned(), collision_bytes)])
        };
        let entry_changed_paths = if object_kind == "gitlink" {
            BTreeSet::from([root_path.to_owned()])
        } else {
            BTreeSet::new()
        };
        let repair = plan_repair_for_skill(
            &bootstrap.after,
            &conflict,
            "release-check",
            actual_scope_files,
            entry_changed_paths,
            std::slice::from_ref(&create.receipt),
            'C',
        );
        assert!(repair.edits.iter().any(|edit| {
            matches!(
                edit,
                gitim_core::skill::SkillTreeEdit::Delete { path }
                    if path == root_path
            )
        }));
        let repair_tip = commit_plan_with_private_index(&repository, &repair);

        let repaired =
            validate_incoming_skill_history(&repository.storage, &conflicted, &repair_tip).unwrap();

        assert!(repaired.checkpoint.conflicts.is_empty(), "{object_kind}");
        assert!(
            tree_oid_at(&repository.storage, &repair_tip, root_path)
                .unwrap()
                .is_none(),
            "{object_kind}"
        );
        assert!(
            tree_oid_at(&repository.storage, &repair_tip, &receipt_path)
                .unwrap()
                .is_none(),
            "{object_kind}"
        );
    }
}

#[test]
fn repair_restores_an_accepted_tree_replaced_by_a_root_blob_or_gitlink() {
    for object_kind in ["blob", "gitlink"] {
        let repository = Repository::new();
        let initial = repository.snapshot();
        let bootstrap = bootstrap_plan(
            &SkillRepositorySnapshot {
                active_users: BTreeSet::from([ALICE.to_owned()]),
                ..Default::default()
            },
            'A',
        );
        let bootstrap_tip = repository.commit_plan(&bootstrap);
        let accepted_bootstrap =
            validate_incoming_skill_history(&repository.storage, &initial, &bootstrap_tip)
                .unwrap()
                .checkpoint;
        let create = create_plan(&bootstrap.after, 'B');
        let create_tip = repository.commit_plan(&create);
        let accepted =
            validate_incoming_skill_history(&repository.storage, &accepted_bootstrap, &create_tip)
                .unwrap()
                .checkpoint;
        let accepted_tree = accepted.skills["release-check"].tree.tree_oid.clone();
        let root_path = "skills/release-check";
        let collision_bytes = b"root collision\n".to_vec();
        let (mode, oid) = if object_kind == "blob" {
            ("100644", hash_blob(repository.root(), &collision_bytes))
        } else {
            ("160000", bootstrap_tip)
        };
        replace_index_scope_root(repository.root(), root_path, mode, &oid);
        commit_index(repository.root(), "replace accepted Skill root", ALICE);
        let rejected_tip = repository.tip();
        let conflicted =
            validate_incoming_skill_history(&repository.storage, &accepted, &rejected_tip)
                .unwrap()
                .checkpoint;
        let conflict = conflicted.conflicts["release-check"].clone();
        let actual_scope_files = if object_kind == "blob" {
            BTreeMap::from([(root_path.to_owned(), collision_bytes)])
        } else {
            BTreeMap::new()
        };
        let entry_changed_paths = if object_kind == "gitlink" {
            BTreeSet::from([root_path.to_owned()])
        } else {
            BTreeSet::new()
        };
        let repair = plan_repair_for_skill(
            &create.after,
            &conflict,
            "release-check",
            actual_scope_files,
            entry_changed_paths,
            &[],
            'C',
        );
        let repair_tip = commit_plan_with_private_index(&repository, &repair);

        let repaired =
            validate_incoming_skill_history(&repository.storage, &conflicted, &repair_tip).unwrap();

        assert!(repaired.checkpoint.conflicts.is_empty(), "{object_kind}");
        assert_eq!(
            tree_oid_at(&repository.storage, &repair_tip, root_path).unwrap(),
            Some(accepted_tree.clone()),
            "{object_kind}"
        );
    }
}

#[test]
fn rejected_receipt_cleanup_is_owned_by_its_declared_skill_in_both_repair_orders() {
    for owner_first in [true, false] {
        let repository = Repository::new();
        let initial = repository.snapshot();
        let bootstrap = bootstrap_plan(
            &SkillRepositorySnapshot {
                active_users: BTreeSet::from([ALICE.to_owned()]),
                ..Default::default()
            },
            'A',
        );
        let bootstrap_tip = repository.commit_plan(&bootstrap);
        let accepted_bootstrap =
            validate_incoming_skill_history(&repository.storage, &initial, &bootstrap_tip)
                .unwrap()
                .checkpoint;
        let release_check = create_plan_for(&bootstrap.after, 'B', "release-check");
        let release_check_tip = repository.commit_plan(&release_check);
        let accepted_release_check = validate_incoming_skill_history(
            &repository.storage,
            &accepted_bootstrap,
            &release_check_tip,
        )
        .unwrap()
        .checkpoint;
        let release_notes = create_plan_for(&release_check.after, 'C', "release-notes");
        let release_notes_tip = repository.commit_plan(&release_notes);
        let accepted = validate_incoming_skill_history(
            &repository.storage,
            &accepted_release_check,
            &release_notes_tip,
        )
        .unwrap()
        .checkpoint;
        let proposal = plan_skill_mutation(
            &release_notes.after,
            &context(Some(package_for("release-check"))),
            &SkillMutationRequest::Propose(SkillProposeRequest {
                request_id: request_id('D'),
                slug: gitim_core::skill::SkillSlug::new("release-check").unwrap(),
                base_revision: release_notes.after.active_skills
                    [&gitim_core::skill::SkillSlug::new("release-check").unwrap()]
                    .meta
                    .current_revision
                    .clone(),
                summary: "Candidate".to_owned(),
                source_directory: "/unused".into(),
            }),
        )
        .unwrap();
        let rejected_receipt_path =
            format!("skills/receipts/{}.meta.yaml", proposal.receipt.id.as_str());
        let receipt_edit = proposal
            .edits
            .iter()
            .find(|edit| {
                matches!(
                    edit,
                    gitim_core::skill::SkillTreeEdit::Upsert { path, .. }
                        if path == &rejected_receipt_path
                )
            })
            .unwrap();
        if let gitim_core::skill::SkillTreeEdit::Upsert { path, bytes } = receipt_edit {
            fs::write(repository.root().join(path), bytes).unwrap();
        }
        let check_meta = "skills/release-check/skill.meta.yaml";
        let notes_meta = "skills/release-notes/skill.meta.yaml";
        let check_corrupt = fs::read_to_string(repository.root().join(check_meta))
            .unwrap()
            .replace("Verify releases.", "Rejected check bytes.");
        let notes_corrupt = fs::read_to_string(repository.root().join(notes_meta))
            .unwrap()
            .replace("Verify releases.", "Rejected notes bytes.");
        fs::write(repository.root().join(check_meta), &check_corrupt).unwrap();
        fs::write(repository.root().join(notes_meta), &notes_corrupt).unwrap();
        commit_all(repository.root(), &proposal.commit_message, ALICE);
        let rejected_tip = repository.tip();
        let mut checkpoint =
            validate_incoming_skill_history(&repository.storage, &accepted, &rejected_tip)
                .unwrap()
                .checkpoint;
        assert_eq!(checkpoint.conflicts.len(), 2);
        assert_eq!(
            checkpoint.conflicts["release-check"].rejected_receipt_paths,
            BTreeSet::from([rejected_receipt_path.clone()])
        );
        assert!(checkpoint.conflicts["release-notes"]
            .rejected_receipt_paths
            .is_empty());

        let mut accepted_model = release_notes.after.clone();
        let mut corrupt_files = BTreeMap::from([
            (
                "release-check",
                skill_scope_files(&accepted_model, "release-check"),
            ),
            (
                "release-notes",
                skill_scope_files(&accepted_model, "release-notes"),
            ),
        ]);
        corrupt_files
            .get_mut("release-check")
            .unwrap()
            .insert(check_meta.to_owned(), check_corrupt.into_bytes());
        corrupt_files
            .get_mut("release-notes")
            .unwrap()
            .insert(notes_meta.to_owned(), notes_corrupt.into_bytes());
        let repair_order = if owner_first {
            ["release-check", "release-notes"]
        } else {
            ["release-notes", "release-check"]
        };
        for (index, slug) in repair_order.into_iter().enumerate() {
            let conflict = checkpoint.conflicts[slug].clone();
            let repair = plan_repair_for_skill(
                &accepted_model,
                &conflict,
                slug,
                corrupt_files[slug].clone(),
                BTreeSet::new(),
                std::slice::from_ref(&proposal.receipt),
                if index == 0 { 'E' } else { 'F' },
            );
            let repair_tip = commit_plan_with_private_index(&repository, &repair);
            let repaired =
                validate_incoming_skill_history(&repository.storage, &checkpoint, &repair_tip)
                    .unwrap();
            checkpoint = repaired.checkpoint;
            accepted_model = repair.after;
        }

        assert!(checkpoint.conflicts.is_empty(), "owner_first={owner_first}");
        assert!(
            tree_oid_at(
                &repository.storage,
                &repository.tip(),
                &rejected_receipt_path
            )
            .unwrap()
            .is_none(),
            "owner_first={owner_first}"
        );
        assert_eq!(
            tree_oid_at(
                &repository.storage,
                &repository.tip(),
                "skills/release-check"
            )
            .unwrap(),
            Some(accepted.skills["release-check"].tree.tree_oid.clone())
        );
        assert_eq!(
            tree_oid_at(
                &repository.storage,
                &repository.tip(),
                "skills/release-notes"
            )
            .unwrap(),
            Some(accepted.skills["release-notes"].tree.tree_oid.clone())
        );
    }
}

#[test]
fn rejected_commits_prune_stale_receipt_cleanup_authority_across_scopes() {
    let repository = Repository::new();
    let initial = repository.snapshot();
    let bootstrap = bootstrap_plan(
        &SkillRepositorySnapshot {
            active_users: BTreeSet::from([ALICE.to_owned()]),
            ..Default::default()
        },
        'A',
    );
    let bootstrap_tip = repository.commit_plan(&bootstrap);
    let accepted = validate_incoming_skill_history(&repository.storage, &initial, &bootstrap_tip)
        .unwrap()
        .checkpoint;
    let create = create_plan(&bootstrap.after, 'B');
    let rejected_receipt_path = format!("skills/receipts/{}.meta.yaml", create.receipt.id.as_str());
    apply_plan(repository.root(), &create);
    let meta_path = "skills/release-check/skill.meta.yaml";
    let malformed_meta = fs::read_to_string(repository.root().join(meta_path))
        .unwrap()
        .replace("Verify releases.", "Rejected bytes.");
    fs::write(repository.root().join(meta_path), &malformed_meta).unwrap();
    commit_all(repository.root(), &create.commit_message, ALICE);
    let first_rejected_tip = repository.tip();
    let mut checkpoint =
        validate_incoming_skill_history(&repository.storage, &accepted, &first_rejected_tip)
            .unwrap()
            .checkpoint;
    assert_eq!(
        checkpoint.conflicts["release-check"].rejected_receipt_paths,
        BTreeSet::from([rejected_receipt_path.clone()])
    );

    fs::remove_file(repository.root().join(&rejected_receipt_path)).unwrap();
    fs::write(
        repository.root().join("skills/release-notes"),
        b"first B collision\n",
    )
    .unwrap();
    commit_all(
        repository.root(),
        "delete A receipt while corrupting B",
        ALICE,
    );
    checkpoint =
        validate_incoming_skill_history(&repository.storage, &checkpoint, &repository.tip())
            .unwrap()
            .checkpoint;
    assert!(checkpoint.conflicts["release-check"]
        .rejected_receipt_paths
        .is_empty());
    assert!(checkpoint.conflicts.contains_key("release-notes"));

    fs::write(
        repository.root().join(&rejected_receipt_path),
        b"not: [valid\n",
    )
    .unwrap();
    fs::write(
        repository.root().join("skills/release-notes"),
        b"second B collision\n",
    )
    .unwrap();
    commit_all(
        repository.root(),
        "malformed A receipt reappears while corrupting B",
        ALICE,
    );
    checkpoint =
        validate_incoming_skill_history(&repository.storage, &checkpoint, &repository.tip())
            .unwrap()
            .checkpoint;
    assert!(checkpoint.conflicts["release-check"]
        .rejected_receipt_paths
        .is_empty());

    fs::remove_file(repository.root().join(&rejected_receipt_path)).unwrap();
    fs::write(
        repository.root().join("skills/release-notes"),
        b"third B collision\n",
    )
    .unwrap();
    commit_all(
        repository.root(),
        "remove malformed A receipt while corrupting B",
        ALICE,
    );
    checkpoint =
        validate_incoming_skill_history(&repository.storage, &checkpoint, &repository.tip())
            .unwrap()
            .checkpoint;
    assert!(checkpoint.conflicts["release-check"]
        .rejected_receipt_paths
        .is_empty());

    fs::write(
        repository.root().join(&rejected_receipt_path),
        serde_yaml::to_string(&create.receipt).unwrap(),
    )
    .unwrap();
    fs::write(
        repository.root().join("skills/release-notes"),
        b"fourth B collision\n",
    )
    .unwrap();
    commit_all(
        repository.root(),
        "valid A receipt explicitly reappears while corrupting B",
        ALICE,
    );
    checkpoint =
        validate_incoming_skill_history(&repository.storage, &checkpoint, &repository.tip())
            .unwrap()
            .checkpoint;
    assert_eq!(
        checkpoint.conflicts["release-check"].rejected_receipt_paths,
        BTreeSet::from([rejected_receipt_path.clone()])
    );

    fs::remove_file(repository.root().join(&rejected_receipt_path)).unwrap();
    fs::write(
        repository.root().join("skills/release-notes"),
        b"fifth B collision\n",
    )
    .unwrap();
    commit_all(
        repository.root(),
        "delete reintroduced A receipt while corrupting B",
        ALICE,
    );
    checkpoint =
        validate_incoming_skill_history(&repository.storage, &checkpoint, &repository.tip())
            .unwrap()
            .checkpoint;
    assert!(checkpoint.conflicts["release-check"]
        .rejected_receipt_paths
        .is_empty());

    let b_conflict = checkpoint.conflicts["release-notes"].clone();
    let mut actual_a_files = skill_scope_files(&create.after, "release-check");
    actual_a_files.insert(meta_path.to_owned(), malformed_meta.into_bytes());
    let repair = plan_repair_for_skill(
        &bootstrap.after,
        &checkpoint.conflicts["release-check"],
        "release-check",
        actual_a_files,
        BTreeSet::new(),
        std::slice::from_ref(&create.receipt),
        'C',
    );
    let repair_tip = commit_plan_with_private_index(&repository, &repair);
    let repaired =
        validate_incoming_skill_history(&repository.storage, &checkpoint, &repair_tip).unwrap();

    assert!(!repaired.checkpoint.conflicts.contains_key("release-check"));
    assert_eq!(repaired.checkpoint.conflicts["release-notes"], b_conflict);
    assert!(
        tree_oid_at(&repository.storage, &repair_tip, &rejected_receipt_path)
            .unwrap()
            .is_none()
    );
}

#[test]
fn receipt_cleanup_authority_requires_a_regular_non_executable_blob() {
    let repository = Repository::new();
    let initial = repository.snapshot();
    let bootstrap = bootstrap_plan(
        &SkillRepositorySnapshot {
            active_users: BTreeSet::from([ALICE.to_owned()]),
            ..Default::default()
        },
        'A',
    );
    let bootstrap_tip = repository.commit_plan(&bootstrap);
    let accepted = validate_incoming_skill_history(&repository.storage, &initial, &bootstrap_tip)
        .unwrap()
        .checkpoint;
    let create = create_plan(&bootstrap.after, 'B');
    let receipt_path = format!("skills/receipts/{}.meta.yaml", create.receipt.id.as_str());
    let receipt_bytes = serde_yaml::to_string(&create.receipt).unwrap().into_bytes();
    apply_plan(repository.root(), &create);
    let meta_path = "skills/release-check/skill.meta.yaml";
    let malformed_meta = fs::read_to_string(repository.root().join(meta_path))
        .unwrap()
        .replace("Verify releases.", "Rejected bytes.");
    fs::write(repository.root().join(meta_path), malformed_meta).unwrap();
    commit_all(repository.root(), &create.commit_message, ALICE);
    let mut checkpoint =
        validate_incoming_skill_history(&repository.storage, &accepted, &repository.tip())
            .unwrap()
            .checkpoint;
    assert_eq!(
        checkpoint.conflicts["release-check"].rejected_receipt_paths,
        BTreeSet::from([receipt_path.clone()])
    );

    git(
        repository.root(),
        &["update-index", "--chmod=+x", "--", &receipt_path],
    );
    git(
        repository.root(),
        &["commit", "-m", "make owned receipt executable"],
    );
    checkpoint =
        validate_incoming_skill_history(&repository.storage, &checkpoint, &repository.tip())
            .unwrap()
            .checkpoint;
    assert!(checkpoint.conflicts["release-check"]
        .rejected_receipt_paths
        .is_empty());

    fs::remove_file(repository.root().join(&receipt_path)).unwrap();
    fs::write(
        repository.root().join("skills/release-notes"),
        b"first B collision\n",
    )
    .unwrap();
    commit_all(
        repository.root(),
        "delete executable receipt while corrupting B",
        ALICE,
    );
    checkpoint =
        validate_incoming_skill_history(&repository.storage, &checkpoint, &repository.tip())
            .unwrap()
            .checkpoint;

    fs::write(repository.root().join(&receipt_path), &receipt_bytes).unwrap();
    fs::write(
        repository.root().join("skills/release-notes"),
        b"second B collision\n",
    )
    .unwrap();
    git(repository.root(), &["add", "-A"]);
    git(
        repository.root(),
        &["update-index", "--chmod=+x", "--", &receipt_path],
    );
    commit_index(
        repository.root(),
        "executable valid owner receipt reappears",
        ALICE,
    );
    checkpoint =
        validate_incoming_skill_history(&repository.storage, &checkpoint, &repository.tip())
            .unwrap()
            .checkpoint;
    assert!(checkpoint.conflicts["release-check"]
        .rejected_receipt_paths
        .is_empty());

    fs::remove_file(repository.root().join(&receipt_path)).unwrap();
    fs::write(
        repository.root().join("skills/release-notes"),
        b"third B collision\n",
    )
    .unwrap();
    commit_all(
        repository.root(),
        "delete executable reappearance while corrupting B",
        ALICE,
    );
    checkpoint =
        validate_incoming_skill_history(&repository.storage, &checkpoint, &repository.tip())
            .unwrap()
            .checkpoint;

    fs::write(
        repository.root().join("skills/release-notes"),
        b"fourth B collision\n",
    )
    .unwrap();
    git(repository.root(), &["add", "--", "skills/release-notes"]);
    let receipt_blob = hash_blob(repository.root(), &receipt_bytes);
    replace_index_scope_root(repository.root(), &receipt_path, "120000", &receipt_blob);
    commit_index(
        repository.root(),
        "parseable symlink owner receipt reappears",
        ALICE,
    );
    checkpoint =
        validate_incoming_skill_history(&repository.storage, &checkpoint, &repository.tip())
            .unwrap()
            .checkpoint;
    assert!(checkpoint.conflicts["release-check"]
        .rejected_receipt_paths
        .is_empty());

    git(
        repository.root(),
        &["update-index", "--force-remove", "--", &receipt_path],
    );
    fs::write(
        repository.root().join("skills/release-notes"),
        b"fifth B collision\n",
    )
    .unwrap();
    git(repository.root(), &["add", "--", "skills/release-notes"]);
    commit_index(
        repository.root(),
        "delete symlink reappearance while corrupting B",
        ALICE,
    );
    checkpoint =
        validate_incoming_skill_history(&repository.storage, &checkpoint, &repository.tip())
            .unwrap()
            .checkpoint;

    fs::write(repository.root().join(&receipt_path), receipt_bytes).unwrap();
    fs::write(
        repository.root().join("skills/release-notes"),
        b"sixth B collision\n",
    )
    .unwrap();
    commit_all(
        repository.root(),
        "canonical valid owner receipt reappears",
        ALICE,
    );
    checkpoint =
        validate_incoming_skill_history(&repository.storage, &checkpoint, &repository.tip())
            .unwrap()
            .checkpoint;
    assert_eq!(
        checkpoint.conflicts["release-check"].rejected_receipt_paths,
        BTreeSet::from([receipt_path])
    );
}

#[test]
fn malformed_first_bootstrap_has_no_repository_visible_repair_authority() {
    let repository = Repository::new();
    fs::write(repository.root().join("ordinary.txt"), "preserve\n").unwrap();
    commit_all(repository.root(), "ordinary", ALICE);
    let initial = repository.snapshot();
    let before = gitim_core::skill::SkillRepositorySnapshot {
        active_users: BTreeSet::from([ALICE.to_owned()]),
        ..Default::default()
    };
    let bootstrap = bootstrap_plan(&before, '4');
    apply_plan(repository.root(), &bootstrap);
    let workspace_path = "skills/workspace.meta.yaml";
    let mut malformed_workspace =
        fs::read_to_string(repository.root().join(workspace_path)).unwrap();
    malformed_workspace =
        malformed_workspace.replace("administrators:\n- alice", "administrators: []");
    fs::write(repository.root().join(workspace_path), &malformed_workspace).unwrap();
    commit_all(repository.root(), &bootstrap.commit_message, ALICE);
    let rejected_tip = repository.tip();
    let conflicted = validate_incoming_skill_history(&repository.storage, &initial, &rejected_tip)
        .unwrap()
        .checkpoint;
    let conflict = conflicted.conflicts["$workspace"].clone();
    assert_eq!(conflict.accepted_tree_oid, None);
    assert!(conflicted.workspace_tree.is_none());
    assert!(repository.root().join(workspace_path).exists());
    assert_eq!(
        fs::read_to_string(repository.root().join("ordinary.txt")).unwrap(),
        "preserve\n"
    );
}

#[test]
fn authorized_repair_restores_absence_after_malformed_first_create() {
    let repository = Repository::new();
    fs::write(repository.root().join("ordinary.txt"), "preserve\n").unwrap();
    commit_all(repository.root(), "ordinary", ALICE);
    let initial = repository.snapshot();
    let bootstrap = bootstrap_plan(
        &gitim_core::skill::SkillRepositorySnapshot {
            active_users: BTreeSet::from([ALICE.to_owned()]),
            ..Default::default()
        },
        '6',
    );
    let bootstrap_tip = repository.commit_plan(&bootstrap);
    let accepted_bootstrap =
        validate_incoming_skill_history(&repository.storage, &initial, &bootstrap_tip)
            .unwrap()
            .checkpoint;
    let create = create_plan(&bootstrap.after, '7');
    let rejected_receipt_path = format!("skills/receipts/{}.meta.yaml", create.receipt.id.as_str());
    apply_plan(repository.root(), &create);
    let meta_path = "skills/release-check/skill.meta.yaml";
    let mut malformed_meta = fs::read_to_string(repository.root().join(meta_path)).unwrap();
    malformed_meta = malformed_meta.replace("Verify releases.", "Rejected bytes.");
    fs::write(repository.root().join(meta_path), &malformed_meta).unwrap();
    commit_all(repository.root(), &create.commit_message, ALICE);
    let rejected_tip = repository.tip();
    let conflicted =
        validate_incoming_skill_history(&repository.storage, &accepted_bootstrap, &rejected_tip)
            .unwrap()
            .checkpoint;
    let conflict = conflicted.conflicts["release-check"].clone();
    let accepted_tree = conflict.accepted_tree_oid.clone().unwrap();
    assert!(!accepted_tree.is_empty());
    assert_eq!(
        conflict.rejected_receipt_paths,
        BTreeSet::from([rejected_receipt_path.clone()])
    );

    let slug = gitim_core::skill::SkillSlug::new("release-check").unwrap();
    let mut repair_before = bootstrap.after.clone();
    repair_before.repository_files.extend(
        create
            .after
            .repository_files
            .iter()
            .filter(|(path, _)| {
                path.starts_with("skills/release-check/") || *path == &rejected_receipt_path
            })
            .map(|(path, bytes)| (path.clone(), bytes.clone())),
    );
    repair_before
        .repository_files
        .insert(meta_path.to_owned(), malformed_meta.into_bytes());
    repair_before
        .receipts
        .insert(create.receipt.id.clone(), create.receipt.clone());
    let mut changed_paths: BTreeSet<_> = repair_before
        .repository_files
        .keys()
        .filter(|path| path.starts_with("skills/release-check/"))
        .cloned()
        .collect();
    changed_paths.insert(rejected_receipt_path.clone());
    repair_before.conflict_checkpoint = Some(SkillConflictCheckpoint {
        conflict_tip: rejected_tip.clone(),
        accepted_tree: accepted_tree.clone(),
        accepted_state: SkillRepairAcceptedState::AbsentSkill { slug: slug.clone() },
        accepted_files: BTreeMap::new(),
        entry_changed_paths: BTreeSet::new(),
        rejected_receipt_paths: BTreeSet::from([rejected_receipt_path.clone()]),
        changed_paths,
    });
    let repair = plan_skill_mutation(
        &repair_before,
        &context(None),
        &SkillMutationRequest::Repair(SkillRepairRequest {
            request_id: request_id('8'),
            scope: SkillRepairScope::Skill(slug),
            conflict_tip: rejected_tip,
            accepted_tree,
        }),
    )
    .unwrap();
    assert!(repair.changed_paths.contains(&rejected_receipt_path));
    assert!(repair.edits.iter().any(|edit| {
        matches!(
            edit,
            gitim_core::skill::SkillTreeEdit::Delete { path }
                if path == &rejected_receipt_path
        )
    }));
    let repair_tip = repository.commit_plan(&repair);
    let committed_paths: BTreeSet<_> = git_output(
        repository.root(),
        &[
            "diff",
            "--name-only",
            &format!("{repair_tip}^"),
            &repair_tip,
        ],
    )
    .lines()
    .map(str::to_owned)
    .collect();
    assert_eq!(committed_paths, repair.changed_paths);

    let repaired =
        validate_incoming_skill_history(&repository.storage, &conflicted, &repair_tip).unwrap();

    assert!(
        repaired.checkpoint.conflicts.is_empty(),
        "{:?}",
        repaired.checkpoint.conflicts
    );
    assert!(!repaired.checkpoint.skills.contains_key("release-check"));
    assert!(git_output(
        repository.root(),
        &[
            "ls-tree",
            "-r",
            "--name-only",
            "HEAD",
            "--",
            "skills/release-check",
        ],
    )
    .is_empty());
    assert!(git_output(
        repository.root(),
        &[
            "ls-tree",
            "-r",
            "--name-only",
            "HEAD",
            "--",
            &rejected_receipt_path,
        ],
    )
    .is_empty());
    assert_eq!(
        fs::read_to_string(repository.root().join("ordinary.txt")).unwrap(),
        "preserve\n"
    );

    let bare = tempfile::tempdir().unwrap();
    git(bare.path(), &["init", "--bare"]);
    git(
        repository.root(),
        &["remote", "add", "origin", bare.path().to_str().unwrap()],
    );
    git(repository.root(), &["push", "-u", "origin", "main"]);
    let archive = tempfile::tempdir().unwrap();
    let outcome = try_fire_rotation(
        &repository.storage,
        "main",
        1,
        archive.path(),
        (ALICE, "alice@example.com"),
        NOW,
    )
    .unwrap();
    let orphan_commit = match outcome {
        RotationOutcome::Won {
            orphan_commit_sha, ..
        } => orphan_commit_sha,
        _ => return,
    };
    let after_rotation =
        validate_incoming_skill_history(&repository.storage, &repaired.checkpoint, &orphan_commit)
            .unwrap();
    assert_eq!(after_rotation.checkpoint.active_epoch, "main-epoch-2");
}

struct RotatedRepository {
    _bare: TempDir,
    clone: TempDir,
    storage: GitStorage,
    orphan_commit: String,
}

impl RotatedRepository {
    fn new() -> Result<Self, String> {
        let bare = tempfile::tempdir().unwrap();
        git(bare.path(), &["init", "--bare"]);
        let clone = tempfile::tempdir().unwrap();
        git(
            clone.path().parent().unwrap(),
            &[
                "clone",
                bare.path().to_str().unwrap(),
                clone.path().to_str().unwrap(),
            ],
        );
        git(clone.path(), &["checkout", "-b", "main"]);
        git(clone.path(), &["config", "user.name", ALICE]);
        git(clone.path(), &["config", "user.email", "alice@example.com"]);
        fs::create_dir_all(clone.path().join("users")).unwrap();
        fs::write(
            clone.path().join("users/alice.meta.yaml"),
            "display_name: Alice\nrole: human\nintroduction: Owner\n",
        )
        .unwrap();
        fs::write(clone.path().join("ordinary.txt"), "target").unwrap();
        commit_all(clone.path(), "seed", ALICE);
        git(clone.path(), &["push", "-u", "origin", "main"]);
        let storage = GitStorage::new(clone.path());
        let archive = tempfile::tempdir().unwrap();
        let outcome = try_fire_rotation(
            &storage,
            "main",
            1,
            archive.path(),
            (ALICE, "alice@example.com"),
            NOW,
        )
        .unwrap();
        let orphan_commit = match outcome {
            RotationOutcome::Won {
                orphan_commit_sha, ..
            } => orphan_commit_sha,
            _ => return Err("rotation did not win".to_owned()),
        };
        Ok(Self {
            _bare: bare,
            clone,
            storage,
            orphan_commit,
        })
    }

    fn replace_orphan_with_current_tree(&mut self) -> String {
        git(self.clone.path(), &["add", "-A"]);
        let tree = git_output(self.clone.path(), &["write-tree"]);
        let replacement = git_output(
            self.clone.path(),
            &["commit-tree", &tree, "-m", "replacement orphan"],
        );
        git(
            self.clone.path(),
            &[
                "update-ref",
                "refs/remotes/origin/main-epoch-2",
                &replacement,
            ],
        );
        self.orphan_commit.clone_from(&replacement);
        replacement
    }
}

#[test]
fn epoch_orphan_requires_an_exact_non_epoch_git_tree() {
    for mutation in ["mode", "user", "ordinary"] {
        let mut repository = RotatedRepository::new().unwrap();
        match mutation {
            "mode" => {
                fs::remove_file(repository.clone.path().join("ordinary.txt")).unwrap();
                #[cfg(unix)]
                std::os::unix::fs::symlink("target", repository.clone.path().join("ordinary.txt"))
                    .unwrap();
            }
            "user" => fs::write(
                repository.clone.path().join("users/alice.meta.yaml"),
                "display_name: Changed\nrole: human\nintroduction: Owner\n",
            )
            .unwrap(),
            "ordinary" => {
                fs::write(repository.clone.path().join("ordinary.txt"), "changed").unwrap()
            }
            _ => unreachable!(),
        }
        let replacement = repository.replace_orphan_with_current_tree();

        let error = validate_incoming_skill_history(
            &repository.storage,
            &SkillValidationCheckpoint::empty("main"),
            &replacement,
        )
        .unwrap_err();

        assert_eq!(error.code(), "skill_epoch_validation_blocked", "{mutation}");
    }
}

#[test]
fn epoch_snapshot_commit_must_be_an_orphan_root() {
    let repository = RotatedRepository::new().unwrap();
    let tree = git_output(repository.clone.path(), &["write-tree"]);
    let active_yaml = git_output(
        repository.clone.path(),
        &[
            "show",
            &format!("{}:gitim.epoch.yaml", repository.orphan_commit),
        ],
    );
    let active: EpochFile = serde_yaml::from_str(&active_yaml).unwrap();
    let source = &active.snapshot.unwrap().source_commit;
    let replacement = git_output(
        repository.clone.path(),
        &[
            "commit-tree",
            &tree,
            "-p",
            source,
            "-m",
            "non-orphan snapshot",
        ],
    );
    git(
        repository.clone.path(),
        &[
            "update-ref",
            "refs/remotes/origin/main-epoch-2",
            &replacement,
        ],
    );

    let error = validate_incoming_skill_history(
        &repository.storage,
        &SkillValidationCheckpoint::empty("main"),
        &replacement,
    )
    .unwrap_err();

    assert_eq!(error.code(), "skill_epoch_validation_blocked");
}

#[test]
fn epoch_seal_metadata_identifies_the_actual_predecessor() {
    for mutation in ["branch", "epoch"] {
        let repository = RotatedRepository::new().unwrap();
        git(repository.clone.path(), &["checkout", "main"]);
        let epoch_path = repository.clone.path().join("gitim.epoch.yaml");
        let mut epoch: EpochFile =
            serde_yaml::from_str(&fs::read_to_string(&epoch_path).unwrap()).unwrap();
        match mutation {
            "branch" => epoch.branch = "not-main".to_owned(),
            "epoch" => epoch.epoch += 7,
            _ => unreachable!(),
        }
        fs::write(epoch_path, serde_yaml::to_string(&epoch).unwrap()).unwrap();
        git(repository.clone.path(), &["add", "gitim.epoch.yaml"]);
        git(repository.clone.path(), &["commit", "--amend", "--no-edit"]);
        let replacement_seal = git_output(repository.clone.path(), &["rev-parse", "HEAD"]);
        git(
            repository.clone.path(),
            &["update-ref", "refs/remotes/origin/main", &replacement_seal],
        );
        git(repository.clone.path(), &["checkout", "main-epoch-2"]);

        let error = validate_incoming_skill_history(
            &repository.storage,
            &SkillValidationCheckpoint::empty("main"),
            &repository.orphan_commit,
        )
        .unwrap_err();

        assert_eq!(error.code(), "skill_epoch_validation_blocked", "{mutation}");
    }
}

#[test]
fn lagging_follower_replays_sealed_predecessor_before_accepting_orphan_snapshot(
) -> Result<(), Box<dyn std::error::Error>> {
    let bare = tempfile::tempdir().unwrap();
    git(bare.path(), &["init", "--bare"]);
    let clone = tempfile::tempdir().unwrap();
    git(
        clone.path().parent().unwrap(),
        &[
            "clone",
            bare.path().to_str().unwrap(),
            clone.path().to_str().unwrap(),
        ],
    );
    git(clone.path(), &["checkout", "-b", "main"]);
    git(clone.path(), &["config", "user.name", ALICE]);
    git(clone.path(), &["config", "user.email", "alice@example.com"]);
    fs::create_dir_all(clone.path().join("users")).unwrap();
    fs::write(
        clone.path().join("users/alice.meta.yaml"),
        "display_name: Alice\nrole: human\nintroduction: Owner\n",
    )
    .unwrap();
    commit_all(clone.path(), "seed", ALICE);
    git(clone.path(), &["push", "-u", "origin", "main"]);
    let storage = GitStorage::new(clone.path());
    let initial = validate_incoming_skill_history(
        &storage,
        &SkillValidationCheckpoint::empty("main"),
        &git_output(clone.path(), &["rev-parse", "HEAD"]),
    )
    .unwrap()
    .checkpoint;
    let before = gitim_core::skill::SkillRepositorySnapshot {
        active_users: BTreeSet::from([ALICE.to_owned()]),
        ..Default::default()
    };
    let bootstrap = bootstrap_plan(&before, 'N');
    apply_plan(clone.path(), &bootstrap);
    commit_all(clone.path(), &bootstrap.commit_message, ALICE);
    git(clone.path(), &["push"]);
    let lagging = validate_incoming_skill_history(
        &storage,
        &initial,
        &git_output(clone.path(), &["rev-parse", "HEAD"]),
    )
    .unwrap()
    .checkpoint;
    let create = create_plan(&bootstrap.after, 'P');
    apply_plan(clone.path(), &create);
    commit_all(clone.path(), &create.commit_message, ALICE);
    git(clone.path(), &["push"]);
    let archive = tempfile::tempdir().unwrap();
    let outcome = try_fire_rotation(
        &storage,
        "main",
        1,
        archive.path(),
        (ALICE, "alice@example.com"),
        NOW,
    )
    .unwrap();
    let orphan_commit_sha = match outcome {
        RotationOutcome::Won {
            orphan_commit_sha, ..
        } => orphan_commit_sha,
        _ => return Err("rotation did not win".into()),
    };

    let accepted = validate_incoming_skill_history(&storage, &lagging, &orphan_commit_sha).unwrap();

    assert_eq!(accepted.checkpoint.active_epoch, "main-epoch-2");
    assert!(accepted.checkpoint.conflicts.is_empty());
    assert_eq!(accepted.accepted_changes.len(), 1);
    assert_eq!(
        accepted.checkpoint.skills["release-check"].tree.commit_oid,
        orphan_commit_sha
    );
    git(
        clone.path(),
        &["update-ref", "-d", "refs/remotes/origin/main"],
    );
    let unavailable = validate_incoming_skill_history(
        &storage,
        &SkillValidationCheckpoint::empty("main"),
        &orphan_commit_sha,
    )
    .unwrap_err();
    assert_eq!(unavailable.code(), "skill_epoch_validation_blocked");
    Ok(())
}

#[test]
fn guarded_push_quarantines_bypassed_skill_history_and_replays_every_ordinary_delta() {
    let fixture = RemoteRepository::new();
    let storage = GitStorage::new(fixture.clone_root());
    let guard = SkillSyncGuard::new(fixture.clone_root()).unwrap();

    fs::write(
        fixture.writer_root().join("channels/general.meta.yaml"),
        "display_name: General\ncreated_by: alice\ncreated_at: 20260730T120000Z\nintroduction: General\nmembers:\n- alice\n- bob\n",
    )
    .unwrap();
    commit_all(fixture.writer_root(), "remote channel membership", "bob");
    git(fixture.writer_root(), &["push"]);

    fs::create_dir_all(fixture.clone_root().join("skills/poison")).unwrap();
    fs::write(
        fixture.clone_root().join("skills/poison/SKILL.md"),
        "# unvalidated\n",
    )
    .unwrap();
    fs::write(
        fixture.clone_root().join("channels/general.meta.yaml"),
        "display_name: General\ncreated_by: alice\ncreated_at: 20260730T120000Z\nintroduction: General\nmembers:\n- alice\n- carol\n",
    )
    .unwrap();
    fs::create_dir_all(fixture.clone_root().join("channels")).unwrap();
    fs::write(
        fixture.clone_root().join("channels/general.thread"),
        "[L000001][P000000][@alice][20260730T120000Z] keep message\n",
    )
    .unwrap();
    commit_all(fixture.clone_root(), "bypass skill plus message", ALICE);

    fs::create_dir_all(fixture.clone_root().join("channels/general/cards/C1")).unwrap();
    fs::write(
        fixture
            .clone_root()
            .join("channels/general/cards/C1/card.meta.yaml"),
        "title: Keep card\n",
    )
    .unwrap();
    commit_all(fixture.clone_root(), "card after bypass", ALICE);
    let bypassed_head = git_output(fixture.clone_root(), &["rev-parse", "HEAD"]);

    let outcome = guard
        .guarded_push(&storage, &Mutex::new(()), ("alice", "alice@example.com"))
        .unwrap();
    assert!(
        matches!(outcome, GuardedPushOutcome::RepairedAndPushed { .. }),
        "expected repaired push, got {outcome:?}"
    );
    let GuardedPushOutcome::RepairedAndPushed { quarantine_ref } = outcome else {
        return;
    };

    assert_eq!(
        git_output(
            fixture.clone_root(),
            &["rev-parse", quarantine_ref.as_str()]
        ),
        bypassed_head
    );
    assert_eq!(
        git_output(
            fixture.clone_root(),
            &["rev-parse", "refs/remotes/origin/main"]
        ),
        git_output(fixture.clone_root(), &["rev-parse", "HEAD"])
    );
    assert!(
        git_status(
            fixture.clone_root(),
            &[
                "cat-file",
                "-e",
                "refs/remotes/origin/main:skills/poison/SKILL.md"
            ]
        )
        .is_none(),
        "the bypassed Skill tree must not reach origin"
    );
    assert_eq!(
        git_output(
            fixture.clone_root(),
            &["show", "refs/remotes/origin/main:channels/general.thread"]
        ),
        "[L000001][P000000][@alice][20260730T120000Z] keep message"
    );
    assert_eq!(
        git_output(
            fixture.clone_root(),
            &[
                "show",
                "refs/remotes/origin/main:channels/general/cards/C1/card.meta.yaml"
            ]
        ),
        "title: Keep card"
    );
    let merged_meta: gitim_core::types::ChannelMeta = serde_yaml::from_str(&git_output(
        fixture.clone_root(),
        &[
            "show",
            "refs/remotes/origin/main:channels/general.meta.yaml",
        ],
    ))
    .unwrap();
    assert_eq!(merged_meta.members, vec!["alice", "bob", "carol"]);
}

#[test]
fn guarded_push_quarantines_transient_skill_touches_hidden_by_the_final_tree() {
    let fixture = RemoteRepository::new();
    let storage = GitStorage::new(fixture.clone_root());
    let guard = SkillSyncGuard::new(fixture.clone_root()).unwrap();

    fs::create_dir_all(fixture.clone_root().join("skills/transient")).unwrap();
    fs::write(
        fixture.clone_root().join("skills/transient/SKILL.md"),
        "# invalid transient Skill\n",
    )
    .unwrap();
    commit_all(fixture.clone_root(), "invalid transient Skill add", ALICE);
    fs::remove_dir_all(fixture.clone_root().join("skills/transient")).unwrap();
    commit_all(
        fixture.clone_root(),
        "hide invalid Skill by deleting it",
        ALICE,
    );
    fs::write(
        fixture.clone_root().join("ordinary.txt"),
        "ordinary delta\n",
    )
    .unwrap();
    commit_all(
        fixture.clone_root(),
        "ordinary after transient bypass",
        ALICE,
    );

    let outcome = guard
        .guarded_push(&storage, &Mutex::new(()), (ALICE, "alice@example.com"))
        .unwrap();

    assert!(matches!(
        outcome,
        GuardedPushOutcome::RepairedAndPushed { .. }
    ));
    assert_eq!(
        git_output(
            fixture.clone_root(),
            &["show", "refs/remotes/origin/main:ordinary.txt"]
        ),
        "ordinary delta"
    );
    let published_messages = git_output(
        fixture.clone_root(),
        &["log", "--format=%s", "refs/remotes/origin/main"],
    );
    assert!(!published_messages.contains("invalid transient Skill add"));
    assert!(!published_messages.contains("hide invalid Skill by deleting it"));
}

#[test]
fn guarded_push_publishes_first_ordinary_history_to_an_empty_remote() {
    let directory = tempfile::tempdir().unwrap();
    let remote = directory.path().join("origin.git");
    git(
        directory.path(),
        &["init", "--bare", remote.to_str().unwrap()],
    );
    let clone = directory.path().join("clone");
    git(
        directory.path(),
        &["init", "-b", "main", clone.to_str().unwrap()],
    );
    git(&clone, &["config", "user.name", ALICE]);
    git(&clone, &["config", "user.email", "alice@example.com"]);
    git(
        &clone,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    fs::create_dir_all(clone.join("users")).unwrap();
    fs::write(
        clone.join("users/alice.meta.yaml"),
        "display_name: Alice\nrole: human\nintroduction: Owner\n",
    )
    .unwrap();
    commit_all(&clone, "seed", ALICE);

    let guard = SkillSyncGuard::new(&clone).unwrap();
    let outcome = guard
        .guarded_push(
            &GitStorage::new(&clone),
            &Mutex::new(()),
            ("alice", "alice@example.com"),
        )
        .unwrap();

    assert_eq!(outcome, GuardedPushOutcome::Pushed);
    assert_eq!(
        git_output(&clone, &["rev-parse", "HEAD"]),
        git_output(&remote, &["rev-parse", "refs/heads/main"])
    );
}

#[test]
fn quarantine_resume_accepts_a_branch_already_moved_to_the_repaired_head() {
    let fixture = RemoteRepository::new();
    let storage = GitStorage::new(fixture.clone_root());
    let guard = SkillSyncGuard::new(fixture.clone_root()).unwrap();
    let upstream = git_output(fixture.clone_root(), &["rev-parse", "origin/main"]);

    fs::create_dir_all(fixture.clone_root().join("skills/poison")).unwrap();
    fs::write(
        fixture.clone_root().join("skills/poison/SKILL.md"),
        "# invalid\n",
    )
    .unwrap();
    fs::write(fixture.clone_root().join("ordinary.txt"), "preserved\n").unwrap();
    commit_all(fixture.clone_root(), "mixed bypass", ALICE);
    let original = git_output(fixture.clone_root(), &["rev-parse", "HEAD"]);
    let quarantine_ref = format!("refs/gitim/quarantine/skill-{original}");
    git(
        fixture.clone_root(),
        &["update-ref", &quarantine_ref, &original],
    );

    git(fixture.clone_root(), &["reset", "--hard", &upstream]);
    fs::write(fixture.clone_root().join("ordinary.txt"), "preserved\n").unwrap();
    commit_all(fixture.clone_root(), "sanitized ordinary replay", ALICE);
    let repaired = git_output(fixture.clone_root(), &["rev-parse", "HEAD"]);
    fs::create_dir_all(fixture.clone_root().join(".gitim")).unwrap();
    fs::write(
        fixture.clone_root().join(".gitim/skill-quarantine.json"),
        serde_json::json!({
            "schema_version": 1,
            "operation_id": original,
            "branch": "main",
            "upstream_oid": upstream,
            "original_head": original,
            "quarantine_ref": quarantine_ref,
            "phase": "replayed",
            "repaired_head": repaired
        })
        .to_string(),
    )
    .unwrap();

    let outcome = guard
        .guarded_push(&storage, &Mutex::new(()), (ALICE, "alice@example.com"))
        .unwrap();

    assert!(matches!(
        outcome,
        GuardedPushOutcome::RepairedAndPushed { .. }
    ));
    assert!(!fixture
        .clone_root()
        .join(".gitim/skill-quarantine.json")
        .exists());
    assert_eq!(
        git_output(
            fixture.clone_root(),
            &["show", "refs/remotes/origin/main:ordinary.txt"]
        ),
        "preserved"
    );
}

#[test]
fn quarantine_resume_replays_the_original_ref_after_remote_advance_and_thread_conflict() {
    let fixture = RemoteRepository::new();
    let storage = GitStorage::new(fixture.clone_root());
    let guard = SkillSyncGuard::new(fixture.clone_root()).unwrap();
    let base_thread = "[L000001][P000000][@alice][20260730T120000Z] base\n";

    fs::write(
        fixture.writer_root().join("channels/general.thread"),
        base_thread,
    )
    .unwrap();
    commit_all(fixture.writer_root(), "seed shared thread", "bob");
    git(fixture.writer_root(), &["push"]);
    git(fixture.clone_root(), &["fetch", "origin"]);
    git(fixture.clone_root(), &["rebase", "origin/main"]);

    fs::create_dir_all(fixture.clone_root().join("skills/poison")).unwrap();
    fs::write(
        fixture.clone_root().join("skills/poison/SKILL.md"),
        "# invalid\n",
    )
    .unwrap();
    fs::write(
        fixture.clone_root().join("channels/general.thread"),
        format!("{base_thread}[L000002][P000000][@alice][20260730T120100Z] local survives\n"),
    )
    .unwrap();
    commit_all(fixture.clone_root(), "local mixed bypass", ALICE);
    let original = git_output(fixture.clone_root(), &["rev-parse", "HEAD"]);
    let quarantine_ref = format!("refs/gitim/quarantine/skill-{original}");

    fs::write(
        fixture.writer_root().join("channels/general.thread"),
        format!("{base_thread}[L000002][P000000][@bob][20260730T120200Z] remote survives\n"),
    )
    .unwrap();
    commit_all(fixture.writer_root(), "concurrent remote message", "bob");

    let hook = fixture.clone_root().join(".git/hooks/pre-push");
    let marker = fixture.clone_root().join(".gitim/remote-advanced");
    fs::create_dir_all(marker.parent().unwrap()).unwrap();
    fs::write(
        &hook,
        format!(
            "#!/bin/sh\nif [ ! -f '{}' ]; then\n  touch '{}'\n  git -C '{}' push origin HEAD:main\nfi\n",
            marker.display(),
            marker.display(),
            fixture.writer_root().display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions).unwrap();
    }

    let first = guard.guarded_push(&storage, &Mutex::new(()), (ALICE, "alice@example.com"));
    assert!(matches!(
        first,
        Err(gitim_sync::skill::checkpoint::SkillSyncError::Git(
            gitim_sync::git::GitError::PushConflict
        ))
    ));
    assert!(fixture
        .clone_root()
        .join(".gitim/skill-quarantine.json")
        .exists());
    fs::remove_file(&hook).unwrap();

    let outcome = guard
        .guarded_push(&storage, &Mutex::new(()), (ALICE, "alice@example.com"))
        .unwrap();

    assert!(matches!(
        outcome,
        GuardedPushOutcome::RepairedAndPushed { .. }
    ));
    assert_eq!(
        git_output(fixture.clone_root(), &["rev-parse", &quarantine_ref]),
        original
    );
    assert!(!fixture
        .clone_root()
        .join(".gitim/skill-quarantine.json")
        .exists());
    let published = git_output(
        fixture.clone_root(),
        &["show", "refs/remotes/origin/main:channels/general.thread"],
    );
    assert!(published.contains("local survives"));
    assert!(published.contains("remote survives"));
    assert!(git_status(
        fixture.clone_root(),
        &[
            "cat-file",
            "-e",
            "refs/remotes/origin/main:skills/poison/SKILL.md"
        ]
    )
    .is_none());
}

#[test]
fn unresolved_quarantine_blocks_destructive_integration_and_rotation() {
    let fixture = RemoteRepository::new();
    let storage = GitStorage::new(fixture.clone_root());
    let guard = SkillSyncGuard::new(fixture.clone_root()).unwrap();
    let head = git_output(fixture.clone_root(), &["rev-parse", "HEAD"]);
    let branch = git_output(fixture.clone_root(), &["branch", "--show-current"]);
    let quarantine_ref = format!("refs/gitim/quarantine/skill-{head}");
    fs::create_dir_all(fixture.clone_root().join(".gitim")).unwrap();
    fs::write(
        fixture.clone_root().join(".gitim/skill-quarantine.json"),
        serde_json::json!({
            "schema_version": 1,
            "operation_id": head,
            "branch": branch,
            "upstream_oid": head,
            "original_head": head,
            "quarantine_ref": quarantine_ref,
            "phase": "prepared"
        })
        .to_string(),
    )
    .unwrap();

    let before = git_output(fixture.clone_root(), &["rev-parse", "HEAD"]);
    let error = guard
        .guarded_integrate(
            &storage,
            &Mutex::new(()),
            IntegrationOperation::HardDivergenceRecovery,
        )
        .unwrap_err();
    assert_eq!(error.code(), "skill_local_quarantine_blocked");
    assert_eq!(
        git_output(fixture.clone_root(), &["rev-parse", "HEAD"]),
        before
    );
    assert_eq!(
        guard.rotation_allowed(&storage).unwrap_err().code(),
        "skill_local_quarantine_blocked"
    );
}

#[test]
fn guarded_integrate_rejects_invalid_fetched_skill_history_before_branch_movement() {
    let fixture = RemoteRepository::new();
    let guard = SkillSyncGuard::new(fixture.clone_root()).unwrap();
    let local = GitStorage::new(fixture.clone_root());
    let before = git_output(fixture.clone_root(), &["rev-parse", "HEAD"]);

    fs::create_dir_all(fixture.writer_root().join("skills/invalid")).unwrap();
    fs::write(
        fixture.writer_root().join("skills/invalid/SKILL.md"),
        "# invalid transition without receipt\n",
    )
    .unwrap();
    commit_all(fixture.writer_root(), "invalid remote skill bypass", "bob");
    git(fixture.writer_root(), &["push", "origin", "HEAD:main"]);

    let error = guard
        .guarded_integrate(
            &local,
            &Mutex::new(()),
            IntegrationOperation::RebaseOntoOrigin,
        )
        .unwrap_err();
    assert_eq!(error.code(), "skill_sync_conflict");
    assert_eq!(
        git_output(fixture.clone_root(), &["rev-parse", "HEAD"]),
        before
    );
}

#[test]
fn guarded_integrate_rejects_a_remote_ref_move_between_validation_and_local_rewrite() {
    let fixture = RemoteRepository::new();
    let local_root = fixture.clone_root().to_path_buf();
    let before = git_output(&local_root, &["rev-parse", "HEAD"]);

    fs::write(fixture.writer_root().join("remote-first.txt"), "first\n").unwrap();
    commit_all(fixture.writer_root(), "first remote advance", "bob");
    git(fixture.writer_root(), &["push"]);
    let first_remote = git_output(fixture.writer_root(), &["rev-parse", "HEAD"]);

    let commit_lock = Arc::new(Mutex::new(()));
    let held = commit_lock.lock().unwrap();
    let worker_lock = commit_lock.clone();
    let worker_root = local_root.clone();
    let worker = std::thread::spawn(move || {
        SkillSyncGuard::new(&worker_root)
            .unwrap()
            .guarded_integrate(
                &GitStorage::new(&worker_root),
                &worker_lock,
                IntegrationOperation::RebaseOntoOrigin,
            )
    });

    let checkpoint = local_root.join(".gitim/skill-validation.json");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if fs::read_to_string(&checkpoint).is_ok_and(|contents| contents.contains(&first_remote)) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        fs::read_to_string(&checkpoint)
            .unwrap_or_default()
            .contains(&first_remote),
        "integration did not finish validating the captured remote tip"
    );

    fs::write(fixture.writer_root().join("remote-second.txt"), "second\n").unwrap();
    commit_all(fixture.writer_root(), "second remote advance", "bob");
    git(fixture.writer_root(), &["push"]);
    GitStorage::new(&local_root).fetch().unwrap();
    drop(held);

    let error = worker.join().unwrap().unwrap_err();
    assert!(matches!(
        error,
        gitim_sync::skill::checkpoint::SkillSyncError::Git(gitim_sync::git::GitError::PushConflict)
    ));
    assert_eq!(git_output(&local_root, &["rev-parse", "HEAD"]), before);
}

#[test]
fn cleanup_failed_fire_does_not_reset_when_the_validated_remote_ref_moves() {
    let fixture = RemoteRepository::new();
    let local_root = fixture.clone_root().to_path_buf();

    fs::write(fixture.writer_root().join("remote-first.txt"), "first\n").unwrap();
    commit_all(fixture.writer_root(), "first cleanup target", "bob");
    git(fixture.writer_root(), &["push"]);
    let first_remote = git_output(fixture.writer_root(), &["rev-parse", "HEAD"]);

    fs::write(local_root.join("gitim.epoch.yaml"), "status: redirected\n").unwrap();
    commit_all(&local_root, "seal: redirect local losing rotation", ALICE);
    let sealed_local = git_output(&local_root, &["rev-parse", "HEAD"]);
    git(&local_root, &["branch", "main-epoch-2", &sealed_local]);

    let commit_lock = Arc::new(Mutex::new(()));
    let held = commit_lock.lock().unwrap();
    let worker_lock = commit_lock.clone();
    let worker_root = local_root.clone();
    let worker = std::thread::spawn(move || {
        SkillSyncGuard::new(&worker_root)
            .unwrap()
            .guarded_integrate(
                &GitStorage::new(&worker_root),
                &worker_lock,
                IntegrationOperation::CleanupFailedFire {
                    orphan_branch: "main-epoch-2".to_owned(),
                },
            )
    });

    let checkpoint = local_root.join(".gitim/skill-validation.json");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if fs::read_to_string(&checkpoint).is_ok_and(|contents| contents.contains(&first_remote)) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(fs::read_to_string(&checkpoint)
        .unwrap_or_default()
        .contains(&first_remote));

    fs::write(fixture.writer_root().join("remote-second.txt"), "second\n").unwrap();
    commit_all(fixture.writer_root(), "second cleanup target", "bob");
    git(fixture.writer_root(), &["push"]);
    GitStorage::new(&local_root).fetch().unwrap();
    drop(held);

    let error = worker.join().unwrap().unwrap_err();
    assert!(matches!(
        error,
        gitim_sync::skill::checkpoint::SkillSyncError::Git(gitim_sync::git::GitError::PushConflict)
    ));
    assert_eq!(
        git_output(&local_root, &["rev-parse", "HEAD"]),
        sealed_local
    );
    assert!(git_status(
        &local_root,
        &["show-ref", "--verify", "refs/heads/main-epoch-2"]
    )
    .is_some());
}

#[test]
fn user_archive_semantic_precondition_rejects_a_concurrent_workspace_admin_bootstrap() {
    let fixture = RemoteRepository::new();
    let storage = GitStorage::new(fixture.clone_root());
    let guard = SkillSyncGuard::new(fixture.clone_root()).unwrap();

    fs::create_dir_all(fixture.clone_root().join("archive/users")).unwrap();
    git(
        fixture.clone_root(),
        &[
            "mv",
            "users/alice.meta.yaml",
            "archive/users/alice.meta.yaml",
        ],
    );
    commit_all(
        fixture.clone_root(),
        "archive: depart user @alice\n\nGitim-Skills-Tree: absent",
        ALICE,
    );
    let archive_head = git_output(fixture.clone_root(), &["rev-parse", "HEAD"]);

    let before = SkillRepositorySnapshot {
        active_users: BTreeSet::from([ALICE.to_owned()]),
        ..Default::default()
    };
    let bootstrap = bootstrap_plan(&before, 'R');
    apply_plan(fixture.writer_root(), &bootstrap);
    commit_all(fixture.writer_root(), &bootstrap.commit_message, ALICE);
    git(fixture.writer_root(), &["push", "origin", "HEAD:main"]);

    let error = guard
        .guarded_push(&storage, &Mutex::new(()), ("alice", "alice@example.com"))
        .unwrap_err();
    assert!(matches!(
        error,
        gitim_sync::skill::checkpoint::SkillSyncError::Git(gitim_sync::git::GitError::PushConflict)
    ));
    assert_eq!(
        git_output(fixture.clone_root(), &["rev-parse", "HEAD"]),
        archive_head,
        "the archive branch must remain available for a semantic retry"
    );
    assert!(
        git_status(
            fixture.clone_root(),
            &[
                "cat-file",
                "-e",
                "refs/remotes/origin/main:users/alice.meta.yaml"
            ]
        )
        .is_some(),
        "the winning administrator bootstrap keeps the target user active"
    );
}

#[test]
fn sync_initial_push_publishes_ordinary_work_through_quarantine_replay() {
    let fixture = RemoteRepository::new();
    let storage = GitStorage::new(fixture.clone_root());

    fs::create_dir_all(fixture.clone_root().join("skills/invalid")).unwrap();
    fs::write(
        fixture.clone_root().join("skills/invalid/SKILL.md"),
        "# bypass\n",
    )
    .unwrap();
    commit_all(fixture.clone_root(), "invalid Skill bypass", ALICE);
    fs::write(
        fixture.clone_root().join("channels/general.thread"),
        "[L000001][P000000][@alice][20260730T120000Z] sync ordinary\n",
    )
    .unwrap();
    commit_all(fixture.clone_root(), "sync ordinary message", ALICE);

    let mut circuit = AuthCircuit::new(Arc::new(AtomicBool::new(false)));
    run_sync_cycle(
        &storage,
        &mut circuit,
        &Mutex::new(()),
        &|_, _| {},
        &|_, _, _| {},
        &|_| {},
        &|| {},
        Some(&(ALICE.to_owned(), "alice@example.com".to_owned())),
    );

    assert_eq!(
        git_output(
            fixture.clone_root(),
            &["show", "refs/remotes/origin/main:channels/general.thread"]
        ),
        "[L000001][P000000][@alice][20260730T120000Z] sync ordinary"
    );
    assert!(git_status(
        fixture.clone_root(),
        &[
            "cat-file",
            "-e",
            "refs/remotes/origin/main:skills/invalid/SKILL.md"
        ]
    )
    .is_none());
    assert!(git_output(
        fixture.clone_root(),
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/gitim/quarantine"
        ]
    )
    .lines()
    .any(|reference| reference.starts_with("refs/gitim/quarantine/skill-")));
}

#[test]
fn sync_loop_routes_destructive_epoch_fallback_through_the_skill_guard() {
    let source = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("sync_loop.rs"),
    )
    .unwrap();
    assert!(
        !source.contains("repo.discard_unpushed()"),
        "sync loop must not discard unpushed commits outside SkillSyncGuard"
    );
    assert!(
        !source.contains("crate::rotate::follow_redirect(repo"),
        "sync loop must not follow epoch redirects outside SkillSyncGuard"
    );
    for guarded_path in [
        "guard.guarded_push(repo, commit_lock",
        "IntegrationOperation::RebaseOntoOrigin",
        "IntegrationOperation::HardDivergenceRecovery",
        "IntegrationOperation::FollowEpochRedirect",
        "IntegrationOperation::FollowEpochRedirectAfterDiscard",
    ] {
        assert!(
            source.contains(guarded_path),
            "sync path is missing guarded transition {guarded_path}"
        );
    }
    assert!(
        source
            .matches("guard.guarded_push(repo, commit_lock")
            .count()
            >= 3,
        "initial, post-rebase, and post-resolve pushes must all use the guard"
    );

    let rotate_source = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("rotate.rs"),
    )
    .unwrap();
    assert!(
        rotate_source.contains("IntegrationOperation::CleanupFailedFire"),
        "failed-fire cleanup must use the guarded exact-ref operation"
    );
    assert!(
        !rotate_source.contains(".reset_branch_to_origin("),
        "failed-fire cleanup must not reset through a symbolic remote ref"
    );
}

struct RemoteRepository {
    _directory: TempDir,
    clone_root: std::path::PathBuf,
    writer_root: std::path::PathBuf,
}

impl RemoteRepository {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let remote = directory.path().join("origin.git");
        git(
            directory.path(),
            &["init", "--bare", remote.to_str().unwrap()],
        );
        let writer_root = directory.path().join("writer");
        git(
            directory.path(),
            &[
                "clone",
                remote.to_str().unwrap(),
                writer_root.to_str().unwrap(),
            ],
        );
        git(&writer_root, &["config", "user.name", "bob"]);
        git(&writer_root, &["config", "user.email", "bob@example.com"]);
        fs::create_dir_all(writer_root.join("users")).unwrap();
        fs::write(
            writer_root.join("users/alice.meta.yaml"),
            "display_name: Alice\nrole: human\nintroduction: Owner\n",
        )
        .unwrap();
        fs::create_dir_all(writer_root.join("channels")).unwrap();
        fs::write(
            writer_root.join("channels/general.meta.yaml"),
            "display_name: General\ncreated_by: alice\ncreated_at: 20260730T120000Z\nintroduction: General\nmembers:\n- alice\n",
        )
        .unwrap();
        commit_all(&writer_root, "seed", "alice");
        git(&writer_root, &["branch", "-M", "main"]);
        git(&writer_root, &["push", "-u", "origin", "main"]);
        git(&remote, &["symbolic-ref", "HEAD", "refs/heads/main"]);

        let clone_root = directory.path().join("clone");
        git(
            directory.path(),
            &[
                "clone",
                remote.to_str().unwrap(),
                clone_root.to_str().unwrap(),
            ],
        );
        git(&clone_root, &["config", "user.name", ALICE]);
        git(&clone_root, &["config", "user.email", "alice@example.com"]);
        Self {
            _directory: directory,
            clone_root,
            writer_root,
        }
    }

    fn clone_root(&self) -> &Path {
        &self.clone_root
    }

    fn writer_root(&self) -> &Path {
        &self.writer_root
    }
}

fn git_status(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).unwrap().trim().to_owned())
}
