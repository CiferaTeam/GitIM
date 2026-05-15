use std::time::Duration;

use futures::StreamExt;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::Serialize;
use tokio::task::AbortHandle;

use crate::http::{AgentActivityEvent, SharedRuntimeState};
use crate::user_config::FleetNodeEntry;

const RECONNECT_DELAY: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FleetEventEnvelope {
    AgentActivity(FleetAgentActivityEvent),
    NodeStatus(FleetNodeStatus),
}

#[derive(Clone, Debug, Serialize)]
pub struct FleetAgentActivityEvent {
    pub node_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_name: Option<String>,
    pub workspace_id: String,
    pub agent_id: String,
    pub received_at: String,
    pub event: AgentActivityEvent,
}

#[derive(Clone, Debug, Serialize)]
pub struct FleetNodeStatus {
    pub node_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_name: Option<String>,
    pub workspace_id: String,
    pub status: FleetNodeConnectionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_connected_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_event_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub retry_count: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FleetNodeConnectionStatus {
    Connecting,
    Connected,
    Down,
}

pub struct FleetNodeRuntime {
    pub entry: FleetNodeEntry,
    handles: Vec<AbortHandle>,
}

impl FleetNodeRuntime {
    fn new(state: SharedRuntimeState, entry: FleetNodeEntry) -> Self {
        let mut handles = Vec::new();
        for workspace in &entry.workspaces {
            let state = state.clone();
            let entry = entry.clone();
            let workspace = workspace.clone();
            let handle = tokio::spawn(async move {
                subscribe_workspace_loop(state, entry, workspace).await;
            });
            handles.push(handle.abort_handle());
        }
        Self { entry, handles }
    }
}

impl Drop for FleetNodeRuntime {
    fn drop(&mut self) {
        for handle in &self.handles {
            handle.abort();
        }
    }
}

pub fn activate_node(state: SharedRuntimeState, entry: FleetNodeEntry) {
    let runtime = FleetNodeRuntime::new(state.clone(), entry.clone());
    let mut initial_statuses = Vec::new();
    {
        let mut s = state.lock().unwrap();
        s.fleet_nodes.insert(entry.node_id.clone(), runtime);
        for workspace in &entry.workspaces {
            let status = FleetNodeStatus {
                node_id: entry.node_id.clone(),
                node_ip: entry.node_ip.clone(),
                node_name: entry.node_name.clone(),
                workspace_id: workspace.clone(),
                status: FleetNodeConnectionStatus::Connecting,
                last_connected_at: None,
                last_event_at: None,
                last_error: None,
                retry_count: 0,
            };
            s.fleet_status
                .insert(status_key(&entry.node_id, workspace), status.clone());
            initial_statuses.push(status);
        }
    }
    for status in initial_statuses {
        publish_status(&state, status);
    }
}

pub fn remove_node(state: &SharedRuntimeState, node_id: &str) -> bool {
    let mut s = state.lock().unwrap();
    s.fleet_status
        .retain(|_, status| status.node_id.as_str() != node_id);
    s.fleet_nodes.remove(node_id).is_some()
}

pub fn recover_from_config(state: SharedRuntimeState) {
    let cfg = crate::user_config::read();
    for entry in cfg.fleet_nodes {
        if let Err(err) = validate_node(&entry) {
            tracing::warn!(node_id = %entry.node_id, error = %err, "skipping invalid fleet node");
            continue;
        }
        activate_node(state.clone(), normalize_node(entry));
    }
}

pub fn validate_node(entry: &FleetNodeEntry) -> Result<(), String> {
    if entry.node_id.trim().is_empty() {
        return Err("node_id is required".to_string());
    }
    if entry.workspaces.is_empty() {
        return Err("at least one workspace is required".to_string());
    }
    if entry.workspaces.iter().any(|w| w.trim().is_empty()) {
        return Err("workspace names must not be empty".to_string());
    }
    let url =
        reqwest::Url::parse(entry.base_url.trim()).map_err(|e| format!("invalid base_url: {e}"))?;
    match url.scheme() {
        "http" | "https" => {}
        other => return Err(format!("base_url must use http or https, got {other}")),
    }
    Ok(())
}

pub fn normalize_node(mut entry: FleetNodeEntry) -> FleetNodeEntry {
    entry.node_id = entry.node_id.trim().to_string();
    entry.base_url = entry.base_url.trim().trim_end_matches('/').to_string();
    entry.node_ip = entry.node_ip.and_then(|v| {
        let trimmed = v.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    });
    entry.node_name = entry.node_name.and_then(|v| {
        let trimmed = v.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    });
    entry.workspaces = entry
        .workspaces
        .into_iter()
        .map(|w| w.trim().to_string())
        .filter(|w| !w.is_empty())
        .collect();
    entry
}

async fn subscribe_workspace_loop(
    state: SharedRuntimeState,
    entry: FleetNodeEntry,
    workspace: String,
) {
    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            tracing::warn!(node_id = %entry.node_id, error = %err, "failed to build fleet client");
            return;
        }
    };
    let url = workspace_events_url(&entry.base_url, &workspace);

    loop {
        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                mark_connected(&state, &entry, &workspace);
                if let Err(err) = consume_sse_response(&state, &entry, resp).await {
                    tracing::warn!(node_id = %entry.node_id, workspace = %workspace, error = %err, "fleet SSE stream ended");
                    mark_down(&state, &entry, &workspace, err);
                } else {
                    mark_down(&state, &entry, &workspace, "SSE stream ended".to_string());
                }
            }
            Ok(resp) => {
                let error = format!("remote returned {}", resp.status());
                tracing::warn!(
                    node_id = %entry.node_id,
                    workspace = %workspace,
                    status = %resp.status(),
                    "fleet SSE request returned non-success status",
                );
                mark_down(&state, &entry, &workspace, error);
            }
            Err(err) => {
                tracing::warn!(node_id = %entry.node_id, workspace = %workspace, error = %err, "fleet SSE request failed");
                mark_down(&state, &entry, &workspace, err.to_string());
            }
        }
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

