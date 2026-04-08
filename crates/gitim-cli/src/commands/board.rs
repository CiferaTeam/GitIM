#![deny(warnings)]

use std::process;

use gitim_client::GitimClient;

use crate::output::OutputMode;

pub async fn cmd_create_board(
    client: &GitimClient,
    mode: &OutputMode,
    name: &str,
    display_name: Option<&str>,
    statuses: Option<&[String]>,
) {
    match client.create_board(name, display_name, statuses).await {
        Ok(resp) => {
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("创建失败: {msg}");
                process::exit(1);
            }
            match mode {
                OutputMode::Human => println!("看板 #{name} 创建成功"),
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

pub async fn cmd_list_boards(client: &GitimClient, mode: &OutputMode) {
    match client.list_boards().await {
        Ok(resp) => {
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("Error: {msg}");
                process::exit(1);
            }
            match mode {
                OutputMode::Human => {
                    let boards = resp
                        .data
                        .as_ref()
                        .and_then(|d| d.get("boards"))
                        .and_then(|b| b.as_array());

                    match boards {
                        Some(arr) if !arr.is_empty() => {
                            for b in arr {
                                let name = b.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                                let display =
                                    b.get("display_name").and_then(|v| v.as_str()).unwrap_or("");
                                let statuses = b
                                    .get("statuses")
                                    .and_then(|v| v.as_array())
                                    .map(|arr| {
                                        arr.iter()
                                            .filter_map(|s| s.as_str())
                                            .collect::<Vec<_>>()
                                            .join(", ")
                                    })
                                    .unwrap_or_default();
                                println!("#{name}  {display}  [{statuses}]");
                            }
                        }
                        _ => println!("没有看板"),
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
