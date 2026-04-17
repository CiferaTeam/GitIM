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
const PEERS: &[(&str, &str)] = &[
    ("gitim", "gitim"),
    ("gitim-daemon", "gitim-daemon"),
];

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
    let output = Command::new(binary_path)
        .arg("--version")
        .output()
        .ok()?;
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
        Err(PreflightError { missing, mismatches })
    }
}

/// Check if Claude CLI is available and return its version.
pub async fn check_claude() -> Result<String, String> {
    let output = tokio::process::Command::new("claude")
        .arg("--version")
        .output()
        .await
        .map_err(|e| format!("claude not found: {e}"))?;

    if !output.status.success() {
        return Err("claude --version exited with non-zero status".to_string());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let version = stdout.trim().to_string();
    if version.is_empty() {
        return Err("claude --version returned empty output".to_string());
    }
    Ok(version)
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
        .args([
            "--system-prompt",
            "Reply with exactly what the user asks.",
        ])
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
            let kind = if e.kind() == std::io::ErrorKind::NotFound {
                ErrorKind::NotInstalled
            } else {
                ErrorKind::Other
            };
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

    // `claude --print --output-format json` returns a JSON array. Scan for
    // the `type == "result"` entry and read its `result` field.
    let parsed: Result<Vec<serde_json::Value>, _> = serde_json::from_str(trimmed_stdout);
    let items = match parsed {
        Ok(v) => v,
        Err(e) => {
            return PreflightResult::failure(
                "claude",
                ErrorKind::Other,
                format!("failed to parse claude JSON output: {e}"),
                duration_ms,
            );
        }
    };

    let result_text = items
        .iter()
        .find(|item| item.get("type").and_then(|t| t.as_str()) == Some("result"))
        .and_then(|item| item.get("result"))
        .and_then(|r| r.as_str())
        .map(|s| s.to_string());

    let text = match result_text {
        Some(t) => t,
        None => {
            return PreflightResult::failure(
                "claude",
                ErrorKind::Other,
                "claude JSON output did not contain a result entry with a `result` field",
                duration_ms,
            );
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
}
