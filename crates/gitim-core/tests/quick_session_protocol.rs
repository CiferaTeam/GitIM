#![allow(clippy::unwrap_used)]

use gitim_core::link::extract_links;
use gitim_core::responses::{
    CreateQuickSessionResponse, ListQuickSessionsResponse, QuickSessionDetail,
    QuickSessionListItem, SendQuickSessionMessageResponse,
};
use gitim_core::types::{
    apply_quick_session_transition, truncate_quick_session_preview,
    validate_quick_session_attempt_id, validate_quick_session_id, validate_quick_session_meta,
    validate_quick_session_summary, validate_quick_session_title, Handler, LinkKind, Message,
    QuickSessionMeta, QuickSessionStatus, QuickSessionTransition, ThreadEntry, TransitionOutcome,
    QUICK_SESSION_PREVIEW_MAX_CHARS, QUICK_SESSION_SUMMARY_MAX_CHARS,
    QUICK_SESSION_TITLE_MAX_CHARS,
};

const SESSION_ID: &str = "qs-01JZZZZZZZZZZZZZZZZZZZZZZZ";
const ATTEMPT_ID: &str = "qa-01JZZZZZZZZZZZZZZZZZZZZZZZ";

fn meta_fixture() -> QuickSessionMeta {
    QuickSessionMeta::new(
        SESSION_ID.to_string(),
        "alice".to_string(),
        "lewis".to_string(),
        "2026-07-11T00:00:00Z".to_string(),
    )
}

fn human(line_number: u64, request_id: Option<&str>, now: &str) -> QuickSessionTransition {
    QuickSessionTransition::HumanMessage {
        actor: "lewis".to_string(),
        line_number,
        request_id: request_id.map(str::to_string),
        preview: "Please investigate the flaky test".to_string(),
        now: now.to_string(),
    }
}

fn claim(input_line: u64, attempt_id: &str, now: &str) -> QuickSessionTransition {
    QuickSessionTransition::Claim {
        actor: "alice".to_string(),
        input_line,
        attempt_id: attempt_id.to_string(),
        now: now.to_string(),
    }
}

fn accept_human(meta: &mut QuickSessionMeta, line_number: u64) {
    apply_quick_session_transition(
        meta,
        human(line_number, Some("setup-request"), "2026-07-11T00:00:00Z"),
    )
    .unwrap();
}

fn title(attempt_id: &str, value: &str, now: &str) -> QuickSessionTransition {
    QuickSessionTransition::SetTitle {
        actor: "alice".to_string(),
        attempt_id: attempt_id.to_string(),
        title: value.to_string(),
        now: now.to_string(),
    }
}

fn reply(input_line: u64, attempt_id: &str, output_line: u64, now: &str) -> QuickSessionTransition {
    QuickSessionTransition::AgentReply {
        actor: "alice".to_string(),
        input_line,
        attempt_id: attempt_id.to_string(),
        output_line,
        preview: "The flaky test is fixed".to_string(),
        now: now.to_string(),
    }
}

#[test]
fn quick_session_ids_enforce_crockford_and_path_safety() {
    assert!(validate_quick_session_id(SESSION_ID).is_ok());
    assert!(validate_quick_session_attempt_id(ATTEMPT_ID).is_ok());

    for invalid in [
        "qs-01JZZZZZZZZZZZZZZZZZZZZZZ",
        "qs-01JZZZZZZZZZZZZZZZZZZZZZZZZ",
        "qa-01JZZZZZZZZZZZZZZZZZZZZZZZ",
        "qs-01jZZZZZZZZZZZZZZZZZZZZZZZ",
        "qs-01IZZZZZZZZZZZZZZZZZZZZZZZ",
        "qs-01LZZZZZZZZZZZZZZZZZZZZZZZ",
        "qs-01OZZZZZZZZZZZZZZZZZZZZZZZ",
        "qs-01UZZZZZZZZZZZZZZZZZZZZZZZ",
        "qs-01/ZZZZZZZZZZZZZZZZZZZZZZZ",
        "qs-01-ZZZZZZZZZZZZZZZZZZZZZZZ",
        "../qs-01JZZZZZZZZZZZZZZZZZZZZZZZ",
    ] {
        assert!(validate_quick_session_id(invalid).is_err(), "{invalid}");
    }

    assert!(validate_quick_session_attempt_id(SESSION_ID).is_err());
    assert!(validate_quick_session_attempt_id("qa-01JZZZZZZZZZZZZZZZZZZZZZZI").is_err());
}

