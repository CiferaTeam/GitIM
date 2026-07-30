#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};

use gitim_core::skill::{
    plan_skill_mutation, validate_package_entries, validate_skill_commit, PackageEntry, ProposalId,
    ProposalStatus, RequestId, RevisionId, SkillConflictCheckpoint, SkillError,
    SkillMutationContext, SkillMutationRequest, SkillOperation, SkillProposalMeta,
    SkillProposalSnapshot, SkillProposalTransitionRequest, SkillProposeRequest,
    SkillPublicationMeta, SkillRepairAcceptedState, SkillRepairRequest, SkillRepairScope,
    SkillRepositorySnapshot, SkillRevisionMeta, SkillRevisionSnapshot, SkillSlug, SkillTreeEdit,
    SkillWorkspaceBootstrapRequest, WorkspaceSkillMeta, SKILL_SCHEMA_VERSION,
};
use gitim_core::types::Handler;

const ALICE: &str = "alice";
const BOB: &str = "bob";
const NOW: &str = "2026-07-30T04:20:00Z";

fn request_id(value: char) -> RequestId {
    RequestId::new(&format!("q-01K1D8QG2S8RX4T9M9BDKQ9Z7{value}")).unwrap()
}

fn revision_id(value: char) -> RevisionId {
    RevisionId::new(&format!("r-01K1D8QG2S8RX4T9M9BDKQ9Z7{value}")).unwrap()
}

fn slug() -> SkillSlug {
    SkillSlug::new("release-check").unwrap()
}

fn handler(value: &str) -> Handler {
    Handler::new(value).unwrap()
}

fn package(body: &str) -> gitim_core::skill::ValidatedPackage {
    validate_package_entries(
        &slug(),
        vec![
            PackageEntry::new(
                "SKILL.md",
                format!("---\nname: release-check\ndescription: Verify releases.\n---\n\n{body}\n")
                    .into_bytes(),
            ),
            PackageEntry::new("scripts/check.sh", b"#!/bin/sh\nexit 0\n".to_vec()),
        ],
    )
    .unwrap()
}

fn empty_snapshot() -> SkillRepositorySnapshot {
    SkillRepositorySnapshot {
        workspace: None,
        active_skills: BTreeMap::new(),
        archived_skills: BTreeMap::new(),
        receipts: BTreeMap::new(),
        active_users: BTreeSet::from([ALICE.to_owned(), BOB.to_owned()]),
        conflict_checkpoint: None,
        repository_files: BTreeMap::new(),
    }
}

fn initialized_snapshot() -> SkillRepositorySnapshot {
    let mut snapshot = empty_snapshot();
    snapshot.workspace = Some(WorkspaceSkillMeta {
        schema_version: SKILL_SCHEMA_VERSION,
        administrators: vec![handler(ALICE)],
        control_revision: 1,
        created_at: "2026-07-30T04:00:00Z".to_owned(),
        updated_at: "2026-07-30T04:00:00Z".to_owned(),
    });
    refresh_repository_files(&mut snapshot);
    snapshot
}

fn context(
    actor: &str,
    package: Option<gitim_core::skill::ValidatedPackage>,
) -> SkillMutationContext {
    SkillMutationContext {
        actor: actor.to_owned(),
        now: NOW.to_owned(),
        package,
    }
}

fn create_active_skill() -> SkillRepositorySnapshot {
    let before = initialized_snapshot();
    let request = SkillMutationRequest::Create(gitim_core::skill::SkillCreateRequest {
        request_id: request_id('A'),
        slug: slug(),
        display_name: "Release Check".to_owned(),
        description: "Verify a release candidate.".to_owned(),
        source_directory: "/unused".into(),
    });
    plan_skill_mutation(&before, &context(ALICE, Some(package("initial"))), &request)
        .unwrap()
        .after
}

fn create_skill_with_open_proposal() -> SkillRepositorySnapshot {
    let before = create_active_skill();
    let base_revision = before.active_skills[&slug()].meta.current_revision.clone();
    let request = SkillMutationRequest::Propose(SkillProposeRequest {
        request_id: request_id('B'),
        slug: slug(),
        base_revision,
        summary: "Add rollback verification.".to_owned(),
        source_directory: "/unused".into(),
    });
    plan_skill_mutation(&before, &context(BOB, Some(package("candidate"))), &request)
        .unwrap()
        .after
}

fn only_proposal(snapshot: &SkillRepositorySnapshot) -> ProposalId {
    snapshot.active_skills[&slug()]
        .proposals
        .keys()
        .next()
        .unwrap()
        .clone()
}

#[test]
fn workspace_bootstrap_is_a_valid_single_transition() {
    let before = empty_snapshot();
    let request = SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
        request_id: request_id('0'),
    });

    let plan = plan_skill_mutation(&before, &context(ALICE, None), &request).unwrap();

    assert_eq!(&plan.receipt.id, request.request_id());
    assert_eq!(
        plan.changed_paths,
        BTreeSet::from([
            "skills/workspace.meta.yaml".to_owned(),
            format!(
                "skills/receipts/{}.meta.yaml",
                request.request_id().as_str()
            ),
        ])
    );
    let outcome = validate_skill_commit(&before, &plan.after, &plan.commit_evidence).unwrap();
    assert_eq!(outcome.changed_skill, None);
    assert_eq!(outcome.control_revision, Some(1));
}

#[test]
fn workspace_bootstrap_rejects_an_already_initialized_snapshot() {
    let before = initialized_snapshot();
    let request = SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
        request_id: request_id('S'),
    });

    assert_eq!(
        plan_skill_mutation(&before, &context(ALICE, None), &request),
        Err(SkillError::SyncConflict)
    );
}

#[test]
fn create_atomically_publishes_the_initial_exact_byte_revision() {
    let before = initialized_snapshot();
    let validated = package("initial");
    let request = SkillMutationRequest::Create(gitim_core::skill::SkillCreateRequest {
        request_id: request_id('1'),
        slug: slug(),
        display_name: "Release Check".to_owned(),
        description: "Verify a release candidate.".to_owned(),
        source_directory: "/never/persisted".into(),
    });

    let plan =
        plan_skill_mutation(&before, &context(ALICE, Some(validated.clone())), &request).unwrap();
    let skill = &plan.after.active_skills[&slug()];
    let revision = skill.meta.current_revision.clone();

    assert_eq!(&plan.receipt.id, request.request_id());
    assert!(skill.publications.contains_key(&revision));
    assert_eq!(skill.revisions[&revision].package, validated);
    assert!(plan.edits.iter().any(|edit| matches!(
        edit,
        SkillTreeEdit::Upsert { path, bytes }
            if path.ends_with("/package/SKILL.md") && bytes == &skill.revisions[&revision].package.skill_markdown
    )));
    validate_skill_commit(&before, &plan.after, &plan.commit_evidence).unwrap();
}

