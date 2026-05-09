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

    #[serde(rename = "channel_unarchived")]
    ChannelUnarchived {
        channel: String,
        author: String,
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
    ListUsers,
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
    ListArchivedChannels,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        author: Option<String>,
    },
    #[serde(rename = "unarchive_user")]
    UnarchiveUser {
        handler: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        author: Option<String>,
    },
    #[serde(rename = "archive_dm")]
    ArchiveDm {
        peer: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        author: Option<String>,
    },
    #[serde(rename = "unarchive_dm")]
    UnarchiveDm {
        peer: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        author: Option<String>,
    },
    #[serde(rename = "list_archived_users")]
    ListArchivedUsers,
    #[serde(rename = "list_archived_dms")]
    ListArchivedDms {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        author: Option<String>,
    },
    #[serde(rename = "depart_user")]
    DepartUser { handler: String },
}

fn default_limit() -> usize {
    50
}

fn default_role() -> String {
    "member".to_string()
}

fn default_introduction() -> String {
    "GitIM user".to_string()
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
            Request::ListArchivedDms { author } => {
                assert_eq!(author, Some("alice".to_string()));
            }
            _ => panic!("wrong variant"),
        }

        // Without author — resolved by dispatch via resolve_author
        let json_no_author = r#"{"method":"list_archived_dms"}"#;
        let req2: Request = serde_json::from_str(json_no_author).unwrap();
        match req2 {
            Request::ListArchivedDms { author } => assert_eq!(author, None),
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
}

#[derive(Debug, Serialize)]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn success(data: serde_json::Value) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(msg.into()),
        }
    }
}
