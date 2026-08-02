use std::path::Path;

use gitim_core::skill::{
    parse_skill_reference_or_shorthand, EventId, ProposalId, ProposalStatus, RevisionId,
    SkillProposal, SkillReference, SkillSlug,
};
use gitim_core::types::Handler;
use serde_json::json;

use crate::api::{Event, Request, Response};
use crate::skill_store::{SkillMutation, SkillStore, SkillStoreError};
use crate::state::SharedState;

pub async fn handle_skill_request(request: Request, state: SharedState) -> Response {
    let current_user = state.current_user.read().await.clone();
    let response = match request {
        Request::SkillList {
            archived,
            limit,
            after,
        } => SkillStore::new(state.as_ref())
            .catalog(archived, limit.unwrap_or(50), after.as_deref())
            .map(Response::json),
        Request::SkillShow { slug } => parse_slug(&slug).and_then(|slug| {
            SkillStore::new(state.as_ref())
                .skill_state(&slug)
                .map(|skill| Response::json(skill_detail(&skill)))
        }),
        Request::SkillLoad { reference } => parse_reference(&reference).and_then(|reference| {
            SkillStore::new(state.as_ref())
                .load(&reference)
                .map(Response::json)
        }),
        Request::SkillResource { reference, path } => {
            parse_reference(&reference).and_then(|reference| {
                SkillStore::new(state.as_ref())
                    .resource(&reference, &path)
                    .map(Response::json)
            })
        }
        Request::SkillRevisions { slug, limit, after } => parse_slug(&slug).and_then(|slug| {
            let store = SkillStore::new(state.as_ref());
            let skill = store.skill_state(&slug)?;
            let mut revisions = store.revisions(&slug)?;
            revisions.retain(|revision| skill.published_revisions.contains(&revision.id));
            if let Some(after) = after {
                let cursor = RevisionId::new(&after).map_err(SkillStoreError::from)?;
                revisions.retain(|revision| revision.id < cursor);
            }
            let limit = limit.unwrap_or(50).clamp(1, 100);
            let has_more = revisions.len() > limit;
            revisions.truncate(limit);
            let next_after = has_more
                .then(|| revisions.last().map(|revision| revision.id.to_string()))
                .flatten();
            Ok(Response::json(json!({
                "revisions": revisions,
                "next_after": next_after,
            })))
        }),
        Request::SkillHistory { slug, limit, after } => parse_slug(&slug).and_then(|slug| {
            let mut history = SkillStore::new(state.as_ref()).skill_state(&slug)?.history;
            if let Some(after) = after {
                let cursor = EventId::new(&after).map_err(SkillStoreError::from)?;
                history.retain(|entry| entry.event.id > cursor);
            }
            let limit = limit.unwrap_or(50).clamp(1, 100);
            let has_more = history.len() > limit;
            history.truncate(limit);
            let next_after = has_more
                .then(|| history.last().map(|entry| entry.event.id.to_string()))
                .flatten();
            Ok(Response::json(json!({
                "events": history,
                "next_after": next_after,
            })))
        }),
        Request::SkillValidate {
            slug,
            source_directory,
        } => parse_slug(&slug).and_then(|slug| {
            SkillStore::new(state.as_ref())
                .validate_directory(&slug, Path::new(&source_directory))
                .map(|package| {
                    Response::json(json!({
                        "slug": slug,
                        "content_sha256": package.content_sha256,
                        "file_count": package.entries.len(),
                        "resources": package.resources,
                    }))
                })
        }),
        Request::SkillCreate {
            slug,
            source_directory,
            display_name,
            description,
            event_id,
        } => with_actor(current_user.as_deref()).and_then(|actor| {
            let slug = SkillSlug::new(&slug).map_err(SkillStoreError::from)?;
            let event_id = parse_event_id(event_id)?;
            SkillStore::new(state.as_ref())
                .create(
                    &actor,
                    &slug,
                    Path::new(&source_directory),
                    &display_name,
                    &description,
                    event_id,
                )
                .map(|mutation| mutation_response(&state, mutation))
        }),
        Request::SkillPropose {
            slug,
            source_directory,
            base_revision,
            summary,
            event_id,
        } => with_actor(current_user.as_deref()).and_then(|actor| {
            let slug = SkillSlug::new(&slug).map_err(SkillStoreError::from)?;
            let base = RevisionId::new(&base_revision).map_err(SkillStoreError::from)?;
            let event_id = parse_event_id(event_id)?;
            SkillStore::new(state.as_ref())
                .propose(
                    &actor,
                    &slug,
                    Path::new(&source_directory),
                    &base,
                    &summary,
                    event_id,
                )
                .map(|mutation| mutation_response(&state, mutation))
        }),
        Request::SkillProposalList {
            slug,
            status,
            limit,
            after,
        } => parse_slug(&slug).and_then(|slug| {
            let mut proposals = SkillStore::new(state.as_ref())
                .skill_state(&slug)?
                .proposals;
            if let Some(status) = status {
                let status = parse_status(&status)?;
                proposals.retain(|proposal| proposal.status == status);
            }
            proposals.sort_by(|left, right| right.id.cmp(&left.id));
            if let Some(after) = after {
                let cursor = ProposalId::new(&after).map_err(SkillStoreError::from)?;
                proposals.retain(|proposal| proposal.id < cursor);
            }
            let limit = limit.unwrap_or(50).clamp(1, 100);
            let has_more = proposals.len() > limit;
            proposals.truncate(limit);
            let next_after = has_more
                .then(|| proposals.last().map(|proposal| proposal.id.to_string()))
                .flatten();
            let proposals = proposals.iter().map(proposal_summary).collect::<Vec<_>>();
            Ok(Response::json(json!({
                "proposals": proposals,
                "next_after": next_after,
            })))
        }),
        Request::SkillProposalShow { slug, proposal } => parse_slug_proposal(&slug, &proposal)
            .and_then(|(slug, proposal)| {
                let store = SkillStore::new(state.as_ref());
                let skill = store.skill_state(&slug)?;
                let mut proposal = skill
                    .proposals
                    .iter()
                    .find(|candidate| candidate.id == proposal)
                    .cloned()
                    .ok_or(SkillStoreError::ProposalNotFound)?;
                let load = store.load_proposal(&slug, &proposal.id)?;
                let comments_truncated = proposal.comments.len() > 100;
                if comments_truncated {
                    proposal.comments = proposal.comments.split_off(proposal.comments.len() - 100);
                }
                Ok(Response::json(json!({
                    "proposal": proposal,
                    "comments_truncated": comments_truncated,
                    "revision": load.revision,
                    "skill_markdown": load.skill_markdown,
                    "resources": load.resources,
                    "archived": load.archived,
                })))
            }),
        Request::SkillProposalResource {
            slug,
            proposal,
            path,
        } => parse_slug_proposal(&slug, &proposal).and_then(|(slug, proposal)| {
            SkillStore::new(state.as_ref())
                .proposal_resource(&slug, &proposal, &path)
                .map(Response::json)
        }),
        Request::SkillProposalComment {
            slug,
            proposal,
            body,
            event_id,
        } => mutation_with_proposal(
            &state,
            current_user.as_deref(),
            &slug,
            &proposal,
            event_id,
            |store, actor, slug, proposal, id| store.comment(actor, slug, proposal, &body, id),
        ),
        Request::SkillProposalPublish {
            slug,
            proposal,
            event_id,
        } => mutation_with_proposal(
            &state,
            current_user.as_deref(),
            &slug,
            &proposal,
            event_id,
            |store, actor, slug, proposal, id| store.publish(actor, slug, proposal, id),
        ),
        Request::SkillProposalReject {
            slug,
            proposal,
            event_id,
        } => mutation_with_proposal(
            &state,
            current_user.as_deref(),
            &slug,
            &proposal,
            event_id,
            |store, actor, slug, proposal, id| store.reject(actor, slug, proposal, id),
        ),
        Request::SkillProposalWithdraw {
            slug,
            proposal,
            event_id,
        } => mutation_with_proposal(
            &state,
            current_user.as_deref(),
            &slug,
            &proposal,
            event_id,
            |store, actor, slug, proposal, id| store.withdraw(actor, slug, proposal, id),
        ),
        Request::SkillMetadataUpdate {
            slug,
            display_name,
            description,
            event_id,
        } => mutation_for_slug(
            &state,
            current_user.as_deref(),
            &slug,
            event_id,
            |store, actor, slug, id| {
                store.update_metadata(actor, slug, display_name, description, id)
            },
        ),
        Request::SkillOwnerAdd {
            slug,
            handler,
            event_id,
        } => mutation_with_role(
            &state,
            current_user.as_deref(),
            &slug,
            &handler,
            event_id,
            |store, actor, slug, target, id| store.owner_add(actor, slug, target, id),
        ),
        Request::SkillOwnerRemove {
            slug,
            handler,
            remove_maintainer,
            event_id,
        } => mutation_with_role(
            &state,
            current_user.as_deref(),
            &slug,
            &handler,
            event_id,
            |store, actor, slug, target, id| {
                store.owner_remove(actor, slug, target, remove_maintainer, id)
            },
        ),
        Request::SkillMaintainerAdd {
            slug,
            handler,
            event_id,
        } => mutation_with_role(
            &state,
            current_user.as_deref(),
            &slug,
            &handler,
            event_id,
            |store, actor, slug, target, id| store.maintainer_add(actor, slug, target, id),
        ),
        Request::SkillMaintainerRemove {
            slug,
            handler,
            event_id,
        } => mutation_with_role(
            &state,
            current_user.as_deref(),
            &slug,
            &handler,
            event_id,
            |store, actor, slug, target, id| store.maintainer_remove(actor, slug, target, id),
        ),
        Request::SkillArchive { slug, event_id } => mutation_for_slug(
            &state,
            current_user.as_deref(),
            &slug,
            event_id,
            |store, actor, slug, id| store.archive(actor, slug, id),
        ),
        Request::SkillUnarchive { slug, event_id } => mutation_for_slug(
            &state,
            current_user.as_deref(),
            &slug,
            event_id,
            |store, actor, slug, id| store.unarchive(actor, slug, id),
        ),
        _ => Ok(Response::error_with_code(
            "invalid Skill request",
            "skill_invalid_request",
        )),
    };

    match response {
        Ok(response) => response,
        Err(error) => store_error_response(error),
    }
}

