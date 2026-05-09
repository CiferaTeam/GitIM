use crate::api::Response;
use crate::state::SharedState;
use gitim_core::types::{Handler, UserMeta, MAX_INTRODUCTION_LEN};

pub async fn handle_register_user(
    state: SharedState,
    handler: String,
    display_name: String,
    role: String,
    introduction: String,
) -> Response {
    // Validate handler format
    if let Err(e) = Handler::new(&handler) {
        return Response::error(format!("invalid handler: {}", e));
    }

    let users_dir = state.repo_root.join("users");
    std::fs::create_dir_all(&users_dir).ok();
    let meta_path = users_dir.join(format!("{}.meta.yaml", handler));

    // If already exists, ensure user is in memory list and return success
    if meta_path.exists() {
        let mut users = state.users.write().await;
        if !users.contains(&handler) {
            users.push(handler.clone());
            users.sort();
        }
        let payload = gitim_core::responses::RegisterUserResponse {
            handler,
            exists: true,
        };
        return Response::success(serde_json::to_value(payload).unwrap());
    }

    // Create meta file
    let meta = UserMeta {
        display_name,
        role,
        introduction,
    };
    let meta_str = serde_yaml::to_string(&meta).unwrap();

    if let Err(e) = std::fs::write(&meta_path, &meta_str) {
        return Response::error(format!("failed to write user meta: {}", e));
    }

    // Add to users list
    {
        let mut users = state.users.write().await;
        if !users.contains(&handler) {
            users.push(handler.clone());
            users.sort();
        }
    }

    // Git add + commit (best effort)
    let (author_name, author_email) = state.author_for(&handler);
    let _ = state.git_storage.add_and_commit_as(
        &[&format!("users/{}.meta.yaml", handler)],
        &format!("user: register @{}", handler),
        Some((&author_name, &author_email)),
    );

    let payload = gitim_core::responses::RegisterUserResponse {
        handler,
        exists: false,
    };
    Response::success(serde_json::to_value(payload).unwrap())
}

