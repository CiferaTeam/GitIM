use std::path::Path;

use gitim_core::dm::parse_dm_filename;
use gitim_core::parser::parse_thread;
use gitim_core::responses::{SearchMessage, SearchResponse};
use tracing::warn;

use crate::api::Response;
use crate::state::SharedState;

const SEARCH_RESULT_LIMIT: usize = 10;

pub async fn handle_search(state: SharedState, query: String) -> Response {
    if query.is_empty() {
        return Response::error("search requires a non-empty query");
    }
    let repo_root = state.repo_root.clone();
    let current_user = state.current_user.read().await.clone();

    match tokio::task::spawn_blocking(move || {
        search_messages(&repo_root, current_user.as_deref(), &query)
    })
    .await
    {
        Ok(messages) => Response::json(SearchResponse { messages }),
        Err(error) => Response::error(format!("search task failed: {error}")),
    }
}

fn search_messages(
    repo_root: &Path,
    current_user: Option<&str>,
    query: &str,
) -> Vec<SearchMessage> {
    let mut matches = Vec::new();

    scan_flat_threads(
        &repo_root.join("channels"),
        "channel",
        query,
        &mut matches,
        |path| {
            path.file_stem()
                .and_then(|value| value.to_str())
                .map(str::to_string)
        },
    );

    scan_card_threads(repo_root, query, &mut matches);

    if let Some(current_user) = current_user {
        scan_flat_threads(&repo_root.join("dm"), "dm", query, &mut matches, |path| {
            let channel = path.file_stem()?.to_str()?;
            let (first, second) = parse_dm_filename(channel)?;
            (first == current_user || second == current_user).then(|| channel.to_string())
        });
    }

    matches.sort_by(|left, right| {
        right
            .timestamp
            .cmp(&left.timestamp)
            .then_with(|| left.channel.cmp(&right.channel))
            .then_with(|| right.line_number.cmp(&left.line_number))
    });
    matches.truncate(SEARCH_RESULT_LIMIT);
    matches
}

fn scan_flat_threads(
    directory: &Path,
    channel_type: &str,
    query: &str,
    matches: &mut Vec<SearchMessage>,
    channel_for_path: impl Fn(&Path) -> Option<String>,
) {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            warn!("search: cannot read {}: {}", directory.display(), error);
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("thread") {
            continue;
        }
        let Some(channel) = channel_for_path(&path) else {
            continue;
        };
        scan_thread(&path, &channel, channel_type, query, matches);
    }
}

fn scan_card_threads(repo_root: &Path, query: &str, matches: &mut Vec<SearchMessage>) {
    let channels_dir = repo_root.join("channels");
    let channels = match std::fs::read_dir(&channels_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            warn!("search: cannot read {}: {}", channels_dir.display(), error);
            return;
        }
    };

    for channel_entry in channels.flatten() {
        let channel_path = channel_entry.path();
        if !channel_path.is_dir() {
            continue;
        }
        let Some(channel) = channel_path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let cards_dir = channel_path.join("cards");
        let cards = match std::fs::read_dir(&cards_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                warn!("search: cannot read {}: {}", cards_dir.display(), error);
                continue;
            }
        };

        for card_entry in cards.flatten() {
            let card_path = card_entry.path();
            if !card_path.is_dir() {
                continue;
            }
            let Some(card_id) = card_path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            let thread_path = card_path.join("discussion.thread");
            let identifier = format!("channels/{channel}/cards/{card_id}");
            scan_thread(&thread_path, &identifier, "card", query, matches);
        }
    }
}

fn scan_thread(
    path: &Path,
    channel: &str,
    channel_type: &str,
    query: &str,
    matches: &mut Vec<SearchMessage>,
) {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            warn!("search: cannot read {}: {}", path.display(), error);
            return;
        }
    };
    let thread = match parse_thread(&content) {
        Ok(thread) => thread,
        Err(error) => {
            warn!("search: cannot parse {}: {}", path.display(), error);
            return;
        }
    };

    matches.extend(
        thread
            .messages()
            .into_iter()
            .filter(|message| message.body.contains(query))
            .map(|message| SearchMessage {
                channel: channel.to_string(),
                channel_type: channel_type.to_string(),
                line_number: message.line_number,
                parent_line: message.point_to,
                author: message.author.to_string(),
                timestamp: message.timestamp.clone(),
                body: message.body.clone(),
            }),
    );
}
