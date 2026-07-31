use gitim_core::skill::{
    validate_package_entries, ProposalId, RequestId, SkillArchiveTransitionRequest,
    SkillCreateRequest, SkillError, SkillListQuery, SkillMetadataUpdateRequest,
    SkillMutationRequest, SkillMutationResult, SkillOperation, SkillPageQuery,
    SkillProposalListQuery, SkillProposalResourceQuery, SkillProposalShowQuery,
    SkillProposalTransitionRequest, SkillProposeRequest, SkillReference, SkillRepairRequest,
    SkillResourceQuery, SkillRoleUpdateRequest, SkillShowQuery, SkillSlug,
    SkillWorkspaceBootstrapRequest, ValidatedPackage,
};
use gitim_sync::skill::checkpoint::SkillSyncError;
use gitim_sync::skill::guard::SkillSyncGuard;
use gitim_sync::skill::transaction::{
    execute_remote_skill_transaction, RemoteSkillTransactionRequest, SkillLocalState,
};
use serde::Serialize;
use std::collections::BTreeSet;
use std::fs;

use crate::api::Response;
use crate::handlers::ensure_author_not_departed;
use crate::skill_import::snapshot_skill_directory;
use crate::state::SharedState;

#[derive(Serialize)]
struct SkillMutationResponse {
    request_id: RequestId,
    #[serde(skip_serializing_if = "Option::is_none")]
    proposal_id: Option<ProposalId>,
    commit_id: String,
    result: SkillMutationResult,
    local_state: SkillLocalState,
}

pub async fn handle_skill_list(state: SharedState, query: SkillListQuery) -> Response {
    if let Err(response) = resolve_skill_actor(&state).await {
        return response;
    }
    skill_result(state.skill_store.list(query))
}

pub async fn handle_skill_show(state: SharedState, query: SkillShowQuery) -> Response {
    if let Err(response) = resolve_skill_actor(&state).await {
        return response;
    }
    skill_result(state.skill_store.show(query))
}

pub async fn handle_skill_load(state: SharedState, reference: SkillReference) -> Response {
    if let Err(response) = resolve_skill_actor(&state).await {
        return response;
    }
    skill_result(state.skill_store.load(&reference))
}

pub async fn handle_skill_resource(state: SharedState, query: SkillResourceQuery) -> Response {
    if let Err(response) = resolve_skill_actor(&state).await {
        return response;
    }
    skill_result(state.skill_store.resource(query))
}

pub async fn handle_skill_revisions(state: SharedState, query: SkillPageQuery) -> Response {
    if let Err(response) = resolve_skill_actor(&state).await {
        return response;
    }
    skill_result(state.skill_store.revisions(query))
}

pub async fn handle_skill_history(state: SharedState, query: SkillPageQuery) -> Response {
    if let Err(response) = resolve_skill_actor(&state).await {
        return response;
    }
    skill_result(state.skill_store.history(query))
}

pub async fn handle_skill_proposal_list(
    state: SharedState,
    query: SkillProposalListQuery,
) -> Response {
    if let Err(response) = resolve_skill_actor(&state).await {
        return response;
    }
    skill_result(state.skill_store.proposal_list(query))
}

pub async fn handle_skill_proposal_show(
    state: SharedState,
    query: SkillProposalShowQuery,
) -> Response {
    if let Err(response) = resolve_skill_actor(&state).await {
        return response;
    }
    skill_result(state.skill_store.proposal_show(query))
}

pub async fn handle_skill_proposal_resource(
    state: SharedState,
    query: SkillProposalResourceQuery,
) -> Response {
    if let Err(response) = resolve_skill_actor(&state).await {
        return response;
    }
    skill_result(state.skill_store.proposal_resource(query))
}

pub async fn handle_skill_workspace_meta(state: SharedState) -> Response {
    if let Err(response) = resolve_skill_actor(&state).await {
        return response;
    }
    skill_result(state.skill_store.workspace_meta())
}

