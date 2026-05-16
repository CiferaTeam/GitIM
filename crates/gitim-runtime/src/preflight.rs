//! Preflight module: real-hello CLI verification.
//! Used by /preflight/{provider} HTTP endpoint.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Per-call overrides for agent-aware preflight invocations.
///
/// Used by `preflight_for_add_request` (and direct callers from `agents_add`)
/// to inject the agent's actual env vars and model into the provider CLI
/// subprocess, so the preflight verifies the same configuration the agent
/// will eventually run under — not the runtime's default profile.
///
/// `Default::default()` produces the legacy behavior used by the
/// `/preflight/{provider}` HTTP route: inherited env, default preflight model.
#[derive(Debug, Clone, Default)]
pub struct PreflightOverrides {
    /// Extra env vars to merge into the child process (overrides inherited values).
    pub env_override: Option<HashMap<String, String>>,
    /// Model name to pass on the CLI (replaces the per-provider preflight constant).
    pub model_override: Option<String>,
}

/// Why a preflight attempt failed. Serialized as snake_case so the
/// WebUI can branch on a stable string (`not_installed`, `timeout`, `other`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    NotInstalled,
    Timeout,
    Other,
}

/// Outcome of a real-hello preflight call against a provider CLI.
///
/// Fields are kept explicit (never skipped) so the JSON shape is stable
/// for the frontend: missing data is `null`, not an absent key.
///
/// `failure_code` is an additive optional field used by
/// [`preflight_for_add_request`] to tag *setup-level* failures (e.g. caller
/// gave only one of `llm_provider`/`llm_model`, default hermes profile has no
/// LLM, unknown provider name) before the dispatcher ever shells out. Plain
/// `_with_config` calls leave it `None`; [`classify_preflight_error_code`]
/// consumes the tag to map to the HTTP top-level `error_code`. Skipped from
/// serialization when `None` so existing JSON consumers (frontend, CLI DTO)
/// don't see a new always-null field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreflightResult {
    pub available: bool,
    pub provider: String,
    pub version: Option<String>,
    pub model_used: Option<String>,
    pub duration_ms: u64,
    pub output_preview: Option<String>,
    pub error: Option<String>,
    pub error_kind: Option<ErrorKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
}

impl PreflightResult {
    /// Build a successful preflight result. `output_preview` is the first
    /// few bytes of the CLI stdout (e.g. "GITIM_OK") for UI display.
    pub fn success(
        provider: impl Into<String>,
        version: Option<String>,
        model_used: Option<String>,
        duration_ms: u64,
        output_preview: Option<String>,
    ) -> Self {
        Self {
            available: true,
            provider: provider.into(),
            version,
            model_used,
            duration_ms,
            output_preview,
            error: None,
            error_kind: None,
            failure_code: None,
        }
    }

    /// Build a failure result. Callers short-circuit with this when the CLI
    /// is missing, times out, or produces an unexpected response.
    pub fn failure(
        provider: impl Into<String>,
        kind: ErrorKind,
        error: impl Into<String>,
        duration_ms: u64,
    ) -> Self {
        Self {
            available: false,
            provider: provider.into(),
            version: None,
            model_used: None,
            duration_ms,
            output_preview: None,
            error: Some(error.into()),
            error_kind: Some(kind),
            failure_code: None,
        }
    }

    /// Build a failure result tagged with a setup-level failure code.
    ///
    /// Used by [`preflight_for_add_request`] for failures it detects before
    /// dispatching to a provider-specific preflight (unknown provider name,
    /// missing-LLM-pair, no LLM in default hermes profile). The tag is
    /// consumed by [`classify_preflight_error_code`] to map to the HTTP
    /// top-level `error_code`.
    pub fn failure_with_code(
        provider: impl Into<String>,
        kind: ErrorKind,
        error: impl Into<String>,
        duration_ms: u64,
        code: impl Into<String>,
    ) -> Self {
        let mut r = Self::failure(provider, kind, error, duration_ms);
        r.failure_code = Some(code.into());
        r
    }
}

/// Binary names to check alongside runtime itself.
const PEERS: &[(&str, &str)] = &[("gitim", "gitim"), ("gitim-daemon", "gitim-daemon")];

#[derive(Debug)]
pub struct VersionMismatch {
    pub binary: String,
    pub found: String,
    pub expected: String,
}

#[derive(Debug)]
pub struct PreflightError {
    pub missing: Vec<String>,
    pub mismatches: Vec<VersionMismatch>,
}

impl std::fmt::Display for PreflightError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "environment preflight failed")?;
        writeln!(f, "  expected version: {RUNTIME_VERSION}")?;
        for m in &self.mismatches {
            writeln!(f, "  {} version mismatch: found {}", m.binary, m.found)?;
        }
        for name in &self.missing {
            writeln!(f, "  {} not found in PATH or runtime directory", name)?;
        }
        Ok(())
    }
}

/// Find a binary: first check the directory where the current exe lives,
/// then fall back to PATH lookup.
fn find_binary(name: &str) -> Option<PathBuf> {
    // Check sibling of current executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    // Fallback: rely on PATH
    which_in_path(name)
}

fn which_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var("PATH").ok()?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Run `<binary> --version`, parse the version string.
/// Expected format: `<name> <version>` (e.g. "gitim 0.3.1").
pub fn query_version(binary_path: &Path) -> Option<String> {
    let output = Command::new(binary_path).arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Take the last whitespace-separated token on the first line
    let first_line = stdout.lines().next()?;
    first_line.split_whitespace().last().map(|s| s.to_string())
}

/// Run environment preflight check.
/// Returns Ok(()) if all binaries are found and version-aligned.
pub fn check_env() -> Result<(), PreflightError> {
    let mut missing = Vec::new();
    let mut mismatches = Vec::new();

    for &(name, binary_name) in PEERS {
        match find_binary(binary_name) {
            None => missing.push(name.to_string()),
            Some(path) => match query_version(&path) {
                None => missing.push(format!("{name} (found but --version failed)")),
                Some(version) if version != RUNTIME_VERSION => {
                    mismatches.push(VersionMismatch {
                        binary: name.to_string(),
                        found: version,
                        expected: RUNTIME_VERSION.to_string(),
                    });
                }
                Some(_) => {} // matched
            },
        }
    }

    if missing.is_empty() && mismatches.is_empty() {
        Ok(())
    } else {
        Err(PreflightError {
            missing,
            mismatches,
        })
    }
}

/// Model forced during the preflight ping. Held constant so response-time
/// and cost are predictable across environments.
const CLAUDE_PREFLIGHT_MODEL: &str = "claude-haiku-4-5";

/// Max chars of stderr/output to surface — keeps logs and UI tooltips bounded
/// when Claude prints a multi-line error or a verbose session transcript.
const STDERR_TRUNCATE: usize = 500;
const PREVIEW_TRUNCATE: usize = 200;

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect()
    }
}

/// Classify a spawn-time `io::Error` into our stable [`ErrorKind`] taxonomy.
/// `NotFound` means the CLI binary isn't on PATH; everything else (permission
/// denied, ENOEXEC, etc.) is bucketed as `Other` so the WebUI only has to
/// special-case "not installed".
fn map_spawn_error(err: &std::io::Error) -> ErrorKind {
    match err.kind() {
        std::io::ErrorKind::NotFound => ErrorKind::NotInstalled,
        _ => ErrorKind::Other,
    }
}

fn split_provider_model(value: &str) -> Option<(&str, &str)> {
    let (provider, model) = value.split_once('/')?;
    if provider.is_empty() || model.is_empty() {
        None
    } else {
        Some((provider, model))
    }
}

