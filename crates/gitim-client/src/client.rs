use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use gitim_core::auth_payload::AuthPayload;
use gitim_core::me_json::MeJson;
use gitim_core::responses::{
    CronDetail, CronRunEntry, CronSummary, HistoryCronResponse, ListCronsResponse,
    ToggleCronResponse,
};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::error::ClientError;
use crate::types::{build_request, ApiResponse};

#[cfg(not(test))]
const DAEMON_REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
#[cfg(test)]
const DAEMON_REQUEST_TIMEOUT: Duration = Duration::from_millis(50);

pub struct GitimClient {
    repo_root: PathBuf,
    socket_path: PathBuf,
}

impl GitimClient {
    pub fn new(repo_root: &Path) -> Self {
        Self {
            repo_root: repo_root.to_path_buf(),
            socket_path: repo_root.join(".gitim/run/gitim.sock"),
        }
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<ApiResponse, ClientError> {
        let stream = UnixStream::connect(&self.socket_path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound
                || e.kind() == std::io::ErrorKind::ConnectionRefused
            {
                ClientError::DaemonNotRunning
            } else {
                ClientError::ConnectionFailed(e.to_string())
            }
        })?;

        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);

        let payload = serde_json::to_string(&build_request(method, params))
            .map_err(|e| ClientError::ProtocolError(e.to_string()))?;

        writer
            .write_all(format!("{payload}\n").as_bytes())
            .await
            .map_err(|e| ClientError::ConnectionFailed(e.to_string()))?;

        let mut line = String::new();
        tokio::time::timeout(DAEMON_REQUEST_TIMEOUT, reader.read_line(&mut line))
            .await
            .map_err(|_| ClientError::Timeout)?
            .map_err(|e| ClientError::ConnectionFailed(e.to_string()))?;

        if line.is_empty() {
            return Err(ClientError::ProtocolError(
                "empty response from daemon".to_string(),
            ));
        }