#[test]
fn proposal_candidate_is_not_a_publication() {
    let before = create_active_skill();
    let base_revision = before.active_skills[&slug()].meta.current_revision.clone();
    let request = SkillMutationRequest::Propose(SkillProposeRequest {
        request_id: request_id('2'),
        slug: slug(),
        base_revision,
        summary: "Add rollback verification.".to_owned(),
        source_directory: "/unused".into(),
    });

    let plan =
        plan_skill_mutation(&before, &context(BOB, Some(package("candidate"))), &request).unwrap();
    let skill = &plan.after.active_skills[&slug()];
    let candidate = skill
        .revisions
        .keys()
        .find(|revision| {
            !before.active_skills[&slug()]
                .revisions
                .contains_key(*revision)
        })
        .unwrap();

    assert!(!skill.publications.contains_key(candidate));
    assert_eq!(skill.meta.open_proposal_count, 1);
    assert_eq!(skill.meta.open_proposal_ids.len(), 1);
    validate_skill_commit(&before, &plan.after, &plan.commit_evidence).unwrap();
}

fn terminal_request(
    before: &SkillRepositorySnapshot,
    operation: SkillOperation,
    actor: &str,
    request: RequestId,
) -> (SkillMutationContext, SkillMutationRequest) {
    let skill = &before.active_skills[&slug()];
    let proposal_id = only_proposal(before);
    let proposal = &skill.proposals[&proposal_id];
    (
        context(actor, None),
        SkillMutationRequest::ProposalTransition(SkillProposalTransitionRequest {
            request_id: request,
            proposal_id,
            operation,
            expected_state_revision: proposal.meta.state_revision,
            expected_control_revision: (operation == SkillOperation::ProposalPublish)
                .then_some(skill.meta.control_revision),
        }),
    )
}

#[test]
fn publish_reject_and_withdraw_are_valid_terminal_transitions() {
    for (operation, actor, suffix, expected_status) in [
        (
            SkillOperation::ProposalPublish,
            ALICE,
            '3',
            ProposalStatus::Published,
        ),
        (
            SkillOperation::ProposalReject,
            ALICE,
            '4',
            ProposalStatus::Rejected,
        ),
        (
            SkillOperation::ProposalWithdraw,
            BOB,
            '5',
            ProposalStatus::Withdrawn,
        ),
    ] {
        let before = create_skill_with_open_proposal();
        let proposal_id = only_proposal(&before);
        let candidate = before.active_skills[&slug()].proposals[&proposal_id]
            .meta
            .candidate_revision
            .clone();
        let (context, request) = terminal_request(&before, operation, actor, request_id(suffix));

        let plan = plan_skill_mutation(&before, &context, &request).unwrap();
        let skill = &plan.after.active_skills[&slug()];
        let proposal = &skill.proposals[&proposal_id];

        assert_eq!(proposal.meta.status, expected_status);
        assert_eq!(skill.meta.open_proposal_count, 0);
        assert!(!skill.meta.open_proposal_ids.contains(&proposal_id));
        assert_eq!(
            skill.publications.contains_key(&candidate),
            operation == SkillOperation::ProposalPublish
        );
        validate_skill_commit(&before, &plan.after, &plan.commit_evidence).unwrap();
    }
}

#[test]
fn matching_retry_returns_recorded_result_without_a_transition() {
    let before = initialized_snapshot();
    let validated = package("initial");
    let request = SkillMutationRequest::Create(gitim_core::skill::SkillCreateRequest {
        request_id: request_id('6'),
        slug: slug(),
        display_name: "Release Check".to_owned(),
        description: "Verify a release candidate.".to_owned(),
        source_directory: "/first/path".into(),
    });
    let first =
        plan_skill_mutation(&before, &context(ALICE, Some(validated.clone())), &request).unwrap();
    let retry_request = SkillMutationRequest::Create(gitim_core::skill::SkillCreateRequest {
        request_id: request_id('6'),
        slug: slug(),
        display_name: "Release Check".to_owned(),
        description: "Verify a release candidate.".to_owned(),
        source_directory: "/different/local/path".into(),
    });

    let retry = plan_skill_mutation(
        &first.after,
        &context(ALICE, Some(validated)),
        &retry_request,
    )
    .unwrap();

    assert_eq!(retry.result, first.result);
    assert!(retry.edits.is_empty());
    assert!(retry.changed_paths.is_empty());
    assert_eq!(retry.after, first.after);
}

#[test]
fn existing_request_id_conflicts_before_actor_or_package_validation() {
    let before = initialized_snapshot();
    let request = SkillMutationRequest::Create(gitim_core::skill::SkillCreateRequest {
        request_id: request_id('V'),
        slug: slug(),
        display_name: "Release Check".to_owned(),
        description: "Verify a release candidate.".to_owned(),
        source_directory: "/unused".into(),
    });
    let first =
        plan_skill_mutation(&before, &context(ALICE, Some(package("initial"))), &request).unwrap();

    assert_eq!(
        plan_skill_mutation(
            &first.after,
            &context("INVALID ACTOR", Some(package("initial"))),
            &request,
        ),
        Err(SkillError::RequestIdConflict)
    );
    assert_eq!(
        plan_skill_mutation(&first.after, &context(ALICE, None), &request),
        Err(SkillError::RequestIdConflict)
    );
}

#[test]
fn matching_retry_returns_recorded_result_when_the_original_target_is_gone() {
    let before = create_skill_with_open_proposal();
    let (transition_context, request) = terminal_request(
        &before,
        SkillOperation::ProposalReject,
        ALICE,
        request_id('W'),
    );
    let first = plan_skill_mutation(&before, &transition_context, &request).unwrap();
    let mut target_gone = first.after.clone();
    target_gone
        .active_skills
        .get_mut(&slug())
        .unwrap()
        .proposals
        .clear();

    let retry = plan_skill_mutation(&target_gone, &transition_context, &request).unwrap();

    assert_eq!(retry.result, first.result);
    assert!(retry.edits.is_empty());
    assert!(retry.changed_paths.is_empty());
    assert_eq!(retry.after, target_gone);
}

