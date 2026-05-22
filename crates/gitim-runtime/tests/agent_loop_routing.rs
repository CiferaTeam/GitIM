//! Tests for the recipients-based routing filter in
//! `format_changes_as_prompt`. Pure-function tests — no daemon needed.
//!
//! The filter rule:
//!   - Skip entries where `author == self_handler` (pre-existing)
//!   - Skip entries with a non-empty `recipients` array that does NOT
//!     contain `self_handler` (NEW)
//!   - Empty or missing `recipients` falls back to broadcast for legacy
//!     chat-like changes. Card changes are explicitly routed by product
//!     semantics: card_thread only wakes on mention, and card_meta wakes
//!     only the assignee or explicitly mentioned handlers.

use gitim_runtime::format_changes_as_prompt;
use gitim_runtime::poller::ChannelChange;

fn entry_with_recipients(
    line: u64,
    parent: u64,
    author: &str,
    body: &str,
    recipients: Option<Vec<&str>>,
) -> serde_json::Value {
    let mut v = serde_json::json!({
        "type": "message",
        "line_number": line,
        "point_to": parent,
        "author": author,
        "timestamp": "2026-05-17T00:00:00Z",
        "body": body,
        "mentions": [],
        "links": [],
    });
    if let Some(r) = recipients {
        v["recipients"] = serde_json::json!(r);
    }
    v
}

fn change(entries: Vec<serde_json::Value>) -> ChannelChange {
    ChannelChange {
        channel: "general".to_string(),
        kind: "channel".to_string(),
        entries,
    }
}

#[test]
fn skips_messages_where_self_not_in_recipients() {
    let changes = vec![change(vec![entry_with_recipients(
        1,
        0,
        "alice",
        "hello",
        Some(vec!["bob"]),
    )])];
    let prompt = format_changes_as_prompt(&changes, "charlie");
    assert!(prompt.is_none(), "charlie should be filtered out");
}

#[test]
fn includes_messages_where_self_in_recipients() {
    let changes = vec![change(vec![entry_with_recipients(
        1,
        0,
        "alice",
        "hello",
        Some(vec!["bob", "charlie"]),
    )])];
    let prompt = format_changes_as_prompt(&changes, "charlie").expect("should have prompt");
    assert!(prompt.contains("@alice"));
    assert!(prompt.contains("hello"));
}

#[test]
fn empty_recipients_broadcasts_to_all() {
    // Empty array = explicit broadcast fallback. Daemon's empty-fallback
    // warn covers this case for new wires; this is the legacy compat.
    let changes = vec![change(vec![entry_with_recipients(
        1,
        0,
        "alice",
        "hello",
        Some(vec![]),
    )])];
    let prompt = format_changes_as_prompt(&changes, "charlie").expect("should broadcast");
    assert!(prompt.contains("hello"));
}

#[test]
fn missing_recipients_field_broadcasts_to_all() {
    // Old daemon (no recipients field at all) = broadcast. Same effect
    // as empty array — runtime keeps legacy semantics.
    let changes = vec![change(vec![entry_with_recipients(
        1, 0, "alice", "world", None,
    )])];
    let prompt = format_changes_as_prompt(&changes, "charlie").expect("should broadcast");
    assert!(prompt.contains("world"));
}

#[test]
fn self_author_skip_takes_priority_over_recipients() {
    // Even if recipients includes self, a self-authored message is
    // still skipped — we never re-prompt ourselves with our own output.
    let changes = vec![change(vec![entry_with_recipients(
        1,
        0,
        "charlie",
        "self note",
        Some(vec!["charlie"]),
    )])];
    let prompt = format_changes_as_prompt(&changes, "charlie");
    assert!(prompt.is_none(), "self-authored messages always skipped");
}

#[test]
fn mixed_entries_filter_independently() {
    // Three entries: one for charlie, one for someone else, one
    // broadcast. Self should see two of the three.
    let changes = vec![change(vec![
        entry_with_recipients(1, 0, "alice", "for charlie", Some(vec!["charlie"])),
        entry_with_recipients(2, 0, "alice", "for bob only", Some(vec!["bob"])),
        entry_with_recipients(3, 0, "alice", "broadcast", None),
    ])];
    let prompt = format_changes_as_prompt(&changes, "charlie").expect("should have prompt");
    assert!(prompt.contains("for charlie"));
    assert!(!prompt.contains("for bob only"));
    assert!(prompt.contains("broadcast"));
}

fn typed_change(kind: &str, channel: &str, entries: Vec<serde_json::Value>) -> ChannelChange {
    ChannelChange {
        channel: channel.to_string(),
        kind: kind.to_string(),
        entries,
    }
}

#[test]
fn card_thread_without_self_mention_is_not_broadcast() {
    let changes = vec![typed_change(
        "card_thread",
        "card:dev/20260522-abc",
        vec![serde_json::json!({
            "type": "message",
            "line_number": 1,
            "point_to": 0,
            "author": "alice",
            "timestamp": "2026-05-22T00:00:00Z",
            "body": "status note",
            "mentions": [],
        })],
    )];

    let prompt = format_changes_as_prompt(&changes, "bob");
    assert!(
        prompt.is_none(),
        "plain card comments should not wake channel members"
    );
}

#[test]
fn card_thread_with_self_mention_is_included() {
    let changes = vec![typed_change(
        "card_thread",
        "card:dev/20260522-abc",
        vec![serde_json::json!({
            "type": "message",
            "line_number": 1,
            "point_to": 0,
            "author": "alice",
            "timestamp": "2026-05-22T00:00:00Z",
            "body": "please check <@bob>",
            "mentions": ["bob"],
        })],
    )];

    let prompt = format_changes_as_prompt(&changes, "bob").expect("mention should wake bob");
    assert!(prompt.contains("[MENTION] [CARD dev/20260522-abc] L1 @alice: please check <@bob>"));
}

#[test]
fn card_meta_assignment_only_wakes_assignee() {
    let changes = vec![typed_change(
        "card_meta",
        "card:dev/20260522-abc",
        vec![serde_json::json!({
            "type": "card_event",
            "event_type": "card_assignment",
            "author": "system",
            "body": "card assigned to bob",
            "assignee": "bob",
            "mentions": [],
        })],
    )];

    let assignee_prompt =
        format_changes_as_prompt(&changes, "bob").expect("assignee should be woken");
    assert!(assignee_prompt.contains("[CARD dev/20260522-abc] @system: card assigned to bob"));
    assert!(
        !assignee_prompt.contains("[MENTION]"),
        "assignment is a task event, not a mention"
    );

    let other_prompt = format_changes_as_prompt(&changes, "charlie");
    assert!(
        other_prompt.is_none(),
        "card assignment should not broadcast to unrelated channel members"
    );
}

#[test]
fn card_meta_mention_wakes_mentioned_handler_only() {
    let changes = vec![typed_change(
        "card_meta",
        "card:dev/20260522-abc",
        vec![serde_json::json!({
            "type": "card_event",
            "event_type": "card_mention",
            "author": "system",
            "body": "card created: follow up with <@charlie>",
            "mentions": ["charlie"],
        })],
    )];

    let mentioned_prompt =
        format_changes_as_prompt(&changes, "charlie").expect("mention should wake charlie");
    assert!(mentioned_prompt.contains("[MENTION] [CARD dev/20260522-abc] @system: card created"));

    let other_prompt = format_changes_as_prompt(&changes, "bob");
    assert!(
        other_prompt.is_none(),
        "card meta mention should not broadcast to unrelated channel members"
    );
}