/// Extract the `result` text from `claude --print --output-format json` stdout.
///
/// Tolerates both shapes the Claude CLI has been observed to emit:
/// - a JSON array of event objects (older CLI versions), or
/// - a single JSON object representing the final result directly.
///
/// Returns the `result` field as a `String`, or a human-readable error suitable
/// for surfacing via [`PreflightResult::failure`].
fn parse_claude_result(stdout: &str) -> Result<String, String> {
    let root: serde_json::Value = serde_json::from_str(stdout)
        .map_err(|e| format!("failed to parse claude JSON output: {e}"))?;
    let items: Vec<serde_json::Value> = match root {
        serde_json::Value::Array(a) => a,
        // Single-object fallback: wrap so the scan below treats it uniformly.
        other => vec![other],
    };

    items
        .iter()
        .find(|item| item.get("type").and_then(|t| t.as_str()) == Some("result"))
        .and_then(|item| item.get("result"))
        .and_then(|r| r.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            "claude JSON output did not contain a result entry with a `result` field".to_string()
        })
}

/// Run a real-hello ping against the Claude CLI at `bin`, with optional
/// per-call overrides for env vars and model name.
///
/// Returns a `PreflightResult` that captures the outcome with a stable error
/// taxonomy (`NotInstalled` / `Timeout` / `Other`). Split from
/// [`preflight_claude`] so tests can inject fake binaries (e.g. `/bin/false`,
/// a stalling shell script) to exercise each error branch without needing a
/// logged-in Claude CLI.
///
/// When `overrides.env_override` is `Some`, those key/values are merged into
/// the child process (overriding any inherited values with the same key).
/// When `overrides.model_override` is `Some`, that string is used as the
/// `--model` argv value (and reflected in `PreflightResult.model_used`);
/// otherwise [`CLAUDE_PREFLIGHT_MODEL`] is used. `Default::default()` therefore
/// preserves the legacy behavior used by [`preflight_claude_with`].
pub async fn preflight_claude_with_config(
    bin: &str,
    timeout: Duration,
    overrides: PreflightOverrides,
) -> PreflightResult {
    let started = Instant::now();

    // Isolate cwd so Claude doesn't pick up project memory, settings, or
    // MCP config from whatever directory the caller happens to be in.
    let tmpdir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => {
            return PreflightResult::failure(
                "claude",
                ErrorKind::Other,
                format!("failed to create tempdir: {e}"),
                started.elapsed().as_millis() as u64,
            );
        }
    };

    let model = overrides
        .model_override
        .as_deref()
        .unwrap_or(CLAUDE_PREFLIGHT_MODEL);

    let mut cmd = tokio::process::Command::new(bin);
    cmd.current_dir(tmpdir.path())
        .arg("--print")
        .args(["--model", model])
        .args(["--output-format", "json"])
        .args(["--setting-sources", ""])
        .args(["--tools", ""])
        .args(["--system-prompt", "Reply with exactly what the user asks."])
        .arg("Reply with exactly: GITIM_OK")
        // Pipe stdin so we can close the write end immediately — some
        // Claude CLI versions block on stdin readiness when it's `null`.
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    if let Some(env) = &overrides.env_override {
        cmd.envs(env);
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let kind = map_spawn_error(&e);
            let msg = if kind == ErrorKind::NotInstalled {
                format!("claude CLI not found at `{bin}`: {e}")
            } else {
                format!("failed to spawn claude: {e}")
            };
            return PreflightResult::failure(
                "claude",
                kind,
                msg,
                started.elapsed().as_millis() as u64,
            );
        }
    };

    // Signal EOF immediately — Claude's --print mode doesn't need input on stdin.
    drop(child.stdin.take());

    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            return PreflightResult::failure(
                "claude",
                ErrorKind::Other,
                format!("claude IO error: {e}"),
                started.elapsed().as_millis() as u64,
            );
        }
        Err(_) => {
            return PreflightResult::failure(
                "claude",
                ErrorKind::Timeout,
                format!("claude preflight exceeded {}ms", timeout.as_millis()),
                started.elapsed().as_millis() as u64,
            );
        }
    };

    let duration_ms = started.elapsed().as_millis() as u64;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let trimmed = truncate(stderr.trim(), STDERR_TRUNCATE);
        let msg = if trimmed.is_empty() {
            format!("claude exited with status {}", output.status)
        } else {
            format!("claude exited with status {}: {}", output.status, trimmed)
        };
        return PreflightResult::failure("claude", ErrorKind::Other, msg, duration_ms);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed_stdout = stdout.trim();
    if trimmed_stdout.is_empty() {
        return PreflightResult::failure(
            "claude",
            ErrorKind::Other,
            "claude returned empty stdout",
            duration_ms,
        );
    }

    // `claude --print --output-format json` may emit either a JSON array of
    // events or a single JSON object (shape varies across CLI versions). Both
    // cases resolve to the same thing: find the `type == "result"` entry and
    // read its `result` field.
    let text = match parse_claude_result(trimmed_stdout) {
        Ok(t) => t,
        Err(msg) => {
            return PreflightResult::failure("claude", ErrorKind::Other, msg, duration_ms);
        }
    };

    if text.contains("GITIM_OK") {
        PreflightResult::success(
            "claude",
            None,
            Some(model.to_string()),
            duration_ms,
            Some(truncate(&text, PREVIEW_TRUNCATE)),
        )
    } else {
        PreflightResult::failure(
            "claude",
            ErrorKind::Other,
            "response did not contain GITIM_OK",
            duration_ms,
        )
    }
}

/// Run a real-hello ping against the Claude CLI at `bin`.
///
/// Thin wrapper over [`preflight_claude_with_config`] preserved for the
/// `/preflight/{provider}` HTTP route and tests that don't need agent-aware
/// overrides. New call sites that have access to an agent's env/model should
/// prefer [`preflight_claude_with_config`] directly.
pub async fn preflight_claude_with(bin: &str, timeout: Duration) -> PreflightResult {
    preflight_claude_with_config(bin, timeout, PreflightOverrides::default()).await
}

/// Run a real-hello preflight against the default `claude` binary.
///
/// Spawns `claude --print` with minimal-context flags, sends a fixed prompt,
/// and returns a classified [`PreflightResult`]. Used by the HTTP preflight
/// route.
pub async fn preflight_claude() -> PreflightResult {
    preflight_claude_with("claude", Duration::from_secs(60)).await
}

/// Model forced during the codex preflight ping. Same rationale as
/// [`CLAUDE_PREFLIGHT_MODEL`]: predictable response-time and cost.
const CODEX_PREFLIGHT_MODEL: &str = "gpt-5.4-mini";

