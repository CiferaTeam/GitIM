use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use gitim_agent_provider::{
    create, Event, ExecOptions, ExecStatus, PromptContext, Provider, ProviderConfig, ProviderUsage,
};
use gitim_client::GitimClient;
use gitim_core::responses::{
    ClaimQuickSessionTurnResponse, ListQuickSessionsResponse, MarkQuickSessionErrorResponse,
    QuickSessionDetail, ReadQuickSessionResponse,
};
use gitim_core::types::{QuickSessionStatus, ThreadEntry};
use tokio::sync::broadcast;

use crate::agent_loop::compute_snapshot;
use crate::context_window::{default_max_tokens, tokenize_for_provider, WARN_AT_PERCENT};
use crate::error::RuntimeError;
use crate::http::{ActivityScope, AgentActivityEvent};
use crate::quick_session_state::QuickSessionRuntimeState;
use crate::state::LastSessionUsage;
use crate::usage_log::AgentUsageLog;

const TRANSCRIPT_ENTRY_LIMIT: usize = 24;
const PROMPT_CHAR_LIMIT: usize = 12_000;
const CLAIMED_ENTRY_RESERVE: usize = 2_048;
const QUICK_SESSION_SCAN_ERROR: &str = "interrupted quick session turn discovered during recovery";

#[async_trait]
pub trait QuickSessionBackend: Send + Sync {
    async fn list(
        &self,
        agent_id: &str,
        actionable: bool,
        status: Option<QuickSessionStatus>,
    ) -> Result<ListQuickSessionsResponse, String>;
    async fn read(&self, session_id: &str) -> Result<ReadQuickSessionResponse, String>;
    async fn read_since(
        &self,
        session_id: &str,
        since: u64,
        limit: usize,
    ) -> Result<ReadQuickSessionResponse, String>;
    async fn claim(
        &self,
        session_id: &str,
        input_line: u64,
        attempt_id: &str,
    ) -> Result<ClaimQuickSessionTurnResponse, String>;
    async fn mark_error(
        &self,
        session_id: &str,
        attempt_id: &str,
        error: &str,
    ) -> Result<MarkQuickSessionErrorResponse, String>;
}

#[async_trait]
impl QuickSessionBackend for GitimClient {
    async fn list(
        &self,
        agent_id: &str,
        actionable: bool,
        status: Option<QuickSessionStatus>,
    ) -> Result<ListQuickSessionsResponse, String> {
        self.list_quick_sessions_filtered(false, Some(agent_id), actionable, status, Some(100))
            .await
            .map_err(|error| error.to_string())
    }

    async fn read(&self, session_id: &str) -> Result<ReadQuickSessionResponse, String> {
        self.read_quick_session(session_id, Some(TRANSCRIPT_ENTRY_LIMIT), None)
            .await
            .map_err(|error| error.to_string())
    }

    async fn read_since(
        &self,
        session_id: &str,
        since: u64,
        limit: usize,
    ) -> Result<ReadQuickSessionResponse, String> {
        self.read_quick_session(session_id, Some(limit), Some(since))
            .await
            .map_err(|error| error.to_string())
    }

    async fn claim(
        &self,
        session_id: &str,
        input_line: u64,
        attempt_id: &str,
    ) -> Result<ClaimQuickSessionTurnResponse, String> {
        self.claim_quick_session_turn(session_id, input_line, attempt_id)
            .await
            .map_err(|error| error.to_string())
    }

    async fn mark_error(
        &self,
        session_id: &str,
        attempt_id: &str,
        error: &str,
    ) -> Result<MarkQuickSessionErrorResponse, String> {
        self.mark_quick_session_error(session_id, attempt_id, error)
            .await
            .map_err(|error| error.to_string())
    }
}

pub trait QuickSessionProviderFactory: Send + Sync {
    fn create(
        &self,
        provider_type: &str,
        config: ProviderConfig,
    ) -> Result<Box<dyn Provider>, String>;
}

struct DefaultProviderFactory;

impl QuickSessionProviderFactory for DefaultProviderFactory {
    fn create(
        &self,
        provider_type: &str,
        config: ProviderConfig,
    ) -> Result<Box<dyn Provider>, String> {
        create(provider_type, config).map_err(|error| error.to_string())
    }
}

