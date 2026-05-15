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
        //
        // Request timeout has to cover the slowest synchronous handler we
        // call. `add-agent` is the bound: the runtime awaits
        // `provision_agent` inline, which `git clone`s the workspace remote
        // before responding. Large monorepos over slow uplinks can take
        // minutes — 60s aborts the client mid-clone, leaving an
        // orphaned half-provisioned state on the runtime side. 5 minutes is
        // an empirical envelope wide enough for realistic GitHub repos but
        // narrow enough to surface a hung server. Status / list-agents are
        // bounded by reqwest's TCP read timeout, not this 300s wall.
        let inner = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(300))
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

/// Body-first response classification with one critical refinement: the
/// `ok: false` synthesis path only fires for 2xx responses. For 4xx/5xx
/// without a structured `error_code` we let HTTP status decide the
/// classification, because the agent's exit-code mapper relies on 5xx
/// mapping to **transient (3)** — synthesizing a code on every `ok: false`
/// would silently flip 5xx into permanent (2) and break the retry contract
/// (Architecture §4).
///
/// Decision order:
/// 1. Try parse body as JSON. If parse fails:
///      a. status 4xx/5xx → `HttpStatus` (status decides exit class)
///      b. status 2xx → `Parse` (the runtime should always return JSON;
///         a 200 with non-JSON is a protocol bug worth surfacing)
/// 2. Body parsed and contains `error_code` → `ResponseErrorCode`
///    (regardless of HTTP status — `error_code` is the canonical signal
///    per Architecture §1, including 5xx + structured code → permanent).
/// 3. Body parsed, status 2xx, `ok: false` (no `error_code`) →
///    `ResponseErrorCode` with synthesized `code = "!cli:missing_error_code"`.
///    Catches the case where the runtime returns 200 with a failure body
///    but forgot to set a structured code — without this, the CLI would
///    treat the response as success. The `!cli:` prefix marks the value as
///    CLI-side synthesis (see the synthesis branch for the convention).
/// 4. Body parsed and status 2xx, no failure signal → `Ok(value)`.
/// 5. Body parsed and status 4xx/5xx without structured fields →
///    `HttpStatus` with body excerpt. Lets exit-code mapper apply the
///    transient-vs-permanent decision off HTTP status alone.
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
    // including 5xx + code → permanent (per spec §4).
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

    // 2xx + `ok: false` without `error_code`: the runtime promised success
    // via status but contradicted itself in the body. Synthesize a sentinel
    // code so we don't silently fall through as success.
    //
    // The `!cli:` prefix is reserved for CLI-side synthesized codes —
    // runtime endpoints only emit snake_case lowercase identifiers via
    // `ErrorBody::with_code`, so `!` cannot collide with a future real
    // error_code. If you ever need another sentinel, namespace it under
    // `!cli:<purpose>`.
    //
    // For 4xx/5xx without `error_code` we deliberately fall through to
    // `HttpStatus` below — synthesizing a code there would map every 5xx
    // into permanent (exit 2) and break the transient-retry contract.
    if status.is_success() && value.get("ok").and_then(|v| v.as_bool()) == Some(false) {
        let message = value
            .get("error")
            .and_then(|v| v.as_str())
            .or_else(|| value.get("message").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        return Err(CliError::ResponseErrorCode {
            code: "!cli:missing_error_code".to_string(),
            message,
            http_status: status_code,
        });
    }

    // Body parsed but has neither `error_code` nor a 2xx `ok: false`.
    // Status decides: 4xx → permanent, 5xx → transient (per the
    // exit-code mapper).
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
        // treat a failure as success. We now synthesize a sentinel code so
        // the exit-code mapper hits the permanent path. The `!cli:` prefix
        // marks the code as CLI-side synthesis so it can't collide with a
        // future runtime-emitted `error_code` (runtime only emits
        // snake_case lowercase).
        let bytes = br#"{"ok":false,"error":"something broke"}"#;
        let err = process_response_inner(StatusCode::OK, bytes).expect_err("must error");
        match err {
            CliError::ResponseErrorCode { code, message, http_status } => {
                assert_eq!(code, "!cli:missing_error_code");
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
                assert_eq!(code, "!cli:missing_error_code");
                assert_eq!(message, "db connection lost");
            }
            other => panic!("expected ResponseErrorCode, got: {other:?}"),
        }
    }

    // ── Status-class boundaries for the body-first/status-fallback split ────

    #[test]
    fn test_5xx_with_ok_false_no_error_code_classifies_as_http_status() {
        // Regression: 5xx + `{ok:false}` without `error_code` must classify
        // as `HttpStatus(500, _)` so the exit-code mapper preserves the
        // transient-retry path. The previous synthesis behaviour incorrectly
        // produced `ResponseErrorCode` here and mapped a transient failure
        // into permanent (exit 2). See `process_response_inner` doc.
        let bytes = br#"{"ok":false,"error":"daemon down"}"#;
        let err = process_response_inner(StatusCode::INTERNAL_SERVER_ERROR, bytes)
            .expect_err("must error");
        match err {
            CliError::HttpStatus(status, body) => {
                assert_eq!(status, 500);
                assert!(body.contains("daemon down"), "body excerpt: {body}");
            }
            other => panic!("expected HttpStatus(500, _), got: {other:?}"),
        }
    }

    #[test]
    fn test_5xx_with_ok_false_and_error_code_classifies_as_response_error() {
        // 5xx + structured `error_code` → `ResponseErrorCode`. The code is
        // the canonical signal even on 5xx; the agent's exit-code mapper
        // then classifies as permanent (exit 2) — the daemon was reachable
        // and gave a structured rejection. Don't retry on a structured no.
        let bytes = br#"{"ok":false,"error_code":"daemon_unreachable","error":"daemon went away"}"#;
        let err = process_response_inner(StatusCode::INTERNAL_SERVER_ERROR, bytes)
            .expect_err("must error");
        match err {
            CliError::ResponseErrorCode {
                code, http_status, ..
            } => {
                assert_eq!(code, "daemon_unreachable");
                assert_eq!(http_status, 500);
            }
            other => panic!("expected ResponseErrorCode, got: {other:?}"),
        }
    }

    #[test]
    fn test_4xx_with_ok_true_classifies_as_http_status() {
        // Conflicting signals: 4xx status paired with `ok: true` body.
        // Status wins because the runtime explicitly emits 4xx for
        // permanent client errors. A buggy server shouldn't be able to
        // sneak past with a misleading `ok: true`.
        let bytes = br#"{"ok":true,"data":42}"#;
        let err = process_response_inner(StatusCode::BAD_REQUEST, bytes).expect_err("must error");
        match err {
            CliError::HttpStatus(status, _) => {
                assert_eq!(status, 400);
            }
            other => panic!("expected HttpStatus(400, _), got: {other:?}"),
        }
    }

    #[test]
    fn test_2xx_with_ok_false_no_error_code_synthesizes_sentinel() {
        // Companion to `ok_false_without_error_code_...` — explicit
        // assertion that the synthesized code is exactly the documented
        // sentinel string. Pin against accidental rename.
        let bytes = br#"{"ok":false,"error":"foo"}"#;
        let err = process_response_inner(StatusCode::OK, bytes).expect_err("must error");
        match err {
            CliError::ResponseErrorCode {
                code,
                message,
                http_status,
            } => {
                assert_eq!(code, "!cli:missing_error_code");
                assert_eq!(message, "foo");
                assert_eq!(http_status, 200);
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

    // ── Timeout *config presence*: dead-port request fails as Transport ────

    /// Build a Client and try a request against a port nothing's listening
    /// on, then assert the failure surfaces as `CliError::Transport`. The
    /// real claim being pinned here is **config wiring**, not the timeout
    /// value itself — loopback connect-refused is instant on macOS/Linux
    /// because the kernel rejects the SYN immediately, so this test would
    /// pass even if `connect_timeout` were removed entirely.
    ///
    /// What we'd need to test the timeout *value* is a TCP listener that
    /// accepts but never replies (so the request hangs waiting for headers).
    /// That's out of scope for this unit suite. The 15s outer guard is
    /// there as a backstop: if a regression somehow makes the request
    /// genuinely hang against connect-refused (very unlikely), the test
    /// fails loudly instead of stalling the test runner.
    ///
    /// In short: this test guarantees the dead-port path → Transport. The
    /// 300s request timeout and 5s connect timeout configured in
    /// `Client::new` are locked in by the build itself (any code path that
    /// drops them is a static change reviewers will catch).
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
