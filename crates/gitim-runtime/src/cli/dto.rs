//! CLI-side wire DTOs for the `gitim-runtime` one-shot CLI.
//!
//! Architecture context (see `docs/plans/runtime-cli/00-requirements.md` §2, §3):
//!
//! - These types are intentionally **separate** from the server-side
//!   `crate::http::AgentInfo`. The server struct is `Serialize`-only and
//!   carries internal fields (`loop_handle: Option<AbortHandle>`, etc.) the
//!   CLI must never deserialize. Keeping the CLI surface a parallel type
//!   tree means we can evolve the wire format without ABI-coupling the two
//!   sides through a shared crate type.
//! - `AgentView` is the **default redacted projection** used by `list-agents`
//!   without `--detailed`. It omits `repo_path`, `system_prompt`, `env`,
//!   `session_usage`, `usage_summary`, `introduction`, `error_message`, and
//!   the Hermes-specific `llm_provider` / `llm_model`. The default view is
//!   safe to dump into a CI log or a Slack message without leaking secrets.
//! - `AgentDetail` opts into the full payload when `--detailed` is passed.
//!   Environment values still pass through `redact_env_secrets` before they
//!   leave the CLI — secrets-shaped keys are replaced with `"<redacted>"`.
//!   The redaction step is **explicit at the call site** (via
//!   `agent_detail_from_value`) rather than buried in a serde `Deserialize`
//!   impl, so reviewers can see exactly where untrusted env values are
//!   sanitized.
//! - The `Serialize` impls preserve serde skip semantics so the CLI's JSON
//!   output round-trips into Rust callers (e.g. shell-script consumers via
//!   `jq`, or future test harnesses parsing CLI stdout).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Substrings (matched against the **uppercased** env key) that flag a value
/// as secret-shaped. Conservatively broad: false positives just redact a
/// harmless value, false negatives leak a real secret to stdout.
///
/// Kept private so callers don't add new patterns ad-hoc — central list
/// makes the redaction policy auditable in one place.
const SECRET_KEY_SUBSTRINGS: &[&str] = &["KEY", "TOKEN", "SECRET", "PASSWORD", "API", "AUTH"];

/// Replacement value for redacted env entries. Chosen to be visibly distinct
/// from any plausible real env value (literal angle brackets won't appear in
/// a normal `KEY=value` line).
const REDACTED_VALUE: &str = "<redacted>";

/// Default (redacted) projection of a runtime agent for `list-agents`.
///
/// Excludes every field that could carry user-private data: filesystem paths,
/// system prompts, env vars, usage telemetry, introduction blurbs, and error
/// messages (which may quote secrets). This shape is what we promise as the
/// "safe to log" default; widening it requires a docs + plan update.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentView {
    pub id: String,
    pub handler: String,
    pub display_name: String,
    /// "idle" / "running" / "error" — the runtime's lifecycle string. Not
    /// modeled as an enum on the CLI side because we want unknown future
    /// states to pass through verbatim (forward compatibility with newer
    /// runtimes) rather than fail to deserialize.
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity: Option<String>,
    pub messages_processed: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Expanded projection for `list-agents --detailed`.
///
/// Carries every field `AgentView` has, plus the sensitive ones. `env` is
/// already redacted by the time it reaches this struct's owner — see
/// `agent_detail_from_value` for the canonical construction path.
///
/// `session_usage` and `usage_summary` stay as opaque `serde_json::Value`
/// because their internal shape is owned by `crate::state` / `crate::usage_log`
/// and we don't want the CLI to recompile every time those types add a field.
/// CLI just passes them through to JSON stdout — no parsing needed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentDetail {
    pub id: String,
    pub handler: String,
    pub display_name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity: Option<String>,
    pub messages_processed: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    pub repo_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub introduction: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_usage: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_summary: Option<serde_json::Value>,
}

