//! HTTP client + base URL discovery + error classification for the runtime CLI.
//!
//! Architecture context (see `docs/plans/runtime-cli/00-requirements.md` §1, §4, §5):
//!
//! - Port discovery follows a strict priority: `--port` flag → `GITIM_RUNTIME_PORT`
//!   env → persisted `listen_port` in `~/.gitim/runtime.json` → `DEFAULT_PORT`.
//! - Error classification is body-first: the response body is always parsed
//!   regardless of HTTP status, and the canonical signal is the JSON
//!   `error_code` / `ok` fields, NOT the HTTP status. The runtime is
//!   inconsistent about which status it pairs with structured errors —
//!   some endpoints return 200 + `{ok:false, error_code:...}`, some 4xx +
//!   `error_code`, some 5xx + `error_code`. All three map to
//!   `CliError::ResponseErrorCode` so the agent's exit-code mapper can make
//!   a single decision off `error_code`. The fallback to `HttpStatus` only
//!   fires when the body has no usable structure (no `error_code`, no
//!   `ok: false`) — see `process_response_inner`.
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
        // Default reqwest builder has NO timeout — a wedged runtime would
        // block the CLI process indefinitely, hanging an agent's Bash tool
        // call. Connect timeout is small because we only ever talk to
        // loopback; anything slower than a second is a stuck listener.
        // Request timeout is loose enough to cover `add-agent`, which does
        // a full clone inside the runtime before responding.
        let inner = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("reqwest client builds with default settings");
        Self { base_url, inner }
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

/// Thin async wrapper: read the response status + body, then hand off to the
/// pure classification function. Keeping the IO boundary minimal makes the
/// real logic in `process_response_inner` unit-testable without spinning up
/// a mock HTTP server (reqwest::Response isn't constructible directly).
async fn process_response(resp: reqwest::Response) -> Result<serde_json::Value, CliError> {
    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| CliError::Transport(e.to_string()))?;
    process_response_inner(status, &bytes)
}

