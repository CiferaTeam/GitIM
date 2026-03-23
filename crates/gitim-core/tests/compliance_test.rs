use gitim_core::validator::compliance::validate_append;

fn make_existing() -> &'static str {
    "[L000001][P000000][@nexus][20250316T120000Z] first message\n[L000002][P000001][@lewis][20250316T120500Z] reply\n"
}

#[test]
fn test_valid_append() {
    let existing = make_existing();
    let new_lines = "[L000003][P000001][@nexus][20250316T121000Z] another reply\n";
    let users = vec!["nexus", "lewis"];
    let result = validate_append(existing, new_lines, &users);
    assert!(result.is_ok());
}

#[test]
fn test_append_wrong_line_number() {
    let existing = make_existing();
    let new_lines = "[L000005][P000001][@nexus][20250316T121000Z] skipped 4\n";
    let users = vec!["nexus"];
    let result = validate_append(existing, new_lines, &users);
    assert!(result.is_err());
}

#[test]
fn test_append_unknown_author() {
    let existing = make_existing();
    let new_lines = "[L000003][P000001][@unknown][20250316T121000Z] who am i\n";
    let users = vec!["nexus", "lewis"];
    let result = validate_append(existing, new_lines, &users);
    assert!(result.is_err());
}

#[test]
fn test_append_invalid_p_reference() {
    let existing = make_existing();
    let new_lines = "[L000003][P000099][@nexus][20250316T121000Z] bad ref\n";
    let users = vec!["nexus"];
    let result = validate_append(existing, new_lines, &users);
    assert!(result.is_err());
}

#[test]
fn test_append_p_references_within_batch() {
    let existing = make_existing();
    let new_lines = "\
[L000003][P000000][@nexus][20250316T121000Z] new topic
[L000004][P000003][@lewis][20250316T121500Z] reply to new topic
";
    let users = vec!["nexus", "lewis"];
    let result = validate_append(existing, new_lines, &users);
    assert!(result.is_ok());
}

#[test]
fn test_append_to_empty_file() {
    let new_lines = "[L000001][P000000][@nexus][20250316T121000Z] first\n";
    let users = vec!["nexus"];
    let result = validate_append("", new_lines, &users);
    assert!(result.is_ok());
}

#[test]
fn test_append_with_valid_mention() {
    let existing = make_existing();
    let new_lines = "[L000003][P000001][@nexus][20250316T121000Z] hey <@lewis> check this\n";
    let users = vec!["nexus", "lewis"];
    let result = validate_append(existing, new_lines, &users);
    assert!(result.is_ok());
}

#[test]
fn test_append_with_unknown_mention_rejected() {
    let existing = make_existing();
    let new_lines = "[L000003][P000001][@nexus][20250316T121000Z] hey <@ghost> check this\n";
    let users = vec!["nexus", "lewis"];
    let result = validate_append(existing, new_lines, &users);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("ghost"));
}

#[test]
fn test_append_bare_at_not_validated() {
    let existing = make_existing();
    let new_lines = "[L000003][P000001][@nexus][20250316T121000Z] cc @ghost 不验证\n";
    let users = vec!["nexus", "lewis"];
    let result = validate_append(existing, new_lines, &users);
    assert!(result.is_ok());
}

#[test]
fn test_append_mention_in_continuation() {
    let existing = make_existing();
    let new_lines = "[L000003][P000001][@nexus][20250316T121000Z] first line\ncc <@unknown> 看看\n";
    let users = vec!["nexus", "lewis"];
    let result = validate_append(existing, new_lines, &users);
    assert!(result.is_err());
}

#[test]
fn test_append_multiple_mentions_one_unknown() {
    let existing = make_existing();
    let new_lines = "[L000003][P000001][@nexus][20250316T121000Z] hey <@lewis> and <@ghost>\n";
    let users = vec!["nexus", "lewis"];
    let result = validate_append(existing, new_lines, &users);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("ghost"));
}

#[test]
fn test_validate_append_event_p_nonzero_rejected() {
    let result = validate_append(
        "",
        "[L000001][P000001][@alice][20260323T120000Z][E:join] {}\n",
        &["alice"],
    );
    assert!(result.is_err());
}

#[test]
fn test_validate_append_event_invalid_json_rejected() {
    let result = validate_append(
        "",
        "[L000001][P000000][@alice][20260323T120000Z][E:join] not-json\n",
        &["alice"],
    );
    assert!(result.is_err());
}

#[test]
fn test_validate_append_event_ok() {
    let result = validate_append(
        "",
        "[L000001][P000000][@alice][20260323T120000Z][E:join] {}\n",
        &["alice"],
    );
    assert!(result.is_ok());
}

#[test]
fn test_validate_append_mixed_messages_and_events() {
    let existing = "[L000001][P000000][@alice][20260323T120000Z][E:join] {}\n";
    let result = validate_append(
        existing,
        "[L000002][P000000][@alice][20260323T120100Z] hello\n",
        &["alice"],
    );
    assert!(result.is_ok());
}