#[derive(Clone)]
pub struct QuickSessionExecutorConfig {
    pub repo_root: PathBuf,
    pub workspace_root: PathBuf,
    pub workspace_id: String,
    pub handler: String,
    pub provider_type: String,
    pub provider_config: ProviderConfig,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub custom_system_prompt: Option<String>,
    pub activity_tx: Option<broadcast::Sender<AgentActivityEvent>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuickSessionRunOutcome {
    Noop,
    Completed { requeue: bool },
    Failed { requeue: bool },
    Discarded,
}

pub struct QuickSessionExecutor {
    config: QuickSessionExecutorConfig,
    backend: Arc<dyn QuickSessionBackend>,
    provider_factory: Arc<dyn QuickSessionProviderFactory>,
}

impl QuickSessionExecutor {
    pub fn new(config: QuickSessionExecutorConfig) -> Self {
        let client = GitimClient::new(&config.repo_root);
        Self::with_dependencies(config, Arc::new(client), Arc::new(DefaultProviderFactory))
    }

    #[doc(hidden)]
    pub fn with_dependencies(
        config: QuickSessionExecutorConfig,
        backend: Arc<dyn QuickSessionBackend>,
        provider_factory: Arc<dyn QuickSessionProviderFactory>,
    ) -> Self {
        Self {
            config,
            backend,
            provider_factory,
        }
    }

    pub async fn recover_actionable(&self) -> Result<Vec<String>, RuntimeError> {
        let actionable_sessions = self
            .backend
            .list(&self.config.handler, true, None)
            .await
            .map_err(provider_error)?;
        let mut actionable: Vec<String> = actionable_sessions
            .sessions
            .into_iter()
            .map(|session| session.id)
            .collect();
        let running_sessions = self
            .backend
            .list(
                &self.config.handler,
                false,
                Some(QuickSessionStatus::Running),
            )
            .await
            .map_err(provider_error)?;
        for session in running_sessions.sessions {
            let detail = self
                .backend
                .read(&session.id)
                .await
                .map_err(provider_error)?
                .session;
            if let Some(attempt_id) = detail.meta.attempt_id.as_deref() {
                if !self
                    .mark_claim_error(&session.id, attempt_id, QUICK_SESSION_SCAN_ERROR)
                    .await?
                {
                    continue;
                }
                let recovered = self
                    .backend
                    .read(&session.id)
                    .await
                    .map_err(provider_error)?
                    .session;
                if is_actionable(&recovered, &self.config.handler)
                    && !actionable.contains(&session.id)
                {
                    actionable.push(session.id);
                }
            }
        }
        Ok(actionable)
    }

