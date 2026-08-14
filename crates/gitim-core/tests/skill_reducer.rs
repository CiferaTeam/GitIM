#![allow(clippy::expect_used, clippy::unwrap_used)]

use gitim_core::skill::{
    reduce_skill, EventId, ProposalId, ProposalStatus, RevisionId, SkillError, SkillEvent,
    SkillEventKind, SkillRevisionMeta, SkillSlug, SKILL_SCHEMA_VERSION,
};
use gitim_core::types::Handler;

const R1: &str = "r-01K1D8QG2S8RX4T9M9BDKQ9Z7N";
const R2: &str = "r-01K1D8QG2S8RX4T9M9BDKQ9Z7P";
const R3: &str = "r-01K1D8QG2S8RX4T9M9BDKQ9Z7Q";
const P1: &str = "p-01K1D8QG2S8RX4T9M9BDKQ9Z7N";
const P2: &str = "p-01K1D8QG2S8RX4T9M9BDKQ9Z7P";

fn slug() -> SkillSlug {
    SkillSlug::new("release-check").expect("slug")
}

fn handler(value: &str) -> Handler {
    Handler::new(value).expect("handler")
}

fn revision(id: &str, base: Option<&str>, author: &str) -> SkillRevisionMeta {
    SkillRevisionMeta {
        schema_version: SKILL_SCHEMA_VERSION,
        id: RevisionId::new(id).expect("revision id"),
        skill: slug(),
        base_revision: base.map(|value| RevisionId::new(value).expect("base revision")),
        content_sha256: format!("{:0>64}", &id[id.len() - 1..]),
        resources: Vec::new(),
        created_by: handler(author),
        created_at: "2026-08-02T10:00:00Z".to_string(),
    }
}

fn event(sequence: char, actor: &str, kind: SkillEventKind) -> SkillEvent {
    let id = format!("e-01K1D8QG2S8RX4T9M9BDKQ9Z7{sequence}");
    SkillEvent {
        schema_version: SKILL_SCHEMA_VERSION,
        id: EventId::new(&id).expect("event id"),
        skill: slug(),
        actor: handler(actor),
        created_at: format!("2026-08-02T10:00:{sequence}Z"),
        kind,
    }
}

fn created() -> SkillEvent {
    event(
        'N',
        "alice",
        SkillEventKind::Created {
            display_name: "Release Check".to_string(),
            description: "Verify a release candidate.".to_string(),
            revision: RevisionId::new(R1).expect("revision"),
        },
    )
}

fn proposal_opened(sequence: char, actor: &str, proposal: &str, revision: &str) -> SkillEvent {
    event(
        sequence,
        actor,
        SkillEventKind::ProposalOpened {
            proposal: ProposalId::new(proposal).expect("proposal"),
            revision: RevisionId::new(revision).expect("revision"),
            base_revision: RevisionId::new(R1).expect("base"),
            summary: format!("Improve {revision}"),
        },
    )
}

#[test]
fn create_derives_initial_state() {
    let state = reduce_skill(&slug(), &[revision(R1, None, "alice")], vec![created()])
        .expect("reduced state");

    assert_eq!(state.display_name, "Release Check");
    assert_eq!(state.current_revision.as_str(), R1);
    assert_eq!(state.created_by.as_str(), "alice");
    assert_eq!(state.owners, vec![handler("alice")]);
    assert_eq!(state.maintainers, vec![handler("alice")]);
    assert_eq!(
        state.published_revisions,
        vec![RevisionId::new(R1).expect("r1")]
    );
    assert!(!state.archived);
    assert!(state.history[0].effective);
}

#[test]
fn events_are_reduced_in_id_order() {
    let update = event(
        'P',
        "alice",
        SkillEventKind::MetadataUpdated {
            display_name: Some("Release Gate".to_string()),
            description: None,
        },
    );
    let state = reduce_skill(
        &slug(),
        &[revision(R1, None, "alice")],
        vec![update, created()],
    )
    .expect("state");
    assert_eq!(state.display_name, "Release Gate");
    assert_eq!(state.history[0].event.id.as_str(), created().id.as_str());
}

