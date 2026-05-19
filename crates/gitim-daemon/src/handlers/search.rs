use crate::api::Response;
use crate::state::SharedState;

const INDEXER_DISABLED_MSG: &str =
    "search index disabled for this clone (set indexer.enabled=true in .gitim/config.yaml and restart daemon)";

#[allow(clippy::too_many_arguments)]
pub async fn handle_search(
    state: SharedState,
    query: Option<String>,
    author: Option<String>,
    channel: Option<String>,
    channel_type: Option<String>,
    limit: usize,
    offset: usize,
    include_cards: bool,
) -> Response {
    let current_user = state.current_user.read().await.clone();
    let index = {
        let guard = state.index.read().unwrap_or_else(|e| e.into_inner());
        match &*guard {
            Some(idx) => idx.clone(),
            None => return Response::error(INDEXER_DISABLED_MSG),
        }
    };

    let params = gitim_index::SearchParams {
        query,
        author,
        channel,
        channel_type,
        current_user,
        limit,
        offset,
        include_cards,
    };

    match tokio::task::spawn_blocking(move || index.search(params)).await {
        Ok(Ok(result)) => {
            use gitim_core::responses::{SearchMessage, SearchResponse};
            let messages: Vec<SearchMessage> = result
                .messages
                .iter()
                .map(|m| SearchMessage {
                    channel: m.channel.clone(),
                    channel_type: m.channel_type.clone(),
                    line_number: m.line_number,
                    parent_line: m.parent_line,
                    author: m.author.clone(),
                    timestamp: m.timestamp.clone(),
                    body: m.body.clone(),
                })
                .collect();
            let payload = SearchResponse {
                messages,
                total: result.total as u64,
            };
            Response::success(serde_json::to_value(payload).unwrap_or_else(|e| { tracing::error!("serializing response: {e}"); serde_json::Value::Null }))
        }
        Ok(Err(gitim_index::IndexError::Rebuilding)) => Response::error("indexing_in_progress"),
        Ok(Err(gitim_index::IndexError::EmptySearch)) => {
            Response::error("search requires at least one of: query, author")
        }
        Ok(Err(e)) => Response::error(format!("search failed: {}", e)),
        Err(e) => Response::error(format!("search task failed: {}", e)),
    }
}

pub async fn handle_reindex(state: SharedState) -> Response {
    let index = {
        let guard = state.index.read().unwrap_or_else(|e| e.into_inner());
        match &*guard {
            Some(idx) => idx.clone(),
            None => return Response::error(INDEXER_DISABLED_MSG),
        }
    };

    let repo_root = state.repo_root.clone();
    let head = match state.git_storage.rev_parse("HEAD") {
        Ok(h) => h,
        Err(e) => return Response::error(format!("failed to get HEAD: {}", e)),
    };

    match tokio::task::spawn_blocking(move || index.reindex(&repo_root, &head)).await {
        Ok(Ok(count)) => {
            let payload = gitim_core::responses::ReindexResponse {
                status: "complete".to_string(),
                messages_indexed: count as u64,
            };
            Response::success(serde_json::to_value(payload).unwrap_or_else(|e| { tracing::error!("serializing response: {e}"); serde_json::Value::Null }))
        }
        Ok(Err(e)) => Response::error(format!("reindex failed: {}", e)),
        Err(e) => Response::error(format!("reindex task failed: {}", e)),
    }
}