#[test]
fn quick_session_unicode_limits_and_preview_are_scalar_safe() {
    assert!(validate_quick_session_title(&"界".repeat(QUICK_SESSION_TITLE_MAX_CHARS)).is_ok());
    assert!(validate_quick_session_title(&"界".repeat(QUICK_SESSION_TITLE_MAX_CHARS + 1)).is_err());
    assert!(validate_quick_session_title("   ").is_err());

    assert!(validate_quick_session_summary(&"🦀".repeat(QUICK_SESSION_SUMMARY_MAX_CHARS)).is_ok());
    assert!(
        validate_quick_session_summary(&"🦀".repeat(QUICK_SESSION_SUMMARY_MAX_CHARS + 1)).is_err()
    );
    assert!(validate_quick_session_summary("\n\t").is_err());

    let preview = truncate_quick_session_preview(&"好".repeat(QUICK_SESSION_PREVIEW_MAX_CHARS + 5));
    assert_eq!(preview.chars().count(), QUICK_SESSION_PREVIEW_MAX_CHARS);
    assert_eq!(preview, "好".repeat(QUICK_SESSION_PREVIEW_MAX_CHARS));
}

#[test]
fn quick_session_meta_constructor_and_serde_defaults_are_canonical() {
    let meta = meta_fixture();
    assert_eq!(meta.status, QuickSessionStatus::NeedsTitle);
    assert_eq!(meta.revision, 1);
    assert_eq!(meta.updated_at, meta.created_at);
    assert!(validate_quick_session_meta(&meta).is_ok());

    let yaml = format!(
        "id: {SESSION_ID}\nagent_id: alice\ncreated_by: lewis\ncreated_at: 2026-07-11T00:00:00Z\nupdated_at: 2026-07-11T00:00:00Z\n"
    );
    let legacy: QuickSessionMeta = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(legacy.status, QuickSessionStatus::NeedsTitle);
    assert_eq!(legacy.revision, 1);
    assert_eq!(legacy.last_message_preview, "");
    assert_eq!(legacy.last_failed_attempt_id, None);
    assert!(validate_quick_session_meta(&legacy).is_ok());

    // Metadata written before the title_source field was removed must still parse.
    let with_legacy_field = format!(
        "id: {SESSION_ID}\ntitle: Investigate auth\ntitle_source: api_set\nstatus: active\nagent_id: alice\ncreated_by: lewis\ncreated_at: 2026-07-11T00:00:00Z\nupdated_at: 2026-07-11T00:00:00Z\n"
    );
    let upgraded: QuickSessionMeta = serde_yaml::from_str(&with_legacy_field).unwrap();
    assert_eq!(upgraded.title.as_deref(), Some("Investigate auth"));
    assert!(validate_quick_session_meta(&upgraded).is_ok());

    let json = serde_json::to_value(&meta).unwrap();
    assert_eq!(json["status"], "needs_title");
    assert!(json.get("title_source").is_none());
}

#[test]
fn quick_session_meta_validation_rejects_impossible_combinations() {
    let mut invalid = meta_fixture();
    invalid.revision = 0;
    assert!(validate_quick_session_meta(&invalid).is_err());

    let mut invalid = meta_fixture();
    invalid.title = Some("A title".to_string());
    assert!(validate_quick_session_meta(&invalid).is_err());

    let mut invalid = meta_fixture();
    invalid.status = QuickSessionStatus::Running;
    assert!(validate_quick_session_meta(&invalid).is_err());

    let mut invalid = meta_fixture();
    invalid.status = QuickSessionStatus::Archived;
    invalid.archived_from = Some(QuickSessionStatus::Running);
    assert!(validate_quick_session_meta(&invalid).is_err());

    let mut invalid = meta_fixture();
    invalid.status = QuickSessionStatus::Archived;
    invalid.archived_from = Some(QuickSessionStatus::Archived);
    assert!(validate_quick_session_meta(&invalid).is_err());
}