#[test]
fn existing_request_id_conflicts_before_transition_operation_validation() {
    let before = initialized_snapshot();
    let original = SkillMutationRequest::Create(gitim_core::skill::SkillCreateRequest {
        request_id: request_id('X'),
        slug: slug(),
        display_name: "Release Check".to_owned(),
        description: "Verify a release candidate.".to_owned(),
        source_directory: "/unused".into(),
    });
    let first = plan_skill_mutation(
        &before,
        &context(ALICE, Some(package("initial"))),
        &original,
    )
    .unwrap();
    let conflicting = SkillMutationRequest::ProposalTransition(SkillProposalTransitionRequest {
        request_id: request_id('X'),
        proposal_id: ProposalId::new("p-01K1D8QG2S8RX4T9M9BDKQ9Z7Z").unwrap(),
        operation: SkillOperation::SkillCreate,
        expected_state_revision: 1,
        expected_control_revision: None,
    });

    assert_eq!(
        plan_skill_mutation(&first.after, &context(ALICE, None), &conflicting),
        Err(SkillError::RequestIdConflict)
    );
}

#[test]
fn request_id_reuse_with_a_different_fingerprint_conflicts() {
    let before = initialized_snapshot();
    let request = SkillMutationRequest::Create(gitim_core::skill::SkillCreateRequest {
        request_id: request_id('7'),
        slug: slug(),
        display_name: "Release Check".to_owned(),
        description: "First meaning.".to_owned(),
        source_directory: "/unused".into(),
    });
    let first =
        plan_skill_mutation(&before, &context(ALICE, Some(package("initial"))), &request).unwrap();
    let conflicting = SkillMutationRequest::Create(gitim_core::skill::SkillCreateRequest {
        request_id: request_id('7'),
        slug: slug(),
        display_name: "Release Check".to_owned(),
        description: "Different meaning.".to_owned(),
        source_directory: "/unused".into(),
    });

    assert_eq!(
        plan_skill_mutation(
            &first.after,
            &context(ALICE, Some(package("initial"))),
            &conflicting
        ),
        Err(SkillError::RequestIdConflict)
    );
}

#[test]
fn request_id_reuse_conflicts_even_when_the_new_target_does_not_exist() {
    let before = initialized_snapshot();
    let original = SkillMutationRequest::Create(gitim_core::skill::SkillCreateRequest {
        request_id: request_id('R'),
        slug: slug(),
        display_name: "Release Check".to_owned(),
        description: "First meaning.".to_owned(),
        source_directory: "/unused".into(),
    });
    let first = plan_skill_mutation(
        &before,
        &context(ALICE, Some(package("initial"))),
        &original,
    )
    .unwrap();
    let conflicting = SkillMutationRequest::ProposalTransition(SkillProposalTransitionRequest {
        request_id: request_id('R'),
        proposal_id: ProposalId::new("p-01K1D8QG2S8RX4T9M9BDKQ9Z7Z").unwrap(),
        operation: SkillOperation::ProposalReject,
        expected_state_revision: 1,
        expected_control_revision: None,
    });

    assert_eq!(
        plan_skill_mutation(&first.after, &context(ALICE, None), &conflicting),
        Err(SkillError::RequestIdConflict)
    );
}

#[test]
fn stale_errors_include_current_content_control_and_proposal_values() {
    let before = create_skill_with_open_proposal();
    let proposal_id = only_proposal(&before);
    let skill = &before.active_skills[&slug()];
    let proposal = &skill.proposals[&proposal_id];

    let stale_state = SkillMutationRequest::ProposalTransition(SkillProposalTransitionRequest {
        request_id: request_id('8'),
        proposal_id: proposal_id.clone(),
        operation: SkillOperation::ProposalPublish,
        expected_state_revision: proposal.meta.state_revision + 1,
        expected_control_revision: Some(skill.meta.control_revision),
    });
    assert_eq!(
        plan_skill_mutation(&before, &context(ALICE, None), &stale_state),
        Err(SkillError::StaleProposalRevision {
            current_revision: skill.meta.current_revision.clone(),
            control_revision: skill.meta.control_revision,
            event_revision: skill.meta.event_revision,
            proposal_status: ProposalStatus::Open,
            proposal_state_revision: proposal.meta.state_revision,
        })
    );

    let stale_control = SkillMutationRequest::ProposalTransition(SkillProposalTransitionRequest {
        request_id: request_id('9'),
        proposal_id: proposal_id.clone(),
        operation: SkillOperation::ProposalPublish,
        expected_state_revision: proposal.meta.state_revision,
        expected_control_revision: Some(skill.meta.control_revision + 1),
    });
    assert_eq!(
        plan_skill_mutation(&before, &context(ALICE, None), &stale_control),
        Err(SkillError::StaleControlRevision {
            current_revision: skill.meta.current_revision.clone(),
            control_revision: skill.meta.control_revision,
            event_revision: skill.meta.event_revision,
        })
    );

    let mut stale_content_before = before.clone();
    let new_current = revision_id('F');
    let new_package = package("other published");
    let (current_control_revision, current_event_revision) = {
        let active = stale_content_before.active_skills.get_mut(&slug()).unwrap();
        active.revisions.insert(
            new_current.clone(),
            SkillRevisionSnapshot {
                meta: SkillRevisionMeta {
                    schema_version: SKILL_SCHEMA_VERSION,
                    id: new_current.clone(),
                    skill: slug(),
                    base_revision: Some(active.meta.current_revision.clone()),
                    content_sha256: new_package.content_sha256.clone(),
                    created_by: handler(ALICE),
                    created_at: NOW.to_owned(),
                },
                package: new_package.clone(),
            },
        );
        active.publications.insert(
            new_current.clone(),
            SkillPublicationMeta {
                schema_version: SKILL_SCHEMA_VERSION,
                skill: slug(),
                revision: new_current.clone(),
                content_sha256: new_package.content_sha256,
                base_revision: Some(active.meta.current_revision.clone()),
                proposal: None,
                published_by: handler(ALICE),
                published_at: NOW.to_owned(),
            },
        );
        active.meta.current_revision = new_current.clone();
        active.meta.control_revision += 1;
        active.meta.event_revision += 1;
        (active.meta.control_revision, active.meta.event_revision)
    };
    refresh_repository_files(&mut stale_content_before);
    let stale_content = SkillMutationRequest::ProposalTransition(SkillProposalTransitionRequest {
        request_id: request_id('C'),
        proposal_id,
        operation: SkillOperation::ProposalPublish,
        expected_state_revision: proposal.meta.state_revision,
        expected_control_revision: Some(current_control_revision),
    });
    assert_eq!(
        plan_skill_mutation(&stale_content_before, &context(ALICE, None), &stale_content),
        Err(SkillError::StaleContentRevision {
            current_revision: new_current,
            control_revision: current_control_revision,
            event_revision: current_event_revision,
        })
    );
}

