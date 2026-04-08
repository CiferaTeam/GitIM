use crate::api::{Event, Response};
use crate::state::{PendingMessage, PushResult, SharedState};
use crate::thread_io;
use gitim_core::types::{BoardMeta, CardMeta, ChannelName, Handler};
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

pub async fn handle_create_board(
    state: SharedState,
    name: String,
    display_name: Option<String>,
    statuses: Option<Vec<String>>,
    author: String,
) -> Response {
    // 1. Validate author
    let _handler = match Handler::new(&author) {
        Ok(h) => h,
        Err(e) => return Response::error(format!("invalid author: {}", e)),
    };
    {
        let users = state.users.read().await;
        if !users.contains(&author) {
            return Response::error(format!("unknown user: {}", author));
        }
    }

    // 2. Validate board name (reuse ChannelName)
    let board_name = match ChannelName::new(&name) {
        Ok(n) => n,
        Err(e) => return Response::error(format!("invalid board name: {}", e)),
    };

    // 3. Check board doesn't already exist
    let boards_dir = state.repo_root.join("boards");
    let board_dir = boards_dir.join(board_name.to_string());
    let meta_path = board_dir.join("board.meta.yaml");
    if meta_path.exists() {
        return Response::error(format!("board '{}' already exists", name));
    }

    // 4. Create board directory
    if let Err(e) = std::fs::create_dir_all(&board_dir) {
        return Response::error(format!("failed to create board dir: {}", e));
    }

    // 5. Write board.meta.yaml
    let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    // Validate statuses is non-empty
    let statuses = statuses.unwrap_or_else(|| {
        vec!["todo".to_string(), "in-progress".to_string(), "done".to_string()]
    });
    if statuses.is_empty() {
        return Response::error("statuses list cannot be empty");
    }
    let meta = BoardMeta {
        name: name.clone(),
        display_name: display_name.unwrap_or_else(|| name.clone()),
        created_by: author.clone(),
        created_at: now,
        statuses,
    };
    let meta_str = serde_yaml::to_string(&meta).unwrap();
    if let Err(e) = std::fs::write(&meta_path, &meta_str) {
        return Response::error(format!("failed to write board meta: {}", e));
    }

    // 6. Commit
    let meta_rel = format!("boards/{}/board.meta.yaml", board_name);
    let commit_msg = format!("board: create {} by @{}", name, author);
    if let Err(e) = state
        .git_storage
        .add_and_commit_as(&[&meta_rel], &commit_msg, Some(&author))
    {
        return Response::error(format!("create_board commit failed: {}", e));
    }

    // 7. Push with retry
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
                        "create_board: push conflict (attempt {}/{}), rebasing",
                        attempt, MAX_PUSH_RETRIES
                    );
                    if let Err(e) = state.git_storage.fetch() {
                        return Response::error(format!("create_board fetch failed: {}", e));
                    }
                    if let Err(e) = state.git_storage.rebase_onto_origin() {
                        return Response::error(format!("create_board rebase failed: {}", e));
                    }
                }
                Err(e) => {
                    return Response::error(format!("create_board push failed: {}", e));
                }
            }
        }
        if !pushed {
            return Response::error(format!(
                "create_board: push still conflicting after {} retries",
                MAX_PUSH_RETRIES
            ));
        }
    }

    info!("board '{}' created by @{}", name, author);

    Response::success(serde_json::json!({
        "board": name,
        "created_by": author,
    }))
}

