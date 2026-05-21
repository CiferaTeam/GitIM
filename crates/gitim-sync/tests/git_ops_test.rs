#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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
    let channels = clone_dir.path().join("channels");
    std::fs::create_dir_all(&channels).unwrap();
    let thread_file = channels.join("general.thread");
    std::fs::write(
        &thread_file,
        "[L000001][P000000][@alice][20250316T120000Z] hello\n",
    )
    .unwrap();
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

    let key = diff.keys().next().unwrap();
    assert!(key.to_str().unwrap().ends_with("general.thread"));

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
    repo.discard_unpushed()
        .expect("discard_unpushed should not error");
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

// ── diff_range tests ──────────────────────────────────────────

#[test]
fn test_diff_range_returns_added_lines() {
    let (_bare_dir, clone_dir, repo) = setup_repo_pair();

    // Create a .thread file, commit and push
    let channels = clone_dir.path().join("channels");
    std::fs::create_dir_all(&channels).unwrap();
    let thread_file = channels.join("general.thread");
    std::fs::write(
        &thread_file,
        "[L000001][P000000][@alice][20250316T120000Z] hello\n",
    )
    .unwrap();
    run_git(clone_dir.path(), &["add", "."]);
    run_git(clone_dir.path(), &["commit", "-m", "add thread"]);
    run_git(clone_dir.path(), &["push"]);

    // Record the commit hash before adding more lines
    let before = repo.rev_parse("HEAD").unwrap();

    // Append a new line, commit
    let mut content = std::fs::read_to_string(&thread_file).unwrap();
    content.push_str("[L000002][P000001][@bob][20250316T120100Z] reply\n");
    std::fs::write(&thread_file, &content).unwrap();
    run_git(clone_dir.path(), &["add", "."]);
    run_git(clone_dir.path(), &["commit", "-m", "add reply"]);

    let after = repo.rev_parse("HEAD").unwrap();

    let diff = repo.diff_range(&before, &after).unwrap();
    assert_eq!(diff.len(), 1);

    let key = diff.keys().next().unwrap();
    assert!(key.to_str().unwrap().ends_with("general.thread"));

    let added = diff.values().next().unwrap();
    assert!(added.contains("[L000002]"));
    assert!(added.contains("reply"));
    // Should NOT contain the original line
    assert!(!added.contains("[L000001]"));
}

#[test]
fn test_diff_range_empty_when_no_changes() {
    let (_bare_dir, _clone_dir, repo) = setup_repo_pair();

    let head = repo.rev_parse("HEAD").unwrap();

    // Same commit on both sides → empty diff
    let diff = repo.diff_range(&head, &head).unwrap();
    assert!(diff.is_empty());
}

#[test]
fn test_changed_files_range_detects_deletion_only_change() {
    let (_bare_dir, clone_dir, repo) = setup_repo_pair();

    let showboards = clone_dir.path().join("showboards/alice");
    std::fs::create_dir_all(&showboards).unwrap();
    let board_file = showboards.join("board.md");
    std::fs::write(
        &board_file,
        "---\nversion: 1\nhandler: alice\nupdated_at: 20260509T120000Z\nstatus: idle\nsummary: ''\ntags: []\n---\n## 当前状态\n\n## 待确认\n",
    )
    .unwrap();
    run_git(clone_dir.path(), &["add", "."]);
    run_git(clone_dir.path(), &["commit", "-m", "add board"]);

    let before = repo.rev_parse("HEAD").unwrap();
    let content = std::fs::read_to_string(&board_file).unwrap();
    let modified = content.replace("## 待确认\n", "");
    assert_ne!(modified, content);
    std::fs::write(&board_file, modified).unwrap();
    run_git(clone_dir.path(), &["add", "."]);
    run_git(clone_dir.path(), &["commit", "-m", "delete board line"]);
    let after = repo.rev_parse("HEAD").unwrap();

    let diff = repo.diff_range(&before, &after).unwrap();
    assert!(
        diff.is_empty(),
        "added-line diff should miss deletion-only board edits"
    );

    let changed = repo.changed_files_range(&before, &after).unwrap();
    assert_eq!(
        changed,
        vec![std::path::PathBuf::from("showboards/alice/board.md")]
    );
}

