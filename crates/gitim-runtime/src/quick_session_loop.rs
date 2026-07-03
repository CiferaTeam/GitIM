/// QuickSessionLoop — background task that executes quick session turns.
///
/// Runs alongside AgentLoop to handle quick-session messages assigned to
/// agents on this runtime. Key invariants:
///
/// - One QuickSessionLoop per workspace, not per agent.
/// - Per-agent FIFO serialization via shared `AgentLockMap` so main
///   AgentLoop and QuickSessionLoop never race on the same provider.
/// - Fresh provider per turn: instantiate, execute, tear down.
/// - Title gate enforced in the execution path before any turn.
/// - Status `needs_title` → skip; `active`/`running` → eligible.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gitim_agent_provider::{create, ExecOptions, ExecStatus};
use gitim_client::GitimClient;
use serde_json::{json, Value};
use tokio::sync::{broadcast, Mutex};
use tracing::{debug, info, warn};

use crate::agent_loop::build_provider_config;
use crate::http::{AgentActivityEvent, AgentInfo, SharedRuntimeState};
use crate::quick_session_runner::{check_title_gate, title_gate_prompt_instruction};
use crate::quick_session_state;
use gitim_core::parser::parse_thread;
use gitim_core::types::{
    QuickSessionMeta, QuickSessionStatus, ThreadEntry, MAX_QUICK_SESSION_TITLE_LEN,
};

/// Shared per-agent lock map to serialize main agent loop and quick session turns.
pub type AgentLockMap = Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>;

pub fn new_agent_lock_map() -> AgentLockMap {
    Arc::new(Mutex::new(HashMap::new()))
}

