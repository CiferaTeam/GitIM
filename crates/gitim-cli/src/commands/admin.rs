#![deny(warnings)]

use std::env;
use std::process;

use gitim_client::{find_repo_root, is_daemon_running, GitimClient};

use crate::output::OutputMode;

pub async fn cmd_stop(mode: &OutputMode) {
    let cwd = env::current_dir().unwrap_or_else(|e| {
        eprintln!("Error: cannot read current directory: {e}");
        process::exit(1);
    });

    let repo_root = match find_repo_root(&cwd) {
        Some(r) => r,
        None => {
            eprintln!("Error: not in a GitIM repository (no .gitim/ found)");
            process::exit(1);
        }
    };

    if !is_daemon_running(&repo_root) {
        match mode {
            OutputMode::Human => println!("Daemon is not running."),
            OutputMode::Json => println!(r#"{{"status":"not_running"}}"#),
        }
        return;
    }

    let client = GitimClient::new(&repo_root);
    match client.stop().await {
        Ok(_resp) => {
            match mode {
                OutputMode::Human => println!("Daemon stopped."),
                OutputMode::Json => println!(r#"{{"status":"stopping"}}"#),
            }
        }
        Err(_) => {
            // Daemon may shut down mid-response — treat connection errors as success
            match mode {
                OutputMode::Human => println!("Daemon stopped."),
                OutputMode::Json => println!(r#"{{"status":"stopping"}}"#),
            }
        }
    }
}

pub async fn cmd_users(client: &GitimClient, mode: &OutputMode) {
    match client.list_users().await {
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

#[allow(clippy::too_many_arguments)]
pub async fn cmd_search(
    client: &GitimClient,
    mode: &OutputMode,
    query: Option<&str>,
    author: Option<&str>,
    channel: Option<&str>,
    channel_type: Option<&str>,
    limit: u64,
    offset: u64,
    include_cards: bool,
) {
    match client
        .search(query, author, channel, channel_type, Some(limit), Some(offset), include_cards)
        .await
    {
        Ok(resp) => {
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("Error: {msg}");
                process::exit(1);
            }
            match mode {
                OutputMode::Human => {
                    let data = resp.data.as_ref();
                    let total = data
                        .and_then(|d| d.get("total"))
                        .and_then(|t| t.as_u64())
                        .unwrap_or(0);

                    println!("Found {total} results:");

                    if let Some(messages) = data.and_then(|d| d.get("messages")).and_then(|m| m.as_array()) {
                        for msg in messages {
                            let ch = msg.get("channel").and_then(|v| v.as_str()).unwrap_or("unknown");
                            let ct = msg.get("channel_type").and_then(|v| v.as_str()).unwrap_or("channel");
                            let author = msg.get("author").and_then(|v| v.as_str()).unwrap_or("unknown");
                            let line = msg.get("line_number").and_then(|v| v.as_u64()).unwrap_or(0);
                            let ts = msg.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
                            let body = msg.get("body").and_then(|v| v.as_str()).unwrap_or("");

                            let prefix = if ct == "dm" {
                                format!("[DM:{ch}]")
                            } else {
                                format!("[#{ch}]")
                            };

                            println!();
                            println!("{prefix} @{author} (L{line}) {ts}");
                            println!("  {body}");
                        }
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

pub async fn cmd_reindex(client: &GitimClient, mode: &OutputMode) {
    match mode {
        OutputMode::Human => eprintln!("Rebuilding search index..."),
        OutputMode::Json => {}
    }

    match client.reindex().await {
        Ok(resp) => {
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("Reindex failed: {msg}");
                process::exit(1);
            }
            match mode {
                OutputMode::Human => {
                    let n = resp
                        .data
                        .as_ref()
                        .and_then(|d| d.get("messages_indexed"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    println!("Done. {n} messages indexed.");
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
            eprintln!("Reindex failed: {e}");
            process::exit(1);
        }
    }
}