#[test]
fn proposal_publish_advances_the_current_revision() {
    let opened = proposal_opened('P', "bob", P1, R2);
    let published = event(
        'Q',
        "alice",
        SkillEventKind::ProposalPublished {
            proposal: ProposalId::new(P1).expect("proposal"),
            expected_current_revision: RevisionId::new(R1).expect("revision"),
        },
    );
    let state = reduce_skill(
        &slug(),
        &[revision(R1, None, "alice"), revision(R2, Some(R1), "bob")],
        vec![published, created(), opened],
    )
    .expect("state");

    assert_eq!(state.current_revision.as_str(), R2);
    assert_eq!(state.published_revisions.len(), 2);
    assert_eq!(state.proposals[0].status, ProposalStatus::Published);
    assert_eq!(
        state.proposals[0].resolved_by.as_ref().map(Handler::as_str),
        Some("alice")
    );
}

#[test]
fn concurrent_proposals_survive_and_competing_publish_has_one_winner() {
    let publish_first = event(
        'R',
        "alice",
        SkillEventKind::ProposalPublished {
            proposal: ProposalId::new(P1).expect("proposal"),
            expected_current_revision: RevisionId::new(R1).expect("revision"),
        },
    );
    let publish_second = event(
        'S',
        "alice",
        SkillEventKind::ProposalPublished {
            proposal: ProposalId::new(P2).expect("proposal"),
            expected_current_revision: RevisionId::new(R1).expect("revision"),
        },
    );
    let state = reduce_skill(
        &slug(),
        &[
            revision(R1, None, "alice"),
            revision(R2, Some(R1), "bob"),
            revision(R3, Some(R1), "carol"),
        ],
        vec![
            publish_second,
            proposal_opened('Q', "carol", P2, R3),
            created(),
            publish_first,
            proposal_opened('P', "bob", P1, R2),
        ],
    )
    .expect("state");

    assert_eq!(state.current_revision.as_str(), R2);
    assert_eq!(state.proposals[0].status, ProposalStatus::Published);
    assert_eq!(state.proposals[1].status, ProposalStatus::Open);
    let losing_publish = state.history.last().expect("last event");
    assert!(!losing_publish.effective);
    assert_eq!(
        losing_publish.reason.as_deref(),
        Some("stale_current_revision")
    );
}

#[test]
fn unauthorized_events_remain_visible_but_ineffective() {
    let update = event(
        'P',
        "bob",
        SkillEventKind::MetadataUpdated {
            display_name: Some("Hijacked".to_string()),
            description: None,
        },
    );
    let add_owner = event(
        'Q',
        "bob",
        SkillEventKind::OwnerAdded {
            handler: handler("bob"),
        },
    );
    let state = reduce_skill(
        &slug(),
        &[revision(R1, None, "alice")],
        vec![created(), add_owner, update],
    )
    .expect("state");

    assert_eq!(state.display_name, "Release Check");
    assert_eq!(state.owners, vec![handler("alice")]);
    assert_eq!(
        state
            .history
            .iter()
            .filter(|entry| !entry.effective)
            .count(),
        2
    );
    assert!(state
        .history
        .iter()
        .filter_map(|entry| entry.reason.as_deref())
        .all(|reason| reason == "not_owner" || reason == "not_maintainer"));
}

#[test]
fn owners_manage_roles_without_removing_the_final_owner() {
    let add_owner = event(
        'P',
        "alice",
        SkillEventKind::OwnerAdded {
            handler: handler("bob"),
        },
    );
    let remove_alice = event(
        'Q',
        "alice",
        SkillEventKind::OwnerRemoved {
            handler: handler("alice"),
            remove_maintainer: true,
        },
    );
    let remove_final = event(
        'R',
        "bob",
        SkillEventKind::OwnerRemoved {
            handler: handler("bob"),
            remove_maintainer: false,
        },
    );
    let state = reduce_skill(
        &slug(),
        &[revision(R1, None, "alice")],
        vec![created(), remove_final, add_owner, remove_alice],
    )
    .expect("state");

    assert_eq!(state.owners, vec![handler("bob")]);
    assert_eq!(state.maintainers, vec![handler("bob")]);
    assert!(!state.history.last().expect("last").effective);
    assert_eq!(
        state
            .history
            .last()
            .and_then(|entry| entry.reason.as_deref()),
        Some("last_owner")
    );
}