/// Aggregate runtime status reported by `status` subcommand (task 6 builds
/// this by combining `/runtime/health` and `/workspaces`). Kept in this DTO
/// module so all CLI wire types are colocated.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeStatus {
    pub runtime_id: String,
    pub version: String,
    pub uptime_secs: u64,
    pub workspaces_count: usize,
    pub agents_total: usize,
}

/// JSON error envelope the CLI emits to stdout when a command fails. Mirrors
/// the runtime's `ErrorBody` shape (`{ok: false, error, error_code?,
/// preflight_detail?}`) so scripts can write one parser for both server and
/// CLI errors.
///
/// `ok` is always `false` here by convention — keeping the field explicit
/// (rather than synthesizing it during serialize) lets callers `match` on
/// the deserialized form without a parallel "success?" branch.
///
/// `preflight_detail` is populated only when the underlying `CliError` carried
/// a nested [`crate::preflight::PreflightResult`] — currently emitted by the
/// server's `agents_add` handler on a per-agent preflight failure. The field
/// keeps the same name and shape the server uses (`ErrorBody::with_preflight`),
/// so a single JSON parser sees the same structure in either error origin.
/// `PartialEq` is dropped here because `serde_json::Value` doesn't implement
/// `Eq` and `PreflightResult` doesn't either; tests round-trip via
/// serialize/deserialize comparison instead.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub ok: bool,
    pub error: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight_detail: Option<crate::preflight::PreflightResult>,
}

/// Response payload from `POST /agents/add`. The runtime returns just
/// `{ok, id}` (full agent state requires a follow-up `GET /agents/<id>`),
/// so we mirror that minimal shape rather than reuse `AgentView`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AddAgentResponse {
    pub ok: bool,
    pub id: String,
}

/// Redact secret-shaped values from an env map.
///
/// Takes ownership because the CLI is one-shot — the caller is constructing
/// the final wire value and won't reuse the input map. Avoiding the borrow
/// also lets us mutate in place via `into_iter().map(...).collect()`.
///
/// Heuristic: any key whose uppercase form contains one of
/// `SECRET_KEY_SUBSTRINGS` gets its value replaced with `"<redacted>"`. The
/// match is case-insensitive (lowercase `api_key` still redacts) and a
/// substring (so `RETRY_TOKEN_DELAY` redacts via `TOKEN`). We err on the
/// side of over-redaction — pure benign keys like `PORT` or `LOG_LEVEL`
/// don't trigger, but anything with `AUTH` / `KEY` / `TOKEN` / etc. does.
pub fn redact_env_secrets(env: HashMap<String, String>) -> HashMap<String, String> {
    env.into_iter()
        .map(|(k, v)| {
            let upper = k.to_uppercase();
            let is_secret = SECRET_KEY_SUBSTRINGS
                .iter()
                .any(|needle| upper.contains(needle));
            let value = if is_secret {
                REDACTED_VALUE.to_string()
            } else {
                v
            };
            (k, value)
        })
        .collect()
}

