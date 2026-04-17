use crate::api::{Event, Response};
use crate::state::{PendingMessage, PushResult, SharedState};
use crate::thread_io;
use gitim_core::types::{
    validate_labels, CardMeta, CardStatus, ChannelName, Handler,
};
use gitim_sync::git::GitError;
use tracing::{info, warn};

const MAX_PUSH_RETRIES: u32 = 3;

fn validate_card_id(card_id: &str) -> Result<(), String> {
    if card_id.is_empty() || card_id.len() > 20 {
        return Err("card_id length out of range".into());
    }
    for ch in card_id.chars() {
        if !matches!(ch, '0'..='9' | 'a'..='f' | '-') {
            return Err(format!("invalid character in card_id: '{}'", ch));
        }
    }
    Ok(())
}

fn generate_card_id() -> String {
    let now = chrono::Utc::now();
    let ts = now.format("%Y%m%d-%H%M%S").to_string();
    let rand_hex = format!("{:03x}", rand::random::<u16>() & 0xFFF);
    format!("{}-{}", ts, rand_hex)
}

fn channel_thread_exists(state: &SharedState, channel: &ChannelName) -> bool {
    let p = state
        .repo_root
        .join("channels")
        .join(format!("{}.thread", channel));
    p.exists()
}

async fn ensure_known_user(state: &SharedState, handler: &str) -> Result<(), String> {
    let users = state.users.read().await;
    if !users.contains(&handler.to_string()) {
        return Err(format!("unknown user: {}", handler));
    }
    Ok(())
}

async fn push_with_retry(state: &SharedState, op: &str) -> Result<(), String> {
    if !state.git_storage.has_remote() {
        return Ok(());
    }
    for attempt in 1..=MAX_PUSH_RETRIES {
        match state.git_storage.push() {
            Ok(()) => return Ok(()),
            Err(GitError::PushConflict) => {
                warn!(
                    "{}: push conflict (attempt {}/{}), rebasing",
                    op, attempt, MAX_PUSH_RETRIES
                );
                state
                    .git_storage
                    .fetch()
                    .map_err(|e| format!("{} fetch failed: {}", op, e))?;
                state
                    .git_storage
                    .rebase_onto_origin()
                    .map_err(|e| format!("{} rebase failed: {}", op, e))?;
            }
            Err(e) => return Err(format!("{} push failed: {}", op, e)),
        }
    }
    Err(format!(
        "{}: push still conflicting after {} retries",
        op, MAX_PUSH_RETRIES
    ))
}

pub async fn handle_create_card(
    state: SharedState,
    channel: String,
    title: String,
    labels: Option<Vec<String>>,
    assignee: Option<String>,
    status: Option<String>,
    author: String,
) -> Response {
    let _h = match Handler::new(&author) {
        Ok(h) => h,
        Err(e) => return Response::error(format!("invalid author: {}", e)),
    };
    if let Err(e) = ensure_known_user(&state, &author).await {
        return Response::error(e);
    }

    let ch_name = match ChannelName::new(&channel) {
        Ok(n) => n,
        Err(e) => return Response::error(format!("invalid channel name: {}", e)),
    };
    if !channel_thread_exists(&state, &ch_name) {
        return Response::error(format!("channel '{}' does not exist", channel));
    }

    let labels_vec = labels.unwrap_or_default();
    if let Err(e) = validate_labels(&labels_vec) {
        return Response::error(format!("invalid labels: {}", e));
    }

    let status_parsed = match status.as_deref() {
        None => CardStatus::Todo,
        Some(s) => match CardStatus::parse(s) {
            Ok(v) => v,
            Err(e) => return Response::error(format!("{}", e)),
        },
    };

    if let Some(ref a) = assignee {
        if let Err(e) = ensure_known_user(&state, a).await {
            return Response::error(format!("assignee invalid: {}", e));
        }
    }

    if title.trim().is_empty() {
        return Response::error("title cannot be empty");
    }

    let card_id = generate_card_id();
    let card_dir = state
        .repo_root
        .join("channels")
        .join(ch_name.to_string())
        .join("cards")
        .join(&card_id);
    if let Err(e) = std::fs::create_dir_all(&card_dir) {
        return Response::error(format!("failed to create card dir: {}", e));
    }

    let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let meta = CardMeta {
        title: title.clone(),
        channel: ch_name.to_string(),
        status: status_parsed,
        labels: labels_vec,
        assignee,
        created_by: author.clone(),
        created_at: now.clone(),
        updated_at: now,
    };
    let meta_str = serde_yaml::to_string(&meta).unwrap();
    let meta_rel = format!(
        "channels/{}/cards/{}/card.meta.yaml",
        ch_name, card_id
    );
    let thread_rel = format!(
        "channels/{}/cards/{}/discussion.thread",
        ch_name, card_id
    );
    if let Err(e) = std::fs::write(card_dir.join("card.meta.yaml"), &meta_str) {
        return Response::error(format!("failed to write card meta: {}", e));
    }
    if let Err(e) = std::fs::write(card_dir.join("discussion.thread"), "") {
        return Response::error(format!("failed to write card thread: {}", e));
    }

    let commit_msg = format!(
        "card: create {} in {} by @{}",
        card_id, channel, author
    );
    if let Err(e) = state
        .git_storage
        .add_and_commit_as(&[&meta_rel, &thread_rel], &commit_msg, Some(&author))
    {
        return Response::error(format!("create_card commit failed: {}", e));
    }

    if let Err(e) = push_with_retry(&state, "create_card").await {
        return Response::error(e);
    }

    let _ = state.event_tx.send(Event::CardCreated {
        channel: ch_name.to_string(),
        card_id: card_id.clone(),
    });

    info!("card '{}' created in channel '{}' by @{}", card_id, channel, author);

    Response::success(serde_json::json!({
        "channel": ch_name.to_string(),
        "card_id": card_id,
        "title": title,
    }))
}