/// Run a real-hello ping against the Codex CLI at `bin`, with optional
/// per-call overrides for env vars and model name.
///
/// Mirrors [`preflight_claude_with_config`]: isolates cwd, spawns the CLI with
/// a fixed prompt, enforces a timeout, and classifies the result with the same
/// `NotInstalled` / `Timeout` / `Other` taxonomy.
///
/// The shape diverges in two places:
/// 1. Codex accepts `Stdio::null()` on stdin (claude needs a piped EOF).
/// 2. `codex exec --json` emits JSONL (one JSON object per line), not a
///    single JSON array. We scan for `turn.completed` (the stream terminator)
///    and extract the `agent_message` text from the matching `item.completed`.
///
/// Override semantics match the claude variant. `Default::default()` preserves
/// the legacy behavior used by [`preflight_codex_with`].
pub async fn preflight_codex_with_config(
    bin: &str,
    timeout: Duration,
    overrides: PreflightOverrides,
) -> PreflightResult {
    let started = Instant::now();

    let tmpdir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => {
            return PreflightResult::failure(
                "codex",
                ErrorKind::Other,
                format!("failed to create tempdir: {e}"),
                started.elapsed().as_millis() as u64,
            );
        }
    };

    let model = overrides
        .model_override
        .as_deref()
        .unwrap_or(CODEX_PREFLIGHT_MODEL);

    let mut cmd = tokio::process::Command::new(bin);
    cmd.current_dir(tmpdir.path())
        .arg("exec")
        .arg("--json")
        // Our tempdir is isolation-by-design, not a git repo. Without
        // `--skip-git-repo-check`, codex refuses to run with "Not inside a
        // trusted directory".
        .arg("--skip-git-repo-check")
        .args(["--model", model])
        .arg("Reply with exactly: GITIM_OK")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    if let Some(env) = &overrides.env_override {
        cmd.envs(env);
    }

    // Codex is happy with a null stdin — unlike claude, no need to drop
    // the write end to signal EOF.
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let kind = map_spawn_error(&e);
            let msg = if kind == ErrorKind::NotInstalled {
                format!("codex CLI not found at `{bin}`: {e}")
            } else {
                format!("failed to spawn codex: {e}")
            };
            return PreflightResult::failure(
                "codex",
                kind,
                msg,
                started.elapsed().as_millis() as u64,
            );
        }
    };

    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            return PreflightResult::failure(
                "codex",
                ErrorKind::Other,
                format!("codex IO error: {e}"),
                started.elapsed().as_millis() as u64,
            );
        }
        Err(_) => {
            return PreflightResult::failure(
                "codex",
                ErrorKind::Timeout,
                format!("codex preflight exceeded {}ms", timeout.as_millis()),
                started.elapsed().as_millis() as u64,
            );
        }
    };

    let duration_ms = started.elapsed().as_millis() as u64;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let trimmed = truncate(stderr.trim(), STDERR_TRUNCATE);
        let msg = if trimmed.is_empty() {
            format!("codex exited with status {}", output.status)
        } else {
            format!("codex exited with status {}: {}", output.status, trimmed)
        };
        return PreflightResult::failure("codex", ErrorKind::Other, msg, duration_ms);
    }

    // JSONL: each non-empty line should be a JSON object. We care about two
    // record types:
    //   - `item.completed` with `item.type == "agent_message"` → carries the text
    //   - `turn.completed` → marks the stream as cleanly finished
    // Non-JSON lines and types we don't recognize are ignored. Codex may log
    // symlink warnings to stderr; those don't affect parsing here.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut saw_turn_completed = false;
    let mut agent_message: Option<String> = None;
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(obj) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match obj.get("type").and_then(|t| t.as_str()) {
            Some("turn.completed") => saw_turn_completed = true,
            Some("item.completed") => {
                let item = obj.get("item");
                let is_agent_message = item.and_then(|i| i.get("type")).and_then(|t| t.as_str())
                    == Some("agent_message");
                if is_agent_message {
                    if let Some(text) = item.and_then(|i| i.get("text")).and_then(|t| t.as_str()) {
                        agent_message = Some(text.to_string());
                    }
                }
            }
            _ => {}
        }
    }

    if !saw_turn_completed {
        return PreflightResult::failure(
            "codex",
            ErrorKind::Other,
            "codex stream ended without turn.completed",
            duration_ms,
        );
    }

    let text = match agent_message {
        Some(t) => t,
        None => {
            return PreflightResult::failure(
                "codex",
                ErrorKind::Other,
                "no agent_message in codex output",
                duration_ms,
            );
        }
    };

    if text.contains("GITIM_OK") {
        PreflightResult::success(
            "codex",
            None,
            Some(model.to_string()),
            duration_ms,
            Some(truncate(&text, PREVIEW_TRUNCATE)),
        )
    } else {
        PreflightResult::failure(
            "codex",
            ErrorKind::Other,
            "response did not contain GITIM_OK",
            duration_ms,
        )
    }
}

/// Run a real-hello ping against the Codex CLI at `bin`.
///
/// Thin wrapper over [`preflight_codex_with_config`] preserved for the
/// `/preflight/{provider}` HTTP route and tests that don't need agent-aware
/// overrides. New call sites that have access to an agent's env/model should
/// prefer [`preflight_codex_with_config`] directly.
pub async fn preflight_codex_with(bin: &str, timeout: Duration) -> PreflightResult {
    preflight_codex_with_config(bin, timeout, PreflightOverrides::default()).await
}

/// Run a real-hello preflight against the default `codex` binary.
///
/// Spawns `codex exec --json`, scans the JSONL stream for the agent message,
/// and returns a classified [`PreflightResult`].
pub async fn preflight_codex() -> PreflightResult {
    preflight_codex_with("codex", Duration::from_secs(60)).await
}

/// Run a real-hello ping against the opencode CLI at `bin`, with optional
/// per-call overrides for env vars.
///
/// When `overrides.model_override` is set, the ping uses opencode's
/// per-invocation `--model provider/model` flag so add-agent verifies the
/// same model the agent will run with. When omitted, opencode uses its
/// configured CLI default. System prompt is injected via OPENCODE_CONFIG_CONTENT
/// as a minimal echo agent to keep the request cheap and deterministic.
///
/// When `overrides.env_override` is `Some`, those key/values are merged into
/// the child process (overriding any inherited values with the same key, and
/// taking precedence over the fixed `OPENCODE_CONFIG_CONTENT` /
/// `OPENCODE_PERMISSION` only if the caller deliberately re-specifies those
/// keys — normal callers shouldn't).
pub async fn preflight_opencode_with_config(
    bin: &str,
    timeout: Duration,
    overrides: PreflightOverrides,
) -> PreflightResult {
    let started = Instant::now();

    let tmpdir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => {
            return PreflightResult::failure(
                "opencode",
                ErrorKind::Other,
                format!("failed to create tempdir: {e}"),
                started.elapsed().as_millis() as u64,
            );
        }
    };

    let config_content = serde_json::json!({
        "agent": {
            "gitim_preflight": {
                "prompt": "Reply with exactly what the user asks, nothing more.",
                "mode": "primary",
            }
        }
    })
    .to_string();

    let mut args = vec![
        "run".to_string(),
        "--format".to_string(),
        "json".to_string(),
        "--dangerously-skip-permissions".to_string(),
    ];
    if let Some(model) = overrides
        .model_override
        .as_deref()
        .filter(|m| !m.is_empty())
    {
        args.extend(["--model".to_string(), model.to_string()]);
    }
    args.extend([
        "--agent".to_string(),
        "gitim_preflight".to_string(),
        "--".to_string(),
        "Reply with exactly: GITIM_OK".to_string(),
    ]);

    let mut cmd = tokio::process::Command::new(bin);
    cmd.current_dir(tmpdir.path())
        .args(&args)
        .env("OPENCODE_CONFIG_CONTENT", &config_content)
        .env("OPENCODE_PERMISSION", r#"{"*":"allow"}"#)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    if let Some(env) = &overrides.env_override {
        cmd.envs(env);
    }

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let kind = map_spawn_error(&e);
            let msg = if kind == ErrorKind::NotInstalled {
                format!("opencode CLI not found at `{bin}`: {e}")
            } else {
                format!("failed to spawn opencode: {e}")
            };
            return PreflightResult::failure(
                "opencode",
                kind,
                msg,
                started.elapsed().as_millis() as u64,
            );
        }
    };

    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            return PreflightResult::failure(
                "opencode",
                ErrorKind::Other,
                format!("opencode IO error: {e}"),
                started.elapsed().as_millis() as u64,
            );
        }
        Err(_) => {
            return PreflightResult::failure(
                "opencode",
                ErrorKind::Timeout,
                format!("opencode preflight exceeded {}ms", timeout.as_millis()),
                started.elapsed().as_millis() as u64,
            );
        }
    };

    let duration_ms = started.elapsed().as_millis() as u64;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let trimmed = truncate(stderr.trim(), STDERR_TRUNCATE);
        let msg = if trimmed.is_empty() {
            format!("opencode exited with status {}", output.status)
        } else {
            format!("opencode exited with status {}: {}", output.status, trimmed)
        };
        return PreflightResult::failure("opencode", ErrorKind::Other, msg, duration_ms);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let text = extract_opencode_text(&stdout);

    if text.contains("GITIM_OK") {
        PreflightResult::success(
            "opencode",
            None,
            overrides.model_override.clone(),
            duration_ms,
            Some(truncate(&text, PREVIEW_TRUNCATE)),
        )
    } else {
        PreflightResult::failure(
            "opencode",
            ErrorKind::Other,
            "response did not contain GITIM_OK",
            duration_ms,
        )
    }
}

