use crate::api::{Event, Request, Response};
use crate::state::{PendingMessage, PushResult, SharedState};
use gitim_core::dm::{dm_filename, parse_dm_filename};
use gitim_core::formatter::{format_event, format_message};
use gitim_core::parser::parse_thread;
use gitim_core::types::{ChannelMeta, ChannelName, Handler, Link, LinkKind, ThreadEntry, UserMeta};
use gitim_core::validator::compliance::validate_append;
use gitim_core::validator::im_rules;
use gitim_sync::git::GitError;
use std::collections::HashMap;
use tracing::{info, warn};

fn link_to_json(link: &Link) -> serde_json::Value {
    match &link.kind {
        LinkKind::Channel { name } => serde_json::json!({
            "kind": "channel",
            "name": name,
            "raw": link.raw,
        }),
        LinkKind::Message {
            channel,
            line_number,
        } => serde_json::json!({
            "kind": "message",
            "channel": channel,
            "line_number": line_number,
            "raw": link.raw,
        }),
        LinkKind::UserProfile { handler } => serde_json::json!({
            "kind": "user_profile",
            "handler": handler.as_str(),
            "raw": link.raw,
        }),
        LinkKind::Softlink { url, title } => {
            let mut v = serde_json::json!({
                "kind": "softlink",
                "url": url,
                "raw": link.raw,
            });
            if let Some(t) = title {
                v["title"] = serde_json::json!(t);
            }
            v
        }
    }
}

pub async fn handle_request(req: Request, state: SharedState) -> Response {
    // Guest mode guard: reject all write operations
    if state.is_guest.load(std::sync::atomic::Ordering::SeqCst) {
        let is_write = matches!(
            req,
            Request::Send { .. }
                | Request::RegisterUser { .. }
                | Request::JoinChannel { .. }
                | Request::LeaveChannel { .. }
                | Request::CreateChannel { .. }
                | Request::ArchiveChannel { .. }
                | Request::CreateBoard { .. }
                | Request::CreateCard { .. }
                | Request::SendCardMessage { .. }
                | Request::UpdateCard { .. }
        );
        if is_write {
            return Response::error("guest mode: write operations are not allowed");
        }
    }

    match req {
        Request::Status => {
            let is_guest = state.is_guest.load(std::sync::atomic::Ordering::SeqCst);
            Response::success(serde_json::json!({
                "version": "0.1.0",
                "status": "running",
                "guest": is_guest,
            }))
        }
        Request::Send {
            channel,
            body,
            reply_to,
            author,
        } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            handle_send(state, channel, body, reply_to, resolved_author).await
        }
        Request::Read {
            channel,
            limit,
            since,
        } => handle_read(state, channel, limit, since).await,
        Request::ListChannels => handle_list_channels(state).await,
        Request::ListUsers => handle_list_users(state).await,
        Request::GetThread {
            channel,
            line_number,
        } => handle_get_thread(state, channel, line_number).await,
        Request::Subscribe => Response::success(serde_json::json!({"subscribed": true})),
        Request::RegisterUser {
            handler,
            display_name,
            role,
            introduction,
        } => handle_register_user(state, handler, display_name, role, introduction).await,
        Request::Poll { since } => handle_poll(state, since).await,
        Request::Stop => handle_stop(state).await,
        Request::Onboard {
            git_server,
            auth,
            admin,
            guest,
        } => crate::onboard::handle_onboard(state, git_server, auth, admin, guest).await,
        Request::JoinChannel {
            channel,
            targets,
            author,
        } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            handle_join_channel(state, channel, targets, resolved_author).await
        }
        Request::LeaveChannel {
            channel,
            targets,
            author,
        } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            handle_leave_channel(state, channel, targets, resolved_author).await
        }
        Request::CreateChannel {
            name,
            display_name,
            introduction,
            author,
            invitees,
        } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            handle_create_channel(state, name, display_name, introduction, resolved_author, invitees).await
        }
        Request::Search {
            query,
            author,
            channel,
            channel_type,
            limit,
            offset,
        } => handle_search(state, query, author, channel, channel_type, limit, offset).await,
        Request::Reindex => handle_reindex(state).await,
        Request::ArchiveChannel { channel, author } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            handle_archive_channel(state, channel, resolved_author).await
        }
        Request::ListArchivedChannels => handle_list_archived_channels(state).await,
        Request::CreateBoard {
            name,
            display_name,
            statuses,
            author,
        } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            crate::board_handlers::handle_create_board(
                state,
                name,
                display_name,
                statuses,
                resolved_author,
            )
            .await
        }
        Request::CreateCard {
            board,
            title,
            assignee,
            status,
            author,
        } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            crate::board_handlers::handle_create_card(
                state,
                board,
                title,
                assignee,
                status,
                resolved_author,
            )
            .await
        }
        Request::ListBoards => crate::board_handlers::handle_list_boards(state).await,
        Request::ListCards { board, status } => {
            crate::board_handlers::handle_list_cards(state, board, status).await
        }
        Request::ReadCard {
            board,
            card_id,
            limit,
            since,
        } => crate::board_handlers::handle_read_card(state, board, card_id, limit, since).await,
        Request::SendCardMessage {
            board,
            card_id,
            body,
            reply_to,
            author,
        } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            crate::board_handlers::handle_send_card_message(
                state,
                board,
                card_id,
                body,
                reply_to,
                resolved_author,
            )
            .await
        }
        Request::UpdateCard {
            board,
            card_id,
            status,
            assignee,
            author,
        } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            crate::board_handlers::handle_update_card(
                state,
                board,
                card_id,
                status,
                assignee,
                resolved_author,
            )
            .await
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
        let name = ChannelName::new(channel)
            .map_err(|e| Response::error(format!("invalid channel name: {}", e)))?;
        let path = state
            .repo_root
            .join("channels")
            .join(format!("{}.thread", name));
        Ok((path, name.to_string()))
    }
}

fn entry_to_json(entry: &ThreadEntry) -> serde_json::Value {
    match entry {
        ThreadEntry::Message(m) => serde_json::json!({
            "type": "message",
            "line_number": m.line_number,
            "point_to": m.point_to,
            "author": m.author.as_str(),
            "timestamp": m.timestamp,
            "body": m.body,
            "mentions": m.mentions.iter().map(|h| h.as_str()).collect::<Vec<_>>(),
            "links": m.links.iter().map(link_to_json).collect::<Vec<_>>(),
        }),
        ThreadEntry::Event(ev) => serde_json::json!({
            "type": "event",
            "event_type": ev.event_type,
            "line_number": ev.line_number,
            "author": ev.author.as_str(),
            "timestamp": ev.timestamp,
            "meta": ev.meta,
        }),
    }
}

async fn resolve_author(author: Option<String>, state: &SharedState) -> Result<String, Response> {
    match author {
        Some(a) if !a.is_empty() => Ok(a),
        _ => {
            let current = state.current_user.read().await;
            match current.clone() {
                Some(u) => Ok(u),
                None => Err(Response::error(
                    "no author specified and no identity configured",
                )),
            }
        }
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

    let next_line = existing_file.last_line_number() + 1;
    let point_to = reply_to.unwrap_or(0);

    // Generate timestamp and format message
    let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let new_content = format_message(next_line, point_to, &handler, &now, &body);

    // Compute allowed_senders based on channel type
    let allowed_senders: Vec<String> = if channel.starts_with("dm:") {
        let participants: Vec<String> =
            channel[3..].split(',').map(|s| s.to_string()).collect();
        // Check both DM participants are registered users
        for p in &participants {
            if !user_list.contains(p) {
                return Response::error(format!(
                    "DM participant '@{}' is not a registered user",
                    p
                ));
            }
        }
        participants
    } else {
        let meta_path = state
            .repo_root
            .join("channels")
            .join(format!("{}.meta.yaml", channel));
        if meta_path.exists() {
            match std::fs::read_to_string(&meta_path) {
                Ok(content) => match serde_yaml::from_str::<ChannelMeta>(&content) {
                    Ok(meta) => meta.members,
                    Err(e) => {
                        return Response::error(format!(
                            "failed to parse channel meta: {}",
                            e
                        ))
                    }
                },
                Err(e) => {
                    return Response::error(format!("failed to read channel meta: {}", e))
                }
            }
        } else {
            let archive_meta = state
                .repo_root
                .join("archive")
                .join("channels")
                .join(format!("{}.meta.yaml", channel));
            if archive_meta.exists() {
                return Response::error(format!("channel '{}' is archived", channel));
            }
            return Response::error(format!("channel '{}' does not exist", channel));
        }
    };
    let allowed_refs: Vec<&str> = allowed_senders.iter().map(|s| s.as_str()).collect();

    // Validate compliance
    if let Err(e) = validate_append(&existing, &new_content, &user_refs, &allowed_refs) {
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
            match state
                .git_storage
                .add_and_commit_as(&[&rel_str], &commit_msg, Some(&author))
            {
                Ok(()) => "committed",
                Err(e) => {
                    warn!(
                        "git commit failed for L{:06} in {}: {}",
                        next_line, thread_name, e
                    );
                    "written"
                }
            }
        }
        Err(e) => {
            warn!("failed to compute relative path for git add: {}", e);
            "written"
        }
    };

    // Record in pending_push and optionally set up push-result channel.
    // Only wait for push if we have a remote AND the sync loop is actually running.
    let should_await_push =
        state.has_remote && state.sync_started.load(std::sync::atomic::Ordering::SeqCst);
    let push_rx = if should_await_push {
        let (tx, rx) = tokio::sync::oneshot::channel::<PushResult>();
        {
            let mut pending = state.pending_push.write().unwrap();
            pending.push(PendingMessage {
                channel: thread_name.clone(),
                line_number: next_line,
                result_tx: Some(tx),
            });
        }
        Some(rx)
    } else {
        {
            let mut pending = state.pending_push.write().unwrap();
            pending.push(PendingMessage {
                channel: thread_name.clone(),
                line_number: next_line,
                result_tx: None,
            });
        }
        None
    };

    // Invalidate cache
    state.thread_cache.write().await.remove(&thread_name);

    // Broadcast event
    let kind = if channel.starts_with("dm:") {
        "dm"
    } else {
        "channel"
    };
    let _ = state.event_tx.send(Event::ThreadChanged {
        channel: thread_name.clone(),
        kind: kind.to_string(),
    });

    info!(
        "message sent to {} by @{} at L{:06}",
        thread_name, author, next_line
    );

    // If has_remote, wake sync_loop and await push result
    if let Some(rx) = push_rx {
        state.push_notify.notify_one();
        match rx.await {
            Ok(PushResult::Pushed { commit_id }) => Response::success(serde_json::json!({
                "line_number": next_line,
                "channel": thread_name,
                "status": "pushed",
                "commit_id": commit_id,
            })),
            Ok(PushResult::Failed { reason }) => Response::success(serde_json::json!({
                "line_number": next_line,
                "channel": thread_name,
                "status": "commit_only",
                "error": reason,
            })),
            Err(_) => {
                // Sender dropped — sync_loop may have been shut down
                Response::success(serde_json::json!({
                    "line_number": next_line,
                    "channel": thread_name,
                    "status": "commit_only",
                    "error": "push result channel closed",
                }))
            }
        }
    } else {
        Response::success(serde_json::json!({
            "line_number": next_line,
            "channel": thread_name,
            "status": commit_status,
        }))
    }
}

