mod channel;
pub mod cron;
mod depart;
mod dm;
mod poll;
mod read;
mod search;
mod send;
pub(crate) mod serde;
mod user;

pub use channel::*;
pub use cron::*;
pub use depart::*;
pub use dm::*;
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

/// Reject any author write whose `archive/users/<author>.meta.yaml`
/// exists. Per archive-protocol Contract 2: once a handler is departed,
/// the actor identity is terminally retired — any subsequent attempt
/// to author a commit under that handle must fail closed.
///
/// Skip this guard on unarchive paths (we still need a way back) and
/// on read-only / system-internal entries (poll, status). Apply it on
/// every active-path mutation that takes an author.
///
/// **Best-effort, not strictly atomic with `commit_lock`.** This gate runs
/// *before* the lock is acquired, so a write that passes the gate at T0
/// can race a concurrent `archive_user` at T1 and still commit at T2
/// attributed to a now-departed author. The in-handler archive checks
/// (e.g. `handle_archive_user`'s archive-path stat under the lock) are
/// the second line of defense for the operations they protect. For other
/// writes, the eventual-consistency model accepts that one cycle of
/// writes may slip past archive transitions; sync_loop and Contract 2's
/// "departed actor can't author further commits" remain true within a
/// sync window.
pub(crate) fn ensure_author_not_departed(
    state: &SharedState,
    author: &str,
) -> Result<(), Response> {
    let archive_path = state
        .repo_root
        .join("archive/users")
        .join(format!("{}.meta.yaml", author));
    if archive_path.exists() {
        return Err(Response::error(format!("user @{} is departed", author)));
    }
    Ok(())
}

/// Board writes are owner-only: callers may omit `author`, or echo the
/// current daemon identity, but may not select another handler's board.
async fn resolve_board_author(
    author: Option<String>,
    state: &SharedState,
) -> Result<String, Response> {
    let current = {
        let current = state.current_user.read().await;
        match current.clone() {
            Some(user) => user,
            None => {
                return Err(Response::error(
                    "board write requires current user identity",
                ))
            }
        }
    };

    if let Some(requested) = author {
        if requested != current {
            return Err(Response::error(format!(
                "board author mismatch: current user is {}, requested {}",
                current, requested
            )));
        }
    }

    ensure_author_not_departed(state, &current)?;
    Ok(current)
}

