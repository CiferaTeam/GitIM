use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gitim_agent_provider::{
    create, ExecOptions, ExecStatus, PromptContext, Provider, ProviderConfig, ProviderUsage,
};
use gitim_client::{ensure_daemon_with_log, GitimClient};
use serde::Serialize;
use tokio::sync::broadcast;
use tracing::info;

use crate::context_window::WARN_AT_PERCENT;
use crate::error::RuntimeError;
use crate::hermes_profile;
use crate::http::{AgentActivityEvent, SharedRuntimeState};
use crate::poller::{ChannelChange, Poller};
use crate::state::{AgentState, LastSessionUsage, SessionUsageSnapshot, UsageSource};
use crate::usage_log::{AgentUsageLog, UsageSummary};

#[derive(Debug, Clone, Default)]
pub struct AgentLoopConfig {
    pub provider_type: String,
    pub handler: String,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub env: HashMap<String, String>,
}

/// SSE `"usage"` event payload. The existing SessionUsageSnapshot fields
/// are flattened to keep frontends that destructure them (e.g. older
/// `use-agent-activity.ts`) working unchanged. `usage_summary` is added as
/// a sibling for clients that need cumulative+today numbers without an
/// extra HTTP round-trip.
#[derive(Serialize)]
struct UsageEventPayload<'a> {
    #[serde(flatten)]
    snap: &'a SessionUsageSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage_summary: Option<&'a UsageSummary>,
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
    /// Workspace root used to locate the per-agent token usage log at
    /// `<workspace>/.gitim-runtime/usage/<handler>.json`. None when running
    /// outside the runtime HTTP shell (CLI / unit tests); the accumulator
    /// path is skipped in that case.
    workspace_root: Option<PathBuf>,
}

impl AgentLoop {
    /// Defaults: provider `claude`, handler from `.gitim/me.json`,
    /// state restored from disk if present.
    pub fn with_defaults(repo_root: &Path) -> Result<Self, RuntimeError> {
        let handler = read_handler_from_me_json(repo_root)?;
        Self::with_provider(repo_root, "claude", &handler)
    }

    /// Explicit provider + handler. State restored from disk if present.
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

