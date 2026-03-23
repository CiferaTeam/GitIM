use std::collections::HashMap;
use tempfile::tempdir;

fn make_msg(author: &str, line: u64, ts: &str, body: &str) -> String {
    format!("[L{:06}][P000000][@{}][{}] {}", line, author, ts, body)
}

#[test]
fn test_full_rebuild_then_incremental_append() {
    let dir = tempdir().unwrap();

    // Create initial .thread files
    let channels = dir.path().join("channels");
    std::fs::create_dir_all(&channels).unwrap();
    std::fs::write(
        channels.join("general.thread"),
        [
            make_msg("alice", 1, "20260323T100000Z", "hello everyone"),
            make_msg("bob", 2, "20260323T100001Z", "hi alice"),
        ]
        .join("\n"),
    )
    .unwrap();

    let dm = dir.path().join("dm");
    std::fs::create_dir_all(&dm).unwrap();
    std::fs::write(
        dm.join("alice--bob.thread"),
        make_msg("alice", 1, "20260323T100000Z", "secret message"),
    )
    .unwrap();

    // Full rebuild
    let index = gitim_index::Index::open_in_memory().unwrap();
    let count = index.rebuild(dir.path(), "commit_aaa").unwrap();
    assert_eq!(count, 3); // 2 channel + 1 dm

    // Verify search works
    let result = index
        .search(gitim_index::SearchParams {
            query: Some("hello".to_string()),
            author: None,
            channel: None,
            channel_type: None,
            current_user: Some("alice".to_string()),
            limit: 50,
            offset: 0,
        })
        .unwrap();
    assert_eq!(result.messages.len(), 1);
    assert_eq!(result.messages[0].author, "alice");

    // Incremental append
    let mut diff = HashMap::new();
    diff.insert(
        "channels/general.thread".to_string(),
        make_msg("charlie", 3, "20260323T100002Z", "hello from charlie"),
    );
    let added = index.append_from_diff(&diff, "commit_bbb").unwrap();
    assert_eq!(added, 1);
    assert_eq!(index.get_commit_id().unwrap().unwrap(), "commit_bbb");

    // Verify search after incremental
    let result = index
        .search(gitim_index::SearchParams {
            query: Some("hello".to_string()),
            author: None,
            channel: None,
            channel_type: None,
            current_user: Some("alice".to_string()),
            limit: 50,
            offset: 0,
        })
        .unwrap();
    assert_eq!(result.messages.len(), 2); // alice + charlie
}

#[test]
fn test_dm_visibility_across_channels() {
    let index = gitim_index::Index::open_in_memory().unwrap();

    // Index multiple DMs and channels
    index
        .index_thread_content(
            "general",
            &make_msg("alice", 1, "20260323T100000Z", "public hello"),
        )
        .unwrap();
    index
        .index_thread_content(
            "alice--bob",
            &make_msg("alice", 1, "20260323T100000Z", "private to bob"),
        )
        .unwrap();
    index
        .index_thread_content(
            "alice--charlie",
            &make_msg("charlie", 1, "20260323T100000Z", "private to alice"),
        )
        .unwrap();
    index
        .index_thread_content(
            "bob--charlie",
            &make_msg("bob", 1, "20260323T100000Z", "private bob charlie"),
        )
        .unwrap();

    // bob searches "private" — should only see alice--bob and bob--charlie
    let result = index
        .search(gitim_index::SearchParams {
            query: Some("private".to_string()),
            author: None,
            channel: None,
            channel_type: None,
            current_user: Some("bob".to_string()),
            limit: 50,
            offset: 0,
        })
        .unwrap();

    let channels: Vec<&str> = result.messages.iter().map(|m| m.channel.as_str()).collect();
    assert!(
        channels.contains(&"alice--bob"),
        "bob should see alice--bob DM"
    );
    assert!(
        channels.contains(&"bob--charlie"),
        "bob should see bob--charlie DM"
    );
    assert!(
        !channels.contains(&"alice--charlie"),
        "bob should NOT see alice--charlie DM"
    );
}

#[test]
fn test_reindex_from_scratch() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test_index.db");

    let channels = dir.path().join("channels");
    std::fs::create_dir_all(&channels).unwrap();
    std::fs::write(
        channels.join("ops.thread"),
        make_msg("alice", 1, "20260323T100000Z", "deploy complete"),
    )
    .unwrap();

    // Create and use index
    let index = gitim_index::Index::open(&db_path).unwrap();
    index.rebuild(dir.path(), "commit_111").unwrap();

    // Reindex from scratch
    let count = index.reindex(dir.path(), "commit_222").unwrap();
    assert_eq!(count, 1);
    assert_eq!(index.get_commit_id().unwrap().unwrap(), "commit_222");

    // Verify search still works after reindex
    let result = index
        .search(gitim_index::SearchParams {
            query: Some("deploy".to_string()),
            author: None,
            channel: None,
            channel_type: None,
            current_user: Some("alice".to_string()),
            limit: 50,
            offset: 0,
        })
        .unwrap();
    assert_eq!(result.messages.len(), 1);
}

#[test]
fn test_pagination() {
    let index = gitim_index::Index::open_in_memory().unwrap();

    // Insert 10 messages
    let content: Vec<String> = (1..=10)
        .map(|i| {
            make_msg(
                "alice",
                i,
                &format!("20260323T{:06}Z", 100000 + i),
                &format!("message {}", i),
            )
        })
        .collect();
    index
        .index_thread_content("general", &content.join("\n"))
        .unwrap();

    // Page 1: limit 3, offset 0
    let result = index
        .search(gitim_index::SearchParams {
            query: None,
            author: Some("alice".to_string()),
            channel: None,
            channel_type: None,
            current_user: Some("alice".to_string()),
            limit: 3,
            offset: 0,
        })
        .unwrap();
    assert_eq!(result.messages.len(), 3);
    assert_eq!(result.total, 10);

    // Page 2: limit 3, offset 3
    let result2 = index
        .search(gitim_index::SearchParams {
            query: None,
            author: Some("alice".to_string()),
            channel: None,
            channel_type: None,
            current_user: Some("alice".to_string()),
            limit: 3,
            offset: 3,
        })
        .unwrap();
    assert_eq!(result2.messages.len(), 3);
    assert_eq!(result2.total, 10);
}

#[test]
fn test_channel_filter() {
    let index = gitim_index::Index::open_in_memory().unwrap();

    index
        .index_thread_content(
            "general",
            &make_msg("alice", 1, "20260323T100000Z", "hello in general"),
        )
        .unwrap();
    index
        .index_thread_content(
            "ops",
            &make_msg("alice", 1, "20260323T100000Z", "hello in ops"),
        )
        .unwrap();

    let result = index
        .search(gitim_index::SearchParams {
            query: Some("hello".to_string()),
            author: None,
            channel: Some("ops".to_string()),
            channel_type: None,
            current_user: Some("alice".to_string()),
            limit: 50,
            offset: 0,
        })
        .unwrap();

    assert_eq!(result.messages.len(), 1);
    assert_eq!(result.messages[0].channel, "ops");
}
