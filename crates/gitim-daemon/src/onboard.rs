use crate::api::Response;
use crate::identity::{GitServer, InferredIdentity};
use crate::state::{AppState, SharedState};
use gitim_core::auth_payload::AuthPayload;
use gitim_core::me_json::MeJson;
use gitim_core::types::{ChannelMeta, Handler, UserMeta};
use gitim_sync::git::GitError;
use tracing::{info, warn};

const MAX_PUSH_RETRIES: u32 = 3;

/// Full onboard orchestration: identity -> me.json -> ensure_repo -> register_user -> sync loop.
pub async fn handle_onboard(
    state: SharedState,
    git_server: String,
    auth: Option<AuthPayload>,
    admin: bool,
    guest: bool,
) -> Response {
    // --- Guest mode: write me.json and start sync, skip everything else ---
    if guest {
        // Guest mode never inspects auth — accept Some or None.
        let _ = auth;
        if let Err(resp) = write_guest_me_json(&state) {
            return resp;
        }
        info!("onboard: guest mode — me.json written");

        // Clear any previous identity
        {
            let mut current = state.current_user.write().await;
            *current = None;
        }
        state
            .is_admin
            .store(false, std::sync::atomic::Ordering::SeqCst);

        // Start sync loop (pull-only, no local commits to push)
        AppState::spawn_sync_loop(state.clone());

        // Initialize search index
        if state.index.read().unwrap().is_none() {
            if let Err(e) = AppState::initialize_index(&state) {
                warn!("index initialization after guest onboard failed: {}", e);
            }
        }

        state
            .is_guest
            .store(true, std::sync::atomic::Ordering::SeqCst);

        return Response::success(serde_json::json!({
            "guest": true,
        }));
    }

    // --- Step A: Infer identity ---
    let auth_payload = match auth {
        Some(a) => a,
        None => return Response::error("onboard: missing auth payload (non-guest mode)"),
    };
    let identity = match infer(git_server.clone(), auth_payload) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let handler = identity.handler.as_str().to_string();
    let display_name = identity.display_name.clone();
    let github_email = identity.email.clone();

    info!("onboard: identity inferred — @{}", handler);

    // --- Step B: Write me.json ---
    if let Err(resp) = write_me_json(
        &state,
        &handler,
        &display_name,
        &git_server,
        github_email.as_deref(),
    ) {
        return resp;
    }
    {
        let mut current = state.current_user.write().await;
        *current = Some(handler.clone());
    }
    // Propagate to AppState so subsequent commits pick it up. Without
    // this step the first onboarded workspace would only see the email
    // after a daemon restart.
    if github_email.is_some() {
        if let Ok(mut slot) = state.github_email.write() {
            *slot = github_email.clone();
        }
    }

    info!("onboard: me.json written for @{}", handler);

    // --- Step C: EnsureRepo (idempotent) ---
    if let Err(resp) = ensure_repo(&state, &handler) {
        return resp;
    }

    info!("onboard: repo structure ensured");

    // --- Step D: RegisterUser (idempotent) ---
    let created = match register_user(&state, &handler, &display_name) {
        Ok(created) => created,
        Err(resp) => return resp,
    };

    info!(
        "onboard: user registered — @{} (created={})",
        handler, created
    );

    // --- Step E: Auto-join general channel (for newly created users) ---
    if created {
        if let Err(resp) = auto_join_general(&state, &handler) {
            return resp;
        }
        info!("onboard: @{} auto-joined general channel", handler);
    }

    // Refresh in-memory user list
    {
        let mut users = state.users.write().await;
        if !users.contains(&handler) {
            users.push(handler.clone());
            users.sort();
        }
    }

    // --- Start sync loop ---
    AppState::spawn_sync_loop(state.clone());

    // Initialize search index (if not already initialized)
    if state.index.read().unwrap().is_none() {
        if let Err(e) = AppState::initialize_index(&state) {
            warn!("index initialization after onboard failed: {}", e);
        }
    }

    // --- Set admin mode (after all steps succeed) ---
    state
        .is_admin
        .store(admin, std::sync::atomic::Ordering::SeqCst);
    if admin {
        info!("onboard: admin mode enabled");
    }

    // Clear guest mode if upgrading from guest to authenticated user
    state
        .is_guest
        .store(false, std::sync::atomic::Ordering::SeqCst);

    Response::success(serde_json::json!({
        "handler": handler,
        "created": created,
    }))
}

