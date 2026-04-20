use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gitim_agent_provider::{
    ExecOptions, ExecStatus, PromptContext, Provider, ProviderConfig, ProviderUsage, create,
};
use gitim_client::GitimClient;
use tokio::sync::broadcast;
use tracing::info;

use crate::context_window::WARN_AT_PERCENT;
use crate::error::RuntimeError;
use crate::http::{AgentActivityEvent, SharedRuntimeState};
use crate::poller::{ChannelChange, Poller};
use crate::state::{AgentState, SessionUsageSnapshot, UsageSource};


#[derive(Debug, Clone, Default)]
pub struct AgentLoopConfig {
    pub provider_type: String,
    pub handler: String,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub env: HashMap<String, String>,
}

pub struct AgentLoop {
    poller: Poller,
    provider: Box<dyn Provider>,
    session_token: Option<String>,
    pub poll_interval: Duration,
    repo_root: PathBuf,
    provider_type: String,
    model: Option<String>,
    custom_system_prompt: Option<String>,
    handler: String,
    activity_tx: Option<broadcast::Sender<AgentActivityEvent>>,
    workspace_id: String,
    /// Optional reference to the runtime's shared state, used by
    /// `update_session_usage` to patch `AgentInfo.session_usage` in place after
    /// every turn — so `GET /agents/:id` returns fresh data without
    /// re-reading `.gitim/agent-state.json` on each request. None in tests
    /// and standalone CLI use where no HTTP state exists.
    runtime_state: Option<SharedRuntimeState>,
}

impl AgentLoop {
    /// Build an AgentLoop with default settings.
    /// Reads handler from `.gitim/me.json`. Restores state from disk if available.
    pub fn with_defaults(repo_root: &Path) -> Result<Self, RuntimeError> {
        let handler = read_handler_from_me_json(repo_root)?;
        Self::with_provider(repo_root, "claude", &handler)
    }

    /// Build an AgentLoop with a specified provider type and handler.
    /// Restores state from disk if available.
    pub fn with_provider(
        repo_root: &Path,
        provider_type: &str,
        handler: &str,
    ) -> Result<Self, RuntimeError> {
        let state = AgentState::load(repo_root)?;

        let poller = match state.cursor {
            Some(cursor) => {
                info!(cursor = %cursor, "restored cursor from state");
                Poller::with_cursor(GitimClient::new(repo_root), cursor)
            }
            None => Poller::new(GitimClient::new(repo_root)),
        };

        let provider = create(provider_type, ProviderConfig::default())
            .map_err(|e| RuntimeError::ProviderFailed(e.to_string()))?;

        if state.session_token.is_some() {
            info!("restored session_token from state");
        }

        Ok(Self {
            poller,
            provider,
            session_token: state.session_token,
            poll_interval: Duration::from_secs(2),
            repo_root: repo_root.to_path_buf(),
            provider_type: provider_type.to_string(),
            model: None,
            custom_system_prompt: None,
            handler: handler.to_string(),
            activity_tx: None,
            workspace_id: String::new(),
            runtime_state: None,
        })
    }

    /// Build an AgentLoop with full config (model, env, system_prompt).
    pub fn with_config(
        repo_root: &Path,
        config: &AgentLoopConfig,
    ) -> Result<Self, RuntimeError> {
        let state = AgentState::load(repo_root)?;

        let poller = match state.cursor {
            Some(cursor) => {
                info!(cursor = %cursor, "restored cursor from state");
                Poller::with_cursor(GitimClient::new(repo_root), cursor)
            }
            None => Poller::new(GitimClient::new(repo_root)),
        };

        let provider_config = ProviderConfig {
            executable_path: None,
            env: config.env.clone(),
        };
        let provider = create(&config.provider_type, provider_config)
            .map_err(|e| RuntimeError::ProviderFailed(e.to_string()))?;

        if state.session_token.is_some() {
            info!("restored session_token from state");
        }

        Ok(Self {
            poller,
            provider,
            session_token: state.session_token,
            poll_interval: Duration::from_secs(2),
            repo_root: repo_root.to_path_buf(),
            provider_type: config.provider_type.clone(),
            model: config.model.clone(),
            custom_system_prompt: config.system_prompt.clone(),
            handler: config.handler.clone(),
            activity_tx: None,
            workspace_id: String::new(),
            runtime_state: None,
        })
    }