/// Run a real-hello ping against the opencode CLI at `bin`.
///
/// Thin wrapper over [`preflight_opencode_with_config`] preserved for the
/// `/preflight/{provider}` HTTP route and tests that don't need agent-aware
/// overrides. New call sites that have access to an agent's env should prefer
/// [`preflight_opencode_with_config`] directly.
pub async fn preflight_opencode_with(bin: &str, timeout: Duration) -> PreflightResult {
    preflight_opencode_with_config(bin, timeout, PreflightOverrides::default()).await
}

/// Run a real-hello preflight against the default `opencode` binary.
pub async fn preflight_opencode() -> PreflightResult {
    preflight_opencode_with("opencode", Duration::from_secs(60)).await
}

/// Run a real-hello ping against the Pi CLI at `bin` using `--mode rpc`, with
/// optional per-call overrides for env vars.
///
/// Protocol:
/// 1. Spawn `pi --mode rpc --no-session --no-tools`
/// 2. Write `{"type":"prompt","message":"Reply with exactly: GITIM_OK"}` to stdin
/// 3. Stream stdout events until `agent_end`; collect text from `message_update` deltas
/// 4. Explicitly kill the process — in `--no-session` mode Pi may exit naturally,
///    but we kill unconditionally to guarantee cleanup in both session/no-session modes
/// 5. Classify the result using the standard `NotInstalled`/`Timeout`/`Other` taxonomy
///
/// When `overrides.env_override` is `Some`, those key/values are merged into
/// the child process (overriding any inherited values with the same key).
///
/// When `overrides.model_override` is set, values shaped as `provider/model`
/// are split into Pi's native `--provider provider --model model` flags. Other
/// values are passed as `--model value`. When omitted, Pi uses its configured
/// default.
pub async fn preflight_pi_with_config(
    bin: &str,
    timeout: Duration,
    overrides: PreflightOverrides,
) -> PreflightResult {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let started = Instant::now();

    let mut args = vec!["--mode".to_string(), "rpc".to_string()];
    if let Some(model) = overrides
        .model_override
        .as_deref()
        .filter(|m| !m.is_empty())
    {
        if let Some((provider, model_id)) = split_provider_model(model) {
            args.extend(["--provider".to_string(), provider.to_string()]);
            args.extend(["--model".to_string(), model_id.to_string()]);
        } else {
            args.extend(["--model".to_string(), model.to_string()]);
        }
    }
    args.extend(["--no-session".to_string(), "--no-tools".to_string()]);

    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    if let Some(env) = &overrides.env_override {
        cmd.envs(env);
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let kind = map_spawn_error(&e);
            let msg = if kind == ErrorKind::NotInstalled {
                format!("pi CLI not found at `{bin}`: {e}")
            } else {
                format!("failed to spawn pi: {e}")
            };
            return PreflightResult::failure("pi", kind, msg, started.elapsed().as_millis() as u64);
        }
    };

    let mut stdin = child.stdin.take().expect("stdin piped");
    let stdout = child.stdout.take().expect("stdout piped");

    // Send the prompt.
    let prompt_msg = b"{\"type\":\"prompt\",\"message\":\"Reply with exactly: GITIM_OK\"}\n";
    if let Err(e) = stdin.write_all(prompt_msg).await {
        let _ = child.start_kill();
        return PreflightResult::failure(
            "pi",
            ErrorKind::Other,
            format!("failed to write prompt to pi: {e}"),
            started.elapsed().as_millis() as u64,
        );
    }
    if let Err(e) = stdin.flush().await {
        let _ = child.start_kill();
        return PreflightResult::failure(
            "pi",
            ErrorKind::Other,
            format!("failed to flush prompt to pi: {e}"),
            started.elapsed().as_millis() as u64,
        );
    }

    // Keep stdin open so pi doesn't get SIGPIPE; we'll drop it after reading.
    let mut reader = BufReader::new(stdout).lines();
    let mut collected_text = String::new();
    let mut saw_agent_end = false;
    let mut rpc_error: Option<String> = None;

    let read_result = tokio::time::timeout(timeout, async {
        while let Ok(Some(line)) = reader.next_line().await {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                let t = v.get("type").and_then(|t| t.as_str());
                match t {
                    Some("response")
                        if v.get("success").and_then(|s| s.as_bool()) == Some(false) =>
                    {
                        rpc_error = Some(
                            v.get("error")
                                .and_then(|e| e.as_str())
                                .unwrap_or("pi RPC command failed")
                                .to_string(),
                        );
                        break;
                    }
                    Some("message_update") => {
                        if let Some(delta) = v
                            .get("assistantMessageEvent")
                            .and_then(|ae| ae.get("delta"))
                            .and_then(|d| d.as_str())
                        {
                            collected_text.push_str(delta);
                        }
                    }
                    Some("agent_end") => {
                        saw_agent_end = true;
                        break;
                    }
                    _ => {}
                }
            }
        }
    })
    .await;

    drop(stdin);
    let _ = child.start_kill();
    let duration_ms = started.elapsed().as_millis() as u64;

    if read_result.is_err() {
        return PreflightResult::failure(
            "pi",
            ErrorKind::Timeout,
            format!("pi preflight exceeded {}ms", timeout.as_millis()),
            duration_ms,
        );
    }

    if let Some(error) = rpc_error {
        return PreflightResult::failure("pi", ErrorKind::Other, error, duration_ms);
    }

    if !saw_agent_end {
        return PreflightResult::failure(
            "pi",
            ErrorKind::Other,
            "pi stream ended without agent_end",
            duration_ms,
        );
    }

    if collected_text.contains("GITIM_OK") {
        PreflightResult::success(
            "pi",
            None,
            overrides.model_override.clone(),
            duration_ms,
            Some(truncate(&collected_text, PREVIEW_TRUNCATE)),
        )
    } else {
        PreflightResult::failure(
            "pi",
            ErrorKind::Other,
            "response did not contain GITIM_OK",
            duration_ms,
        )
    }
}

/// Run a real-hello ping against the Pi CLI at `bin`.
///
/// Thin wrapper over [`preflight_pi_with_config`] preserved for the
/// `/preflight/{provider}` HTTP route and tests that don't need agent-aware
/// overrides. New call sites that have access to an agent's env should prefer
/// [`preflight_pi_with_config`] directly.
pub async fn preflight_pi_with(bin: &str, timeout: Duration) -> PreflightResult {
    preflight_pi_with_config(bin, timeout, PreflightOverrides::default()).await
}

/// Run a real-hello preflight against the default `pi` binary.
pub async fn preflight_pi() -> PreflightResult {
    preflight_pi_with("pi", Duration::from_secs(60)).await
}