        let provider_config = build_provider_config(provider_type, handler, HashMap::new())?;
        let provider = create(provider_type, provider_config)
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
            workspace_root: None,
        })
    }

    /// Build an AgentLoop with full config (model, env, system_prompt).
    pub fn with_config(repo_root: &Path, config: &AgentLoopConfig) -> Result<Self, RuntimeError> {
        let state = AgentState::load(repo_root)?;

        let poller = match state.cursor {
            Some(cursor) => {
                info!(cursor = %cursor, "restored cursor from state");
                Poller::with_cursor(GitimClient::new(repo_root), cursor)
            }
            None => Poller::new(GitimClient::new(repo_root)),
        };

        let provider_config =
            build_provider_config(&config.provider_type, &config.handler, config.env.clone())?;
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
            workspace_root: None,
        })
    }

    /// Attach a reference to the runtime's shared state so per-turn usage
    /// snapshots can be patched into `AgentInfo.session_usage` in place.
    /// Must be called after construction and before the loop spawns; tests
    /// that don't drive HTTP handlers can skip this entirely.
    pub fn set_runtime_state(&mut self, state: SharedRuntimeState) {
        self.runtime_state = Some(state);
    }

    /// Attach the workspace root so the per-agent token usage log can be
    /// resolved to `<workspace>/.gitim-runtime/usage/<handler>.json`.
    /// Standalone CLI and unit-test paths skip this; the accumulator gates
    /// on the field being `Some`.
    pub fn set_workspace_root(&mut self, root: PathBuf) {
        self.workspace_root = Some(root);
    }

    /// Test-only seam to swap the underlying provider after construction.
    /// Production paths run through `with_provider` / `with_config` which
    /// resolve the provider through the public `create()` factory; this
    /// entry point exists so the e2e suite can inject a mock that declares
    /// `usage_is_cumulative()` (or other trait flags) without round-
    /// tripping through env vars.
    #[doc(hidden)]
    pub fn replace_provider_for_test(&mut self, provider: Box<dyn Provider>) {
        self.provider = provider;
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

    pub async fn ensure_daemon_running(&self) -> Result<(), RuntimeError> {
        let repo_root = self.repo_root.clone();
        let log_path = crate::daemon_log::daemon_log_path(&repo_root);
        tokio::task::spawn_blocking(move || ensure_daemon_with_log(&repo_root, &log_path))
            .await
            .map_err(|e| {
                RuntimeError::PollFailed(format!("daemon restart task panicked: {e}"))
            })??;
        Ok(())
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
    ///
    /// Public to allow E2E integration tests (`tests/session_usage_e2e.rs`,
    /// `tests/threshold_injection_e2e.rs`) to drive the computation + persist
    /// path directly without wiring up a real provider + daemon. Production
    /// callers should continue to go through `run_once`.
    pub fn update_session_usage(
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
            self.provider.usage_is_cumulative(),
            &now,
        );

        let prev_pct = state.session_usage.as_ref().map(|s| s.used_percent);
        let self_managed = self.provider.self_managed_context();
        if let Some(snap) = &new_snapshot {
            if just_crossed_threshold(prev_pct, snap.used_percent) {
                // Self-managed providers (e.g. hermes) compress in-loop and
                // do not need the runtime-side `[系统通知]` preamble +
                // `[[RESET]]` handoff. Log the crossing for observability but
                // do not arm the preamble. For other providers the default
                // pressure-relief flow stays unchanged.
                if !self_managed {
                    state.usage_notice_pending = true;
                }
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
                    self_managed = self_managed,
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

        // Step A — Normalize provider_reported into a per-turn delta. We do
        // this *here*, after compute_snapshot has already used the raw value
        // for percentage display, so the visible HUD remains driven by the
        // provider's authoritative cumulative number. The delta is what the
        // statistics layer accumulates.
        let delta = self.normalize_to_delta(state, session_id, provider_reported);

        // Persist updated last_session_usage baseline alongside session_usage.
        state.save(&self.repo_root)?;

        // Step B — Accumulate the delta into the per-agent usage log. Save
        // failures bump a counter and warn-log; they cannot fail the turn.
        let usage_summary = self.accumulate_usage_log(delta.as_ref());

        // Step C — Patch the in-memory AgentInfo so polling clients
        // (GET /agents/:id) see fresh data without re-reading disk. Both
        // mirrors update unconditionally (independent of snapshot
        // availability) so a turn from a provider without `reports_usage`
        // still bumps the in-memory turn counter via usage_summary.
        if let Some(rs) = &self.runtime_state {
            if let Ok(mut s) = rs.lock() {
                if let Some(ctx) = s.workspaces.get_mut(&self.workspace_id) {
                    if let Some(info) = ctx.agents.get_mut(&self.handler) {
                        if let Some(snap) = &new_snapshot {
                            info.session_usage = Some(snap.clone());
                        }
                        if let Some(summary) = &usage_summary {
                            info.usage_summary = Some(summary.clone());
                        }
                    }
                }
            }
        }

        // Step D — Broadcast the "usage" SSE event when we have a snapshot.
        // Payload keeps the existing SessionUsageSnapshot fields inline (so
        // older WebUI clients that destructure them keep working) and adds
        // `usage_summary` as a sibling for new clients. The SSE channel is
        // snap-driven historically; we don't fabricate one when the
        // provider has nothing to say about session occupancy — clients
        // will see the new totals on the next GET /agents poll instead.
        if let Some(snap) = &new_snapshot {
            let payload = UsageEventPayload {
                snap,
                usage_summary: usage_summary.as_ref(),
            };
            let detail = serde_json::to_string(&payload).unwrap_or_default();
            self.emit_activity("usage", &detail);
        }

        Ok(())
    }

    /// Refresh the WebUI/context HUD from a provider's live usage event.
    ///
    /// This deliberately does not mutate AgentState on disk, update
    /// `last_session_usage`, accumulate token logs, or arm threshold reset
    /// notices. Those are per-turn accounting decisions and remain owned by
    /// `update_session_usage` after `provider.execute()` returns.
    fn update_live_session_usage(&self, session_id: &str, provider_reported: &ProviderUsage) {
        let estimated_tokens = AgentState::load(&self.repo_root)
            .map(|state| state.estimated_tokens)
            .unwrap_or(0);
        let model = self.model.as_deref().unwrap_or("");
        let max = crate::context_window::default_max_tokens(&self.provider_type, model);
        let now = chrono::Utc::now().to_rfc3339();
        let Some(snap) = compute_snapshot(
            session_id,
            Some(provider_reported),
            estimated_tokens,
            max,
            self.provider.usage_is_cumulative(),
            &now,
        ) else {
            return;
        };

        if let Some(rs) = &self.runtime_state {
            if let Ok(mut s) = rs.lock() {
                if let Some(ctx) = s.workspaces.get_mut(&self.workspace_id) {
                    if let Some(info) = ctx.agents.get_mut(&self.handler) {
                        info.session_usage = Some(snap.clone());
                    }
                }
            }
        }

        let payload = UsageEventPayload {
            snap: &snap,
            usage_summary: None,
        };
        let detail = serde_json::to_string(&payload).unwrap_or_default();
        self.emit_activity("usage", &detail);
    }

    /// Convert the provider's raw `ProviderUsage` into a per-turn delta.
    ///
    /// - When the provider declares `reports_usage() == false` (gemini,
    ///   openclaw), there is no delta — the accumulator will only count
    ///   turns.
    /// - When `usage_is_cumulative() == true` (codex), subtract the
    ///   `last_session_usage` baseline and update it. saturating_sub makes
    ///   non-monotone counters (cache invalidation upstream) safe; we warn
    ///   when we see the regression so it doesn't pass silently.
    /// - Otherwise the provider already reports per-turn deltas, so we
    ///   forward as-is.
    fn normalize_to_delta(
        &self,
        state: &mut AgentState,
        session_id: &str,
        provider_reported: Option<&ProviderUsage>,
    ) -> Option<ProviderUsage> {
        if !self.provider.reports_usage() {
            return None;
        }
        let current = provider_reported?.clone();
        if !self.provider.usage_is_cumulative() {
            return Some(current);
        }
        let baseline = match &state.last_session_usage {
            Some(prev) if prev.session_id == session_id => prev.usage.clone(),
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
            // used_percent is provider-authoritative; passing through but the
            // accumulator ignores it.
            used_percent: current.used_percent,
            context_tokens: current.context_tokens,
            context_window_tokens: current.context_window_tokens,
        };

        // Cache reads aren't monotone (Anthropic's prompt cache can be
        // invalidated upstream). Warn loudly so a sustained regression is
        // visible without scraping all turn logs.
        if current.cache_read_tokens.unwrap_or(0) < baseline.cache_read_tokens.unwrap_or(0) {
            tracing::warn!(
                handler = %self.handler,
                session_id = %session_id,
                current = current.cache_read_tokens.unwrap_or(0),
                baseline = baseline.cache_read_tokens.unwrap_or(0),
                "cache_read decreased between turns; likely upstream cache invalidation"
            );
        }

        state.last_session_usage = Some(LastSessionUsage {
            session_id: session_id.to_string(),
            usage: current,
        });

        Some(delta)
    }

    /// Load the agent's usage log, accumulate the turn delta, save back.
    /// Returns the freshly computed `UsageSummary` so callers can patch
    /// in-memory state and broadcast it. Returns `None` when no workspace
    /// root is wired (CLI / unit tests).
    ///
    /// Save failures bump `usage_save_failures` on `RuntimeState` and warn —
    /// they never propagate. The runtime's HUD percentage still works off
    /// the per-turn snapshot regardless of accumulator health.
    fn accumulate_usage_log(&self, delta: Option<&ProviderUsage>) -> Option<UsageSummary> {
        let workspace_root = self.workspace_root.as_ref()?;
        let model = self.model.as_deref().unwrap_or("");
        let mut log = AgentUsageLog::load_or_default(
            workspace_root,
            &self.handler,
            &self.provider_type,
            model,
            self.provider.reports_usage(),
        );
        let now = chrono::Utc::now();
        let today = now.format("%Y-%m-%d").to_string();
        let now_iso = now.to_rfc3339();
        log.accumulate(&today, delta, &now_iso);
        if let Err(e) = log.save(workspace_root, &today) {
            tracing::warn!(
                handler = %self.handler,
                error = %e,
                "failed to save token usage log"
            );
            if let Some(rs) = &self.runtime_state {
                if let Ok(s) = rs.lock() {
                    s.usage_save_failures
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }
        Some(log.summary(&today))
    }

    /// Clear the in-memory mirror of `session_usage` on the runtime's shared
    /// state and broadcast an empty `usage` activity event.
    ///
    /// Pairs with `AgentState::clear_session()` on disk so the WebUI HUD does
    /// not keep displaying the pre-reset percentage after `[[RESET]]` or a
    /// failed execute. A missing runtime_state or activity_tx is fine —
    /// standalone CLI / tests skip silently.
    fn clear_runtime_session_usage(&self) {
        if let Some(rs) = &self.runtime_state {
            if let Ok(mut s) = rs.lock() {
                if let Some(ctx) = s.workspaces.get_mut(&self.workspace_id) {
                    if let Some(info) = ctx.agents.get_mut(&self.handler) {
                        info.session_usage = None;
                    }
                }
            }
        }
        // Empty payload tells reactive clients to drop their cached snapshot.
        self.emit_activity("usage", "");
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

        // Load state up front so we can decide between "real idle" and
        // "post-reset self-wake" before any early return. The runtime arms
        // `post_reset_pending` when the previous cycle ended in `[[RESET]]`,
        // and the next cycle's job is to give the cold-started agent one
        // synthetic kick — otherwise the orientation notes it just wrote
        // would sit unread until something external happens to mention it.
        let mut state = AgentState::load(&self.repo_root)?;

        let external_prompt = if result.changes.is_empty() {
            None
        } else {
            // `format_changes_as_prompt` returns None when every entry was
            // self-authored — semantically the same as "no external input"
            // from the agent's point of view.
            format_changes_as_prompt(&result.changes, &self.handler)
        };

        // Decide what (if anything) to run this cycle:
        //   - external input present                  → run with that prompt
        //   - no external input, post_reset armed     → run a cold-start kick
        //   - no external input, no continuation flag → idle
        let prompt = match (external_prompt, state.post_reset_pending) {
            (Some(p), _) => p,
            (None, true) => {
                tracing::info!(
                    handler = %self.handler,
                    "post-reset self-wake: empty poll, kicking cold-start continuation turn"
                );
                // Sentinel — the post_reset_pending branch below replaces
                // an empty body with the preamble itself.
                String::new()
            }
            (None, false) => {
                tracing::debug!("idle: no external changes, no post-reset continuation pending");
                self.save_state()?;
                return Ok(false);
            }
        };

        info!(prompt = %prompt, "sending to provider");
        self.emit_activity("thinking", "processing...");

        let opts = self.build_exec_options();

        // Consume post_reset_pending FIRST. If the body is empty (no external
        // input on this cycle) the preamble stands alone; otherwise it
        // prefixes the real changes so the agent sees both "you just
        // cold-started" *and* "and by the way here's a new message" in one
        // turn. Clearing the flag before execute mirrors usage_notice's
        // safety — a mid-turn crash won't cause it to re-fire on restart.
        let prompt = if state.post_reset_pending {
            let preamble = build_post_reset_preamble();
            state.post_reset_pending = false;
            if prompt.is_empty() {
                preamble
            } else {
                format!("{preamble}\n\n---\n\n{prompt}")
            }
        } else {
            prompt
        };

        // Consume usage_notice_pending and accumulate the tiktoken estimate
        // in a single load/save cycle. If the notice flag was set by a
        // previous turn's threshold crossing, prepend the system preamble
        // here and clear the flag before execute — this way a mid-turn
        // crash won't cause the notice to re-fire.
        //
        // In practice usage_notice and post_reset are mutually exclusive
        // (clear_session wipes usage_notice as part of the RESET path), but
        // ordering this *after* post_reset is the safer composition: a stray
        // usage_notice from before a hand-rolled state edit still gets its
        // own preamble layered on top.
        let prompt = if state.usage_notice_pending {
            let pct = state
                .session_usage
                .as_ref()
                .map(|s| s.used_percent)
                .unwrap_or(80.0);
            let preamble = build_usage_notice_preamble(pct);
            state.usage_notice_pending = false;
            format!("{preamble}\n\n---\n\n{prompt}")
        } else {
            prompt
        };

        // Pre-execute tiktoken accumulation.
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
        // Buffer everything the model emitted or consumed this turn for the
        // tiktoken estimate: assistant text, thinking blocks, tool-use JSON
        // arguments, tool results that feed back as next-turn input. Tool I/O
        // dominates real context once tools fire (file reads, bash output,
        // grep), so dropping it leaves the fallback estimate underreporting
        // by an order of magnitude vs. the real provider-reported usage —
        // which is why providers without `usage` (gemini/hermes/openclaw/pi
        // pre-fix) showed ~5% while Claude reported ~35% on identical work.
        // Intentionally uncapped (unlike text_tail) — we want a complete
        // turn footprint, not a sliding window.
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
                                        let safe = floor_char_boundary(&text_tail, cut);
                                        text_tail.drain(..safe);
                                    }
                                }
                                gitim_agent_provider::Event::ToolUse { tool, input, .. } => {
                                    let snippet = summarize_tool_input(tool, input);
                                    info!(tool = %tool, input = %snippet, "agent tool use");
                                    self.emit_activity("tool_use", &format!("{tool}: {snippet}"));
                                    // Tool-call arguments are emitted by the model and consume
                                    // assistant tokens. Snippet is for human display; the model
                                    // emits the full JSON, so we tokenize that.
                                    assistant_text_buf.push_str(tool);
                                    assistant_text_buf.push(' ');
                                    assistant_text_buf.push_str(&input.to_string());
                                    assistant_text_buf.push('\n');
                                }
                                gitim_agent_provider::Event::ToolResult { call_id, output } => {
                                    tracing::debug!(call_id = %call_id, output_len = output.len(), "tool result");
                                    // Tool results land in the next turn's input. Counting them
                                    // here cumulatively over the session is the right shape:
                                    // estimated_tokens is reset on cold start and accumulated
                                    // until [[RESET]] / failure clears it.
                                    assistant_text_buf.push_str(output);
                                    assistant_text_buf.push('\n');
                                }
                                gitim_agent_provider::Event::Thinking { content } => {
                                    // Extended-thinking blocks are real context tokens for
                                    // models that emit them (Claude extended thinking, o1
                                    // reasoning summaries when surfaced).
                                    assistant_text_buf.push_str(content);
                                    assistant_text_buf.push('\n');
                                }
                                gitim_agent_provider::Event::Error { content } => {
                                    tracing::warn!(error = %content, "agent error event");
                                    self.emit_activity("error", content);
                                }
                                gitim_agent_provider::Event::Usage { session_id, usage } => {
                                    self.update_live_session_usage(session_id, usage);
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
        //
        // Reset is invisible to IM (no chat message), but the WebUI HUD still
        // needs a terminal signal — otherwise the `thinking`/`processing...`
        // spinner stays on indefinitely and the pre-reset `used_percent`
        // sticks on screen. Emit a `done` activity with `detail="reset"` so
        // reactive clients can terminate the spinner without showing a
        // user-facing completion message, and clear the in-memory
        // session_usage mirror to match the on-disk clear_session().
        if reset_requested {
            info!(
                handler = %self.handler,
                duration_ms = exec_result.duration_ms,
                "context reset complete, clearing session_token"
            );
            self.session_token = None;
            let mut state = AgentState::load(&self.repo_root)?;

            // Accumulate the final pre-reset turn into the usage log before
            // we wipe the session. The token statistics layer is supposed
            // to survive resets — skipping this branch would silently drop
            // the very turns that triggered the reset (high context
            // pressure → biggest tokens of the session). Reuses
            // update_session_usage so the in-memory mirror also reflects
            // this turn briefly; clear_runtime_session_usage immediately
            // below clears it again, which is the correct end state.
            let sid_for_accumulate: String = exec_result
                .session_token
                .clone()
                .or_else(|| state.session_token.clone())
                .unwrap_or_default();
            if !sid_for_accumulate.is_empty() {
                state.estimated_tokens += crate::context_window::tokenize_for_provider(
                    &self.provider_type,
                    &assistant_text_buf,
                );
                self.update_session_usage(
                    &mut state,
                    exec_result.usage.as_ref(),
                    &sid_for_accumulate,
                )?;
            }

            let sid_for_log = state.session_usage.as_ref().map(|s| s.session_id.clone());
            state.clear_session();
            // Arm the post-reset self-wake so the next run_once cycle kicks
            // a cold-start turn even if the poll comes back empty. Without
            // this the agent would silently fall off the loop the moment
            // it handed off — the orientation notes it just wrote are not
            // channel events, so nothing else would wake it.
            state.post_reset_pending = true;
            state.save(&self.repo_root)?;
            tracing::info!(session_id = ?sid_for_log, reason = "agent_emitted_reset", "session_reset");
            self.clear_runtime_session_usage();
            self.emit_activity("done", "reset");
            self.save_state()?;
            return Ok(true);
        }

        let duration_s = exec_result.duration_ms as f64 / 1000.0;
        let provider_completed = if is_provider_failure_status(&exec_result.status) {
            tracing::error!(
                status = ?exec_result.status,
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
            // Mirror the on-disk clear into the runtime's shared state
            // so the HUD stops showing a stale percentage.
            self.clear_runtime_session_usage();
            false
        } else {
            match exec_result.status {
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
                    true
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
                    true
                }
            }
        };

        self.save_state()?;
        Ok(provider_completed)
    }

    /// Run the agent loop indefinitely with exponential backoff on errors.
    ///
    /// Stops cleanly when poll surfaces `RuntimeError::SelfDeparted` — the
    /// agent's own handler has been archived, so retrying would just re-trip
    /// the daemon's self-departed gate. This standalone entry point cannot
    /// run the WebUI-side cleanup (no `SharedRuntimeState`), so it just
    /// returns — the runtime spawn loop in `http.rs::start_agent_loop`
    /// holds the production self-heal that drives clone removal + SSE.
    pub async fn run(&mut self) -> Result<(), RuntimeError> {
        if self.poller.cursor().is_none() {
            match self.poller.poll().await {
                Ok(_) => {
                    self.save_state()?;
                    info!("agent loop started, cursor initialized");
                }
                Err(RuntimeError::SelfDeparted) => {
                    info!(
                        handler = %self.handler,
                        "agent self-departed during cursor init; exiting loop"
                    );
                    return Ok(());
                }
                Err(e) => return Err(e),
            }
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
                Err(RuntimeError::SelfDeparted) => {
                    info!(
                        handler = %self.handler,
                        "agent self-departed; exiting loop"
                    );
                    return Ok(());
                }
                Err(e) => {
                    if is_daemon_not_running_poll_error(&e) {
                        tracing::warn!(
                            handler = %self.handler,
                            "agent daemon missing during poll; attempting restart"
                        );
                        match self.ensure_daemon_running().await {
                            Ok(()) => {
                                consecutive_errors = 0;
                                tracing::info!(
                                    handler = %self.handler,
                                    "agent daemon restarted after poll failure"
                                );
                                continue;
                            }
                            Err(restart_err) => {
                                tracing::error!(
                                    handler = %self.handler,
                                    error = %restart_err,
                                    "agent daemon restart failed"
                                );
                            }
                        }
                    }
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

fn is_provider_failure_status(status: &ExecStatus) -> bool {
    matches!(status, ExecStatus::Failed | ExecStatus::Timeout)
}

pub(crate) fn is_daemon_not_running_poll_error(error: &RuntimeError) -> bool {
    matches!(error, RuntimeError::PollFailed(msg) if msg == "daemon not running")
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
        format!("{}…", &raw[..floor_char_boundary(&raw, MAX)])
    }
}

/// Stable replacement for `str::floor_char_boundary` (nightly-only under
/// feature `round_char_boundary`, tracking issue #93743). Returns the largest
/// `j <= i` such that `s.is_char_boundary(j)` — i.e. a safe slice endpoint
/// that won't split a UTF-8 character. Bounded to `s.len()`.
fn floor_char_boundary(s: &str, i: usize) -> usize {
    let mut j = i.min(s.len());
    while j > 0 && !s.is_char_boundary(j) {
        j -= 1;
    }
    j
}

/// Format channel changes into a prompt, filtering out self-authored messages.
/// Returns `None` if no external events remain after filtering.
pub fn format_changes_as_prompt(changes: &[ChannelChange], self_handler: &str) -> Option<String> {
    let mut prompt = String::from("以下是你上次醒来后发生的事件：\n\n");
    let mut has_external = false;
    let mention = format!("@{self_handler}");

    for change in changes {
        if change.kind == "channel_meta" {
            continue;
        }

        for entry in &change.entries {
            let author = entry["author"].as_str().unwrap_or("unknown");

            if author == self_handler {
                continue;
            }

            // Recipients-based routing filter.
            //
            // Daemon attaches `recipients: [handler...]` to each
            // routable message entry (channel + dm). If the array is
            // present and non-empty, this message is only "for" the
            // listed handlers — skip if self isn't one of them.
            //
            // Empty array or missing field falls back to broadcast
            // (legacy behavior). That covers three cases at once:
            // (1) old daemon without the field, (2) card_thread and
            // cron_thread which intentionally don't route, (3) the
            // daemon-side empty-recipients warn fallback for malformed
            // channel meta.
            if let Some(recipients) = entry["recipients"].as_array() {
                if !recipients.is_empty()
                    && !recipients.iter().any(|v| v.as_str() == Some(self_handler))
                {
                    continue;
                }
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
            let mention_tag = if body.contains(&mention) {
                "[MENTION] "
            } else {
                ""
            };
            let scope = match change.kind.as_str() {
                "dm" => format!("[DM {}]", channel.strip_prefix("dm:").unwrap_or(channel)),
                "card_thread" => {
                    format!(
                        "[CARD {}]",
                        channel.strip_prefix("card:").unwrap_or(channel)
                    )
                }
                // Cron fires arrive as `kind: "cron_thread"` keyed by
                // `cron:<name>`. The wake-up trigger is structural (the
                // engine wrote a synthetic `[@system]` message) — not a
                // mention — so the [MENTION] tag never applies here even
                // if the prompt body happens to contain the agent's
                // handler. We still pass the body through unchanged so
                // the agent sees its full prompt template.
                "cron_thread" => {
                    format!(
                        "[CRON {}]",
                        channel.strip_prefix("cron:").unwrap_or(channel)
                    )
                }
                _ => format!("[#{channel}]"),
            };

            if line_id.is_empty() {
                prompt.push_str(&format!("{ts}{mention_tag}{scope} @{author}: {body}\n"));
            } else {
                prompt.push_str(&format!(
                    "{ts}{mention_tag}{scope} {line_id} @{author}: {body}\n"
                ));
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
/// 1. provider_reported.used_percent — providers that compute it
///    against their own context window (Codex rate-limit %).
/// 2. provider_reported.context_tokens / context_window_tokens —
///    explicit context occupancy from rollout (Codex).
/// 3. provider_reported is session-cumulative (`usage_is_cumulative`) —
///    fall through to the estimate path so the ratio resets per turn
///    instead of monotonically clamping at 100% (codex
///    total_token_usage, hermes session_input_tokens).
/// 4. provider_reported.(input + cache_read + cache_creation) / max_tokens (Claude).
/// 5. estimated_tokens / max_tokens (cl100k fallback).
/// 6. None (no data available).
///
/// For Claude, Anthropic's `input_tokens` excludes tokens served from the
/// prompt cache. With caching active, `input_tokens` drops to a few hundred
/// per turn while `cache_read_input_tokens` carries 100k+ of context. The
/// true occupancy is the sum — using `input_tokens` alone collapses the
/// percentage to ~0% (see `parse_result_with_cache_only_has_tiny_input`).
///
/// `used_percent` is clamped to `[0, 100]`. Callers are responsible for
/// logging unusual values (e.g. >110 as a protocol-drift signal).
pub fn compute_snapshot(
    session_id: &str,
    provider_reported: Option<&ProviderUsage>,
    estimated_tokens: u64,
    max_tokens: Option<u64>,
    usage_is_cumulative: bool,
    updated_at: &str,
) -> Option<SessionUsageSnapshot> {
    let (used_percent, source, input_tokens, output_tokens, snapshot_max_tokens) = if let Some(pu) =
        provider_reported
    {
        if let Some(pct) = pu.used_percent {
            // An explicit `used_percent` is authoritative — providers that
            // ship it (codex's rate-limit %, some opencode shapes) compute
            // it themselves against their context window. It is not derived
            // from the cumulative counts, so the cumulative-flag dodge does
            // not apply here.
            (
                pct,
                UsageSource::ProviderReported,
                pu.input_tokens,
                pu.output_tokens,
                max_tokens,
            )
        } else if let (Some(context_tokens), Some(context_window)) =
            (pu.context_tokens, pu.context_window_tokens)
        {
            if context_window == 0 {
                return compute_from_estimate(session_id, estimated_tokens, max_tokens, updated_at);
            }
            (
                (context_tokens as f64) / (context_window as f64) * 100.0,
                UsageSource::ProviderReported,
                pu.input_tokens,
                pu.output_tokens,
                Some(context_window),
            )
        } else if usage_is_cumulative {
            // Provider reports session-cumulative counts (codex
            // total_token_usage, hermes session_input_tokens, etc.). Using
            // those as the numerator against `max_tokens` makes the ratio
            // climb monotonically across turns and clamp at 100%, which
            // misrepresents per-LLM-call context occupancy. Fall back to
            // the cl100k estimate path — that one *is* per-turn-resetting.
            return compute_from_estimate(session_id, estimated_tokens, max_tokens, updated_at);
        } else if let Some(max) = max_tokens {
            let effective = effective_input_tokens(pu);
            if effective == 0 {
                return compute_from_estimate(session_id, estimated_tokens, max_tokens, updated_at);
            }
            let pct = (effective as f64) / (max as f64) * 100.0;
            (
                pct,
                UsageSource::ProviderReported,
                pu.input_tokens,
                pu.output_tokens,
                max_tokens,
            )
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
        max_tokens: snapshot_max_tokens,
        used_percent,
        source,
        updated_at: updated_at.to_string(),
    })
}

/// Sum of `input_tokens + cache_read_tokens + cache_creation_tokens` — the
/// real context-window occupancy for Claude turns. Missing fields count as 0.
fn effective_input_tokens(pu: &ProviderUsage) -> u64 {
    pu.input_tokens
        .unwrap_or(0)
        .saturating_add(pu.cache_read_tokens.unwrap_or(0))
        .saturating_add(pu.cache_creation_tokens.unwrap_or(0))
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
    let pct = (estimated_tokens as f64) / (max as f64) * 100.0;
    // Overflow guard: the estimator is a monotonic lower bound built from
    // `tokenize_for_provider(assistant_text_buf)` accumulated across turns.
    // Once it grows past max it has lost the resolution to mean anything —
    // clamping to 100 would trip `just_crossed_threshold` and inject the
    // `[[RESET]]` preamble, which on a cumulative-usage provider (codex
    // stdout `turn.completed.usage` is session-cumulative; no per-LLM-call
    // signal is available outside the rollout file) traps the agent in a
    // reset-spin without any real context pressure. Observed live on cfo
    // (gpt-5.5): estimated_tokens=518906, max=272000 → 190% → clamp 100 →
    // RESET loop.
    //
    // Returning None is the honest signal: "no trustworthy snapshot". The
    // HUD treats it as missing data; the threshold-crossing logic doesn't
    // fire. Provider-reported >=100 still lands via the ProviderReported
    // branch above and remains authoritative.
    if pct >= 100.0 {
        return None;
    }
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
/// `used_percent` first crosses `WARN_AT_PERCENT`. Written to speak to the
/// agent as a model would experience context pressure — "handoff, not
/// completion" framing. Per 01-design.md §4.5.
///
/// Uses a single flowing imperative that names the allowed tool surface
/// explicitly — a numbered list with "stop all tool calls" then "write
/// orientation to memory" is self-contradictory under pressure and the
/// agent resolves it loosely (misdirected side actions before `[[RESET]]`).
pub fn build_usage_notice_preamble(used_percent: f64) -> String {
    format!(
        "[系统通知] 对话窗口已用 {pct:.0}%。\n\
         \n\
         此刻对你最有价值的动作是给下一个窗口的你做一次干净的交接 —— \
         注意力被稀释后继续推进新任务的边际收益很低。\n\
         \n\
         立即只做这一件事：在你的记忆文件里写一段 orientation \
         （方向感，不是流水账）—— 当前任务位置 / 下一步该做什么 / \
         已经形成但还没落笔的判断、用户偏好、关键未决 tension —— \
         让冷启动的你能快速接回。\n\
         \n\
         允许的工具：Read 和 Edit 记忆文件。不要发消息、不要回复用户、\
         不要启动新任务。写完后，在输出末尾附加 [[RESET]]，\
         runtime 会给你开一个干净的新窗口。\n\
         \n\
         新窗口的你会读这些记忆文件冷启动 —— 你此刻留下什么，\
         它就从什么开始。本提醒仅发送一次。",
        pct = used_percent,
    )
}

/// One-shot preamble fired on the first turn after `[[RESET]]` when the
/// poll cycle has no external changes. Without it the agent would stay
/// idle indefinitely after a handoff — the orientation notes it just
/// wrote are passive artifacts (file edits, not channel messages), so
/// nothing in the poll stream would wake it again until a human or
/// another agent happens to ping.
///
/// The framing mirrors `build_usage_notice_preamble`: speak to the agent
/// as a model would experience the moment, name the allowed surface
/// explicitly, and give a clear "do nothing" exit so an empty memory
/// file doesn't spam noise into channels. The agent decides whether
/// there's real work to pick up; if not, this turn ends silently and
/// the runtime returns to its normal poll cadence.
pub fn build_post_reset_preamble() -> String {
    String::from(
        "[系统] 你刚完成上一窗口的 handoff 并冷启动 —— \
         runtime 还没收到新的外部消息，但上一个你可能在记忆文件里 \
         留了未完成的 orientation。\n\
         \n\
         请按这个顺序判断：\n\
         1. 读一下你的记忆文件，看上一窗口留了什么方向感 / 未决事项；\n\
         2. 如果有明确的未完工作 → 自然接回去推进；\n\
         3. 如果没有指引、或上下文已经收尾 → 不要发任何消息、\
            不要找事做，直接结束本轮即可（runtime 会回到 idle，\
            等下一个外部事件再唤醒你）。\n\
         \n\
         本提醒仅发送一次。",
    )
}

/// Construct a `ProviderConfig` with provider-specific env defaults.
///
/// For the `hermes` provider, injects `HERMES_HOME=<profile_dir>` so the agent
/// runs against its isolated profile (`~/.hermes/profiles/gitim-<handler>`).
/// An explicit `HERMES_HOME` already in `extra_env` wins — callers can override
/// the default via `me.json.env`.
fn build_provider_config(
    provider_type: &str,
    handler: &str,
    extra_env: HashMap<String, String>,
) -> Result<ProviderConfig, RuntimeError> {
    let mut env = extra_env;
    if provider_type == "hermes" && !env.contains_key("HERMES_HOME") {
        let path = hermes_profile::profile_dir(handler)
            .map_err(|e| RuntimeError::ProviderFailed(e.to_string()))?;
        env.insert("HERMES_HOME".to_string(), path.display().to_string());
    }
    Ok(ProviderConfig {
        executable_path: None,
        env,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::RuntimeState;

    #[test]
    fn build_provider_config_for_hermes_injects_home() {
        let cfg = build_provider_config("hermes", "alice", HashMap::new()).unwrap();
        let expected = hermes_profile::profile_dir("alice")
            .unwrap()
            .display()
            .to_string();
        assert_eq!(cfg.env.get("HERMES_HOME"), Some(&expected));
    }

    #[test]
    fn build_provider_config_for_claude_does_not_inject_home() {
        let cfg = build_provider_config("claude", "alice", HashMap::new()).unwrap();
        assert!(!cfg.env.contains_key("HERMES_HOME"));
    }

    #[test]
    fn with_provider_for_hermes_constructs_successfully() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let loop_ = AgentLoop::with_provider(tmp.path(), "hermes", "alice")
            .expect("hermes AgentLoop should construct without spawning hermes");
        assert_eq!(loop_.handler, "alice");
        assert_eq!(loop_.provider_type, "hermes");
    }

    #[test]
    fn post_reset_preamble_names_memory_and_idle_exit() {
        // The agent needs three pieces of information to do the right thing
        // after a synthetic cold-start kick: (1) it just reset and is cold,
        // (2) memory files are the place to look for unfinished work,
        // (3) doing nothing is a valid outcome — silence > spammy "how can
        // I help" turns when memory is empty. Each maps to one assertion;
        // wording can drift but the orienting facts must remain.
        let p = build_post_reset_preamble();
        assert!(
            p.contains("冷启动") || p.contains("handoff"),
            "preamble must name the cold-start fact: {p}"
        );
        assert!(
            p.contains("记忆文件"),
            "preamble must point at the memory file as the handoff medium: {p}"
        );
        assert!(
            p.contains("不要发") || p.contains("idle"),
            "preamble must give an explicit do-nothing exit so empty memory \
             doesn't spam channels: {p}"
        );
    }

    #[test]
    fn post_reset_preamble_is_distinct_from_usage_notice() {
        // Defensive: if these two ever converge to the same string we lose
        // the "post-reset vs over-budget" distinction in logs and tests,
        // and we'd silently double-send the same content if both flags
        // fired in the same cycle. Keep them visibly different.
        let usage = build_usage_notice_preamble(82.0);
        let post_reset = build_post_reset_preamble();
        assert_ne!(usage, post_reset);
        assert!(
            !post_reset.contains("[[RESET]]"),
            "post-reset preamble is sent AFTER reset — it must not re-instruct \
             the agent to emit [[RESET]] (that would tee up an immediate \
             second reset on an empty session)"
        );
    }

    #[test]
    fn build_provider_config_explicit_env_overrides_home() {
        let mut env = HashMap::new();
        env.insert("HERMES_HOME".to_string(), "/custom/path".to_string());
        let cfg = build_provider_config("hermes", "alice", env).unwrap();
        assert_eq!(
            cfg.env.get("HERMES_HOME").map(|s| s.as_str()),
            Some("/custom/path"),
        );
    }

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

    #[test]
    fn timeout_status_is_provider_failure() {
        assert!(is_provider_failure_status(&ExecStatus::Timeout));
        assert!(is_provider_failure_status(&ExecStatus::Failed));
        assert!(!is_provider_failure_status(&ExecStatus::Completed));
        assert!(!is_provider_failure_status(&ExecStatus::Aborted));
    }

    #[test]
    fn daemon_not_running_poll_error_is_recoverable() {
        assert!(is_daemon_not_running_poll_error(&RuntimeError::PollFailed(
            "daemon not running".to_string()
        )));
        assert!(!is_daemon_not_running_poll_error(
            &RuntimeError::PollFailed("poll response missing data".to_string())
        ));
        assert!(!is_daemon_not_running_poll_error(
            &RuntimeError::ProviderFailed("daemon not running".to_string())
        ));
    }

    /// Harness with a single agent at 89% `session_usage`. Returns the
    /// loop, shared state, broadcast receiver on the workspace activity
    /// channel, and the workspace tempdir.
    fn harness_with_usage_snapshot(
        handler: &str,
        slug: &str,
    ) -> (
        AgentLoop,
        SharedRuntimeState,
        tokio::sync::broadcast::Receiver<AgentActivityEvent>,
        tempfile::TempDir,
    ) {
        use crate::state::{SessionUsageSnapshot, UsageSource};
        use crate::workspace::WorkspaceContext;
        use std::sync::{Arc, Mutex};

        let tmp = tempfile::TempDir::new().expect("tempdir");
        let gitim_dir = tmp.path().join(".gitim");
        std::fs::create_dir_all(&gitim_dir).unwrap();
        std::fs::write(
            gitim_dir.join("me.json"),
            format!("{{\"handler\":\"{handler}\"}}"),
        )
        .unwrap();

        let mut loop_ = AgentLoop::with_provider(tmp.path(), "mock", handler).expect("build loop");

        let mut ctx =
            WorkspaceContext::new(slug.to_string(), slug.to_string(), tmp.path().to_path_buf());
        let rx = ctx.activity_tx.subscribe();
        let activity_tx = ctx.activity_tx.clone();

        ctx.agents.insert(
            handler.to_string(),
            crate::http::AgentInfo {
                id: handler.to_string(),
                handler: handler.to_string(),
                display_name: handler.to_string(),
                status: "running".to_string(),
                last_activity: None,
                messages_processed: 0,
                repo_path: tmp.path().display().to_string(),
                provider: Some("mock".to_string()),
                model: None,
                system_prompt: None,
                introduction: None,
                env: Default::default(),
                error_message: None,
                session_usage: Some(SessionUsageSnapshot {
                    session_id: "sid-pre-reset".to_string(),
                    input_tokens: Some(7),
                    output_tokens: Some(100),
                    max_tokens: Some(200_000),
                    used_percent: 89.0,
                    source: UsageSource::ProviderReported,
                    updated_at: "2026-04-21T07:34:02Z".to_string(),
                }),
                llm_provider: None,
                llm_model: None,
                usage_summary: None,
                loop_handle: None,
            },
        );

        let state = Arc::new(Mutex::new(RuntimeState::default()));
        state
            .lock()
            .unwrap()
            .workspaces
            .insert(slug.to_string(), ctx);

        loop_.set_runtime_state(state.clone());
        loop_.set_activity_tx_with_workspace(activity_tx, slug.to_string());

        (loop_, state, rx, tmp)
    }

    #[test]
    fn clear_runtime_session_usage_drops_hud_snapshot() {
        // Guards against the regression where the WebUI HUD kept displaying
        // the pre-reset percentage after [[RESET]] — the in-memory mirror
        // was never cleared to match the on-disk clear_session().
        let (loop_, state, _rx, _tmp) = harness_with_usage_snapshot("framer-opus", "gitim-company");

        // Sanity: pre-condition — HUD snapshot is installed at 89%.
        {
            let s = state.lock().unwrap();
            let info = s.workspaces["gitim-company"]
                .agents
                .get("framer-opus")
                .expect("agent present");
            assert_eq!(
                info.session_usage.as_ref().unwrap().used_percent,
                89.0,
                "precondition: agent should have 89% snapshot"
            );
        }

        loop_.clear_runtime_session_usage();

        let s = state.lock().unwrap();
        let info = s.workspaces["gitim-company"]
            .agents
            .get("framer-opus")
            .expect("agent still present");
        assert!(
            info.session_usage.is_none(),
            "clear must drop the in-memory snapshot so HUD stops showing stale percent"
        );
    }

    #[test]
    fn live_session_usage_updates_hud_without_persisting_accounting_state() {
        let (loop_, state, mut rx, tmp) =
            harness_with_usage_snapshot("framer-opus", "gitim-company");
        let usage = ProviderUsage {
            context_tokens: Some(5_000),
            context_window_tokens: Some(10_000),
            ..Default::default()
        };

        loop_.update_live_session_usage("sid-live", &usage);

        {
            let s = state.lock().unwrap();
            let info = s.workspaces["gitim-company"]
                .agents
                .get("framer-opus")
                .expect("agent present");
            let snap = info.session_usage.as_ref().expect("live snapshot");
            assert_eq!(snap.session_id, "sid-live");
            assert_eq!(snap.max_tokens, Some(10_000));
            assert!(
                (snap.used_percent - 50.0).abs() < 0.01,
                "live usage percent: {}",
                snap.used_percent
            );
        }

        let ev = rx.try_recv().expect("live usage event must be emitted");
        assert_eq!(ev.event_type, "usage");
        let payload: serde_json::Value = serde_json::from_str(&ev.detail).expect("usage json");
        assert_eq!(payload["session_id"], "sid-live");
        assert_eq!(payload["max_tokens"], 10_000);
        assert!(
            payload.get("usage_summary").is_none(),
            "live HUD refresh must not fabricate cumulative usage summary"
        );
        assert!(
            !AgentState::state_path(tmp.path()).exists(),
            "live HUD refresh must not persist agent-state; final turn accounting owns disk writes"
        );
    }

    #[test]
    fn format_changes_renders_cron_thread_with_cron_scope() {
        // Regression guard: the runtime side must render `kind:
        // "cron_thread"` ChannelChanges with a `[CRON <name>]` scope tag,
        // matching the daemon's poll branch that emits
        // `channel: "cron:<name>"` + `kind: "cron_thread"`. Without this,
        // a cron fire would either fall through the default `_ =>` arm
        // (rendering as `[#cron:<name>]` — confusing) or the agent's
        // prompt template wouldn't recognize the scope.
        let change = ChannelChange {
            channel: "cron:weekly".to_string(),
            kind: "cron_thread".to_string(),
            entries: vec![serde_json::json!({
                "author": "system",
                "body": "cron(weekly): scan logs",
                "timestamp": "20260102T090000Z",
                "line_number": 1u64,
                "point_to": 0u64,
            })],
        };
        let out = format_changes_as_prompt(&[change], "alice").expect("renders");
        assert!(
            out.contains("[CRON weekly]"),
            "expected [CRON weekly] scope in prompt; got:\n{}",
            out
        );
        // Author tag goes through unchanged — body still attributed to system.
        assert!(out.contains("@system"), "got:\n{}", out);
        // No mention tag — cron is structural, not a mention, even if the
        // body happened to contain @<self>.
        assert!(!out.contains("[MENTION]"), "got:\n{}", out);
    }

    #[test]
    fn clear_runtime_session_usage_broadcasts_empty_usage_event() {
        // Reactive SSE clients cache the last `usage` event's payload. An
        // empty payload tells them "drop the cached snapshot" so the HUD
        // number disappears without requiring a poll of GET /agents/:id.
        let (loop_, _state, mut rx, _tmp) =
            harness_with_usage_snapshot("framer-opus", "gitim-company");

        loop_.clear_runtime_session_usage();

        let ev = rx.try_recv().expect("activity event must be emitted");
        assert_eq!(ev.event_type, "usage");
        assert_eq!(ev.detail, "", "empty detail signals 'drop cached snapshot'");
        assert_eq!(ev.workspace_id, "gitim-company");
        assert_eq!(ev.agent_id, "framer-opus");
    }
}
