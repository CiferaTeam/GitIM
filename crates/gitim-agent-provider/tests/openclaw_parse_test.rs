use gitim_agent_provider::openclaw::{parse_line, ParsedMessage};
use serde_json::json;

#[test]
fn parse_step_start() {
    let line = json!({"type": "step_start", "sessionId": "s-123", "data": {}}).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::StepStart { session_id } if session_id == "s-123"));
}

#[test]
fn parse_text() {
    let line = json!({"type": "text", "sessionId": "s-1", "data": {"text": "Hello"}}).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::Text { content } if content == "Hello"));
}

#[test]
fn parse_thinking() {
    let line = json!({"type": "thinking", "sessionId": "s-1", "data": {"text": "Hmm..."}}).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::Thinking { content } if content == "Hmm..."));
}

#[test]
fn parse_tool_call_pending() {
    let line = json!({
        "type": "tool_call", "sessionId": "s-1",
        "data": {"name": "Bash", "callId": "c-1", "input": {"command": "ls"}, "status": "pending"}
    }).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::ToolCall { ref name, ref call_id, ref status, .. }
        if name == "Bash" && call_id == "c-1" && status == "pending"));
}

#[test]
fn parse_tool_call_completed_with_output() {
    let line = json!({
        "type": "tool_call", "sessionId": "s-1",
        "data": {"name": "Bash", "callId": "c-1", "input": {"command": "ls"}, "status": "completed", "output": "file1.rs"}
    }).to_string();
    let msg = parse_line(&line).unwrap();
    match msg {
        ParsedMessage::ToolCall { status, output, .. } => {
            assert_eq!(status, "completed");
            assert_eq!(output.as_deref(), Some("file1.rs"));
        }
        _ => panic!("expected ToolCall"),
    }
}

#[test]
fn parse_result_completed() {
    let line = json!({"type": "result", "sessionId": "s-1", "data": {"status": "completed"}}).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::Result { is_error } if !is_error));
}

#[test]
fn parse_result_error() {
    let line = json!({"type": "result", "sessionId": "s-1", "data": {"status": "error"}}).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::Result { is_error } if is_error));
}

#[test]
fn parse_error_event() {
    let line = json!({"type": "error", "sessionId": "s-1", "data": {"message": "boom", "code": "E001"}}).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::Error { ref message } if message == "boom"));
}

#[test]
fn parse_empty_returns_none() {
    assert!(parse_line("").is_none());
}

#[test]
fn parse_step_end_returns_none() {
    assert!(parse_line(&json!({"type": "step_end", "data": {}}).to_string()).is_none());
}