#[test]
fn quick_session_meta_enforces_stable_status_and_line_invariants() {
    let mut archived_without_source = meta_fixture();
    archived_without_source.status = QuickSessionStatus::Archived;
    archived_without_source.archived_at = Some("2026-07-11T00:00:01Z".to_string());
    assert!(validate_quick_session_meta(&archived_without_source).is_err());

    let mut active_without_title = meta_fixture();
    active_without_title.status = QuickSessionStatus::Active;
    assert!(validate_quick_session_meta(&active_without_title).is_err());

    let mut needs_title_with_title = meta_fixture();
    needs_title_with_title.title = Some("Already titled".to_string());
    assert!(validate_quick_session_meta(&needs_title_with_title).is_err());

    let mut archived_wrong_source = meta_fixture();
    archived_wrong_source.title = Some("Already titled".to_string());
    archived_wrong_source.status = QuickSessionStatus::Archived;
    archived_wrong_source.archived_at = Some("2026-07-11T00:00:01Z".to_string());
    archived_wrong_source.archived_from = Some(QuickSessionStatus::NeedsTitle);
    assert!(validate_quick_session_meta(&archived_wrong_source).is_err());

    let mut unknown_processing_line = meta_fixture();
    unknown_processing_line.status = QuickSessionStatus::Running;
    unknown_processing_line.processing_input_line = Some(3);
    unknown_processing_line.processing_started_at = Some("2026-07-11T00:00:01Z".to_string());
    unknown_processing_line.attempt_id = Some(ATTEMPT_ID.to_string());
    unknown_processing_line.last_human_line = Some(2);
    assert!(validate_quick_session_meta(&unknown_processing_line).is_err());

    let mut completion_after_known_human = meta_fixture();
    completion_after_known_human.last_human_line = Some(2);
    completion_after_known_human.last_completed_attempt_id = Some(ATTEMPT_ID.to_string());
    completion_after_known_human.last_completed_input_line = Some(3);
    completion_after_known_human.last_completed_line = Some(4);
    assert!(validate_quick_session_meta(&completion_after_known_human).is_err());
}

#[test]
fn quick_session_human_message_is_authorized_idempotent_and_recovers_error() {
    let mut meta = meta_fixture();
    assert_eq!(
        apply_quick_session_transition(
            &mut meta,
            human(1, Some("request-1"), "2026-07-11T00:00:01Z")
        )
        .unwrap(),
        TransitionOutcome::Applied
    );
    assert_eq!(meta.revision, 2);
    assert_eq!(meta.last_human_line, Some(1));

    let revision = meta.revision;
    assert_eq!(
        apply_quick_session_transition(
            &mut meta,
            human(99, Some("request-1"), "2026-07-11T00:00:02Z")
        )
        .unwrap(),
        TransitionOutcome::Duplicate {
            line_number: Some(1)
        }
    );
    assert_eq!(meta.revision, revision);

    let mut wrong_actor = human(2, Some("request-2"), "2026-07-11T00:00:03Z");
    if let QuickSessionTransition::HumanMessage { actor, .. } = &mut wrong_actor {
        *actor = "mallory".to_string();
    }
    assert!(apply_quick_session_transition(&mut meta, wrong_actor).is_err());

    apply_quick_session_transition(&mut meta, claim(1, ATTEMPT_ID, "2026-07-11T00:00:04Z"))
        .unwrap();
    apply_quick_session_transition(
        &mut meta,
        QuickSessionTransition::MarkError {
            actor: "alice".to_string(),
            attempt_id: ATTEMPT_ID.to_string(),
            error: "provider exited".to_string(),
            now: "2026-07-11T00:00:05Z".to_string(),
        },
    )
    .unwrap();
    assert_eq!(meta.status, QuickSessionStatus::Error);

    apply_quick_session_transition(
        &mut meta,
        human(2, Some("request-2"), "2026-07-11T00:00:06Z"),
    )
    .unwrap();
    assert_eq!(meta.status, QuickSessionStatus::NeedsTitle);
    assert_eq!(meta.error, None);
}

