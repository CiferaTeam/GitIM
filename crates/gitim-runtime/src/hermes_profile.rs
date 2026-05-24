//! Per-agent hermes profile management.
//!
//! Each gitim agent is paired 1:1 with a hermes profile at
//! `~/.hermes/profiles/gitim-<handler>/`. This module owns the naming
//! convention, path resolution, and shell-out wrappers around the
//! `hermes profile create / delete` CLI.

use std::path::{Path, PathBuf};

use gitim_agent_provider::PromptContext;
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub enum HermesProfileError {
    #[error("home directory not found")]
    HomeDirNotFound,

    #[error("hermes CLI not found in PATH; install hermes or run `hermes setup` first")]
    CliNotFound,

    #[error("{0}")]
    Other(String),
}

/// Returns the hermes profile name for a given agent handler.
/// Profile name format is `gitim-<handler>`.
pub fn profile_name(handler: &str) -> String {
    format!("gitim-{handler}")
}

/// Returns the hermes profile directory path for a given agent handler.
/// Returns `<home>/.hermes/profiles/gitim-<handler>`.
pub fn profile_dir(handler: &str) -> Result<PathBuf, HermesProfileError> {
    let home = dirs::home_dir().ok_or(HermesProfileError::HomeDirNotFound)?;
    Ok(home.join(".hermes/profiles").join(profile_name(handler)))
}

/// Returns true when the user's default hermes profile (the source of
/// `--clone` for new agent profiles) appears to have been set up — i.e.
/// at least one of `.env` (API keys) or `auth.json` (OAuth state) is
/// present. False when the user has installed hermes but never run
/// `hermes setup`, in which case cloning would yield an unusable agent.
///
/// Respects `HERMES_HOME` env override; falls back to `~/.hermes`.
pub fn default_profile_ready() -> bool {
    let home = std::env::var_os("HERMES_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".hermes")));

    match home {
        Some(h) => h.join(".env").is_file() || h.join("auth.json").is_file(),
        None => false,
    }
}

/// Outcome of an `ensure_profile` call.
#[derive(Debug, PartialEq, Eq)]
pub enum EnsureOutcome {
    /// A new profile was created (and bundled skills synced).
    Created,
    /// The profile already existed; nothing was changed.
    AlreadyExists,
}

/// Idempotently create a hermes profile for `handler` by clone-from-active.
///
/// Calls `hermes profile create gitim-<handler> --clone --no-alias`. The
/// `--clone` flag copies `config.yaml` / `.env` / `SOUL.md` / `memories/`
/// from the user's currently active profile (typically `~/.hermes`) so the
/// new agent inherits LLM provider configuration without manual setup.
/// `--no-alias` skips wrapper-script creation under `~/.local/bin/`.
pub async fn ensure_profile(handler: &str) -> Result<EnsureOutcome, HermesProfileError> {
    ensure_profile_with(handler, "hermes").await
}

/// Same as [`ensure_profile`] but with a configurable hermes binary path
/// (used by tests to inject a non-existent path and verify `CliNotFound`).
pub async fn ensure_profile_with(
    handler: &str,
    bin: &str,
) -> Result<EnsureOutcome, HermesProfileError> {
    let name = profile_name(handler);
    let output = tokio::process::Command::new(bin)
        .args(["profile", "create", &name, "--clone", "--no-alias"])
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                HermesProfileError::CliNotFound
            } else {
                HermesProfileError::Other(format!("spawn {bin}: {e}"))
            }
        })?;

    if output.status.success() {
        return Ok(EnsureOutcome::Created);
    }

    let combined = combined_output(&output.stdout, &output.stderr);
    if combined.contains("already exists") {
        Ok(EnsureOutcome::AlreadyExists)
    } else {
        Err(HermesProfileError::Other(format!(
            "hermes profile create failed: {}",
            combined.trim()
        )))
    }
}