/// Preflight for the Hermes ACP provider.
///
/// Two modes depending on `llm_provider` / `llm_model`:
///
/// **ACP mode** (both are `None`): Spawns `hermes acp` and performs the ACP
/// initialize handshake. A valid response containing `authMethods` is treated
/// as "available". This proves both CLI existence and ACP server responsiveness
/// without sending a full prompt (avoiding token spend during preflight).
///
/// **Chat mode** (both are `Some`): Spawns `hermes chat --provider <X> --model
/// <Y> "Reply with: GITIM_OK"` on the default profile and looks for `GITIM_OK`
/// in stdout. This validates that the specified (provider, model) pair can
/// handshake using the default profile's credentials before an agent profile
/// commits to that configuration. Only one of the two params being `Some` is
/// treated as both `None` (chat mode requires both or neither).
///
/// `hermes_home`, when set, is injected as `HERMES_HOME` into the spawned
/// process so the call exercises a specific profile (e.g.
/// `~/.hermes/profiles/gitim-<handler>/`) rather than the default profile.
/// `None` preserves the inherited environment.
///
/// `env_override`, when set, is merged into the child process env (overriding
/// inherited values with the same key, and applied AFTER `HERMES_HOME` so a
/// caller-supplied `HERMES_HOME` in `env_override` wins). Used by
/// `preflight_for_add_request` to inject the agent's configured env vars (e.g.
/// `ANTHROPIC_API_KEY`) so the preflight exercises the same credentials the
/// agent will run under. `None` preserves the inherited environment.
pub async fn preflight_hermes_with(
    bin: &str,
    timeout: Duration,
    hermes_home: Option<&Path>,
    llm_provider: Option<&str>,
    llm_model: Option<&str>,
    env_override: Option<HashMap<String, String>>,
) -> PreflightResult {
    match (llm_provider, llm_model) {
        (Some(provider), Some(model)) => {
            preflight_hermes_chat(
                bin,
                timeout,
                hermes_home,
                provider,
                model,
                env_override.as_ref(),
            )
            .await
        }
        _ => preflight_hermes_acp(bin, timeout, hermes_home, env_override.as_ref()).await,
    }
}

/// Inner: ACP initialize handshake (no LLM call).
async fn preflight_hermes_acp(
    bin: &str,
    timeout: Duration,
    hermes_home: Option<&Path>,
    env_override: Option<&HashMap<String, String>>,
) -> PreflightResult {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let started = Instant::now();

    // Version check via `hermes --version` (sync, fast).
    let version = Command::new(bin)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout);
            // "Hermes Agent v0.10.0 …" → extract the version token after 'v'
            s.split_whitespace()
                .find(|t| t.starts_with('v'))
                .map(|t| t.trim_start_matches('v').to_string())
        });

    let mut cmd = tokio::process::Command::new(bin);
    cmd.arg("acp")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    if let Some(home) = hermes_home {
        cmd.env("HERMES_HOME", home);
    }
    if let Some(env) = env_override {
        cmd.envs(env);
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let kind = map_spawn_error(&e);
            let msg = if kind == ErrorKind::NotInstalled {
                format!("hermes CLI not found at `{bin}`: {e}")
            } else {
                format!("failed to spawn hermes acp: {e}")
            };
            return PreflightResult::failure(
                "hermes",
                kind,
                msg,
                started.elapsed().as_millis() as u64,
            );
        }
    };

    let mut stdin = child.stdin.take().expect("stdin piped");
    let stdout = child.stdout.take().expect("stdout piped");
    let mut reader = BufReader::new(stdout).lines();

    // Send ACP initialize and wait for a valid response.
    let handshake = async {
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": {
                "protocolVersion": 1,
                "clientInfo": {"name": "gitim-preflight", "version": "0.1.0"},
                "clientCapabilities": {},
            }
        });
        let mut buf = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
        buf.push(b'\n');
        stdin
            .write_all(&buf)
            .await
            .map_err(|e| format!("stdin write: {e}"))?;

        loop {
            match reader.next_line().await {
                Ok(Some(line)) => {
                    let line = line.trim().to_string();
                    if line.is_empty() {
                        continue;
                    }
                    let v: serde_json::Value =
                        serde_json::from_str(&line).map_err(|e| format!("parse: {e}"))?;
                    if v.get("id").and_then(|id| id.as_u64()) == Some(0) {
                        if v.get("error").is_some() {
                            return Err(format!(
                                "initialize error: {}",
                                v["error"]["message"].as_str().unwrap_or("unknown")
                            ));
                        }
                        // Verify authMethods is present — proves ACP server is ready.
                        let _ = v["result"]["authMethods"]
                            .as_array()
                            .ok_or("authMethods missing from initialize response")?;
                        return Ok(());
                    }
                }
                Ok(None) => return Err("ACP stream ended before initialize response".to_string()),
                Err(e) => return Err(format!("stdout read: {e}")),
            }
        }
    };

    let result = tokio::time::timeout(timeout, handshake).await;
    let _ = child.start_kill();

    match result {
        Ok(Ok(())) => PreflightResult::success(
            "hermes",
            version,
            None,
            started.elapsed().as_millis() as u64,
            Some("ACP initialize OK".to_string()),
        ),
        Ok(Err(e)) => PreflightResult::failure(
            "hermes",
            ErrorKind::Other,
            format!("ACP handshake failed: {e}"),
            started.elapsed().as_millis() as u64,
        ),
        Err(_) => PreflightResult::failure(
            "hermes",
            ErrorKind::Timeout,
            format!("hermes preflight exceeded {}ms", timeout.as_millis()),
            started.elapsed().as_millis() as u64,
        ),
    }
}

/// Inner: chat-based preflight with explicit provider + model override.
///
/// Spawns `hermes chat --provider <provider> --model <model> "Reply with:
/// GITIM_OK"` and verifies the response contains "GITIM_OK". Used by the
/// `/preflight/hermes?llm_provider=X&llm_model=Y` endpoint to validate a
/// (provider, model) pair against the default profile's credentials before
/// committing an agent profile.
async fn preflight_hermes_chat(
    bin: &str,
    timeout: Duration,
    hermes_home: Option<&Path>,
    llm_provider: &str,
    llm_model: &str,
    env_override: Option<&HashMap<String, String>>,
) -> PreflightResult {
    let started = Instant::now();

    let mut cmd = tokio::process::Command::new(bin);
    cmd.arg("chat")
        .args(["--provider", llm_provider])
        .args(["--model", llm_model])
        // -Q suppresses the interactive banner/spinner so stdout is just the response.
        .arg("-Q")
        .args(["--query", "Reply with exactly: GITIM_OK"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(home) = hermes_home {
        cmd.env("HERMES_HOME", home);
    }
    if let Some(env) = env_override {
        cmd.envs(env);
    }

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let kind = map_spawn_error(&e);
            let msg = if kind == ErrorKind::NotInstalled {
                format!("hermes CLI not found at `{bin}`: {e}")
            } else {
                format!("failed to spawn hermes chat: {e}")
            };
            return PreflightResult::failure(
                "hermes",
                kind,
                msg,
                started.elapsed().as_millis() as u64,
            );
        }
    };

    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            return PreflightResult::failure(
                "hermes",
                ErrorKind::Other,
                format!("hermes IO error: {e}"),
                started.elapsed().as_millis() as u64,
            );
        }
        Err(_) => {
            return PreflightResult::failure(
                "hermes",
                ErrorKind::Timeout,
                format!("hermes preflight exceeded {}ms", timeout.as_millis()),
                started.elapsed().as_millis() as u64,
            );
        }
    };

    let duration_ms = started.elapsed().as_millis() as u64;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let trimmed = truncate(stderr.trim(), STDERR_TRUNCATE);
        let msg = if trimmed.is_empty() {
            format!("hermes exited with status {}", output.status)
        } else {
            format!("hermes exited with status {}: {}", output.status, trimmed)
        };
        return PreflightResult::failure("hermes", ErrorKind::Other, msg, duration_ms);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let text = stdout.trim();

    if text.contains("GITIM_OK") {
        PreflightResult::success(
            "hermes",
            None,
            Some(format!("{llm_provider}/{llm_model}")),
            duration_ms,
            Some(truncate(text, PREVIEW_TRUNCATE)),
        )
    } else {
        PreflightResult::failure(
            "hermes",
            ErrorKind::Other,
            "response did not contain GITIM_OK",
            duration_ms,
        )
    }
}

