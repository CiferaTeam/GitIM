use crate::api::{Event, Response};
use crate::handlers::ensure_author_not_departed;
use crate::state::SharedState;

use gitim_core::formatter::format_event;
use gitim_core::parser::parse_thread;
use gitim_core::types::{ChannelMeta, ChannelName, Handler};
use gitim_core::validator::compliance::validate_append;
use gitim_core::validator::im_rules;
use gitim_sync::git::GitError;
use tracing::{info, warn};

pub async fn handle_join_channel(
    state: SharedState,
    channel: String,
    targets: Vec<String>,
    author: String,
) -> Response {
    write_channel_event(state, channel, targets, author, "join").await
}

pub async fn handle_leave_channel(
    state: SharedState,
    channel: String,
    targets: Vec<String>,
    author: String,
) -> Response {
    write_channel_event(state, channel, targets, author, "leave").await
}

const MAX_PUSH_RETRIES: u32 = 3;

pub async fn handle_create_channel(
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
    if let Err(resp) = ensure_author_not_departed(&state, &author) {
        return resp;
    }
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

    // 7. Write .thread with join event.
    // Creator's event carries invitees as targets, so it renders as
    // "@alice added @bob, @carol" — same shape as `handle_join_channel` emits
    // for subsequent invites. Empty targets when no invitees.
    let thread_path = channels_dir.join(format!("{}.thread", channel_name));
    let payload = if meta.members.len() > 1 {
        serde_json::json!({ "targets": &meta.members[1..] })
    } else {
        serde_json::json!({})
    };
    let join_line = format_event(1, &handler, &now, "join", &payload);
    if let Err(e) = std::fs::write(&thread_path, &join_line) {
        return Response::error(format!("failed to write channel thread: {}", e));
    }

    // 8. Commit
    let meta_rel = format!("channels/{}.meta.yaml", channel_name);
    let thread_rel = format!("channels/{}.thread", channel_name);
    let commit_msg = format!("channel: create #{} by @{}", name, author);
    let (author_name, author_email) = state.author_for(&author);
    if let Err(e) = state.git_storage.add_and_commit_as(
        &[&meta_rel, &thread_rel],
        &commit_msg,
        Some((&author_name, &author_email)),
    ) {
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
    let payload = gitim_core::responses::CreateChannelResponse {
        channel: name,
        created_by: author,
    };
    Response::success(serde_json::to_value(payload).unwrap())
}

pub async fn handle_archive_channel(
    state: SharedState,
    channel: String,
    author: String,
) -> Response {
    // 1. Validate channel name
    let channel_name = match ChannelName::new(&channel) {
        Ok(n) => n,
        Err(e) => return Response::error(format!("invalid channel name: {}", e)),
    };

    // 2. Validate author is registered + not departed
    if let Err(resp) = ensure_author_not_departed(&state, &author) {
        return resp;
    }
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
    let (author_name, author_email) = state.author_for(&author);
    if let Err(e) = state.git_storage.add_and_commit_as(
        &[&thread_to, &meta_to],
        &commit_msg,
        Some((&author_name, &author_email)),
    ) {
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
    let payload = gitim_core::responses::ArchiveChannelResponse {
        channel,
        archived_by: author,
    };
    Response::success(serde_json::to_value(payload).unwrap())
}

pub async fn handle_unarchive_channel(
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

    // 3. Read archive meta; fail if source not present
    let archive_meta_path = state
        .repo_root
        .join(format!("archive/channels/{}.meta.yaml", channel_name));
    let meta_str = match std::fs::read_to_string(&archive_meta_path) {
        Ok(s) => s,
        Err(_) => {
            return Response::error(format!(
                "archive source does not exist for channel '{}'",
                channel
            ));
        }
    };
    let meta: ChannelMeta = match serde_yaml::from_str(&meta_str) {
        Ok(m) => m,
        Err(e) => return Response::error(format!("failed to parse archive channel meta: {}", e)),
    };

    // 4. Permission: only creator can unarchive
    if meta.created_by != author {
        return Response::error("only channel creator can unarchive");
    }

    // 5. Name conflict: active meta must not already exist
    let active_meta_path = state
        .repo_root
        .join(format!("channels/{}.meta.yaml", channel_name));
    if active_meta_path.exists() {
        return Response::error(format!(
            "channel '{}' already exists in active location; unarchive aborted",
            channel
        ));
    }

    // 6. Ensure channels/ parent dir exists
    let channels_dir = state.repo_root.join("channels");
    if let Err(e) = std::fs::create_dir_all(&channels_dir) {
        return Response::error(format!("failed to create channels dir: {}", e));
    }

    // 7. git mv archive → active for both thread and meta.
    //    Move thread first; on meta-mv failure, reverse the thread mv.
    let thread_from = format!("archive/channels/{}.thread", channel_name);
    let thread_to = format!("channels/{}.thread", channel_name);
    let meta_from = format!("archive/channels/{}.meta.yaml", channel_name);
    let meta_to = format!("channels/{}.meta.yaml", channel_name);

    if let Err(e) = state.git_storage.mv(&thread_from, &thread_to) {
        return Response::error(format!("git mv thread failed: {}", e));
    }
    if let Err(e) = state.git_storage.mv(&meta_from, &meta_to) {
        // Reverse thread mv to leave tree clean.
        if let Err(rb) = state.git_storage.mv(&thread_to, &thread_from) {
            warn!("unarchive_channel: rollback thread mv also failed: {}", rb);
        }
        return Response::error(format!("git mv meta failed: {}", e));
    }

    // 8. add + commit as author. On failure, reverse BOTH mvs so archive is intact.
    let commit_msg = format!("unarchive: #{} by @{}", channel, author);
    let (author_name, author_email) = state.author_for(&author);
    if let Err(e) = state.git_storage.add_and_commit_as(
        &[&thread_to, &meta_to],
        &commit_msg,
        Some((&author_name, &author_email)),
    ) {
        // Reverse meta mv first, then thread mv — mirror archive direction.
        if let Err(rb) = state.git_storage.mv(&meta_to, &meta_from) {
            warn!("unarchive_channel: rollback meta mv also failed: {}", rb);
        }
        if let Err(rb) = state.git_storage.mv(&thread_to, &thread_from) {
            warn!("unarchive_channel: rollback thread mv also failed: {}", rb);
        }
        return Response::error(format!(
            "unarchive_channel commit failed: {}; rolled back git mv",
            e
        ));
    }

    // 9. Push with retry (mirror archive_channel)
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
                        "unarchive_channel: push conflict (attempt {}/{}), rebasing",
                        attempt, MAX_PUSH_RETRIES
                    );
                    if let Err(e) = state.git_storage.fetch() {
                        return Response::error(format!("unarchive_channel fetch failed: {}", e));
                    }
                    if let Err(e) = state.git_storage.rebase_onto_origin() {
                        return Response::error(format!("unarchive_channel rebase failed: {}", e));
                    }
                }
                Err(e) => {
                    return Response::error(format!("unarchive_channel push failed: {}", e));
                }
            }
        }
        if !pushed {
            return Response::error(format!(
                "unarchive_channel: push still conflicting after {} retries",
                MAX_PUSH_RETRIES
            ));
        }
    }

    // 10. Remove channel from thread_cache (symmetry with archive_channel)
    state.thread_cache.write().await.remove(&channel);

    // 11. Emit SSE event
    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let _ = state.event_tx.send(Event::ChannelUnarchived {
        channel: channel_name.to_string(),
        author: author.clone(),
        timestamp,
    });

    info!("channel '{}' unarchived by @{}", channel, author);

    // 12. Return success
    let payload = gitim_core::responses::UnarchiveChannelResponse {
        channel,
        unarchived_by: author,
    };
    Response::success(serde_json::to_value(payload).unwrap())
}

