#![deny(warnings)]

use std::process;

use gitim_client::GitimClient;

use crate::output::OutputMode;

pub async fn cmd_create_card(
    client: &GitimClient,
    mode: &OutputMode,
    board: &str,
    title: &str,
    assignee: Option<&str>,
    status: Option<&str>,
) {
    match client.create_card(board, title, assignee, status).await {
        Ok(resp) => {
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("创建失败: {msg}");
                process::exit(1);
            }
            match mode {
                OutputMode::Human => {
                    let card_id = resp
                        .data
                        .as_ref()
                        .and_then(|d| d.get("card_id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    println!("卡片 {card_id} 创建成功 ({board})");
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
            eprintln!("创建失败: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_list_cards(
    client: &GitimClient,
    mode: &OutputMode,
    board: &str,
    status: Option<&str>,
) {
    match client.list_cards(board, status).await {
        Ok(resp) => {
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("Error: {msg}");
                process::exit(1);
            }
            match mode {
                OutputMode::Human => {
                    let cards = resp
                        .data
                        .as_ref()
                        .and_then(|d| d.get("cards"))
                        .and_then(|c| c.as_array());

                    match cards {
                        Some(arr) if !arr.is_empty() => {
                            for c in arr {
                                let st =
                                    c.get("status").and_then(|v| v.as_str()).unwrap_or("?");
                                let id =
                                    c.get("card_id").and_then(|v| v.as_str()).unwrap_or("?");
                                let title =
                                    c.get("title").and_then(|v| v.as_str()).unwrap_or("");
                                let assignee =
                                    c.get("assignee").and_then(|v| v.as_str());
                                match assignee {
                                    Some(a) => println!("[{st}] {id}  {title}  @{a}"),
                                    None => println!("[{st}] {id}  {title}"),
                                }
                            }
                        }
                        _ => println!("没有卡片"),
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

pub async fn cmd_read_card(
    client: &GitimClient,
    mode: &OutputMode,
    board: &str,
    card_id: &str,
    limit: Option<u64>,
    since: Option<u64>,
) {
    match client.read_card(board, card_id, limit, since).await {
        Ok(resp) => {
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("Error: {msg}");
                process::exit(1);
            }
            match mode {
                OutputMode::Human => {
                    let entries = resp
                        .data
                        .as_ref()
                        .and_then(|d| d.get("entries"))
                        .and_then(|e| e.as_array());

                    if let Some(arr) = entries {
                        for entry in arr {
                            let line = entry
                                .get("line_number")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            let author = entry
                                .get("author")
                                .and_then(|v| v.as_str())
                                .unwrap_or("?");
                            let ts = entry
                                .get("timestamp")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let point_to = entry
                                .get("point_to")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            let body = entry
                                .get("body")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");

                            if point_to > 0 {
                                println!("[L{line}] @{author} {ts} (re: L{point_to})");
                            } else {
                                println!("[L{line}] @{author} {ts}");
                            }
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

pub async fn cmd_send_card_message(
    client: &GitimClient,
    mode: &OutputMode,
    board: &str,
    card_id: &str,
    body: &str,
    reply_to: Option<u64>,
) {
    match client
        .send_card_message(board, card_id, body, reply_to)
        .await
    {
        Ok(resp) => {
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("Error: {msg}");
                process::exit(1);
            }
            match mode {
                OutputMode::Human => println!("Message sent."),
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

pub async fn cmd_update_card(
    client: &GitimClient,
    mode: &OutputMode,
    board: &str,
    card_id: &str,
    status: Option<&str>,
    assignee: Option<&str>,
) {
    match client.update_card(board, card_id, status, assignee).await {
        Ok(resp) => {
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("更新失败: {msg}");
                process::exit(1);
            }
            match mode {
                OutputMode::Human => {
                    let st = status.unwrap_or("none");
                    let asg = assignee.unwrap_or("none");
                    println!("卡片 {card_id} 已更新: status={st}, assignee={asg}");
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
            eprintln!("更新失败: {e}");
            process::exit(1);
        }
    }
}
