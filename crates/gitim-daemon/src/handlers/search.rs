use crate::api::Response;
use crate::state::SharedState;

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
        let guard = state.index.read().unwrap();
        match &*guard {
            Some(idx) => idx.clone(),
            None => return Response::error("search index not available"),
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
            let messages: Vec<serde_json::Value> = result
                .messages
                .iter()
                .map(|m| {
                    serde_json::json!({
                        "channel": m.channel,
                        "channel_type": m.channel_type,
                        "line_number": m.line_number,
                        "parent_line": m.parent_line,
                        "author": m.author,
                        "timestamp": m.timestamp,
                        "body": m.body,
                    })
                })
                .collect();
            Response::success(serde_json::json!({
                "messages": messages,
                "total": result.total,
            }))
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
        let guard = state.index.read().unwrap();
        match &*guard {
            Some(idx) => idx.clone(),
            None => return Response::error("search index not available"),
        }
    };

    let repo_root = state.repo_root.clone();
    let head = match state.git_storage.rev_parse("HEAD") {
        Ok(h) => h,
        Err(e) => return Response::error(format!("failed to get HEAD: {}", e)),
    };

    match tokio::task::spawn_blocking(move || index.reindex(&repo_root, &head)).await {
        Ok(Ok(count)) => Response::success(serde_json::json!({
            "status": "complete",
            "messages_indexed": count,
        })),
        Ok(Err(e)) => Response::error(format!("reindex failed: {}", e)),
        Err(e) => Response::error(format!("reindex task failed: {}", e)),
    }
}