fn combined_output(stdout: &[u8], stderr: &[u8]) -> String {
    let mut s = String::from_utf8_lossy(stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(stderr));
    s
}

/// Configure the LLM model settings for an agent's hermes profile.
///
/// Writes `model.provider`, `model.default`, and (when `base_url` is Some)
/// `model.base_url` into `gitim-<handler>`'s `config.yaml` via sequential
/// `hermes -p gitim-<handler> config set <key> <value>` shell-outs.
///
/// **On any failure, caller MUST `delete_profile` to avoid partial state —
/// see add_agent flow. If, for example, `model.default` is written but the
/// optional `model.base_url` step fails, the profile is left in a
/// half-configured state. The caller (add_agent) is responsible for calling
/// `delete_profile` on error.**
pub async fn apply_model_config(
    handler: &str,
    llm_provider: &str,
    llm_model: &str,
    base_url: Option<&str>,
) -> Result<(), HermesProfileError> {
    apply_model_config_with(handler, llm_provider, llm_model, base_url, "hermes").await
}

/// Same as [`apply_model_config`] but with a configurable hermes binary path
/// (used by tests to inject a fake/non-existent binary).
pub async fn apply_model_config_with(
    handler: &str,
    llm_provider: &str,
    llm_model: &str,
    base_url: Option<&str>,
    bin: &str,
) -> Result<(), HermesProfileError> {
    let profile = profile_name(handler);

    // Step 1: set model.provider
    run_config_set(bin, &profile, "model.provider", llm_provider).await?;

    // Step 2: set model.default
    run_config_set(bin, &profile, "model.default", llm_model).await?;

    // Step 3 (conditional): set model.base_url
    if let Some(url) = base_url {
        run_config_set(bin, &profile, "model.base_url", url).await?;
    }

    Ok(())
}

async fn run_config_set(
    bin: &str,
    profile: &str,
    key: &str,
    value: &str,
) -> Result<(), HermesProfileError> {
    let output = tokio::process::Command::new(bin)
        .args(["-p", profile, "config", "set", key, value])
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                HermesProfileError::CliNotFound
            } else {
                HermesProfileError::Other(format!("spawn {bin}: {e}"))
            }
        })?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    Err(HermesProfileError::Other(format!(
        "config set {key} failed: {}",
        stderr.trim()
    )))
}

/// Best-effort delete of the agent's hermes profile.
///
/// Calls `hermes profile delete gitim-<handler> -y`. Idempotent: returns
/// `Ok(())` when the profile is already gone. When the hermes CLI itself is
/// missing (user uninstalled hermes but agents remain), logs a warning and
/// returns `Ok(())` so this never blocks `hard_delete_agent` callers.
pub async fn delete_profile(handler: &str) -> Result<(), HermesProfileError> {
    delete_profile_with(handler, "hermes").await
}

/// Same as [`delete_profile`] but with a configurable hermes binary path.
pub async fn delete_profile_with(handler: &str, bin: &str) -> Result<(), HermesProfileError> {
    let name = profile_name(handler);
    let output = match tokio::process::Command::new(bin)
        .args(["profile", "delete", &name, "-y"])
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::warn!(
                "hermes CLI not found while deleting profile {name}; \
                 leaving profile dir on disk"
            );
            return Ok(());
        }
        Err(e) => {
            return Err(HermesProfileError::Other(format!("spawn {bin}: {e}")));
        }
    };

    if output.status.success() {
        return Ok(());
    }

    let combined = combined_output(&output.stdout, &output.stderr);
    if combined.contains("does not exist") {
        Ok(())
    } else {
        Err(HermesProfileError::Other(format!(
            "hermes profile delete failed: {}",
            combined.trim()
        )))
    }
}