    /// Attach a reference to the runtime's shared state so per-turn usage
    /// snapshots can be patched into `AgentInfo.session_usage` in place.
    /// Must be called after construction and before the loop spawns; tests
    /// that don't drive HTTP handlers can skip this entirely.
    pub fn set_runtime_state(&mut self, state: SharedRuntimeState) {
        self.runtime_state = Some(state);
    }

    /// Attach a broadcast sender and tag emitted events with a workspace slug.
    pub fn set_activity_tx_with_workspace(
        &mut self,
        tx: broadcast::Sender<AgentActivityEvent>,
        workspace_id: String,
    ) {
        self.activity_tx = Some(tx);
        self.workspace_id = workspace_id;
    }

    fn emit_activity(&self, event_type: &str, detail: &str) {
        if let Some(tx) = &self.activity_tx {
            let _ = tx.send(AgentActivityEvent {
                agent_id: self.handler.clone(),
                workspace_id: self.workspace_id.clone(),
                event_type: event_type.to_string(),
                detail: detail.to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
            });
        }
    }

    fn save_state(&self) -> Result<(), RuntimeError> {
        let mut state = AgentState::load(&self.repo_root)?;
        state.cursor = self.poller.cursor().map(|s| s.to_string());
        state.session_token = self.session_token.clone();
        state.save(&self.repo_root)
    }

    fn build_exec_options(&self) -> ExecOptions {
        let system_prompt = if self.session_token.is_none() {
            let ctx = PromptContext {
                handler: &self.handler,
                model: self.model.as_deref(),
            };
            let mut prompt = self.provider.build_system_prompt(&ctx);
            if let Some(custom) = &self.custom_system_prompt {
                if !custom.is_empty() {
                    prompt.push_str("\n\n## 用户自定义指令\n\n");
                    prompt.push_str(custom);
                }
            }
            Some(prompt)
        } else {
            None
        };

        ExecOptions {
            cwd: Some(self.repo_root.clone()),
            model: self.model.clone(),
            system_prompt,
            max_turns: Some(32),
            resume_token: self.session_token.clone(),
            ..Default::default()
        }
    }

