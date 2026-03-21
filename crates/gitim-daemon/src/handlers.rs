use crate::api::{Event, Request, Response};
use crate::state::{PendingMessage, SharedState};
use gitim_core::dm::dm_filename;
use gitim_core::formatter::format_message;
use gitim_core::parser::parse_thread;
use gitim_core::types::Handler;
use gitim_core::validator::compliance::validate_append;
use tracing::{info, warn};

pub async fn handle_request(req: Request, state: SharedState) -> Response {
    match req {
        Request::Status => Response::success(serde_json::json!({
            "version": "0.1.0",
            "status": "running",
        })),
        Request::Send { channel, body, reply_to, author } => {
            // Resolve author: explicit > current_user > error
            let resolved_author = match author {
                Some(a) if !a.is_empty() => a,
                _ => {
                    let current = state.current_user.read().await;
                    match current.clone() {
                        Some(u) => u,
                        None => return Response::error("no author specified and no identity configured".to_string()),
                    }
                }
            };
            handle_send(state, channel, body, reply_to, resolved_author).await
        }
        Request::Read { channel, limit, since } => {
            handle_read(state, channel, limit, since).await
        }
        Request::ListChannels => handle_list_channels(state).await,
        Request::ListUsers => handle_list_users(state).await,
        Request::GetThread { channel, line_number } => {
            handle_get_thread(state, channel, line_number).await
        }
        Request::Subscribe => {
            Response::success(serde_json::json!({"subscribed": true}))
        }
        Request::RegisterUser { handler, display_name, role, introduction } => {
            handle_register_user(state, handler, display_name, role, introduction).await
        }
        Request::Stop => handle_stop(state).await,
        Request::Onboard { git_server, auth: _ } => {
            // TODO(Task 6): implement full onboard logic
            let known = matches!(git_server.as_str(), "git" | "github" | "gitea" | "gitlab");
            if !known {
                return Response::error(format!("unknown git_server: {}", git_server));
            }
            Response::success(serde_json::json!({
                "status": "not_implemented",
                "git_server": git_server,
            }))
        }
    }
}

/// Resolve a channel string to a filesystem path and a cache key.
/// Channels: "channels/{name}.thread", DMs: "dm:{h1},{h2}" -> "dm/{h1}--{h2}.thread"
fn resolve_thread_path(
    state: &SharedState,
    channel: &str,
) -> Result<(std::path::PathBuf, String), Response> {
    if channel.starts_with("dm:") {
        let parts: Vec<&str> = channel[3..].split(',').collect();
        if parts.len() != 2 {
            return Err(Response::error("DM format must be dm:handler1,handler2"));
        }
        let h1 = Handler::new(parts[0])
            .map_err(|e| Response::error(format!("invalid DM handler: {}", e)))?;
        let h2 = Handler::new(parts[1])
            .map_err(|e| Response::error(format!("invalid DM handler: {}", e)))?;
        let name = dm_filename(&h1, &h2);
        let path = state.repo_root.join("dm").join(format!("{}.thread", name));
        Ok((path, name))
    } else {
        let path = state
            .repo_root
            .join("channels")
            .join(format!("{}.thread", channel));
        Ok((path, channel.to_string()))
    }
}

