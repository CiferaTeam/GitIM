//! `update-agent` subcommand — patch editable fields on an existing agent.
//!
//! Mirrors `PATCH /workspaces/{slug}/agents/{id}` (see
//! `http.rs::AgentUpdateRequest`). The runtime side uses three-state
//! semantics for the string fields (absent / null / set) via the custom
//! `deser_triple_option` deserializer:
//!
//!   - absent key → `None`         → no-op
//!   - explicit `null` → `Some(None)` → clear the field
//!   - `"s"`            → `Some(Some(s))` → set to `"s"`
//!
//! **v1 only supports omit (no-op) and set.** Clearing-to-null is rarely
//! needed in practice and exposing it would either require dedicated
//! `--clear-*` flags or some "empty string means clear" footgun. We pick
//! neither today — v2 can add `--clear-system-prompt` / etc. as needed.
//!
//! Field-omission shape: the body builder only emits keys for the fields the
//! user explicitly set. Anything they didn't pass stays out of the JSON
//! object entirely. This relies on serde's `#[serde(default)]` on the
//! request struct mapping missing-key to `None` (no-op) on the runtime side.
//!
//! Body construction lives in the pure helper `build_update_body` so the
//! wire shape is auditable without spinning up a router. The HTTP layer
//! is a thin pass-through over `Client::patch`.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::json;

use crate::cli::http::{CliError, Client, LONG_REQUEST_TIMEOUT};
use crate::cli::workspace::resolve_workspace;
use crate::http::AGENT_FILE_MAX_BYTES;

/// Bundled args from the clap subcommand. Mirrors `Command::UpdateAgent` in
/// `bin/runtime.rs` field-for-field — the destructuring site there feeds
/// straight into this struct.
#[derive(Debug)]
pub struct Args {
    pub workspace: Option<String>,
    pub id: String,
    pub display_name: Option<String>,
    pub system_prompt: Option<String>,
    pub system_prompt_file: Option<PathBuf>,
    pub model: Option<String>,
    /// Replacement effort level (Claude only). Empty string clears it.
    pub effort: Option<String>,
    pub introduction: Option<String>,
    /// Raw `KEY=VALUE` entries from repeated `--env`. Validated and split
    /// inside `run` so the error message is `CliError::InvalidConfig`
    /// (exit 1) rather than clap's stderr-only parse failure.
    pub env: Vec<String>,
    pub dotenv_file: Option<PathBuf>,
    /// When true, wipes the agent's session state via the HTTP handler.
    /// Maps to `clear_session: true` in the PATCH body.
    pub clear_session: bool,
}