/// Overwrite the `introduction` field of an existing user's meta.yaml.
///
/// Pre-conditions:
///   - the user must already be registered (we don't auto-register here —
///     the WebUI add-agent flow does that via onboard before calling us)
///   - `introduction` must be ≤ MAX_INTRODUCTION_LEN bytes
///
/// On success: rewrite `users/<handler>.meta.yaml`, git add + commit + push
/// using the same author identity as register_user. Empty introduction
/// becomes an empty string in the YAML (we don't drop the field — UserMeta
/// requires it).
pub async fn handle_update_user(
    state: SharedState,
    handler: String,
    introduction: String,
) -> Response {
    if let Err(e) = Handler::new(&handler) {
        return Response::error(format!("invalid handler: {}", e));
    }

    if introduction.len() > MAX_INTRODUCTION_LEN {
        return Response::error(format!(
            "introduction exceeds {} byte limit",
            MAX_INTRODUCTION_LEN
        ));
    }

    let meta_path = state
        .repo_root
        .join("users")
        .join(format!("{}.meta.yaml", handler));
    if !meta_path.exists() {
        return Response::error(format!("user not found: {}", handler));
    }

    let existing = match std::fs::read_to_string(&meta_path) {
        Ok(s) => s,
        Err(e) => return Response::error(format!("failed to read user meta: {}", e)),
    };
    let mut meta: UserMeta = match serde_yaml::from_str(&existing) {
        Ok(m) => m,
        Err(e) => return Response::error(format!("failed to parse user meta: {}", e)),
    };

    // Idempotent no-op: same value → return success without rewriting or
    // creating a noisy "no-change" git commit.
    if meta.introduction == introduction {
        let payload = gitim_core::responses::RegisterUserResponse {
            handler,
            exists: true,
        };
        return Response::success(serde_json::to_value(payload).unwrap());
    }

    meta.introduction = introduction;
    let meta_str = serde_yaml::to_string(&meta).unwrap();
    if let Err(e) = std::fs::write(&meta_path, &meta_str) {
        return Response::error(format!("failed to write user meta: {}", e));
    }

    let (author_name, author_email) = state.author_for(&handler);
    if let Err(e) = state.git_storage.add_and_commit_as(
        &[&format!("users/{}.meta.yaml", handler)],
        &format!("user: update introduction @{}", handler),
        Some((&author_name, &author_email)),
    ) {
        return Response::error(format!("update_user commit failed: {}", e));
    }

    let payload = gitim_core::responses::RegisterUserResponse {
        handler,
        exists: true,
    };
    Response::success(serde_json::to_value(payload).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use gitim_core::types::config::Config;
    use std::sync::Arc;
    use tokio::sync::broadcast;

    fn setup_state(tmp: &std::path::Path) -> SharedState {
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
        for (k, v) in [("user.email", "test@test.com"), ("user.name", "Test")] {
            std::process::Command::new("git")
                .args(["config", k, v])
                .current_dir(&repo)
                .output()
                .unwrap();
        }
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
        let (tx, _) = broadcast::channel(16);
        Arc::new(AppState::new(repo, Config::default(), tx, None))
    }

    async fn register(state: &SharedState, handler: &str) {
        let resp = handle_register_user(
            state.clone(),
            handler.to_string(),
            "Display".to_string(),
            "member".to_string(),
            "GitIM user".to_string(),
        )
        .await;
        assert!(resp.ok, "register_user failed: {:?}", resp.error);
    }

    #[tokio::test]
    async fn update_user_overwrites_introduction() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());
        register(&state, "alice").await;

        let resp = handle_update_user(
            state.clone(),
            "alice".to_string(),
            "AI assistant for code review".to_string(),
        )
        .await;
        assert!(resp.ok, "update_user failed: {:?}", resp.error);

        let meta_path = state.repo_root.join("users/alice.meta.yaml");
        let content = std::fs::read_to_string(&meta_path).unwrap();
        let meta: UserMeta = serde_yaml::from_str(&content).unwrap();
        assert_eq!(meta.introduction, "AI assistant for code review");
        // role + display_name preserved through the rewrite
        assert_eq!(meta.role, "member");
        assert_eq!(meta.display_name, "Display");
    }

    #[tokio::test]
    async fn update_user_idempotent_no_extra_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());
        register(&state, "alice").await;

        let resp = handle_update_user(
            state.clone(),
            "alice".to_string(),
            "GitIM user".to_string(),
        )
        .await;
        assert!(resp.ok);

        // No new commit beyond `init` + the registration: same-value short-
        // circuit must not produce a "user: update introduction" entry.
        let log = std::process::Command::new("git")
            .args(["log", "--oneline"])
            .current_dir(&state.repo_root)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&log.stdout);
        assert!(
            !stdout.contains("update introduction"),
            "no-op update should not commit, got log:\n{stdout}"
        );
    }

    #[tokio::test]
    async fn update_user_rejects_overlong_blurb() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());
        register(&state, "alice").await;

        let too_long = "x".repeat(MAX_INTRODUCTION_LEN + 1);
        let resp = handle_update_user(state.clone(), "alice".to_string(), too_long).await;
        assert!(!resp.ok);
        assert!(
            resp.error.unwrap_or_default().contains("byte limit"),
            "error should mention byte limit"
        );
    }

    #[tokio::test]
    async fn update_user_rejects_unknown_handler() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());
        // No register call — user doesn't exist.

        let resp = handle_update_user(
            state.clone(),
            "ghost".to_string(),
            "should fail".to_string(),
        )
        .await;
        assert!(!resp.ok);
        assert!(
            resp.error.unwrap_or_default().contains("not found"),
            "error should mention not found"
        );
    }

    #[tokio::test]
    async fn update_user_rejects_invalid_handler() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());

        let resp = handle_update_user(
            state.clone(),
            "INVALID_UPPER".to_string(),
            "blurb".to_string(),
        )
        .await;
        assert!(!resp.ok);
        assert!(
            resp.error.unwrap_or_default().contains("invalid handler"),
            "error should flag invalid handler"
        );
    }

    #[tokio::test]
    async fn update_user_writes_commit_to_git_log() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());
        register(&state, "alice").await;

        let resp = handle_update_user(
            state.clone(),
            "alice".to_string(),
            "Senior engineer".to_string(),
        )
        .await;
        assert!(resp.ok);

        let log = std::process::Command::new("git")
            .args(["log", "--oneline"])
            .current_dir(&state.repo_root)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&log.stdout);
        assert!(
            stdout.contains("update introduction @alice"),
            "expected an update commit in:\n{stdout}"
        );
    }
}
