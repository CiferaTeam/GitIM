use crate::api::{Event, Response};
use crate::state::{PendingMessage, PushResult, SharedState};
use crate::thread_io;
use gitim_core::types::{
    validate_labels, CardMeta, CardStatus, ChannelName, Handler,
};
use gitim_sync::git::GitError;
use tracing::{error, info, warn};

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

pub(crate) struct LocatedCard {
    pub rel_path: String,
    pub is_archived: bool,
}

/// Resolves a card to its on-disk location (active or archived).
///
/// Returns `None` if the card does not exist in either location.
/// If both locations exist (anomalous state from manual git manipulation),
/// prefers the active location and logs a warning rather than failing —
/// failing here would block legitimate operations on a card that merely
/// has stale archive files left behind.
pub(crate) fn locate_card(
    state: &SharedState,
    channel: &ChannelName,
    card_id: &str,
) -> Option<LocatedCard> {
    let active_rel = format!("channels/{}/cards/{}", channel, card_id);
    let archived_rel = format!("archive/channels/{}/cards/{}", channel, card_id);

    let active_exists = state
        .repo_root
        .join(&active_rel)
        .join("card.meta.yaml")
        .exists();
    let archived_exists = state
        .repo_root
        .join(&archived_rel)
        .join("card.meta.yaml")
        .exists();

    if active_exists && archived_exists {
        warn!(
            "card {} has both active and archived paths in channel {}; preferring active",
            card_id, channel
        );
        return Some(LocatedCard {
            rel_path: active_rel,
            is_archived: false,
        });
    }
    if active_exists {
        return Some(LocatedCard {
            rel_path: active_rel,
            is_archived: false,
        });
    }
    if archived_exists {
        return Some(LocatedCard {
            rel_path: archived_rel,
            is_archived: true,
        });
    }
    None
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

/// Push to remote with bounded retries on push conflict.
///
/// If this returns `Err`, the local commit is already durable — `sync_loop` will
/// retry the push on its next tick. Callers should surface the error to the client
/// as a transient push failure rather than implying the operation did not happen
/// locally.
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

    if title.trim().is_empty() {
        return Response::error("title cannot be empty");
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

pub async fn handle_archive_card(
    state: SharedState,
    channel: String,
    card_id: String,
    author: String,
) -> Response {
    // 1. Validate user
    if let Err(e) = ensure_known_user(&state, &author).await {
        return Response::error(e);
    }

    // 2. Validate channel name format
    let ch_name = match ChannelName::new(&channel) {
        Ok(n) => n,
        Err(e) => return Response::error(format!("invalid channel name: {}", e)),
    };

    // 3. Validate card_id
    if let Err(e) = validate_card_id(&card_id) {
        return Response::error(format!("invalid card_id: {}", e));
    }

    // 4. Locate card
    let located = match locate_card(&state, &ch_name, &card_id) {
        None => return Response::error(format!("card '{}' not found in channel '{}'", card_id, channel)),
        Some(loc) if loc.is_archived => return Response::error(format!("card '{}' is already archived", card_id)),
        Some(loc) => loc,
    };

    // 5. Read card.meta.yaml
    let meta_path = state.repo_root.join(&located.rel_path).join("card.meta.yaml");
    let meta: gitim_core::types::CardMeta = match std::fs::read_to_string(&meta_path) {
        Ok(c) => match serde_yaml::from_str(&c) {
            Ok(m) => m,
            Err(e) => return Response::error(format!("failed to parse card meta: {}", e)),
        },
        Err(_) => return Response::error(format!("card '{}' not found in channel '{}'", card_id, channel)),
    };

    // 6. Permission check: only creator or assignee can archive
    let is_creator = meta.created_by == author;
    let is_assignee = meta.assignee.as_deref() == Some(author.as_str());
    if !is_creator && !is_assignee {
        return Response::error("only creator or assignee can archive");
    }

    // 7. Create archive target parent directory
    let archive_cards_dir = state
        .repo_root
        .join("archive")
        .join("channels")
        .join(ch_name.to_string())
        .join("cards");
    if let Err(e) = std::fs::create_dir_all(&archive_cards_dir) {
        return Response::error(format!("failed to create archive dir: {}", e));
    }

    // 8. git mv (directory rename — git 2.x handles directories atomically)
    let from_rel = &located.rel_path; // channels/<ch>/cards/<id>
    let to_rel = format!("archive/channels/{}/cards/{}", ch_name, card_id);
    if let Err(e) = state.git_storage.mv(from_rel, &to_rel) {
        return Response::error(format!("git mv failed: {}", e));
    }

    // 9. add + commit as author — pass specific files since add_and_commit_as does git add on paths
    let meta_to = format!("{}/card.meta.yaml", to_rel);
    let thread_to = format!("{}/discussion.thread", to_rel);
    let commit_msg = format!("card: archive {} in {} by @{}", card_id, channel, author);
    if let Err(e) = state
        .git_storage
        .add_and_commit_as(&[&meta_to, &thread_to], &commit_msg, Some(&author))
    {
        // Rollback the git mv to leave the working tree clean.
        if let Err(rb_err) = state.git_storage.mv(&to_rel, from_rel) {
            error!("archive_card: rollback mv also failed: {}", rb_err);
        }
        return Response::error(format!("archive_card commit failed: {}; rolled back git mv", e));
    }

    // 10. Push with retry
    if let Err(e) = push_with_retry(&state, "archive_card").await {
        return Response::error(e);
    }

    // 11. Emit event
    let _ = state.event_tx.send(Event::CardArchived {
        channel: ch_name.to_string(),
        card_id: card_id.clone(),
        author: author.clone(),
    });

    // 12. Info log
    info!("card '{}' archived in channel '{}' by @{}", card_id, channel, author);

    // 13. Return success
    Response::success(serde_json::json!({
        "channel": ch_name.to_string(),
        "card_id": card_id,
        "archived_by": author,
    }))
}

pub async fn handle_unarchive_card(
    state: SharedState,
    channel: String,
    card_id: String,
    author: String,
) -> Response {
    // 1. Validate user
    if let Err(e) = ensure_known_user(&state, &author).await {
        return Response::error(e);
    }

    // 2. Validate channel name format
    let ch_name = match ChannelName::new(&channel) {
        Ok(n) => n,
        Err(e) => return Response::error(format!("invalid channel name: {}", e)),
    };

    // 3. Validate card_id
    if let Err(e) = validate_card_id(&card_id) {
        return Response::error(format!("invalid card_id: {}", e));
    }

    // 4. Locate card — must be archived
    let located = match locate_card(&state, &ch_name, &card_id) {
        None => return Response::error(format!("card '{}' not found in channel '{}'", card_id, channel)),
        Some(loc) if !loc.is_archived => return Response::error(format!("card '{}' is not archived", card_id)),
        Some(loc) => loc,
    };

    // 4b. Guard: refuse to unarchive into an inactive (archived or deleted) channel
    if !channel_thread_exists(&state, &ch_name) {
        return Response::error(format!(
            "cannot unarchive card: channel '{}' is not active (may be archived or deleted)",
            channel
        ));
    }

    // 5. Read card.meta.yaml
    let meta_path = state.repo_root.join(&located.rel_path).join("card.meta.yaml");
    let meta: gitim_core::types::CardMeta = match std::fs::read_to_string(&meta_path) {
        Ok(c) => match serde_yaml::from_str(&c) {
            Ok(m) => m,
            Err(e) => return Response::error(format!("failed to parse card meta: {}", e)),
        },
        Err(_) => return Response::error(format!("card '{}' not found in channel '{}'", card_id, channel)),
    };

    // 6. Permission check: only creator or assignee can unarchive
    let is_creator = meta.created_by == author;
    let is_assignee = meta.assignee.as_deref() == Some(author.as_str());
    if !is_creator && !is_assignee {
        return Response::error("only creator or assignee can unarchive");
    }

    // 7. Ensure target parent directory exists (defensive — likely already there)
    let active_cards_dir = state
        .repo_root
        .join("channels")
        .join(ch_name.to_string())
        .join("cards");
    if let Err(e) = std::fs::create_dir_all(&active_cards_dir) {
        return Response::error(format!("failed to create cards dir: {}", e));
    }

    // 8. git mv: archive/channels/<ch>/cards/<id> → channels/<ch>/cards/<id>
    let from_rel = &located.rel_path; // archive/channels/<ch>/cards/<id>
    let to_rel = format!("channels/{}/cards/{}", ch_name, card_id);
    if let Err(e) = state.git_storage.mv(from_rel, &to_rel) {
        return Response::error(format!("git mv failed: {}", e));
    }

    // 9. add + commit as author
    let meta_to = format!("{}/card.meta.yaml", to_rel);
    let thread_to = format!("{}/discussion.thread", to_rel);
    let commit_msg = format!("card: unarchive {} in {} by @{}", card_id, channel, author);
    if let Err(e) = state
        .git_storage
        .add_and_commit_as(&[&meta_to, &thread_to], &commit_msg, Some(&author))
    {
        // Rollback the git mv to leave the working tree clean.
        if let Err(rb_err) = state.git_storage.mv(&to_rel, from_rel) {
            error!("unarchive_card: rollback mv also failed: {}", rb_err);
        }
        return Response::error(format!("unarchive_card commit failed: {}; rolled back git mv", e));
    }

    // 10. Push with retry
    if let Err(e) = push_with_retry(&state, "unarchive_card").await {
        return Response::error(e);
    }

    // 11. Emit event
    let _ = state.event_tx.send(Event::CardUnarchived {
        channel: ch_name.to_string(),
        card_id: card_id.clone(),
        author: author.clone(),
    });

    // 12. Info log
    info!("card '{}' unarchived in channel '{}' by @{}", card_id, channel, author);

    // 13. Return success
    Response::success(serde_json::json!({
        "channel": ch_name.to_string(),
        "card_id": card_id,
        "unarchived_by": author,
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

pub async fn handle_list_archived_cards(
    state: SharedState,
    channel: Option<String>,
) -> Response {
    // Determine which channel directories to scan under archive/channels/
    let arch_channels_dir = state.repo_root.join("archive").join("channels");

    let channels_to_scan: Vec<String> = match channel {
        Some(ref c) => {
            let name = match ChannelName::new(c) {
                Ok(n) => n,
                Err(e) => return Response::error(format!("invalid channel name: {}", e)),
            };
            vec![name.to_string()]
        }
        None => {
            let mut names = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&arch_channels_dir) {
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
        let cards_dir = arch_channels_dir.join(ch).join("cards");
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
            let card_id = entry.file_name().to_string_lossy().to_string();
            debug_assert_eq!(
                meta.channel, *ch,
                "archived card meta.channel diverged from dir name for card {card_id}"
            );
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
    let located = match locate_card(&state, &ch_name, &card_id) {
        Some(l) => l,
        None => {
            return Response::error(format!(
                "card '{}' not found in channel '{}'",
                card_id, channel
            ))
        }
    };
    let card_dir = state.repo_root.join(&located.rel_path);
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
        "archived": located.is_archived,
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
    let located = match locate_card(&state, &ch_name, &card_id) {
        Some(l) => l,
        None => {
            return Response::error(format!(
                "card '{}' not found in channel '{}'",
                card_id, channel
            ))
        }
    };
    if located.is_archived {
        return Response::error(format!(
            "cannot send to archived card '{}' in channel '{}'",
            card_id, channel
        ));
    }
    let card_dir = state.repo_root.join(&located.rel_path);
    let thread_path = card_dir.join("discussion.thread");

    // Commit-tree lock: keeps the read-append-commit sequence serial across
    // all writers (and blocks sync_loop's rebase from interleaving). No
    // `.await` inside the locked region — std::sync::Mutex is intentional.
    let write_guard = state.commit_lock.lock().expect("commit_lock poisoned");

    let (next_line, _new_content) =
        match thread_io::append_message_to_thread(&thread_path, &handler, &body, reply_to) {
            Ok(v) => v,
            Err(e) => return Response::error(e),
        };

    let thread_rel = format!("{}/discussion.thread", located.rel_path);
    let channel_key = located.rel_path.clone();
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

    // File committed (or at least written) — release before the push await
    // so pending push completion doesn't block other writers.
    drop(write_guard);

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

    let located = match locate_card(&state, &ch_name, &card_id) {
        Some(l) => l,
        None => {
            return Response::error(format!(
                "card '{}' not found in channel '{}'",
                card_id, channel
            ))
        }
    };
    if located.is_archived {
        return Response::error(format!(
            "cannot update archived card '{}' in channel '{}'",
            card_id, channel
        ));
    }

    let card_dir = state.repo_root.join(&located.rel_path);
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

    let meta_rel = format!("{}/card.meta.yaml", located.rel_path);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use gitim_core::types::Config;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::broadcast;

    fn make_config() -> Config {
        serde_yaml::from_str("version: 1").unwrap()
    }

    async fn setup_test_repo() -> (TempDir, SharedState) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        std::fs::create_dir_all(root.join("users")).unwrap();
        std::fs::create_dir_all(root.join("channels")).unwrap();
        std::fs::write(
            root.join("users/alice.meta.yaml"),
            "display_name: Alice\nrole: dev\nintroduction: hi\n",
        )
        .unwrap();
        std::fs::write(
            root.join("users/bob.meta.yaml"),
            "display_name: Bob\nrole: dev\nintroduction: hello\n",
        )
        .unwrap();
        std::fs::write(root.join("channels/dev.thread"), "").unwrap();
        let run_git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@test.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@test.com")
                .output()
                .unwrap()
        };
        run_git(&["init"]);
        run_git(&["add", "."]);
        run_git(&["commit", "-m", "init"]);
        let (tx, _) = broadcast::channel(100);
        let state = Arc::new(AppState::new(
            root,
            make_config(),
            tx,
            Some("alice".to_string()),
        ));
        {
            let mut users = state.users.write().await;
            *users = vec!["alice".to_string(), "bob".to_string()];
        }
        (tmp, state)
    }

    /// Write a minimal CardMeta yaml to a given path.
    fn write_card_meta(path: &std::path::Path, channel: &str) {
        let content = format!(
            "title: Test Card\nchannel: {}\nstatus: todo\nlabels: []\nassignee: ~\ncreated_by: alice\ncreated_at: 20260101T000000Z\nupdated_at: 20260101T000000Z\n",
            channel
        );
        std::fs::write(path, content).unwrap();
    }

    const CARD_ID: &str = "20260101-000000-abc";

    // ─── locate_card tests ────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_locate_card_finds_active_path() {
        let (_tmp, state) = setup_test_repo().await;
        let ch = ChannelName::new("foo").unwrap();
        let card_dir = state
            .repo_root
            .join("channels/foo/cards")
            .join(CARD_ID);
        std::fs::create_dir_all(&card_dir).unwrap();
        write_card_meta(&card_dir.join("card.meta.yaml"), "foo");

        let result = locate_card(&state, &ch, CARD_ID);
        let loc = result.expect("should find the active card");
        assert!(!loc.is_archived, "should be active");
        assert_eq!(
            loc.rel_path,
            format!("channels/foo/cards/{}", CARD_ID)
        );
    }

    #[tokio::test]
    async fn test_locate_card_finds_archived_path() {
        let (_tmp, state) = setup_test_repo().await;
        let ch = ChannelName::new("foo").unwrap();
        let card_dir = state
            .repo_root
            .join("archive/channels/foo/cards")
            .join(CARD_ID);
        std::fs::create_dir_all(&card_dir).unwrap();
        write_card_meta(&card_dir.join("card.meta.yaml"), "foo");

        let result = locate_card(&state, &ch, CARD_ID);
        let loc = result.expect("should find the archived card");
        assert!(loc.is_archived, "should be archived");
        assert_eq!(
            loc.rel_path,
            format!("archive/channels/foo/cards/{}", CARD_ID)
        );
    }

    #[tokio::test]
    async fn test_locate_card_prefers_active_when_both_exist() {
        let (_tmp, state) = setup_test_repo().await;
        let ch = ChannelName::new("foo").unwrap();

        // Setup both paths (anomalous state)
        let active_dir = state
            .repo_root
            .join("channels/foo/cards")
            .join(CARD_ID);
        std::fs::create_dir_all(&active_dir).unwrap();
        write_card_meta(&active_dir.join("card.meta.yaml"), "foo");

        let archived_dir = state
            .repo_root
            .join("archive/channels/foo/cards")
            .join(CARD_ID);
        std::fs::create_dir_all(&archived_dir).unwrap();
        write_card_meta(&archived_dir.join("card.meta.yaml"), "foo");

        let result = locate_card(&state, &ch, CARD_ID);
        let loc = result.expect("should find a card");
        assert!(!loc.is_archived, "should prefer active over archived");
        assert_eq!(
            loc.rel_path,
            format!("channels/foo/cards/{}", CARD_ID)
        );
    }

    #[tokio::test]
    async fn test_locate_card_not_found() {
        let (_tmp, state) = setup_test_repo().await;
        let ch = ChannelName::new("foo").unwrap();
        let result = locate_card(&state, &ch, CARD_ID);
        assert!(result.is_none(), "card should not be found");
    }

    // ─── handle_read_card tests ───────────────────────────────────────────────

    async fn create_active_card_fixture(state: &SharedState, channel: &str) -> String {
        let resp = handle_create_card(
            state.clone(),
            channel.to_string(),
            "Test Card".to_string(),
            None,
            None,
            None,
            "alice".to_string(),
        )
        .await;
        assert!(resp.ok, "create_card should succeed: {:?}", resp.error);
        resp.data.unwrap()["card_id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[tokio::test]
    async fn test_read_card_active_returns_archived_false() {
        let (_tmp, state) = setup_test_repo().await;
        let card_id = create_active_card_fixture(&state, "dev").await;

        let resp = handle_read_card(
            state.clone(),
            "dev".to_string(),
            card_id.clone(),
            None,
            None,
        )
        .await;
        assert!(resp.ok, "read should succeed: {:?}", resp.error);
        let data = resp.data.unwrap();
        assert_eq!(
            data["archived"].as_bool().unwrap(),
            false,
            "active card should have archived=false"
        );
    }

    #[tokio::test]
    async fn test_read_card_archived_returns_archived_true_and_messages() {
        let (_tmp, state) = setup_test_repo().await;
        // Create card the normal way to get a valid card_id format
        let card_id = create_active_card_fixture(&state, "dev").await;

        // Send a message to it first
        let send_resp = handle_send_card_message(
            state.clone(),
            "dev".to_string(),
            card_id.clone(),
            "a message".to_string(),
            None,
            "alice".to_string(),
        )
        .await;
        assert!(send_resp.ok, "send should succeed");

        // Manually move the card directory to the archive location
        let active_dir = state
            .repo_root
            .join("channels/dev/cards")
            .join(&card_id);
        let archive_dir = state
            .repo_root
            .join("archive/channels/dev/cards")
            .join(&card_id);
        std::fs::create_dir_all(archive_dir.parent().unwrap()).unwrap();
        std::fs::rename(&active_dir, &archive_dir).unwrap();

        let resp = handle_read_card(
            state.clone(),
            "dev".to_string(),
            card_id.clone(),
            None,
            None,
        )
        .await;
        assert!(resp.ok, "read of archived card should succeed: {:?}", resp.error);
        let data = resp.data.unwrap();
        assert_eq!(
            data["archived"].as_bool().unwrap(),
            true,
            "archived card should have archived=true"
        );
        let entries = data["entries"].as_array().unwrap();
        assert!(!entries.is_empty(), "archived card entries should be readable");
    }

    #[tokio::test]
    async fn test_read_card_not_found_returns_error() {
        let (_tmp, state) = setup_test_repo().await;
        let resp = handle_read_card(
            state.clone(),
            "dev".to_string(),
            CARD_ID.to_string(),
            None,
            None,
        )
        .await;
        assert!(!resp.ok, "read of missing card should fail");
        let err = resp.error.unwrap();
        assert!(
            err.contains("not found"),
            "error should mention 'not found': {}",
            err
        );
    }

    // ─── handle_send_card_message reject archived tests ───────────────────────

    #[tokio::test]
    async fn test_send_card_message_rejects_archived() {
        let (_tmp, state) = setup_test_repo().await;
        let card_id = create_active_card_fixture(&state, "dev").await;

        // Move to archive
        let active_dir = state
            .repo_root
            .join("channels/dev/cards")
            .join(&card_id);
        let archive_dir = state
            .repo_root
            .join("archive/channels/dev/cards")
            .join(&card_id);
        std::fs::create_dir_all(archive_dir.parent().unwrap()).unwrap();
        std::fs::rename(&active_dir, &archive_dir).unwrap();

        let resp = handle_send_card_message(
            state.clone(),
            "dev".to_string(),
            card_id.clone(),
            "should fail".to_string(),
            None,
            "alice".to_string(),
        )
        .await;
        assert!(!resp.ok, "send to archived card should fail");
        let err = resp.error.unwrap();
        assert!(
            err.contains("archived"),
            "error should mention 'archived': {}",
            err
        );
    }

    // ─── handle_update_card reject archived tests ─────────────────────────────

    #[tokio::test]
    async fn test_update_card_rejects_archived() {
        let (_tmp, state) = setup_test_repo().await;
        let card_id = create_active_card_fixture(&state, "dev").await;

        // Move to archive
        let active_dir = state
            .repo_root
            .join("channels/dev/cards")
            .join(&card_id);
        let archive_dir = state
            .repo_root
            .join("archive/channels/dev/cards")
            .join(&card_id);
        std::fs::create_dir_all(archive_dir.parent().unwrap()).unwrap();
        std::fs::rename(&active_dir, &archive_dir).unwrap();

        let resp = handle_update_card(
            state.clone(),
            "dev".to_string(),
            card_id.clone(),
            Some("done".to_string()),
            None,
            None,
            "alice".to_string(),
        )
        .await;
        assert!(!resp.ok, "update of archived card should fail");
        let err = resp.error.unwrap();
        assert!(
            err.contains("archived"),
            "error should mention 'archived': {}",
            err
        );
    }

    // ─── handle_archive_card tests ────────────────────────────────────────────

    /// Create a card with specific created_by / assignee for archive permission tests.
    fn write_card_meta_full(
        path: &std::path::Path,
        channel: &str,
        created_by: &str,
        assignee: Option<&str>,
        status: &str,
    ) {
        let assignee_field = match assignee {
            Some(a) => format!("assignee: {}", a),
            None => "assignee: ~".to_string(),
        };
        let content = format!(
            "title: Test Card\nchannel: {}\nstatus: {}\nlabels: []\n{}\ncreated_by: {}\ncreated_at: 20260101T000000Z\nupdated_at: 20260101T000000Z\n",
            channel, status, assignee_field, created_by
        );
        std::fs::write(path, content).unwrap();
    }

    async fn create_card_for_archive(
        state: &SharedState,
        channel: &str,
        card_id: &str,
        created_by: &str,
        assignee: Option<&str>,
        status: &str,
    ) {
        let card_dir = state
            .repo_root
            .join("channels")
            .join(channel)
            .join("cards")
            .join(card_id);
        std::fs::create_dir_all(&card_dir).unwrap();
        write_card_meta_full(
            &card_dir.join("card.meta.yaml"),
            channel,
            created_by,
            assignee,
            status,
        );
        std::fs::write(card_dir.join("discussion.thread"), "").unwrap();

        // git add + commit so git mv will work
        let run_git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&state.repo_root)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@test.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@test.com")
                .output()
                .unwrap()
        };
        run_git(&["add", "."]);
        run_git(&["commit", "-m", &format!("add card {}", card_id)]);
    }

    const ARCHIVE_CARD_ID: &str = "20260101-120000-abc";

    #[tokio::test]
    async fn test_archive_card_by_creator_success() {
        let (_tmp, state) = setup_test_repo().await;
        // Setup: alice creates a card, no assignee
        create_card_for_archive(&state, "dev", ARCHIVE_CARD_ID, "alice", None, "todo").await;

        let mut event_rx = state.event_tx.subscribe();

        let resp = handle_archive_card(
            state.clone(),
            "dev".to_string(),
            ARCHIVE_CARD_ID.to_string(),
            "alice".to_string(),
        )
        .await;
        assert!(resp.ok, "archive by creator should succeed: {:?}", resp.error);

        // Active path should no longer exist
        let active_dir = state
            .repo_root
            .join("channels/dev/cards")
            .join(ARCHIVE_CARD_ID);
        assert!(
            !active_dir.exists(),
            "active card dir should be gone after archive"
        );

        // Archived path should exist with card.meta.yaml preserved
        let archive_meta = state
            .repo_root
            .join("archive/channels/dev/cards")
            .join(ARCHIVE_CARD_ID)
            .join("card.meta.yaml");
        assert!(
            archive_meta.exists(),
            "archived card.meta.yaml should exist"
        );

        // Meta content should be preserved (status unchanged)
        let content = std::fs::read_to_string(&archive_meta).unwrap();
        assert!(content.contains("status: todo"), "status should be preserved: {}", content);

        // CardArchived event should be emitted
        let event = event_rx.try_recv().expect("should have received an event");
        match event {
            crate::api::Event::CardArchived { channel, card_id, author } => {
                assert_eq!(channel, "dev");
                assert_eq!(card_id, ARCHIVE_CARD_ID);
                assert_eq!(author, "alice");
            }
            other => panic!("expected CardArchived, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_archive_card_by_assignee_success() {
        let (_tmp, state) = setup_test_repo().await;
        // card created by alice, assigned to lewis (bob is in users list)
        // Add lewis to users list for this test
        {
            let mut users = state.users.write().await;
            users.push("lewis".to_string());
        }
        std::fs::write(
            state.repo_root.join("users/lewis.meta.yaml"),
            "display_name: Lewis\nrole: dev\nintroduction: hi\n",
        )
        .unwrap();

        create_card_for_archive(
            &state,
            "dev",
            ARCHIVE_CARD_ID,
            "alice",
            Some("lewis"),
            "doing",
        )
        .await;

        let resp = handle_archive_card(
            state.clone(),
            "dev".to_string(),
            ARCHIVE_CARD_ID.to_string(),
            "lewis".to_string(), // assignee archives it
        )
        .await;
        assert!(resp.ok, "archive by assignee should succeed: {:?}", resp.error);

        let archive_dir = state
            .repo_root
            .join("archive/channels/dev/cards")
            .join(ARCHIVE_CARD_ID);
        assert!(archive_dir.exists(), "archived dir should exist");
    }

    #[tokio::test]
    async fn test_archive_card_rejects_non_creator_non_assignee() {
        let (_tmp, state) = setup_test_repo().await;
        // card created by alice, no assignee; bob tries to archive
        create_card_for_archive(&state, "dev", ARCHIVE_CARD_ID, "alice", None, "todo").await;

        let resp = handle_archive_card(
            state.clone(),
            "dev".to_string(),
            ARCHIVE_CARD_ID.to_string(),
            "bob".to_string(),
        )
        .await;
        assert!(!resp.ok, "non-creator/non-assignee should be rejected");
        let err = resp.error.unwrap();
        assert!(
            err.contains("only creator or assignee"),
            "error should mention permission: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_archive_card_rejects_already_archived() {
        let (_tmp, state) = setup_test_repo().await;
        // Place card directly in archive location
        let archive_card_dir = state
            .repo_root
            .join("archive/channels/dev/cards")
            .join(ARCHIVE_CARD_ID);
        std::fs::create_dir_all(&archive_card_dir).unwrap();
        write_card_meta_full(
            &archive_card_dir.join("card.meta.yaml"),
            "dev",
            "alice",
            None,
            "todo",
        );
        std::fs::write(archive_card_dir.join("discussion.thread"), "").unwrap();

        let run_git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&state.repo_root)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@test.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@test.com")
                .output()
                .unwrap()
        };
        run_git(&["add", "."]);
        run_git(&["commit", "-m", "add archived card"]);

        let resp = handle_archive_card(
            state.clone(),
            "dev".to_string(),
            ARCHIVE_CARD_ID.to_string(),
            "alice".to_string(),
        )
        .await;
        assert!(!resp.ok, "already archived card should be rejected");
        let err = resp.error.unwrap();
        assert!(
            err.contains("already archived"),
            "error should mention 'already archived': {}",
            err
        );
    }

    #[tokio::test]
    async fn test_archive_card_rejects_unknown_card() {
        let (_tmp, state) = setup_test_repo().await;
        // No card setup at all
        let resp = handle_archive_card(
            state.clone(),
            "dev".to_string(),
            ARCHIVE_CARD_ID.to_string(),
            "alice".to_string(),
        )
        .await;
        assert!(!resp.ok, "unknown card should be rejected");
        let err = resp.error.unwrap();
        assert!(
            err.contains("not found"),
            "error should mention 'not found': {}",
            err
        );
    }

    #[tokio::test]
    async fn test_archive_card_rejects_unknown_author() {
        let (_tmp, state) = setup_test_repo().await;
        create_card_for_archive(&state, "dev", ARCHIVE_CARD_ID, "alice", None, "todo").await;

        let resp = handle_archive_card(
            state.clone(),
            "dev".to_string(),
            ARCHIVE_CARD_ID.to_string(),
            "nobody".to_string(), // not in users list
        )
        .await;
        assert!(!resp.ok, "unknown author should be rejected");
        let err = resp.error.unwrap();
        assert!(
            err.contains("unknown user"),
            "error should mention 'unknown user': {}",
            err
        );
    }

    #[tokio::test]
    async fn test_archive_card_preserves_status_field() {
        let (_tmp, state) = setup_test_repo().await;
        // Card in "doing" status — archive should not change status
        create_card_for_archive(&state, "dev", ARCHIVE_CARD_ID, "alice", None, "doing").await;

        let resp = handle_archive_card(
            state.clone(),
            "dev".to_string(),
            ARCHIVE_CARD_ID.to_string(),
            "alice".to_string(),
        )
        .await;
        assert!(resp.ok, "archive should succeed: {:?}", resp.error);

        let archive_meta = state
            .repo_root
            .join("archive/channels/dev/cards")
            .join(ARCHIVE_CARD_ID)
            .join("card.meta.yaml");
        let content = std::fs::read_to_string(&archive_meta).unwrap();
        assert!(
            content.contains("status: doing"),
            "status 'doing' should be preserved after archive, got: {}",
            content
        );
    }

    #[tokio::test]
    async fn test_archive_card_rolls_back_git_mv_on_commit_failure() {
        let (_tmp, state) = setup_test_repo().await;
        let card_id = "20260101-120000-ee1";
        create_card_for_archive(&state, "dev", card_id, "alice", None, "todo").await;

        // Install a pre-commit hook that rejects all commits, triggering commit failure.
        let hooks_dir = state.repo_root.join(".git/hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        let hook_path = hooks_dir.join("pre-commit");
        std::fs::write(&hook_path, "#!/bin/sh\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let resp = handle_archive_card(
            state.clone(),
            "dev".to_string(),
            card_id.to_string(),
            "alice".to_string(),
        )
        .await;

        // Response must be an error
        assert!(!resp.ok, "archive should fail when commit is rejected");
        let err = resp.error.unwrap();
        assert!(
            err.contains("rolled back"),
            "error should mention rollback: {}",
            err
        );

        // Card file must still be in active location (rollback succeeded)
        let active_meta = state
            .repo_root
            .join("channels/dev/cards")
            .join(card_id)
            .join("card.meta.yaml");
        assert!(
            active_meta.exists(),
            "card.meta.yaml should still be in active location after rollback"
        );

        // Archive location must be empty (no partial move left behind)
        let archive_dir = state
            .repo_root
            .join("archive/channels/dev/cards")
            .join(card_id);
        assert!(
            !archive_dir.exists(),
            "archive dir should not exist after rollback"
        );

        // Working tree should be clean (no staged git mv)
        let status_output = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&state.repo_root)
            .output()
            .unwrap();
        let status_str = String::from_utf8_lossy(&status_output.stdout);
        assert!(
            status_str.trim().is_empty(),
            "working tree should be clean after rollback, got: {}",
            status_str
        );
    }

    // ─── handle_unarchive_card tests ─────────────────────────────────────────

    /// Create a card directly in the archive location (already git-committed),
    /// ready for unarchive tests.
    async fn create_archived_card_fixture(
        state: &SharedState,
        channel: &str,
        card_id: &str,
        created_by: &str,
        assignee: Option<&str>,
    ) {
        let archive_card_dir = state
            .repo_root
            .join("archive")
            .join("channels")
            .join(channel)
            .join("cards")
            .join(card_id);
        std::fs::create_dir_all(&archive_card_dir).unwrap();
        write_card_meta_full(
            &archive_card_dir.join("card.meta.yaml"),
            channel,
            created_by,
            assignee,
            "todo",
        );
        std::fs::write(archive_card_dir.join("discussion.thread"), "").unwrap();

        let run_git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&state.repo_root)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@test.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@test.com")
                .output()
                .unwrap()
        };
        run_git(&["add", "."]);
        run_git(&["commit", "-m", &format!("add archived card {}", card_id)]);
    }

    const UNARCHIVE_CARD_ID: &str = "20260102-120000-def";

    #[tokio::test]
    async fn test_unarchive_card_by_creator_success() {
        let (_tmp, state) = setup_test_repo().await;
        create_archived_card_fixture(&state, "dev", UNARCHIVE_CARD_ID, "alice", None).await;

        let mut event_rx = state.event_tx.subscribe();

        let resp = handle_unarchive_card(
            state.clone(),
            "dev".to_string(),
            UNARCHIVE_CARD_ID.to_string(),
            "alice".to_string(),
        )
        .await;
        assert!(resp.ok, "unarchive by creator should succeed: {:?}", resp.error);

        // Active path should now exist with card.meta.yaml
        let active_meta = state
            .repo_root
            .join("channels/dev/cards")
            .join(UNARCHIVE_CARD_ID)
            .join("card.meta.yaml");
        assert!(active_meta.exists(), "card.meta.yaml should exist in active location");

        // Archive path should be gone
        let archive_dir = state
            .repo_root
            .join("archive/channels/dev/cards")
            .join(UNARCHIVE_CARD_ID);
        assert!(!archive_dir.exists(), "archived dir should be gone after unarchive");

        // CardUnarchived event should be emitted
        let event = event_rx.try_recv().expect("should have received an event");
        match event {
            crate::api::Event::CardUnarchived { channel, card_id, author } => {
                assert_eq!(channel, "dev");
                assert_eq!(card_id, UNARCHIVE_CARD_ID);
                assert_eq!(author, "alice");
            }
            other => panic!("expected CardUnarchived, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_unarchive_card_by_assignee_success() {
        let (_tmp, state) = setup_test_repo().await;
        {
            let mut users = state.users.write().await;
            users.push("lewis".to_string());
        }
        std::fs::write(
            state.repo_root.join("users/lewis.meta.yaml"),
            "display_name: Lewis\nrole: dev\nintroduction: hi\n",
        )
        .unwrap();

        create_archived_card_fixture(&state, "dev", UNARCHIVE_CARD_ID, "alice", Some("lewis"))
            .await;

        let resp = handle_unarchive_card(
            state.clone(),
            "dev".to_string(),
            UNARCHIVE_CARD_ID.to_string(),
            "lewis".to_string(),
        )
        .await;
        assert!(resp.ok, "unarchive by assignee should succeed: {:?}", resp.error);

        let active_dir = state
            .repo_root
            .join("channels/dev/cards")
            .join(UNARCHIVE_CARD_ID);
        assert!(active_dir.exists(), "active dir should exist after unarchive");
    }

    #[tokio::test]
    async fn test_unarchive_card_rejects_non_creator_non_assignee() {
        let (_tmp, state) = setup_test_repo().await;
        // card created by alice, no assignee; bob tries to unarchive
        create_archived_card_fixture(&state, "dev", UNARCHIVE_CARD_ID, "alice", None).await;

        let resp = handle_unarchive_card(
            state.clone(),
            "dev".to_string(),
            UNARCHIVE_CARD_ID.to_string(),
            "bob".to_string(),
        )
        .await;
        assert!(!resp.ok, "non-creator/non-assignee should be rejected");
        let err = resp.error.unwrap();
        assert!(
            err.contains("only creator or assignee"),
            "error should mention permission: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_unarchive_card_rejects_not_archived() {
        let (_tmp, state) = setup_test_repo().await;
        // Place an active (non-archived) card
        create_card_for_archive(&state, "dev", UNARCHIVE_CARD_ID, "alice", None, "todo").await;

        let resp = handle_unarchive_card(
            state.clone(),
            "dev".to_string(),
            UNARCHIVE_CARD_ID.to_string(),
            "alice".to_string(),
        )
        .await;
        assert!(!resp.ok, "unarchiving a non-archived card should fail");
        let err = resp.error.unwrap();
        assert!(
            err.contains("not archived"),
            "error should mention 'not archived': {}",
            err
        );
    }

    #[tokio::test]
    async fn test_unarchive_card_rejects_unknown_card() {
        let (_tmp, state) = setup_test_repo().await;
        // No card setup at all
        let resp = handle_unarchive_card(
            state.clone(),
            "dev".to_string(),
            UNARCHIVE_CARD_ID.to_string(),
            "alice".to_string(),
        )
        .await;
        assert!(!resp.ok, "unknown card should be rejected");
        let err = resp.error.unwrap();
        assert!(
            err.contains("not found"),
            "error should mention 'not found': {}",
            err
        );
    }

    #[tokio::test]
    async fn test_unarchive_card_rejects_when_channel_archived() {
        // Card is archived; but the channel itself is also archived (thread file moved to archive/).
        // Fixture: place the card in archive/channels/dev/cards/<id> (as normal),
        // but also move the channel thread to archive/channels/ to simulate an archived channel.
        let (_tmp, state) = setup_test_repo().await;

        // Put the card in the archive location and commit it
        create_archived_card_fixture(&state, "dev", UNARCHIVE_CARD_ID, "alice", None).await;

        // Simulate channel "dev" being archived: remove channels/dev.thread
        // (channel_thread_exists checks for channels/<ch>.thread)
        let channel_thread = state.repo_root.join("channels/dev.thread");
        let run_git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&state.repo_root)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@test.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@test.com")
                .output()
                .unwrap()
        };
        // Move thread file to simulate archived channel
        let archive_ch_dir = state.repo_root.join("archive/channels");
        std::fs::create_dir_all(&archive_ch_dir).unwrap();
        std::fs::rename(&channel_thread, archive_ch_dir.join("dev.thread")).unwrap();
        run_git(&["add", "."]);
        run_git(&["commit", "-m", "archive channel dev"]);

        let resp = handle_unarchive_card(
            state.clone(),
            "dev".to_string(),
            UNARCHIVE_CARD_ID.to_string(),
            "alice".to_string(),
        )
        .await;
        assert!(!resp.ok, "unarchive into archived channel should fail");
        let err = resp.error.unwrap();
        assert!(
            err.contains("channel") && err.contains("not active"),
            "error should mention channel not active: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_unarchive_card_rejects_when_channel_deleted() {
        // Card is archived; but the channel does not exist at all (deleted).
        let (_tmp, state) = setup_test_repo().await;

        // Put the card in the archive location
        create_archived_card_fixture(&state, "dev", UNARCHIVE_CARD_ID, "alice", None).await;

        // Delete channels/dev.thread entirely to simulate a deleted channel
        let channel_thread = state.repo_root.join("channels/dev.thread");
        let run_git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&state.repo_root)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@test.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@test.com")
                .output()
                .unwrap()
        };
        std::fs::remove_file(&channel_thread).unwrap();
        run_git(&["add", "."]);
        run_git(&["commit", "-m", "delete channel dev"]);

        let resp = handle_unarchive_card(
            state.clone(),
            "dev".to_string(),
            UNARCHIVE_CARD_ID.to_string(),
            "alice".to_string(),
        )
        .await;
        assert!(!resp.ok, "unarchive into deleted channel should fail");
        let err = resp.error.unwrap();
        assert!(
            err.contains("channel") && err.contains("not active"),
            "error should mention channel not active: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_unarchive_card_rolls_back_git_mv_on_commit_failure() {
        let (_tmp, state) = setup_test_repo().await;
        let card_id = "20260102-120000-ee2";
        // Create card in archive (channel dev remains active)
        create_archived_card_fixture(&state, "dev", card_id, "alice", None).await;

        // Install a pre-commit hook that rejects all commits, triggering commit failure.
        let hooks_dir = state.repo_root.join(".git/hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        let hook_path = hooks_dir.join("pre-commit");
        std::fs::write(&hook_path, "#!/bin/sh\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let resp = handle_unarchive_card(
            state.clone(),
            "dev".to_string(),
            card_id.to_string(),
            "alice".to_string(),
        )
        .await;

        // Response must be an error
        assert!(!resp.ok, "unarchive should fail when commit is rejected");
        let err = resp.error.unwrap();
        assert!(
            err.contains("rolled back"),
            "error should mention rollback: {}",
            err
        );

        // Card must still be in archive location (rollback succeeded)
        let archive_meta = state
            .repo_root
            .join("archive/channels/dev/cards")
            .join(card_id)
            .join("card.meta.yaml");
        assert!(
            archive_meta.exists(),
            "card.meta.yaml should still be in archive location after rollback"
        );

        // Active location must not exist (no partial move left behind)
        let active_dir = state
            .repo_root
            .join("channels/dev/cards")
            .join(card_id);
        assert!(
            !active_dir.exists(),
            "active dir should not exist after rollback"
        );

        // Working tree should be clean (no staged git mv)
        let status_output = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&state.repo_root)
            .output()
            .unwrap();
        let status_str = String::from_utf8_lossy(&status_output.stdout);
        assert!(
            status_str.trim().is_empty(),
            "working tree should be clean after rollback, got: {}",
            status_str
        );
    }

    // ─── handle_list_archived_cards tests ────────────────────────────────────

    /// Write a CardMeta yaml directly into the archive location for a given channel and card_id.
    fn write_archived_card(state: &SharedState, channel: &str, card_id: &str) {
        let dir = state
            .repo_root
            .join("archive")
            .join("channels")
            .join(channel)
            .join("cards")
            .join(card_id);
        std::fs::create_dir_all(&dir).unwrap();
        let content = format!(
            "title: Archived Card\nchannel: {}\nstatus: todo\nlabels: []\nassignee: ~\ncreated_by: alice\ncreated_at: 20260101T000000Z\nupdated_at: 20260101T000000Z\n",
            channel
        );
        std::fs::write(dir.join("card.meta.yaml"), content).unwrap();
        std::fs::write(dir.join("discussion.thread"), "").unwrap();
    }

    #[tokio::test]
    async fn test_list_archived_cards_empty() {
        let (_tmp, state) = setup_test_repo().await;
        // No archive directory at all
        let resp = handle_list_archived_cards(state.clone(), None).await;
        assert!(resp.ok, "should succeed: {:?}", resp.error);
        let data = resp.data.unwrap();
        let cards = data["cards"].as_array().unwrap();
        assert!(cards.is_empty(), "no archived cards should return empty list");
    }

    #[tokio::test]
    async fn test_list_archived_cards_returns_all_when_no_channel_filter() {
        let (_tmp, state) = setup_test_repo().await;
        // channel "a": 2 cards, channel "b": 1 card
        write_archived_card(&state, "a", "20260101-000001-aaa");
        write_archived_card(&state, "a", "20260101-000002-bbb");
        write_archived_card(&state, "b", "20260101-000003-ccc");

        let resp = handle_list_archived_cards(state.clone(), None).await;
        assert!(resp.ok, "should succeed: {:?}", resp.error);
        let data = resp.data.unwrap();
        let cards = data["cards"].as_array().unwrap();
        assert_eq!(cards.len(), 3, "should return all 3 archived cards");

        // Verify stable sort by (channel, card_id)
        assert_eq!(cards[0]["channel"].as_str().unwrap(), "a");
        assert_eq!(cards[0]["card_id"].as_str().unwrap(), "20260101-000001-aaa");
        assert_eq!(cards[1]["channel"].as_str().unwrap(), "a");
        assert_eq!(cards[1]["card_id"].as_str().unwrap(), "20260101-000002-bbb");
        assert_eq!(cards[2]["channel"].as_str().unwrap(), "b");
        assert_eq!(cards[2]["card_id"].as_str().unwrap(), "20260101-000003-ccc");
    }

    #[tokio::test]
    async fn test_list_archived_cards_filters_by_channel() {
        let (_tmp, state) = setup_test_repo().await;
        write_archived_card(&state, "a", "20260101-000001-aaa");
        write_archived_card(&state, "a", "20260101-000002-bbb");
        write_archived_card(&state, "b", "20260101-000003-ccc");

        let resp = handle_list_archived_cards(state.clone(), Some("a".to_string())).await;
        assert!(resp.ok, "should succeed: {:?}", resp.error);
        let data = resp.data.unwrap();
        let cards = data["cards"].as_array().unwrap();
        assert_eq!(cards.len(), 2, "should return only channel 'a' cards");
        assert!(
            cards.iter().all(|c| c["channel"].as_str().unwrap() == "a"),
            "all returned cards should be from channel 'a'"
        );
    }

    #[tokio::test]
    async fn test_list_archived_cards_unknown_channel_returns_empty() {
        let (_tmp, state) = setup_test_repo().await;
        write_archived_card(&state, "a", "20260101-000001-aaa");

        // Channel "nonexistent" has no archived cards — should return empty, not error
        let resp = handle_list_archived_cards(state.clone(), Some("nonexistent".to_string())).await;
        assert!(resp.ok, "should succeed even for unknown channel: {:?}", resp.error);
        let data = resp.data.unwrap();
        let cards = data["cards"].as_array().unwrap();
        assert!(cards.is_empty(), "unknown channel should return empty list");
    }

    #[tokio::test]
    async fn test_list_archived_cards_ignores_active_cards() {
        let (_tmp, state) = setup_test_repo().await;
        // One archived card and one active card in the same channel
        write_archived_card(&state, "dev", "20260101-000001-arch");

        // Active card in channels/dev/cards/
        let active_dir = state
            .repo_root
            .join("channels/dev/cards/20260101-000002-active");
        std::fs::create_dir_all(&active_dir).unwrap();
        write_card_meta(&active_dir.join("card.meta.yaml"), "dev");

        let resp = handle_list_archived_cards(state.clone(), None).await;
        assert!(resp.ok, "should succeed: {:?}", resp.error);
        let data = resp.data.unwrap();
        let cards = data["cards"].as_array().unwrap();
        assert_eq!(cards.len(), 1, "should only return the archived card");
        assert_eq!(cards[0]["card_id"].as_str().unwrap(), "20260101-000001-arch");
    }
}
