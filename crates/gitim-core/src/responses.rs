//! Typed response payloads for daemon IPC methods.
//!
//! One struct per `Request` variant's success `data`. Daemon handlers
//! construct these and `serde_json::to_value` them into the response
//! envelope; clients reach them via `ApiResponse::parse_data::<T>()`.

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
/// The local commit is the ack point. Send returns as soon as the
/// message is on disk and committed to the local git tree; push to
/// the remote happens asynchronously in `sync_loop` and is observable
/// via `Event::MessagesPushed` (SSE) and `sync_loop` log.
///
/// `status` values:
/// - `"committed"`: local commit succeeded; `commit_id` is the local
///   HEAD hash at commit time. Note: a subsequent rebase in `sync_loop`
///   may rewrite this commit, so the hash on the remote can differ.
/// - `"written"`: message text was written to the thread file but
///   `git commit` failed; `sync_loop` will sweep it up on its next
///   cycle. `commit_id` is None.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SendResponse {
    /// Thread line number assigned to this message (`L%06d` on disk).
    pub line_number: u64,
    /// Resolved channel/thread name (matches request input — duplicated
    /// so async consumers don't have to track the request).
    pub channel: String,
    /// Outcome string. Current values: `"committed"`, `"written"`.
    /// Treated as a hint, not a closed enum.
    pub status: String,
    /// Local HEAD hash captured under `commit_lock` immediately after the
    /// commit. None when the commit itself failed (status = `"written"`)
    /// or when `rev_parse HEAD` errored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_id: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
}

/// Response payload for `Request::ListChannels` and
/// `Request::ListArchivedChannels` (both use the same row shape; only
/// the `kind` discriminator differs).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ListChannelsResponse {
    pub channels: Vec<ChannelSummary>,
}

/// Paginated response payload for `Request::ListArchivedChannels`.
/// Active channels stay on `ListChannelsResponse`; archive browsing can grow
/// without bound, so callers page it lazily.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ListArchivedChannelsResponse {
    pub channels: Vec<ChannelSummary>,
    #[serde(default)]
    pub has_more: bool,
}

/// Active users with optional archived users in a single payload.
///
/// `include_archived` on the request controls whether `archived_users` is
/// populated. We merge both lists into one response (rather than the two-shape
/// pattern used by `ListChannelsResponse` / `ListArchivedChannelsResponse`)
/// because Contract 2 enforces strict mutual exclusion: a handler can be in
/// `users/` xor `archive/users/`, never both. The single shape makes the
/// invariant explicit at the wire level — clients see `users ∩ archived_users
/// = ∅` by construction.
///
/// Default call (`include_archived = false`) returns only `users` —
/// `archived_users` is omitted on the wire. When the caller opts in with
/// `include_archived = true`, daemon also returns `archived_users: Some(_)`
/// alongside. Field name is `archived_users` (not bare `archived`) to keep it
/// distinct from `ReadResponse.archived: bool` — same word, very different
/// shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ListUsersResponse {
    pub users: Vec<String>,
    /// Populated only when the request was `include_archived: true`.
    /// Wire-additive — old clients see no field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_users: Option<Vec<String>>,
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

/// Response payload for `Request::ArchiveUser`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArchiveUserResponse {
    pub handler: String,
    pub archived_by: String,
}

/// Response payload for `Request::UnarchiveUser`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UnarchiveUserResponse {
    pub handler: String,
    pub unarchived_by: String,
}

/// One row in `ListArchivedUsersResponse.users`. The handler is
/// structurally guaranteed (it's the file stem under `archive/users/`);
/// `display_name` is best-effort — the daemon parses the archived
/// `UserMeta` yaml on each list call and omits the field when the file
/// is absent or unparseable. Frontends must render gracefully when
/// `display_name` is `None`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArchivedUserEntry {
    pub handler: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

/// Response payload for `Request::ListArchivedUsers`.
///
/// Wire shape: `users` is a list of `{handler, display_name?}` objects.
/// `display_name` is best-effort — daemon emits it when known, clients
/// must tolerate its absence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ListArchivedUsersResponse {
    pub users: Vec<ArchivedUserEntry>,
}

/// Response payload for `Request::DepartUser`.
///
/// `commits` reports how many commits this invocation produced. On a
/// fresh burn this counts every phase that did real work; on an
/// idempotent retry the count drops as already-completed steps skip.
/// `already_departed` flags the terminal-state shortcut — the caller
/// can distinguish "depart just finished" from "depart was already
/// done before this call" without diffing git logs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DepartUserResponse {
    pub handler: String,
    pub commits: u64,
    pub already_departed: bool,
}