fn workspace_events_url(base_url: &str, workspace: &str) -> String {
    let encoded = utf8_percent_encode(workspace, NON_ALPHANUMERIC).to_string();
    format!("{base_url}/workspaces/{encoded}/agents/events")
}

async fn consume_sse_response(
    state: &SharedRuntimeState,
    entry: &FleetNodeEntry,
    resp: reqwest::Response,
) -> Result<(), String> {
    let mut parser = SseDataParser::default();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&chunk);
        for data in parser.push_str(&text) {
            match serde_json::from_str::<AgentActivityEvent>(&data) {
                Ok(event) => publish_event(state, entry, event),
                Err(err) => {
                    tracing::warn!(node_id = %entry.node_id, error = %err, "failed to parse remote fleet event");
                }
            }
        }
    }
    Ok(())
}

fn publish_event(state: &SharedRuntimeState, entry: &FleetNodeEntry, event: AgentActivityEvent) {
    mark_event(&event.workspace_id, state, entry);
    let envelope = FleetEventEnvelope::AgentActivity(FleetAgentActivityEvent {
        node_id: entry.node_id.clone(),
        node_ip: entry.node_ip.clone(),
        node_name: entry.node_name.clone(),
        workspace_id: event.workspace_id.clone(),
        agent_id: event.agent_id.clone(),
        received_at: chrono::Utc::now().to_rfc3339(),
        event,
    });
    let tx = {
        let s = state.lock().unwrap();
        s.fleet_tx.clone()
    };
    let _ = tx.send(envelope);
}

fn mark_connected(state: &SharedRuntimeState, entry: &FleetNodeEntry, workspace: &str) {
    update_status(state, entry, workspace, |status| {
        status.status = FleetNodeConnectionStatus::Connected;
        status.last_connected_at = Some(chrono::Utc::now().to_rfc3339());
        status.last_error = None;
    });
}

fn mark_event(workspace: &str, state: &SharedRuntimeState, entry: &FleetNodeEntry) {
    update_status(state, entry, workspace, |status| {
        status.status = FleetNodeConnectionStatus::Connected;
        status.last_event_at = Some(chrono::Utc::now().to_rfc3339());
        status.last_error = None;
    });
}

fn mark_down(state: &SharedRuntimeState, entry: &FleetNodeEntry, workspace: &str, error: String) {
    update_status(state, entry, workspace, |status| {
        status.status = FleetNodeConnectionStatus::Down;
        status.last_error = Some(error);
        status.retry_count = status.retry_count.saturating_add(1);
    });
}

fn update_status(
    state: &SharedRuntimeState,
    entry: &FleetNodeEntry,
    workspace: &str,
    update: impl FnOnce(&mut FleetNodeStatus),
) {
    let status = {
        let mut s = state.lock().unwrap();
        let key = status_key(&entry.node_id, workspace);
        let status = s
            .fleet_status
            .entry(key)
            .or_insert_with(|| FleetNodeStatus {
                node_id: entry.node_id.clone(),
                node_ip: entry.node_ip.clone(),
                node_name: entry.node_name.clone(),
                workspace_id: workspace.to_string(),
                status: FleetNodeConnectionStatus::Connecting,
                last_connected_at: None,
                last_event_at: None,
                last_error: None,
                retry_count: 0,
            });
        update(status);
        status.clone()
    };
    publish_status(state, status);
}

fn publish_status(state: &SharedRuntimeState, status: FleetNodeStatus) {
    let tx = {
        let s = state.lock().unwrap();
        s.fleet_tx.clone()
    };
    let _ = tx.send(FleetEventEnvelope::NodeStatus(status));
}

pub fn status_key(node_id: &str, workspace: &str) -> String {
    format!("{node_id}\u{0}{workspace}")
}

#[derive(Default)]
struct SseDataParser {
    pending: String,
    data: String,
}

impl SseDataParser {
    fn push_str(&mut self, chunk: &str) -> Vec<String> {
        self.pending.push_str(chunk);
        let mut out = Vec::new();

        while let Some(pos) = self.pending.find('\n') {
            let mut line = self.pending[..pos].to_string();
            self.pending.drain(..=pos);
            if line.ends_with('\r') {
                line.pop();
            }

            if line.is_empty() {
                if !self.data.is_empty() {
                    let data = self.data.trim_end_matches('\n').to_string();
                    self.data.clear();
                    out.push(data);
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("data:") {
                self.data.push_str(rest.trim_start());
                self.data.push('\n');
            }
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_extracts_data_frames() {
        let mut parser = SseDataParser::default();
        let out = parser.push_str("data: {\"a\":1}\n\ndata: {\"b\":2}\n\n");
        assert_eq!(out, vec!["{\"a\":1}", "{\"b\":2}"]);
    }

    #[test]
    fn parser_handles_split_lines() {
        let mut parser = SseDataParser::default();
        assert!(parser.push_str("data: {\"a\"").is_empty());
        let out = parser.push_str(":1}\n\n");
        assert_eq!(out, vec!["{\"a\":1}"]);
    }
}