/// Body-first response classification. The HTTP status is only consulted as
/// a fallback when the body lacks structured fields — the runtime is
/// inconsistent about pairing structured errors with status codes, so we
/// privilege the body.
///
/// Decision order:
/// 1. Try parse body as JSON.
/// 2. If parsed: `error_code` present → ResponseErrorCode (regardless of
///    HTTP status — handles 200/4xx/5xx + structured error uniformly).
/// 3. If parsed: `ok: false` (without `error_code`) → ResponseErrorCode with
///    `code = "unspecified"`, message from `error` field. Catches the case
///    where the runtime returns 200 with a failure body but forgot to set
///    a structured code — without this, the CLI would treat the response
///    as success.
/// 4. If parsed and 2xx → Ok(value).
/// 5. If parsed and 4xx/5xx without structured fields → HttpStatus with body
///    excerpt.
/// 6. If parse failed and 4xx/5xx → HttpStatus with body excerpt.
/// 7. If parse failed and 2xx → Parse error (the runtime should always
///    return JSON; a 200 with non-JSON is a protocol bug).
fn process_response_inner(
    status: reqwest::StatusCode,
    bytes: &[u8],
) -> Result<serde_json::Value, CliError> {
    let status_code = status.as_u16();

    let value: serde_json::Value = match serde_json::from_slice(bytes) {
        Ok(v) => v,
        Err(e) => {
            // No JSON. If status indicates a problem, surface that with a
            // body excerpt — most useful failure mode for callers. If
            // status was 2xx but body wasn't JSON, that's a protocol bug.
            let excerpt = bytes_to_excerpt(bytes);
            return if status.is_client_error() || status.is_server_error() {
                Err(CliError::HttpStatus(status_code, excerpt))
            } else {
                Err(CliError::Parse(format!("{e}: body excerpt: {excerpt}")))
            };
        }
    };

    // Body parsed. Structured `error_code` wins regardless of HTTP status —
    // see decision order in the doc comment above.
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

    // No structured `error_code`, but body explicitly signals failure via
    // `ok: false`. Don't let this fall through as success. Synthesize a
    // generic code so the agent still hits the exit-2 (permanent) path,
    // matching how a code-bearing failure would have classified.
    if value.get("ok").and_then(|v| v.as_bool()) == Some(false) {
        let message = value
            .get("error")
            .and_then(|v| v.as_str())
            .or_else(|| value.get("message").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        return Err(CliError::ResponseErrorCode {
            code: "unspecified".to_string(),
            message,
            http_status: status_code,
        });
    }

    // Body parsed but has neither `error_code` nor `ok: false`. Status
    // decides: 4xx/5xx → HttpStatus, 2xx → Ok.
    if status.is_client_error() || status.is_server_error() {
        return Err(CliError::HttpStatus(status_code, bytes_to_excerpt(bytes)));
    }

    Ok(value)
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

    // ── process_response_inner: body-first classification ───────────────────

    use reqwest::StatusCode;

    #[test]
    fn ok_response_returns_value() {
        let bytes = br#"{"ok":true,"data":42}"#;
        let v = process_response_inner(StatusCode::OK, bytes).expect("2xx + valid JSON → Ok");
        assert_eq!(v.get("data").and_then(|n| n.as_i64()), Some(42));
    }

    #[test]
    fn error_code_in_200_classified_as_response_error() {
        // 200 + `error_code` — the runtime sometimes returns this shape for
        // permanent failures it considers "expected" (e.g. handler_conflict).
        let bytes = br#"{"ok":false,"error":"name taken","error_code":"handler_conflict"}"#;
        let err = process_response_inner(StatusCode::OK, bytes).expect_err("must error");
        match err {
            CliError::ResponseErrorCode { code, message, http_status } => {
                assert_eq!(code, "handler_conflict");
                assert_eq!(message, "name taken");
                assert_eq!(http_status, 200);
            }
            other => panic!("expected ResponseErrorCode, got: {other:?}"),
        }
    }

    #[test]
    fn error_code_in_4xx_classified_as_response_error() {
        // 4xx + `error_code` — common pattern for input validation
        // failures (provider validation, missing fields, etc).
        let bytes = br#"{"ok":false,"error":"bad input","error_code":"validation_failed"}"#;
        let err =
            process_response_inner(StatusCode::BAD_REQUEST, bytes).expect_err("must error");
        match err {
            CliError::ResponseErrorCode { code, http_status, .. } => {
                assert_eq!(code, "validation_failed");
                assert_eq!(http_status, 400);
            }
            other => panic!("expected ResponseErrorCode, got: {other:?}"),
        }
    }

    #[test]
    fn error_code_in_5xx_parsed_not_swallowed_by_status() {
        // Regression: previous behavior short-circuited 5xx without parsing
        // the body, so a structured `error_code` would never surface. Burn
        // / preflight / sync error codes ship in 5xx bodies and the agent
        // needs them to decide permanent (don't retry) vs transient (retry).
        let bytes = br#"{"ok":false,"error":"daemon RPC failed","error_code":"daemon_unreachable"}"#;
        let err = process_response_inner(StatusCode::INTERNAL_SERVER_ERROR, bytes)
            .expect_err("must error");
        match err {
            CliError::ResponseErrorCode { code, http_status, .. } => {
                assert_eq!(code, "daemon_unreachable");
                assert_eq!(http_status, 500);
            }
            other => panic!(
                "expected ResponseErrorCode (5xx body parsed), got: {other:?}",
            ),
        }
    }

    #[test]
    fn ok_false_without_error_code_classified_as_response_error() {
        // Regression: previous behavior fell through to `Ok(value)` for
        // 200 + `{ok:false}` without `error_code`. That made the caller
        // treat a failure as success. We now synthesize a generic code so
        // the exit-code mapper hits the permanent path.
        let bytes = br#"{"ok":false,"error":"something broke"}"#;
        let err = process_response_inner(StatusCode::OK, bytes).expect_err("must error");
        match err {
            CliError::ResponseErrorCode { code, message, http_status } => {
                assert_eq!(code, "unspecified");
                assert_eq!(message, "something broke");
                assert_eq!(http_status, 200);
            }
            other => panic!("expected ResponseErrorCode, got: {other:?}"),
        }
    }

    #[test]
    fn ok_false_without_error_uses_message_field() {
        // Edge case for `ok_false_without_error_code...`: when neither
        // `error` nor `error_code` is set, fall back to `message`.
        let bytes = br#"{"ok":false,"message":"db connection lost"}"#;
        let err = process_response_inner(StatusCode::OK, bytes).expect_err("must error");
        match err {
            CliError::ResponseErrorCode { code, message, .. } => {
                assert_eq!(code, "unspecified");
                assert_eq!(message, "db connection lost");
            }
            other => panic!("expected ResponseErrorCode, got: {other:?}"),
        }
    }

    #[test]
    fn five_xx_without_parseable_body_falls_to_http_status() {
        // Common case for upstream proxy errors — body is HTML or
        // plaintext, not JSON. Must still classify as HttpStatus(5xx)
        // so the exit-code mapper hits the transient path.
        let bytes = b"<html><body>Bad Gateway</body></html>";
        let err =
            process_response_inner(StatusCode::BAD_GATEWAY, bytes).expect_err("must error");
        match err {
            CliError::HttpStatus(status, body) => {
                assert_eq!(status, 502);
                assert!(body.contains("Bad Gateway"), "body must include excerpt: {body}");
            }
            other => panic!("expected HttpStatus, got: {other:?}"),
        }
    }

    #[test]
    fn four_xx_without_json_falls_to_http_status() {
        // No JSON, no error_code → HttpStatus with body excerpt.
        let bytes = b"not found";
        let err =
            process_response_inner(StatusCode::NOT_FOUND, bytes).expect_err("must error");
        match err {
            CliError::HttpStatus(status, body) => {
                assert_eq!(status, 404);
                assert!(body.contains("not found"));
            }
            other => panic!("expected HttpStatus, got: {other:?}"),
        }
    }

    #[test]
    fn four_xx_with_json_no_error_code_falls_to_http_status() {
        // Edge case: 4xx with valid JSON but no `error_code` and no
        // `ok: false` — fall through to HttpStatus so exit code = 2.
        let bytes = br#"{"detail":"some plain rejection"}"#;
        let err =
            process_response_inner(StatusCode::UNPROCESSABLE_ENTITY, bytes).expect_err("must error");
        match err {
            CliError::HttpStatus(status, body) => {
                assert_eq!(status, 422);
                assert!(body.contains("plain rejection"));
            }
            other => panic!("expected HttpStatus, got: {other:?}"),
        }
    }

    #[test]
    fn two_xx_without_json_is_parse_error() {
        // 2xx with non-JSON body is a protocol bug — the runtime always
        // returns JSON. Surface as Parse so the agent sees the bug.
        let bytes = b"surprise plaintext";
        let err = process_response_inner(StatusCode::OK, bytes).expect_err("must error");
        match err {
            CliError::Parse(msg) => {
                assert!(msg.contains("surprise plaintext"));
            }
            other => panic!("expected Parse, got: {other:?}"),
        }
    }

    // ── Timeout: connect to a non-listening port → fails fast, doesn't hang ─

    /// Build a Client and try a request against a port nothing's listening
    /// on. The connect must fail within the configured connect_timeout (5s)
    /// — much less than the outer 15s guard. Without our timeout config the
    /// OS-default could be 1-2 minutes.
    ///
    /// Loopback connect-refused is usually instant on macOS/Linux (the
    /// kernel rejects without waiting). The real value of this test is the
    /// outer `tokio::time::timeout` guard: if a future regression accidentally
    /// drops the timeout config, a hung listener scenario won't surface here,
    /// but a flat `connect_timeout` removal won't make this test hang either —
    /// it'll just rely on kernel refusal. To exercise the timeout itself we
    /// would need a TCP listener that accepts but never replies. That's
    /// out-of-scope; locking the build-time config in `Client::new` is
    /// adequate coverage.
    #[tokio::test]
    async fn client_request_does_not_hang_against_dead_port() {
        // Pick a likely-dead port. Bind+drop a TCP listener to grab a free
        // ephemeral port assignment from the OS, then close it so subsequent
        // connects are refused. This is more robust than a hardcoded high
        // port that might be in use.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let client = Client::new(format!("http://127.0.0.1:{port}"));
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            client.get("/status"),
        )
        .await;

        // The outer timeout must NOT fire — that would mean the inner
        // request hung. The inner result must be a Transport error.
        let inner = outcome.expect("client must error within 15s, not hang");
        assert!(
            matches!(inner, Err(CliError::Transport(_))),
            "expected Transport error from dead port, got: {inner:?}",
        );
    }
}