pub async fn handle_list_cards(
    state: SharedState,
    channel: Option<String>,
    labels: Option<Vec<String>>,
    status: Option<String>,
    assignee: Option<String>,
) -> Response {
    let status_filter = match status {
        None => None,
        Some(s) => match CardStatus::parse(&s) {
            Ok(v) => Some(v),
            Err(e) => return Response::error(format!("{}", e)),
        },
    };

    let label_filter = labels.unwrap_or_default();
    let channels_to_scan: Vec<String> = match channel {
        Some(c) => {
            let name = match ChannelName::new(&c) {
                Ok(n) => n,
                Err(e) => return Response::error(format!("invalid channel name: {}", e)),
            };
            vec![name.to_string()]
        }
        None => {
            let channels_dir = state.repo_root.join("channels");
            let mut names = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&channels_dir) {
                for entry in entries.flatten() {
                    if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        names.push(entry.file_name().to_string_lossy().to_string());
                    }
                }
            }
            names
        }
    };

    let mut cards: Vec<serde_json::Value> = Vec::new();
    for ch in &channels_to_scan {
        let cards_dir = state.repo_root.join("channels").join(ch).join("cards");
        if !cards_dir.exists() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&cards_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let meta_path = entry.path().join("card.meta.yaml");
            let Ok(content) = std::fs::read_to_string(&meta_path) else {
                continue;
            };
            let Ok(meta) = serde_yaml::from_str::<CardMeta>(&content) else {
                continue;
            };
            if let Some(ref s) = status_filter {
                if meta.status != *s {
                    continue;
                }
            }
            if !label_filter.is_empty() {
                let all_match = label_filter.iter().all(|l| meta.labels.contains(l));
                if !all_match {
                    continue;
                }
            }
            if let Some(ref a) = assignee {
                if meta.assignee.as_deref() != Some(a.as_str()) {
                    continue;
                }
            }
            let card_id = entry.file_name().to_string_lossy().to_string();
            cards.push(serde_json::json!({
                "card_id": card_id,
                "channel": meta.channel,
                "title": meta.title,
                "status": meta.status.as_str(),
                "labels": meta.labels,
                "assignee": meta.assignee,
                "created_by": meta.created_by,
                "created_at": meta.created_at,
                "updated_at": meta.updated_at,
            }));
        }
    }

    cards.sort_by(|a, b| {
        let ca = a["channel"].as_str().unwrap_or("");
        let cb = b["channel"].as_str().unwrap_or("");
        ca.cmp(cb)
            .then(a["card_id"].as_str().unwrap_or("").cmp(b["card_id"].as_str().unwrap_or("")))
    });
    Response::success(serde_json::json!({ "cards": cards }))
}

