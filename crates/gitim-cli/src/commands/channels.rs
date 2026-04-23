#![deny(warnings)]

use std::process;

use gitim_client::GitimClient;

use crate::output::OutputMode;

pub async fn cmd_channels(client: &GitimClient, mode: &OutputMode) {
    match client.list_channels().await {
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

pub async fn cmd_create_channel(
    client: &GitimClient,
    mode: &OutputMode,
    name: &str,
    display_name: Option<&str>,
    introduction: Option<&str>,
) {
    match client
        .create_channel(name, display_name, introduction, &[])
        .await
    {
        Ok(resp) => {
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("创建失败: {msg}");
                process::exit(1);
            }
            match mode {
                OutputMode::Human => println!("频道 #{name} 创建成功"),
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

pub async fn cmd_join_channel(
    client: &GitimClient,
    mode: &OutputMode,
    channel: &str,
    targets: &[String],
) {
    match client.join_channel(channel, targets).await {
        Ok(resp) => {
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("加入失败: {msg}");
                process::exit(1);
            }
            match mode {
                OutputMode::Human => {
                    let who = if targets.is_empty() {
                        "你".to_string()
                    } else {
                        targets.join(", ")
                    };
                    println!("{who} 已加入 #{channel}");
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
            eprintln!("加入失败: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_leave_channel(client: &GitimClient, mode: &OutputMode, channel: &str) {
    match client.leave_channel(channel, &[]).await {
        Ok(resp) => {
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("退出失败: {msg}");
                process::exit(1);
            }
            match mode {
                OutputMode::Human => println!("已退出 #{channel}"),
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
            eprintln!("退出失败: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_archive_channel(client: &GitimClient, mode: &OutputMode, name: &str) {
    match client.archive_channel(name).await {
        Ok(resp) => {
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("归档失败: {msg}");
                process::exit(1);
            }
            match mode {
                OutputMode::Human => println!("频道 #{name} 已归档"),
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

pub async fn cmd_unarchive_channel(client: &GitimClient, mode: &OutputMode, name: &str) {
    match client.unarchive_channel(name).await {
        Ok(resp) => {
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("取消归档失败: {msg}");
                process::exit(1);
            }
            match mode {
                OutputMode::Human => println!("频道 #{name} 已取消归档"),
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

pub async fn cmd_archived_channels(client: &GitimClient, mode: &OutputMode) {
    match client.list_archived_channels().await {
        Ok(resp) => {
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("Error: {msg}");
                process::exit(1);
            }
            match mode {
                OutputMode::Human => {
                    let channels = resp
                        .data
                        .as_ref()
                        .and_then(|d| d.get("channels"))
                        .and_then(|c| c.as_array());

                    match channels {
                        Some(arr) if !arr.is_empty() => {
                            for ch in arr {
                                if let Some(name) = ch.get("name").and_then(|n| n.as_str()) {
                                    println!("#{name}");
                                }
                            }
                        }
                        _ => println!("暂无已归档频道"),
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
