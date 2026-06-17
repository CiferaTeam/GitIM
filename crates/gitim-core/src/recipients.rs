//! Compute the recipients set for a channel message.
//!
//! Recipients = union of:
//!   1. Channel owner (`ChannelMeta.created_by`)
//!   2. Parent-chain participants: ancestor authors and mentions
//!   3. Explicit @mentions in the message body
//!
//! Output is sorted (BTreeSet-derived) and deduped. Returned as
//! `Vec<String>` to match the wire format and ChannelMeta field types.
//!
//! DM channels are NOT handled here — callers inline `recipients =
//! members` for DM threads. This function is for channel threads only.
//!
//! Card discussion threads use [`compute_card_thread_recipients`]
//! instead, which routes by the card's task roles (reporter +
//! assignee) plus mentions — not by channel membership.

use crate::types::message::Message;
use crate::types::{CardMeta, ChannelMeta};
use std::collections::{BTreeSet, HashSet};

pub fn compute_recipients(
    message: &Message,
    channel_meta: &ChannelMeta,
    all_messages: &[Message],
) -> Vec<String> {
    let mut recipients: BTreeSet<String> = BTreeSet::new();

    // Rule 1: channel owner.
    if !channel_meta.created_by.is_empty() {
        recipients.insert(channel_meta.created_by.clone());
    }

    // Rule 2: parent chain — walk point_to upward, collect ancestor participants.
    // `visited` guards against cycles in malformed input (well-formed thread
    // files have strictly decreasing point_to, but daemon must not panic on
    // adversarial or race-corrupted state).
    let mut cursor = message.point_to;
    let mut visited: HashSet<u64> = HashSet::new();
    while cursor != 0 && visited.insert(cursor) {
        match all_messages.iter().find(|m| m.line_number == cursor) {
            Some(ancestor) => {
                recipients.insert(ancestor.author.as_str().to_string());
                for handler in &ancestor.mentions {
                    recipients.insert(handler.as_str().to_string());
                }
                cursor = ancestor.point_to;
            }
            None => break,
        }
    }

    // Rule 3: explicit @mentions in the new message body.
    for handler in &message.mentions {
        recipients.insert(handler.as_str().to_string());
    }

    recipients.into_iter().collect()
}

