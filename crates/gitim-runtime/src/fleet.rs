use std::time::Duration;

use futures::StreamExt;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::{Deserialize, Serialize};
use tokio::task::AbortHandle;

use crate::http::{AgentActivityEvent, AgentInfo, SharedRuntimeState};
use crate::preconditions;
use crate::user_config::{FleetNodeEntry, FleetWorkspaceMapping};

const RECONNECT_DELAY: Duration = Duration::from_secs(2);

/// How often the tunnel watcher probes a healthy tunnel.
const TUNNEL_POLL_INTERVAL: Duration = Duration::from_secs(10);
/// Initial retry delay after a tunnel watcher failure.
const TUNNEL_BACKOFF_INITIAL: Duration = Duration::from_secs(5);
/// Upper bound on the fleet tunnel watcher's retry backoff.
const TUNNEL_BACKOFF_MAX: Duration = Duration::from_secs(120);

/// Double the current backoff, capped at [`TUNNEL_BACKOFF_MAX`].
fn next_backoff(current: Duration) -> Duration {
    (current * 2).min(TUNNEL_BACKOFF_MAX)
}

/// Build a tunnel [`LaunchConfig`](crate::cli::tunnel::LaunchConfig) for a node's
/// watcher, or `None` when the node has no `ssh_tunnel` or no fixed `local_port`
/// to maintain (an auto-selected ephemeral port can't be re-bound across restarts).
fn tunnel_launch_config(entry: &FleetNodeEntry) -> Option<crate::cli::tunnel::LaunchConfig> {
    let tunnel = entry.ssh_tunnel.as_ref()?;
    let local_port = tunnel.local_port?;
    Some(crate::cli::tunnel::LaunchConfig {
        node_id: entry.node_id.clone(),
        ssh_target: tunnel.ssh_target.clone(),
        remote_host: tunnel.remote_host.clone(),
        remote_port: tunnel.remote_port,
        local_port,
    })
}

/// Keep a node's SSH tunnel alive for the lifetime of its [`FleetNodeRuntime`].
///
/// Periodically calls the idempotent [`ensure_running`](crate::cli::tunnel::ensure_running)
/// — healthy tunnel is a no-op, dead/unhealthy one is rebuilt. Polls every
/// [`TUNNEL_POLL_INTERVAL`] when healthy; exponential backoff on failure. No auth
/// circuit-breaker: ssh retries cost no remote rate limit, and unbounded retry is
/// exactly the keep-alive we want (remote can come back hours later).
async fn tunnel_watcher_loop(launch: crate::cli::tunnel::LaunchConfig) {
    let mut backoff = TUNNEL_BACKOFF_INITIAL;
    loop {
        match crate::cli::tunnel::ensure_running(&launch).await {
            Ok(_) => {
                backoff = TUNNEL_BACKOFF_INITIAL;
                tokio::time::sleep(TUNNEL_POLL_INTERVAL).await;
            }
            Err(err) => {
                tracing::warn!(
                    node_id = %launch.node_id,
                    error = %err,
                    "fleet tunnel watcher: ensure_running failed, retrying"
                );
                tokio::time::sleep(backoff).await;
                backoff = next_backoff(backoff);
            }
        }
    }
}

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_workspace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_identity: Option<String>,
    pub workspace_id: String,
    pub agent_id: String,
    pub received_at: String,
    pub event: AgentActivityEvent,
}

#[derive(Clone, Debug, Serialize)]
pub struct FleetAgentSnapshot {
    pub node_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_workspace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_identity: Option<String>,
    pub workspace_id: String,
    pub agent: AgentInfo,
}