        serde_json::from_str::<ApiResponse>(&line)
            .map_err(|e| ClientError::ProtocolError(e.to_string()))
    }

    // -- convenience methods, each delegating to request() --

    pub async fn status(&self) -> Result<ApiResponse, ClientError> {
        self.request("status", json!({})).await
    }

    pub async fn send(
        &self,
        channel: &str,
        body: &str,
        author: Option<&str>,
        reply_to: Option<u64>,
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "send",
            json!({
                "channel": channel,
                "body": body,
                "author": author,
                "reply_to": reply_to,
            }),
        )
        .await
    }

    pub async fn read(
        &self,
        channel: &str,
        limit: Option<u64>,
        since: Option<u64>,
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "read",
            json!({
                "channel": channel,
                "limit": limit,
                "since": since,
            }),
        )
        .await
    }

    pub async fn list_channels(&self) -> Result<ApiResponse, ClientError> {
        self.request("channels", json!({})).await
    }

    pub async fn list_users(&self) -> Result<ApiResponse, ClientError> {
        self.request("users", json!({})).await
    }

    pub async fn get_thread(
        &self,
        channel: &str,
        line_number: u64,
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "thread",
            json!({
                "channel": channel,
                "line_number": line_number,
            }),
        )
        .await
    }

    pub async fn register_user(
        &self,
        handler: &str,
        display_name: &str,
        role: Option<&str>,
        introduction: Option<&str>,
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "register_user",
            json!({
                "handler": handler,
                "display_name": display_name,
                "role": role.unwrap_or("member"),
                "introduction": introduction.unwrap_or("GitIM user"),
            }),
        )
        .await
    }

    /// Overwrite an already-registered user's `introduction` blurb.
    /// The runtime calls this after `add_agent` (post-onboard) and on
    /// `PATCH /workspaces/{slug}/agents/{id}` when the WebUI submits
    /// a new value. The 256-byte ceiling is enforced daemon-side too.
    pub async fn update_user(
        &self,
        handler: &str,
        introduction: &str,
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "update_user",
            json!({
                "handler": handler,
                "introduction": introduction,
            }),
        )
        .await
    }

    pub async fn onboard(
        &self,
        git_server: &str,
        auth: Option<AuthPayload>,
        admin: bool,
        guest: bool,
        join_general: bool,
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "onboard",
            json!({
                "git_server": git_server,
                "auth": auth,
                "admin": admin,
                "guest": guest,
                "join_general": join_general,
            }),
        )
        .await
    }

    pub async fn join_channel(
        &self,
        channel: &str,
        targets: &[String],
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "join_channel",
            json!({
                "channel": channel,
                "targets": targets,
            }),
        )
        .await
    }

    pub async fn leave_channel(
        &self,
        channel: &str,
        targets: &[String],
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "leave_channel",
            json!({
                "channel": channel,
                "targets": targets,
            }),
        )
        .await
    }

    pub async fn create_channel(
        &self,
        name: &str,
        display_name: Option<&str>,
        introduction: Option<&str>,
        invitees: &[String],
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "create_channel",
            json!({
                "name": name,
                "display_name": display_name,
                "introduction": introduction,
                "invitees": invitees,
            }),
        )
        .await
    }

    pub async fn archive_channel(&self, channel: &str) -> Result<ApiResponse, ClientError> {
        self.request("archive_channel", json!({ "channel": channel }))
            .await
    }

    pub async fn unarchive_channel(&self, channel: &str) -> Result<ApiResponse, ClientError> {
        self.request("unarchive_channel", json!({ "channel": channel }))
            .await
    }

    pub async fn list_archived_channels(
        &self,
        prefix: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<ApiResponse, ClientError> {
        let mut body = serde_json::Map::new();
        if let Some(p) = prefix {
            body.insert("prefix".into(), serde_json::Value::String(p.to_string()));
        }
        body.insert("offset".into(), serde_json::Value::Number(offset.into()));
        body.insert("limit".into(), serde_json::Value::Number(limit.into()));
        self.request("archived_channels", serde_json::Value::Object(body))
            .await
    }

    pub async fn archive_dm(&self, peer: &str) -> Result<ApiResponse, ClientError> {
        self.request("archive_dm", json!({ "peer": peer })).await
    }

    pub async fn unarchive_dm(&self, peer: &str) -> Result<ApiResponse, ClientError> {
        self.request("unarchive_dm", json!({ "peer": peer })).await
    }

    /// List the caller's archived DMs with optional prefix filter + paging.
    /// The daemon resolves the caller via `resolve_author` at dispatch time;
    /// `prefix` filters peer handlers case-insensitively, `offset`/`limit`
    /// drive lazy paging. Daemon clamps `limit` to `[1,100]`; the runtime
    /// HTTP layer also clamps as defence-in-depth.
    pub async fn list_archived_dms(
        &self,
        prefix: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<ApiResponse, ClientError> {
        let mut body = serde_json::Map::new();
        if let Some(p) = prefix {
            body.insert("prefix".into(), serde_json::Value::String(p.to_string()));
        }
        body.insert("offset".into(), serde_json::Value::Number(offset.into()));
        body.insert("limit".into(), serde_json::Value::Number(limit.into()));
        self.request("list_archived_dms", serde_json::Value::Object(body))
            .await
    }

    /// List handlers that have been departed (`archive/users/<h>.meta.yaml`
    /// present). Workspace-global, no params — caller-agnostic.
    pub async fn list_archived_users(&self) -> Result<ApiResponse, ClientError> {
        self.request("list_archived_users", json!({})).await
    }

    /// Restore a departed user — moves `archive/users/<handler>.meta.yaml`
    /// back to `users/<handler>.meta.yaml`. The inverse of `depart_user`,
    /// used by WebUI to offer a recovery path from the archived-agent view.
    ///
    /// Daemon resolves caller via the connection's me.json when `author`
    /// is omitted — matches `archive_dm` / `unarchive_dm` shape.
    pub async fn unarchive_user(&self, handler: &str) -> Result<ApiResponse, ClientError> {
        self.request("unarchive_user", json!({ "handler": handler }))
            .await
    }

    pub async fn stop(&self) -> Result<ApiResponse, ClientError> {
        self.request("stop", json!({})).await
    }

    pub async fn poll(&self, since: Option<&str>) -> Result<ApiResponse, ClientError> {
        self.request("poll", json!({ "since": since })).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn search(
        &self,
        query: Option<&str>,
        author: Option<&str>,
        channel: Option<&str>,
        channel_type: Option<&str>,
        limit: Option<u64>,
        offset: Option<u64>,
        include_cards: bool,
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "search",
            json!({
                "query": query,
                "author": author,
                "channel": channel,
                "channel_type": channel_type,
                "limit": limit.unwrap_or(50),
                "offset": offset.unwrap_or(0),
                "include_cards": include_cards,
            }),
        )
        .await
    }

    pub async fn reindex(&self) -> Result<ApiResponse, ClientError> {
        self.request("reindex", json!({})).await
    }

    pub async fn create_card(
        &self,
        channel: &str,
        title: &str,
        labels: Option<&[String]>,
        assignee: Option<&str>,
        status: Option<&str>,
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "create_card",
            json!({
                "channel": channel,
                "title": title,
                "labels": labels,
                "assignee": assignee,
                "status": status,
            }),
        )
        .await
    }

    pub async fn list_cards(
        &self,
        channel: Option<&str>,
        labels: Option<&[String]>,
        status: Option<&str>,
        assignee: Option<&str>,
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "list_cards",
            json!({
                "channel": channel,
                "labels": labels,
                "status": status,
                "assignee": assignee,
            }),
        )
        .await
    }

    pub async fn read_card(
        &self,
        channel: &str,
        card_id: &str,
        limit: Option<u64>,
        since: Option<u64>,
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "read_card",
            json!({
                "channel": channel,
                "card_id": card_id,
                "limit": limit,
                "since": since,
            }),
        )
        .await
    }

    pub async fn send_card_message(
        &self,
        channel: &str,
        card_id: &str,
        body: &str,
        reply_to: Option<u64>,
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "send_card_message",
            json!({
                "channel": channel,
                "card_id": card_id,
                "body": body,
                "reply_to": reply_to,
            }),
        )
        .await
    }

    pub async fn update_card(
        &self,
        channel: &str,
        card_id: &str,
        status: Option<&str>,
        labels: Option<&[String]>,
        assignee: Option<&str>,
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "update_card",
            json!({
                "channel": channel,
                "card_id": card_id,
                "status": status,
                "labels": labels,
                "assignee": assignee,
            }),
        )
        .await
    }

    pub async fn archive_card(
        &self,
        channel: &str,
        card_id: &str,
        author: &str,
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "archive_card",
            json!({
                "channel": channel,
                "card_id": card_id,
                "author": author,
            }),
        )
        .await
    }

    pub async fn unarchive_card(
        &self,
        channel: &str,
        card_id: &str,
        author: &str,
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "unarchive_card",
            json!({
                "channel": channel,
                "card_id": card_id,
                "author": author,
            }),
        )
        .await
    }

    pub async fn list_archived_cards(
        &self,
        channel: Option<&str>,
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "list_archived_cards",
            json!({
                "channel": channel,
            }),
        )
        .await
    }

    /// Compose the archive-protocol "burn" sequence on the daemon side:
    /// append leave-workspace events, archive DMs, scrub channel members,
    /// and finally `git mv users/<h>.meta.yaml` into `archive/users/`.
    ///
    /// The daemon walks an idempotent multi-commit phase chain and uses
    /// `archive/users/<handler>.meta.yaml` as the single source of truth
    /// for "depart complete" — so retrying after a partial failure is
    /// safe and resumes from the first incomplete step.
    pub async fn depart_user(&self, handler: &str) -> Result<ApiResponse, ClientError> {
        self.request("depart_user", json!({ "handler": handler }))
            .await
    }

    /// Read this client's own handler from `<repo_root>/.gitim/me.json` and
    /// request the daemon to depart that handler. Used by `gitim burn-self` —
    /// the agent self-burning its own identity.
    ///
    /// **No handler parameter is accepted** to prevent cross-burn from
    /// caller code (CLI, runtime SDK, etc.) reaching into a clone and
    /// asking the daemon to burn a different agent. The daemon's
    /// `depart_user` is type-agnostic — handler is just a string — so the
    /// safety guard is enforced here at the client surface where we know
    /// the local clone's identity.
    ///
    /// Errors:
    /// - `ProtocolError` when me.json is unreadable, malformed, or has no
    ///   `handler` (guest mode — guests have nothing to depart from).
    pub async fn depart_self(&self) -> Result<ApiResponse, ClientError> {
        let handler = self.read_own_handler()?;
        self.depart_user(&handler).await
    }

    // -- Cron triggers --
    //
    // The cron methods break with the rest of the client surface — they
    // return typed payloads instead of `ApiResponse`. Callers (CLI cron
    // subcommands, runtime `/crons` HTTP layer) want `Vec<CronSummary>`
    // not raw JSON, and the daemon's `error_code` taxonomy
    // (`name_conflict`, `not_found`, `invalid_schedule`, ...) needs to
    // travel up so the CLI can render Chinese messages per code. A typed
    // surface keeps that mapping at the right level.

    /// Create a new cron trigger. `target` accepts `@self`, `@bob`, or a
    /// bare handler `bob` — the daemon strips the leading `@` if present
    /// and resolves `@self` against the connection's me.json author.
    pub async fn create_cron(
        &self,
        name: &str,
        schedule: &str,
        timezone: Option<&str>,
        target: &str,
        prompt: &str,
    ) -> Result<(), ClientError> {
        let resp = self
            .request(
                "create_cron",
                json!({
                    "name": name,
                    "schedule": schedule,
                    "timezone": timezone,
                    "target": target,
                    "prompt": prompt,
                }),
            )
            .await?;
        decode_unit(resp)
    }

    pub async fn list_crons(&self) -> Result<Vec<CronSummary>, ClientError> {
        let resp = self.request("list_crons", json!({})).await?;
        let payload: ListCronsResponse = decode_typed(resp)?;
        Ok(payload.crons)
    }

    pub async fn show_cron(&self, name: &str) -> Result<CronDetail, ClientError> {
        let resp = self.request("show_cron", json!({ "name": name })).await?;
        decode_typed(resp)
    }

    pub async fn history_cron(
        &self,
        name: &str,
        limit: Option<u32>,
    ) -> Result<Vec<CronRunEntry>, ClientError> {
        let resp = self
            .request(
                "history_cron",
                json!({
                    "name": name,
                    "limit": limit,
                }),
            )
            .await?;
        let payload: HistoryCronResponse = decode_typed(resp)?;
        Ok(payload.runs)
    }

    pub async fn enable_cron(&self, name: &str) -> Result<ToggleCronResponse, ClientError> {
        let resp = self.request("enable_cron", json!({ "name": name })).await?;
        decode_typed(resp)
    }

    pub async fn disable_cron(&self, name: &str) -> Result<ToggleCronResponse, ClientError> {
        let resp = self
            .request("disable_cron", json!({ "name": name }))
            .await?;
        decode_typed(resp)
    }

    pub async fn delete_cron(&self, name: &str) -> Result<(), ClientError> {
        let resp = self.request("delete_cron", json!({ "name": name })).await?;
        decode_unit(resp)
    }

    /// Convenience wrapper: re-runs `show_cron` and pulls the computed
    /// `next_fire`. The daemon stores `next_fire` as ISO 8601 UTC; this
    /// parses it into a `DateTime<Utc>` for callers that need to
    /// compare to `Utc::now()`. Returns `None` when the daemon couldn't
    /// compute one (disabled spec or unparseable schedule).
    pub async fn next_fire_for(&self, name: &str) -> Result<Option<DateTime<Utc>>, ClientError> {
        let detail = self.show_cron(name).await?;
        match detail.next_fire {
            None => Ok(None),
            Some(s) => DateTime::parse_from_rfc3339(&s)
                .map(|dt| Some(dt.with_timezone(&Utc)))
                .map_err(|e| {
                    ClientError::ProtocolError(format!(
                        "daemon returned unparseable next_fire {s:?}: {e}"
                    ))
                }),
        }
    }

    /// Resolve the caller's own handler from local `me.json`. Pulled out so
    /// `depart_self` can be unit-tested against a temp `me.json` without
    /// hitting the daemon socket.
    fn read_own_handler(&self) -> Result<String, ClientError> {
        let me_path = self.repo_root.join(".gitim/me.json");
        let contents = std::fs::read_to_string(&me_path).map_err(|e| {
            ClientError::ProtocolError(format!("failed to read {}: {e}", me_path.display()))
        })?;
        let me: MeJson = serde_json::from_str(&contents).map_err(|e| {
            ClientError::ProtocolError(format!("failed to parse {}: {e}", me_path.display()))
        })?;
        me.handler.ok_or_else(|| {
            ClientError::ProtocolError(format!(
                "{} has no handler (guest mode cannot depart)",
                me_path.display()
            ))
        })
    }

    pub async fn board_show(&self, handler: &str) -> Result<ApiResponse, ClientError> {
        self.request("board_show", json!({ "handler": handler }))
            .await
    }

    pub async fn board_list(&self) -> Result<ApiResponse, ClientError> {
        self.request("board_list", json!({})).await
    }

    pub async fn board_init(&self) -> Result<ApiResponse, ClientError> {
        self.request("board_init", json!({})).await
    }

    pub async fn board_publish(&self, content: Option<&str>) -> Result<ApiResponse, ClientError> {
        self.request("board_publish", json!({ "content": content }))
            .await
    }

    pub async fn board_set(&self, field: &str, value: &str) -> Result<ApiResponse, ClientError> {
        self.request(
            "board_set",
            json!({
                "field": field,
                "value": value,
            }),
        )
        .await
    }

    pub async fn board_section_set(
        &self,
        section: &str,
        value: &str,
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "board_section_set",
            json!({
                "section": section,
                "value": value,
            }),
        )
        .await
    }

    pub async fn board_section_append(
        &self,
        section: &str,
        value: &str,
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "board_section_append",
            json!({
                "section": section,
                "value": value,
            }),
        )
        .await
    }

    pub async fn flow_list(&self) -> Result<ApiResponse, ClientError> {
        self.request("flow_list", json!({})).await
    }

    pub async fn flow_show(&self, slug: &str) -> Result<ApiResponse, ClientError> {
        self.request("flow_show", json!({ "slug": slug })).await
    }

    pub async fn flow_create(
        &self,
        slug: &str,
        name: &str,
        description: &str,
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "flow_create",
            json!({
                "slug": slug,
                "name": name,
                "description": description,
            }),
        )
        .await
    }

    pub async fn flow_remove(&self, slug: &str) -> Result<ApiResponse, ClientError> {
        self.request("flow_remove", json!({ "slug": slug })).await
    }

    pub async fn flow_validate(&self, slug: &str) -> Result<ApiResponse, ClientError> {
        self.request("flow_validate", json!({ "slug": slug })).await
    }

    pub async fn flow_run_start(
        &self,
        slug: &str,
        channel: &str,
    ) -> Result<ApiResponse, ClientError> {
        self.request("flow_run_start", json!({"slug": slug, "channel": channel}))
            .await
    }

    pub async fn flow_run_list(
        &self,
        slug: Option<&str>,
        channel: Option<&str>,
        status: Option<&str>,
    ) -> Result<ApiResponse, ClientError> {
        let mut params = serde_json::Map::new();
        if let Some(s) = slug {
            params.insert("slug".into(), json!(s));
        }
        if let Some(c) = channel {
            params.insert("channel".into(), json!(c));
        }
        if let Some(st) = status {
            params.insert("status".into(), json!(st));
        }
        self.request("flow_run_list", serde_json::Value::Object(params))
            .await
    }

    pub async fn flow_run_show(&self, run_id: &str) -> Result<ApiResponse, ClientError> {
        self.request("flow_run_show", json!({"run_id": run_id}))
            .await
    }

    pub async fn flow_node_set(
        &self,
        run_id: &str,
        node_id: &str,
        status: &str,
        actor: Option<&str>,
        result_ref: Option<&str>,
    ) -> Result<ApiResponse, ClientError> {
        let mut params = serde_json::Map::new();
        params.insert("run_id".into(), json!(run_id));
        params.insert("node_id".into(), json!(node_id));
        params.insert("status".into(), json!(status));
        if let Some(a) = actor {
            params.insert("actor".into(), json!(a));
        }
        if let Some(r) = result_ref {
            params.insert("result_ref".into(), json!(r));
        }
        self.request("flow_node_set", serde_json::Value::Object(params))
            .await
    }

    pub async fn flow_run_cancel(&self, run_id: &str) -> Result<ApiResponse, ClientError> {
        self.request("flow_run_cancel", json!({"run_id": run_id}))
            .await
    }
}

