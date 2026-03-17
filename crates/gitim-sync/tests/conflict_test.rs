use std::path::{Path, PathBuf};
use std::process::Command;

use gitim_sync::conflict::resolve_thread_conflicts;
use gitim_sync::git::GitRepo;
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

/// Create a bare repo + two clones (a and b), with an initial commit on main.
/// Returns (bare_dir, clone_a_dir, clone_b_dir) as TempDirs.
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

    // Initial commit from clone A so main exists
    let init_file = clone_a_dir.path().join("init.txt");
    std::fs::write(&init_file, "init").unwrap();
    run_git(clone_a_dir.path(), &["add", "init.txt"]);
    run_git(clone_a_dir.path(), &["commit", "-m", "initial"]);
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
fn test_resolve_conflict_renumbers_local_messages() {
    let (_bare_dir, clone_a_dir, clone_b_dir) = setup_two_clones();

    let thread_rel = PathBuf::from("channels/general.thread");

    // Create the channels directory and empty thread in clone_a, push
    let thread_a = clone_a_dir.path().join(&thread_rel);
    std::fs::create_dir_all(thread_a.parent().unwrap()).unwrap();
    std::fs::write(&thread_a, "").unwrap();
    run_git(clone_a_dir.path(), &["add", "."]);
    run_git(clone_a_dir.path(), &["commit", "-m", "create thread"]);
    run_git(clone_a_dir.path(), &["push"]);

    // Clone B: pull to sync
    run_git(clone_b_dir.path(), &["pull"]);

    // Clone A: append 2 messages, commit, push
    let a_content = "\
[L000001][P000000][@alice][20260317T100000Z] hello from alice
[L000002][P000001][@alice][20260317T100100Z] second from alice
";
    std::fs::write(&thread_a, a_content).unwrap();
    run_git(clone_a_dir.path(), &["add", "."]);
    run_git(clone_a_dir.path(), &["commit", "-m", "alice messages"]);
    run_git(clone_a_dir.path(), &["push"]);

    // Clone B: append 2 messages with same line numbers, commit (don't push)
    let b_content = "\
[L000001][P000000][@bob][20260317T100200Z] hello from bob
[L000002][P000001][@bob][20260317T100300Z] second from bob
";
    let thread_b = clone_b_dir.path().join(&thread_rel);
    std::fs::write(&thread_b, b_content).unwrap();
    run_git(clone_b_dir.path(), &["add", "."]);
    run_git(clone_b_dir.path(), &["commit", "-m", "bob messages"]);

    // Clone B: capture local additions before resolution
    let repo_b = GitRepo::new(clone_b_dir.path());
    repo_b.fetch().unwrap();
    let local_additions = repo_b.diff_unpushed_thread_additions().unwrap();
    assert!(!local_additions.is_empty(), "should have local additions");

    // Resolve conflicts
    let mappings = resolve_thread_conflicts(&repo_b, &local_additions).unwrap();

    // Read the final thread content
    let final_content = std::fs::read_to_string(&thread_b).unwrap();

    // Verify: 4 messages total
    let file = gitim_core::parser::parse_thread(&final_content).unwrap();
    assert_eq!(file.messages.len(), 4, "should have 4 messages total");

    // First 2 are from alice (L000001, L000002 — the remote ones)
    assert_eq!(file.messages[0].line_number, 1);
    assert_eq!(file.messages[0].author.as_str(), "alice");
    assert_eq!(file.messages[1].line_number, 2);
    assert_eq!(file.messages[1].author.as_str(), "alice");

    // Next 2 are bob's, renumbered to L000003, L000004
    assert_eq!(file.messages[2].line_number, 3);
    assert_eq!(file.messages[2].author.as_str(), "bob");
    assert_eq!(file.messages[3].line_number, 4);
    assert_eq!(file.messages[3].author.as_str(), "bob");

    // Verify mappings
    assert_eq!(mappings.len(), 2);
    // old=1 -> new=3, old=2 -> new=4
    let mapping_set: Vec<(u64, u64)> = mappings.iter().map(|m| (m.old_line, m.new_line)).collect();
    assert!(mapping_set.contains(&(1, 3)));
    assert!(mapping_set.contains(&(2, 4)));

    // Verify: can push successfully after resolution
    repo_b.push().expect("push should succeed after conflict resolution");
}