/// Compute the recipients set for a card discussion message.
///
/// Cards are task records, not chat threads, so routing is by task
/// roles rather than channel membership:
///   1. Reporter (`CardMeta.created_by`) — wants progress on what they filed
///   2. Current assignee (`CardMeta.assignee`) — owns the work
///   3. Explicit @mentions in the message body
///
/// Channel members who aren't the reporter, assignee, or mentioned
/// are NOT notified — that would be a fanout broadcast, which is
/// explicitly out of scope (see the "narrow card wakeups" commit).
///
/// The runtime's `author == self_handler` skip handles the case
/// where the message author is also the reporter/assignee (the
/// author won't be woken by their own message).
pub fn compute_card_thread_recipients(message: &Message, card_meta: &CardMeta) -> Vec<String> {
    let mut recipients: BTreeSet<String> = BTreeSet::new();

    if !card_meta.created_by.is_empty() {
        recipients.insert(card_meta.created_by.clone());
    }
    if let Some(assignee) = &card_meta.assignee {
        if !assignee.is_empty() {
            recipients.insert(assignee.clone());
        }
    }
    for handler in &message.mentions {
        recipients.insert(handler.as_str().to_string());
    }

    recipients.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::card::CardStatus;
    use crate::types::Handler;

    fn meta(created_by: &str) -> ChannelMeta {
        ChannelMeta {
            display_name: "test".into(),
            created_by: created_by.into(),
            created_at: "2026-05-17T00:00:00Z".into(),
            introduction: String::new(),
            members: vec![],
            project: None,
        }
    }

    fn msg(line: u64, parent: u64, author: &str, mentions: Vec<&str>) -> Message {
        Message {
            line_number: line,
            point_to: parent,
            author: Handler::new(author).unwrap(),
            timestamp: "2026-05-17T00:00:00Z".into(),
            body: String::new(),
            mentions: mentions
                .into_iter()
                .map(|m| Handler::new(m).unwrap())
                .collect(),
            links: vec![],
        }
    }

    #[test]
    fn root_message_no_mentions_only_owner() {
        let m = msg(1, 0, "alice", vec![]);
        let r = compute_recipients(&m, &meta("owner"), &[]);
        assert_eq!(r, vec!["owner".to_string()]);
    }

    #[test]
    fn root_message_with_mention_includes_owner_and_mentioned() {
        let m = msg(1, 0, "alice", vec!["bob"]);
        let r = compute_recipients(&m, &meta("owner"), &[]);
        assert_eq!(r, vec!["bob".to_string(), "owner".to_string()]);
    }

    #[test]
    fn reply_walks_parent_chain() {
        let root = msg(1, 0, "alice", vec![]);
        let mid = msg(2, 1, "bob", vec![]);
        let new = msg(3, 2, "charlie", vec![]);
        let r = compute_recipients(&new, &meta("owner"), &[root, mid]);
        assert_eq!(
            r,
            vec!["alice".to_string(), "bob".to_string(), "owner".to_string()]
        );
    }

    #[test]
    fn reply_with_mention_dedups_against_chain() {
        let root = msg(1, 0, "alice", vec![]);
        let new = msg(2, 1, "bob", vec!["alice"]);
        let r = compute_recipients(&new, &meta("owner"), &[root]);
        assert_eq!(r, vec!["alice".to_string(), "owner".to_string()]);
    }

    #[test]
    fn reply_inherits_ancestor_mentions() {
        let root = msg(1, 0, "alice", vec!["reviewer"]);
        let new = msg(2, 1, "bob", vec![]);
        let r = compute_recipients(&new, &meta("owner"), &[root]);
        assert_eq!(
            r,
            vec![
                "alice".to_string(),
                "owner".to_string(),
                "reviewer".to_string(),
            ]
        );
    }

    #[test]
    fn cycle_in_parent_chain_terminates() {
        // A self-pointing message: point_to == line_number. Walking
        // from a child that points to the cyclic line picks up its
        // author once, then the `visited` set short-circuits the next
        // hop without infinite-looping or panicking.
        let cyclic = msg(1, 1, "alice", vec![]);
        let new = msg(2, 1, "bob", vec![]);
        let r = compute_recipients(&new, &meta("owner"), &[cyclic]);
        assert_eq!(r, vec!["alice".to_string(), "owner".to_string()]);
    }

    #[test]
    fn multi_hop_cycle_terminates() {
        // A → C, B → A, C → B (cycle A↔B↔C) with a new message
        // pointing into the cycle. The walk picks up each ancestor
        // exactly once, then the `visited` set short-circuits before
        // re-entering A.
        let a = msg(1, 3, "alice", vec![]); // A points to C
        let b = msg(2, 1, "bob", vec![]); // B points to A
        let c = msg(3, 2, "charlie", vec![]); // C points to B
        let new = msg(4, 1, "dave", vec![]); // new points to A
        let r = compute_recipients(&new, &meta("owner"), &[a, b, c]);
        assert_eq!(
            r,
            vec![
                "alice".to_string(),
                "bob".to_string(),
                "charlie".to_string(),
                "owner".to_string(),
            ]
        );
    }

    #[test]
    fn parent_chain_with_missing_ancestor_breaks_cleanly() {
        // new's parent points to line 99 which doesn't exist in
        // all_messages — walking stops, recipients is just the owner.
        let new = msg(2, 99, "bob", vec![]);
        let r = compute_recipients(&new, &meta("owner"), &[]);
        assert_eq!(r, vec!["owner".to_string()]);
    }

    #[test]
    fn empty_created_by_skips_rule_1() {
        let m = msg(1, 0, "alice", vec!["bob"]);
        let r = compute_recipients(&m, &meta(""), &[]);
        // Only bob from rule 3; rule 1 skipped because created_by empty.
        assert_eq!(r, vec!["bob".to_string()]);
    }

    #[test]
    fn self_mention_included_caller_dedups_at_consumption() {
        // If a message @s its own author, the author appears in
        // recipients. The runtime's author == self_handler skip
        // takes precedence over recipients membership, so no loop.
        let m = msg(1, 0, "alice", vec!["alice"]);
        let r = compute_recipients(&m, &meta("owner"), &[]);
        assert_eq!(r, vec!["alice".to_string(), "owner".to_string()]);
    }

    fn card(created_by: &str, assignee: Option<&str>) -> CardMeta {
        CardMeta {
            title: "demo".into(),
            channel: "dev".into(),
            status: CardStatus::Todo,
            labels: vec![],
            assignee: assignee.map(|s| s.to_string()),
            created_by: created_by.into(),
            created_at: "2026-05-22T00:00:00Z".into(),
            updated_at: "2026-05-22T00:00:00Z".into(),
            archived_via: None,
        }
    }

    #[test]
    fn card_thread_routes_to_reporter_and_assignee() {
        // Most common case: A files a card and assigns B, B drops
        // a progress note. Recipients should be {A, B}; the runtime
        // skips B as the author, leaving A woken — closing the loop.
        let m = msg(1, 0, "bob", vec![]);
        let r = compute_card_thread_recipients(&m, &card("alice", Some("bob")));
        assert_eq!(r, vec!["alice".to_string(), "bob".to_string()]);
    }

    #[test]
    fn card_thread_unassigned_routes_to_reporter_only() {
        let m = msg(1, 0, "charlie", vec![]);
        let r = compute_card_thread_recipients(&m, &card("alice", None));
        assert_eq!(r, vec!["alice".to_string()]);
    }

    #[test]
    fn card_thread_mention_unions_with_roles() {
        let m = msg(1, 0, "bob", vec!["charlie"]);
        let r = compute_card_thread_recipients(&m, &card("alice", Some("bob")));
        assert_eq!(
            r,
            vec![
                "alice".to_string(),
                "bob".to_string(),
                "charlie".to_string()
            ]
        );
    }

    #[test]
    fn card_thread_dedupes_when_reporter_is_assignee() {
        // Self-filed task: same handler is reporter and assignee.
        // Only one entry in recipients.
        let m = msg(1, 0, "alice", vec![]);
        let r = compute_card_thread_recipients(&m, &card("alice", Some("alice")));
        assert_eq!(r, vec!["alice".to_string()]);
    }

    #[test]
    fn card_thread_empty_reporter_skipped() {
        // Defensive: corrupt card.meta.yaml with empty created_by
        // shouldn't inject a blank handler into recipients.
        let m = msg(1, 0, "bob", vec![]);
        let r = compute_card_thread_recipients(&m, &card("", Some("bob")));
        assert_eq!(r, vec!["bob".to_string()]);
    }
}
