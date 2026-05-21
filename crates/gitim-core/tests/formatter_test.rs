#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use gitim_core::formatter::{format_event, format_message};
use gitim_core::types::Handler;

#[test]
fn test_format_simple_message() {
    let result = format_message(
        1,
        0,
        &Handler::new("nexus").unwrap(),
        "20250316T120000Z",
        "hello",
    );
    assert_eq!(
        result,
        "[L000001][P000000][@nexus][20250316T120000Z] hello\n"
    );
}

#[test]
fn test_format_reply() {
    let result = format_message(
        5,
        3,
        &Handler::new("lewis").unwrap(),
        "20250316T120000Z",
        "reply",
    );
    assert_eq!(
        result,
        "[L000005][P000003][@lewis][20250316T120000Z] reply\n"
    );
}

#[test]
fn test_format_multiline_body() {
    let result = format_message(
        1,
        0,
        &Handler::new("nexus").unwrap(),
        "20250316T120000Z",
        "line one\nline two\nline three",
    );
    assert_eq!(
        result,
        "[L000001][P000000][@nexus][20250316T120000Z] line one\nline two\nline three\n"
    );
}

#[test]
fn test_format_body_needing_escape() {
    let body = "[L000001] looks like a message prefix";
    let result = format_message(
        2,
        0,
        &Handler::new("nexus").unwrap(),
        "20250316T120000Z",
        &format!("see:\n{}", body),
    );
    assert_eq!(
        result,
        "[L000002][P000000][@nexus][20250316T120000Z] see:\n [L000001] looks like a message prefix\n"
    );
}

#[test]
fn test_format_large_line_number() {
    let result = format_message(
        1000000,
        0,
        &Handler::new("nexus").unwrap(),
        "20250316T120000Z",
        "big",
    );
    assert_eq!(
        result,
        "[L1000000][P0000000][@nexus][20250316T120000Z] big\n"
    );
}

#[test]
fn test_format_event_self_join() {
    let author = Handler::new("nexus").unwrap();
    let result = format_event(
        1,
        &author,
        "20250316T120000Z",
        "join",
        &serde_json::json!({}),
    );
    assert_eq!(
        result,
        "[L000001][P000000][@nexus][20250316T120000Z][E:join] {}\n"
    );
}

#[test]
fn test_format_event_with_targets() {
    let author = Handler::new("nexus").unwrap();
    let meta = serde_json::json!({"targets": ["lewis", "coder"]});
    let result = format_event(5, &author, "20250316T120000Z", "leave", &meta);
    assert!(result.starts_with("[L000005][P000000][@nexus][20250316T120000Z][E:leave] "));
    assert!(result.contains("\"targets\""));
    assert!(result.ends_with('\n'));
}
