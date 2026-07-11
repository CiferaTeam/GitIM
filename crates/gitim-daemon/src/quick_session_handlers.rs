use crate::api::{Event, Response};
use crate::card_handlers::push_with_retry;
use crate::handlers::ensure_author_not_departed;
use crate::state::{PendingMessage, SharedState};

use gitim_core::formatter::format_message;
use gitim_core::parser::parse_thread;
use gitim_core::responses::{
    ArchiveQuickSessionResponse, ClaimQuickSessionTurnResponse, CreateQuickSessionResponse,
    ListQuickSessionsResponse, MarkQuickSessionErrorResponse, QuickSessionDetail,
    QuickSessionListItem, ReadQuickSessionResponse, SendQuickSessionMessageResponse,
    SetQuickSessionSummaryResponse, SetQuickSessionTitleResponse, UnarchiveQuickSessionResponse,
};
use gitim_core::types::{
    apply_quick_session_transition, validate_quick_session_id, validate_quick_session_meta,
    Handler, QuickSessionError, QuickSessionMeta, QuickSessionStatus, QuickSessionTransition,
    ThreadEntry, TransitionOutcome,
};
use gitim_core::validator::compliance::validate_append;
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::{error, warn};

const QUICK_SESSION_MESSAGE_MAX_BYTES: usize = 64 * 1024;

#[derive(Clone)]
struct LocatedQuickSession {
    rel_dir: String,
    archived: bool,
}

fn timestamp() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}

fn error_response(error: QuickSessionError) -> Response {
    let code = match error {
        QuickSessionError::UnauthorizedActor => "quick_session_forbidden",
        QuickSessionError::StaleAttempt => "quick_session_stale_attempt",
        QuickSessionError::TitleRequired => "quick_session_title_required",
        QuickSessionError::InvalidState
        | QuickSessionError::StaleInputLine
        | QuickSessionError::InputLineMismatch
        | QuickSessionError::InvalidLineNumber => "quick_session_invalid_state",
        QuickSessionError::InvalidSessionId => "invalid_quick_session_id",
        _ => "invalid_quick_session",
    };
    Response::error_with_code(error.to_string(), code)
}

fn validate_body(body: &str) -> Result<(), Response> {
    if body.trim().is_empty() {
        return Err(Response::error_with_code(
            "quick session message cannot be empty",
            "invalid_quick_session_message",
        ));
    }
    if body.len() > QUICK_SESSION_MESSAGE_MAX_BYTES {
        return Err(Response::error_with_code(
            "quick session message exceeds 64 KB",
            "invalid_quick_session_message",
        ));
    }
    Ok(())
}

fn canonical_message_body(author: &Handler, body: &str) -> Result<String, Response> {
    let formatted = format_message(1, 0, author, "19700101T000000Z", body);
    let parsed = parse_thread(&formatted)
        .map_err(|error| Response::error(format!("failed to normalize message: {error}")))?;
    match parsed.entries.first() {
        Some(ThreadEntry::Message(message)) => Ok(message.body.clone()),
        _ => Err(Response::error("failed to normalize message")),
    }
}

fn atomic_write_file(path: &Path, content: impl AsRef<[u8]>) -> std::io::Result<()> {
    atomic_write_file_with_hook(path, content, |_| Ok(()))
}

fn atomic_write_file_with_hook<F>(
    path: &Path,
    content: impl AsRef<[u8]>,
    before_persist: F,
) -> std::io::Result<()>
where
    F: FnOnce(&Path) -> std::io::Result<()>,
{
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "atomic write destination has no parent",
        )
    })?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(content.as_ref())?;
    temporary.as_file_mut().sync_all()?;
    before_persist(temporary.path())?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn session_rel_dir(session_id: &str, archived: bool) -> Result<String, Response> {
    validate_quick_session_id(session_id).map_err(error_response)?;
    Ok(if archived {
        format!("archive/quick-sessions/{session_id}")
    } else {
        format!("quick-sessions/{session_id}")
    })
}