pub async fn handle_create_card(
    state: SharedState,
    board: String,
    title: String,
    assignee: Option<String>,
    status: Option<String>,
    author: String,
) -> Response {
    // 1. Validate author
    let _handler = match Handler::new(&author) {
        Ok(h) => h,
        Err(e) => return Response::error(format!("invalid author: {}", e)),
    };
    {
        let users = state.users.read().await;
        if !users.contains(&author) {
            return Response::error(format!("unknown user: {}", author));
        }
    }

    // 2. Validate board name
    let board_name = match ChannelName::new(&board) {
        Ok(n) => n,
        Err(e) => return Response::error(format!("invalid board name: {}", e)),
    };

    // 3. Read board meta
    let board_dir = state.repo_root.join("boards").join(board_name.to_string());
    let board_meta_path = board_dir.join("board.meta.yaml");
    let board_meta: BoardMeta = match std::fs::read_to_string(&board_meta_path) {
        Ok(content) => match serde_yaml::from_str(&content) {
            Ok(m) => m,
            Err(e) => return Response::error(format!("failed to parse board meta: {}", e)),
        },
        Err(_) => return Response::error(format!("board '{}' does not exist", board)),
    };

    // Validate assignee if provided
    if let Some(ref a) = assignee {
        let users = state.users.read().await;
        if !users.contains(a) {
            return Response::error(format!("assignee '{}' is not a registered user", a));
        }
    }

    // 4. Validate status
    let card_status = status.unwrap_or_else(|| {
        board_meta.statuses.first().cloned().unwrap_or_else(|| "todo".to_string())
    });
    if !board_meta.statuses.contains(&card_status) {
        return Response::error(format!(
            "invalid status '{}', allowed: {:?}",
            card_status, board_meta.statuses
        ));
    }

    // 5. Generate card_id and create card directory
    let card_id = generate_card_id();
    let card_dir = board_dir.join(&card_id);
    if let Err(e) = std::fs::create_dir_all(&card_dir) {
        return Response::error(format!("failed to create card dir: {}", e));
    }

    // 6. Write card.meta.yaml
    let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let card_meta = CardMeta {
        title: title.clone(),
        status: card_status,
        assignee,
        created_by: author.clone(),
        created_at: now.clone(),
        updated_at: now,
    };
    let meta_str = serde_yaml::to_string(&card_meta).unwrap();
    let card_meta_path = card_dir.join("card.meta.yaml");
    if let Err(e) = std::fs::write(&card_meta_path, &meta_str) {
        return Response::error(format!("failed to write card meta: {}", e));
    }

    // 7. Create empty .thread file
    let thread_path = card_dir.join("discussion.thread");
    if let Err(e) = std::fs::write(&thread_path, "") {
        return Response::error(format!("failed to write card thread: {}", e));
    }

    // 8. Commit
    let meta_rel = format!("boards/{}/{}/card.meta.yaml", board_name, card_id);
    let thread_rel = format!("boards/{}/{}/discussion.thread", board_name, card_id);
    let commit_msg = format!("card: create {} in {} by @{}", card_id, board, author);
    if let Err(e) = state
        .git_storage
        .add_and_commit_as(&[&meta_rel, &thread_rel], &commit_msg, Some(&author))
    {
        return Response::error(format!("create_card commit failed: {}", e));
    }

    // 9. Push with retry
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
                        "create_card: push conflict (attempt {}/{}), rebasing",
                        attempt, MAX_PUSH_RETRIES
                    );
                    if let Err(e) = state.git_storage.fetch() {
                        return Response::error(format!("create_card fetch failed: {}", e));
                    }
                    if let Err(e) = state.git_storage.rebase_onto_origin() {
                        return Response::error(format!("create_card rebase failed: {}", e));
                    }
                }
                Err(e) => {
                    return Response::error(format!("create_card push failed: {}", e));
                }
            }
        }
        if !pushed {
            return Response::error(format!(
                "create_card: push still conflicting after {} retries",
                MAX_PUSH_RETRIES
            ));
        }
    }

    // 10. Broadcast event
    let _ = state.event_tx.send(Event::CardCreated {
        board: board.clone(),
        card_id: card_id.clone(),
    });

    info!(
        "card '{}' created in board '{}' by @{}",
        card_id, board, author
    );

    Response::success(serde_json::json!({
        "board": board,
        "card_id": card_id,
        "title": title,
    }))
}