/// Response payload for `Request::ArchiveDm`.
///
/// `dm_pair_stem` is the on-disk filename stem `<min>--<max>` (output of
/// `gitim_core::dm::dm_filename`), so callers can re-derive participants
/// or display the archive entry without re-deriving the sort. No
/// timestamp here — RPC responses across `ArchiveUserResponse`,
/// `ArchiveChannelResponse`, and this one stay aligned. The
/// `Event::DmArchived` broadcast carries the timestamp.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArchiveDmResponse {
    pub archived_by: String,
    pub dm_pair_stem: String,
}

/// Response payload for `Request::UnarchiveDm`. Symmetric to `ArchiveDmResponse`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UnarchiveDmResponse {
    pub unarchived_by: String,
    pub dm_pair_stem: String,
}

/// One row in `ListArchivedDmsResponse.dms`. `peer` is the participant
/// other than the caller; `dm_pair_stem` is the canonical sorted-pair
/// filename stem so the client can reconstruct the storage key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArchivedDmEntry {
    pub peer: String,
    pub dm_pair_stem: String,
}

/// Response payload for `Request::ListArchivedDms`. Daemon filters to
/// rows where the caller participated in the DM; third parties never
/// appear here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ListArchivedDmsResponse {
    pub dms: Vec<ArchivedDmEntry>,
    #[serde(default)]
    pub has_more: bool,
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
    /// Commit outcome string, same conventions as `SendResponse::status`
    /// (`"committed"` or `"written"`).
    pub status: String,
}

/// One row in `SearchResponse.messages`. Mirrors the `gitim_index`
/// search hit projection that handler `handle_search` was building by
/// hand from a `SearchHit`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchMessage {
    pub channel: String,
    /// `"channel"` / `"dm"` / `"card"`.
    pub channel_type: String,
    pub line_number: u64,
    /// `0` when the entry is itself a thread root.
    pub parent_line: u64,
    pub author: String,
    pub timestamp: String,
    pub body: String,
}

/// Response payload for `Request::Search`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResponse {
    pub messages: Vec<SearchMessage>,
    pub total: u64,
}

/// Response payload for `Request::Reindex`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReindexResponse {
    /// Currently always `"complete"` on success.
    pub status: String,
    pub messages_indexed: u64,
}

/// Response payload for `Request::RegisterUser`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegisterUserResponse {
    pub handler: String,
    /// `true` if the user already had a meta file (idempotent re-register).
    pub exists: bool,
}

/// Response payload for `Request::Stop`. Sent right before the daemon
/// exits — clients should expect the connection to close shortly after.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StopResponse {
    /// Currently always `"stopping"`.
    pub status: String,
}

/// Response payload for `Request::Subscribe`. Just an ack — after
/// sending this the connection switches into a stream of `Event`s
/// (already typed; see `api::Event`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubscribeResponse {
    pub subscribed: bool,
}

/// Response payload for `Request::Onboard`. Two variants on the wire,
/// chosen by the request — guest mode produces `{ "guest": true }`,
/// authenticated mode produces `{ "handler": ..., "created": ... }`.
/// Untagged because the daemon emits these literal shapes; clients can
/// `parse_data::<OnboardResponse>()` and match on the enum.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum OnboardResponse {
    /// Guest-mode onboard. Read-only identity, no commit author.
    Guest { guest: bool },
    /// Authenticated onboard. `created` is `true` when a fresh user
    /// meta file was written; `false` on re-onboard of an existing user.
    User { handler: String, created: bool },
}

// -- Card responses --

/// One row in `ListCardsResponse.cards` / `ListArchivedCardsResponse.cards`.
/// Shared shape — distinction is which list the row came from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CardSummary {
    pub card_id: String,
    pub channel: String,
    pub title: String,
    /// `CardStatus::as_str()` value: `"todo"`, `"doing"`, or `"done"`.
    /// Kept as String so the wire schema doesn't depend on the enum.
    pub status: String,
    pub labels: Vec<String>,
    /// `null` on the wire when no assignee is set.
    pub assignee: Option<String>,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Response payload for `Request::ListCards` and
/// `Request::ListArchivedCards`. Both produce identical shape — caller
/// disambiguates by which RPC they invoked.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ListCardsResponse {
    pub cards: Vec<CardSummary>,
}

/// Response payload for `Request::CreateCard`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateCardResponse {
    pub channel: String,
    pub card_id: String,
    pub title: String,
}

/// Response payload for `Request::ArchiveCard`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArchiveCardResponse {
    pub channel: String,
    pub card_id: String,
    pub archived_by: String,
}

/// Response payload for `Request::UnarchiveCard`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UnarchiveCardResponse {
    pub channel: String,
    pub card_id: String,
    pub unarchived_by: String,
}

