//! `add-agent` subcommand — provision a new agent in a workspace.
//!
//! Mirrors `POST /workspaces/{slug}/agents/add` (see `http.rs::AgentAddRequest`).
//! The CLI surface is a flat set of clap flags that get bundled into `Args` and
//! handed to `run`; `run` itself owns the validation that's cheaper to do
//! client-side (env parsing, file size cap, provider/llm cross-field rules)
//! and lets the runtime own everything that requires repo or daemon state.
//!
//! Body building lives in a pure helper (`build_add_agent_body`) so the wire
//! shape is unit-testable without spinning a router. The CLI's contract with
//! the runtime — which optional fields to omit vs. send as `null` — is the
//! interesting bit; routing the assembly through this one function keeps that
//! contract auditable in one place.
//!
//! `--system-prompt` and `--system-prompt-file` are clap-level
//! `conflicts_with`, so the only way a `system_prompt_file` reaches `run()` is
//! if `system_prompt` is `None`. We still cap the file at 64KB defensively —
//! a multi-megabyte prompt smells like a wrong-path mistake (e.g. pointing at
//! a transcript) more than a real prompt, and rejecting it client-side beats
//! getting a generic 4xx after the runtime starts cloning.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::json;

use crate::cli::dto::AddAgentResponse;
use crate::cli::http::{CliError, Client, LONG_REQUEST_TIMEOUT};
use crate::cli::workspace::resolve_workspace;
use crate::http::AGENT_FILE_MAX_BYTES;

/// Bundled args from the clap subcommand. Keeping this as a struct (rather
/// than 12 positional parameters) makes the `bin/runtime.rs` dispatch site
/// readable — the destructuring there mirrors this struct field-for-field.
#[derive(Debug)]
pub struct Args {
    pub workspace: Option<String>,
    pub handler: String,
    pub display_name: String,
    pub provider: String,
    pub model: Option<String>,
    /// Effort level (Claude only): low / medium / high / xhigh / max.
    pub effort: Option<String>,
    pub system_prompt: Option<String>,
    pub system_prompt_file: Option<PathBuf>,
    /// Raw `KEY=VALUE` entries from `--env`. Validated and split inside `run`
    /// so the error message can be `CliError::InvalidConfig` (exit 1) instead
    /// of clap's stderr-only parse failure.
    pub env: Vec<String>,
    pub introduction: Option<String>,
    /// When true, runtime is told to skip the #general auto-join. When false
    /// we omit the field entirely so the runtime default ("join general")
    /// applies — see `build_add_agent_body` for the omit-vs-send-false logic.
    pub no_join_general: bool,
    pub llm_provider: Option<String>,
    pub llm_model: Option<String>,
}

/// Entry point. Sequence:
///   1. Resolve `--system-prompt-file` to string (capped at 64KB)
///   2. Parse `--env KEY=VALUE` flags into a `HashMap`
///   3. Cross-field validate Hermes-only flags (`--llm-provider`/`--llm-model`)
///   4. Resolve workspace slug (auto-pick if exactly one, else require flag)
///   5. Build POST body and call `/workspaces/{slug}/agents/add`
///   6. Parse the `{ok, id}` response and print as JSON
pub async fn run(client: &Client, args: Args) -> Result<i32, CliError> {
    // ── Phase 1: client-side validation ─────────────────────────────────
    //
    // All of this runs before any HTTP call so we don't waste a roundtrip
    // (or worse, leave a partial-provision state) on input we could have
    // caught locally.

    let system_prompt = resolve_system_prompt(&args.system_prompt, &args.system_prompt_file)?;
    let env_map = parse_env_entries(&args.env)?;
    validate_llm_flags(&args.provider, &args.llm_provider, &args.llm_model)?;

    // ── Phase 2: HTTP composition ───────────────────────────────────────
    let slug = resolve_workspace(client, args.workspace.as_deref()).await?;

    let body = build_add_agent_body(BuildArgs {
        handler: &args.handler,
        display_name: &args.display_name,
        provider: &args.provider,
        model: args.model.as_deref(),
        effort: args.effort.as_deref(),
        system_prompt: system_prompt.as_deref(),
        introduction: args.introduction.as_deref(),
        env: &env_map,
        no_join_general: args.no_join_general,
        llm_provider: args.llm_provider.as_deref(),
        llm_model: args.llm_model.as_deref(),
    })?;

    // `add-agent` opts into the long-form timeout: the runtime handler
    // `git clone`s the workspace remote inline before responding, and a
    // realistic GitHub repo over a slow uplink can take minutes. The
    // 30s default would abort mid-clone and leave a half-provisioned
    // state on the runtime side. See `LONG_REQUEST_TIMEOUT` doc.
    let response = client
        .post_with_timeout(
            &format!("/workspaces/{slug}/agents/add"),
            &body,
            LONG_REQUEST_TIMEOUT,
        )
        .await?;

    // ── Phase 3: response handling ──────────────────────────────────────
    let parsed: AddAgentResponse = serde_json::from_value(response.clone())
        .map_err(|e| CliError::Parse(format!("parse AddAgentResponse: {e}")))?;
    let out = serde_json::to_string(&parsed)
        .map_err(|e| CliError::Parse(format!("serialize AddAgentResponse: {e}")))?;
    println!("{out}");
    Ok(0)
}