/// Compose the full SOUL.md body for a hermes agent: hermes-tailored
/// system prompt (drops AGENTS.md / notes / [[RESET]] sections — see
/// `gitim_agent_provider::hermes::prompts`) plus an optional per-agent
/// suffix from `me.json::system_prompt`.
///
/// This is the single source of truth for what ends up in SOUL.md. Both
/// `add_agent` (provision time) and PATCH (system_prompt update) call it
/// so the file stays consistent regardless of how the agent state changes.
pub fn build_hermes_soul_body(
    handler: &str,
    model: Option<&str>,
    custom_system_prompt: Option<&str>,
) -> String {
    // `ProviderConfig::default` is fine here — we only need the trait's
    // prompt methods, not the executable path / env. The trait surface
    // for system-prompt assembly is stateless wrt config.
    let provider = crate::preconditions::hermes_provider();
    let ctx = PromptContext { handler, model };
    let mut body = provider.build_system_prompt(&ctx);
    if let Some(custom) = custom_system_prompt {
        if !custom.is_empty() {
            body.push_str("\n\n## 用户自定义指令\n\n");
            body.push_str(custom);
        }
    }
    body
}

// ── SOUL.md management ──
//
// SOUL.md is hermes' "agent persona" file (see acp_adapter/server.py + run_agent.py
// `_build_system_prompt`). Hermes loads it as the **frozen** identity slot of every
// session and rebuilds the prompt after each in-loop compression event, so anything
// in this file survives compression without runtime re-injection. That's why it's the
// right place to plant the GitIM system prompt for hermes agents.
//
// **Boundary**: SOUL.md is shared territory — the user is allowed to hand-edit it for
// persona customization, and hermes itself ships a placeholder template at profile-
// create time. We must not clobber either case silently. The marker discipline below
// is how we keep the runtime out of files it shouldn't own.

/// First-line marker that identifies a SOUL.md as runtime-managed.
///
/// Format: `<!-- gitim-managed-soul: v=1 sha256=<hex> -->\n`
/// The hex is sha256 of the body (everything after the marker line +
/// blank line). When the marker is present, we own the file and can rewrite
/// at will. When it's missing, the file is user-owned (or hermes' shipped
/// template); we leave it alone and surface that to the caller.
const SOUL_MARKER_PREFIX: &str = "<!-- gitim-managed-soul: v=1 sha256=";
const SOUL_MARKER_SUFFIX: &str = " -->";

/// How [`write_soul_md`] should treat a file that exists without our marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoulWriteMode {
    /// Refuse the write if the file is present but lacks our marker
    /// (hand-edited by the user, or some unexpected third party). Use this
    /// for PATCH-time updates so the user's customisation is never silently
    /// clobbered. Absent file / marker-present-stale → always written.
    PreserveUserEdits,
    /// Always (re)write. Use this only at provision time, when the SOUL.md
    /// we're seeing is the hermes-shipped template from `--clone` and is
    /// expected to be replaced by the runtime-managed version. After this
    /// call the file carries our marker, so subsequent
    /// `PreserveUserEdits` writes correctly distinguish hermes-template
    /// (now gone) from real user edits.
    Force,
}

/// Outcome of a [`write_soul_md`] call.
#[derive(Debug, PartialEq, Eq)]
pub enum SoulWriteOutcome {
    /// The file did not exist or was runtime-managed with stale content —
    /// we wrote fresh content + a refreshed marker.
    Wrote,
    /// The file is runtime-managed and already has the requested content;
    /// nothing on disk changed.
    SkippedUnchanged,
    /// `PreserveUserEdits` only: the file exists without our marker. We
    /// did not touch it. Caller should surface this to the user — typically
    /// log a warning that the system prompt update did not propagate to
    /// hermes because the user is managing SOUL.md by hand.
    RefusedUserEdited,
}

fn soul_path(handler: &str) -> Result<PathBuf, HermesProfileError> {
    Ok(profile_dir(handler)?.join("SOUL.md"))
}