#[test]
fn permissions_come_from_the_immediately_preceding_snapshot() {
    let before = create_skill_with_open_proposal();
    let (_, publish) = terminal_request(
        &before,
        SkillOperation::ProposalPublish,
        BOB,
        request_id('D'),
    );
    assert_eq!(
        plan_skill_mutation(&before, &context(BOB, None), &publish),
        Err(SkillError::NotMaintainer)
    );

    let mut promoted = before.clone();
    promoted
        .active_skills
        .get_mut(&slug())
        .unwrap()
        .meta
        .maintainers
        .push(handler(BOB));
    refresh_repository_files(&mut promoted);
    let (_, publish) = terminal_request(
        &promoted,
        SkillOperation::ProposalPublish,
        BOB,
        request_id('E'),
    );
    assert!(plan_skill_mutation(&promoted, &context(BOB, None), &publish).is_ok());
}

#[test]
fn archived_skill_rejects_normal_mutations() {
    let mut before = create_active_skill();
    let skill = before.active_skills.remove(&slug()).unwrap();
    before.archived_skills.insert(slug(), skill);
    refresh_repository_files(&mut before);
    let request = SkillMutationRequest::Propose(SkillProposeRequest {
        request_id: request_id('G'),
        slug: slug(),
        base_revision: before.archived_skills[&slug()]
            .meta
            .current_revision
            .clone(),
        summary: "Cannot mutate archived state.".to_owned(),
        source_directory: "/unused".into(),
    });

    assert_eq!(
        plan_skill_mutation(&before, &context(BOB, Some(package("candidate"))), &request),
        Err(SkillError::Archived)
    );
}

fn repair_checkpoint(
    accepted_state: SkillRepairAcceptedState,
    accepted_path: &str,
    accepted_bytes: &[u8],
) -> SkillConflictCheckpoint {
    let mut accepted_files = accepted_state_files(&accepted_state);
    accepted_files.insert(accepted_path.to_owned(), accepted_bytes.to_vec());
    let mut changed_paths = BTreeSet::from([accepted_path.to_owned()]);
    match &accepted_state {
        SkillRepairAcceptedState::ActiveSkill { slug, .. } => {
            changed_paths.insert(format!("skills/{}/rejected-only.meta.yaml", slug.as_str()));
        }
        SkillRepairAcceptedState::ArchivedSkill { slug, .. } => {
            changed_paths.insert(format!(
                "archive/skills/{}/rejected-only.meta.yaml",
                slug.as_str()
            ));
        }
        SkillRepairAcceptedState::Workspace(_) => {}
    }
    SkillConflictCheckpoint {
        conflict_tip: "bad-commit-oid".to_owned(),
        accepted_tree: "accepted-tree-oid".to_owned(),
        accepted_state,
        accepted_files,
        changed_paths,
    }
}

fn accepted_state_files(state: &SkillRepairAcceptedState) -> BTreeMap<String, Vec<u8>> {
    match state {
        SkillRepairAcceptedState::Workspace(workspace) => BTreeMap::from([(
            "skills/workspace.meta.yaml".to_owned(),
            serde_yaml::to_string(workspace).unwrap().into_bytes(),
        )]),
        SkillRepairAcceptedState::ActiveSkill { slug, skill } => {
            object_files(&format!("skills/{}", slug.as_str()), skill)
        }
        SkillRepairAcceptedState::ArchivedSkill { slug, skill } => {
            object_files(&format!("archive/skills/{}", slug.as_str()), skill)
        }
    }
}

fn object_files(
    root: &str,
    skill: &gitim_core::skill::SkillObjectSnapshot,
) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::from([
        (
            format!("{root}/skill.meta.yaml"),
            serde_yaml::to_string(&skill.meta).unwrap().into_bytes(),
        ),
        (
            format!("{root}/history.thread"),
            skill.history.as_bytes().to_vec(),
        ),
    ]);
    for (revision_id, revision) in &skill.revisions {
        files.insert(
            format!(
                "{root}/revisions/{}/revision.meta.yaml",
                revision_id.as_str()
            ),
            serde_yaml::to_string(&revision.meta).unwrap().into_bytes(),
        );
        for entry in &revision.package.entries {
            files.insert(
                format!(
                    "{root}/revisions/{}/package/{}",
                    revision_id.as_str(),
                    entry.path
                ),
                entry.bytes.clone(),
            );
        }
    }
    for (revision_id, publication) in &skill.publications {
        files.insert(
            format!("{root}/publications/{}.meta.yaml", revision_id.as_str()),
            serde_yaml::to_string(publication).unwrap().into_bytes(),
        );
    }
    for (proposal_id, proposal) in &skill.proposals {
        files.insert(
            format!(
                "{root}/proposals/{}/proposal.meta.yaml",
                proposal_id.as_str()
            ),
            serde_yaml::to_string(&proposal.meta).unwrap().into_bytes(),
        );
        files.insert(
            format!(
                "{root}/proposals/{}/discussion.thread",
                proposal_id.as_str()
            ),
            proposal.discussion.as_bytes().to_vec(),
        );
    }
    files
}

fn refresh_repository_files(snapshot: &mut SkillRepositorySnapshot) {
    let mut files = BTreeMap::new();
    if let Some(workspace) = &snapshot.workspace {
        files.insert(
            "skills/workspace.meta.yaml".to_owned(),
            serde_yaml::to_string(workspace).unwrap().into_bytes(),
        );
    }
    for (skill_slug, skill) in &snapshot.active_skills {
        files.extend(object_files(
            &format!("skills/{}", skill_slug.as_str()),
            skill,
        ));
    }
    for (skill_slug, skill) in &snapshot.archived_skills {
        files.extend(object_files(
            &format!("archive/skills/{}", skill_slug.as_str()),
            skill,
        ));
    }
    for (request_id, receipt) in &snapshot.receipts {
        files.insert(
            format!("skills/receipts/{}.meta.yaml", request_id.as_str()),
            serde_yaml::to_string(receipt).unwrap().into_bytes(),
        );
    }
    snapshot.repository_files = files;
}