#[test]
fn proposal_comments_and_terminal_transitions_follow_permissions() {
    let opened = proposal_opened('P', "bob", P1, R2);
    let comment = event(
        'Q',
        "carol",
        SkillEventKind::ProposalCommented {
            proposal: ProposalId::new(P1).expect("proposal"),
            body: "Please add a rollback check.".to_string(),
        },
    );
    let wrong_withdraw = event(
        'R',
        "carol",
        SkillEventKind::ProposalWithdrawn {
            proposal: ProposalId::new(P1).expect("proposal"),
        },
    );
    let reject = event(
        'S',
        "alice",
        SkillEventKind::ProposalRejected {
            proposal: ProposalId::new(P1).expect("proposal"),
        },
    );
    let late_comment = event(
        'T',
        "bob",
        SkillEventKind::ProposalCommented {
            proposal: ProposalId::new(P1).expect("proposal"),
            body: "Too late".to_string(),
        },
    );
    let state = reduce_skill(
        &slug(),
        &[revision(R1, None, "alice"), revision(R2, Some(R1), "bob")],
        vec![
            created(),
            opened,
            comment,
            wrong_withdraw,
            reject,
            late_comment,
        ],
    )
    .expect("state");

    assert_eq!(state.proposals[0].status, ProposalStatus::Rejected);
    assert_eq!(state.proposals[0].comments.len(), 1);
    assert_eq!(
        state.history[3].reason.as_deref(),
        Some("not_proposal_author")
    );
    assert_eq!(
        state.history[5].reason.as_deref(),
        Some("proposal_terminal")
    );
}

#[test]
fn archived_skill_allows_role_cleanup_and_unarchive() {
    let add_owner = event(
        'P',
        "alice",
        SkillEventKind::OwnerAdded {
            handler: handler("bob"),
        },
    );
    let archive = event('Q', "alice", SkillEventKind::Archived);
    let remove_alice = event(
        'R',
        "bob",
        SkillEventKind::OwnerRemoved {
            handler: handler("alice"),
            remove_maintainer: true,
        },
    );
    let blocked_metadata = event(
        'S',
        "bob",
        SkillEventKind::MetadataUpdated {
            display_name: Some("Archived edit".to_string()),
            description: None,
        },
    );
    let unarchive = event('T', "bob", SkillEventKind::Unarchived);
    let state = reduce_skill(
        &slug(),
        &[revision(R1, None, "alice")],
        vec![
            created(),
            add_owner,
            archive,
            remove_alice,
            blocked_metadata,
            unarchive,
        ],
    )
    .expect("state");

    assert!(!state.archived);
    assert_eq!(state.owners, vec![handler("bob")]);
    assert_eq!(state.display_name, "Release Check");
    assert_eq!(state.history[4].reason.as_deref(), Some("skill_archived"));
}

#[test]
fn missing_or_invalid_creation_is_invalid_history() {
    let update = event(
        'P',
        "alice",
        SkillEventKind::MetadataUpdated {
            display_name: Some("No Skill".to_string()),
            description: None,
        },
    );
    assert_eq!(
        reduce_skill(&slug(), &[revision(R1, None, "alice")], vec![update]),
        Err(SkillError::InvalidHistory)
    );

    let wrong_initial = revision(R1, Some(R2), "alice");
    assert_eq!(
        reduce_skill(&slug(), &[wrong_initial], vec![created()]),
        Err(SkillError::InvalidHistory)
    );
}

#[test]
fn event_yaml_uses_a_flat_stable_type_tag() {
    let yaml = serde_yaml::to_string(&created()).expect("serialize event");
    assert!(yaml.contains("type: created\n"));
    assert!(yaml.contains("revision: r-01K1D8QG2S8RX4T9M9BDKQ9Z7N\n"));
    let decoded: SkillEvent = serde_yaml::from_str(&yaml).expect("deserialize event");
    assert_eq!(decoded, created());
}
