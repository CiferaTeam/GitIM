use std::path::{Path, PathBuf};
use std::process::Command;

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

/// Create a bare repo + two clones (a and b), with an initial commit on main.
fn setup_two_clones() -> (TempDir, TempDir, TempDir) {
    let bare_dir = TempDir::new().unwrap();
    let clone_a_dir = TempDir::new().unwrap();
    let clone_b_dir = TempDir::new().unwrap();

    run_git(bare_dir.path(), &["init", "--bare"]);

    run_git(
        clone_a_dir.path().parent().unwrap(),
        &[
            "clone",
            bare_dir.path().to_str().unwrap(),
            clone_a_dir.path().to_str().unwrap(),
        ],
    );
    setup_git_config(clone_a_dir.path(), "Alice", "alice@test.com");

    let init_file = clone_a_dir.path().join("init.txt");
    std::fs::write(&init_file, "init").unwrap();
    run_git(clone_a_dir.path(), &["add", "init.txt"]);
    run_git(clone_a_dir.path(), &["commit", "-m", "initial"]);
    run_git(clone_a_dir.path(), &["push", "-u", "origin", "main"]);

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
fn test_resolve_content_renumbers_local_messages() {
    let (_bare_dir, clone_a_dir, clone_b_dir) = setup_two_clones();

    let thread_rel = PathBuf::from("channels/general.thread");

    // Clone A: create empty thread, push
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

    // Capture local additions, then discard + resolve
    let repo_b = GitStorage::new(clone_b_dir.path());
    repo_b.fetch().unwrap();
    let local_additions = repo_b.diff_unpushed("*.thread").unwrap();
    assert!(!local_additions.is_empty());

    // Discard unpushed (SyncLoop responsibility), then resolve content
    repo_b.discard_unpushed().unwrap();
    let (resolved_files, mappings) = resolve_content(&local_additions, repo_b.root()).unwrap();

    // Write resolved content + commit (SyncLoop responsibility)
    for resolved in &resolved_files {
        let abs_path = repo_b.root().join(&resolved.path);
        std::fs::write(&abs_path, &resolved.content).unwrap();
    }
    let paths: Vec<&str> = resolved_files.iter().map(|r| r.path.to_str().unwrap()).collect();
    repo_b.add_and_commit(&paths, "resolved").unwrap();

    // Verify: 4 messages total
    let final_content = std::fs::read_to_string(&thread_b).unwrap();
    let file = gitim_core::parser::parse_thread(&final_content).unwrap();
    assert_eq!(file.messages().len(), 4);

    assert_eq!(file.messages()[0].line_number, 1);
    assert_eq!(file.messages()[0].author.as_str(), "alice");
    assert_eq!(file.messages()[1].line_number, 2);
    assert_eq!(file.messages()[1].author.as_str(), "alice");
    assert_eq!(file.messages()[2].line_number, 3);
    assert_eq!(file.messages()[2].author.as_str(), "bob");
    assert_eq!(file.messages()[3].line_number, 4);
    assert_eq!(file.messages()[3].author.as_str(), "bob");

    assert_eq!(mappings.len(), 2);
    let mapping_set: Vec<(u64, u64)> = mappings.iter().map(|m| (m.old_line, m.new_line)).collect();
    assert!(mapping_set.contains(&(1, 3)));
    assert!(mapping_set.contains(&(2, 4)));

    repo_b.push().expect("push should succeed after resolution");
}

#[test]
fn test_resolve_content_updates_p_references() {
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

    // Clone B: pull
    run_git(clone_b_dir.path(), &["pull"]);

    // Clone A: append L000002, push
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

    let repo_b = GitStorage::new(clone_b_dir.path());
    repo_b.fetch().unwrap();
    let local_additions = repo_b.diff_unpushed("*.thread").unwrap();

    repo_b.discard_unpushed().unwrap();
    let (resolved_files, mappings) = resolve_content(&local_additions, repo_b.root()).unwrap();

    for resolved in &resolved_files {
        std::fs::write(repo_b.root().join(&resolved.path), &resolved.content).unwrap();
    }

    let final_content = std::fs::read_to_string(&thread_b).unwrap();
    let file = gitim_core::parser::parse_thread(&final_content).unwrap();

    assert_eq!(file.messages().len(), 4);

    let bob_msg1 = &file.messages()[2];
    let bob_msg2 = &file.messages()[3];
    assert_eq!(bob_msg1.line_number, 3);
    assert_eq!(bob_msg2.line_number, 4);

    // P reference within batch: bob_msg2 should point to bob_msg1 (was P2->P3)
    assert_eq!(bob_msg2.point_to, 3, "intra-batch P should be remapped");
    assert_eq!(bob_msg1.point_to, 0, "root P should remain 0");

    assert_eq!(mappings.len(), 2);
}

#[test]
fn test_resolve_content_preserves_external_p_references() {
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

    // Clone B: pull
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

    // Clone B: append 1 message referencing alice's L000001 (external P ref)
    let b_content = "\
[L000001][P000000][@alice][20260317T100000Z] first
[L000002][P000001][@alice][20260317T100100Z] second
[L000003][P000001][@bob][20260317T100300Z] reply to alice first
";
    let thread_b = clone_b_dir.path().join(&thread_rel);
    std::fs::write(&thread_b, b_content).unwrap();
    run_git(clone_b_dir.path(), &["add", "."]);
    run_git(clone_b_dir.path(), &["commit", "-m", "bob reply"]);

    let repo_b = GitStorage::new(clone_b_dir.path());
    repo_b.fetch().unwrap();
    let local_additions = repo_b.diff_unpushed("*.thread").unwrap();

    repo_b.discard_unpushed().unwrap();
    let (resolved_files, _mappings) = resolve_content(&local_additions, repo_b.root()).unwrap();

    for resolved in &resolved_files {
        std::fs::write(repo_b.root().join(&resolved.path), &resolved.content).unwrap();
    }

    let final_content = std::fs::read_to_string(&thread_b).unwrap();
    let file = gitim_core::parser::parse_thread(&final_content).unwrap();

    assert_eq!(file.messages().len(), 4);

    let bob_msg = &file.messages()[3];
    assert_eq!(bob_msg.line_number, 4);
    assert_eq!(bob_msg.author.as_str(), "bob");
    assert_eq!(bob_msg.point_to, 1, "external P reference should be preserved");
}

use gitim_core::types::ChannelMeta;
use gitim_sync::conflict::merge_channel_meta;

#[test]
fn test_merge_channel_meta_union_and_dedup() {
    // Scenario 1: disjoint members → union
    let local = ChannelMeta {
        display_name: "General".into(),
        created_by: "god".into(),
        created_at: "20260330T120000Z".into(),
        introduction: "默认频道".into(),
        members: vec!["alice".into(), "god".into()],
    };
    let remote = ChannelMeta {
        display_name: "General".into(),
        created_by: "god".into(),
        created_at: "20260330T120000Z".into(),
        introduction: "默认频道".into(),
        members: vec!["bob".into(), "god".into()],
    };
    let merged = merge_channel_meta(&local, &remote);
    assert_eq!(merged.members, vec!["alice", "bob", "god"]);

    // Scenario 2: overlapping members → dedup
    let local = ChannelMeta {
        display_name: "General".into(),
        created_by: "god".into(),
        created_at: "20260330T120000Z".into(),
        introduction: "默认频道".into(),
        members: vec!["alice".into(), "bob".into(), "god".into()],
    };
    let remote = ChannelMeta {
        display_name: "General".into(),
        created_by: "god".into(),
        created_at: "20260330T120000Z".into(),
        introduction: "默认频道".into(),
        members: vec!["alice".into(), "god".into()],
    };
    let merged = merge_channel_meta(&local, &remote);
    assert_eq!(merged.members, vec!["alice", "bob", "god"]);
}

#[test]
fn test_merge_channel_meta_scalars_from_remote() {
    let local = ChannelMeta {
        display_name: "Local Name".into(),
        created_by: "god".into(),
        created_at: "20260330T120000Z".into(),
        introduction: "local intro".into(),
        members: vec!["alice".into()],
    };
    let remote = ChannelMeta {
        display_name: "Remote Name".into(),
        created_by: "god".into(),
        created_at: "20260330T120000Z".into(),
        introduction: "remote intro".into(),
        members: vec!["bob".into()],
    };
    let merged = merge_channel_meta(&local, &remote);
    assert_eq!(merged.display_name, "Remote Name");
    assert_eq!(merged.introduction, "remote intro");
    assert_eq!(merged.members, vec!["alice", "bob"]);
}
