#![allow(clippy::unwrap_used)]

use gitim_core::skill::{
    parse_skill_reference, scan_skill_references, ProposalId, RequestId, RevisionId,
    SkillCreateRequest, SkillError, SkillListQuery, SkillMutationRequest,
    SkillProposalTransitionRequest, SkillPublicationMeta, SkillReceiptRequest, SkillReference,
    SkillShowQuery, SkillSlug,
};
use gitim_core::types::Handler;

#[test]
fn identifiers_are_prefixed_uppercase_ulids() {
    let revision = RevisionId::generate();
    let proposal = ProposalId::generate();
    let request = RequestId::generate();
    assert!(revision.as_str().starts_with("r-"));
    assert!(proposal.as_str().starts_with("p-"));
    assert!(request.as_str().starts_with("q-"));
    assert_eq!(revision.as_str().len(), 28);
}

#[test]
fn canonical_reference_round_trips() {
    let parsed = parse_skill_reference("skill:release-check@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N").unwrap();
    assert_eq!(parsed.slug.as_str(), "release-check");
    assert_eq!(
        parsed.revision.unwrap().as_str(),
        "r-01K1D8QG2S8RX4T9M9BDKQ9Z7N"
    );
}

#[test]
fn scanner_ignores_code_urls_and_escapes() {
    let refs = scan_skill_references(
        r#"`skill:nope@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N`
https://x/skill:nope@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N
\skill:nope@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N
skill:ok@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N"#,
    );
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].slug.as_str(), "ok");
}

#[test]
fn errors_have_stable_codes() {
    assert_eq!(SkillError::NotFound.code(), "skill_not_found");
    assert_eq!(SkillError::RequestIdConflict.code(), "request_id_conflict");
}

#[test]
fn every_skill_error_has_its_protocol_code() {
    let revision = RevisionId::new("r-01K1D8QG2S8RX4T9M9BDKQ9Z7N").unwrap();
    let stale_proposal = SkillError::StaleProposalRevision {
        current_revision: revision.clone(),
        control_revision: 3,
        event_revision: 4,
        proposal_status: gitim_core::skill::ProposalStatus::Open,
        proposal_state_revision: 2,
    };
    let cases = [
        (SkillError::NotFound, "skill_not_found"),
        (SkillError::Archived, "skill_archived"),
        (SkillError::Exists, "skill_exists"),
        (SkillError::InvalidSlug, "skill_invalid_slug"),
        (SkillError::InvalidPackage, "skill_invalid_package"),
        (SkillError::PackageTooLarge, "skill_package_too_large"),
        (SkillError::RevisionNotFound, "skill_revision_not_found"),
        (
            SkillError::RevisionUnpublished,
            "skill_revision_unpublished",
        ),
        (SkillError::RevisionCorrupted, "skill_revision_corrupted"),
        (SkillError::ProposalNotFound, "skill_proposal_not_found"),
        (SkillError::ProposalTerminal, "skill_proposal_terminal"),
        (SkillError::OpenProposalLimit, "skill_open_proposal_limit"),
        (
            SkillError::StaleContentRevision {
                current_revision: revision.clone(),
                control_revision: 3,
                event_revision: 4,
            },
            "skill_stale_content_revision",
        ),
        (
            SkillError::StaleControlRevision {
                current_revision: revision,
                control_revision: 3,
                event_revision: 4,
            },
            "skill_stale_control_revision",
        ),
        (stale_proposal, "skill_stale_proposal_revision"),
        (SkillError::NotMaintainer, "skill_not_maintainer"),
        (SkillError::NotOwner, "skill_not_owner"),
        (SkillError::AdminRequired, "skill_admin_required"),
        (SkillError::AdminUninitialized, "skill_admin_uninitialized"),
        (SkillError::LastAdmin, "skill_last_admin"),
        (SkillError::AdminRolePresent, "skill_admin_role_present"),
        (SkillError::LastOwner, "skill_last_owner"),
        (SkillError::OwnerIsMaintainer, "skill_owner_is_maintainer"),
        (SkillError::RoleTargetInvalid, "skill_role_target_invalid"),
        (SkillError::RoleTargetInactive, "skill_role_target_inactive"),
        (SkillError::RolesPresent, "skill_roles_present"),
        (SkillError::RemoteRequired, "skill_remote_required"),
        (SkillError::SyncConflict, "skill_sync_conflict"),
        (
            SkillError::LocalQuarantineBlocked,
            "skill_local_quarantine_blocked",
        ),
        (
            SkillError::EpochValidationBlocked,
            "skill_epoch_validation_blocked",
        ),
        (SkillError::LoadUnavailable, "skill_load_unavailable"),
        (SkillError::RequestIdConflict, "request_id_conflict"),
        (SkillError::OutputExists, "output_exists"),
        (SkillError::StaleCursor, "stale_cursor"),
    ];
    for (error, code) in cases {
        assert_eq!(error.code(), code);
    }
}

