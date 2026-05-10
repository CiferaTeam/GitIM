#![deny(warnings)]

//! `gitim cron` subcommands.
//!
//! Each command shells out to a daemon RPC via `GitimClient`; the typed
//! cron methods on the client return `Result<T, ClientError>` where
//! `ClientError::Api { code }` carries the daemon's `error_code` taxonomy.
//! `error_message_for_code` maps those tags to user-facing Chinese
//! messages so the CLI surface stays consistent (`name_conflict` →
//! "已存在同名 cron `<name>`", etc.) instead of leaking the daemon's
//! English error strings.

use std::path::Path;
use std::process;

use gitim_client::{ClientError, GitimClient};
use gitim_core::types::cron::MAX_PROMPT_BYTES;

use crate::output::OutputMode;

// -- prompt loading --------------------------------------------------------

/// Resolve `--prompt` vs `--prompt-file` into the prompt body. Clap
/// already enforces "exactly one of {prompt, prompt-file}" via
/// `conflicts_with` + `required_unless_present`, so this only handles the
/// file-IO + UTF-8 + size validation that clap can't.
///
/// `MAX_PROMPT_BYTES` is the daemon-side ceiling — failing fast here
/// gives a more specific error than letting the daemon emit
/// `prompt_too_large` (which would be the same outcome but without
/// "the file you pointed at is too big").
pub fn load_prompt(prompt: Option<&str>, prompt_file: Option<&Path>) -> Result<String, String> {
    match (prompt, prompt_file) {
        (Some(p), None) => {
            // Symmetric size guard with --prompt-file. Without this, an
            // oversized inline prompt would round-trip to the daemon
            // before failing as `prompt_too_large` — which is correct
            // but loses the "你给的字符串太大了" specificity the file
            // path gets.
            if p.len() > MAX_PROMPT_BYTES {
                return Err(format!(
                    "prompt 大小 {} 字节，超过上限 {} 字节",
                    p.len(),
                    MAX_PROMPT_BYTES
                ));
            }
            Ok(p.to_string())
        }
        (None, Some(path)) => {
            let bytes = std::fs::read(path)
                .map_err(|e| format!("无法读取 prompt 文件 {}: {e}", path.display()))?;
            let text = String::from_utf8(bytes)
                .map_err(|_| format!("prompt 文件 {} 不是 UTF-8", path.display()))?;
            if text.len() > MAX_PROMPT_BYTES {
                return Err(format!(
                    "prompt 文件 {} 大小 {} 字节，超过上限 {} 字节",
                    path.display(),
                    text.len(),
                    MAX_PROMPT_BYTES
                ));
            }
            Ok(text)
        }
        (Some(_), Some(_)) | (None, None) => {
            // clap's required_unless_present + conflicts_with should
            // make this unreachable. Defensive return so the caller
            // doesn't have to model "impossible" as a panic.
            Err("必须指定 --prompt 或 --prompt-file（且不能同时给出）".to_string())
        }
    }
}

// -- error_code → friendly Chinese ----------------------------------------

/// Translate a daemon `error_code` into a Chinese message. `default` is
/// the daemon's English `error` field, used as a fallback for codes we
/// don't have a translation for (or when the daemon omitted the code
/// tag entirely).
///
/// Keep this list in sync with `crates/gitim-daemon/src/handlers/cron.rs`
/// where the codes are emitted. New `error_with_code(...)` calls there
/// without an entry here will fall through to the English message.
pub fn error_message_for_code(code: Option<&str>, default: &str) -> String {
    match code {
        Some("name_conflict") => "cron 名称已存在（active 或 archive 里同名）".to_string(),
        Some("not_found") => "找不到该 cron".to_string(),
        Some("invalid_name") => {
            "cron 名称非法（小写 a-z 0-9 连字符，1–63 字符，不能用 archive）".to_string()
        }
        Some("invalid_target") => "target 不是合法 handler".to_string(),
        Some("target_not_found") => "目标 handler 在 workspace 里不存在".to_string(),
        Some("invalid_author") => "author handler 格式非法".to_string(),
        Some("invalid_schedule") => "schedule 不是合法 cron 表达式".to_string(),
        Some("invalid_timezone") => "timezone 不是有效 IANA 时区".to_string(),
        Some("invalid_version") => "spec.yaml version 不被本版本支持".to_string(),
        Some("invalid_created_at") => "spec.yaml created_at 必须是 UTC ISO 8601".to_string(),
        Some("prompt_empty") => "prompt 不能为空".to_string(),
        Some("prompt_too_large") => {
            format!("prompt 超过 {} 字节上限", MAX_PROMPT_BYTES)
        }
        Some("invalid_spec") => "spec.yaml 解析失败".to_string(),
        Some("spec_unreadable") => "无法读取 spec.yaml".to_string(),
        Some("serialize_failed") => "无法序列化 spec.yaml".to_string(),
        Some("fs_error") => "文件系统错误".to_string(),
        Some("commit_failed") => "git commit 失败".to_string(),
        Some("archive_conflict") => "archive 里已存在同名 cron".to_string(),
        Some("git_mv_failed") => "git mv 失败".to_string(),
        _ => default.to_string(),
    }
}

