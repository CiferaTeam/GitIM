use std::path::Path;
use std::process::Command;

use gitim_sync::git::{GitError, GitStorage};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

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

fn setup_git_config(dir: &Path, name: &str, email: &str) {
    run_git(dir, &["config", "user.email", email]);
    run_git(dir, &["config", "user.name", name]);
}

/// Create a bare repo + two clones (a and b), with an initial commit pushed to main.
/// Returns (bare_dir, clone_a_dir, clone_b_dir).
fn setup_two_clones_with_initial_commit() -> (TempDir, TempDir, TempDir) {
    let bare_dir = TempDir::new().unwrap();
    let clone_a_dir = TempDir::new().unwrap();
    let clone_b_dir = TempDir::new().unwrap();

    run_git(bare_dir.path(), &["init", "--bare"]);

    // Clone A
    run_git(
        clone_a_dir.path().parent().unwrap(),
        &[
            "clone",
            bare_dir.path().to_str().unwrap(),
            clone_a_dir.path().to_str().unwrap(),
        ],
    );
    setup_git_config(clone_a_dir.path(), "Alice", "alice@test.com");

    // Initial commit from Clone A
    std::fs::write(clone_a_dir.path().join("init.txt"), "init").unwrap();
    run_git(clone_a_dir.path(), &["add", "init.txt"]);
    run_git(clone_a_dir.path(), &["commit", "-m", "initial"]);
    run_git(clone_a_dir.path(), &["push", "-u", "origin", "HEAD"]);

    // Clone B
    run_git(
        clone_b_dir.path().parent().unwrap(),
        &[
            "clone",
            bare_dir.path().to_str().unwrap(),
            clone_b_dir.path().to_str().unwrap(),
        ],
    );
    setup_git_config(clone_b_dir.path(), "Bob", "bob@test.com");

    (bare_dir, clone_a_dir, clone_b_dir)
}

/// Simulate the ensure_repo file creation: .gitignore + channels/general.meta.yaml +
/// channels/general.thread. Returns the list of relative paths that were written.
fn write_ensure_repo_files(root: &Path) -> Vec<String> {
    // .gitignore
    let gitignore_path = root.join(".gitignore");
    std::fs::write(&gitignore_path, ".gitim/\n").unwrap();

    // channels/
    let channels_dir = root.join("channels");
    std::fs::create_dir_all(&channels_dir).unwrap();

    let meta_path = channels_dir.join("general.meta.yaml");
    let meta = "display_name: General\ncreated_by: alice\ncreated_at: 20260321T000000Z\nintroduction: 默认频道\n";
    std::fs::write(&meta_path, meta).unwrap();

    let thread_path = channels_dir.join("general.thread");
    std::fs::write(&thread_path, "").unwrap();

    vec![
        ".gitignore".to_string(),
        "channels/general.meta.yaml".to_string(),
        "channels/general.thread".to_string(),
    ]
}

// ---------------------------------------------------------------------------
// Test 1: EnsureRepo push conflict → discard → clean state
// ---------------------------------------------------------------------------

/// Scenario: two clones both try to initialize the repo structure (ensure_repo).
/// Clone A succeeds. Clone B gets PushConflict and discards its unpushed commit.
/// Verifies both that origin retains Clone A's state and that Clone B's working
/// tree is clean (no unpushed commits, no leftover rebase state) after discard.
#[test]
fn test_discard_unpushed_recovers_from_conflict() {
    let (bare_dir, clone_a_dir, clone_b_dir) = setup_two_clones_with_initial_commit();

    // ---- Clone A: write ensure_repo files, commit, push (succeeds) ----
    let repo_a = GitStorage::new(clone_a_dir.path());
    let paths_a = write_ensure_repo_files(clone_a_dir.path());
    let path_refs_a: Vec<&str> = paths_a.iter().map(|s| s.as_str()).collect();
    repo_a
        .add_and_commit(
            &path_refs_a,
            "init: repo structure (.gitignore + general channel)",
        )
        .unwrap();
    repo_a.push().expect("Clone A push should succeed");

    // ---- Clone B: write same ensure_repo files (without pulling), commit ----
    let repo_b = GitStorage::new(clone_b_dir.path());
    let paths_b = write_ensure_repo_files(clone_b_dir.path());
    let path_refs_b: Vec<&str> = paths_b.iter().map(|s| s.as_str()).collect();
    repo_b
        .add_and_commit(
            &path_refs_b,
            "init: repo structure (.gitignore + general channel)",
        )
        .unwrap();

    // Clone B has an unpushed commit at this point
    assert!(
        repo_b.has_unpushed_commits().unwrap(),
        "Clone B should have an unpushed commit before push attempt"
    );

    // ---- Clone B: push → expect PushConflict ----
    let push_result = repo_b.push();
    assert!(
        matches!(push_result, Err(GitError::PushConflict)),
        "Clone B push should fail with PushConflict, got: {:?}",
        push_result
    );

    // ---- Clone B: discard_unpushed → should succeed ----
    repo_b
        .discard_unpushed()
        .expect("discard_unpushed should succeed after PushConflict");

    // ---- Verify: Clone B no longer has unpushed commits ----
    assert!(
        !repo_b.has_unpushed_commits().unwrap(),
        "Clone B should have no unpushed commits after discard"
    );

    // ---- Verify: no leftover rebase state in Clone B ----
    let rebase_merge = clone_b_dir.path().join(".git/rebase-merge");
    let rebase_apply = clone_b_dir.path().join(".git/rebase-apply");
    assert!(
        !rebase_merge.exists() && !rebase_apply.exists(),
        "no rebase state should remain after discard_unpushed"
    );

    // ---- Verify: origin has Clone A's commit (the general channel files exist) ----
    let verify_clone = TempDir::new().unwrap();
    run_git(
        verify_clone.path().parent().unwrap(),
        &[
            "clone",
            bare_dir.path().to_str().unwrap(),
            verify_clone.path().to_str().unwrap(),
        ],
    );
    assert!(
        verify_clone.path().join(".gitignore").exists(),
        "origin should have .gitignore from Clone A"
    );
    assert!(
        verify_clone
            .path()
            .join("channels/general.meta.yaml")
            .exists(),
        "origin should have general.meta.yaml from Clone A"
    );

    let gitignore_content =
        std::fs::read_to_string(verify_clone.path().join(".gitignore")).unwrap();
    assert!(
        gitignore_content.contains(".gitim/"),
        "origin .gitignore should contain .gitim/"
    );
}