#[test]
fn tracked_administrator_repairs_workspace_from_checkpoint_exact_bytes() {
    let mut before = initialized_snapshot();
    let accepted = before.workspace.clone().unwrap();
    let mut accepted_bytes = serde_yaml::to_string(&accepted).unwrap().into_bytes();
    accepted_bytes.extend_from_slice(b"# exact accepted checkpoint bytes\n");
    before.conflict_checkpoint = Some(repair_checkpoint(
        SkillRepairAcceptedState::Workspace(accepted),
        "skills/workspace.meta.yaml",
        &accepted_bytes,
    ));
    let request = SkillMutationRequest::Repair(SkillRepairRequest {
        request_id: request_id('H'),
        scope: SkillRepairScope::Workspace,
        conflict_tip: "bad-commit-oid".to_owned(),
        accepted_tree: "accepted-tree-oid".to_owned(),
    });

    let plan = plan_skill_mutation(&before, &context(ALICE, None), &request).unwrap();

    assert_eq!(
        plan.receipt.request.conflict_tip.as_deref(),
        Some("bad-commit-oid")
    );
    assert_eq!(
        plan.receipt.request.accepted_tree.as_deref(),
        Some("accepted-tree-oid")
    );
    assert!(plan.edits.contains(&SkillTreeEdit::Upsert {
        path: "skills/workspace.meta.yaml".to_owned(),
        bytes: accepted_bytes,
    }));
    validate_skill_commit(&before, &plan.after, &plan.commit_evidence).unwrap();
}

#[test]
fn repair_recovers_a_corrupt_workspace_scope_from_the_accepted_checkpoint() {
    let mut before = initialized_snapshot();
    let accepted = before.workspace.clone().unwrap();
    let accepted_bytes = serde_yaml::to_string(&accepted).unwrap().into_bytes();
    before.workspace.as_mut().unwrap().schema_version = SKILL_SCHEMA_VERSION + 1;
    before.conflict_checkpoint = Some(repair_checkpoint(
        SkillRepairAcceptedState::Workspace(accepted.clone()),
        "skills/workspace.meta.yaml",
        &accepted_bytes,
    ));
    let request = SkillMutationRequest::Repair(SkillRepairRequest {
        request_id: request_id('Y'),
        scope: SkillRepairScope::Workspace,
        conflict_tip: "bad-commit-oid".to_owned(),
        accepted_tree: "accepted-tree-oid".to_owned(),
    });

    let plan = plan_skill_mutation(&before, &context(ALICE, None), &request).unwrap();

    assert_eq!(plan.after.workspace, Some(accepted));
    validate_skill_commit(&before, &plan.after, &plan.commit_evidence).unwrap();
}

#[test]
fn repair_recovers_a_corrupt_skill_scope_from_the_accepted_checkpoint() {
    let mut before = create_active_skill();
    let accepted = before.active_skills[&slug()].clone();
    let accepted_bytes = serde_yaml::to_string(&accepted.meta).unwrap().into_bytes();
    before
        .active_skills
        .get_mut(&slug())
        .unwrap()
        .meta
        .open_proposal_count = 1;
    before.conflict_checkpoint = Some(repair_checkpoint(
        SkillRepairAcceptedState::ActiveSkill {
            slug: slug(),
            skill: accepted.clone(),
        },
        "skills/release-check/skill.meta.yaml",
        &accepted_bytes,
    ));
    let request = SkillMutationRequest::Repair(SkillRepairRequest {
        request_id: request_id('Z'),
        scope: SkillRepairScope::Skill(slug()),
        conflict_tip: "bad-commit-oid".to_owned(),
        accepted_tree: "accepted-tree-oid".to_owned(),
    });

    let plan = plan_skill_mutation(&before, &context(ALICE, None), &request).unwrap();

    assert_eq!(plan.after.active_skills[&slug()], accepted);
    validate_skill_commit(&before, &plan.after, &plan.commit_evidence).unwrap();
}

#[test]
fn repair_requires_tracked_admin_and_exact_checkpoint_identity() {
    let mut before = initialized_snapshot();
    let accepted = before.workspace.clone().unwrap();
    let accepted_bytes = serde_yaml::to_string(&accepted).unwrap().into_bytes();
    before.conflict_checkpoint = Some(repair_checkpoint(
        SkillRepairAcceptedState::Workspace(accepted),
        "skills/workspace.meta.yaml",
        &accepted_bytes,
    ));
    let request = SkillMutationRequest::Repair(SkillRepairRequest {
        request_id: request_id('J'),
        scope: SkillRepairScope::Workspace,
        conflict_tip: "bad-commit-oid".to_owned(),
        accepted_tree: "accepted-tree-oid".to_owned(),
    });
    assert_eq!(
        plan_skill_mutation(&before, &context(BOB, None), &request),
        Err(SkillError::AdminRequired)
    );

    let wrong_tree = SkillMutationRequest::Repair(SkillRepairRequest {
        request_id: request_id('K'),
        scope: SkillRepairScope::Workspace,
        conflict_tip: "bad-commit-oid".to_owned(),
        accepted_tree: "different-tree".to_owned(),
    });
    assert_eq!(
        plan_skill_mutation(&before, &context(ALICE, None), &wrong_tree),
        Err(SkillError::SyncConflict)
    );
}

#[test]
fn repair_rejects_checkpoint_bytes_that_do_not_decode_to_the_accepted_state() {
    let mut before = initialized_snapshot();
    let accepted = before.workspace.clone().unwrap();
    before.conflict_checkpoint = Some(repair_checkpoint(
        SkillRepairAcceptedState::Workspace(accepted),
        "skills/workspace.meta.yaml",
        b"administrators: [mallory]\n",
    ));
    let request = SkillMutationRequest::Repair(SkillRepairRequest {
        request_id: request_id('T'),
        scope: SkillRepairScope::Workspace,
        conflict_tip: "bad-commit-oid".to_owned(),
        accepted_tree: "accepted-tree-oid".to_owned(),
    });

    assert_eq!(
        plan_skill_mutation(&before, &context(ALICE, None), &request),
        Err(SkillError::SyncConflict)
    );
}

#[test]
fn tracked_administrator_can_repair_only_the_named_skill_scope() {
    let mut before = create_active_skill();
    let accepted = before.active_skills[&slug()].clone();
    let mut accepted_bytes = serde_yaml::to_string(&accepted.meta).unwrap().into_bytes();
    accepted_bytes.extend_from_slice(b"# exact accepted checkpoint bytes\n");
    before.conflict_checkpoint = Some(repair_checkpoint(
        SkillRepairAcceptedState::ActiveSkill {
            slug: slug(),
            skill: accepted,
        },
        "skills/release-check/skill.meta.yaml",
        &accepted_bytes,
    ));
    let request = SkillMutationRequest::Repair(SkillRepairRequest {
        request_id: request_id('Q'),
        scope: SkillRepairScope::Skill(slug()),
        conflict_tip: "bad-commit-oid".to_owned(),
        accepted_tree: "accepted-tree-oid".to_owned(),
    });

    let plan = plan_skill_mutation(&before, &context(ALICE, None), &request).unwrap();

    assert!(plan.edits.contains(&SkillTreeEdit::Upsert {
        path: "skills/release-check/skill.meta.yaml".to_owned(),
        bytes: accepted_bytes,
    }));
    assert!(plan.edits.contains(&SkillTreeEdit::Delete {
        path: "skills/release-check/rejected-only.meta.yaml".to_owned(),
    }));
    assert_eq!(plan.after.workspace, before.workspace);
    validate_skill_commit(&before, &plan.after, &plan.commit_evidence).unwrap();
}