/// Canonical constructor: parse a raw runtime JSON payload into `AgentDetail`,
/// applying `redact_env_secrets` before returning.
///
/// Lives as a free function (not `AgentDetail::from_json`) so callers can
/// see the redaction step at the import site (`use cli::dto::{AgentDetail,
/// agent_detail_from_value}`) — it's the only blessed way to materialize an
/// `AgentDetail` with sanitized env. Constructing the struct directly is
/// allowed (for tests) but the redaction is your responsibility then.
pub fn agent_detail_from_value(
    value: &serde_json::Value,
) -> Result<AgentDetail, serde_json::Error> {
    let mut detail: AgentDetail = serde_json::from_value(value.clone())?;
    detail.env = redact_env_secrets(std::mem::take(&mut detail.env));
    Ok(detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Default `list-agents` view must drop sensitive fields on both read
    /// (deserialize from a full runtime payload) and write (serialize back
    /// to JSON for stdout). Combined into one test because the property is
    /// symmetric: each direction proves the field is genuinely absent.
    #[test]
    fn agent_view_omits_sensitive_fields() {
        let raw = serde_json::json!({
            "id": "agent-1",
            "handler": "alice",
            "display_name": "Alice",
            "status": "idle",
            "messages_processed": 7,
            "repo_path": "/abs/path/to/repo",
            "system_prompt": "You are a helpful agent.",
            "env": {"API_KEY": "secret-value"},
        });

        let view: AgentView = serde_json::from_value(raw).expect("AgentView ignores extra fields");
        assert_eq!(view.id, "agent-1");
        assert_eq!(view.handler, "alice");
        assert_eq!(view.messages_processed, 7);

        let out = serde_json::to_value(&view).expect("serialize AgentView");
        let obj = out.as_object().expect("agent view serializes as object");
        for forbidden in [
            "repo_path",
            "system_prompt",
            "env",
            "session_usage",
            "usage_summary",
            "introduction",
            "error_message",
            "llm_provider",
            "llm_model",
        ] {
            assert!(
                !obj.contains_key(forbidden),
                "serialized AgentView leaked sensitive field {forbidden}: {obj:?}",
            );
        }
    }

    /// `skip_serializing_if = "Option::is_none"` must actually drop None
    /// fields from JSON output — otherwise downstream consumers see noisy
    /// `"key": null` entries the schema doesn't document.
    #[test]
    fn agent_view_optional_fields_skipped_when_none() {
        let view = AgentView {
            id: "agent-2".into(),
            handler: "bob".into(),
            display_name: "Bob".into(),
            status: "idle".into(),
            last_activity: None,
            messages_processed: 0,
            provider: None,
            model: None,
        };
        let out = serde_json::to_value(&view).expect("serialize");
        let obj = out.as_object().expect("object");
        for skipped in ["last_activity", "provider", "model"] {
            assert!(
                !obj.contains_key(skipped),
                "expected {skipped} to be skipped when None: {obj:?}",
            );
        }
        // Sanity check the required fields *are* present so we know the test
        // isn't trivially passing on an empty serialization.
        assert!(obj.contains_key("id"));
        assert!(obj.contains_key("handler"));
        assert!(obj.contains_key("display_name"));
        assert!(obj.contains_key("status"));
        assert!(obj.contains_key("messages_processed"));
    }

    /// `--detailed` view must surface `repo_path` so operators can see where
    /// the agent's clone lives on disk.
    #[test]
    fn agent_detail_preserves_repo_path() {
        let raw = serde_json::json!({
            "id": "agent-1",
            "handler": "alice",
            "display_name": "Alice",
            "status": "idle",
            "messages_processed": 0,
            "repo_path": "/abs/path",
        });
        let detail: AgentDetail = serde_json::from_value(raw).expect("deserialize");
        assert_eq!(detail.repo_path, "/abs/path");
    }

    /// Core redaction property: secret-shaped keys swap to "<redacted>",
    /// benign keys preserve their values exactly. Tests multiple substrings
    /// in one map so a regression in `SECRET_KEY_SUBSTRINGS` is caught here.
    #[test]
    fn redact_env_secrets_replaces_secret_shaped_keys() {
        let mut env = HashMap::new();
        env.insert("API_KEY".to_string(), "real".to_string());
        env.insert("DATABASE_PASSWORD".to_string(), "real".to_string());
        env.insert("DEBUG".to_string(), "1".to_string());
        env.insert("NORMAL_VAR".to_string(), "ok".to_string());

        let redacted = redact_env_secrets(env);
        assert_eq!(redacted.get("API_KEY"), Some(&"<redacted>".to_string()));
        assert_eq!(
            redacted.get("DATABASE_PASSWORD"),
            Some(&"<redacted>".to_string())
        );
        assert_eq!(redacted.get("DEBUG"), Some(&"1".to_string()));
        assert_eq!(redacted.get("NORMAL_VAR"), Some(&"ok".to_string()));
    }

    /// Lowercase keys still redact because we uppercase before matching. If
    /// someone writes `api_key=...` in `.env`, that's no less sensitive than
    /// `API_KEY=...`.
    #[test]
    fn redact_env_secrets_case_insensitive() {
        let mut env = HashMap::new();
        env.insert("api_key".to_string(), "real".to_string());
        let redacted = redact_env_secrets(env);
        assert_eq!(redacted.get("api_key"), Some(&"<redacted>".to_string()));
    }

    /// A key hitting multiple needles (`AUTH` and `TOKEN`) shouldn't break
    /// the iteration — `any()` short-circuits, but we want the test pinned
    /// against future refactors that might do something fancier.
    #[test]
    fn redact_env_secrets_substring_match() {
        let mut env = HashMap::new();
        env.insert("AUTH_TOKEN".to_string(), "real".to_string());
        let redacted = redact_env_secrets(env);
        assert_eq!(redacted.get("AUTH_TOKEN"), Some(&"<redacted>".to_string()));
    }

    /// Substring match in the middle of a key — `RETRY_TOKEN_DELAY` could
    /// look benign but the value might legitimately be a secret-adjacent
    /// rate-limit config that leaks deployment shape. We redact it; the
    /// false-positive cost is "operator can't see a retry delay in logs"
    /// which is recoverable.
    #[test]
    fn redact_env_secrets_token_in_middle() {
        let mut env = HashMap::new();
        env.insert("RETRY_TOKEN_DELAY".to_string(), "30".to_string());
        let redacted = redact_env_secrets(env);
        assert_eq!(
            redacted.get("RETRY_TOKEN_DELAY"),
            Some(&"<redacted>".to_string())
        );
    }

    /// Negative case — keys that don't hit any substring keep their value.
    /// This pins the redaction's "boring keys pass through" guarantee.
    #[test]
    fn redact_env_secrets_neutral_keys_pass_through() {
        let mut env = HashMap::new();
        env.insert("PORT".to_string(), "8080".to_string());
        env.insert("LOG_LEVEL".to_string(), "info".to_string());
        let redacted = redact_env_secrets(env);
        assert_eq!(redacted.get("PORT"), Some(&"8080".to_string()));
        assert_eq!(redacted.get("LOG_LEVEL"), Some(&"info".to_string()));
    }

    /// End-to-end constructor check: raw runtime JSON → `AgentDetail` with
    /// env values sanitized. This is the only path a real command should
    /// use; if it ever skips redaction, this test fails loud.
    #[test]
    fn agent_detail_redaction_via_constructor() {
        let raw = serde_json::json!({
            "id": "agent-3",
            "handler": "carol",
            "display_name": "Carol",
            "status": "running",
            "messages_processed": 12,
            "repo_path": "/abs/carol",
            "env": {
                "API_KEY": "secret",
                "PORT": "8080",
            }
        });
        let detail = agent_detail_from_value(&raw).expect("from_value");
        assert_eq!(detail.env.get("API_KEY"), Some(&"<redacted>".to_string()));
        assert_eq!(detail.env.get("PORT"), Some(&"8080".to_string()));
    }

    /// Round-trip discipline for the status DTO — if a field is added later
    /// but the impl forgets to wire it through, serialize→deserialize will
    /// diverge and this catches it.
    #[test]
    fn runtime_status_roundtrip() {
        let original = RuntimeStatus {
            runtime_id: "rt-xyz".into(),
            version: "0.1.2".into(),
            uptime_secs: 3600,
            workspaces_count: 2,
            agents_total: 5,
        };
        let json = serde_json::to_value(&original).expect("serialize");
        let parsed: RuntimeStatus = serde_json::from_value(json).expect("deserialize");
        assert_eq!(original, parsed);
    }

    /// Error envelope must accept the structured code path the runtime
    /// uses for permanent failures like `handler_conflict`.
    #[test]
    fn error_response_with_code() {
        let raw = serde_json::json!({
            "ok": false,
            "error": "foo",
            "error_code": "handler_conflict",
        });
        let parsed: ErrorResponse = serde_json::from_value(raw).expect("deserialize");
        assert!(!parsed.ok);
        assert_eq!(parsed.error, "foo");
        assert_eq!(parsed.error_code.as_deref(), Some("handler_conflict"));
        assert!(parsed.preflight_detail.is_none());
    }

    /// Some runtime error paths omit `error_code` (HTTP 4xx with no
    /// structured signal). The CLI must still parse those — falling back to
    /// the `error` string is the documented behavior.
    #[test]
    fn error_response_without_code() {
        let raw = serde_json::json!({
            "ok": false,
            "error": "foo",
        });
        let parsed: ErrorResponse = serde_json::from_value(raw).expect("deserialize");
        assert!(parsed.error_code.is_none());
        assert!(parsed.preflight_detail.is_none());
        assert_eq!(parsed.error, "foo");
        assert!(!parsed.ok);
    }

    /// T7: ErrorResponse must round-trip a nested `preflight_detail` so the
    /// runtime's provisioning-preflight failure surface stays parseable on
    /// both directions of the wire (server emit, CLI re-emit).
    ///
    /// The shape mirrors `ErrorBody::with_preflight` in server `http.rs` —
    /// keys at the top level (`ok` / `error` / `error_code`) plus the nested
    /// `preflight_detail` object with the `PreflightResult` field set.
    #[test]
    fn error_response_with_preflight_detail_roundtrip() {
        let raw = serde_json::json!({
            "ok": false,
            "error": "model not found",
            "error_code": "provision_preflight_failed",
            "preflight_detail": {
                "available": false,
                "provider": "claude",
                "version": "1.2.3",
                "model_used": "bogus-model",
                "duration_ms": 245,
                "output_preview": "API returned: model 'bogus-model' not found",
                "error": "model not found",
                "error_kind": "other"
            }
        });
        let parsed: ErrorResponse =
            serde_json::from_value(raw.clone()).expect("deserialize ErrorResponse");

        assert!(!parsed.ok);
        assert_eq!(parsed.error, "model not found");
        assert_eq!(
            parsed.error_code.as_deref(),
            Some("provision_preflight_failed")
        );
        let pf = parsed
            .preflight_detail
            .as_ref()
            .expect("preflight_detail must be parsed");
        assert_eq!(pf.provider, "claude");
        assert!(!pf.available);
        assert_eq!(pf.version.as_deref(), Some("1.2.3"));
        assert_eq!(pf.model_used.as_deref(), Some("bogus-model"));
        assert_eq!(pf.duration_ms, 245);
        assert_eq!(
            pf.output_preview.as_deref(),
            Some("API returned: model 'bogus-model' not found")
        );
        assert_eq!(pf.error.as_deref(), Some("model not found"));
        assert_eq!(pf.error_kind, Some(crate::preflight::ErrorKind::Other));

        // Round-trip: re-serialize and confirm the shape is stable
        // (`skip_serializing_if = Option::is_none` must not drop the present
        // detail; field name must stay `preflight_detail` to match server).
        let reserialized =
            serde_json::to_value(&parsed).expect("serialize ErrorResponse with detail");
        let obj = reserialized
            .as_object()
            .expect("serialized ErrorResponse is an object");
        assert!(
            obj.contains_key("preflight_detail"),
            "preflight_detail must survive re-serialize: {obj:?}",
        );
        let nested = obj["preflight_detail"]
            .as_object()
            .expect("preflight_detail is an object after re-serialize");
        assert_eq!(nested["provider"], serde_json::json!("claude"));
        assert_eq!(nested["error_kind"], serde_json::json!("other"));
    }

    /// `add-agent` response is the smallest typed DTO we ship; round-trip
    /// guards against future drift in field names.
    #[test]
    fn add_agent_response_roundtrip() {
        let original = AddAgentResponse {
            ok: true,
            id: "agent-xyz".into(),
        };
        let json = serde_json::to_value(&original).expect("serialize");
        let parsed: AddAgentResponse = serde_json::from_value(json).expect("deserialize");
        assert_eq!(original, parsed);
    }
}
