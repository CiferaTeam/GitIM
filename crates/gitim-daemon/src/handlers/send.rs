use crate::api::{Event, Response};
use crate::state::{PendingMessage, PushResult, SharedState};
use crate::handlers::resolve_thread_path;

use gitim_core::formatter::format_message;
use gitim_core::parser::parse_thread;
use gitim_core::types::{ChannelMeta, Handler};
use gitim_core::validator::compliance::validate_append;
use tracing::{info, warn};

pub async fn handle_send(
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

    // Commit-tree lock: held for the whole read-parse-append-commit span so
    // no other writer (and no sync_loop rebase) can mutate the commit tree
    // in parallel. Dropped before push_rx.await so a slow remote push can't
    // stall other writers. The critical section is entirely blocking I/O —
    // std::sync::Mutex is the right primitive here and must not be held
    // across any `.await`.
    let write_guard = state.commit_lock.lock().expect("commit_lock poisoned");

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
            let (author_name, author_email) = state.author_for(&author);
            match state
                .git_storage
                .add_and_commit_as(&[&rel_str], &commit_msg, Some((&author_name, &author_email)))
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

    // File is on disk and (if possible) committed — safe to let the next
    // writer race past us. Push await below must not hold the lock.
    drop(write_guard);

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