fn locate_session(
    state: &SharedState,
    session_id: &str,
) -> Result<Option<LocatedQuickSession>, Response> {
    let active = session_rel_dir(session_id, false)?;
    let archived = session_rel_dir(session_id, true)?;
    let active_exists = state
        .repo_root
        .join(&active)
        .join("session.meta.yaml")
        .is_file();
    let archived_exists = state
        .repo_root
        .join(&archived)
        .join("session.meta.yaml")
        .is_file();
    if active_exists && archived_exists {
        warn!(
            session_id,
            "quick session exists in active and archive; preferring active"
        );
    }
    Ok(if active_exists {
        Some(LocatedQuickSession {
            rel_dir: active,
            archived: false,
        })
    } else if archived_exists {
        Some(LocatedQuickSession {
            rel_dir: archived,
            archived: true,
        })
    } else {
        None
    })
}

fn load_meta(
    state: &SharedState,
    located: &LocatedQuickSession,
) -> Result<QuickSessionMeta, Response> {
    let path = state
        .repo_root
        .join(&located.rel_dir)
        .join("session.meta.yaml");
    let content = std::fs::read_to_string(&path)
        .map_err(|e| Response::error(format!("failed to read quick session metadata: {e}")))?;
    let meta: QuickSessionMeta = serde_yaml::from_str(&content)
        .map_err(|e| Response::error(format!("failed to parse quick session metadata: {e}")))?;
    validate_quick_session_meta(&meta).map_err(error_response)?;
    Ok(meta)
}

fn load_thread(
    state: &SharedState,
    located: &LocatedQuickSession,
) -> Result<(String, Vec<ThreadEntry>), Response> {
    let path = state
        .repo_root
        .join(&located.rel_dir)
        .join("discussion.thread");
    let content = std::fs::read_to_string(&path)
        .map_err(|e| Response::error(format!("failed to read quick session thread: {e}")))?;
    let parsed = parse_thread(&content)
        .map_err(|e| Response::error(format!("failed to parse quick session thread: {e}")))?;
    Ok((content, parsed.entries))
}

fn bounded_entries(
    entries: Vec<ThreadEntry>,
    limit: Option<usize>,
    since: Option<u64>,
) -> Vec<ThreadEntry> {
    let mut entries: Vec<ThreadEntry> = entries
        .into_iter()
        .filter(|entry| since.is_none_or(|line| entry.line_number() > line))
        .collect();
    if let Some(limit) = limit {
        if since.is_some() {
            entries.truncate(limit);
        } else {
            let drain = entries.len().saturating_sub(limit);
            entries.drain(..drain);
        }
    }
    entries
}

fn detail(
    state: &SharedState,
    located: &LocatedQuickSession,
    limit: Option<usize>,
    since: Option<u64>,
) -> Result<QuickSessionDetail, Response> {
    let meta = load_meta(state, located)?;
    let (_, entries) = load_thread(state, located)?;
    Ok(QuickSessionDetail {
        meta,
        entries: bounded_entries(entries, limit, since),
        archived: located.archived,
    })
}

fn ensure_active_user(state: &SharedState, handler: &str, role: &str) -> Result<Handler, Response> {
    let parsed =
        Handler::new(handler).map_err(|e| Response::error(format!("invalid {role}: {e}")))?;
    let active = state
        .repo_root
        .join("users")
        .join(format!("{handler}.meta.yaml"));
    let archived = state
        .repo_root
        .join("archive/users")
        .join(format!("{handler}.meta.yaml"));
    if archived.exists() {
        return Err(Response::error(format!("{role} @{handler} is departed")));
    }
    if !active.is_file() {
        return Err(Response::error(format!("unknown {role}: {handler}")));
    }
    Ok(parsed)
}

fn yaml(meta: &QuickSessionMeta) -> Result<String, Response> {
    Response::yaml_string(meta, "quick session metadata")
}

fn reset_paths(repo_root: &Path, paths: &[&str], context: &str) {
    if paths.is_empty() {
        return;
    }
    let output = std::process::Command::new("git")
        .arg("reset")
        .arg("HEAD")
        .arg("--")
        .args(paths)
        .current_dir(repo_root)
        .output();
    match output {
        Ok(output) if !output.status.success() => warn!(
            context,
            stderr = %String::from_utf8_lossy(&output.stderr),
            "quick session rollback reset failed"
        ),
        Err(error) => warn!(context, %error, "quick session rollback reset failed"),
        _ => {}
    }
}

fn restore_files(state: &SharedState, paths: &[(&Path, &str)], rel_paths: &[&str], context: &str) {
    reset_paths(&state.repo_root, rel_paths, context);
    for (path, content) in paths {
        if let Err(error) = atomic_write_file(path, content) {
            error!(context, path = %path.display(), %error, "quick session rollback write failed");
        }
    }
}