#[test]
fn quick_session_human_retry_remains_duplicate_after_archive() {
    let mut meta = meta_fixture();
    apply_quick_session_transition(
        &mut meta,
        human(1, Some("request-1"), "2026-07-11T00:00:01Z"),
    )
    .unwrap();
    apply_quick_session_transition(
        &mut meta,
        QuickSessionTransition::Archive {
            actor: "lewis".to_string(),
            now: "2026-07-11T00:00:02Z".to_string(),
        },
    )
    .unwrap();

    let revision = meta.revision;
    assert_eq!(
        apply_quick_session_transition(
            &mut meta,
            human(9, Some("request-1"), "2026-07-11T00:00:03Z")
        )
        .unwrap(),
        TransitionOutcome::Duplicate {
            line_number: Some(1)
        }
    );
    assert_eq!(meta.status, QuickSessionStatus::Archived);
    assert_eq!(meta.revision, revision);
}

#[test]
fn quick_session_human_lines_never_move_backward() {
    let mut meta = meta_fixture();
    apply_quick_session_transition(
        &mut meta,
        human(2, Some("request-2"), "2026-07-11T00:00:01Z"),
    )
    .unwrap();

    assert!(apply_quick_session_transition(
        &mut meta,
        human(1, Some("request-1"), "2026-07-11T00:00:02Z")
    )
    .is_err());
    assert!(apply_quick_session_transition(
        &mut meta,
        human(2, Some("different-request"), "2026-07-11T00:00:03Z")
    )
    .is_err());
    assert_eq!(meta.last_human_line, Some(2));
}

#[test]
fn quick_session_claim_is_compare_and_set_and_requires_new_input() {
    let mut meta = meta_fixture();
    accept_human(&mut meta, 1);
    apply_quick_session_transition(&mut meta, claim(1, ATTEMPT_ID, "2026-07-11T00:00:01Z"))
        .unwrap();
    assert_eq!(meta.status, QuickSessionStatus::Running);
    assert_eq!(meta.processing_input_line, Some(1));

    let revision = meta.revision;
    assert_eq!(
        apply_quick_session_transition(&mut meta, claim(1, ATTEMPT_ID, "2026-07-11T00:00:02Z"))
            .unwrap(),
        TransitionOutcome::Duplicate { line_number: None }
    );
    assert_eq!(meta.revision, revision);
    assert!(apply_quick_session_transition(
        &mut meta,
        claim(1, "qa-01JYYYYYYYYYYYYYYYYYYYYYYY", "2026-07-11T00:00:03Z")
    )
    .is_err());

    let mut completed = meta_fixture();
    completed.last_human_line = Some(4);
    completed.last_completed_attempt_id = Some(ATTEMPT_ID.to_string());
    completed.last_completed_input_line = Some(4);
    completed.last_completed_line = Some(5);
    assert!(apply_quick_session_transition(
        &mut completed,
        claim(4, ATTEMPT_ID, "2026-07-11T00:00:04Z")
    )
    .is_err());

    let mut wrong_actor = meta_fixture();
    accept_human(&mut wrong_actor, 1);
    let mut transition = claim(1, ATTEMPT_ID, "2026-07-11T00:00:05Z");
    if let QuickSessionTransition::Claim { actor, .. } = &mut transition {
        *actor = "lewis".to_string();
    }
    assert!(apply_quick_session_transition(&mut wrong_actor, transition).is_err());
}

#[test]
fn quick_session_claim_requires_the_latest_known_human_line() {
    let mut meta = meta_fixture();
    apply_quick_session_transition(
        &mut meta,
        human(3, Some("request-3"), "2026-07-11T00:00:01Z"),
    )
    .unwrap();

    assert!(apply_quick_session_transition(
        &mut meta,
        claim(2, ATTEMPT_ID, "2026-07-11T00:00:02Z")
    )
    .is_err());
    assert!(apply_quick_session_transition(
        &mut meta,
        claim(4, ATTEMPT_ID, "2026-07-11T00:00:03Z")
    )
    .is_err());
    assert!(apply_quick_session_transition(
        &mut meta,
        claim(3, ATTEMPT_ID, "2026-07-11T00:00:04Z")
    )
    .is_ok());
}

