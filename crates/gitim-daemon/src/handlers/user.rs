use crate::api::{Event, Response};
use crate::handlers::ensure_author_not_departed;
use crate::state::SharedState;
use gitim_core::types::{Handler, UserMeta, MAX_INTRODUCTION_LEN};
use gitim_sync::git::GitError;
use tracing::{info, warn};

const MAX_PUSH_RETRIES: u32 = 3;

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

    // Archive Contract 2: handlers are terminally unique once departed.
    // Reject re-registration before touching `users/` so a stale call
    // can't recreate an active meta.yaml that would race with the
    // archived one.
    let archive_meta = state
        .repo_root
        .join("archive/users")
        .join(format!("{}.meta.yaml", handler));
    if archive_meta.exists() {
        return Response::error(format!(
            "handler @{} is reserved (previously departed)",
            handler
        ));
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
        return Response::json(payload);
    }

    // Create meta file
    let meta = UserMeta {
        display_name,
        role,
        introduction,
    };
    let meta_str = match Response::yaml_string(&meta, "user meta") {
        Ok(meta_str) => meta_str,
        Err(resp) => return resp,
    };

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
    Response::json(payload)
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
        return Response::json(payload);
    }

    meta.introduction = introduction;
    let meta_str = match Response::yaml_string(&meta, "user meta") {
        Ok(meta_str) => meta_str,
        Err(resp) => return resp,
    };
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
    Response::json(payload)
}