fn emit_changed(state: &SharedState, meta: &QuickSessionMeta) {
    let _ = state.event_tx.send(Event::QuickSessionChanged {
        session_id: meta.id.clone(),
        agent_id: meta.agent_id.clone(),
        status: meta.status,
        revision: meta.revision,
    });
}

async fn push_after_commit(state: &SharedState, operation: &str) -> Result<(), Response> {
    push_with_retry(state, operation)
        .await
        .map_err(Response::error)
}

pub async fn handle_create_quick_session(
    state: SharedState,
    session_id: String,
    agent_id: String,
    first_message: String,
    author: String,
) -> Response {
    if let Err(response) = validate_body(&first_message) {
        return response;
    }
    if let Err(error) = validate_quick_session_id(&session_id) {
        return error_response(error);
    }
    let creator = match ensure_active_user(&state, &author, "creator") {
        Ok(handler) => handler,
        Err(response) => return response,
    };
    if let Err(response) = ensure_author_not_departed(&state, &author) {
        return response;
    }
    if let Err(response) = ensure_active_user(&state, &agent_id, "agent") {
        return response;
    }
    let first_message = match canonical_message_body(&creator, &first_message) {
        Ok(body) => body,
        Err(response) => return response,
    };

    let registered_users = state.users.read().await.clone();
    let registered_refs: Vec<&str> = registered_users.iter().map(String::as_str).collect();
    let allowed = [author.as_str(), agent_id.as_str()];
    let guard = state.commit_lock.lock().unwrap_or_else(|e| e.into_inner());

    let existing = match locate_session(&state, &session_id) {
        Ok(existing) => existing,
        Err(response) => return response,
    };
    if let Some(located) = existing {
        let existing_detail = match detail(&state, &located, None, None) {
            Ok(detail) => detail,
            Err(response) => return response,
        };
        let first_matches = matches!(
            existing_detail.entries.first(),
            Some(ThreadEntry::Message(message))
                if message.line_number == 1
                    && message.point_to == 0
                    && message.author.as_str() == author
                    && message.body == first_message
        );
        if existing_detail.meta.id != session_id
            || existing_detail.meta.agent_id != agent_id
            || existing_detail.meta.created_by != author
            || !first_matches
        {
            return Response::error_with_code(
                "quick session id collides with a different object",
                "quick_session_id_collision",
            );
        }
        let session_ref = existing_detail.meta.ref_string();
        return Response::json(CreateQuickSessionResponse {
            session: existing_detail,
            line_number: 1,
            r#ref: session_ref,
        });
    }

    let now = timestamp();
    let mut meta = QuickSessionMeta::new(
        session_id.clone(),
        agent_id.clone(),
        author.clone(),
        now.clone(),
    );
    if let Err(error) = apply_quick_session_transition(
        &mut meta,
        QuickSessionTransition::HumanMessage {
            actor: author.clone(),
            line_number: 1,
            request_id: None,
            preview: first_message.clone(),
            now: now.clone(),
        },
    ) {
        return error_response(error);
    }
    let thread = format_message(1, 0, &creator, &now, &first_message);
    if let Err(error) = validate_append("", &thread, &registered_refs, &allowed) {
        return Response::error(format!("invalid quick session message: {error}"));
    }
    let meta_yaml = match yaml(&meta) {
        Ok(yaml) => yaml,
        Err(response) => return response,
    };
    let rel_dir = match session_rel_dir(&session_id, false) {
        Ok(path) => path,
        Err(response) => return response,
    };
    let dir = state.repo_root.join(&rel_dir);
    let meta_rel = format!("{rel_dir}/session.meta.yaml");
    let thread_rel = format!("{rel_dir}/discussion.thread");
    if let Err(error) = std::fs::create_dir_all(&dir)
        .and_then(|()| atomic_write_file(&dir.join("session.meta.yaml"), &meta_yaml))
        .and_then(|()| atomic_write_file(&dir.join("discussion.thread"), &thread))
    {
        let _ = std::fs::remove_dir_all(&dir);
        return Response::error(format!("failed to write quick session: {error}"));
    }
    let (name, email) = state.author_for(&author);
    let commit_message = format!("session: create {session_id} for @{agent_id} by @{author}");
    if let Err(error) = state.git_storage.add_and_commit_as(
        &[&meta_rel, &thread_rel],
        &commit_message,
        Some((&name, &email)),
    ) {
        reset_paths(
            &state.repo_root,
            &[&meta_rel, &thread_rel],
            "create_quick_session",
        );
        if let Err(rollback) = std::fs::remove_dir_all(&dir) {
            warn!(%rollback, "create quick session rollback directory removal failed");
        }
        return Response::error(format!(
            "create quick session commit failed: {error}; rolled back"
        ));
    }
    drop(guard);
    if let Err(response) = push_after_commit(&state, "create_quick_session").await {
        return response;
    }
    emit_changed(&state, &meta);

    Response::json(CreateQuickSessionResponse {
        session: QuickSessionDetail {
            meta: meta.clone(),
            entries: parse_thread(&thread)
                .map(|parsed| parsed.entries)
                .unwrap_or_default(),
            archived: false,
        },
        line_number: 1,
        r#ref: meta.ref_string(),
    })
}

