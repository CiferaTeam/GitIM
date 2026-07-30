use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{formatter::format_event, types::Handler};

use super::{
    portable_path::valid_portable_relative_paths, ProposalId, ProposalStatus, RequestId,
    RevisionId, SkillError, SkillMeta, SkillMutationRequest, SkillMutationResult, SkillOperation,
    SkillProposalMeta, SkillPublicationMeta, SkillReceipt, SkillReceiptRequest, SkillReceiptScope,
    SkillRepairScope, SkillRevisionMeta, SkillSlug, ValidatedPackage, WorkspaceSkillMeta,
    SKILL_SCHEMA_VERSION,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SkillRepositorySnapshot {
    pub workspace: Option<WorkspaceSkillMeta>,
    pub active_skills: BTreeMap<SkillSlug, SkillObjectSnapshot>,
    pub archived_skills: BTreeMap<SkillSlug, SkillObjectSnapshot>,
    pub receipts: BTreeMap<RequestId, SkillReceipt>,
    pub active_users: BTreeSet<String>,
    pub conflict_checkpoint: Option<SkillConflictCheckpoint>,
    pub repository_files: BTreeMap<String, Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillObjectSnapshot {
    pub meta: SkillMeta,
    pub revisions: BTreeMap<RevisionId, SkillRevisionSnapshot>,
    pub publications: BTreeMap<RevisionId, SkillPublicationMeta>,
    pub proposals: BTreeMap<ProposalId, SkillProposalSnapshot>,
    pub history: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillRevisionSnapshot {
    pub meta: SkillRevisionMeta,
    pub package: ValidatedPackage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillProposalSnapshot {
    pub meta: SkillProposalMeta,
    pub discussion: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SkillRepairAcceptedState {
    Workspace(WorkspaceSkillMeta),
    ActiveSkill {
        slug: SkillSlug,
        skill: SkillObjectSnapshot,
    },
    ArchivedSkill {
        slug: SkillSlug,
        skill: SkillObjectSnapshot,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillConflictCheckpoint {
    pub conflict_tip: String,
    pub accepted_tree: String,
    pub accepted_state: SkillRepairAcceptedState,
    pub accepted_files: BTreeMap<String, Vec<u8>>,
    pub changed_paths: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SkillTreeEdit {
    Upsert { path: String, bytes: Vec<u8> },
    Delete { path: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillMutationContext {
    pub actor: String,
    pub now: String,
    pub package: Option<ValidatedPackage>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillMutationPlan {
    pub after: SkillRepositorySnapshot,
    pub edits: Vec<SkillTreeEdit>,
    pub receipt: SkillReceipt,
    pub result: SkillMutationResult,
    pub changed_paths: BTreeSet<String>,
    pub commit_message: String,
    pub commit_evidence: SkillCommitEvidence,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillCommitEvidence {
    pub commit_author: String,
    pub request_trailer: RequestId,
    pub parent_count: usize,
    pub receipt: SkillReceipt,
    pub changed_paths: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillTransitionOutcome {
    pub changed_skill: Option<SkillSlug>,
    pub event_revision: Option<u64>,
    pub control_revision: Option<u64>,
}

pub fn plan_skill_mutation(
    before: &SkillRepositorySnapshot,
    context: &SkillMutationContext,
    request: &SkillMutationRequest,
) -> Result<SkillMutationPlan, SkillError> {
    if let Some(recorded) = before.receipts.get(request.request_id()) {
        if raw_semantic_request_matches(recorded, context, request) {
            validate_snapshot(before)?;
            return Ok(duplicate_plan(before, recorded));
        }
        return Err(SkillError::RequestIdConflict);
    }

    let actor = Handler::new(&context.actor).map_err(|_| SkillError::RoleTargetInvalid)?;
    let receipt = receipt_for_request(before, context, request, actor)?;
    let (mut after, final_receipt) = execute_transition(before, receipt, context.package.as_ref())?;
    validate_canonical_receipt_request(before, &after, &final_receipt, context.package.as_ref())?;
    let edits = expected_edits(before, &after, &final_receipt)?;
    after.repository_files = apply_tree_edits(&before.repository_files, &edits);
    let changed_paths = edit_paths(&edits);
    let commit_evidence = SkillCommitEvidence {
        commit_author: context.actor.clone(),
        request_trailer: final_receipt.id.clone(),
        parent_count: 1,
        receipt: final_receipt.clone(),
        changed_paths: changed_paths.clone(),
    };

    validate_skill_commit(before, &after, &commit_evidence)?;

    Ok(SkillMutationPlan {
        after,
        edits,
        receipt: final_receipt.clone(),
        result: final_receipt.result.clone(),
        changed_paths,
        commit_message: format!(
            "feat(skills): {}\n\nGitim-Request-Id: {}",
            operation_name(final_receipt.operation),
            final_receipt.id.as_str()
        ),
        commit_evidence,
    })
}

fn duplicate_plan(before: &SkillRepositorySnapshot, recorded: &SkillReceipt) -> SkillMutationPlan {
    SkillMutationPlan {
        after: before.clone(),
        edits: Vec::new(),
        receipt: recorded.clone(),
        result: recorded.result.clone(),
        changed_paths: BTreeSet::new(),
        commit_message: String::new(),
        commit_evidence: SkillCommitEvidence {
            commit_author: recorded.actor.as_str().to_owned(),
            request_trailer: recorded.id.clone(),
            parent_count: 1,
            receipt: recorded.clone(),
            changed_paths: BTreeSet::new(),
        },
    }
}

fn raw_semantic_request_matches(
    recorded: &SkillReceipt,
    context: &SkillMutationContext,
    request: &SkillMutationRequest,
) -> bool {
    if recorded.schema_version != SKILL_SCHEMA_VERSION
        || &recorded.id != request.request_id()
        || recorded.actor.as_str() != context.actor
    {
        return false;
    }

    let fingerprint = match request {
        SkillMutationRequest::WorkspaceBootstrap(_) => Some((
            SkillReceiptScope::Workspace,
            None,
            SkillOperation::WorkspaceBootstrap,
            SkillReceiptRequest {
                payload_sha256: hash_bytes(
                    operation_name(SkillOperation::WorkspaceBootstrap).as_bytes(),
                ),
                target: Some(recorded.actor.clone()),
                ..SkillReceiptRequest::default()
            },
        )),
        SkillMutationRequest::Create(request) => {
            let Some(package) = context.package.as_ref() else {
                return false;
            };
            let Ok(revision) = revision_for_request(&request.request_id) else {
                return false;
            };
            Some((
                SkillReceiptScope::Skill,
                Some(request.slug.clone()),
                SkillOperation::SkillCreate,
                SkillReceiptRequest {
                    payload_sha256: package.content_sha256.clone(),
                    slug: Some(request.slug.clone()),
                    revision: Some(revision),
                    display_name: Some(request.display_name.clone()),
                    description: Some(request.description.clone()),
                    ..SkillReceiptRequest::default()
                },
            ))
        }
        SkillMutationRequest::Propose(request) => {
            let Some(package) = context.package.as_ref() else {
                return false;
            };
            let Ok(candidate_revision) = revision_for_request(&request.request_id) else {
                return false;
            };
            let Ok(proposal) = proposal_for_request(&request.request_id) else {
                return false;
            };
            Some((
                SkillReceiptScope::Skill,
                Some(request.slug.clone()),
                SkillOperation::ProposalCreate,
                SkillReceiptRequest {
                    payload_sha256: package.content_sha256.clone(),
                    slug: Some(request.slug.clone()),
                    base_revision: Some(request.base_revision.clone()),
                    candidate_revision: Some(candidate_revision),
                    proposal: Some(proposal),
                    summary: Some(request.summary.clone()),
                    ..SkillReceiptRequest::default()
                },
            ))
        }
        SkillMutationRequest::ProposalTransition(request) => {
            if !matches!(
                request.operation,
                SkillOperation::ProposalPublish
                    | SkillOperation::ProposalReject
                    | SkillOperation::ProposalWithdraw
            ) {
                return false;
            }
            let Some(slug) = recorded.skill.clone() else {
                return false;
            };
            Some((
                SkillReceiptScope::Skill,
                Some(slug.clone()),
                request.operation,
                SkillReceiptRequest {
                    payload_sha256: hash_bytes(operation_name(request.operation).as_bytes()),
                    slug: Some(slug),
                    proposal: Some(request.proposal_id.clone()),
                    expected_control_revision: request.expected_control_revision,
                    expected_proposal_revision: Some(request.expected_state_revision),
                    ..SkillReceiptRequest::default()
                },
            ))
        }
        SkillMutationRequest::Repair(request) => {
            let (scope, skill) = match &request.scope {
                SkillRepairScope::Workspace => (SkillReceiptScope::Workspace, None),
                SkillRepairScope::Skill(slug) => (SkillReceiptScope::Skill, Some(slug.clone())),
            };
            Some((
                scope,
                skill.clone(),
                SkillOperation::RepairSkillState,
                SkillReceiptRequest {
                    payload_sha256: hash_bytes(
                        operation_name(SkillOperation::RepairSkillState).as_bytes(),
                    ),
                    slug: skill,
                    conflict_tip: Some(request.conflict_tip.clone()),
                    accepted_tree: Some(request.accepted_tree.clone()),
                    ..SkillReceiptRequest::default()
                },
            ))
        }
    };

    fingerprint.is_some_and(|(scope, skill, operation, typed_request)| {
        recorded.scope == scope
            && recorded.skill == skill
            && recorded.operation == operation
            && recorded.request == typed_request
    })
}

pub fn validate_skill_commit(
    before: &SkillRepositorySnapshot,
    after: &SkillRepositorySnapshot,
    evidence: &SkillCommitEvidence,
) -> Result<SkillTransitionOutcome, SkillError> {
    if evidence.parent_count != 1
        || evidence.commit_author != evidence.receipt.actor.as_str()
        || evidence.request_trailer != evidence.receipt.id
        || before.receipts.contains_key(&evidence.receipt.id)
        || after.receipts.get(&evidence.receipt.id) != Some(&evidence.receipt)
    {
        return Err(SkillError::SyncConflict);
    }

    let package = transition_package(after, &evidence.receipt)?;
    validate_canonical_receipt_request(before, after, &evidence.receipt, package)?;
    let (mut expected_after, expected_receipt) =
        execute_transition(before, evidence.receipt.clone(), package)?;
    let expected_edits = expected_edits(before, &expected_after, &evidence.receipt)?;
    expected_after.repository_files = apply_tree_edits(&before.repository_files, &expected_edits);
    if expected_receipt != evidence.receipt || expected_after != *after {
        return Err(SkillError::SyncConflict);
    }
    if edit_paths(&expected_edits) != evidence.changed_paths {
        return Err(SkillError::SyncConflict);
    }

    let changed_skill = evidence.receipt.skill.clone();
    let (event_revision, control_revision) = changed_skill
        .as_ref()
        .and_then(|slug| {
            after
                .active_skills
                .get(slug)
                .or_else(|| after.archived_skills.get(slug))
        })
        .map_or((None, evidence.receipt.result.control_revision), |skill| {
            (
                Some(skill.meta.event_revision),
                Some(skill.meta.control_revision),
            )
        });

    Ok(SkillTransitionOutcome {
        changed_skill,
        event_revision,
        control_revision,
    })
}

fn receipt_for_request(
    before: &SkillRepositorySnapshot,
    context: &SkillMutationContext,
    request: &SkillMutationRequest,
    actor: Handler,
) -> Result<SkillReceipt, SkillError> {
    let (scope, skill, operation, mut typed_request) = match request {
        SkillMutationRequest::WorkspaceBootstrap(_) => (
            SkillReceiptScope::Workspace,
            None,
            SkillOperation::WorkspaceBootstrap,
            SkillReceiptRequest {
                target: Some(actor.clone()),
                ..SkillReceiptRequest::default()
            },
        ),
        SkillMutationRequest::Create(request) => {
            validate_bounded_text(&request.display_name, 80)?;
            validate_bounded_text(&request.description, 1_024)?;
            let package = context.package.as_ref().ok_or(SkillError::InvalidPackage)?;
            let revision = revision_for_request(&request.request_id)?;
            (
                SkillReceiptScope::Skill,
                Some(request.slug.clone()),
                SkillOperation::SkillCreate,
                SkillReceiptRequest {
                    slug: Some(request.slug.clone()),
                    revision: Some(revision),
                    display_name: Some(request.display_name.clone()),
                    description: Some(request.description.clone()),
                    ..SkillReceiptRequest::default()
                },
            )
                .with_payload(package.content_sha256.clone())
        }
        SkillMutationRequest::Propose(request) => {
            validate_bounded_text(&request.summary, 500)?;
            let package = context.package.as_ref().ok_or(SkillError::InvalidPackage)?;
            (
                SkillReceiptScope::Skill,
                Some(request.slug.clone()),
                SkillOperation::ProposalCreate,
                SkillReceiptRequest {
                    slug: Some(request.slug.clone()),
                    base_revision: Some(request.base_revision.clone()),
                    candidate_revision: Some(revision_for_request(&request.request_id)?),
                    proposal: Some(proposal_for_request(&request.request_id)?),
                    summary: Some(request.summary.clone()),
                    ..SkillReceiptRequest::default()
                },
            )
                .with_payload(package.content_sha256.clone())
        }
        SkillMutationRequest::ProposalTransition(request) => {
            if !matches!(
                request.operation,
                SkillOperation::ProposalPublish
                    | SkillOperation::ProposalReject
                    | SkillOperation::ProposalWithdraw
            ) {
                return Err(SkillError::SyncConflict);
            }
            let slug = find_proposal(before, &request.proposal_id)
                .map(|(slug, _, _)| slug.clone())
                .ok_or(SkillError::ProposalNotFound)?;
            (
                SkillReceiptScope::Skill,
                Some(slug.clone()),
                request.operation,
                SkillReceiptRequest {
                    slug: Some(slug),
                    proposal: Some(request.proposal_id.clone()),
                    expected_control_revision: request.expected_control_revision,
                    expected_proposal_revision: Some(request.expected_state_revision),
                    ..SkillReceiptRequest::default()
                },
            )
        }
        SkillMutationRequest::Repair(request) => {
            let skill = match &request.scope {
                SkillRepairScope::Workspace => None,
                SkillRepairScope::Skill(slug) => Some(slug.clone()),
            };
            (
                match request.scope {
                    SkillRepairScope::Workspace => SkillReceiptScope::Workspace,
                    SkillRepairScope::Skill(_) => SkillReceiptScope::Skill,
                },
                skill.clone(),
                SkillOperation::RepairSkillState,
                SkillReceiptRequest {
                    slug: skill,
                    conflict_tip: Some(request.conflict_tip.clone()),
                    accepted_tree: Some(request.accepted_tree.clone()),
                    ..SkillReceiptRequest::default()
                },
            )
        }
    };

    if typed_request.payload_sha256.is_empty() {
        typed_request.payload_sha256 = hash_bytes(operation_name(operation).as_bytes());
    }

    Ok(SkillReceipt {
        schema_version: SKILL_SCHEMA_VERSION,
        id: request.request_id().clone(),
        scope,
        skill,
        actor,
        operation,
        request: typed_request,
        result: empty_result(),
        created_at: context.now.clone(),
    })
}

struct CanonicalReceiptRequest {
    scope: SkillReceiptScope,
    skill: Option<SkillSlug>,
    request: SkillReceiptRequest,
}

fn validate_canonical_receipt_request(
    before: &SkillRepositorySnapshot,
    after: &SkillRepositorySnapshot,
    receipt: &SkillReceipt,
    package: Option<&ValidatedPackage>,
) -> Result<(), SkillError> {
    if receipt.schema_version != SKILL_SCHEMA_VERSION {
        return Err(SkillError::SyncConflict);
    }
    let canonical = canonical_receipt_request(before, after, receipt, package)?;
    if receipt.scope != canonical.scope
        || receipt.skill != canonical.skill
        || receipt.request != canonical.request
    {
        return Err(SkillError::SyncConflict);
    }
    Ok(())
}

fn canonical_receipt_request(
    before: &SkillRepositorySnapshot,
    after: &SkillRepositorySnapshot,
    receipt: &SkillReceipt,
    package: Option<&ValidatedPackage>,
) -> Result<CanonicalReceiptRequest, SkillError> {
    let operation_payload = || hash_bytes(operation_name(receipt.operation).as_bytes());
    match receipt.operation {
        SkillOperation::WorkspaceBootstrap => Ok(CanonicalReceiptRequest {
            scope: SkillReceiptScope::Workspace,
            skill: None,
            request: SkillReceiptRequest {
                payload_sha256: operation_payload(),
                target: Some(receipt.actor.clone()),
                ..SkillReceiptRequest::default()
            },
        }),
        SkillOperation::SkillCreate => {
            let slug = receipt
                .skill
                .as_ref()
                .ok_or(SkillError::SyncConflict)?
                .clone();
            let revision = revision_for_request(&receipt.id)?;
            let package = package.ok_or(SkillError::InvalidPackage)?;
            let skill = after
                .active_skills
                .get(&slug)
                .ok_or(SkillError::SyncConflict)?;
            Ok(CanonicalReceiptRequest {
                scope: SkillReceiptScope::Skill,
                skill: Some(slug.clone()),
                request: SkillReceiptRequest {
                    payload_sha256: package.content_sha256.clone(),
                    slug: Some(slug),
                    revision: Some(revision),
                    display_name: Some(skill.meta.display_name.clone()),
                    description: Some(skill.meta.description.clone()),
                    ..SkillReceiptRequest::default()
                },
            })
        }
        SkillOperation::ProposalCreate => {
            let slug = receipt
                .skill
                .as_ref()
                .ok_or(SkillError::SyncConflict)?
                .clone();
            let candidate_revision = revision_for_request(&receipt.id)?;
            let proposal = proposal_for_request(&receipt.id)?;
            let package = package.ok_or(SkillError::InvalidPackage)?;
            let proposal_meta = &after
                .active_skills
                .get(&slug)
                .ok_or(SkillError::SyncConflict)?
                .proposals
                .get(&proposal)
                .ok_or(SkillError::SyncConflict)?
                .meta;
            Ok(CanonicalReceiptRequest {
                scope: SkillReceiptScope::Skill,
                skill: Some(slug.clone()),
                request: SkillReceiptRequest {
                    payload_sha256: package.content_sha256.clone(),
                    slug: Some(slug),
                    base_revision: Some(proposal_meta.base_revision.clone()),
                    candidate_revision: Some(candidate_revision),
                    proposal: Some(proposal),
                    summary: Some(proposal_meta.summary.clone()),
                    ..SkillReceiptRequest::default()
                },
            })
        }
        SkillOperation::ProposalPublish
        | SkillOperation::ProposalReject
        | SkillOperation::ProposalWithdraw => {
            let proposal = receipt
                .request
                .proposal
                .as_ref()
                .ok_or(SkillError::SyncConflict)?
                .clone();
            let (slug, skill, _) =
                find_proposal(before, &proposal).ok_or(SkillError::ProposalNotFound)?;
            let expected_control_revision = if receipt.operation == SkillOperation::ProposalPublish
                || receipt.request.expected_control_revision.is_some()
            {
                Some(skill.meta.control_revision)
            } else {
                None
            };
            Ok(CanonicalReceiptRequest {
                scope: SkillReceiptScope::Skill,
                skill: Some(slug.clone()),
                request: SkillReceiptRequest {
                    payload_sha256: operation_payload(),
                    slug: Some(slug.clone()),
                    proposal: Some(proposal.clone()),
                    expected_control_revision,
                    expected_proposal_revision: Some(
                        skill.proposals[&proposal].meta.state_revision,
                    ),
                    ..SkillReceiptRequest::default()
                },
            })
        }
        SkillOperation::RepairSkillState => {
            let checkpoint = before
                .conflict_checkpoint
                .as_ref()
                .ok_or(SkillError::SyncConflict)?;
            let (scope, skill) = match &checkpoint.accepted_state {
                SkillRepairAcceptedState::Workspace(_) => (SkillReceiptScope::Workspace, None),
                SkillRepairAcceptedState::ActiveSkill { slug, .. }
                | SkillRepairAcceptedState::ArchivedSkill { slug, .. } => {
                    (SkillReceiptScope::Skill, Some(slug.clone()))
                }
            };
            Ok(CanonicalReceiptRequest {
                scope,
                skill: skill.clone(),
                request: SkillReceiptRequest {
                    payload_sha256: operation_payload(),
                    slug: skill,
                    conflict_tip: Some(checkpoint.conflict_tip.clone()),
                    accepted_tree: Some(checkpoint.accepted_tree.clone()),
                    ..SkillReceiptRequest::default()
                },
            })
        }
        _ => Err(SkillError::SyncConflict),
    }
}

trait ReceiptTuplePayload {
    fn with_payload(self, payload: String) -> Self;
}

impl ReceiptTuplePayload
    for (
        SkillReceiptScope,
        Option<SkillSlug>,
        SkillOperation,
        SkillReceiptRequest,
    )
{
    fn with_payload(mut self, payload: String) -> Self {
        self.3.payload_sha256 = payload;
        self
    }
}

fn execute_transition(
    before: &SkillRepositorySnapshot,
    mut receipt: SkillReceipt,
    package: Option<&ValidatedPackage>,
) -> Result<(SkillRepositorySnapshot, SkillReceipt), SkillError> {
    if receipt.operation == SkillOperation::RepairSkillState {
        validate_repair_pre_state(before, &receipt)?;
    } else {
        validate_snapshot(before)?;
    }
    authorize_and_check_preconditions(before, &receipt)?;

    let mut after = before.clone();
    let result = match receipt.operation {
        SkillOperation::WorkspaceBootstrap => apply_workspace_bootstrap(&mut after, &receipt)?,
        SkillOperation::SkillCreate => apply_skill_create(&mut after, &receipt, package)?,
        SkillOperation::ProposalCreate => apply_proposal_create(&mut after, &receipt, package)?,
        SkillOperation::ProposalPublish
        | SkillOperation::ProposalReject
        | SkillOperation::ProposalWithdraw => apply_proposal_transition(&mut after, &receipt)?,
        SkillOperation::RepairSkillState => apply_repair(&mut after, &receipt)?,
        _ => return Err(SkillError::SyncConflict),
    };
    receipt.result = result;
    after.receipts.insert(receipt.id.clone(), receipt.clone());
    validate_typed_snapshot(&after)?;
    Ok((after, receipt))
}

fn validate_repair_pre_state(
    before: &SkillRepositorySnapshot,
    receipt: &SkillReceipt,
) -> Result<(), SkillError> {
    let checkpoint = before
        .conflict_checkpoint
        .as_ref()
        .ok_or(SkillError::SyncConflict)?;
    if !repair_scope_matches(receipt, &checkpoint.accepted_state) {
        return Err(SkillError::SyncConflict);
    }

    let mut unaffected = before.clone();
    match &checkpoint.accepted_state {
        SkillRepairAcceptedState::Workspace(workspace) => {
            unaffected.workspace = Some(workspace.clone());
            let accepted_bytes = checkpoint
                .accepted_files
                .get("skills/workspace.meta.yaml")
                .ok_or(SkillError::SyncConflict)?;
            unaffected.repository_files.insert(
                "skills/workspace.meta.yaml".to_owned(),
                accepted_bytes.clone(),
            );
        }
        SkillRepairAcceptedState::ActiveSkill { slug, .. }
        | SkillRepairAcceptedState::ArchivedSkill { slug, .. } => {
            unaffected.active_skills.remove(slug);
            unaffected.archived_skills.remove(slug);
            let active_prefix = format!("skills/{}/", slug.as_str());
            let archived_prefix = format!("archive/skills/{}/", slug.as_str());
            unaffected.repository_files.retain(|path, _| {
                !path.starts_with(&active_prefix) && !path.starts_with(&archived_prefix)
            });
        }
    }
    validate_snapshot(&unaffected)
}

fn authorize_and_check_preconditions(
    before: &SkillRepositorySnapshot,
    receipt: &SkillReceipt,
) -> Result<(), SkillError> {
    if before.receipts.contains_key(&receipt.id) {
        return Err(SkillError::RequestIdConflict);
    }
    if !before.active_users.contains(receipt.actor.as_str()) {
        return Err(SkillError::RoleTargetInactive);
    }
    if before.conflict_checkpoint.is_some() && receipt.operation != SkillOperation::RepairSkillState
    {
        return Err(SkillError::SyncConflict);
    }

    match receipt.operation {
        SkillOperation::WorkspaceBootstrap => {
            if before.workspace.is_some() {
                return Err(SkillError::SyncConflict);
            }
            if !before.active_skills.is_empty() || !before.archived_skills.is_empty() {
                return Err(SkillError::AdminUninitialized);
            }
        }
        SkillOperation::SkillCreate | SkillOperation::ProposalCreate => {
            if before.workspace.is_none() {
                return Err(SkillError::AdminUninitialized);
            }
        }
        SkillOperation::ProposalPublish
        | SkillOperation::ProposalReject
        | SkillOperation::ProposalWithdraw => {
            let proposal_id = receipt
                .request
                .proposal
                .as_ref()
                .ok_or(SkillError::SyncConflict)?;
            let (slug, skill, archived) =
                find_proposal(before, proposal_id).ok_or(SkillError::ProposalNotFound)?;
            if archived {
                return Err(SkillError::Archived);
            }
            if receipt.skill.as_ref() != Some(slug) {
                return Err(SkillError::SyncConflict);
            }
            let proposal = &skill.proposals[proposal_id].meta;
            if proposal.status != ProposalStatus::Open {
                return Err(SkillError::ProposalTerminal);
            }
            if receipt.request.expected_proposal_revision != Some(proposal.state_revision) {
                return Err(stale_proposal(skill, proposal));
            }
            if receipt.operation == SkillOperation::ProposalWithdraw {
                if proposal.created_by != receipt.actor {
                    return Err(SkillError::NotOwner);
                }
            } else if !contains_handler(&skill.meta.maintainers, &receipt.actor) {
                return Err(SkillError::NotMaintainer);
            }
            if let Some(expected) = receipt.request.expected_control_revision {
                if expected != skill.meta.control_revision {
                    return Err(stale_control(skill));
                }
            } else if receipt.operation == SkillOperation::ProposalPublish {
                return Err(stale_control(skill));
            }
            if receipt.operation == SkillOperation::ProposalPublish
                && (proposal.base_revision != skill.meta.current_revision
                    || skill.revisions[&proposal.candidate_revision]
                        .meta
                        .base_revision
                        .as_ref()
                        != Some(&skill.meta.current_revision))
            {
                return Err(stale_content(skill));
            }
        }
        SkillOperation::RepairSkillState => {
            let checkpoint = before
                .conflict_checkpoint
                .as_ref()
                .ok_or(SkillError::SyncConflict)?;
            if receipt.request.conflict_tip.as_deref() != Some(&checkpoint.conflict_tip)
                || receipt.request.accepted_tree.as_deref() != Some(&checkpoint.accepted_tree)
                || !repair_scope_matches(receipt, &checkpoint.accepted_state)
                || !valid_repair_checkpoint(before, checkpoint)
            {
                return Err(SkillError::SyncConflict);
            }
            let workspace = match &checkpoint.accepted_state {
                SkillRepairAcceptedState::Workspace(workspace) => workspace,
                SkillRepairAcceptedState::ActiveSkill { .. }
                | SkillRepairAcceptedState::ArchivedSkill { .. } => before
                    .workspace
                    .as_ref()
                    .ok_or(SkillError::AdminUninitialized)?,
            };
            if !contains_handler(&workspace.administrators, &receipt.actor) {
                return Err(SkillError::AdminRequired);
            }
        }
        _ => return Err(SkillError::SyncConflict),
    }
    Ok(())
}

fn apply_workspace_bootstrap(
    after: &mut SkillRepositorySnapshot,
    receipt: &SkillReceipt,
) -> Result<SkillMutationResult, SkillError> {
    after.workspace = Some(WorkspaceSkillMeta {
        schema_version: SKILL_SCHEMA_VERSION,
        administrators: vec![receipt.actor.clone()],
        control_revision: 1,
        created_at: receipt.created_at.clone(),
        updated_at: receipt.created_at.clone(),
    });
    Ok(SkillMutationResult {
        control_revision: Some(1),
        ..empty_result()
    })
}

fn apply_skill_create(
    after: &mut SkillRepositorySnapshot,
    receipt: &SkillReceipt,
    package: Option<&ValidatedPackage>,
) -> Result<SkillMutationResult, SkillError> {
    let slug = receipt.skill.as_ref().ok_or(SkillError::SyncConflict)?;
    if after.active_skills.contains_key(slug) || after.archived_skills.contains_key(slug) {
        return Err(SkillError::Exists);
    }
    let package = package.ok_or(SkillError::InvalidPackage)?;
    if receipt.request.payload_sha256 != package.content_sha256 {
        return Err(SkillError::InvalidPackage);
    }
    let revision = receipt
        .request
        .revision
        .as_ref()
        .ok_or(SkillError::SyncConflict)?
        .clone();
    let revision_meta = SkillRevisionMeta {
        schema_version: SKILL_SCHEMA_VERSION,
        id: revision.clone(),
        skill: slug.clone(),
        base_revision: None,
        content_sha256: package.content_sha256.clone(),
        created_by: receipt.actor.clone(),
        created_at: receipt.created_at.clone(),
    };
    let publication = SkillPublicationMeta {
        schema_version: SKILL_SCHEMA_VERSION,
        skill: slug.clone(),
        revision: revision.clone(),
        content_sha256: package.content_sha256.clone(),
        base_revision: None,
        proposal: None,
        published_by: receipt.actor.clone(),
        published_at: receipt.created_at.clone(),
    };
    let mut skill = SkillObjectSnapshot {
        meta: SkillMeta {
            schema_version: SKILL_SCHEMA_VERSION,
            slug: slug.clone(),
            display_name: receipt
                .request
                .display_name
                .clone()
                .ok_or(SkillError::SyncConflict)?,
            description: receipt
                .request
                .description
                .clone()
                .ok_or(SkillError::SyncConflict)?,
            created_by: receipt.actor.clone(),
            owners: vec![receipt.actor.clone()],
            maintainers: vec![receipt.actor.clone()],
            current_revision: revision.clone(),
            open_proposal_count: 0,
            open_proposal_ids: Vec::new(),
            control_revision: 1,
            event_revision: 1,
            created_at: receipt.created_at.clone(),
            updated_at: receipt.created_at.clone(),
        },
        revisions: BTreeMap::from([(
            revision.clone(),
            SkillRevisionSnapshot {
                meta: revision_meta,
                package: package.clone(),
            },
        )]),
        publications: BTreeMap::from([(revision.clone(), publication)]),
        proposals: BTreeMap::new(),
        history: String::new(),
    };
    append_history(&mut skill, receipt);
    after.active_skills.insert(slug.clone(), skill);
    Ok(SkillMutationResult {
        canonical_ref: Some(super::SkillReference {
            slug: slug.clone(),
            revision: Some(revision.clone()),
        }),
        current_revision: Some(revision),
        control_revision: Some(1),
        event_revision: Some(1),
        ..empty_result()
    })
}

fn apply_proposal_create(
    after: &mut SkillRepositorySnapshot,
    receipt: &SkillReceipt,
    package: Option<&ValidatedPackage>,
) -> Result<SkillMutationResult, SkillError> {
    let slug = receipt.skill.as_ref().ok_or(SkillError::SyncConflict)?;
    if after.archived_skills.contains_key(slug) {
        return Err(SkillError::Archived);
    }
    let skill = after
        .active_skills
        .get_mut(slug)
        .ok_or(SkillError::NotFound)?;
    if skill.meta.open_proposal_count >= 100 {
        return Err(SkillError::OpenProposalLimit);
    }
    let base = receipt
        .request
        .base_revision
        .as_ref()
        .ok_or(SkillError::SyncConflict)?;
    if !skill.revisions.contains_key(base) {
        return Err(SkillError::RevisionNotFound);
    }
    if !skill.publications.contains_key(base) {
        return Err(SkillError::RevisionUnpublished);
    }
    let package = package.ok_or(SkillError::InvalidPackage)?;
    if receipt.request.payload_sha256 != package.content_sha256 {
        return Err(SkillError::InvalidPackage);
    }
    let candidate = receipt
        .request
        .candidate_revision
        .as_ref()
        .ok_or(SkillError::SyncConflict)?
        .clone();
    let proposal_id = receipt
        .request
        .proposal
        .as_ref()
        .ok_or(SkillError::SyncConflict)?
        .clone();
    if skill.revisions.contains_key(&candidate) || skill.proposals.contains_key(&proposal_id) {
        return Err(SkillError::Exists);
    }
    skill.revisions.insert(
        candidate.clone(),
        SkillRevisionSnapshot {
            meta: SkillRevisionMeta {
                schema_version: SKILL_SCHEMA_VERSION,
                id: candidate.clone(),
                skill: slug.clone(),
                base_revision: Some(base.clone()),
                content_sha256: package.content_sha256.clone(),
                created_by: receipt.actor.clone(),
                created_at: receipt.created_at.clone(),
            },
            package: package.clone(),
        },
    );
    skill.proposals.insert(
        proposal_id.clone(),
        SkillProposalSnapshot {
            meta: SkillProposalMeta {
                schema_version: SKILL_SCHEMA_VERSION,
                id: proposal_id.clone(),
                skill: slug.clone(),
                candidate_revision: candidate,
                base_revision: base.clone(),
                summary: receipt
                    .request
                    .summary
                    .clone()
                    .ok_or(SkillError::SyncConflict)?,
                status: ProposalStatus::Open,
                created_by: receipt.actor.clone(),
                created_at: receipt.created_at.clone(),
                updated_at: receipt.created_at.clone(),
                state_revision: 1,
                resolved_by: None,
                resolved_at: None,
            },
            discussion: String::new(),
        },
    );
    skill.meta.open_proposal_ids.push(proposal_id);
    sort_proposal_ids(&mut skill.meta.open_proposal_ids);
    skill.meta.open_proposal_count += 1;
    skill.meta.event_revision += 1;
    skill.meta.updated_at = receipt.created_at.clone();
    append_history(skill, receipt);
    Ok(result_for_skill(skill, None))
}

fn apply_proposal_transition(
    after: &mut SkillRepositorySnapshot,
    receipt: &SkillReceipt,
) -> Result<SkillMutationResult, SkillError> {
    let slug = receipt.skill.as_ref().ok_or(SkillError::SyncConflict)?;
    let skill = after
        .active_skills
        .get_mut(slug)
        .ok_or(SkillError::NotFound)?;
    let proposal_id = receipt
        .request
        .proposal
        .as_ref()
        .ok_or(SkillError::SyncConflict)?;
    let status = match receipt.operation {
        SkillOperation::ProposalPublish => ProposalStatus::Published,
        SkillOperation::ProposalReject => ProposalStatus::Rejected,
        SkillOperation::ProposalWithdraw => ProposalStatus::Withdrawn,
        _ => return Err(SkillError::SyncConflict),
    };
    let proposal_meta = {
        let proposal = skill
            .proposals
            .get_mut(proposal_id)
            .ok_or(SkillError::ProposalNotFound)?;
        proposal.meta.status = status;
        proposal.meta.state_revision += 1;
        proposal.meta.updated_at = receipt.created_at.clone();
        proposal.meta.resolved_by = Some(receipt.actor.clone());
        proposal.meta.resolved_at = Some(receipt.created_at.clone());
        proposal.meta.clone()
    };
    skill.meta.open_proposal_ids.retain(|id| id != proposal_id);
    skill.meta.open_proposal_count = skill
        .meta
        .open_proposal_count
        .checked_sub(1)
        .ok_or(SkillError::SyncConflict)?;
    skill.meta.event_revision += 1;
    skill.meta.updated_at = receipt.created_at.clone();

    if receipt.operation == SkillOperation::ProposalPublish {
        let candidate = proposal_meta.candidate_revision.clone();
        let revision = skill
            .revisions
            .get(&candidate)
            .ok_or(SkillError::RevisionNotFound)?;
        skill.publications.insert(
            candidate.clone(),
            SkillPublicationMeta {
                schema_version: SKILL_SCHEMA_VERSION,
                skill: slug.clone(),
                revision: candidate.clone(),
                content_sha256: revision.package.content_sha256.clone(),
                base_revision: Some(proposal_meta.base_revision.clone()),
                proposal: Some(proposal_id.clone()),
                published_by: receipt.actor.clone(),
                published_at: receipt.created_at.clone(),
            },
        );
        skill.meta.current_revision = candidate;
        skill.meta.control_revision += 1;
    }
    append_history(skill, receipt);
    Ok(result_for_skill(skill, Some(&proposal_meta)))
}

fn apply_repair(
    after: &mut SkillRepositorySnapshot,
    receipt: &SkillReceipt,
) -> Result<SkillMutationResult, SkillError> {
    let checkpoint = after
        .conflict_checkpoint
        .clone()
        .ok_or(SkillError::SyncConflict)?;
    let result = match checkpoint.accepted_state {
        SkillRepairAcceptedState::Workspace(workspace) => {
            let control_revision = workspace.control_revision;
            after.workspace = Some(workspace);
            SkillMutationResult {
                control_revision: Some(control_revision),
                ..empty_result()
            }
        }
        SkillRepairAcceptedState::ActiveSkill { slug, skill } => {
            after.archived_skills.remove(&slug);
            after.active_skills.insert(slug, skill.clone());
            result_for_skill(&skill, None)
        }
        SkillRepairAcceptedState::ArchivedSkill { slug, skill } => {
            after.active_skills.remove(&slug);
            after.archived_skills.insert(slug, skill.clone());
            result_for_skill(&skill, None)
        }
    };
    if receipt.request.accepted_tree.as_deref() != Some(&checkpoint.accepted_tree) {
        return Err(SkillError::SyncConflict);
    }
    after.conflict_checkpoint = None;
    Ok(result)
}

fn validate_snapshot(snapshot: &SkillRepositorySnapshot) -> Result<(), SkillError> {
    validate_typed_snapshot(snapshot)?;
    validate_repository_files(snapshot)
}

fn validate_typed_snapshot(snapshot: &SkillRepositorySnapshot) -> Result<(), SkillError> {
    if let Some(workspace) = &snapshot.workspace {
        if workspace.schema_version != SKILL_SCHEMA_VERSION
            || workspace.administrators.is_empty()
            || !sorted_unique_handlers(&workspace.administrators)
            || workspace
                .administrators
                .iter()
                .any(|handler| !snapshot.active_users.contains(handler.as_str()))
        {
            return Err(SkillError::SyncConflict);
        }
    } else if !snapshot.active_skills.is_empty() || !snapshot.archived_skills.is_empty() {
        return Err(SkillError::AdminUninitialized);
    }

    if snapshot
        .active_skills
        .keys()
        .any(|slug| snapshot.archived_skills.contains_key(slug))
    {
        return Err(SkillError::SyncConflict);
    }
    for (slug, skill) in snapshot
        .active_skills
        .iter()
        .chain(snapshot.archived_skills.iter())
    {
        validate_skill_object(snapshot, slug, skill)?;
    }
    for (id, receipt) in &snapshot.receipts {
        if id != &receipt.id
            || receipt.schema_version != SKILL_SCHEMA_VERSION
            || receipt.request.payload_sha256.is_empty()
        {
            return Err(SkillError::SyncConflict);
        }
    }
    Ok(())
}

fn validate_repository_files(snapshot: &SkillRepositorySnapshot) -> Result<(), SkillError> {
    let mut expected_paths = BTreeSet::new();
    if let Some(workspace) = &snapshot.workspace {
        let path = "skills/workspace.meta.yaml";
        let bytes = snapshot
            .repository_files
            .get(path)
            .ok_or(SkillError::SyncConflict)?;
        if !yaml_matches(bytes, workspace) {
            return Err(SkillError::SyncConflict);
        }
        expected_paths.insert(path.to_owned());
    }
    for (slug, skill) in &snapshot.active_skills {
        let root = format!("skills/{}", slug.as_str());
        let skill_paths = skill_object_paths(&root, skill);
        if !skill_paths
            .iter()
            .all(|path| snapshot.repository_files.contains_key(path))
            || !accepted_object_bytes_match(&root, skill, &snapshot.repository_files)
        {
            return Err(SkillError::SyncConflict);
        }
        expected_paths.extend(skill_paths);
    }
    for (slug, skill) in &snapshot.archived_skills {
        let root = format!("archive/skills/{}", slug.as_str());
        let skill_paths = skill_object_paths(&root, skill);
        if !skill_paths
            .iter()
            .all(|path| snapshot.repository_files.contains_key(path))
            || !accepted_object_bytes_match(&root, skill, &snapshot.repository_files)
        {
            return Err(SkillError::SyncConflict);
        }
        expected_paths.extend(skill_paths);
    }
    for (request_id, receipt) in &snapshot.receipts {
        let path = receipt_path(request_id);
        let bytes = snapshot
            .repository_files
            .get(&path)
            .ok_or(SkillError::SyncConflict)?;
        if !yaml_matches(bytes, receipt) {
            return Err(SkillError::SyncConflict);
        }
        expected_paths.insert(path);
    }
    if snapshot
        .repository_files
        .keys()
        .any(|path| managed_skill_path(path) && !expected_paths.contains(path))
    {
        return Err(SkillError::SyncConflict);
    }
    Ok(())
}

fn managed_skill_path(path: &str) -> bool {
    path.starts_with("skills/") || path.starts_with("archive/skills/")
}

fn validate_skill_object(
    repository: &SkillRepositorySnapshot,
    slug: &SkillSlug,
    skill: &SkillObjectSnapshot,
) -> Result<(), SkillError> {
    if &skill.meta.slug != slug
        || skill.meta.schema_version != SKILL_SCHEMA_VERSION
        || !valid_bounded_text(&skill.meta.display_name, 80)
        || !valid_bounded_text(&skill.meta.description, 1_024)
        || skill.meta.owners.is_empty()
        || skill.meta.owners.len() > 32
        || skill.meta.maintainers.len() > 64
        || !sorted_unique_handlers(&skill.meta.owners)
        || !sorted_unique_handlers(&skill.meta.maintainers)
        || skill
            .meta
            .owners
            .iter()
            .any(|owner| !contains_handler(&skill.meta.maintainers, owner))
        || skill
            .meta
            .owners
            .iter()
            .chain(skill.meta.maintainers.iter())
            .any(|handler| !repository.active_users.contains(handler.as_str()))
    {
        return Err(SkillError::SyncConflict);
    }

    for (id, revision) in &skill.revisions {
        if id != &revision.meta.id
            || &revision.meta.skill != slug
            || revision.meta.schema_version != SKILL_SCHEMA_VERSION
            || revision.meta.content_sha256 != revision.package.content_sha256
        {
            return Err(SkillError::RevisionCorrupted);
        }
    }
    for (id, publication) in &skill.publications {
        let revision = skill
            .revisions
            .get(id)
            .ok_or(SkillError::RevisionCorrupted)?;
        if id != &publication.revision
            || &publication.skill != slug
            || publication.schema_version != SKILL_SCHEMA_VERSION
            || publication.content_sha256 != revision.package.content_sha256
            || publication.base_revision != revision.meta.base_revision
        {
            return Err(SkillError::RevisionCorrupted);
        }
        if let Some(proposal_id) = &publication.proposal {
            let proposal = skill
                .proposals
                .get(proposal_id)
                .ok_or(SkillError::SyncConflict)?;
            if proposal.meta.status != ProposalStatus::Published
                || proposal.meta.candidate_revision != *id
            {
                return Err(SkillError::SyncConflict);
            }
        }
    }
    let current = skill
        .revisions
        .get(&skill.meta.current_revision)
        .ok_or(SkillError::RevisionNotFound)?;
    let publication = skill
        .publications
        .get(&skill.meta.current_revision)
        .ok_or(SkillError::RevisionUnpublished)?;
    if current.package.content_sha256 != publication.content_sha256 {
        return Err(SkillError::RevisionCorrupted);
    }

    let mut open_ids = Vec::new();
    for (id, proposal) in &skill.proposals {
        if id != &proposal.meta.id
            || &proposal.meta.skill != slug
            || proposal.meta.schema_version != SKILL_SCHEMA_VERSION
            || !valid_bounded_text(&proposal.meta.summary, 500)
            || !skill
                .revisions
                .contains_key(&proposal.meta.candidate_revision)
            || !skill
                .publications
                .contains_key(&proposal.meta.base_revision)
            || skill.revisions[&proposal.meta.candidate_revision]
                .meta
                .base_revision
                .as_ref()
                != Some(&proposal.meta.base_revision)
        {
            return Err(SkillError::SyncConflict);
        }
        match proposal.meta.status {
            ProposalStatus::Open => {
                if proposal.meta.resolved_by.is_some()
                    || proposal.meta.resolved_at.is_some()
                    || skill
                        .publications
                        .contains_key(&proposal.meta.candidate_revision)
                {
                    return Err(SkillError::SyncConflict);
                }
                open_ids.push(id.clone());
            }
            ProposalStatus::Published => {
                if proposal.meta.resolved_by.is_none()
                    || proposal.meta.resolved_at.is_none()
                    || skill
                        .publications
                        .get(&proposal.meta.candidate_revision)
                        .and_then(|publication| publication.proposal.as_ref())
                        != Some(id)
                {
                    return Err(SkillError::SyncConflict);
                }
            }
            ProposalStatus::Rejected | ProposalStatus::Withdrawn => {
                if proposal.meta.resolved_by.is_none()
                    || proposal.meta.resolved_at.is_none()
                    || skill
                        .publications
                        .contains_key(&proposal.meta.candidate_revision)
                {
                    return Err(SkillError::SyncConflict);
                }
            }
        }
    }
    sort_proposal_ids(&mut open_ids);
    if skill.meta.open_proposal_ids != open_ids
        || usize::from(skill.meta.open_proposal_count) != open_ids.len()
        || open_ids.len() > 100
    {
        return Err(SkillError::SyncConflict);
    }
    Ok(())
}

fn expected_edits(
    before: &SkillRepositorySnapshot,
    after: &SkillRepositorySnapshot,
    receipt: &SkillReceipt,
) -> Result<Vec<SkillTreeEdit>, SkillError> {
    let mut edits = Vec::new();
    match receipt.operation {
        SkillOperation::WorkspaceBootstrap => {
            edits.push(upsert_yaml(
                "skills/workspace.meta.yaml",
                after.workspace.as_ref().ok_or(SkillError::SyncConflict)?,
            )?);
        }
        SkillOperation::SkillCreate => {
            let slug = receipt.skill.as_ref().ok_or(SkillError::SyncConflict)?;
            let skill = after
                .active_skills
                .get(slug)
                .ok_or(SkillError::SyncConflict)?;
            let revision = receipt
                .request
                .revision
                .as_ref()
                .ok_or(SkillError::SyncConflict)?;
            push_skill_meta_and_history(&mut edits, slug, skill)?;
            push_revision_edits(&mut edits, slug, revision, &skill.revisions[revision])?;
            edits.push(upsert_yaml(
                &publication_path(slug, revision),
                &skill.publications[revision],
            )?);
        }
        SkillOperation::ProposalCreate => {
            let slug = receipt.skill.as_ref().ok_or(SkillError::SyncConflict)?;
            let skill = after
                .active_skills
                .get(slug)
                .ok_or(SkillError::SyncConflict)?;
            let revision = receipt
                .request
                .candidate_revision
                .as_ref()
                .ok_or(SkillError::SyncConflict)?;
            let proposal = receipt
                .request
                .proposal
                .as_ref()
                .ok_or(SkillError::SyncConflict)?;
            push_skill_meta_and_history(&mut edits, slug, skill)?;
            push_revision_edits(&mut edits, slug, revision, &skill.revisions[revision])?;
            edits.push(upsert_yaml(
                &proposal_meta_path(slug, proposal),
                &skill.proposals[proposal].meta,
            )?);
            edits.push(SkillTreeEdit::Upsert {
                path: proposal_discussion_path(slug, proposal),
                bytes: skill.proposals[proposal].discussion.as_bytes().to_vec(),
            });
        }
        SkillOperation::ProposalPublish
        | SkillOperation::ProposalReject
        | SkillOperation::ProposalWithdraw => {
            let slug = receipt.skill.as_ref().ok_or(SkillError::SyncConflict)?;
            let skill = after
                .active_skills
                .get(slug)
                .ok_or(SkillError::SyncConflict)?;
            let proposal = receipt
                .request
                .proposal
                .as_ref()
                .ok_or(SkillError::SyncConflict)?;
            push_skill_meta_and_history(&mut edits, slug, skill)?;
            edits.push(upsert_yaml(
                &proposal_meta_path(slug, proposal),
                &skill.proposals[proposal].meta,
            )?);
            if receipt.operation == SkillOperation::ProposalPublish {
                let revision = &skill.proposals[proposal].meta.candidate_revision;
                edits.push(upsert_yaml(
                    &publication_path(slug, revision),
                    &skill.publications[revision],
                )?);
            }
        }
        SkillOperation::RepairSkillState => {
            let checkpoint = before
                .conflict_checkpoint
                .as_ref()
                .ok_or(SkillError::SyncConflict)?;
            for path in &checkpoint.changed_paths {
                if let Some(bytes) = checkpoint.accepted_files.get(path) {
                    edits.push(SkillTreeEdit::Upsert {
                        path: path.clone(),
                        bytes: bytes.clone(),
                    });
                } else {
                    edits.push(SkillTreeEdit::Delete { path: path.clone() });
                }
            }
        }
        _ => return Err(SkillError::SyncConflict),
    }
    edits.push(upsert_yaml(
        &receipt_path(&receipt.id),
        after
            .receipts
            .get(&receipt.id)
            .ok_or(SkillError::SyncConflict)?,
    )?);
    edits.sort_by(|left, right| edit_path(left).cmp(edit_path(right)));
    Ok(edits)
}

fn push_skill_meta_and_history(
    edits: &mut Vec<SkillTreeEdit>,
    slug: &SkillSlug,
    skill: &SkillObjectSnapshot,
) -> Result<(), SkillError> {
    edits.push(upsert_yaml(&skill_meta_path(slug), &skill.meta)?);
    edits.push(SkillTreeEdit::Upsert {
        path: skill_history_path(slug),
        bytes: skill.history.as_bytes().to_vec(),
    });
    Ok(())
}

fn push_revision_edits(
    edits: &mut Vec<SkillTreeEdit>,
    slug: &SkillSlug,
    revision: &RevisionId,
    snapshot: &SkillRevisionSnapshot,
) -> Result<(), SkillError> {
    edits.push(upsert_yaml(
        &revision_meta_path(slug, revision),
        &snapshot.meta,
    )?);
    for entry in &snapshot.package.entries {
        edits.push(SkillTreeEdit::Upsert {
            path: format!(
                "skills/{}/revisions/{}/package/{}",
                slug.as_str(),
                revision.as_str(),
                entry.path
            ),
            bytes: entry.bytes.clone(),
        });
    }
    Ok(())
}

fn transition_package<'a>(
    after: &'a SkillRepositorySnapshot,
    receipt: &SkillReceipt,
) -> Result<Option<&'a ValidatedPackage>, SkillError> {
    let revision = match receipt.operation {
        SkillOperation::SkillCreate => receipt.request.revision.as_ref(),
        SkillOperation::ProposalCreate => receipt.request.candidate_revision.as_ref(),
        _ => return Ok(None),
    }
    .ok_or(SkillError::SyncConflict)?;
    let slug = receipt.skill.as_ref().ok_or(SkillError::SyncConflict)?;
    Ok(Some(
        &after
            .active_skills
            .get(slug)
            .ok_or(SkillError::SyncConflict)?
            .revisions
            .get(revision)
            .ok_or(SkillError::RevisionNotFound)?
            .package,
    ))
}

fn append_history(skill: &mut SkillObjectSnapshot, receipt: &SkillReceipt) {
    let line_number = skill
        .history
        .lines()
        .filter(|line| line.starts_with("[L"))
        .count() as u64
        + 1;
    let meta = serde_json::json!({
        "request_id": receipt.id.as_str(),
        "operation": operation_name(receipt.operation),
    });
    skill.history.push_str(&format_event(
        line_number,
        &receipt.actor,
        &receipt.created_at,
        operation_name(receipt.operation),
        &meta,
    ));
}

fn result_for_skill(
    skill: &SkillObjectSnapshot,
    proposal: Option<&SkillProposalMeta>,
) -> SkillMutationResult {
    SkillMutationResult {
        canonical_ref: Some(super::SkillReference {
            slug: skill.meta.slug.clone(),
            revision: Some(skill.meta.current_revision.clone()),
        }),
        current_revision: Some(skill.meta.current_revision.clone()),
        control_revision: Some(skill.meta.control_revision),
        event_revision: Some(skill.meta.event_revision),
        proposal_state_revision: proposal.map(|proposal| proposal.state_revision),
        proposal_status: proposal.map(|proposal| proposal.status),
    }
}

fn empty_result() -> SkillMutationResult {
    SkillMutationResult {
        canonical_ref: None,
        current_revision: None,
        control_revision: None,
        event_revision: None,
        proposal_state_revision: None,
        proposal_status: None,
    }
}

fn find_proposal<'a>(
    snapshot: &'a SkillRepositorySnapshot,
    proposal: &ProposalId,
) -> Option<(&'a SkillSlug, &'a SkillObjectSnapshot, bool)> {
    snapshot
        .active_skills
        .iter()
        .find(|(_, skill)| skill.proposals.contains_key(proposal))
        .map(|(slug, skill)| (slug, skill, false))
        .or_else(|| {
            snapshot
                .archived_skills
                .iter()
                .find(|(_, skill)| skill.proposals.contains_key(proposal))
                .map(|(slug, skill)| (slug, skill, true))
        })
}

fn repair_scope_matches(receipt: &SkillReceipt, accepted: &SkillRepairAcceptedState) -> bool {
    match accepted {
        SkillRepairAcceptedState::Workspace(_) => {
            receipt.scope == SkillReceiptScope::Workspace && receipt.skill.is_none()
        }
        SkillRepairAcceptedState::ActiveSkill { slug, .. }
        | SkillRepairAcceptedState::ArchivedSkill { slug, .. } => {
            receipt.scope == SkillReceiptScope::Skill && receipt.skill.as_ref() == Some(slug)
        }
    }
}

fn valid_repair_checkpoint(
    before: &SkillRepositorySnapshot,
    checkpoint: &SkillConflictCheckpoint,
) -> bool {
    if checkpoint.conflict_tip.is_empty()
        || checkpoint.accepted_tree.is_empty()
        || checkpoint.accepted_files.is_empty()
        || !valid_portable_relative_paths(checkpoint.changed_paths.iter().map(String::as_str))
    {
        return false;
    }
    let expected_paths = match &checkpoint.accepted_state {
        SkillRepairAcceptedState::Workspace(_) => {
            BTreeSet::from(["skills/workspace.meta.yaml".to_owned()])
        }
        SkillRepairAcceptedState::ActiveSkill { slug, skill } => {
            skill_object_paths(&format!("skills/{}", slug.as_str()), skill)
        }
        SkillRepairAcceptedState::ArchivedSkill { slug, skill } => {
            skill_object_paths(&format!("archive/skills/{}", slug.as_str()), skill)
        }
    };
    let accepted_paths: BTreeSet<_> = checkpoint.accepted_files.keys().cloned().collect();
    if accepted_paths != expected_paths {
        return false;
    }
    if checkpoint.changed_paths != exact_repair_raw_diff(before, checkpoint) {
        return false;
    }
    match &checkpoint.accepted_state {
        SkillRepairAcceptedState::Workspace(workspace) => yaml_matches(
            &checkpoint.accepted_files["skills/workspace.meta.yaml"],
            workspace,
        ),
        SkillRepairAcceptedState::ActiveSkill { slug, skill } => accepted_object_bytes_match(
            &format!("skills/{}", slug.as_str()),
            skill,
            &checkpoint.accepted_files,
        ),
        SkillRepairAcceptedState::ArchivedSkill { slug, skill } => accepted_object_bytes_match(
            &format!("archive/skills/{}", slug.as_str()),
            skill,
            &checkpoint.accepted_files,
        ),
    }
}

fn exact_repair_raw_diff(
    before: &SkillRepositorySnapshot,
    checkpoint: &SkillConflictCheckpoint,
) -> BTreeSet<String> {
    let path_in_scope = |path: &str| match &checkpoint.accepted_state {
        SkillRepairAcceptedState::Workspace(_) => path == "skills/workspace.meta.yaml",
        SkillRepairAcceptedState::ActiveSkill { slug, .. } => {
            path.starts_with(&format!("skills/{}/", slug.as_str()))
                || path.starts_with(&format!("archive/skills/{}/", slug.as_str()))
        }
        SkillRepairAcceptedState::ArchivedSkill { slug, .. } => {
            path.starts_with(&format!("skills/{}/", slug.as_str()))
                || path.starts_with(&format!("archive/skills/{}/", slug.as_str()))
        }
    };
    before
        .repository_files
        .keys()
        .chain(checkpoint.accepted_files.keys())
        .filter(|path| path_in_scope(path))
        .filter(|path| before.repository_files.get(*path) != checkpoint.accepted_files.get(*path))
        .cloned()
        .collect()
}

fn skill_object_paths(root: &str, skill: &SkillObjectSnapshot) -> BTreeSet<String> {
    let mut paths = BTreeSet::from([
        format!("{root}/skill.meta.yaml"),
        format!("{root}/history.thread"),
    ]);
    for (revision_id, revision) in &skill.revisions {
        paths.insert(format!(
            "{root}/revisions/{}/revision.meta.yaml",
            revision_id.as_str()
        ));
        for entry in &revision.package.entries {
            paths.insert(format!(
                "{root}/revisions/{}/package/{}",
                revision_id.as_str(),
                entry.path
            ));
        }
    }
    for revision_id in skill.publications.keys() {
        paths.insert(format!(
            "{root}/publications/{}.meta.yaml",
            revision_id.as_str()
        ));
    }
    for proposal_id in skill.proposals.keys() {
        paths.insert(format!(
            "{root}/proposals/{}/proposal.meta.yaml",
            proposal_id.as_str()
        ));
        paths.insert(format!(
            "{root}/proposals/{}/discussion.thread",
            proposal_id.as_str()
        ));
    }
    paths
}

fn accepted_object_bytes_match(
    root: &str,
    skill: &SkillObjectSnapshot,
    files: &BTreeMap<String, Vec<u8>>,
) -> bool {
    if !yaml_matches(&files[&format!("{root}/skill.meta.yaml")], &skill.meta)
        || files[&format!("{root}/history.thread")] != skill.history.as_bytes()
    {
        return false;
    }
    for (revision_id, revision) in &skill.revisions {
        if !yaml_matches(
            &files[&format!(
                "{root}/revisions/{}/revision.meta.yaml",
                revision_id.as_str()
            )],
            &revision.meta,
        ) {
            return false;
        }
        for entry in &revision.package.entries {
            if files[&format!(
                "{root}/revisions/{}/package/{}",
                revision_id.as_str(),
                entry.path
            )] != entry.bytes
            {
                return false;
            }
        }
    }
    for (revision_id, publication) in &skill.publications {
        if !yaml_matches(
            &files[&format!("{root}/publications/{}.meta.yaml", revision_id.as_str())],
            publication,
        ) {
            return false;
        }
    }
    for (proposal_id, proposal) in &skill.proposals {
        if !yaml_matches(
            &files[&format!(
                "{root}/proposals/{}/proposal.meta.yaml",
                proposal_id.as_str()
            )],
            &proposal.meta,
        ) || files[&format!(
            "{root}/proposals/{}/discussion.thread",
            proposal_id.as_str()
        )] != proposal.discussion.as_bytes()
        {
            return false;
        }
    }
    true
}

fn yaml_matches<T>(bytes: &[u8], expected: &T) -> bool
where
    T: serde::de::DeserializeOwned + PartialEq,
{
    serde_yaml::from_slice::<T>(bytes).is_ok_and(|value| &value == expected)
}

fn stale_content(skill: &SkillObjectSnapshot) -> SkillError {
    SkillError::StaleContentRevision {
        current_revision: skill.meta.current_revision.clone(),
        control_revision: skill.meta.control_revision,
        event_revision: skill.meta.event_revision,
    }
}

fn stale_control(skill: &SkillObjectSnapshot) -> SkillError {
    SkillError::StaleControlRevision {
        current_revision: skill.meta.current_revision.clone(),
        control_revision: skill.meta.control_revision,
        event_revision: skill.meta.event_revision,
    }
}

fn stale_proposal(skill: &SkillObjectSnapshot, proposal: &SkillProposalMeta) -> SkillError {
    SkillError::StaleProposalRevision {
        current_revision: skill.meta.current_revision.clone(),
        control_revision: skill.meta.control_revision,
        event_revision: skill.meta.event_revision,
        proposal_status: proposal.status,
        proposal_state_revision: proposal.state_revision,
    }
}

fn contains_handler(handlers: &[Handler], needle: &Handler) -> bool {
    handlers.iter().any(|handler| handler == needle)
}

fn sorted_unique_handlers(handlers: &[Handler]) -> bool {
    handlers
        .windows(2)
        .all(|window| window[0].as_str() < window[1].as_str())
}

fn sort_proposal_ids(ids: &mut [ProposalId]) {
    ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
}

fn validate_bounded_text(value: &str, max_chars: usize) -> Result<(), SkillError> {
    if valid_bounded_text(value, max_chars) {
        Ok(())
    } else {
        Err(SkillError::InvalidPackage)
    }
}

fn valid_bounded_text(value: &str, max_chars: usize) -> bool {
    let count = value.chars().count();
    count > 0 && count <= max_chars
}

fn revision_for_request(request: &RequestId) -> Result<RevisionId, SkillError> {
    RevisionId::new(&format!("r-{}", &request.as_str()[2..]))
}

fn proposal_for_request(request: &RequestId) -> Result<ProposalId, SkillError> {
    ProposalId::new(&format!("p-{}", &request.as_str()[2..]))
}

fn hash_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn upsert_yaml(path: &str, value: &impl Serialize) -> Result<SkillTreeEdit, SkillError> {
    let bytes = serde_yaml::to_string(value)
        .map_err(|_| SkillError::SyncConflict)?
        .into_bytes();
    Ok(SkillTreeEdit::Upsert {
        path: path.to_owned(),
        bytes,
    })
}

fn edit_paths(edits: &[SkillTreeEdit]) -> BTreeSet<String> {
    edits
        .iter()
        .map(|edit| edit_path(edit).to_owned())
        .collect()
}

fn apply_tree_edits(
    before: &BTreeMap<String, Vec<u8>>,
    edits: &[SkillTreeEdit],
) -> BTreeMap<String, Vec<u8>> {
    let mut after = before.clone();
    for edit in edits {
        match edit {
            SkillTreeEdit::Upsert { path, bytes } => {
                after.insert(path.clone(), bytes.clone());
            }
            SkillTreeEdit::Delete { path } => {
                after.remove(path);
            }
        }
    }
    after
}

fn edit_path(edit: &SkillTreeEdit) -> &str {
    match edit {
        SkillTreeEdit::Upsert { path, .. } | SkillTreeEdit::Delete { path } => path,
    }
}

fn skill_meta_path(slug: &SkillSlug) -> String {
    format!("skills/{}/skill.meta.yaml", slug.as_str())
}

fn skill_history_path(slug: &SkillSlug) -> String {
    format!("skills/{}/history.thread", slug.as_str())
}

fn revision_meta_path(slug: &SkillSlug, revision: &RevisionId) -> String {
    format!(
        "skills/{}/revisions/{}/revision.meta.yaml",
        slug.as_str(),
        revision.as_str()
    )
}

fn publication_path(slug: &SkillSlug, revision: &RevisionId) -> String {
    format!(
        "skills/{}/publications/{}.meta.yaml",
        slug.as_str(),
        revision.as_str()
    )
}

fn proposal_meta_path(slug: &SkillSlug, proposal: &ProposalId) -> String {
    format!(
        "skills/{}/proposals/{}/proposal.meta.yaml",
        slug.as_str(),
        proposal.as_str()
    )
}

fn proposal_discussion_path(slug: &SkillSlug, proposal: &ProposalId) -> String {
    format!(
        "skills/{}/proposals/{}/discussion.thread",
        slug.as_str(),
        proposal.as_str()
    )
}

fn receipt_path(request: &RequestId) -> String {
    format!("skills/receipts/{}.meta.yaml", request.as_str())
}

const fn operation_name(operation: SkillOperation) -> &'static str {
    match operation {
        SkillOperation::WorkspaceBootstrap => "workspace_bootstrap",
        SkillOperation::WorkspaceAdminAdd => "workspace_admin_add",
        SkillOperation::WorkspaceAdminRemove => "workspace_admin_remove",
        SkillOperation::SkillCreate => "skill_create",
        SkillOperation::ProposalCreate => "proposal_create",
        SkillOperation::ProposalComment => "proposal_comment",
        SkillOperation::ProposalPublish => "proposal_publish",
        SkillOperation::ProposalReject => "proposal_reject",
        SkillOperation::ProposalWithdraw => "proposal_withdraw",
        SkillOperation::MetadataUpdate => "metadata_update",
        SkillOperation::OwnerAdd => "owner_add",
        SkillOperation::OwnerRemove => "owner_remove",
        SkillOperation::MaintainerAdd => "maintainer_add",
        SkillOperation::MaintainerRemove => "maintainer_remove",
        SkillOperation::Archive => "archive",
        SkillOperation::Unarchive => "unarchive",
        SkillOperation::OwnerRecovered => "owner_recovered",
        SkillOperation::RepairSkillState => "repair_skill_state",
    }
}
