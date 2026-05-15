use gitim_runtime::cli::cmd_fleet::{build_add_body, extract_nodes_array, AddArgs};
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
fn build_add_body_rejects_missing_workspace() {
    let err = build_add_body(AddArgs {
        node_id: "remote-runtime-a".to_string(),
        base_url: "http://100.64.0.10:16868".to_string(),
        node_ip: None,
        node_name: None,
        workspaces: Vec::new(),
    })
    .expect_err("must reject empty workspace list");

    assert!(err.to_string().contains("--workspace"));
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
