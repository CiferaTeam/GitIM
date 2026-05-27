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
    Response::json(payload)
}

pub async fn handle_board_list(state: SharedState) -> Response {
    let root = state.repo_root.join("showboards");
    let mut boards = Vec::new();
    let users = state.users.read().await;

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
            if !users.iter().any(|u| u == &handler) {
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
                labels: doc.meta.labels,
            });
        }
    }

    boards.sort_by(|a, b| a.handler.cmp(&b.handler));
    Response::json(ListBoardsResponse { boards })
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
    let _guard = state.commit_lock.lock().unwrap_or_else(|e| e.into_inner());
    let rel = board_path(author).map_err(|e| Response::error(e.to_string()))?;
    if state.repo_root.join(&rel).exists() {
        return Err(Response::error(format!(
            "board already exists for @{}",
            author
        )));
    }
    let now = current_timestamp();
    let doc = default_board(author, &now).map_err(|e| Response::error(e.to_string()))?;
    commit_board_document_locked(state, author, doc, "board: init")
}

fn publish_board(
    state: &SharedState,
    author: &str,
    content: Option<String>,
) -> Result<CommittedBoard, Response> {
    let _guard = state.commit_lock.lock().unwrap_or_else(|e| e.into_inner());
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
    let _guard = state.commit_lock.lock().unwrap_or_else(|e| e.into_inner());
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
    Response::json(payload)
}

fn board_meta_summary(meta: &BoardMeta) -> BoardMetaSummary {
    BoardMetaSummary {
        version: meta.version,
        handler: meta.handler.clone(),
        updated_at: meta.updated_at.clone(),
        status: meta.status.clone(),
        summary: meta.summary.clone(),
        labels: meta.labels.clone(),
    }
}

fn current_timestamp() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use gitim_core::types::config::Config;
    use std::sync::Arc;
    use tokio::sync::broadcast;

    fn setup_state(tmp: &std::path::Path) -> SharedState {
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
        for (k, v) in [("user.email", "test@test.com"), ("user.name", "Test")] {
            std::process::Command::new("git")
                .args(["config", k, v])
                .current_dir(&repo)
                .output()
                .unwrap();
        }
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
        let (tx, _) = broadcast::channel(16);
        Arc::new(AppState::new(repo, Config::default(), tx, None))
    }

    fn write_board(state: &SharedState, handler: &str) {
        let board_dir = state.repo_root.join(format!("showboards/{}", handler));
        std::fs::create_dir_all(&board_dir).unwrap();
        let content = format!(
            "---\nversion: 1\nhandler: {}\nupdated_at: 20260525T000000Z\nstatus: active\nsummary: test\nlabels: []\n---\n",
            handler
        );
        std::fs::write(board_dir.join("board.md"), &content).unwrap();
        std::process::Command::new("git")
            .args(["add", &format!("showboards/{}/board.md", handler)])
            .current_dir(&state.repo_root)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", &format!("board: init @{}", handler)])
            .current_dir(&state.repo_root)
            .output()
            .unwrap();
    }

    #[tokio::test]
    async fn board_list_excludes_archived_users() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());

        // alice and bob are active; carol is archived.
        {
            let mut users = state.users.write().await;
            users.push("alice".to_string());
            users.push("bob".to_string());
            users.sort();
        }

        write_board(&state, "alice");
        write_board(&state, "bob");
        write_board(&state, "carol");

        let resp = handle_board_list(state.clone()).await;
        assert!(resp.ok, "board_list failed: {:?}", resp.error);
        let data: ListBoardsResponse = serde_json::from_value(resp.data.unwrap()).unwrap();
        let handlers: Vec<String> = data.boards.iter().map(|b| b.handler.clone()).collect();

        assert!(handlers.contains(&"alice".to_string()));
        assert!(handlers.contains(&"bob".to_string()));
        assert!(
            !handlers.contains(&"carol".to_string()),
            "archived user's board should be excluded, got: {:?}",
            handlers
        );
    }
}