    /// After `provider.execute()` returns, update state.session_usage based on
    /// whatever the provider reported plus the current estimator. Persists state.
    fn update_session_usage(
        &self,
        state: &mut AgentState,
        provider_reported: Option<&ProviderUsage>,
        session_id: &str,
    ) -> Result<(), RuntimeError> {
        let model = self.model.as_deref().unwrap_or("");
        let max = crate::context_window::default_max_tokens(&self.provider_type, model);
        let now = chrono::Utc::now().to_rfc3339();
        let new_snapshot = compute_snapshot(
            session_id,
            provider_reported,
            state.estimated_tokens,
            max,
            &now,
        );

        let prev_pct = state.session_usage.as_ref().map(|s| s.used_percent);
        if let Some(snap) = &new_snapshot {
            if just_crossed_threshold(prev_pct, snap.used_percent) {
                state.usage_notice_pending = true;
                let est_pct = max
                    .map(|m| (state.estimated_tokens as f64) / (m as f64) * 100.0)
                    .unwrap_or(0.0);
                tracing::info!(
                    session_id = %session_id,
                    provider_input_tokens = ?provider_reported.and_then(|p| p.input_tokens),
                    provider_used_pct = snap.used_percent,
                    estimated_tokens = state.estimated_tokens,
                    estimated_used_pct = est_pct,
                    delta_pp = snap.used_percent - est_pct,
                    max_tokens = ?max,
                    provider = %self.provider_type,
                    model = %model,
                    "threshold_crossed_80pct"
                );
            }
            if snap.used_percent > 110.0 {
                tracing::warn!(
                    session_id = %session_id,
                    used_percent = snap.used_percent,
                    "provider reported >110% — protocol drift signal"
                );
            }
        }

        if let Some(snap) = &new_snapshot {
            let est_pct = max
                .map(|m| (state.estimated_tokens as f64) / (m as f64) * 100.0)
                .unwrap_or(0.0);
            tracing::debug!(
                session_id = %session_id,
                provider_pct = snap.used_percent,
                estimated_pct = est_pct,
                delta_pp = snap.used_percent - est_pct,
                source = ?snap.source,
                "turn_usage"
            );
        }

        state.session_usage = new_snapshot.clone();
        state.save(&self.repo_root)?;

        // Patch the in-memory AgentInfo so polling clients (GET /agents/:id)
        // see fresh data without re-reading disk on every request, and
        // broadcast the snapshot as a "usage" SSE event on the existing
        // activity channel so reactive clients (/agents/events subscribers)
        // can patch their local store. A missing runtime_state or
        // activity_tx is fine — standalone CLI / tests skip silently.
        if let Some(snap) = &new_snapshot {
            if let Some(rs) = &self.runtime_state {
                if let Ok(mut s) = rs.lock() {
                    if let Some(ctx) = s.workspaces.get_mut(&self.workspace_id) {
                        if let Some(info) = ctx.agents.get_mut(&self.handler) {
                            info.session_usage = Some(snap.clone());
                        }
                    }
                }
            }
            let detail = serde_json::to_string(snap).unwrap_or_default();
            self.emit_activity("usage", &detail);
        }

        Ok(())
    }

    /// Initialize the poller cursor if not already set.
    /// Call this once before entering a manual run_once() loop.
    pub async fn init(&mut self) -> Result<(), RuntimeError> {
        if self.poller.cursor().is_none() {
            self.poller.poll().await?;
            self.save_state()?;
            info!("agent loop started, cursor initialized");
        } else {
            info!("agent loop started, cursor restored from state");
        }
        Ok(())
    }

