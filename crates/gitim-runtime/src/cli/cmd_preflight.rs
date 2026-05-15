//! `preflight` subcommand — surface a provider CLI's real-hello status.
//!
//! Thin pass-through over `GET /preflight/{provider}` (root-level, not
//! workspace-scoped). The server's response shape is provider-specific
//! (`PreflightResult` for the runtime, an introspect-style payload for hermes)
//! and is intentionally **not** type-checked client-side: this command's job
//! is to expose the runtime's preflight surface to scripts and AI agents, so
//! whatever the server emits passes through to stdout verbatim.
//!
//! Two preconditions are enforced client-side before issuing the request:
//!
//! 1. `--llm-provider` / `--llm-model` are hermes-only. Passing them with any
//!    other provider is a CLI-side validation failure (exit 1) — no HTTP is
//!    issued. The server would silently ignore them today, but a noisy error
//!    surfaces user intent mismatches early.
//!
//! 2. URL query-encoding. `llm_provider` values like `custom:foo` carry
//!    reserved characters; we percent-encode them so the server's
//!    `axum::extract::Query` doesn't choke on raw colons.
//!
//! Exit conventions:
//! - Server returned 2xx (whether preflight `available:true` or `available:false`)
//!   → print body to stdout, return 0. The runtime treats /preflight as a
//!   status surface, not a fail-fast probe; the client matches that.
//! - Server returned 4xx (e.g. unknown provider → 400) → `CliError::HttpStatus`
//!   bubbles up; mapping to exit 2 happens in `exit_code::from_cli_error`.
//! - Transport / parse errors → exit 1 / exit 1 respectively.

use crate::cli::http::{CliError, Client};
use percent_encoding::{AsciiSet, CONTROLS};

/// Provider identifier that gates hermes-only flags. Centralized so the check
/// can't drift between the validation and the path-building steps.
const HERMES_PROVIDER: &str = "hermes";

/// Character set escaped in query-value position. We start from `CONTROLS`
/// (must escape) and add:
/// - query-structural chars: `&`, `=`, `?`, `#`, `+`, ` ` — these would
///   restructure or terminate the query string in `axum::extract::Query`
/// - URL-context chars: `/` (path separator), `\`, `"`, `<`, `>`, `^`, `` ` ``,
///   `{`, `|`, `}` (per WHATWG URL "query" percent-encode set)
/// - `:` (reserved, but legal as `pchar`) — escaped for readability in shell
///   logs where bare colons in `custom:foo` provider IDs would otherwise
///   look like part of the host:port
///
/// Hyphens, dots, underscores, tildes (RFC 3986 unreserved) flow through
/// literally so URLs stay readable.
const QUERY_VALUE_ESCAPE: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'&')
    .add(b'+')
    .add(b'/')
    .add(b'<')
    .add(b'=')
    .add(b'>')
    .add(b'?')
    .add(b'\\')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}')
    .add(b':');

/// Run the preflight subcommand.
///
/// Validates the hermes-only flag preconditions, builds the path with an
/// optional query string, GETs it, prints the raw JSON response to stdout.
pub async fn run(
    client: &Client,
    provider: String,
    llm_provider: Option<String>,
    llm_model: Option<String>,
) -> Result<i32, CliError> {
    if provider != HERMES_PROVIDER && (llm_provider.is_some() || llm_model.is_some()) {
        return Err(CliError::InvalidConfig(
            "--llm-provider/--llm-model are only valid for hermes provider".to_string(),
        ));
    }

    let path = build_preflight_path(&provider, llm_provider.as_deref(), llm_model.as_deref());
    let body = client.get(&path).await?;

    // Pass-through: server's response shape is provider-specific and we
    // deliberately don't type-check it here. Use pretty form so humans
    // reading the terminal can scan the structure; jq pipelines still work.
    let out = serde_json::to_string_pretty(&body)
        .map_err(|e| CliError::Parse(format!("serialize preflight response: {e}")))?;
    println!("{out}");
    Ok(0)
}

/// Build the `/preflight/{provider}[?llm_provider=...&llm_model=...]` path.
///
/// Pure function for easy unit testing. Both query params are URL-encoded
/// because typical hermes values include reserved characters (e.g.
/// `custom:my-endpoint`). When both params are absent, no `?` is emitted —
/// keeps the wire format identical to the pre-hermes default path.
pub fn build_preflight_path(
    provider: &str,
    llm_provider: Option<&str>,
    llm_model: Option<&str>,
) -> String {
    let mut path = format!("/preflight/{provider}");
    let mut pairs: Vec<(&str, &str)> = Vec::with_capacity(2);
    if let Some(v) = llm_provider {
        pairs.push(("llm_provider", v));
    }
    if let Some(v) = llm_model {
        pairs.push(("llm_model", v));
    }
    if pairs.is_empty() {
        return path;
    }
    path.push('?');
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            path.push('&');
        }
        path.push_str(k);
        path.push('=');
        path.push_str(&percent_encoding::utf8_percent_encode(v, QUERY_VALUE_ESCAPE).to_string());
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_path_no_query_when_no_llm_params() {
        let path = build_preflight_path("claude", None, None);
        assert_eq!(path, "/preflight/claude");
    }

    #[test]
    fn build_path_includes_llm_provider_only() {
        let path = build_preflight_path("hermes", Some("anthropic"), None);
        assert_eq!(path, "/preflight/hermes?llm_provider=anthropic");
    }

    #[test]
    fn build_path_includes_llm_model_only() {
        // Hyphens are unreserved (RFC 3986) — they pass through literally
        // for readable URLs. Only reserved/restructuring chars get escaped.
        let path = build_preflight_path("hermes", None, Some("claude-opus-4-7"));
        assert_eq!(path, "/preflight/hermes?llm_model=claude-opus-4-7");
    }

    #[test]
    fn build_path_includes_both_in_order() {
        // Order is fixed (provider first, model second) so the URL is
        // deterministic — useful for snapshot tests downstream and for
        // server-side request logging.
        let path = build_preflight_path("hermes", Some("anthropic"), Some("claude-opus-4-7"));
        assert_eq!(
            path,
            "/preflight/hermes?llm_provider=anthropic&llm_model=claude-opus-4-7",
        );
    }

    #[test]
    fn build_path_percent_encodes_colon() {
        // `custom:myendpoint`-style provider IDs round-trip through
        // percent-encoding. Colon is legal in `pchar` per RFC 3986 but we
        // escape it for readability when shell-logged.
        let path = build_preflight_path("hermes", Some("custom:myendpoint"), None);
        assert_eq!(path, "/preflight/hermes?llm_provider=custom%3Amyendpoint");
    }

    #[test]
    fn build_path_percent_encodes_special_chars() {
        // Belt-and-suspenders: even if a value carries chars that would
        // otherwise terminate or restructure the query string (`&`, `=`,
        // `?`, space), the encoder neutralizes them before they hit the
        // wire. The server reads back the original via decode.
        let path = build_preflight_path("hermes", Some("a&b=c?d e"), None);
        assert_eq!(path, "/preflight/hermes?llm_provider=a%26b%3Dc%3Fd%20e");
    }
}