/// Move `users/<handler>.meta.yaml` → `archive/users/<handler>.meta.yaml`
/// in a single commit. Mirrors `handle_archive_channel` in shape: validate,
/// `git mv`, commit-as-author, push with retry, rollback on failure.
///
/// Unlike channels, users have no creator-only permission gate at this
/// layer — the depart_user composite (A.4) is the principal caller and
/// daemon resolves the author. A direct call from a registered user is
/// treated as authoritative; refining permissions is deferred to A.5/A.7.
pub async fn handle_archive_user(state: SharedState, handler: String, author: String) -> Response {
    // 1. Validate handler format.
    if let Err(e) = Handler::new(&handler) {
        return Response::error(format!("invalid handler: {}", e));
    }

    // 2. Validate author is registered + not already departed.
    // archive_user gates on departed author for consistency with the rest
    // of Contract 2 — a departed actor can't author further commits, even
    // ones that archive someone else. unarchive_user, by contrast, must
    // remain reachable so departure is reversible.
    if let Err(resp) = ensure_author_not_departed(&state, &author) {
        return resp;
    }
    {
        let users = state.users.read().await;
        if !users.contains(&author) {
            return Response::error(format!("unknown user: {}", author));
        }
    }

    // Check archive path before active path: if both somehow exist (e.g., split-brain
    // recovery), the user-actionable state is "already archived"; if only archive exists,
    // reporting "not found" would mislead the caller.
    let archive_dir = state.repo_root.join("archive/users");
    let archive_path = archive_dir.join(format!("{}.meta.yaml", handler));
    if archive_path.exists() {
        return Response::error(format!("user @{} is already archived", handler));
    }

    // 4. Validate active path exists.
    let active_path = state.repo_root.join(format!("users/{}.meta.yaml", handler));
    if !active_path.exists() {
        return Response::error(format!("user @{} not found", handler));
    }

    // 5. Ensure archive/users/ directory exists.
    if let Err(e) = std::fs::create_dir_all(&archive_dir) {
        return Response::error(format!("failed to create archive/users dir: {}", e));
    }

    // Commit-tree lock: held across git mv + commit + push so a concurrent
    // `handle_send` (also takes this lock) can't slip a `git add` + `git
    // commit` in between our staged mv and our `add_and_commit_as`, which
    // would bundle the unrelated send into our archive commit. Critical
    // section is all blocking subprocess calls; std::sync::Mutex guard
    // must not cross any `.await`.
    let _commit_guard = state.commit_lock.lock().unwrap_or_else(|e| e.into_inner());

    // 6. git mv users/<h>.meta.yaml → archive/users/<h>.meta.yaml
    let from_rel = format!("users/{}.meta.yaml", handler);
    let to_rel = format!("archive/users/{}.meta.yaml", handler);
    if let Err(e) = state.git_storage.mv(&from_rel, &to_rel) {
        return Response::error(format!("git mv failed: {}", e));
    }

    // 6b. If the user has a board, git mv it into archive/showboards/ as well.
    let board_from_rel = format!("showboards/{}/board.md", handler);
    let board_to_rel = format!("archive/showboards/{}/board.md", handler);
    let board_active_path = state.repo_root.join(&board_from_rel);
    let mut commit_paths: Vec<&str> = vec![&to_rel];
    let mut board_moved = false;
    if board_active_path.exists() {
        let board_archive_dir = state.repo_root.join("archive/showboards");
        if let Err(e) = std::fs::create_dir_all(board_archive_dir.join(&handler)) {
            // Rollback the meta git mv so the tree stays clean.
            let _ = state.git_storage.mv(&to_rel, &from_rel);
            return Response::error(format!(
                "archive_user: failed to create archive/showboards dir: {}",
                e
            ));
        }
        if let Err(e) = state.git_storage.mv(&board_from_rel, &board_to_rel) {
            let _ = state.git_storage.mv(&to_rel, &from_rel);
            return Response::error(format!(
                "archive_user: board git mv failed: {}; rolled back meta git mv",
                e
            ));
        }
        board_moved = true;
        commit_paths.push(&board_to_rel);
    }

    // 7. Commit. On failure, reverse all git mv operations to leave the tree clean.
    let commit_msg = format!("archive: depart user @{}", handler);
    let (author_name, author_email) = state.author_for(&author);
    if let Err(e) = state.git_storage.add_and_commit_as(
        &commit_paths,
        &commit_msg,
        Some((&author_name, &author_email)),
    ) {
        if board_moved {
            let _ = state.git_storage.mv(&board_to_rel, &board_from_rel);
        }
        if let Err(rb) = state.git_storage.mv(&to_rel, &from_rel) {
            warn!("archive_user: rollback git mv also failed: {}", rb);
        }
        return Response::error(format!(
            "archive_user commit failed: {}; rolled back git mv",
            e
        ));
    }

    // 8. Push with retry (skip if no remote).
    if state.git_storage.has_remote() {
        let mut pushed = false;
        for attempt in 1..=MAX_PUSH_RETRIES {
            match state.git_storage.push() {
                Ok(()) => {
                    pushed = true;
                    break;
                }
                Err(GitError::PushConflict) => {
                    warn!(
                        "archive_user: push conflict (attempt {}/{}), rebasing",
                        attempt, MAX_PUSH_RETRIES
                    );
                    if let Err(e) = state.git_storage.fetch() {
                        return Response::error(format!("archive_user fetch failed: {}", e));
                    }
                    if let Err(e) = state.git_storage.rebase_onto_origin() {
                        return Response::error(format!("archive_user rebase failed: {}", e));
                    }
                }
                Err(e) => {
                    return Response::error(format!("archive_user push failed: {}", e));
                }
            }
        }
        if !pushed {
            return Response::error(format!(
                "archive_user: push still conflicting after {} retries",
                MAX_PUSH_RETRIES
            ));
        }
    }

    // Commit tree is stable — drop the lock BEFORE any `.await` below.
    // std::sync::MutexGuard must not cross await points, and everything
    // from here on (in-memory users update, event broadcast) is non-mutating
    // for the commit tree.
    drop(_commit_guard);

    // 9. Drop archived handler from in-memory users list. The post-sync
    //    refresh in state.rs already does this from disk after the next
    //    cycle, but updating now keeps `list_users` consistent before sync.
    {
        let mut users = state.users.write().await;
        users.retain(|u| u != &handler);
    }

    // Broadcast SSE event so subscribers (WebUI / runtime) can react without
    // waiting for the next sync cycle. Symmetric with Event::CardArchived.
    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let _ = state.event_tx.send(Event::UserArchived {
        handler: handler.clone(),
        archived_by: author.clone(),
        timestamp,
    });

    info!("user @{} archived by @{}", handler, author);

    let payload = gitim_core::responses::ArchiveUserResponse {
        handler,
        archived_by: author,
    };
    Response::json(payload)
}

