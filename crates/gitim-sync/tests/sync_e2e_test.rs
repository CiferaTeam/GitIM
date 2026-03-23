use std::path::{Path, PathBuf};
use std::process::Command;

use gitim_core::formatter::format_message;
use gitim_core::parser::parse_thread;
use gitim_core::types::Handler;
use gitim_sync::conflict::resolve_content;
use gitim_sync::git::GitStorage;
use tempfile::TempDir;

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

/// Create a bare repo + two clones with an initial commit on main
/// and an empty `channels/general.thread` file ready to use.
/// Returns (bare_dir, clone_a_dir, clone_b_dir).
fn setup_two_clones() -> (TempDir, TempDir, TempDir) {
    let bare_dir = TempDir::new().unwrap();
    let clone_a_dir = TempDir::new().unwrap();
    let clone_b_dir = TempDir::new().unwrap();

    // Init bare repo
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

    // Initial commit: create channels/general.thread (empty), push
    let channels_dir = clone_a_dir.path().join("channels");
    std::fs::create_dir_all(&channels_dir).unwrap();
    let thread_path = channels_dir.join("general.thread");
    std::fs::write(&thread_path, "").unwrap();
    run_git(clone_a_dir.path(), &["add", "."]);
    run_git(clone_a_dir.path(), &["commit", "-m", "initial: empty thread"]);
    run_git(clone_a_dir.path(), &["push", "-u", "origin", "main"]);

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

#[test]
fn test_sync_pushes_committed_messages() {
    let (_bare_dir, clone_a_dir, _clone_b_dir) = setup_two_clones();

    let repo = GitStorage::new(clone_a_dir.path());
    let thread_path = clone_a_dir.path().join("channels/general.thread");

    // Write a message using format_message
    let alice = Handler::new("alice").unwrap();
    let msg = format_message(1, 0, &alice, "20260317T100000Z", "hello world");
    std::fs::write(&thread_path, &msg).unwrap();

    // Commit
    repo.add_and_commit(&["channels/general.thread"], "msg: hello")
        .unwrap();

    // Verify: has unpushed commits
    assert!(
        repo.has_unpushed_commits().unwrap(),
        "should have unpushed commits after local commit"
    );

    // Push
    repo.push().expect("push should succeed");

    // Verify: no unpushed commits after push
    assert!(
        !repo.has_unpushed_commits().unwrap(),
        "should have no unpushed commits after push"
    );

    // Verify: the remote contains the message (clone_b can pull and see it)
    let verify_clone = TempDir::new().unwrap();
    run_git(
        verify_clone.path().parent().unwrap(),
        &[
            "clone",
            _bare_dir.path().to_str().unwrap(),
            verify_clone.path().to_str().unwrap(),
        ],
    );
    let remote_content =
        std::fs::read_to_string(verify_clone.path().join("channels/general.thread")).unwrap();
    let file = parse_thread(&remote_content).unwrap();
    assert_eq!(file.messages().len(), 1);
    assert_eq!(file.messages()[0].line_number, 1);
    assert_eq!(file.messages()[0].body, "hello world");
}

#[test]
fn test_sync_resolves_concurrent_writes() {
    let (_bare_dir, clone_a_dir, clone_b_dir) = setup_two_clones();

    let thread_rel = PathBuf::from("channels/general.thread");
    let thread_a = clone_a_dir.path().join(&thread_rel);
    let thread_b = clone_b_dir.path().join(&thread_rel);

    let alice = Handler::new("alice").unwrap();
    let bob = Handler::new("bob").unwrap();

    // Clone A: append L000001, L000002, commit, push
    let mut a_content = String::new();
    a_content.push_str(&format_message(1, 0, &alice, "20260317T100000Z", "alice msg 1"));
    a_content.push_str(&format_message(2, 1, &alice, "20260317T100100Z", "alice msg 2"));
    std::fs::write(&thread_a, &a_content).unwrap();
    let repo_a = GitStorage::new(clone_a_dir.path());
    repo_a
        .add_and_commit(&["channels/general.thread"], "alice messages")
        .unwrap();
    repo_a.push().unwrap();

    // Clone B: append L000001, L000002, commit (no push)
    let mut b_content = String::new();
    b_content.push_str(&format_message(1, 0, &bob, "20260317T100200Z", "bob msg 1"));
    b_content.push_str(&format_message(2, 1, &bob, "20260317T100300Z", "bob msg 2"));
    std::fs::write(&thread_b, &b_content).unwrap();
    let repo_b = GitStorage::new(clone_b_dir.path());
    repo_b
        .add_and_commit(&["channels/general.thread"], "bob messages")
        .unwrap();

    // Clone B: conflict resolution flow (mirrors SyncLoop logic)
    repo_b.fetch().unwrap();
    let local_additions = repo_b.diff_unpushed("*.thread").unwrap();
    assert!(!local_additions.is_empty(), "should have local additions");

    // SyncLoop manages git state: discard → resolve → write → commit
    repo_b.discard_unpushed().unwrap();
    let (resolved_files, _mappings) = resolve_content(&local_additions, repo_b.root()).unwrap();
    for resolved in &resolved_files {
        std::fs::write(repo_b.root().join(&resolved.path), &resolved.content).unwrap();
    }
    let paths: Vec<&str> = resolved_files.iter().map(|r| r.path.to_str().unwrap()).collect();
    repo_b.add_and_commit(&paths, "resolved").unwrap();

    // 5. Verify: thread_b has 4 messages with sequential L000001-L000004
    let final_content = std::fs::read_to_string(&thread_b).unwrap();
    let file = parse_thread(&final_content).unwrap();
    assert_eq!(file.messages().len(), 4, "should have 4 messages total");

    // First 2 are alice's (from remote)
    assert_eq!(file.messages()[0].line_number, 1);
    assert_eq!(file.messages()[0].author.as_str(), "alice");
    assert_eq!(file.messages()[1].line_number, 2);
    assert_eq!(file.messages()[1].author.as_str(), "alice");

    // Next 2 are bob's, renumbered to L000003, L000004
    assert_eq!(file.messages()[2].line_number, 3);
    assert_eq!(file.messages()[2].author.as_str(), "bob");
    assert_eq!(file.messages()[3].line_number, 4);
    assert_eq!(file.messages()[3].author.as_str(), "bob");

    // 6. Push should succeed
    repo_b.push().expect("push should succeed after conflict resolution");

    // 7. Clone A can pull and see all 4 messages
    repo_a.pull_rebase().unwrap();
    let a_final = std::fs::read_to_string(&thread_a).unwrap();
    let a_file = parse_thread(&a_final).unwrap();
    assert_eq!(a_file.messages().len(), 4, "clone_a should see all 4 messages");
}

#[test]
fn test_sync_pulls_when_nothing_to_push() {
    let (_bare_dir, clone_a_dir, clone_b_dir) = setup_two_clones();

    let thread_a = clone_a_dir.path().join("channels/general.thread");
    let thread_b = clone_b_dir.path().join("channels/general.thread");

    let alice = Handler::new("alice").unwrap();

    // Clone A: add content, commit, push
    let content = format_message(1, 0, &alice, "20260317T100000Z", "hello from alice");
    std::fs::write(&thread_a, &content).unwrap();
    let repo_a = GitStorage::new(clone_a_dir.path());
    repo_a
        .add_and_commit(&["channels/general.thread"], "alice msg")
        .unwrap();
    repo_a.push().unwrap();

    // Clone B: no local changes
    let repo_b = GitStorage::new(clone_b_dir.path());
    assert!(
        !repo_b.has_unpushed_commits().unwrap(),
        "clone_b should have no unpushed commits"
    );

    // Clone B: pull_rebase succeeds
    repo_b.pull_rebase().expect("pull should succeed");

    // Verify: clone_b now has the content from clone_a
    let b_content = std::fs::read_to_string(&thread_b).unwrap();
    let file = parse_thread(&b_content).unwrap();
    assert_eq!(file.messages().len(), 1);
    assert_eq!(file.messages()[0].author.as_str(), "alice");
    assert_eq!(file.messages()[0].body, "hello from alice");
}
