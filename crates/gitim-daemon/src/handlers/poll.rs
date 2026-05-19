use crate::api::Response;
use crate::handlers::entry_to_json;
use crate::state::SharedState;

use gitim_core::dm::parse_dm_filename;
use gitim_core::parser::parse_thread;
use gitim_core::types::{ChannelMeta, Handler, Message, ThreadEntry};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tracing::warn;

pub async fn handle_poll(state: SharedState, since: Option<String>) -> Response {
    // Self-departure check. If `archive/users/<self_handler>.meta.yaml`
    // exists, this daemon's own agent has been departed (via burn-self,
    // or another clone burning this handler and the change syncing in).
    // Surfacing a tagged `self_departed` error lets the runtime fast-path
    // into self-cleanup (kill daemon + rm clone + agents removal + SSE)
    // instead of exponential backoff on a corpse.
    //
    // Placed first so we skip the rest of the I/O once the agent is gone.
    // No-op for guest sessions (they never write user.meta.yaml) and for
    // admin/system identities (not tracked in users/).
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
                    return Response::success(serde_json::to_value(payload).unwrap_or_else(|e| { tracing::error!("serializing response: {e}"); serde_json::Value::Null }));
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
        return Response::success(serde_json::to_value(payload).unwrap_or_else(|e| { tracing::error!("serializing response: {e}"); serde_json::Value::Null }));
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
        for path in diff.keys() {
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
                                .is_some_and(|me| meta.members.contains(me))
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
                            let entries: Vec<serde_json::Value> =
                                parsed.entries.iter().map(entry_to_json).collect();
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
        } else if path_str.starts_with("crons/")
            && path_str.ends_with(".thread")
            && !path_str.starts_with("archive/")
        {
            // Cron fire: `crons/<name>/<theoretical_ts>.thread`. Each fire
            // is a synthetic [@system] thread file written by `cron_engine::fire`
            // when a spec's schedule comes due. The runtime poller drives
            // agent_loop wake-ups; without a poll branch here, every
            // cron fire would be silently dropped — runtime never sees it,
            // target agent never wakes, the entire feature is dead.
            //
            // Ownership filter: in a multi-clone workspace, every clone
            // sees every other clone's cron fires syncing in via git.
            // Only the clone whose own me.json handler matches `spec.target`
            // should surface the change to its agent_loop. Other clones
            // would otherwise wake their (wrong) agents on someone else's
            // schedule. We read `crons/<name>/spec.yaml` from HEAD (current
            // state, not the historical revision in the diff) because the
            // ownership decision is "is this fire for me right now?", not
            // "was it for me when the spec was the way it was at fire time".
            //
            // archive/ exclusion: `path_str.starts_with("archive/")` is
            // false here because `path_str.starts_with("crons/")` is true
            // and the prefixes don't overlap — but spelling it out as a
            // belt-and-suspenders so a future refactor that shifts the
            // strip_prefix order won't accidentally start surfacing
            // archived crons.
            let rest = match path_str.strip_prefix("crons/") {
                Some(r) => r,
                None => continue,
            };
            // `crons/<name>/<ts>.thread` — the cron name is the directory
            // segment immediately under `crons/`. Anything without a slash
            // (e.g. a stray top-level file) is not a cron fire.
            let cron_name = match rest.split_once('/') {
                Some((n, _)) => n,
                None => continue,
            };
            // Read spec.yaml at HEAD via fs (same semantic as state-side
            // reads in cron handlers); the file is committed before the
            // fire so it must exist when the diff carries the fire path.
            // If the spec is gone (delete-then-fire race archived it
            // between fire and our poll), drop the change silently —
            // ownership can't be evaluated and an undecidable change is
            // worse than a missed wake-up.
            let spec_path = state
                .repo_root
                .join("crons")
                .join(cron_name)
                .join("spec.yaml");
            let spec_str = match std::fs::read_to_string(&spec_path) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let spec_target: String = match serde_yaml::from_str::<serde_yaml::Value>(&spec_str) {
                Ok(v) => v
                    .get("target")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string(),
                Err(_) => continue,
            };
            if spec_target.is_empty() {
                continue;
            }
            // Self check uses `current_user_snapshot` (read once at the top
            // of this fn, same source the channel/dm filters use). admin /
            // guest flags bypass the visibility filter elsewhere but cron
            // ownership is structural — admins shouldn't get woken by
            // every workspace cron. Apply unconditionally.
            let self_match = current_user_snapshot
                .as_deref()
                .map(|me| me == spec_target.as_str())
                .unwrap_or(false);
            if !self_match {
                continue;
            }
            let parsed = match parse_thread(added_content) {
                Ok(f) => f,
                Err(e) => {
                    warn!("poll: failed to parse cron thread {}: {}", path_str, e);
                    continue;
                }
            };
            if parsed.entries.is_empty() {
                continue;
            }
            let entries: Vec<serde_json::Value> =
                parsed.entries.iter().map(entry_to_json).collect();
            changes.push(gitim_core::responses::PollChange {
                channel: format!("cron:{}", cron_name),
                kind: "cron_thread".to_string(),
                entries,
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
        if kind == "channel"
            && !skip_filter
            && !channel_membership.get(&channel).copied().unwrap_or(true)
        {
            continue;
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

        let entries = enrich_entries_with_recipients(
            &parsed.entries,
            kind,
            &channel,
            &path_str,
            &state.repo_root,
        );

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
    Response::success(serde_json::to_value(payload).unwrap_or_else(|e| { tracing::error!("serializing response: {e}"); serde_json::Value::Null }))
}

/// Render thread entries to JSON, attaching a `recipients` field to
/// Message entries based on the channel kind:
///   - kind == "channel" → 3-rule routing via `compute_recipients`
///     (channel owner + parent-chain ancestors + explicit mentions)
///   - kind == "dm"      → recipients = sorted [member_a, member_b]
///   - other kinds       → no recipients (broadcast fallback applies
///     on the runtime side; preserves prior
///     behavior for card_thread / cron_thread)
///
/// Event entries (join/leave/etc.) never carry recipients regardless
/// of kind — routing is per-message and events are workspace-wide.
pub(super) fn enrich_entries_with_recipients(
    entries: &[ThreadEntry],
    kind: &str,
    channel: &str,
    path_str: &str,
    repo_root: &Path,
) -> Vec<serde_json::Value> {
    // Pre-load the channel meta + full thread once per change for parent
    // chain context. The diff only carries newly-added lines, so to walk
    // a reply's parent into pre-existing history we must read the full
    // committed thread file from disk.
    let channel_context: Option<(ChannelMeta, Vec<Message>)> = if kind == "channel" {
        let meta_path = repo_root
            .join("channels")
            .join(format!("{}.meta.yaml", channel));
        let thread_path = repo_root
            .join("channels")
            .join(format!("{}.thread", channel));
        let meta = std::fs::read_to_string(&meta_path)
            .ok()
            .and_then(|s| serde_yaml::from_str::<ChannelMeta>(&s).ok());
        let messages: Vec<Message> = std::fs::read_to_string(&thread_path)
            .ok()
            .and_then(|s| parse_thread(&s).ok())
            .map(|tf| {
                tf.entries
                    .into_iter()
                    .filter_map(|e| match e {
                        ThreadEntry::Message(m) => Some(m),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        meta.map(|m| (m, messages))
    } else {
        None
    };

    // For DM threads, recipients = sorted member pair derived from the
    // filename stem `<a>--<b>`. `parse_dm_filename` already returns the
    // pair in lex order (filenames are canonicalized at write time by
    // `dm_filename`), so the explicit `sort` here is defense against
    // future filename-canonicalization drift rather than a real reorder.
    let dm_members: Option<Vec<String>> = if kind == "dm" {
        path_str
            .strip_prefix("dm/")
            .and_then(|s| s.strip_suffix(".thread"))
            .and_then(parse_dm_filename)
            .map(|(a, b)| {
                let mut v = vec![a.to_string(), b.to_string()];
                v.sort();
                v
            })
    } else {
        None
    };

    entries
        .iter()
        .map(|entry| {
            let mut json = entry_to_json(entry);
            if let ThreadEntry::Message(msg) = entry {
                let recipients: Option<Vec<String>> = match (kind, &channel_context, &dm_members) {
                    ("channel", Some((meta, msgs)), _) => {
                        let r = gitim_core::recipients::compute_recipients(msg, meta, msgs);
                        if r.is_empty() {
                            // Spec guarantees Rule 1 always fires when
                            // created_by is set; empty here means either
                            // missing meta (logged above as failed read)
                            // or malformed meta. Surface a warn so it's
                            // diagnosable, fall through to broadcast.
                            warn!(
                                channel = %channel,
                                line = msg.line_number,
                                "poll: empty recipients computed; falling back to broadcast"
                            );
                            None
                        } else {
                            Some(r)
                        }
                    }
                    ("dm", _, Some(members)) => Some(members.clone()),
                    _ => None,
                };
                if let Some(r) = recipients {
                    json["recipients"] = serde_json::json!(r);
                }
            }
            json
        })
        .collect()
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
