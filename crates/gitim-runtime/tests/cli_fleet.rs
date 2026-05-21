#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use gitim_runtime::cli::cmd_fleet::{
    build_add_body, build_tunnel_node_body, extract_nodes_array, extract_status_array, AddArgs,
    TunnelUpArgs,
};
use serde_json::json;

#[test]
fn build_add_body_includes_node_identity_and_workspaces() {
    let body = build_add_body(AddArgs {
        node_id: "remote-runtime-a".to_string(),
        base_url: "http://100.64.0.10:16868".to_string(),
        node_ip: Some("100.64.0.10".to_string()),
        node_name: Some("mac-mini".to_string()),
        workspaces: vec!["room".to_string(), "lab".to_string()],
    })
    .expect("body");

    assert_eq!(body["node_id"], "remote-runtime-a");
    assert_eq!(body["base_url"], "http://100.64.0.10:16868");
    assert_eq!(body["node_ip"], "100.64.0.10");
    assert_eq!(body["node_name"], "mac-mini");
    assert_eq!(body["workspaces"], json!(["room", "lab"]));
}

#[test]
fn build_add_body_allows_missing_workspace_for_auto_mapping() {
    let body = build_add_body(AddArgs {
        node_id: "remote-runtime-a".to_string(),
        base_url: "http://100.64.0.10:16868".to_string(),
        node_ip: None,
        node_name: None,
        workspaces: Vec::new(),
    })
    .expect("body");

    assert_eq!(body["node_id"], "remote-runtime-a");
    assert!(body["workspaces"].as_array().unwrap().is_empty());
}

#[test]
fn build_tunnel_node_body_targets_loopback_base_url_and_persists_ssh_config() {
    let body = build_tunnel_node_body(TunnelUpArgs {
        node_id: "mac-mini".to_string(),
        ssh_target: "lewis@mac-mini".to_string(),
        remote_host: "127.0.0.1".to_string(),
        remote_port: 16868,
        local_port: Some(18068),
        node_name: Some("Mac Mini".to_string()),
        workspaces: vec!["room".to_string()],
    })
    .expect("body");

    assert_eq!(body["node_id"], "mac-mini");
    assert_eq!(body["base_url"], "http://127.0.0.1:18068");
    assert_eq!(body["node_name"], "Mac Mini");
    assert_eq!(body["workspaces"], json!(["room"]));
    assert_eq!(body["ssh_tunnel"]["ssh_target"], "lewis@mac-mini");
    assert_eq!(body["ssh_tunnel"]["remote_host"], "127.0.0.1");
    assert_eq!(body["ssh_tunnel"]["remote_port"], 16868);
    assert_eq!(body["ssh_tunnel"]["local_port"], 18068);
}

#[test]
fn extract_nodes_array_unwraps_runtime_response() {
    let body = json!({
        "ok": true,
        "nodes": [
            {"node_id": "remote-a", "base_url": "http://a", "workspaces": ["room"]},
        ],
    });

    let nodes = extract_nodes_array(&body).expect("nodes");
    assert_eq!(nodes.as_array().unwrap().len(), 1);
    assert_eq!(nodes[0]["node_id"], "remote-a");
}

#[test]
fn extract_status_array_unwraps_runtime_response() {
    let body = json!({
        "ok": true,
        "nodes": [
            {
                "node_id": "remote-a",
                "workspace_id": "room",
                "status": "down",
                "retry_count": 3,
            },
        ],
    });

    let nodes = extract_status_array(&body).expect("status nodes");
    assert_eq!(nodes.as_array().unwrap().len(), 1);
    assert_eq!(nodes[0]["status"], "down");
    assert_eq!(nodes[0]["retry_count"], 3);
}
