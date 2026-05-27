use gitim_core::auth_payload::AuthPayload;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "event")]
pub enum Event {
    #[serde(rename = "thread_changed")]
    ThreadChanged { channel: String, kind: String },

    #[serde(rename = "messages_pushed")]
    MessagesPushed {
        channel: String,
        line_numbers: Vec<u64>,
    },

    #[serde(rename = "message_renumbered")]
    MessageRenumbered {
        channel: String,
        old_line: u64,
        new_line: u64,
    },

    #[serde(rename = "membership_changed")]
    MembershipChanged {
        channel: String,
        event_type: String,
        author: String,
        targets: Vec<String>,
    },

    #[serde(rename = "card_created")]
    CardCreated { channel: String, card_id: String },

    #[serde(rename = "card_status_changed")]
    CardStatusChanged {
        channel: String,
        card_id: String,
        old_status: String,
        new_status: String,
        author: String,
    },

    #[serde(rename = "card_message_appended")]
    CardMessageAppended {
        channel: String,
        card_id: String,
        line_numbers: Vec<u64>,
    },

    #[serde(rename = "card_archived")]
    CardArchived {
        channel: String,
        card_id: String,
        author: String,
    },

    #[serde(rename = "card_unarchived")]
    CardUnarchived {
        channel: String,
        card_id: String,
        author: String,
    },

    #[serde(rename = "board_updated")]
    BoardUpdated { handler: String },

    #[serde(rename = "flow_changed")]
    FlowChanged { slug: String },

    #[serde(rename = "flow_run_started")]
    FlowRunStarted {
        run_id: String,
        flow_slug: String,
        channel: String,
    },
    #[serde(rename = "flow_run_node_updated")]
    FlowRunNodeUpdated {
        run_id: String,
        node_id: String,
        status: String,
    },
    #[serde(rename = "flow_run_completed")]
    FlowRunCompleted { run_id: String, status: String },

    #[serde(rename = "channel_unarchived")]
    ChannelUnarchived {
        channel: String,
        author: String,
        timestamp: String,
    },

    #[serde(rename = "user_archived")]
    UserArchived {
        handler: String,
        archived_by: String,
        timestamp: String,
    },

    #[serde(rename = "user_unarchived")]
    UserUnarchived {
        handler: String,
        unarchived_by: String,
        timestamp: String,
    },

    #[serde(rename = "dm_archived")]
    DmArchived {
        peer: String,
        archived_by: String,
        timestamp: String,
    },

    #[serde(rename = "dm_unarchived")]
    DmUnarchived {
        peer: String,
        unarchived_by: String,
        timestamp: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "method")]
