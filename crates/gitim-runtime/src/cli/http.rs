//! HTTP client + base URL discovery + error classification for the runtime CLI.
//!
//! Architecture context (see `docs/plans/runtime-cli/00-requirements.md` §1, §4, §5):
//!
//! - Port discovery follows a strict priority: `--port` flag → `GITIM_RUNTIME_PORT`
//!   env → persisted `listen_port` in `~/.gitim/runtime.json` → `DEFAULT_PORT`.
//! - Error classification keys off the response body's `error_code` field, NOT
//!   HTTP status alone. The runtime sometimes returns HTTP 200 with
//!   `{ok:false, error_code:"..."}` for permanent errors, and sometimes 4xx +
//!   error_code; both map to `CliError::ResponseErrorCode` so the agent can
//!   distinguish permanent (don't retry) from transient (may retry) failures.
//! - HTTP 5xx is unconditionally transient; HTTP 4xx without `error_code` is
//!   treated as permanent (covered downstream by `exit_code::from_cli_error`).
//!
//! The verb helpers (`get`/`post`/`patch`) are async because reqwest in this
//! crate is built without the blocking feature — keeps a single runtime model
//! across server and CLI modes.
//!
//! Subcommand handlers in tasks 6-12 build on this surface; this module is the
//! shared seam, no per-command knowledge lives here.

use crate::http::DEFAULT_PORT;
use crate::user_config;

/// Cap response-body excerpts in error messages so a misbehaving server can't
/// blow stderr up to multi-megabyte log lines. 512 bytes is enough to keep a
/// JSON error payload mostly intact for debugging.
const BODY_EXCERPT_BYTES: usize = 512;

/// Errors the CLI HTTP layer surfaces to the dispatch / exit-code mapper.
///
/// The shape is deliberately flat — each variant maps 1:1 to an exit-code class
/// (see `cli::exit_code::from_cli_error`). Don't merge variants without
/// updating that mapping in lockstep.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// Network failure — connection refused, DNS, TLS, body read error.
    /// Agent should treat as transient (exit 1) and may retry.
    #[error("transport error: {0}")]
    Transport(String),

    /// HTTP error status (4xx or 5xx) where the body did NOT carry an
    /// `error_code`. The excerpt is the first `BODY_EXCERPT_BYTES` of the
    /// response body, useful for human debugging.
    #[error("http {0}: {1}")]
    HttpStatus(u16, String),

    /// Body received but couldn't be parsed as JSON. Indicates a protocol bug
    /// — the runtime should always return JSON; if it doesn't, something is
    /// fundamentally wrong with the deployment.
    #[error("response parse failed: {0}")]
    Parse(String),

    /// The runtime returned a structured error code in the response body.
    /// `code` is the `error_code` string (e.g. "handler_conflict"), `message`
    /// is a human-readable description (from `message` or `error` field if
    /// present), `http_status` is the underlying HTTP status for telemetry.
    ///
    /// Per Architecture §1, this is the canonical signal for permanent
    /// failures and the agent should NOT retry — see `exit_code` mapping.
    #[error("runtime error [{code}]: {message}")]
    ResponseErrorCode {
        code: String,
        message: String,
        http_status: u16,
    },

    /// CLI-side configuration problem — no workspace configured, ambiguous
    /// workspace selection, missing required arg. Maps to exit 1.
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
}

/// Resolve the runtime base URL using the documented priority chain.
///
/// Priority (highest first):
/// 1. `port_arg` from `--port` CLI flag
/// 2. `GITIM_RUNTIME_PORT` env var (must parse as u16)
/// 3. `listen_port` field from `~/.gitim/runtime.json`
/// 4. `DEFAULT_PORT` (16868)
///
/// Returns `http://127.0.0.1:<port>` with no trailing slash. Always uses the
/// loopback IP — runtime is local-only by design.
pub fn resolve_base_url(port_arg: Option<u16>) -> String {
    let port = port_arg
        .or_else(|| {
            std::env::var("GITIM_RUNTIME_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
        })
        .or_else(|| user_config::read().listen_port)
        .unwrap_or(DEFAULT_PORT);
    format!("http://127.0.0.1:{port}")
}

/// Async HTTP client wrapper around `reqwest::Client`. Owns the base URL so
/// callers pass paths (`/workspaces`, `/agents/...`) instead of full URLs.
///
/// One-shot: each verb call creates a fresh request from the shared reqwest
/// client. No connection pooling tuning — the CLI runs one or two requests
/// then exits.
pub struct Client {
    base_url: String,
    inner: reqwest::Client,
}

impl Client {
    pub fn new(base_url: String) -> Self {
        Self {
            base_url,
            inner: reqwest::Client::new(),
        }
    }

    /// GET `<base>/<path>`. See module docs for error classification.
    pub async fn get(&self, path: &str) -> Result<serde_json::Value, CliError> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .inner
            .get(&url)
            .send()
            .await
            .map_err(|e| CliError::Transport(e.to_string()))?;
        process_response(resp).await
    }

    /// POST `<base>/<path>` with JSON body. See module docs for error
    /// classification.
    pub async fn post(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, CliError> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .inner
            .post(&url)
            .json(body)
            .send()
            .await
            .map_err(|e| CliError::Transport(e.to_string()))?;
        process_response(resp).await
    }

    /// PATCH `<base>/<path>` with JSON body. See module docs for error
    /// classification.
    pub async fn patch(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, CliError> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .inner
            .patch(&url)
            .json(body)
            .send()
            .await
            .map_err(|e| CliError::Transport(e.to_string()))?;
        process_response(resp).await
    }
}