fn soul_marker_line(body: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(body.as_bytes());
    let hex = hex_encode(&hasher.finalize());
    format!("{SOUL_MARKER_PREFIX}{hex}{SOUL_MARKER_SUFFIX}\n")
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Parse the marker line out of an existing SOUL.md. Returns the recorded
/// sha256 hex if the first line matches our marker format, `None` otherwise
/// (which includes the hermes shipped template `# Hermes Agent Persona`).
fn parse_marker(existing: &str) -> Option<&str> {
    let first_line = existing.split('\n').next()?;
    let inner = first_line
        .strip_prefix(SOUL_MARKER_PREFIX)?
        .strip_suffix(SOUL_MARKER_SUFFIX)?;
    Some(inner)
}

/// Idempotently write the GitIM-managed SOUL.md for `handler`.
///
/// `body` is the system-prompt text — caller passes the output of
/// `Provider::build_system_prompt(ctx)` + any agent-specific suffix. The
/// function:
///   1. Computes sha256 of `body`.
///   2. If the file is absent or starts with our marker:
///      - Marker hash matches → return `SkippedUnchanged` (no write).
///      - Marker hash differs or marker missing entirely → write
///        marker + body, return `Wrote`.
///   3. If the file exists but has no marker:
///      - `mode = PreserveUserEdits` → return `RefusedUserEdited` without
///        touching the file.
///      - `mode = Force` → overwrite anyway. Provisioning passes `Force`
///        because the no-marker file is just the hermes-shipped template
///        from `--clone`, not a real user edit.
///
/// Atomic on POSIX: writes to `SOUL.md.tmp` then `rename` so a crash mid-
/// write cannot leave a truncated SOUL.md.
pub fn write_soul_md(
    handler: &str,
    body: &str,
    mode: SoulWriteMode,
) -> Result<SoulWriteOutcome, HermesProfileError> {
    let path = soul_path(handler)?;
    write_soul_md_at(&path, body, mode)
}

/// Test seam: same as [`write_soul_md`] but with explicit destination path.
pub fn write_soul_md_at(
    path: &Path,
    body: &str,
    mode: SoulWriteMode,
) -> Result<SoulWriteOutcome, HermesProfileError> {
    let marker = soul_marker_line(body);
    let expected_hash = marker
        .strip_prefix(SOUL_MARKER_PREFIX)
        .and_then(|rest| rest.strip_suffix(&format!("{SOUL_MARKER_SUFFIX}\n")))
        .unwrap_or("")
        .to_string();

    if path.exists() {
        let existing = std::fs::read_to_string(path)
            .map_err(|e| HermesProfileError::Other(format!("read {}: {e}", path.display())))?;
        match parse_marker(&existing) {
            Some(prev_hash) if prev_hash == expected_hash => {
                return Ok(SoulWriteOutcome::SkippedUnchanged);
            }
            Some(_) => {
                // runtime-managed but stale — fall through to rewrite.
            }
            None => match mode {
                SoulWriteMode::PreserveUserEdits => {
                    return Ok(SoulWriteOutcome::RefusedUserEdited);
                }
                SoulWriteMode::Force => {
                    // Hermes-shipped template at provision time; replace it.
                }
            },
        }
    }

    let parent = path.parent().ok_or_else(|| {
        HermesProfileError::Other(format!("SOUL.md has no parent: {}", path.display()))
    })?;
    std::fs::create_dir_all(parent)
        .map_err(|e| HermesProfileError::Other(format!("mkdir {}: {e}", parent.display())))?;

    let tmp = path.with_extension("md.tmp");
    let mut content = String::with_capacity(marker.len() + body.len() + 1);
    content.push_str(&marker);
    content.push('\n');
    content.push_str(body);
    if !content.ends_with('\n') {
        content.push('\n');
    }

    std::fs::write(&tmp, &content)
        .map_err(|e| HermesProfileError::Other(format!("write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        HermesProfileError::Other(format!(
            "rename {} -> {}: {e}",
            tmp.display(),
            path.display()
        ))
    })?;
    Ok(SoulWriteOutcome::Wrote)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_name_for_alice() {
        assert_eq!(profile_name("alice"), "gitim-alice");
    }

    #[test]
    fn profile_dir_for_alice() {
        let result = profile_dir("alice").unwrap();
        let expected = dirs::home_dir()
            .unwrap()
            .join(".hermes/profiles/gitim-alice");
        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn ensure_profile_with_nonexistent_binary_returns_cli_not_found() {
        let err = ensure_profile_with("alice", "/nonexistent/binary/xyz")
            .await
            .expect_err("expected CliNotFound");
        assert!(matches!(err, HermesProfileError::CliNotFound));
    }

    #[tokio::test]
    async fn delete_profile_with_nonexistent_binary_is_ok() {
        // Best-effort: if hermes is gone (uninstalled mid-life), hard_delete
        // must still succeed instead of leaving the agent half-deleted.
        delete_profile_with("alice", "/nonexistent/binary/xyz")
            .await
            .expect("delete should be best-effort when CLI is missing");
    }

    mod soul_md_tests {
        use super::super::{
            parse_marker, write_soul_md_at, SoulWriteMode, SoulWriteOutcome, SOUL_MARKER_PREFIX,
        };
        use tempfile::TempDir;

        fn body_with_marker_hash(s: &str, hash: &str) -> String {
            format!("{SOUL_MARKER_PREFIX}{hash} -->\n\n{s}\n")
        }

        #[test]
        fn writes_fresh_when_file_absent() {
            let tmp = TempDir::new().unwrap();
            let path = tmp.path().join("SOUL.md");
            let body = "## op2 — GitIM Coordinator\n\nYou are op2.";
            let outcome = write_soul_md_at(&path, body, SoulWriteMode::PreserveUserEdits).unwrap();
            assert_eq!(outcome, SoulWriteOutcome::Wrote);
            let written = std::fs::read_to_string(&path).unwrap();
            assert!(written.starts_with(SOUL_MARKER_PREFIX));
            assert!(written.contains("You are op2."));
        }

        #[test]
        fn skips_when_identical_body() {
            let tmp = TempDir::new().unwrap();
            let path = tmp.path().join("SOUL.md");
            let body = "stable identity";
            assert_eq!(
                write_soul_md_at(&path, body, SoulWriteMode::PreserveUserEdits).unwrap(),
                SoulWriteOutcome::Wrote
            );
            assert_eq!(
                write_soul_md_at(&path, body, SoulWriteMode::PreserveUserEdits).unwrap(),
                SoulWriteOutcome::SkippedUnchanged
            );
        }

        #[test]
        fn rewrites_when_marker_present_but_body_changed() {
            let tmp = TempDir::new().unwrap();
            let path = tmp.path().join("SOUL.md");
            assert_eq!(
                write_soul_md_at(&path, "v1 body", SoulWriteMode::PreserveUserEdits).unwrap(),
                SoulWriteOutcome::Wrote
            );
            let outcome =
                write_soul_md_at(&path, "v2 body", SoulWriteMode::PreserveUserEdits).unwrap();
            assert_eq!(outcome, SoulWriteOutcome::Wrote);
            let written = std::fs::read_to_string(&path).unwrap();
            assert!(written.contains("v2 body"));
            assert!(!written.contains("v1 body"));
        }

        #[test]
        fn preserve_mode_refuses_when_no_marker_present() {
            // PATCH-time semantics: user-edited file is protected.
            let tmp = TempDir::new().unwrap();
            let path = tmp.path().join("SOUL.md");
            std::fs::write(
                &path,
                "# Hermes Agent Persona\n\n<!-- user customised this -->\n",
            )
            .unwrap();
            let outcome =
                write_soul_md_at(&path, "runtime body", SoulWriteMode::PreserveUserEdits).unwrap();
            assert_eq!(outcome, SoulWriteOutcome::RefusedUserEdited);
            let after = std::fs::read_to_string(&path).unwrap();
            assert!(after.contains("Hermes Agent Persona"));
            assert!(!after.contains("runtime body"));
        }

        #[test]
        fn force_mode_overwrites_shipped_template() {
            // Provision-time semantics: the no-marker file is the
            // hermes-shipped template freshly cloned by `ensure_profile`,
            // and the runtime is expected to install its own version on top.
            let tmp = TempDir::new().unwrap();
            let path = tmp.path().join("SOUL.md");
            std::fs::write(&path, "# Hermes Agent Persona\n\n<!-- template -->\n").unwrap();
            let outcome = write_soul_md_at(&path, "runtime body", SoulWriteMode::Force).unwrap();
            assert_eq!(outcome, SoulWriteOutcome::Wrote);
            let after = std::fs::read_to_string(&path).unwrap();
            assert!(after.starts_with(SOUL_MARKER_PREFIX));
            assert!(after.contains("runtime body"));
            assert!(!after.contains("Hermes Agent Persona"));
        }

        #[test]
        fn force_mode_still_skips_when_marker_matches() {
            // Even Force shouldn't write when content is byte-identical.
            // Avoids spurious mtime churn on every provision retry.
            let tmp = TempDir::new().unwrap();
            let path = tmp.path().join("SOUL.md");
            assert_eq!(
                write_soul_md_at(&path, "body", SoulWriteMode::Force).unwrap(),
                SoulWriteOutcome::Wrote
            );
            assert_eq!(
                write_soul_md_at(&path, "body", SoulWriteMode::Force).unwrap(),
                SoulWriteOutcome::SkippedUnchanged
            );
        }

        #[test]
        fn refuses_when_marker_corrupted() {
            let tmp = TempDir::new().unwrap();
            let path = tmp.path().join("SOUL.md");
            std::fs::write(&path, "<!-- not-our-marker -->\nuser body\n").unwrap();
            let outcome =
                write_soul_md_at(&path, "runtime body", SoulWriteMode::PreserveUserEdits).unwrap();
            assert_eq!(outcome, SoulWriteOutcome::RefusedUserEdited);
        }

        #[test]
        fn parse_marker_handles_our_format() {
            let raw = body_with_marker_hash("body", "abc123");
            assert_eq!(parse_marker(&raw), Some("abc123"));
        }

        #[test]
        fn parse_marker_returns_none_for_template() {
            assert!(parse_marker("# Hermes Agent Persona\n").is_none());
        }
    }

    // `default_profile_ready` reads HERMES_HOME, a process-global env var.
    // serial_test prevents these from running concurrently with each other
    // or with other tests that touch HERMES_HOME.
    mod default_profile_ready_tests {
        use super::super::default_profile_ready;
        use serial_test::serial;
        use tempfile::TempDir;

        #[test]
        #[serial(hermes_home_env)]
        fn ready_when_env_file_exists() {
            let tmp = TempDir::new().unwrap();
            std::fs::write(tmp.path().join(".env"), "FOO=bar").unwrap();
            std::env::set_var("HERMES_HOME", tmp.path());
            assert!(default_profile_ready());
            std::env::remove_var("HERMES_HOME");
        }

        #[test]
        #[serial(hermes_home_env)]
        fn ready_when_authjson_exists() {
            let tmp = TempDir::new().unwrap();
            std::fs::write(tmp.path().join("auth.json"), "{}").unwrap();
            std::env::set_var("HERMES_HOME", tmp.path());
            assert!(default_profile_ready());
            std::env::remove_var("HERMES_HOME");
        }

        #[test]
        #[serial(hermes_home_env)]
        fn not_ready_when_empty() {
            let tmp = TempDir::new().unwrap();
            std::env::set_var("HERMES_HOME", tmp.path());
            assert!(!default_profile_ready());
            std::env::remove_var("HERMES_HOME");
        }
    }
}