    pub async fn execute(&self, session_id: &str) -> Result<QuickSessionRunOutcome, RuntimeError> {
        let before = self
            .backend
            .read(session_id)
            .await
            .map_err(provider_error)?
            .session;
        if !is_actionable(&before, &self.config.handler) {
            return Ok(QuickSessionRunOutcome::Noop);
        }
        let input_line = latest_creator_line(&before).ok_or_else(|| {
            RuntimeError::ProviderFailed("quick session has no creator input".to_string())
        })?;
        let mut prompt_detail = before.clone();
        if !prompt_detail
            .entries
            .iter()
            .any(|entry| entry.line_number() == input_line)
        {
            let claimed = self
                .backend
                .read_since(session_id, input_line.saturating_sub(1), 1)
                .await
                .map_err(provider_error)?
                .session
                .entries
                .into_iter()
                .find(|entry| entry.line_number() == input_line)
                .ok_or_else(|| {
                    RuntimeError::ProviderFailed(
                        "quick session claimed input is missing from transcript".to_string(),
                    )
                })?;
            prompt_detail.entries.push(claimed);
        }
        let attempt_id = generate_attempt_id();
        let claim = self
            .backend
            .claim(session_id, input_line, &attempt_id)
            .await
            .map_err(provider_error)?;

        let original_local =
            QuickSessionRuntimeState::load(&self.config.workspace_root, session_id)?;
        let mut local = original_local.clone();
        local.last_attempted_line = Some(input_line);
        local.save(&self.config.workspace_root, session_id)?;
        let captured_generation = local.context_generation;

        let provider = match self.provider_factory.create(
            &self.config.provider_type,
            self.config.provider_config.clone(),
        ) {
            Ok(provider) => provider,
            Err(error) => {
                if !self
                    .mark_claim_error(session_id, &attempt_id, &error)
                    .await?
                {
                    return Ok(QuickSessionRunOutcome::Discarded);
                }
                return self.failed_outcome(session_id).await;
            }
        };
        let self_managed = provider.self_managed_context();
        let cold_start = local.session_token.is_none();
        let prompt = build_quick_session_prompt(
            &prompt_detail,
            &attempt_id,
            input_line,
            local.reset_required && !self_managed,
        );
        let system_prompt = cold_start.then(|| self.build_system_prompt(provider.as_ref()));
        let opts = ExecOptions {
            cwd: Some(self.config.repo_root.clone()),
            model: self.config.model.clone(),
            effort: self.config.effort.clone(),
            system_prompt,
            max_turns: Some(32),
            resume_token: local.session_token.clone(),
            ..Default::default()
        };

        self.emit(
            "thinking",
            "processing...",
            session_id,
            &attempt_id,
            claim.revision,
            captured_generation,
        );
        let mut session = match provider.execute(&prompt, opts).await {
            Ok(session) => session,
            Err(error) => {
                if !self
                    .mark_claim_error(session_id, &attempt_id, &error.to_string())
                    .await?
                {
                    return Ok(QuickSessionRunOutcome::Discarded);
                }
                return self.failed_outcome(session_id).await;
            }
        };
        let mut reset_requested = false;
        let mut turn_text = String::new();
        while let Some(event) = session.events.recv().await {
            match event {
                Event::Text { content } => {
                    turn_text.push_str(&content);
                    self.emit(
                        "stream",
                        &content,
                        session_id,
                        &attempt_id,
                        claim.revision,
                        captured_generation,
                    );
                    if !self_managed && turn_text.contains("[[RESET]]") {
                        reset_requested = true;
                        session.cancel();
                    }
                }
                Event::Thinking { content } => self.emit(
                    "thinking",
                    &content,
                    session_id,
                    &attempt_id,
                    claim.revision,
                    captured_generation,
                ),
                Event::ToolUse { tool, input, .. } => {
                    let detail = format!("{tool}: {input}");
                    turn_text.push_str(&detail);
                    self.emit(
                        "tool_use",
                        &detail,
                        session_id,
                        &attempt_id,
                        claim.revision,
                        captured_generation,
                    );
                }
                Event::ToolResult { output, .. } => turn_text.push_str(&output),
                Event::Error { content } => self.emit(
                    "error",
                    &content,
                    session_id,
                    &attempt_id,
                    claim.revision,
                    captured_generation,
                ),
                Event::Usage { usage, .. } => {
                    let detail = serde_json::to_string(&usage).unwrap_or_default();
                    self.emit(
                        "usage",
                        &detail,
                        session_id,
                        &attempt_id,
                        claim.revision,
                        captured_generation,
                    );
                }
                Event::Status { status } => self.emit(
                    "status",
                    &status,
                    session_id,
                    &attempt_id,
                    claim.revision,
                    captured_generation,
                ),
                Event::Log { .. } => {}
            }
        }
        let result = match session.result.await {
            Ok(result) => result,
            Err(_) => {
                if !self
                    .mark_claim_error(session_id, &attempt_id, "provider result channel closed")
                    .await?
                {
                    return Ok(QuickSessionRunOutcome::Discarded);
                }
                return self.failed_outcome(session_id).await;
            }
        };

        let current_local =
            QuickSessionRuntimeState::load(&self.config.workspace_root, session_id)?;
        if current_local.context_generation != captured_generation {
            return Ok(QuickSessionRunOutcome::Discarded);
        }
        let after = self
            .backend
            .read(session_id)
            .await
            .map_err(provider_error)?
            .session;
        let owns_active_attempt = after.meta.status == QuickSessionStatus::Running
            && after.meta.attempt_id.as_deref() == Some(&attempt_id);
        let owns_completed_attempt =
            after.meta.last_completed_attempt_id.as_deref() == Some(&attempt_id);
        if after.archived
            || after.meta.status == QuickSessionStatus::Archived
            || (!owns_active_attempt && !owns_completed_attempt)
        {
            original_local.save(&self.config.workspace_root, session_id)?;
            return Ok(QuickSessionRunOutcome::Discarded);
        }

        let completed_entry = match after.meta.last_completed_line {
            Some(line)
                if after
                    .entries
                    .iter()
                    .any(|entry| entry.line_number() == line) =>
            {
                after
                    .entries
                    .iter()
                    .find(|entry| entry.line_number() == line)
                    .cloned()
            }
            Some(line) => self
                .backend
                .read_since(session_id, line.saturating_sub(1), 1)
                .await
                .map_err(provider_error)?
                .session
                .entries
                .into_iter()
                .find(|entry| entry.line_number() == line),
            None => None,
        };
        let completed = after.meta.title.is_some()
            && owns_completed_attempt
            && after.meta.last_completed_input_line == Some(input_line)
            && completed_entry.is_some_and(|entry| {
                entry.author().as_str() == self.config.handler && entry.point_to() == input_line
            });
        let summary_updated = after.meta.summary.is_some()
            && after.meta.summary_updated_at != before.meta.summary_updated_at;
        let provider_failed = matches!(result.status, ExecStatus::Failed | ExecStatus::Timeout);
        if provider_failed || !completed || (reset_requested && !summary_updated) {
            if after.meta.status == QuickSessionStatus::Running
                && after.meta.attempt_id.as_deref() == Some(&attempt_id)
            {
                let diagnostic = if provider_failed {
                    result
                        .error
                        .as_deref()
                        .unwrap_or("quick session provider failed")
                } else if reset_requested && !summary_updated {
                    "quick session reset requires an updated durable summary"
                } else {
                    "quick session turn completed without both a title and an agent reply"
                };
                if !self
                    .mark_claim_error(session_id, &attempt_id, diagnostic)
                    .await?
                {
                    return Ok(QuickSessionRunOutcome::Discarded);
                }
            }
            self.emit(
                "error",
                "quick session turn incomplete",
                session_id,
                &attempt_id,
                claim.revision,
                captured_generation,
            );
            return self.failed_outcome(session_id).await;
        }

        local = current_local;
        let prompt_tokens = tokenize_for_provider(&self.config.provider_type, &prompt);
        let output_tokens = tokenize_for_provider(&self.config.provider_type, &turn_text);
        if cold_start {
            local.estimated_tokens = 0;
        }
        local.estimated_tokens = local
            .estimated_tokens
            .saturating_add(prompt_tokens)
            .saturating_add(output_tokens);
        let session_token = result
            .session_token
            .clone()
            .or_else(|| local.session_token.clone())
            .unwrap_or_else(|| attempt_id.clone());
        local.session_token = Some(session_token.clone());
        let context_usage = result
            .usage_report
            .context
            .as_ref()
            .or(result.usage.as_ref());
        local.session_usage = compute_snapshot(
            &session_token,
            context_usage,
            local.estimated_tokens,
            default_max_tokens(
                &self.config.provider_type,
                self.config.model.as_deref().unwrap_or(""),
            ),
            provider.usage_is_cumulative(),
            &chrono::Utc::now().to_rfc3339(),
        );
        self.accumulate_usage(
            provider.as_ref(),
            &mut local,
            &session_token,
            result
                .usage_report
                .billing
                .as_ref()
                .or(result.usage.as_ref()),
        );
        if !self_managed {
            local.reset_required = local
                .session_usage
                .as_ref()
                .is_some_and(|usage| usage.used_percent >= WARN_AT_PERCENT);
        } else {
            local.reset_required = false;
        }
        local.last_completed_input_line = after.meta.last_completed_input_line;
        local.last_completed_line = after.meta.last_completed_line;
        if reset_requested && !self_managed {
            local.session_token = None;
            local.session_usage = None;
            local.estimated_tokens = 0;
            local.last_session_usage = None;
            local.reset_required = false;
            local.bump_context_generation();
        }
        local.save(&self.config.workspace_root, session_id)?;
        self.emit(
            "done",
            if reset_requested { "reset" } else { "done" },
            session_id,
            &attempt_id,
            claim.revision,
            captured_generation,
        );
        let requeue = is_actionable(&after, &self.config.handler);
        Ok(QuickSessionRunOutcome::Completed { requeue })
    }

