use crate::api::Response;
use crate::handlers::{enrich_entries_with_recipients, resolve_thread_path};
use crate::state::SharedState;

use gitim_core::parser::parse_thread;
use gitim_core::types::{ChannelMeta, ChannelName, ThreadEntry, UserMeta};

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

    // Read fallback (archive-protocol Contract 3): when the active
    // surface file is missing, try the archive mirror at
    // `archive/<surface>/<stem>.thread`. `resolve_thread_path` already
    // returned the canonical sorted stem (`dm_filename` for DMs,
    // validated channel name otherwise) in `name`, so the archive path
    // is just the same stem under `archive/<surface>/`.
    let (thread_path, is_archived) = if !thread_path.exists() {
        let archive_subdir = if channel.starts_with("dm:") {
            "dm"
        } else {
            "channels"
        };
        let archive_path = state
            .repo_root
            .join("archive")
            .join(archive_subdir)
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

    // Three calling modes (see docs/plans/2026-05-11-channel-history-pagination/):
    //   limit only           → tail-cut, last N entries (channel open default)
    //   since only           → all entries after since (no truncation)
    //   since + limit        → head-cut, first N entries after since
    //                          (covers both incremental poll and history paging)
    if let Some(lim) = limit {
        if since.is_some() {
            entries.truncate(lim);
        } else {
            let drop_count = entries.len().saturating_sub(lim);
            entries.drain(..drop_count);
        }
    }

    let kind = if channel.starts_with("dm:") {
        "dm"
    } else {
        "channel"
    };
    let path_str = if kind == "dm" {
        format!("dm/{}.thread", name)
    } else {
        format!("channels/{}.thread", name)
    };
    let selected_entries: Vec<ThreadEntry> = entries.into_iter().cloned().collect();
    let entries =
        enrich_entries_with_recipients(&selected_entries, kind, &name, &path_str, &state.repo_root);

    let payload = gitim_core::responses::ReadResponse {
        channel,
        entries,
        archived: is_archived,
    };
    Response::json(payload)
}

pub async fn handle_list_channels(state: SharedState) -> Response {
    use gitim_core::responses::{ChannelSummary, ListChannelsResponse};
    let mut channels: Vec<ChannelSummary> = Vec::new();

    // 扫描 channels/*.meta.yaml — 读取 members 字段
    let ch_dir = state.repo_root.join("channels");
    if ch_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&ch_dir) {
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                if fname.ends_with(".meta.yaml") {
                    let name = fname.trim_end_matches(".meta.yaml").to_string();
                    let meta = std::fs::read_to_string(entry.path())
                        .ok()
                        .and_then(|c| serde_yaml::from_str::<ChannelMeta>(&c).ok());
                    let members = meta.as_ref().map(|m| m.members.clone()).unwrap_or_default();
                    let created_by = meta.map(|m| m.created_by);
                    channels.push(ChannelSummary {
                        name,
                        kind: "channel".to_string(),
                        members,
                        created_by,
                    });
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
                    channels.push(ChannelSummary {
                        name,
                        kind: "dm".to_string(),
                        members,
                        created_by: None,
                    });
                }
            }
        }
    }

    channels.sort_by(|a, b| a.name.cmp(&b.name));
    Response::json(ListChannelsResponse { channels })
}