#[derive(Clone, Debug, Serialize)]
pub struct FleetNodeStatus {
    pub node_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_workspace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_identity: Option<String>,
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
        for subscription in workspace_subscriptions(&entry) {
            let state = state.clone();
            let entry = entry.clone();
            let handle = tokio::spawn(async move {
                subscribe_workspace_loop(state, entry, subscription).await;
            });
            handles.push(handle.abort_handle());
        }
        // Per-node tunnel watcher: keep the SSH tunnel the observers depend on
        // alive across restarts and transient drops. One per node, outside the
        // subscription loop. Shares the AbortHandle lifecycle (Drop aborts it).
        if let Some(launch) = tunnel_launch_config(&entry) {
            let handle = tokio::spawn(tunnel_watcher_loop(launch));
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
        // mutex_lock documents the poisoned-guard invariant
        let mut s = preconditions::mutex_lock(&state);
        s.fleet_nodes.insert(entry.node_id.clone(), runtime);
        for subscription in workspace_subscriptions(&entry) {
            let status = FleetNodeStatus {
                node_id: entry.node_id.clone(),
                node_ip: entry.node_ip.clone(),
                node_name: entry.node_name.clone(),
                remote_workspace_id: subscription.remote_workspace_id(),
                workspace_identity: subscription.workspace_identity.clone(),
                workspace_id: subscription.local_workspace_id.clone(),
                status: FleetNodeConnectionStatus::Connecting,
                last_connected_at: None,
                last_event_at: None,
                last_error: None,
                retry_count: 0,
            };
            s.fleet_status.insert(
                status_key(
                    &entry.node_id,
                    &subscription.local_workspace_id,
                    &subscription.remote_workspace_id,
                ),
                status.clone(),
            );
            initial_statuses.push(status);
        }
    }
    for status in initial_statuses {
        publish_status(&state, status);
    }
}

