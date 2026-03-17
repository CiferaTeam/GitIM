use std::path::Path;
use std::process::Command;

use gitim_sync::git::GitRepo;
use tempfile::TempDir;

/// Helper: create a bare repo and clone it, returning (bare_dir, clone_dir, GitRepo).
/// Both TempDirs are returned so they stay alive for the test's lifetime.
fn setup_repo_pair() -> (TempDir, TempDir, GitRepo) {
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

    let repo = GitRepo::new(clone_dir.path());
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
fn test_diff_unpushed_thread_additions() {
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

    let diff = repo.diff_unpushed_thread_additions().unwrap();
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
fn test_diff_unpushed_ignores_non_thread_files() {
    let (_bare_dir, clone_dir, repo) = setup_repo_pair();

    // Commit a non-thread file (should not appear in diff)
    std::fs::write(clone_dir.path().join("notes.txt"), "some notes").unwrap();
    run_git(clone_dir.path(), &["add", "."]);
    run_git(clone_dir.path(), &["commit", "-m", "add notes"]);

    let diff = repo.diff_unpushed_thread_additions().unwrap();
    assert!(diff.is_empty());
}

#[test]
fn test_rebase_abort_no_error_without_rebase() {
    let (_bare_dir, _clone_dir, repo) = setup_repo_pair();

    // Calling rebase_abort when no rebase is in progress should not error
    repo.rebase_abort().expect("rebase_abort should not error");
}

#[test]
fn test_reset_hard_origin() {
    let (_bare_dir, clone_dir, repo) = setup_repo_pair();

    // Create a local commit
    std::fs::write(clone_dir.path().join("local.txt"), "data").unwrap();
    run_git(clone_dir.path(), &["add", "local.txt"]);
    run_git(clone_dir.path(), &["commit", "-m", "local"]);

    // Verify the file exists
    assert!(clone_dir.path().join("local.txt").exists());

    // Reset hard to origin/main
    repo.reset_hard_origin().expect("reset should succeed");

    // The local commit (and its file) should be gone
    assert!(!clone_dir.path().join("local.txt").exists());
}
