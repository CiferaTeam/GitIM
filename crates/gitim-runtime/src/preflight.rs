//! Preflight module: real-hello CLI verification.
//! Used by /preflight/{provider} HTTP endpoint.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

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
        }
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

/// Run a real-hello ping against the Claude CLI at `bin`.
///
/// Returns a `PreflightResult` that captures the outcome with a stable error
/// taxonomy (`NotInstalled` / `Timeout` / `Other`). Split from
/// [`preflight_claude`] so tests can inject fake binaries (e.g. `/bin/false`,
/// a stalling shell script) to exercise each error branch without needing a
/// logged-in Claude CLI.
pub async fn preflight_claude_with(bin: &str, timeout: Duration) -> PreflightResult {
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

    let mut cmd = tokio::process::Command::new(bin);
    cmd.current_dir(tmpdir.path())
        .arg("--print")
        .args(["--model", CLAUDE_PREFLIGHT_MODEL])
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
            Some(CLAUDE_PREFLIGHT_MODEL.to_string()),
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

/// Run a real-hello preflight against the default `claude` binary.
///
/// Spawns `claude --print` with minimal-context flags, sends a fixed prompt,
/// and returns a classified [`PreflightResult`]. Used by the HTTP preflight
/// route once Task 5 wires it up.
pub async fn preflight_claude() -> PreflightResult {
    preflight_claude_with("claude", Duration::from_secs(60)).await
}

/// Model forced during the codex preflight ping. Same rationale as
/// [`CLAUDE_PREFLIGHT_MODEL`]: predictable response-time and cost.
const CODEX_PREFLIGHT_MODEL: &str = "gpt-5.4-mini";

/// Run a real-hello ping against the Codex CLI at `bin`.
///
/// Mirrors [`preflight_claude_with`]: isolates cwd, spawns the CLI with a
/// fixed prompt, enforces a timeout, and classifies the result with the same
/// `NotInstalled` / `Timeout` / `Other` taxonomy.
///
/// The shape diverges in two places:
/// 1. Codex accepts `Stdio::null()` on stdin (claude needs a piped EOF).
/// 2. `codex exec --json` emits JSONL (one JSON object per line), not a
///    single JSON array. We scan for `turn.completed` (the stream terminator)
///    and extract the `agent_message` text from the matching `item.completed`.
pub async fn preflight_codex_with(bin: &str, timeout: Duration) -> PreflightResult {
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

    let mut cmd = tokio::process::Command::new(bin);
    cmd.current_dir(tmpdir.path())
        .arg("exec")
        .arg("--json")
        // Our tempdir is isolation-by-design, not a git repo. Without
        // `--skip-git-repo-check`, codex refuses to run with "Not inside a
        // trusted directory".
        .arg("--skip-git-repo-check")
        .args(["--model", CODEX_PREFLIGHT_MODEL])
        .arg("Reply with exactly: GITIM_OK")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

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
            Some(CODEX_PREFLIGHT_MODEL.to_string()),
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

/// Run a real-hello preflight against the default `codex` binary.
///
/// Spawns `codex exec --json`, scans the JSONL stream for the agent message,
/// and returns a classified [`PreflightResult`].
pub async fn preflight_codex() -> PreflightResult {
    preflight_codex_with("codex", Duration::from_secs(60)).await
}

/// Run a real-hello ping against the opencode CLI at `bin`.
///
/// Unlike claude/codex where we force a cheap model, opencode uses whatever
/// model the user authenticated with via `opencode auth login`. We cannot
/// predict that at preflight time, so we accept the variance. System prompt
/// is injected via OPENCODE_CONFIG_CONTENT as a minimal echo agent to keep
/// the request cheap and deterministic.
pub async fn preflight_opencode_with(bin: &str, timeout: Duration) -> PreflightResult {
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

    let mut cmd = tokio::process::Command::new(bin);
    cmd.current_dir(tmpdir.path())
        .args([
            "run",
            "--format",
            "json",
            "--dangerously-skip-permissions",
            "--agent",
            "gitim_preflight",
            "--",
            "Reply with exactly: GITIM_OK",
        ])
        .env("OPENCODE_CONFIG_CONTENT", &config_content)
        .env("OPENCODE_PERMISSION", r#"{"*":"allow"}"#)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

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
            None, // model_used = whatever user auth'd; unknown at CLI level
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

/// Run a real-hello preflight against the default `opencode` binary.
pub async fn preflight_opencode() -> PreflightResult {
    preflight_opencode_with("opencode", Duration::from_secs(60)).await
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
}
