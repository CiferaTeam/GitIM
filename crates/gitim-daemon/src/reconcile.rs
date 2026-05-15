use crate::state::SharedState;
use gitim_core::types::card::ArchivedVia;
use gitim_core::types::CardMeta;
use std::collections::HashSet;
use tracing::{info, warn};

/// Scan `channels/<ch>/` directories whose corresponding channel meta has
/// moved to `archive/channels/<ch>.meta.yaml` (legacy `archive_channel` left
/// these as orphans). For each orphan card dir:
///   - stamp `archived_via: channel` in `card.meta.yaml`
///   - git mv the whole card dir from `channels/<ch>/cards/<id>` to
///     `archive/channels/<ch>/cards/<id>`
///
/// All moves are committed as a single commit authored by `system@gitim`.
/// Returns the number of cards migrated. When 0, no commit is produced.
/// This function is idempotent: running it on a clean repo is a no-op.
///
/// **Locking**: holds `state.commit_lock` for the entire function body.  Any
/// operation that mutates the git commit tree must hold this lock first (see
/// project invariant `project_commit_tree_lock.md`).  The HTTP server may
/// already be live when this runs at boot, so concurrent handlers could race
/// without the lock.
pub async fn reconcile_orphan_cards(state: SharedState) -> Result<usize, String> {
    // Acquire commit_lock before touching the git tree.  Hold for the full
    // function so no concurrent handler can interleave git operations.
    let _guard = state.commit_lock.lock().expect("commit_lock poisoned");

    let repo_root = &state.repo_root;
    let channels_dir = repo_root.join("channels");
    if !channels_dir.exists() {
        return Ok(0);
    }

    let mut card_moves: Vec<(String, String)> = Vec::new();
    let mut commit_paths: Vec<String> = Vec::new();
    let mut processed_channels: HashSet<String> = HashSet::new();

    let entries = std::fs::read_dir(&channels_dir).map_err(|e| format!("read channels/: {}", e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("read dir entry: {}", e))?;
        let ft = entry.file_type().map_err(|e| format!("file_type: {}", e))?;
        if !ft.is_dir() {
            continue; // skip .meta.yaml files, .thread files, etc.
        }
        let channel_name = entry.file_name().to_string_lossy().to_string();

        let active_meta = channels_dir.join(format!("{}.meta.yaml", channel_name));
        let archive_meta = repo_root
            .join("archive/channels")
            .join(format!("{}.meta.yaml", channel_name));

        // Only process orphan channels: active meta gone AND archive meta present.
        if active_meta.exists() || !archive_meta.exists() {
            continue;
        }

        let cards_dir = entry.path().join("cards");
        if !cards_dir.exists() {
            continue;
        }

        let card_entries = std::fs::read_dir(&cards_dir)
            .map_err(|e| format!("read {}/cards: {}", channel_name, e))?;

        for card_entry in card_entries {
            let card_entry = card_entry.map_err(|e| format!("read card entry: {}", e))?;
            if !card_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let card_id = card_entry.file_name().to_string_lossy().to_string();
            let meta_path = card_entry.path().join("card.meta.yaml");
            if !meta_path.exists() {
                continue;
            }

            // Stamp archived_via = Channel.
            let yaml = std::fs::read_to_string(&meta_path)
                .map_err(|e| format!("read card meta {}: {}", card_id, e))?;
            let mut meta: CardMeta = serde_yaml::from_str(&yaml)
                .map_err(|e| format!("parse card meta {}: {}", card_id, e))?;
            meta.archived_via = Some(ArchivedVia::Channel);
            let new_yaml = serde_yaml::to_string(&meta)
                .map_err(|e| format!("serialize card meta {}: {}", card_id, e))?;
            std::fs::write(&meta_path, new_yaml)
                .map_err(|e| format!("write card meta {}: {}", card_id, e))?;

            let from_rel = format!("channels/{}/cards/{}", channel_name, card_id);
            let to_rel = format!("archive/channels/{}/cards/{}", channel_name, card_id);
            commit_paths.push(format!("{}/card.meta.yaml", to_rel));
            commit_paths.push(format!("{}/discussion.thread", to_rel));
            card_moves.push((from_rel, to_rel));
            processed_channels.insert(channel_name.clone());
        }
    }

    if card_moves.is_empty() {
        return Ok(0);
    }

    // Ensure target parent directories exist before git mv.
    for (_, to_rel) in &card_moves {
        let to_parent = repo_root
            .join(to_rel)
            .parent()
            .map(|p| p.to_path_buf())
            .ok_or_else(|| format!("invalid to_rel path: {}", to_rel))?;
        std::fs::create_dir_all(&to_parent)
            .map_err(|e| format!("mkdir {}: {}", to_parent.display(), e))?;
    }

    // git mv each card dir.
    for (from_rel, to_rel) in &card_moves {
        state
            .git_storage
            .mv(from_rel, to_rel)
            .map_err(|e| format!("git mv {} -> {}: {}", from_rel, to_rel, e))?;
    }

    // Best-effort removal of empty channel directories left on disk.
    // Git doesn't track empty dirs so they don't affect the index, but they
    // are debris that confuses future directory scans.  If a dir is not yet
    // empty (shouldn't happen, but be defensive), we just warn and move on.
    for ch in &processed_channels {
        let cards_dir = channels_dir.join(ch).join("cards");
        if let Err(e) = std::fs::remove_dir(&cards_dir) {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!(
                    "reconcile: could not remove empty dir {}: {}",
                    cards_dir.display(),
                    e
                );
            }
        }
        let ch_dir = channels_dir.join(ch);
        if let Err(e) = std::fs::remove_dir(&ch_dir) {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!(
                    "reconcile: could not remove empty dir {}: {}",
                    ch_dir.display(),
                    e
                );
            }
        }
    }

    // Single commit, system author.
    let path_refs: Vec<&str> = commit_paths.iter().map(|s| s.as_str()).collect();
    let commit_msg = "chore: reconcile orphan cards under archived channels";
    state
        .git_storage
        .add_and_commit_as(&path_refs, commit_msg, Some(("system", "system@gitim")))
        .map_err(|e| format!("reconcile commit failed: {}", e))?;

    // Best-effort push; failure is non-fatal — sync_loop will retry.
    if state.git_storage.has_remote() {
        if let Err(e) = state.git_storage.push() {
            warn!("reconcile push failed (will retry via sync_loop): {}", e);
        }
    }

    info!(
        "reconcile: migrated {} orphan card(s) to archive",
        card_moves.len()
    );
    Ok(card_moves.len())
}