/// Restore `archive/users/<handler>.meta.yaml` → `users/<handler>.meta.yaml`.
/// Symmetric reverse of `handle_archive_user`; same rollback semantics.
pub async fn handle_unarchive_user(
    state: SharedState,
    handler: String,
    author: String,
) -> Response {
    // 1. Validate handler format.
    if let Err(e) = Handler::new(&handler) {
        return Response::error(format!("invalid handler: {}", e));
    }

    // 2. Validate author is registered.
    {
        let users = state.users.read().await;
        if !users.contains(&author) {
            return Response::error(format!("unknown user: {}", author));
        }
    }

    // Check archive path before active path: the actionable error in the
    // common case is "no archive entry to restore". If the active path
    // already exists too (split-brain), the conflict guard below catches it
    // — but reporting "archive source missing" first when neither exists
    // matches the user's mental model of unarchive.
    let archive_path = state
        .repo_root
        .join(format!("archive/users/{}.meta.yaml", handler));
    if !archive_path.exists() {
        return Response::error(format!(
            "archive source does not exist for user @{}",
            handler
        ));
    }

    // 4. Active path must not already exist (handler reuse conflict).
    let active_path = state.repo_root.join(format!("users/{}.meta.yaml", handler));
    if active_path.exists() {
        return Response::error(format!(
            "user @{} already exists in active location; unarchive aborted",
            handler
        ));
    }

    // 5. Ensure users/ parent dir exists.
    let users_dir = state.repo_root.join("users");
    if let Err(e) = std::fs::create_dir_all(&users_dir) {
        return Response::error(format!("failed to create users dir: {}", e));
    }

    // Commit-tree lock: held across git mv + commit + push so a concurrent
    // `handle_send` (also takes this lock) can't slip a `git add` + `git
    // commit` in between our staged mv and our `add_and_commit_as`, which
    // would bundle the unrelated send into our unarchive commit. Critical
    // section is all blocking subprocess calls; std::sync::Mutex guard
    // must not cross any `.await`.
    let _commit_guard = state.commit_lock.lock().unwrap_or_else(|e| e.into_inner());

    // 6. git mv archive → active.
    let from_rel = format!("archive/users/{}.meta.yaml", handler);
    let to_rel = format!("users/{}.meta.yaml", handler);
    if let Err(e) = state.git_storage.mv(&from_rel, &to_rel) {
        return Response::error(format!("git mv failed: {}", e));
    }

    // 6b. If the user has an archived board, restore it.
    let board_from_rel = format!("archive/showboards/{}/board.md", handler);
    let board_to_rel = format!("showboards/{}/board.md", handler);
    let board_archive_path = state.repo_root.join(&board_from_rel);
    let mut commit_paths: Vec<&str> = vec![&to_rel];
    let mut board_moved = false;
    if board_archive_path.exists() {
        let showboards_dir = state.repo_root.join("showboards");
        if let Err(e) = std::fs::create_dir_all(showboards_dir.join(&handler)) {
            let _ = state.git_storage.mv(&to_rel, &from_rel);
            return Response::error(format!(
                "unarchive_user: failed to create showboards dir: {}",
                e
            ));
        }
        if let Err(e) = state.git_storage.mv(&board_from_rel, &board_to_rel) {
            let _ = state.git_storage.mv(&to_rel, &from_rel);
            return Response::error(format!(
                "unarchive_user: board git mv failed: {}; rolled back meta git mv",
                e
            ));
        }
        board_moved = true;
        commit_paths.push(&board_to_rel);
    }

    // 7. Commit. Rollback git mv on failure.
    let commit_msg = format!("archive: restore user @{}", handler);
    let (author_name, author_email) = state.author_for(&author);
    if let Err(e) = state.git_storage.add_and_commit_as(
        &commit_paths,
        &commit_msg,
        Some((&author_name, &author_email)),
    ) {
        if board_moved {
            let _ = state.git_storage.mv(&board_to_rel, &board_from_rel);
        }
        if let Err(rb) = state.git_storage.mv(&to_rel, &from_rel) {
            warn!("unarchive_user: rollback git mv also failed: {}", rb);
        }
        return Response::error(format!(
            "unarchive_user commit failed: {}; rolled back git mv",
            e
        ));
    }

    // 8. Push with retry.
    if state.git_storage.has_remote() {
        let mut pushed = false;
        for attempt in 1..=MAX_PUSH_RETRIES {
            match state.git_storage.push() {
                Ok(()) => {
                    pushed = true;
                    break;
                }
                Err(GitError::PushConflict) => {
                    warn!(
                        "unarchive_user: push conflict (attempt {}/{}), rebasing",
                        attempt, MAX_PUSH_RETRIES
                    );
                    if let Err(e) = state.git_storage.fetch() {
                        return Response::error(format!("unarchive_user fetch failed: {}", e));
                    }
                    if let Err(e) = state.git_storage.rebase_onto_origin() {
                        return Response::error(format!("unarchive_user rebase failed: {}", e));
                    }
                }
                Err(e) => {
                    return Response::error(format!("unarchive_user push failed: {}", e));
                }
            }
        }
        if !pushed {
            return Response::error(format!(
                "unarchive_user: push still conflicting after {} retries",
                MAX_PUSH_RETRIES
            ));
        }
    }

    // Commit tree is stable — drop the lock BEFORE any `.await` below.
    // std::sync::MutexGuard must not cross await points, and everything
    // from here on (in-memory users update, event broadcast) is non-mutating
    // for the commit tree.
    drop(_commit_guard);

    // 9. Re-add restored handler to in-memory users list (mirror archive's
    //    drop above).
    {
        let mut users = state.users.write().await;
        if !users.contains(&handler) {
            users.push(handler.clone());
            users.sort();
        }
    }

    // Broadcast SSE event so subscribers (WebUI / runtime) can react without
    // waiting for the next sync cycle. Symmetric with Event::CardUnarchived.
    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let _ = state.event_tx.send(Event::UserUnarchived {
        handler: handler.clone(),
        unarchived_by: author.clone(),
        timestamp,
    });

    info!("user @{} unarchived by @{}", handler, author);

    let payload = gitim_core::responses::UnarchiveUserResponse {
        handler,
        unarchived_by: author,
    };
    Response::json(payload)
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

    fn init_board_for(state: &SharedState, handler: &str) {
        let board_dir = state.repo_root.join(format!("showboards/{}", handler));
        std::fs::create_dir_all(&board_dir).unwrap();
        let content = format!(
            "---\nversion: 1\nhandler: {}\nupdated_at: 20260525T000000Z\nstatus: active\nsummary: test\ntags: []\n---\n",
            handler
        );
        std::fs::write(board_dir.join("board.md"), &content).unwrap();
        std::process::Command::new("git")
            .args(["add", &format!("showboards/{}/board.md", handler)])
            .current_dir(&state.repo_root)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", &format!("board: init @{}", handler)])
            .current_dir(&state.repo_root)
            .output()
            .unwrap();
    }

    #[tokio::test]
    async fn archive_user_moves_board_to_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());
        register(&state, "alice").await;
        register(&state, "bob").await;
        init_board_for(&state, "alice");

        let resp = handle_archive_user(state.clone(), "alice".to_string(), "bob".to_string()).await;
        assert!(resp.ok, "archive_user failed: {:?}", resp.error);

        assert!(!state.repo_root.join("users/alice.meta.yaml").exists());
        assert!(!state.repo_root.join("showboards/alice/board.md").exists());
        assert!(state
            .repo_root
            .join("archive/users/alice.meta.yaml")
            .exists());
        assert!(state
            .repo_root
            .join("archive/showboards/alice/board.md")
            .exists());
    }

    #[tokio::test]
    async fn archive_user_without_board_skips_board_step() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());
        register(&state, "alice").await;
        register(&state, "bob").await;

        let resp = handle_archive_user(state.clone(), "alice".to_string(), "bob".to_string()).await;
        assert!(resp.ok, "archive_user failed: {:?}", resp.error);

        assert!(!state.repo_root.join("users/alice.meta.yaml").exists());
        assert!(state
            .repo_root
            .join("archive/users/alice.meta.yaml")
            .exists());
        assert!(!state
            .repo_root
            .join("archive/showboards/alice/board.md")
            .exists());
    }

    #[tokio::test]
    async fn unarchive_user_restores_board_from_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());
        register(&state, "alice").await;
        register(&state, "bob").await;
        init_board_for(&state, "alice");

        let resp = handle_archive_user(state.clone(), "alice".to_string(), "bob".to_string()).await;
        assert!(resp.ok, "archive_user failed: {:?}", resp.error);

        let resp =
            handle_unarchive_user(state.clone(), "alice".to_string(), "bob".to_string()).await;
        assert!(resp.ok, "unarchive_user failed: {:?}", resp.error);

        assert!(state.repo_root.join("users/alice.meta.yaml").exists());
        assert!(state.repo_root.join("showboards/alice/board.md").exists());
        assert!(!state
            .repo_root
            .join("archive/users/alice.meta.yaml")
            .exists());
        assert!(!state
            .repo_root
            .join("archive/showboards/alice/board.md")
            .exists());
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

        let resp =
            handle_update_user(state.clone(), "alice".to_string(), "GitIM user".to_string()).await;
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

    fn install_failing_precommit_hook(repo: &std::path::Path) {
        let hook = repo.join(".git/hooks/pre-commit");
        std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&hook).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&hook, perms).unwrap();
        }
    }

    fn remove_precommit_hook(repo: &std::path::Path) {
        let _ = std::fs::remove_file(repo.join(".git/hooks/pre-commit"));
    }

    #[tokio::test]
    async fn archive_user_rollbacks_board_on_commit_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_state(tmp.path());
        register(&state, "alice").await;
        register(&state, "bob").await;
        init_board_for(&state, "alice");

        install_failing_precommit_hook(&state.repo_root);

        let resp = handle_archive_user(state.clone(), "alice".to_string(), "bob".to_string()).await;
        assert!(!resp.ok, "expected commit failure, got ok");
        let err = resp.error.unwrap_or_default();
        assert!(
            err.contains("commit failed"),
            "expected 'commit failed' in error, got: {}",
            err
        );

        remove_precommit_hook(&state.repo_root);

        // Both meta and board must be rolled back to active locations.
        assert!(
            state.repo_root.join("users/alice.meta.yaml").exists(),
            "meta should be rolled back to users/"
        );
        assert!(
            state.repo_root.join("showboards/alice/board.md").exists(),
            "board should be rolled back to showboards/"
        );
        assert!(
            !state
                .repo_root
                .join("archive/users/alice.meta.yaml")
                .exists(),
            "archive meta should not exist after rollback"
        );
        assert!(
            !state
                .repo_root
                .join("archive/showboards/alice/board.md")
                .exists(),
            "archive board should not exist after rollback"
        );
    }
}