#[test]
fn identifier_and_slug_validation_match_the_protocol() {
    assert!(SkillSlug::new("release-check").is_ok());
    assert!(SkillSlug::new("Release-Check").is_err());
    assert!(SkillSlug::new("-release-check").is_err());
    assert!(SkillSlug::new("release--check").is_err());
    assert!(RevisionId::new("r-01K1D8QG2S8RX4T9M9BDKQ9Z7N").is_ok());
    assert!(RevisionId::new("r-01k1D8QG2S8RX4T9M9BDKQ9Z7N").is_err());
    assert!(RevisionId::new("r-Z0000000000000000000000000").is_err());
}

#[test]
fn scanner_ignores_fenced_and_link_destination_references() {
    let refs = scan_skill_references(
        "```md\nskill:fenced@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N\n```\n[link](skill:link@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N)\n(skill:valid@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N)",
    );
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].slug.as_str(), "valid");
}

#[test]
fn scanner_rejects_boundary_violations() {
    let refs = scan_skill_references(
        "xskill:bad@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N skill:ok@r-01K1D8QG2S8RX4T9M9BDKQ9Z7Nx _skill:nope@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N",
    );
    assert!(refs.is_empty());
}

#[test]
fn references_serialize_without_an_empty_revision() {
    let reference = SkillReference {
        slug: SkillSlug::new("release-check").unwrap(),
        revision: None,
    };
    let value = serde_json::to_value(reference).unwrap();
    assert_eq!(value, serde_json::json!({ "slug": "release-check" }));
}

#[test]
fn immutable_metadata_keeps_null_optional_fields() {
    let publication = SkillPublicationMeta {
        schema_version: 1,
        skill: SkillSlug::new("release-check").unwrap(),
        revision: RevisionId::new("r-01K1D8QG2S8RX4T9M9BDKQ9Z7N").unwrap(),
        content_sha256: "a".repeat(64),
        base_revision: None,
        proposal: None,
        published_by: Handler::new("alice").unwrap(),
        published_at: "2026-07-30T04:20:00Z".to_owned(),
    };

    let value = serde_yaml::to_value(publication).unwrap();
    let mapping = value.as_mapping().unwrap();
    for field in ["base_revision", "proposal"] {
        let key = serde_yaml::Value::String(field.to_owned());
        assert!(mapping.contains_key(&key));
        assert_eq!(mapping.get(&key), Some(&serde_yaml::Value::Null));
    }
}

#[test]
fn serde_deserialization_validates_identifiers() {
    assert!(serde_json::from_str::<SkillSlug>(r#""release--check""#).is_err());
    assert!(serde_json::from_str::<RevisionId>(r#""r-01k1D8QG2S8RX4T9M9BDKQ9Z7N""#).is_err());
    assert!(serde_json::from_str::<ProposalId>(r#""q-01K1D8QG2S8RX4T9M9BDKQ9Z7N""#).is_err());
    assert!(serde_json::from_str::<RequestId>(r#""q-01K1D8QG2S8RX4T9M9BDKQ9Z7I""#).is_err());
}

#[test]
fn scanner_skips_escaped_unicode_scalars() {
    let refs = scan_skill_references("\\🙂skill:ok@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N");
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].slug.as_str(), "ok");
}

#[test]
fn scanner_matches_fence_delimiters() {
    let refs = scan_skill_references(
        "````\nskill:still-code@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N\n```\nskill:also-code@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N\n````\n~~~\nskill:tilde@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N\n~~~\nskill:visible@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N",
    );
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].slug.as_str(), "visible");
}

#[test]
fn scanner_does_not_open_a_fence_inside_inline_code() {
    let refs = scan_skill_references(
        "``inline ``` skill:hidden@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N `` skill:visible@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N",
    );
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].slug.as_str(), "visible");
}

#[test]
fn scanner_treats_escapes_as_literal_inside_code() {
    let refs = scan_skill_references(
        "`inline \\` skill:inline-visible@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N\n```\n\\escaped\n```\nskill:fence-visible@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N",
    );
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0].slug.as_str(), "inline-visible");
    assert_eq!(refs[1].slug.as_str(), "fence-visible");
}

