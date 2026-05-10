//! Cron trigger handlers.
//!
//! See `docs/plans/2026-05-09-cron-trigger/design.md` for the protocol-level
//! framing — `crons/<name>/spec.yaml` + `crons/<name>/<theoretical_ts>.thread`.
//! The engine that scans + fires lives in `cron_engine.rs` (Task 2.5+); this
//! module only covers the IPC surface (create, list, show, history, enable,
//! disable, delete).
//!
//! Validation is layered:
//!   - `Handler::new` for any handler-shaped string (target, author).
//!   - `validate_cron_name` for the directory stem (lowercase a-z 0-9 hyphen,
//!     1–63 chars, no leading hyphen, not a reserved word).
//!   - `CronSpec::validate` (in gitim-core) for schedule, timezone, prompt
//!     size, version, created_at format. We construct an in-memory spec and
//!     hand it through that single source of truth so the daemon and
//!     yaml-loader can never disagree.

use std::collections::BTreeMap;

use chrono::SecondsFormat;
use gitim_core::responses::CreateCronResponse;
use gitim_core::types::{CronSpec, Handler};
use tracing::info;

use crate::api::Response;
use crate::state::SharedState;

/// Reject names that would alias the archive convention or shadow the
/// crons/ root. `archive` and `crons` are the two top-level neighbors a
/// stem could collide with after `git mv`; `.`-prefixed names would clash
/// with hidden-file discipline (`.gitim/`, `.git/`).
const RESERVED_CRON_NAMES: &[&str] = &["archive", "crons"];