fn parse_slug(value: &str) -> Result<SkillSlug, SkillStoreError> {
    SkillSlug::new(value).map_err(Into::into)
}

fn parse_reference(value: &str) -> Result<SkillReference, SkillStoreError> {
    parse_skill_reference_or_shorthand(value).map_err(Into::into)
}

fn parse_event_id(value: Option<String>) -> Result<Option<EventId>, SkillStoreError> {
    value
        .map(|value| EventId::new(&value).map_err(SkillStoreError::from))
        .transpose()
}

fn parse_slug_proposal(
    slug: &str,
    proposal: &str,
) -> Result<(SkillSlug, ProposalId), SkillStoreError> {
    Ok((
        SkillSlug::new(slug).map_err(SkillStoreError::from)?,
        ProposalId::new(proposal).map_err(SkillStoreError::from)?,
    ))
}

fn parse_status(value: &str) -> Result<ProposalStatus, SkillStoreError> {
    match value {
        "open" => Ok(ProposalStatus::Open),
        "published" => Ok(ProposalStatus::Published),
        "rejected" => Ok(ProposalStatus::Rejected),
        "withdrawn" => Ok(ProposalStatus::Withdrawn),
        _ => Err(SkillStoreError::InvalidInput),
    }
}

fn with_actor(current_user: Option<&str>) -> Result<Handler, SkillStoreError> {
    let actor = current_user.ok_or(SkillStoreError::RoleTargetInactive)?;
    Handler::new(actor)
        .map_err(|error| SkillStoreError::ReadFailed(format!("invalid daemon identity: {error}")))
}