pub fn remove_node(state: &SharedRuntimeState, node_id: &str) -> bool {
    let mut s = preconditions::mutex_lock(state);
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
    if entry.workspaces.iter().any(|w| w.trim().is_empty()) {
        return Err("workspace names must not be empty".to_string());
    }
    for mapping in &entry.workspace_mappings {
        if mapping.remote_workspace_id.trim().is_empty()
            || mapping.local_workspace_id.trim().is_empty()
            || mapping.workspace_identity.trim().is_empty()
        {
            return Err("workspace mappings must be complete".to_string());
        }
    }
    if let Some(tunnel) = &entry.ssh_tunnel {
        if tunnel.ssh_target.trim().is_empty() {
            return Err("ssh_tunnel.ssh_target is required".to_string());
        }
        if tunnel.remote_host.trim().is_empty() {
            return Err("ssh_tunnel.remote_host is required".to_string());
        }
        if tunnel.remote_port == 0 {
            return Err("ssh_tunnel.remote_port must be non-zero".to_string());
        }
        if tunnel.local_port == Some(0) {
            return Err("ssh_tunnel.local_port must be non-zero".to_string());
        }
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
    entry.workspace_mappings = entry
        .workspace_mappings
        .into_iter()
        .filter_map(|mapping| {
            let remote_workspace_id = mapping.remote_workspace_id.trim().to_string();
            let local_workspace_id = mapping.local_workspace_id.trim().to_string();
            let workspace_identity = mapping.workspace_identity.trim().to_string();
            (!remote_workspace_id.is_empty()
                && !local_workspace_id.is_empty()
                && !workspace_identity.is_empty())
            .then_some(FleetWorkspaceMapping {
                remote_workspace_id,
                local_workspace_id,
                workspace_identity,
            })
        })
        .collect();
    entry.ssh_tunnel = entry.ssh_tunnel.map(|tunnel| {
        let ssh_target = tunnel.ssh_target.trim().to_string();
        let remote_host = tunnel.remote_host.trim().to_string();
        crate::user_config::FleetSshTunnelConfig {
            ssh_target,
            remote_host,
            remote_port: tunnel.remote_port,
            local_port: tunnel.local_port,
        }
    });
    entry
}

pub async fn resolve_workspace_mappings(
    state: &SharedRuntimeState,
    mut entry: FleetNodeEntry,
) -> Result<FleetNodeEntry, String> {
    if !entry.workspace_mappings.is_empty() {
        if entry.workspaces.is_empty() {
            entry.workspaces = entry
                .workspace_mappings
                .iter()
                .map(|mapping| mapping.remote_workspace_id.clone())
                .collect();
        }
        return Ok(entry);
    }

    let local_workspaces = local_remote_identities(state);
    if local_workspaces.is_empty() {
        return Err("no local github workspaces with remote identity are available".to_string());
    }

    let remote = fetch_remote_workspaces(&entry.base_url).await?;
    let requested: std::collections::HashSet<_> = entry.workspaces.iter().cloned().collect();
    let mut mappings = Vec::new();
    for remote_workspace in remote.workspaces {
        if !requested.is_empty() && !requested.contains(&remote_workspace.slug) {
            continue;
        }
        let Some(identity) = remote_workspace.remote_identity else {
            continue;
        };
        if let Some((local_slug, _)) = local_workspaces
            .iter()
            .find(|(_, local_identity)| *local_identity == identity)
        {
            mappings.push(FleetWorkspaceMapping {
                remote_workspace_id: remote_workspace.slug,
                local_workspace_id: local_slug.clone(),
                workspace_identity: identity,
            });
        }
    }
    mappings.sort_by(|a, b| {
        a.local_workspace_id
            .cmp(&b.local_workspace_id)
            .then_with(|| a.remote_workspace_id.cmp(&b.remote_workspace_id))
    });
    mappings.dedup_by(|a, b| {
        a.local_workspace_id == b.local_workspace_id
            && a.remote_workspace_id == b.remote_workspace_id
            && a.workspace_identity == b.workspace_identity
    });
    if mappings.is_empty() {
        return Err(
            "no remote workspace has a git remote identity matching a local workspace".to_string(),
        );
    }
    entry.workspaces = mappings
        .iter()
        .map(|mapping| mapping.remote_workspace_id.clone())
        .collect();
    entry.workspace_mappings = mappings;
    Ok(entry)
}

pub async fn fetch_agent_snapshots(state: &SharedRuntimeState) -> Vec<FleetAgentSnapshot> {
    let nodes: Vec<_> = {
        let s = preconditions::mutex_lock(state);
        s.fleet_nodes
            .values()
            .map(|runtime| runtime.entry.clone())
            .collect()
    };
    if nodes.is_empty() {
        return Vec::new();
    }

    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(8))
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            tracing::warn!(error = %err, "failed to build fleet agents client");
            return Vec::new();
        }
    };

    let mut snapshots = Vec::new();
    for entry in nodes {
        for subscription in workspace_subscriptions(&entry) {
            match fetch_remote_agents(&client, &entry.base_url, &subscription.remote_workspace_id)
                .await
            {
                Ok(agents) => {
                    snapshots.extend(agents.into_iter().map(|agent| FleetAgentSnapshot {
                        node_id: entry.node_id.clone(),
                        node_ip: entry.node_ip.clone(),
                        node_name: entry.node_name.clone(),
                        remote_workspace_id: subscription.remote_workspace_id(),
                        workspace_identity: subscription.workspace_identity.clone(),
                        workspace_id: subscription.local_workspace_id.clone(),
                        agent,
                    }));
                }
                Err(err) => {
                    tracing::warn!(
                        node_id = %entry.node_id,
                        workspace = %subscription.remote_workspace_id,
                        error = %err,
                        "failed to fetch remote fleet agents",
                    );
                }
            }
        }
    }

    snapshots.sort_by(|a, b| {
        a.node_id
            .cmp(&b.node_id)
            .then_with(|| a.workspace_id.cmp(&b.workspace_id))
            .then_with(|| a.agent.id.cmp(&b.agent.id))
    });
    snapshots
}

