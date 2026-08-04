use std::collections::{BTreeMap, BTreeSet};

use crate::types::Handler;

use super::{
    validate_comment, validate_description, validate_display_name, validate_summary,
    ProposalStatus, ReducedEvent, RevisionId, SkillComment, SkillError, SkillEvent, SkillEventKind,
    SkillProposal, SkillRevisionMeta, SkillSlug, SkillState, SKILL_SCHEMA_VERSION,
};

pub fn reduce_skill(
    slug: &SkillSlug,
    revisions: &[SkillRevisionMeta],
    mut events: Vec<SkillEvent>,
) -> Result<SkillState, SkillError> {
    let revision_map = revision_map(slug, revisions)?;
    validate_and_sort_events(slug, &mut events)?;

    let mut state = None;
    let mut history = Vec::with_capacity(events.len());
    for event in events {
        let result = match &mut state {
            Some(existing) => apply_existing_event(existing, &revision_map, &event),
            None => create_initial_state(slug, &revision_map, &event).map(|created| {
                state = Some(created);
            }),
        };
        let (effective, reason) = match result {
            Ok(()) => (true, None),
            Err(reason) => (false, Some(reason.to_string())),
        };
        history.push(ReducedEvent {
            event,
            effective,
            reason,
        });
    }

    let mut state = state.ok_or(SkillError::InvalidHistory)?;
    let last_event = history.last().ok_or(SkillError::InvalidHistory)?;
    state.last_event_id = last_event.event.id.clone();
    state.history = history;
    Ok(state)
}

fn revision_map<'a>(
    slug: &SkillSlug,
    revisions: &'a [SkillRevisionMeta],
) -> Result<BTreeMap<&'a RevisionId, &'a SkillRevisionMeta>, SkillError> {
    let mut result = BTreeMap::new();
    for revision in revisions {
        if revision.schema_version != SKILL_SCHEMA_VERSION || revision.skill != *slug {
            return Err(SkillError::InvalidHistory);
        }
        if result.insert(&revision.id, revision).is_some() {
            return Err(SkillError::InvalidHistory);
        }
    }
    Ok(result)
}

fn validate_and_sort_events(slug: &SkillSlug, events: &mut [SkillEvent]) -> Result<(), SkillError> {
    events.sort_by(|left, right| left.id.cmp(&right.id));
    let mut ids = BTreeSet::new();
    for event in events {
        if event.schema_version != SKILL_SCHEMA_VERSION
            || event.skill != *slug
            || !ids.insert(event.id.clone())
        {
            return Err(SkillError::InvalidHistory);
        }
    }
    Ok(())
}

fn create_initial_state(
    slug: &SkillSlug,
    revisions: &BTreeMap<&RevisionId, &SkillRevisionMeta>,
    event: &SkillEvent,
) -> Result<SkillState, &'static str> {
    let SkillEventKind::Created {
        display_name,
        description,
        revision,
    } = &event.kind
    else {
        return Err("skill_not_created");
    };
    if !validate_display_name(display_name) || !validate_description(description) {
        return Err("invalid_metadata");
    }
    let Some(meta) = revisions.get(revision) else {
        return Err("initial_revision_missing");
    };
    if meta.base_revision.is_some() || meta.created_by != event.actor {
        return Err("invalid_initial_revision");
    }

    Ok(SkillState {
        slug: slug.clone(),
        display_name: display_name.clone(),
        description: description.clone(),
        created_by: event.actor.clone(),
        owners: vec![event.actor.clone()],
        maintainers: vec![event.actor.clone()],
        current_revision: revision.clone(),
        published_revisions: vec![revision.clone()],
        proposals: Vec::new(),
        archived: false,
        created_at: event.created_at.clone(),
        updated_at: event.created_at.clone(),
        last_event_id: event.id.clone(),
        history: Vec::new(),
    })
}

fn apply_existing_event(
    state: &mut SkillState,
    revisions: &BTreeMap<&RevisionId, &SkillRevisionMeta>,
    event: &SkillEvent,
) -> Result<(), &'static str> {
    let result = match &event.kind {
        SkillEventKind::Created { .. } => Err("already_created"),
        SkillEventKind::ProposalOpened {
            proposal,
            revision,
            base_revision,
            summary,
        } => open_proposal(
            state,
            revisions,
            event,
            proposal,
            revision,
            base_revision,
            summary,
        ),
        SkillEventKind::ProposalCommented { proposal, body } => {
            comment_on_proposal(state, event, proposal, body)
        }
        SkillEventKind::ProposalPublished {
            proposal,
            expected_current_revision,
        } => publish_proposal(state, event, proposal, expected_current_revision),
        SkillEventKind::ProposalRejected { proposal } => {
            resolve_proposal(state, event, proposal, ProposalStatus::Rejected)
        }
        SkillEventKind::ProposalWithdrawn { proposal } => withdraw_proposal(state, event, proposal),
        SkillEventKind::MetadataUpdated {
            display_name,
            description,
        } => update_metadata(
            state,
            event,
            display_name.as_deref(),
            description.as_deref(),
        ),
        SkillEventKind::OwnerAdded { handler } => add_owner(state, event, handler),
        SkillEventKind::OwnerRemoved {
            handler,
            remove_maintainer,
        } => remove_owner(state, event, handler, *remove_maintainer),
        SkillEventKind::MaintainerAdded { handler } => add_maintainer(state, event, handler),
        SkillEventKind::MaintainerRemoved { handler } => remove_maintainer(state, event, handler),
        SkillEventKind::Archived => archive(state, event),
        SkillEventKind::Unarchived => unarchive(state, event),
    };
    if result.is_ok() {
        state.updated_at.clone_from(&event.created_at);
    }
    result
}