pub enum Request {
    #[serde(rename = "send")]
    Send {
        channel: String,
        body: String,
        #[serde(default)]
        reply_to: Option<u64>,
        #[serde(default)]
        author: Option<String>,
    },
    #[serde(rename = "read")]
    Read {
        channel: String,
        limit: Option<usize>,
        since: Option<u64>,
    },
    #[serde(rename = "channels")]
    ListChannels,
    #[serde(rename = "users")]
    ListUsers {
        /// When true, daemon also returns the archived handlers in
        /// the response's `archived` field. Per archive-protocol P2.a
        /// — caller-uniform: every caller (CLI, WebUI, agent) flips the
        /// same flag, daemon does not gate on caller type. Default
        /// false keeps the legacy single-list behavior.
        #[serde(default)]
        include_archived: bool,
    },
    #[serde(rename = "thread")]
    GetThread { channel: String, line_number: u64 },
    #[serde(rename = "status")]
    Status,
    #[serde(rename = "subscribe")]
    Subscribe,
    #[serde(rename = "stop")]
    Stop,
    #[serde(rename = "poll")]
    Poll {
        #[serde(default)]
        since: Option<String>,
    },
    #[serde(rename = "register_user")]
    RegisterUser {
        handler: String,
        display_name: String,
        #[serde(default = "default_role")]
        role: String,
        #[serde(default = "default_introduction")]
        introduction: String,
    },
    /// Overwrite an already-registered user's `introduction` field.
    /// Used by `PATCH /workspaces/{slug}/agents/{id}` and by the post-onboard
    /// step in `POST /agents/add` when the WebUI submits an initial blurb.
    /// Requires the user to already exist (returns error otherwise) — this
    /// is an update, not a create. Empty string clears the field.
    #[serde(rename = "update_user")]
    UpdateUser {
        handler: String,
        introduction: String,
    },
    #[serde(rename = "onboard")]
    Onboard {
        git_server: String,
        /// Identity payload. Required for non-guest onboards. Guest mode
        /// sends `null` (or omits the field) — daemon ignores it.
        #[serde(default)]
        auth: Option<AuthPayload>,
        #[serde(default)]
        admin: bool,
        #[serde(default)]
        guest: bool,
        /// When false, daemon skips the auto_join_general step on first
        /// registration. Default true preserves the legacy behavior for
        /// any caller (CLI human onboard, runtime workspace owner) that
        /// doesn't set the field — only opt-out callers (runtime agent
        /// provision via POST /agents/add) need to send `false`.
        #[serde(default = "default_true")]
        join_general: bool,
    },
    #[serde(rename = "join_channel")]
    JoinChannel {
        channel: String,
        #[serde(default)]
        targets: Vec<String>,
        #[serde(default)]
        author: Option<String>,
    },
    #[serde(rename = "leave_channel")]
    LeaveChannel {
        channel: String,
        #[serde(default)]
        targets: Vec<String>,
        #[serde(default)]
        author: Option<String>,
    },
    #[serde(rename = "create_channel")]
    CreateChannel {
        name: String,
        #[serde(default)]
        display_name: Option<String>,
        #[serde(default)]
        introduction: Option<String>,
        #[serde(default)]
        author: Option<String>,
        #[serde(default)]
        invitees: Vec<String>,
    },
    #[serde(rename = "search")]
    Search {
        #[serde(default)]
        query: Option<String>,
        #[serde(default)]
        author: Option<String>,
        #[serde(default)]
        channel: Option<String>,
        #[serde(default)]
        channel_type: Option<String>,
        #[serde(default = "default_limit")]
        limit: usize,
        #[serde(default)]
        offset: usize,
        #[serde(default)]
        include_cards: bool,
    },
    #[serde(rename = "reindex")]
    Reindex,
    #[serde(rename = "archive_channel")]
    ArchiveChannel {
        channel: String,
        #[serde(default)]
        author: Option<String>,
    },
    #[serde(rename = "unarchive_channel")]
    UnarchiveChannel {
        channel: String,
        #[serde(default)]
        author: Option<String>,
    },
    #[serde(rename = "archived_channels")]
    ListArchivedChannels {
        #[serde(default)]
        prefix: Option<String>,
        #[serde(default)]
        offset: usize,
        #[serde(default = "default_archived_channels_limit")]
        limit: usize,
    },
    #[serde(rename = "create_card")]
    CreateCard {
        channel: String,
        title: String,
        #[serde(default)]
        labels: Option<Vec<String>>,
        #[serde(default)]
        assignee: Option<String>,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        author: Option<String>,
    },
    #[serde(rename = "list_cards")]
    ListCards {
        #[serde(default)]
        channel: Option<String>,
        #[serde(default)]
        labels: Option<Vec<String>>,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        assignee: Option<String>,
    },
    #[serde(rename = "read_card")]
    ReadCard {
        channel: String,
        card_id: String,
        #[serde(default)]
        limit: Option<usize>,
        #[serde(default)]
        since: Option<u64>,
    },
    #[serde(rename = "send_card_message")]
    SendCardMessage {
        channel: String,
        card_id: String,
        body: String,
        #[serde(default)]
        reply_to: Option<u64>,
        #[serde(default)]
        author: Option<String>,
    },
    #[serde(rename = "update_card")]
    UpdateCard {
        channel: String,
        card_id: String,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        labels: Option<Vec<String>>,
        #[serde(default)]
        assignee: Option<String>,
        #[serde(default)]
        author: Option<String>,
    },
    #[serde(rename = "archive_card")]
    ArchiveCard {
        channel: String,
        card_id: String,
        author: String,
    },
    #[serde(rename = "unarchive_card")]
    UnarchiveCard {
        channel: String,
        card_id: String,
        author: String,
    },
    #[serde(rename = "list_archived_cards")]
    ListArchivedCards {
        #[serde(default)]
        channel: Option<String>,
    },
    #[serde(rename = "archive_user")]
    ArchiveUser {
        handler: String,
        #[serde(default)]
        author: Option<String>,
    },
    #[serde(rename = "unarchive_user")]
    UnarchiveUser {
        handler: String,
        #[serde(default)]
        author: Option<String>,
    },
    #[serde(rename = "archive_dm")]
    ArchiveDm {
        peer: String,
        #[serde(default)]
        author: Option<String>,
    },
    #[serde(rename = "unarchive_dm")]
    UnarchiveDm {
        peer: String,
        #[serde(default)]
        author: Option<String>,
    },
    #[serde(rename = "list_archived_users")]
    ListArchivedUsers,
    #[serde(rename = "list_archived_dms")]
    ListArchivedDms {
        #[serde(default)]
        author: Option<String>,
        #[serde(default)]
        prefix: Option<String>,
        #[serde(default)]
        offset: usize,
        #[serde(default = "default_archived_dms_limit")]
        limit: usize,
    },
    #[serde(rename = "depart_user")]
    DepartUser { handler: String },
    #[serde(rename = "board_show")]
    BoardShow { handler: String },
    #[serde(rename = "board_list")]
    BoardList,
    #[serde(rename = "board_init")]
    BoardInit {
        #[serde(default)]
        author: Option<String>,
    },
    #[serde(rename = "board_publish")]
    BoardPublish {
        #[serde(default)]
        content: Option<String>,
        #[serde(default)]
        author: Option<String>,
    },
    #[serde(rename = "board_set")]
    BoardSet {
        field: String,
        value: String,
        #[serde(default)]
        author: Option<String>,
    },
    #[serde(rename = "board_section_set")]
    BoardSectionSet {
        section: String,
        value: String,
        #[serde(default)]
        author: Option<String>,
    },
    #[serde(rename = "board_section_append")]
    BoardSectionAppend {
        section: String,
        value: String,
        #[serde(default)]
        author: Option<String>,
    },

