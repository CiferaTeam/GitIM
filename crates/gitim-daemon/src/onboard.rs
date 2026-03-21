use crate::api::Response;
use crate::identity::{AuthData, GitServer, InferredIdentity};
use crate::state::{AppState, SharedState};
use gitim_sync::git::GitError;
use tracing::{info, warn};

const MAX_PUSH_RETRIES: u32 = 3;

/// Full onboard orchestration: identity -> me.json -> ensure_repo -> register_user -> sync loop.
pub async fn handle_onboard(
    state: SharedState,
    git_server: String,
    auth: serde_json::Value,
) -> Response {
    // --- Step A: Infer identity ---
    let identity = match infer(git_server.clone(), auth) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let handler = identity.handler.as_str().to_string();
    let display_name = identity.display_name.clone();

    info!("onboard: identity inferred — @{}", handler);

    // --- Step B: Write me.json ---
    if let Err(resp) = write_me_json(&state, &handler, &display_name, &git_server) {
        return resp;
    }
    {
        let mut current = state.current_user.write().await;
        *current = Some(handler.clone());
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

    info!("onboard: user registered — @{} (created={})", handler, created);

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

    Response::success(serde_json::json!({
        "handler": handler,
        "created": created,
    }))
}

// ---------------------------------------------------------------------------
// Step A helpers
// ---------------------------------------------------------------------------

fn infer(git_server: String, auth: serde_json::Value) -> Result<InferredIdentity, Response> {
    let server: GitServer = serde_json::from_value(serde_json::Value::String(git_server.clone()))
        .map_err(|_| Response::error(format!("unknown git_server: {}", git_server)))?;

    // Determine which AuthData variant to deserialize based on git_server string.
    // The auth JSON must contain the fields for the matching variant (without the "type" tag),
    // so we inject the tag before deserializing.
    let mut auth_obj = auth;
    if let Some(obj) = auth_obj.as_object_mut() {
        obj.insert("type".to_string(), serde_json::Value::String(git_server.clone()));
    } else {
        return Err(Response::error("auth must be a JSON object"));
    }

    let auth_data: AuthData = serde_json::from_value(auth_obj)
        .map_err(|e| Response::error(format!("invalid auth data: {}", e)))?;

    crate::identity::infer_identity(server, auth_data)
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
) -> Result<(), Response> {
    let gitim_dir = state.repo_root.join(".gitim");
    std::fs::create_dir_all(&gitim_dir)
        .map_err(|e| Response::error(format!("failed to create .gitim dir: {}", e)))?;

    let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let me = serde_json::json!({
        "handler": handler,
        "git_server": git_server,
        "display_name": display_name,
        "inferred_at": now,
    });

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
    if !gitignore_content.lines().any(|line| line.trim() == ".gitim/") {
        let mut new_content = gitignore_content;
        if !new_content.is_empty() && !new_content.ends_with('\n') {
            new_content.push('\n');
        }
        new_content.push_str(".gitim/\n");
        std::fs::write(&gitignore_path, &new_content)
            .map_err(|e| Response::error(format!("failed to write .gitignore: {}", e)))?;
        changed_paths.push(".gitignore".to_string());
    }

    // 2. channels/general.meta.json + channels/general.thread
    let channels_dir = state.repo_root.join("channels");
    std::fs::create_dir_all(&channels_dir)
        .map_err(|e| Response::error(format!("failed to create channels dir: {}", e)))?;

    let meta_path = channels_dir.join("general.meta.json");
    if !meta_path.exists() {
        let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let meta = serde_json::json!({
            "display_name": "General",
            "created_by": handler,
            "created_at": now,
            "introduction": "默认频道",
        });
        let meta_str = serde_json::to_string_pretty(&meta).unwrap();
        std::fs::write(&meta_path, &meta_str)
            .map_err(|e| Response::error(format!("failed to write general.meta.json: {}", e)))?;
        changed_paths.push("channels/general.meta.json".to_string());

        // Create empty thread file
        let thread_path = channels_dir.join("general.thread");
        if !thread_path.exists() {
            std::fs::write(&thread_path, "")
                .map_err(|e| Response::error(format!("failed to write general.thread: {}", e)))?;
            changed_paths.push("channels/general.thread".to_string());
        }
    }

    // 3. If anything changed: commit + push
    if !changed_paths.is_empty() {
        let path_refs: Vec<&str> = changed_paths.iter().map(|s| s.as_str()).collect();
        state
            .git_storage
            .add_and_commit(&path_refs, "init: repo structure (.gitignore + general channel)")
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

fn register_user(
    state: &SharedState,
    handler: &str,
    display_name: &str,
) -> Result<bool, Response> {
    let users_dir = state.repo_root.join("users");
    std::fs::create_dir_all(&users_dir)
        .map_err(|e| Response::error(format!("failed to create users dir: {}", e)))?;

    let meta_path = users_dir.join(format!("{}.meta.json", handler));
    if meta_path.exists() {
        return Ok(false); // already registered
    }

    let meta = serde_json::json!({
        "display_name": display_name,
        "role": "member",
        "introduction": "GitIM user",
    });
    let meta_str = serde_json::to_string_pretty(&meta).unwrap();
    std::fs::write(&meta_path, &meta_str)
        .map_err(|e| Response::error(format!("failed to write user meta: {}", e)))?;

    let rel_path = format!("users/{}.meta.json", handler);
    let commit_msg = format!("user: register @{}", handler);

    state
        .git_storage
        .add_and_commit(&[&rel_path], &commit_msg)
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
                    Response::error(format!(
                        "rebase failed (real conflict on user file): {}",
                        e
                    ))
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
            .args(["push", "-u", "origin", "main"])
            .current_dir(&repo)
            .output()
            .unwrap();

        let (event_tx, _) = broadcast::channel(16);
        Arc::new(AppState::new(repo, Config::default(), event_tx, None))
    }

    #[test]
    fn infer_git_mode_ok() {
        let auth = serde_json::json!({
            "handler": "alice",
            "display_name": "Alice"
        });
        let id = infer("git".to_string(), auth).unwrap();
        assert_eq!(id.handler.as_str(), "alice");
        assert_eq!(id.display_name, "Alice");
    }

    #[test]
    fn infer_unknown_server_returns_error() {
        let auth = serde_json::json!({"handler": "x", "display_name": "X"});
        let result = infer("unknown".to_string(), auth);
        assert!(result.is_err());
    }

    #[test]
    fn infer_bad_auth_returns_error() {
        // Missing required fields for github variant
        let auth = serde_json::json!({});
        let result = infer("github".to_string(), auth);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn write_me_json_creates_file() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let (tx, _) = broadcast::channel(16);
        let state = Arc::new(AppState::new(repo.clone(), Config::default(), tx, None));

        write_me_json(&state, "alice", "Alice W", "github").unwrap();

        let me_path = repo.join(".gitim").join("me.json");
        assert!(me_path.exists());
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&me_path).unwrap()).unwrap();
        assert_eq!(content["handler"], "alice");
        assert_eq!(content["git_server"], "github");
        assert_eq!(content["display_name"], "Alice W");
        assert!(content["inferred_at"].as_str().is_some());
    }

    #[tokio::test]
    async fn ensure_repo_creates_gitignore_and_channel() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());

        ensure_repo(&state, "alice").unwrap();

        // .gitignore should contain .gitim/
        let gitignore = std::fs::read_to_string(state.repo_root.join(".gitignore")).unwrap();
        assert!(gitignore.contains(".gitim/"));

        // channels/general.meta.json should exist
        assert!(state
            .repo_root
            .join("channels/general.meta.json")
            .exists());
        assert!(state.repo_root.join("channels/general.thread").exists());
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
        assert!(state.repo_root.join("users/bob.meta.json").exists());

        let content: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(state.repo_root.join("users/bob.meta.json")).unwrap(),
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

        let auth = serde_json::json!({
            "handler": "alice",
            "display_name": "Alice"
        });
        let resp = handle_onboard(state.clone(), "git".to_string(), auth).await;
        assert!(resp.ok, "response should be ok: {:?}", resp.error);

        let data = resp.data.unwrap();
        assert_eq!(data["handler"], "alice");
        assert!(data["created"].as_bool().unwrap());

        // Verify side effects
        assert!(state.repo_root.join(".gitim/me.json").exists());
        assert!(state.repo_root.join("channels/general.meta.json").exists());
        assert!(state.repo_root.join("users/alice.meta.json").exists());

        let current = state.current_user.read().await;
        assert_eq!(current.as_deref(), Some("alice"));

        let users = state.users.read().await;
        assert!(users.contains(&"alice".to_string()));
    }

    #[tokio::test]
    async fn full_onboard_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());

        let auth = serde_json::json!({
            "handler": "alice",
            "display_name": "Alice"
        });

        let resp1 = handle_onboard(state.clone(), "git".to_string(), auth.clone()).await;
        assert!(resp1.ok);
        assert!(resp1.data.unwrap()["created"].as_bool().unwrap());

        // Reset sync_started so spawn_sync_loop can be called again in second onboard
        state
            .sync_started
            .store(false, std::sync::atomic::Ordering::SeqCst);

        let resp2 = handle_onboard(state.clone(), "git".to_string(), auth).await;
        assert!(resp2.ok);
        // Second time: user already exists
        assert!(!resp2.data.unwrap()["created"].as_bool().unwrap());
    }
}
