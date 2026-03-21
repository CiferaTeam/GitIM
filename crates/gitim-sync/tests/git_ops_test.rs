use std::path::Path;
use std::process::Command;

use gitim_sync::git::GitStorage;
use tempfile::TempDir;

/// Helper: create a bare repo and clone it, returning (bare_dir, clone_dir, GitStorage).
/// Both TempDirs are returned so they stay alive for the test's lifetime.
fn setup_repo_pair() -> (TempDir, TempDir, GitStorage) {
    let bare_dir = TempDir::new().unwrap();
    let clone_dir = TempDir::new().unwrap();

    // Init bare repo
    run_git(bare_dir.path(), &["init", "--bare"]);

    // Clone it
    run_git(
        clone_dir.path().parent().unwrap(),
        &[
            "clone",
            bare_dir.path().to_str().unwrap(),
            clone_dir.path().to_str().unwrap(),
        ],
    );

    // Configure user in clone (needed for commits)
    run_git(clone_dir.path(), &["config", "user.email", "test@test.com"]);
    run_git(clone_dir.path(), &["config", "user.name", "Test"]);

    // Create initial commit so main branch exists
    let init_file = clone_dir.path().join("init.txt");
    std::fs::write(&init_file, "init").unwrap();
    run_git(clone_dir.path(), &["add", "init.txt"]);
    run_git(clone_dir.path(), &["commit", "-m", "initial"]);
    run_git(clone_dir.path(), &["push", "-u", "origin", "main"]);

    let repo = GitStorage::new(clone_dir.path());
    (bare_dir, clone_dir, repo)
}