/// Pick between `--system-prompt` and `--system-prompt-file`. The two flags are
/// `conflicts_with` at the clap level so they can't both be Some here; this
/// function still handles the path-read branch and the size cap.
fn resolve_system_prompt(
    inline: &Option<String>,
    file: &Option<PathBuf>,
) -> Result<Option<String>, CliError> {
    if let Some(prompt) = inline {
        return Ok(Some(prompt.clone()));
    }
    let Some(path) = file else {
        return Ok(None);
    };

    // Check size before reading — `metadata().len()` is one syscall and
    // skips the case where someone hands us /dev/zero or a gigabyte file.
    let metadata = std::fs::metadata(path)
        .map_err(|e| CliError::InvalidConfig(format!("system_prompt_file stat: {e}")))?;
    if metadata.len() > AGENT_FILE_MAX_BYTES {
        return Err(CliError::InvalidConfig(
            "system_prompt_file exceeds 64KB".to_string(),
        ));
    }

    let content = std::fs::read_to_string(path)
        .map_err(|e| CliError::InvalidConfig(format!("system_prompt_file read: {e}")))?;
    Ok(Some(content))
}

/// Parse `--env KEY=VALUE` repeated entries. The `=` is the only required
/// separator; whitespace inside the key or value passes through (the agent's
/// daemon will validate further). An entry without `=` is a typo, not
/// shorthand for "set KEY to empty" — fail loud.
fn parse_env_entries(entries: &[String]) -> Result<HashMap<String, String>, CliError> {
    let mut out = HashMap::with_capacity(entries.len());
    for entry in entries {
        let Some((key, value)) = entry.split_once('=') else {
            return Err(CliError::InvalidConfig(format!(
                "invalid --env entry: {entry}, expected KEY=VALUE"
            )));
        };
        out.insert(key.to_string(), value.to_string());
    }
    Ok(out)
}

/// Hermes is the only provider that consumes `--llm-provider` / `--llm-model`.
/// For other providers, surfacing those flags client-side beats letting the
/// runtime reject — error message can be more specific.
///
/// Inside Hermes mode we only emit a stderr warning when exactly one of the
/// two flags is set, because the runtime's hermes branch already enforces
/// the both-or-neither rule with proper error_code (`missing_llm_provider`),
/// and we want users to see the runtime's structured message rather than
/// double-rejecting at the CLI.
fn validate_llm_flags(
    provider: &str,
    llm_provider: &Option<String>,
    llm_model: &Option<String>,
) -> Result<(), CliError> {
    if provider != "hermes" {
        if llm_provider.is_some() || llm_model.is_some() {
            return Err(CliError::InvalidConfig(
                "--llm-provider/--llm-model are only valid for hermes provider".to_string(),
            ));
        }
        return Ok(());
    }
    // Hermes: warn but don't fail on one-of-two — runtime preflight has the
    // canonical error and message.
    if llm_provider.is_some() != llm_model.is_some() {
        eprintln!(
            "warning: hermes typically requires both --llm-provider and --llm-model, or neither"
        );
    }
    Ok(())
}

