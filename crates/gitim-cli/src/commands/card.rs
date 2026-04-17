#![deny(warnings)]

use std::{env, fs, process};
use std::path::Path;

use gitim_client::{find_repo_root, GitimClient};

use crate::output::OutputMode;

fn get_repo_root() -> std::path::PathBuf {
    let cwd = env::current_dir().unwrap_or_else(|e| {
        eprintln!("Error: cannot read current directory: {e}");
        process::exit(1);
    });
    match find_repo_root(&cwd) {
        Some(r) => r,
        None => {
            eprintln!("Error: not in a GitIM repository (no .gitim/ found)");
            process::exit(1);
        }
    }
}

fn read_my_handler(repo_root: &Path) -> String {
    let me_path = repo_root.join(".gitim/me.json");
    let contents = match fs::read_to_string(&me_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: cannot read {}: {e}", me_path.display());
            process::exit(1);
        }
    };
    let v: serde_json::Value = match serde_json::from_str(&contents) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: invalid me.json: {e}");
            process::exit(1);
        }
    };
    match v.get("handler").and_then(|h| h.as_str()) {
        Some(h) => h.to_string(),
        None => {
            eprintln!("Error: me.json missing \"handler\" field");
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
    match mode {
        OutputMode::Human => {
            if let Some(d) = &resp.data {
                human_success(d);
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

pub async fn cmd_create_card(
    client: &GitimClient,
    mode: &OutputMode,
    channel: &str,
    title: &str,
    labels: Option<&[String]>,
    assignee: Option<&str>,
    status: Option<&str>,
) {
    match client.create_card(channel, title, labels, assignee, status).await {
        Ok(resp) => print_or_exit(resp, mode, |d| {
            let id = d["card_id"].as_str().unwrap_or("?");
            let ch = d["channel"].as_str().unwrap_or("?");
            println!("创建卡片 #{}/{}", ch, id);
        }),
        Err(e) => {
            eprintln!("创建失败: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_list_cards(
    client: &GitimClient,
    mode: &OutputMode,
    channel: Option<&str>,
    labels: Option<&[String]>,
    status: Option<&str>,
    assignee: Option<&str>,
) {
    match client.list_cards(channel, labels, status, assignee).await {
        Ok(resp) => print_or_exit(resp, mode, |d| {
            let cards = d.get("cards").and_then(|v| v.as_array());
            match cards {
                Some(arr) if !arr.is_empty() => {
                    for c in arr {
                        let ch = c["channel"].as_str().unwrap_or("?");
                        let id = c["card_id"].as_str().unwrap_or("?");
                        let t = c["title"].as_str().unwrap_or("");
                        let s = c["status"].as_str().unwrap_or("");
                        let a = c["assignee"].as_str().unwrap_or("-");
                        let ls: Vec<&str> = c["labels"]
                            .as_array()
                            .map(|arr| arr.iter().filter_map(|l| l.as_str()).collect())
                            .unwrap_or_default();
                        println!(
                            "#{ch}/{id}  [{s}]  {t}  @{a}  {}",
                            if ls.is_empty() { String::new() } else { format!("[{}]", ls.join(", ")) }
                        );
                    }
                }
                _ => println!("没有卡片"),
            }
        }),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_read_card(
    client: &GitimClient,
    mode: &OutputMode,
    channel: &str,
    card_id: &str,
    limit: Option<u64>,
    since: Option<u64>,
) {
    match client.read_card(channel, card_id, limit, since).await {
        Ok(resp) => print_or_exit(resp, mode, |d| {
            let meta = &d["meta"];
            println!(
                "#{}/{}  [{}]  {}",
                d["channel"].as_str().unwrap_or("?"),
                d["card_id"].as_str().unwrap_or("?"),
                meta["status"].as_str().unwrap_or(""),
                meta["title"].as_str().unwrap_or(""),
            );
            if let Some(entries) = d["entries"].as_array() {
                for e in entries {
                    let ln = e["line_number"].as_u64().unwrap_or(0);
                    let author = e["author"].as_str().unwrap_or("?");
                    let body = e["body"].as_str().unwrap_or("");
                    println!("L{:06} @{}: {}", ln, author, body);
                }
            }
        }),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_send_card_message(
    client: &GitimClient,
    mode: &OutputMode,
    channel: &str,
    card_id: &str,
    body: &str,
    reply_to: Option<u64>,
) {
    match client.send_card_message(channel, card_id, body, reply_to).await {
        Ok(resp) => print_or_exit(resp, mode, |d| {
            println!(
                "L{:06} -> #{}/{}",
                d["line_number"].as_u64().unwrap_or(0),
                d["channel"].as_str().unwrap_or("?"),
                d["card_id"].as_str().unwrap_or("?"),
            );
        }),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_update_card(
    client: &GitimClient,
    mode: &OutputMode,
    channel: &str,
    card_id: &str,
    status: Option<&str>,
    labels: Option<&[String]>,
    assignee: Option<&str>,
) {
    match client.update_card(channel, card_id, status, labels, assignee).await {
        Ok(resp) => print_or_exit(resp, mode, |d| {
            println!(
                "更新 #{}/{}  status={}  assignee={}",
                d["channel"].as_str().unwrap_or("?"),
                d["card_id"].as_str().unwrap_or("?"),
                d["status"].as_str().unwrap_or(""),
                d["assignee"].as_str().unwrap_or("-"),
            );
        }),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_archive_card(
    client: &GitimClient,
    mode: &OutputMode,
    channel: &str,
    card_id: &str,
) {
    let repo_root = get_repo_root();
    let author = read_my_handler(&repo_root);
    match client.archive_card(channel, card_id, &author).await {
        Ok(resp) => print_or_exit(resp, mode, |_| {
            println!("已归档卡片 #{}/{}", channel, card_id);
        }),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_unarchive_card(
    client: &GitimClient,
    mode: &OutputMode,
    channel: &str,
    card_id: &str,
) {
    let repo_root = get_repo_root();
    let author = read_my_handler(&repo_root);
    match client.unarchive_card(channel, card_id, &author).await {
        Ok(resp) => print_or_exit(resp, mode, |_| {
            println!("已取消归档卡片 #{}/{}", channel, card_id);
        }),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_archived_cards(
    client: &GitimClient,
    mode: &OutputMode,
    channel: Option<&str>,
) {
    match client.list_archived_cards(channel).await {
        Ok(resp) => print_or_exit(resp, mode, |d| {
            let cards = d.get("cards").and_then(|v| v.as_array());
            match cards {
                Some(arr) if !arr.is_empty() => {
                    for c in arr {
                        let ch = c["channel"].as_str().unwrap_or("?");
                        let id = c["card_id"].as_str().unwrap_or("?");
                        let t = c["title"].as_str().unwrap_or("");
                        let s = c["status"].as_str().unwrap_or("");
                        let a = c["assignee"].as_str().unwrap_or("-");
                        let ls: Vec<&str> = c["labels"]
                            .as_array()
                            .map(|arr| arr.iter().filter_map(|l| l.as_str()).collect())
                            .unwrap_or_default();
                        println!(
                            "#{ch}/{id}  [{s}]  {t}  @{a}  {}",
                            if ls.is_empty() { String::new() } else { format!("[{}]", ls.join(", ")) }
                        );
                    }
                }
                _ => println!("没有已归档的卡片"),
            }
        }),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}