pub(super) async fn write_channel_event(
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

    // Archive Contract 2: a departed author can't author join/leave events.
    if let Err(resp) = ensure_author_not_departed(&state, &author) {
        return resp;
    }

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

    // Commit-tree lock: covers read → re-validate → append → commit so
    // concurrent joins (and sync_loop's rebase) can't interleave. Critical
    // section is all blocking I/O; no `.await` between here and the commit.
    let _write_guard = state.commit_lock.lock().expect("commit_lock poisoned");

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

    // Re-check join/leave rules against the latest on-disk state so a write
    // that waited behind another writer doesn't append a now-invalid event
    // (e.g. duplicate join after the other writer already added the target).
    let latest_meta: ChannelMeta = match std::fs::read_to_string(&meta_path) {
        Ok(content) => match serde_yaml::from_str(&content) {
            Ok(m) => m,
            Err(e) => return Response::error(format!("failed to parse channel meta: {}", e)),
        },
        Err(e) => return Response::error(format!("failed to read channel meta: {}", e)),
    };
    let latest_members: Vec<&str> = latest_meta.members.iter().map(|s| s.as_str()).collect();
    let revalidate = match event_type {
        "join" => im_rules::validate_join(&author, &target_refs, &user_refs, &latest_members),
        "leave" => im_rules::validate_leave(&author, &target_refs, &user_refs, &latest_members),
        _ => Ok(()),
    };
    if let Err(e) = revalidate {
        return Response::error(format!("{} validation failed: {}", event_type, e));
    }
    channel_meta = latest_meta;

    // Build event meta and format
    let meta = if targets.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::json!({"targets": targets})
    };
    let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let new_content = format_event(next_line, &handler, &now, event_type, &meta);

    // Compliance check: same belt-and-suspenders defense used on the message
    // path. Under the lock this can't fail on concurrency; it still catches
    // any out-of-band thread mutation (e.g. a hand-edit).
    let allowed_refs: Vec<&str> = channel_meta.members.iter().map(|s| s.as_str()).collect();
    if let Err(e) = validate_append(&existing, &new_content, &user_refs, &allowed_refs) {
        return Response::error(format!("compliance check failed: {}", e));
    }

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
    let (author_name, author_email) = state.author_for(&author);
    let commit_status = match state.git_storage.add_and_commit_as(
        &[&thread_rel, &meta_rel],
        &commit_msg,
        Some((&author_name, &author_email)),
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

    // Commit tree is stable — drop the lock BEFORE any `.await` below.
    // std::sync::MutexGuard must not cross await points, and everything
    // from here on (event broadcast, cache invalidation) is non-mutating.
    drop(_write_guard);

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
    let payload = gitim_core::responses::ChannelEventResponse {
        channel,
        event_type: event_type.to_string(),
        author,
        targets: affected,
        line_number: next_line,
        status: commit_status.to_string(),
    };
    Response::success(serde_json::to_value(payload).unwrap())
}