/// Inner `meta` object embedded in `ReadCardResponse`. Same fields as
/// `CardSummary` minus the redundant `card_id`/`channel` (those are on
/// the outer `ReadCardResponse`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CardMetaSummary {
    pub title: String,
    pub status: String,
    pub labels: Vec<String>,
    pub assignee: Option<String>,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Response payload for `Request::ReadCard`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReadCardResponse {
    pub channel: String,
    pub card_id: String,
    pub archived: bool,
    pub meta: CardMetaSummary,
    /// Card thread entries — same opaque per-entry shape as
    /// ReadResponse / GetThreadResponse.
    pub entries: Vec<Value>,
}

/// Response payload for `Request::SendCardMessage`. Same shape and
/// semantics as `SendResponse`, plus the `card_id` it was sent into.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SendCardMessageResponse {
    pub line_number: u64,
    pub channel: String,
    pub card_id: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_id: Option<String>,
}

/// Response payload for `Request::UpdateCard`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UpdateCardResponse {
    pub channel: String,
    pub card_id: String,
    pub status: String,
    pub labels: Vec<String>,
    pub assignee: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BoardMetaSummary {
    pub version: u32,
    pub handler: String,
    pub updated_at: String,
    pub status: String,
    pub summary: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BoardSummary {
    pub handler: String,
    pub path: String,
    pub updated_at: String,
    pub status: String,
    pub summary: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ListBoardsResponse {
    pub boards: Vec<BoardSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReadBoardResponse {
    pub handler: String,
    pub path: String,
    pub meta: BoardMetaSummary,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WriteBoardResponse {
    pub handler: String,
    pub path: String,
    pub status: String,
    pub commit_id: String,
}

/// One change entry in `PollResponse.changes`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PollChange {
    pub channel: String,
    /// `"channel"` / `"dm"` / `"card"` / `"board"`, etc.
    pub kind: String,
    /// Same opaque per-entry shape as ReadResponse / GetThreadResponse.
    pub entries: Vec<Value>,
}

/// Response payload for `Request::Poll`. Returns commits since the
/// caller-supplied cursor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PollResponse {
    /// 40-char hex of the latest commit at poll time. Use as the next
    /// `since` cursor.
    pub commit_id: String,
    pub changes: Vec<PollChange>,
}

// -- Cron responses --

/// One row in `ListCronsResponse.crons`. Lightweight summary suitable for
/// list views — full spec body and history live behind `show_cron`.
///
/// `next_fire` is computed at list time (no mutable state on disk) so it
/// reflects the same croner+timezone resolution that the engine will use
/// at fire time. Disabled specs still expose a `next_fire` so the calendar
/// UI can grey out future occurrences without recomputing the schedule.
///
/// NOTE: The runtime's timeline endpoint
/// (`gitim-runtime::http::crons_timeline` via
/// `synthesize_spec_for_iteration`) builds an in-memory `CronSpec` from
/// these fields to drive `next_fire_after` iteration without a second
/// IPC round trip. The fields tagged `// timeline: required` below are
/// load-bearing for that synthesis — dropping or renaming any of them
/// silently degrades the timeline endpoint (future / missed entries
/// vanish for affected crons). A unit test in the runtime
/// (`synthesize_spec_for_iteration_locks_summary_contract`) re-fires
/// synthesis end-to-end so a breaking change here fails CI rather than
/// drifting unnoticed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CronSummary {
    pub name: String,
    pub schedule: String, // timeline: required
    /// IANA timezone string, or `None` for UTC. Stays optional to mirror
    /// `CronSpec.timezone` — the wire shape is "absent" not "explicit UTC".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>, // timeline: required (Option preserved verbatim)
    pub target: String,   // timeline: required
    pub enabled: bool,
    pub created_by: String, // timeline: required
    pub created_at: String, // timeline: required (RFC 3339 UTC, ends with 'Z')
    /// Computed via `next_fire_after(spec, now)`. `None` only on a spec
    /// whose schedule somehow fails to parse at list time (defensive — the
    /// daemon already validated on create, but a hand-edited spec.yaml
    /// could regress).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_fire: Option<String>,
}

/// Response payload for `Request::ListCrons`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ListCronsResponse {
    pub crons: Vec<CronSummary>,
}

/// One past-fire entry in `CronDetail.recent_runs` and
/// `HistoryCronResponse.runs`. Each row corresponds to one
/// `crons/<name>/<ts>.thread` file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CronRunEntry {
    /// Theoretical fire timestamp baked into the thread filename, ISO 8601
    /// UTC with `:` rewritten to `-` (e.g. `2026-05-11T09-00-00Z`). Same
    /// string used as the URL-safe id in the runtime HTTP layer.
    pub ts: String,
    /// On-disk filename relative to the cron directory (matches `ts +
    /// ".thread"`). Useful for clients that fetch the raw thread.
    pub filename: String,
}