fn local_remote_identities(state: &SharedRuntimeState) -> Vec<(String, String)> {
    let mut identities: Vec<_> = {
        let s = preconditions::mutex_lock(state);
        s.workspaces
            .values()
            .filter_map(|ctx| {
                let identity = ctx.git_config.as_ref()?.git.remote_identity()?;
                Some((ctx.slug.clone(), identity))
            })
            .collect()
    };
    identities.sort_by(|a, b| a.0.cmp(&b.0));
    identities
}

#[derive(Debug, Deserialize)]
struct RemoteWorkspacesResponse {
    #[serde(default)]
    workspaces: Vec<RemoteWorkspaceSummary>,
}

#[derive(Debug, Deserialize)]
struct RemoteWorkspaceSummary {
    slug: String,
    #[serde(default)]
    remote_identity: Option<String>,
}

async fn fetch_remote_workspaces(base_url: &str) -> Result<RemoteWorkspacesResponse, String> {
    let url = format!("{}/workspaces", base_url.trim_end_matches('/'));
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| format!("failed to build fleet client: {e}"))?
        .get(url)
        .send()
        .await
        .map_err(|e| format!("failed to fetch remote workspaces: {e}"))?
        .error_for_status()
        .map_err(|e| format!("remote workspaces returned error: {e}"))?
        .json::<RemoteWorkspacesResponse>()
        .await
        .map_err(|e| format!("failed to parse remote workspaces: {e}"))
}

#[derive(Debug, Deserialize)]
struct RemoteAgentsResponse {
    #[serde(default)]
    agents: Vec<AgentInfo>,
}

async fn fetch_remote_agents(
    client: &reqwest::Client,
    base_url: &str,
    workspace: &str,
) -> Result<Vec<AgentInfo>, String> {
    let url = format!(
        "{}/workspaces/{}/agents",
        base_url.trim_end_matches('/'),
        utf8_percent_encode(workspace, NON_ALPHANUMERIC)
    );
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("failed to fetch remote agents: {e}"))?
        .error_for_status()
        .map_err(|e| format!("remote agents returned error: {e}"))?
        .json::<RemoteAgentsResponse>()
        .await
        .map_err(|e| format!("failed to parse remote agents: {e}"))?;
    Ok(response.agents)
}

async fn subscribe_workspace_loop(
    state: SharedRuntimeState,
    entry: FleetNodeEntry,
    subscription: FleetWorkspaceSubscription,
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
    let url = workspace_events_url(&entry.base_url, &subscription.remote_workspace_id);

    loop {
        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                mark_connected(&state, &entry, &subscription);
                if let Err(err) = consume_sse_response(&state, &entry, &subscription, resp).await {
                    tracing::warn!(node_id = %entry.node_id, workspace = %subscription.remote_workspace_id, error = %err, "fleet SSE stream ended");
                    mark_down(&state, &entry, &subscription, err);
                } else {
                    mark_down(
                        &state,
                        &entry,
                        &subscription,
                        "SSE stream ended".to_string(),
                    );
                }
            }
            Ok(resp) => {
                let error = format!("remote returned {}", resp.status());
                tracing::warn!(
                    node_id = %entry.node_id,
                    workspace = %subscription.remote_workspace_id,
                    status = %resp.status(),
                    "fleet SSE request returned non-success status",
                );
                mark_down(&state, &entry, &subscription, error);
            }
            Err(err) => {
                tracing::warn!(node_id = %entry.node_id, workspace = %subscription.remote_workspace_id, error = %err, "fleet SSE request failed");
                mark_down(&state, &entry, &subscription, err.to_string());
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
    subscription: &FleetWorkspaceSubscription,
    resp: reqwest::Response,
) -> Result<(), String> {
    let mut parser = SseDataParser::default();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&chunk);
        for data in parser.push_str(&text) {
            match serde_json::from_str::<AgentActivityEvent>(&data) {
                Ok(event) => publish_event(state, entry, subscription, event),
                Err(err) => {
                    tracing::warn!(node_id = %entry.node_id, error = %err, "failed to parse remote fleet event");
                }
            }
        }
    }
    Ok(())
}

