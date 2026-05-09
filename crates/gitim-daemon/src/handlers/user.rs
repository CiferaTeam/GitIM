use crate::api::Response;
use crate::state::SharedState;
use gitim_core::types::{Handler, UserMeta};
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

/// Move `users/<handler>.meta.yaml` → `archive/users/<handler>.meta.yaml`
/// in a single commit. Mirrors `handle_archive_channel` in shape: validate,
/// `git mv`, commit-as-author, push with retry, rollback on failure.
///
/// Unlike channels, users have no creator-only permission gate at this
/// layer — the depart_user composite (A.4) is the principal caller and
/// daemon resolves the author. A direct call from a registered user is
/// treated as authoritative; refining permissions is deferred to A.5/A.7.
pub async fn handle_archive_user(
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

    // 3. Already-archived check first — if the archive entry exists, the
    //    "user not found" message would mislead callers; report the actual
    //    state instead.
    let archive_dir = state.repo_root.join("archive/users");
    let archive_path = archive_dir.join(format!("{}.meta.yaml", handler));
    if archive_path.exists() {
        return Response::error(format!("user @{} is already archived", handler));
    }

    // 4. Validate active path exists.
    let active_path = state
        .repo_root
        .join(format!("users/{}.meta.yaml", handler));
    if !active_path.exists() {
        return Response::error(format!("user @{} not found", handler));
    }

    // 5. Ensure archive/users/ directory exists.
    if let Err(e) = std::fs::create_dir_all(&archive_dir) {
        return Response::error(format!("failed to create archive/users dir: {}", e));
    }

    // 6. git mv users/<h>.meta.yaml → archive/users/<h>.meta.yaml
    let from_rel = format!("users/{}.meta.yaml", handler);
    let to_rel = format!("archive/users/{}.meta.yaml", handler);
    if let Err(e) = state.git_storage.mv(&from_rel, &to_rel) {
        return Response::error(format!("git mv failed: {}", e));
    }

    // 7. Commit. On failure, reverse the git mv to leave the tree clean.
    let commit_msg = format!("archive: depart user @{}", handler);
    let (author_name, author_email) = state.author_for(&author);
    if let Err(e) = state.git_storage.add_and_commit_as(
        &[&to_rel],
        &commit_msg,
        Some((&author_name, &author_email)),
    ) {
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

    // 9. Drop archived handler from in-memory users list. The post-sync
    //    refresh in state.rs already does this from disk after the next
    //    cycle, but updating now keeps `list_users` consistent before sync.
    {
        let mut users = state.users.write().await;
        users.retain(|u| u != &handler);
    }

    info!("user @{} archived by @{}", handler, author);

    let payload = gitim_core::responses::ArchiveUserResponse {
        handler,
        archived_by: author,
    };
    Response::success(serde_json::to_value(payload).unwrap())
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

    // 3. Validate archive path exists.
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
    let active_path = state
        .repo_root
        .join(format!("users/{}.meta.yaml", handler));
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

    // 6. git mv archive → active.
    let from_rel = format!("archive/users/{}.meta.yaml", handler);
    let to_rel = format!("users/{}.meta.yaml", handler);
    if let Err(e) = state.git_storage.mv(&from_rel, &to_rel) {
        return Response::error(format!("git mv failed: {}", e));
    }

    // 7. Commit. Rollback git mv on failure.
    let commit_msg = format!("archive: restore user @{}", handler);
    let (author_name, author_email) = state.author_for(&author);
    if let Err(e) = state.git_storage.add_and_commit_as(
        &[&to_rel],
        &commit_msg,
        Some((&author_name, &author_email)),
    ) {
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

    // 9. Re-add restored handler to in-memory users list (mirror archive's
    //    drop above).
    {
        let mut users = state.users.write().await;
        if !users.contains(&handler) {
            users.push(handler.clone());
            users.sort();
        }
    }

    info!("user @{} unarchived by @{}", handler, author);

    let payload = gitim_core::responses::UnarchiveUserResponse {
        handler,
        unarchived_by: author,
    };
    Response::success(serde_json::to_value(payload).unwrap())
}

pub async fn handle_depart_user(_state: SharedState, _handler: String) -> Response {
    Response::error("not yet implemented (A.4 pending)")
}