fn open_proposal(
    state: &mut SkillState,
    revisions: &BTreeMap<&RevisionId, &SkillRevisionMeta>,
    event: &SkillEvent,
    proposal: &super::ProposalId,
    revision: &RevisionId,
    base_revision: &RevisionId,
    summary: &str,
) -> Result<(), &'static str> {
    ensure_active(state)?;
    if !validate_summary(summary) {
        return Err("invalid_summary");
    }
    if state.proposals.iter().any(|item| item.id == *proposal) {
        return Err("proposal_exists");
    }
    if !state.published_revisions.contains(base_revision) {
        return Err("base_revision_unpublished");
    }
    let Some(meta) = revisions.get(revision) else {
        return Err("candidate_revision_missing");
    };
    if meta.base_revision.as_ref() != Some(base_revision) || meta.created_by != event.actor {
        return Err("invalid_candidate_revision");
    }
    if state.published_revisions.contains(revision) {
        return Err("candidate_already_published");
    }
    state.proposals.push(SkillProposal {
        id: proposal.clone(),
        revision: revision.clone(),
        base_revision: base_revision.clone(),
        summary: summary.to_string(),
        status: ProposalStatus::Open,
        created_by: event.actor.clone(),
        created_at: event.created_at.clone(),
        comments: Vec::new(),
        resolved_by: None,
        resolved_at: None,
    });
    Ok(())
}

fn comment_on_proposal(
    state: &mut SkillState,
    event: &SkillEvent,
    proposal: &super::ProposalId,
    body: &str,
) -> Result<(), &'static str> {
    ensure_active(state)?;
    if !validate_comment(body) {
        return Err("invalid_comment");
    }
    let item = proposal_mut(state, proposal)?;
    ensure_proposal_open(item)?;
    item.comments.push(SkillComment {
        event_id: event.id.clone(),
        actor: event.actor.clone(),
        body: body.to_string(),
        created_at: event.created_at.clone(),
    });
    Ok(())
}

fn publish_proposal(
    state: &mut SkillState,
    event: &SkillEvent,
    proposal: &super::ProposalId,
    expected_current_revision: &RevisionId,
) -> Result<(), &'static str> {
    ensure_active(state)?;
    ensure_maintainer(state, &event.actor)?;
    let index = proposal_index(state, proposal)?;
    let item = &state.proposals[index];
    ensure_proposal_open(item)?;
    if state.current_revision != *expected_current_revision
        || item.base_revision != state.current_revision
    {
        return Err("stale_current_revision");
    }
    let revision = item.revision.clone();
    let item = &mut state.proposals[index];
    item.status = ProposalStatus::Published;
    item.resolved_by = Some(event.actor.clone());
    item.resolved_at = Some(event.created_at.clone());
    state.current_revision = revision.clone();
    insert_revision(&mut state.published_revisions, revision);
    Ok(())
}

fn resolve_proposal(
    state: &mut SkillState,
    event: &SkillEvent,
    proposal: &super::ProposalId,
    status: ProposalStatus,
) -> Result<(), &'static str> {
    ensure_active(state)?;
    ensure_maintainer(state, &event.actor)?;
    let item = proposal_mut(state, proposal)?;
    ensure_proposal_open(item)?;
    item.status = status;
    item.resolved_by = Some(event.actor.clone());
    item.resolved_at = Some(event.created_at.clone());
    Ok(())
}

fn withdraw_proposal(
    state: &mut SkillState,
    event: &SkillEvent,
    proposal: &super::ProposalId,
) -> Result<(), &'static str> {
    ensure_active(state)?;
    let item = proposal_mut(state, proposal)?;
    ensure_proposal_open(item)?;
    if item.created_by != event.actor {
        return Err("not_proposal_author");
    }
    item.status = ProposalStatus::Withdrawn;
    item.resolved_by = Some(event.actor.clone());
    item.resolved_at = Some(event.created_at.clone());
    Ok(())
}