pub async fn handle_list_quick_sessions(
    state: SharedState,
    archived: bool,
    agent_id: Option<String>,
    actionable: bool,
    limit: Option<usize>,
) -> Response {
    if let Some(agent_id) = agent_id.as_deref() {
        if let Err(error) = Handler::new(agent_id) {
            return Response::error(format!("invalid agent: {error}"));
        }
    }
    let root = if archived {
        state.repo_root.join("archive/quick-sessions")
    } else {
        state.repo_root.join("quick-sessions")
    };
    let mut sessions = Vec::new();
    if let Ok(directories) = std::fs::read_dir(root) {
        for directory in directories.flatten() {
            let session_id = directory.file_name().to_string_lossy().to_string();
            let located = match session_rel_dir(&session_id, archived) {
                Ok(rel_dir) => LocatedQuickSession { rel_dir, archived },
                Err(_) => {
                    warn!(session_id, "ignoring invalid quick session directory");
                    continue;
                }
            };
            let meta = match load_meta(&state, &located) {
                Ok(meta) => meta,
                Err(response) => {
                    warn!(session_id, error = ?response.error, "ignoring invalid quick session metadata");
                    continue;
                }
            };
            if agent_id
                .as_deref()
                .is_some_and(|agent| agent != meta.agent_id)
            {
                continue;
            }
            if actionable {
                if !matches!(
                    meta.status,
                    QuickSessionStatus::NeedsTitle | QuickSessionStatus::Active
                ) {
                    continue;
                }
                let (_, entries) = match load_thread(&state, &located) {
                    Ok(thread) => thread,
                    Err(response) => {
                        warn!(session_id, error = ?response.error, "ignoring invalid quick session thread");
                        continue;
                    }
                };
                let newest_creator_line = entries
                    .iter()
                    .filter_map(|entry| match entry {
                        ThreadEntry::Message(message)
                            if message.author.as_str() == meta.created_by =>
                        {
                            Some(message.line_number)
                        }
                        _ => None,
                    })
                    .max();
                if newest_creator_line.is_none_or(|line| {
                    meta.last_completed_input_line
                        .is_some_and(|completed| line <= completed)
                }) {
                    continue;
                }
            }
            sessions.push(QuickSessionListItem::from_meta(&meta, archived));
        }
    }
    sessions.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    sessions.truncate(limit.unwrap_or(100).clamp(1, 100));
    Response::json(ListQuickSessionsResponse { sessions })
}

pub async fn handle_read_quick_session(
    state: SharedState,
    session_id: String,
    limit: Option<usize>,
    since: Option<u64>,
) -> Response {
    let located = match locate_session(&state, &session_id) {
        Ok(Some(located)) => located,
        Ok(None) => return Response::error("quick session not found"),
        Err(response) => return response,
    };
    match detail(&state, &located, limit, since) {
        Ok(session) => Response::json(ReadQuickSessionResponse { session }),
        Err(response) => response,
    }
}