/// Centralize the response → CliError mapping in one place so all three verbs
/// share identical semantics. The order matters: 5xx without body inspection
/// is always transient (runtime is broken, not the request), then we try to
/// parse the body to pick out a structured `error_code`, and only fall back
/// to raw 4xx if there's no JSON or no `error_code`.
async fn process_response(resp: reqwest::Response) -> Result<serde_json::Value, CliError> {
    let status = resp.status();
    let status_code = status.as_u16();

    // 5xx is unconditionally transport-class transient. Don't bother parsing
    // — the response is likely not even JSON (could be a proxy error page).
    if status.is_server_error() {
        let body = read_body_excerpt(resp).await;
        return Err(CliError::HttpStatus(status_code, body));
    }

    // 2xx and 4xx: read body fully, then try to parse as JSON. Both flavors
    // can carry `error_code` (Architecture §1), so we treat parsing as the
    // canonical demux step regardless of status.
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| CliError::Transport(e.to_string()))?;

    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => {
            // No JSON. If status was 4xx, surface as HttpStatus with body
            // excerpt — that's the most useful failure mode for the caller.
            // If status was 2xx but body wasn't JSON, that's a parse bug.
            let excerpt = bytes_to_excerpt(&bytes);
            return if status.is_client_error() {
                Err(CliError::HttpStatus(status_code, excerpt))
            } else {
                Err(CliError::Parse(format!("{e}: body excerpt: {excerpt}")))
            };
        }
    };

    // Body parsed. Look for `error_code` — present in either 200-with-error
    // pattern or 4xx-with-error pattern. Pull message from `message` first,
    // fall back to `error`, fall back to empty string.
    if let Some(code) = value.get("error_code").and_then(|v| v.as_str()) {
        let message = value
            .get("message")
            .and_then(|v| v.as_str())
            .or_else(|| value.get("error").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        return Err(CliError::ResponseErrorCode {
            code: code.to_string(),
            message,
            http_status: status_code,
        });
    }

    // 4xx body that parsed but had no error_code — still a failure, but no
    // structured code to act on. Map to HttpStatus so exit code = 2 (permanent).
    if status.is_client_error() {
        return Err(CliError::HttpStatus(status_code, bytes_to_excerpt(&bytes)));
    }

    Ok(value)
}

async fn read_body_excerpt(resp: reqwest::Response) -> String {
    match resp.bytes().await {
        Ok(b) => bytes_to_excerpt(&b),
        Err(e) => format!("<body read failed: {e}>"),
    }
}