fn update_metadata(
    state: &mut SkillState,
    event: &SkillEvent,
    display_name: Option<&str>,
    description: Option<&str>,
) -> Result<(), &'static str> {
    ensure_active(state)?;
    ensure_maintainer(state, &event.actor)?;
    if display_name.is_none() && description.is_none()
        || display_name.is_some_and(|value| !validate_display_name(value))
        || description.is_some_and(|value| !validate_description(value))
    {
        return Err("invalid_metadata");
    }
    if let Some(value) = display_name {
        state.display_name = value.to_string();
    }
    if let Some(value) = description {
        state.description = value.to_string();
    }
    Ok(())
}

fn add_owner(
    state: &mut SkillState,
    event: &SkillEvent,
    handler: &Handler,
) -> Result<(), &'static str> {
    ensure_owner(state, &event.actor)?;
    if contains_handler(&state.owners, handler) {
        return Err("role_unchanged");
    }
    insert_handler(&mut state.owners, handler.clone());
    insert_handler(&mut state.maintainers, handler.clone());
    Ok(())
}

fn remove_owner(
    state: &mut SkillState,
    event: &SkillEvent,
    handler: &Handler,
    remove_maintainer_role: bool,
) -> Result<(), &'static str> {
    ensure_owner(state, &event.actor)?;
    if !contains_handler(&state.owners, handler) {
        return Err("role_unchanged");
    }
    if state.owners.len() == 1 {
        return Err("last_owner");
    }
    remove_handler(&mut state.owners, handler);
    if remove_maintainer_role {
        remove_handler(&mut state.maintainers, handler);
    }
    Ok(())
}

fn add_maintainer(
    state: &mut SkillState,
    event: &SkillEvent,
    handler: &Handler,
) -> Result<(), &'static str> {
    ensure_owner(state, &event.actor)?;
    if contains_handler(&state.maintainers, handler) {
        return Err("role_unchanged");
    }
    insert_handler(&mut state.maintainers, handler.clone());
    Ok(())
}

fn remove_maintainer(
    state: &mut SkillState,
    event: &SkillEvent,
    handler: &Handler,
) -> Result<(), &'static str> {
    ensure_owner(state, &event.actor)?;
    if contains_handler(&state.owners, handler) {
        return Err("owner_is_maintainer");
    }
    if !contains_handler(&state.maintainers, handler) {
        return Err("role_unchanged");
    }
    remove_handler(&mut state.maintainers, handler);
    Ok(())
}

fn archive(state: &mut SkillState, event: &SkillEvent) -> Result<(), &'static str> {
    ensure_owner(state, &event.actor)?;
    if state.archived {
        return Err("already_archived");
    }
    state.archived = true;
    Ok(())
}

fn unarchive(state: &mut SkillState, event: &SkillEvent) -> Result<(), &'static str> {
    ensure_owner(state, &event.actor)?;
    if !state.archived {
        return Err("not_archived");
    }
    state.archived = false;
    Ok(())
}

fn ensure_active(state: &SkillState) -> Result<(), &'static str> {
    if state.archived {
        Err("skill_archived")
    } else {
        Ok(())
    }
}

fn ensure_owner(state: &SkillState, actor: &Handler) -> Result<(), &'static str> {
    if contains_handler(&state.owners, actor) {
        Ok(())
    } else {
        Err("not_owner")
    }
}

fn ensure_maintainer(state: &SkillState, actor: &Handler) -> Result<(), &'static str> {
    if contains_handler(&state.maintainers, actor) {
        Ok(())
    } else {
        Err("not_maintainer")
    }
}

fn proposal_index(state: &SkillState, proposal: &super::ProposalId) -> Result<usize, &'static str> {
    state
        .proposals
        .iter()
        .position(|item| item.id == *proposal)
        .ok_or("proposal_not_found")
}

fn proposal_mut<'a>(
    state: &'a mut SkillState,
    proposal: &super::ProposalId,
) -> Result<&'a mut SkillProposal, &'static str> {
    state
        .proposals
        .iter_mut()
        .find(|item| item.id == *proposal)
        .ok_or("proposal_not_found")
}

fn ensure_proposal_open(proposal: &SkillProposal) -> Result<(), &'static str> {
    if proposal.status == ProposalStatus::Open {
        Ok(())
    } else {
        Err("proposal_terminal")
    }
}

fn contains_handler(handlers: &[Handler], target: &Handler) -> bool {
    handlers.iter().any(|handler| handler == target)
}

fn insert_handler(handlers: &mut Vec<Handler>, handler: Handler) {
    if !contains_handler(handlers, &handler) {
        handlers.push(handler);
        handlers.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    }
}

fn remove_handler(handlers: &mut Vec<Handler>, target: &Handler) {
    handlers.retain(|handler| handler != target);
}

fn insert_revision(revisions: &mut Vec<RevisionId>, revision: RevisionId) {
    if !revisions.contains(&revision) {
        revisions.push(revision);
        revisions.sort();
    }
}