pub async fn handle_send_quick_session_message(
    state: SharedState,
    session_id: String,
    body: String,
    reply_to: Option<u64>,
    request_id: Option<String>,
    attempt_id: Option<String>,
    author: String,
) -> Response {
    if let Err(response) = validate_body(&body) {
        return response;
    }
    let handler = match ensure_active_user(&state, &author, "author") {
        Ok(handler) => handler,
        Err(response) => return response,
    };
    if let Err(response) = ensure_author_not_departed(&state, &author) {
        return response;
    }
    let registered_users = state.users.read().await.clone();
    let registered_refs: Vec<&str> = registered_users.iter().map(String::as_str).collect();
    let guard = state.commit_lock.lock().unwrap_or_else(|e| e.into_inner());
    let located = match locate_session(&state, &session_id) {
        Ok(Some(located)) if !located.archived => located,
        Ok(Some(_)) => return error_response(QuickSessionError::InvalidState),
        Ok(None) => return Response::error("quick session not found"),
        Err(response) => return response,
    };
    let mut meta = match load_meta(&state, &located) {
        Ok(meta) => meta,
        Err(response) => return response,
    };
    let (old_thread, entries) = match load_thread(&state, &located) {
        Ok(thread) => thread,
        Err(response) => return response,
    };
    let is_creator = author == meta.created_by;
    let is_agent = author == meta.agent_id;
    if !is_creator && !is_agent {
        return error_response(QuickSessionError::UnauthorizedActor);
    }
    if is_creator && request_id.as_deref().is_none_or(str::is_empty) {
        return Response::error_with_code(
            "creator messages require request_id",
            "invalid_quick_session_message",
        );
    }
    if is_agent && attempt_id.as_deref().is_none_or(str::is_empty) {
        return Response::error_with_code(
            "agent messages require attempt_id",
            "quick_session_stale_attempt",
        );
    }
    let completed_retry = is_agent
        && attempt_id.as_deref() == meta.last_completed_attempt_id.as_deref()
        && reply_to == meta.last_completed_input_line;
    if is_agent && !completed_retry && reply_to != meta.processing_input_line {
        return error_response(QuickSessionError::InputLineMismatch);
    }

    let next_line = entries.last().map_or(1, |entry| entry.line_number() + 1);
    let now = timestamp();
    let transition = if is_creator {
        QuickSessionTransition::HumanMessage {
            actor: author.clone(),
            line_number: next_line,
            request_id,
            preview: body.clone(),
            now: now.clone(),
        }
    } else {
        QuickSessionTransition::AgentReply {
            actor: author.clone(),
            input_line: reply_to.unwrap_or(0),
            attempt_id: attempt_id.unwrap_or_default(),
            output_line: next_line,
            preview: body.clone(),
            now: now.clone(),
        }
    };
    let outcome = match apply_quick_session_transition(&mut meta, transition) {
        Ok(outcome) => outcome,
        Err(error) => return error_response(error),
    };
    if let TransitionOutcome::Duplicate { line_number } = outcome {
        return Response::json(SendQuickSessionMessageResponse {
            session_id,
            line_number: line_number.unwrap_or(next_line),
            status: meta.status,
            revision: meta.revision,
            r#ref: meta.ref_string(),
        });
    }

    let new_message = format_message(next_line, reply_to.unwrap_or(0), &handler, &now, &body);
    let allowed = [meta.created_by.as_str(), meta.agent_id.as_str()];
    if let Err(error) = validate_append(&old_thread, &new_message, &registered_refs, &allowed) {
        return Response::error(format!("invalid quick session message: {error}"));
    }
    let mut new_thread = old_thread.clone();
    new_thread.push_str(&new_message);
    let new_meta = match yaml(&meta) {
        Ok(yaml) => yaml,
        Err(response) => return response,
    };
    let meta_path = state
        .repo_root
        .join(&located.rel_dir)
        .join("session.meta.yaml");
    let thread_path = state
        .repo_root
        .join(&located.rel_dir)
        .join("discussion.thread");
    let old_meta = match std::fs::read_to_string(&meta_path) {
        Ok(content) => content,
        Err(error) => {
            return Response::error(format!("failed to read quick session metadata: {error}"))
        }
    };
    if let Err(error) = atomic_write_file(&meta_path, &new_meta)
        .and_then(|()| atomic_write_file(&thread_path, &new_thread))
    {
        restore_files(
            &state,
            &[(&meta_path, &old_meta), (&thread_path, &old_thread)],
            &[],
            "send_quick_session_message",
        );
        return Response::error(format!("failed to write quick session message: {error}"));
    }
    let meta_rel = format!("{}/session.meta.yaml", located.rel_dir);
    let thread_rel = format!("{}/discussion.thread", located.rel_dir);
    let (name, email) = state.author_for(&author);
    let commit_message = format!("session-msg: @{author} -> {session_id} L{next_line:06}");
    if let Err(error) = state.git_storage.add_and_commit_as(
        &[&meta_rel, &thread_rel],
        &commit_message,
        Some((&name, &email)),
    ) {
        restore_files(
            &state,
            &[(&meta_path, &old_meta), (&thread_path, &old_thread)],
            &[&meta_rel, &thread_rel],
            "send_quick_session_message",
        );
        return Response::error(format!(
            "send quick session message commit failed: {error}; rolled back"
        ));
    }
    drop(guard);
    if let Err(response) = push_after_commit(&state, "send_quick_session_message").await {
        return response;
    }
    state
        .pending_push
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .push(PendingMessage {
            channel: format!("quick_session:{session_id}"),
            line_number: next_line,
        });
    emit_changed(&state, &meta);
    Response::json(SendQuickSessionMessageResponse {
        session_id,
        line_number: next_line,
        status: meta.status,
        revision: meta.revision,
        r#ref: meta.ref_string(),
    })
}

