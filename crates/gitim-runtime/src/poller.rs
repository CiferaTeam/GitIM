use gitim_client::GitimClient;

use crate::error::RuntimeError;

#[derive(Debug)]
pub struct ChannelChange {
    pub channel: String,
    pub kind: String,
    pub entries: Vec<serde_json::Value>,
}

#[derive(Debug)]
pub struct PollResult {
    pub changes: Vec<ChannelChange>,
}

pub struct Poller {
    client: GitimClient,
    cursor: Option<String>,
}

impl Poller {
    pub fn new(client: GitimClient) -> Self {
        Self {
            client,
            cursor: None,
        }
    }

    /// Create a Poller with a saved cursor (for restart recovery).
    pub fn with_cursor(client: GitimClient, cursor: String) -> Self {
        Self {
            client,
            cursor: Some(cursor),
        }
    }

    /// Poll the daemon for new changes since the last cursor.
    ///
    /// First call initializes the cursor and returns empty changes.
    /// Subsequent calls return changes since the last poll.
    pub async fn poll(&mut self) -> Result<PollResult, RuntimeError> {
        let resp = self
            .client
            .poll(self.cursor.as_deref())
            .await
            .map_err(|e| RuntimeError::PollFailed(e.to_string()))?;

        if !resp.ok {
            let msg = resp.error.unwrap_or_else(|| "poll failed".into());
            return Err(RuntimeError::PollFailed(msg));
        }

        let data = resp.data.ok_or_else(|| {
            RuntimeError::PollFailed("poll response missing data".into())
        })?;

        // Update cursor
        if let Some(commit_id) = data["commit_id"].as_str() {
            self.cursor = Some(commit_id.to_string());
        }

        // Parse changes
        let changes = data["changes"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|c| {
                        let channel = c["channel"].as_str()?.to_string();
                        let kind = c["kind"].as_str()?.to_string();
                        let entries = c["entries"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default();
                        Some(ChannelChange { channel, kind, entries })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(PollResult { changes })
    }

    /// Returns the current cursor value (commit hash), if initialized.
    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }
}