/// Entry point. Sequence:
///   1. Resolve `--system-prompt-file` → string (cap 64 KB) — clap already
///      enforces `conflicts_with = system_prompt_file`, so at most one of
///      the two is `Some` here.
///   2. Resolve `--dotenv-file` → string (cap 64 KB).
///   3. Parse `--env KEY=VALUE` repeats into a `HashMap`.
///   4. Require **at least one** update field. An empty PATCH is almost
///      certainly a user mistake; reject locally rather than burning a
///      no-op round-trip.
///   5. Resolve workspace slug (auto-pick when exactly one configured).
///   6. Build PATCH body via `build_update_body` and call
///      `/workspaces/{slug}/agents/{id}`.
///   7. Print the runtime's response verbatim to stdout.
pub async fn run(client: &Client, args: Args) -> Result<i32, CliError> {
    // ── Phase 1: client-side validation + file reads ────────────────────
    let system_prompt = resolve_optional_file(
        &args.system_prompt,
        &args.system_prompt_file,
        "system_prompt_file",
    )?;
    let dotenv = read_capped_file(args.dotenv_file.as_deref(), "dotenv_file")?;
    let env_map = parse_env_entries(&args.env)?;

    // At least one update field must be set. An "update nothing" call would
    // succeed at the runtime (every field maps to None / no-op), but it's
    // never what the user intended. Fail loud at the CLI boundary.
    if system_prompt.is_none()
        && args.display_name.is_none()
        && args.model.is_none()
        && args.effort.is_none()
        && args.introduction.is_none()
        && env_map.is_none()
        && dotenv.is_none()
        && !args.clear_session
    {
        return Err(CliError::InvalidConfig(
            "no update fields specified; pass at least one of \
             --display-name, --system-prompt, --model, --effort, --introduction, --env, --dotenv-file, --clear-session"
                .to_string(),
        ));
    }

    // ── Phase 2: HTTP composition ───────────────────────────────────────
    let slug = resolve_workspace(client, args.workspace.as_deref()).await?;

    let body = build_update_body(BuildArgs {
        display_name: args.display_name.as_deref(),
        system_prompt: system_prompt.as_deref(),
        model: args.model.as_deref(),
        effort: args.effort.as_deref(),
        introduction: args.introduction.as_deref(),
        env: env_map.as_ref(),
        dotenv: dotenv.as_deref(),
        clear_session: args.clear_session,
    });

    // `update-agent` opts into the long-form timeout: the runtime handler
    // can write up to 64KB dotenv content to disk plus do the me.json
    // merge + commit, neither of which we can bound below the wall clock.
    // System-prompt-only updates would fit the fast-verb 30s default, but
    // routing all `update-agent` calls through the same envelope keeps the
    // CLI surface consistent with `add-agent` and avoids a per-flag branch
    // here. See `LONG_REQUEST_TIMEOUT` doc.
    let response = client
        .patch_with_timeout(
            &format!("/workspaces/{slug}/agents/{}", args.id),
            &body,
            LONG_REQUEST_TIMEOUT,
        )
        .await?;

    // ── Phase 3: response handling ──────────────────────────────────────
    // The runtime returns whatever shape `agents_patch` produces today
    // (typically the patched `AgentInfo`). We don't try to project it into
    // a typed DTO here — the patch handler's response shape isn't
    // load-bearing for the CLI's exit contract, and forwarding the raw
    // JSON keeps the surface compatible if the runtime ever extends it.
    let out = serde_json::to_string(&response)
        .map_err(|e| CliError::Parse(format!("serialize update response: {e}")))?;
    println!("{out}");
    Ok(0)
}

/// Resolve the inline/file pair for `system_prompt`. Clap's `conflicts_with`
/// means we never see both `Some`; the file branch reuses the shared
/// `read_capped_file` helper for the size check + read.
fn resolve_optional_file(
    inline: &Option<String>,
    file: &Option<PathBuf>,
    label: &str,
) -> Result<Option<String>, CliError> {
    if let Some(value) = inline {
        return Ok(Some(value.clone()));
    }
    read_capped_file(file.as_deref(), label)
}

/// stat → size-check → read. Centralized so `--system-prompt-file` and
/// `--dotenv-file` share identical semantics: 64 KB cap, `InvalidConfig`
/// on overflow, `InvalidConfig` on any I/O error (file-not-found, perm
/// denied, etc.).
fn read_capped_file(
    path: Option<&std::path::Path>,
    label: &str,
) -> Result<Option<String>, CliError> {
    let Some(path) = path else {
        return Ok(None);
    };
    let metadata = std::fs::metadata(path)
        .map_err(|e| CliError::InvalidConfig(format!("{label} stat: {e}")))?;
    if metadata.len() > AGENT_FILE_MAX_BYTES {
        return Err(CliError::InvalidConfig(format!("{label} exceeds 64KB")));
    }
    let content = std::fs::read_to_string(path)
        .map_err(|e| CliError::InvalidConfig(format!("{label} read: {e}")))?;
    Ok(Some(content))
}

/// Parse `--env KEY=VALUE` repeats. Returns `Ok(None)` when the slice is
/// empty so callers can distinguish "user didn't pass --env at all"
/// (no-op) from "user passed an explicit empty env" (which would require
/// dedicated `--clear-env` v2 flag, not modeled here).
///
/// Identical splitting rules to `cmd_add_agent::parse_env_entries`:
/// first `=` separates key/value, the rest of the value can contain
/// arbitrary bytes (URLs, base64, query strings).
fn parse_env_entries(entries: &[String]) -> Result<Option<HashMap<String, String>>, CliError> {
    if entries.is_empty() {
        return Ok(None);
    }
    let mut out = HashMap::with_capacity(entries.len());
    for entry in entries {
        let Some((key, value)) = entry.split_once('=') else {
            return Err(CliError::InvalidConfig(format!(
                "invalid --env entry: {entry}, expected KEY=VALUE"
            )));
        };
        out.insert(key.to_string(), value.to_string());
    }
    Ok(Some(out))
}