fn apply_meta_transition(
    state: &SharedState,
    session_id: &str,
    author: &str,
    transition: QuickSessionTransition,
    operation: &str,
) -> Result<(QuickSessionMeta, bool), Response> {
    let located = locate_session(state, session_id)?
        .ok_or_else(|| Response::error("quick session not found"))?;
    if located.archived {
        return Err(error_response(QuickSessionError::InvalidState));
    }
    let mut meta = load_meta(state, &located)?;
    let old_meta = std::fs::read_to_string(
        state
            .repo_root
            .join(&located.rel_dir)
            .join("session.meta.yaml"),
    )
    .map_err(|e| Response::error(format!("failed to read quick session metadata: {e}")))?;
    let outcome = apply_quick_session_transition(&mut meta, transition).map_err(error_response)?;
    if matches!(outcome, TransitionOutcome::Duplicate { .. }) {
        return Ok((meta, false));
    }
    let new_meta = yaml(&meta)?;
    let meta_path = state
        .repo_root
        .join(&located.rel_dir)
        .join("session.meta.yaml");
    atomic_write_file(&meta_path, new_meta)
        .map_err(|e| Response::error(format!("failed to write quick session metadata: {e}")))?;
    let meta_rel = format!("{}/session.meta.yaml", located.rel_dir);
    let (name, email) = state.author_for(author);
    let commit_message = format!("session: {operation} {session_id} by @{author}");
    if let Err(error) =
        state
            .git_storage
            .add_and_commit_as(&[&meta_rel], &commit_message, Some((&name, &email)))
    {
        restore_files(state, &[(&meta_path, &old_meta)], &[&meta_rel], operation);
        return Err(Response::error(format!(
            "{operation} commit failed: {error}; rolled back"
        )));
    }
    Ok((meta, true))
}

pub async fn handle_claim_quick_session_turn(
    state: SharedState,
    session_id: String,
    input_line: u64,
    attempt_id: String,
    author: String,
) -> Response {
    if let Err(response) = ensure_active_user(&state, &author, "author") {
        return response;
    }
    let guard = state.commit_lock.lock().unwrap_or_else(|e| e.into_inner());
    let located = match locate_session(&state, &session_id) {
        Ok(Some(located)) if !located.archived => located,
        Ok(Some(_)) => return error_response(QuickSessionError::InvalidState),
        Ok(None) => return Response::error("quick session not found"),
        Err(response) => return response,
    };
    let meta = match load_meta(&state, &located) {
        Ok(meta) => meta,
        Err(response) => return response,
    };
    if meta.agent_id != author {
        return error_response(QuickSessionError::UnauthorizedActor);
    }
    let (_, entries) = match load_thread(&state, &located) {
        Ok(thread) => thread,
        Err(response) => return response,
    };
    let latest_creator_line = entries
        .iter()
        .filter_map(|entry| match entry {
            ThreadEntry::Message(message) if message.author.as_str() == meta.created_by => {
                Some(message.line_number)
            }
            _ => None,
        })
        .max();
    let is_duplicate = meta.status == QuickSessionStatus::Running
        && meta.processing_input_line == Some(input_line)
        && meta.attempt_id.as_deref() == Some(&attempt_id);
    if !is_duplicate && latest_creator_line != Some(input_line) {
        return error_response(QuickSessionError::InputLineMismatch);
    }
    let transition = QuickSessionTransition::Claim {
        actor: author.clone(),
        input_line,
        attempt_id: attempt_id.clone(),
        now: timestamp(),
    };
    let (meta, changed) = match apply_meta_transition(
        &state,
        &session_id,
        &author,
        transition,
        "claim quick session",
    ) {
        Ok(result) => result,
        Err(response) => return response,
    };
    drop(guard);
    if changed {
        if let Err(response) = push_after_commit(&state, "claim_quick_session_turn").await {
            return response;
        }
        emit_changed(&state, &meta);
    }
    Response::json(ClaimQuickSessionTurnResponse {
        session_id,
        input_line,
        attempt_id,
        status: meta.status,
        revision: meta.revision,
    })
}

