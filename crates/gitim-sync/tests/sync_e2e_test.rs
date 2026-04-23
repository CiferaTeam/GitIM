use std::path::{Path, PathBuf};
use std::process::Command;

use gitim_core::formatter::format_message;
use gitim_core::parser::parse_thread;
use gitim_core::types::{ChannelMeta, Handler};
use gitim_sync::conflict::{merge_channel_meta, resolve_content};
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
    run_git(
        clone_a_dir.path(),
        &["commit", "-m", "initial: empty thread"],
    );
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
    a_content.push_str(&format_message(
        1,
        0,
        &alice,
        "20260317T100000Z",
        "alice msg 1",
    ));
    a_content.push_str(&format_message(
        2,
        1,
        &alice,
        "20260317T100100Z",
        "alice msg 2",
    ));
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
    let paths: Vec<&str> = resolved_files
        .iter()
        .map(|r| r.path.to_str().unwrap())
        .collect();
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
    repo_b
        .push()
        .expect("push should succeed after conflict resolution");

    // 7. Clone A can pull and see all 4 messages
    repo_a.pull_rebase().unwrap();
    let a_final = std::fs::read_to_string(&thread_a).unwrap();
    let a_file = parse_thread(&a_final).unwrap();
    assert_eq!(
        a_file.messages().len(),
        4,
        "clone_a should see all 4 messages"
    );
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

#[test]
fn test_sync_resolves_concurrent_meta_changes() {
    let (_bare_dir, clone_a_dir, clone_b_dir) = setup_two_clones();

    let meta_rel = PathBuf::from("channels/general.meta.yaml");
    let meta_a = clone_a_dir.path().join(&meta_rel);
    let meta_b = clone_b_dir.path().join(&meta_rel);

    let repo_a = GitStorage::new(clone_a_dir.path());
    let repo_b = GitStorage::new(clone_b_dir.path());

    // Step 1: repo_a creates channels/general.meta.yaml with members: [god], pushes
    let initial_meta = ChannelMeta {
        display_name: "General".to_string(),
        created_by: "god".to_string(),
        created_at: "20260317T100000Z".to_string(),
        introduction: "General channel".to_string(),
        members: vec!["god".to_string()],
    };
    std::fs::write(&meta_a, serde_yaml::to_string(&initial_meta).unwrap()).unwrap();
    repo_a
        .add_and_commit(&["channels/general.meta.yaml"], "meta: create general")
        .unwrap();
    repo_a.push().unwrap();

    // Step 2: repo_b pulls
    repo_b.pull_rebase().unwrap();

    // Step 3: repo_a adds alice to members, pushes
    let mut a_meta: ChannelMeta =
        serde_yaml::from_str(&std::fs::read_to_string(&meta_a).unwrap()).unwrap();
    a_meta.members.push("alice".to_string());
    a_meta.members.sort();
    std::fs::write(&meta_a, serde_yaml::to_string(&a_meta).unwrap()).unwrap();
    repo_a
        .add_and_commit(&["channels/general.meta.yaml"], "meta: add alice")
        .unwrap();
    repo_a.push().unwrap();

    // Step 4: repo_b adds bob to members (from old base with only god), commits
    let mut b_meta: ChannelMeta =
        serde_yaml::from_str(&std::fs::read_to_string(&meta_b).unwrap()).unwrap();
    b_meta.members.push("bob".to_string());
    b_meta.members.sort();
    std::fs::write(&meta_b, serde_yaml::to_string(&b_meta).unwrap()).unwrap();
    repo_b
        .add_and_commit(&["channels/general.meta.yaml"], "meta: add bob")
        .unwrap();

    // Step 5: Manual sync flow mirroring sync_loop logic
    // Push should fail (remote has diverged)
    assert!(repo_b.push().is_err(), "push should fail due to conflict");

    // Fetch remote changes
    repo_b.fetch().unwrap();

    // Capture changed meta files
    let changed_meta_files = repo_b.changed_files_unpushed("*.meta.yaml").unwrap();
    assert!(
        !changed_meta_files.is_empty(),
        "should have changed meta files"
    );

    // Read local meta content before discard
    let mut local_metas = std::collections::HashMap::new();
    for rel_path in &changed_meta_files {
        let abs_path = clone_b_dir.path().join(rel_path);
        let content = std::fs::read_to_string(&abs_path).unwrap();
        local_metas.insert(rel_path.clone(), content);
    }

    // Discard unpushed (reset to remote state)
    repo_b.discard_unpushed().unwrap();

    // Read remote content, parse, merge, write
    for (rel_path, local_content) in &local_metas {
        let abs_path = clone_b_dir.path().join(rel_path);
        let remote_content = std::fs::read_to_string(&abs_path).unwrap();

        let local_meta: ChannelMeta = serde_yaml::from_str(local_content).unwrap();
        let remote_meta: ChannelMeta = serde_yaml::from_str(&remote_content).unwrap();

        let merged = merge_channel_meta(&local_meta, &remote_meta);
        std::fs::write(&abs_path, serde_yaml::to_string(&merged).unwrap()).unwrap();
    }

    // Commit and push
    let meta_paths: Vec<&str> = changed_meta_files
        .iter()
        .map(|p| p.to_str().unwrap())
        .collect();
    repo_b
        .add_and_commit(&meta_paths, "meta: sync after rebase")
        .unwrap();
    repo_b.push().expect("push should succeed after meta merge");

    // Step 6: Assert merged members = ["alice", "bob", "god"]
    let final_content = std::fs::read_to_string(&meta_b).unwrap();
    let final_meta: ChannelMeta = serde_yaml::from_str(&final_content).unwrap();
    assert_eq!(
        final_meta.members,
        vec!["alice".to_string(), "bob".to_string(), "god".to_string()],
        "merged members should be union of both sides, sorted"
    );
}

