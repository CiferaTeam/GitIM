use std::io::ErrorKind;

use crate::api::{Event, Response};
use crate::state::SharedState;
use gitim_core::responses::{
    BoardMetaSummary, BoardSummary, ListBoardsResponse, ReadBoardResponse, WriteBoardResponse,
};
use gitim_core::types::{
    append_board_section, board_path, default_board, parse_board_markdown, set_board_field,
    set_board_section, stringify_board_markdown, validate_board_for_handler, BoardDocument,
    BoardError, BoardMeta, Handler,
};

struct CommittedBoard {
    handler: String,
    path: String,
    commit_id: String,
}

pub async fn handle_board_show(state: SharedState, handler: String) -> Response {
    if let Err(e) = Handler::new(&handler) {
        return Response::error(format!("invalid handler: {}", e));
    }

    let rel = match board_path(&handler) {
        Ok(path) => path,
        Err(e) => return Response::error(e.to_string()),
    };
    let content = match read_board_content(&state, &rel, &handler) {
        Ok(content) => content,
        Err(resp) => return resp,
    };
    let doc = match parse_board_markdown(&content) {
        Ok(doc) => doc,
        Err(e) => return Response::error(format!("invalid board: {}", e)),
    };
    if let Err(e) = validate_board_for_handler(&doc, &handler) {
        return Response::error(e.to_string());
    }

    let payload = ReadBoardResponse {
        handler,
        path: rel.to_string_lossy().to_string(),
        meta: board_meta_summary(&doc.meta),
        body: doc.body,
    };
    Response::success(serde_json::to_value(payload).unwrap())
}

pub async fn handle_board_list(state: SharedState) -> Response {
    let root = state.repo_root.join("showboards");
    let mut boards = Vec::new();

    if root.exists() {
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(e) => return Response::error(format!("failed to list boards: {}", e)),
        };

        for entry in entries.flatten() {
            let handler = entry.file_name().to_string_lossy().to_string();
            if Handler::new(&handler).is_err() {
                continue;
            }
            let rel = match board_path(&handler) {
                Ok(path) => path,
                Err(_) => continue,
            };
            let Ok(content) = std::fs::read_to_string(state.repo_root.join(&rel)) else {
                continue;
            };
            let Ok(doc) = parse_board_markdown(&content) else {
                continue;
            };
            if validate_board_for_handler(&doc, &handler).is_err() {
                continue;
            }

            boards.push(BoardSummary {
                handler,
                path: rel.to_string_lossy().to_string(),
                updated_at: doc.meta.updated_at,
                status: doc.meta.status,
                summary: doc.meta.summary,
                tags: doc.meta.tags,
            });
        }
    }

    boards.sort_by(|a, b| a.handler.cmp(&b.handler));
    Response::success(serde_json::to_value(ListBoardsResponse { boards }).unwrap())
}

pub async fn handle_board_init(state: SharedState, author: String) -> Response {
    if let Err(resp) = ensure_known_user(&state, &author).await {
        return resp;
    }

    match init_board(&state, &author) {
        Ok(committed) => board_write_success(&state, committed),
        Err(resp) => resp,
    }
}

pub async fn handle_board_publish(
    state: SharedState,
    author: String,
    content: Option<String>,
) -> Response {
    if let Err(resp) = ensure_known_user(&state, &author).await {
        return resp;
    }

    match publish_board(&state, &author, content) {
        Ok(committed) => board_write_success(&state, committed),
        Err(resp) => resp,
    }
}

pub async fn handle_board_set(
    state: SharedState,
    author: String,
    field: String,
    value: String,
) -> Response {
    if let Err(resp) = ensure_known_user(&state, &author).await {
        return resp;
    }

    match mutate_existing_board(&state, &author, |doc| set_board_field(doc, &field, &value)) {
        Ok(committed) => board_write_success(&state, committed),
        Err(resp) => resp,
    }
}

pub async fn handle_board_section_set(
    state: SharedState,
    author: String,
    section: String,
    value: String,
) -> Response {
    if let Err(resp) = ensure_known_user(&state, &author).await {
        return resp;
    }

    match mutate_existing_board(&state, &author, |doc| {
        set_board_section(doc, &section, &value)
    }) {
        Ok(committed) => board_write_success(&state, committed),
        Err(resp) => resp,
    }
}

pub async fn handle_board_section_append(
    state: SharedState,
    author: String,
    section: String,
    value: String,
) -> Response {
    if let Err(resp) = ensure_known_user(&state, &author).await {
        return resp;
    }

    match mutate_existing_board(&state, &author, |doc| {
        append_board_section(doc, &section, &value)
    }) {
        Ok(committed) => board_write_success(&state, committed),
        Err(resp) => resp,
    }
}