/// `^[a-z0-9][a-z0-9-]{0,62}$` enforced by hand to avoid a regex dep here.
/// Same shape as channel names (see `ChannelName::new`) but kept separate
/// because cron names live in their own namespace and we don't want a
/// future channel-name policy change to silently re-shape cron rules.
fn validate_cron_name(name: &str) -> Result<(), Response> {
    if name.is_empty() {
        return Err(Response::error_with_code(
            "cron name cannot be empty",
            "invalid_name",
        ));
    }
    if name.len() > 63 {
        return Err(Response::error_with_code(
            format!("cron name exceeds 63 characters (got {})", name.len()),
            "invalid_name",
        ));
    }
    if name.starts_with('.') {
        return Err(Response::error_with_code(
            format!("cron name '{}' cannot start with '.'", name),
            "invalid_name",
        ));
    }
    if RESERVED_CRON_NAMES.contains(&name) {
        return Err(Response::error_with_code(
            format!("cron name '{}' is reserved", name),
            "invalid_name",
        ));
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !matches!(first, 'a'..='z' | '0'..='9') {
        return Err(Response::error_with_code(
            format!(
                "cron name '{}' must start with a lowercase letter or digit",
                name
            ),
            "invalid_name",
        ));
    }
    for ch in std::iter::once(first).chain(chars) {
        if !matches!(ch, 'a'..='z' | '0'..='9' | '-') {
            return Err(Response::error_with_code(
                format!(
                    "cron name '{}' contains invalid character '{}' (allowed: a-z 0-9 -)",
                    name, ch
                ),
                "invalid_name",
            ));
        }
    }
    Ok(())
}

/// Create `crons/<name>/spec.yaml` with the given schedule + target +
/// prompt, validate every field, then commit under `commit_lock`.
///
/// `target` accepts the literal string `@self`, which is rewritten in
/// place to the resolved author handler before the spec is built.
/// Resolution happens here (not at the dispatch layer) because the
/// author is the only place we can be sure of the substitution context.
///
/// Validation order is intentional: cheap structural checks first
/// (name, target, author handler shape), then a probe of the on-disk
/// archive/active spec collisions, then the spec body — schedule,
/// timezone, prompt — through `CronSpec::validate`. Each step returns
/// a typed `error_code` so clients (CLI, WebUI) can render specific
/// messages without parsing the human-readable `error`.
pub async fn handle_create_cron(
    state: SharedState,
    name: String,
    schedule: String,
    timezone: Option<String>,
    target: String,
    prompt: String,
    author: String,
) -> Response {
    // 1. Validate name — bounds + charset + reserved words.
    if let Err(resp) = validate_cron_name(&name) {
        return resp;
    }

    // 2. Validate author handler format. Same source of truth as every
    //    other write path (handle_send / handle_archive_*).
    let author_handler = match Handler::new(&author) {
        Ok(h) => h,
        Err(e) => {
            return Response::error_with_code(
                format!("invalid author: {}", e),
                "invalid_author",
            )
        }
    };

    // 3. Resolve `@self` → author. Anything else must parse as a Handler
    //    and exist in `users/<target>.meta.yaml`. We don't allow creating
    //    crons for departed users — `archive/users/<h>.meta.yaml` doesn't
    //    count as "exists".
    let resolved_target = if target == "@self" {
        author_handler.clone()
    } else {
        // Strip a leading `@` if the caller wrapped the handler. CLI users
        // tend to type `--target @bob` instinctively; the spec stores the
        // bare handler.
        let stripped = target.strip_prefix('@').unwrap_or(&target);
        let h = match Handler::new(stripped) {
            Ok(h) => h,
            Err(e) => {
                return Response::error_with_code(
                    format!("invalid target handler: {}", e),
                    "invalid_target",
                )
            }
        };
        let target_meta = state
            .repo_root
            .join("users")
            .join(format!("{}.meta.yaml", h.as_str()));
        if !target_meta.exists() {
            return Response::error_with_code(
                format!("target user @{} not found", h.as_str()),
                "target_not_found",
            );
        }
        h
    };

    // 4. Uniqueness — both active and archived paths. Doing this before
    //    spec construction so the user gets a name-conflict error rather
    //    than a generic "spec invalid" one.
    let cron_dir = state.repo_root.join("crons").join(&name);
    let active_spec = cron_dir.join("spec.yaml");
    if active_spec.exists() {
        return Response::error_with_code(
            format!("cron '{}' already exists", name),
            "name_conflict",
        );
    }
    let archive_spec = state
        .repo_root
        .join("archive/crons")
        .join(&name)
        .join("spec.yaml");
    if archive_spec.exists() {
        return Response::error_with_code(
            format!("cron '{}' exists in archive", name),
            "name_conflict",
        );
    }

    // 5. Build the spec in memory + run `validate`. This is the single
    //    source of truth shared with the YAML loader — schedule,
    //    timezone, prompt, version, created_at all checked in one shot.
    let now_iso = chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let spec = CronSpec {
        version: gitim_core::types::cron::CURRENT_VERSION,
        schedule: schedule.clone(),
        timezone: timezone.clone(),
        target: resolved_target.clone(),
        prompt,
        enabled: true,
        created_by: author_handler.clone(),
        created_at: now_iso,
        extra: BTreeMap::new(),
    };

    if let Err(e) = spec.validate() {
        // Map the typed error into a stable error_code. This is the only
        // place the wire surfaces these distinctions; the CLI/WebUI maps
        // them to user-facing messages.
        let code = match &e {
            gitim_core::types::CronSpecError::InvalidSchedule(_) => "invalid_schedule",
            gitim_core::types::CronSpecError::InvalidTimezone(_) => "invalid_timezone",
            gitim_core::types::CronSpecError::EmptyPrompt => "prompt_empty",
            gitim_core::types::CronSpecError::OversizedPrompt { .. } => "prompt_too_large",
            gitim_core::types::CronSpecError::InvalidVersion(_) => "invalid_version",
            gitim_core::types::CronSpecError::InvalidCreatedAt(_)
            | gitim_core::types::CronSpecError::CreatedAtNotUtc(_) => "invalid_created_at",
            gitim_core::types::CronSpecError::Yaml(_) => "invalid_spec",
        };
        return Response::error_with_code(format!("{}", e), code);
    }

    // 6. Serialize the spec for disk. Done before taking the lock so the
    //    critical section is just fs + git work.
    let spec_yaml = match spec.to_yaml() {
        Ok(s) => s,
        Err(e) => {
            return Response::error_with_code(
                format!("failed to serialize cron spec: {}", e),
                "serialize_failed",
            )
        }
    };

    // 7. Write under the commit-tree lock. Same pattern as
    //    `handle_send` / `handle_archive_dm`: read-mutate-commit happens
    //    while holding `commit_lock` so a concurrent writer (or
    //    sync_loop's rebase) can't slip a `git add` in between our
    //    file write and our `add_and_commit_as`.
    {
        let _commit_guard = state.commit_lock.lock().expect("commit_lock poisoned");

        // Re-check under lock — between our pre-check and now another
        // create_cron could have raced us into the same directory.
        if active_spec.exists() {
            return Response::error_with_code(
                format!("cron '{}' already exists", name),
                "name_conflict",
            );
        }

        if let Err(e) = std::fs::create_dir_all(&cron_dir) {
            return Response::error_with_code(
                format!("failed to create cron dir: {}", e),
                "fs_error",
            );
        }
        if let Err(e) = std::fs::write(&active_spec, &spec_yaml) {
            // Best-effort cleanup of the (likely empty) directory so a
            // retry sees a clean slate. Ignore failure — fs_error already
            // signals user the operation didn't complete.
            let _ = std::fs::remove_dir(&cron_dir);
            return Response::error_with_code(
                format!("failed to write spec.yaml: {}", e),
                "fs_error",
            );
        }

        let spec_rel = format!("crons/{}/spec.yaml", name);
        let commit_msg = format!("cron: create {} by @{}", name, author_handler.as_str());
        let (author_name, author_email) = state.author_for(author_handler.as_str());
        if let Err(e) = state.git_storage.add_and_commit_as(
            &[&spec_rel],
            &commit_msg,
            Some((&author_name, &author_email)),
        ) {
            // Roll back the on-disk write so the working tree mirrors HEAD.
            // Best-effort — if cleanup fails the user's retry will hit the
            // re-check above and report name_conflict, which surfaces the
            // problem clearly enough.
            let _ = std::fs::remove_file(&active_spec);
            let _ = std::fs::remove_dir(&cron_dir);
            return Response::error_with_code(
                format!("create_cron commit failed: {}", e),
                "commit_failed",
            );
        }

        // commit_guard drops at end of scope before any await below.
    }

    info!(
        "cron '{}' created by @{} (target=@{})",
        name,
        author_handler.as_str(),
        resolved_target.as_str()
    );

    let payload = CreateCronResponse {
        name,
        created_by: author_handler.as_str().to_string(),
        target: resolved_target.as_str().to_string(),
    };
    Response::success(serde_json::to_value(payload).unwrap())
}

/// Stub for [`Request::ListCrons`]. Real implementation lands in Task 2.3.
pub async fn handle_list_crons(_state: SharedState) -> Response {
    not_implemented("list_crons")
}

/// Stub for [`Request::ShowCron`]. Real implementation lands in Task 2.3.
pub async fn handle_show_cron(_state: SharedState, _name: String) -> Response {
    not_implemented("show_cron")
}

/// Stub for [`Request::HistoryCron`]. Real implementation lands in Task 2.3.
pub async fn handle_history_cron(
    _state: SharedState,
    _name: String,
    _limit: Option<u32>,
) -> Response {
    not_implemented("history_cron")
}

/// Stub for [`Request::EnableCron`]. Real implementation lands in Task 2.4.
pub async fn handle_enable_cron(
    _state: SharedState,
    _name: String,
    _author: String,
) -> Response {
    not_implemented("enable_cron")
}

/// Stub for [`Request::DisableCron`]. Real implementation lands in Task 2.4.
pub async fn handle_disable_cron(
    _state: SharedState,
    _name: String,
    _author: String,
) -> Response {
    not_implemented("disable_cron")
}

/// Stub for [`Request::DeleteCron`]. Real implementation lands in Task 2.4.
pub async fn handle_delete_cron(
    _state: SharedState,
    _name: String,
    _author: String,
) -> Response {
    not_implemented("delete_cron")
}

/// Tagged error helper. The `error_code: "not_implemented"` lets the client
/// short-circuit on unfinished daemon endpoints without parsing the human
/// message.
fn not_implemented(method: &str) -> Response {
    Response::error_with_code(
        format!("{method}: not implemented yet (cron Wave 2 in progress)"),
        "not_implemented",
    )
}

#[cfg(test)]
mod tests {
    //! Task 2.1 scope tests: roundtrip every cron `Request` variant
    //! through `handle_request`'s dispatch and confirm we land on the
    //! cron stub, not some other handler.

    use crate::api::{Request, Response};
    use crate::handlers::handle_request;
    use crate::state::AppState;
    use gitim_core::types::Config;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::broadcast;

    fn make_config() -> Config {
        serde_yaml::from_str("version: 1").unwrap()
    }

    /// Minimal AppState with no users registered. Sufficient for stub
    /// dispatch tests — the stubs short-circuit before touching state.
    async fn make_state() -> (TempDir, Arc<AppState>) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        // git init so any future handler that reaches GitStorage doesn't
        // panic on missing repo. Stubs don't need it but a future test
        // that promotes into 2.2/2.3/2.4 will reuse this fixture.
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(&root)
            .output()
            .unwrap();
        let (tx, _) = broadcast::channel(16);
        let state = Arc::new(AppState::new(
            root,
            make_config(),
            tx,
            Some("alice".to_string()),
        ));
        (tmp, state)
    }

    fn assert_not_implemented(resp: &Response, method: &str) {
        assert!(!resp.ok, "{method}: expected error, got success");
        assert_eq!(
            resp.error_code.as_deref(),
            Some("not_implemented"),
            "{method}: expected error_code=not_implemented, got {:?}",
            resp.error_code
        );
        let msg = resp.error.as_deref().unwrap_or("");
        assert!(
            msg.contains(method),
            "{method}: error message should mention method name, got {msg}",
        );
    }

    #[tokio::test]
    async fn list_crons_dispatches_to_stub() {
        let (_tmp, state) = make_state().await;
        let req: Request = serde_json::from_value(serde_json::json!({
            "method": "list_crons",
        }))
        .unwrap();
        let resp = handle_request(req, state).await;
        assert_not_implemented(&resp, "list_crons");
    }

    #[tokio::test]
    async fn show_cron_dispatches_to_stub() {
        let (_tmp, state) = make_state().await;
        let req: Request = serde_json::from_value(serde_json::json!({
            "method": "show_cron",
            "name": "weekly",
        }))
        .unwrap();
        let resp = handle_request(req, state).await;
        assert_not_implemented(&resp, "show_cron");
    }

    #[tokio::test]
    async fn history_cron_dispatches_to_stub() {
        let (_tmp, state) = make_state().await;
        let req: Request = serde_json::from_value(serde_json::json!({
            "method": "history_cron",
            "name": "weekly",
            "limit": 5,
        }))
        .unwrap();
        let resp = handle_request(req, state).await;
        assert_not_implemented(&resp, "history_cron");
    }

    #[tokio::test]
    async fn enable_cron_dispatches_to_stub() {
        let (_tmp, state) = make_state().await;
        let req: Request = serde_json::from_value(serde_json::json!({
            "method": "enable_cron",
            "name": "weekly",
        }))
        .unwrap();
        let resp = handle_request(req, state).await;
        assert_not_implemented(&resp, "enable_cron");
    }

    #[tokio::test]
    async fn disable_cron_dispatches_to_stub() {
        let (_tmp, state) = make_state().await;
        let req: Request = serde_json::from_value(serde_json::json!({
            "method": "disable_cron",
            "name": "weekly",
        }))
        .unwrap();
        let resp = handle_request(req, state).await;
        assert_not_implemented(&resp, "disable_cron");
    }

    #[tokio::test]
    async fn delete_cron_dispatches_to_stub() {
        let (_tmp, state) = make_state().await;
        let req: Request = serde_json::from_value(serde_json::json!({
            "method": "delete_cron",
            "name": "weekly",
        }))
        .unwrap();
        let resp = handle_request(req, state).await;
        assert_not_implemented(&resp, "delete_cron");
    }
}
