use gitim_core::validator::read_check::{check_thread_integrity, IntegrityIssue};

#[test]
fn test_clean_thread() {
    let input = "\
[L000001][P000000][@nexus][20250316T120000Z] hello
[L000002][P000001][@lewis][20250316T120500Z] reply
";
    let users = vec!["nexus", "lewis"];
    let issues = check_thread_integrity(input, &users);
    assert!(issues.is_empty());
}

#[test]
fn test_detect_gap() {
    let input = "\
[L000001][P000000][@nexus][20250316T120000Z] hello
[L000003][P000001][@lewis][20250316T120500Z] skipped 2
";
    let users = vec!["nexus", "lewis"];
    let issues = check_thread_integrity(input, &users);
    assert!(issues
        .iter()
        .any(|i| matches!(i, IntegrityIssue::LineNumberGap { .. })));
}

#[test]
fn test_detect_unknown_author() {
    let input = "[L000001][P000000][@unknown][20250316T120000Z] who\n";
    let users = vec!["nexus"];
    let issues = check_thread_integrity(input, &users);
    assert!(issues
        .iter()
        .any(|i| matches!(i, IntegrityIssue::UnknownAuthor(_))));
}

#[test]
fn test_detect_invalid_p_ref() {
    let input = "[L000001][P000099][@nexus][20250316T120000Z] bad ref\n";
    let users = vec!["nexus"];
    let issues = check_thread_integrity(input, &users);
    assert!(issues
        .iter()
        .any(|i| matches!(i, IntegrityIssue::InvalidPointTo(_))));
}

#[test]
fn test_detect_unknown_mention() {
    let input = "[L000001][P000000][@nexus][20250316T120000Z] hey <@ghost>\n";
    let users = vec!["nexus"];
    let issues = check_thread_integrity(input, &users);
    assert!(issues.iter().any(
        |i| matches!(i, IntegrityIssue::UnknownMention { handler, .. } if handler == "ghost")
    ));
}

#[test]
fn test_valid_mention_no_issue() {
    let input = "[L000001][P000000][@nexus][20250316T120000Z] hey <@lewis>\n";
    let users = vec!["nexus", "lewis"];
    let issues = check_thread_integrity(input, &users);
    assert!(issues.is_empty());
}

#[test]
fn test_bare_at_no_issue() {
    let input = "[L000001][P000000][@nexus][20250316T120000Z] hey @ghost\n";
    let users = vec!["nexus"];
    let issues = check_thread_integrity(input, &users);
    assert!(issues.is_empty());
}