/// Run preflight against the default `hermes` binary against the user's
/// active profile (no `HERMES_HOME` override, no LLM override).
pub async fn preflight_hermes() -> PreflightResult {
    preflight_hermes_with("hermes", Duration::from_secs(30), None, None, None, None).await
}

/// Read `(provider, model)` from the user's hermes default profile config.
///
/// Resolves the hermes home directory from the `HERMES_HOME` env var (falling
/// back to `~/.hermes`), then reads `<hermes_home>/config.yaml` and extracts
/// `model.provider` and `model.default`. Used by `preflight_for_add_request`
/// when the agent add request omits both `llm_provider` and `llm_model` —
/// the runtime resolves the default-profile pair and forwards it to chat-mode
/// preflight so we exercise the same configuration the agent will inherit.
///
/// Returns `None` if any of: the config file is missing, parsing fails, or
/// either field is absent / non-string. Returning `None` lets callers fall
/// back to ACP-mode preflight rather than fail the add outright.
///
/// **Schema note**: the hermes `config.yaml` shape is owned by hermes; this
/// function parses only the two fields it needs to avoid coupling to hermes
/// internal layout. If hermes renames or restructures these keys we'll surface
/// `None` and the caller will degrade to ACP-mode preflight.
pub fn read_default_profile_llm() -> Option<(String, String)> {
    let home = std::env::var_os("HERMES_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".hermes")))?;
    read_default_profile_llm_from(&home)
}

/// Testable variant of [`read_default_profile_llm`] with an explicit
/// `hermes_home` directory. The env-aware version resolves the directory
/// then calls this. Tests can call this directly to avoid mutating
/// `HERMES_HOME` (which is process-global and races under cargo's
/// multi-threaded runner).
pub fn read_default_profile_llm_from(hermes_home: &Path) -> Option<(String, String)> {
    let config_path = hermes_home.join("config.yaml");
    let content = std::fs::read_to_string(&config_path).ok()?;
    let root: serde_yaml::Value = serde_yaml::from_str(&content).ok()?;
    let model = root.get("model")?;
    let provider = model.get("provider").and_then(|v| v.as_str())?.to_string();
    let default_model = model.get("default").and_then(|v| v.as_str())?.to_string();
    Some((provider, default_model))
}

// ─── Entry point: add-time preflight dispatch ────────────────────────────────
//
// `preflight_for_add_request` is the single entry the add-agent path calls
// after `handler_conflict` clears and before `provision_agent` commits any
// artifacts. It dispatches on `provider` to the appropriate `_with_config`
// helper, threading the agent's own env/model into the spawned CLI so the
// verification matches the configuration the agent will actually run under.
//
// Setup-level failures (unknown provider, hermes missing one LLM half,
// hermes no LLM in default profile) are tagged via `PreflightResult.failure_code`
// so the caller can pick a more specific HTTP `error_code` than the generic
// `provision_preflight_failed`.

/// Hard ceiling on a single preflight call. Tighter than runtime's
/// LONG_REQUEST_TIMEOUT (300s) because the add-agent request needs to feel
/// responsive — but loose enough that a real Claude/Codex hello with a slow
/// upstream still completes. Per requirements §4.
pub const PROVIDER_PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(90);

/// Default binary paths (PATH-resolved at spawn time). Tests use the
/// `_test` entry seam below to inject fake binaries without mutating PATH.
const DEFAULT_BIN_CLAUDE: &str = "claude";
const DEFAULT_BIN_CODEX: &str = "codex";
const DEFAULT_BIN_OPENCODE: &str = "opencode";
const DEFAULT_BIN_PI: &str = "pi";
const DEFAULT_BIN_HERMES: &str = "hermes";

/// Setup-level failure tags, attached to `PreflightResult.failure_code` by
/// [`preflight_for_add_request`] and consumed by
/// [`classify_preflight_error_code`]. Kept as `pub const`s so callers in
/// http.rs / CLI can match against the same string instead of typing it
/// inline at each site.
pub const FAILURE_CODE_UNKNOWN_PROVIDER: &str = "unknown_provider";
pub const FAILURE_CODE_MISSING_LLM_PROVIDER: &str = "missing_llm_provider";
pub const FAILURE_CODE_HERMES_NO_LLM: &str = "hermes_default_profile_no_llm";

/// Default HTTP top-level error_code for any preflight failure that didn't
/// arrive pre-tagged with a more specific [`failure_code`](PreflightResult::failure_code).
pub const ERROR_CODE_PROVISION_PREFLIGHT_FAILED: &str = "provision_preflight_failed";

/// Test-only seam for [`preflight_for_add_request`]. Lets tests inject:
/// - alternative binary paths (fake shell scripts) without mutating PATH, and
/// - a tighter outer timeout so timeout-classification can be exercised in
///   under a second without sleeping `PROVIDER_PREFLIGHT_TIMEOUT` (90s) live.
/// - an explicit `hermes_home` so the default-profile YAML resolution exercises
///   a test-controlled directory rather than reading `$HOME/.hermes`.
///
/// Production callers should use [`preflight_for_add_request`] which wires
/// `PreflightDispatchOverrides::default()`.
#[derive(Debug, Clone, Default)]
pub struct PreflightDispatchOverrides {
    pub claude_bin: Option<String>,
    pub codex_bin: Option<String>,
    pub opencode_bin: Option<String>,
    pub pi_bin: Option<String>,
    pub hermes_bin: Option<String>,
    pub outer_timeout: Option<Duration>,
    pub hermes_home: Option<PathBuf>,
}

/// Add-time preflight entry point: dispatch on provider, thread the agent's
/// env/model into the spawned CLI, and classify setup-level failures with
/// stable tags.
///
/// Behavior summary:
/// - `mock` → short-circuit `success` (no spawn). Used by tests and the mock
///   provider that has no CLI binary at all.
/// - `claude` / `codex` / `opencode` / `pi` → spawn the CLI with `env` and
///   `model` overrides where the provider exposes a per-invocation model flag.
/// - `hermes` → branch on `(llm_provider, llm_model)`:
///   - both `Some` → chat-mode preflight with the explicit pair.
///   - both `None` → read default profile's `config.yaml`; if it has a
///     `(provider, model)`, dispatch to chat-mode with that pair; otherwise
///     return `failure_code = "hermes_default_profile_no_llm"`.
///   - exactly one `Some` → `failure_code = "missing_llm_provider"` (no spawn).
/// - anything else → `failure_code = "unknown_provider"` (no spawn).
///
/// The whole dispatch is wrapped in `tokio::time::timeout` at
/// [`PROVIDER_PREFLIGHT_TIMEOUT`]. A trip there produces
/// `error_kind = Timeout` with no `failure_code` (a generic provision failure,
/// not a setup-level one).
pub async fn preflight_for_add_request(
    provider: &str,
    env: Option<&HashMap<String, String>>,
    model: Option<&str>,
    llm_provider: Option<&str>,
    llm_model: Option<&str>,
) -> PreflightResult {
    preflight_for_add_request_with_overrides(
        provider,
        env,
        model,
        llm_provider,
        llm_model,
        PreflightDispatchOverrides::default(),
    )
    .await
}

/// Same as [`preflight_for_add_request`] but accepts the test seam struct so
/// tests can inject fake binaries and tighter timeouts without touching PATH.
pub async fn preflight_for_add_request_with_overrides(
    provider: &str,
    env: Option<&HashMap<String, String>>,
    model: Option<&str>,
    llm_provider: Option<&str>,
    llm_model: Option<&str>,
    overrides: PreflightDispatchOverrides,
) -> PreflightResult {
    let outer_timeout = overrides
        .outer_timeout
        .unwrap_or(PROVIDER_PREFLIGHT_TIMEOUT);

    // We let the inner provider helpers manage their own per-call timeout
    // (they all use Duration::from_secs(60) when called via _with), but the
    // outer wrap is the hard cap that guarantees the add-agent request never
    // hangs past PROVIDER_PREFLIGHT_TIMEOUT regardless of inner behavior.
    let started_outer = Instant::now();
    let inner = dispatch_preflight(provider, env, model, llm_provider, llm_model, &overrides);

    match tokio::time::timeout(outer_timeout, inner).await {
        Ok(result) => result,
        Err(_) => PreflightResult::failure(
            provider,
            ErrorKind::Timeout,
            format!(
                "provisioning preflight exceeded {}ms",
                outer_timeout.as_millis()
            ),
            started_outer.elapsed().as_millis() as u64,
        ),
    }
}

/// Internal dispatcher. Kept separate so the outer
/// `preflight_for_add_request_with_overrides` can wrap the whole call in a
/// single `tokio::time::timeout` regardless of which provider branch fires.
async fn dispatch_preflight(
    provider: &str,
    env: Option<&HashMap<String, String>>,
    model: Option<&str>,
    llm_provider: Option<&str>,
    llm_model: Option<&str>,
    overrides: &PreflightDispatchOverrides,
) -> PreflightResult {
    // Standard env/model overrides bundle used by claude/codex/opencode/pi.
    let prov_overrides = PreflightOverrides {
        env_override: env.cloned(),
        model_override: model.map(String::from),
    };

    // Inner per-provider timeout — same 60s the zero-arg variants use. The
    // outer 90s wrap is the hard cap.
    let inner_timeout = Duration::from_secs(60);

    match provider {
        "mock" => {
            // The mock provider has no CLI to verify; short-circuit so the
            // add-agent path is testable end-to-end without any binaries.
            PreflightResult::success(
                "mock",
                None,
                model.map(String::from),
                0,
                Some("mock provider — preflight skipped".to_string()),
            )
        }
        "claude" => {
            let bin = overrides
                .claude_bin
                .as_deref()
                .unwrap_or(DEFAULT_BIN_CLAUDE);
            preflight_claude_with_config(bin, inner_timeout, prov_overrides).await
        }
        "codex" => {
            let bin = overrides.codex_bin.as_deref().unwrap_or(DEFAULT_BIN_CODEX);
            preflight_codex_with_config(bin, inner_timeout, prov_overrides).await
        }
        "opencode" => {
            let bin = overrides
                .opencode_bin
                .as_deref()
                .unwrap_or(DEFAULT_BIN_OPENCODE);
            preflight_opencode_with_config(bin, inner_timeout, prov_overrides).await
        }
        "pi" => {
            let bin = overrides.pi_bin.as_deref().unwrap_or(DEFAULT_BIN_PI);
            preflight_pi_with_config(bin, inner_timeout, prov_overrides).await
        }
        "hermes" => {
            let bin = overrides
                .hermes_bin
                .as_deref()
                .unwrap_or(DEFAULT_BIN_HERMES);
            dispatch_hermes(
                bin,
                inner_timeout,
                env.cloned(),
                llm_provider,
                llm_model,
                overrides.hermes_home.as_deref(),
            )
            .await
        }
        other => PreflightResult::failure_with_code(
            other,
            ErrorKind::Other,
            format!("unknown provider: {other}"),
            0,
            FAILURE_CODE_UNKNOWN_PROVIDER,
        ),
    }
}

/// Hermes-specific dispatch logic, separated for readability.
/// Resolves the `(llm_provider, llm_model)` pair (explicit > default profile)
/// or returns a tagged failure when the pair can't be satisfied.
async fn dispatch_hermes(
    bin: &str,
    timeout: Duration,
    env: Option<HashMap<String, String>>,
    llm_provider: Option<&str>,
    llm_model: Option<&str>,
    hermes_home: Option<&Path>,
) -> PreflightResult {
    match (llm_provider, llm_model) {
        (Some(p), Some(m)) => {
            preflight_hermes_with(bin, timeout, hermes_home, Some(p), Some(m), env).await
        }
        (None, None) => {
            // Both omitted — fall back to default-profile config.yaml.
            // When tests pass an explicit hermes_home use that; otherwise
            // resolve from HERMES_HOME / ~/.hermes.
            let resolved = match hermes_home {
                Some(p) => read_default_profile_llm_from(p),
                None => read_default_profile_llm(),
            };
            match resolved {
                Some((p, m)) => {
                    preflight_hermes_with(bin, timeout, hermes_home, Some(&p), Some(&m), env).await
                }
                None => PreflightResult::failure_with_code(
                    "hermes",
                    ErrorKind::Other,
                    "no LLM configured in default hermes profile (model.default + \
                     model.provider both required in <hermes_home>/config.yaml)",
                    0,
                    FAILURE_CODE_HERMES_NO_LLM,
                ),
            }
        }
        // Exactly one of llm_provider / llm_model supplied — runtime contract
        // requires both or neither.
        _ => PreflightResult::failure_with_code(
            "hermes",
            ErrorKind::Other,
            "llm_provider and llm_model must be specified together",
            0,
            FAILURE_CODE_MISSING_LLM_PROVIDER,
        ),
    }
}

/// Map a [`PreflightResult`] into the stable HTTP top-level `error_code`
/// the add-agent error envelope should carry.
///
/// Returns:
/// - the value of `pf.failure_code` if it's one of the known setup-level tags
///   (currently: `unknown_provider`, `missing_llm_provider`,
///   `hermes_default_profile_no_llm`).
/// - [`ERROR_CODE_PROVISION_PREFLIGHT_FAILED`] otherwise — covers spawn
///   failures, exit-1 from the CLI, timeouts, malformed JSON output, etc.
///
/// Returning `&'static str` keeps the value cheap to embed in `ErrorBody`
/// and forces the closed-set discipline: any new tag must be added here.
pub fn classify_preflight_error_code(pf: &PreflightResult) -> &'static str {
    match pf.failure_code.as_deref() {
        Some(FAILURE_CODE_UNKNOWN_PROVIDER) => FAILURE_CODE_UNKNOWN_PROVIDER,
        Some(FAILURE_CODE_MISSING_LLM_PROVIDER) => FAILURE_CODE_MISSING_LLM_PROVIDER,
        Some(FAILURE_CODE_HERMES_NO_LLM) => FAILURE_CODE_HERMES_NO_LLM,
        _ => ERROR_CODE_PROVISION_PREFLIGHT_FAILED,
    }
}