async fn handle_read(
    state: SharedState,
    channel: String,
    limit: Option<usize>,
    since: Option<u64>,
) -> Response {
    let (thread_path, name) = match resolve_thread_path(&state, &channel) {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    // Membership check: non-DM channels require the reader to be a member
    // (admin and guest skip — admin has god-view, guest is a read-only observer)
    if !channel.starts_with("dm:")
        && !state.is_admin.load(std::sync::atomic::Ordering::SeqCst)
        && !state.is_guest.load(std::sync::atomic::Ordering::SeqCst)
    {
        let meta_path = state
            .repo_root
            .join("channels")
            .join(format!("{}.meta.yaml", name));
        if meta_path.exists() {
            if let Some(ref current_user) = *state.current_user.read().await {
                let is_member = std::fs::read_to_string(&meta_path)
                    .ok()
                    .and_then(|c| serde_yaml::from_str::<ChannelMeta>(&c).ok())
                    .map(|m| m.members.contains(current_user))
                    .unwrap_or(true);
                if !is_member {
                    return Response::error("not_member");
                }
            }
        }
    }

    // For non-DM channels, fall back to archive path if the primary path doesn't exist
    let (thread_path, is_archived) = if !channel.starts_with("dm:") && !thread_path.exists() {
        let archive_path = state
            .repo_root
            .join("archive")
            .join("channels")
            .join(format!("{}.thread", name));
        if archive_path.exists() {
            (archive_path, true)
        } else {
            (thread_path, false)
        }
    } else {
        (thread_path, false)
    };

    let content = std::fs::read_to_string(&thread_path).unwrap_or_default();
    let file = match parse_thread(&content) {
        Ok(f) => f,
        Err(e) => return Response::error(format!("parse error: {}", e)),
    };

    let mut entries: Vec<&ThreadEntry> = file.entries.iter().collect();

    if let Some(since_line) = since {
        entries.retain(|e| e.line_number() > since_line);
    }

    if let Some(lim) = limit {
        let start = entries.len().saturating_sub(lim);
        entries = entries[start..].to_vec();
    }

    let json_entries: Vec<serde_json::Value> =
        entries.iter().map(|entry| entry_to_json(entry)).collect();

    Response::success(serde_json::json!({
        "channel": channel,
        "entries": json_entries,
        "archived": is_archived,
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
    let meta_path = users_dir.join(format!("{}.meta.yaml", handler));

    // If already exists, ensure user is in memory list and return success
    if meta_path.exists() {
        let mut users = state.users.write().await;
        if !users.contains(&handler) {
            users.push(handler.clone());
            users.sort();
        }
        return Response::success(serde_json::json!({
            "handler": handler,
            "exists": true
        }));
    }

    // Create meta file
    let meta = UserMeta {
        display_name,
        role,
        introduction,
    };
    let meta_str = serde_yaml::to_string(&meta).unwrap();

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
    let _ = state.git_storage.add_and_commit_as(
        &[&format!("users/{}.meta.yaml", handler)],
        &format!("user: register @{}", handler),
        Some(&handler),
    );

    Response::success(serde_json::json!({
        "handler": handler,
        "exists": false
    }))
}

async fn handle_list_channels(state: SharedState) -> Response {
    let mut channels: Vec<serde_json::Value> = Vec::new();

    // 扫描 channels/*.meta.yaml — 读取 members 字段
    let ch_dir = state.repo_root.join("channels");
    if ch_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&ch_dir) {
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                if fname.ends_with(".meta.yaml") {
                    let name = fname.trim_end_matches(".meta.yaml").to_string();
                    let members: Vec<String> = std::fs::read_to_string(entry.path())
                        .ok()
                        .and_then(|c| serde_yaml::from_str::<ChannelMeta>(&c).ok())
                        .map(|m| m.members)
                        .unwrap_or_default();
                    channels.push(serde_json::json!({
                        "name": name,
                        "kind": "channel",
                        "members": members,
                    }));
                }
            }
        }
    }

    // 扫描 dm/*.thread — 从文件名提取双方 handler 作为 members
    let dm_dir = state.repo_root.join("dm");
    if dm_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&dm_dir) {
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                if fname.ends_with(".thread") {
                    let name = fname.trim_end_matches(".thread").to_string();
                    let members: Vec<String> = name.split("--").map(|s| s.to_string()).collect();
                    channels.push(serde_json::json!({
                        "name": name,
                        "kind": "dm",
                        "members": members,
                    }));
                }
            }
        }
    }

    channels.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Response::success(serde_json::json!({ "channels": channels }))
}

async fn handle_list_archived_channels(state: SharedState) -> Response {
    let mut channels: Vec<serde_json::Value> = Vec::new();

    // 扫描 archive/channels/*.meta.yaml — 读取 members 字段
    let arch_ch_dir = state.repo_root.join("archive").join("channels");
    if arch_ch_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&arch_ch_dir) {
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                if fname.ends_with(".meta.yaml") {
                    let name = fname.trim_end_matches(".meta.yaml").to_string();
                    let members: Vec<String> = std::fs::read_to_string(entry.path())
                        .ok()
                        .and_then(|c| serde_yaml::from_str::<ChannelMeta>(&c).ok())
                        .map(|m| m.members)
                        .unwrap_or_default();
                    channels.push(serde_json::json!({
                        "name": name,
                        "kind": "archived_channel",
                        "members": members,
                    }));
                }
            }
        }
    }

    channels.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Response::success(serde_json::json!({ "channels": channels }))
}

async fn handle_list_users(state: SharedState) -> Response {
    let users = state.users.read().await;
    let mut sorted: Vec<String> = users.clone();
    sorted.sort();
    Response::success(serde_json::json!({ "users": sorted }))
}