// ---------------------------------------------------------------------------
// Step A helpers
// ---------------------------------------------------------------------------

fn infer(git_server: String, auth: AuthPayload) -> Result<InferredIdentity, Response> {
    let server: GitServer =
        serde_json::from_value(serde_json::Value::String(git_server.clone()))
            .map_err(|_| Response::error(format!("unknown git_server: {}", git_server)))?;

    crate::identity::infer_identity(server, auth)
        .map_err(|e| Response::error(format!("identity inference failed: {}", e)))
}

// ---------------------------------------------------------------------------
// Step B: me.json
// ---------------------------------------------------------------------------

fn write_me_json(
    state: &SharedState,
    handler: &str,
    display_name: &str,
    git_server: &str,
    github_email: Option<&str>,
) -> Result<(), Response> {
    let gitim_dir = state.repo_root.join(".gitim");
    std::fs::create_dir_all(&gitim_dir)
        .map_err(|e| Response::error(format!("failed to create .gitim dir: {}", e)))?;

    let me_path = gitim_dir.join("me.json");

    // Merge semantics: start from the existing file (if any) so fields this
    // code path doesn't set — github_email, provider, model, system_prompt,
    // env, user-added extras — aren't silently erased on re-onboard.
    let existing = read_existing_me(&me_path);

    let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let patch = MeJson {
        handler: Some(handler.to_string()),
        display_name: Some(display_name.to_string()),
        git_server: Some(git_server.to_string()),
        onboarded_at: Some(now),
        github_email: github_email.map(str::to_string),
        ..Default::default()
    };

    let mut merged = existing.merged_with(patch);
    // Going from guest → real identity, drop any stale guest flag.
    merged.clear_guest();

    let content = serde_json::to_string_pretty(&merged).unwrap();
    std::fs::write(&me_path, &content)
        .map_err(|e| Response::error(format!("failed to write me.json: {}", e)))?;

    Ok(())
}

/// Read existing me.json into a typed `MeJson`. Missing file / invalid JSON
/// degrade to `MeJson::default()` so callers can always merge into something.
fn read_existing_me(me_path: &std::path::Path) -> MeJson {
    let content = match std::fs::read_to_string(me_path) {
        Ok(s) => s,
        Err(_) => return MeJson::default(),
    };
    serde_json::from_str(&content).unwrap_or_default()
}