    /// Run one poll-and-process cycle. Returns true if messages were processed.
    pub async fn run_once(&mut self) -> Result<bool, RuntimeError> {
        let result = self.poller.poll().await?;

        if result.changes.is_empty() {
            self.save_state()?;
            return Ok(false);
        }

        let prompt = match format_changes_as_prompt(&result.changes, &self.handler) {
            Some(p) => p,
            None => {
                tracing::debug!("all changes are self-authored, skipping");
                self.save_state()?;
                return Ok(false);
            }
        };
        info!(prompt = %prompt, "sending to provider");
        self.emit_activity("thinking", "processing...");

        let opts = self.build_exec_options();

        // Consume usage_notice_pending (Task 14) and accumulate tiktoken estimate
        // (Task 12) in a single load/save cycle. If the notice flag was set by
        // a previous turn's threshold crossing, prepend the system preamble here
        // and clear the flag before execute — this way a mid-turn crash won't
        // cause the notice to re-fire.
        let mut state = AgentState::load(&self.repo_root)?;
        let prompt = if state.usage_notice_pending {
            let pct = state.session_usage.as_ref().map(|s| s.used_percent).unwrap_or(80.0);
            let preamble = build_usage_notice_preamble(pct);
            state.usage_notice_pending = false;
            format!("{preamble}\n\n---\n\n{prompt}")
        } else {
            prompt
        };

        // Pre-execute tiktoken accumulation (Task 12).
        // Cold start (no resume token) → reset estimator and seed with the system prompt.
        // Every turn adds the user prompt before execute; assistant text is added after.
        let cold_start = self.session_token.is_none();
        if cold_start {
            state.estimated_tokens = 0;
        }
        state.estimated_tokens +=
            crate::context_window::tokenize_for_provider(&self.provider_type, &prompt);
        if cold_start {
            if let Some(sp) = opts.system_prompt.as_deref() {
                state.estimated_tokens +=
                    crate::context_window::tokenize_for_provider(&self.provider_type, sp);
            }
        }
        state.save(&self.repo_root)?;

        let mut session = self
            .provider
            .execute(&prompt, opts)
            .await
            .map_err(|e| RuntimeError::ProviderFailed(e.to_string()))?;

        // Drain events with periodic steering check
        let mut steering_check = tokio::time::interval(Duration::from_secs(5));
        steering_check.tick().await; // consume the immediate first tick

        // Sliding window for [[RESET]] detection across streaming text chunks.
        // The agent signals an intentional context reset by emitting "[[RESET]]" in its output.
        // This is a private runtime protocol — silent, not surfaced to IM or the WebUI.
        let mut text_tail = String::new();
        // Task 12: accumulate full assistant text across the turn for tiktoken estimate.
        // Intentionally uncapped (unlike text_tail) — we want total token count.
        let mut assistant_text_buf = String::new();
        let mut reset_requested = false;

        loop {
            tokio::select! {
                event = session.events.recv() => {
                    match event {
                        Some(event) => {
                            match &event {
                                gitim_agent_provider::Event::Text { content } => {
                                    tracing::debug!(text_len = content.len(), "agent text");
                                    text_tail.push_str(content);
                                    assistant_text_buf.push_str(content);
                                    if text_tail.contains("[[RESET]]") {
                                        info!(
                                            handler = %self.handler,
                                            "agent requested context reset"
                                        );
                                        session.cancel();
                                        reset_requested = true;
                                        break;
                                    }
                                    // Cap tail size: RESET tag is 9 bytes, 128 leaves ample margin
                                    // for the tag to survive chunk boundaries without unbounded growth.
                                    const TAIL_MAX: usize = 128;
                                    if text_tail.len() > TAIL_MAX {
                                        let cut = text_tail.len() - TAIL_MAX;
                                        let safe = text_tail.floor_char_boundary(cut);
                                        text_tail.drain(..safe);
                                    }
                                }
                                gitim_agent_provider::Event::ToolUse { tool, input, .. } => {
                                    let snippet = summarize_tool_input(tool, input);
                                    info!(tool = %tool, input = %snippet, "agent tool use");
                                    self.emit_activity("tool_use", &format!("{tool}: {snippet}"));
                                }
                                gitim_agent_provider::Event::ToolResult { call_id, output } => {
                                    tracing::debug!(call_id = %call_id, output_len = output.len(), "tool result");
                                }
                                gitim_agent_provider::Event::Error { content } => {
                                    tracing::warn!(error = %content, "agent error event");
                                    self.emit_activity("error", content);
                                }
                                _ => {}
                            }
                        }
                        None => break, // event channel closed, normal completion
                    }
                }
                _ = steering_check.tick() => {
                    match self.poller.peek().await {
                        Ok(peek_result) if !peek_result.changes.is_empty() => {
                            if detect_steering_trigger(&peek_result.changes, &self.handler) {
                                info!("steering trigger detected, cancelling session");
                                self.emit_activity("steering", "urgent message detected, interrupting");
                                session.cancel();
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "steering peek failed, continuing");
                        }
                        _ => {}
                    }
                }
            }
        }

        // Await final result
        let exec_result = session
            .result
            .await
            .map_err(|_| RuntimeError::ProviderFailed("result channel closed".into()))?;

        // Silent reset short-circuit: agent asked to reset its own context.
        // Clear session_token so next cycle rebuilds the system prompt.
        // Cursor is preserved — the messages in this cycle have been consumed.
        // Intentionally skip status match + emit_activity: this is invisible to IM and UI.
        if reset_requested {
            info!(
                handler = %self.handler,
                duration_ms = exec_result.duration_ms,
                "context reset complete, clearing session_token"
            );
            self.session_token = None;
            let mut state = AgentState::load(&self.repo_root)?;
            let sid_for_log = state.session_usage.as_ref().map(|s| s.session_id.clone());
            state.clear_session();
            state.save(&self.repo_root)?;
            tracing::info!(session_id = ?sid_for_log, reason = "agent_emitted_reset", "session_reset");
            self.save_state()?;
            return Ok(true);
        }

        let duration_s = exec_result.duration_ms as f64 / 1000.0;
        match exec_result.status {
            ExecStatus::Failed => {
                tracing::error!(
                    duration_ms = exec_result.duration_ms,
                    error = ?exec_result.error,
                    output = %exec_result.output.chars().take(300).collect::<String>(),
                    "provider failed"
                );
                self.emit_activity("error", "execution failed");
                // Clear session_token to avoid resuming a broken session
                self.session_token = None;
                let mut state = AgentState::load(&self.repo_root)?;
                state.clear_session();
                state.save(&self.repo_root)?;
            }
            ExecStatus::Aborted => {
                info!(
                    duration_ms = exec_result.duration_ms,
                    "provider aborted by steering"
                );
                self.emit_activity("steered", &format!("interrupted ({duration_s:.1}s)"));
                // Extract session_id from the just-completed turn. For Claude the
                // session_token and session_id are the same opaque string; for Codex
                // it's the thread_id. In either case it's exec_result.session_token.
                if let Some(sid) = exec_result.session_token.as_deref() {
                    let mut state = AgentState::load(&self.repo_root)?;
                    state.estimated_tokens += crate::context_window::tokenize_for_provider(
                        &self.provider_type,
                        &assistant_text_buf,
                    );
                    self.update_session_usage(&mut state, exec_result.usage.as_ref(), sid)?;
                }
                // Keep session_token for resume in next cycle
                if let Some(token) = exec_result.session_token {
                    self.session_token = Some(token);
                }
            }
            _ => {
                info!(
                    duration_ms = exec_result.duration_ms,
                    output = %exec_result.output.chars().take(100).collect::<String>(),
                    "provider ok"
                );
                self.emit_activity("done", &format!("done ({duration_s:.1}s)"));
                // Extract session_id from the just-completed turn. For Claude the
                // session_token and session_id are the same opaque string; for Codex
                // it's the thread_id. In either case it's exec_result.session_token.
                if let Some(sid) = exec_result.session_token.as_deref() {
                    let mut state = AgentState::load(&self.repo_root)?;
                    state.estimated_tokens += crate::context_window::tokenize_for_provider(
                        &self.provider_type,
                        &assistant_text_buf,
                    );
                    self.update_session_usage(&mut state, exec_result.usage.as_ref(), sid)?;
                }
                if let Some(token) = exec_result.session_token {
                    self.session_token = Some(token);
                }
            }
        }

        self.save_state()?;
        Ok(true)
    }