    fn build_system_prompt(&self, provider: &dyn Provider) -> String {
        let ctx = PromptContext {
            handler: &self.config.handler,
            model: self.config.model.as_deref(),
        };
        let mut prompt = provider.build_system_prompt(&ctx);
        if let Some(custom) = self
            .config
            .custom_system_prompt
            .as_deref()
            .filter(|prompt| !prompt.is_empty())
        {
            prompt.push_str("\n\n## User instructions\n\n");
            prompt.push_str(custom);
        }
        prompt.push_str(
            "\n\n## Quick Session turn\n\n\
             Use only the supplied `gitim session title`, `gitim session send`, and \
             `gitim session summarize` commands for this conversation. Set the title before \
             the first reply. Do not send the Quick Session response to channels, DMs, or cards.",
        );
        prompt
    }

    async fn mark_claim_error(
        &self,
        session_id: &str,
        attempt_id: &str,
        diagnostic: &str,
    ) -> Result<bool, RuntimeError> {
        let detail = self
            .backend
            .read(session_id)
            .await
            .map_err(provider_error)?
            .session;
        if detail.archived
            || detail.meta.status != QuickSessionStatus::Running
            || detail.meta.attempt_id.as_deref() != Some(attempt_id)
        {
            return Ok(false);
        }
        self.backend
            .mark_error(session_id, attempt_id, diagnostic)
            .await
            .map_err(provider_error)?;
        Ok(true)
    }

