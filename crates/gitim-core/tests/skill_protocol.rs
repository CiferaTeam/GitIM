#![allow(clippy::expect_used, clippy::unwrap_used)]

use gitim_core::skill::{
    parse_skill_reference, parse_skill_reference_or_shorthand, EventId, ProposalId, RevisionId,
    SkillError, SkillReference, SkillSlug,
};

const REVISION: &str = "r-01K1D8QG2S8RX4T9M9BDKQ9Z7N";

#[test]
fn skill_slug_accepts_canonical_names() {
    let slug = SkillSlug::new("release-check").expect("valid slug");
    assert_eq!(slug.as_str(), "release-check");
    assert_eq!(slug.to_string(), "release-check");
}

#[test]
fn skill_slug_rejects_non_portable_names() {
    for value in [
        "",
        "Release-Check",
        "release_check",
        "-release",
        "release-",
        "release--check",
        "a/b",
    ] {
        assert_eq!(
            SkillSlug::new(value),
            Err(SkillError::InvalidSlug),
            "{value}"
        );
    }
}

#[test]
fn prefixed_ids_require_canonical_ulids() {
    assert_eq!(
        RevisionId::new(REVISION).expect("revision").as_str(),
        REVISION
    );
    assert!(ProposalId::new("p-01K1D8QG2S8RX4T9M9BDKQ9Z7N").is_ok());
    assert!(EventId::new("e-01K1D8QG2S8RX4T9M9BDKQ9Z7N").is_ok());

    for invalid in [
        "01K1D8QG2S8RX4T9M9BDKQ9Z7N",
        "r-01k1d8qg2s8rx4t9m9bdkq9z7n",
        "r-01K1D8QG2S8RX4T9M9BDKQ9Z7O",
        "p-01K1D8QG2S8RX4T9M9BDKQ9Z7N",
    ] {
        assert!(RevisionId::new(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn generated_event_id_is_greater_than_observed_maximum() {
    let maximum = EventId::new("e-01K1D8QG2S8RX4T9M9BDKQ9Z7N").expect("event");
    let generated = EventId::generate_after(Some(&maximum)).expect("next event");
    assert!(generated > maximum);
}

#[test]
fn canonical_reference_round_trips() {
    let parsed = parse_skill_reference(&format!("skill:release-check@{REVISION}"))
        .expect("canonical reference");
    assert_eq!(parsed.slug.as_str(), "release-check");
    assert_eq!(
        parsed.revision.as_ref().map(RevisionId::as_str),
        Some(REVISION)
    );
    assert_eq!(
        parsed.to_string(),
        format!("skill:release-check@{REVISION}")
    );

    let json = serde_json::to_string(&parsed).expect("serialize");
    assert_eq!(json, format!("\"skill:release-check@{REVISION}\""));
    assert_eq!(
        serde_json::from_str::<SkillReference>(&json).expect("deserialize"),
        parsed
    );
}

#[test]
fn canonical_reference_supports_unpinned_form() {
    let parsed = parse_skill_reference("skill:release-check").expect("reference");
    assert!(parsed.revision.is_none());
    assert_eq!(parsed.to_string(), "skill:release-check");
}

#[test]
fn cli_reference_accepts_shorthand() {
    let pinned = parse_skill_reference_or_shorthand(&format!("release-check@{REVISION}"))
        .expect("shorthand");
    assert_eq!(
        pinned.to_string(),
        format!("skill:release-check@{REVISION}")
    );

    let unpinned = parse_skill_reference_or_shorthand("release-check").expect("shorthand");
    assert_eq!(unpinned.to_string(), "skill:release-check");
}

#[test]
fn references_reject_extra_separators_and_bad_ids() {
    for value in [
        "release-check",
        "skill:release-check@",
        "skill:release-check@@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N",
        "skill:release-check@r-bad",
        "skill:Release",
    ] {
        assert!(parse_skill_reference(value).is_err(), "{value}");
    }
}