async fn handle_get_thread(state: SharedState, channel: String, line_number: u64) -> Response {
    if let Err(e) = ChannelName::new(&channel) {
        return Response::error(format!("invalid channel name: {}", e));
    }
    let thread_path = state
        .repo_root
        .join("channels")
        .join(format!("{}.thread", channel));
    let thread_path = if !thread_path.exists() {
        let archive_path = state
            .repo_root
            .join("archive")
            .join("channels")
            .join(format!("{}.thread", channel));
        if archive_path.exists() {
            archive_path
        } else {
            thread_path
        }
    } else {
        thread_path
    };
    let content = std::fs::read_to_string(&thread_path).unwrap_or_default();
    let file = match parse_thread(&content) {
        Ok(f) => f,
        Err(e) => return Response::error(format!("parse error: {}", e)),
    };

    // Walk `point_to` upward from the clicked line to find the true root
    // (the topmost ancestor whose point_to == 0). Without this, clicking a
    // reply mid-chain would show that reply as the thread's root and hide
    // every earlier ancestor.
    let by_line: std::collections::HashMap<u64, &_> = file
        .entries
        .iter()
        .map(|e| (e.line_number(), e))
        .collect();
    let mut root_line = line_number;
    let mut seen_up = std::collections::HashSet::new();
    while let Some(entry) = by_line.get(&root_line) {
        if !seen_up.insert(root_line) {
            break; // cycle guard — malformed file
        }
        let parent = entry.point_to();
        if parent == 0 || !by_line.contains_key(&parent) {
            break;
        }
        root_line = parent;
    }

    // Collect the root entry and all descendants (entries pointing to it, recursively)
    let mut thread_entries: Vec<serde_json::Value> = Vec::new();
    let mut stack = vec![root_line];
    let mut visited = std::collections::HashSet::new();

    while let Some(target) = stack.pop() {
        if !visited.insert(target) {
            continue;
        }
        for entry in &file.entries {
            if entry.line_number() == target || entry.point_to() == target {
                thread_entries.push(entry_to_json(entry));
                if entry.line_number() != target {
                    stack.push(entry.line_number());
                }
            }
        }
    }

    // Sort by line number
    thread_entries.sort_by(|a, b| {
        a["line_number"]
            .as_u64()
            .unwrap()
            .cmp(&b["line_number"].as_u64().unwrap())
    });

    // Deduplicate (an entry could match both by line_number and point_to)
    thread_entries.dedup_by(|a, b| a["line_number"] == b["line_number"]);

    Response::success(serde_json::json!({
        "channel": channel,
        "root_line": root_line,
        "entries": thread_entries,
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

async fn handle_poll(state: SharedState, since: Option<String>) -> Response {
    // Use @{upstream} (current branch's tracking ref) when available, else HEAD.
    let ref_name = if state.git_storage.has_remote()
        && state.git_storage.rev_parse("@{upstream}").is_ok()
    {
        "@{upstream}"
    } else {
        "HEAD"
    };

    // Get current commit hash
    let current_commit = match state.git_storage.rev_parse(ref_name) {
        Ok(hash) => hash,
        Err(e) => return Response::error(format!("failed to get commit: {}", e)),
    };

    // No cursor → start from parent commit so the first poll picks up recent messages
    let since_commit = match since {
        Some(s) if !s.is_empty() => s,
        _ => {
            match state.git_storage.rev_parse(&format!("{}~1", ref_name)) {
                Ok(parent) => parent,
                Err(_) => {
                    // No parent (initial commit) — return sync point with no changes
                    return Response::success(serde_json::json!({
                        "commit_id": current_commit,
                        "changes": [],
                    }));
                }
            }
        }
    };

    // Validate commit hash format
    if since_commit.len() != 40 || !since_commit.chars().all(|c| c.is_ascii_hexdigit()) {
        return Response::error("invalid commit hash: expected 40-character hex string");
    }

    // Same cursor → no changes
    if since_commit == current_commit {
        return Response::success(serde_json::json!({
            "commit_id": current_commit,
            "changes": [],
        }));
    }

    // Compute diff
    let diff = match state.git_storage.diff_range(&since_commit, &current_commit) {
        Ok(d) => d,
        Err(e) => return Response::error(format!("diff failed (commit may not exist): {}", e)),
    };

    // Parse changed files into entries
    let mut changes: Vec<serde_json::Value> = Vec::new();

    let current_user_snapshot = state.current_user.read().await.clone();
    let is_admin = state.is_admin.load(std::sync::atomic::Ordering::SeqCst);
    let is_guest = state.is_guest.load(std::sync::atomic::Ordering::SeqCst);
    let skip_filter = is_admin || is_guest;

    // Step 1: Build channel membership cache (admin skips — never checked)
    let mut channel_membership: HashMap<String, bool> = HashMap::new();
    if !skip_filter {
        for (path, _) in &diff {
            let path_str = path.to_string_lossy();
            if let Some(ch_name) = path_str.strip_prefix("channels/").and_then(|s| {
                s.strip_suffix(".thread")
                    .or_else(|| s.strip_suffix(".meta.yaml"))
            }) {
                if channel_membership.contains_key(ch_name) {
                    continue;
                }
                let meta_path = state
                    .repo_root
                    .join("channels")
                    .join(format!("{}.meta.yaml", ch_name));
                let is_member = if let Ok(content) = std::fs::read_to_string(&meta_path) {
                    if let Ok(meta) = serde_yaml::from_str::<ChannelMeta>(&content) {
                        if meta.members.is_empty() {
                            true // Legacy: no members list = everyone has access
                        } else {
                            current_user_snapshot
                                .as_ref()
                                .map_or(false, |me| meta.members.contains(me))
                        }
                    } else {
                        true
                    }
                } else {
                    true
                };
                channel_membership.insert(ch_name.to_string(), is_member);
            }
        }
    } // end if !skip_filter

    // Step 2: Process diff entries with membership filter
    for (path, added_content) in &diff {
        let path_str = path.to_string_lossy();

        let (channel, kind) = if let Some(name) = path_str.strip_prefix("channels/") {
            if let Some(ch_name) = name.strip_suffix(".thread") {
                (ch_name.to_string(), "channel")
            } else if let Some(ch_name) = name.strip_suffix(".meta.yaml") {
                // Meta change — only push if user is (now) a member
                if !skip_filter && !channel_membership.get(ch_name).copied().unwrap_or(true) {
                    continue;
                }
                changes.push(serde_json::json!({
                    "channel": ch_name,
                    "kind": "channel_meta",
                    "entries": [],
                }));
                continue;
            } else {
                continue;
            }
        } else if let Some(name) = path_str.strip_prefix("dm/") {
            let name = name.strip_suffix(".thread").unwrap_or(name);
            (format!("dm:{}", name.replace("--", ",")), "dm")
        } else {
            continue;
        };

        // Channel membership filter
        if kind == "channel" && !skip_filter {
            if !channel_membership.get(&channel).copied().unwrap_or(true) {
                continue;
            }
        }

        // DM visibility filter — skip DMs not involving current user
        if kind == "dm" && !skip_filter {
            if let Some(stem) = path_str
                .strip_prefix("dm/")
                .and_then(|s| s.strip_suffix(".thread"))
            {
                if let Some((a, b)) = parse_dm_filename(stem) {
                    match &current_user_snapshot {
                        Some(me) if me == a || me == b => { /* allowed */ }
                        _ => continue,
                    }
                }
            }
        }

        // Parse added lines as entries (both messages and events)
        let parsed = match parse_thread(added_content) {
            Ok(f) => f,
            Err(e) => {
                warn!("poll: failed to parse diff for {}: {}", path_str, e);
                continue;
            }
        };

        if parsed.entries.is_empty() {
            continue;
        }

        let entries: Vec<serde_json::Value> = parsed
            .entries
            .iter()
            .map(|entry| entry_to_json(entry))
            .collect();

        changes.push(serde_json::json!({
            "channel": channel,
            "kind": kind,
            "entries": entries,
        }));
    }

    Response::success(serde_json::json!({
        "commit_id": current_commit,
        "changes": changes,
    }))
}

async fn handle_join_channel(
    state: SharedState,
    channel: String,
    targets: Vec<String>,
    author: String,
) -> Response {
    write_channel_event(state, channel, targets, author, "join").await
}

async fn handle_leave_channel(
    state: SharedState,
    channel: String,
    targets: Vec<String>,
    author: String,
) -> Response {
    write_channel_event(state, channel, targets, author, "leave").await
}

const MAX_PUSH_RETRIES: u32 = 3;

async fn handle_create_channel(
    state: SharedState,
    name: String,
    display_name: Option<String>,
    introduction: Option<String>,
    author: String,
    invitees: Vec<String>,
) -> Response {
    // 1. Validate author
    let handler = match Handler::new(&author) {
        Ok(h) => h,
        Err(e) => return Response::error(format!("invalid author: {}", e)),
    };
    {
        let users = state.users.read().await;
        if !users.contains(&author) {
            return Response::error(format!("unknown user: {}", author));
        }
        // Validate all invitees before any I/O
        for invitee in &invitees {
            if Handler::new(invitee).is_err() {
                return Response::error(format!("invalid invitee handle: {}", invitee));
            }
            if !users.contains(invitee) {
                return Response::error(format!("invitee '{}' is not registered", invitee));
            }
        }
    }

    // 2. Validate channel name
    let channel_name = match ChannelName::new(&name) {
        Ok(n) => n,
        Err(e) => return Response::error(format!("invalid channel name: {}", e)),
    };

    // 3. Check channel doesn't already exist
    let channels_dir = state.repo_root.join("channels");
    let meta_path = channels_dir.join(format!("{}.meta.yaml", channel_name));
    if meta_path.exists() {
        return Response::error(format!("channel '{}' already exists", name));
    }
    let archive_meta = state
        .repo_root
        .join("archive")
        .join("channels")
        .join(format!("{}.meta.yaml", channel_name));
    if archive_meta.exists() {
        return Response::error(format!("channel '{}' exists in archive", name));
    }

    // 4. Create channels/ dir
    if let Err(e) = std::fs::create_dir_all(&channels_dir) {
        return Response::error(format!("failed to create channels dir: {}", e));
    }

    // 5. Build members list: author first, then invitees in order, deduped
    let mut members: Vec<String> = vec![author.clone()];
    for invitee in invitees {
        if !members.contains(&invitee) {
            members.push(invitee);
        }
    }

    // 6. Write meta.yaml
    let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let meta = ChannelMeta {
        display_name: display_name.unwrap_or_else(|| name.clone()),
        created_by: author.clone(),
        created_at: now.clone(),
        introduction: introduction.unwrap_or_default(),
        members,
    };
    let meta_str = serde_yaml::to_string(&meta).unwrap();
    if let Err(e) = std::fs::write(&meta_path, &meta_str) {
        return Response::error(format!("failed to write channel meta: {}", e));
    }

    // 7. Write .thread with join event
    // Only the creator's join event is recorded. Invitees appear in members (meta.yaml)
    // but do NOT receive a synthetic thread event — by design, see needs doc.
    let thread_path = channels_dir.join(format!("{}.thread", channel_name));
    let join_line = format_event(1, &handler, &now, "join", &serde_json::json!({}));
    if let Err(e) = std::fs::write(&thread_path, &join_line) {
        return Response::error(format!("failed to write channel thread: {}", e));
    }

    // 8. Commit
    let meta_rel = format!("channels/{}.meta.yaml", channel_name);
    let thread_rel = format!("channels/{}.thread", channel_name);
    let commit_msg = format!("channel: create #{} by @{}", name, author);
    if let Err(e) = state
        .git_storage
        .add_and_commit_as(&[&meta_rel, &thread_rel], &commit_msg, Some(&author))
    {
        return Response::error(format!("create_channel commit failed: {}", e));
    }

    // 9. Push with retry (skip if no remote)
    if state.git_storage.has_remote() {
        let mut pushed = false;
        for attempt in 1..=MAX_PUSH_RETRIES {
            match state.git_storage.push() {
                Ok(()) => {
                    pushed = true;
                    break;
                }
                Err(GitError::PushConflict) => {
                    warn!(
                        "create_channel: push conflict (attempt {}/{}), rebasing",
                        attempt, MAX_PUSH_RETRIES
                    );
                    if let Err(e) = state.git_storage.fetch() {
                        return Response::error(format!("create_channel fetch failed: {}", e));
                    }
                    if let Err(e) = state.git_storage.rebase_onto_origin() {
                        return Response::error(format!("create_channel rebase failed: {}", e));
                    }
                }
                Err(e) => {
                    return Response::error(format!("create_channel push failed: {}", e));
                }
            }
        }
        if !pushed {
            return Response::error(format!(
                "create_channel: push still conflicting after {} retries",
                MAX_PUSH_RETRIES
            ));
        }
    }

    info!("channel '{}' created by @{}", name, author);

    // 10. Return success
    Response::success(serde_json::json!({
        "channel": name,
        "created_by": author,
    }))
}

async fn handle_archive_channel(
    state: SharedState,
    channel: String,
    author: String,
) -> Response {
    // 1. Validate channel name
    let channel_name = match ChannelName::new(&channel) {
        Ok(n) => n,
        Err(e) => return Response::error(format!("invalid channel name: {}", e)),
    };

    // 2. Validate author is registered
    {
        let users = state.users.read().await;
        if !users.contains(&author) {
            return Response::error(format!("unknown user: {}", author));
        }
    }

    // 3. Read channel meta, confirm channel exists
    let meta_path = state
        .repo_root
        .join(format!("channels/{}.meta.yaml", channel_name));
    let meta_str = match std::fs::read_to_string(&meta_path) {
        Ok(s) => s,
        Err(_) => {
            return Response::error(format!("channel '{}' does not exist", channel));
        }
    };
    let meta: ChannelMeta = match serde_yaml::from_str(&meta_str) {
        Ok(m) => m,
        Err(e) => return Response::error(format!("failed to parse channel meta: {}", e)),
    };

    // 4. Check permission: only creator can archive
    if meta.created_by != author {
        return Response::error("only channel creator can archive");
    }

    // 5. Create archive/channels/ directory
    let archive_dir = state.repo_root.join("archive/channels");
    if let Err(e) = std::fs::create_dir_all(&archive_dir) {
        return Response::error(format!("failed to create archive dir: {}", e));
    }

    // 6. git mv both files to archive/channels/
    let thread_from = format!("channels/{}.thread", channel_name);
    let thread_to = format!("archive/channels/{}.thread", channel_name);
    let meta_from = format!("channels/{}.meta.yaml", channel_name);
    let meta_to = format!("archive/channels/{}.meta.yaml", channel_name);

    if let Err(e) = state.git_storage.mv(&thread_from, &thread_to) {
        return Response::error(format!("git mv thread failed: {}", e));
    }
    if let Err(e) = state.git_storage.mv(&meta_from, &meta_to) {
        let _ = state.git_storage.mv(&thread_to, &thread_from);
        return Response::error(format!("git mv meta failed: {}", e));
    }

    // 7. git add + commit
    let commit_msg = format!("archive: #{} by @{}", channel, author);
    if let Err(e) = state
        .git_storage
        .add_and_commit_as(&[&thread_to, &meta_to], &commit_msg, Some(&author))
    {
        return Response::error(format!("archive commit failed: {}", e));
    }

    // 8. Push with retry
    if state.git_storage.has_remote() {
        let mut pushed = false;
        for attempt in 1..=MAX_PUSH_RETRIES {
            match state.git_storage.push() {
                Ok(()) => {
                    pushed = true;
                    break;
                }
                Err(GitError::PushConflict) => {
                    warn!(
                        "archive_channel: push conflict (attempt {}/{}), rebasing",
                        attempt, MAX_PUSH_RETRIES
                    );
                    if let Err(e) = state.git_storage.fetch() {
                        return Response::error(format!("archive_channel fetch failed: {}", e));
                    }
                    if let Err(e) = state.git_storage.rebase_onto_origin() {
                        return Response::error(format!("archive_channel rebase failed: {}", e));
                    }
                }
                Err(e) => {
                    return Response::error(format!("archive_channel push failed: {}", e));
                }
            }
        }
        if !pushed {
            return Response::error(format!(
                "archive_channel: push still conflicting after {} retries",
                MAX_PUSH_RETRIES
            ));
        }
    }

    // 9. Remove channel from thread_cache
    state.thread_cache.write().await.remove(&channel);

    info!("channel '{}' archived by @{}", channel, author);

    // 10. Return success
    Response::success(serde_json::json!({
        "channel": channel,
        "archived_by": author,
    }))
}

async fn write_channel_event(
    state: SharedState,
    channel: String,
    targets: Vec<String>,
    author: String,
    event_type: &str,
) -> Response {
    // Validate channel name
    if let Err(e) = ChannelName::new(&channel) {
        return Response::error(format!("invalid channel name: {}", e));
    }

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

    // Validate target handler formats
    for t in &targets {
        if let Err(e) = Handler::new(t) {
            return Response::error(format!("invalid target: {}", e));
        }
    }

    // Read channel meta.yaml
    let meta_path = state
        .repo_root
        .join("channels")
        .join(format!("{}.meta.yaml", channel));
    let mut channel_meta: ChannelMeta = if meta_path.exists() {
        match std::fs::read_to_string(&meta_path) {
            Ok(content) => match serde_yaml::from_str(&content) {
                Ok(m) => m,
                Err(e) => return Response::error(format!("failed to parse channel meta: {}", e)),
            },
            Err(e) => return Response::error(format!("failed to read channel meta: {}", e)),
        }
    } else {
        let archive_meta = state
            .repo_root
            .join("archive")
            .join("channels")
            .join(format!("{}.meta.yaml", channel));
        if archive_meta.exists() {
            return Response::error(format!("channel '{}' is archived", channel));
        }
        return Response::error(format!("channel '{}' does not exist", channel));
    };

    let current_members: Vec<&str> = channel_meta.members.iter().map(|s| s.as_str()).collect();
    let target_refs: Vec<&str> = targets.iter().map(|s| s.as_str()).collect();

    // Validate join or leave rules
    match event_type {
        "join" => {
            if let Err(e) =
                im_rules::validate_join(&author, &target_refs, &user_refs, &current_members)
            {
                return Response::error(format!("join validation failed: {}", e));
            }
        }
        "leave" => {
            if let Err(e) =
                im_rules::validate_leave(&author, &target_refs, &user_refs, &current_members)
            {
                return Response::error(format!("leave validation failed: {}", e));
            }
        }
        _ => return Response::error(format!("unknown event type: {}", event_type)),
    }

    // Read .thread for next line number
    let thread_path = state
        .repo_root
        .join("channels")
        .join(format!("{}.thread", channel));
    let existing = std::fs::read_to_string(&thread_path).unwrap_or_default();
    let existing_file = match parse_thread(&existing) {
        Ok(f) => f,
        Err(e) => return Response::error(format!("failed to parse thread: {}", e)),
    };
    let next_line = existing_file.last_line_number() + 1;

    // Build event meta and format
    let meta = if targets.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::json!({"targets": targets})
    };
    let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let new_content = format_event(next_line, &handler, &now, event_type, &meta);

    // Append to .thread
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

    // Update meta.yaml members
    let affected: Vec<String> = if targets.is_empty() {
        vec![author.clone()]
    } else {
        targets.clone()
    };

    match event_type {
        "join" => {
            for user in &affected {
                if !channel_meta.members.contains(user) {
                    channel_meta.members.push(user.clone());
                }
            }
            channel_meta.members.sort();
        }
        "leave" => {
            channel_meta.members.retain(|m| !affected.contains(m));
        }
        _ => {}
    }

    let meta_str = serde_yaml::to_string(&channel_meta).unwrap();
    if let Err(e) = std::fs::write(&meta_path, &meta_str) {
        return Response::error(format!("failed to write channel meta: {}", e));
    }

    // Git commit both files
    let thread_rel = format!("channels/{}.thread", channel);
    let meta_rel = format!("channels/{}.meta.yaml", channel);
    let commit_msg = format!("event: @{} {} {}", author, event_type, channel);
    let commit_status = match state.git_storage.add_and_commit_as(
        &[&thread_rel, &meta_rel],
        &commit_msg,
        Some(&author),
    ) {
        Ok(()) => "committed",
        Err(e) => {
            warn!(
                "git commit failed for {} event in {}: {}",
                event_type, channel, e
            );
            "written"
        }
    };

    // Broadcast MembershipChanged event
    let _ = state.event_tx.send(Event::MembershipChanged {
        channel: channel.clone(),
        event_type: event_type.to_string(),
        author: author.clone(),
        targets: affected.clone(),
    });

    // Invalidate thread cache
    state.thread_cache.write().await.remove(&channel);

    info!(
        "{} event in {} by @{} at L{:06} (targets: {:?})",
        event_type, channel, author, next_line, affected
    );
    Response::success(serde_json::json!({
        "channel": channel,
        "event_type": event_type,
        "author": author,
        "targets": affected,
        "line_number": next_line,
        "status": commit_status,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use gitim_core::types::config::Config;
    use std::sync::Arc;
    use tokio::sync::broadcast;

    fn setup_test_state(tmp: &std::path::Path) -> SharedState {
        let remote = tmp.join("remote.git");
        std::fs::create_dir_all(&remote).unwrap();
        std::process::Command::new("git")
            .args(["init", "--bare"])
            .current_dir(&remote)
            .output()
            .unwrap();

        let repo = tmp.join("repo");
        std::process::Command::new("git")
            .args(["clone", remote.to_str().unwrap(), repo.to_str().unwrap()])
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(&repo)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(&repo)
            .output()
            .unwrap();

        // Initial commit so main branch exists
        std::fs::write(repo.join(".keep"), "").unwrap();
        std::process::Command::new("git")
            .args(["add", ".keep"])
            .current_dir(&repo)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(&repo)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["push", "-u", "origin", "HEAD"])
            .current_dir(&repo)
            .output()
            .unwrap();

        let (event_tx, _) = broadcast::channel(16);
        Arc::new(AppState::new(repo, Config::default(), event_tx, None))
    }

    /// Register a user by creating meta.yaml and adding to in-memory user list.
    async fn register_test_user(state: &SharedState, handler: &str) {
        let users_dir = state.repo_root.join("users");
        std::fs::create_dir_all(&users_dir).unwrap();
        let meta = UserMeta {
            display_name: handler.to_string(),
            role: "member".to_string(),
            introduction: "test user".to_string(),
        };
        std::fs::write(
            users_dir.join(format!("{}.meta.yaml", handler)),
            serde_yaml::to_string(&meta).unwrap(),
        )
        .unwrap();
        let rel = format!("users/{}.meta.yaml", handler);
        let _ = state
            .git_storage
            .add_and_commit(&[&rel], &format!("user: register @{}", handler));
        let mut users = state.users.write().await;
        if !users.contains(&handler.to_string()) {
            users.push(handler.to_string());
            users.sort();
        }
    }

    /// Create a channel with meta.yaml and empty .thread file.
    fn create_test_channel(state: &SharedState, name: &str, created_by: &str) {
        let ch_dir = state.repo_root.join("channels");
        std::fs::create_dir_all(&ch_dir).unwrap();
        let meta = ChannelMeta {
            display_name: name.to_string(),
            created_by: created_by.to_string(),
            created_at: "20260323T000000Z".to_string(),
            introduction: "test channel".to_string(),
            members: Vec::new(),
        };
        std::fs::write(
            ch_dir.join(format!("{}.meta.yaml", name)),
            serde_yaml::to_string(&meta).unwrap(),
        )
        .unwrap();
        std::fs::write(ch_dir.join(format!("{}.thread", name)), "").unwrap();
        let meta_rel = format!("channels/{}.meta.yaml", name);
        let thread_rel = format!("channels/{}.thread", name);
        let _ = state.git_storage.add_and_commit(
            &[&meta_rel, &thread_rel],
            &format!("init: channel {}", name),
        );
    }

    #[tokio::test]
    async fn test_join_channel_self() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        register_test_user(&state, "alice").await;
        create_test_channel(&state, "general", "alice");

        let resp = handle_request(
            Request::JoinChannel {
                channel: "general".to_string(),
                targets: vec![],
                author: Some("alice".to_string()),
            },
            state.clone(),
        )
        .await;
        assert!(resp.ok, "join failed: {:?}", resp.error);

        // Verify .thread has join event
        let thread =
            std::fs::read_to_string(state.repo_root.join("channels/general.thread")).unwrap();
        assert!(thread.contains("[E:join]"), "thread missing join event");
        assert!(thread.contains("@alice"), "thread missing author");

        // Verify meta.yaml has alice in members
        let meta_str =
            std::fs::read_to_string(state.repo_root.join("channels/general.meta.yaml")).unwrap();
        let meta: ChannelMeta = serde_yaml::from_str(&meta_str).unwrap();
        assert!(
            meta.members.contains(&"alice".to_string()),
            "alice not in members"
        );
    }

    #[tokio::test]
    async fn test_join_channel_pull_others() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        register_test_user(&state, "alice").await;
        register_test_user(&state, "bob").await;
        create_test_channel(&state, "general", "alice");

        // Alice joins first
        let resp1 = handle_request(
            Request::JoinChannel {
                channel: "general".to_string(),
                targets: vec![],
                author: Some("alice".to_string()),
            },
            state.clone(),
        )
        .await;
        assert!(resp1.ok, "alice join failed: {:?}", resp1.error);

        // Alice pulls bob in
        let resp2 = handle_request(
            Request::JoinChannel {
                channel: "general".to_string(),
                targets: vec!["bob".to_string()],
                author: Some("alice".to_string()),
            },
            state.clone(),
        )
        .await;
        assert!(resp2.ok, "pull bob failed: {:?}", resp2.error);

        // Verify both in members
        let meta_str =
            std::fs::read_to_string(state.repo_root.join("channels/general.meta.yaml")).unwrap();
        let meta: ChannelMeta = serde_yaml::from_str(&meta_str).unwrap();
        assert!(
            meta.members.contains(&"alice".to_string()),
            "alice not in members"
        );
        assert!(
            meta.members.contains(&"bob".to_string()),
            "bob not in members"
        );

        // Verify thread has 2 events
        let thread =
            std::fs::read_to_string(state.repo_root.join("channels/general.thread")).unwrap();
        let join_count = thread.matches("[E:join]").count();
        assert_eq!(join_count, 2, "expected 2 join events, got {}", join_count);
    }

    #[tokio::test]
    async fn test_leave_channel_self() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        register_test_user(&state, "alice").await;
        create_test_channel(&state, "general", "alice");

        // Alice joins
        let resp1 = handle_request(
            Request::JoinChannel {
                channel: "general".to_string(),
                targets: vec![],
                author: Some("alice".to_string()),
            },
            state.clone(),
        )
        .await;
        assert!(resp1.ok, "join failed: {:?}", resp1.error);

        // Alice leaves
        let resp2 = handle_request(
            Request::LeaveChannel {
                channel: "general".to_string(),
                targets: vec![],
                author: Some("alice".to_string()),
            },
            state.clone(),
        )
        .await;
        assert!(resp2.ok, "leave failed: {:?}", resp2.error);

        // Verify meta.yaml members is empty
        let meta_str =
            std::fs::read_to_string(state.repo_root.join("channels/general.meta.yaml")).unwrap();
        let meta: ChannelMeta = serde_yaml::from_str(&meta_str).unwrap();
        assert!(
            meta.members.is_empty(),
            "members should be empty, got: {:?}",
            meta.members
        );

        // Verify thread has both join and leave events
        let thread =
            std::fs::read_to_string(state.repo_root.join("channels/general.thread")).unwrap();
        assert!(thread.contains("[E:join]"), "thread missing join event");
        assert!(thread.contains("[E:leave]"), "thread missing leave event");
    }

    #[tokio::test]
    async fn test_read_returns_entries_with_type() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        register_test_user(&state, "alice").await;
        create_test_channel(&state, "general", "alice");

        // Alice joins (creates an event entry)
        let join_resp = handle_request(
            Request::JoinChannel {
                channel: "general".to_string(),
                targets: vec![],
                author: Some("alice".to_string()),
            },
            state.clone(),
        )
        .await;
        assert!(join_resp.ok, "join failed: {:?}", join_resp.error);

        // Alice sends a message
        let send_resp = handle_request(
            Request::Send {
                channel: "general".to_string(),
                body: "hello".to_string(),
                reply_to: None,
                author: Some("alice".to_string()),
            },
            state.clone(),
        )
        .await;
        assert!(send_resp.ok, "send failed: {:?}", send_resp.error);

        // Read the channel
        let read_resp = handle_request(
            Request::Read {
                channel: "general".to_string(),
                limit: None,
                since: None,
            },
            state.clone(),
        )
        .await;
        assert!(read_resp.ok, "read failed: {:?}", read_resp.error);

        let data = read_resp.data.unwrap();
        let entries = data["entries"].as_array().expect("expected entries array");
        assert_eq!(
            entries.len(),
            2,
            "expected 2 entries, got {}",
            entries.len()
        );

        // First entry is the join event
        assert_eq!(entries[0]["type"], "event");
        assert_eq!(entries[0]["event_type"], "join");
        assert_eq!(entries[0]["author"], "alice");

        // Second entry is the message
        assert_eq!(entries[1]["type"], "message");
        assert_eq!(entries[1]["body"], "hello");
        assert_eq!(entries[1]["author"], "alice");

        // Verify "messages" key is absent
        assert!(
            data.get("messages").is_none(),
            "should not have 'messages' key"
        );
    }

    #[tokio::test]
    async fn test_poll_filters_non_member_channels() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        register_test_user(&state, "alice").await;
        register_test_user(&state, "bob").await;
        create_test_channel(&state, "general", "alice");
        create_test_channel(&state, "random", "alice");

        // Bob joins random so its members list is non-empty (not legacy/open)
        let bob_join = handle_request(
            Request::JoinChannel {
                channel: "random".to_string(),
                targets: vec![],
                author: Some("bob".to_string()),
            },
            state.clone(),
        )
        .await;
        assert!(bob_join.ok, "bob join random failed: {:?}", bob_join.error);

        // Push initial state to origin
        state.git_storage.push().ok();

        // Alice joins general only
        let join_resp = handle_request(
            Request::JoinChannel {
                channel: "general".to_string(),
                targets: vec![],
                author: Some("alice".to_string()),
            },
            state.clone(),
        )
        .await;
        assert!(join_resp.ok, "join general failed: {:?}", join_resp.error);

        // Set current_user to alice
        {
            let mut cu = state.current_user.write().await;
            *cu = Some("alice".to_string());
        }

        // Get cursor before changes
        state.git_storage.push().ok();
        let poll_before = handle_request(Request::Poll { since: None }, state.clone()).await;
        let cursor = poll_before.data.unwrap()["commit_id"]
            .as_str()
            .unwrap()
            .to_string();

        // Alice sends to general (she is a member) — should succeed
        let send_general = handle_request(
            Request::Send {
                channel: "general".to_string(),
                body: "hello general".to_string(),
                reply_to: None,
                author: Some("alice".to_string()),
            },
            state.clone(),
        )
        .await;
        assert!(
            send_general.ok,
            "send general failed: {:?}",
            send_general.error
        );

        // Alice sends to random (she is NOT a member) — should be rejected
        let send_random = handle_request(
            Request::Send {
                channel: "random".to_string(),
                body: "hello random".to_string(),
                reply_to: None,
                author: Some("alice".to_string()),
            },
            state.clone(),
        )
        .await;
        assert!(
            !send_random.ok,
            "send random should have been rejected"
        );
        assert!(
            send_random
                .error
                .as_ref()
                .unwrap()
                .contains("not a member"),
            "expected 'not a member' error, got: {:?}",
            send_random.error
        );

        // Push so poll can see changes
        state.git_storage.push().ok();

        // Poll with cursor
        let poll_resp = handle_request(
            Request::Poll {
                since: Some(cursor),
            },
            state.clone(),
        )
        .await;
        assert!(poll_resp.ok, "poll failed: {:?}", poll_resp.error);

        let data = poll_resp.data.unwrap();
        let changes = data["changes"].as_array().unwrap();

        // Should only contain general-related changes, not random
        let channel_names: Vec<&str> = changes
            .iter()
            .filter(|c| c["kind"] == "channel" || c["kind"] == "channel_meta")
            .filter_map(|c| c["channel"].as_str())
            .collect();
        assert!(
            channel_names.contains(&"general"),
            "general should be in poll results: {:?}",
            channel_names
        );
        assert!(
            !channel_names.contains(&"random"),
            "random should NOT be in poll results (not a member): {:?}",
            channel_names
        );
    }

    #[tokio::test]
    async fn test_poll_admin_bypass_returns_all_channels() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        register_test_user(&state, "alice").await;
        register_test_user(&state, "bob").await;
        create_test_channel(&state, "general", "alice");
        create_test_channel(&state, "random", "alice");

        // Bob joins random so its members list is non-empty
        let bob_join = handle_request(
            Request::JoinChannel {
                channel: "random".to_string(),
                targets: vec![],
                author: Some("bob".to_string()),
            },
            state.clone(),
        )
        .await;
        assert!(bob_join.ok, "bob join random failed: {:?}", bob_join.error);

        // Alice joins general only
        let join_resp = handle_request(
            Request::JoinChannel {
                channel: "general".to_string(),
                targets: vec![],
                author: Some("alice".to_string()),
            },
            state.clone(),
        )
        .await;
        assert!(join_resp.ok, "join general failed: {:?}", join_resp.error);

        // Set current_user to alice and enable admin mode
        {
            let mut cu = state.current_user.write().await;
            *cu = Some("alice".to_string());
        }
        state
            .is_admin
            .store(true, std::sync::atomic::Ordering::SeqCst);

        // Get cursor before changes
        state.git_storage.push().ok();
        let poll_before = handle_request(Request::Poll { since: None }, state.clone()).await;
        let cursor = poll_before.data.unwrap()["commit_id"]
            .as_str()
            .unwrap()
            .to_string();

        // Alice sends to general (she is a member)
        let send_general = handle_request(
            Request::Send {
                channel: "general".to_string(),
                body: "hello general".to_string(),
                reply_to: None,
                author: Some("alice".to_string()),
            },
            state.clone(),
        )
        .await;
        assert!(
            send_general.ok,
            "send general failed: {:?}",
            send_general.error
        );

        // Bob sends to random (he is a member)
        let send_random = handle_request(
            Request::Send {
                channel: "random".to_string(),
                body: "hello random".to_string(),
                reply_to: None,
                author: Some("bob".to_string()),
            },
            state.clone(),
        )
        .await;
        assert!(
            send_random.ok,
            "send random failed: {:?}",
            send_random.error
        );

        // Push so poll can see changes
        state.git_storage.push().ok();

        // Poll with cursor — admin should see ALL channels
        let poll_resp = handle_request(
            Request::Poll {
                since: Some(cursor),
            },
            state.clone(),
        )
        .await;
        assert!(poll_resp.ok, "poll failed: {:?}", poll_resp.error);

        let data = poll_resp.data.unwrap();
        let changes = data["changes"].as_array().unwrap();

        let channel_names: Vec<&str> = changes
            .iter()
            .filter(|c| c["kind"] == "channel" || c["kind"] == "channel_meta")
            .filter_map(|c| c["channel"].as_str())
            .collect();
        assert!(
            channel_names.contains(&"general"),
            "general should be in admin poll results: {:?}",
            channel_names
        );
        assert!(
            channel_names.contains(&"random"),
            "random SHOULD be in admin poll results (admin bypass): {:?}",
            channel_names
        );
    }

    #[tokio::test]
    async fn test_send_member_channel_succeeds() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        register_test_user(&state, "alice").await;
        create_test_channel(&state, "general", "alice");

        // Alice joins general
        let join_resp = handle_request(
            Request::JoinChannel {
                channel: "general".to_string(),
                targets: vec![],
                author: Some("alice".to_string()),
            },
            state.clone(),
        )
        .await;
        assert!(join_resp.ok, "join failed: {:?}", join_resp.error);

        // Alice sends to general — should succeed
        let send_resp = handle_request(
            Request::Send {
                channel: "general".to_string(),
                body: "hello from member".to_string(),
                reply_to: None,
                author: Some("alice".to_string()),
            },
            state.clone(),
        )
        .await;
        assert!(send_resp.ok, "send failed: {:?}", send_resp.error);
    }

    #[tokio::test]
    async fn test_send_non_member_channel_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        register_test_user(&state, "alice").await;
        register_test_user(&state, "bob").await;
        create_test_channel(&state, "general", "alice");

        // Bob joins so members list is non-empty
        let join_resp = handle_request(
            Request::JoinChannel {
                channel: "general".to_string(),
                targets: vec![],
                author: Some("bob".to_string()),
            },
            state.clone(),
        )
        .await;
        assert!(join_resp.ok, "bob join failed: {:?}", join_resp.error);

        // Alice sends to general — she is NOT a member
        let send_resp = handle_request(
            Request::Send {
                channel: "general".to_string(),
                body: "should be rejected".to_string(),
                reply_to: None,
                author: Some("alice".to_string()),
            },
            state.clone(),
        )
        .await;
        assert!(!send_resp.ok, "send should have been rejected");
        assert!(
            send_resp
                .error
                .as_ref()
                .unwrap()
                .contains("not a member"),
            "expected 'not a member' error, got: {:?}",
            send_resp.error
        );
    }

    #[tokio::test]
    async fn test_send_open_channel_succeeds() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        register_test_user(&state, "alice").await;
        // create_test_channel creates with empty members (open channel)
        create_test_channel(&state, "general", "alice");

        // Alice sends to general — open channel, should succeed
        let send_resp = handle_request(
            Request::Send {
                channel: "general".to_string(),
                body: "open channel message".to_string(),
                reply_to: None,
                author: Some("alice".to_string()),
            },
            state.clone(),
        )
        .await;
        assert!(send_resp.ok, "send failed: {:?}", send_resp.error);
    }

    #[tokio::test]
    async fn test_send_dm_participant_succeeds() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        register_test_user(&state, "alice").await;
        register_test_user(&state, "bob").await;

        // Alice sends DM to dm:alice,bob — she is a participant
        let send_resp = handle_request(
            Request::Send {
                channel: "dm:alice,bob".to_string(),
                body: "hey bob".to_string(),
                reply_to: None,
                author: Some("alice".to_string()),
            },
            state.clone(),
        )
        .await;
        assert!(send_resp.ok, "dm send failed: {:?}", send_resp.error);
    }

    #[tokio::test]
    async fn test_send_dm_non_participant_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        register_test_user(&state, "alice").await;
        register_test_user(&state, "bob").await;
        register_test_user(&state, "charlie").await;

        // Charlie sends to dm:alice,bob — he is NOT a participant
        let send_resp = handle_request(
            Request::Send {
                channel: "dm:alice,bob".to_string(),
                body: "sneaky message".to_string(),
                reply_to: None,
                author: Some("charlie".to_string()),
            },
            state.clone(),
        )
        .await;
        assert!(!send_resp.ok, "dm send should have been rejected");
        assert!(
            send_resp
                .error
                .as_ref()
                .unwrap()
                .contains("not a member"),
            "expected 'not a member' error, got: {:?}",
            send_resp.error
        );
    }

    #[tokio::test]
    async fn test_send_invalid_channel_name_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        register_test_user(&state, "alice").await;

        let resp = handle_request(
            Request::Send {
                channel: "../../etc/passwd".to_string(),
                body: "pwn".to_string(),
                reply_to: None,
                author: Some("alice".to_string()),
            },
            state.clone(),
        )
        .await;
        assert!(!resp.ok, "send to traversal path should be rejected");
        assert!(
            resp.error
                .as_ref()
                .unwrap()
                .contains("invalid channel name"),
            "expected 'invalid channel name' error, got: {:?}",
            resp.error
        );
    }

    #[tokio::test]
    async fn test_read_invalid_channel_name_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        register_test_user(&state, "alice").await;

        let resp = handle_request(
            Request::Read {
                channel: "../../etc/passwd".to_string(),
                limit: None,
                since: None,
            },
            state.clone(),
        )
        .await;
        assert!(!resp.ok, "read from traversal path should be rejected");
        assert!(
            resp.error
                .as_ref()
                .unwrap()
                .contains("invalid channel name"),
            "expected 'invalid channel name' error, got: {:?}",
            resp.error
        );
    }

    #[tokio::test]
    async fn test_send_nonexistent_channel_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        register_test_user(&state, "alice").await;
        // DO NOT create a channel — "nonexistent" has no meta.json

        let resp = handle_request(
            Request::Send {
                channel: "nonexistent".to_string(),
                body: "hello".to_string(),
                reply_to: None,
                author: Some("alice".to_string()),
            },
            state.clone(),
        )
        .await;
        assert!(!resp.ok, "send to nonexistent channel should be rejected");
        assert!(
            resp.error
                .as_ref()
                .unwrap()
                .contains("does not exist"),
            "expected 'does not exist' error, got: {:?}",
            resp.error
        );
    }

    #[tokio::test]
    async fn test_send_dm_unregistered_participant_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        register_test_user(&state, "alice").await;
        // ghost is NOT registered

        let resp = handle_request(
            Request::Send {
                channel: "dm:alice,ghost".to_string(),
                body: "hello ghost".to_string(),
                reply_to: None,
                author: Some("alice".to_string()),
            },
            state.clone(),
        )
        .await;
        assert!(
            !resp.ok,
            "send to DM with unregistered participant should be rejected"
        );
        assert!(
            resp.error
                .as_ref()
                .unwrap()
                .contains("not a registered user"),
            "expected 'not a registered user' error, got: {:?}",
            resp.error
        );
    }

    #[tokio::test]
    async fn test_create_channel_basic() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        register_test_user(&state, "alice").await;

        let resp = handle_request(
            Request::CreateChannel {
                name: "random".to_string(),
                display_name: Some("Random".to_string()),
                introduction: Some("A random channel".to_string()),
                author: Some("alice".to_string()),
                invitees: vec![],
            },
            state.clone(),
        )
        .await;
        assert!(resp.ok, "create_channel failed: {:?}", resp.error);

        let data = resp.data.unwrap();
        assert_eq!(data["channel"], "random");
        assert_eq!(data["created_by"], "alice");

        // Verify meta.yaml exists with correct content
        let meta_str =
            std::fs::read_to_string(state.repo_root.join("channels/random.meta.yaml")).unwrap();
        let meta: serde_yaml::Value = serde_yaml::from_str(&meta_str).unwrap();
        assert_eq!(meta["display_name"], "Random");
        assert_eq!(meta["created_by"], "alice");
        assert_eq!(meta["introduction"], "A random channel");
        let members = meta["members"].as_sequence().unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0], "alice");

        // Verify .thread exists with a join event
        let thread =
            std::fs::read_to_string(state.repo_root.join("channels/random.thread")).unwrap();
        assert!(thread.contains("[E:join]"), "thread missing join event");
        assert!(thread.contains("@alice"), "thread missing author");
    }

    #[tokio::test]
    async fn test_create_channel_already_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        register_test_user(&state, "alice").await;
        create_test_channel(&state, "general", "alice");

        let resp = handle_request(
            Request::CreateChannel {
                name: "general".to_string(),
                display_name: None,
                introduction: None,
                author: Some("alice".to_string()),
                invitees: vec![],
            },
            state.clone(),
        )
        .await;
        assert!(!resp.ok, "create_channel should fail for existing channel");
        assert!(
            resp.error.as_ref().unwrap().contains("already exists"),
            "expected 'already exists' error, got: {:?}",
            resp.error
        );
    }

    #[tokio::test]
    async fn test_create_channel_invalid_name() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        register_test_user(&state, "alice").await;

        let resp = handle_request(
            Request::CreateChannel {
                name: "../../bad".to_string(),
                display_name: None,
                introduction: None,
                author: Some("alice".to_string()),
                invitees: vec![],
            },
            state.clone(),
        )
        .await;
        assert!(!resp.ok, "create_channel should fail for invalid name");
        assert!(
            resp.error.as_ref().unwrap().contains("invalid channel name"),
            "expected 'invalid channel name' error, got: {:?}",
            resp.error
        );
    }

    #[tokio::test]
    async fn test_create_channel_then_send() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        register_test_user(&state, "alice").await;

        // Create channel
        let create_resp = handle_request(
            Request::CreateChannel {
                name: "dev".to_string(),
                display_name: None,
                introduction: None,
                author: Some("alice".to_string()),
                invitees: vec![],
            },
            state.clone(),
        )
        .await;
        assert!(create_resp.ok, "create_channel failed: {:?}", create_resp.error);

        // Send message to the new channel
        let send_resp = handle_request(
            Request::Send {
                channel: "dev".to_string(),
                body: "hello dev channel".to_string(),
                reply_to: None,
                author: Some("alice".to_string()),
            },
            state.clone(),
        )
        .await;
        assert!(send_resp.ok, "send to new channel failed: {:?}", send_resp.error);
    }

    // --- Task 2: create_channel invitees 测试（红阶段）---
    // Tests 1-4 are expected to FAIL until Task 3 implements invitees in handle_create_channel.
    // Test 5 is a regression guard and may PASS already.

    #[tokio::test]
    async fn test_create_channel_with_invitees() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        register_test_user(&state, "alice").await;
        register_test_user(&state, "bob").await;
        register_test_user(&state, "carol").await;

        let resp = handle_request(
            Request::CreateChannel {
                name: "team-alpha".to_string(),
                display_name: None,
                introduction: None,
                author: Some("alice".to_string()),
                invitees: vec!["bob".to_string(), "carol".to_string()],
            },
            state.clone(),
        )
        .await;
        assert!(resp.ok, "create_channel with invitees failed: {:?}", resp.error);

        let meta_str = std::fs::read_to_string(
            state.repo_root.join("channels/team-alpha.meta.yaml"),
        )
        .expect("meta.yaml should exist after successful create");
        let meta: ChannelMeta = serde_yaml::from_str(&meta_str).unwrap();

        assert_eq!(
            meta.members,
            vec!["alice".to_string(), "bob".to_string(), "carol".to_string()],
            "members should be [author, invitees...] in order; got: {:?}",
            meta.members
        );
    }

    #[tokio::test]
    async fn test_create_channel_invitee_dedup_duplicates() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        register_test_user(&state, "alice").await;
        register_test_user(&state, "bob").await;
        register_test_user(&state, "carol").await;

        let resp = handle_request(
            Request::CreateChannel {
                name: "dedup-test".to_string(),
                display_name: None,
                introduction: None,
                author: Some("alice".to_string()),
                invitees: vec![
                    "bob".to_string(),
                    "bob".to_string(),
                    "carol".to_string(),
                ],
            },
            state.clone(),
        )
        .await;
        assert!(resp.ok, "create_channel failed: {:?}", resp.error);

        let meta_str = std::fs::read_to_string(
            state.repo_root.join("channels/dedup-test.meta.yaml"),
        )
        .expect("meta.yaml should exist");
        let meta: ChannelMeta = serde_yaml::from_str(&meta_str).unwrap();

        assert_eq!(
            meta.members,
            vec!["alice".to_string(), "bob".to_string(), "carol".to_string()],
            "duplicate invitees should be deduped; got: {:?}",
            meta.members
        );
    }

    #[tokio::test]
    async fn test_create_channel_invitee_dedup_self() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        register_test_user(&state, "alice").await;
        register_test_user(&state, "bob").await;

        // invitees contains the author themselves — author should not appear twice
        let resp = handle_request(
            Request::CreateChannel {
                name: "self-dedup".to_string(),
                display_name: None,
                introduction: None,
                author: Some("alice".to_string()),
                invitees: vec!["alice".to_string(), "bob".to_string()],
            },
            state.clone(),
        )
        .await;
        assert!(resp.ok, "create_channel failed: {:?}", resp.error);

        let meta_str = std::fs::read_to_string(
            state.repo_root.join("channels/self-dedup.meta.yaml"),
        )
        .expect("meta.yaml should exist");
        let meta: ChannelMeta = serde_yaml::from_str(&meta_str).unwrap();

        assert_eq!(
            meta.members,
            vec!["alice".to_string(), "bob".to_string()],
            "author in invitees should not cause duplicate; got: {:?}",
            meta.members
        );
    }

    #[tokio::test]
    async fn test_create_channel_invitee_unregistered_rejects() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        register_test_user(&state, "alice").await;
        // "ghost" is intentionally NOT registered

        let resp = handle_request(
            Request::CreateChannel {
                name: "ghost-channel".to_string(),
                display_name: None,
                introduction: None,
                author: Some("alice".to_string()),
                invitees: vec!["ghost".to_string()],
            },
            state.clone(),
        )
        .await;

        assert!(
            !resp.ok,
            "create_channel should reject unregistered invitee; got ok=true"
        );
        let err = resp.error.as_deref().unwrap_or("");
        assert!(
            err.contains("ghost") || err.contains("not registered"),
            "error message should mention 'ghost' or 'not registered'; got: {:?}",
            resp.error
        );

        // Channel must NOT have been created (full transactional reject)
        assert!(
            !state
                .repo_root
                .join("channels/ghost-channel.meta.yaml")
                .exists(),
            "meta.yaml must NOT be created when an invitee is unregistered"
        );
        assert!(
            !state
                .repo_root
                .join("channels/ghost-channel.thread")
                .exists(),
            "thread file must NOT be created when an invitee is unregistered"
        );
    }

    #[tokio::test]
    async fn test_create_channel_without_invitees() {
        // Regression: empty invitees list must preserve the original "author only" behavior.
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        register_test_user(&state, "alice").await;

        let resp = handle_request(
            Request::CreateChannel {
                name: "solo-channel".to_string(),
                display_name: None,
                introduction: None,
                author: Some("alice".to_string()),
                invitees: vec![],
            },
            state.clone(),
        )
        .await;
        assert!(resp.ok, "create_channel without invitees failed: {:?}", resp.error);

        let meta_str = std::fs::read_to_string(
            state.repo_root.join("channels/solo-channel.meta.yaml"),
        )
        .expect("meta.yaml should exist");
        let meta: ChannelMeta = serde_yaml::from_str(&meta_str).unwrap();

        assert_eq!(
            meta.members,
            vec!["alice".to_string()],
            "no invitees → members should only contain author; got: {:?}",
            meta.members
        );
    }

    fn make_guest_state(tmp: &std::path::Path) -> SharedState {
        let repo = tmp.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(&repo)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(&repo)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(&repo)
            .output()
            .unwrap();

        let (tx, _) = broadcast::channel(16);
        let state = Arc::new(AppState::new(repo, Config::default(), tx, None));
        state
            .is_guest
            .store(true, std::sync::atomic::Ordering::SeqCst);
        state
    }

    #[tokio::test]
    async fn guest_send_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let state = make_guest_state(tmp.path());

        let resp = handle_request(
            Request::Send {
                channel: "general".to_string(),
                body: "hello".to_string(),
                reply_to: None,
                author: None,
            },
            state,
        )
        .await;

        assert!(!resp.ok, "guest send should fail");
        assert!(
            resp.error.as_deref().unwrap().contains("guest"),
            "error should mention guest mode: {:?}",
            resp.error
        );
    }

    #[tokio::test]
    async fn guest_create_channel_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let state = make_guest_state(tmp.path());

        let resp = handle_request(
            Request::CreateChannel {
                name: "test-ch".to_string(),
                display_name: None,
                introduction: None,
                author: None,
                invitees: vec![],
            },
            state,
        )
        .await;

        assert!(!resp.ok, "guest create_channel should fail");
        assert!(
            resp.error.as_deref().unwrap().contains("guest"),
            "error should mention guest mode: {:?}",
            resp.error
        );
    }

    #[tokio::test]
    async fn guest_read_operations_are_allowed() {
        let tmp = tempfile::tempdir().unwrap();
        let state = make_guest_state(tmp.path());

        let resp = handle_request(Request::Status, state.clone()).await;
        assert!(resp.ok, "guest status should succeed");

        let resp = handle_request(Request::ListChannels, state.clone()).await;
        assert!(resp.ok, "guest list_channels should succeed");

        let resp = handle_request(Request::ListUsers, state.clone()).await;
        assert!(resp.ok, "guest list_users should succeed");
    }
}

