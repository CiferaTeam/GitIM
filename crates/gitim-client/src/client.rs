use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::error::ClientError;
use crate::types::{build_request, ApiResponse};

pub struct GitimClient {
    socket_path: PathBuf,
}

impl GitimClient {
    pub fn new(repo_root: &Path) -> Self {
        Self {
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

    pub async fn onboard(
        &self,
        git_server: &str,
        auth: Value,
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
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "create_channel",
            json!({
                "name": name,
                "display_name": display_name,
                "introduction": introduction,
            }),
        )
        .await
    }

    pub async fn archive_channel(&self, channel: &str) -> Result<ApiResponse, ClientError> {
        self.request("archive_channel", json!({ "channel": channel }))
            .await
    }

    pub async fn list_archived_channels(&self) -> Result<ApiResponse, ClientError> {
        self.request("archived_channels", json!({})).await
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
            }),
        )
        .await
    }

    pub async fn reindex(&self) -> Result<ApiResponse, ClientError> {
        self.request("reindex", json!({})).await
    }

    pub async fn create_board(
        &self,
        name: &str,
        display_name: Option<&str>,
        statuses: Option<&[String]>,
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "create_board",
            json!({
                "name": name,
                "display_name": display_name,
                "statuses": statuses,
            }),
        )
        .await
    }

    pub async fn create_card(
        &self,
        board: &str,
        title: &str,
        assignee: Option<&str>,
        status: Option<&str>,
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "create_card",
            json!({
                "board": board,
                "title": title,
                "assignee": assignee,
                "status": status,
            }),
        )
        .await
    }

    pub async fn list_boards(&self) -> Result<ApiResponse, ClientError> {
        self.request("list_boards", json!({})).await
    }

    pub async fn list_cards(
        &self,
        board: &str,
        status: Option<&str>,
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "list_cards",
            json!({
                "board": board,
                "status": status,
            }),
        )
        .await
    }

    pub async fn read_card(
        &self,
        board: &str,
        card_id: &str,
        limit: Option<u64>,
        since: Option<u64>,
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "read_card",
            json!({
                "board": board,
                "card_id": card_id,
                "limit": limit,
                "since": since,
            }),
        )
        .await
    }

    pub async fn send_card_message(
        &self,
        board: &str,
        card_id: &str,
        body: &str,
        reply_to: Option<u64>,
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "send_card_message",
            json!({
                "board": board,
                "card_id": card_id,
                "body": body,
                "reply_to": reply_to,
            }),
        )
        .await
    }

    pub async fn update_card(
        &self,
        board: &str,
        card_id: &str,
        status: Option<&str>,
        assignee: Option<&str>,
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "update_card",
            json!({
                "board": board,
                "card_id": card_id,
                "status": status,
                "assignee": assignee,
            }),
        )
        .await
    }
}
