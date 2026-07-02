use crate::api::Response;
use crate::handlers::ensure_author_not_departed;
use crate::state::SharedState;
use gitim_core::types::{
    validate_quick_session_id, Handler, QuickSessionListItem, QuickSessionMeta, QuickSessionStatus,
    QuickSessionTitleSource, QUICK_SESSION_ID_PREFIX,
};
use gitim_sync::git::GitError;
use tracing::warn;

const QUICK_SESSION_ID_RETRIES: u32 = 3;

/// Generate a ULID (26-char Crockford base32).
/// 10-char timestamp (ms) + 16-char random.
fn generate_ulid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let mut ts = now;
    let mut ts_chars = Vec::with_capacity(10);
    let crockford = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    for _ in 0..10 {
        ts_chars.push(crockford[(ts % 32) as usize] as char);
        ts /= 32;
    }
    ts_chars.reverse();

    let rand_chars: String = (0..16)
        .map(|_| crockford[rand::random::<u8>() as usize % 32] as char)
        .collect();

    format!("{}{}", ts_chars.into_iter().collect::<String>(), rand_chars)
}

fn generate_quick_session_id() -> String {
    format!("{}{}", QUICK_SESSION_ID_PREFIX, generate_ulid())
}

/// Check if quick-sessions/<id>/ exists; retry on collision up to N times.
fn generate_unique_session_id(state: &SharedState) -> Result<String, String> {
    for _ in 0..QUICK_SESSION_ID_RETRIES {
        let id = generate_quick_session_id();
        let session_dir = state.repo_root.join("quick-sessions").join(&id);
        if !session_dir.exists() {
            return Ok(id);
        }
    }
    Err(format!(
        "quick session id collision after {} retries",
        QUICK_SESSION_ID_RETRIES
    ))
}

fn session_dir(state: &SharedState, session_id: &str) -> std::path::PathBuf {
    state.repo_root.join("quick-sessions").join(session_id)
}

fn meta_path(state: &SharedState, session_id: &str) -> std::path::PathBuf {
    session_dir(state, session_id).join("session.meta.yaml")
}

fn thread_path(state: &SharedState, session_id: &str) -> std::path::PathBuf {
    session_dir(state, session_id).join("discussion.thread")
}

fn read_meta(state: &SharedState, session_id: &str) -> Result<QuickSessionMeta, String> {
    let path = meta_path(state, session_id);
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("failed to read session meta: {}", e))?;
    serde_yaml::from_str::<QuickSessionMeta>(&content)
        .map_err(|e| format!("failed to parse session meta: {}", e))
}

fn write_meta(
    state: &SharedState,
    session_id: &str,
    meta: &QuickSessionMeta,
) -> Result<(), String> {
    let path = meta_path(state, session_id);
    std::fs::create_dir_all(path.parent().unwrap())
        .map_err(|e| format!("failed to create session dir: {}", e))?;
    let yaml =
        serde_yaml::to_string(meta).map_err(|e| format!("failed to serialize meta: {}", e))?;
    std::fs::write(&path, yaml).map_err(|e| format!("failed to write meta: {}", e))
}

