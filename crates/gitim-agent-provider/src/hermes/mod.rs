use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use tokio_util::sync::CancellationToken;

use crate::acp::{AcpClient, AcpHooks};
use crate::acp::parse::detect_api_failure;
use crate::{
    Event, ExecOptions, ExecResult, ExecStatus, PromptContext, Provider, ProviderConfig,
    ProviderError, ProviderUsage, Session,
};

pub mod prompts;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const HANDSHAKE_TIMEOUT_MAX: Duration = Duration::from_secs(30);
const EVENT_CHANNEL_BUFFER: usize = 256;

pub struct HermesProvider {
    config: ProviderConfig,
}

impl HermesProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Provider for HermesProvider {
    /// Hermes compresses in-loop at `compression.threshold` (default 50%).
    /// Opt out of the runtime's occupancy gauge + `[[RESET]]` preamble — see
    /// `Provider::self_managed_context` for the full reasoning.
    fn self_managed_context(&self) -> bool {
        true
    }

    /// Hermes' ACP `result.usage` is session-cumulative.
    /// `normalize_to_delta` uses this flag to subtract a per-session
    /// baseline so the accumulator gets real per-turn deltas;
    /// `self_managed_context` short-circuits `compute_snapshot` so the
    /// cumulative numbers never feed the HUD occupancy gauge.
    fn usage_is_cumulative(&self) -> bool {
        true
    }

    /// Drop sections that assume the Claude-style `AGENTS.md` + `notes/`
    /// filesystem-memory model. Hermes manages identity / memory through
    /// SOUL.md + MEMORY.md / USER.md inside the profile directory, and
    /// re-loads them after every in-loop compression, so re-injecting any
    /// of this from the runtime side would be noise (or worse, conflict
    /// with hermes' own guidance).
    fn prompt_memory(&self, _ctx: &PromptContext) -> String {
        // hermes injects its own MEMORY_GUIDANCE when the memory tool is
        // loaded — see run_agent.py::_build_system_prompt.
        String::new()
    }
    fn prompt_reset_protocol(&self, _ctx: &PromptContext) -> String {
        // hermes self-compresses; no `[[RESET]]` sentinel.
        String::new()
    }
    fn prompt_cold_start(&self, _ctx: &PromptContext) -> String {
        // No bootstrap of AGENTS.md / notes/ for hermes — memory files live
        // inside the hermes profile and are seeded by `hermes_profile`.
        String::new()
    }

    /// Hermes-flavored identity (drops the `AGENTS.md` / `notes/` carve-out).
    fn prompt_identity(&self, ctx: &PromptContext) -> String {
        prompts::identity(ctx)
    }
    /// Hermes-flavored collaboration norms (drops the
    /// "用你的记忆 / `notes/` 跟踪每条线" filesystem-memory suggestion).
    fn prompt_collaboration(&self, ctx: &PromptContext) -> String {
        prompts::collaboration(ctx)
    }
    /// Hermes-flavored gitim CLI guidance (neutralises the AGENTS.md
    /// continuity-sink line in the board / memory contrast).
    fn prompt_gitim_api(&self, ctx: &PromptContext) -> String {
        prompts::gitim_api(ctx)
    }

    async fn execute(&self, prompt: &str, opts: ExecOptions) -> Result<Session, ProviderError> {
        let exec_path = self
            .config
            .executable_path
            .clone()
            .unwrap_or_else(|| "hermes".to_string());

        crate::util::which(&exec_path).map_err(|_| ProviderError::ExecutableNotFound {
            path: exec_path.clone(),
        })?;

        let timeout = opts.timeout.unwrap_or(DEFAULT_TIMEOUT);

        let mut cmd = Command::new(&exec_path);
        cmd.arg("acp")
            .stdout(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .env("HERMES_YOLO_MODE", "1");

        if let Some(cwd) = &opts.cwd {
            cmd.current_dir(cwd);
        }
        for (k, v) in &self.config.env {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn()?;
        let pid = child.id().unwrap_or(0);
        info!(pid, cwd = ?opts.cwd, "hermes started");

        let stdout = child.stdout.take().expect("stdout piped");
        let stdin = child.stdin.take().expect("stdin piped");
        let stderr = child.stderr.take().expect("stderr piped");

        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_BUFFER);
        let (result_tx, result_rx) = oneshot::channel();

        let cancel_token = CancellationToken::new();
        let cancel_token_inner = cancel_token.clone();

        // `opts.system_prompt` deliberately unused — SOUL.md is the channel.
        let _ = &opts.system_prompt;
        let prompt = build_prompt_payload(prompt);
        let resume_token = opts.resume_token.clone();
        let cwd_str = opts
            .cwd
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string());

        let hooks = AcpHooks {
            tool_name_mapper: hermes_tool_name_from_title,
            accept_notification: None,
            // ExecResult.usage is owned by the prompt-response value;
            // mid-stream usage_update notifications are intentionally
            // dropped (display-only) so the runtime token accumulator
            // never sees them as live events either.
            emit_live_usage: false,
        };
        let acp = Arc::new(AcpClient::new("hermes", stdin, hooks));

        let join_handle = tokio::spawn(async move {
            drive_session(
                child,
                stdout,
                stderr,
                acp,
                event_tx,
                result_tx,
                timeout,
                pid,
                cancel_token_inner,
                prompt,
                resume_token,
                cwd_str,
            )
            .await;
        });

        Ok(Session::new(
            event_rx,
            result_rx,
            join_handle.abort_handle(),
            cancel_token,
        ))
    }
}