fn mutation_for_slug<F>(
    state: &SharedState,
    current_user: Option<&str>,
    slug: &str,
    event_id: Option<String>,
    action: F,
) -> Result<Response, SkillStoreError>
where
    F: FnOnce(
        &SkillStore<'_>,
        &Handler,
        &SkillSlug,
        Option<EventId>,
    ) -> Result<SkillMutation, SkillStoreError>,
{
    let actor = with_actor(current_user)?;
    let slug = SkillSlug::new(slug).map_err(SkillStoreError::from)?;
    let event_id = parse_event_id(event_id)?;
    let store = SkillStore::new(state.as_ref());
    action(&store, &actor, &slug, event_id).map(|mutation| mutation_response(state, mutation))
}

fn mutation_with_proposal<F>(
    state: &SharedState,
    current_user: Option<&str>,
    slug: &str,
    proposal: &str,
    event_id: Option<String>,
    action: F,
) -> Result<Response, SkillStoreError>
where
    F: FnOnce(
        &SkillStore<'_>,
        &Handler,
        &SkillSlug,
        &ProposalId,
        Option<EventId>,
    ) -> Result<SkillMutation, SkillStoreError>,
{
    let actor = with_actor(current_user)?;
    let (slug, proposal) = parse_slug_proposal(slug, proposal)?;
    let event_id = parse_event_id(event_id)?;
    let store = SkillStore::new(state.as_ref());
    action(&store, &actor, &slug, &proposal, event_id)
        .map(|mutation| mutation_response(state, mutation))
}