/// Create a new quick session.
pub async fn handle_create_quick_session(
    state: SharedState,
    agent_id: String,
    first_message: String,
    author: String,
) -> Response {
    // Validate author
    if let Err(resp) = ensure_author_not_departed(&state, &author) {
        return resp;
    }

    // Validate agent exists
    {
        let users = state.users.read().await;
        if !users.contains(&agent_id) {
            return Response::error(format!("unknown agent: {}", agent_id));
        }
    }

    // Generate unique session ID
    let session_id = match generate_unique_session_id(&state) {
        Ok(id) => id,
        Err(e) => return Response::error(e),
    };

    let now = chrono::Utc::now();
    let ts = now.format("%Y%m%dT%H%M%SZ").to_string();

    let meta = QuickSessionMeta {
        id: session_id.clone(),
        title: String::new(),
        title_source: QuickSessionTitleSource::None,
        agent_id: agent_id.clone(),
        created_by: Handler::new(&author).unwrap_or_else(|_| Handler::new("system").unwrap()),
        status: QuickSessionStatus::NeedsTitle,
        created_at: ts.clone(),
        updated_at: ts.clone(),
        archived_at: None,
        summary: None,
        last_message_preview: None,
        ref_: Some(format!("session:{}", session_id)),
    };

    if let Err(e) = write_meta(&state, &session_id, &meta) {
        return Response::error(e);
    }

    // Write first user message to discussion thread
    let thread_line = format!(
        "{} {} {} {}",
        format_line_number(1),
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ"),
        author,
        first_message
    );
    let thread_path = thread_path(&state, &session_id);
    if let Err(e) = std::fs::write(&thread_path, thread_line + "\n") {
        return Response::error(format!("failed to write thread: {}", e));
    }

    // Git commit
    let rel_meta = format!("quick-sessions/{}/session.meta.yaml", session_id);
    let rel_thread = format!("quick-sessions/{}/discussion.thread", session_id);
    let msg = format!("quick-session: create {}", session_id);

    if let Err(e) = state
        .git_storage
        .add_and_commit(&[&rel_meta, &rel_thread], &msg)
    {
        return Response::error(format!("git commit failed: {}", e));
    }

    let _ = push_with_retry(&state).await;

    let resp = QuickSessionListItem {
        id: session_id.clone(),
        title: String::new(),
        agent_id: agent_id.clone(),
        status: QuickSessionStatus::NeedsTitle,
        updated_at: ts.clone(),
        ref_: format!("session:{}", session_id),
        last_message_preview: Some(first_message.chars().take(80).collect()),
    };
    Response::json(resp)
}

/// List active quick sessions.
pub async fn handle_list_quick_sessions(state: SharedState, include_archived: bool) -> Response {
    let qs_dir = state.repo_root.join("quick-sessions");
    if !qs_dir.exists() {
        return Response::json(Vec::<QuickSessionListItem>::new());
    }

    let mut items = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&qs_dir) {
        for entry in entries.flatten() {
            let meta_path = entry.path().join("session.meta.yaml");
            if !meta_path.exists() {
                continue;
            }
            let content = match std::fs::read_to_string(&meta_path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let meta: QuickSessionMeta = match serde_yaml::from_str(&content) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if !include_archived && meta.status == QuickSessionStatus::Archived {
                continue;
            }
            items.push(QuickSessionListItem {
                id: meta.id.clone(),
                title: meta.title.clone(),
                agent_id: meta.agent_id.clone(),
                status: meta.status.clone(),
                updated_at: meta.updated_at.clone(),
                ref_: meta.ref_string(),
                last_message_preview: meta.last_message_preview.clone(),
            });
        }
    }
    items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Response::json(items)
}

/// Read a quick session (metadata + transcript).
pub async fn handle_read_quick_session(state: SharedState, session_id: String) -> Response {
    if let Err(e) = validate_quick_session_id(&session_id) {
        return Response::error(format!("invalid session_id: {:?}", e));
    }
    let meta = match read_meta(&state, &session_id) {
        Ok(m) => m,
        Err(e) => return Response::error(e),
    };

    let thread_content =
        std::fs::read_to_string(thread_path(&state, &session_id)).unwrap_or_default();

    Response::json(serde_json::json!({
        "meta": meta,
        "thread": thread_content,
    }))
}

/// Set quick session title (title API gate).
pub async fn handle_set_quick_session_title(
    state: SharedState,
    session_id: String,
    title: String,
) -> Response {
    if let Err(e) = validate_quick_session_id(&session_id) {
        return Response::error(format!("invalid session_id: {:?}", e));
    }
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return Response::error("title cannot be empty");
    }
    if trimmed.len() > 80 {
        return Response::error("title too long (max 80 chars)");
    }

    let mut meta = match read_meta(&state, &session_id) {
        Ok(m) => m,
        Err(e) => return Response::error(e),
    };

    meta.title = trimmed.to_string();
    meta.title_source = QuickSessionTitleSource::ApiSet;
    if meta.status == QuickSessionStatus::NeedsTitle {
        meta.status = QuickSessionStatus::Active;
    }
    meta.updated_at = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();

    if let Err(e) = write_meta(&state, &session_id, &meta) {
        return Response::error(e);
    }

    let rel = format!("quick-sessions/{}/session.meta.yaml", session_id);
    let msg = format!("quick-session: set title {}", session_id);
    if let Err(e) = state.git_storage.add_and_commit(&[&rel], &msg) {
        return Response::error(format!("git commit failed: {}", e));
    }
    let _ = push_with_retry(&state).await;

    Response::json(&meta)
}