#[test]
fn quick_session_title_and_summary_are_attempt_bound_and_idempotent() {
    let mut meta = meta_fixture();
    accept_human(&mut meta, 1);
    apply_quick_session_transition(&mut meta, claim(1, ATTEMPT_ID, "2026-07-11T00:00:01Z"))
        .unwrap();

    apply_quick_session_transition(
        &mut meta,
        title(ATTEMPT_ID, "Investigate flaky test", "2026-07-11T00:00:02Z"),
    )
    .unwrap();
    assert_eq!(meta.title.as_deref(), Some("Investigate flaky test"));
    assert_eq!(meta.status, QuickSessionStatus::Running);

    let revision = meta.revision;
    assert_eq!(
        apply_quick_session_transition(
            &mut meta,
            title(ATTEMPT_ID, "Investigate flaky test", "2026-07-11T00:00:03Z")
        )
        .unwrap(),
        TransitionOutcome::Duplicate { line_number: None }
    );
    assert_eq!(meta.revision, revision);

    assert!(apply_quick_session_transition(
        &mut meta,
        title(
            "qa-01JYYYYYYYYYYYYYYYYYYYYYYY",
            "Wrong attempt",
            "2026-07-11T00:00:04Z"
        )
    )
    .is_err());

    apply_quick_session_transition(
        &mut meta,
        QuickSessionTransition::SetSummary {
            actor: "alice".to_string(),
            attempt_id: ATTEMPT_ID.to_string(),
            summary: "The failure is isolated to a clock race.".to_string(),
            now: "2026-07-11T00:00:05Z".to_string(),
        },
    )
    .unwrap();
    assert_eq!(
        meta.summary.as_deref(),
        Some("The failure is isolated to a clock race.")
    );
    assert_eq!(
        meta.summary_updated_at.as_deref(),
        Some("2026-07-11T00:00:05Z")
    );

    let revision = meta.revision;
    assert_eq!(
        apply_quick_session_transition(
            &mut meta,
            QuickSessionTransition::SetSummary {
                actor: "alice".to_string(),
                attempt_id: ATTEMPT_ID.to_string(),
                summary: "The failure is isolated to a clock race.".to_string(),
                now: "2026-07-11T00:00:06Z".to_string(),
            },
        )
        .unwrap(),
        TransitionOutcome::Duplicate { line_number: None }
    );
    assert_eq!(meta.revision, revision);
}

#[test]
fn quick_session_agent_reply_requires_title_attempt_and_claimed_input() {
    let mut meta = meta_fixture();
    accept_human(&mut meta, 1);
    apply_quick_session_transition(&mut meta, claim(1, ATTEMPT_ID, "2026-07-11T00:00:01Z"))
        .unwrap();
    assert!(apply_quick_session_transition(
        &mut meta,
        reply(1, ATTEMPT_ID, 2, "2026-07-11T00:00:02Z")
    )
    .is_err());

    apply_quick_session_transition(
        &mut meta,
        title(ATTEMPT_ID, "Investigate flaky test", "2026-07-11T00:00:03Z"),
    )
    .unwrap();
    assert!(apply_quick_session_transition(
        &mut meta,
        reply(9, ATTEMPT_ID, 10, "2026-07-11T00:00:04Z")
    )
    .is_err());
    assert!(apply_quick_session_transition(
        &mut meta,
        reply(
            1,
            "qa-01JYYYYYYYYYYYYYYYYYYYYYYY",
            2,
            "2026-07-11T00:00:05Z"
        )
    )
    .is_err());

    assert_eq!(
        apply_quick_session_transition(&mut meta, reply(1, ATTEMPT_ID, 2, "2026-07-11T00:00:06Z"))
            .unwrap(),
        TransitionOutcome::Applied
    );
    assert_eq!(meta.status, QuickSessionStatus::Active);
    assert_eq!(meta.last_completed_attempt_id.as_deref(), Some(ATTEMPT_ID));
    assert_eq!(meta.last_completed_input_line, Some(1));
    assert_eq!(meta.last_completed_line, Some(2));
    assert_eq!(meta.attempt_id, None);

    let revision = meta.revision;
    assert_eq!(
        apply_quick_session_transition(&mut meta, reply(1, ATTEMPT_ID, 99, "2026-07-11T00:00:07Z"))
            .unwrap(),
        TransitionOutcome::Duplicate {
            line_number: Some(2)
        }
    );
    assert_eq!(meta.revision, revision);
}