pub async fn handle_list_archived_channels(
    state: SharedState,
    prefix: Option<String>,
    offset: usize,
    limit: usize,
) -> Response {
    use gitim_core::responses::{ChannelSummary, ListArchivedChannelsResponse};

    if limit == 0 || limit > 100 {
        return Response::error(format!("invalid limit {limit}: must be 1..=100"));
    }
    let needle = prefix
        .as_deref()
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    let mut channels: Vec<ChannelSummary> = Vec::new();

    // 扫描 archive/channels/*.meta.yaml — 读取 members 字段
    let arch_ch_dir = state.repo_root.join("archive").join("channels");
    if arch_ch_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&arch_ch_dir) {
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                if fname.ends_with(".meta.yaml") {
                    let name = fname.trim_end_matches(".meta.yaml").to_string();
                    if !needle.is_empty() && !name.to_ascii_lowercase().starts_with(&needle) {
                        continue;
                    }
                    let meta = std::fs::read_to_string(entry.path())
                        .ok()
                        .and_then(|c| serde_yaml::from_str::<ChannelMeta>(&c).ok());
                    let members = meta.as_ref().map(|m| m.members.clone()).unwrap_or_default();
                    let created_by = meta.map(|m| m.created_by);
                    channels.push(ChannelSummary {
                        name,
                        kind: "archived_channel".to_string(),
                        members,
                        created_by,
                    });
                }
            }
        }
    }

    channels.sort_by(|a, b| a.name.cmp(&b.name));

    let window: Vec<_> = channels.into_iter().skip(offset).take(limit + 1).collect();
    let has_more = window.len() > limit;
    let channels: Vec<_> = window.into_iter().take(limit).collect();

    Response::json(ListArchivedChannelsResponse { channels, has_more })
}

/// List active users. When `include_archived` is true, also scan
/// `archive/users/*.meta.yaml` and return the archived handlers in
/// the response's optional `archived_users` field.
///
/// Caller-uniform per archive-protocol P2.a: every caller (human via
/// CLI/WebUI, agent via runtime) flips the same `include_archived`
/// flag — daemon does not gate on caller type. The active and archive
/// dirs are mutually exclusive (Contract 2 / write interception in A.5),
/// so a handler appears in exactly one list at any moment; no dedup is
/// needed downstream.
pub async fn handle_list_users(state: SharedState, include_archived: bool) -> Response {
    let users = state.users.read().await;
    let mut sorted: Vec<String> = users.clone();
    sorted.sort();

    let archived_users = if include_archived {
        let arch_users_dir = state.repo_root.join("archive").join("users");
        let mut handlers: Vec<String> = Vec::new();
        if arch_users_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&arch_users_dir) {
                for entry in entries.flatten() {
                    let fname = entry.file_name().to_string_lossy().to_string();
                    if let Some(handler) = fname.strip_suffix(".meta.yaml") {
                        handlers.push(handler.to_string());
                    }
                }
            }
        }
        handlers.sort();
        Some(handlers)
    } else {
        None
    };

    let payload = gitim_core::responses::ListUsersResponse {
        users: sorted,
        archived_users,
    };
    Response::json(payload)
}

/// Scan `archive/users/*.meta.yaml` and return one `ArchivedUserEntry`
/// per file, sorted by handler. Mirrors `handle_list_archived_channels`
/// in shape; the per-row payload carries `handler` (always) and
/// `display_name` (best-effort — parsed from the archived `UserMeta`
/// yaml, omitted when the file is missing or unparseable). Frontends
/// fall back to rendering the bare handler when `display_name` is
/// absent.
pub async fn handle_list_archived_users(state: SharedState) -> Response {
    use gitim_core::responses::{ArchivedUserEntry, ListArchivedUsersResponse};
    let arch_users_dir = state.repo_root.join("archive").join("users");
    let mut entries: Vec<ArchivedUserEntry> = Vec::new();
    if arch_users_dir.exists() {
        if let Ok(rd) = std::fs::read_dir(&arch_users_dir) {
            for entry in rd.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                let Some(handler) = fname.strip_suffix(".meta.yaml") else {
                    continue;
                };
                // Best-effort display_name lookup. A read or parse failure
                // means the entry simply has no display_name on the wire —
                // not an error condition for the list call.
                let display_name = std::fs::read_to_string(entry.path())
                    .ok()
                    .and_then(|c| serde_yaml::from_str::<UserMeta>(&c).ok())
                    .map(|m| m.display_name);
                entries.push(ArchivedUserEntry {
                    handler: handler.to_string(),
                    display_name,
                });
            }
        }
    }
    entries.sort_by(|a, b| a.handler.cmp(&b.handler));
    let payload = ListArchivedUsersResponse { users: entries };
    Response::json(payload)
}