fn bytes_to_excerpt(bytes: &[u8]) -> String {
    let lossy = String::from_utf8_lossy(bytes);
    if lossy.len() <= BODY_EXCERPT_BYTES {
        return lossy.into_owned();
    }
    // Truncate on a char boundary to keep UTF-8 valid in the excerpt.
    let mut end = BODY_EXCERPT_BYTES;
    while end > 0 && !lossy.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = lossy[..end].to_string();
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// Helper: clear the env var so the priority chain falls through it
    /// cleanly. Tests that exercise the env path must set it themselves.
    fn clear_runtime_port_env() {
        std::env::remove_var("GITIM_RUNTIME_PORT");
    }

    /// Point HOME at a fresh tempdir so the `user_config::read()` step in
    /// `resolve_base_url` reads an empty config (no `listen_port`), letting
    /// the test isolate the priority slot we care about.
    struct HomeIsolate {
        original: Option<std::ffi::OsString>,
        _tmp: tempfile::TempDir,
    }

    impl HomeIsolate {
        fn install() -> Self {
            let tmp = tempfile::TempDir::new().expect("tempdir for HOME");
            let original = std::env::var_os("HOME");
            std::env::set_var("HOME", tmp.path());
            Self {
                original,
                _tmp: tmp,
            }
        }
    }

    impl Drop for HomeIsolate {
        fn drop(&mut self) {
            match self.original.take() {
                Some(val) => std::env::set_var("HOME", val),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    // All resolve_base_url tests run #[serial] because they touch process-wide
    // env vars (HOME + GITIM_RUNTIME_PORT). Without serialization they race
    // each other and with any other test using HomeGuard.

    #[test]
    #[serial]
    fn priority_port_arg_wins() {
        let _home = HomeIsolate::install();
        clear_runtime_port_env();
        // Even with env unset and no runtime.json, --port should win outright.
        let url = resolve_base_url(Some(7000));
        assert_eq!(url, "http://127.0.0.1:7000");
    }

    #[test]
    #[serial]
    fn priority_port_arg_beats_env_and_config() {
        let home = HomeIsolate::install();
        // Plant a config file with a listen_port hint.
        let cfg_path = home._tmp.path().join(".gitim/runtime.json");
        std::fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
        std::fs::write(
            &cfg_path,
            r#"{"runtime_id":"abc","workspaces":[],"listen_port":9999}"#,
        )
        .unwrap();
        std::env::set_var("GITIM_RUNTIME_PORT", "8001");

        let url = resolve_base_url(Some(7000));
        assert_eq!(url, "http://127.0.0.1:7000");

        std::env::remove_var("GITIM_RUNTIME_PORT");
    }

    #[test]
    #[serial]
    fn priority_env_over_config() {
        let home = HomeIsolate::install();
        // Plant a config with a listen_port; env should still win.
        let cfg_path = home._tmp.path().join(".gitim/runtime.json");
        std::fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
        std::fs::write(
            &cfg_path,
            r#"{"runtime_id":"abc","workspaces":[],"listen_port":9999}"#,
        )
        .unwrap();
        std::env::set_var("GITIM_RUNTIME_PORT", "8001");

        let url = resolve_base_url(None);
        assert_eq!(url, "http://127.0.0.1:8001");

        std::env::remove_var("GITIM_RUNTIME_PORT");
    }

    #[test]
    #[serial]
    fn priority_config_over_default() {
        let home = HomeIsolate::install();
        clear_runtime_port_env();
        let cfg_path = home._tmp.path().join(".gitim/runtime.json");
        std::fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
        std::fs::write(
            &cfg_path,
            r#"{"runtime_id":"abc","workspaces":[],"listen_port":9999}"#,
        )
        .unwrap();

        let url = resolve_base_url(None);
        assert_eq!(url, "http://127.0.0.1:9999");
    }

    #[test]
    #[serial]
    fn priority_default_when_nothing_set() {
        let _home = HomeIsolate::install();
        clear_runtime_port_env();
        // No env, no runtime.json under tempdir HOME, no port arg → DEFAULT_PORT.
        let url = resolve_base_url(None);
        assert_eq!(url, format!("http://127.0.0.1:{DEFAULT_PORT}"));
    }

    #[test]
    #[serial]
    fn malformed_env_falls_through_to_config() {
        let home = HomeIsolate::install();
        std::env::set_var("GITIM_RUNTIME_PORT", "not-a-number");
        let cfg_path = home._tmp.path().join(".gitim/runtime.json");
        std::fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
        std::fs::write(
            &cfg_path,
            r#"{"runtime_id":"abc","workspaces":[],"listen_port":9999}"#,
        )
        .unwrap();

        // Garbage env value should be treated as "absent" and fall through
        // to the next priority slot, not crash or fall directly to default.
        let url = resolve_base_url(None);
        assert_eq!(url, "http://127.0.0.1:9999");

        std::env::remove_var("GITIM_RUNTIME_PORT");
    }

    #[test]
    fn bytes_to_excerpt_truncates_long_input() {
        let big = vec![b'a'; 1024];
        let s = bytes_to_excerpt(&big);
        // BODY_EXCERPT_BYTES + "..." marker
        assert_eq!(s.len(), BODY_EXCERPT_BYTES + 3);
        assert!(s.ends_with("..."));
    }

    #[test]
    fn bytes_to_excerpt_passes_short_input_through() {
        let small = b"hello";
        let s = bytes_to_excerpt(small);
        assert_eq!(s, "hello");
    }
}
