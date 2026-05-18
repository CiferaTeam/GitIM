use std::process;

use gitim_client::GitimClient;
use gitim_core::types::board_path;

use super::{get_repo_root, read_my_handler};
use crate::output::OutputMode;

fn print_json(value: serde_json::Value) {
    match serde_json::to_string(&value) {
        Ok(s) => println!("{s}"),
        Err(e) => {
            eprintln!("Error: failed to format output: {e}");
            process::exit(1);
        }
    }
}

fn print_or_exit(
    resp: gitim_client::ApiResponse,
    mode: &OutputMode,
    human_success: impl FnOnce(&serde_json::Value),
) {
    if !resp.ok {
        eprintln!("Error: {}", resp.error.as_deref().unwrap_or("unknown"));
        process::exit(1);
    }

    let data = resp.data.unwrap_or(serde_json::Value::Null);
    match mode {
        OutputMode::Human => human_success(&data),
        OutputMode::Json => print_json(data),
    }
}

fn print_board_write(data: &serde_json::Value) {
    let path = data["path"].as_str().unwrap_or("?");
    let commit = data["commit_id"].as_str().unwrap_or("");
    if commit.len() >= 8 {
        println!("board updated: {path} {}", &commit[..8]);
    } else {
        println!("board updated: {path}");
    }
}

pub fn cmd_path(mode: &OutputMode) {
    let repo_root = get_repo_root();
    let handler = read_my_handler(&repo_root);
    let rel = match board_path(&handler) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };
    let path = repo_root.join(&rel);
    match mode {
        OutputMode::Human => println!("{}", path.display()),
        OutputMode::Json => print_json(serde_json::json!({
            "handler": handler,
            "path": path.to_string_lossy(),
        })),
    }
}

pub async fn cmd_init(client: &GitimClient, mode: &OutputMode) {
    match client.board_init().await {
        Ok(resp) => print_or_exit(resp, mode, print_board_write),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_show(client: &GitimClient, mode: &OutputMode, handler: &str) {
    match client.board_show(handler).await {
        Ok(resp) => print_or_exit(resp, mode, |data| {
            let meta = &data["meta"];
            let handler = data["handler"].as_str().unwrap_or(handler);
            let status = meta["status"].as_str().unwrap_or("");
            let summary = meta["summary"].as_str().unwrap_or("");
            println!("@{handler} [{status}] {summary}");
            if let Some(body) = data["body"].as_str() {
                if !body.trim().is_empty() {
                    println!("{}", body.trim_end());
                }
            }
        }),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_ls(client: &GitimClient, mode: &OutputMode) {
    match client.board_list().await {
        Ok(resp) => print_or_exit(resp, mode, |data| {
            let boards = data.get("boards").and_then(|v| v.as_array());
            match boards {
                Some(items) if !items.is_empty() => {
                    for board in items {
                        let handler = board["handler"].as_str().unwrap_or("?");
                        let status = board["status"].as_str().unwrap_or("");
                        let summary = board["summary"].as_str().unwrap_or("");
                        println!("@{handler} [{status}] {summary}");
                    }
                }
                _ => println!("no boards"),
            }
        }),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_publish(client: &GitimClient, mode: &OutputMode, content: Option<&str>) {
    match client.board_publish(content).await {
        Ok(resp) => print_or_exit(resp, mode, print_board_write),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_set(client: &GitimClient, mode: &OutputMode, field: &str, value: &str) {
    match client.board_set(field, value).await {
        Ok(resp) => print_or_exit(resp, mode, print_board_write),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_section_set(client: &GitimClient, mode: &OutputMode, section: &str, value: &str) {
    match client.board_section_set(section, value).await {
        Ok(resp) => print_or_exit(resp, mode, print_board_write),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_section_append(
    client: &GitimClient,
    mode: &OutputMode,
    section: &str,
    value: &str,
) {
    match client.board_section_append(section, value).await {
        Ok(resp) => print_or_exit(resp, mode, print_board_write),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}