/// Response payload for `Request::ShowCron`. `spec` holds the full
/// validated body; `recent_runs` carries the last few past fires (most
/// recent first); `next_fire` is the computed next theoretical fire.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CronDetail {
    /// Job name (filesystem stem under `crons/<name>/`). Surfaced
    /// separately so callers don't have to read it back out of the file
    /// path.
    pub name: String,
    /// Full spec body. Matches the on-disk `spec.yaml`.
    pub spec: serde_yaml::Value,
    pub recent_runs: Vec<CronRunEntry>,
    /// Same shape as `CronSummary.next_fire`. `None` when schedule fails
    /// to parse (defensive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_fire: Option<String>,
}

/// Response payload for `Request::HistoryCron`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryCronResponse {
    pub name: String,
    pub runs: Vec<CronRunEntry>,
}

/// Response payload for `Request::CreateCron`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateCronResponse {
    pub name: String,
    pub created_by: String,
    /// Resolved target — `@self` is replaced with the author handler at
    /// create time, so the response always reflects the literal handler
    /// stored in `spec.yaml`.
    pub target: String,
}

/// Response payload for `Request::EnableCron` / `Request::DisableCron`.
/// Idempotent: when the spec is already in the requested state, the
/// daemon returns `changed: false` and produces no commit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToggleCronResponse {
    pub name: String,
    pub enabled: bool,
    /// `true` when this call mutated `spec.yaml` and produced a commit;
    /// `false` on the no-op idempotent path.
    pub changed: bool,
}

