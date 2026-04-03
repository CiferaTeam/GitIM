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
        auth: serde_json::Value,
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
    },
    #[serde(rename = "reindex")]
    Reindex,
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