async fn acquire_agent_lock(locks: &AgentLockMap, agent_id: &str) -> Arc<Mutex<()>> {
    let mut map = locks.lock().await;
    map.entry(agent_id.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// Run the quick session loop indefinitely.
///
/// Periodic daemon poll for sessions → dispatch turns for local agents.
/// Idempotent per workspace: if multiple agents share a workspace, only
/// one loop instance should run. The caller guards against duplicate
/// spawns via `WorkspaceContext::quick_session_loop_running`.
pub async fn run_quick_session_loop(
    // Retained in signature for call-site compatibility; daemon client now
    // binds to workspace_root (stable workspace-level path) instead of the
    // first agent's repo_root, so QuickSessionLoop is independent of agent
    // startup order.
    _repo_root: PathBuf,
    workspace_root: PathBuf,
    state: SharedRuntimeState,
    slug: String,
    activity_tx: broadcast::Sender<AgentActivityEvent>,
    agent_locks: AgentLockMap,
    poll_interval: Duration,
) {
    info!(slug = %slug, "quick session loop started");

    // Bind daemon client to workspace_root (workspace-level path) rather
    // than the first-started agent's repo_root. This keeps QuickSessionLoop
    // independent of agent startup order — any session operation routes
    // through the workspace daemon, not an agent-specific clone.
    let client = GitimClient::new(&workspace_root);

    loop {
        match poll_and_process_sessions(
            &client,
            &state,
            &slug,
            &activity_tx,
            &agent_locks,
            &workspace_root,
        )
        .await
        {
            Ok(n) if n > 0 => {
                info!(processed = n, slug = %slug, "quick session loop: turn(s) executed");
            }
            Err(e) => {
                warn!(error = %e, slug = %slug, "quick session loop: error");
            }
            _ => {}
        }

        tokio::time::sleep(poll_interval).await;
    }
}

/// One poll cycle: list sessions → for each eligible session, execute turn.
async fn poll_and_process_sessions(
    client: &GitimClient,
    state: &SharedRuntimeState,
    slug: &str,
    activity_tx: &broadcast::Sender<AgentActivityEvent>,
    agent_locks: &AgentLockMap,
    workspace_root: &Path,
) -> Result<usize, String> {
    let resp = client
        .request("list_quick_sessions", json!({"include_archived": false}))
        .await
        .map_err(|e| format!("list_quick_sessions daemon call failed: {e}"))?;

    // Parse list response — daemon returns JSON array of QuickSessionListItem
    let data = resp
        .data
        .ok_or_else(|| "list_quick_sessions: missing data".to_string())?;

    let sessions: &Vec<Value> = data
        .as_array()
        .ok_or_else(|| format!("list_quick_sessions: expected array, got {:?}", data))?;

    let mut processed = 0;

    for session in sessions {
        let session_id = match session.get("id").and_then(|v: &Value| v.as_str()) {
            Some(id) => id,
            None => continue,
        };
        let agent_id = match session.get("agent_id").and_then(|v: &Value| v.as_str()) {
            Some(id) => id,
            None => continue,
        };
        let status = session
            .get("status")
            .and_then(|v: &Value| v.as_str())
            .unwrap_or("");

        // Process active, running, and needs_title sessions.
        // needs_title sessions need dispatch so the agent can set title
        // before responding (per required-title-api owner decision).
        if status != "active" && status != "running" && status != "needs_title" {
            continue;
        }

        // Check if the agent is hosted on this runtime
        let agent_info: Option<AgentInfo> = {
            let s = crate::preconditions::arc_mutex_lock(state);
            s.workspaces
                .get(slug)
                .and_then(|ctx| ctx.agents.get(agent_id))
                .cloned()
        };

        let agent_info = match agent_info {
            Some(info) => info,
            None => {
                debug!(agent_id = %agent_id, "agent not local, skipping quick session");
                continue;
            }
        };

        // Acquire per-agent lock to serialize with main AgentLoop
        let lock = acquire_agent_lock(agent_locks, agent_id).await;
        let _guard = lock.lock().await;

        if let Err(e) = execute_quick_session_turn(
            client,
            session_id,
            &agent_info,
            activity_tx,
            slug,
            workspace_root,
        )
        .await
        {
            warn!(
                session_id = %session_id,
                agent_id = %agent_id,
                error = %e,
                "quick session turn failed"
            );
            // Emit error event scoped to this session
            let _ = activity_tx.send(AgentActivityEvent {
                agent_id: agent_id.to_string(),
                workspace_id: slug.to_string(),
                event_type: "error".to_string(),
                detail: format!("quick session turn failed: {}", e),
                timestamp: chrono::Utc::now().to_rfc3339(),
                scope: "quick_session".to_string(),
                session_id: Some(session_id.to_string()),
                ref_: None,
            });
        }

        processed += 1;
    }

    Ok(processed)
}

/// Generate a title for a `needs_title` quick session and set it via daemon IPC.
///
/// Makes a dedicated provider call to generate a short title (max 80 chars)
/// from the conversation thread, then calls `set_quick_session_title` to
/// transition the session to `active`. Returns the updated meta on success,
/// or an error if title generation or setting fails.
async fn generate_and_set_title(
    client: &GitimClient,
    session_id: &str,
    agent_info: &AgentInfo,
    activity_tx: &broadcast::Sender<AgentActivityEvent>,
    workspace_id: &str,
    thread_raw: &str,
) -> Result<QuickSessionMeta, String> {
    let _ = activity_tx.send(AgentActivityEvent {
        agent_id: agent_info.handler.clone(),
        workspace_id: workspace_id.to_string(),
        event_type: "generating_title".to_string(),
        detail: format!("generating title for session {}", session_id),
        timestamp: chrono::Utc::now().to_rfc3339(),
        scope: "quick_session".to_string(),
        session_id: Some(session_id.to_string()),
        ref_: None,
    });

    // Build a fresh provider for title generation
    let provider_type = agent_info.provider.as_deref().unwrap_or("claude");
    let handler = &agent_info.handler;
    let provider_config = build_provider_config(provider_type, handler, agent_info.env.clone())
        .map_err(|e| format!("build_provider_config for title: {e}"))?;

    let provider = create(provider_type, provider_config)
        .map_err(|e| format!("create provider for title: {e}"))?;

    let base_prompt = agent_info.system_prompt.as_deref().unwrap_or("");
    let conversation =
        format_thread_for_prompt(thread_raw).unwrap_or_else(|_| thread_raw.to_string());
    let title_prompt = format!(
        "{}\n\nYou are generating a title for a quick session. Based on the conversation below, output ONLY a short title (max 80 characters) that summarizes the topic. Do not include quotes, markdown, or any other text — just the title.\n\nConversation:\n{}",
        base_prompt, conversation
    );

    let cwd = PathBuf::from(&agent_info.repo_path);
    let opts = ExecOptions {
        cwd: Some(cwd),
        model: agent_info.model.clone(),
        effort: agent_info.effort.clone(),
        system_prompt: Some(title_prompt),
        max_turns: Some(1),
        resume_token: None,
        ..Default::default()
    };

    info!(
        session_id = %session_id,
        agent_id = %handler,
        "quick session: generating title"
    );

    let mut session = provider
        .execute("Generate a title", opts)
        .await
        .map_err(|e| format!("title provider execute failed: {e}"))?;

    let mut title_output = String::new();
    while let Some(event) = session.events.recv().await {
        if let gitim_agent_provider::Event::Text { content } = &event {
            title_output.push_str(content);
        }
    }

    let exec_result = session
        .result
        .await
        .map_err(|_| "title result channel closed".to_string())?;

    let title = if matches!(
        exec_result.status,
        ExecStatus::Completed | ExecStatus::Aborted
    ) && !title_output.is_empty()
    {
        title_output.trim().to_string()
    } else if !exec_result.output.trim().is_empty() {
        exec_result.output.trim().to_string()
    } else {
        return Err("title generation produced empty output".to_string());
    };

    let title = if title.chars().count() > MAX_QUICK_SESSION_TITLE_LEN {
        title.chars().take(MAX_QUICK_SESSION_TITLE_LEN).collect()
    } else {
        title
    };

    // Set title via daemon IPC
    client
        .request(
            "set_quick_session_title",
            json!({"session_id": session_id, "title": title}),
        )
        .await
        .map_err(|e| format!("set_quick_session_title daemon call failed: {e}"))?;

    info!(
        session_id = %session_id,
        title = %title,
        "quick session: title set"
    );

    // Re-read session meta to get updated status (should now be active)
    let detail_resp = client
        .request("read_quick_session", json!({"session_id": session_id}))
        .await
        .map_err(|e| format!("read_quick_session after title set failed: {e}"))?;

    let detail_data = detail_resp
        .data
        .ok_or_else(|| "read_quick_session after title: missing data".to_string())?;

    let updated_meta: QuickSessionMeta = serde_json::from_value(
        detail_data
            .get("meta")
            .ok_or_else(|| "read_quick_session after title: missing meta".to_string())?
            .clone(),
    )
    .map_err(|e| format!("parse meta after title: {e}"))?;

    Ok(updated_meta)
}

/// Execute one turn for a quick session.
///
/// 1. Read session details (meta + thread)
/// 2. Verify title gate (status must be active/running, not needs_title)
/// 3. Check last message — if it's from the agent, already processed
/// 4. Build a fresh provider from agent config
/// 5. Execute one turn with thread context
/// 6. Write assistant response to daemon
/// 7. Teardown provider (on drop)
async fn execute_quick_session_turn(
    client: &GitimClient,
    session_id: &str,
    agent_info: &AgentInfo,
    activity_tx: &broadcast::Sender<AgentActivityEvent>,
    workspace_id: &str,
    workspace_root: &Path,
) -> Result<(), String> {
    // 1. Read session details
    let detail_resp = client
        .request("read_quick_session", json!({"session_id": session_id}))
        .await
        .map_err(|e| format!("read_quick_session daemon call failed: {e}"))?;

    let detail_data = detail_resp
        .data
        .ok_or_else(|| "read_quick_session: missing data".to_string())?;

    let meta_value = detail_data
        .get("meta")
        .ok_or_else(|| "read_quick_session: missing meta".to_string())?;

    let meta: QuickSessionMeta =
        serde_json::from_value(meta_value.clone()).map_err(|e| format!("parse meta: {e}"))?;

    let thread_raw = detail_data
        .get("thread")
        .and_then(|v: &Value| v.as_str())
        .unwrap_or("");

    // 2. Title gate: generate and set title if session is needs_title
    let meta = if meta.status == QuickSessionStatus::NeedsTitle {
        generate_and_set_title(
            client,
            session_id,
            agent_info,
            activity_tx,
            workspace_id,
            thread_raw,
        )
        .await?
    } else {
        meta
    };

    check_title_gate(&meta)?;

    // 3. Check last message author — skip if already answered by agent
    if let Some(author) = last_thread_author(thread_raw)? {
        if author == agent_info.handler {
            debug!(
                session_id = %session_id,
                "last message is from agent; session already processed"
            );
            return Ok(());
        }
    }

    let prompt = format_thread_for_prompt(thread_raw)?;
    if prompt.trim().is_empty() {
        return Err("empty thread — nothing to respond to".to_string());
    }

    // 4. Build a fresh provider from agent config
    let provider_type = agent_info.provider.as_deref().unwrap_or("claude");
    let handler = &agent_info.handler;
    let provider_config = build_provider_config(provider_type, handler, agent_info.env.clone())
        .map_err(|e| format!("build_provider_config: {e}"))?;

    let provider =
        create(provider_type, provider_config).map_err(|e| format!("create provider: {e}"))?;

    // 5. Build system prompt: base + title gate instruction
    let base_prompt = agent_info.system_prompt.as_deref().unwrap_or("");
    let system_prompt = if base_prompt.is_empty() {
        title_gate_prompt_instruction().to_string()
    } else {
        format!("{}\n\n{}", base_prompt, title_gate_prompt_instruction())
    };

    // Emit thinking event scoped to this session
    let _ = activity_tx.send(AgentActivityEvent {
        agent_id: handler.clone(),
        workspace_id: workspace_id.to_string(),
        event_type: "thinking".to_string(),
        detail: format!("quick session turn: {}", session_id),
        timestamp: chrono::Utc::now().to_rfc3339(),
        scope: "quick_session".to_string(),
        session_id: Some(session_id.to_string()),
        ref_: Some(meta.ref_string()),
    });

    let cwd = PathBuf::from(&agent_info.repo_path);

    // Read runtime state for resume token (per-agent session continuity).
    // State is stored under workspace_root, not the agent clone,
    // to avoid untracked .gitim-runtime/ pollution in agent repos.
    let runtime_state = quick_session_state::read_state(workspace_root, session_id);
    let resume_token = runtime_state.as_ref().and_then(|s| s.session_token.clone());

    let opts = ExecOptions {
        cwd: Some(cwd.clone()),
        model: agent_info.model.clone(),
        effort: agent_info.effort.clone(),
        system_prompt: Some(system_prompt),
        max_turns: Some(1),
        resume_token,
        ..Default::default()
    };

    // 6. Execute turn
    info!(
        session_id = %session_id,
        agent_id = %handler,
        provider = %provider_type,
        "quick session: executing turn"
    );

    let mut session = provider
        .execute(&prompt, opts)
        .await
        .map_err(|e| format!("provider execute failed: {e}"))?;

    // Drain events and collect assistant output
    let mut assistant_output = String::new();
    while let Some(event) = session.events.recv().await {
        if let gitim_agent_provider::Event::Text { content } = &event {
            assistant_output.push_str(content);
        }
    }

    let exec_result = session
        .result
        .await
        .map_err(|_| "result channel closed".to_string())?;

    // Extract fields before match (partial move safety)
    let session_token = exec_result.session_token.clone();
    let usage = exec_result.usage.clone();

    match exec_result.status {
        ExecStatus::Completed | ExecStatus::Aborted => {
            if assistant_output.is_empty() {
                assistant_output = exec_result.output;
            }
        }
        _ => {
            return Err(format!(
                "provider execution failed: {:?}",
                exec_result.status
            ));
        }
    }

    if assistant_output.trim().is_empty() {
        warn!(session_id = %session_id, "assistant output empty");
        return Ok(());
    }

    // 7. Write assistant response via daemon
    client
        .request(
            "send_quick_session_message",
            json!({
                "session_id": session_id,
                "body": assistant_output.trim(),
                "author": handler,
            }),
        )
        .await
        .map_err(|e| format!("send_quick_session_message failed: {e}"))?;

    // Persist runtime state for provider session continuity (resume token)
    if session_token.is_some() || runtime_state.is_some() {
        let mut new_state = runtime_state.unwrap_or_default();
        if let Some(token) = session_token {
            new_state.session_token = Some(token);
        }
        if let Some(u) = usage {
            new_state.session_usage = Some(serde_json::to_value(&u).unwrap_or_default());
        }
        let _ = quick_session_state::write_state(workspace_root, session_id, &new_state);
    }

    // Emit done event
    let _ = activity_tx.send(AgentActivityEvent {
        agent_id: handler.clone(),
        workspace_id: workspace_id.to_string(),
        event_type: "done".to_string(),
        detail: format!("quick session turn complete: {}", session_id),
        timestamp: chrono::Utc::now().to_rfc3339(),
        scope: "quick_session".to_string(),
        session_id: Some(session_id.to_string()),
        ref_: Some(meta.ref_string()),
    });

    info!(
        session_id = %session_id,
        output_len = assistant_output.trim().len(),
        "quick session: turn complete"
    );

    // Provider is torn down on drop
    Ok(())
}

fn last_thread_author(raw: &str) -> Result<Option<String>, String> {
    let file = parse_thread(raw).map_err(|e| format!("parse quick session thread: {e}"))?;
    Ok(file
        .entries
        .last()
        .map(|entry| entry.author().as_str().to_string()))
}

/// Format a raw thread into a clean conversation transcript for the provider.
fn format_thread_for_prompt(raw: &str) -> Result<String, String> {
    let file = parse_thread(raw).map_err(|e| format!("parse quick session thread: {e}"))?;
    Ok(file
        .entries
        .iter()
        .filter_map(|entry| match entry {
            ThreadEntry::Message(message) => {
                Some(format!("@{}: {}", message.author.as_str(), message.body))
            }
            ThreadEntry::Event(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_last_thread_author() {
        assert_eq!(
            last_thread_author("[L000001][P000000][@flame4][20260702T141101Z] Hello world\n")
                .unwrap(),
            Some("flame4".to_string())
        );
        assert_eq!(
            last_thread_author(
                "[L000001][P000000][@flame4][20260702T141101Z] Hello world\n[L000002][P000000][@dev-qiangzai][20260702T141201Z] response\n"
            )
            .unwrap(),
            Some("dev-qiangzai".to_string())
        );
        assert!(last_thread_author("short").is_err());
    }

    #[test]
    fn test_format_thread_for_prompt() {
        let raw = "[L000001][P000000][@flame4][20260702T141101Z] Hello world\n[L000002][P000000][@dev-qiangzai][20260702T141201Z] response\n";
        let formatted = format_thread_for_prompt(raw).unwrap();
        assert!(formatted.contains("@flame4: Hello world"));
        assert!(formatted.contains("@dev-qiangzai: response"));
        assert!(!formatted.contains("L000001"));
    }
}