/// Response payload for `Request::DeleteCron`. Soft delete — `git mv` of
/// `crons/<name>/` into `archive/crons/<name>/`. Mirrors the
/// `ArchiveChannelResponse` shape so the wire stays uniform across
/// archive operations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeleteCronResponse {
    pub name: String,
    pub deleted_by: String,
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
    fn send_response_committed_wire_shape() {
        let r = SendResponse {
            line_number: 42,
            channel: "general".to_string(),
            status: "committed".to_string(),
            commit_id: Some("abc123".to_string()),
        };
        let v = serde_json::to_value(&r).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 4);
        assert_eq!(obj.get("line_number").and_then(|v| v.as_u64()), Some(42));
        assert_eq!(obj.get("channel").and_then(|v| v.as_str()), Some("general"));
        assert_eq!(
            obj.get("status").and_then(|v| v.as_str()),
            Some("committed")
        );
        assert_eq!(
            obj.get("commit_id").and_then(|v| v.as_str()),
            Some("abc123")
        );
    }

    #[test]
    fn send_response_written_omits_commit_id() {
        let r = SendResponse {
            line_number: 1,
            channel: "x".to_string(),
            status: "written".to_string(),
            commit_id: None,
        };
        let v = serde_json::to_value(&r).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(
            obj.len(),
            3,
            "written status (commit failed) omits commit_id"
        );
        assert!(!obj.contains_key("commit_id"));
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
                created_by: Some("alice".to_string()),
            }],
        };
        let v = serde_json::to_value(&r).unwrap();
        let arr = v.get("channels").unwrap().as_array().unwrap();
        let first = arr[0].as_object().unwrap();
        assert_eq!(first.get("name").and_then(|v| v.as_str()), Some("general"));
        assert_eq!(first.get("kind").and_then(|v| v.as_str()), Some("channel"));
        assert_eq!(
            first.get("created_by").and_then(|v| v.as_str()),
            Some("alice"),
        );
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
            archived_users: None,
        };
        let v = serde_json::to_value(&r).unwrap();
        let obj = v.as_object().unwrap();
        // Default call: only `users`. `archived_users` is skipped when None
        // so older clients that don't know the field still parse cleanly.
        assert_eq!(obj.len(), 1);
        let users = obj.get("users").unwrap().as_array().unwrap();
        assert_eq!(users.len(), 2);
        assert_eq!(users[0].as_str(), Some("alice"));
        assert!(!obj.contains_key("archived_users"));
        // Also assert no bare `archived` — keeps us honest if someone reverts
        // the rename without updating both sides.
        assert!(!obj.contains_key("archived"));
    }

    #[test]
    fn list_users_response_with_archived_wire_shape() {
        let r = ListUsersResponse {
            users: vec!["alice".to_string()],
            archived_users: Some(vec!["bob".to_string(), "carol".to_string()]),
        };
        let v = serde_json::to_value(&r).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 2);
        assert_eq!(
            obj.get("users").unwrap().as_array().map(|a| a.len()),
            Some(1),
        );
        let archived_users = obj.get("archived_users").unwrap().as_array().unwrap();
        assert_eq!(archived_users.len(), 2);
        assert_eq!(archived_users[0].as_str(), Some("bob"));
        // Disambiguation guard: no bare `archived` key — that's reserved for
        // `ReadResponse.archived: bool` semantics.
        assert!(!obj.contains_key("archived"));
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
        assert_eq!(
            obj.get("channel").and_then(|v| v.as_str()),
            Some("engineering")
        );
        assert_eq!(
            obj.get("created_by").and_then(|v| v.as_str()),
            Some("alice")
        );
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
    fn user_archive_unarchive_response_distinct_fields() {
        let a = serde_json::to_value(ArchiveUserResponse {
            handler: "alice".to_string(),
            archived_by: "alice".to_string(),
        })
        .unwrap();
        let u = serde_json::to_value(UnarchiveUserResponse {
            handler: "alice".to_string(),
            unarchived_by: "alice".to_string(),
        })
        .unwrap();
        assert_eq!(a.get("handler").and_then(|v| v.as_str()), Some("alice"));
        assert!(a.as_object().unwrap().contains_key("archived_by"));
        assert!(!a.as_object().unwrap().contains_key("unarchived_by"));
        assert!(u.as_object().unwrap().contains_key("unarchived_by"));
        assert!(!u.as_object().unwrap().contains_key("archived_by"));
    }

    #[test]
    fn dm_archive_unarchive_response_distinct_fields() {
        let a = serde_json::to_value(ArchiveDmResponse {
            archived_by: "alice".to_string(),
            dm_pair_stem: "alice--bob".to_string(),
        })
        .unwrap();
        let u = serde_json::to_value(UnarchiveDmResponse {
            unarchived_by: "alice".to_string(),
            dm_pair_stem: "alice--bob".to_string(),
        })
        .unwrap();
        assert_eq!(
            a.get("dm_pair_stem").and_then(|v| v.as_str()),
            Some("alice--bob"),
        );
        assert!(a.as_object().unwrap().contains_key("archived_by"));
        assert!(!a.as_object().unwrap().contains_key("unarchived_by"));
        assert!(u.as_object().unwrap().contains_key("unarchived_by"));
        assert!(!u.as_object().unwrap().contains_key("archived_by"));
        // Timestamps live on Event::DmArchived / DmUnarchived, not the
        // response — kept aligned with ArchiveUser / ArchiveChannel.
        assert!(!a.as_object().unwrap().contains_key("archived_at"));
        assert!(!u.as_object().unwrap().contains_key("unarchived_at"));
    }

    #[test]
    fn list_archived_dms_response_has_more_field() {
        let r = ListArchivedDmsResponse {
            dms: vec![],
            has_more: true,
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["has_more"], serde_json::json!(true));
        // Backward compatible: missing has_more deserializes as false (default).
        let r2: ListArchivedDmsResponse = serde_json::from_str(r#"{"dms":[]}"#).unwrap();
        assert!(!r2.has_more);
    }

    #[test]
    fn list_archived_dms_response_wire_shape() {
        let r = ListArchivedDmsResponse {
            dms: vec![
                ArchivedDmEntry {
                    peer: "alice".to_string(),
                    dm_pair_stem: "alice--charlie".to_string(),
                },
                ArchivedDmEntry {
                    peer: "bob".to_string(),
                    dm_pair_stem: "bob--charlie".to_string(),
                },
            ],
            has_more: false,
        };
        let v = serde_json::to_value(&r).unwrap();
        let arr = v.get("dms").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 2);
        let first = arr[0].as_object().unwrap();
        assert_eq!(first.get("peer").and_then(|v| v.as_str()), Some("alice"));
        assert_eq!(
            first.get("dm_pair_stem").and_then(|v| v.as_str()),
            Some("alice--charlie"),
        );
    }

    #[test]
    fn list_archived_users_response_wire_shape() {
        let r = ListArchivedUsersResponse {
            users: vec![
                ArchivedUserEntry {
                    handler: "alice".to_string(),
                    display_name: Some("Alice".to_string()),
                },
                ArchivedUserEntry {
                    handler: "bob".to_string(),
                    display_name: None,
                },
            ],
        };
        let v = serde_json::to_value(&r).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 1);
        let arr = obj.get("users").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 2);

        // Row with display_name: present on the wire.
        let alice = arr[0].as_object().unwrap();
        assert_eq!(alice.get("handler").and_then(|v| v.as_str()), Some("alice"));
        assert_eq!(
            alice.get("display_name").and_then(|v| v.as_str()),
            Some("Alice"),
        );

        // Row without display_name: field skipped on the wire (no `null`).
        // `skip_serializing_if = Option::is_none` keeps the payload minimal
        // and matches the rest of the response module's conventions.
        let bob = arr[1].as_object().unwrap();
        assert_eq!(bob.get("handler").and_then(|v| v.as_str()), Some("bob"));
        assert!(
            !bob.contains_key("display_name"),
            "absent display_name must be omitted, not serialized as null"
        );
    }

    #[test]
    fn search_response_wire_shape() {
        let r = SearchResponse {
            messages: vec![SearchMessage {
                channel: "general".to_string(),
                channel_type: "channel".to_string(),
                line_number: 42,
                parent_line: 0,
                author: "alice".to_string(),
                timestamp: "20260507T120000Z".to_string(),
                body: "hello".to_string(),
            }],
            total: 1,
        };
        let v = serde_json::to_value(&r).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 2);
        assert_eq!(obj.get("total").and_then(|v| v.as_u64()), Some(1));
        let msgs = obj.get("messages").unwrap().as_array().unwrap();
        let m = msgs[0].as_object().unwrap();
        assert_eq!(m.len(), 7);
        assert_eq!(m.get("body").and_then(|v| v.as_str()), Some("hello"));
        assert_eq!(m.get("parent_line").and_then(|v| v.as_u64()), Some(0));
    }

    #[test]
    fn reindex_response_wire_shape() {
        let r = ReindexResponse {
            status: "complete".to_string(),
            messages_indexed: 12345,
        };
        let v = serde_json::to_value(&r).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 2);
        assert_eq!(
            obj.get("messages_indexed").and_then(|v| v.as_u64()),
            Some(12345),
        );
    }

    #[test]
    fn register_user_response_wire_shape() {
        let r = RegisterUserResponse {
            handler: "alice".to_string(),
            exists: false,
        };
        let v = serde_json::to_value(&r).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 2);
        assert_eq!(obj.get("exists").and_then(|v| v.as_bool()), Some(false));
    }

    #[test]
    fn stop_response_wire_shape() {
        let r = StopResponse {
            status: "stopping".to_string(),
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v.as_object().unwrap().len(), 1);
    }

    #[test]
    fn poll_response_wire_shape_empty_changes() {
        let r = PollResponse {
            commit_id: "abcdef0123456789".to_string(),
            changes: vec![],
        };
        let v = serde_json::to_value(&r).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 2);
        assert!(obj.get("changes").unwrap().as_array().unwrap().is_empty());
    }

    #[test]
    fn poll_change_wire_shape() {
        let c = PollChange {
            channel: "general".to_string(),
            kind: "channel".to_string(),
            entries: vec![serde_json::json!({"line_number": 1})],
        };
        let v = serde_json::to_value(&c).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 3);
        assert_eq!(obj.get("kind").and_then(|v| v.as_str()), Some("channel"));
    }

    #[test]
    fn create_card_response_wire_shape() {
        let r = CreateCardResponse {
            channel: "general".to_string(),
            card_id: "card-1".to_string(),
            title: "Fix bug".to_string(),
        };
        let v = serde_json::to_value(&r).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 3);
    }

    #[test]
    fn read_card_response_wire_shape_with_null_assignee() {
        let r = ReadCardResponse {
            channel: "general".to_string(),
            card_id: "card-1".to_string(),
            archived: false,
            meta: CardMetaSummary {
                title: "Fix bug".to_string(),
                status: "open".to_string(),
                labels: vec!["bug".to_string()],
                assignee: None,
                created_by: "alice".to_string(),
                created_at: "20260507T100000Z".to_string(),
                updated_at: "20260507T100000Z".to_string(),
            },
            entries: vec![],
        };
        let v = serde_json::to_value(&r).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 5);
        let meta = obj.get("meta").unwrap().as_object().unwrap();
        // assignee is preserved as null on wire (no skip_serializing_if)
        assert_eq!(meta.get("assignee"), Some(&serde_json::Value::Null));
    }

    #[test]
    fn send_card_message_response_committed() {
        let r = SendCardMessageResponse {
            line_number: 7,
            channel: "general".to_string(),
            card_id: "card-1".to_string(),
            status: "committed".to_string(),
            commit_id: Some("hash".to_string()),
        };
        let v = serde_json::to_value(&r).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 5);
        assert!(obj.contains_key("commit_id"));
        assert_eq!(
            obj.get("status").and_then(|v| v.as_str()),
            Some("committed")
        );
    }

    #[test]
    fn update_card_response_keeps_null_assignee() {
        let r = UpdateCardResponse {
            channel: "general".to_string(),
            card_id: "card-1".to_string(),
            status: "done".to_string(),
            labels: vec![],
            assignee: None,
        };
        let v = serde_json::to_value(&r).unwrap();
        let obj = v.as_object().unwrap();
        // assignee stays present as null — matches pre-typed json! shape
        assert_eq!(obj.get("assignee"), Some(&serde_json::Value::Null));
    }

    #[test]
    fn list_cards_response_with_summary() {
        let r = ListCardsResponse {
            cards: vec![CardSummary {
                card_id: "c1".to_string(),
                channel: "general".to_string(),
                title: "T".to_string(),
                status: "open".to_string(),
                labels: vec![],
                assignee: None,
                created_by: "alice".to_string(),
                created_at: "x".to_string(),
                updated_at: "x".to_string(),
            }],
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v.get("cards").unwrap().as_array().unwrap().len(), 1);
    }

    #[test]
    fn list_boards_response_wire_shape() {
        let r = ListBoardsResponse {
            boards: vec![BoardSummary {
                handler: "alice".to_string(),
                path: "showboards/alice/board.md".to_string(),
                updated_at: "20260509T120000Z".to_string(),
                status: "working".to_string(),
                summary: "checking release".to_string(),
                tags: vec!["release".to_string()],
            }],
        };
        let v = serde_json::to_value(&r).unwrap();
        let boards = v.get("boards").unwrap().as_array().unwrap();
        assert_eq!(boards.len(), 1);
        let first = boards[0].as_object().unwrap();
        assert_eq!(first.len(), 6);
        assert_eq!(first.get("handler").and_then(|v| v.as_str()), Some("alice"));
        assert_eq!(
            first.get("path").and_then(|v| v.as_str()),
            Some("showboards/alice/board.md")
        );
        assert_eq!(
            first
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|a| a.len()),
            Some(1),
        );
    }

    #[test]
    fn read_board_response_wire_shape() {
        let r = ReadBoardResponse {
            handler: "alice".to_string(),
            path: "showboards/alice/board.md".to_string(),
            meta: BoardMetaSummary {
                version: 1,
                handler: "alice".to_string(),
                updated_at: "20260509T120000Z".to_string(),
                status: "working".to_string(),
                summary: "checking release".to_string(),
                tags: vec!["release".to_string()],
            },
            body: "## 当前状态\n\nworking\n".to_string(),
        };
        let v = serde_json::to_value(&r).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 4);
        assert_eq!(obj.get("handler").and_then(|v| v.as_str()), Some("alice"));
        assert_eq!(
            obj.get("body").and_then(|v| v.as_str()),
            Some("## 当前状态\n\nworking\n")
        );
        let meta = obj.get("meta").unwrap().as_object().unwrap();
        assert_eq!(meta.len(), 6);
        assert_eq!(meta.get("version").and_then(|v| v.as_u64()), Some(1));
    }

    #[test]
    fn write_board_response_wire_shape() {
        let r = WriteBoardResponse {
            handler: "alice".to_string(),
            path: "showboards/alice/board.md".to_string(),
            status: "pushed".to_string(),
            commit_id: "abc123".to_string(),
        };
        let v = serde_json::to_value(&r).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 4);
        assert_eq!(obj.get("status").and_then(|v| v.as_str()), Some("pushed"));
        assert_eq!(
            obj.get("commit_id").and_then(|v| v.as_str()),
            Some("abc123")
        );
    }

    #[test]
    fn onboard_response_guest_variant_wire_shape() {
        let r = OnboardResponse::Guest { guest: true };
        let v = serde_json::to_value(&r).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 1);
        assert_eq!(obj.get("guest").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn onboard_response_user_variant_wire_shape() {
        let r = OnboardResponse::User {
            handler: "alice".to_string(),
            created: true,
        };
        let v = serde_json::to_value(&r).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 2);
        assert_eq!(obj.get("handler").and_then(|v| v.as_str()), Some("alice"));
        assert_eq!(obj.get("created").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn onboard_response_round_trip_via_untagged() {
        // Wire bytes the daemon sent in guest mode ...
        let guest_wire = r#"{"guest":true}"#;
        let parsed: OnboardResponse = serde_json::from_str(guest_wire).unwrap();
        assert!(matches!(parsed, OnboardResponse::Guest { guest: true }));

        // ... and the user-mode bytes pick the other variant.
        let user_wire = r#"{"handler":"alice","created":false}"#;
        let parsed: OnboardResponse = serde_json::from_str(user_wire).unwrap();
        assert!(matches!(
            parsed,
            OnboardResponse::User { ref handler, created: false } if handler == "alice"
        ));
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

// -- Flow responses --

use crate::flow::{FlowNode, FlowRun, FlowRunNode, NodeStatus, NodeType, RunStatus};

/// Lightweight summary of a flow, used in list views.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowSummary {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub node_count: usize,
    pub updated_at: Option<String>,
}

/// Response payload for `Request::ListFlows`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListFlowsResponse {
    pub flows: Vec<FlowSummary>,
}

/// One node entry in `ShowFlowResponse.nodes`. Typed projection of
/// `FlowNode` — `signal` and `exits` are v2 fields and omitted here
/// intentionally; callers that need them read `raw_markdown`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowNodeSummary {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: NodeType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub participants: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,
    pub prompt: String,
}