async fn ensure_known_user(state: &SharedState, handler: &str) -> Result<(), Response> {
    let handler =
        Handler::new(handler).map_err(|e| Response::error(format!("invalid author: {}", e)))?;
    let users = state.users.read().await;
    if !users.iter().any(|user| user == handler.as_str()) {
        return Err(Response::error(format!("unknown user: {}", handler)));
    }
    Ok(())
}

fn init_board(state: &SharedState, author: &str) -> Result<CommittedBoard, Response> {
    let _guard = state.commit_lock.lock().expect("commit_lock poisoned");
    let now = current_timestamp();
    let doc = default_board(author, &now).map_err(|e| Response::error(e.to_string()))?;
    commit_board_document_locked(state, author, doc, "board: init")
}

fn publish_board(
    state: &SharedState,
    author: &str,
    content: Option<String>,
) -> Result<CommittedBoard, Response> {
    let _guard = state.commit_lock.lock().expect("commit_lock poisoned");
    let rel = board_path(author).map_err(|e| Response::error(e.to_string()))?;

    let mut doc = match content {
        Some(content) => parse_board_markdown(&content)
            .map_err(|e| Response::error(format!("invalid board: {}", e)))?,
        None => {
            let content = read_board_content(state, &rel, author)?;
            parse_board_markdown(&content)
                .map_err(|e| Response::error(format!("invalid board: {}", e)))?
        }
    };

    validate_board_for_handler(&doc, author).map_err(|e| Response::error(e.to_string()))?;
    doc.meta.updated_at = current_timestamp();
    commit_board_document_locked(state, author, doc, "board: update")
}

fn mutate_existing_board<F>(
    state: &SharedState,
    author: &str,
    mutate: F,
) -> Result<CommittedBoard, Response>
where
    F: FnOnce(&mut BoardDocument) -> Result<(), BoardError>,
{
    let _guard = state.commit_lock.lock().expect("commit_lock poisoned");
    let rel = board_path(author).map_err(|e| Response::error(e.to_string()))?;
    let content = read_board_content(state, &rel, author)?;
    let mut doc = parse_board_markdown(&content)
        .map_err(|e| Response::error(format!("invalid board: {}", e)))?;
    validate_board_for_handler(&doc, author).map_err(|e| Response::error(e.to_string()))?;

    mutate(&mut doc).map_err(|e| Response::error(e.to_string()))?;
    doc.meta.updated_at = current_timestamp();

    commit_board_document_locked(state, author, doc, "board: update")
}

fn commit_board_document_locked(
    state: &SharedState,
    author: &str,
    doc: BoardDocument,
    message_prefix: &str,
) -> Result<CommittedBoard, Response> {
    validate_board_for_handler(&doc, author).map_err(|e| Response::error(e.to_string()))?;
    let rel = board_path(author).map_err(|e| Response::error(e.to_string()))?;
    let rendered = stringify_board_markdown(&doc)
        .map_err(|e| Response::error(format!("invalid board: {}", e)))?;
    let abs = state.repo_root.join(&rel);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Response::error(format!("failed to create board dir: {}", e)))?;
    }
    std::fs::write(&abs, rendered)
        .map_err(|e| Response::error(format!("failed to write board: {}", e)))?;

    let path = rel.to_string_lossy().to_string();
    let message = format!("{} @{}", message_prefix, author);
    let (author_name, author_email) = state.author_for(author);
    let commit_id = state
        .git_storage
        .add_and_commit_only_as(&path, &message, Some((&author_name, &author_email)))
        .map_err(|e| Response::error(format!("board commit failed: {}", e)))?;

    Ok(CommittedBoard {
        handler: author.to_string(),
        path,
        commit_id,
    })
}

fn read_board_content(
    state: &SharedState,
    rel: &std::path::Path,
    handler: &str,
) -> Result<String, Response> {
    match std::fs::read_to_string(state.repo_root.join(rel)) {
        Ok(content) => Ok(content),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            Err(Response::error(format!("board not found for @{}", handler)))
        }
        Err(e) => Err(Response::error(format!("failed to read board: {}", e))),
    }
}

fn board_write_success(state: &SharedState, committed: CommittedBoard) -> Response {
    let _ = state.event_tx.send(Event::BoardUpdated {
        handler: committed.handler.clone(),
    });
    state.push_notify.notify_one();

    let payload = WriteBoardResponse {
        handler: committed.handler,
        path: committed.path,
        status: "committed".to_string(),
        commit_id: committed.commit_id,
    };
    Response::success(serde_json::to_value(payload).unwrap())
}

fn board_meta_summary(meta: &BoardMeta) -> BoardMetaSummary {
    BoardMetaSummary {
        version: meta.version,
        handler: meta.handler.clone(),
        updated_at: meta.updated_at.clone(),
        status: meta.status.clone(),
        summary: meta.summary.clone(),
        tags: meta.tags.clone(),
    }
}

fn current_timestamp() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}