    /// Run the agent loop indefinitely with exponential backoff on errors.
    pub async fn run(&mut self) -> Result<(), RuntimeError> {
        if self.poller.cursor().is_none() {
            self.poller.poll().await?;
            self.save_state()?;
            info!("agent loop started, cursor initialized");
        } else {
            info!("agent loop started, cursor restored from state");
        }

        let mut consecutive_errors: u32 = 0;
        const MAX_BACKOFF_SECS: u64 = 60;

        loop {
            match self.run_once().await {
                Ok(true) => {
                    consecutive_errors = 0;
                    // provider finished/failed logs are already emitted in run_once
                }
                Ok(false) => {
                    consecutive_errors = 0;
                    tracing::trace!("idle");
                }
                Err(e) => {
                    consecutive_errors += 1;
                    let backoff = Duration::from_secs(
                        (2u64.saturating_pow(consecutive_errors)).min(MAX_BACKOFF_SECS),
                    );
                    tracing::error!(
                        error = %e,
                        consecutive = consecutive_errors,
                        backoff_secs = backoff.as_secs(),
                        "agent loop error, backing off"
                    );
                    tokio::time::sleep(backoff).await;
                    continue;
                }
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }
}

/// Extract a short snippet from tool input for logging.
fn summarize_tool_input(tool: &str, input: &serde_json::Value) -> String {
    const MAX: usize = 512;
    let raw = match tool {
        "Bash" => input["command"].as_str().unwrap_or("").to_string(),
        "Read" | "Write" => input["file_path"].as_str().unwrap_or("").to_string(),
        "Edit" => {
            let path = input["file_path"].as_str().unwrap_or("");
            let old = input["old_string"].as_str().unwrap_or("");
            format!("{path} :: {old}")
        }
        "Grep" => input["pattern"].as_str().unwrap_or("").to_string(),
        "Glob" => input["pattern"].as_str().unwrap_or("").to_string(),
        _ => input.to_string(),
    };
    if raw.len() <= MAX {
        raw
    } else {
        format!("{}…", &raw[..raw.floor_char_boundary(MAX)])
    }
}

/// Format channel changes into a prompt, filtering out self-authored messages.
/// Returns `None` if no external events remain after filtering.
pub fn format_changes_as_prompt(changes: &[ChannelChange], self_handler: &str) -> Option<String> {
    let mut prompt = String::from("以下是你上次醒来后发生的事件：\n\n");
    let mut has_external = false;

    for change in changes {
        if change.kind == "channel_meta" {
            continue;
        }

        for entry in &change.entries {
            let author = entry["author"].as_str().unwrap_or("unknown");

            if author == self_handler {
                continue;
            }

            has_external = true;
            let body = entry["body"].as_str().unwrap_or("");
            let timestamp = entry["timestamp"].as_str().unwrap_or("");
            let channel = &change.channel;
            let line_number = entry["line_number"].as_u64();
            let point_to = entry["point_to"].as_u64().unwrap_or(0);

            // Build line id: "L42" or "L42→L38" when replying
            let line_id = match line_number {
                Some(ln) if point_to > 0 => format!("L{ln}→L{point_to}"),
                Some(ln) => format!("L{ln}"),
                None => String::new(),
            };

            let ts = if timestamp.is_empty() {
                String::new()
            } else {
                format!("[{timestamp}] ")
            };

            if line_id.is_empty() {
                prompt.push_str(&format!("{ts}[#{channel}] @{author}: {body}\n"));
            } else {
                prompt.push_str(&format!("{ts}[#{channel}] {line_id} @{author}: {body}\n"));
            }
        }
    }

    if has_external {
        Some(prompt)
    } else {
        None
    }
}

/// Check whether any change contains a steering trigger.
///
/// Trigger condition: message from another user that @mentions self_handler
/// AND contains "急急急". Self-authored messages are ignored.
pub fn detect_steering_trigger(changes: &[ChannelChange], self_handler: &str) -> bool {
    let mention = format!("@{self_handler}");
    for change in changes {
        if change.kind == "channel_meta" {
            continue;
        }
        for entry in &change.entries {
            let author = entry["author"].as_str().unwrap_or("");
            if author == self_handler {
                continue;
            }
            let body = entry["body"].as_str().unwrap_or("");
            if body.contains(&mention) && body.contains("急急急") {
                return true;
            }
        }
    }
    false
}

fn read_handler_from_me_json(repo_root: &Path) -> Result<String, RuntimeError> {
    let me_path = repo_root.join(".gitim/me.json");
    let content = std::fs::read_to_string(&me_path).map_err(|e| {
        RuntimeError::Io(std::io::Error::new(
            e.kind(),
            format!("failed to read .gitim/me.json: {e}"),
        ))
    })?;
    let parsed: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        RuntimeError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("failed to parse .gitim/me.json: {e}"),
        ))
    })?;
    parsed["handler"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| {
            RuntimeError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "missing handler field in .gitim/me.json",
            ))
        })
}