pub async fn handle_read_card(
    state: SharedState,
    channel: String,
    card_id: String,
    limit: Option<usize>,
    since: Option<u64>,
) -> Response {
    let ch_name = match ChannelName::new(&channel) {
        Ok(n) => n,
        Err(e) => return Response::error(format!("invalid channel name: {}", e)),
    };
    if let Err(e) = validate_card_id(&card_id) {
        return Response::error(format!("invalid card_id: {}", e));
    }
    let card_dir = state
        .repo_root
        .join("channels")
        .join(ch_name.to_string())
        .join("cards")
        .join(&card_id);
    let meta_path = card_dir.join("card.meta.yaml");
    let meta: CardMeta = match std::fs::read_to_string(&meta_path) {
        Ok(c) => match serde_yaml::from_str(&c) {
            Ok(m) => m,
            Err(e) => return Response::error(format!("failed to parse card meta: {}", e)),
        },
        Err(_) => {
            return Response::error(format!(
                "card '{}' not found in channel '{}'",
                card_id, channel
            ))
        }
    };
    let thread_path = card_dir.join("discussion.thread");
    let entries = match thread_io::read_thread_entries(&thread_path, limit, since) {
        Ok(e) => e,
        Err(e) => return Response::error(e),
    };
    Response::success(serde_json::json!({
        "channel": ch_name.to_string(),
        "card_id": card_id,
        "meta": {
            "title": meta.title,
            "status": meta.status.as_str(),
            "labels": meta.labels,
            "assignee": meta.assignee,
            "created_by": meta.created_by,
            "created_at": meta.created_at,
            "updated_at": meta.updated_at,
        },
        "entries": entries,
    }))
}

pub async fn handle_send_card_message(
    state: SharedState,
    channel: String,
    card_id: String,
    body: String,
    reply_to: Option<u64>,
    author: String,
) -> Response {
    let handler = match Handler::new(&author) {
        Ok(h) => h,
        Err(e) => return Response::error(format!("invalid author: {}", e)),
    };
    if let Err(e) = ensure_known_user(&state, &author).await {
        return Response::error(e);
    }
    let ch_name = match ChannelName::new(&channel) {
        Ok(n) => n,
        Err(e) => return Response::error(format!("invalid channel name: {}", e)),
    };
    if let Err(e) = validate_card_id(&card_id) {
        return Response::error(format!("invalid card_id: {}", e));
    }
    let card_dir = state
        .repo_root
        .join("channels")
        .join(ch_name.to_string())
        .join("cards")
        .join(&card_id);
    let meta_path = card_dir.join("card.meta.yaml");
    if !meta_path.exists() {
        return Response::error(format!(
            "card '{}' not found in channel '{}'",
            card_id, channel
        ));
    }
    let thread_path = card_dir.join("discussion.thread");
    let (next_line, _new_content) =
        match thread_io::append_message_to_thread(&thread_path, &handler, &body, reply_to) {
            Ok(v) => v,
            Err(e) => return Response::error(e),
        };

    let thread_rel = format!(
        "channels/{}/cards/{}/discussion.thread",
        ch_name, card_id
    );
    let channel_key = format!("channels/{}/cards/{}", ch_name, card_id);
    let commit_msg = format!(
        "card-msg: @{} -> {}/{} L{:06}",
        author, ch_name, card_id, next_line
    );
    let commit_status = match state
        .git_storage
        .add_and_commit_as(&[&thread_rel], &commit_msg, Some(&author))
    {
        Ok(()) => "committed",
        Err(e) => {
            warn!(
                "git commit failed for L{:06} in {}/{}: {}",
                next_line, ch_name, card_id, e
            );
            "written"
        }
    };

    let should_await_push =
        state.has_remote && state.sync_started.load(std::sync::atomic::Ordering::SeqCst);
    let push_rx = if should_await_push {
        let (tx, rx) = tokio::sync::oneshot::channel::<PushResult>();
        {
            let mut pending = state.pending_push.write().unwrap();
            pending.push(PendingMessage {
                channel: channel_key.clone(),
                line_number: next_line,
                result_tx: Some(tx),
            });
        }
        Some(rx)
    } else {
        {
            let mut pending = state.pending_push.write().unwrap();
            pending.push(PendingMessage {
                channel: channel_key.clone(),
                line_number: next_line,
                result_tx: None,
            });
        }
        None
    };

    let _ = state.event_tx.send(Event::CardMessageAppended {
        channel: ch_name.to_string(),
        card_id: card_id.clone(),
        line_numbers: vec![next_line],
    });

    info!(
        "card message sent to {}/{} by @{} at L{:06}",
        ch_name, card_id, author, next_line
    );

    if let Some(rx) = push_rx {
        state.push_notify.notify_one();
        match rx.await {
            Ok(PushResult::Pushed { commit_id }) => Response::success(serde_json::json!({
                "line_number": next_line,
                "channel": ch_name.to_string(),
                "card_id": card_id,
                "status": "pushed",
                "commit_id": commit_id,
            })),
            Ok(PushResult::Failed { reason }) => Response::success(serde_json::json!({
                "line_number": next_line,
                "channel": ch_name.to_string(),
                "card_id": card_id,
                "status": "commit_only",
                "error": reason,
            })),
            Err(_) => Response::success(serde_json::json!({
                "line_number": next_line,
                "channel": ch_name.to_string(),
                "card_id": card_id,
                "status": "commit_only",
                "error": "push result channel closed",
            })),
        }
    } else {
        Response::success(serde_json::json!({
            "line_number": next_line,
            "channel": ch_name.to_string(),
            "card_id": card_id,
            "status": commit_status,
        }))
    }
}