/// Response payload for `Request::ShowFlow`.
///
/// `nodes` gives the frontend a typed, render-ready node list.
/// `raw_markdown` is the full source markdown so agents can read and
/// rewrite the flow without a second IPC round trip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShowFlowResponse {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: Option<String>,
    pub nodes: Vec<FlowNodeSummary>,
    pub raw_markdown: String,
}

impl From<&FlowNode> for FlowNodeSummary {
    fn from(n: &FlowNode) -> Self {
        Self {
            id: n.id.clone(),
            node_type: n.node_type.clone(),
            owner: n.owner.clone(),
            participants: n.participants.clone(),
            needs: n.needs.clone(),
            prompt: n.prompt.clone(),
        }
    }
}

/// Response payload for `Request::WriteFlow` (create or update).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WriteFlowResponse {
    pub slug: String,
    pub path: String,
    /// Same push-outcome conventions as `WriteBoardResponse::status`.
    pub status: String,
    pub commit_id: String,
}

/// One issue in `ValidateFlowResponse.items`.
/// `kind` is `"error"` or `"warning"` — kept as `String` so the wire
/// stays simple and frontend-friendly without coupling to an enum.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowValidationItem {
    pub kind: String,
    pub message: String,
}

/// Response payload for `Request::ValidateFlow`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidateFlowResponse {
    pub slug: String,
    pub ok: bool,
    pub items: Vec<FlowValidationItem>,
}