/// Scan `archive/dm/*.thread` and return the rows where `author` is one
/// of the two participants. Filtering by participation is the whole
/// access-control story for archived DMs at this layer — the daemon does
/// not expose third-party listings.
///
/// The peer is the participant other than `author`. Self-DMs (handler
/// equal to itself, which `dm_filename` produces as `<h>--<h>`) report
/// `author` as the peer; this branch is unreachable through normal
/// `archive_dm` flow because that requires `author != peer`, but the
/// listing tolerates the malformed file shape rather than panicking.
pub async fn handle_list_archived_dms(
    state: SharedState,
    author: String,
    prefix: Option<String>,
    offset: usize,
    limit: usize,
) -> Response {
    use gitim_core::dm::parse_dm_filename;
    use gitim_core::responses::{ArchivedDmEntry, ListArchivedDmsResponse};

    if limit == 0 || limit > 100 {
        return Response::error(format!("invalid limit {limit}: must be 1..=100"));
    }
    let needle = prefix
        .as_deref()
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    let arch_dm_dir = state.repo_root.join("archive").join("dm");
    let mut entries: Vec<ArchivedDmEntry> = Vec::new();
    if arch_dm_dir.exists() {
        if let Ok(rd) = std::fs::read_dir(&arch_dm_dir) {
            for entry in rd.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                let stem = match fname.strip_suffix(".thread") {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                let (first, second) = match parse_dm_filename(&stem) {
                    Some(pair) => pair,
                    None => continue,
                };
                let peer = if first == author {
                    second
                } else if second == author {
                    first
                } else {
                    // Caller did not participate in this DM — skip.
                    continue;
                };
                if !needle.is_empty() && !peer.to_ascii_lowercase().starts_with(&needle) {
                    continue;
                }
                entries.push(ArchivedDmEntry {
                    peer: peer.to_string(),
                    dm_pair_stem: stem,
                });
            }
        }
    }
    entries.sort_by(|a, b| a.peer.cmp(&b.peer));

    // Peek limit+1 to compute has_more without counting the whole directory.
    let window: Vec<_> = entries.into_iter().skip(offset).take(limit + 1).collect();
    let has_more = window.len() > limit;
    let dms: Vec<_> = window.into_iter().take(limit).collect();

    let payload = ListArchivedDmsResponse { dms, has_more };
    Response::json(payload)
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
    let by_line: std::collections::HashMap<u64, &_> =
        file.entries.iter().map(|e| (e.line_number(), e)).collect();
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
    let mut thread_entries: Vec<ThreadEntry> = Vec::new();
    let mut stack = vec![root_line];
    let mut visited = std::collections::HashSet::new();

    while let Some(target) = stack.pop() {
        if !visited.insert(target) {
            continue;
        }
        for entry in &file.entries {
            if entry.line_number() == target || entry.point_to() == target {
                thread_entries.push(entry.clone());
                if entry.line_number() != target {
                    stack.push(entry.line_number());
                }
            }
        }
    }

    // Sort by line number
    thread_entries.sort_by_key(|entry| entry.line_number());

    // Deduplicate (an entry could match both by line_number and point_to)
    thread_entries.dedup_by_key(|entry| entry.line_number());
    let path_str = format!("channels/{}.thread", channel);
    let thread_entries = enrich_entries_with_recipients(
        &thread_entries,
        "channel",
        &channel,
        &path_str,
        &state.repo_root,
    );

    let payload = gitim_core::responses::GetThreadResponse {
        channel,
        root_line,
        entries: thread_entries,
    };
    Response::json(payload)
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

    let payload = gitim_core::responses::StopResponse {
        status: "stopping".to_string(),
    };
    Response::json(payload)
}
