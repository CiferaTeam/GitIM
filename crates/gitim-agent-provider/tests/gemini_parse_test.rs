use gitim_agent_provider::gemini::{parse_line, ParsedMessage};
use serde_json::json;

#[test]
fn parse_init_extracts_session_id() {
    let line = json!({"type": "init", "session_id": "ses_123"}).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::Init { session_id } if session_id == "ses_123"));
}

#[test]
fn parse_message_text() {
    let line = json!({"type": "message", "role": "assistant", "content": "Hello world"}).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::Text { content } if content == "Hello world"));
}

#[test]
fn parse_tool_use() {
    let line = json!({"type": "tool_use", "tool_name": "terminal", "tool_id": "tc-abc", "parameters": {"command": "ls"}}).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::ToolUse { tool, call_id, .. } if tool == "terminal" && call_id == "tc-abc"));
}

#[test]
fn parse_tool_result() {
    let line = json!({"type": "tool_result", "tool_id": "tc-abc", "status": "completed", "output": "file1.rs"}).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::ToolResult { call_id, output } if call_id == "tc-abc" && output == "file1.rs"));
}

#[test]
fn parse_error_event() {
    let line = json!({"type": "error", "severity": "error", "message": "Model not found"}).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::Error { message } if message == "Model not found"));
}

#[test]
fn parse_result_completed() {
    let line = json!({"type": "result", "status": "completed", "stats": {"input_tokens": 100}}).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::Result { status, .. } if status == "completed"));
}

#[test]
fn parse_result_failed() {
    let line = json!({"type": "result", "status": "error"}).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::Result { status, .. } if status == "error"));
}

#[test]
fn parse_empty_returns_none() {
    assert!(parse_line("").is_none());
}

#[test]
fn parse_unknown_type_returns_none() {
    let line = json!({"type": "debug", "data": "x"}).to_string();
    assert!(parse_line(&line).is_none());
}
