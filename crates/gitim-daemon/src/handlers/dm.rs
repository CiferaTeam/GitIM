use crate::api::Response;
use crate::state::SharedState;

pub async fn handle_archive_dm(
    _state: SharedState,
    _peer: String,
    _author: String,
) -> Response {
    Response::error("not yet implemented (A.3 pending)")
}

pub async fn handle_unarchive_dm(
    _state: SharedState,
    _peer: String,
    _author: String,
) -> Response {
    Response::error("not yet implemented (A.3 pending)")
}

pub async fn handle_list_archived_dms(_state: SharedState, _author: String) -> Response {
    Response::error("not yet implemented (A.3 pending)")
}