#[test]
fn repair_moves_a_rejected_active_skill_to_the_accepted_archive_location() {
    let mut before = create_active_skill();
    let accepted = before.active_skills[&slug()].clone();
    let rejected_files = object_files("skills/release-check", &accepted);
    let accepted_state = SkillRepairAcceptedState::ArchivedSkill {
        slug: slug(),
        skill: accepted.clone(),
    };
    let accepted_files = accepted_state_files(&accepted_state);
    let expected_upserts = accepted_files.clone();
    let mut checkpoint = SkillConflictCheckpoint {
        conflict_tip: "bad-commit-oid".to_owned(),
        accepted_tree: "accepted-tree-oid".to_owned(),
        accepted_state,
        accepted_files,
        changed_paths: BTreeSet::new(),
    };
    checkpoint
        .changed_paths
        .extend(checkpoint.accepted_files.keys().cloned());
    checkpoint
        .changed_paths
        .extend(object_files("skills/release-check", &before.active_skills[&slug()]).into_keys());
    before.conflict_checkpoint = Some(checkpoint);
    let request = SkillMutationRequest::Repair(SkillRepairRequest {
        request_id: request_id('C'),
        scope: SkillRepairScope::Skill(slug()),
        conflict_tip: "bad-commit-oid".to_owned(),
        accepted_tree: "accepted-tree-oid".to_owned(),
    });

    let plan = plan_skill_mutation(&before, &context(ALICE, None), &request).unwrap();

    assert!(!plan.after.active_skills.contains_key(&slug()));
    assert_eq!(plan.after.archived_skills[&slug()], accepted);
    assert!(plan.edits.contains(&SkillTreeEdit::Delete {
        path: "skills/release-check/skill.meta.yaml".to_owned(),
    }));
    for (path, bytes) in expected_upserts {
        assert!(plan.edits.contains(&SkillTreeEdit::Upsert { path, bytes }));
    }
    for path in rejected_files.into_keys() {
        assert!(plan.edits.contains(&SkillTreeEdit::Delete { path }));
    }
    validate_skill_commit(&before, &plan.after, &plan.commit_evidence).unwrap();
}

#[test]
fn repair_moves_a_rejected_archived_skill_to_the_accepted_active_location() {
    let mut before = create_active_skill();
    let accepted = before.active_skills.remove(&slug()).unwrap();
    before.archived_skills.insert(slug(), accepted.clone());
    refresh_repository_files(&mut before);
    let accepted_state = SkillRepairAcceptedState::ActiveSkill {
        slug: slug(),
        skill: accepted.clone(),
    };
    let accepted_files = accepted_state_files(&accepted_state);
    let expected_upserts = accepted_files.clone();
    let rejected_files = object_files("archive/skills/release-check", &accepted);
    let mut checkpoint = SkillConflictCheckpoint {
        conflict_tip: "bad-commit-oid".to_owned(),
        accepted_tree: "accepted-tree-oid".to_owned(),
        accepted_state,
        accepted_files,
        changed_paths: BTreeSet::new(),
    };
    checkpoint
        .changed_paths
        .extend(checkpoint.accepted_files.keys().cloned());
    checkpoint.changed_paths.extend(
        object_files(
            "archive/skills/release-check",
            &before.archived_skills[&slug()],
        )
        .into_keys(),
    );
    before.conflict_checkpoint = Some(checkpoint);
    let request = SkillMutationRequest::Repair(SkillRepairRequest {
        request_id: request_id('D'),
        scope: SkillRepairScope::Skill(slug()),
        conflict_tip: "bad-commit-oid".to_owned(),
        accepted_tree: "accepted-tree-oid".to_owned(),
    });

    let plan = plan_skill_mutation(&before, &context(ALICE, None), &request).unwrap();

    assert_eq!(plan.after.active_skills[&slug()], accepted);
    assert!(!plan.after.archived_skills.contains_key(&slug()));
    assert!(plan.edits.contains(&SkillTreeEdit::Delete {
        path: "archive/skills/release-check/skill.meta.yaml".to_owned(),
    }));
    for (path, bytes) in expected_upserts {
        assert!(plan.edits.contains(&SkillTreeEdit::Upsert { path, bytes }));
    }
    for path in rejected_files.into_keys() {
        assert!(plan.edits.contains(&SkillTreeEdit::Delete { path }));
    }
    validate_skill_commit(&before, &plan.after, &plan.commit_evidence).unwrap();
}

#[test]
fn repair_relocation_requires_materializing_every_missing_accepted_file() {
    let mut before = create_active_skill();
    let accepted = before.active_skills[&slug()].clone();
    let accepted_state = SkillRepairAcceptedState::ArchivedSkill {
        slug: slug(),
        skill: accepted,
    };
    let accepted_files = accepted_state_files(&accepted_state);
    let changed_paths = object_files("skills/release-check", &before.active_skills[&slug()])
        .into_keys()
        .collect();
    before.conflict_checkpoint = Some(SkillConflictCheckpoint {
        conflict_tip: "bad-commit-oid".to_owned(),
        accepted_tree: "accepted-tree-oid".to_owned(),
        accepted_state,
        accepted_files,
        changed_paths,
    });
    let request = SkillMutationRequest::Repair(SkillRepairRequest {
        request_id: request_id('F'),
        scope: SkillRepairScope::Skill(slug()),
        conflict_tip: "bad-commit-oid".to_owned(),
        accepted_tree: "accepted-tree-oid".to_owned(),
    });

    assert_eq!(
        plan_skill_mutation(&before, &context(ALICE, None), &request),
        Err(SkillError::SyncConflict)
    );
}