pub async fn handle_set_quick_session_title(
    state: SharedState,
    session_id: String,
    title: String,
    attempt_id: String,
    author: String,
) -> Response {
    mutate_agent_meta(
        state,
        session_id,
        author,
        QuickSessionTransition::SetTitle {
            actor: String::new(),
            attempt_id,
            title,
            now: timestamp(),
        },
        "set quick session title",
        |meta| SetQuickSessionTitleResponse {
            session_id: meta.id.clone(),
            title: meta.title.clone().unwrap_or_default(),
            status: meta.status,
            revision: meta.revision,
        },
    )
    .await
}

pub async fn handle_set_quick_session_summary(
    state: SharedState,
    session_id: String,
    summary: String,
    attempt_id: String,
    author: String,
) -> Response {
    mutate_agent_meta(
        state,
        session_id,
        author,
        QuickSessionTransition::SetSummary {
            actor: String::new(),
            attempt_id,
            summary,
            now: timestamp(),
        },
        "set quick session summary",
        |meta| SetQuickSessionSummaryResponse {
            session_id: meta.id.clone(),
            summary: meta.summary.clone().unwrap_or_default(),
            status: meta.status,
            revision: meta.revision,
        },
    )
    .await
}

pub async fn handle_mark_quick_session_error(
    state: SharedState,
    session_id: String,
    attempt_id: String,
    diagnostic: String,
    author: String,
) -> Response {
    mutate_agent_meta(
        state,
        session_id,
        author,
        QuickSessionTransition::MarkError {
            actor: String::new(),
            attempt_id,
            error: diagnostic,
            now: timestamp(),
        },
        "mark quick session error",
        |meta| MarkQuickSessionErrorResponse {
            session_id: meta.id.clone(),
            status: meta.status,
            revision: meta.revision,
        },
    )
    .await
}

async fn mutate_agent_meta<T: serde::Serialize>(
    state: SharedState,
    session_id: String,
    author: String,
    mut transition: QuickSessionTransition,
    operation: &str,
    response: impl FnOnce(&QuickSessionMeta) -> T,
) -> Response {
    if let Err(response) = ensure_active_user(&state, &author, "author") {
        return response;
    }
    match &mut transition {
        QuickSessionTransition::SetTitle { actor, .. }
        | QuickSessionTransition::SetSummary { actor, .. }
        | QuickSessionTransition::MarkError { actor, .. } => *actor = author.clone(),
        _ => {}
    }
    let guard = state.commit_lock.lock().unwrap_or_else(|e| e.into_inner());
    let result = apply_meta_transition(&state, &session_id, &author, transition, operation);
    let (meta, changed) = match result {
        Ok(result) => result,
        Err(response) => return response,
    };
    drop(guard);
    if changed {
        if let Err(response) = push_after_commit(&state, operation).await {
            return response;
        }
        emit_changed(&state, &meta);
    }
    Response::json(response(&meta))
}