async fn handle_search(
    state: SharedState,
    query: Option<String>,
    author: Option<String>,
    channel: Option<String>,
    channel_type: Option<String>,
    limit: usize,
    offset: usize,
) -> Response {
    let current_user = state.current_user.read().await.clone();
    let index = {
        let guard = state.index.read().unwrap();
        match &*guard {
            Some(idx) => idx.clone(),
            None => return Response::error("search index not available"),
        }
    };

    let params = gitim_index::SearchParams {
        query,
        author,
        channel,
        channel_type,
        current_user,
        limit,
        offset,
    };

    match tokio::task::spawn_blocking(move || index.search(params)).await {
        Ok(Ok(result)) => {
            let messages: Vec<serde_json::Value> = result
                .messages
                .iter()
                .map(|m| {
                    serde_json::json!({
                        "channel": m.channel,
                        "channel_type": m.channel_type,
                        "line_number": m.line_number,
                        "parent_line": m.parent_line,
                        "author": m.author,
                        "timestamp": m.timestamp,
                        "body": m.body,
                    })
                })
                .collect();
            Response::success(serde_json::json!({
                "messages": messages,
                "total": result.total,
            }))
        }
        Ok(Err(gitim_index::IndexError::Rebuilding)) => Response::error("indexing_in_progress"),
        Ok(Err(gitim_index::IndexError::EmptySearch)) => {
            Response::error("search requires at least one of: query, author")
        }
        Ok(Err(e)) => Response::error(format!("search failed: {}", e)),
        Err(e) => Response::error(format!("search task failed: {}", e)),
    }
}

async fn handle_reindex(state: SharedState) -> Response {
    let index = {
        let guard = state.index.read().unwrap();
        match &*guard {
            Some(idx) => idx.clone(),
            None => return Response::error("search index not available"),
        }
    };

    let repo_root = state.repo_root.clone();
    let head = match state.git_storage.rev_parse("HEAD") {
        Ok(h) => h,
        Err(e) => return Response::error(format!("failed to get HEAD: {}", e)),
    };

    match tokio::task::spawn_blocking(move || index.reindex(&repo_root, &head)).await {
        Ok(Ok(count)) => Response::success(serde_json::json!({
            "status": "complete",
            "messages_indexed": count,
        })),
        Ok(Err(e)) => Response::error(format!("reindex failed: {}", e)),
        Err(e) => Response::error(format!("reindex task failed: {}", e)),
    }
}