#[test]
fn quick_session_human_input_queues_while_running() {
    let mut meta = meta_fixture();
    accept_human(&mut meta, 1);
    apply_quick_session_transition(&mut meta, claim(1, ATTEMPT_ID, "2026-07-11T00:00:01Z"))
        .unwrap();
    apply_quick_session_transition(
        &mut meta,
        human(3, Some("request-2"), "2026-07-11T00:00:02Z"),
    )
    .unwrap();
    assert_eq!(meta.status, QuickSessionStatus::Running);
    assert_eq!(meta.processing_input_line, Some(1));
    assert_eq!(meta.last_human_line, Some(3));

    apply_quick_session_transition(
        &mut meta,
        title(ATTEMPT_ID, "Investigate flaky test", "2026-07-11T00:00:03Z"),
    )
    .unwrap();
    apply_quick_session_transition(&mut meta, reply(1, ATTEMPT_ID, 4, "2026-07-11T00:00:04Z"))
        .unwrap();
    assert_eq!(meta.last_completed_input_line, Some(1));
    assert_eq!(meta.last_human_line, Some(3));

    apply_quick_session_transition(
        &mut meta,
        claim(3, "qa-01JYYYYYYYYYYYYYYYYYYYYYYY", "2026-07-11T00:00:05Z"),
    )
    .unwrap();
    assert_eq!(meta.processing_input_line, Some(3));
}

#[test]
fn quick_session_running_archive_clears_claim_and_unarchive_restores_stable_state() {
    let mut meta = meta_fixture();
    accept_human(&mut meta, 1);
    apply_quick_session_transition(&mut meta, claim(1, ATTEMPT_ID, "2026-07-11T00:00:01Z"))
        .unwrap();
    apply_quick_session_transition(
        &mut meta,
        title(ATTEMPT_ID, "Investigate flaky test", "2026-07-11T00:00:02Z"),
    )
    .unwrap();

    let mut wrong_actor = QuickSessionTransition::Archive {
        actor: "alice".to_string(),
        now: "2026-07-11T00:00:03Z".to_string(),
    };
    assert!(apply_quick_session_transition(&mut meta, wrong_actor.clone()).is_err());
    if let QuickSessionTransition::Archive { actor, .. } = &mut wrong_actor {
        *actor = "lewis".to_string();
    }
    apply_quick_session_transition(&mut meta, wrong_actor).unwrap();
    assert_eq!(meta.status, QuickSessionStatus::Archived);
    assert_eq!(meta.archived_from, Some(QuickSessionStatus::Active));
    assert_eq!(meta.archived_at.as_deref(), Some("2026-07-11T00:00:03Z"));
    assert_eq!(meta.attempt_id, None);
    assert!(validate_quick_session_meta(&meta).is_ok());

    assert!(apply_quick_session_transition(
        &mut meta,
        title(ATTEMPT_ID, "Late title", "2026-07-11T00:00:04Z")
    )
    .is_err());

    apply_quick_session_transition(
        &mut meta,
        QuickSessionTransition::Unarchive {
            actor: "lewis".to_string(),
            now: "2026-07-11T00:00:05Z".to_string(),
        },
    )
    .unwrap();
    assert_eq!(meta.status, QuickSessionStatus::Active);
    assert_eq!(meta.archived_at, None);
    assert_eq!(meta.archived_from, None);
}

#[test]
fn quick_session_error_archive_round_trips_error_state() {
    let mut meta = meta_fixture();
    accept_human(&mut meta, 1);
    apply_quick_session_transition(&mut meta, claim(1, ATTEMPT_ID, "2026-07-11T00:00:01Z"))
        .unwrap();
    apply_quick_session_transition(
        &mut meta,
        QuickSessionTransition::MarkError {
            actor: "alice".to_string(),
            attempt_id: ATTEMPT_ID.to_string(),
            error: "provider exited".to_string(),
            now: "2026-07-11T00:00:02Z".to_string(),
        },
    )
    .unwrap();
    apply_quick_session_transition(
        &mut meta,
        QuickSessionTransition::Archive {
            actor: "lewis".to_string(),
            now: "2026-07-11T00:00:03Z".to_string(),
        },
    )
    .unwrap();
    assert_eq!(meta.archived_from, Some(QuickSessionStatus::Error));

    apply_quick_session_transition(
        &mut meta,
        QuickSessionTransition::Unarchive {
            actor: "lewis".to_string(),
            now: "2026-07-11T00:00:04Z".to_string(),
        },
    )
    .unwrap();
    assert_eq!(meta.status, QuickSessionStatus::Error);
    assert_eq!(meta.error.as_deref(), Some("provider exited"));
}