    // -- Flow triggers --
    #[serde(rename = "flow_list")]
    FlowList,
    #[serde(rename = "flow_show")]
    FlowShow { slug: String },
    #[serde(rename = "flow_create")]
    FlowCreate {
        slug: String,
        name: String,
        #[serde(default)]
        description: String,
        author: Option<String>,
    },
    #[serde(rename = "flow_remove")]
    FlowRemove {
        slug: String,
        author: Option<String>,
    },
    #[serde(rename = "flow_validate")]
    FlowValidate { slug: String },
    /// Replace a single node's prompt body. Frontmatter fields stay
    /// immutable on this path — node add/remove and meta edits still
    /// flow through `flow_create` / `flow_remove` / direct repo edits.
    #[serde(rename = "flow_update_node")]
    FlowUpdateNode {
        slug: String,
        node_id: String,
        prompt: String,
        author: Option<String>,
    },

    #[serde(rename = "flow_run_start")]
    FlowRunStart {
        slug: String,
        channel: String,
        author: Option<String>,
    },
    #[serde(rename = "flow_run_list")]
    FlowRunList {
        #[serde(default)]
        slug: Option<String>,
        #[serde(default)]
        channel: Option<String>,
        #[serde(default)]
        status: Option<String>,
    },
    #[serde(rename = "flow_run_show")]
    FlowRunShow { run_id: String },
    #[serde(rename = "flow_node_set")]
    FlowNodeSet {
        run_id: String,
        node_id: String,
        status: String,
        #[serde(default)]
        actor: Option<String>,
        #[serde(default)]
        result_ref: Option<String>,
        author: Option<String>,
    },
    #[serde(rename = "flow_run_cancel")]
    FlowRunCancel {
        run_id: String,
        author: Option<String>,
    },