pub async fn handle_list_boards(state: SharedState) -> Response {
    let mut boards: Vec<serde_json::Value> = Vec::new();

    let boards_dir = state.repo_root.join("boards");
    if boards_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&boards_dir) {
            for entry in entries.flatten() {
                if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }
                let meta_path = entry.path().join("board.meta.yaml");
                if let Ok(content) = std::fs::read_to_string(&meta_path) {
                    if let Ok(meta) = serde_yaml::from_str::<BoardMeta>(&content) {
                        boards.push(serde_json::json!({
                            "name": meta.name,
                            "display_name": meta.display_name,
                            "created_by": meta.created_by,
                            "created_at": meta.created_at,
                            "statuses": meta.statuses,
                        }));
                    }
                }
            }
        }
    }

    boards.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Response::success(serde_json::json!({ "boards": boards }))
}

pub async fn handle_list_cards(
    state: SharedState,
    board: String,
    status: Option<String>,
) -> Response {
    // Validate board name
    let board_name = match ChannelName::new(&board) {
        Ok(n) => n,
        Err(e) => return Response::error(format!("invalid board name: {}", e)),
    };

    let board_dir = state.repo_root.join("boards").join(board_name.to_string());
    if !board_dir.join("board.meta.yaml").exists() {
        return Response::error(format!("board '{}' does not exist", board));
    }

    let mut cards: Vec<serde_json::Value> = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&board_dir) {
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let card_meta_path = entry.path().join("card.meta.yaml");
            if let Ok(content) = std::fs::read_to_string(&card_meta_path) {
                if let Ok(meta) = serde_yaml::from_str::<CardMeta>(&content) {
                    // Filter by status if provided
                    if let Some(ref s) = status {
                        if &meta.status != s {
                            continue;
                        }
                    }
                    let card_id = entry.file_name().to_string_lossy().to_string();
                    cards.push(serde_json::json!({
                        "card_id": card_id,
                        "title": meta.title,
                        "status": meta.status,
                        "assignee": meta.assignee,
                        "created_by": meta.created_by,
                        "created_at": meta.created_at,
                        "updated_at": meta.updated_at,
                    }));
                }
            }
        }
    }

    cards.sort_by(|a, b| a["card_id"].as_str().cmp(&b["card_id"].as_str()));
    Response::success(serde_json::json!({
        "board": board,
        "cards": cards,
    }))
}

pub async fn handle_read_card(
    state: SharedState,
    board: String,
    card_id: String,
    limit: Option<usize>,
    since: Option<u64>,
) -> Response {
    // Validate board name
    let board_name = match ChannelName::new(&board) {
        Ok(n) => n,
        Err(e) => return Response::error(format!("invalid board name: {}", e)),
    };

    // Validate card_id
    if let Err(e) = validate_card_id(&card_id) {
        return Response::error(format!("invalid card_id: {}", e));
    }

    let card_dir = state
        .repo_root
        .join("boards")
        .join(board_name.to_string())
        .join(&card_id);

    // Read card meta
    let card_meta_path = card_dir.join("card.meta.yaml");
    let card_meta: CardMeta = match std::fs::read_to_string(&card_meta_path) {
        Ok(content) => match serde_yaml::from_str(&content) {
            Ok(m) => m,
            Err(e) => return Response::error(format!("failed to parse card meta: {}", e)),
        },
        Err(_) => {
            return Response::error(format!(
                "card '{}' not found in board '{}'",
                card_id, board
            ))
        }
    };

    // Read thread entries
    let thread_path = card_dir.join("discussion.thread");
    let entries = match thread_io::read_thread_entries(&thread_path, limit, since) {
        Ok(e) => e,
        Err(e) => return Response::error(e),
    };

    Response::success(serde_json::json!({
        "board": board,
        "card_id": card_id,
        "meta": {
            "title": card_meta.title,
            "status": card_meta.status,
            "assignee": card_meta.assignee,
            "created_by": card_meta.created_by,
            "created_at": card_meta.created_at,
            "updated_at": card_meta.updated_at,
        },
        "entries": entries,
    }))
}

