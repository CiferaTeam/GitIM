//! `fleet` subcommands — manage optional remote runtime subscriptions.

use crate::cli::http::{CliError, Client};

#[derive(Debug, Clone)]
pub struct AddArgs {
    pub node_id: String,
    pub base_url: String,
    pub node_ip: Option<String>,
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