/// Build the text sent to `session/prompt`.
///
/// Hermes ACP does not expose a per-request system prompt parameter. Earlier
/// versions of this provider prepended the runtime's system prompt to the
/// first user message, but that put GitIM identity / protocol rules into
/// hermes' conversation history — where its in-loop compressor was free to
/// summarise them away.
///
/// The system prompt now lives in `~/.hermes/profiles/gitim-<handler>/SOUL.md`
/// (managed by `gitim_runtime::hermes_profile`), which hermes auto-loads into
/// its frozen system-prompt slot at every session start and rebuilds after
/// each compression event. So the ACP user payload is just the user text —
/// `opts.system_prompt` is intentionally ignored here.
pub fn build_prompt_payload(prompt: &str) -> String {
    prompt.to_string()
}

/// Map a hermes-emitted ACP tool title onto the runtime-expected name.
///
/// Hermes already emits tool titles whose prefix is the canonical name
/// (e.g. `"terminal: ls -la"` → `"terminal"`, `"file_edit: path"` →
/// `"file_edit"`). `parse_notification` strips everything after the
/// first `:` and trims, so this mapper just passes the prefix through.
/// The hook exists so kimi can plug in its capitalised-title normalizer
/// (e.g. `"Read file"` → `"read_file"`) without changing hermes.
pub fn hermes_tool_name_from_title(name: &str) -> String {
    name.to_string()
}