// ---------------------------------------------------------------------------
// Test 2: RegisterUser concurrent → PushConflict → fetch + rebase → push succeeds
// ---------------------------------------------------------------------------

/// Scenario: two clones register different users simultaneously.
/// Clone A (alice) pushes first. Clone B (bob) gets PushConflict, then does
/// fetch + rebase_onto_origin + retry push. Verifies both that origin contains
/// both user files with correct content and that Clone B has no unpushed commits
/// after the successful retry push.
#[test]
fn test_concurrent_registration_rebase_succeeds() {
    let (bare_dir, clone_a_dir, clone_b_dir) = setup_two_clones_with_initial_commit();

    // ---- Bootstrap: push initial repo structure from Clone A ----
    let repo_a = GitStorage::new(clone_a_dir.path());
    let init_paths = write_ensure_repo_files(clone_a_dir.path());
    let init_refs: Vec<&str> = init_paths.iter().map(|s| s.as_str()).collect();
    repo_a
        .add_and_commit(&init_refs, "init: repo structure")
        .unwrap();
    repo_a.push().unwrap();

    // Clone B: pull the initial structure
    run_git(clone_b_dir.path(), &["pull"]);

    // ---- Clone A: register alice ----
    let users_dir_a = clone_a_dir.path().join("users");
    std::fs::create_dir_all(&users_dir_a).unwrap();
    std::fs::write(
        users_dir_a.join("alice.meta.yaml"),
        "display_name: Alice\nrole: member\nintroduction: GitIM user\n",
    )
    .unwrap();
    repo_a
        .add_and_commit(&["users/alice.meta.yaml"], "user: register @alice")
        .unwrap();
    repo_a.push().expect("Clone A (alice) push should succeed");

    // ---- Clone B: register bob (without pulling — simulates concurrent registration) ----
    let repo_b = GitStorage::new(clone_b_dir.path());
    let users_dir_b = clone_b_dir.path().join("users");
    std::fs::create_dir_all(&users_dir_b).unwrap();
    std::fs::write(
        users_dir_b.join("bob.meta.yaml"),
        "display_name: Bob\nrole: member\nintroduction: GitIM user\n",
    )
    .unwrap();
    repo_b
        .add_and_commit(&["users/bob.meta.yaml"], "user: register @bob")
        .unwrap();

    // ---- Clone B: push → PushConflict ----
    let push_result = repo_b.push();
    assert!(
        matches!(push_result, Err(GitError::PushConflict)),
        "Clone B push should fail with PushConflict, got: {:?}",
        push_result
    );

    // ---- Clone B: fetch + rebase_onto_origin ----
    repo_b.fetch().expect("fetch should succeed");
    repo_b
        .rebase_onto_origin()
        .expect("rebase should succeed (different files — no conflict)");

    // ---- Clone B: retry push → should succeed ----
    repo_b
        .push()
        .expect("Clone B push should succeed after rebase");

    // ---- Verify: Clone B has no unpushed commits after successful push ----
    assert!(
        !repo_b.has_unpushed_commits().unwrap(),
        "no unpushed commits should remain after successful push"
    );

    // ---- Verify: origin has both user files ----
    let verify_clone = TempDir::new().unwrap();
    run_git(
        verify_clone.path().parent().unwrap(),
        &[
            "clone",
            bare_dir.path().to_str().unwrap(),
            verify_clone.path().to_str().unwrap(),
        ],
    );
    assert!(
        verify_clone.path().join("users/alice.meta.yaml").exists(),
        "origin should have alice.meta.yaml"
    );
    assert!(
        verify_clone.path().join("users/bob.meta.yaml").exists(),
        "origin should have bob.meta.yaml"
    );

    // Verify content of both user files
    let alice_content: serde_yaml::Value = serde_yaml::from_str(
        &std::fs::read_to_string(verify_clone.path().join("users/alice.meta.yaml")).unwrap(),
    )
    .unwrap();
    assert_eq!(alice_content["display_name"], "Alice");

    let bob_content: serde_yaml::Value = serde_yaml::from_str(
        &std::fs::read_to_string(verify_clone.path().join("users/bob.meta.yaml")).unwrap(),
    )
    .unwrap();
    assert_eq!(bob_content["display_name"], "Bob");
}