/// Borrowed view of the fields used to construct the POST body. Carved out
/// as its own type so `build_add_agent_body` stays a pure, single-arg
/// function (easier to test, easier to extend later without re-threading
/// argument lists through `run`).
struct BuildArgs<'a> {
    handler: &'a str,
    display_name: &'a str,
    provider: &'a str,
    model: Option<&'a str>,
    effort: Option<&'a str>,
    system_prompt: Option<&'a str>,
    introduction: Option<&'a str>,
    env: &'a HashMap<String, String>,
    no_join_general: bool,
    llm_provider: Option<&'a str>,
    llm_model: Option<&'a str>,
}

/// Pure body builder. Tests assert the exact wire shape here so a regression
/// in the field-omission logic doesn't ride into HTTP-layer integration tests.
///
/// Omission rules:
/// - Required: handler, display_name, provider (always present)
/// - Optional fields are omitted entirely when None / empty so the runtime's
///   `#[serde(default)]` defaults take effect. Sending `null` vs. omitting is
///   semantically the same for these fields, but omission keeps the wire
///   payload small and the diff-against-spec readable.
/// - `join_general`: only emitted when the user explicitly passed
///   `--no-join-general`. Default behavior at the runtime is "join", so we
///   simply don't send the field unless we're overriding to false.
fn build_add_agent_body(args: BuildArgs<'_>) -> Result<serde_json::Value, CliError> {
    let mut body = json!({
        "handler": args.handler,
        "display_name": args.display_name,
        "provider": args.provider,
    });
    let obj = body
        .as_object_mut()
        .ok_or_else(|| CliError::InvalidConfig("failed to build agent body".to_string()))?;

    if let Some(model) = args.model {
        obj.insert("model".to_string(), json!(model));
    }
    if let Some(effort) = args.effort {
        obj.insert("effort".to_string(), json!(effort));
    }
    if let Some(prompt) = args.system_prompt {
        obj.insert("system_prompt".to_string(), json!(prompt));
    }
    if let Some(intro) = args.introduction {
        obj.insert("introduction".to_string(), json!(intro));
    }
    if !args.env.is_empty() {
        obj.insert("env".to_string(), json!(args.env));
    }
    if args.no_join_general {
        obj.insert("join_general".to_string(), json!(false));
    }
    if let Some(p) = args.llm_provider {
        obj.insert("llm_provider".to_string(), json!(p));
    }
    if let Some(m) = args.llm_model {
        obj.insert("llm_model".to_string(), json!(m));
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimum viable args — exercises the "every optional field omitted"
    /// path through `build_add_agent_body`. Asserts that we don't accidentally
    /// emit `null` for fields the runtime would otherwise default.
    #[test]
    fn build_body_minimal_omits_optional_fields() {
        let env = HashMap::new();
        let body = build_add_agent_body(BuildArgs {
            handler: "alice",
            display_name: "Alice",
            provider: "claude",
            model: None,
            effort: None,
            system_prompt: None,
            introduction: None,
            env: &env,
            no_join_general: false,
            llm_provider: None,
            llm_model: None,
        })
        .unwrap();
        let obj = body.as_object().expect("body is object");
        assert_eq!(obj["handler"], "alice");
        assert_eq!(obj["display_name"], "Alice");
        assert_eq!(obj["provider"], "claude");
        // Optional fields must not appear at all (not as null) so runtime
        // serde defaults can take over.
        for omitted in [
            "model",
            "effort",
            "system_prompt",
            "introduction",
            "env",
            "join_general",
            "llm_provider",
            "llm_model",
        ] {
            assert!(
                !obj.contains_key(omitted),
                "expected {omitted} omitted from minimal body, got: {obj:?}"
            );
        }
    }

    /// Effort is forwarded verbatim when present (Claude only at the UI layer,
    /// but the wire builder is provider-agnostic — the runtime owns the gate).
    #[test]
    fn build_body_effort_forwarded() {
        let env = HashMap::new();
        let body = build_add_agent_body(BuildArgs {
            handler: "alice",
            display_name: "Alice",
            provider: "claude",
            model: Some("claude-opus-4-8"),
            effort: Some("xhigh"),
            system_prompt: None,
            introduction: None,
            env: &env,
            no_join_general: false,
            llm_provider: None,
            llm_model: None,
        })
        .unwrap();
        assert_eq!(body["effort"], "xhigh");
        assert_eq!(body["model"], "claude-opus-4-8");
    }

    /// Hermes happy path: both LLM flags get forwarded as siblings under
    /// `provider: "hermes"`. The runtime's hermes branch reads these fields
    /// to configure the cloned profile.
    #[test]
    fn build_body_hermes_with_llm_flags() {
        let env = HashMap::new();
        let body = build_add_agent_body(BuildArgs {
            handler: "bot",
            display_name: "Bot",
            provider: "hermes",
            model: None,
            effort: None,
            system_prompt: None,
            introduction: None,
            env: &env,
            no_join_general: false,
            llm_provider: Some("anthropic"),
            llm_model: Some("claude-opus-4-7"),
        })
        .unwrap();
        let obj = body.as_object().expect("body is object");
        assert_eq!(obj["provider"], "hermes");
        assert_eq!(obj["llm_provider"], "anthropic");
        assert_eq!(obj["llm_model"], "claude-opus-4-7");
    }

    /// `--no-join-general` is the only way the field appears on the wire,
    /// and when it does it must be `false` (the runtime treats `Some(false)`
    /// distinctly from `None`).
    #[test]
    fn build_body_no_join_general_emits_false() {
        let env = HashMap::new();
        let body = build_add_agent_body(BuildArgs {
            handler: "bot",
            display_name: "Bot",
            provider: "claude",
            model: None,
            effort: None,
            system_prompt: None,
            introduction: None,
            env: &env,
            no_join_general: true,
            llm_provider: None,
            llm_model: None,
        })
        .unwrap();
        assert_eq!(body["join_general"], false);
    }

    /// Empty env map omits the key — sending `"env": {}` is harmless but
    /// noisy. Keep the wire payload minimal so diffing failed requests stays
    /// readable.
    #[test]
    fn build_body_empty_env_omitted() {
        let env = HashMap::new();
        let body = build_add_agent_body(BuildArgs {
            handler: "bot",
            display_name: "Bot",
            provider: "claude",
            model: None,
            effort: None,
            system_prompt: None,
            introduction: None,
            env: &env,
            no_join_general: false,
            llm_provider: None,
            llm_model: None,
        })
        .unwrap();
        assert!(body.get("env").is_none());
    }

    /// Non-empty env map round-trips through serde_json correctly (HashMap
    /// → Map). Catches a regression where we'd accidentally box the value
    /// as a list of pairs or a JSON string.
    #[test]
    fn build_body_env_passthrough() {
        let mut env = HashMap::new();
        env.insert("DEBUG".to_string(), "1".to_string());
        env.insert("PORT".to_string(), "8080".to_string());
        let body = build_add_agent_body(BuildArgs {
            handler: "bot",
            display_name: "Bot",
            provider: "claude",
            model: None,
            effort: None,
            system_prompt: None,
            introduction: None,
            env: &env,
            no_join_general: false,
            llm_provider: None,
            llm_model: None,
        });
        let body = body.unwrap();
        let env_obj = body["env"].as_object().expect("env is object");
        assert_eq!(env_obj["DEBUG"], "1");
        assert_eq!(env_obj["PORT"], "8080");
    }

    #[test]
    fn parse_env_entries_happy_path() {
        let entries = vec!["KEY=value".to_string(), "OTHER=more".to_string()];
        let map = parse_env_entries(&entries).expect("parses");
        assert_eq!(map.get("KEY"), Some(&"value".to_string()));
        assert_eq!(map.get("OTHER"), Some(&"more".to_string()));
    }

    /// `KEY=value=with=equals` keeps the suffix intact — `split_once` on `=`
    /// only splits at the first occurrence. Real values (URLs, base64,
    /// connection strings) routinely contain `=`.
    #[test]
    fn parse_env_entries_value_with_equals() {
        let entries = vec!["URL=https://x.com?a=b&c=d".to_string()];
        let map = parse_env_entries(&entries).expect("parses");
        assert_eq!(
            map.get("URL"),
            Some(&"https://x.com?a=b&c=d".to_string()),
            "value must preserve all bytes after first '='"
        );
    }

    /// Empty value is legal — `--env DEBUG=` clears an inherited variable.
    /// We don't second-guess this; pass through what the user typed.
    #[test]
    fn parse_env_entries_empty_value() {
        let entries = vec!["EMPTY=".to_string()];
        let map = parse_env_entries(&entries).expect("parses");
        assert_eq!(map.get("EMPTY"), Some(&"".to_string()));
    }

    #[test]
    fn parse_env_entries_missing_equals_errors() {
        let entries = vec!["MALFORMED".to_string()];
        let err = parse_env_entries(&entries).expect_err("must error on no `=`");
        let msg = err.to_string();
        assert!(matches!(err, CliError::InvalidConfig(_)));
        assert!(msg.contains("MALFORMED"));
        assert!(msg.contains("KEY=VALUE"));
    }

    #[test]
    fn validate_llm_flags_non_hermes_with_flags_errors() {
        let err = validate_llm_flags(
            "claude",
            &Some("anthropic".to_string()),
            &Some("claude-opus".to_string()),
        )
        .expect_err("non-hermes + llm flags must error");
        assert!(matches!(err, CliError::InvalidConfig(_)));
        assert!(err.to_string().contains("hermes"));
    }

    #[test]
    fn validate_llm_flags_non_hermes_without_flags_ok() {
        validate_llm_flags("claude", &None, &None).expect("no llm flags, no error");
    }

    #[test]
    fn validate_llm_flags_hermes_both_or_neither_ok() {
        validate_llm_flags("hermes", &None, &None).expect("neither is ok");
        validate_llm_flags(
            "hermes",
            &Some("anthropic".to_string()),
            &Some("claude-opus-4-7".to_string()),
        )
        .expect("both is ok");
    }

    /// Hermes with one-of-two doesn't error (runtime owns the real check);
    /// we just emit a stderr warning. The test verifies we don't accidentally
    /// upgrade it to a hard error.
    #[test]
    fn validate_llm_flags_hermes_one_of_two_warns_only() {
        validate_llm_flags("hermes", &Some("anthropic".to_string()), &None)
            .expect("warn only, not error");
        validate_llm_flags("hermes", &None, &Some("model".to_string()))
            .expect("warn only, not error");
    }

    #[test]
    fn resolve_system_prompt_inline_wins() {
        let prompt = resolve_system_prompt(&Some("inline content".to_string()), &None).expect("ok");
        assert_eq!(prompt.as_deref(), Some("inline content"));
    }

    #[test]
    fn resolve_system_prompt_none_when_unset() {
        let prompt = resolve_system_prompt(&None, &None).expect("ok");
        assert!(prompt.is_none());
    }

    #[test]
    fn resolve_system_prompt_reads_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("prompt.txt");
        std::fs::write(&path, "from-file").expect("write");
        let prompt = resolve_system_prompt(&None, &Some(path)).expect("ok");
        assert_eq!(prompt.as_deref(), Some("from-file"));
    }

    /// The 64KB cap is a defensive guard against pointing at the wrong file
    /// (transcript, log, etc.). Plant a 65KB file and confirm the InvalidConfig
    /// path triggers before any read.
    #[test]
    fn resolve_system_prompt_rejects_oversize_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("huge.txt");
        let content = vec![b'a'; 65 * 1024];
        std::fs::write(&path, &content).expect("write");
        let err = resolve_system_prompt(&None, &Some(path)).expect_err("must error");
        assert!(matches!(err, CliError::InvalidConfig(_)));
        assert!(err.to_string().contains("64KB"));
    }

    /// File-not-found surfaces as `InvalidConfig` (exit 1), not a panic. The
    /// CLI's contract is that user-input failures are recoverable errors.
    #[test]
    fn resolve_system_prompt_missing_file_errors() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let missing = tmp.path().join("does-not-exist");
        let err = resolve_system_prompt(&None, &Some(missing)).expect_err("must error");
        assert!(matches!(err, CliError::InvalidConfig(_)));
    }
}
