use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(tag = "method")]
pub enum Request {
    #[serde(rename = "send")]
    Send {
        channel: String,
        body: String,
        reply_to: Option<u64>,
        author: String,
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
    GetThread {
        channel: String,
        line_number: u64,
    },
    #[serde(rename = "status")]
    Status,
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
        Self { ok: true, data: Some(data), error: None }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self { ok: false, data: None, error: Some(msg.into()) }
    }
}
