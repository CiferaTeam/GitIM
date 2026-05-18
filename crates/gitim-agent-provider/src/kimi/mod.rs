//! Kimi Code CLI provider — ACP transport via shared `acp::AcpClient`.
//!
//! Kimi (<https://github.com/MoonshotAI/kimi-cli>) speaks the same ACP
//! JSON-RPC protocol as hermes via `kimi acp`. This provider is hermes
//! with three swapped knobs:
//!
//! 1. The spawn target is `kimi --afk acp` and `HERMES_YOLO_MODE` is
//!    **not** injected. The root `--afk` flag tells Kimi this is a
//!    headless run; the daemon still handles explicit ACP permission
//!    requests as a backstop.
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
//! - `reports_usage()` is **overridden to `false`** — Kimi Code 1.44 ACP
//!   `session/prompt` responses return `stopReason` but no `usage` block, and
//!   the observed stream does not emit `usage_update` notifications.
//! - `self_managed_context()` stays `false` (default written explicitly
//!   here) — unlike hermes there is no in-loop compression, so the runtime
//!   owns the `[[RESET]]` channel + `[系统通知]` occupancy preamble, same
//!   as claude/codex.
//! - All `prompt_*` defaults inherit unchanged — there is no SOUL.md /
//!   MEMORY.md self-managed memory model on kimi.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::acp::{try_send_event, AcpClient, AcpHooks};
use crate::{
    Event, ExecOptions, ExecResult, ExecStatus, Provider, ProviderConfig, ProviderError,
    ProviderUsage, Session,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const HANDSHAKE_TIMEOUT_MAX: Duration = Duration::from_secs(30);
const EVENT_CHANNEL_BUFFER: usize = 256;
const IDLE_COMPLETE_AFTER_TERMINAL_ACTIVITY: Duration = Duration::from_secs(15);
const IDLE_COMPLETE_AFTER_TOOL_USE: Duration = Duration::from_secs(75);
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
    /// Kimi's ACP stream does not currently expose token counts. The runtime
    /// still counts turns and can use its local estimate for the session
    /// occupancy HUD, but it must not render zero provider tokens as if Kimi
    /// had reported them.
    fn reports_usage(&self) -> bool {
        false
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
        // `--afk` is a root flag, so it must come before the `acp`
        // subcommand. It puts Kimi in headless mode: AskUserQuestion is
        // auto-dismissed and tool calls are treated as unattended. We still
        // reply to `session/request_permission` inside the ACP client because
        // Kimi 1.44 emits that request even in AFK mode.
        cmd.arg("--afk")
            .arg("acp")
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

        let stdout = child.stdout.take().expect("stdout piped");
        let stdin = child.stdin.take().expect("stdin piped");
        let stderr = child.stderr.take().expect("stderr piped");

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
            let mut tail = stderr_tail_clone.lock().unwrap();
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
    let reader_handle = tokio::spawn(async move {
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
        IdleComplete,
    }

    let (idle_complete_tx, mut idle_complete_rx) = watch::channel(false);
    let idle_watch_cancel = CancellationToken::new();
    let idle_watch_cancel_inner = idle_watch_cancel.clone();
    let idle_watch_acp = Arc::clone(&acp);
    let idle_watch_handle = tokio::spawn(async move {
        watch_idle_completion(
            idle_watch_acp,
            idle_complete_tx,
            idle_watch_cancel_inner,
            pid,
        )
        .await;
    });

    let prompt_future = prompt_acp.prompt(&prompt_sid, &prompt_text);
    tokio::pin!(prompt_future);
    let timeout_sleep = tokio::time::sleep(timeout);
    tokio::pin!(timeout_sleep);

    let prompt_outcome = loop {
        tokio::select! {
            r = &mut prompt_future => break PromptWait::Finished(r),
            _ = cancel_token.cancelled() => break PromptWait::Cancelled,
            _ = &mut timeout_sleep => break PromptWait::Timeout,
            r = idle_complete_rx.changed() => {
                if r.is_ok() && *idle_complete_rx.borrow() {
                    break PromptWait::IdleComplete;
                }
            },
        }
    };
    idle_watch_cancel.cancel();
    idle_watch_handle.abort();

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
        PromptWait::IdleComplete => {
            info!(
                pid,
                terminal_idle_after = ?IDLE_COMPLETE_AFTER_TERMINAL_ACTIVITY,
                tool_idle_after = ?IDLE_COMPLETE_AFTER_TOOL_USE,
                "kimi prompt response idle-completed after provider activity"
            );
        }
    }

    // ── Post-loop cleanup ──

    // Kimi's ACP server is a session server: after `session/prompt`
    // resolves it keeps the stdio process alive waiting for the next
    // request. GitIM launches one provider process per agent turn, so the
    // correct cleanup is to stop this child explicitly instead of waiting
    // for a natural EOF exit.
    let killed_for_shutdown = true;
    let _ = child.start_kill();

    // Drain the reader so trailing notifications (if any) make it into
    // the text accumulator before we sample the final output. After
    // this await, no further `handle_line` calls happen and the
    // accumulator is stable — a single read is sufficient.
    let _ = reader_handle.await;

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

    let duration = start.elapsed();
    info!(pid, ?final_status, ?duration, "kimi finished");

    stderr_handle.abort();

    // If failed with no error message, fall back to stderr tail so the
    // user sees something actionable. Mirror hermes' pattern.
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

async fn watch_idle_completion(
    acp: Arc<AcpClient>,
    complete: watch::Sender<bool>,
    cancel: CancellationToken,
    pid: u32,
) {
    info!(
        pid,
        terminal_idle_after = ?IDLE_COMPLETE_AFTER_TERMINAL_ACTIVITY,
        tool_idle_after = ?IDLE_COMPLETE_AFTER_TOOL_USE,
        "kimi idle completion watchdog started"
    );
    let mut activity_logged = false;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
        }

        let terminal_idle = acp
            .last_terminal_activity_at()
            .await
            .map(|last| last.elapsed());
        let tool_idle = acp.last_tool_use_at().await.map(|last| last.elapsed());

        if !activity_logged && (terminal_idle.is_some() || tool_idle.is_some()) {
            info!(
                pid,
                terminal_idle = ?terminal_idle,
                tool_idle = ?tool_idle,
                "kimi idle completion watchdog observed provider activity"
            );
            activity_logged = true;
        }

        if let Some(idle) = terminal_idle {
            if idle >= IDLE_COMPLETE_AFTER_TERMINAL_ACTIVITY {
                info!(
                    pid,
                    idle = ?idle,
                    threshold = ?IDLE_COMPLETE_AFTER_TERMINAL_ACTIVITY,
                    "kimi idle completion watchdog reached terminal idle threshold"
                );
                let _ = complete.send(true);
                break;
            }
        }
        if let Some(idle) = tool_idle {
            if idle >= IDLE_COMPLETE_AFTER_TOOL_USE {
                info!(
                    pid,
                    idle = ?idle,
                    threshold = ?IDLE_COMPLETE_AFTER_TOOL_USE,
                    "kimi idle completion watchdog reached tool idle threshold"
                );
                let _ = complete.send(true);
                break;
            }
        }
    }
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
        assert!(!p.reports_usage());
        assert!(!p.usage_is_cumulative());
        // self_managed_context overridden to false (runtime owns
        // [[RESET]] / occupancy preamble, unlike hermes).
        assert!(!p.self_managed_context());
    }
}