/// Resolve a channel string to a filesystem path and a cache key.
/// Channels: "channels/{name}.thread", DMs: "dm:{h1},{h2}" -> "dm/{h1}--{h2}.thread"
pub(super) fn resolve_thread_path(
    state: &SharedState,
    channel: &str,
) -> Result<(std::path::PathBuf, String), Response> {
    use gitim_core::dm::dm_filename;
    use gitim_core::types::{ChannelName, Handler};

    if let Some(dm_rest) = channel.strip_prefix("dm:") {
        let parts: Vec<&str> = dm_rest.split(',').collect();
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
                | Request::UpdateUser { .. }
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
                | Request::ArchiveUser { .. }
                | Request::UnarchiveUser { .. }
                | Request::ArchiveDm { .. }
                | Request::UnarchiveDm { .. }
                | Request::DepartUser { .. }
                | Request::BoardInit { .. }
                | Request::BoardPublish { .. }
                | Request::BoardSet { .. }
                | Request::BoardSectionSet { .. }
                | Request::BoardSectionAppend { .. }
                | Request::FlowCreate { .. }
                | Request::FlowRemove { .. }
                | Request::FlowRunStart { .. }
                | Request::FlowNodeSet { .. }
                | Request::FlowRunCancel { .. }
                | Request::CreateCron { .. }
                | Request::EnableCron { .. }
                | Request::DisableCron { .. }
                | Request::DeleteCron { .. }
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
            // Wire-additive: attach the parsed gitim.epoch.yaml snapshot as
            // an `epoch` sibling on the StatusResponse data object so clients
            // can observe Active vs Redirected and read the migration target.
            // `StatusResponse` lives in gitim-core; layer the field into the
            // serialized JSON value here to keep the gitim-core wire contract
            // additive. Absent epoch file → no field, identical wire shape.
            // Matches `Response::json` error handling: log + structured error
            // instead of unwrap-panic.
            match serde_json::to_value(payload) {
                Ok(mut data) => {
                    if let Some(epoch_file) = state.epoch_status_snapshot() {
                        if let Some(obj) = data.as_object_mut() {
                            if let Ok(epoch_value) = serde_json::to_value(epoch_file) {
                                obj.insert("epoch".to_string(), epoch_value);
                            }
                        }
                    }
                    Response::success(data)
                }
                Err(e) => {
                    tracing::error!("serializing status response: {e}");
                    Response::error("internal serialization error")
                }
            }
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
        Request::ListUsers { include_archived } => handle_list_users(state, include_archived).await,
        Request::GetThread {
            channel,
            line_number,
        } => handle_get_thread(state, channel, line_number).await,
        Request::Subscribe => {
            let payload = gitim_core::responses::SubscribeResponse { subscribed: true };
            Response::json(payload)
        }
        Request::RegisterUser {
            handler,
            display_name,
            role,
            introduction,
        } => handle_register_user(state, handler, display_name, role, introduction).await,
        Request::UpdateUser {
            handler,
            introduction,
        } => handle_update_user(state, handler, introduction).await,
        Request::Poll { since } => handle_poll(state, since).await,
        Request::Stop => handle_stop(state).await,
        Request::Onboard {
            git_server,
            auth,
            admin,
            guest,
            join_general,
        } => {
            crate::onboard::handle_onboard(state, git_server, auth, admin, guest, join_general)
                .await
        }
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
        Request::ListArchivedChannels {
            prefix,
            offset,
            limit,
        } => handle_list_archived_channels(state, prefix, offset, limit).await,
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
        Request::ArchiveUser { handler, author } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            handle_archive_user(state, handler, resolved_author).await
        }
        Request::UnarchiveUser { handler, author } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            handle_unarchive_user(state, handler, resolved_author).await
        }
        Request::ArchiveDm { peer, author } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            handle_archive_dm(state, peer, resolved_author).await
        }
        Request::UnarchiveDm { peer, author } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            handle_unarchive_dm(state, peer, resolved_author).await
        }
        Request::ListArchivedUsers => handle_list_archived_users(state).await,
        Request::ListArchivedDms {
            author,
            prefix,
            offset,
            limit,
        } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            handle_list_archived_dms(state, resolved_author, prefix, offset, limit).await
        }
        Request::DepartUser { handler } => handle_depart_user(state, handler).await,
        Request::BoardShow { handler } => {
            crate::board_handlers::handle_board_show(state, handler).await
        }
        Request::BoardList => crate::board_handlers::handle_board_list(state).await,
        Request::BoardInit { author } => {
            let resolved_author = match resolve_board_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            crate::board_handlers::handle_board_init(state, resolved_author).await
        }
        Request::BoardPublish { content, author } => {
            let resolved_author = match resolve_board_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            crate::board_handlers::handle_board_publish(state, resolved_author, content).await
        }
        Request::BoardSet {
            field,
            value,
            author,
        } => {
            let resolved_author = match resolve_board_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            crate::board_handlers::handle_board_set(state, resolved_author, field, value).await
        }
        Request::BoardSectionSet {
            section,
            value,
            author,
        } => {
            let resolved_author = match resolve_board_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            crate::board_handlers::handle_board_section_set(state, resolved_author, section, value)
                .await
        }
        Request::BoardSectionAppend {
            section,
            value,
            author,
        } => {
            let resolved_author = match resolve_board_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            crate::board_handlers::handle_board_section_append(
                state,
                resolved_author,
                section,
                value,
            )
            .await
        }
        Request::FlowList => crate::flow_handlers::handle_flow_list(state).await,
        Request::FlowShow { slug } => crate::flow_handlers::handle_flow_show(state, slug).await,
        Request::FlowCreate {
            slug,
            name,
            description,
            author,
        } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            crate::flow_handlers::handle_flow_create(
                state,
                slug,
                name,
                description,
                resolved_author,
            )
            .await
        }
        Request::FlowRemove { slug, author } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            crate::flow_handlers::handle_flow_remove(state, slug, resolved_author).await
        }
        Request::FlowValidate { slug } => {
            crate::flow_handlers::handle_flow_validate(state, slug).await
        }
        Request::FlowRunStart {
            slug,
            channel,
            author,
        } => {
            let resolved = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            crate::flow_run_handlers::handle_flow_run_start(state, slug, channel, resolved).await
        }
        Request::FlowRunList {
            slug,
            channel,
            status,
        } => crate::flow_run_handlers::handle_flow_run_list(state, slug, channel, status).await,
        Request::FlowRunShow { run_id } => {
            crate::flow_run_handlers::handle_flow_run_show(state, run_id).await
        }
        Request::FlowNodeSet {
            run_id,
            node_id,
            status,
            actor,
            result_ref,
            author,
        } => {
            let resolved = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            crate::flow_run_handlers::handle_flow_node_set(
                state, run_id, node_id, status, actor, result_ref, resolved,
            )
            .await
        }
        Request::FlowRunCancel { run_id, author } => {
            let resolved = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            crate::flow_run_handlers::handle_flow_run_cancel(state, run_id, resolved).await
        }
        Request::CreateCron {
            name,
            schedule,
            timezone,
            target,
            prompt,
            author,
        } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            handle_create_cron(
                state,
                name,
                schedule,
                timezone,
                target,
                prompt,
                resolved_author,
            )
            .await
        }
        Request::ListCrons => handle_list_crons(state).await,
        Request::ShowCron { name } => handle_show_cron(state, name).await,
        Request::HistoryCron { name, limit } => handle_history_cron(state, name, limit).await,
        Request::EnableCron { name, author } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            handle_enable_cron(state, name, resolved_author).await
        }
        Request::DisableCron { name, author } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            handle_disable_cron(state, name, resolved_author).await
        }
        Request::DeleteCron { name, author } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            handle_delete_cron(state, name, resolved_author).await
        }
    }
}