async fn handle_send(
    state: SharedState,
    channel: String,
    body: String,
    reply_to: Option<u64>,
    author: String,
) -> Response {
    // Validate author handler format
    let handler = match Handler::new(&author) {
        Ok(h) => h,
        Err(e) => return Response::error(format!("invalid author: {}", e)),
    };

    // Check author is registered
    let user_list: Vec<String> = {
        let users = state.users.read().await;
        if !users.contains(&author) {
            return Response::error(format!("unknown user: {}", author));
        }
        users.clone()
    };
    let user_refs: Vec<&str> = user_list.iter().map(|s| s.as_str()).collect();

    // Resolve file path
    let (thread_path, thread_name) = match resolve_thread_path(&state, &channel) {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    // Ensure parent directory exists
    if let Some(parent) = thread_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    // Read existing content and parse
    let existing = std::fs::read_to_string(&thread_path).unwrap_or_default();
    let existing_file = match parse_thread(&existing) {
        Ok(f) => f,
        Err(e) => return Response::error(format!("failed to parse thread: {}", e)),
    };

    let next_line = existing_file
        .messages
        .last()
        .map(|m| m.line_number + 1)
        .unwrap_or(1);
    let point_to = reply_to.unwrap_or(0);

    // Generate timestamp and format message
    let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let new_content = format_message(next_line, point_to, &handler, &now, &body);

    // Validate compliance
    if let Err(e) = validate_append(&existing, &new_content, &user_refs) {
        return Response::error(format!("compliance check failed: {}", e));
    }

    // Append to file
    use std::io::Write;
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&thread_path)
    {
        Ok(mut file) => {
            if let Err(e) = file.write_all(new_content.as_bytes()) {
                return Response::error(format!("write failed: {}", e));
            }
        }
        Err(e) => return Response::error(format!("open failed: {}", e)),
    }

    // Git add + commit (best effort — message was already written)
    let commit_status = match thread_path.strip_prefix(&state.repo_root) {
        Ok(rel) => {
            let rel_str = rel.to_string_lossy().to_string();
            let commit_msg = format!("msg: @{} -> {} L{:06}", author, thread_name, next_line);
            match state.git_storage.add_and_commit(&[&rel_str], &commit_msg) {
                Ok(()) => "committed",
                Err(e) => {
                    warn!("git commit failed for L{:06} in {}: {}", next_line, thread_name, e);
                    "written"
                }
            }
        }
        Err(e) => {
            warn!("failed to compute relative path for git add: {}", e);
            "written"
        }
    };

    // Record in pending_push
    {
        let mut pending = state.pending_push.write().unwrap();
        pending.push(PendingMessage {
            channel: thread_name.clone(),
            line_number: next_line,
        });
    }

    // Invalidate cache
    state.thread_cache.write().await.remove(&thread_name);

    // Broadcast event
    let kind = if channel.starts_with("dm:") { "dm" } else { "channel" };
    let _ = state.event_tx.send(Event::ThreadChanged {
        channel: thread_name.clone(),
        kind: kind.to_string(),
    });

    info!(
        "message sent to {} by @{} at L{:06}",
        thread_name, author, next_line
    );
    Response::success(serde_json::json!({
        "line_number": next_line,
        "channel": thread_name,
        "status": commit_status,
    }))
}

