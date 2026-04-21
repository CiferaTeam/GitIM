use crate::api::Response;
use crate::state::SharedState;
use crate::handlers::{entry_to_json, resolve_thread_path};

use gitim_core::parser::parse_thread;
use gitim_core::types::{ChannelMeta, ChannelName, ThreadEntry};

pub async fn handle_read(
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



pub async fn handle_list_channels(state: SharedState) -> Response {
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

pub async fn handle_list_archived_channels(state: SharedState) -> Response {
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

pub async fn handle_list_users(state: SharedState) -> Response {
    let users = state.users.read().await;
    let mut sorted: Vec<String> = users.clone();
    sorted.sort();
    Response::success(serde_json::json!({ "users": sorted }))
}

pub async fn handle_get_thread(state: SharedState, channel: String, line_number: u64) -> Response {
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

pub async fn handle_stop(state: SharedState) -> Response {
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
