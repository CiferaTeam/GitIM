#![deny(warnings)]

use std::{fs, process};

use gitim_client::GitimClient;

use super::{get_repo_root, read_my_handler};
use crate::output::OutputMode;

/// Build the DM channel name from two handlers: `dm:{sorted[0]},{sorted[1]}`.
fn dm_channel(h1: &str, h2: &str) -> String {
    let (a, b) = if h1 < h2 { (h1, h2) } else { (h2, h1) };
    format!("dm:{a},{b}")
}

pub async fn cmd_dm_send(
    client: &GitimClient,
    mode: &OutputMode,
    target: &str,
    body: &str,
    author: Option<&str>,
    reply_to: Option<u64>,
) {
    let repo_root = get_repo_root();
    let my_handler = author
        .map(|a| a.to_string())
        .unwrap_or_else(|| read_my_handler(&repo_root));
    let channel = dm_channel(&my_handler, target);

    match client
        .send(&channel, body, Some(&my_handler), reply_to)
        .await
    {
        Ok(resp) => {
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("Error: {msg}");
                process::exit(1);
            }
            match mode {
                OutputMode::Human => println!("DM sent."),
                OutputMode::Json => {
                    let data = resp.data.unwrap_or(serde_json::Value::Null);
                    match serde_json::to_string(&data) {
                        Ok(s) => println!("{s}"),
                        Err(e) => {
                            eprintln!("Error: failed to format output: {e}");
                            process::exit(1);
                        }
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_dm_read(
    client: &GitimClient,
    mode: &OutputMode,
    target: &str,
    author: Option<&str>,
    limit: Option<u64>,
    since: Option<u64>,
) {
    let repo_root = get_repo_root();
    let my_handler = author
        .map(|a| a.to_string())
        .unwrap_or_else(|| read_my_handler(&repo_root));
    let channel = dm_channel(&my_handler, target);

    match client.read(&channel, limit, since).await {
        Ok(resp) => {
            let code = mode.print(&resp);
            if code != 0 {
                process::exit(code);
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_archive_dm(client: &GitimClient, mode: &OutputMode, peer: &str) {
    match client.archive_dm(peer).await {
        Ok(resp) => {
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("归档失败: {msg}");
                process::exit(1);
            }
            match mode {
                OutputMode::Human => println!("已归档与 @{peer} 的私信"),
                OutputMode::Json => {
                    let data = resp.data.unwrap_or(serde_json::Value::Null);
                    match serde_json::to_string(&data) {
                        Ok(s) => println!("{s}"),
                        Err(e) => {
                            eprintln!("Error: failed to format output: {e}");
                            process::exit(1);
                        }
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("归档失败: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_unarchive_dm(client: &GitimClient, mode: &OutputMode, peer: &str) {
    match client.unarchive_dm(peer).await {
        Ok(resp) => {
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("取消归档失败: {msg}");
                process::exit(1);
            }
            match mode {
                OutputMode::Human => println!("已恢复与 @{peer} 的私信"),
                OutputMode::Json => {
                    let data = resp.data.unwrap_or(serde_json::Value::Null);
                    match serde_json::to_string(&data) {
                        Ok(s) => println!("{s}"),
                        Err(e) => {
                            eprintln!("Error: failed to format output: {e}");
                            process::exit(1);
                        }
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("取消归档失败: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_list_archived_dms(client: &GitimClient, mode: &OutputMode) {
    match client.list_archived_dms().await {
        Ok(resp) => {
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("Error: {msg}");
                process::exit(1);
            }
            match mode {
                OutputMode::Human => {
                    let dms = resp
                        .data
                        .as_ref()
                        .and_then(|d| d.get("dms"))
                        .and_then(|c| c.as_array());

                    match dms {
                        Some(arr) if !arr.is_empty() => {
                            for entry in arr {
                                if let Some(peer) =
                                    entry.get("peer").and_then(|p| p.as_str())
                                {
                                    println!("{peer}");
                                }
                            }
                        }
                        _ => println!("暂无已归档私信"),
                    }
                }
                OutputMode::Json => {
                    let data = resp.data.unwrap_or(serde_json::Value::Null);
                    match serde_json::to_string(&data) {
                        Ok(s) => println!("{s}"),
                        Err(e) => {
                            eprintln!("Error: failed to format output: {e}");
                            process::exit(1);
                        }
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_list_archived_users(client: &GitimClient, mode: &OutputMode) {
    match client.list_archived_users().await {
        Ok(resp) => {
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("Error: {msg}");
                process::exit(1);
            }
            match mode {
                OutputMode::Human => {
                    let users = resp
                        .data
                        .as_ref()
                        .and_then(|d| d.get("users"))
                        .and_then(|u| u.as_array());

                    match users {
                        Some(arr) if !arr.is_empty() => {
                            for handler in arr {
                                if let Some(h) = handler.as_str() {
                                    println!("{h}");
                                }
                            }
                        }
                        _ => println!("暂无已退出用户"),
                    }
                }
                OutputMode::Json => {
                    let data = resp.data.unwrap_or(serde_json::Value::Null);
                    match serde_json::to_string(&data) {
                        Ok(s) => println!("{s}"),
                        Err(e) => {
                            eprintln!("Error: failed to format output: {e}");
                            process::exit(1);
                        }
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub fn cmd_dm_list(mode: &OutputMode) {
    let repo_root = get_repo_root();
    let dm_dir = repo_root.join("dm");

    let entries = match fs::read_dir(&dm_dir) {
        Ok(e) => e,
        Err(_) => {
            match mode {
                OutputMode::Human => println!("No DM conversations."),
                OutputMode::Json => println!("[]"),
            }
            return;
        }
    };

    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.strip_suffix(".thread").map(|s| s.to_string())
        })
        .collect();

    if names.is_empty() {
        match mode {
            OutputMode::Human => println!("No DM conversations."),
            OutputMode::Json => println!("[]"),
        }
        return;
    }

    names.sort();

    match mode {
        OutputMode::Human => {
            for name in &names {
                println!("{name}");
            }
        }
        OutputMode::Json => {
            let arr: Vec<serde_json::Value> = names
                .iter()
                .map(|n| serde_json::Value::String(n.clone()))
                .collect();
            match serde_json::to_string(&arr) {
                Ok(s) => println!("{s}"),
                Err(e) => {
                    eprintln!("Error: failed to format output: {e}");
                    process::exit(1);
                }
            }
        }
    }
}