    // -- Cron triggers --
    //
    // `name` semantics for these variants: the on-disk directory stem
    // under `crons/<name>/`. Validation rules (length, charset, reserved
    // words) live in the handler — surfacing here as `String` keeps the
    // wire schema flat and lets the daemon return a typed
    // `error_code: invalid_name` rather than a serde failure.
    /// Create a new cron trigger. `target` accepts the literal handler
    /// or the special string `@self`, which the daemon resolves to the
    /// author handler at create time.
    #[serde(rename = "create_cron")]
    CreateCron {
        name: String,
        schedule: String,
        #[serde(default)]
        timezone: Option<String>,
        target: String,
        prompt: String,
        #[serde(default)]
        author: Option<String>,
    },
    /// List all active (non-archived) cron triggers in the workspace,
    /// sorted by name.
    #[serde(rename = "list_crons")]
    ListCrons,
    /// Read full spec for a single cron, plus its most-recent past fires
    /// and the computed next-fire timestamp.
    #[serde(rename = "show_cron")]
    ShowCron { name: String },
    /// Read past fires for a cron, newest first. `limit` defaults to 50
    /// and is capped at the daemon side.
    #[serde(rename = "history_cron")]
    HistoryCron {
        name: String,
        #[serde(default)]
        limit: Option<u32>,
    },
    /// Flip `spec.yaml#enabled` to `true`. Idempotent: re-enable on an
    /// already-enabled spec produces no commit.
    #[serde(rename = "enable_cron")]
    EnableCron {
        name: String,
        #[serde(default)]
        author: Option<String>,
    },
    /// Flip `spec.yaml#enabled` to `false`. Idempotent: disable on an
    /// already-disabled spec produces no commit.
    #[serde(rename = "disable_cron")]
    DisableCron {
        name: String,
        #[serde(default)]
        author: Option<String>,
    },
    /// Soft-delete: `git mv crons/<name>/ archive/crons/<name>/`. Mirrors
    /// the channel-archive precedent — history is preserved under
    /// `archive/`, not `git rm`'d.
    #[serde(rename = "delete_cron")]
    DeleteCron {
        name: String,
        #[serde(default)]
        author: Option<String>,
    },

    // ===== Unified labels space (docs/plans/unified-labels/) =====
    /// Add labels to a user's `users/<target>.meta.yaml`. Self-claim only —
    /// daemon rejects if target != daemon's bound handler.
    #[serde(rename = "labels_add")]
    LabelsAdd { target: String, labels: Vec<String> },

    /// Remove labels from a user's `users/<target>.meta.yaml`. Self-claim only.
    #[serde(rename = "labels_remove")]
    LabelsRemove { target: String, labels: Vec<String> },

    /// List labels for any active user. Returns 404 for unknown or departed
    /// handlers (excludes `archive/users/`).
    #[serde(rename = "labels_list")]
    LabelsList { target: String },

    /// Find active agents whose `users/<h>.meta.yaml.labels` is a superset
    /// of the query labels (all-of subset match). Empty query → empty result.
    /// Excludes departed handlers.
    #[serde(rename = "agents_with_labels")]
    AgentsWithLabels { labels: Vec<String> },
}

fn default_limit() -> usize {
    50
}

fn default_archived_dms_limit() -> usize {
    5
}

fn default_archived_channels_limit() -> usize {
    10
}

fn default_role() -> String {
    "member".to_string()
}

