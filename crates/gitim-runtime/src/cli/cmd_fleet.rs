//! `fleet` subcommands — manage optional remote runtime subscriptions.

use crate::cli::http::{CliError, Client};
use crate::cli::tunnel::{self, LaunchConfig, TunnelStatusKind};

#[derive(Debug, Clone)]
pub struct AddArgs {
    pub node_id: String,
    pub base_url: String,
    pub node_ip: Option<String>,
    pub node_name: Option<String>,
    pub workspaces: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TunnelUpArgs {
    pub node_id: String,
    pub ssh_target: String,
    pub remote_host: String,
    pub remote_port: u16,
    pub local_port: Option<u16>,
    pub node_name: Option<String>,
    pub workspaces: Vec<String>,
}

pub async fn list(client: &Client) -> Result<i32, CliError> {
    let body = client.get("/fleet/nodes").await?;
    let nodes = extract_nodes_array(&body)?;
    let out = serde_json::to_string(&nodes)
        .map_err(|e| CliError::Parse(format!("serialize fleet nodes array: {e}")))?;
    println!("{out}");
    Ok(0)
}

pub async fn status(client: &Client) -> Result<i32, CliError> {
    let body = client.get("/fleet/status").await?;
    let nodes = extract_status_array(&body)?;
    let out = serde_json::to_string(&nodes)
        .map_err(|e| CliError::Parse(format!("serialize fleet status array: {e}")))?;
    println!("{out}");
    Ok(0)
}

pub async fn add(client: &Client, args: AddArgs) -> Result<i32, CliError> {
    let body = build_add_body(args)?;
    let res = client.post("/fleet/nodes", &body).await?;
    let out = serde_json::to_string_pretty(&res)
        .map_err(|e| CliError::Parse(format!("serialize fleet add response: {e}")))?;
    println!("{out}");
    Ok(0)
}

pub async fn remove(client: &Client, node_id: String) -> Result<i32, CliError> {
    if node_id.trim().is_empty() {
        return Err(CliError::InvalidConfig("--node-id is required".to_string()));
    }
    let path = format!(
        "/fleet/nodes/{}",
        percent_encoding::utf8_percent_encode(node_id.trim(), percent_encoding::NON_ALPHANUMERIC)
    );
    let res = client.delete(&path).await?;
    let out = serde_json::to_string_pretty(&res)
        .map_err(|e| CliError::Parse(format!("serialize fleet remove response: {e}")))?;
    println!("{out}");
    Ok(0)
}

pub async fn tunnel_up(client: &Client, mut args: TunnelUpArgs) -> Result<i32, CliError> {
    if args.local_port.is_none() {
        args.local_port = configured_tunnel_local_port(&args.node_id)?;
    }
    let local_port = tunnel::resolve_local_port(args.local_port)?;
    args.local_port = Some(local_port);
    let launch = LaunchConfig {
        node_id: args.node_id.clone(),
        ssh_target: args.ssh_target.clone(),
        remote_host: args.remote_host.clone(),
        remote_port: args.remote_port,
        local_port,
    };
    let state = tunnel::ensure_running(&launch).await?;
    let body = build_tunnel_node_body(args)?;
    let node = match client.post("/fleet/nodes", &body).await {
        Ok(node) => node,
        Err(err) => {
            let _ = tunnel::stop(&state.node_id).await;
            return Err(err);
        }
    };
    let out = serde_json::json!({
        "ok": true,
        "node_id": state.node_id,
        "pid": state.pid,
        "base_url": state.base_url,
        "local_port": state.local_port,
        "remote_host": state.remote_host,
        "remote_port": state.remote_port,
        "tunnel_status": "up",
        "runtime_status": "healthy",
        "node": node.get("node").cloned().unwrap_or(node),
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&out)
            .map_err(|e| CliError::Parse(format!("serialize tunnel up response: {e}")))?
    );
    Ok(0)
}

fn configured_tunnel_local_port(node_id: &str) -> Result<Option<u16>, CliError> {
    if let Some(state) = tunnel::read_state(node_id)? {
        return Ok(Some(state.local_port));
    }
    Ok(tunnel::find_node(node_id)
        .ok()
        .and_then(|entry| entry.ssh_tunnel.and_then(|tunnel| tunnel.local_port)))
}

pub async fn tunnel_status(node_id: String) -> Result<i32, CliError> {
    if node_id.trim().is_empty() {
        return Err(CliError::InvalidConfig("--node-id is required".to_string()));
    }
    let status = tunnel::status(node_id.trim()).await?;
    let out = tunnel_status_json(node_id.trim(), status);
    println!(
        "{}",
        serde_json::to_string_pretty(&out)
            .map_err(|e| CliError::Parse(format!("serialize tunnel status response: {e}")))?
    );
    Ok(0)
}

pub async fn tunnel_down(node_id: String) -> Result<i32, CliError> {
    if node_id.trim().is_empty() {
        return Err(CliError::InvalidConfig("--node-id is required".to_string()));
    }
    let stopped = tunnel::stop(node_id.trim()).await?;
    let out = serde_json::json!({
        "ok": true,
        "node_id": node_id.trim(),
        "tunnel_status": "down",
        "stopped_pid": stopped.as_ref().map(|state| state.pid),
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&out)
            .map_err(|e| CliError::Parse(format!("serialize tunnel down response: {e}")))?
    );
    Ok(0)
}

pub fn build_add_body(args: AddArgs) -> Result<serde_json::Value, CliError> {
    if args.node_id.trim().is_empty() {
        return Err(CliError::InvalidConfig("--node-id is required".to_string()));
    }
    if args.base_url.trim().is_empty() {
        return Err(CliError::InvalidConfig(
            "--base-url is required".to_string(),
        ));
    }
    let mut body = serde_json::json!({
        "node_id": args.node_id,
        "base_url": args.base_url,
        "workspaces": args.workspaces,
    });
    if let Some(ip) = args.node_ip {
        body["node_ip"] = serde_json::Value::String(ip);
    }
    if let Some(name) = args.node_name {
        body["node_name"] = serde_json::Value::String(name);
    }
    Ok(body)
}

pub fn build_tunnel_node_body(args: TunnelUpArgs) -> Result<serde_json::Value, CliError> {
    if args.node_id.trim().is_empty() {
        return Err(CliError::InvalidConfig("--node-id is required".to_string()));
    }
    if args.ssh_target.trim().is_empty() {
        return Err(CliError::InvalidConfig(
            "--ssh-target is required".to_string(),
        ));
    }
    if args.remote_host.trim().is_empty() {
        return Err(CliError::InvalidConfig(
            "--remote-host is required".to_string(),
        ));
    }
    if args.remote_port == 0 {
        return Err(CliError::InvalidConfig(
            "--remote-port must be non-zero".to_string(),
        ));
    }
    let local_port = args.local_port.ok_or_else(|| {
        CliError::InvalidConfig("local port must be resolved before registration".to_string())
    })?;
    if local_port == 0 {
        return Err(CliError::InvalidConfig(
            "--local-port must be non-zero".to_string(),
        ));
    }

    let mut body = serde_json::json!({
        "node_id": args.node_id,
        "base_url": tunnel::base_url_for_port(local_port),
        "workspaces": args.workspaces,
        "ssh_tunnel": {
            "ssh_target": args.ssh_target,
            "remote_host": args.remote_host,
            "remote_port": args.remote_port,
            "local_port": local_port,
        },
    });
    if let Some(name) = args.node_name {
        body["node_name"] = serde_json::Value::String(name);
    }
    Ok(body)
}

fn tunnel_status_json(
    node_id: &str,
    status: crate::cli::tunnel::TunnelStatus,
) -> serde_json::Value {
    let tunnel_status = match status.tunnel_status {
        TunnelStatusKind::Up => "up",
        TunnelStatusKind::Down => "down",
        TunnelStatusKind::Stale => "stale",
    };
    let runtime_status = if status.runtime_healthy {
        "healthy"
    } else if status.state.is_some() {
        "unreachable"
    } else {
        "unknown"
    };
    let mut out = serde_json::json!({
        "ok": true,
        "node_id": node_id,
        "tunnel_status": tunnel_status,
        "runtime_status": runtime_status,
    });
    if let Some(state) = status.state {
        out["pid"] = serde_json::json!(state.pid);
        out["base_url"] = serde_json::json!(state.base_url);
        out["local_port"] = serde_json::json!(state.local_port);
        out["remote_host"] = serde_json::json!(state.remote_host);
        out["remote_port"] = serde_json::json!(state.remote_port);
    }
    if let Some(error) = status.runtime_error {
        out["runtime_error"] = serde_json::json!(error);
    }
    out
}

pub fn extract_nodes_array(body: &serde_json::Value) -> Result<serde_json::Value, CliError> {
    let nodes = body
        .get("nodes")
        .ok_or_else(|| CliError::Parse("/fleet/nodes missing 'nodes' key".to_string()))?;
    if !nodes.is_array() {
        return Err(CliError::Parse(
            "/fleet/nodes 'nodes' field is not an array".to_string(),
        ));
    }
    Ok(nodes.clone())
}

pub fn extract_status_array(body: &serde_json::Value) -> Result<serde_json::Value, CliError> {
    let nodes = body
        .get("nodes")
        .ok_or_else(|| CliError::Parse("/fleet/status missing 'nodes' key".to_string()))?;
    if !nodes.is_array() {
        return Err(CliError::Parse(
            "/fleet/status 'nodes' field is not an array".to_string(),
        ));
    }
    Ok(nodes.clone())
}
