use crate::api::Response;
use crate::handlers::entry_to_json;
use crate::state::SharedState;

use gitim_core::dm::parse_dm_filename;
use gitim_core::parser::parse_thread;
use gitim_core::types::{ChannelMeta, Handler};
use std::collections::{HashMap, HashSet};
use tracing::warn;

pub async fn handle_poll(state: SharedState, since: Option<String>) -> Response {
    // Self-departure check (archive-protocol B.4).
    //
    // If `archive/users/<self_handler>.meta.yaml` exists, this daemon's
    // own agent has been departed — most commonly via `gitim burn-self`
    // (C.3) writing the depart commits, but also reachable when another
    // clone burns this handler and the change syncs in. Either way the
    // agent_loop on the runtime side has nothing useful to do here:
    // every other poll branch would either return stale data or trip
    // the existing per-author commit-time guard. Surfacing a tagged
    // `self_departed` error lets the runtime fast-path into self-cleanup
    // (kill daemon + rm clone + ctx.agents removal + SSE) instead of
    // falling into exponential backoff on a corpse.
    //
    // Placed at the very top so we skip the rest of the I/O (rev-parse,
    // diff, membership cache) once the agent is gone — there's no
    // reason to do the work when the response is going to be discarded.
    //
    // Guest sessions never write the user.meta.yaml in the first place,
    // so this check is a no-op for them; admin/system identity is also
    // unaffected because it isn't tracked in users/.
    let self_handler_snapshot = state.current_user.read().await.clone();
    if let Some(handler) = self_handler_snapshot.as_deref() {
        let archive_path = state
            .repo_root
            .join("archive/users")
            .join(format!("{}.meta.yaml", handler));
        if archive_path.exists() {
            return Response::error_with_code("agent self-departed via burn-self", "self_departed");
        }
    }

    // Use @{upstream} (current branch's tracking ref) when available, else HEAD.
    let ref_name =
        if state.git_storage.has_remote() && state.git_storage.rev_parse("@{upstream}").is_ok() {
            "@{upstream}"
        } else {
            "HEAD"
        };

    // Get current commit hash
    let current_commit = match state.git_storage.rev_parse(ref_name) {
        Ok(hash) => hash,
        Err(e) => return Response::error(format!("failed to get commit: {}", e)),
    };

    // No cursor → start from parent commit so the first poll picks up recent messages
    let since_commit = match since {
        Some(s) if !s.is_empty() => s,
        _ => {
            match state.git_storage.rev_parse(&format!("{}~1", ref_name)) {
                Ok(parent) => parent,
                Err(_) => {
                    // No parent (initial commit) — return sync point with no changes
                    let payload = gitim_core::responses::PollResponse {
                        commit_id: current_commit,
                        changes: Vec::new(),
                    };
                    return Response::success(serde_json::to_value(payload).unwrap());
                }
            }
        }
    };

    // Validate commit hash format
    if since_commit.len() != 40 || !since_commit.chars().all(|c| c.is_ascii_hexdigit()) {
        return Response::error("invalid commit hash: expected 40-character hex string");
    }

    // Same cursor → no changes
    if since_commit == current_commit {
        let payload = gitim_core::responses::PollResponse {
            commit_id: current_commit,
            changes: Vec::new(),
        };
        return Response::success(serde_json::to_value(payload).unwrap());
    }

    // Compute diff
    let diff = match state.git_storage.diff_range(&since_commit, &current_commit) {
        Ok(d) => d,
        Err(e) => return Response::error(format!("diff failed (commit may not exist): {}", e)),
    };
    let changed_files = match state
        .git_storage
        .changed_files_range(&since_commit, &current_commit)
    {
        Ok(files) => files,
        Err(e) => {
            return Response::error(format!(
                "changed files failed (commit may not exist): {}",
                e
            ))
        }
    };

    // Parse changed files into entries
    let mut changes: Vec<gitim_core::responses::PollChange> = Vec::new();

    let current_user_snapshot = state.current_user.read().await.clone();
    let is_admin = state.is_admin.load(std::sync::atomic::Ordering::SeqCst);
    let is_guest = state.is_guest.load(std::sync::atomic::Ordering::SeqCst);
    let skip_filter = is_admin || is_guest;

    // Step 1: Build channel membership cache (admin skips — never checked)
    //
    // Channel names we want to pre-populate the cache for:
    //   channels/<ch>.thread             → ch
    //   channels/<ch>.meta.yaml          → ch
    //   channels/<ch>/cards/<id>/<file>  → ch (outer channel owns the card's membership)
    let extract_channel = |path_str: &str| -> Option<String> {
        let rest = path_str.strip_prefix("channels/")?;
        if let Some(stem) = rest
            .strip_suffix(".thread")
            .or_else(|| rest.strip_suffix(".meta.yaml"))
        {
            // Top-level channel file — the stem may contain no '/', that's the channel name.
            if !stem.contains('/') {
                return Some(stem.to_string());
            }
        }
        // Nested card path: channels/<ch>/cards/<id>/<file>
        let (ch, tail) = rest.split_once('/')?;
        if tail.starts_with("cards/") {
            return Some(ch.to_string());
        }
        None
    };

    let mut channel_membership: HashMap<String, bool> = HashMap::new();
    if !skip_filter {
        for (path, _) in &diff {
            let path_str = path.to_string_lossy();
            if let Some(ch_name) = extract_channel(&path_str) {
                if channel_membership.contains_key(&ch_name) {
                    continue;
                }
                let meta_path = state
                    .repo_root
                    .join("channels")
                    .join(format!("{}.meta.yaml", ch_name));
                let is_member = if let Ok(content) = std::fs::read_to_string(&meta_path) {
                    if let Ok(meta) = serde_yaml::from_str::<ChannelMeta>(&content) {
                        if meta.members.is_empty() {
                            true // Legacy: no members list = everyone has access
                        } else {
                            current_user_snapshot
                                .as_ref()
                                .map_or(false, |me| meta.members.contains(me))
                        }
                    } else {
                        true
                    }
                } else {
                    true
                };
                channel_membership.insert(ch_name, is_member);
            }
        }
    } // end if !skip_filter

    let mut emitted_boards: HashSet<String> = HashSet::new();
    for path in &changed_files {
        let path_str = path.to_string_lossy();
        if let Some(handler) = board_handler_from_path(&path_str) {
            let handler = handler.to_string();
            if !emitted_boards.insert(handler.clone()) {
                continue;
            }
            changes.push(gitim_core::responses::PollChange {
                channel: handler,
                kind: "board".to_string(),
                entries: Vec::new(),
            });
        }
    }

    // Step 2: Process diff entries with membership filter
    for (path, added_content) in &diff {
        let path_str = path.to_string_lossy();

        if board_handler_from_path(&path_str).is_some() {
            continue;
        }

        // Match card paths first so they don't fall through to the channel_meta /
        // channel branches below (which would otherwise mangle the channel name).
        if let Some(rest) = path_str.strip_prefix("channels/") {
            if let Some((ch, tail)) = rest.split_once('/') {
                if let Some(card_rest) = tail.strip_prefix("cards/") {
                    if let Some((card_id, file)) = card_rest.split_once('/') {
                        // Membership check via outer channel
                        if !skip_filter && !channel_membership.get(ch).copied().unwrap_or(true) {
                            continue;
                        }
                        let card_key = format!("card:{}/{}", ch, card_id);
                        if file == "card.meta.yaml" {
                            changes.push(gitim_core::responses::PollChange {
                                channel: card_key,
                                kind: "card_meta".to_string(),
                                entries: Vec::new(),
                            });
                            continue;
                        }
                        if file == "discussion.thread" {
                            let parsed = match parse_thread(added_content) {
                                Ok(f) => f,
                                Err(e) => {
                                    warn!("poll: failed to parse card thread {}: {}", path_str, e);
                                    continue;
                                }
                            };
                            if parsed.entries.is_empty() {
                                continue;
                            }
                            let entries: Vec<serde_json::Value> = parsed
                                .entries
                                .iter()
                                .map(|entry| entry_to_json(entry))
                                .collect();
                            changes.push(gitim_core::responses::PollChange {
                                channel: card_key,
                                kind: "card_thread".to_string(),
                                entries,
                            });
                            continue;
                        }
                        // Other files inside the card dir are ignored.
                        continue;
                    }
                }
            }
        }

        let (channel, kind) = if let Some(name) = path_str.strip_prefix("channels/") {
            if name.contains('/') {
                // Nested path we didn't handle above (e.g., future subtree). Skip
                // rather than let strip_suffix swallow it and emit malformed events.
                continue;
            }
            if let Some(ch_name) = name.strip_suffix(".thread") {
                (ch_name.to_string(), "channel")
            } else if let Some(ch_name) = name.strip_suffix(".meta.yaml") {
                // Meta change — only push if user is (now) a member
                if !skip_filter && !channel_membership.get(ch_name).copied().unwrap_or(true) {
                    continue;
                }
                changes.push(gitim_core::responses::PollChange {
                    channel: ch_name.to_string(),
                    kind: "channel_meta".to_string(),
                    entries: Vec::new(),
                });
                continue;
            } else {
                continue;
            }
        } else if let Some(name) = path_str.strip_prefix("dm/") {
            let name = name.strip_suffix(".thread").unwrap_or(name);
            (format!("dm:{}", name.replace("--", ",")), "dm")
        } else if let Some(name) = path_str.strip_prefix("archive/channels/") {
            // A channel showing up in `archive/channels/` means it was just
            // archived (or was created and archived inside this diff range).
            // Emit a `channel_meta` event so the client refetches both the
            // active and archived lists — otherwise the record silently
            // vanishes from every UI surface.
            if name.contains('/') {
                // Nested path (e.g. archive/channels/X/cards/...) — not our
                // business here; skip cleanly instead of letting the suffix
                // strippers mangle the name.
                continue;
            }
            let ch_name = name
                .strip_suffix(".thread")
                .or_else(|| name.strip_suffix(".meta.yaml"));
            if let Some(ch_name) = ch_name {
                changes.push(gitim_core::responses::PollChange {
                    channel: ch_name.to_string(),
                    kind: "channel_meta".to_string(),
                    entries: Vec::new(),
                });
            }
            continue;
        } else if let Some(name) = path_str.strip_prefix("archive/dm/") {
            // A DM thread showing up in `archive/dm/<sorted>.thread` means it
            // was just archived. Mirror the `archive/channels/` branch above:
            // emit a path-shaped event with no entries, keyed off the
            // canonical `dm:<a>,<b>` channel form so clients can reconcile
            // against their existing DM index.
            //
            // Visibility: archived DMs are still private to the two
            // participants — third parties must not learn the pair existed.
            // Apply the same filter as the active `dm/` branch.
            //
            // Unarchive (active path re-appears) flows through the normal
            // `dm/` branch above and emits `kind: "dm"` with the thread
            // content — symmetric with how channel unarchive surfaces, no
            // dedicated event needed.
            if name.contains('/') {
                // Future-proof: archive/dm/<X>/<...> isn't a thing in v1
                // but the strippers below would mangle the stem. Skip.
                continue;
            }
            let stem = match name.strip_suffix(".thread") {
                Some(s) => s,
                None => continue,
            };
            let (a, b) = match parse_dm_filename(stem) {
                Some(pair) => pair,
                None => continue,
            };
            if !skip_filter {
                match &current_user_snapshot {
                    Some(me) if me == a || me == b => { /* allowed */ }
                    _ => continue,
                }
            }
            changes.push(gitim_core::responses::PollChange {
                channel: format!("dm:{},{}", a, b),
                kind: "dm_archived".to_string(),
                entries: Vec::new(),
            });
            continue;
        } else if let Some(name) = path_str.strip_prefix("archive/users/") {
            // A user meta showing up in `archive/users/<handler>.meta.yaml`
            // means the handler departed. Workspace-wide event — anyone
            // polling needs to know so the active-user list refetches and
            // departed indicators surface. No participant filter (mirrors
            // channel_meta which is also broadcast to all callers).
            //
            // The plan keys the event by `handler`; the wire field is
            // `channel` because PollChange has no dedicated handler slot.
            // `kind: "user_archived"` is the discriminator.
            //
            // Note: user *unarchive* (active `users/<h>.meta.yaml` re-
            // appearing) currently emits nothing — poll has never had a
            // branch for `users/`, so user creates/updates also don't
            // surface. That's a pre-existing systemic gap, not a A.7
            // regression; introducing an explicit user_unarchived without
            // also surfacing user creates would be asymmetric.
            if name.contains('/') {
                continue;
            }
            let handler = match name.strip_suffix(".meta.yaml") {
                Some(h) => h,
                None => continue,
            };
            changes.push(gitim_core::responses::PollChange {
                channel: handler.to_string(),
                kind: "user_archived".to_string(),
                entries: Vec::new(),
            });
            continue;
        } else {
            continue;
        };

        // Channel membership filter
        if kind == "channel" && !skip_filter {
            if !channel_membership.get(&channel).copied().unwrap_or(true) {
                continue;
            }
        }

        // DM visibility filter — skip DMs not involving current user
        if kind == "dm" && !skip_filter {
            if let Some(stem) = path_str
                .strip_prefix("dm/")
                .and_then(|s| s.strip_suffix(".thread"))
            {
                if let Some((a, b)) = parse_dm_filename(stem) {
                    match &current_user_snapshot {
                        Some(me) if me == a || me == b => { /* allowed */ }
                        _ => continue,
                    }
                }
            }
        }

        // Parse added lines as entries (both messages and events)
        let parsed = match parse_thread(added_content) {
            Ok(f) => f,
            Err(e) => {
                warn!("poll: failed to parse diff for {}: {}", path_str, e);
                continue;
            }
        };

        if parsed.entries.is_empty() {
            continue;
        }

        let entries: Vec<serde_json::Value> = parsed
            .entries
            .iter()
            .map(|entry| entry_to_json(entry))
            .collect();

        changes.push(gitim_core::responses::PollChange {
            channel,
            kind: kind.to_string(),
            entries,
        });
    }

    let payload = gitim_core::responses::PollResponse {
        commit_id: current_commit,
        changes,
    };
    Response::success(serde_json::to_value(payload).unwrap())
}

fn board_handler_from_path(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("showboards/")?;
    let (handler, file) = rest.split_once('/')?;
    if file == "board.md" && Handler::new(handler).is_ok() {
        Some(handler)
    } else {
        None
    }
}
