# Agent Routing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Daemon attaches a `recipients: [handler...]` field to every message entry it emits via poll; runtime skips messages where self is not in recipients. Eliminates the N-agent-fan-out cascade on a single user message.

**Architecture:** A new pure function `compute_recipients(message, channel_meta, all_messages) -> Vec<String>` in `gitim-core` computes the union of (channel owner, parent-chain ancestors, explicit @mentions) for any channel message. `gitim-daemon::handlers::poll` enriches each channel-thread message JSON with this field; DM threads get `recipients = members` directly. `gitim-runtime::agent_loop::format_changes_as_prompt` adds a filter step: skip entries whose non-empty `recipients` array does not contain `self_handler`. Empty/missing `recipients` is a backward-compat broadcast fallback.

**Tech Stack:** Rust (workspace crates `gitim-core`, `gitim-daemon`, `gitim-runtime`), serde_json for opaque wire entries, BTreeSet for sorted dedup.

**Spec source of truth:** [docs/plans/agent-routing/00-requirements.md](00-requirements.md)

---

## File Structure

| Path | Action | Purpose |
|---|---|---|
| `crates/gitim-core/src/recipients.rs` | **Create** | Pure `compute_recipients` function + inline unit tests |
| `crates/gitim-core/src/lib.rs` | Modify | Add `pub mod recipients;` |
| `crates/gitim-daemon/src/handlers/poll.rs` | Modify | Inject `recipients` into message entry JSON for channel & DM kinds |
| `crates/gitim-daemon/tests/poll_recipients.rs` | **Create** | Integration test for daemon poll wire shape |
| `crates/gitim-runtime/src/agent_loop.rs` | Modify | Add recipients filter in `format_changes_as_prompt` |
| `crates/gitim-runtime/tests/agent_loop_routing.rs` | **Create** | Unit test for filter (pure function, doesn't need daemon) |

**Non-targets** (do not touch in this plan):
- `card_thread` / `cron_thread` entries — no recipients (v1 scope: channels + DMs only). Card discussions and cron fires preserve current broadcast/targeted behavior via the broadcast fallback.
- `ChannelMeta` schema — unchanged. `created_by` field reused as-is.
- WebUI / CLI — unchanged, they ignore the new JSON field.

---

## Task 1: `compute_recipients` pure function

**Files:**
- Create: `crates/gitim-core/src/recipients.rs`
- Modify: `crates/gitim-core/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/gitim-core/src/recipients.rs` with the function signature and an inline test module:

```rust
//! Compute the recipients set for a channel message.
//!
//! Recipients = union of:
//!   1. Channel owner (`ChannelMeta.created_by`)
//!   2. Parent-chain ancestor authors (walk `point_to` upward)
//!   3. Explicit @mentions in the message body
//!
//! Output is sorted (BTreeSet-derived) and deduped. Returned as
//! `Vec<String>` to match the wire format and ChannelMeta field types.
//!
//! DM channels are NOT handled here — callers inline `recipients =
//! members` for DM threads. This function is for channel threads only.

use crate::types::ChannelMeta;
use crate::types::message::Message;
use std::collections::{BTreeSet, HashSet};

pub fn compute_recipients(
    message: &Message,
    channel_meta: &ChannelMeta,
    all_messages: &[Message],
) -> Vec<String> {
    let mut recipients: BTreeSet<String> = BTreeSet::new();

    // Rule 1: channel owner
    if !channel_meta.created_by.is_empty() {
        recipients.insert(channel_meta.created_by.clone());
    }

    // Rule 2: parent chain — walk point_to upward, collect authors
    let mut cursor = message.point_to;
    let mut visited: HashSet<u64> = HashSet::new();
    while cursor != 0 && visited.insert(cursor) {
        match all_messages.iter().find(|m| m.line_number == cursor) {
            Some(ancestor) => {
                recipients.insert(ancestor.author.as_str().to_string());
                cursor = ancestor.point_to;
            }
            None => break,
        }
    }

    // Rule 3: explicit mentions
    for handler in &message.mentions {
        recipients.insert(handler.as_str().to_string());
    }

    recipients.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Handler;

    fn meta(created_by: &str) -> ChannelMeta {
        ChannelMeta {
            display_name: "test".into(),
            created_by: created_by.into(),
            created_at: "2026-05-17T00:00:00Z".into(),
            introduction: String::new(),
            members: vec![],
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
            vec![
                "alice".to_string(),
                "bob".to_string(),
                "owner".to_string()
            ]
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
    fn cycle_in_parent_chain_terminates() {
        // A self-pointing message: point_to == line_number
        let cyclic = msg(1, 1, "alice", vec![]);
        let new = msg(2, 1, "bob", vec![]);
        // all_messages contains the cyclic root
        let r = compute_recipients(&new, &meta("owner"), &[cyclic]);
        // alice (cyclic root author) gets in once, then the visited
        // set stops the walk; we don't infinite-loop and we don't
        // panic.
        assert_eq!(r, vec!["alice".to_string(), "owner".to_string()]);
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
        // Only bob from rule 3; rule 1 skipped because created_by empty
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
}
```

- [ ] **Step 2: Wire the module into the crate**

Edit `crates/gitim-core/src/lib.rs` — add the `pub mod recipients;` declaration alongside the other `pub mod` lines (keep alphabetical order with `parser` / `responses`):

```rust
pub mod parser;
pub mod recipients;  // ← new
pub mod responses;
```

- [ ] **Step 3: Run the tests to verify they pass**

Run: `cargo test -p gitim-core recipients`
Expected: 8 tests pass (`root_message_no_mentions_only_owner`, `root_message_with_mention_includes_owner_and_mentioned`, `reply_walks_parent_chain`, `reply_with_mention_dedups_against_chain`, `cycle_in_parent_chain_terminates`, `parent_chain_with_missing_ancestor_breaks_cleanly`, `empty_created_by_skips_rule_1`, `self_mention_included_caller_dedups_at_consumption`)

- [ ] **Step 4: Format and commit**

```bash
cargo fmt -p gitim-core
git add crates/gitim-core/src/recipients.rs crates/gitim-core/src/lib.rs
git commit -m "feat(core): add compute_recipients pure function

Implements the 3-rule routing policy (channel owner, parent chain
ancestors, explicit mentions) as a pure function with no IO. Output is
sorted-deduped Vec<String> matching wire format and ChannelMeta types."
```

---

## Task 2: Inject `recipients` into daemon poll JSON

**Files:**
- Modify: `crates/gitim-daemon/src/handlers/poll.rs`
- Create: `crates/gitim-daemon/tests/poll_recipients.rs`

- [ ] **Step 1: Write the failing integration test**

Create `crates/gitim-daemon/tests/poll_recipients.rs`. Use the existing test scaffolding pattern (look at any sibling test file in `crates/gitim-daemon/tests/` for the standard repo+state setup; usually a helper that creates a temp git repo, writes channel meta + thread, then invokes `handle_poll` directly).

```rust
// Skeleton — adapt the helper imports/calls to match the existing
// poll_*.rs integration test pattern in crates/gitim-daemon/tests/.

mod common;  // existing test helper if present

use serde_json::Value;

#[tokio::test]
async fn poll_attaches_recipients_to_channel_message_entries() {
    let env = common::TestRepo::new().await;
    env.write_channel_meta("general", "owner-alice", &[]).await;
    env.commit_initial().await;
    env.append_message("general", 1, 0, "alice", "hello").await;
    env.append_message("general", 2, 1, "bob", "hi @charlie").await;
    env.commit("two messages").await;

    let response = env.poll_as("agent-x").await;
    let changes = response["changes"].as_array().unwrap();
    let general = changes
        .iter()
        .find(|c| c["channel"] == "general")
        .expect("general change present");
    let entries = general["entries"].as_array().unwrap();

    // First message (root): recipients == [owner-alice]
    assert_eq!(
        entries[0]["recipients"],
        serde_json::json!(["owner-alice"])
    );

    // Second message (reply to root @-ing charlie):
    // recipients == [alice (parent), charlie (mention), owner-alice (rule 1)]
    let rcps: Vec<String> = entries[1]["recipients"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(rcps, vec!["alice", "charlie", "owner-alice"]);
}

#[tokio::test]
async fn poll_attaches_recipients_to_dm_message_entries() {
    let env = common::TestRepo::new().await;
    env.write_dm_thread("alice", "bob").await;
    env.append_dm_message("alice", "bob", 1, 0, "alice", "hi").await;
    env.commit("dm message").await;

    let response = env.poll_as("alice").await;
    let changes = response["changes"].as_array().unwrap();
    let dm = changes
        .iter()
        .find(|c| c["kind"] == "dm")
        .expect("dm change present");
    let entries = dm["entries"].as_array().unwrap();
    let rcps: Vec<String> = entries[0]["recipients"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(rcps, vec!["alice", "bob"]);  // sorted, both members
}

#[tokio::test]
async fn poll_does_not_attach_recipients_to_event_entries() {
    let env = common::TestRepo::new().await;
    env.write_channel_meta("general", "owner", &[]).await;
    env.append_event("general", "join", "alice").await;
    env.commit("alice joins").await;

    let response = env.poll_as("agent-x").await;
    let changes = response["changes"].as_array().unwrap();
    let entries = changes[0]["entries"].as_array().unwrap();
    assert_eq!(entries[0]["type"], "event");
    assert!(entries[0].get("recipients").is_none(),
            "events must not carry recipients");
}
```

**If the existing test helper does not exist or lacks these methods**, build a minimal one inline in this test file (do not pollute `tests/common/mod.rs` with API surface specific to this feature; either reuse what's there or roll a 30-line local helper). The existing daemon tests under `crates/gitim-daemon/tests/` use `tempfile::TempDir` + manual git init + direct `handle_poll` calls — follow that pattern.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p gitim-daemon --test poll_recipients`
Expected: all three tests FAIL with assertions like `entries[0]["recipients"]` being `null` (field missing because daemon doesn't emit it yet).

- [ ] **Step 3: Implement recipients injection in `poll.rs`**

Edit `crates/gitim-daemon/src/handlers/poll.rs`. The change is in two places:

**Place A — channel-thread branch (around line 478-500)**:

Replace:

```rust
        // Parse added lines as entries (both messages and events)
        let parsed = match parse_thread(added_content) {
            Ok(f) => f,
            Err(e) => {
                warn!("poll: failed to parse diff for {}: {}", path_str, e);
                continue;
            }
        };

        if parsed.entries.is_empty() {
            continue;
        }

        let entries: Vec<serde_json::Value> = parsed
            .entries
            .iter()
            .map(|entry| entry_to_json(entry))
            .collect();
```

With:

```rust
        // Parse added lines as entries (both messages and events)
        let parsed = match parse_thread(added_content) {
            Ok(f) => f,
            Err(e) => {
                warn!("poll: failed to parse diff for {}: {}", path_str, e);
                continue;
            }
        };

        if parsed.entries.is_empty() {
            continue;
        }

        let entries: Vec<serde_json::Value> = enrich_entries_with_recipients(
            &parsed.entries,
            kind,
            &channel,
            &path_str,
            &state.repo_root,
        );
```

**Place B — card-thread branch (around line 216-225) and cron-thread branch (around line 410-419)**: leave unchanged. Card and cron threads use the plain `entry_to_json` mapping. They produce no `recipients` field — runtime broadcast fallback preserves current behavior. (Card and cron routing is non-target per spec.)

**Add a new helper** at the bottom of `poll.rs`, alongside `board_handler_from_path`:

```rust
/// Render thread entries to JSON, attaching a `recipients` field to
/// Message entries based on the channel kind:
///   - kind == "channel" → 3-rule routing via `compute_recipients`
///   - kind == "dm"      → recipients = [member_a, member_b] (sorted)
///   - other kinds       → no recipients (broadcast fallback)
/// Event entries never carry recipients regardless of kind.
fn enrich_entries_with_recipients(
    entries: &[ThreadEntry],
    kind: &str,
    channel: &str,
    path_str: &str,
    repo_root: &Path,
) -> Vec<serde_json::Value> {
    // Pre-load context needed to compute recipients.
    let channel_context: Option<(ChannelMeta, Vec<Message>)> = if kind == "channel" {
        let meta_path = repo_root
            .join("channels")
            .join(format!("{}.meta.yaml", channel));
        let thread_path = repo_root
            .join("channels")
            .join(format!("{}.thread", channel));
        let meta = std::fs::read_to_string(&meta_path)
            .ok()
            .and_then(|s| serde_yaml::from_str::<ChannelMeta>(&s).ok());
        let messages: Vec<Message> = std::fs::read_to_string(&thread_path)
            .ok()
            .and_then(|s| parse_thread(&s).ok())
            .map(|tf| {
                tf.entries
                    .into_iter()
                    .filter_map(|e| match e {
                        ThreadEntry::Message(m) => Some(m),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        meta.map(|m| (m, messages))
    } else {
        None
    };

    // For DM threads, derive sorted member pair from the filename stem.
    let dm_members: Option<Vec<String>> = if kind == "dm" {
        path_str
            .strip_prefix("dm/")
            .and_then(|s| s.strip_suffix(".thread"))
            .and_then(parse_dm_filename)
            .map(|(a, b)| {
                let mut v = vec![a.to_string(), b.to_string()];
                v.sort();
                v
            })
    } else {
        None
    };

    entries
        .iter()
        .map(|entry| {
            let mut json = entry_to_json(entry);
            if let ThreadEntry::Message(msg) = entry {
                let recipients: Option<Vec<String>> = match (kind, &channel_context, &dm_members) {
                    ("channel", Some((meta, msgs)), _) => {
                        let r = gitim_core::recipients::compute_recipients(msg, meta, msgs);
                        if r.is_empty() {
                            warn!(
                                "poll: empty recipients for {} L{}",
                                channel, msg.line_number
                            );
                            None
                        } else {
                            Some(r)
                        }
                    }
                    ("dm", _, Some(members)) => Some(members.clone()),
                    _ => None,
                };
                if let Some(r) = recipients {
                    json["recipients"] = serde_json::json!(r);
                }
            }
            json
        })
        .collect()
}
```

**Imports** at the top of `poll.rs`: add `Message`, `ThreadEntry` to the existing `use gitim_core::types::{...}` (currently has `ChannelMeta, Handler`), and `std::path::Path`:

```rust
use gitim_core::types::{ChannelMeta, Handler, Message, ThreadEntry};
use std::path::Path;
```

- [ ] **Step 4: Run the integration test to verify it passes**

Run: `cargo test -p gitim-daemon --test poll_recipients`
Expected: all three tests PASS.

- [ ] **Step 5: Re-run any other daemon tests that touch poll to confirm no regression**

Run: `cargo test -p gitim-daemon poll`
Expected: all existing poll tests still pass (the change is purely additive — JSON objects gain a new optional field, existing fields untouched).

- [ ] **Step 6: Format and commit**

```bash
cargo fmt -p gitim-daemon
git add crates/gitim-daemon/src/handlers/poll.rs crates/gitim-daemon/tests/poll_recipients.rs
git commit -m "feat(daemon): attach recipients to message entries in poll

For channel-kind entries, compute via gitim_core::recipients (owner,
parent chain, mentions). For dm-kind entries, recipients = sorted
member pair. Event entries never receive recipients. Card and cron
threads are not in v1 scope (broadcast fallback preserves current
behavior)."
```

---

## Task 3: Add recipients filter in `format_changes_as_prompt`

**Files:**
- Modify: `crates/gitim-runtime/src/agent_loop.rs`
- Create: `crates/gitim-runtime/tests/agent_loop_routing.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/gitim-runtime/tests/agent_loop_routing.rs`:

```rust
//! Tests for the recipients-based routing filter in
//! `format_changes_as_prompt`. Pure function tests — no daemon needed.

use gitim_runtime::agent_loop::format_changes_as_prompt;
use gitim_runtime::poller::ChannelChange;
use serde_json::json;

fn entry(line: u64, parent: u64, author: &str, body: &str, recipients: Option<Vec<&str>>) -> serde_json::Value {
    let mut v = json!({
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
        v["recipients"] = json!(r);
    }
    v
}

#[test]
fn skips_messages_where_self_not_in_recipients() {
    let changes = vec![ChannelChange {
        channel: "general".into(),
        kind: "channel".into(),
        entries: vec![entry(1, 0, "alice", "hello", Some(vec!["bob"]))],
    }];
    let prompt = format_changes_as_prompt(&changes, "charlie");
    assert!(prompt.is_none(), "charlie should be filtered out");
}

#[test]
fn includes_messages_where_self_in_recipients() {
    let changes = vec![ChannelChange {
        channel: "general".into(),
        kind: "channel".into(),
        entries: vec![entry(
            1,
            0,
            "alice",
            "hello",
            Some(vec!["bob", "charlie"]),
        )],
    }];
    let prompt = format_changes_as_prompt(&changes, "charlie").expect("should have prompt");
    assert!(prompt.contains("@alice"));
    assert!(prompt.contains("hello"));
}

#[test]
fn empty_recipients_broadcasts_to_all() {
    // Backward-compat fallback: empty or missing recipients = no filter,
    // every non-self message gets included (legacy behavior).
    let changes = vec![ChannelChange {
        channel: "general".into(),
        kind: "channel".into(),
        entries: vec![
            entry(1, 0, "alice", "hello", Some(vec![])),
            entry(2, 0, "alice", "world", None),
        ],
    }];
    let prompt = format_changes_as_prompt(&changes, "charlie").expect("should broadcast");
    assert!(prompt.contains("hello"));
    assert!(prompt.contains("world"));
}

#[test]
fn self_author_skip_takes_priority_over_recipients() {
    let changes = vec![ChannelChange {
        channel: "general".into(),
        kind: "channel".into(),
        entries: vec![entry(
            1,
            0,
            "charlie",
            "self note",
            Some(vec!["charlie"]),
        )],
    }];
    let prompt = format_changes_as_prompt(&changes, "charlie");
    assert!(prompt.is_none(), "self-authored messages always skipped");
}
```

Check whether `ChannelChange` and `format_changes_as_prompt` are `pub` from `gitim_runtime` (they need to be accessible from the external `tests/` dir). If not, expose them — they're already exposed for the existing `crates/gitim-runtime/tests/` directory (which uses them).

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p gitim-runtime --test agent_loop_routing`
Expected: `skips_messages_where_self_not_in_recipients` and `self_author_skip_takes_priority_over_recipients` FAIL because the filter isn't implemented yet (currently format_changes_as_prompt includes all non-self messages). The other two should pass even without the change.

- [ ] **Step 3: Implement the filter in `format_changes_as_prompt`**

Edit `crates/gitim-runtime/src/agent_loop.rs` around line 1086 (inside the `for entry in &change.entries` loop, AFTER the existing `author == self_handler` skip):

Current code:

```rust
        for entry in &change.entries {
            let author = entry["author"].as_str().unwrap_or("unknown");

            if author == self_handler {
                continue;
            }

            has_external = true;
```

Replace with:

```rust
        for entry in &change.entries {
            let author = entry["author"].as_str().unwrap_or("unknown");

            if author == self_handler {
                continue;
            }

            // Recipients-based routing filter.
            //
            // If the entry carries a non-empty `recipients` array and
            // self_handler is not in it, this message is not for us —
            // skip without prompting the LLM. This cuts the multi-agent
            // cascade where N agents in a channel each process every
            // user message.
            //
            // Empty array or missing field = backward-compat broadcast
            // fallback (old daemon, card/cron threads, or any future
            // entry kind that hasn't opted into recipients). Behavior
            // matches pre-routing semantics: all non-self messages
            // flow through.
            if let Some(recipients) = entry["recipients"].as_array() {
                if !recipients.is_empty()
                    && !recipients
                        .iter()
                        .any(|v| v.as_str() == Some(self_handler))
                {
                    continue;
                }
            }

            has_external = true;
```

Keep the rest of the function unchanged — `body.contains(&mention)` continues to set the `[MENTION]` display tag, which is useful signal for the LLM independent of routing.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p gitim-runtime --test agent_loop_routing`
Expected: all four tests PASS.

- [ ] **Step 5: Run sibling agent_loop tests to confirm no regression**

Run: `cargo test -p gitim-runtime agent_loop`
Expected: existing tests still pass. Particularly:
- `detect_steering_trigger` tests are independent of the filter (it uses body.contains + 急急急, not recipients)
- Any existing `format_changes_as_prompt` test that didn't set recipients should still pass via the broadcast fallback

If a pre-existing test now fails because it implicitly relied on "all messages go through with no recipients field," update that test to either set `recipients` explicitly or document its broadcast intent.

- [ ] **Step 6: Format and commit**

```bash
cargo fmt -p gitim-runtime
git add crates/gitim-runtime/src/agent_loop.rs crates/gitim-runtime/tests/agent_loop_routing.rs
git commit -m "feat(runtime): filter prompt entries by recipients

Adds a routing filter to format_changes_as_prompt: skip entries
whose non-empty recipients array does not contain self_handler.
Empty or missing recipients keeps the legacy broadcast behavior
(card/cron threads, old daemons). [MENTION] display tag unchanged."
```

---

## Task 4: Workspace test + orientation update

**Files:**
- Modify: `CLAUDE.md` (Current Orientation section)

- [ ] **Step 1: Run the full test suite**

Run: `cargo test --workspace`
Expected: All tests pass. Pay attention to `gitim-runtime` poller integration tests (they spawn a real daemon and round-trip a poll) — if any of those parse a JSON entry shape, confirm the new `recipients` field doesn't break their assertions. If a test fails, fix it before proceeding.

- [ ] **Step 2: Update `Current Orientation` in CLAUDE.md**

Append one sentence to the "Where we are" bullet describing agent routing. Find the line beginning `**Where we are**` in `CLAUDE.md`, and add this clause:

> **Agent routing v1** 已落地:daemon 在 poll 返回时给每条 channel/DM message entry 附 `recipients: [handler...]`(channel: owner + parent-chain + mentions; DM: 双方);runtime `format_changes_as_prompt` 加过滤,self 不在非空 recipients 就 skip。Cascade(N agent 同时处理同一条消息)由此 cap 在路由命中的 agent 集合。Card / cron threads 不在 v1 scope,走 broadcast fallback。`ChannelMeta.created_by` 直接当群主,immutable;群主转让 / per-agent `responds_to` / agent-agent cascade 深度收敛是 non-goal,留给 v2。

- [ ] **Step 3: Commit the orientation update**

```bash
git add CLAUDE.md
git commit -m "docs: record agent routing v1 in current orientation"
```

- [ ] **Step 4: Final full test pass**

Run: `cargo test --workspace`
Expected: Everything green. This is the gate before requesting code review.

---

## Self-Review Checklist (run after writing this plan, before handing to implementer)

**Spec coverage:**
- [x] P1 three rules → Task 1 `compute_recipients`
- [x] P2 agent-only scope → Task 3 (filter lives in runtime only; daemon emits for all callers, WebUI/CLI ignore)
- [x] P3 daemon-side computation → Task 2
- [x] P4 reuse `created_by` → Task 1 reads from `ChannelMeta.created_by` directly, no schema change
- [x] P5 JSON-field wire format → Task 2 injects into Value object, no wrapper struct
- [x] P6 DM bypass → Task 2 `enrich_entries_with_recipients` DM branch returns sorted member pair
- [x] P7 keep [MENTION] tag, add filter → Task 3 leaves body.contains alone, adds recipients check
- [x] Empty recipients fallback → Task 3 explicit broadcast on empty/missing
- [x] Edge cases (cycle, missing parent, empty created_by, self-mention) → Task 1 unit tests
- [x] Test matrix (core unit / daemon integration / runtime unit) → Tasks 1-3

**Placeholder scan:** No TBD/TODO; every code block contains the actual code; every test has actual assertions.

**Type consistency:**
- `Vec<String>` output from `compute_recipients` — matches wire format and `ChannelMeta.members: Vec<String>` / `created_by: String`
- `Handler::as_str()` used consistently to convert Handler → &str → String
- `serde_json::json!(...)` used consistently in JSON construction

**Non-goals explicit:** Card/cron threads, ChannelMeta schema changes, WebUI changes, owner transfer, agent-agent cascade depth.