fn publish_event(
    state: &SharedRuntimeState,
    entry: &FleetNodeEntry,
    subscription: &FleetWorkspaceSubscription,
    mut event: AgentActivityEvent,
) {
    let remote_workspace_id = event.workspace_id.clone();
    mark_event(state, entry, subscription);
    event.workspace_id = subscription.local_workspace_id.clone();
    let envelope = FleetEventEnvelope::AgentActivity(FleetAgentActivityEvent {
        node_id: entry.node_id.clone(),
        node_ip: entry.node_ip.clone(),
        node_name: entry.node_name.clone(),
        remote_workspace_id: Some(remote_workspace_id),
        workspace_identity: subscription.workspace_identity.clone(),
        workspace_id: event.workspace_id.clone(),
        agent_id: event.agent_id.clone(),
        received_at: chrono::Utc::now().to_rfc3339(),
        event,
    });
    let tx = {
        let s = preconditions::mutex_lock(state);
        s.fleet_tx.clone()
    };
    let _ = tx.send(envelope);
}

fn mark_connected(
    state: &SharedRuntimeState,
    entry: &FleetNodeEntry,
    subscription: &FleetWorkspaceSubscription,
) {
    update_status(state, entry, subscription, |status| {
        status.status = FleetNodeConnectionStatus::Connected;
        status.last_connected_at = Some(chrono::Utc::now().to_rfc3339());
        status.last_error = None;
    });
}

fn mark_event(
    state: &SharedRuntimeState,
    entry: &FleetNodeEntry,
    subscription: &FleetWorkspaceSubscription,
) {
    update_status(state, entry, subscription, |status| {
        status.status = FleetNodeConnectionStatus::Connected;
        status.last_event_at = Some(chrono::Utc::now().to_rfc3339());
        status.last_error = None;
    });
}

fn mark_down(
    state: &SharedRuntimeState,
    entry: &FleetNodeEntry,
    subscription: &FleetWorkspaceSubscription,
    error: String,
) {
    update_status(state, entry, subscription, |status| {
        status.status = FleetNodeConnectionStatus::Down;
        status.last_error = Some(error);
        status.retry_count = status.retry_count.saturating_add(1);
    });
}