fn mutation_with_role<F>(
    state: &SharedState,
    current_user: Option<&str>,
    slug: &str,
    handler: &str,
    event_id: Option<String>,
    action: F,
) -> Result<Response, SkillStoreError>
where
    F: FnOnce(
        &SkillStore<'_>,
        &Handler,
        &SkillSlug,
        Handler,
        Option<EventId>,
    ) -> Result<SkillMutation, SkillStoreError>,
{
    let target = Handler::new(handler).map_err(|_| SkillStoreError::InvalidInput)?;
    mutation_for_slug(
        state,
        current_user,
        slug,
        event_id,
        |store, actor, slug, event_id| action(store, actor, slug, target, event_id),
    )
}

fn mutation_response(state: &SharedState, mutation: SkillMutation) -> Response {
    if !mutation.idempotent {
        let _ = state.event_tx.send(Event::SkillChanged {
            slug: mutation.state.slug.to_string(),
            event_id: mutation.event_id.to_string(),
        });
    }
    Response::json(json!({
        "event_id": mutation.event_id,
        "revision": mutation.revision,
        "proposal": mutation.proposal,
        "canonical_ref": mutation.canonical_ref,
        "current_revision": mutation.state.current_revision,
        "archived": mutation.state.archived,
        "commit_id": mutation.commit_id,
        "idempotent": mutation.idempotent,
    }))
}

fn proposal_summary(proposal: &SkillProposal) -> serde_json::Value {
    json!({
        "id": proposal.id,
        "revision": proposal.revision,
        "base_revision": proposal.base_revision,
        "summary": proposal.summary,
        "status": proposal.status,
        "created_by": proposal.created_by,
        "created_at": proposal.created_at,
        "resolved_by": proposal.resolved_by,
        "resolved_at": proposal.resolved_at,
    })
}

fn skill_detail(state: &gitim_core::skill::SkillState) -> serde_json::Value {
    json!({
        "slug": state.slug,
        "display_name": state.display_name,
        "description": state.description,
        "created_by": state.created_by,
        "owners": state.owners,
        "maintainers": state.maintainers,
        "current_revision": state.current_revision,
        "open_proposal_count": state.proposals.iter().filter(|proposal| proposal.status == ProposalStatus::Open).count(),
        "archived": state.archived,
        "created_at": state.created_at,
        "updated_at": state.updated_at,
        "last_event_id": state.last_event_id,
    })
}

fn store_error_response(error: SkillStoreError) -> Response {
    let code = error.code();
    Response::error_with_code(error.to_string(), code)
}