#[test]
fn repair_requires_an_upsert_when_accepted_bytes_differ_at_the_current_path() {
    let mut before = create_active_skill();
    let accepted = before.active_skills[&slug()].clone();
    let mut accepted_bytes = serde_yaml::to_string(&accepted.meta).unwrap().into_bytes();
    accepted_bytes.extend_from_slice(b"# accepted raw bytes\n");
    let mut checkpoint = repair_checkpoint(
        SkillRepairAcceptedState::ActiveSkill {
            slug: slug(),
            skill: accepted,
        },
        "skills/release-check/skill.meta.yaml",
        &accepted_bytes,
    );
    checkpoint
        .changed_paths
        .remove("skills/release-check/skill.meta.yaml");
    before.conflict_checkpoint = Some(checkpoint);
    let request = SkillMutationRequest::Repair(SkillRepairRequest {
        request_id: request_id('G'),
        scope: SkillRepairScope::Skill(slug()),
        conflict_tip: "bad-commit-oid".to_owned(),
        accepted_tree: "accepted-tree-oid".to_owned(),
    });

    assert_eq!(
        plan_skill_mutation(&before, &context(ALICE, None), &request),
        Err(SkillError::SyncConflict)
    );
}

#[test]
fn repair_requires_upsert_for_semantically_equal_nonidentical_current_yaml() {
    let mut before = create_active_skill();
    before
        .repository_files
        .get_mut("skills/release-check/skill.meta.yaml")
        .unwrap()
        .extend_from_slice(b"# current formatting\n");
    let accepted = before.active_skills[&slug()].clone();
    let accepted_bytes = serde_yaml::to_string(&accepted.meta).unwrap().into_bytes();
    let mut checkpoint = repair_checkpoint(
        SkillRepairAcceptedState::ActiveSkill {
            slug: slug(),
            skill: accepted,
        },
        "skills/release-check/skill.meta.yaml",
        &accepted_bytes,
    );
    checkpoint
        .changed_paths
        .remove("skills/release-check/skill.meta.yaml");
    before.conflict_checkpoint = Some(checkpoint.clone());
    let request = SkillMutationRequest::Repair(SkillRepairRequest {
        request_id: request_id('J'),
        scope: SkillRepairScope::Skill(slug()),
        conflict_tip: "bad-commit-oid".to_owned(),
        accepted_tree: "accepted-tree-oid".to_owned(),
    });

    assert_eq!(
        plan_skill_mutation(&before, &context(ALICE, None), &request),
        Err(SkillError::SyncConflict)
    );

    checkpoint
        .changed_paths
        .insert("skills/release-check/skill.meta.yaml".to_owned());
    before.conflict_checkpoint = Some(checkpoint);
    let plan = plan_skill_mutation(&before, &context(ALICE, None), &request).unwrap();
    assert!(plan.edits.contains(&SkillTreeEdit::Upsert {
        path: "skills/release-check/skill.meta.yaml".to_owned(),
        bytes: accepted_bytes,
    }));
}

#[test]
fn commit_validation_rejects_raw_bytes_not_materialized_by_the_plan() {
    let mut before = create_active_skill();
    let accepted = before.active_skills[&slug()].clone();
    let mut accepted_bytes = serde_yaml::to_string(&accepted.meta).unwrap().into_bytes();
    accepted_bytes.extend_from_slice(b"# accepted bytes\n");
    before.conflict_checkpoint = Some(repair_checkpoint(
        SkillRepairAcceptedState::ActiveSkill {
            slug: slug(),
            skill: accepted,
        },
        "skills/release-check/skill.meta.yaml",
        &accepted_bytes,
    ));
    let request = SkillMutationRequest::Repair(SkillRepairRequest {
        request_id: request_id('K'),
        scope: SkillRepairScope::Skill(slug()),
        conflict_tip: "bad-commit-oid".to_owned(),
        accepted_tree: "accepted-tree-oid".to_owned(),
    });
    let plan = plan_skill_mutation(&before, &context(ALICE, None), &request).unwrap();
    let mut tampered_after = plan.after.clone();
    tampered_after
        .repository_files
        .get_mut("skills/release-check/skill.meta.yaml")
        .unwrap()
        .extend_from_slice(b"# unplanned raw change\n");

    assert_eq!(
        validate_skill_commit(&before, &tampered_after, &plan.commit_evidence),
        Err(SkillError::SyncConflict)
    );
}

#[test]
fn repair_relocation_deletes_unknown_raw_files_in_the_rejected_subtree() {
    let mut before = create_active_skill();
    before.repository_files.insert(
        "skills/release-check/unmodeled.bin".to_owned(),
        b"unknown".to_vec(),
    );
    let accepted = before.active_skills[&slug()].clone();
    let accepted_state = SkillRepairAcceptedState::ArchivedSkill {
        slug: slug(),
        skill: accepted,
    };
    let accepted_files = accepted_state_files(&accepted_state);
    let mut changed_paths: BTreeSet<_> = accepted_files.keys().cloned().collect();
    changed_paths.extend(
        before
            .repository_files
            .keys()
            .filter(|path| path.starts_with("skills/release-check/"))
            .cloned(),
    );
    before.conflict_checkpoint = Some(SkillConflictCheckpoint {
        conflict_tip: "bad-commit-oid".to_owned(),
        accepted_tree: "accepted-tree-oid".to_owned(),
        accepted_state,
        accepted_files,
        changed_paths,
    });
    let request = SkillMutationRequest::Repair(SkillRepairRequest {
        request_id: request_id('M'),
        scope: SkillRepairScope::Skill(slug()),
        conflict_tip: "bad-commit-oid".to_owned(),
        accepted_tree: "accepted-tree-oid".to_owned(),
    });

    let plan = plan_skill_mutation(&before, &context(ALICE, None), &request).unwrap();

    assert!(plan.edits.contains(&SkillTreeEdit::Delete {
        path: "skills/release-check/unmodeled.bin".to_owned(),
    }));
    assert!(!plan
        .after
        .repository_files
        .contains_key("skills/release-check/unmodeled.bin"));
}

#[test]
fn repair_rejects_unrelated_paths_inside_the_opposite_skill_location() {
    let mut before = create_active_skill();
    let accepted = before.active_skills[&slug()].clone();
    let accepted_state = SkillRepairAcceptedState::ArchivedSkill {
        slug: slug(),
        skill: accepted,
    };
    let accepted_files = accepted_state_files(&accepted_state);
    let mut changed_paths: BTreeSet<_> = accepted_files.keys().cloned().collect();
    changed_paths
        .extend(object_files("skills/release-check", &before.active_skills[&slug()]).into_keys());
    changed_paths.insert("skills/release-check/unrelated.meta.yaml".to_owned());
    before.conflict_checkpoint = Some(SkillConflictCheckpoint {
        conflict_tip: "bad-commit-oid".to_owned(),
        accepted_tree: "accepted-tree-oid".to_owned(),
        accepted_state,
        accepted_files,
        changed_paths,
    });
    let request = SkillMutationRequest::Repair(SkillRepairRequest {
        request_id: request_id('E'),
        scope: SkillRepairScope::Skill(slug()),
        conflict_tip: "bad-commit-oid".to_owned(),
        accepted_tree: "accepted-tree-oid".to_owned(),
    });

    assert_eq!(
        plan_skill_mutation(&before, &context(ALICE, None), &request),
        Err(SkillError::SyncConflict)
    );
}