/// Compute a `SessionUsageSnapshot` from available usage signals.
///
/// Authoritative-value policy (matches 01-design.md §4.5):
/// 1. provider_reported.used_percent (Codex)
/// 2. provider_reported.input_tokens / max_tokens (Claude)
/// 3. estimated_tokens / max_tokens (fallback)
/// 4. None (no data available)
///
/// `used_percent` is clamped to `[0, 100]`. Callers are responsible for
/// logging unusual values (e.g. >110 as a protocol-drift signal).
pub fn compute_snapshot(
    session_id: &str,
    provider_reported: Option<&ProviderUsage>,
    estimated_tokens: u64,
    max_tokens: Option<u64>,
    updated_at: &str,
) -> Option<SessionUsageSnapshot> {
    let (used_percent, source, input_tokens, output_tokens) =
        if let Some(pu) = provider_reported {
            if let Some(pct) = pu.used_percent {
                (pct, UsageSource::ProviderReported, pu.input_tokens, pu.output_tokens)
            } else if let (Some(input), Some(max)) = (pu.input_tokens, max_tokens) {
                let pct = (input as f64) / (max as f64) * 100.0;
                (pct, UsageSource::ProviderReported, pu.input_tokens, pu.output_tokens)
            } else {
                return compute_from_estimate(session_id, estimated_tokens, max_tokens, updated_at);
            }
        } else {
            return compute_from_estimate(session_id, estimated_tokens, max_tokens, updated_at);
        };

    let used_percent = used_percent.clamp(0.0, 100.0);
    Some(SessionUsageSnapshot {
        session_id: session_id.to_string(),
        input_tokens,
        output_tokens,
        max_tokens,
        used_percent,
        source,
        updated_at: updated_at.to_string(),
    })
}