#[test]
fn test_resolve_conflict_updates_p_references() {
    let (_bare_dir, clone_a_dir, clone_b_dir) = setup_two_clones();

    let thread_rel = PathBuf::from("channels/dev.thread");

    // Clone A: create thread with 1 message, push
    let thread_a = clone_a_dir.path().join(&thread_rel);
    std::fs::create_dir_all(thread_a.parent().unwrap()).unwrap();
    let a_content = "[L000001][P000000][@alice][20260317T100000Z] base message\n";
    std::fs::write(&thread_a, a_content).unwrap();
    run_git(clone_a_dir.path(), &["add", "."]);
    run_git(clone_a_dir.path(), &["commit", "-m", "base"]);
    run_git(clone_a_dir.path(), &["push"]);

    // Clone B: pull to sync
    run_git(clone_b_dir.path(), &["pull"]);

    // Clone A: append another message L000002, push
    let a_content2 = "\
[L000001][P000000][@alice][20260317T100000Z] base message
[L000002][P000001][@alice][20260317T100100Z] alice follow-up
";
    std::fs::write(&thread_a, a_content2).unwrap();
    run_git(clone_a_dir.path(), &["add", "."]);
    run_git(clone_a_dir.path(), &["commit", "-m", "alice follow-up"]);
    run_git(clone_a_dir.path(), &["push"]);

    // Clone B: append 2 messages where second references first (P within batch)
    let b_content = "\
[L000001][P000000][@alice][20260317T100000Z] base message
[L000002][P000000][@bob][20260317T100200Z] bob starts topic
[L000003][P000002][@bob][20260317T100300Z] bob replies to own msg
";
    let thread_b = clone_b_dir.path().join(&thread_rel);
    std::fs::write(&thread_b, b_content).unwrap();
    run_git(clone_b_dir.path(), &["add", "."]);
    run_git(clone_b_dir.path(), &["commit", "-m", "bob messages"]);

    // Capture local additions and resolve
    let repo_b = GitRepo::new(clone_b_dir.path());
    repo_b.fetch().unwrap();
    let local_additions = repo_b.diff_unpushed_thread_additions().unwrap();

    let mappings = resolve_thread_conflicts(&repo_b, &local_additions).unwrap();

    // Read final content
    let final_content = std::fs::read_to_string(&thread_b).unwrap();
    let file = gitim_core::parser::parse_thread(&final_content).unwrap();

    assert_eq!(file.messages.len(), 4);

    // Remote messages: alice L1 and L2
    assert_eq!(file.messages[0].line_number, 1);
    assert_eq!(file.messages[0].author.as_str(), "alice");
    assert_eq!(file.messages[1].line_number, 2);
    assert_eq!(file.messages[1].author.as_str(), "alice");

    // Bob's messages renumbered: L3 and L4
    let bob_msg1 = &file.messages[2];
    let bob_msg2 = &file.messages[3];
    assert_eq!(bob_msg1.line_number, 3);
    assert_eq!(bob_msg1.author.as_str(), "bob");
    assert_eq!(bob_msg2.line_number, 4);
    assert_eq!(bob_msg2.author.as_str(), "bob");

    // P reference within batch: bob_msg2 should point to bob_msg1 (was P2->P3)
    assert_eq!(bob_msg2.point_to, 3, "intra-batch P should be remapped");

    // bob_msg1 P should stay 0 (it was P000000, root)
    assert_eq!(bob_msg1.point_to, 0, "root P should remain 0");

    // Verify mappings
    assert_eq!(mappings.len(), 2);
    let mapping_set: Vec<(u64, u64)> = mappings.iter().map(|m| (m.old_line, m.new_line)).collect();
    assert!(mapping_set.contains(&(2, 3)));
    assert!(mapping_set.contains(&(3, 4)));
}

#[test]
fn test_resolve_conflict_preserves_external_p_references() {
    let (_bare_dir, clone_a_dir, clone_b_dir) = setup_two_clones();

    let thread_rel = PathBuf::from("channels/general.thread");

    // Clone A: create thread with 2 messages, push
    let thread_a = clone_a_dir.path().join(&thread_rel);
    std::fs::create_dir_all(thread_a.parent().unwrap()).unwrap();
    let a_content = "\
[L000001][P000000][@alice][20260317T100000Z] first
[L000002][P000001][@alice][20260317T100100Z] second
";
    std::fs::write(&thread_a, a_content).unwrap();
    run_git(clone_a_dir.path(), &["add", "."]);
    run_git(clone_a_dir.path(), &["commit", "-m", "alice messages"]);
    run_git(clone_a_dir.path(), &["push"]);

    // Clone B: pull to sync
    run_git(clone_b_dir.path(), &["pull"]);

    // Clone A: append L000003, push
    let a_content2 = "\
[L000001][P000000][@alice][20260317T100000Z] first
[L000002][P000001][@alice][20260317T100100Z] second
[L000003][P000002][@alice][20260317T100200Z] third
";
    std::fs::write(&thread_a, a_content2).unwrap();
    run_git(clone_a_dir.path(), &["add", "."]);
    run_git(clone_a_dir.path(), &["commit", "-m", "alice third"]);
    run_git(clone_a_dir.path(), &["push"]);

    // Clone B: append 1 message that references alice's L000001 (external P ref)
    let b_content = "\
[L000001][P000000][@alice][20260317T100000Z] first
[L000002][P000001][@alice][20260317T100100Z] second
[L000003][P000001][@bob][20260317T100300Z] reply to alice first
";
    let thread_b = clone_b_dir.path().join(&thread_rel);
    std::fs::write(&thread_b, b_content).unwrap();
    run_git(clone_b_dir.path(), &["add", "."]);
    run_git(clone_b_dir.path(), &["commit", "-m", "bob reply"]);

    // Capture local additions and resolve
    let repo_b = GitRepo::new(clone_b_dir.path());
    repo_b.fetch().unwrap();
    let local_additions = repo_b.diff_unpushed_thread_additions().unwrap();

    let _mappings = resolve_thread_conflicts(&repo_b, &local_additions).unwrap();

    // Read final content
    let final_content = std::fs::read_to_string(&thread_b).unwrap();
    let file = gitim_core::parser::parse_thread(&final_content).unwrap();

    assert_eq!(file.messages.len(), 4);

    // Remote: alice L1, L2, L3
    assert_eq!(file.messages[0].line_number, 1);
    assert_eq!(file.messages[1].line_number, 2);
    assert_eq!(file.messages[2].line_number, 3);
    assert_eq!(file.messages[2].author.as_str(), "alice");

    // Bob's message renumbered to L4
    let bob_msg = &file.messages[3];
    assert_eq!(bob_msg.line_number, 4);
    assert_eq!(bob_msg.author.as_str(), "bob");

    // P reference to alice's L1 should be PRESERVED (not remapped)
    // The original was P000001, which is an external ref (alice's message, not in the local batch)
    assert_eq!(
        bob_msg.point_to, 1,
        "external P reference should be preserved"
    );
}