/// Print a daemon error to stderr in the project's standard format,
/// then exit 1. Centralized so every subcommand surfaces the same shape.
fn die_with_api_error(prefix: &str, err: ClientError) -> ! {
    match err {
        ClientError::Api { message, code } => {
            let translated = error_message_for_code(code.as_deref(), &message);
            eprintln!("{prefix}: {translated}");
        }
        other => {
            eprintln!("{prefix}: {other}");
        }
    }
    process::exit(1);
}

// -- subcommands -----------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub async fn cmd_create(
    client: &GitimClient,
    mode: &OutputMode,
    name: &str,
    schedule: &str,
    target: &str,
    timezone: Option<&str>,
    prompt: &str,
) {
    match client
        .create_cron(name, schedule, timezone, target, prompt)
        .await
    {
        Ok(()) => match mode {
            OutputMode::Human => println!("已创建 cron '{name}'"),
            OutputMode::Json => {
                println!("{}", serde_json::json!({"name": name, "status": "created"}));
            }
        },
        Err(e) => die_with_api_error("创建 cron 失败", e),
    }
}

pub async fn cmd_list(client: &GitimClient, mode: &OutputMode) {
    match client.list_crons().await {
        Ok(crons) => match mode {
            OutputMode::Json => {
                let value = serde_json::to_value(&crons).unwrap_or(serde_json::Value::Null);
                println!("{value}");
            }
            OutputMode::Human => {
                if crons.is_empty() {
                    println!("暂无 cron");
                    return;
                }
                // Simple aligned columns — same human style as
                // `gitim channels`. No external table crate so the CLI
                // dep graph stays tight.
                let name_w = crons.iter().map(|c| c.name.len()).max().unwrap_or(0).max(4);
                let sched_w = crons
                    .iter()
                    .map(|c| c.schedule.len())
                    .max()
                    .unwrap_or(0)
                    .max(8);
                let target_w = crons
                    .iter()
                    .map(|c| c.target.len())
                    .max()
                    .unwrap_or(0)
                    .max(6);
                println!(
                    "{:<name_w$}  {:<sched_w$}  {:<target_w$}  {:<8}  {}",
                    "NAME",
                    "SCHEDULE",
                    "TARGET",
                    "ENABLED",
                    "NEXT FIRE",
                    name_w = name_w,
                    sched_w = sched_w,
                    target_w = target_w,
                );
                for c in &crons {
                    let next = c.next_fire.as_deref().unwrap_or("-");
                    let enabled = if c.enabled { "yes" } else { "no" };
                    println!(
                        "{:<name_w$}  {:<sched_w$}  @{:<target_w$}  {:<8}  {}",
                        c.name,
                        c.schedule,
                        c.target,
                        enabled,
                        next,
                        name_w = name_w,
                        sched_w = sched_w,
                        target_w = target_w.saturating_sub(1),
                    );
                }
            }
        },
        Err(e) => die_with_api_error("拉取 cron 列表失败", e),
    }
}

pub async fn cmd_show(client: &GitimClient, mode: &OutputMode, name: &str) {
    match client.show_cron(name).await {
        Ok(detail) => match mode {
            OutputMode::Json => {
                let v = serde_json::to_value(&detail).unwrap_or(serde_json::Value::Null);
                println!("{v}");
            }
            OutputMode::Human => {
                println!("Cron: {}", detail.name);
                // The spec is serde_yaml::Value on the wire; printing it as
                // pretty JSON keeps gitim-cli free of a serde_yaml dep
                // (it stays in core/daemon where the spec is parsed).
                if let Ok(spec_json) = serde_json::to_value(&detail.spec) {
                    if let Ok(s) = serde_json::to_string_pretty(&spec_json) {
                        println!("Spec:");
                        for line in s.lines() {
                            println!("  {line}");
                        }
                    }
                }
                println!("Next fire: {}", detail.next_fire.as_deref().unwrap_or("-"));
                println!("Recent runs ({}):", detail.recent_runs.len());
                for r in &detail.recent_runs {
                    println!("  {}  {}", r.ts, r.filename);
                }
            }
        },
        Err(e) => die_with_api_error("读取 cron 失败", e),
    }
}

pub async fn cmd_history(client: &GitimClient, mode: &OutputMode, name: &str, limit: Option<u32>) {
    match client.history_cron(name, limit).await {
        Ok(runs) => match mode {
            OutputMode::Json => {
                let v = serde_json::to_value(&runs).unwrap_or(serde_json::Value::Null);
                println!("{v}");
            }
            OutputMode::Human => {
                if runs.is_empty() {
                    println!("暂无历史 fire");
                    return;
                }
                for r in &runs {
                    println!("{}  {}", r.ts, r.filename);
                }
            }
        },
        Err(e) => die_with_api_error("读取 cron 历史失败", e),
    }
}

pub async fn cmd_enable(client: &GitimClient, _mode: &OutputMode, name: &str) {
    match client.enable_cron(name).await {
        Ok(resp) => {
            if resp.changed {
                println!("已启用 cron '{}'", resp.name);
            } else {
                println!("cron '{}' 已是启用状态（无变化）", resp.name);
            }
        }
        Err(e) => die_with_api_error("启用 cron 失败", e),
    }
}