#[test]
fn quick_session_mark_error_retry_is_idempotent() {
    let mut meta = meta_fixture();
    accept_human(&mut meta, 1);
    apply_quick_session_transition(&mut meta, claim(1, ATTEMPT_ID, "2026-07-11T00:00:01Z"))
        .unwrap();
    let transition = QuickSessionTransition::MarkError {
        actor: "alice".to_string(),
        attempt_id: ATTEMPT_ID.to_string(),
        error: "provider exited".to_string(),
        now: "2026-07-11T00:00:02Z".to_string(),
    };
    apply_quick_session_transition(&mut meta, transition.clone()).unwrap();
    let revision = meta.revision;

    assert_eq!(
        apply_quick_session_transition(&mut meta, transition).unwrap(),
        TransitionOutcome::Duplicate { line_number: None }
    );
    assert_eq!(meta.revision, revision);
    assert_eq!(meta.last_failed_attempt_id.as_deref(), Some(ATTEMPT_ID));

    assert!(apply_quick_session_transition(
        &mut meta,
        QuickSessionTransition::MarkError {
            actor: "alice".to_string(),
            attempt_id: "qa-01JYYYYYYYYYYYYYYYYYYYYYYY".to_string(),
            error: "late failure".to_string(),
            now: "2026-07-11T00:00:03Z".to_string(),
        }
    )
    .is_err());
}

fn fail_with_queued_human_input(title_value: Option<&str>) -> QuickSessionMeta {
    let mut meta = meta_fixture();
    accept_human(&mut meta, 1);
    apply_quick_session_transition(&mut meta, claim(1, ATTEMPT_ID, "2026-07-11T00:00:01Z"))
        .unwrap();
    if let Some(title_value) = title_value {
        apply_quick_session_transition(
            &mut meta,
            title(ATTEMPT_ID, title_value, "2026-07-11T00:00:02Z"),
        )
        .unwrap();
    }
    apply_quick_session_transition(
        &mut meta,
        human(2, Some("request-2"), "2026-07-11T00:00:03Z"),
    )
    .unwrap();
    apply_quick_session_transition(
        &mut meta,
        QuickSessionTransition::MarkError {
            actor: "alice".to_string(),
            attempt_id: ATTEMPT_ID.to_string(),
            error: "provider exited".to_string(),
            now: "2026-07-11T00:00:04Z".to_string(),
        },
    )
    .unwrap();
    meta
}

#[test]
fn quick_session_failed_untitled_turn_preserves_queued_input() {
    let mut meta = fail_with_queued_human_input(None);
    assert_eq!(meta.status, QuickSessionStatus::NeedsTitle);
    assert_eq!(meta.error.as_deref(), Some("provider exited"));
    assert_eq!(meta.last_failed_attempt_id.as_deref(), Some(ATTEMPT_ID));

    let revision = meta.revision;
    assert_eq!(
        apply_quick_session_transition(
            &mut meta,
            QuickSessionTransition::MarkError {
                actor: "alice".to_string(),
                attempt_id: ATTEMPT_ID.to_string(),
                error: "provider exited".to_string(),
                now: "2026-07-11T00:00:05Z".to_string(),
            },
        )
        .unwrap(),
        TransitionOutcome::Duplicate { line_number: None }
    );
    assert_eq!(meta.revision, revision);

    apply_quick_session_transition(
        &mut meta,
        claim(2, "qa-01JYYYYYYYYYYYYYYYYYYYYYYY", "2026-07-11T00:00:06Z"),
    )
    .unwrap();
    assert_eq!(meta.status, QuickSessionStatus::Running);
    assert_eq!(meta.processing_input_line, Some(2));
    assert_eq!(meta.error, None);
    assert_eq!(meta.last_failed_attempt_id, None);
}

#[test]
fn quick_session_failed_titled_turn_preserves_queued_input() {
    let mut meta = fail_with_queued_human_input(Some("Investigate flaky test"));
    assert_eq!(meta.status, QuickSessionStatus::Active);
    assert_eq!(meta.error.as_deref(), Some("provider exited"));

    apply_quick_session_transition(
        &mut meta,
        claim(2, "qa-01JYYYYYYYYYYYYYYYYYYYYYYY", "2026-07-11T00:00:05Z"),
    )
    .unwrap();
    assert_eq!(meta.status, QuickSessionStatus::Running);
    assert_eq!(meta.processing_input_line, Some(2));
}