fn default_introduction() -> String {
    "GitIM user".to_string()
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    // Request deserialization tests: Request only derives Deserialize (wire format comes in as JSON).
    // We construct the JSON string directly (as clients would send it) and verify deserialization.

    #[test]
    fn test_archive_card_request_roundtrip() {
        let json = r#"{"method":"archive_card","channel":"foo","card_id":"abc","author":"lewis"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req {
            Request::ArchiveCard {
                channel,
                card_id,
                author,
            } => {
                assert_eq!(channel, "foo");
                assert_eq!(card_id, "abc");
                assert_eq!(author, "lewis");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_unarchive_card_request_roundtrip() {
        let json =
            r#"{"method":"unarchive_card","channel":"bar","card_id":"xyz","author":"alice"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req {
            Request::UnarchiveCard {
                channel,
                card_id,
                author,
            } => {
                assert_eq!(channel, "bar");
                assert_eq!(card_id, "xyz");
                assert_eq!(author, "alice");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_list_archived_cards_request_roundtrip() {
        // With channel
        let json = r#"{"method":"list_archived_cards","channel":"eng"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req {
            Request::ListArchivedCards { channel } => assert_eq!(channel, Some("eng".to_string())),
            _ => panic!("wrong variant"),
        }

        // Without channel — serde(default) means omitting the field deserializes to None
        let json_no_ch = r#"{"method":"list_archived_cards"}"#;
        let req2: Request = serde_json::from_str(json_no_ch).unwrap();
        match req2 {
            Request::ListArchivedCards { channel } => assert_eq!(channel, None),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn archived_channels_request_carries_pagination() {
        let json = r#"{"method":"archived_channels","prefix":"eng","offset":10,"limit":20}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req {
            Request::ListArchivedChannels {
                prefix,
                offset,
                limit,
            } => {
                assert_eq!(prefix.as_deref(), Some("eng"));
                assert_eq!(offset, 10);
                assert_eq!(limit, 20);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn archived_channels_request_defaults() {
        let json = r#"{"method":"archived_channels"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req {
            Request::ListArchivedChannels {
                prefix,
                offset,
                limit,
            } => {
                assert_eq!(prefix, None);
                assert_eq!(offset, 0);
                assert_eq!(limit, 10);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn list_archived_dms_request_carries_pagination() {
        let json = r#"{"method":"list_archived_dms","author":"alice","prefix":"bo","offset":5,"limit":10}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req {
            Request::ListArchivedDms {
                author,
                prefix,
                offset,
                limit,
            } => {
                assert_eq!(author.as_deref(), Some("alice"));
                assert_eq!(prefix.as_deref(), Some("bo"));
                assert_eq!(offset, 5);
                assert_eq!(limit, 10);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn list_archived_dms_request_defaults() {
        let json = r#"{"method":"list_archived_dms"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req {
            Request::ListArchivedDms {
                author,
                prefix,
                offset,
                limit,
            } => {
                assert!(author.is_none());
                assert!(prefix.is_none());
                assert_eq!(offset, 0);
                assert_eq!(limit, 5);
            }
            _ => panic!("wrong variant"),
        }
    }

    // Event serialization tests: Event derives Serialize for SSE push to clients.

    #[test]
    fn test_card_archived_event_roundtrip() {
        let ev = Event::CardArchived {
            channel: "general".to_string(),
            card_id: "card-1".to_string(),
            author: "bob".to_string(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(
            json.contains("\"event\":\"card_archived\""),
            "json was: {json}"
        );
        assert!(json.contains("\"channel\":\"general\""));
        assert!(json.contains("\"card_id\":\"card-1\""));
        assert!(json.contains("\"author\":\"bob\""));
    }

    #[test]
    fn test_card_unarchived_event_roundtrip() {
        let ev = Event::CardUnarchived {
            channel: "design".to_string(),
            card_id: "card-2".to_string(),
            author: "carol".to_string(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(
            json.contains("\"event\":\"card_unarchived\""),
            "json was: {json}"
        );
        assert!(json.contains("\"channel\":\"design\""));
        assert!(json.contains("\"card_id\":\"card-2\""));
        assert!(json.contains("\"author\":\"carol\""));
    }

    #[test]
    fn test_unarchive_channel_request_roundtrip() {
        let json = r#"{"method":"unarchive_channel","channel":"design","author":"lewis"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req {
            Request::UnarchiveChannel { channel, author } => {
                assert_eq!(channel, "design");
                assert_eq!(author, Some("lewis".to_string()));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_channel_unarchived_event_roundtrip() {
        let ev = Event::ChannelUnarchived {
            channel: "design".to_string(),
            author: "lewis".to_string(),
            timestamp: "20260418T120000Z".to_string(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(
            json.contains("\"event\":\"channel_unarchived\""),
            "json was: {json}"
        );
        assert!(json.contains("\"channel\":\"design\""));
        assert!(json.contains("\"author\":\"lewis\""));
        assert!(json.contains("\"timestamp\":\"20260418T120000Z\""));
    }

    #[test]
    fn test_user_archived_event_roundtrip() {
        let ev = Event::UserArchived {
            handler: "alice".to_string(),
            archived_by: "lewis".to_string(),
            timestamp: "20260418T120000Z".to_string(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(
            json.contains("\"event\":\"user_archived\""),
            "json was: {json}"
        );
        assert!(json.contains("\"handler\":\"alice\""));
        assert!(json.contains("\"archived_by\":\"lewis\""));
        assert!(json.contains("\"timestamp\":\"20260418T120000Z\""));
    }

    #[test]
    fn test_user_unarchived_event_roundtrip() {
        let ev = Event::UserUnarchived {
            handler: "alice".to_string(),
            unarchived_by: "bob".to_string(),
            timestamp: "20260418T120000Z".to_string(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(
            json.contains("\"event\":\"user_unarchived\""),
            "json was: {json}"
        );
        assert!(json.contains("\"handler\":\"alice\""));
        assert!(json.contains("\"unarchived_by\":\"bob\""));
        assert!(json.contains("\"timestamp\":\"20260418T120000Z\""));
    }

    #[test]
    fn test_dm_archived_event_roundtrip() {
        let ev = Event::DmArchived {
            peer: "bob".to_string(),
            archived_by: "alice".to_string(),
            timestamp: "20260509T120000Z".to_string(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(
            json.contains("\"event\":\"dm_archived\""),
            "json was: {json}"
        );
        assert!(json.contains("\"peer\":\"bob\""));
        assert!(json.contains("\"archived_by\":\"alice\""));
        assert!(json.contains("\"timestamp\":\"20260509T120000Z\""));
    }

    #[test]
    fn test_dm_unarchived_event_roundtrip() {
        let ev = Event::DmUnarchived {
            peer: "bob".to_string(),
            unarchived_by: "alice".to_string(),
            timestamp: "20260509T120000Z".to_string(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(
            json.contains("\"event\":\"dm_unarchived\""),
            "json was: {json}"
        );
        assert!(json.contains("\"peer\":\"bob\""));
        assert!(json.contains("\"unarchived_by\":\"alice\""));
        assert!(json.contains("\"timestamp\":\"20260509T120000Z\""));
    }

    #[test]
    fn test_archive_user_request_roundtrip() {
        let json = r#"{"method":"archive_user","handler":"alice","author":"lewis"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req {
            Request::ArchiveUser { handler, author } => {
                assert_eq!(handler, "alice");
                assert_eq!(author, Some("lewis".to_string()));
            }
            _ => panic!("wrong variant"),
        }

        // Author omitted — serde(default) deserializes to None; resolve_author
        // fills it in dispatch.
        let json_no_author = r#"{"method":"archive_user","handler":"alice"}"#;
        let req2: Request = serde_json::from_str(json_no_author).unwrap();
        match req2 {
            Request::ArchiveUser { handler, author } => {
                assert_eq!(handler, "alice");
                assert_eq!(author, None);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_unarchive_user_request_roundtrip() {
        let json = r#"{"method":"unarchive_user","handler":"bob","author":"carol"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req {
            Request::UnarchiveUser { handler, author } => {
                assert_eq!(handler, "bob");
                assert_eq!(author, Some("carol".to_string()));
            }
            _ => panic!("wrong variant"),
        }

        let json_no_author = r#"{"method":"unarchive_user","handler":"bob"}"#;
        let req2: Request = serde_json::from_str(json_no_author).unwrap();
        match req2 {
            Request::UnarchiveUser { handler, author } => {
                assert_eq!(handler, "bob");
                assert_eq!(author, None);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_archive_dm_request_roundtrip() {
        let json = r#"{"method":"archive_dm","peer":"bob","author":"alice"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req {
            Request::ArchiveDm { peer, author } => {
                assert_eq!(peer, "bob");
                assert_eq!(author, Some("alice".to_string()));
            }
            _ => panic!("wrong variant"),
        }

        let json_no_author = r#"{"method":"archive_dm","peer":"bob"}"#;
        let req2: Request = serde_json::from_str(json_no_author).unwrap();
        match req2 {
            Request::ArchiveDm { peer, author } => {
                assert_eq!(peer, "bob");
                assert_eq!(author, None);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_unarchive_dm_request_roundtrip() {
        let json = r#"{"method":"unarchive_dm","peer":"bob","author":"alice"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req {
            Request::UnarchiveDm { peer, author } => {
                assert_eq!(peer, "bob");
                assert_eq!(author, Some("alice".to_string()));
            }
            _ => panic!("wrong variant"),
        }

        let json_no_author = r#"{"method":"unarchive_dm","peer":"bob"}"#;
        let req2: Request = serde_json::from_str(json_no_author).unwrap();
        match req2 {
            Request::UnarchiveDm { peer, author } => {
                assert_eq!(peer, "bob");
                assert_eq!(author, None);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_list_archived_users_request_roundtrip() {
        let json = r#"{"method":"list_archived_users"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req {
            Request::ListArchivedUsers => {}
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_list_archived_dms_request_roundtrip() {
        // With author
        let json = r#"{"method":"list_archived_dms","author":"alice"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req {
            Request::ListArchivedDms { author, .. } => {
                assert_eq!(author, Some("alice".to_string()));
            }
            _ => panic!("wrong variant"),
        }

        // Without author — resolved by dispatch via resolve_author
        let json_no_author = r#"{"method":"list_archived_dms"}"#;
        let req2: Request = serde_json::from_str(json_no_author).unwrap();
        match req2 {
            Request::ListArchivedDms { author, .. } => assert_eq!(author, None),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_list_users_request_roundtrip() {
        // include_archived omitted — serde(default) deserializes to false.
        let json = r#"{"method":"users"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req {
            Request::ListUsers { include_archived } => {
                assert!(!include_archived, "default should be false");
            }
            _ => panic!("wrong variant"),
        }

        // Explicit false.
        let json_false = r#"{"method":"users","include_archived":false}"#;
        let req2: Request = serde_json::from_str(json_false).unwrap();
        match req2 {
            Request::ListUsers { include_archived } => {
                assert!(!include_archived);
            }
            _ => panic!("wrong variant"),
        }

        // Explicit true — caller-uniform per P2.a.
        let json_true = r#"{"method":"users","include_archived":true}"#;
        let req3: Request = serde_json::from_str(json_true).unwrap();
        match req3 {
            Request::ListUsers { include_archived } => {
                assert!(include_archived);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_depart_user_request_roundtrip() {
        let json = r#"{"method":"depart_user","handler":"alice"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req {
            Request::DepartUser { handler } => {
                assert_eq!(handler, "alice");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_board_publish_request_roundtrip() {
        let json = r#"{"method":"board_publish","content":"---\nversion: 1\nhandler: alice\nupdated_at: 20260509T120000Z\nstatus: working\nsummary: x\ntags: []\n---\nbody\n","author":"alice"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req {
            Request::BoardPublish { content, author } => {
                assert!(content.unwrap().contains("handler: alice"));
                assert_eq!(author, Some("alice".to_string()));
            }
            _ => panic!("wrong variant"),
        }
    }

    // -- Cron request roundtrip tests --

    #[test]
    fn test_create_cron_request_roundtrip_full() {
        let json = r#"{"method":"create_cron","name":"weekly-report","schedule":"0 9 * * 1","timezone":"America/Los_Angeles","target":"alice","prompt":"weekly checkin","author":"alice"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req {
            Request::CreateCron {
                name,
                schedule,
                timezone,
                target,
                prompt,
                author,
            } => {
                assert_eq!(name, "weekly-report");
                assert_eq!(schedule, "0 9 * * 1");
                assert_eq!(timezone, Some("America/Los_Angeles".to_string()));
                assert_eq!(target, "alice");
                assert_eq!(prompt, "weekly checkin");
                assert_eq!(author, Some("alice".to_string()));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_board_updated_event_roundtrip() {
        let ev = Event::BoardUpdated {
            handler: "alice".to_string(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(
            json.contains("\"event\":\"board_updated\""),
            "json was: {json}"
        );
        assert!(json.contains("\"handler\":\"alice\""));
    }

    #[test]
    fn request_onboard_join_general_omitted_defaults_to_true() {
        // Caller omits join_general entirely — must deserialize to true to
        // preserve legacy CLI / workspace-owner behavior. This pins the
        // default_true semantic at the daemon's wire boundary so a future
        // non-Rust IPC client can rely on the same default.
        let json = r#"{"method":"onboard","git_server":"git","auth":{"type":"git","handler":"alice","display_name":"Alice"}}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req {
            Request::Onboard { join_general, .. } => {
                assert!(join_general, "omitted join_general must default to true");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_create_cron_request_minimal_optionals_default_none() {
        // timezone + author both omitted — `serde(default)` deserializes them
        // to `None`, which the handler interprets as UTC + dispatch-resolved
        // author respectively.
        let json = r#"{"method":"create_cron","name":"daily","schedule":"@daily","target":"@self","prompt":"hi"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req {
            Request::CreateCron {
                name,
                schedule,
                timezone,
                target,
                prompt,
                author,
            } => {
                assert_eq!(name, "daily");
                assert_eq!(schedule, "@daily");
                assert_eq!(timezone, None);
                assert_eq!(target, "@self");
                assert_eq!(prompt, "hi");
                assert_eq!(author, None);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_onboard_join_general_explicit_false_deserializes() {
        // Opt-out caller (runtime agent provision) sends join_general=false
        // explicitly — the daemon must honor it.
        let json = r#"{"method":"onboard","git_server":"git","auth":{"type":"git","handler":"bob","display_name":"Bob"},"join_general":false}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req {
            Request::Onboard { join_general, .. } => {
                assert!(!join_general, "explicit false must deserialize as false");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_list_crons_request_roundtrip() {
        let json = r#"{"method":"list_crons"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req {
            Request::ListCrons => {}
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_show_cron_request_roundtrip() {
        let json = r#"{"method":"show_cron","name":"weekly-report"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req {
            Request::ShowCron { name } => {
                assert_eq!(name, "weekly-report");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_history_cron_request_roundtrip() {
        // With explicit limit.
        let json = r#"{"method":"history_cron","name":"daily","limit":10}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req {
            Request::HistoryCron { name, limit } => {
                assert_eq!(name, "daily");
                assert_eq!(limit, Some(10));
            }
            _ => panic!("wrong variant"),
        }

        // Without limit — `serde(default)` → `None`, handler applies its default.
        let json_no_limit = r#"{"method":"history_cron","name":"daily"}"#;
        let req2: Request = serde_json::from_str(json_no_limit).unwrap();
        match req2 {
            Request::HistoryCron { name, limit } => {
                assert_eq!(name, "daily");
                assert_eq!(limit, None);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_enable_disable_cron_request_roundtrip() {
        let json = r#"{"method":"enable_cron","name":"weekly","author":"alice"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req {
            Request::EnableCron { name, author } => {
                assert_eq!(name, "weekly");
                assert_eq!(author, Some("alice".to_string()));
            }
            _ => panic!("wrong variant"),
        }

        let json = r#"{"method":"disable_cron","name":"weekly"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req {
            Request::DisableCron { name, author } => {
                assert_eq!(name, "weekly");
                assert_eq!(author, None);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_delete_cron_request_roundtrip() {
        let json = r#"{"method":"delete_cron","name":"old-job","author":"alice"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        match req {
            Request::DeleteCron { name, author } => {
                assert_eq!(name, "old-job");
                assert_eq!(author, Some("alice".to_string()));
            }
            _ => panic!("wrong variant"),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Optional machine-readable error tag. Wire-additive: existing clients
    /// that only read `error` are unaffected. New code that needs to branch
    /// on a specific failure mode (e.g. runtime self-heal on
    /// `self_departed`) reads this instead of parsing the human message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

impl Response {
    pub fn success(data: serde_json::Value) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
            error_code: None,
        }
    }

    pub fn json(data: impl Serialize) -> Self {
        match serde_json::to_value(data) {
            Ok(data) => Self::success(data),
            Err(e) => {
                tracing::error!("serializing response: {e}");
                Self::error("internal serialization error")
            }
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(msg.into()),
            error_code: None,
        }
    }

    /// Tagged error variant. Use when a downstream consumer needs to
    /// distinguish this failure from generic ones — e.g. the runtime
    /// agent_loop checking for `self_departed` to trigger self-cleanup.
    pub fn error_with_code(msg: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(msg.into()),
            error_code: Some(code.into()),
        }
    }

    pub fn yaml_string<T: Serialize + ?Sized>(value: &T, context: &str) -> Result<String, Self> {
        serde_yaml::to_string(value).map_err(|e| {
            tracing::error!("serializing {context}: {e}");
            Self::error(format!("failed to serialize {context}: {e}"))
        })
    }

    pub fn json_pretty_string<T: Serialize + ?Sized>(
        value: &T,
        context: &str,
    ) -> Result<String, Self> {
        serde_json::to_string_pretty(value).map_err(|e| {
            tracing::error!("serializing {context}: {e}");
            Self::error(format!("failed to serialize {context}: {e}"))
        })
    }

    pub fn json_line<T: Serialize + ?Sized>(value: &T, context: &str) -> String {
        match serde_json::to_string(value) {
            Ok(mut json) => {
                json.push('\n');
                json
            }
            Err(e) => {
                tracing::error!("serializing {context}: {e}");
                "{\"ok\":false,\"error\":\"internal serialization error\"}\n".to_string()
            }
        }
    }
}