#[test]
fn test_rev_parse_returns_commit_hash() {
    let (_bare_dir, _clone_dir, repo) = setup_repo_pair();

    let hash = repo.rev_parse("HEAD").unwrap();
    assert_eq!(hash.len(), 40, "SHA should be 40 hex chars");
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn test_rev_parse_upstream() {
    let (_bare_dir, _clone_dir, repo) = setup_repo_pair();

    let hash = repo.rev_parse("@{upstream}").unwrap();
    assert_eq!(hash.len(), 40, "SHA should be 40 hex chars");
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn test_diff_range_invalid_commit() {
    let (_bare_dir, _clone_dir, repo) = setup_repo_pair();

    let result = repo.diff_range("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef", "HEAD");
    assert!(
        result.is_err(),
        "diff_range with invalid commit should error"
    );
}

#[test]
fn test_changed_files_unpushed_detects_meta() {
    let (_bare_dir, clone_dir, repo) = setup_repo_pair();

    let ch_dir = clone_dir.path().join("channels");
    std::fs::create_dir_all(&ch_dir).unwrap();
    std::fs::write(ch_dir.join("general.meta.yaml"), "display_name: General\n").unwrap();
    run_git(clone_dir.path(), &["add", "."]);
    run_git(clone_dir.path(), &["commit", "-m", "add meta"]);

    let changed = repo.changed_files_unpushed("*.meta.yaml").unwrap();
    assert_eq!(changed.len(), 1);
    assert!(changed[0].to_str().unwrap().contains("general.meta.yaml"));
}

#[test]
fn test_changed_files_unpushed_empty_when_pushed() {
    let (_bare_dir, clone_dir, repo) = setup_repo_pair();

    let ch_dir = clone_dir.path().join("channels");
    std::fs::create_dir_all(&ch_dir).unwrap();
    std::fs::write(ch_dir.join("general.meta.yaml"), "display_name: General\n").unwrap();
    run_git(clone_dir.path(), &["add", "."]);
    run_git(clone_dir.path(), &["commit", "-m", "add meta"]);
    repo.push().unwrap();

    let changed = repo.changed_files_unpushed("*.meta.yaml").unwrap();
    assert!(changed.is_empty());
}

#[test]
fn count_commits_on_branch_returns_total_reachable_count() {
    use gitim_sync::git::GitStorage;
    use std::process::Command;

    let dir = tempfile::TempDir::new().unwrap();
    Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "t@t"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "t"])
        .current_dir(dir.path())
        .status()
        .unwrap();

    // Make 3 commits.
    for i in 0..3 {
        std::fs::write(dir.path().join(format!("f{i}")), "x").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .status()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", &format!("c{i}")])
            .current_dir(dir.path())
            .status()
            .unwrap();
    }

    let storage = GitStorage::new(dir.path());
    let n = storage.count_commits_on_branch("main").expect("count");
    assert_eq!(n, 3);
}

#[test]
fn count_commits_on_branch_errs_for_missing_branch() {
    use gitim_sync::git::GitStorage;
    use std::process::Command;

    let dir = tempfile::TempDir::new().unwrap();
    Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(dir.path())
        .status()
        .unwrap();

    let storage = GitStorage::new(dir.path());
    // Empty branch is not yet born — count should error (chosen contract).
    let res = storage.count_commits_on_branch("nonexistent");
    assert!(
        res.is_err(),
        "missing branch must surface error, got {:?}",
        res
    );
}

#[test]
fn create_orphan_commit_produces_root_commit_on_new_branch() {
    use gitim_sync::git::GitStorage;
    use std::process::Command;

    let dir = tempfile::TempDir::new().unwrap();
    Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "t@t"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "t"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(dir.path())
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(dir.path())
        .status()
        .unwrap();

    let storage = GitStorage::new(dir.path());
    let sha = storage
        .create_orphan_commit(
            "main-epoch-2",
            "gitim.epoch.yaml",
            "schema_version: 1\nstatus: active\nepoch: 2\nbranch: main-epoch-2\n",
            "snapshot: epoch 2 from main",
            ("daemon", "daemon@gitim"),
        )
        .expect("orphan");
    assert!(!sha.is_empty(), "orphan commit must return sha");

    // Verify branch exists and is a root commit (no parents).
    let parents = std::process::Command::new("git")
        .args(["rev-list", "--parents", "-n", "1", "main-epoch-2"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let parents_str = String::from_utf8_lossy(&parents.stdout);
    let parts: Vec<&str> = parents_str.split_whitespace().collect();
    assert_eq!(
        parts.len(),
        1,
        "orphan must have zero parents, got {:?}",
        parts
    );

    // Verify the new file is in the orphan tree AND existing working tree files are too.
    let ls = std::process::Command::new("git")
        .args(["ls-tree", "-r", "--name-only", "main-epoch-2"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let names: Vec<String> = String::from_utf8_lossy(&ls.stdout)
        .lines()
        .map(|s| s.to_string())
        .collect();
    assert!(names.contains(&"gitim.epoch.yaml".to_string()));
    assert!(
        names.contains(&"a.txt".to_string()),
        "orphan must include existing working tree files, got {:?}",
        names
    );
}

#[test]
fn write_redirect_commit_appends_to_current_branch() {
    use gitim_sync::git::GitStorage;
    use std::process::Command;

    let dir = tempfile::TempDir::new().unwrap();
    Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "t@t"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "t"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(dir.path())
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(dir.path())
        .status()
        .unwrap();

    let storage = GitStorage::new(dir.path());
    let sha = storage
        .write_redirect_commit(
            "gitim.epoch.yaml",
            "schema_version: 1\nstatus: redirected\nepoch: 1\nbranch: main\n",
            "redirect: seal epoch 1",
            ("daemon", "daemon@gitim"),
        )
        .expect("redirect");
    assert!(!sha.is_empty());

    // Verify main is one commit ahead of the previous tip.
    let log = Command::new("git")
        .args(["log", "--format=%H", "main"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let commits: Vec<&str> = std::str::from_utf8(&log.stdout)
        .unwrap()
        .trim()
        .lines()
        .collect();
    assert_eq!(
        commits.len(),
        2,
        "expected 2 commits on main, got {:?}",
        commits
    );
    assert_eq!(commits[0], sha.trim());
}
