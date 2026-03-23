use gitim_core::parser::parse_thread;
use gitim_core::types::ThreadEntry;

#[test]
fn test_parse_single_message() {
    let input = "[L000001][P000000][@nexus][20250316T120000Z] hello world\n";
    let result = parse_thread(input).unwrap();
    assert_eq!(result.messages().len(), 1);
    let msg = &result.messages()[0];
    assert_eq!(msg.line_number, 1);
    assert_eq!(msg.point_to, 0);
    assert_eq!(msg.author.as_str(), "nexus");
    assert_eq!(msg.timestamp, "20250316T120000Z");
    assert_eq!(msg.body, "hello world");
}

#[test]
fn test_parse_message_with_continuation() {
    let input = "[L000001][P000000][@nexus][20250316T120000Z] line one\nline two\nline three\n";
    let result = parse_thread(input).unwrap();
    assert_eq!(result.messages().len(), 1);
    assert_eq!(result.messages()[0].body, "line one\nline two\nline three");
}

#[test]
fn test_parse_multiple_messages() {
    let input = "\
[L000001][P000000][@nexus][20250316T120000Z] first
[L000002][P000001][@lewis][20250316T120500Z] reply
";
    let result = parse_thread(input).unwrap();
    assert_eq!(result.messages().len(), 2);
    assert_eq!(result.messages()[0].line_number, 1);
    assert_eq!(result.messages()[1].line_number, 2);
    assert_eq!(result.messages()[1].point_to, 1);
}

#[test]
fn test_parse_mixed_messages_and_continuations() {
    let input = "\
[L000001][P000000][@nexus][20250316T120000Z] multi
continuation line
[L000002][P000001][@lewis][20250316T120500Z] reply
also multi
line three
";
    let result = parse_thread(input).unwrap();
    assert_eq!(result.messages().len(), 2);
    assert_eq!(result.messages()[0].body, "multi\ncontinuation line");
    assert_eq!(result.messages()[1].body, "reply\nalso multi\nline three");
}

#[test]
fn test_parse_empty_file() {
    let result = parse_thread("").unwrap();
    assert_eq!(result.messages().len(), 0);
}

#[test]
fn test_parse_body_with_brackets() {
    let input = "[L000001][P000000][@nexus][20250316T120000Z] check [this] out\n";
    let result = parse_thread(input).unwrap();
    assert_eq!(result.messages()[0].body, "check [this] out");
}

#[test]
fn test_parse_escaped_continuation() {
    let input = "\
[L000001][P000000][@nexus][20250316T120000Z] see example:
 [L000001] this is escaped continuation
";
    let result = parse_thread(input).unwrap();
    assert_eq!(result.messages().len(), 1);
    assert_eq!(result.messages()[0].body, "see example:\n[L000001] this is escaped continuation");
}

#[test]
fn test_parse_large_line_numbers() {
    let input = "[L1000000][P0000000][@nexus][20250316T120000Z] big numbers\n";
    let result = parse_thread(input).unwrap();
    assert_eq!(result.messages()[0].line_number, 1000000);
}

#[test]
fn test_parse_extracts_mentions_from_body() {
    let input = "[L000001][P000000][@nexus][20250316T120000Z] hey <@lewis> check this\n";
    let result = parse_thread(input).unwrap();
    assert_eq!(result.messages()[0].mentions.len(), 1);
    assert_eq!(result.messages()[0].mentions[0].as_str(), "lewis");
}

#[test]
fn test_parse_extracts_mentions_from_continuation() {
    let input = "\
[L000001][P000000][@nexus][20250316T120000Z] first line
need <@coder> to review
";
    let result = parse_thread(input).unwrap();
    assert_eq!(result.messages()[0].mentions.len(), 1);
    assert_eq!(result.messages()[0].mentions[0].as_str(), "coder");
}

#[test]
fn test_parse_no_mentions() {
    let input = "[L000001][P000000][@nexus][20250316T120000Z] plain message\n";
    let result = parse_thread(input).unwrap();
    assert!(result.messages()[0].mentions.is_empty());
}

#[test]
fn test_parse_bare_at_not_extracted() {
    let input = "[L000001][P000000][@nexus][20250316T120000Z] cc @lewis 看看\n";
    let result = parse_thread(input).unwrap();
    assert!(result.messages()[0].mentions.is_empty());
}

#[test]
fn test_parse_event_line() {
    let input = "[L000001][P000000][@nexus][20250316T120000Z][E:join] {}\n";
    let result = parse_thread(input).unwrap();
    assert_eq!(result.entries.len(), 1);
    assert_eq!(result.messages().len(), 0);
    assert_eq!(result.events().len(), 1);
    let ev = result.events()[0];
    assert_eq!(ev.line_number, 1);
    assert_eq!(ev.point_to, 0);
    assert_eq!(ev.author.as_str(), "nexus");
    assert_eq!(ev.event_type, "join");
    assert_eq!(ev.meta, serde_json::json!({}));
}

#[test]
fn test_parse_mixed_messages_and_events() {
    let input = "\
[L000001][P000000][@nexus][20250316T120000Z][E:join] {}
[L000002][P000000][@nexus][20250316T120100Z] hello everyone
[L000003][P000000][@lewis][20250316T120200Z][E:join] {}
[L000004][P000002][@lewis][20250316T120300Z] hi nexus
";
    let result = parse_thread(input).unwrap();
    assert_eq!(result.entries.len(), 4);
    assert_eq!(result.messages().len(), 2);
    assert_eq!(result.events().len(), 2);
    assert!(matches!(&result.entries[0], ThreadEntry::Event(_)));
    assert!(matches!(&result.entries[1], ThreadEntry::Message(_)));
    assert!(matches!(&result.entries[2], ThreadEntry::Event(_)));
    assert!(matches!(&result.entries[3], ThreadEntry::Message(_)));
}

#[test]
fn test_parse_event_multiline_json_body() {
    let input = "\
[L000001][P000000][@nexus][20250316T120000Z][E:leave] {\"reason\":
\"goodbye\"}
";
    let result = parse_thread(input).unwrap();
    assert_eq!(result.events().len(), 1);
    let ev = result.events()[0];
    assert_eq!(ev.event_type, "leave");
    assert_eq!(ev.meta, serde_json::json!({"reason": "goodbye"}));
}