pub async fn handle_send_card_message(
    state: SharedState,
    board: String,
    card_id: String,
    body: String,
    reply_to: Option<u64>,
    author: String,
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
    }

    // 2. Validate board name
    let board_name = match ChannelName::new(&board) {
        Ok(n) => n,
        Err(e) => return Response::error(format!("invalid board name: {}", e)),
    };

    // Validate card_id
    if let Err(e) = validate_card_id(&card_id) {
        return Response::error(format!("invalid card_id: {}", e));
    }

    // 3. Check card exists
    let card_dir = state
        .repo_root
        .join("boards")
        .join(board_name.to_string())
        .join(&card_id);
    let card_meta_path = card_dir.join("card.meta.yaml");
    if !card_meta_path.exists() {
        return Response::error(format!(
            "card '{}' not found in board '{}'",
            card_id, board
        ));
    }

    // 4. Ensure thread dir exists
    let thread_path = card_dir.join("discussion.thread");
    if let Some(parent) = thread_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    // 5. Append message
    let (next_line, _new_content) =
        match thread_io::append_message_to_thread(&thread_path, &handler, &body, reply_to) {
            Ok(v) => v,
            Err(e) => return Response::error(e),
        };

    // 6. Git commit
    let thread_name = format!("boards/{}/{}", board_name, card_id);
    let thread_rel = format!("{}/discussion.thread", thread_name);
    let commit_msg = format!(
        "card-msg: @{} -> {}/{} L{:06}",
        author, board, card_id, next_line
    );
    let commit_status = match state
        .git_storage
        .add_and_commit_as(&[&thread_rel], &commit_msg, Some(&author))
    {
        Ok(()) => "committed",
        Err(e) => {
            warn!(
                "git commit failed for L{:06} in {}/{}: {}",
                next_line, board, card_id, e
            );
            "written"
        }
    };

    // 7. Record pending_push and optionally await push result
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

    info!(
        "card message sent to {}/{} by @{} at L{:06}",
        board, card_id, author, next_line
    );

    // 8. Await push result if applicable
    if let Some(rx) = push_rx {
        state.push_notify.notify_one();
        match rx.await {
            Ok(PushResult::Pushed { commit_id }) => Response::success(serde_json::json!({
                "line_number": next_line,
                "board": board,
                "card_id": card_id,
                "status": "pushed",
                "commit_id": commit_id,
            })),
            Ok(PushResult::Failed { reason }) => Response::success(serde_json::json!({
                "line_number": next_line,
                "board": board,
                "card_id": card_id,
                "status": "commit_only",
                "error": reason,
            })),
            Err(_) => Response::success(serde_json::json!({
                "line_number": next_line,
                "board": board,
                "card_id": card_id,
                "status": "commit_only",
                "error": "push result channel closed",
            })),
        }
    } else {
        Response::success(serde_json::json!({
            "line_number": next_line,
            "board": board,
            "card_id": card_id,
            "status": commit_status,
        }))
    }
}

