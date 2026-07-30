#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::process::Command;

use gitim_core::epoch::EpochFile;
use gitim_core::skill::{
    plan_skill_mutation, validate_package_entries, PackageEntry, RequestId,
    SkillConflictCheckpoint, SkillCreateRequest, SkillMutationContext, SkillMutationPlan,
    SkillMutationRequest, SkillRepairAcceptedState, SkillRepairRequest, SkillRepairScope,
    SkillWorkspaceBootstrapRequest,
};
use gitim_sync::git::GitStorage;
use gitim_sync::rotate::{try_fire_rotation, RotationOutcome};
use gitim_sync::skill::checkpoint::{
    validate_incoming_skill_history, SkillCheckpointStore, SkillValidationCheckpoint,
};
use tempfile::TempDir;

const ALICE: &str = "alice";
const NOW: &str = "2026-07-30T10:00:00Z";

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
fn authorized_repair_restores_absence_after_malformed_first_bootstrap() {
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
    let accepted_tree = conflict.accepted_tree_oid.clone().unwrap();
    assert!(!accepted_tree.is_empty());

    let mut repair_before = bootstrap.after.clone();
    repair_before.workspace = Some(serde_yaml::from_str(&malformed_workspace).unwrap());
    repair_before
        .repository_files
        .insert(workspace_path.to_owned(), malformed_workspace.into_bytes());
    repair_before.conflict_checkpoint = Some(SkillConflictCheckpoint {
        conflict_tip: rejected_tip.clone(),
        accepted_tree: accepted_tree.clone(),
        accepted_state: SkillRepairAcceptedState::AbsentWorkspace,
        accepted_files: BTreeMap::new(),
        changed_paths: BTreeSet::from([workspace_path.to_owned()]),
    });
    let repair = plan_skill_mutation(
        &repair_before,
        &context(None),
        &SkillMutationRequest::Repair(SkillRepairRequest {
            request_id: request_id('5'),
            scope: SkillRepairScope::Workspace,
            conflict_tip: rejected_tip,
            accepted_tree,
        }),
    )
    .unwrap();
    let repair_tip = repository.commit_plan(&repair);

    let repaired =
        validate_incoming_skill_history(&repository.storage, &conflicted, &repair_tip).unwrap();

    assert!(repaired.checkpoint.conflicts.is_empty());
    assert!(repaired.checkpoint.workspace_tree.is_none());
    assert!(!repository.root().join(workspace_path).exists());
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

    let slug = gitim_core::skill::SkillSlug::new("release-check").unwrap();
    let mut repair_before = create.after.clone();
    repair_before
        .repository_files
        .insert(meta_path.to_owned(), malformed_meta.into_bytes());
    let changed_paths = repair_before
        .repository_files
        .keys()
        .filter(|path| path.starts_with("skills/release-check/"))
        .cloned()
        .collect();
    repair_before.conflict_checkpoint = Some(SkillConflictCheckpoint {
        conflict_tip: rejected_tip.clone(),
        accepted_tree: accepted_tree.clone(),
        accepted_state: SkillRepairAcceptedState::AbsentSkill { slug: slug.clone() },
        accepted_files: BTreeMap::new(),
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
    let repair_tip = repository.commit_plan(&repair);

    let repaired =
        validate_incoming_skill_history(&repository.storage, &conflicted, &repair_tip).unwrap();

    assert!(repaired.checkpoint.conflicts.is_empty());
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
    assert_eq!(
        fs::read_to_string(repository.root().join("ordinary.txt")).unwrap(),
        "preserve\n"
    );
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