/// Borrowed view of the patch fields. Pulled out as its own type so
/// `build_update_body` stays a pure single-arg function.
struct BuildArgs<'a> {
    display_name: Option<&'a str>,
    system_prompt: Option<&'a str>,
    model: Option<&'a str>,
    effort: Option<&'a str>,
    introduction: Option<&'a str>,
    env: Option<&'a HashMap<String, String>>,
    dotenv: Option<&'a str>,
    clear_session: bool,
}

/// Pure body builder. Each `Option::None` here corresponds to "user
/// didn't pass the flag" → key is **omitted entirely** from the JSON
/// object. The runtime's `deser_triple_option` maps a missing key to
/// `None` (no-op).
///
/// We never emit `null` for the string fields in v1 — that's the
/// "clear-to-null" path which would map to `Some(None)` server-side and
/// actually wipe the field. See module doc for the rationale.
fn build_update_body(args: BuildArgs<'_>) -> serde_json::Value {
    let mut body = serde_json::Map::new();
    if let Some(name) = args.display_name {
        body.insert("display_name".to_string(), json!(name));
    }
    if let Some(sp) = args.system_prompt {
        body.insert("system_prompt".to_string(), json!(sp));
    }
    if let Some(m) = args.model {
        body.insert("model".to_string(), json!(m));
    }
    // Effort is forwarded verbatim, including an explicit empty string — the
    // runtime maps `""` to "clear" (its triple-option handler treats empty as
    // None). This is the only field where we intentionally emit `""`.
    if let Some(e) = args.effort {
        body.insert("effort".to_string(), json!(e));
    }
    if let Some(i) = args.introduction {
        body.insert("introduction".to_string(), json!(i));
    }
    if let Some(env) = args.env {
        body.insert("env".to_string(), json!(env));
    }
    if let Some(de) = args.dotenv {
        body.insert("dotenv".to_string(), json!(de));
    }
    if args.clear_session {
        body.insert("clear_session".to_string(), json!(true));
    }
    serde_json::Value::Object(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Single-field update emits exactly that one key. Other keys must be
    /// absent (not null) so the runtime's `#[serde(default)]` keeps them
    /// at `None` (no-op).
    #[test]
    fn build_body_system_prompt_only() {
        let body = build_update_body(BuildArgs {
            display_name: None,
            system_prompt: Some("new prompt"),
            model: None,
            effort: None,
            introduction: None,
            env: None,
            dotenv: None,
            clear_session: false,
        });
        let obj = body.as_object().expect("body is object");
        assert_eq!(obj["system_prompt"], "new prompt");
        for omitted in ["model", "introduction", "env", "dotenv"] {
            assert!(
                !obj.contains_key(omitted),
                "expected {omitted} omitted, got: {obj:?}"
            );
        }
        assert_eq!(obj.len(), 1, "exactly one key");
    }

    #[test]
    fn build_body_display_name_only() {
        let body = build_update_body(BuildArgs {
            display_name: Some("Alice W"),
            system_prompt: None,
            model: None,
            effort: None,
            introduction: None,
            env: None,
            dotenv: None,
            clear_session: false,
        });
        let obj = body.as_object().expect("body is object");
        assert_eq!(obj["display_name"], "Alice W");
        assert_eq!(obj.len(), 1);
    }

    /// Effort set forwards the level; an explicit empty string forwards `""`
    /// so the runtime's triple-option handler clears the field.
    #[test]
    fn build_body_effort_set_and_clear() {
        let set = build_update_body(BuildArgs {
            display_name: None,
            system_prompt: None,
            model: None,
            effort: Some("max"),
            introduction: None,
            env: None,
            dotenv: None,
            clear_session: false,
        });
        assert_eq!(set["effort"], "max");

        let clear = build_update_body(BuildArgs {
            display_name: None,
            system_prompt: None,
            model: None,
            effort: Some(""),
            introduction: None,
            env: None,
            dotenv: None,
            clear_session: false,
        });
        assert_eq!(clear["effort"], "");
    }

    /// All five fields set in one call — the e2e shape the CLI must
    /// support for "patch everything at once".
    #[test]
    fn build_body_all_fields_present() {
        let mut env = HashMap::new();
        env.insert("KEY".to_string(), "VAL".to_string());
        let body = build_update_body(BuildArgs {
            display_name: None,
            system_prompt: Some("X"),
            model: Some("Y"),
            effort: Some("low"),
            introduction: Some("Z"),
            env: Some(&env),
            dotenv: Some("FOO=bar\n"),
            clear_session: false,
        });
        let obj = body.as_object().expect("body is object");
        assert_eq!(obj["system_prompt"], "X");
        assert_eq!(obj["model"], "Y");
        assert_eq!(obj["effort"], "low");
        assert_eq!(obj["introduction"], "Z");
        let env_obj = obj["env"].as_object().expect("env is object");
        assert_eq!(env_obj["KEY"], "VAL");
        assert_eq!(obj["dotenv"], "FOO=bar\n");
        assert_eq!(obj.len(), 6);
    }

    /// An empty BuildArgs produces `{}` — the body builder doesn't enforce
    /// "at least one field" itself; `run` does that one layer up so the
    /// builder stays pure and reusable.
    #[test]
    fn build_body_all_none_emits_empty_object() {
        let body = build_update_body(BuildArgs {
            display_name: None,
            system_prompt: None,
            model: None,
            effort: None,
            introduction: None,
            env: None,
            dotenv: None,
            clear_session: false,
        });
        let obj = body.as_object().expect("body is object");
        assert!(obj.is_empty(), "all-None must produce empty object");
    }

    /// Empty env map still produces an `env: {}` key — different semantic
    /// from "user didn't pass --env at all" (`env: None`, omitted). The
    /// runtime treats `Some({})` as "remove env field" and `None` as
    /// no-op. v1 CLI doesn't expose `--clear-env`, but the body builder
    /// supports the shape if a future flag wants to use it.
    #[test]
    fn build_body_empty_env_map_emits_empty_object_value() {
        let env = HashMap::new();
        let body = build_update_body(BuildArgs {
            display_name: None,
            system_prompt: None,
            model: None,
            effort: None,
            introduction: None,
            env: Some(&env),
            dotenv: None,
            clear_session: false,
        });
        assert!(body["env"].as_object().expect("env object").is_empty());
    }

    /// `env: None` → key entirely absent. This is the path `run` takes
    /// when the user didn't pass any `--env` flags.
    #[test]
    fn build_body_no_env_omits_key() {
        let body = build_update_body(BuildArgs {
            display_name: None,
            system_prompt: Some("X"),
            model: None,
            effort: None,
            introduction: None,
            env: None,
            dotenv: None,
            clear_session: false,
        });
        assert!(!body.as_object().unwrap().contains_key("env"));
    }

    /// Multi-entry env round-trips into a JSON object map (not a list of
    /// pairs / a JSON string). Catches a regression where we'd box the
    /// HashMap wrong.
    #[test]
    fn build_body_env_passthrough() {
        let mut env = HashMap::new();
        env.insert("DEBUG".to_string(), "1".to_string());
        env.insert("PORT".to_string(), "8080".to_string());
        let body = build_update_body(BuildArgs {
            display_name: None,
            system_prompt: None,
            model: None,
            effort: None,
            introduction: None,
            env: Some(&env),
            dotenv: None,
            clear_session: false,
        });
        let env_obj = body["env"].as_object().expect("env is object");
        assert_eq!(env_obj["DEBUG"], "1");
        assert_eq!(env_obj["PORT"], "8080");
    }

    #[test]
    fn parse_env_entries_happy_path() {
        let entries = vec!["KEY=value".to_string(), "OTHER=more".to_string()];
        let map = parse_env_entries(&entries).expect("parses").expect("Some");
        assert_eq!(map.get("KEY"), Some(&"value".to_string()));
        assert_eq!(map.get("OTHER"), Some(&"more".to_string()));
    }

    /// Empty slice → `Ok(None)`. Distinguishes "user passed no --env" from
    /// "user passed --env KEY=" so `run` can omit the field correctly.
    #[test]
    fn parse_env_entries_empty_is_none() {
        let entries: Vec<String> = Vec::new();
        let result = parse_env_entries(&entries).expect("parses");
        assert!(result.is_none());
    }

    #[test]
    fn parse_env_entries_value_with_equals() {
        // Real values (URLs, base64, connection strings) routinely contain
        // `=`. split_once only fires on the first occurrence.
        let entries = vec!["URL=https://x.com?a=b&c=d".to_string()];
        let map = parse_env_entries(&entries).expect("parses").expect("Some");
        assert_eq!(map.get("URL"), Some(&"https://x.com?a=b&c=d".to_string()));
    }

    #[test]
    fn parse_env_entries_empty_value_legal() {
        // `--env KEY=` is valid — clears the var in the agent's env.
        let entries = vec!["EMPTY=".to_string()];
        let map = parse_env_entries(&entries).expect("parses").expect("Some");
        assert_eq!(map.get("EMPTY"), Some(&"".to_string()));
    }

    #[test]
    fn parse_env_entries_missing_equals_errors() {
        let entries = vec!["MALFORMED".to_string()];
        let err = parse_env_entries(&entries).expect_err("must error on no `=`");
        assert!(matches!(err, CliError::InvalidConfig(_)));
        let msg = err.to_string();
        assert!(msg.contains("MALFORMED"));
        assert!(msg.contains("KEY=VALUE"));
    }

    #[test]
    fn resolve_optional_file_inline_wins() {
        let v = resolve_optional_file(&Some("inline".to_string()), &None, "system_prompt_file")
            .expect("ok");
        assert_eq!(v.as_deref(), Some("inline"));
    }

    #[test]
    fn resolve_optional_file_none_when_unset() {
        let v = resolve_optional_file(&None, &None, "system_prompt_file").expect("ok");
        assert!(v.is_none());
    }

    #[test]
    fn resolve_optional_file_reads_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("prompt.txt");
        std::fs::write(&path, "from-file").unwrap();
        let v = resolve_optional_file(&None, &Some(path), "system_prompt_file").expect("ok");
        assert_eq!(v.as_deref(), Some("from-file"));
    }

    /// 65 KB rejects before any read. Mirrors the cap on `add-agent` and
    /// matches the runtime's 64 KB limit on `dotenv`.
    #[test]
    fn read_capped_file_rejects_oversize() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("huge.txt");
        std::fs::write(&path, vec![b'a'; 65 * 1024]).unwrap();
        let err = read_capped_file(Some(&path), "dotenv_file").expect_err("must error");
        assert!(matches!(err, CliError::InvalidConfig(_)));
        assert!(err.to_string().contains("64KB"));
        assert!(err.to_string().contains("dotenv_file"));
    }

    #[test]
    fn read_capped_file_missing_file_errors() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let missing = tmp.path().join("does-not-exist");
        let err = read_capped_file(Some(&missing), "dotenv_file").expect_err("must error");
        assert!(matches!(err, CliError::InvalidConfig(_)));
    }

    #[test]
    fn read_capped_file_under_cap_passes() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("ok.txt");
        std::fs::write(&path, "hello").unwrap();
        let v = read_capped_file(Some(&path), "dotenv_file").expect("ok");
        assert_eq!(v.as_deref(), Some("hello"));
    }

    // ── clear_session tests ─────────────────────────────────────────────

    /// `--clear-session` → body contains `clear_session: true`.
    #[test]
    fn build_body_clear_session_true() {
        let body = build_update_body(BuildArgs {
            display_name: None,
            system_prompt: None,
            model: None,
            effort: None,
            introduction: None,
            env: None,
            dotenv: None,
            clear_session: true,
        });
        let obj = body.as_object().expect("body is object");
        assert_eq!(obj["clear_session"], true);
        assert_eq!(obj.len(), 1, "only clear_session key");
    }

    /// When `clear_session` is false, the key must be absent — not `false`.
    /// The runtime maps a missing key to no-op; an explicit `false` would
    /// still be no-op, but omitting it keeps the body minimal.
    #[test]
    fn build_body_clear_session_false_omits_key() {
        let body = build_update_body(BuildArgs {
            display_name: None,
            system_prompt: Some("X"),
            model: None,
            effort: None,
            introduction: None,
            env: None,
            dotenv: None,
            clear_session: false,
        });
        let obj = body.as_object().expect("body is object");
        assert!(
            !obj.contains_key("clear_session"),
            "must be absent when false"
        );
        assert_eq!(obj["system_prompt"], "X");
    }

    /// `clear_session=true` can coexist with other fields.
    #[test]
    fn build_body_clear_session_with_model() {
        let body = build_update_body(BuildArgs {
            display_name: None,
            system_prompt: None,
            model: Some("claude-opus-4-7"),
            effort: None,
            introduction: None,
            env: None,
            dotenv: None,
            clear_session: true,
        });
        let obj = body.as_object().expect("body is object");
        assert_eq!(obj["clear_session"], true);
        assert_eq!(obj["model"], "claude-opus-4-7");
        assert_eq!(obj.len(), 2);
    }
}