#[test]
fn test_pull_only_via_fetch_rebase() {
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

    // Clone B: no local changes — use fetch + rebase (new pull-only path)
    let repo_b = GitStorage::new(clone_b_dir.path());
    assert!(!repo_b.has_unpushed_commits().unwrap());

    repo_b.fetch().expect("fetch should succeed");
    repo_b
        .rebase_onto_origin()
        .expect("rebase should succeed (fast-forward)");

    // Verify: clone_b now has the content from clone_a
    let b_content = std::fs::read_to_string(&thread_b).unwrap();
    let file = parse_thread(&b_content).unwrap();
    assert_eq!(file.messages().len(), 1);
    assert_eq!(file.messages()[0].author.as_str(), "alice");
    assert_eq!(file.messages()[0].body, "hello from alice");
}

#[test]
fn test_pull_only_abort_rebase_preserves_racing_commit() {
    let (_bare_dir, clone_a_dir, clone_b_dir) = setup_two_clones();

    let thread_a = clone_a_dir.path().join("channels/general.thread");
    let thread_b = clone_b_dir.path().join("channels/general.thread");

    let alice = Handler::new("alice").unwrap();
    let bob = Handler::new("bob").unwrap();

    // Clone A: add a message, push
    let a_content = format_message(1, 0, &alice, "20260317T100000Z", "alice msg");
    std::fs::write(&thread_a, &a_content).unwrap();
    let repo_a = GitStorage::new(clone_a_dir.path());
    repo_a
        .add_and_commit(&["channels/general.thread"], "alice msg")
        .unwrap();
    repo_a.push().unwrap();

    // Clone B: "racing" local commit (simulating handler write during pull-only window)
    let b_content = format_message(1, 0, &bob, "20260317T100100Z", "bob msg");
    std::fs::write(&thread_b, &b_content).unwrap();
    let repo_b = GitStorage::new(clone_b_dir.path());
    repo_b
        .add_and_commit(&["channels/general.thread"], "bob msg")
        .unwrap();

    // Clone B: fetch succeeds
    repo_b.fetch().unwrap();

    // Clone B: rebase fails (both wrote L000001 to same file)
    let rebase_result = repo_b.rebase_onto_origin();
    assert!(rebase_result.is_err(), "rebase should fail due to conflict");

    // Clone B: abort_rebase (NOT discard_unpushed!)
    repo_b.abort_rebase().unwrap();

    // KEY: bob's local commit is preserved
    let final_content = std::fs::read_to_string(&thread_b).unwrap();
    assert!(
        final_content.contains("bob msg"),
        "local commit must survive abort_rebase"
    );

    // KEY: unpushed commits still detected — next cycle will use push path
    assert!(
        repo_b.has_unpushed_commits().unwrap(),
        "unpushed commit should still exist"
    );
}
