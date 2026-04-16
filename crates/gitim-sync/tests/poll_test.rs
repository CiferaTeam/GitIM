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
    run_git(clone_dir.path(), &["push", "-u", "origin", "HEAD"]);

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

// ── Test 1: poll workflow with no cursor ──────────────────────

#[test]
fn test_poll_workflow_no_cursor_returns_commit() {
    let (_bare_dir, _clone_dir, repo) = setup_repo_pair();

    // rev_parse("@{upstream}") should return a valid 40-char hex commit hash
    let cursor = repo.rev_parse("@{upstream}").unwrap();
    assert_eq!(cursor.len(), 40, "SHA should be 40 hex chars");
    assert!(cursor.chars().all(|c| c.is_ascii_hexdigit()));

    // diff_range from cursor to @{upstream} (same commit) should be empty
    let diff = repo.diff_range(&cursor, "@{upstream}").unwrap();
    assert!(diff.is_empty(), "diff from cursor to itself should be empty");
}

// ── Test 2: poll detects new message ──────────────────────────

#[test]
fn test_poll_workflow_detects_new_message() {
    let (_bare_dir, clone_dir, repo) = setup_repo_pair();

    // Record initial cursor (@{upstream} after setup)
    let old_cursor = repo.rev_parse("@{upstream}").unwrap();

    // Create a .thread file, commit and push
    let channels = clone_dir.path().join("channels");
    std::fs::create_dir_all(&channels).unwrap();
    let thread_file = channels.join("general.thread");
    std::fs::write(
        &thread_file,
        "[L000001][P000000][@alice][20260322T100000Z] hello world\n",
    )
    .unwrap();
    run_git(clone_dir.path(), &["add", "."]);
    run_git(clone_dir.path(), &["commit", "-m", "add message"]);
    run_git(clone_dir.path(), &["push"]);

    // Fetch so @{upstream} is updated
    repo.fetch().unwrap();

    // diff_range from old cursor to @{upstream} should find the new content
    let diff = repo.diff_range(&old_cursor, "@{upstream}").unwrap();
    assert!(!diff.is_empty(), "diff should detect the new .thread file");

    let key = diff.keys().next().unwrap();
    assert!(
        key.to_str().unwrap().ends_with("general.thread"),
        "diff key should be the thread file path"
    );
    let added = diff.values().next().unwrap();
    assert!(added.contains("hello world"), "diff should contain the message text");

    // New cursor should be different from old
    let new_cursor = repo.rev_parse("@{upstream}").unwrap();
    assert_ne!(old_cursor, new_cursor, "cursor should advance after push");

    // Re-polling with new cursor should return empty diff
    let re_diff = repo.diff_range(&new_cursor, "@{upstream}").unwrap();
    assert!(re_diff.is_empty(), "re-poll with new cursor should be empty");
}

// ── Test 3: poll detects multiple channels ────────────────────

#[test]
fn test_poll_workflow_multiple_channels() {
    let (_bare_dir, clone_dir, repo) = setup_repo_pair();

    let old_cursor = repo.rev_parse("@{upstream}").unwrap();

    // Create a channels/ .thread file
    let ch_dir = clone_dir.path().join("channels");
    std::fs::create_dir_all(&ch_dir).unwrap();
    std::fs::write(
        ch_dir.join("general.thread"),
        "[L000001][P000000][@alice][20260322T100000Z] channel msg\n",
    )
    .unwrap();

    // Create a dm/ .thread file
    let dm_dir = clone_dir.path().join("dm");
    std::fs::create_dir_all(&dm_dir).unwrap();
    std::fs::write(
        dm_dir.join("alice--bob.thread"),
        "[L000001][P000000][@alice][20260322T100100Z] dm msg\n",
    )
    .unwrap();

    run_git(clone_dir.path(), &["add", "."]);
    run_git(clone_dir.path(), &["commit", "-m", "add channel and dm"]);
    run_git(clone_dir.path(), &["push"]);

    repo.fetch().unwrap();

    let diff = repo.diff_range(&old_cursor, "@{upstream}").unwrap();
    assert_eq!(diff.len(), 2, "diff should find both channel and dm thread files");

    // Verify both paths are present
    let paths: Vec<String> = diff.keys().map(|p| p.to_str().unwrap().to_string()).collect();
    assert!(
        paths.iter().any(|p| p.contains("channels/")),
        "should contain channels/ path"
    );
    assert!(
        paths.iter().any(|p| p.contains("dm/")),
        "should contain dm/ path"
    );

    // Verify content
    for added in diff.values() {
        assert!(
            added.contains("channel msg") || added.contains("dm msg"),
            "each diff entry should contain message text"
        );
    }
}

// ── Test 4: poll without remote uses HEAD ─────────────────────

#[test]
fn test_poll_no_remote_uses_head() {
    let local_dir = TempDir::new().unwrap();

    // Init a plain repo (no remote)
    run_git(local_dir.path(), &["init"]);
    run_git(local_dir.path(), &["config", "user.email", "test@test.com"]);
    run_git(local_dir.path(), &["config", "user.name", "Test"]);

    // Create initial commit
    std::fs::write(local_dir.path().join("init.txt"), "init").unwrap();
    run_git(local_dir.path(), &["add", "init.txt"]);
    run_git(local_dir.path(), &["commit", "-m", "initial"]);

    let repo = GitStorage::new(local_dir.path());

    // has_remote() should be false
    assert!(!repo.has_remote(), "repo without origin should return false");

    // rev_parse("HEAD") should still work and return valid hash
    let head = repo.rev_parse("HEAD").unwrap();
    assert_eq!(head.len(), 40, "SHA should be 40 hex chars");
    assert!(head.chars().all(|c| c.is_ascii_hexdigit()));

    // Create a .thread file and commit
    let ch_dir = local_dir.path().join("channels");
    std::fs::create_dir_all(&ch_dir).unwrap();
    std::fs::write(
        ch_dir.join("dev.thread"),
        "[L000001][P000000][@alice][20260322T100000Z] local msg\n",
    )
    .unwrap();
    run_git(local_dir.path(), &["add", "."]);
    run_git(local_dir.path(), &["commit", "-m", "local thread"]);

    let new_head = repo.rev_parse("HEAD").unwrap();
    assert_ne!(head, new_head, "HEAD should advance after commit");

    // diff_range from old HEAD to new HEAD should detect the change
    let diff = repo.diff_range(&head, &new_head).unwrap();
    assert!(!diff.is_empty(), "diff should detect new .thread file");

    let key = diff.keys().next().unwrap();
    assert!(key.to_str().unwrap().ends_with("dev.thread"));
    let added = diff.values().next().unwrap();
    assert!(added.contains("local msg"));
}
