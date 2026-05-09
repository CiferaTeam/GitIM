use crate::api::Response;
use crate::state::SharedState;
use gitim_core::types::{Handler, UserMeta};

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

pub async fn handle_archive_user(
    _state: SharedState,
    _handler: String,
    _author: String,
) -> Response {
    Response::error("not yet implemented (A.2 pending)")
}

pub async fn handle_unarchive_user(
    _state: SharedState,
    _handler: String,
    _author: String,
) -> Response {
    Response::error("not yet implemented (A.2 pending)")
}

pub async fn handle_list_archived_users(_state: SharedState) -> Response {
    Response::error("not yet implemented (A.2 pending)")
}

pub async fn handle_depart_user(_state: SharedState, _handler: String) -> Response {
    Response::error("not yet implemented (A.4 pending)")
}