fn update_status(
    state: &SharedRuntimeState,
    entry: &FleetNodeEntry,
    subscription: &FleetWorkspaceSubscription,
    update: impl FnOnce(&mut FleetNodeStatus),
) {
    let status = {
        let mut s = preconditions::mutex_lock(state);
        let key = status_key(
            &entry.node_id,
            &subscription.local_workspace_id,
            &subscription.remote_workspace_id,
        );
        let status = s
            .fleet_status
            .entry(key)
            .or_insert_with(|| FleetNodeStatus {
                node_id: entry.node_id.clone(),
                node_ip: entry.node_ip.clone(),
                node_name: entry.node_name.clone(),
                remote_workspace_id: subscription.remote_workspace_id(),
                workspace_identity: subscription.workspace_identity.clone(),
                workspace_id: subscription.local_workspace_id.clone(),
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
        let s = preconditions::mutex_lock(state);
        s.fleet_tx.clone()
    };
    let _ = tx.send(FleetEventEnvelope::NodeStatus(status));
}

pub fn status_key(node_id: &str, local_workspace: &str, remote_workspace: &str) -> String {
    format!("{node_id}\u{0}{local_workspace}\u{0}{remote_workspace}")
}

#[derive(Clone, Debug)]
struct FleetWorkspaceSubscription {
    remote_workspace_id: String,
    local_workspace_id: String,
    workspace_identity: Option<String>,
}

impl FleetWorkspaceSubscription {
    fn remote_workspace_id(&self) -> Option<String> {
        (self.remote_workspace_id != self.local_workspace_id)
            .then(|| self.remote_workspace_id.clone())
    }
}

fn workspace_subscriptions(entry: &FleetNodeEntry) -> Vec<FleetWorkspaceSubscription> {
    if !entry.workspace_mappings.is_empty() {
        return entry
            .workspace_mappings
            .iter()
            .map(|mapping| FleetWorkspaceSubscription {
                remote_workspace_id: mapping.remote_workspace_id.clone(),
                local_workspace_id: mapping.local_workspace_id.clone(),
                workspace_identity: Some(mapping.workspace_identity.clone()),
            })
            .collect();
    }
    entry
        .workspaces
        .iter()
        .map(|workspace| FleetWorkspaceSubscription {
            remote_workspace_id: workspace.clone(),
            local_workspace_id: workspace.clone(),
            workspace_identity: None,
        })
        .collect()
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

    #[test]
    fn validate_node_rejects_incomplete_ssh_tunnel() {
        let entry = FleetNodeEntry {
            node_id: "node-a".to_string(),
            base_url: "http://127.0.0.1:18068".to_string(),
            node_ip: None,
            node_name: None,
            workspaces: vec!["room".to_string()],
            workspace_mappings: Vec::new(),
            ssh_tunnel: Some(crate::user_config::FleetSshTunnelConfig {
                ssh_target: " ".to_string(),
                remote_host: "127.0.0.1".to_string(),
                remote_port: 16868,
                local_port: Some(18068),
            }),
        };

        assert_eq!(
            validate_node(&normalize_node(entry)).unwrap_err(),
            "ssh_tunnel.ssh_target is required"
        );
    }

    #[test]
    fn next_backoff_doubles_and_caps() {
        assert_eq!(
            next_backoff(Duration::from_secs(5)),
            Duration::from_secs(10)
        );
        assert_eq!(
            next_backoff(Duration::from_secs(10)),
            Duration::from_secs(20)
        );
        assert_eq!(
            next_backoff(Duration::from_secs(80)),
            Duration::from_secs(120)
        );
        assert_eq!(
            next_backoff(Duration::from_secs(120)),
            Duration::from_secs(120)
        );
    }

    fn entry_with_tunnel(local_port: Option<u16>) -> FleetNodeEntry {
        FleetNodeEntry {
            node_id: "mac-mini".to_string(),
            base_url: "http://127.0.0.1:18068".to_string(),
            node_ip: None,
            node_name: None,
            workspaces: vec!["room".to_string()],
            workspace_mappings: Vec::new(),
            ssh_tunnel: Some(crate::user_config::FleetSshTunnelConfig {
                ssh_target: "lewis@host".to_string(),
                remote_host: "127.0.0.1".to_string(),
                remote_port: 16868,
                local_port,
            }),
        }
    }

    #[test]
    fn tunnel_launch_config_maps_fields_when_complete() {
        let launch = tunnel_launch_config(&entry_with_tunnel(Some(18068)));
        assert_eq!(
            launch.as_ref().map(|l| l.node_id.as_str()),
            Some("mac-mini")
        );
        assert_eq!(
            launch.as_ref().map(|l| l.ssh_target.as_str()),
            Some("lewis@host")
        );
        assert_eq!(
            launch.as_ref().map(|l| l.remote_host.as_str()),
            Some("127.0.0.1")
        );
        assert_eq!(launch.as_ref().map(|l| l.remote_port), Some(16868));
        assert_eq!(launch.as_ref().map(|l| l.local_port), Some(18068));
    }

    #[test]
    fn tunnel_launch_config_none_without_tunnel_or_port() {
        let mut no_tunnel = entry_with_tunnel(Some(18068));
        no_tunnel.ssh_tunnel = None;
        assert!(tunnel_launch_config(&no_tunnel).is_none());

        // ssh_tunnel present but no fixed local_port → not watchable
        assert!(tunnel_launch_config(&entry_with_tunnel(None)).is_none());
    }
}