fn run_git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run git");
    if !output.status.success() {
        panic!(
            "git {:?} failed in {}: {}",
            args,
            dir.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn test_fetch_succeeds_with_remote() {
    let (bare_dir, _clone_dir, repo) = setup_repo_pair();

    // Add a new commit to the bare repo via a second clone
    let second_clone = TempDir::new().unwrap();
    run_git(
        second_clone.path().parent().unwrap(),
        &[
            "clone",
            bare_dir.path().to_str().unwrap(),
            second_clone.path().to_str().unwrap(),
        ],
    );
    run_git(second_clone.path(), &["config", "user.email", "b@b.com"]);
    run_git(second_clone.path(), &["config", "user.name", "B"]);
    std::fs::write(second_clone.path().join("new.txt"), "data").unwrap();
    run_git(second_clone.path(), &["add", "new.txt"]);
    run_git(second_clone.path(), &["commit", "-m", "remote commit"]);
    run_git(second_clone.path(), &["push"]);

    // Fetch in the original clone should succeed
    repo.fetch().expect("fetch should succeed");
}

#[test]
fn test_has_unpushed_commits() {
    let (_bare_dir, clone_dir, repo) = setup_repo_pair();

    // Initially no unpushed commits
    assert!(!repo.has_unpushed_commits().unwrap());

    // Create a local commit
    std::fs::write(clone_dir.path().join("local.txt"), "local").unwrap();
    run_git(clone_dir.path(), &["add", "local.txt"]);
    run_git(clone_dir.path(), &["commit", "-m", "local commit"]);

    // Now we have unpushed commits
    assert!(repo.has_unpushed_commits().unwrap());

    // Push and verify
    run_git(clone_dir.path(), &["push"]);
    assert!(!repo.has_unpushed_commits().unwrap());
}

#[test]
fn test_diff_unpushed() {
    let (_bare_dir, clone_dir, repo) = setup_repo_pair();

    // Create a channels directory and a .thread file
    let channels = clone_dir.path().join("channels").join("general");
    std::fs::create_dir_all(&channels).unwrap();
    let thread_file = channels.join("main.thread");
    std::fs::write(&thread_file, "[L000001][P000000][@alice][20250316T120000Z] hello\n").unwrap();
    run_git(clone_dir.path(), &["add", "."]);
    run_git(clone_dir.path(), &["commit", "-m", "add thread"]);
    run_git(clone_dir.path(), &["push"]);

    // Now append a new line (unpushed)
    let mut content = std::fs::read_to_string(&thread_file).unwrap();
    content.push_str("[L000002][P000001][@bob][20250316T120100Z] reply\n");
    std::fs::write(&thread_file, &content).unwrap();
    run_git(clone_dir.path(), &["add", "."]);
    run_git(clone_dir.path(), &["commit", "-m", "add reply"]);

    let diff = repo.diff_unpushed("*.thread").unwrap();
    assert_eq!(diff.len(), 1);

    let key = diff
        .keys()
        .next()
        .unwrap();
    assert!(key.to_str().unwrap().ends_with("main.thread"));

    let added = diff.values().next().unwrap();
    assert!(added.contains("[L000002]"));
    assert!(added.contains("reply"));
    // Should NOT contain the original line (it wasn't added in this diff)
    assert!(!added.contains("[L000001]"));
}

#[test]
fn test_diff_unpushed_ignores_non_matching_pattern() {
    let (_bare_dir, clone_dir, repo) = setup_repo_pair();

    // Commit a non-thread file (should not appear in diff with *.thread pattern)
    std::fs::write(clone_dir.path().join("notes.txt"), "some notes").unwrap();
    run_git(clone_dir.path(), &["add", "."]);
    run_git(clone_dir.path(), &["commit", "-m", "add notes"]);

    let diff = repo.diff_unpushed("*.thread").unwrap();
    assert!(diff.is_empty());
}

#[test]
fn test_discard_unpushed_no_error_without_changes() {
    let (_bare_dir, _clone_dir, repo) = setup_repo_pair();

    // Calling discard_unpushed when no divergence should not error
    repo.discard_unpushed().expect("discard_unpushed should not error");
}

#[test]
fn test_pull_rebase_conflict_leaves_rebase_state_and_discard_recovers() {
    let (bare_dir, clone_dir, _repo) = setup_repo_pair();

    // Create second clone
    let clone_b_dir = TempDir::new().unwrap();
    run_git(
        clone_b_dir.path().parent().unwrap(),
        &[
            "clone",
            bare_dir.path().to_str().unwrap(),
            clone_b_dir.path().to_str().unwrap(),
        ],
    );
    run_git(clone_b_dir.path(), &["config", "user.email", "b@test.com"]);
    run_git(clone_b_dir.path(), &["config", "user.name", "B"]);

    // Clone A: modify init.txt and push
    std::fs::write(clone_dir.path().join("init.txt"), "A's version").unwrap();
    run_git(clone_dir.path(), &["add", "init.txt"]);
    run_git(clone_dir.path(), &["commit", "-m", "A change"]);
    run_git(clone_dir.path(), &["push"]);

    // Clone B: modify init.txt conflictingly, commit locally
    std::fs::write(clone_b_dir.path().join("init.txt"), "B's version").unwrap();
    run_git(clone_b_dir.path(), &["add", "init.txt"]);
    run_git(clone_b_dir.path(), &["commit", "-m", "B change"]);

    let repo_b = GitStorage::new(clone_b_dir.path());

    // pull_rebase fails due to conflict
    let result = repo_b.pull_rebase();
    assert!(result.is_err(), "pull_rebase should fail due to conflict");

    // BUG: repo is stuck in rebase state
    let rebase_merge = clone_b_dir.path().join(".git/rebase-merge");
    let rebase_apply = clone_b_dir.path().join(".git/rebase-apply");
    assert!(
        rebase_merge.exists() || rebase_apply.exists(),
        "repo should be in rebase state after failed pull_rebase"
    );

    // FIX: discard_unpushed recovers from the rebase state
    repo_b
        .discard_unpushed()
        .expect("discard should recover from rebase state");

    // Verify: repo is clean
    assert!(
        !rebase_merge.exists() && !rebase_apply.exists(),
        "repo should be clean after discard_unpushed"
    );
}

#[test]
fn test_discard_unpushed_resets_to_origin() {
    let (_bare_dir, clone_dir, repo) = setup_repo_pair();

    // Create a local commit
    std::fs::write(clone_dir.path().join("local.txt"), "data").unwrap();
    run_git(clone_dir.path(), &["add", "local.txt"]);
    run_git(clone_dir.path(), &["commit", "-m", "local"]);

    // Verify the file exists
    assert!(clone_dir.path().join("local.txt").exists());

    // Discard unpushed changes
    repo.discard_unpushed().expect("discard should succeed");

    // The local commit (and its file) should be gone
    assert!(!clone_dir.path().join("local.txt").exists());
}
