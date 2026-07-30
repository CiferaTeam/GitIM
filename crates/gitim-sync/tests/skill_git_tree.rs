#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use gitim_core::skill::{RequestId, SkillTreeEdit};
use gitim_sync::git::{GitError, GitStorage};
use gitim_sync::skill::git_tree::{
    build_private_index_commit, list_tree_recursive, push_commit_fast_forward, read_blob_at,
    tree_oid_at, PrivateIndexCommitRequest,
};
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
        .expect("failed to run git");
    assert!(
        output.status.success(),
        "git failed in {}: {}",
        root.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn git_stdout<I, S>(root: &Path, args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    String::from_utf8(git(root, args).stdout)
        .unwrap()
        .trim()
        .to_owned()
}

struct Fixture {
    remote: TempDir,
    clone: TempDir,
    repo: GitStorage,
    base: String,
}

fn fixture() -> Fixture {
    let remote = TempDir::new().unwrap();
    git(remote.path(), ["init", "--bare", "-b", "main"]);

    let clone = TempDir::new().unwrap();
    git(
        clone.path(),
        [
            OsStr::new("clone"),
            remote.path().as_os_str(),
            OsStr::new("."),
        ],
    );
    git(clone.path(), ["config", "user.name", "Local User"]);
    git(clone.path(), ["config", "user.email", "local@example.com"]);
    git(clone.path(), ["config", "commit.gpgsign", "false"]);
    git(clone.path(), ["config", "push.autoSetupRemote", "false"]);

    fs::create_dir_all(clone.path().join("bin")).unwrap();
    fs::create_dir_all(clone.path().join("skills/release-check")).unwrap();
    fs::write(clone.path().join("tracked.txt"), b"base tracked\n").unwrap();
    fs::write(clone.path().join("delete.txt"), b"delete me\n").unwrap();
    fs::write(clone.path().join("bin/keep-exec"), b"#!/bin/sh\nexit 0\n").unwrap();
    fs::write(
        clone.path().join("skills/release-check/old.txt"),
        b"base skill\n",
    )
    .unwrap();
    git(clone.path(), ["add", "--", "."]);
    git(
        clone.path(),
        ["update-index", "--chmod=+x", "--", "bin/keep-exec"],
    );
    git(clone.path(), ["commit", "-m", "base"]);
    git(clone.path(), ["push", "-u", "origin", "main"]);
    let base = git_stdout(clone.path(), ["rev-parse", "HEAD"]);

    fs::write(clone.path().join("head-only.txt"), b"active head only\n").unwrap();
    git(clone.path(), ["add", "--", "head-only.txt"]);
    git(clone.path(), ["commit", "-m", "active checkout advances"]);

    Fixture {
        remote,
        repo: GitStorage::new(clone.path()),
        clone,
        base,
    }
}

fn request(base: &str, private_index: &Path, marker: &str) -> PrivateIndexCommitRequest {
    PrivateIndexCommitRequest {
        base_commit: base.to_owned(),
        private_index: private_index.to_path_buf(),
        edits: vec![
            SkillTreeEdit::Upsert {
                path: "skills/release-check/SKILL.md".to_owned(),
                bytes: format!("# Release check\n\n{marker}\n").into_bytes(),
            },
            SkillTreeEdit::Upsert {
                path: "bin/new-regular".to_owned(),
                bytes: b"regular blob\n".to_vec(),
            },
            SkillTreeEdit::Delete {
                path: "delete.txt".to_owned(),
            },
        ],
        message: format!("skill: create release-check {marker}"),
        author_name: "alice".to_owned(),
        author_email: "alice@example.com".to_owned(),
        request_id: RequestId::new("q-01K1D8QG2S8RX4T9M9BDKQ9Z7N").unwrap(),
    }
}

#[test]
fn private_index_commit_uses_explicit_base_and_preserves_checkout_state() {
    let fixture = fixture();

    fs::write(
        fixture.clone.path().join("tracked.txt"),
        b"staged checkout bytes\n",
    )
    .unwrap();
    git(fixture.clone.path(), ["add", "--", "tracked.txt"]);
    fs::write(
        fixture.clone.path().join("tracked.txt"),
        b"dirty worktree bytes\n",
    )
    .unwrap();

    let head_before = git_stdout(fixture.clone.path(), ["rev-parse", "HEAD"]);
    let index_path = fixture.clone.path().join(".git/index");
    let index_before = fs::read(&index_path).unwrap();
    let staged_before = git(fixture.clone.path(), ["diff", "--cached", "--binary", "--"]).stdout;
    let worktree_before = fs::read(fixture.clone.path().join("tracked.txt")).unwrap();
    let branch_config_before = git(
        fixture.clone.path(),
        ["config", "--local", "--get-regexp", "^branch\\."],
    )
    .stdout;

    let transaction = TempDir::new().unwrap();
    let built = build_private_index_commit(
        &fixture.repo,
        &request(
            &fixture.base,
            &transaction.path().join("private-index"),
            "first",
        ),
    )
    .unwrap();

    assert_eq!(
        git_stdout(
            fixture.clone.path(),
            ["show", "-s", "--format=%P", &built.commit_oid]
        ),
        fixture.base
    );
    assert_eq!(
        git_stdout(
            fixture.clone.path(),
            ["show", "-s", "--format=%T", &built.commit_oid]
        ),
        built.tree_oid
    );
    assert_eq!(
        read_blob_at(
            &fixture.repo,
            &built.commit_oid,
            "skills/release-check/SKILL.md"
        )
        .unwrap(),
        Some(b"# Release check\n\nfirst\n".to_vec())
    );
    assert_eq!(
        read_blob_at(&fixture.repo, &built.commit_oid, "tracked.txt").unwrap(),
        Some(b"base tracked\n".to_vec()),
        "candidate tree must come from the requested base, not HEAD or the active index"
    );
    assert_eq!(
        read_blob_at(&fixture.repo, &built.commit_oid, "head-only.txt").unwrap(),
        None
    );
    assert_eq!(
        read_blob_at(&fixture.repo, &built.commit_oid, "delete.txt").unwrap(),
        None
    );

    let entries =
        list_tree_recursive(&fixture.repo, &built.commit_oid, "bin").expect("list candidate tree");
    let keep = entries
        .iter()
        .find(|entry| entry.path == "bin/keep-exec")
        .unwrap();
    let added = entries
        .iter()
        .find(|entry| entry.path == "bin/new-regular")
        .unwrap();
    assert_eq!(keep.mode, "100755");
    assert_eq!(added.mode, "100644");
    assert_eq!(added.object_type, "blob");
    assert_eq!(
        tree_oid_at(&fixture.repo, &built.commit_oid, "bin/new-regular").unwrap(),
        Some(added.oid.clone())
    );
    assert!(tree_oid_at(&fixture.repo, &built.commit_oid, "missing")
        .unwrap()
        .is_none());

    let commit_metadata = git_stdout(
        fixture.clone.path(),
        [
            "show",
            "-s",
            "--format=%an%n%ae%n%cn%n%ce%n%B",
            &built.commit_oid,
        ],
    );
    assert_eq!(
        commit_metadata
            .lines()
            .take(4)
            .collect::<Vec<_>>()
            .as_slice(),
        ["alice", "alice@example.com", "alice", "alice@example.com"]
    );
    assert_eq!(
        commit_metadata
            .lines()
            .filter(|line| line.starts_with("Gitim-Request-Id: "))
            .collect::<Vec<_>>(),
        ["Gitim-Request-Id: q-01K1D8QG2S8RX4T9M9BDKQ9Z7N"]
    );
    let commit_message = git(
        fixture.clone.path(),
        ["show", "-s", "--format=%B", &built.commit_oid],
    )
    .stdout;
    let message_file = transaction.path().join("commit-message");
    fs::write(&message_file, commit_message).unwrap();
    assert_eq!(
        git_stdout(
            fixture.clone.path(),
            [
                OsStr::new("interpret-trailers"),
                OsStr::new("--parse"),
                message_file.as_os_str()
            ]
        ),
        "Gitim-Request-Id: q-01K1D8QG2S8RX4T9M9BDKQ9Z7N"
    );

    assert_eq!(
        git_stdout(fixture.clone.path(), ["rev-parse", "HEAD"]),
        head_before
    );
    assert_eq!(fs::read(index_path).unwrap(), index_before);
    assert_eq!(
        git(fixture.clone.path(), ["diff", "--cached", "--binary", "--"]).stdout,
        staged_before
    );
    assert_eq!(
        fs::read(fixture.clone.path().join("tracked.txt")).unwrap(),
        worktree_before
    );
    assert_eq!(
        git(
            fixture.clone.path(),
            ["config", "--local", "--get-regexp", "^branch\\."]
        )
        .stdout,
        branch_config_before
    );
}

#[test]
fn explicit_commit_push_preserves_local_branch_config_and_classifies_stale_base() {
    let fixture = fixture();
    let branch_config_before = git(
        fixture.clone.path(),
        ["config", "--local", "--get-regexp", "^branch\\."],
    )
    .stdout;
    let head_before = git_stdout(fixture.clone.path(), ["rev-parse", "HEAD"]);

    let first_dir = TempDir::new().unwrap();
    let first = build_private_index_commit(
        &fixture.repo,
        &request(
            &fixture.base,
            &first_dir.path().join("private-index"),
            "winner",
        ),
    )
    .unwrap();
    let stale_dir = TempDir::new().unwrap();
    let stale = build_private_index_commit(
        &fixture.repo,
        &request(
            &fixture.base,
            &stale_dir.path().join("private-index"),
            "stale",
        ),
    )
    .unwrap();

    push_commit_fast_forward(&fixture.repo, &first.commit_oid, "main").unwrap();
    assert_eq!(
        git_stdout(fixture.remote.path(), ["rev-parse", "refs/heads/main"]),
        first.commit_oid
    );
    assert!(matches!(
        push_commit_fast_forward(&fixture.repo, &stale.commit_oid, "main"),
        Err(GitError::PushConflict)
    ));

    assert_eq!(
        git_stdout(fixture.clone.path(), ["rev-parse", "HEAD"]),
        head_before
    );
    assert_eq!(
        git(
            fixture.clone.path(),
            ["config", "--local", "--get-regexp", "^branch\\."]
        )
        .stdout,
        branch_config_before
    );
}

#[test]
fn plumbing_rejects_option_like_revisions_refs_and_active_index() {
    let fixture = fixture();
    let transaction = TempDir::new().unwrap();
    let mut invalid_base = request(
        "--help",
        &transaction.path().join("private-index"),
        "invalid",
    );
    assert!(build_private_index_commit(&fixture.repo, &invalid_base).is_err());
    assert!(tree_oid_at(&fixture.repo, "--help", "tracked.txt").is_err());
    assert!(push_commit_fast_forward(&fixture.repo, "--help", "main").is_err());

    invalid_base.base_commit = fixture.base.clone();
    invalid_base.private_index = fixture.clone.path().join(".git/index");
    assert!(build_private_index_commit(&fixture.repo, &invalid_base).is_err());

    let hard_linked_index = transaction.path().join("hard-linked-index");
    let active_index = fixture.clone.path().join(".git/index");
    let active_index_before = fs::read(&active_index).unwrap();
    fs::hard_link(&active_index, &hard_linked_index).unwrap();
    let hard_link_request = request(&fixture.base, &hard_linked_index, "hard-link");
    build_private_index_commit(&fixture.repo, &hard_link_request).unwrap();
    assert_eq!(
        fs::read(active_index).unwrap(),
        active_index_before,
        "rebuilding a pre-existing private index must not follow a hard link to the active index"
    );

    let built = build_private_index_commit(
        &fixture.repo,
        &request(
            &fixture.base,
            &transaction.path().join("second-index"),
            "valid",
        ),
    )
    .unwrap();
    assert!(push_commit_fast_forward(&fixture.repo, &built.commit_oid, "--mirror").is_err());
}

#[test]
fn commit_message_rejects_semantic_request_id_trailer_variants() {
    let fixture = fixture();
    for (position, trailer) in [
        "gitim-request-id: q-01K1D8QG2S8RX4T9M9BDKQ9Z7P",
        "GITIM-REQUEST-ID : q-01K1D8QG2S8RX4T9M9BDKQ9Z7Q",
        "  Gitim-Request-Id\t: q-01K1D8QG2S8RX4T9M9BDKQ9Z7R",
    ]
    .iter()
    .enumerate()
    {
        let transaction = TempDir::new().unwrap();
        let mut candidate = request(
            &fixture.base,
            &transaction.path().join(format!("private-index-{position}")),
            "duplicate-trailer",
        );
        candidate.message = format!("skill: duplicate trailer\n\n{trailer}");
        assert!(
            build_private_index_commit(&fixture.repo, &candidate).is_err(),
            "message containing {trailer:?} must be rejected"
        );
    }
}