/// Decode a daemon response into a typed payload `T`. Used by the cron
/// methods (and any future typed surface) to fold the `ok` / `error` /
/// `error_code` envelope into a `ClientError::Api { message, code }` so
/// callers can match on the typed code without unwrapping ApiResponse.
fn decode_typed<T: serde::de::DeserializeOwned>(resp: ApiResponse) -> Result<T, ClientError> {
    if !resp.ok {
        return Err(ClientError::Api {
            message: resp
                .error
                .unwrap_or_else(|| "unknown daemon error".to_string()),
            code: resp.error_code,
        });
    }
    resp.parse_data::<T>()
}

/// Variant of `decode_typed` for handlers that return only an ack (no
/// payload). Same error semantics on the failure path.
fn decode_unit(resp: ApiResponse) -> Result<(), ClientError> {
    if !resp.ok {
        return Err(ClientError::Api {
            message: resp
                .error
                .unwrap_or_else(|| "unknown daemon error".to_string()),
            code: resp.error_code,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    use tokio::net::UnixListener;

    fn write_me_json(repo_root: &Path, body: &str) {
        let dir = repo_root.join(".gitim");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("me.json"), body).unwrap();
    }

    #[test]
    fn read_own_handler_returns_handler_from_me_json() {
        let tmp = TempDir::new().unwrap();
        write_me_json(
            tmp.path(),
            r#"{"handler":"alice","display_name":"Alice W"}"#,
        );
        let client = GitimClient::new(tmp.path());
        let handler = client.read_own_handler().unwrap();
        assert_eq!(handler, "alice");
    }

    #[test]
    fn read_own_handler_errors_when_me_json_missing() {
        let tmp = TempDir::new().unwrap();
        // No .gitim/ at all.
        let client = GitimClient::new(tmp.path());
        let err = client.read_own_handler().unwrap_err();
        assert!(matches!(err, ClientError::ProtocolError(_)));
        let msg = err.to_string();
        assert!(msg.contains("failed to read"), "msg = {msg}");
        assert!(msg.contains("me.json"), "msg = {msg}");
    }

    #[test]
    fn read_own_handler_errors_when_me_json_malformed() {
        let tmp = TempDir::new().unwrap();
        write_me_json(tmp.path(), "not json {{{");
        let client = GitimClient::new(tmp.path());
        let err = client.read_own_handler().unwrap_err();
        assert!(matches!(err, ClientError::ProtocolError(_)));
        let msg = err.to_string();
        assert!(msg.contains("failed to parse"), "msg = {msg}");
    }

    /// Guest-mode me.json has `handler: null`. depart_self must refuse —
    /// a guest has no identity in users/<handler>.meta.yaml to archive,
    /// so the daemon would error anyway. Failing fast at the client
    /// surface gives a clearer message.
    #[test]
    fn read_own_handler_errors_when_handler_null_guest_mode() {
        let tmp = TempDir::new().unwrap();
        write_me_json(tmp.path(), r#"{"handler":null,"guest":true}"#);
        let client = GitimClient::new(tmp.path());
        let err = client.read_own_handler().unwrap_err();
        assert!(matches!(err, ClientError::ProtocolError(_)));
        let msg = err.to_string();
        assert!(msg.contains("no handler"), "msg = {msg}");
    }

    /// `depart_self` is the public entrypoint and accepts no params — this
    /// is the cross-burn safety guard. If the signature ever drifts to take
    /// an argument, this test stops compiling and reviewers must justify it.
    #[tokio::test]
    async fn depart_self_takes_no_handler_param() {
        let tmp = TempDir::new().unwrap();
        write_me_json(tmp.path(), r#"{"handler":"alice"}"#);
        let client = GitimClient::new(tmp.path());
        // No daemon running → DaemonNotRunning. The point of this test is
        // signature-shape: depart_self() with no args compiles and reaches
        // the socket layer. The pre-socket me.json read also succeeded,
        // so any failure other than DaemonNotRunning is a regression.
        let result = client.depart_self().await;
        let err = result.unwrap_err();
        assert!(
            matches!(err, ClientError::DaemonNotRunning),
            "expected DaemonNotRunning (no daemon spawned), got: {err:?}",
        );
    }

    /// depart_self short-circuits with a clear error before touching the
    /// daemon socket when me.json is missing — so a misconfigured clone
    /// can't accidentally fall through to a daemon error path.
    #[tokio::test]
    async fn depart_self_short_circuits_when_me_json_missing() {
        let tmp = TempDir::new().unwrap();
        let client = GitimClient::new(tmp.path());
        let err = client.depart_self().await.unwrap_err();
        // Should fail at me.json read, not at socket connect.
        assert!(matches!(err, ClientError::ProtocolError(_)));
        assert!(err.to_string().contains("me.json"));
    }

    #[tokio::test]
    async fn request_times_out_when_daemon_accepts_but_never_replies() {
        let tmp = TempDir::new().unwrap();
        let run_dir = tmp.path().join(".gitim/run");
        fs::create_dir_all(&run_dir).unwrap();
        let socket_path = run_dir.join("gitim.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let _server = tokio::spawn(async move {
            if let Ok((_stream, _addr)) = listener.accept().await {
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        });

        let client = GitimClient::new(tmp.path());
        let err = client.status().await.unwrap_err();
        assert!(matches!(err, ClientError::Timeout));
    }

    /// build_request shapes for the no-param archive-protocol methods.
    /// These are the wire payloads the daemon dispatches on; if the
    /// method strings drift the daemon-side switch in handlers/depart.rs
    /// will silently fail to match.
    #[test]
    fn build_request_shapes_archive_dm_methods() {
        use crate::types::build_request;

        let archive = build_request("archive_dm", json!({"peer": "bob"}));
        assert_eq!(archive, json!({"method": "archive_dm", "peer": "bob"}),);

        let unarchive = build_request("unarchive_dm", json!({"peer": "bob"}));
        assert_eq!(unarchive, json!({"method": "unarchive_dm", "peer": "bob"}),);

        let list_dms = build_request("list_archived_dms", json!({}));
        assert_eq!(list_dms, json!({"method": "list_archived_dms"}));

        // Pagination wire shape: when the client passes prefix/offset/limit,
        // they ride along inside the same flat JSON object the daemon
        // dispatches on. If this drifts, the daemon-side
        // `Request::ListArchivedDms` serde deserializer in api.rs will
        // silently fall back to defaults and the prefix/page won't apply.
        let list_dms_paged = build_request(
            "list_archived_dms",
            json!({"prefix": "al", "offset": 0, "limit": 5}),
        );
        assert_eq!(
            list_dms_paged,
            json!({
                "method": "list_archived_dms",
                "prefix": "al",
                "offset": 0,
                "limit": 5,
            }),
        );

        let list_channels_paged =
            build_request("archived_channels", json!({"offset": 10, "limit": 25}));
        assert_eq!(
            list_channels_paged,
            json!({
                "method": "archived_channels",
                "offset": 10,
                "limit": 25,
            }),
        );

        let list_users = build_request("list_archived_users", json!({}));
        assert_eq!(list_users, json!({"method": "list_archived_users"}));

        let unarchive_user = build_request("unarchive_user", json!({"handler": "bob"}));
        assert_eq!(
            unarchive_user,
            json!({"method": "unarchive_user", "handler": "bob"}),
        );
    }

    // -- Cron methods --
    //
    // The cron methods take a typed surface (Vec<CronSummary> etc.) so the
    // round-trip happens in two halves:
    //   1. build_request → wire JSON the daemon will dispatch on;
    //   2. decode_typed / decode_unit → consume the daemon's response
    //      envelope, mapping ok=false to ClientError::Api { code }.
    // Together these cover the same surface area a real-socket roundtrip
    // would, without depending on a running daemon.

    #[test]
    fn cron_create_request_shape() {
        use crate::types::build_request;
        let req = build_request(
            "create_cron",
            json!({
                "name": "weekly-report",
                "schedule": "0 9 * * 1",
                "timezone": "America/Los_Angeles",
                "target": "@self",
                "prompt": "summarize last week",
            }),
        );
        assert_eq!(
            req,
            json!({
                "method": "create_cron",
                "name": "weekly-report",
                "schedule": "0 9 * * 1",
                "timezone": "America/Los_Angeles",
                "target": "@self",
                "prompt": "summarize last week",
            }),
        );
    }

    #[test]
    fn cron_create_request_shape_no_timezone() {
        use crate::types::build_request;
        // timezone: None serializes to JSON null — daemon's
        // `#[serde(default)] timezone: Option<String>` accepts both
        // missing key and null and treats both as "default to UTC".
        let req = build_request(
            "create_cron",
            json!({
                "name": "daily",
                "schedule": "@daily",
                "timezone": null,
                "target": "alice",
                "prompt": "hi",
            }),
        );
        assert_eq!(req.get("timezone"), Some(&Value::Null),);
    }

    #[test]
    fn cron_list_request_shape() {
        use crate::types::build_request;
        let req = build_request("list_crons", json!({}));
        assert_eq!(req, json!({"method": "list_crons"}));
    }

    #[test]
    fn cron_show_request_shape() {
        use crate::types::build_request;
        let req = build_request("show_cron", json!({"name": "weekly-report"}));
        assert_eq!(req, json!({"method": "show_cron", "name": "weekly-report"}),);
    }

    #[test]
    fn cron_history_request_shape_with_limit() {
        use crate::types::build_request;
        let req = build_request("history_cron", json!({"name": "daily", "limit": 10}));
        assert_eq!(
            req,
            json!({"method": "history_cron", "name": "daily", "limit": 10}),
        );
    }

    #[test]
    fn cron_history_request_shape_without_limit() {
        use crate::types::build_request;
        let req = build_request("history_cron", json!({"name": "daily", "limit": null}));
        assert_eq!(req.get("limit"), Some(&Value::Null));
    }

    #[test]
    fn cron_enable_disable_delete_request_shapes() {
        use crate::types::build_request;
        let enable = build_request("enable_cron", json!({"name": "weekly"}));
        assert_eq!(enable, json!({"method": "enable_cron", "name": "weekly"}),);

        let disable = build_request("disable_cron", json!({"name": "weekly"}));
        assert_eq!(disable, json!({"method": "disable_cron", "name": "weekly"}),);

        let delete = build_request("delete_cron", json!({"name": "old-job"}));
        assert_eq!(delete, json!({"method": "delete_cron", "name": "old-job"}),);
    }

    /// decode_unit happy-path: ok=true is mapped to Ok(()).
    #[test]
    fn decode_unit_ok_returns_unit() {
        let resp = ApiResponse {
            ok: true,
            data: Some(json!({"name": "weekly"})),
            error: None,
            error_code: None,
        };
        let r = decode_unit(resp);
        assert!(r.is_ok());
    }

    /// decode_unit error-path: error_code surfaces in ClientError::Api.code
    /// so callers can render code-specific messages.
    #[test]
    fn decode_unit_error_carries_code() {
        let resp = ApiResponse {
            ok: false,
            data: None,
            error: Some("cron 'weekly' already exists".to_string()),
            error_code: Some("name_conflict".to_string()),
        };
        let err = decode_unit(resp).unwrap_err();
        match err {
            ClientError::Api { message, code } => {
                assert_eq!(code.as_deref(), Some("name_conflict"));
                assert!(message.contains("already exists"));
            }
            other => panic!("expected Api, got {other:?}"),
        }
    }

    /// decode_typed extracts a typed payload on success.
    #[test]
    fn decode_typed_ok_extracts_payload() {
        let resp = ApiResponse {
            ok: true,
            data: Some(json!({
                "crons": [
                    {
                        "name": "weekly-report",
                        "schedule": "0 9 * * 1",
                        "target": "alice",
                        "enabled": true,
                        "created_by": "alice",
                        "created_at": "2026-05-09T00:00:00Z",
                        "next_fire": "2026-05-11T16:00:00Z"
                    }
                ]
            })),
            error: None,
            error_code: None,
        };
        let payload: ListCronsResponse = decode_typed(resp).unwrap();
        assert_eq!(payload.crons.len(), 1);
        assert_eq!(payload.crons[0].name, "weekly-report");
        assert_eq!(
            payload.crons[0].next_fire.as_deref(),
            Some("2026-05-11T16:00:00Z")
        );
    }

    /// decode_typed error-path: ok=false maps to ClientError::Api with
    /// preserved error_code (the typed taxonomy).
    #[test]
    fn decode_typed_error_carries_code() {
        let resp = ApiResponse {
            ok: false,
            data: None,
            error: Some("cron 'missing' does not exist".to_string()),
            error_code: Some("not_found".to_string()),
        };
        let r: Result<CronDetail, _> = decode_typed(resp);
        let err = r.unwrap_err();
        match err {
            ClientError::Api { code, .. } => {
                assert_eq!(code.as_deref(), Some("not_found"));
            }
            other => panic!("expected Api, got {other:?}"),
        }
    }

    /// decode_typed for an ok=false without error_code — legacy daemon
    /// path. ClientError::Api.code is None, message is preserved.
    #[test]
    fn decode_typed_error_without_code_falls_back_to_message() {
        let resp = ApiResponse {
            ok: false,
            data: None,
            error: Some("legacy error".to_string()),
            error_code: None,
        };
        let r: Result<CronDetail, _> = decode_typed(resp);
        match r.unwrap_err() {
            ClientError::Api { message, code } => {
                assert!(code.is_none());
                assert_eq!(message, "legacy error");
            }
            other => panic!("expected Api, got {other:?}"),
        }
    }

    /// next_fire_for parses the ISO 8601 string the daemon emits.
    /// The daemon's CronDetail.next_fire is wire-typed as Option<String>;
    /// the client converts it to DateTime<Utc> for caller convenience.
    #[test]
    fn next_fire_parsing_from_show_response() {
        // Same parse path next_fire_for runs after show_cron.
        let detail: CronDetail = serde_json::from_value(json!({
            "name": "weekly-report",
            "spec": {},
            "recent_runs": [],
            "next_fire": "2026-05-11T16:00:00Z"
        }))
        .unwrap();
        let s = detail.next_fire.unwrap();
        let parsed = DateTime::parse_from_rfc3339(&s).unwrap();
        assert_eq!(
            parsed.with_timezone(&Utc).to_rfc3339(),
            "2026-05-11T16:00:00+00:00"
        );
    }

    /// next_fire absent is propagated as Ok(None) — disabled spec or
    /// unparseable schedule, the daemon emits null and the client doesn't
    /// fabricate a timestamp.
    #[test]
    fn next_fire_none_when_daemon_omits() {
        let detail: CronDetail = serde_json::from_value(json!({
            "name": "no-fire",
            "spec": {},
            "recent_runs": []
        }))
        .unwrap();
        assert!(detail.next_fire.is_none());
    }

    /// CronRunEntry decodes from history_cron's `runs` field.
    #[test]
    fn cron_history_response_decodes_runs() {
        let resp = ApiResponse {
            ok: true,
            data: Some(json!({
                "name": "weekly-report",
                "runs": [
                    {"ts": "2026-05-11T09-00-00Z", "filename": "2026-05-11T09-00-00Z.thread"},
                    {"ts": "2026-05-04T09-00-00Z", "filename": "2026-05-04T09-00-00Z.thread"}
                ]
            })),
            error: None,
            error_code: None,
        };
        let payload: HistoryCronResponse = decode_typed(resp).unwrap();
        assert_eq!(payload.runs.len(), 2);
        assert_eq!(payload.runs[0].ts, "2026-05-11T09-00-00Z");
    }

    /// ToggleCronResponse decoding — used by enable_cron / disable_cron.
    /// Idempotent path returns changed=false.
    #[test]
    fn toggle_cron_response_decodes_changed_flag() {
        let resp = ApiResponse {
            ok: true,
            data: Some(json!({
                "name": "weekly-report",
                "enabled": false,
                "changed": false
            })),
            error: None,
            error_code: None,
        };
        let payload: ToggleCronResponse = decode_typed(resp).unwrap();
        assert!(!payload.changed);
        assert!(!payload.enabled);
        assert_eq!(payload.name, "weekly-report");
    }
}