#[test]
fn scanner_recognizes_only_line_fences() {
    let refs = scan_skill_references(
        "```\nembedded ``` text\nskill:backtick-hidden@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N\n```\nskill:backtick-visible@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N\n~~~\nembedded ~~~ text\nskill:tilde-hidden@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N\n~~~\nskill:tilde-visible@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N",
    );
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0].slug.as_str(), "backtick-visible");
    assert_eq!(refs[1].slug.as_str(), "tilde-visible");
}

#[test]
fn scanner_closes_backtick_and_tilde_fences_with_crlf() {
    let refs = scan_skill_references(
        "```\r\nskill:backtick-hidden@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N\r\n```\r\nskill:backtick-visible@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N\r\n~~~\r\nskill:tilde-hidden@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N\r\n~~~\r\nskill:tilde-visible@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N",
    );
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0].slug.as_str(), "backtick-visible");
    assert_eq!(refs[1].slug.as_str(), "tilde-visible");
}

#[test]
fn scanner_ignores_code_delimiters_inside_link_destinations() {
    let refs = scan_skill_references(
        "[x](foo`bar) skill:visible@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N [nested](a(b`c)) skill:also-visible@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N",
    );
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0].slug.as_str(), "visible");
    assert_eq!(refs[1].slug.as_str(), "also-visible");
}

#[test]
fn scanner_keeps_escaped_parentheses_inside_link_destinations_literal() {
    let refs = scan_skill_references(
        "[x](foo\\) skill:still-hidden@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N) skill:visible@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N [y](foo\\(bar) skill:also-visible@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N",
    );
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0].slug.as_str(), "visible");
    assert_eq!(refs[1].slug.as_str(), "also-visible");
}

#[test]
fn scanner_does_not_treat_an_escaped_label_closer_as_a_link() {
    let refs = scan_skill_references(
        "[x\\](skill:literal@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N) skill:visible@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N",
    );
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0].slug.as_str(), "literal");
    assert_eq!(refs[1].slug.as_str(), "visible");
}

#[test]
fn local_request_dtos_round_trip_and_expose_request_ids() {
    let request = SkillMutationRequest::Create(SkillCreateRequest {
        request_id: RequestId::new("q-01K1D8QG2S8RX4T9M9BDKQ9Z7N").unwrap(),
        slug: SkillSlug::new("release-check").unwrap(),
        display_name: "Release Check".to_owned(),
        description: "Verify a release candidate.".to_owned(),
        source_directory: std::path::PathBuf::from("/tmp/release-check"),
    });
    let decoded: SkillMutationRequest =
        serde_json::from_value(serde_json::to_value(&request).unwrap()).unwrap();
    assert_eq!(
        decoded.request_id().as_str(),
        "q-01K1D8QG2S8RX4T9M9BDKQ9Z7N"
    );

    let query = SkillShowQuery {
        slug: SkillSlug::new("release-check").unwrap(),
        revision: None,
    };
    let decoded: SkillShowQuery =
        serde_json::from_value(serde_json::to_value(query).unwrap()).unwrap();
    assert_eq!(decoded.slug.as_str(), "release-check");
}

#[test]
fn optional_wire_request_fields_deserialize_when_omitted() {
    let list: SkillListQuery =
        serde_json::from_value(serde_json::json!({ "archived": false, "limit": 50 })).unwrap();
    assert_eq!(list.cursor, None);

    let show: SkillShowQuery =
        serde_json::from_value(serde_json::json!({ "slug": "release-check" })).unwrap();
    assert_eq!(show.revision, None);

    let transition: SkillProposalTransitionRequest = serde_json::from_value(serde_json::json!({
        "request_id": "q-01K1D8QG2S8RX4T9M9BDKQ9Z7N",
        "proposal_id": "p-01K1D8QG2S8RX4T9M9BDKQ9Z7N",
        "operation": "proposal_reject",
        "expected_state_revision": 1
    }))
    .unwrap();
    assert_eq!(transition.expected_control_revision, None);
}

#[test]
fn receipt_request_uses_typed_semantic_fields() {
    let request = SkillReceiptRequest {
        payload_sha256: "a".repeat(64),
        proposal: Some(ProposalId::new("p-01K1D8QG2S8RX4T9M9BDKQ9Z7N").unwrap()),
        expected_proposal_revision: Some(1),
        expected_control_revision: Some(3),
        ..Default::default()
    };
    let value = serde_json::to_value(request).unwrap();
    assert_eq!(value["proposal"], "p-01K1D8QG2S8RX4T9M9BDKQ9Z7N");
    assert_eq!(value["expected_proposal_revision"], 1);
    assert_eq!(value["expected_control_revision"], 3);
    assert!(value.get("source_directory").is_none());
}