async fn move_session(
    state: SharedState,
    session_id: String,
    author: String,
    archive: bool,
) -> Response {
    if let Err(response) = ensure_active_user(&state, &author, "creator") {
        return response;
    }
    let guard = state.commit_lock.lock().unwrap_or_else(|e| e.into_inner());
    let located = match locate_session(&state, &session_id) {
        Ok(Some(located)) => located,
        Ok(None) => return Response::error("quick session not found"),
        Err(response) => return response,
    };
    if located.archived == archive {
        return error_response(QuickSessionError::InvalidState);
    }
    let mut meta = match load_meta(&state, &located) {
        Ok(meta) => meta,
        Err(response) => return response,
    };
    let old_meta = match std::fs::read_to_string(
        state
            .repo_root
            .join(&located.rel_dir)
            .join("session.meta.yaml"),
    ) {
        Ok(content) => content,
        Err(error) => {
            return Response::error(format!("failed to read quick session metadata: {error}"))
        }
    };
    let transition = if archive {
        QuickSessionTransition::Archive {
            actor: author.clone(),
            now: timestamp(),
        }
    } else {
        QuickSessionTransition::Unarchive {
            actor: author.clone(),
            now: timestamp(),
        }
    };
    if let Err(error) = apply_quick_session_transition(&mut meta, transition) {
        return error_response(error);
    }
    let new_meta = match yaml(&meta) {
        Ok(yaml) => yaml,
        Err(response) => return response,
    };
    let source_rel = located.rel_dir;
    let target_rel = match session_rel_dir(&session_id, archive) {
        Ok(rel) => rel,
        Err(response) => return response,
    };
    let source_meta = state.repo_root.join(&source_rel).join("session.meta.yaml");
    if let Err(error) = atomic_write_file(&source_meta, new_meta) {
        return Response::error(format!("failed to write quick session metadata: {error}"));
    }
    let target_parent = state
        .repo_root
        .join(&target_rel)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(&state.repo_root));
    if let Err(error) = std::fs::create_dir_all(target_parent) {
        if let Err(rollback) = atomic_write_file(&source_meta, &old_meta) {
            error!(%rollback, "quick session metadata rollback failed");
        }
        return Response::error(format!(
            "failed to create quick session archive directory: {error}"
        ));
    }
    if let Err(error) = state.git_storage.mv(&source_rel, &target_rel) {
        if let Err(rollback) = atomic_write_file(&source_meta, &old_meta) {
            error!(%rollback, "quick session metadata rollback failed");
        }
        return Response::error(format!("failed to move quick session: {error}"));
    }
    let target_meta_rel = format!("{target_rel}/session.meta.yaml");
    let target_thread_rel = format!("{target_rel}/discussion.thread");
    let (name, email) = state.author_for(&author);
    let verb = if archive { "archive" } else { "unarchive" };
    let commit_message = format!("session: {verb} {session_id} by @{author}");
    if let Err(error) = state.git_storage.add_and_commit_as(
        &[&target_meta_rel, &target_thread_rel],
        &commit_message,
        Some((&name, &email)),
    ) {
        if let Err(rollback) = state.git_storage.mv(&target_rel, &source_rel) {
            error!(%rollback, "quick session move rollback failed");
        }
        let source_meta_rel = format!("{source_rel}/session.meta.yaml");
        reset_paths(
            &state.repo_root,
            &[&source_meta_rel, &target_meta_rel, &target_thread_rel],
            verb,
        );
        if let Err(rollback) = atomic_write_file(&source_meta, &old_meta) {
            error!(%rollback, "quick session metadata rollback failed");
        }
        return Response::error(format!(
            "{verb} quick session commit failed: {error}; rolled back"
        ));
    }
    drop(guard);
    if let Err(response) = push_after_commit(&state, verb).await {
        return response;
    }
    emit_changed(&state, &meta);
    if archive {
        Response::json(ArchiveQuickSessionResponse {
            session_id,
            status: meta.status,
            revision: meta.revision,
            archived_at: meta.archived_at.clone().unwrap_or_default(),
        })
    } else {
        Response::json(UnarchiveQuickSessionResponse {
            session_id,
            status: meta.status,
            revision: meta.revision,
        })
    }
}

pub async fn handle_archive_quick_session(
    state: SharedState,
    session_id: String,
    author: String,
) -> Response {
    move_session(state, session_id, author, true).await
}

pub async fn handle_unarchive_quick_session(
    state: SharedState,
    session_id: String,
    author: String,
) -> Response {
    move_session(state, session_id, author, false).await
}

#[cfg(test)]
mod atomic_write_tests {
    use super::*;

    #[test]
    fn atomic_write_failure_preserves_original_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("session.meta.yaml");
        std::fs::write(&path, b"original").unwrap();

        let result = atomic_write_file_with_hook(&path, b"replacement", |_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "injected persist failure",
            ))
        });

        assert!(result.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }
}