pub async fn handle_update_card(
    state: SharedState,
    channel: String,
    card_id: String,
    status: Option<String>,
    labels: Option<Vec<String>>,
    assignee: Option<String>,
    author: String,
) -> Response {
    let _h = match Handler::new(&author) {
        Ok(h) => h,
        Err(e) => return Response::error(format!("invalid author: {}", e)),
    };
    if let Err(e) = ensure_known_user(&state, &author).await {
        return Response::error(e);
    }
    let ch_name = match ChannelName::new(&channel) {
        Ok(n) => n,
        Err(e) => return Response::error(format!("invalid channel name: {}", e)),
    };
    if let Err(e) = validate_card_id(&card_id) {
        return Response::error(format!("invalid card_id: {}", e));
    }
    if status.is_none() && labels.is_none() && assignee.is_none() {
        return Response::error("must provide at least one field to update");
    }

    let card_dir = state
        .repo_root
        .join("channels")
        .join(ch_name.to_string())
        .join("cards")
        .join(&card_id);
    let meta_path = card_dir.join("card.meta.yaml");
    let mut meta: CardMeta = match std::fs::read_to_string(&meta_path) {
        Ok(c) => match serde_yaml::from_str(&c) {
            Ok(m) => m,
            Err(e) => return Response::error(format!("failed to parse card meta: {}", e)),
        },
        Err(_) => {
            return Response::error(format!(
                "card '{}' not found in channel '{}'",
                card_id, channel
            ))
        }
    };

    let old_status = meta.status.clone();
    if let Some(ref s) = status {
        match CardStatus::parse(s) {
            Ok(v) => meta.status = v,
            Err(e) => return Response::error(format!("{}", e)),
        }
    }
    if let Some(ref new_labels) = labels {
        if let Err(e) = validate_labels(new_labels) {
            return Response::error(format!("invalid labels: {}", e));
        }
        meta.labels = new_labels.clone();
    }
    if let Some(ref a) = assignee {
        if let Err(e) = ensure_known_user(&state, a).await {
            return Response::error(format!("assignee invalid: {}", e));
        }
        meta.assignee = Some(a.clone());
    }

    meta.updated_at = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let meta_str = serde_yaml::to_string(&meta).unwrap();
    if let Err(e) = std::fs::write(&meta_path, &meta_str) {
        return Response::error(format!("failed to write card meta: {}", e));
    }

    let meta_rel = format!(
        "channels/{}/cards/{}/card.meta.yaml",
        ch_name, card_id
    );
    let commit_msg = format!(
        "card: update {} in {} by @{}",
        card_id, channel, author
    );
    if let Err(e) = state
        .git_storage
        .add_and_commit_as(&[&meta_rel], &commit_msg, Some(&author))
    {
        return Response::error(format!("update_card commit failed: {}", e));
    }

    if let Err(e) = push_with_retry(&state, "update_card").await {
        return Response::error(e);
    }

    if status.is_some() && old_status != meta.status {
        let _ = state.event_tx.send(Event::CardStatusChanged {
            channel: ch_name.to_string(),
            card_id: card_id.clone(),
            old_status: old_status.as_str().to_string(),
            new_status: meta.status.as_str().to_string(),
            author: author.clone(),
        });
    }

    info!("card '{}' updated in channel '{}' by @{}", card_id, channel, author);

    Response::success(serde_json::json!({
        "channel": ch_name.to_string(),
        "card_id": card_id,
        "status": meta.status.as_str(),
        "labels": meta.labels,
        "assignee": meta.assignee,
    }))
}