/// Concatenate all `text` part payloads from opencode's NDJSON stream.
fn extract_opencode_text(stdout: &str) -> String {
    let mut out = String::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(val): Result<serde_json::Value, _> = serde_json::from_str(line) else {
            continue;
        };
        if val.get("type").and_then(|t| t.as_str()) != Some("text") {
            continue;
        }
        if let Some(text) = val
            .get("part")
            .and_then(|p| p.get("text"))
            .and_then(|t| t.as_str())
        {
            out.push_str(text);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preflight_result_success_serializes_with_stable_shape() {
        let result = PreflightResult::success(
            "claude",
            None,
            Some("claude-haiku-4-5".to_string()),
            2140,
            Some("GITIM_OK".to_string()),
        );
        let v: serde_json::Value = serde_json::to_value(&result).unwrap();

        assert_eq!(v["available"], serde_json::Value::Bool(true));
        assert_eq!(v["provider"], serde_json::Value::String("claude".into()));
        assert_eq!(v["version"], serde_json::Value::Null);
        assert_eq!(
            v["model_used"],
            serde_json::Value::String("claude-haiku-4-5".into())
        );
        assert_eq!(v["duration_ms"], serde_json::Value::Number(2140.into()));
        assert_eq!(
            v["output_preview"],
            serde_json::Value::String("GITIM_OK".into())
        );
        assert_eq!(v["error"], serde_json::Value::Null);
        assert_eq!(v["error_kind"], serde_json::Value::Null);

        // Null fields must be present (not elided) so the frontend can branch.
        let obj = v.as_object().unwrap();
        for key in [
            "available",
            "provider",
            "version",
            "model_used",
            "duration_ms",
            "output_preview",
            "error",
            "error_kind",
        ] {
            assert!(obj.contains_key(key), "missing key: {key}");
        }
    }

    #[test]
    fn preflight_result_failure_serializes_error_kind_as_snake_case() {
        let result = PreflightResult::failure(
            "codex",
            ErrorKind::NotInstalled,
            "codex not found in PATH",
            35,
        );
        let v: serde_json::Value = serde_json::to_value(&result).unwrap();

        assert_eq!(v["available"], serde_json::Value::Bool(false));
        assert_eq!(v["provider"], serde_json::Value::String("codex".into()));
        assert_eq!(v["duration_ms"], serde_json::Value::Number(35.into()));
        assert_eq!(
            v["error"],
            serde_json::Value::String("codex not found in PATH".into())
        );
        assert_eq!(
            v["error_kind"],
            serde_json::Value::String("not_installed".into())
        );
    }

    #[test]
    fn error_kind_variants_serialize_as_snake_case() {
        assert_eq!(
            serde_json::to_value(ErrorKind::NotInstalled).unwrap(),
            serde_json::Value::String("not_installed".into())
        );
        assert_eq!(
            serde_json::to_value(ErrorKind::Timeout).unwrap(),
            serde_json::Value::String("timeout".into())
        );
        assert_eq!(
            serde_json::to_value(ErrorKind::Other).unwrap(),
            serde_json::Value::String("other".into())
        );
    }

    #[test]
    fn test_map_spawn_error_not_found() {
        let err = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert_eq!(map_spawn_error(&err), ErrorKind::NotInstalled);
    }

    #[test]
    fn test_map_spawn_error_other() {
        // PermissionDenied is a representative non-NotFound kind; anything
        // that isn't NotFound should funnel into ErrorKind::Other.
        let err = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert_eq!(map_spawn_error(&err), ErrorKind::Other);

        let err = std::io::Error::from(std::io::ErrorKind::Other);
        assert_eq!(map_spawn_error(&err), ErrorKind::Other);
    }

    #[test]
    fn parse_claude_result_array_shape() {
        // Regression: older CLI versions emit an array of event objects.
        let stdout = r#"[
            {"type": "system", "subtype": "init"},
            {"type": "assistant", "message": "..."},
            {"type": "result", "result": "GITIM_OK", "is_error": false}
        ]"#;
        let text = parse_claude_result(stdout).expect("array shape should parse");
        assert_eq!(text, "GITIM_OK");
    }

    #[test]
    fn parse_claude_result_single_object_shape() {
        // P0 fix: newer CLI versions emit a single object directly rather
        // than wrapping it in an array. Both shapes must resolve identically.
        let stdout = r#"{"type": "result", "result": "GITIM_OK", "is_error": false}"#;
        let text = parse_claude_result(stdout).expect("single-object shape should parse");
        assert_eq!(text, "GITIM_OK");
    }

    #[test]
    fn parse_claude_result_missing_result_entry() {
        // Neither an array with no result entry nor an unrelated single
        // object should spuriously succeed.
        let arr = r#"[{"type": "system"}]"#;
        assert!(parse_claude_result(arr).is_err());

        let obj = r#"{"type": "assistant", "message": "hi"}"#;
        assert!(parse_claude_result(obj).is_err());
    }

    #[test]
    fn parse_claude_result_invalid_json() {
        let err = parse_claude_result("not json").unwrap_err();
        assert!(err.contains("failed to parse"));
    }

    #[test]
    fn extract_opencode_text_concatenates_text_parts() {
        let stdout = r#"
{"type":"step_start","sessionID":"s1","part":{}}
{"type":"text","sessionID":"s1","part":{"text":"GITIM_"}}
{"type":"text","sessionID":"s1","part":{"text":"OK"}}
{"type":"step_finish","sessionID":"s1","part":{}}
"#;
        assert_eq!(extract_opencode_text(stdout), "GITIM_OK");
    }

    #[test]
    fn extract_opencode_text_ignores_non_text_lines() {
        let stdout = r#"
not json
{"type":"tool_use","part":{}}
{"type":"text","part":{"text":"hello"}}
"#;
        assert_eq!(extract_opencode_text(stdout), "hello");
    }

    // ── failure_code field + classify mapping ───────────────────────────────

    #[test]
    fn failure_code_field_is_skipped_when_none() {
        // Default constructors leave failure_code None; the field must NOT
        // appear in the JSON output. This is the legacy shape — older
        // clients (frontend, CLI DTO) read the body before the field
        // existed and must keep working unchanged.
        let pf = PreflightResult::failure("claude", ErrorKind::NotInstalled, "not found", 0);
        let v: serde_json::Value = serde_json::to_value(&pf).unwrap();
        assert!(
            !v.as_object().unwrap().contains_key("failure_code"),
            "failure_code should be omitted when None, got: {v}"
        );
    }

    #[test]
    fn failure_with_code_serializes_failure_code() {
        let pf = PreflightResult::failure_with_code(
            "hermes",
            ErrorKind::Other,
            "no LLM in default profile",
            0,
            FAILURE_CODE_HERMES_NO_LLM,
        );
        let v: serde_json::Value = serde_json::to_value(&pf).unwrap();
        assert_eq!(
            v["failure_code"],
            serde_json::Value::String(FAILURE_CODE_HERMES_NO_LLM.into())
        );
    }

    #[test]
    fn classify_returns_known_tags_unchanged() {
        let pf = PreflightResult::failure_with_code(
            "x",
            ErrorKind::Other,
            "",
            0,
            FAILURE_CODE_UNKNOWN_PROVIDER,
        );
        assert_eq!(
            classify_preflight_error_code(&pf),
            FAILURE_CODE_UNKNOWN_PROVIDER
        );
    }

    #[test]
    fn classify_falls_through_to_provision_preflight_failed() {
        // No failure_code → generic.
        let pf = PreflightResult::failure("claude", ErrorKind::Timeout, "slow", 1000);
        assert_eq!(
            classify_preflight_error_code(&pf),
            ERROR_CODE_PROVISION_PREFLIGHT_FAILED
        );
        // Unknown failure_code value → still falls through (defensive).
        let pf2 = PreflightResult::failure_with_code(
            "x",
            ErrorKind::Other,
            "",
            0,
            "novel-tag-not-yet-mapped",
        );
        assert_eq!(
            classify_preflight_error_code(&pf2),
            ERROR_CODE_PROVISION_PREFLIGHT_FAILED
        );
    }
}
