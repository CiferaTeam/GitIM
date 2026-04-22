use gitim_agent_provider::{create, PromptContext, ProviderConfig};
use gitim_runtime::{format_changes_as_prompt, ChannelChange};

#[test]
fn test_build_system_prompt_includes_handler() {
    let provider = create("mock", ProviderConfig::default()).unwrap();
    let ctx = PromptContext {
        handler: "test-agent",
        model: None,
    };
    let prompt = provider.build_system_prompt(&ctx);
    assert!(
        prompt.contains("test-agent"),
        "prompt should contain handler"
    );
    assert!(
        prompt.contains("协调者"),
        "prompt should contain coordinator identity"
    );
    assert!(
        prompt.contains("感知"),
        "prompt should contain perception layer"
    );
    assert!(
        prompt.contains("gitim send"),
        "prompt should mention gitim send"
    );
    assert!(
        prompt.contains("subagent"),
        "prompt should mention subagent delegation"
    );
}

#[test]
fn test_format_changes_new_format() {
    let changes = vec![ChannelChange {
        channel: "general".to_string(),
        kind: "channel".to_string(),
        entries: vec![serde_json::json!({
            "author": "alice",
            "body": "hello",
            "timestamp": "2026-04-13T10:00:00Z",
        })],
    }];

    let prompt = format_changes_as_prompt(&changes, "self-agent").unwrap();
    assert!(
        prompt.starts_with("以下是你上次醒来后发生的事件"),
        "should use neutral event header, got: {}",
        &prompt[..50.min(prompt.len())]
    );
    assert!(
        !prompt.contains("请处理"),
        "should not contain directive to process"
    );
    assert!(prompt.contains("@alice"), "should contain author");
    assert!(prompt.contains("hello"), "should contain body");
}

#[test]
fn test_format_changes_includes_timestamp() {
    let changes = vec![ChannelChange {
        channel: "dev".to_string(),
        kind: "channel".to_string(),
        entries: vec![serde_json::json!({
            "author": "bob",
            "body": "deploy ready",
            "timestamp": "2026-04-13T12:30:00Z",
        })],
    }];

    let prompt = format_changes_as_prompt(&changes, "self-agent").unwrap();
    assert!(
        prompt.contains("2026-04-13T12:30:00Z"),
        "should include timestamp in output"
    );
    assert!(
        prompt.contains("[2026-04-13T12:30:00Z]"),
        "timestamp should be bracketed"
    );
}

#[test]
fn test_format_changes_missing_timestamp() {
    let changes = vec![ChannelChange {
        channel: "general".to_string(),
        kind: "channel".to_string(),
        entries: vec![serde_json::json!({
            "author": "carol",
            "body": "hey there",
        })],
    }];

    let prompt = format_changes_as_prompt(&changes, "self-agent").unwrap();
    assert!(prompt.contains("@carol"), "should still contain author");
    assert!(prompt.contains("hey there"), "should still contain body");
    assert!(
        prompt.contains("[#general] @carol: hey there"),
        "should fall back to format without timestamp"
    );
}

#[test]
fn test_format_changes_marks_direct_mentions() {
    let changes = vec![ChannelChange {
        channel: "general".to_string(),
        kind: "channel".to_string(),
        entries: vec![serde_json::json!({
            "author": "carol",
            "body": "hey @self-agent, can you take this?",
        })],
    }];

    let prompt = format_changes_as_prompt(&changes, "self-agent").unwrap();
    assert!(
        prompt.contains("[MENTION] [#general] @carol: hey @self-agent, can you take this?"),
        "direct mentions should be surfaced with an explicit marker"
    );
}

#[test]
fn test_format_changes_marks_dm_scope() {
    let changes = vec![ChannelChange {
        channel: "dm:alice,bob".to_string(),
        kind: "dm".to_string(),
        entries: vec![serde_json::json!({
            "author": "alice",
            "body": "ping",
            "line_number": 3,
        })],
    }];

    let prompt = format_changes_as_prompt(&changes, "self-agent").unwrap();
    assert!(
        prompt.contains("[DM alice,bob] L3 @alice: ping"),
        "DM events should be labeled as DM scope"
    );
}

#[test]
fn test_format_changes_marks_card_scope() {
    let changes = vec![ChannelChange {
        channel: "card:dev/20260422-abc".to_string(),
        kind: "card_thread".to_string(),
        entries: vec![serde_json::json!({
            "author": "alice",
            "body": "blocked on review",
            "line_number": 12,
        })],
    }];

    let prompt = format_changes_as_prompt(&changes, "self-agent").unwrap();
    assert!(
        prompt.contains("[CARD dev/20260422-abc] L12 @alice: blocked on review"),
        "card discussion events should be labeled as card scope"
    );
}

#[test]
fn test_format_changes_filters_self_authored() {
    let changes = vec![ChannelChange {
        channel: "general".to_string(),
        kind: "channel".to_string(),
        entries: vec![
            serde_json::json!({
                "author": "my-agent",
                "body": "I said something",
                "timestamp": "2026-04-14T01:00:00Z",
            }),
            serde_json::json!({
                "author": "alice",
                "body": "hello agent",
                "timestamp": "2026-04-14T01:01:00Z",
            }),
        ],
    }];

    let prompt = format_changes_as_prompt(&changes, "my-agent").unwrap();
    assert!(
        !prompt.contains("my-agent"),
        "should filter out self-authored messages"
    );
    assert!(prompt.contains("@alice"), "should keep external messages");
}

#[test]
fn test_format_changes_returns_none_when_all_self() {
    let changes = vec![ChannelChange {
        channel: "general".to_string(),
        kind: "channel".to_string(),
        entries: vec![serde_json::json!({
            "author": "my-agent",
            "body": "talking to myself",
        })],
    }];

    assert!(
        format_changes_as_prompt(&changes, "my-agent").is_none(),
        "should return None when all messages are self-authored"
    );
}