async fn handle_read(
    state: SharedState,
    channel: String,
    limit: Option<usize>,
    since: Option<u64>,
) -> Response {
    let (thread_path, _) = match resolve_thread_path(&state, &channel) {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    let content = std::fs::read_to_string(&thread_path).unwrap_or_default();
    let file = match parse_thread(&content) {
        Ok(f) => f,
        Err(e) => return Response::error(format!("parse error: {}", e)),
    };

    let mut messages: Vec<&gitim_core::types::Message> = file.messages.iter().collect();

    if let Some(since_line) = since {
        messages.retain(|m| m.line_number > since_line);
    }

    if let Some(lim) = limit {
        let start = messages.len().saturating_sub(lim);
        messages = messages[start..].to_vec();
    }

    let json_msgs: Vec<serde_json::Value> = messages
        .iter()
        .map(|m| {
            serde_json::json!({
                "line_number": m.line_number,
                "point_to": m.point_to,
                "author": m.author.as_str(),
                "timestamp": m.timestamp,
                "body": m.body,
            })
        })
        .collect();

    Response::success(serde_json::json!({
        "channel": channel,
        "messages": json_msgs,
    }))
}

async fn handle_register_user(
    state: SharedState,
    handler: String,
    display_name: String,
    role: String,
    introduction: String,
) -> Response {
    // Validate handler format
    if let Err(e) = Handler::new(&handler) {
        return Response::error(format!("invalid handler: {}", e));
    }

    let users_dir = state.repo_root.join("users");
    std::fs::create_dir_all(&users_dir).ok();
    let meta_path = users_dir.join(format!("{}.meta.json", handler));

    // If already exists, return success with exists=true
    if meta_path.exists() {
        return Response::success(serde_json::json!({
            "handler": handler,
            "exists": true
        }));
    }

    // Create meta file
    let meta = serde_json::json!({
        "display_name": display_name,
        "role": role,
        "introduction": introduction
    });
    let meta_str = serde_json::to_string_pretty(&meta).unwrap();

    if let Err(e) = std::fs::write(&meta_path, &meta_str) {
        return Response::error(format!("failed to write user meta: {}", e));
    }

    // Add to users list
    {
        let mut users = state.users.write().await;
        if !users.contains(&handler) {
            users.push(handler.clone());
            users.sort();
        }
    }

    // Git add + commit (best effort)
    let _ = state.git_storage.add_and_commit(
        &[&format!("users/{}.meta.json", handler)],
        &format!("msg: register @{}", handler),
    );

    Response::success(serde_json::json!({
        "handler": handler,
        "exists": false
    }))
}

async fn handle_list_channels(state: SharedState) -> Response {
    let ch_dir = state.repo_root.join("channels");
    let mut channels = Vec::new();
    if ch_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&ch_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".meta.json") {
                    channels.push(name.trim_end_matches(".meta.json").to_string());
                }
            }
        }
    }
    channels.sort();
    Response::success(serde_json::json!({ "channels": channels }))
}

async fn handle_list_users(state: SharedState) -> Response {
    let users = state.users.read().await;
    let mut sorted: Vec<String> = users.clone();
    sorted.sort();
    Response::success(serde_json::json!({ "users": sorted }))
}

async fn handle_get_thread(
    state: SharedState,
    channel: String,
    line_number: u64,
) -> Response {
    let thread_path = state
        .repo_root
        .join("channels")
        .join(format!("{}.thread", channel));
    let content = std::fs::read_to_string(&thread_path).unwrap_or_default();
    let file = match parse_thread(&content) {
        Ok(f) => f,
        Err(e) => return Response::error(format!("parse error: {}", e)),
    };

    // Collect the root message and all descendants (messages pointing to it, recursively)
    let mut thread_msgs = Vec::new();
    let mut stack = vec![line_number];
    let mut visited = std::collections::HashSet::new();

    while let Some(target) = stack.pop() {
        if !visited.insert(target) {
            continue;
        }
        for msg in &file.messages {
            if msg.line_number == target || msg.point_to == target {
                thread_msgs.push(serde_json::json!({
                    "line_number": msg.line_number,
                    "point_to": msg.point_to,
                    "author": msg.author.as_str(),
                    "timestamp": msg.timestamp,
                    "body": msg.body,
                }));
                if msg.line_number != target {
                    stack.push(msg.line_number);
                }
            }
        }
    }

    // Sort by line number
    thread_msgs.sort_by(|a, b| {
        a["line_number"]
            .as_u64()
            .unwrap()
            .cmp(&b["line_number"].as_u64().unwrap())
    });

    // Deduplicate (a message could match both by line_number and point_to)
    thread_msgs.dedup_by(|a, b| a["line_number"] == b["line_number"]);

    Response::success(serde_json::json!({
        "channel": channel,
        "root_line": line_number,
        "messages": thread_msgs,
    }))
}

async fn handle_stop(state: SharedState) -> Response {
    let lifecycle = crate::lifecycle::DaemonLifecycle::new(&state.repo_root);
    lifecycle.cleanup();
    tracing::info!("daemon stopping via API request");

    // Spawn a delayed exit so the response can be sent first
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        std::process::exit(0);
    });

    Response::success(serde_json::json!({ "status": "stopping" }))
}
