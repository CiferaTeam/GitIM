use crate::api::{Event, Response};
use crate::handlers::ensure_author_not_departed;
use crate::state::SharedState;
use gitim_core::dm::dm_filename;
use gitim_core::types::Handler;
use gitim_sync::git::GitError;
use tracing::{info, warn};

const MAX_PUSH_RETRIES: u32 = 3;

/// Move `dm/<sorted-pair>.thread` → `archive/dm/<sorted-pair>.thread` in
/// a single commit. Per archive-protocol Contract 1, the archive path
/// mirrors the active path with only an `archive/` prefix — both use the
/// `dm/` (singular) directory to match the existing send/read convention,
/// and parallel `archive/users/` / `archive/channels/`.
///
/// Decision B1 from the plan: archive is single-party — `author` can
/// archive without confirmation from `peer`. Either side may archive
/// independently. (No locked-write semantics on DM threads themselves;
/// once moved, A.5's send-time guard rejects writes via the archived
/// path. That guard is a separate task.)
pub async fn handle_archive_dm(state: SharedState, peer: String, author: String) -> Response {
    // 1. Validate handler formats. Build the canonical sorted-pair stem
    //    via dm_filename so we never concatenate handlers manually.
    let peer_h = match Handler::new(&peer) {
        Ok(h) => h,
        Err(e) => return Response::error(format!("invalid peer: {}", e)),
    };
    let author_h = match Handler::new(&author) {
        Ok(h) => h,
        Err(e) => return Response::error(format!("invalid author: {}", e)),
    };
    let stem = dm_filename(&author_h, &peer_h);

    // 2. Validate author is registered + not already departed. archive_dm
    // gates on departed author for symmetry with archive_user / handle_send;
    // unarchive_dm intentionally does not, so DM threads remain reversible
    // even after a participant departs.
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
    let archive_dir = state.repo_root.join("archive/dm");
    let archive_path = archive_dir.join(format!("{}.thread", stem));
    if archive_path.exists() {
        return Response::error(format!("DM with @{} is already archived", peer));
    }

    // 3. Validate active path exists.
    let active_path = state.repo_root.join(format!("dm/{}.thread", stem));
    if !active_path.exists() {
        return Response::error(format!("DM with @{} not found", peer));
    }

    // 4. Ensure archive/dm/ directory exists.
    if let Err(e) = std::fs::create_dir_all(&archive_dir) {
        return Response::error(format!("failed to create archive/dm dir: {}", e));
    }

    // Commit-tree lock: held across git mv + commit + push so a concurrent
    // `handle_send` (also takes this lock) can't slip a `git add` + `git
    // commit` in between our staged mv and our `add_and_commit_as`, which
    // would bundle the unrelated send into our archive commit. Critical
    // section is all blocking subprocess calls; std::sync::Mutex guard
    // must not cross any `.await`.
    let _commit_guard = state.commit_lock.lock().unwrap_or_else(|e| e.into_inner());

    // 5. git mv dm/<stem>.thread → archive/dm/<stem>.thread
    let from_rel = format!("dm/{}.thread", stem);
    let to_rel = format!("archive/dm/{}.thread", stem);
    if let Err(e) = state.git_storage.mv(&from_rel, &to_rel) {
        return Response::error(format!("git mv failed: {}", e));
    }

    // 6. Commit. On failure, reverse the git mv to leave the tree clean.
    let commit_msg = format!("archive: dm with @{}", peer);
    let (author_name, author_email) = state.author_for(&author);
    if let Err(e) = state.git_storage.add_and_commit_as(
        &[&to_rel],
        &commit_msg,
        Some((&author_name, &author_email)),
    ) {
        if let Err(rb) = state.git_storage.mv(&to_rel, &from_rel) {
            warn!("archive_dm: rollback git mv also failed: {}", rb);
        }
        return Response::error(format!(
            "archive_dm commit failed: {}; rolled back git mv",
            e
        ));
    }

    // 7. Push with retry (skip if no remote).
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
                        "archive_dm: push conflict (attempt {}/{}), rebasing",
                        attempt, MAX_PUSH_RETRIES
                    );
                    if let Err(e) = state.git_storage.fetch() {
                        return Response::error(format!("archive_dm fetch failed: {}", e));
                    }
                    if let Err(e) = state.git_storage.rebase_onto_origin() {
                        return Response::error(format!("archive_dm rebase failed: {}", e));
                    }
                }
                Err(e) => {
                    return Response::error(format!("archive_dm push failed: {}", e));
                }
            }
        }
        if !pushed {
            return Response::error(format!(
                "archive_dm: push still conflicting after {} retries",
                MAX_PUSH_RETRIES
            ));
        }
    }

    // Commit tree is stable — drop the lock BEFORE any `.await` below.
    // std::sync::MutexGuard must not cross await points, and everything
    // from here on (event broadcast) is non-mutating for the commit tree.
    drop(_commit_guard);

    // Broadcast SSE event so subscribers (WebUI / runtime) can react without
    // waiting for the next sync cycle. Symmetric with Event::UserArchived.
    // Timestamp lives on the event, not on the RPC response — the response
    // shape stays aligned with ArchiveUserResponse / ArchiveChannelResponse.
    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let _ = state.event_tx.send(Event::DmArchived {
        peer: peer.clone(),
        archived_by: author.clone(),
        timestamp,
    });

    info!("dm with @{} archived by @{}", peer, author);

    let payload = gitim_core::responses::ArchiveDmResponse {
        archived_by: author,
        dm_pair_stem: stem,
    };
    Response::success(serde_json::to_value(payload).unwrap_or_else(|e| { tracing::error!("serializing response: {e}"); serde_json::Value::Null }))
}

