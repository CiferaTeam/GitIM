//! Kimi Code CLI provider — ACP transport via shared `acp::AcpClient`.
//!
//! Kimi Code CLI (<https://www.kimi.com/code>) speaks the same ACP
//! JSON-RPC protocol as hermes via `kimi acp`. This provider is hermes
//! with a few swapped knobs:
//!
//! 1. The spawn target is `kimi acp` (no `--afk`; Kimi Code CLI >= 0.14
//!    exposes ACP through the `acp` subcommand directly). Permission
//!    requests that arrive mid-session are auto-approved by
//!    `AcpClient::handle_agent_request`.
//! 2. When `ExecOptions::model` is non-empty the driver calls
//!    `session/set_model` after `session/new` / `session/resume` and
//!    before the first `session/prompt`. Failure fails the task — we
//!    do **not** silently fall back to whatever default kimi picked,
//!    because the user expects their model selection to be honoured.
//! 3. [`kimi_tool_name_from_title`] normalises kimi's capitalised tool
//!    titles (e.g. `"Read file: …"`, `"Run command: …"`) into the
//!    snake_case identifiers the runtime/UI expects.
//!
//! Spec: `docs/plans/kimi-cursor-providers/00-requirements.md` §"Kimi 设计"
//! Reference: `multica/server/pkg/agent/kimi.go` (Go).
//!
//! Provider trait flags (and why they differ from `Provider`'s defaults):
//! - `reports_usage()` stays `true` — Kimi Code CLI ACP prompt responses
//!   omit standard usage, so the provider derives a local estimate from
//!   the CLI's local session files when available. The home directory is
//!   discovered from `KIMI_HOME` / `KIMI_CODE_HOME` env vars, then
//!   `~/.kimi-code` (new Kimi Code CLI) and finally `~/.kimi` (legacy
//!   Kimi CLI). The estimate is recorded as cache-read style input plus
//!   context-window occupancy.
//! - `self_managed_context()` stays `false` (default written explicitly
//!   here) — unlike hermes there is no in-loop compression, so the runtime
//!   owns the `[[RESET]]` channel + `[系统通知]` occupancy preamble, same
//!   as claude/codex.
//! - All `prompt_*` defaults inherit unchanged — there is no SOUL.md /
//!   MEMORY.md self-managed memory model on kimi.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::acp::{try_send_event, AcpClient, AcpHooks};
use crate::{
    preconditions, Event, ExecOptions, ExecResult, ExecStatus, Provider, ProviderConfig,
    ProviderError, ProviderUsage, ProviderUsageReport, Session,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const HANDSHAKE_TIMEOUT_MAX: Duration = Duration::from_secs(30);
const CLEAN_SHUTDOWN_GRACE: Duration = Duration::from_millis(1_500);
const EVENT_CHANNEL_BUFFER: usize = 256;
const KIMI_CONTEXT_WINDOW_TOKENS: u64 = 200_000;
const HOST_TURN_COMPLETION_NOTE: &str = "\
Host turn completion note: after completing the requested tool or GitIM action, \
finish this ACP turn by replying exactly `done` in this provider response. \
Do not send that `done` message to GitIM unless the user explicitly asked for it.";

pub struct KimiProvider {
    config: ProviderConfig,
}

impl KimiProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Provider for KimiProvider {
    /// Kimi's ACP stream omits standard usage. The provider reads Kimi's
    /// local session context counter after each completed prompt and records
    /// that window-sized input estimate as cache-read style usage.
    fn reports_usage(&self) -> bool {
        true
    }

    /// Kimi has no in-loop compression like hermes. Runtime owns the
    /// `[[RESET]]` channel and `[系统通知]` occupancy preamble, same as
    /// claude / codex / cursor. Default is `false`; restated explicitly
    /// here so the reasoning lives next to the trait flag.
    fn self_managed_context(&self) -> bool {
        false
    }

    async fn execute(&self, prompt: &str, opts: ExecOptions) -> Result<Session, ProviderError> {
        let exec_path = self
            .config
            .executable_path
            .clone()
            .unwrap_or_else(|| "kimi".to_string());

        crate::util::which(&exec_path).map_err(|_| ProviderError::ExecutableNotFound {
            path: exec_path.clone(),
        })?;

        let timeout = opts.timeout.unwrap_or(DEFAULT_TIMEOUT);

        let mut cmd = Command::new(&exec_path);
        // Kimi Code CLI (>= 0.14) exposes the ACP server through the `acp`
        // subcommand directly. The legacy `--afk` root flag is no longer
        // accepted, so we omit it. Permission requests that arrive mid-session
        // are auto-approved by `AcpClient::handle_agent_request`.
        cmd.arg("acp")
            .stdout(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        if let Some(cwd) = &opts.cwd {
            cmd.current_dir(cwd);
        }
        for (k, v) in &self.config.env {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn()?;
        let pid = child.id().unwrap_or(0);
        info!(pid, cwd = ?opts.cwd, model = ?opts.model, "kimi started");

        let stdout = preconditions::take_tokio_piped_stdout(&mut child);
        let stdin = preconditions::take_tokio_piped_stdin(&mut child);
        let stderr = preconditions::take_tokio_piped_stderr(&mut child);

        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_BUFFER);
        let (result_tx, result_rx) = oneshot::channel();

        let cancel_token = CancellationToken::new();
        let cancel_token_inner = cancel_token.clone();

        // Kimi prepends an optional system prompt to the user payload —
        // ACP `session/prompt` has no separate system-prompt slot, so we
        // concatenate with a `\n\n---\n\n` separator. Matches multica's
        // kimi.go behaviour exactly. (Hermes loads its system prompt from
        // SOUL.md inside the profile dir instead, which is why hermes'
        // build_prompt_payload ignores opts.system_prompt — that path is
        // not appropriate for kimi.)
        let base_user_text = match opts.system_prompt.as_deref() {
            Some(sp) if !sp.is_empty() => format!("{sp}\n\n---\n\n{prompt}"),
            _ => prompt.to_string(),
        };
        let user_text = format!("{base_user_text}\n\n---\n\n{HOST_TURN_COMPLETION_NOTE}");
        let resume_token = opts.resume_token.clone();
        let model = opts.model.clone();
        let kimi_home = kimi_home_from_env(&self.config.env);
        let cwd_str = opts
            .cwd
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string());

        let hooks = AcpHooks {
            tool_name_mapper: kimi_tool_name_from_title,
            accept_notification: None,
            // Kimi Code 1.44 does not emit ACP usage updates. Keep this off
            // so a future protocol change does not accidentally create a
            // live-only usage path while the provider still declares
            // `reports_usage() == false`.
            emit_live_usage: false,
        };
        let acp = Arc::new(AcpClient::new("kimi", stdin, hooks));

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
                user_text,
                resume_token,
                model,
                kimi_home,
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

/// Map an ACP tool title emitted by Kimi's CLI into the snake_case
/// identifier the runtime/UI expects.
///
/// Kimi follows the ACP spec where `title` is a short human-readable
/// label such as `"Read file: /path/to/foo.go"` or `"Run command: ls"`.
/// Hermes' lowercase convention (`"read:"`, `"patch (replace)"`) is
/// handled upstream by `hermes_tool_name_from_title`, but kimi's
/// capitalised format slips through — so this hook re-normalises
/// after `parse_notification` has already stripped everything after
/// the first `:`. The fallback is `lower-cased + spaces→underscores`
/// so unknown titles still produce stable snake_case identifiers.
///
/// Reference: multica/server/pkg/agent/kimi.go:358-403.
pub(crate) fn kimi_tool_name_from_title(title: &str) -> String {
    let t = title.trim();
    if t.is_empty() {
        return String::new();
    }
    // Belt-and-braces: even though `parse_notification` strips after
    // the first `:`, the mapper is also exposed for direct unit tests
    // (and could be called by future ACP servers that bypass that
    // path), so handle the colon here too.
    let prefix = match t.find(':') {
        Some(i) => t[..i].trim(),
        None => t,
    };
    let lower = prefix.to_lowercase();
    match lower.as_str() {
        "read" | "read file" => "read_file".to_string(),
        "write" | "write file" => "write_file".to_string(),
        "edit" | "patch" => "edit_file".to_string(),
        "shell" | "bash" | "terminal" | "run command" | "run shell command" => {
            "terminal".to_string()
        }
        "search" | "grep" | "find" => "search_files".to_string(),
        "glob" => "glob".to_string(),
        "web search" => "web_search".to_string(),
        "fetch" | "web fetch" => "web_fetch".to_string(),
        "todo" | "todo write" => "todo_write".to_string(),
        // Fallback: snake_case the title so the UI gets a stable
        // identifier. Matches multica's behaviour
        // (`strings.ReplaceAll(lower, " ", "_")`).
        _ => lower.replace(' ', "_"),
    }
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
    user_text: String,
    resume_token: Option<String>,
    model: Option<String>,
    kimi_home: Option<PathBuf>,
    cwd_str: String,
) {
    let start = Instant::now();
    let mut session_id = String::new();
    let mut final_status = ExecStatus::Completed;
    let mut final_error: Option<String> = None;
    let mut latest_usage: Option<ProviderUsage> = None;

    // Collect stderr tail for error reporting.
    let stderr_tail: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let stderr_tail_clone = stderr_tail.clone();
    let stderr_handle = tokio::spawn(async move {
        const TAIL_LINES: usize = 20;
        let mut r = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = r.next_line().await {
            debug!(target: "kimi:stderr", "{}", line);
            let mut tail = preconditions::mutex_lock_arc(&stderr_tail_clone);
            tail.push(line);
            if tail.len() > TAIL_LINES {
                tail.remove(0);
            }
        }
    });

    // Reader task — pumps stdout through `AcpClient::handle_line` so
    // JSON-RPC responses land on their pending oneshots and
    // `session/update` notifications turn into `Event::*` on event_tx.
    // On stream exit we must `fail_pending` so an in-flight `request()`
    // unblocks immediately rather than waiting on the outer timeout
    // (see hermes/mod.rs for the same pattern + reasoning).
    let reader_acp = Arc::clone(&acp);
    let reader_event_tx = event_tx.clone();
    let mut reader_handle = tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        loop {
            match reader.next_line().await {
                Ok(Some(line)) => {
                    reader_acp.handle_line(&line, &reader_event_tx).await;
                }
                Ok(None) => break,
                Err(e) => {
                    warn!(error = %e, "kimi stdout read error");
                    break;
                }
            }
        }
        reader_acp.fail_pending().await;
    });

    // ── Handshake (initialize → new_session/resume → optional set_model) ──
    // 30s ceiling, separate from the outer execute() timeout. Note: unlike
    // hermes we do NOT call `authenticate_first_method` — kimi's ACP
    // server advertises no auth methods and multica's reference also
    // skips this step. If a future kimi version starts requiring it,
    // `AcpClient::authenticate_first_method` is a no-op when
    // `authMethods` is absent, so adding the call back is safe.

    let handshake_timeout = timeout.min(HANDSHAKE_TIMEOUT_MAX);
    let handshake_acp = Arc::clone(&acp);
    let resume_clone = resume_token.clone();
    let cwd_clone = cwd_str.clone();
    let model_clone = model.clone();
    // Handshake returns `Result<sid, (partial_sid, err)>` so the failure
    // arm can carry a session id that was already established before a
    // later step (specifically `set_session_model`) failed. Plan contract
    // (01-plan.md:1333): set_session_model failure must produce
    // `ExecResult { status: Failed, session_token: Some(sid), … }` so the
    // user can retry with a corrected model and resume the same
    // conversation. Earlier code used `?` short-circuit and dropped the
    // locally-bound sid, which silently broke that continuity guarantee.
    let handshake = async move {
        handshake_acp.initialize().await.map_err(|e| (None, e))?;
        let sid = if let Some(token) = resume_clone {
            let (actual, changed) = handshake_acp
                .resume_session(&cwd_clone, &token)
                .await
                .map_err(|e| (None, e))?;
            if changed {
                warn!(
                    backend = "kimi",
                    requested = %token,
                    actual = %actual,
                    "kimi agent returned a different session id on resume — original was likely lost; continuing with the new id"
                );
            }
            actual
        } else {
            handshake_acp
                .new_session(&cwd_clone)
                .await
                .map_err(|e| (None, e))?
        };
        // If the caller chose a model, switch the session to it before
        // any prompt. Failure here MUST fail the task — silently falling
        // back to kimi's default would let the user think their pick
        // was honoured while the task actually ran on something else.
        // (multica kimi.go:251-268 — same contract.) The Err arm carries
        // `Some(sid)` so the outer driver can stamp it onto
        // ExecResult.session_token.
        if let Some(m) = model_clone.as_deref().filter(|s| !s.is_empty()) {
            handshake_acp
                .set_session_model(&sid, m)
                .await
                .map_err(|e| {
                    (
                        Some(sid.clone()),
                        ProviderError::Protocol(format!(
                            "kimi could not switch to model {m:?}: {e}"
                        )),
                    )
                })?;
            info!(session_id = %sid, model = %m, "kimi session model set");
        }
        Ok::<String, (Option<String>, ProviderError)>(sid)
    };

    match tokio::time::timeout(handshake_timeout, handshake).await {
        Ok(Ok(sid)) => {
            session_id = sid;
            info!(pid, session_id = %session_id, "kimi session established");
        }
        Ok(Err((partial_sid, e))) => {
            warn!(pid, error = %e, "kimi handshake failed");
            // Preserve any partial session id — when set_session_model
            // fails the runtime needs the (now-established) sid back so
            // the user's next-turn `resume_token` can carry the same
            // session forward after the user corrects the model.
            if let Some(sid) = partial_sid {
                session_id = sid;
            }
            let _ = child.start_kill();
            send_result(
                result_tx,
                ExecStatus::Failed,
                String::new(),
                Some(e.to_string()),
                start,
                &session_id,
                None,
            );
            stderr_handle.abort();
            reader_handle.abort();
            return;
        }
        Err(_) => {
            warn!(pid, "kimi handshake timed out after {handshake_timeout:?}");
            let _ = child.start_kill();
            send_result(
                result_tx,
                ExecStatus::Timeout,
                String::new(),
                Some(format!(
                    "kimi handshake timed out after {handshake_timeout:?}"
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
    let prompt_text = user_text.clone();

    enum PromptWait {
        Finished(Result<crate::acp::PromptOutcome, ProviderError>),
        Cancelled,
        Timeout,
    }

    let prompt_future = prompt_acp.prompt(&prompt_sid, &prompt_text);
    tokio::pin!(prompt_future);
    let timeout_sleep = tokio::time::sleep(timeout);
    tokio::pin!(timeout_sleep);

    let prompt_outcome = tokio::select! {
        r = &mut prompt_future => PromptWait::Finished(r),
        _ = cancel_token.cancelled() => PromptWait::Cancelled,
        _ = &mut timeout_sleep => PromptWait::Timeout,
    };

    match prompt_outcome {
        PromptWait::Finished(Ok(outcome)) => {
            info!(
                pid,
                stop_reason = %outcome.stop_reason,
                "kimi prompt response received"
            );
            // Kimi reports `stopReason: "cancelled"` when the agent
            // itself aborted the prompt (e.g. user interrupted via
            // ACP's cancel channel). Surface that distinctly so the
            // runtime can record the right ExecStatus — hermes
            // intentionally doesn't check this (it has its own
            // cancel flow), but for kimi this is the only place the
            // signal arrives.
            if outcome.stop_reason == "cancelled" {
                final_status = ExecStatus::Aborted;
                final_error = Some("kimi cancelled the prompt".to_string());
            }
            latest_usage = outcome.usage;
        }
        PromptWait::Finished(Err(e)) => {
            final_status = ExecStatus::Failed;
            final_error = Some(e.to_string());
        }
        PromptWait::Cancelled => {
            info!(pid, "cancelled by steering");
            final_status = ExecStatus::Aborted;
            final_error = Some("cancelled by steering".to_string());
        }
        PromptWait::Timeout => {
            final_status = ExecStatus::Timeout;
            final_error = Some(format!("kimi timed out after {timeout:?}"));
        }
    }

    // ── Post-loop cleanup ──

    // Kimi's ACP server is a session server: after `session/prompt`
    // resolves it keeps the stdio process alive waiting for the next
    // request. GitIM launches one provider process per agent turn, so the
    // correct cleanup is to stop this child explicitly instead of waiting
    // for a natural EOF exit.
    //
    // Kimi Code CLI writes the turn's usage record to
    // `agents/main/wire.jsonl` asynchronously after the prompt response.
    // Killing instantly would truncate the file before the usage flush, so
    // we close stdin first and give the process a short grace window to
    // shut down cleanly. If it lingers, we force-kill.
    // Closing stdin is itself an intentional shutdown signal — we are done
    // with this turn. Mark it as such so kimi's subsequent exit code is not
    // allowed to flip a Completed turn to Failed (Kimi Code CLI may exit
    // non-zero on EOF; we only care that we got the response we asked for).
    let killed_for_shutdown = true;
    acp.close_stdin().await;
    if tokio::time::timeout(CLEAN_SHUTDOWN_GRACE, &mut reader_handle)
        .await
        .is_err()
    {
        debug!(
            pid,
            ?CLEAN_SHUTDOWN_GRACE,
            "kimi kept stdout open after stdin close; killing ACP server"
        );
        let _ = child.start_kill();
        if tokio::time::timeout(CLEAN_SHUTDOWN_GRACE, &mut reader_handle)
            .await
            .is_err()
        {
            reader_handle.abort();
        }
    }

    let output = acp.collected_output().await;

    // NB: hermes runs `detect_api_failure` here to promote
    // completed→failed when an upstream LLM HTTP error gets buried in
    // the assistant text stream. Kimi v1 deliberately does NOT call
    // it — we have not observed kimi swallowing HTTP errors into the
    // text stream the same way. If that pattern shows up in practice,
    // wire it back in (the helper lives in
    // `crate::acp::parse::detect_api_failure`).

    if final_status != ExecStatus::Timeout {
        match child.wait().await {
            Ok(status)
                if !status.success()
                    && final_status == ExecStatus::Completed
                    && !killed_for_shutdown =>
            {
                final_status = ExecStatus::Failed;
                final_error = Some(format!("kimi exited with status: {status}"));
            }
            Err(e) if final_status == ExecStatus::Completed => {
                final_status = ExecStatus::Failed;
                final_error = Some(format!("failed to wait for kimi: {e}"));
            }
            _ => {}
        }
    }

    if final_status == ExecStatus::Completed {
        if let Some(local_usage) = read_kimi_local_context_usage(kimi_home.as_deref(), &session_id)
        {
            attach_kimi_local_usage(&mut latest_usage, local_usage);
        }
    }

    let duration = start.elapsed();
    info!(pid, ?final_status, ?duration, "kimi finished");

    stderr_handle.abort();

    // If failed with no error message, fall back to stderr tail so the
    // user sees something actionable. Mirror hermes' pattern.
    if final_status == ExecStatus::Failed && final_error.as_ref().is_none_or(|e| e.is_empty()) {
        let tail = preconditions::mutex_lock_arc(&stderr_tail);
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
        usage_report: ProviderUsageReport::from_usage(latest_usage.clone()),
        usage: latest_usage,
    });
}

fn kimi_home_from_env(provider_env: &std::collections::HashMap<String, String>) -> Option<PathBuf> {
    // Explicit overrides take precedence, then new Kimi Code CLI defaults,
    // then the legacy Kimi CLI home for backward compatibility.
    for key in ["KIMI_HOME", "KIMI_CODE_HOME"] {
        if let Some(dir) = provider_env
            .get(key)
            .cloned()
            .or_else(|| std::env::var(key).ok())
            .filter(|d| !d.is_empty())
        {
            // Honor explicit overrides regardless of whether the directory
            // currently exists — the user asked for this path.
            return Some(PathBuf::from(dir));
        }
    }

    let home = std::env::var("HOME").ok()?;
    let new_default = PathBuf::from(&home).join(".kimi-code");
    let legacy_default = PathBuf::from(home).join(".kimi");

    // For implicit defaults, prefer whichever already exists.
    if new_default.exists() {
        return Some(new_default);
    }
    if legacy_default.exists() {
        return Some(legacy_default);
    }

    // Fall back to the new default even if it doesn't exist yet, so callers
    // can attempt to read it without silently switching to legacy paths.
    Some(new_default)
}

fn attach_kimi_local_usage(latest_usage: &mut Option<ProviderUsage>, local_usage: ProviderUsage) {
    let usage = latest_usage.get_or_insert_with(ProviderUsage::default);
    if usage.input_tokens.is_none() {
        usage.input_tokens = local_usage.input_tokens;
    }
    if usage.output_tokens.is_none() {
        usage.output_tokens = local_usage.output_tokens;
    }
    if usage.cache_read_tokens.is_none() {
        usage.cache_read_tokens = local_usage.cache_read_tokens;
    }
    if usage.cache_creation_tokens.is_none() {
        usage.cache_creation_tokens = local_usage.cache_creation_tokens;
    }
    usage.context_tokens = local_usage.context_tokens;
    usage.context_window_tokens = local_usage.context_window_tokens;
}

fn read_kimi_local_context_usage(
    kimi_home: Option<&Path>,
    session_id: &str,
) -> Option<ProviderUsage> {
    let kimi_home = kimi_home?;
    let session_dir = find_kimi_session_dir(&kimi_home.join("sessions"), session_id)?;

    // New Kimi Code CLI (>= 0.14) writes per-turn usage into
    // agents/main/wire.jsonl as `usage.record` events.
    let wire_path = session_dir.join("agents/main/wire.jsonl");
    if wire_path.is_file() {
        if let Ok(content) = std::fs::read_to_string(&wire_path) {
            if let Some(usage) = scan_kimi_code_wire_usage(&content) {
                return Some(usage);
            }
        }
    }

    // Legacy Kimi CLI stored usage in context*.jsonl files.
    let mut files = kimi_context_files(&session_dir);
    files.sort_by_key(|path| kimi_context_file_index(path).unwrap_or(0));
    for path in files.iter().rev() {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        if let Some(usage) = scan_kimi_context_usage(&content) {
            return Some(usage);
        }
    }
    None
}

fn find_kimi_session_dir(sessions_dir: &Path, session_id: &str) -> Option<PathBuf> {
    for workspace in read_subdirs(sessions_dir) {
        let candidate = workspace.join(session_id);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
}

fn read_subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    out.sort();
    out
}

fn kimi_context_files(session_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(session_dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| kimi_context_file_index(p).is_some())
        .collect()
}

fn kimi_context_file_index(path: &Path) -> Option<u32> {
    let name = path.file_name()?.to_str()?;
    if name == "context.jsonl" {
        return Some(0);
    }
    let rest = name.strip_prefix("context_")?.strip_suffix(".jsonl")?;
    rest.parse().ok()
}

fn scan_kimi_code_wire_usage(content: &str) -> Option<ProviderUsage> {
    for line in content.lines().rev() {
        if let Some(usage) = parse_kimi_code_usage_record(line) {
            return Some(usage);
        }
    }
    None
}

fn parse_kimi_code_usage_record(line: &str) -> Option<ProviderUsage> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "usage.record" {
        return None;
    }
    // We only want per-turn records; session-scoped records would double-count
    // accumulated usage when the runtime does its own delta math.
    if v.get("usageScope").and_then(Value::as_str) != Some("turn") {
        return None;
    }
    let usage = v.get("usage")?;
    let input_other = usage.get("inputOther").and_then(Value::as_u64);
    let output = usage.get("output").and_then(Value::as_u64);
    let cache_read = usage.get("inputCacheRead").and_then(Value::as_u64);
    let cache_creation = usage.get("inputCacheCreation").and_then(Value::as_u64);
    if input_other.is_none() && output.is_none() && cache_read.is_none() && cache_creation.is_none()
    {
        return None;
    }
    let input_sum = input_other
        .unwrap_or(0)
        .saturating_add(cache_read.unwrap_or(0))
        .saturating_add(cache_creation.unwrap_or(0));
    let context_tokens = if input_sum > 0 || output.is_some() {
        Some(input_sum.saturating_add(output.unwrap_or(0)))
    } else {
        None
    };
    Some(ProviderUsage {
        input_tokens: input_other,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_creation_tokens: cache_creation,
        context_tokens,
        context_window_tokens: Some(KIMI_CONTEXT_WINDOW_TOKENS),
        ..Default::default()
    })
}

fn scan_kimi_context_usage(content: &str) -> Option<ProviderUsage> {
    for line in content.lines().rev() {
        if let Some(usage) = parse_kimi_context_usage_line(line) {
            return Some(usage);
        }
    }
    None
}

fn parse_kimi_context_usage_line(line: &str) -> Option<ProviderUsage> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("role")?.as_str()? != "_usage" {
        return None;
    }
    let token_count = v.get("token_count")?.as_u64()?;
    Some(ProviderUsage {
        cache_read_tokens: Some(token_count),
        context_tokens: Some(token_count),
        context_window_tokens: Some(KIMI_CONTEXT_WINDOW_TOKENS),
        ..Default::default()
    })
}

/// Helper to send ExecResult on the result oneshot when the driver
/// bails early (handshake failure / timeout). Mirrors hermes.
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
        usage_report: ProviderUsageReport::from_usage(usage.clone()),
        usage,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    // Cases adapted from multica/server/pkg/agent/kimi_test.go
    // TestKimiToolNameFromTitle. The mapper is the only piece of kimi
    // that's straightforward to unit-test in isolation; the rest of
    // execute() needs a live `kimi acp` subprocess (see `#[ignore]`
    // e2e tests in similar providers — not added here per task scope).

    #[test]
    fn maps_read_file() {
        assert_eq!(
            kimi_tool_name_from_title("Read file: /tmp/foo.txt"),
            "read_file"
        );
        // Lowercase variant — multica covers this case too.
        assert_eq!(kimi_tool_name_from_title("read"), "read_file");
    }

    #[test]
    fn maps_write_file() {
        assert_eq!(
            kimi_tool_name_from_title("Write file: /tmp/bar.txt"),
            "write_file"
        );
        // Multica also covers the bare "Write: …" form.
        assert_eq!(
            kimi_tool_name_from_title("Write: /tmp/bar.go"),
            "write_file"
        );
    }

    #[test]
    fn maps_edit_and_patch() {
        // `"Edit"` (bare) and `"Patch: …"` (colon-stripped → "Patch")
        // both map; multica's switch hits the lowercase prefix.
        assert_eq!(kimi_tool_name_from_title("Edit"), "edit_file");
        assert_eq!(kimi_tool_name_from_title("Edit file: foo"), "edit_file");
        assert_eq!(kimi_tool_name_from_title("Patch: /tmp/x"), "edit_file");
    }

    #[test]
    fn maps_shell_variants() {
        // Multica's switch covers these exact lowercased prefixes:
        //   "shell" | "bash" | "terminal" | "run command" | "run shell command"
        // Note: `"Shell command: ls"` is NOT in that list (prefix lowercases
        // to "shell command", not "shell"), so it falls through to the
        // snake_case fallback — this matches multica's behaviour.
        for t in [
            "Shell: ls -la",
            "Bash",
            "Bash: pwd",
            "Terminal: echo",
            "Run command: gcc",
            "Run shell command: make",
        ] {
            assert_eq!(kimi_tool_name_from_title(t), "terminal", "input: {t}");
        }
    }

    #[test]
    fn maps_search_grep_find() {
        for t in ["Search: foo", "Grep: bar", "Find: baz"] {
            assert_eq!(kimi_tool_name_from_title(t), "search_files", "input: {t}");
        }
    }

    #[test]
    fn maps_glob_web_todo() {
        assert_eq!(kimi_tool_name_from_title("Glob: **/*.rs"), "glob");
        assert_eq!(kimi_tool_name_from_title("Web search: rust"), "web_search");
        assert_eq!(
            kimi_tool_name_from_title("Web fetch: https://"),
            "web_fetch"
        );
        assert_eq!(kimi_tool_name_from_title("Todo write"), "todo_write");
        assert_eq!(kimi_tool_name_from_title("Todo Write"), "todo_write");
    }

    #[test]
    fn empty_returns_empty() {
        assert_eq!(kimi_tool_name_from_title(""), "");
        assert_eq!(kimi_tool_name_from_title("   "), "");
    }

    #[test]
    fn unknown_falls_through_to_snake_case() {
        // multica fallback: `strings.ReplaceAll(strings.ToLower(t), " ", "_")`.
        // "Custom Thing" (no colon) → "custom_thing".
        assert_eq!(kimi_tool_name_from_title("Custom Thing"), "custom_thing");
        // With a colon: prefix-only, then snake_case.
        // "Custom tool: arg" → prefix "Custom tool" → "custom_tool".
        assert_eq!(kimi_tool_name_from_title("Custom tool: arg"), "custom_tool");
    }

    #[test]
    fn provider_trait_flags() {
        let p = KimiProvider::new(ProviderConfig::default());
        assert!(p.reports_usage());
        assert!(!p.usage_is_cumulative());
        // self_managed_context overridden to false (runtime owns
        // [[RESET]] / occupancy preamble, unlike hermes).
        assert!(!p.self_managed_context());
    }

    #[test]
    fn parses_local_context_usage_as_cache_read_estimate() {
        let usage = parse_kimi_context_usage_line(r#"{"role":"_usage","token_count":25695}"#)
            .expect("usage");

        assert_eq!(usage.cache_read_tokens, Some(25_695));
        assert_eq!(usage.context_tokens, Some(25_695));
        assert_eq!(
            usage.context_window_tokens,
            Some(KIMI_CONTEXT_WINDOW_TOKENS)
        );
        assert_eq!(usage.input_tokens, None);
        assert_eq!(usage.output_tokens, None);
    }

    #[test]
    fn reads_latest_local_context_usage_for_session() {
        let root = std::env::temp_dir().join(format!(
            "gitim-kimi-usage-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let session_id = "c7c8556e-9fa6-4f90-8992-2a9d1c614bf3";
        let session_dir = root
            .join("sessions")
            .join("workspace-hash")
            .join(session_id);
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(
            session_dir.join("context.jsonl"),
            r#"{"role":"_usage","token_count":1000}"#,
        )
        .unwrap();
        std::fs::write(
            session_dir.join("context_1.jsonl"),
            r#"{"role":"_usage","token_count":2000}"#,
        )
        .unwrap();

        let usage = read_kimi_local_context_usage(Some(&root), session_id).expect("usage");
        assert_eq!(usage.cache_read_tokens, Some(2_000));
        assert_eq!(usage.context_tokens, Some(2_000));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn falls_back_when_latest_context_file_has_no_usage() {
        let root = std::env::temp_dir().join(format!(
            "gitim-kimi-usage-fallback-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let session_id = "384aef86-486f-4aa1-ad11-9c49987b25eb";
        let session_dir = root
            .join("sessions")
            .join("workspace-hash")
            .join(session_id);
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(
            session_dir.join("context_1.jsonl"),
            r#"{"role":"_usage","token_count":3000}"#,
        )
        .unwrap();
        std::fs::write(
            session_dir.join("context_2.jsonl"),
            r#"{"role":"assistant","content":"done"}"#,
        )
        .unwrap();

        let usage = read_kimi_local_context_usage(Some(&root), session_id).expect("usage");
        assert_eq!(usage.cache_read_tokens, Some(3_000));
        assert_eq!(usage.context_tokens, Some(3_000));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parses_kimi_code_wire_usage_record() {
        let line = r#"{"type":"usage.record","model":"kimi-code/kimi-for-coding","usage":{"inputOther":1891,"output":36,"inputCacheRead":14336,"inputCacheCreation":0},"usageScope":"turn","time":1781322540934}"#;
        let usage = parse_kimi_code_usage_record(line).expect("usage");

        assert_eq!(usage.input_tokens, Some(1_891));
        assert_eq!(usage.output_tokens, Some(36));
        assert_eq!(usage.cache_read_tokens, Some(14_336));
        assert_eq!(usage.cache_creation_tokens, Some(0));
        assert_eq!(usage.context_tokens, Some(16_263));
        assert_eq!(
            usage.context_window_tokens,
            Some(KIMI_CONTEXT_WINDOW_TOKENS)
        );
    }

    #[test]
    fn context_tokens_includes_cache_creation() {
        // First turn often pays the cache-creation cost; those tokens are
        // still part of the prompt context and must count toward occupancy.
        let line = r#"{"type":"usage.record","usage":{"inputOther":500,"output":30,"inputCacheRead":1000,"inputCacheCreation":8000},"usageScope":"turn"}"#;
        let usage = parse_kimi_code_usage_record(line).expect("usage");

        assert_eq!(usage.input_tokens, Some(500));
        assert_eq!(usage.output_tokens, Some(30));
        assert_eq!(usage.cache_read_tokens, Some(1_000));
        assert_eq!(usage.cache_creation_tokens, Some(8_000));
        assert_eq!(usage.context_tokens, Some(9_530));
    }

    #[test]
    fn ignores_session_scoped_usage_record() {
        let line = r#"{"type":"usage.record","usage":{"inputOther":1000,"output":20,"inputCacheRead":5000,"inputCacheCreation":0},"usageScope":"session"}"#;
        assert!(parse_kimi_code_usage_record(line).is_none());
    }

    #[test]
    fn reads_latest_turn_usage_from_wire_jsonl() {
        let root = std::env::temp_dir().join(format!(
            "gitim-kimi-code-wire-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let session_id = "session_a1b2c3d4-e5f6-7890-abcd-ef1234567890";
        let session_dir = root
            .join("sessions")
            .join("workspace-hash")
            .join(session_id);
        let agents_dir = session_dir.join("agents/main");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("wire.jsonl"),
            r#"{"type":"usage.record","usage":{"inputOther":100,"output":5,"inputCacheRead":50,"inputCacheCreation":1},"usageScope":"turn"}
{"type":"usage.record","usage":{"inputOther":200,"output":10,"inputCacheRead":100,"inputCacheCreation":2},"usageScope":"turn"}
"#,
        )
        .unwrap();

        let usage = read_kimi_local_context_usage(Some(&root), session_id).expect("usage");
        assert_eq!(usage.input_tokens, Some(200));
        assert_eq!(usage.output_tokens, Some(10));
        assert_eq!(usage.cache_read_tokens, Some(100));
        assert_eq!(usage.cache_creation_tokens, Some(2));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn wire_jsonl_takes_precedence_over_legacy_context() {
        let root = std::env::temp_dir().join(format!(
            "gitim-kimi-code-prec-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let session_id = "session_11111111-2222-3333-4444-555555555555";
        let session_dir = root
            .join("sessions")
            .join("workspace-hash")
            .join(session_id);
        let agents_dir = session_dir.join("agents/main");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("wire.jsonl"),
            r#"{"type":"usage.record","usage":{"inputOther":123,"output":7,"inputCacheRead":89,"inputCacheCreation":3},"usageScope":"turn"}"#,
        )
        .unwrap();
        std::fs::write(
            session_dir.join("context.jsonl"),
            r#"{"role":"_usage","token_count":9999}"#,
        )
        .unwrap();

        let usage = read_kimi_local_context_usage(Some(&root), session_id).expect("usage");
        assert_eq!(usage.input_tokens, Some(123));
        assert_eq!(usage.output_tokens, Some(7));

        std::fs::remove_dir_all(root).unwrap();
    }
}