pub async fn handle_skill_workspace_bootstrap(
    state: SharedState,
    request: SkillWorkspaceBootstrapRequest,
) -> Response {
    let actor = match resolve_skill_actor(&state).await {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    run_mutation(
        state,
        actor,
        SkillMutationRequest::WorkspaceBootstrap(request),
        None,
        None,
        "workspace_bootstrapped",
    )
    .await
}

pub async fn handle_skill_create(state: SharedState, request: SkillCreateRequest) -> Response {
    let actor = match resolve_skill_actor(&state).await {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let package = match import_package(
        &state,
        request.request_id.as_str(),
        &request.source_directory,
    ) {
        Ok(package) => package,
        Err(error) => return skill_error(error),
    };
    let package = match validate_package_entries(&request.slug, package.entries) {
        Ok(package) => package,
        Err(error) => return skill_error(error),
    };
    let slug = request.slug.clone();
    run_mutation(
        state,
        actor,
        SkillMutationRequest::Create(request),
        Some(package),
        Some(slug),
        "skill_created",
    )
    .await
}

pub async fn handle_skill_propose(state: SharedState, request: SkillProposeRequest) -> Response {
    let actor = match resolve_skill_actor(&state).await {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let package = match import_package(
        &state,
        request.request_id.as_str(),
        &request.source_directory,
    ) {
        Ok(package) => package,
        Err(error) => return skill_error(error),
    };
    let package = match validate_package_entries(&request.slug, package.entries) {
        Ok(package) => package,
        Err(error) => return skill_error(error),
    };
    let slug = request.slug.clone();
    let proposal_id = ProposalId::new(&format!("p-{}", &request.request_id.as_str()[2..])).ok();
    run_mutation_with_proposal(
        state,
        actor,
        SkillMutationRequest::Propose(request),
        Some(package),
        Some(slug),
        "proposal_created",
        proposal_id,
    )
    .await
}

pub async fn handle_skill_proposal_transition(
    state: SharedState,
    request: SkillProposalTransitionRequest,
) -> Response {
    let actor = match resolve_skill_actor(&state).await {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let slug = match state.skill_store.proposal_skill(&request.proposal_id) {
        Ok(slug) => slug,
        Err(error) => return skill_error(error),
    };
    let kind = match request.operation {
        SkillOperation::ProposalPublish => "proposal_published",
        SkillOperation::ProposalReject => "proposal_rejected",
        SkillOperation::ProposalWithdraw => "proposal_withdrawn",
        _ => return skill_error(SkillError::SyncConflict),
    };
    let proposal_id = request.proposal_id.clone();
    run_mutation_with_proposal(
        state,
        actor,
        SkillMutationRequest::ProposalTransition(request),
        None,
        Some(slug),
        kind,
        Some(proposal_id),
    )
    .await
}

pub async fn handle_skill_metadata_update(
    state: SharedState,
    request: SkillMetadataUpdateRequest,
) -> Response {
    let actor = match resolve_skill_actor(&state).await {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let slug = request.slug.clone();
    run_mutation(
        state,
        actor,
        SkillMutationRequest::MetadataUpdate(request),
        None,
        Some(slug),
        "metadata_updated",
    )
    .await
}

pub async fn handle_skill_role_update(
    state: SharedState,
    request: SkillRoleUpdateRequest,
) -> Response {
    let actor = match resolve_skill_actor(&state).await {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let kind = match request.operation {
        SkillOperation::OwnerAdd => "owner_added",
        SkillOperation::OwnerRemove => "owner_removed",
        SkillOperation::MaintainerAdd => "maintainer_added",
        SkillOperation::MaintainerRemove => "maintainer_removed",
        _ => return skill_error(SkillError::SyncConflict),
    };
    let slug = request.slug.clone();
    run_mutation(
        state,
        actor,
        SkillMutationRequest::RoleUpdate(request),
        None,
        Some(slug),
        kind,
    )
    .await
}

pub async fn handle_skill_archive_transition(
    state: SharedState,
    request: SkillArchiveTransitionRequest,
) -> Response {
    let actor = match resolve_skill_actor(&state).await {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let kind = match request.operation {
        SkillOperation::Archive => "skill_archived",
        SkillOperation::Unarchive => "skill_unarchived",
        _ => return skill_error(SkillError::SyncConflict),
    };
    let slug = request.slug.clone();
    run_mutation(
        state,
        actor,
        SkillMutationRequest::ArchiveTransition(request),
        None,
        Some(slug),
        kind,
    )
    .await
}

pub async fn handle_skill_repair(state: SharedState, request: SkillRepairRequest) -> Response {
    let actor = match resolve_skill_actor(&state).await {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    let slug = match &request.scope {
        gitim_core::skill::SkillRepairScope::Workspace => None,
        gitim_core::skill::SkillRepairScope::Skill(slug) => Some(slug.clone()),
    };
    run_mutation(
        state,
        actor,
        SkillMutationRequest::Repair(request),
        None,
        slug,
        "state_repaired",
    )
    .await
}

async fn resolve_skill_actor(state: &SharedState) -> Result<String, Response> {
    let actor = state.current_user.read().await.clone().ok_or_else(|| {
        Response::error_with_code(
            "Skill operation requires an identity",
            "skill_admin_required",
        )
    })?;
    ensure_author_not_departed(state, &actor)?;
    if !state.users.read().await.iter().any(|user| user == &actor) {
        return Err(skill_error(SkillError::RoleTargetInactive));
    }
    Ok(actor)
}

fn import_package(
    state: &SharedState,
    request_id: &str,
    source: &std::path::Path,
) -> Result<ValidatedPackage, SkillError> {
    let request_dir = state
        .repo_root
        .join(".gitim/skill-imports")
        .join(request_id);
    if request_dir.exists() {
        fs::remove_dir_all(&request_dir).map_err(|_| SkillError::InvalidPackage)?;
    }
    let package = snapshot_skill_directory(source, &request_dir);
    let cleanup = fs::remove_dir_all(&request_dir);
    if cleanup.is_err() && package.is_ok() {
        return Err(SkillError::InvalidPackage);
    }
    package
}

async fn run_mutation(
    state: SharedState,
    actor: String,
    request: SkillMutationRequest,
    package: Option<ValidatedPackage>,
    slug: Option<SkillSlug>,
    kind: &'static str,
) -> Response {
    run_mutation_with_proposal(state, actor, request, package, slug, kind, None).await
}

#[allow(clippy::too_many_arguments)]
async fn run_mutation_with_proposal(
    state: SharedState,
    actor: String,
    request: SkillMutationRequest,
    package: Option<ValidatedPackage>,
    slug: Option<SkillSlug>,
    kind: &'static str,
    proposal_id: Option<ProposalId>,
) -> Response {
    let request_id = request.request_id().clone();
    if !state.has_remote {
        return skill_error(SkillError::RemoteRequired);
    }
    let active_users = state
        .users
        .read()
        .await
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let (_, author_email) = state.author_for(&actor);
    let root = state.repo_root.clone();
    let transaction = tokio::task::spawn_blocking(move || {
        let repo = gitim_sync::git::GitStorage::new(&root);
        let guard = SkillSyncGuard::new(&root)?;
        execute_remote_skill_transaction(
            &repo,
            &guard,
            RemoteSkillTransactionRequest {
                request,
                actor,
                author_email,
                now: chrono::Utc::now().to_rfc3339(),
                package,
                active_users,
            },
        )
    })
    .await;
    let result = match transaction {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => return sync_error(error),
        Err(error) => {
            return Response::error_with_code(
                format!("Skill transaction task failed: {error}"),
                SkillError::SyncConflict.code(),
            )
        }
    };
    state.skill_store.invalidate();
    if let Some(slug) = slug {
        if let (Some(event_revision), Some(control_revision)) =
            (result.result.event_revision, result.result.control_revision)
        {
            state.record_local_skill_event(
                slug.as_str(),
                kind,
                event_revision,
                control_revision,
                proposal_id
                    .as_ref()
                    .map(|proposal| proposal.as_str().to_owned()),
                result.result.proposal_state_revision,
            );
        }
    }
    Response::json(SkillMutationResponse {
        request_id,
        proposal_id,
        commit_id: result.commit_id,
        result: result.result,
        local_state: result.local_state,
    })
}

fn skill_result<T: Serialize>(result: Result<T, SkillError>) -> Response {
    match result {
        Ok(value) => Response::json(value),
        Err(error) => skill_error(error),
    }
}

fn skill_error(error: SkillError) -> Response {
    Response::error_with_code(error.to_string(), error.code())
}

fn sync_error(error: SkillSyncError) -> Response {
    Response::error_with_code(error.to_string(), error.code())
}