/// Restore `archive/dm/<sorted-pair>.thread` → `dm/<sorted-pair>.thread`.
/// Symmetric reverse of `handle_archive_dm`; same rollback semantics.
pub async fn handle_unarchive_dm(state: SharedState, peer: String, author: String) -> Response {
    // 1. Validate handler formats and derive sorted-pair stem.
    let peer_h = match Handler::new(&peer) {
        Ok(h) => h,
        Err(e) => return Response::error(format!("invalid peer: {}", e)),
    };
    let author_h = match Handler::new(&author) {
        Ok(h) => h,
        Err(e) => return Response::error(format!("invalid author: {}", e)),
    };
    let stem = dm_filename(&author_h, &peer_h);

    // 2. Validate author is registered.
    {
        let users = state.users.read().await;
        if !users.contains(&author) {
            return Response::error(format!("unknown user: {}", author));
        }
    }

    // Check active path before archive path: if active still exists, the
    // user-actionable state is "already active / not archived"; reporting
    // "missing archive source" would obscure the more specific signal.
    let active_path = state.repo_root.join(format!("dm/{}.thread", stem));
    if active_path.exists() {
        return Response::error(format!("DM with @{} is not archived", peer));
    }

    // 3. Validate archive path exists.
    let archive_path = state.repo_root.join(format!("archive/dm/{}.thread", stem));
    if !archive_path.exists() {
        return Response::error(format!("DM with @{} not found in archive", peer));
    }

    // 4. Ensure dm/ parent dir exists.
    let dm_dir = state.repo_root.join("dm");
    if let Err(e) = std::fs::create_dir_all(&dm_dir) {
        return Response::error(format!("failed to create dm dir: {}", e));
    }

    // Commit-tree lock: see archive_dm rationale.
    let _commit_guard = state.commit_lock.lock().unwrap_or_else(|e| e.into_inner());

    // 5. git mv archive → active.
    let from_rel = format!("archive/dm/{}.thread", stem);
    let to_rel = format!("dm/{}.thread", stem);
    if let Err(e) = state.git_storage.mv(&from_rel, &to_rel) {
        return Response::error(format!("git mv failed: {}", e));
    }

    // 6. Commit. Rollback git mv on failure.
    let commit_msg = format!("archive: restore dm with @{}", peer);
    let (author_name, author_email) = state.author_for(&author);
    if let Err(e) = state.git_storage.add_and_commit_as(
        &[&to_rel],
        &commit_msg,
        Some((&author_name, &author_email)),
    ) {
        if let Err(rb) = state.git_storage.mv(&to_rel, &from_rel) {
            warn!("unarchive_dm: rollback git mv also failed: {}", rb);
        }
        return Response::error(format!(
            "unarchive_dm commit failed: {}; rolled back git mv",
            e
        ));
    }

    // 7. Push with retry.
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
                        "unarchive_dm: push conflict (attempt {}/{}), rebasing",
                        attempt, MAX_PUSH_RETRIES
                    );
                    if let Err(e) = state.git_storage.fetch() {
                        return Response::error(format!("unarchive_dm fetch failed: {}", e));
                    }
                    if let Err(e) = state.git_storage.rebase_onto_origin() {
                        return Response::error(format!("unarchive_dm rebase failed: {}", e));
                    }
                }
                Err(e) => {
                    return Response::error(format!("unarchive_dm push failed: {}", e));
                }
            }
        }
        if !pushed {
            return Response::error(format!(
                "unarchive_dm: push still conflicting after {} retries",
                MAX_PUSH_RETRIES
            ));
        }
    }

    // Commit tree is stable — drop the lock BEFORE any `.await` below.
    drop(_commit_guard);

    // Broadcast SSE event so subscribers (WebUI / runtime) can react without
    // waiting for the next sync cycle. Timestamp lives on the event only.
    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let _ = state.event_tx.send(Event::DmUnarchived {
        peer: peer.clone(),
        unarchived_by: author.clone(),
        timestamp,
    });

    info!("dm with @{} unarchived by @{}", peer, author);

    let payload = gitim_core::responses::UnarchiveDmResponse {
        unarchived_by: author,
        dm_pair_stem: stem,
    };
    Response::success(serde_json::to_value(payload).unwrap_or_else(|e| { tracing::error!("serializing response: {e}"); serde_json::Value::Null }))
}