// ── Driver task ────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn drive_session(
    mut child: tokio::process::Child,
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    acp: Arc<AcpClient>,
    event_tx: mpsc::Sender<Event>,
    result_tx: oneshot::Sender<ExecResult>,
    timeout: Duration,
    pid: u32,
    cancel_token: CancellationToken,
    prompt: String,
    resume_token: Option<String>,
    cwd_str: String,
) {
    let start = Instant::now();
    let mut session_id = String::new();
    let mut final_status = ExecStatus::Completed;
    let mut final_error: Option<String> = None;
    let mut latest_usage: Option<ProviderUsage> = None;

    // Collect stderr tail for error reporting
    let stderr_tail: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let stderr_tail_clone = stderr_tail.clone();
    let stderr_handle = tokio::spawn(async move {
        const TAIL_LINES: usize = 20;
        let mut r = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = r.next_line().await {
            debug!(target: "hermes:stderr", "{}", line);
            let mut tail = stderr_tail_clone.lock().unwrap();
            tail.push(line);
            if tail.len() > TAIL_LINES {
                tail.remove(0);
            }
        }
    });

    // Reader task — pumps stdout through AcpClient::handle_line so JSON-RPC
    // responses land on their pending oneshots and session/update
    // notifications turn into Event::* on event_tx.
    let reader_acp = Arc::clone(&acp);
    let reader_event_tx = event_tx.clone();
    let reader_handle = tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        loop {
            match reader.next_line().await {
                Ok(Some(line)) => {
                    reader_acp.handle_line(&line, &reader_event_tx).await;
                }
                Ok(None) => break,
                Err(e) => {
                    warn!(error = %e, "hermes stdout read error");
                    break;
                }
            }
        }
    });

    // ── Handshake (initialize → authenticate → new_session/resume) ──
    // Carries its own 30s ceiling, separate from the outer execute() timeout.

    let handshake_timeout = timeout.min(HANDSHAKE_TIMEOUT_MAX);
    let handshake_acp = Arc::clone(&acp);
    let resume_clone = resume_token.clone();
    let cwd_clone = cwd_str.clone();
    let handshake = async move {
        let init = handshake_acp.initialize().await?;
        handshake_acp.authenticate_first_method(&init).await?;
        let sid = if let Some(token) = resume_clone {
            let (actual, _changed) = handshake_acp.resume_session(&cwd_clone, &token).await?;
            actual
        } else {
            handshake_acp.new_session(&cwd_clone).await?
        };
        Ok::<String, ProviderError>(sid)
    };

    match tokio::time::timeout(handshake_timeout, handshake).await {
        Ok(Ok(sid)) => {
            session_id = sid;
            info!(pid, session_id = %session_id, "hermes session established");
        }
        Ok(Err(e)) => {
            warn!(pid, error = %e, "hermes handshake failed");
            let _ = child.start_kill();
            send_result(
                result_tx,
                ExecStatus::Failed,
                String::new(),
                Some(provider_error_message(&e)),
                start,
                &session_id,
                None,
            );
            stderr_handle.abort();
            reader_handle.abort();
            return;
        }
        Err(_) => {
            warn!(pid, "hermes handshake timed out after {handshake_timeout:?}");
            let _ = child.start_kill();
            send_result(
                result_tx,
                ExecStatus::Timeout,
                String::new(),
                Some(format!(
                    "hermes handshake timed out after {handshake_timeout:?}"
                )),
                start,
                &session_id,
                None,
            );
            stderr_handle.abort();
            reader_handle.abort();
            return;
        }
    }

    try_send_event(
        &event_tx,
        Event::Status {
            status: "running".to_string(),
        },
    );

    // ── Prompt + outer timeout + cancel race ──

    let prompt_acp = Arc::clone(&acp);
    let prompt_sid = session_id.clone();
    let prompt_text = prompt.clone();

    let prompt_outcome = tokio::time::timeout(timeout, async {
        tokio::select! {
            r = prompt_acp.prompt(&prompt_sid, &prompt_text) => Some(r),
            _ = cancel_token.cancelled() => None,
        }
    })
    .await;

    match prompt_outcome {
        Ok(Some(Ok(outcome))) => {
            // Match historical behavior: any cleanly-arriving prompt
            // response is treated as Completed regardless of stopReason.
            // (Mid-stream cancellation flows through the cancel_token arm
            // below.)
            latest_usage = outcome.usage;
        }
        Ok(Some(Err(e))) => {
            final_status = ExecStatus::Failed;
            final_error = Some(provider_error_message(&e));
        }
        Ok(None) => {
            info!(pid, "cancelled by steering");
            final_status = ExecStatus::Aborted;
            final_error = Some("cancelled by steering".to_string());
        }
        Err(_) => {
            final_status = ExecStatus::Timeout;
            final_error = Some(format!("hermes timed out after {timeout:?}"));
        }
    }

    // ── Post-loop cleanup ──

    // Signal end-of-input to hermes so it shuts the session cleanly; for
    // timeout / abort we kill outright instead.
    if final_status == ExecStatus::Timeout || final_status == ExecStatus::Aborted {
        let _ = child.start_kill();
    } else {
        acp.close_stdin().await;
    }

    // The reader keeps running until the child closes stdout (or is
    // killed). Wait for it to drain so any trailing notifications that
    // arrived after the prompt response — usually none for hermes, but
    // possible — get processed into the text accumulator before we
    // sample the final output.
    let _ = reader_handle.await;

    let mut output = acp.collected_output().await;

    if final_status == ExecStatus::Completed {
        if let Some(api_err) = detect_api_failure(&output) {
            warn!(pid, error = %api_err, "hermes returned API failure as text");
            final_status = ExecStatus::Failed;
            final_error = Some(api_err);
        }
    }

    if final_status != ExecStatus::Timeout {
        match child.wait().await {
            Ok(status) if !status.success() && final_status == ExecStatus::Completed => {
                final_status = ExecStatus::Failed;
                final_error = Some(format!("hermes exited with status: {status}"));
            }
            Err(e) if final_status == ExecStatus::Completed => {
                final_status = ExecStatus::Failed;
                final_error = Some(format!("failed to wait for hermes: {e}"));
            }
            _ => {}
        }
    }

    // Final output drain — covers the case where additional text content
    // arrived between the previous sample and child exit.
    output = acp.collected_output().await;

    let duration = start.elapsed();
    info!(pid, ?final_status, ?duration, "hermes finished");

    stderr_handle.abort();

    // If failed with no error message, fall back to stderr tail
    if final_status == ExecStatus::Failed && final_error.as_ref().is_none_or(|e| e.is_empty()) {
        let tail = stderr_tail.lock().unwrap();
        if !tail.is_empty() {
            final_error = Some(format!("(stderr) {}", tail.join("\n")));
        }
    }

    let _ = result_tx.send(ExecResult {
        status: final_status,
        output,
        error: final_error,
        duration_ms: duration.as_millis() as u64,
        session_token: if session_id.is_empty() {
            None
        } else {
            Some(session_id)
        },
        usage: latest_usage,
    });
}

/// Helper to send ExecResult on the result oneshot.
#[allow(clippy::too_many_arguments)]
fn send_result(
    result_tx: oneshot::Sender<ExecResult>,
    status: ExecStatus,
    output: String,
    error: Option<String>,
    start: Instant,
    session_id: &str,
    usage: Option<ProviderUsage>,
) {
    let _ = result_tx.send(ExecResult {
        status,
        output,
        error,
        duration_ms: start.elapsed().as_millis() as u64,
        session_token: if session_id.is_empty() {
            None
        } else {
            Some(session_id.to_string())
        },
        usage,
    });
}

fn try_send_event(tx: &mpsc::Sender<Event>, event: Event) {
    if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) = tx.try_send(event) {
        warn!("event channel full, dropping event");
    }
}

/// Extract the human-readable inner message from a [`ProviderError`],
/// matching the un-prefixed shape the previous inline rpc_call produced
/// in `ExecResult.error`. The `thiserror`-generated `Display` impl
/// prepends `"protocol error: "` which would be redundant noise here.
fn provider_error_message(e: &ProviderError) -> String {
    match e {
        ProviderError::Protocol(msg) => msg.clone(),
        other => other.to_string(),
    }
}
