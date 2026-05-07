//! Typed response payloads for daemon IPC methods.
//!
//! One struct per `Request` variant's success `data`. Daemon handlers
//! construct these and `serde_json::to_value` them into the response
//! envelope; clients reach them via `ApiResponse::parse_data::<T>()`.
//! Field renames anywhere here surface as compile errors at every
//! call site instead of silent `unwrap_or("unknown")` fallbacks.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Response payload for `Request::Status`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StatusResponse {
    /// Daemon binary version (cargo `CARGO_PKG_VERSION` or hand-set).
    pub version: String,
    /// Top-level state string. Currently always `"running"` once the
    /// handler is reachable; reserved for future degraded states.
    pub status: String,
    /// Whether the daemon is in guest mode (read-only, no committed
    /// identity in `me.json`).
    pub guest: bool,
}

/// Response payload for `Request::Send`.
///
/// Shape is the same flat struct in three cases:
/// 1. **Pushed**: remote write succeeded — `commit_id` populated, `error` None.
/// 2. **Commit-only with reason**: local commit ok, push failed — `error`
///    populated, `commit_id` None.
/// 3. **No remote**: local-only repo, no push attempted — both None.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SendResponse {
    /// Thread line number assigned to this message (`L%06d` on disk).
    pub line_number: u64,
    /// Resolved channel/thread name (matches request input — duplicated
    /// so async consumers don't have to track the request).
    pub channel: String,
    /// Outcome string. Current values: `"pushed"`, `"commit_only"`,
    /// or whatever local-only `commit_status` produces. Treated as a
    /// hint, not a closed enum (sync layer can extend).
    pub status: String,
    /// Remote commit hash on push success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_id: Option<String>,
    /// Reason if push attempted but failed (auth, conflict, channel closed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Response payload for `Request::Read`.
///
/// `entries` carry the per-entry shape produced by `handlers::serde::
/// entry_to_json` (message lines, events, card payloads). That shape is
/// its own protocol layer outside this struct; from the wire envelope's
/// perspective each entry is an opaque JSON object.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReadResponse {
    pub channel: String,
    pub entries: Vec<Value>,
    pub archived: bool,
}

/// One row in a list-of-channels payload (active channels, archived
/// channels, DMs). `kind` distinguishes them on the wire.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelSummary {
    pub name: String,
    /// Currently `"channel"`, `"dm"`, or `"archived_channel"`.
    pub kind: String,
    pub members: Vec<String>,
}

/// Response payload for `Request::ListChannels` and
/// `Request::ListArchivedChannels` (both use the same row shape; only
/// the `kind` discriminator differs).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ListChannelsResponse {
    pub channels: Vec<ChannelSummary>,
}

/// Response payload for `Request::ListUsers`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ListUsersResponse {
    pub users: Vec<String>,
}

/// Response payload for `Request::GetThread`. `entries` keep the same
/// `entry_to_json` shape as `ReadResponse`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GetThreadResponse {
    pub channel: String,
    pub root_line: u64,
    pub entries: Vec<Value>,
}

/// Response payload for `Request::CreateChannel`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateChannelResponse {
    pub channel: String,
    pub created_by: String,
}

/// Response payload for `Request::ArchiveChannel`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArchiveChannelResponse {
    pub channel: String,
    pub archived_by: String,
}

/// Response payload for `Request::UnarchiveChannel`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UnarchiveChannelResponse {
    pub channel: String,
    pub unarchived_by: String,
}