fn compute_from_estimate(
    session_id: &str,
    estimated_tokens: u64,
    max_tokens: Option<u64>,
    updated_at: &str,
) -> Option<SessionUsageSnapshot> {
    let max = max_tokens?;
    if estimated_tokens == 0 {
        return None;
    }
    let pct = ((estimated_tokens as f64) / (max as f64) * 100.0).clamp(0.0, 100.0);
    Some(SessionUsageSnapshot {
        session_id: session_id.to_string(),
        input_tokens: None,
        output_tokens: None,
        max_tokens: Some(max),
        used_percent: pct,
        source: UsageSource::RuntimeEstimated,
        updated_at: updated_at.to_string(),
    })
}

/// `true` iff this turn is the first to observe `new_pct >= WARN_AT_PERCENT`
/// in the current session. Never returns `true` twice for the same session
/// (subsequent turns see `prev_pct >= WARN_AT_PERCENT`).
pub fn just_crossed_threshold(prev_pct: Option<f64>, new_pct: f64) -> bool {
    if new_pct < WARN_AT_PERCENT {
        return false;
    }
    match prev_pct {
        None => true,
        Some(p) => p < WARN_AT_PERCENT,
    }
}

/// The one-shot preamble inserted before the next user prompt when
/// `used_percent` first crosses `WARN_AT_PERCENT`. Content is deliberately
/// firm — no tiered wording, no retry guidance — per 01-design.md §4.5.
pub fn build_usage_notice_preamble(used_percent: f64) -> String {
    format!(
        "[系统通知] 你的对话窗口使用率已达 {pct:.0}%。请在本轮完成手头任务后立即结束本次对话：\n\
         1. 把所有需要长期记忆的内容（重要决定、用户偏好、未完成事项、进度交接）写入你的记忆文件\n\
         2. 在输出末尾附加标记 [[RESET]]，runtime 会在下一轮为你开启全新窗口\n\
         本提醒仅发送一次。",
        pct = used_percent,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_activity_event_includes_workspace_id() {
        let e = AgentActivityEvent {
            agent_id: "a".to_string(),
            workspace_id: "ws1".to_string(),
            event_type: "tool_use".to_string(),
            detail: "d".to_string(),
            timestamp: "t".to_string(),
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"workspace_id\":\"ws1\""));
    }
}