/// Append a message to a quick session thread.  
/// Enforces the title API gate: if status is `needs_title` and the author
/// is not the session creator, the message is blocked until a title is set.
pub async fn handle_send_quick_session_message(
    state: SharedState,
    session_id: String,
    body: String,
    author: String,
) -> Response {
    if let Err(e) = validate_quick_session_id(&session_id) {
        return Response::error(format!("invalid session_id: {:?}", e));
    }
    if let Err(resp) = ensure_author_not_departed(&state, &author) {
        return resp;
    }

    let meta = match read_meta(&state, &session_id) {
        Ok(m) => m,
        Err(e) => return Response::error(e),
    };

    if meta.status == QuickSessionStatus::Archived {
        return Response::error("cannot send message to archived session");
    }

    // Title API gate: block assistant output until title is set.
    // The session creator (human) is exempt so they can send the first message
    // and any follow-up messages before the agent sets a title.
    if meta.status == QuickSessionStatus::NeedsTitle && author != meta.created_by.as_str() {
        return Response::error(
            "QUICK_SESSION_TITLE_REQUIRED: agent must call set_quick_session_title before sending assistant content",
        );
    }

    let tp = thread_path(&state, &session_id);
    let existing = std::fs::read_to_string(&tp).unwrap_or_default();
    let line_count = existing.lines().count();
    let new_line_number = (line_count + 1) as u64;

    let line = format!(
        "{} {} {} {}",
        format_line_number(new_line_number),
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ"),
        author,
        body
    );
    if let Err(e) = std::fs::write(&tp, existing + &line + "\n") {
        return Response::error(format!("failed to write thread: {}", e));
    }

    let rel = format!("quick-sessions/{}/discussion.thread", session_id);
    let msg_text = format!("quick-session: message {}", session_id);
    if let Err(e) = state.git_storage.add_and_commit(&[&rel], &msg_text) {
        return Response::error(format!("git commit failed: {}", e));
    }
    let _ = push_with_retry(&state).await;

    Response::json(serde_json::json!({
        "line_number": new_line_number,
    }))
}

/// Archive a quick session.
pub async fn handle_archive_quick_session(
    state: SharedState,
    session_id: String,
    author: String,
) -> Response {
    if let Err(e) = validate_quick_session_id(&session_id) {
        return Response::error(format!("invalid session_id: {:?}", e));
    }
    if let Err(resp) = ensure_author_not_departed(&state, &author) {
        return resp;
    }

    let mut meta = match read_meta(&state, &session_id) {
        Ok(m) => m,
        Err(e) => return Response::error(e),
    };

    meta.status = QuickSessionStatus::Archived;
    meta.archived_at = Some(chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string());
    meta.updated_at = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();

    if let Err(e) = write_meta(&state, &session_id, &meta) {
        return Response::error(e);
    }

    let rel = format!("quick-sessions/{}/session.meta.yaml", session_id);
    let msg = format!("quick-session: archive {}", session_id);
    if let Err(e) = state.git_storage.add_and_commit(&[&rel], &msg) {
        return Response::error(format!("git commit failed: {}", e));
    }
    let _ = push_with_retry(&state).await;

    Response::json(&meta)
}

fn format_line_number(n: u64) -> String {
    format!("L{:06}", n)
}

/// Push to remote with bounded retries on retryable errors.
pub(crate) async fn push_with_retry(state: &SharedState) -> Result<(), String> {
    for attempt in 1..=3 {
        match state.git_storage.push() {
            Ok(()) => return Ok(()),
            Err(GitError::PushConflict) | Err(GitError::RateLimited) => {
                if attempt == 3 {
                    warn!("push failed after {} attempts", attempt);
                    return Err("push failed after 3 attempts".into());
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            Err(e) => return Err(format!("push error: {}", e)),
        }
    }
    Ok(())
}