/// Shared response shape for `Request::JoinChannel` and
/// `Request::LeaveChannel`. Both go through `write_channel_event` and
/// emit identical wire fields; `event_type` discriminates.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelEventResponse {
    pub channel: String,
    /// `"join"` or `"leave"`.
    pub event_type: String,
    pub author: String,
    pub targets: Vec<String>,
    pub line_number: u64,
    /// Push outcome string, same conventions as `SendResponse::status`.
    pub status: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    /// Locks the wire shape — these are the field names other tools
    /// (CLI `gitim status` JSON output, future WebUI `/runtime/status`)
    /// rely on. Renames need to be intentional and update consumers.
    #[test]
    fn status_response_wire_shape() {
        let r = StatusResponse {
            version: "0.1.0".to_string(),
            status: "running".to_string(),
            guest: false,
        };
        let v = serde_json::to_value(&r).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 3);
        assert_eq!(obj.get("version").and_then(|v| v.as_str()), Some("0.1.0"));
        assert_eq!(obj.get("status").and_then(|v| v.as_str()), Some("running"));
        assert_eq!(obj.get("guest").and_then(|v| v.as_bool()), Some(false));
    }

    #[test]
    fn send_response_pushed_wire_shape() {
        let r = SendResponse {
            line_number: 42,
            channel: "general".to_string(),
            status: "pushed".to_string(),
            commit_id: Some("abc123".to_string()),
            error: None,
        };
        let v = serde_json::to_value(&r).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 4, "pushed-case omits `error`");
        assert_eq!(obj.get("line_number").and_then(|v| v.as_u64()), Some(42));
        assert_eq!(obj.get("channel").and_then(|v| v.as_str()), Some("general"));
        assert_eq!(obj.get("status").and_then(|v| v.as_str()), Some("pushed"));
        assert_eq!(obj.get("commit_id").and_then(|v| v.as_str()), Some("abc123"));
    }

    #[test]
    fn send_response_commit_only_with_error() {
        let r = SendResponse {
            line_number: 99,
            channel: "general".to_string(),
            status: "commit_only".to_string(),
            commit_id: None,
            error: Some("auth failed".to_string()),
        };
        let v = serde_json::to_value(&r).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 4, "commit_only with error omits `commit_id`");
        assert_eq!(obj.get("error").and_then(|v| v.as_str()), Some("auth failed"));
        assert!(!obj.contains_key("commit_id"));
    }

    #[test]
    fn send_response_no_remote() {
        let r = SendResponse {
            line_number: 1,
            channel: "x".to_string(),
            status: "committed".to_string(),
            commit_id: None,
            error: None,
        };
        let v = serde_json::to_value(&r).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 3, "no-remote path omits both commit_id and error");
    }

    #[test]
    fn read_response_wire_shape() {
        let r = ReadResponse {
            channel: "general".to_string(),
            entries: vec![serde_json::json!({"line": 1, "body": "hi"})],
            archived: false,
        };
        let v = serde_json::to_value(&r).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 3);
        assert_eq!(obj.get("channel").and_then(|v| v.as_str()), Some("general"));
        assert_eq!(obj.get("archived").and_then(|v| v.as_bool()), Some(false));
        assert!(obj.get("entries").unwrap().is_array());
    }

    #[test]
    fn list_channels_response_wire_shape() {
        let r = ListChannelsResponse {
            channels: vec![ChannelSummary {
                name: "general".to_string(),
                kind: "channel".to_string(),
                members: vec!["alice".to_string(), "bob".to_string()],
            }],
        };
        let v = serde_json::to_value(&r).unwrap();
        let arr = v.get("channels").unwrap().as_array().unwrap();
        let first = arr[0].as_object().unwrap();
        assert_eq!(first.get("name").and_then(|v| v.as_str()), Some("general"));
        assert_eq!(first.get("kind").and_then(|v| v.as_str()), Some("channel"));
        assert_eq!(
            first
                .get("members")
                .and_then(|v| v.as_array())
                .map(|a| a.len()),
            Some(2),
        );
    }

    #[test]
    fn list_users_response_wire_shape() {
        let r = ListUsersResponse {
            users: vec!["alice".to_string(), "bob".to_string()],
        };
        let v = serde_json::to_value(&r).unwrap();
        let users = v.get("users").unwrap().as_array().unwrap();
        assert_eq!(users.len(), 2);
        assert_eq!(users[0].as_str(), Some("alice"));
    }

    #[test]
    fn get_thread_response_wire_shape() {
        let r = GetThreadResponse {
            channel: "general".to_string(),
            root_line: 42,
            entries: vec![],
        };
        let v = serde_json::to_value(&r).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 3);
        assert_eq!(obj.get("channel").and_then(|v| v.as_str()), Some("general"));
        assert_eq!(obj.get("root_line").and_then(|v| v.as_u64()), Some(42));
        assert!(obj.get("entries").unwrap().is_array());
    }

    #[test]
    fn create_channel_response_wire_shape() {
        let r = CreateChannelResponse {
            channel: "engineering".to_string(),
            created_by: "alice".to_string(),
        };
        let v = serde_json::to_value(&r).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 2);
        assert_eq!(obj.get("channel").and_then(|v| v.as_str()), Some("engineering"));
        assert_eq!(obj.get("created_by").and_then(|v| v.as_str()), Some("alice"));
    }

    #[test]
    fn channel_event_response_wire_shape() {
        let r = ChannelEventResponse {
            channel: "general".to_string(),
            event_type: "join".to_string(),
            author: "alice".to_string(),
            targets: vec!["bob".to_string(), "carol".to_string()],
            line_number: 17,
            status: "pushed".to_string(),
        };
        let v = serde_json::to_value(&r).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 6);
        assert_eq!(obj.get("event_type").and_then(|v| v.as_str()), Some("join"));
        assert_eq!(obj.get("line_number").and_then(|v| v.as_u64()), Some(17));
        assert_eq!(
            obj.get("targets").unwrap().as_array().map(|a| a.len()),
            Some(2),
        );
    }

    #[test]
    fn archive_unarchive_response_distinct_fields() {
        let a = serde_json::to_value(ArchiveChannelResponse {
            channel: "ch".to_string(),
            archived_by: "alice".to_string(),
        })
        .unwrap();
        let u = serde_json::to_value(UnarchiveChannelResponse {
            channel: "ch".to_string(),
            unarchived_by: "alice".to_string(),
        })
        .unwrap();
        assert!(a.as_object().unwrap().contains_key("archived_by"));
        assert!(!a.as_object().unwrap().contains_key("unarchived_by"));
        assert!(u.as_object().unwrap().contains_key("unarchived_by"));
        assert!(!u.as_object().unwrap().contains_key("archived_by"));
    }

    #[test]
    fn status_response_round_trip() {
        let r = StatusResponse {
            version: "9.9.9".to_string(),
            status: "running".to_string(),
            guest: true,
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: StatusResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(r, back);
    }
}