pub async fn handle_update_card(
    state: SharedState,
    board: String,
    card_id: String,
    status: Option<String>,
    assignee: Option<String>,
    author: String,
) -> Response {
    // 1. Validate author
    let _handler = match Handler::new(&author) {
        Ok(h) => h,
        Err(e) => return Response::error(format!("invalid author: {}", e)),
    };
    {
        let users = state.users.read().await;
        if !users.contains(&author) {
            return Response::error(format!("unknown user: {}", author));
        }
    }

    // 2. Validate board name
    let board_name = match ChannelName::new(&board) {
        Ok(n) => n,
        Err(e) => return Response::error(format!("invalid board name: {}", e)),
    };

    // Validate card_id
    if let Err(e) = validate_card_id(&card_id) {
        return Response::error(format!("invalid card_id: {}", e));
    }

    // 2.5. Check at least one field to update
    if status.is_none() && assignee.is_none() {
        return Response::error("must provide at least one field to update (status or assignee)");
    }

    // 3. Read board meta (for status validation)
    let board_dir = state.repo_root.join("boards").join(board_name.to_string());
    let board_meta_path = board_dir.join("board.meta.yaml");
    let board_meta: BoardMeta = match std::fs::read_to_string(&board_meta_path) {
        Ok(content) => match serde_yaml::from_str(&content) {
            Ok(m) => m,
            Err(e) => return Response::error(format!("failed to parse board meta: {}", e)),
        },
        Err(_) => return Response::error(format!("board '{}' does not exist", board)),
    };

    // 4. Read card meta
    let card_dir = board_dir.join(&card_id);
    let card_meta_path = card_dir.join("card.meta.yaml");
    let mut card_meta: CardMeta = match std::fs::read_to_string(&card_meta_path) {
        Ok(content) => match serde_yaml::from_str(&content) {
            Ok(m) => m,
            Err(e) => return Response::error(format!("failed to parse card meta: {}", e)),
        },
        Err(_) => {
            return Response::error(format!(
                "card '{}' not found in board '{}'",
                card_id, board
            ))
        }
    };

    // 5. Validate and apply status change
    let old_status = card_meta.status.clone();
    if let Some(ref new_status) = status {
        if !board_meta.statuses.contains(new_status) {
            return Response::error(format!(
                "invalid status '{}', allowed: {:?}",
                new_status, board_meta.statuses
            ));
        }
        card_meta.status = new_status.clone();
    }

    // Validate assignee if provided
    if let Some(ref new_assignee) = assignee {
        let users = state.users.read().await;
        if !users.contains(new_assignee) {
            return Response::error(format!("assignee '{}' is not a registered user", new_assignee));
        }
    }

    // 6. Apply assignee change
    if let Some(ref new_assignee) = assignee {
        card_meta.assignee = Some(new_assignee.clone());
    }

    // 7. Update timestamp
    let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    card_meta.updated_at = now;

    // 8. Write card meta
    let meta_str = serde_yaml::to_string(&card_meta).unwrap();
    if let Err(e) = std::fs::write(&card_meta_path, &meta_str) {
        return Response::error(format!("failed to write card meta: {}", e));
    }

    // 9. Commit
    let meta_rel = format!("boards/{}/{}/card.meta.yaml", board_name, card_id);
    let commit_msg = format!("card: update {} in {} by @{}", card_id, board, author);
    if let Err(e) = state
        .git_storage
        .add_and_commit_as(&[&meta_rel], &commit_msg, Some(&author))
    {
        return Response::error(format!("update_card commit failed: {}", e));
    }

    // 10. Push with retry
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
                        "update_card: push conflict (attempt {}/{}), rebasing",
                        attempt, MAX_PUSH_RETRIES
                    );
                    if let Err(e) = state.git_storage.fetch() {
                        return Response::error(format!("update_card fetch failed: {}", e));
                    }
                    if let Err(e) = state.git_storage.rebase_onto_origin() {
                        return Response::error(format!("update_card rebase failed: {}", e));
                    }
                }
                Err(e) => {
                    return Response::error(format!("update_card push failed: {}", e));
                }
            }
        }
        if !pushed {
            return Response::error(format!(
                "update_card: push still conflicting after {} retries",
                MAX_PUSH_RETRIES
            ));
        }
    }

    // 11. Broadcast CardStatusChanged event if status changed
    if let Some(ref new_status) = status {
        if old_status != *new_status {
            let _ = state.event_tx.send(Event::CardStatusChanged {
                board: board.clone(),
                card_id: card_id.clone(),
                old_status: old_status.clone(),
                new_status: new_status.clone(),
                author: author.clone(),
            });
        }
    }

    info!("card '{}' updated in board '{}' by @{}", card_id, board, author);

    Response::success(serde_json::json!({
        "board": board,
        "card_id": card_id,
        "status": card_meta.status,
        "assignee": card_meta.assignee,
    }))
}
