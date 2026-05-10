use std::path::{Path, PathBuf};

use gitim_core::auth_payload::AuthPayload;
use gitim_core::me_json::MeJson;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::error::ClientError;
use crate::types::{build_request, ApiResponse};

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
        reader
            .read_line(&mut line)
            .await
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
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "onboard",
            json!({
                "git_server": git_server,
                "auth": auth,
                "admin": admin,
                "guest": guest,
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

    pub async fn list_archived_channels(&self) -> Result<ApiResponse, ClientError> {
        self.request("archived_channels", json!({})).await
    }

    pub async fn archive_dm(&self, peer: &str) -> Result<ApiResponse, ClientError> {
        self.request("archive_dm", json!({ "peer": peer })).await
    }

    pub async fn unarchive_dm(&self, peer: &str) -> Result<ApiResponse, ClientError> {
        self.request("unarchive_dm", json!({ "peer": peer })).await
    }

    /// List the caller's archived DMs. The daemon resolves the caller via
    /// `resolve_author` at dispatch time, so the client passes no params —
    /// matches the `list_archived_channels` shape.
    pub async fn list_archived_dms(&self) -> Result<ApiResponse, ClientError> {
        self.request("list_archived_dms", json!({})).await
    }

    /// List handlers that have been departed (`archive/users/<h>.meta.yaml`
    /// present). Workspace-global, no params — caller-agnostic.
    pub async fn list_archived_users(&self) -> Result<ApiResponse, ClientError> {
        self.request("list_archived_users", json!({})).await
    }

    /// Restore a departed user — moves `archive/users/<handler>.meta.yaml`
    /// back to `users/<handler>.meta.yaml`. The runtime burn endpoint
    /// (B.1) drives departure via `depart_user`; this is the inverse, so
    /// WebUI can offer a recovery path on the archived-agent view (E.3).
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
    /// safe and resumes from the first incomplete step. C.1 will add
    /// the rest of the archive-protocol surface (`archive_dm`,
    /// `unarchive_dm`, etc.) to this client; B.1 only needs `depart_user`
    /// to unblock the runtime burn endpoint.
    pub async fn depart_user(&self, handler: &str) -> Result<ApiResponse, ClientError> {
        self.request("depart_user", json!({ "handler": handler }))
            .await
    }

    /// Read this client's own handler from `<repo_root>/.gitim/me.json` and
    /// request the daemon to depart that handler. Used by `gitim burn-self`
    /// (C.3) — the agent self-burning its own identity.
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

    /// Resolve the caller's own handler from local `me.json`. Pulled out so
    /// `depart_self` can be unit-tested against a temp `me.json` without
    /// hitting the daemon socket.
    fn read_own_handler(&self) -> Result<String, ClientError> {
        let me_path = self.repo_root.join(".gitim/me.json");
        let contents = std::fs::read_to_string(&me_path).map_err(|e| {
            ClientError::ProtocolError(format!(
                "failed to read {}: {e}",
                me_path.display()
            ))
        })?;
        let me: MeJson = serde_json::from_str(&contents).map_err(|e| {
            ClientError::ProtocolError(format!(
                "failed to parse {}: {e}",
                me_path.display()
            ))
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

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

    /// build_request shapes for the new no-param archive-protocol methods.
    /// These are the wire payloads the daemon will dispatch on; if the
    /// method strings drift the daemon-side switch in handlers/depart.rs
    /// will silently fail to match.
    #[test]
    fn build_request_shapes_archive_dm_methods() {
        use crate::types::build_request;

        let archive = build_request("archive_dm", json!({"peer": "bob"}));
        assert_eq!(
            archive,
            json!({"method": "archive_dm", "peer": "bob"}),
        );

        let unarchive = build_request("unarchive_dm", json!({"peer": "bob"}));
        assert_eq!(
            unarchive,
            json!({"method": "unarchive_dm", "peer": "bob"}),
        );

        let list_dms = build_request("list_archived_dms", json!({}));
        assert_eq!(list_dms, json!({"method": "list_archived_dms"}));

        let list_users = build_request("list_archived_users", json!({}));
        assert_eq!(list_users, json!({"method": "list_archived_users"}));

        let unarchive_user = build_request("unarchive_user", json!({"handler": "bob"}));
        assert_eq!(
            unarchive_user,
            json!({"method": "unarchive_user", "handler": "bob"}),
        );
    }
}