fn write_guest_me_json(state: &SharedState) -> Result<(), Response> {
    let gitim_dir = state.repo_root.join(".gitim");
    std::fs::create_dir_all(&gitim_dir)
        .map_err(|e| Response::error(format!("failed to create .gitim dir: {}", e)))?;

    let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    // Guest mode intentionally does NOT merge with an existing me.json —
    // entering guest clears prior identity (see handle_onboard guest branch).
    let me = MeJson {
        handler: None,
        guest: Some(true),
        onboarded_at: Some(now),
        ..Default::default()
    };

    let me_path = gitim_dir.join("me.json");
    let content = serde_json::to_string_pretty(&me).unwrap();
    std::fs::write(&me_path, &content)
        .map_err(|e| Response::error(format!("failed to write me.json: {}", e)))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Step C: EnsureRepo
// ---------------------------------------------------------------------------

fn ensure_repo(state: &SharedState, handler: &str) -> Result<(), Response> {
    let mut changed_paths: Vec<String> = Vec::new();

    // 1. .gitignore: ensure it contains ".gitim/"
    let gitignore_path = state.repo_root.join(".gitignore");
    let gitignore_content = std::fs::read_to_string(&gitignore_path).unwrap_or_default();
    if !gitignore_content
        .lines()
        .any(|line| line.trim() == ".gitim/")
    {
        let mut new_content = gitignore_content;
        if !new_content.is_empty() && !new_content.ends_with('\n') {
            new_content.push('\n');
        }
        new_content.push_str(".gitim/\n");
        std::fs::write(&gitignore_path, &new_content)
            .map_err(|e| Response::error(format!("failed to write .gitignore: {}", e)))?;
        changed_paths.push(".gitignore".to_string());
    }

    // 2. channels/general.meta.yaml + channels/general.thread
    let channels_dir = state.repo_root.join("channels");
    std::fs::create_dir_all(&channels_dir)
        .map_err(|e| Response::error(format!("failed to create channels dir: {}", e)))?;

    let meta_path = channels_dir.join("general.meta.yaml");
    if !meta_path.exists() {
        let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let meta = ChannelMeta {
            display_name: "General".to_string(),
            created_by: handler.to_string(),
            created_at: now.clone(),
            introduction: "默认频道".to_string(),
            members: vec![handler.to_string()],
        };
        let meta_str = serde_yaml::to_string(&meta).unwrap();
        std::fs::write(&meta_path, &meta_str)
            .map_err(|e| Response::error(format!("failed to write general.meta.yaml: {}", e)))?;
        changed_paths.push("channels/general.meta.yaml".to_string());

        // Create thread file with join event
        let thread_path = channels_dir.join("general.thread");
        if !thread_path.exists() {
            let h = Handler::new(handler)
                .map_err(|e| Response::error(format!("invalid handler: {}", e)))?;
            let join_line =
                gitim_core::formatter::format_event(1, &h, &now, "join", &serde_json::json!({}));
            std::fs::write(&thread_path, &join_line)
                .map_err(|e| Response::error(format!("failed to write general.thread: {}", e)))?;
            changed_paths.push("channels/general.thread".to_string());
        }
    }

    // 3. If anything changed: commit + push
    if !changed_paths.is_empty() {
        let path_refs: Vec<&str> = changed_paths.iter().map(|s| s.as_str()).collect();
        state
            .git_storage
            .add_and_commit(
                &path_refs,
                "init: repo structure (.gitignore + general channel)",
            )
            .map_err(|e| Response::error(format!("ensure_repo commit failed: {}", e)))?;

        if state.git_storage.has_remote() {
            match state.git_storage.push() {
                Ok(()) => {}
                Err(GitError::PushConflict) => {
                    // Someone else already initialized — discard our commit and move on
                    warn!("ensure_repo: push conflict — discarding local init (someone else initialized)");
                    state
                        .git_storage
                        .discard_unpushed()
                        .map_err(|e| Response::error(format!("discard_unpushed failed: {}", e)))?;
                }
                Err(e) => return Err(Response::error(format!("ensure_repo push failed: {}", e))),
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Step D: RegisterUser
// ---------------------------------------------------------------------------

fn register_user(state: &SharedState, handler: &str, display_name: &str) -> Result<bool, Response> {
    let users_dir = state.repo_root.join("users");
    std::fs::create_dir_all(&users_dir)
        .map_err(|e| Response::error(format!("failed to create users dir: {}", e)))?;

    let meta_path = users_dir.join(format!("{}.meta.yaml", handler));
    if meta_path.exists() {
        return Ok(false); // already registered
    }

    let meta = UserMeta {
        display_name: display_name.to_string(),
        role: "member".to_string(),
        introduction: "GitIM user".to_string(),
    };
    let meta_str = serde_yaml::to_string(&meta).unwrap();
    std::fs::write(&meta_path, &meta_str)
        .map_err(|e| Response::error(format!("failed to write user meta: {}", e)))?;

    let rel_path = format!("users/{}.meta.yaml", handler);
    let commit_msg = format!("user: register @{}", handler);

    let (name, email) = state.author_for(handler);
    state
        .git_storage
        .add_and_commit_as(&[&rel_path], &commit_msg, Some((&name, &email)))
        .map_err(|e| Response::error(format!("register_user commit failed: {}", e)))?;

    // Push with retry on conflict (skip if no remote, e.g. local git mode)
    if !state.git_storage.has_remote() {
        return Ok(true);
    }

    for attempt in 1..=MAX_PUSH_RETRIES {
        match state.git_storage.push() {
            Ok(()) => return Ok(true),
            Err(GitError::PushConflict) => {
                warn!(
                    "register_user: push conflict (attempt {}/{}), rebasing",
                    attempt, MAX_PUSH_RETRIES
                );
                state
                    .git_storage
                    .fetch()
                    .map_err(|e| Response::error(format!("fetch failed during retry: {}", e)))?;
                state.git_storage.rebase_onto_origin().map_err(|e| {
                    Response::error(format!("rebase failed (real conflict on user file): {}", e))
                })?;
            }
            Err(e) => return Err(Response::error(format!("register_user push failed: {}", e))),
        }
    }

    Err(Response::error(format!(
        "register_user: push still conflicting after {} retries",
        MAX_PUSH_RETRIES
    )))
}

// ---------------------------------------------------------------------------
// Auto-join general channel on registration
// ---------------------------------------------------------------------------

fn auto_join_general(state: &SharedState, handler: &str) -> Result<(), Response> {
    let meta_path = state.repo_root.join("channels/general.meta.yaml");
    if !meta_path.exists() {
        return Ok(());
    }

    let meta_content = std::fs::read_to_string(&meta_path)
        .map_err(|e| Response::error(format!("read meta: {}", e)))?;
    let mut meta: gitim_core::types::ChannelMeta = serde_yaml::from_str(&meta_content)
        .map_err(|e| Response::error(format!("parse meta: {}", e)))?;

    if meta.members.contains(&handler.to_string()) {
        return Ok(()); // idempotent
    }

    // Write join event to .thread
    let thread_path = state.repo_root.join("channels/general.thread");
    let existing = std::fs::read_to_string(&thread_path).unwrap_or_default();
    let file = gitim_core::parser::parse_thread(&existing)
        .map_err(|e| Response::error(format!("parse: {}", e)))?;
    let next_line = file.last_line_number() + 1;
    let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let h = Handler::new(handler).map_err(|e| Response::error(format!("handler: {}", e)))?;
    let event_line =
        gitim_core::formatter::format_event(next_line, &h, &now, "join", &serde_json::json!({}));

    use std::io::Write;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&thread_path)
        .and_then(|mut f| f.write_all(event_line.as_bytes()))
        .map_err(|e| Response::error(format!("write event: {}", e)))?;

    // Update meta.yaml members
    meta.members.push(handler.to_string());
    meta.members.sort();
    let meta_str = serde_yaml::to_string(&meta).unwrap();
    std::fs::write(&meta_path, &meta_str)
        .map_err(|e| Response::error(format!("write meta: {}", e)))?;

    // Git commit
    let (name, email) = state.author_for(handler);
    let _ = state.git_storage.add_and_commit_as(
        &["channels/general.thread", "channels/general.meta.yaml"],
        &format!("event: @{} join general", handler),
        Some((&name, &email)),
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use gitim_core::types::config::Config;
    use std::sync::Arc;
    use tokio::sync::broadcast;

    fn setup_test_state(tmp: &std::path::Path) -> SharedState {
        // Init a bare git repo to act as remote, then clone it
        let remote = tmp.join("remote.git");
        std::fs::create_dir_all(&remote).unwrap();
        std::process::Command::new("git")
            .args(["init", "--bare"])
            .current_dir(&remote)
            .output()
            .unwrap();

        let repo = tmp.join("repo");
        std::process::Command::new("git")
            .args(["clone", remote.to_str().unwrap(), repo.to_str().unwrap()])
            .output()
            .unwrap();

        // Configure git user for commits
        std::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(&repo)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(&repo)
            .output()
            .unwrap();

        // Create initial commit so we have a main branch
        std::fs::write(repo.join(".keep"), "").unwrap();
        std::process::Command::new("git")
            .args(["add", ".keep"])
            .current_dir(&repo)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(&repo)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["push", "-u", "origin", "HEAD"])
            .current_dir(&repo)
            .output()
            .unwrap();

        let (event_tx, _) = broadcast::channel(16);
        Arc::new(AppState::new(repo, Config::default(), event_tx, None))
    }

    fn git_auth(handler: &str, display_name: &str) -> AuthPayload {
        AuthPayload::Git {
            handler: handler.to_string(),
            display_name: display_name.to_string(),
            github_email: None,
        }
    }

    #[test]
    fn infer_git_mode_ok() {
        let id = infer("git".to_string(), git_auth("alice", "Alice")).unwrap();
        assert_eq!(id.handler.as_str(), "alice");
        assert_eq!(id.display_name, "Alice");
    }

    #[test]
    fn infer_unknown_server_returns_error() {
        let result = infer("unknown".to_string(), git_auth("x", "X"));
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn write_me_json_creates_file() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let (tx, _) = broadcast::channel(16);
        let state = Arc::new(AppState::new(repo.clone(), Config::default(), tx, None));

        write_me_json(
            &state,
            "alice",
            "Alice W",
            "github",
            Some("alice@example.com"),
        )
        .unwrap();

        let me_path = repo.join(".gitim").join("me.json");
        assert!(me_path.exists());
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&me_path).unwrap()).unwrap();
        assert_eq!(content["handler"], "alice");
        assert_eq!(content["git_server"], "github");
        assert_eq!(content["display_name"], "Alice W");
        assert_eq!(content["github_email"], "alice@example.com");
        assert!(content["onboarded_at"].as_str().is_some());
    }

    /// CLAUDE.md merge semantics: a re-onboard that doesn't pass
    /// github_email must NOT erase the value written by an earlier onboard.
    /// Same goes for any field this code path doesn't set (provider, model,
    /// system_prompt — written by runtime add_agent on top).
    #[tokio::test]
    async fn write_me_json_preserves_existing_unrelated_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join(".gitim")).unwrap();
        // Pre-existing me.json with a runtime-config field and an email
        std::fs::write(
            repo.join(".gitim/me.json"),
            r#"{
                "handler": "alice",
                "display_name": "Alice W",
                "git_server": "github",
                "github_email": "alice@example.com",
                "provider": "claude",
                "model": "sonnet-4-6"
            }"#,
        )
        .unwrap();
        let (tx, _) = broadcast::channel(16);
        let state = Arc::new(AppState::new(repo.clone(), Config::default(), tx, None));

        // Re-onboard without an email — simulates a token that no longer
        // exposes the verified email.
        write_me_json(&state, "alice", "Alice W", "github", None).unwrap();

        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(repo.join(".gitim/me.json")).unwrap())
                .unwrap();
        assert_eq!(
            content["github_email"], "alice@example.com",
            "github_email must survive re-onboard with None email"
        );
        assert_eq!(
            content["provider"], "claude",
            "runtime-only fields must survive re-onboard"
        );
        assert_eq!(content["model"], "sonnet-4-6");
    }

    #[tokio::test]
    async fn write_me_json_omits_email_when_none() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let (tx, _) = broadcast::channel(16);
        let state = Arc::new(AppState::new(repo.clone(), Config::default(), tx, None));

        write_me_json(&state, "alice", "Alice W", "git", None).unwrap();

        let me_path = repo.join(".gitim").join("me.json");
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&me_path).unwrap()).unwrap();
        assert!(
            content.get("github_email").is_none(),
            "github_email should be absent when no email inferred"
        );
    }

    #[tokio::test]
    async fn ensure_repo_creates_gitignore_and_channel() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());

        ensure_repo(&state, "alice").unwrap();

        // .gitignore should contain .gitim/
        let gitignore = std::fs::read_to_string(state.repo_root.join(".gitignore")).unwrap();
        assert!(gitignore.contains(".gitim/"));

        // channels/general.meta.yaml should exist
        assert!(state.repo_root.join("channels/general.meta.yaml").exists());
        assert!(state.repo_root.join("channels/general.thread").exists());

        // meta.yaml should have creator in members
        let meta_content =
            std::fs::read_to_string(state.repo_root.join("channels/general.meta.yaml")).unwrap();
        let meta: gitim_core::types::ChannelMeta = serde_yaml::from_str(&meta_content).unwrap();
        assert_eq!(meta.members, vec!["alice"]);

        // .thread should have a join event (not be empty)
        let thread_content =
            std::fs::read_to_string(state.repo_root.join("channels/general.thread")).unwrap();
        assert!(!thread_content.is_empty(), "thread should not be empty");
        assert!(
            thread_content.contains("[E:join]"),
            "thread should contain join event"
        );
        assert!(
            thread_content.contains("@alice"),
            "join event should reference alice"
        );
    }

    #[tokio::test]
    async fn ensure_repo_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());

        ensure_repo(&state, "alice").unwrap();
        // Second call should not fail
        ensure_repo(&state, "alice").unwrap();

        // .gitignore should not have duplicate entries
        let gitignore = std::fs::read_to_string(state.repo_root.join(".gitignore")).unwrap();
        let count = gitignore.lines().filter(|l| l.trim() == ".gitim/").count();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn register_user_creates_meta_and_pushes() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());

        let created = register_user(&state, "bob", "Bob Builder").unwrap();
        assert!(created);
        assert!(state.repo_root.join("users/bob.meta.yaml").exists());

        let content: serde_yaml::Value = serde_yaml::from_str(
            &std::fs::read_to_string(state.repo_root.join("users/bob.meta.yaml")).unwrap(),
        )
        .unwrap();
        assert_eq!(content["display_name"], "Bob Builder");
        assert_eq!(content["role"], "member");
    }

    #[tokio::test]
    async fn register_user_skips_if_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());

        // First registration
        let created = register_user(&state, "bob", "Bob").unwrap();
        assert!(created);

        // Second registration — should skip
        let created = register_user(&state, "bob", "Bob").unwrap();
        assert!(!created);
    }

    #[tokio::test]
    async fn full_onboard_git_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());

        let resp = handle_onboard(
            state.clone(),
            "git".to_string(),
            Some(git_auth("alice", "Alice")),
            false,
            false,
        )
        .await;
        assert!(resp.ok, "response should be ok: {:?}", resp.error);

        let data = resp.data.unwrap();
        assert_eq!(data["handler"], "alice");
        assert!(data["created"].as_bool().unwrap());

        // Verify side effects
        assert!(state.repo_root.join(".gitim/me.json").exists());
        assert!(state.repo_root.join("channels/general.meta.yaml").exists());
        assert!(state.repo_root.join("users/alice.meta.yaml").exists());

        let current = state.current_user.read().await;
        assert_eq!(current.as_deref(), Some("alice"));

        let users = state.users.read().await;
        assert!(users.contains(&"alice".to_string()));
        drop(users);
        drop(current);

        // Idempotent: second onboard should return created: false
        // Reset sync_started so spawn_sync_loop can be called again
        state
            .sync_started
            .store(false, std::sync::atomic::Ordering::SeqCst);

        let resp2 = handle_onboard(
            state.clone(),
            "git".to_string(),
            Some(git_auth("alice", "Alice")),
            false,
            false,
        )
        .await;
        assert!(resp2.ok);
        assert!(!resp2.data.unwrap()["created"].as_bool().unwrap());
    }

    /// Helper: create a clone of the bare remote with git config set up
    fn clone_from_bare(bare: &std::path::Path, dest: &std::path::Path) {
        std::process::Command::new("git")
            .args(["clone", bare.to_str().unwrap(), dest.to_str().unwrap()])
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dest)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dest)
            .output()
            .unwrap();
    }

    fn make_state(repo_path: std::path::PathBuf) -> SharedState {
        let (event_tx, _) = broadcast::channel(16);
        Arc::new(AppState::new(repo_path, Config::default(), event_tx, None))
    }

    /// 3 bots onboard concurrently to the same repo, then each sends a message.
    /// Regardless of ordering, all 3 should succeed.
    #[tokio::test]
    async fn three_bots_concurrent_onboard_and_send() {
        let tmp = tempfile::tempdir().unwrap();

        // Create bare remote with initial commit
        let bare = tmp.path().join("remote.git");
        std::fs::create_dir_all(&bare).unwrap();
        std::process::Command::new("git")
            .args(["init", "--bare"])
            .current_dir(&bare)
            .output()
            .unwrap();

        // Seed the bare repo with an initial commit (via a temp clone)
        let seed = tmp.path().join("seed");
        clone_from_bare(&bare, &seed);
        std::fs::write(seed.join(".keep"), "").unwrap();
        std::process::Command::new("git")
            .args(["add", ".keep"])
            .current_dir(&seed)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(&seed)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["push", "-u", "origin", "HEAD"])
            .current_dir(&seed)
            .output()
            .unwrap();

        // Create 3 clones (simulating 3 bots on different machines)
        let bot_a_path = tmp.path().join("bot-a");
        let bot_b_path = tmp.path().join("bot-b");
        let bot_c_path = tmp.path().join("bot-c");
        clone_from_bare(&bare, &bot_a_path);
        clone_from_bare(&bare, &bot_b_path);
        clone_from_bare(&bare, &bot_c_path);

        let state_a = make_state(bot_a_path.clone());
        let state_b = make_state(bot_b_path.clone());
        let state_c = make_state(bot_c_path.clone());

        // === Phase 1: All 3 bots onboard ===
        // Bot A goes first — creates everything, pushes successfully
        let resp_a = handle_onboard(
            state_a.clone(),
            "git".to_string(),
            Some(git_auth("bot-a", "Bot A")),
            false,
            false,
        )
        .await;
        assert!(resp_a.ok, "bot-a onboard failed: {:?}", resp_a.error);
        assert!(resp_a.data.as_ref().unwrap()["created"].as_bool().unwrap());

        // Bot B goes second — ensure_repo hits PushConflict, discards, registers ok
        let resp_b = handle_onboard(
            state_b.clone(),
            "git".to_string(),
            Some(git_auth("bot-b", "Bot B")),
            false,
            false,
        )
        .await;
        assert!(resp_b.ok, "bot-b onboard failed: {:?}", resp_b.error);
        assert!(resp_b.data.as_ref().unwrap()["created"].as_bool().unwrap());

        // Bot C goes third — same pattern
        let resp_c = handle_onboard(
            state_c.clone(),
            "git".to_string(),
            Some(git_auth("bot-c", "Bot C")),
            false,
            false,
        )
        .await;
        assert!(resp_c.ok, "bot-c onboard failed: {:?}", resp_c.error);
        assert!(resp_c.data.as_ref().unwrap()["created"].as_bool().unwrap());

        // === Verify: all 3 users registered in remote ===
        let verify = tmp.path().join("verify");
        clone_from_bare(&bare, &verify);
        assert!(
            verify.join("users/bot-a.meta.yaml").exists(),
            "bot-a user file missing in remote"
        );
        assert!(
            verify.join("users/bot-b.meta.yaml").exists(),
            "bot-b user file missing in remote"
        );
        assert!(
            verify.join("users/bot-c.meta.yaml").exists(),
            "bot-c user file missing in remote"
        );
        assert!(
            verify.join("channels/general.meta.yaml").exists(),
            "general channel missing"
        );
        assert!(
            verify.join("channels/general.thread").exists(),
            "general thread missing"
        );

        // === Phase 2: Each bot sends a message ===
        // Each bot needs to pull latest state first (simulating sync)
        for state in [&state_a, &state_b, &state_c] {
            state.git_storage.pull_rebase().unwrap();
            // Refresh users list from disk
            let users_dir = state.repo_root.join("users");
            let mut users = state.users.write().await;
            users.clear();
            if let Ok(entries) = std::fs::read_dir(&users_dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.ends_with(".meta.yaml") {
                        users.push(name.trim_end_matches(".meta.yaml").to_string());
                    }
                }
            }
        }

        // Bot A sends
        let send_a = crate::handlers::handle_request(
            crate::api::Request::Send {
                channel: "general".to_string(),
                body: "hello from bot-a".to_string(),
                reply_to: None,
                author: Some("bot-a".to_string()),
            },
            state_a.clone(),
        )
        .await;
        assert!(send_a.ok, "bot-a send failed: {:?}", send_a.error);

        // Bot B sends
        let send_b = crate::handlers::handle_request(
            crate::api::Request::Send {
                channel: "general".to_string(),
                body: "hello from bot-b".to_string(),
                reply_to: None,
                author: Some("bot-b".to_string()),
            },
            state_b.clone(),
        )
        .await;
        assert!(send_b.ok, "bot-b send failed: {:?}", send_b.error);

        // Bot C sends
        let send_c = crate::handlers::handle_request(
            crate::api::Request::Send {
                channel: "general".to_string(),
                body: "hello from bot-c".to_string(),
                reply_to: None,
                author: Some("bot-c".to_string()),
            },
            state_c.clone(),
        )
        .await;
        assert!(send_c.ok, "bot-c send failed: {:?}", send_c.error);

        // === Verify: all 3 messages in thread ===
        // Each bot committed locally. Read from bot-a's local thread.
        let thread_a = std::fs::read_to_string(bot_a_path.join("channels/general.thread")).unwrap();
        assert!(
            thread_a.contains("hello from bot-a"),
            "bot-a message missing"
        );

        // Bot B and C have their messages locally too
        let thread_b = std::fs::read_to_string(bot_b_path.join("channels/general.thread")).unwrap();
        assert!(
            thread_b.contains("hello from bot-b"),
            "bot-b message missing"
        );

        let thread_c = std::fs::read_to_string(bot_c_path.join("channels/general.thread")).unwrap();
        assert!(
            thread_c.contains("hello from bot-c"),
            "bot-c message missing"
        );

        println!("=== 3-bot concurrent onboard + send: ALL PASSED ===");
        println!("Bot A thread:\n{}", thread_a);
        println!("Bot B thread:\n{}", thread_b);
        println!("Bot C thread:\n{}", thread_c);
    }

    #[tokio::test]
    async fn test_onboard_new_user_joins_general() {
        let tmp = tempfile::tempdir().unwrap();

        // Create bare remote with initial commit
        let bare = tmp.path().join("remote.git");
        std::fs::create_dir_all(&bare).unwrap();
        std::process::Command::new("git")
            .args(["init", "--bare"])
            .current_dir(&bare)
            .output()
            .unwrap();

        let seed = tmp.path().join("seed");
        clone_from_bare(&bare, &seed);
        std::fs::write(seed.join(".keep"), "").unwrap();
        std::process::Command::new("git")
            .args(["add", ".keep"])
            .current_dir(&seed)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(&seed)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["push", "-u", "origin", "HEAD"])
            .current_dir(&seed)
            .output()
            .unwrap();

        // Bot A onboards first — creates general channel, auto-joins
        let bot_a_path = tmp.path().join("bot-a");
        clone_from_bare(&bare, &bot_a_path);
        let state_a = make_state(bot_a_path.clone());

        let resp_a = handle_onboard(
            state_a.clone(),
            "git".to_string(),
            Some(git_auth("bot-a", "Bot A")),
            false,
            false,
        )
        .await;
        assert!(resp_a.ok, "bot-a onboard failed: {:?}", resp_a.error);

        // Bot B onboards second — should auto-join existing general channel
        let bot_b_path = tmp.path().join("bot-b");
        clone_from_bare(&bare, &bot_b_path);
        let state_b = make_state(bot_b_path.clone());

        let resp_b = handle_onboard(
            state_b.clone(),
            "git".to_string(),
            Some(git_auth("bot-b", "Bot B")),
            false,
            false,
        )
        .await;
        assert!(resp_b.ok, "bot-b onboard failed: {:?}", resp_b.error);

        // Verify: bot-b's local general.meta.yaml should have both members
        let meta_content =
            std::fs::read_to_string(bot_b_path.join("channels/general.meta.yaml")).unwrap();
        let meta: gitim_core::types::ChannelMeta = serde_yaml::from_str(&meta_content).unwrap();
        assert!(
            meta.members.contains(&"bot-a".to_string()),
            "bot-a should be a member"
        );
        assert!(
            meta.members.contains(&"bot-b".to_string()),
            "bot-b should be a member"
        );

        // Verify: thread should have join events for both
        let thread_content =
            std::fs::read_to_string(bot_b_path.join("channels/general.thread")).unwrap();
        let file = gitim_core::parser::parse_thread(&thread_content).unwrap();
        let events = file.events();
        assert!(
            events.len() >= 2,
            "should have at least 2 join events, got {}",
            events.len()
        );
    }

    #[tokio::test]
    async fn onboard_admin_mode_sets_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());

        let resp = handle_onboard(
            state.clone(),
            "git".to_string(),
            Some(git_auth("admin-user", "Admin")),
            true,
            false,
        )
        .await;
        assert!(resp.ok, "onboard should succeed: {:?}", resp.error);
        assert!(
            state.is_admin.load(std::sync::atomic::Ordering::SeqCst),
            "is_admin should be true"
        );
    }

    #[tokio::test]
    async fn onboard_guest_mode_writes_me_json_and_skips_registration() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());

        let resp = handle_onboard(
            state.clone(),
            "git".to_string(),
            None,
            false,
            true,
        )
        .await;
        assert!(resp.ok, "guest onboard should succeed: {:?}", resp.error);

        let data = resp.data.unwrap();
        assert_eq!(data["guest"], true);

        let me_path = state.repo_root.join(".gitim/me.json");
        assert!(me_path.exists(), "me.json should be written");
        let me: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&me_path).unwrap()).unwrap();
        assert_eq!(me["guest"], true);
        assert!(me["handler"].is_null(), "handler should be null for guest");

        assert!(
            !state.repo_root.join("users").exists()
                || std::fs::read_dir(state.repo_root.join("users"))
                    .unwrap()
                    .count()
                    == 0,
            "no user files should be created"
        );

        let current = state.current_user.read().await;
        assert!(current.is_none(), "current_user should be None for guest");

        assert!(
            state.is_guest.load(std::sync::atomic::Ordering::SeqCst),
            "is_guest should be true"
        );
    }
}