// -- Flow run responses --

/// Response payload for `Request::StartFlowRun`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StartFlowRunResponse {
    pub run_id: String,
    pub flow_slug: String,
    pub channel: String,
    pub path: String,
    pub commit_id: String,
}

/// Lightweight summary of a flow run, used in list views.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowRunSummary {
    pub run_id: String,
    pub flow_slug: String,
    pub channel: String,
    pub status: RunStatus,
    pub started_by: String,
    pub started_at: String,
    pub updated_at: String,
    pub node_count: usize,
    pub nodes_done: usize,
}

/// Response payload for `Request::ListFlowRuns`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListFlowRunsResponse {
    pub runs: Vec<FlowRunSummary>,
}

/// One node entry in `ShowFlowRunResponse.nodes`. Typed projection of
/// `FlowRunNode` — optional fields use `skip_serializing_if` to keep the
/// wire minimal for pending nodes where most fields are absent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowRunNodeSummary {
    pub id: String,
    pub status: NodeStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_ref: Option<String>,
}

impl From<&FlowRunNode> for FlowRunNodeSummary {
    fn from(n: &FlowRunNode) -> Self {
        Self {
            id: n.id.clone(),
            status: n.status,
            actor: n.actor.clone(),
            started_at: n.started_at.clone(),
            completed_at: n.completed_at.clone(),
            result_ref: n.result_ref.clone(),
        }
    }
}

/// Response payload for `Request::ShowFlowRun`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShowFlowRunResponse {
    pub run_id: String,
    pub flow_slug: String,
    pub channel: String,
    pub started_at: String,
    pub started_by: String,
    pub status: RunStatus,
    pub updated_at: String,
    pub nodes: Vec<FlowRunNodeSummary>,
}

impl From<&FlowRun> for ShowFlowRunResponse {
    fn from(r: &FlowRun) -> Self {
        Self {
            run_id: r.run_id.clone(),
            flow_slug: r.flow_slug.clone(),
            channel: r.channel.clone(),
            started_at: r.started_at.clone(),
            started_by: r.started_by.clone(),
            status: r.status,
            updated_at: r.updated_at.clone(),
            nodes: r.nodes.iter().map(FlowRunNodeSummary::from).collect(),
        }
    }
}

/// Response payload for `Request::UpdateFlowNode`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateFlowNodeResponse {
    pub run_id: String,
    pub node_id: String,
    pub status: NodeStatus,
    pub run_status: RunStatus,
    pub commit_id: String,
}

/// Response payload for `Request::CancelFlowRun`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CancelFlowRunResponse {
    pub run_id: String,
    pub commit_id: String,
}