pub async fn cmd_disable(client: &GitimClient, _mode: &OutputMode, name: &str) {
    match client.disable_cron(name).await {
        Ok(resp) => {
            if resp.changed {
                println!("已禁用 cron '{}'", resp.name);
            } else {
                println!("cron '{}' 已是禁用状态（无变化）", resp.name);
            }
        }
        Err(e) => die_with_api_error("禁用 cron 失败", e),
    }
}

pub async fn cmd_delete(client: &GitimClient, _mode: &OutputMode, name: &str) {
    match client.delete_cron(name).await {
        Ok(()) => println!("已删除 cron '{name}'（已移到 archive/crons/）"),
        Err(e) => die_with_api_error("删除 cron 失败", e),
    }
}

pub async fn cmd_next(client: &GitimClient, _mode: &OutputMode, name: &str) {
    match client.next_fire_for(name).await {
        Ok(Some(dt)) => {
            // Single-line ISO 8601 UTC, scriptable. Use `Z` suffix
            // (RFC 3339 + UTC convention) — `to_rfc3339()` would emit
            // `+00:00` which is harder to grep.
            println!("{}", dt.format("%Y-%m-%dT%H:%M:%SZ"));
        }
        Ok(None) => {
            eprintln!("Error: cron '{name}' 当前没有 next_fire（可能已禁用或 schedule 无法解析）");
            process::exit(1);
        }
        Err(e) => die_with_api_error("计算 next fire 失败", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn load_prompt_inline() {
        let result = load_prompt(Some("hello"), None).unwrap();
        assert_eq!(result, "hello");
    }

    #[test]
    fn load_prompt_from_file() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "multi\nline\nprompt").unwrap();
        let result = load_prompt(None, Some(f.path())).unwrap();
        assert_eq!(result, "multi\nline\nprompt");
    }

    #[test]
    fn load_prompt_missing_file() {
        let err = load_prompt(None, Some(Path::new("/nonexistent/path/xyzz"))).unwrap_err();
        assert!(err.contains("无法读取"), "msg = {err}");
    }

    #[test]
    fn load_prompt_invalid_utf8() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(&[0xff, 0xfe, 0x80]).unwrap();
        let err = load_prompt(None, Some(f.path())).unwrap_err();
        assert!(err.contains("UTF-8"), "msg = {err}");
    }

    #[test]
    fn load_prompt_oversized_file() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(&vec![b'a'; MAX_PROMPT_BYTES + 1]).unwrap();
        let err = load_prompt(None, Some(f.path())).unwrap_err();
        assert!(err.contains("超过上限"), "msg = {err}");
    }

    #[test]
    fn load_prompt_oversized_inline() {
        // Symmetric guard with `load_prompt_oversized_file` — an inline
        // --prompt over the 8KiB ceiling must fail client-side instead
        // of round-tripping through IPC to be rejected by the daemon's
        // CronSpec::validate.
        let big = "a".repeat(MAX_PROMPT_BYTES + 1);
        let err = load_prompt(Some(&big), None).unwrap_err();
        assert!(err.contains("超过上限"), "msg = {err}");
    }

    #[test]
    fn load_prompt_neither_given() {
        let err = load_prompt(None, None).unwrap_err();
        assert!(err.contains("--prompt"), "msg = {err}");
    }

    #[test]
    fn load_prompt_both_given() {
        let err = load_prompt(Some("inline"), Some(Path::new("/tmp/x"))).unwrap_err();
        assert!(err.contains("--prompt"), "msg = {err}");
    }

    #[test]
    fn error_message_translates_known_codes() {
        let m = error_message_for_code(Some("name_conflict"), "english fallback");
        assert!(m.contains("已存在"), "msg = {m}");

        let m = error_message_for_code(Some("not_found"), "english fallback");
        assert!(m.contains("找不到"), "msg = {m}");

        let m = error_message_for_code(Some("invalid_schedule"), "english fallback");
        assert!(m.contains("schedule"), "msg = {m}");

        let m = error_message_for_code(Some("invalid_timezone"), "english fallback");
        assert!(m.contains("时区"), "msg = {m}");

        let m = error_message_for_code(Some("target_not_found"), "english fallback");
        assert!(m.contains("目标"), "msg = {m}");

        let m = error_message_for_code(Some("prompt_empty"), "english fallback");
        assert!(m.contains("prompt"), "msg = {m}");

        let m = error_message_for_code(Some("prompt_too_large"), "english fallback");
        assert!(m.contains("prompt"), "msg = {m}");
        assert!(m.contains("8192"), "msg = {m}");
    }

    #[test]
    fn error_message_falls_back_for_unknown_code() {
        let m = error_message_for_code(Some("future_unknown_code"), "raw english");
        assert_eq!(m, "raw english");
    }

    #[test]
    fn error_message_falls_back_when_no_code() {
        let m = error_message_for_code(None, "raw english");
        assert_eq!(m, "raw english");
    }
}
