use gitim_client::{ApiResponse, GitimClient};

use crate::error::RuntimeError;

/// Map a non-ok daemon poll response to a typed `RuntimeError`.
///
/// Single source of truth for the `error_code` wire contract — both
/// `poll()` and `peek()` route through here so a new tagged code only
/// has to be wired in one place. Caller must have already checked
/// `!resp.ok` before calling; passing an `ok=true` response would
/// silently produce a `PollFailed` with whatever `resp.error` happened
/// to be (which is fine — it's nonsensical input).
fn map_response_error(resp: ApiResponse) -> RuntimeError {
    if resp.error_code.as_deref() == Some("self_departed") {
        RuntimeError::SelfDeparted
    } else {
        RuntimeError::PollFailed(resp.error.unwrap_or_else(|| "poll failed".into()))
    }
}

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
            // Map daemon's tagged error codes to typed RuntimeError variants
            // so call sites (agent_loop) can pattern-match on the failure
            // mode instead of substring-grepping the human message. The
            // wire-level "error_code" contract lives in `map_response_error`.
            return Err(map_response_error(resp));
        }

        let data = resp
            .data
            .ok_or_else(|| RuntimeError::PollFailed("poll response missing data".into()))?;

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
                        let entries = c["entries"].as_array().cloned().unwrap_or_default();
                        Some(ChannelChange {
                            channel,
                            kind,
                            entries,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(PollResult { changes })
    }

    /// Check for new changes without advancing the cursor.
    ///
    /// Same as `poll()` but does not update the internal cursor.
    /// Used by steering detection to check for urgent messages
    /// while the provider is executing.
    pub async fn peek(&self) -> Result<PollResult, RuntimeError> {
        let resp = self
            .client
            .poll(self.cursor.as_deref())
            .await
            .map_err(|e| RuntimeError::PollFailed(e.to_string()))?;

        if !resp.ok {
            // Mirror `poll`'s mapping for symmetry of the wire contract.
            // Note: `peek` callers (agent_loop steering checks during
            // provider execution) currently treat all peek errors as
            // warn+continue — the typed `SelfDeparted` variant is here
            // for future-proofing, NOT yet wired to trigger mid-session
            // self-heal. The next full `poll()` cycle catches a
            // self-departed handler and routes it through the agent_loop
            // SelfDeparted arm correctly.
            return Err(map_response_error(resp));
        }

        let data = resp
            .data
            .ok_or_else(|| RuntimeError::PollFailed("poll response missing data".into()))?;

        let changes = data["changes"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|c| {
                        let channel = c["channel"].as_str()?.to_string();
                        let kind = c["kind"].as_str()?.to_string();
                        let entries = c["entries"].as_array().cloned().unwrap_or_default();
                        Some(ChannelChange {
                            channel,
                            kind,
                            entries,
                        })
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