#[test]
fn repair_rejects_non_normalized_changed_paths_before_planning_deletes() {
    let before = create_active_skill();
    let accepted = before.active_skills[&slug()].clone();
    let accepted_state = SkillRepairAcceptedState::ActiveSkill {
        slug: slug(),
        skill: accepted,
    };
    let accepted_files = accepted_state_files(&accepted_state);
    let request = SkillMutationRequest::Repair(SkillRepairRequest {
        request_id: request_id('H'),
        scope: SkillRepairScope::Skill(slug()),
        conflict_tip: "bad-commit-oid".to_owned(),
        accepted_tree: "accepted-tree-oid".to_owned(),
    });

    for malicious_path in [
        "skills/release-check/../../channels/general.thread",
        "skills/release-check/./outside.meta.yaml",
        "skills/release-check//outside.meta.yaml",
        "skills/release-check/trailing/",
        "skills/release-check/\0outside.meta.yaml",
        "skills/release-check/\u{001f}outside.meta.yaml",
        "skills\\release-check\\outside.meta.yaml",
        "/skills/release-check/outside.meta.yaml",
        "C:/skills/release-check/outside.meta.yaml",
        "skills/release-check/file:name",
        "skills/release-check/CON",
        "skills/release-check/con.txt",
        "skills/release-check/name.",
        "skills/release-check/name ",
    ] {
        let mut attempt = before.clone();
        attempt.conflict_checkpoint = Some(SkillConflictCheckpoint {
            conflict_tip: "bad-commit-oid".to_owned(),
            accepted_tree: "accepted-tree-oid".to_owned(),
            accepted_state: accepted_state.clone(),
            accepted_files: accepted_files.clone(),
            changed_paths: BTreeSet::from([malicious_path.to_owned()]),
        });

        assert_eq!(
            plan_skill_mutation(&attempt, &context(ALICE, None), &request),
            Err(SkillError::SyncConflict),
            "unsafe changed path must be rejected: {malicious_path:?}"
        );
    }

    let mut case_fold_attempt = before.clone();
    case_fold_attempt.conflict_checkpoint = Some(SkillConflictCheckpoint {
        conflict_tip: "bad-commit-oid".to_owned(),
        accepted_tree: "accepted-tree-oid".to_owned(),
        accepted_state,
        accepted_files,
        changed_paths: BTreeSet::from([
            "skills/release-check/File.md".to_owned(),
            "skills/release-check/file.md".to_owned(),
        ]),
    });
    assert_eq!(
        plan_skill_mutation(&case_fold_attempt, &context(ALICE, None), &request),
        Err(SkillError::SyncConflict)
    );
}

#[test]
fn commit_validation_rejects_merges_wrong_author_trailer_receipt_and_paths() {
    let before = initialized_snapshot();
    let request = SkillMutationRequest::Create(gitim_core::skill::SkillCreateRequest {
        request_id: request_id('M'),
        slug: slug(),
        display_name: "Release Check".to_owned(),
        description: "Verify releases.".to_owned(),
        source_directory: "/unused".into(),
    });
    let plan =
        plan_skill_mutation(&before, &context(ALICE, Some(package("initial"))), &request).unwrap();

    let mut merge = plan.commit_evidence.clone();
    merge.parent_count = 2;
    assert_eq!(
        validate_skill_commit(&before, &plan.after, &merge),
        Err(SkillError::SyncConflict)
    );

    let mut wrong_author = plan.commit_evidence.clone();
    wrong_author.commit_author = BOB.to_owned();
    assert_eq!(
        validate_skill_commit(&before, &plan.after, &wrong_author),
        Err(SkillError::SyncConflict)
    );

    let mut wrong_trailer = plan.commit_evidence.clone();
    wrong_trailer.request_trailer = request_id('N');
    assert_eq!(
        validate_skill_commit(&before, &plan.after, &wrong_trailer),
        Err(SkillError::SyncConflict)
    );

    let mut wrong_receipt = plan.commit_evidence.clone();
    wrong_receipt.receipt.actor = handler(BOB);
    assert_eq!(
        validate_skill_commit(&before, &plan.after, &wrong_receipt),
        Err(SkillError::SyncConflict)
    );

    let mut unrelated_path = plan.commit_evidence.clone();
    unrelated_path
        .changed_paths
        .insert("channels/general/meta.yaml".to_owned());
    assert_eq!(
        validate_skill_commit(&before, &plan.after, &unrelated_path),
        Err(SkillError::SyncConflict)
    );
}

#[test]
fn invariant_validation_rejects_candidate_publication_without_publish_transition() {
    let before = create_active_skill();
    let base_revision = before.active_skills[&slug()].meta.current_revision.clone();
    let request = SkillMutationRequest::Propose(SkillProposeRequest {
        request_id: request_id('P'),
        slug: slug(),
        base_revision,
        summary: "Candidate only.".to_owned(),
        source_directory: "/unused".into(),
    });
    let plan =
        plan_skill_mutation(&before, &context(BOB, Some(package("candidate"))), &request).unwrap();
    let mut tampered = plan.after.clone();
    let skill = tampered.active_skills.get_mut(&slug()).unwrap();
    let proposal = skill.proposals.values().next().unwrap();
    let candidate = proposal.meta.candidate_revision.clone();
    skill.publications.insert(
        candidate.clone(),
        SkillPublicationMeta {
            schema_version: SKILL_SCHEMA_VERSION,
            skill: slug(),
            revision: candidate.clone(),
            content_sha256: skill.revisions[&candidate].package.content_sha256.clone(),
            base_revision: Some(skill.meta.current_revision.clone()),
            proposal: Some(proposal.meta.id.clone()),
            published_by: handler(ALICE),
            published_at: NOW.to_owned(),
        },
    );

    assert_eq!(
        validate_skill_commit(&before, &tampered, &plan.commit_evidence),
        Err(SkillError::SyncConflict)
    );
}

#[allow(dead_code)]
fn proposal_snapshot(meta: SkillProposalMeta) -> SkillProposalSnapshot {
    SkillProposalSnapshot {
        meta,
        discussion: String::new(),
    }
}
