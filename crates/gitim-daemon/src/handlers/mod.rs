mod channel;
mod poll;
mod read;
mod search;
mod send;
pub(crate) mod serde;
mod user;

pub use channel::*;
pub use poll::*;
pub use read::*;
pub use search::*;
pub use send::*;
pub use user::*;

pub(crate) use serde::entry_to_json;

use crate::api::{Request, Response};
use crate::state::SharedState;

/// Resolve author from explicit param or daemon identity.
pub(super) async fn resolve_author(
    author: Option<String>,
    state: &SharedState,
) -> Result<String, Response> {
    match author {
        Some(a) if !a.is_empty() => Ok(a),
        _ => {
            let current = state.current_user.read().await;
            match current.clone() {
                Some(u) => Ok(u),
                None => Err(Response::error(
                    "no author specified and no identity configured",
                )),
            }
        }
    }
}

/// Resolve a channel string to a filesystem path and a cache key.
/// Channels: "channels/{name}.thread", DMs: "dm:{h1},{h2}" -> "dm/{h1}--{h2}.thread"
pub(super) fn resolve_thread_path(
    state: &SharedState,
    channel: &str,
) -> Result<(std::path::PathBuf, String), Response> {
    use gitim_core::dm::dm_filename;
    use gitim_core::types::{ChannelName, Handler};

    if channel.starts_with("dm:") {
        let parts: Vec<&str> = channel[3..].split(',').collect();
        if parts.len() != 2 {
            return Err(Response::error("DM format must be dm:handler1,handler2"));
        }
        let h1 = Handler::new(parts[0])
            .map_err(|e| Response::error(format!("invalid DM handler: {}", e)))?;
        let h2 = Handler::new(parts[1])
            .map_err(|e| Response::error(format!("invalid DM handler: {}", e)))?;
        let name = dm_filename(&h1, &h2);
        let path = state.repo_root.join("dm").join(format!("{}.thread", name));
        Ok((path, name))
    } else {
        let name = ChannelName::new(channel)
            .map_err(|e| Response::error(format!("invalid channel name: {}", e)))?;
        let path = state
            .repo_root
            .join("channels")
            .join(format!("{}.thread", name));
        Ok((path, name.to_string()))
    }
}

pub async fn handle_request(req: Request, state: SharedState) -> Response {
    // Guest mode guard: reject all write operations
    if state.is_guest.load(std::sync::atomic::Ordering::SeqCst) {
        let is_write = matches!(
            req,
            Request::Send { .. }
                | Request::RegisterUser { .. }
                | Request::JoinChannel { .. }
                | Request::LeaveChannel { .. }
                | Request::CreateChannel { .. }
                | Request::ArchiveChannel { .. }
                | Request::UnarchiveChannel { .. }
                | Request::CreateCard { .. }
                | Request::SendCardMessage { .. }
                | Request::UpdateCard { .. }
                | Request::ArchiveCard { .. }
                | Request::UnarchiveCard { .. }
        );
        if is_write {
            return Response::error("guest mode: write operations are not allowed");
        }
    }

    match req {
        Request::Status => {
            let is_guest = state.is_guest.load(std::sync::atomic::Ordering::SeqCst);
            let payload = gitim_core::responses::StatusResponse {
                version: "0.1.0".to_string(),
                status: "running".to_string(),
                guest: is_guest,
            };
            Response::success(serde_json::to_value(payload).unwrap())
        }
        Request::Send {
            channel,
            body,
            reply_to,
            author,
        } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            handle_send(state, channel, body, reply_to, resolved_author).await
        }
        Request::Read {
            channel,
            limit,
            since,
        } => handle_read(state, channel, limit, since).await,
        Request::ListChannels => handle_list_channels(state).await,
        Request::ListUsers => handle_list_users(state).await,
        Request::GetThread {
            channel,
            line_number,
        } => handle_get_thread(state, channel, line_number).await,
        Request::Subscribe => {
            let payload = gitim_core::responses::SubscribeResponse { subscribed: true };
            Response::success(serde_json::to_value(payload).unwrap())
        }
        Request::RegisterUser {
            handler,
            display_name,
            role,
            introduction,
        } => handle_register_user(state, handler, display_name, role, introduction).await,
        Request::Poll { since } => handle_poll(state, since).await,
        Request::Stop => handle_stop(state).await,
        Request::Onboard {
            git_server,
            auth,
            admin,
            guest,
        } => crate::onboard::handle_onboard(state, git_server, auth, admin, guest).await,
        Request::JoinChannel {
            channel,
            targets,
            author,
        } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            handle_join_channel(state, channel, targets, resolved_author).await
        }
        Request::LeaveChannel {
            channel,
            targets,
            author,
        } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            handle_leave_channel(state, channel, targets, resolved_author).await
        }
        Request::CreateChannel {
            name,
            display_name,
            introduction,
            author,
            invitees,
        } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            handle_create_channel(
                state,
                name,
                display_name,
                introduction,
                resolved_author,
                invitees,
            )
            .await
        }
        Request::Search {
            query,
            author,
            channel,
            channel_type,
            limit,
            offset,
            include_cards,
        } => {
            handle_search(
                state,
                query,
                author,
                channel,
                channel_type,
                limit,
                offset,
                include_cards,
            )
            .await
        }
        Request::Reindex => handle_reindex(state).await,
        Request::ArchiveChannel { channel, author } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            handle_archive_channel(state, channel, resolved_author).await
        }
        Request::UnarchiveChannel { channel, author } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            handle_unarchive_channel(state, channel, resolved_author).await
        }
        Request::ListArchivedChannels => handle_list_archived_channels(state).await,
        Request::CreateCard {
            channel,
            title,
            labels,
            assignee,
            status,
            author,
        } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            crate::card_handlers::handle_create_card(
                state,
                channel,
                title,
                labels,
                assignee,
                status,
                resolved_author,
            )
            .await
        }
        Request::ListCards {
            channel,
            labels,
            status,
            assignee,
        } => {
            crate::card_handlers::handle_list_cards(state, channel, labels, status, assignee).await
        }
        Request::ReadCard {
            channel,
            card_id,
            limit,
            since,
        } => crate::card_handlers::handle_read_card(state, channel, card_id, limit, since).await,
        Request::SendCardMessage {
            channel,
            card_id,
            body,
            reply_to,
            author,
        } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            crate::card_handlers::handle_send_card_message(
                state,
                channel,
                card_id,
                body,
                reply_to,
                resolved_author,
            )
            .await
        }
        Request::UpdateCard {
            channel,
            card_id,
            status,
            labels,
            assignee,
            author,
        } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            crate::card_handlers::handle_update_card(
                state,
                channel,
                card_id,
                status,
                labels,
                assignee,
                resolved_author,
            )
            .await
        }
        Request::ArchiveCard {
            channel,
            card_id,
            author,
        } => crate::card_handlers::handle_archive_card(state, channel, card_id, author).await,
        Request::UnarchiveCard {
            channel,
            card_id,
            author,
        } => crate::card_handlers::handle_unarchive_card(state, channel, card_id, author).await,
        Request::ListArchivedCards { channel } => {
            crate::card_handlers::handle_list_archived_cards(state, channel).await
        }
    }
}