    async fn failed_outcome(
        &self,
        session_id: &str,
    ) -> Result<QuickSessionRunOutcome, RuntimeError> {
        let detail = self
            .backend
            .read(session_id)
            .await
            .map_err(provider_error)?
            .session;
        Ok(QuickSessionRunOutcome::Failed {
            requeue: is_actionable(&detail, &self.config.handler),
        })
    }

    fn accumulate_usage(
        &self,
        provider: &dyn Provider,
        state: &mut QuickSessionRuntimeState,
        session_id: &str,
        reported: Option<&ProviderUsage>,
    ) {
        let delta = normalize_usage(provider, state, session_id, reported);
        let model = self.config.model.as_deref().unwrap_or("");
        let mut log = AgentUsageLog::load_or_default(
            &self.config.workspace_root,
            &self.config.handler,
            &self.config.provider_type,
            model,
            provider.reports_usage(),
        );
        let now = chrono::Utc::now();
        let today = now.format("%Y-%m-%d").to_string();
        log.accumulate(&today, delta.as_ref(), &now.to_rfc3339());
        if let Err(error) = log.save(&self.config.workspace_root, &today) {
            tracing::warn!(
                session_id,
                error = %error,
                "failed to save quick session usage"
            );
        }
    }

    fn emit(
        &self,
        event_type: &str,
        detail: &str,
        session_id: &str,
        attempt_id: &str,
        revision: u64,
        generation: u64,
    ) {
        if let Some(tx) = &self.config.activity_tx {
            let _ = tx.send(AgentActivityEvent {
                agent_id: self.config.handler.clone(),
                workspace_id: self.config.workspace_id.clone(),
                event_type: event_type.to_string(),
                detail: detail.to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                scope: ActivityScope::QuickSession,
                session_id: Some(session_id.to_string()),
                r#ref: Some(format!("session:{session_id}")),
                session_revision: Some(revision),
                attempt_id: Some(attempt_id.to_string()),
                context_generation: Some(generation),
            });
        }
    }
}

fn provider_error(error: String) -> RuntimeError {
    RuntimeError::ProviderFailed(error)
}

fn latest_creator_line(detail: &QuickSessionDetail) -> Option<u64> {
    detail
        .entries
        .iter()
        .filter(|entry| entry.author().as_str() == detail.meta.created_by)
        .map(ThreadEntry::line_number)
        .max()
        .or(detail.meta.last_human_line)
}

fn is_actionable(detail: &QuickSessionDetail, handler: &str) -> bool {
    !detail.archived
        && detail.meta.agent_id == handler
        && matches!(
            detail.meta.status,
            QuickSessionStatus::NeedsTitle | QuickSessionStatus::Active
        )
        && latest_creator_line(detail).is_some_and(|line| {
            detail
                .meta
                .last_completed_input_line
                .is_none_or(|completed| line > completed)
        })
}

fn generate_attempt_id() -> String {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut value = uuid::Uuid::new_v4().as_u128();
    let mut encoded = [b'0'; 26];
    for index in (0..26).rev() {
        encoded[index] = ALPHABET[(value & 31) as usize];
        value >>= 5;
    }
    format!("qa-{}", String::from_utf8_lossy(&encoded))
}

fn build_quick_session_prompt(
    detail: &QuickSessionDetail,
    attempt_id: &str,
    input_line: u64,
    reset_required: bool,
) -> String {
    let title = detail.meta.title.as_deref().unwrap_or("(unset)");
    let summary = detail.meta.summary.as_deref().unwrap_or("(none)");
    let reset = if reset_required {
        "\nContext handoff required: update the durable summary with `gitim session summarize`, then emit [[RESET]].\n"
    } else {
        ""
    };
    let mut prompt = format!(
        "Quick Session execution claim\n\
         Session id: {}\n\
         Ref: session:{}\n\
         Attempt: {}\n\
         Input line: L{}\n\
         Title: {}\n\
         Summary: {}\n\
         {}\n\
         Commands:\n\
         gitim session title {} <title> --attempt-id {}\n\
         gitim session send {} --stdin --reply-to {} --attempt-id {}\n\
         gitim session summarize {} --stdin --attempt-id {}\n\
         \nBounded transcript (newest first):\n",
        detail.meta.id,
        detail.meta.id,
        attempt_id,
        input_line,
        title,
        summary,
        reset,
        detail.meta.id,
        attempt_id,
        detail.meta.id,
        input_line,
        attempt_id,
        detail.meta.id,
        attempt_id,
    );
    let mut remaining = PROMPT_CHAR_LIMIT.saturating_sub(prompt.chars().count());
    let mut newest: Vec<&ThreadEntry> = detail.entries.iter().collect();
    newest.sort_by_key(|entry| std::cmp::Reverse(entry.line_number()));
    let claimed_entry = newest
        .iter()
        .find(|entry| entry.line_number() == input_line)
        .copied();
    let mut selected: Vec<&ThreadEntry> = newest.into_iter().take(TRANSCRIPT_ENTRY_LIMIT).collect();
    if let Some(claimed_entry) = claimed_entry {
        if !selected
            .iter()
            .any(|entry| entry.line_number() == input_line)
        {
            selected.push(claimed_entry);
            selected.sort_by_key(|entry| std::cmp::Reverse(entry.line_number()));
        }
    }
    let claimed_is_present = claimed_entry.is_some();
    let mut claimed_rendered = false;
    for entry in selected {
        if matches!(entry, ThreadEntry::Event(_)) {
            continue;
        }
        let is_claimed = entry.line_number() == input_line;
        let reserve = if claimed_is_present && !claimed_rendered && !is_claimed {
            CLAIMED_ENTRY_RESERVE.min(remaining)
        } else {
            0
        };
        let allowance = remaining.saturating_sub(reserve);
        let Some(rendered) = render_bounded_entry(entry, allowance) else {
            continue;
        };
        remaining = remaining.saturating_sub(rendered.chars().count());
        prompt.push_str(&rendered);
        claimed_rendered |= is_claimed;
        if remaining == 0 {
            break;
        }
    }
    prompt
}

fn render_bounded_entry(entry: &ThreadEntry, allowance: usize) -> Option<String> {
    const TRUNCATED: &str = " … [truncated]\n";
    let ThreadEntry::Message(message) = entry else {
        return None;
    };
    let prefix = format!("L{} @{}: ", entry.line_number(), entry.author().as_str());
    let full_len = prefix.chars().count() + message.body.chars().count() + 1;
    if full_len <= allowance {
        return Some(format!("{prefix}{}\n", message.body));
    }
    let fixed = prefix.chars().count() + TRUNCATED.chars().count();
    if allowance <= fixed {
        return None;
    }
    let body_budget = allowance - fixed;
    let body: String = message.body.chars().take(body_budget).collect();
    Some(format!("{prefix}{body}{TRUNCATED}"))
}

fn normalize_usage(
    provider: &dyn Provider,
    state: &mut QuickSessionRuntimeState,
    session_id: &str,
    reported: Option<&ProviderUsage>,
) -> Option<ProviderUsage> {
    if !provider.reports_usage() {
        return None;
    }
    let current = reported?.clone();
    if !provider.usage_is_cumulative() {
        return Some(current);
    }
    let baseline = match &state.last_session_usage {
        Some(previous) if previous.session_id == session_id => previous.usage.clone(),
        _ => ProviderUsage::default(),
    };
    let delta = ProviderUsage {
        input_tokens: Some(
            current
                .input_tokens
                .unwrap_or(0)
                .saturating_sub(baseline.input_tokens.unwrap_or(0)),
        ),
        output_tokens: Some(
            current
                .output_tokens
                .unwrap_or(0)
                .saturating_sub(baseline.output_tokens.unwrap_or(0)),
        ),
        cache_read_tokens: Some(
            current
                .cache_read_tokens
                .unwrap_or(0)
                .saturating_sub(baseline.cache_read_tokens.unwrap_or(0)),
        ),
        cache_creation_tokens: Some(
            current
                .cache_creation_tokens
                .unwrap_or(0)
                .saturating_sub(baseline.cache_creation_tokens.unwrap_or(0)),
        ),
        used_percent: current.used_percent,
        context_tokens: current.context_tokens,
        context_window_tokens: current.context_window_tokens,
    };
    state.last_session_usage = Some(LastSessionUsage {
        session_id: session_id.to_string(),
        usage: current,
    });
    Some(delta)
}