#[test]
fn quick_session_transition_wire_shape_is_shared_with_wasm() {
    let transition = human(7, Some("request-7"), "2026-07-11T00:00:07Z");
    let value = serde_json::to_value(&transition).unwrap();
    assert_eq!(value["kind"], "human_message");
    assert_eq!(value["line_number"], 7);
    assert_eq!(value["request_id"], "request-7");

    let decoded: QuickSessionTransition = serde_json::from_value(value).unwrap();
    assert_eq!(decoded, transition);
    assert_eq!(
        serde_json::to_value(TransitionOutcome::Duplicate {
            line_number: Some(7)
        })
        .unwrap(),
        serde_json::json!({"kind": "duplicate", "line_number": 7})
    );
}

#[test]
fn quick_session_refs_parse_only_at_text_boundaries() {
    let bare = format!("See session:{SESSION_ID} now");
    assert_eq!(
        extract_links(&bare)[0].kind,
        LinkKind::QuickSession {
            session_id: SESSION_ID.to_string(),
            line_number: None,
        }
    );

    let line = format!("(session:{SESSION_ID}:L000001)");
    assert_eq!(
        extract_links(&line)[0].kind,
        LinkKind::QuickSession {
            session_id: SESSION_ID.to_string(),
            line_number: Some(1),
        }
    );

    for invalid in [
        format!("xsession:{SESSION_ID}"),
        format!("session:{SESSION_ID}x"),
        format!("session:{SESSION_ID}:L1"),
        format!("session:{SESSION_ID}:L000001x"),
        format!("界session:{SESSION_ID}"),
        format!("session:{SESSION_ID}界"),
        format!("/session:{SESSION_ID}"),
        format!("session:{SESSION_ID}/discussion.thread"),
        format!("session:{SESSION_ID}:L18446744073709551616"),
    ] {
        assert!(extract_links(&invalid).is_empty(), "{invalid}");
    }
}

#[test]
fn quick_session_response_dtos_have_stable_wire_shapes() {
    let meta = meta_fixture();
    let entry = ThreadEntry::Message(Message {
        line_number: 1,
        point_to: 0,
        author: Handler::new("lewis").unwrap(),
        timestamp: "2026-07-11T00:00:00Z".to_string(),
        body: "Please investigate".to_string(),
        mentions: vec![],
        links: vec![],
    });
    let detail = QuickSessionDetail {
        meta: meta.clone(),
        entries: vec![entry],
        archived: false,
    };
    let create = CreateQuickSessionResponse {
        session: detail,
        line_number: 1,
        r#ref: format!("session:{SESSION_ID}"),
    };
    let value = serde_json::to_value(&create).unwrap();
    assert_eq!(value["session"]["meta"]["id"], SESSION_ID);
    assert_eq!(value["session"]["entries"][0]["type"], "message");
    assert_eq!(value["line_number"], 1);
    assert_eq!(value["ref"], format!("session:{SESSION_ID}"));

    let list = ListQuickSessionsResponse {
        sessions: vec![QuickSessionListItem {
            id: SESSION_ID.to_string(),
            title: None,
            agent_id: "alice".to_string(),
            created_by: "lewis".to_string(),
            status: QuickSessionStatus::NeedsTitle,
            updated_at: "2026-07-11T00:00:00Z".to_string(),
            last_message_preview: "Please investigate".to_string(),
            revision: 1,
            archived: false,
            r#ref: format!("session:{SESSION_ID}"),
        }],
    };
    assert_eq!(
        serde_json::to_value(&list).unwrap()["sessions"][0]["revision"],
        1
    );

    let sent = SendQuickSessionMessageResponse {
        session_id: SESSION_ID.to_string(),
        line_number: 2,
        status: QuickSessionStatus::Active,
        revision: 4,
        r#ref: format!("session:{SESSION_ID}:L000002"),
    };
    let encoded = serde_json::to_string(&sent).unwrap();
    let decoded: SendQuickSessionMessageResponse = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, sent);
}
