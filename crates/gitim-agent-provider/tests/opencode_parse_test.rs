use gitim_agent_provider::opencode::{parse_line, ParsedMessage};
use serde_json::json;

#[test]
fn parse_step_start() {
    let line = json!({"type": "step_start", "sessionID": "s-1", "part": {}}).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::StepStart { session_id } if session_id == "s-1"));
}

#[test]
fn parse_text() {
    let line = json!({"type": "text", "sessionID": "s-1", "part": {"text": "Hi"}}).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::Text { content } if content == "Hi"));
}

#[test]
fn parse_tool_use_pending() {
    let line = json!({
        "type": "tool_use", "sessionID": "s-1",
        "part": {"tool": "Bash", "callID": "c-1", "state": {"status": "pending", "input": {"command": "ls"}}}
    }).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(
        matches!(msg, ParsedMessage::ToolUse { ref tool, ref call_id, ref status, .. }
        if tool == "Bash" && call_id == "c-1" && status == "pending")
    );
}

#[test]
fn parse_tool_use_completed() {
    let line = json!({
        "type": "tool_use", "sessionID": "s-1",
        "part": {"tool": "Bash", "callID": "c-1", "state": {"status": "completed", "input": {"command": "ls"}, "output": "file.rs"}}
    }).to_string();
    let msg = parse_line(&line).unwrap();
    match msg {
        ParsedMessage::ToolUse { status, output, .. } => {
            assert_eq!(status, "completed");
            assert_eq!(output.as_deref(), Some("file.rs"));
        }
        _ => panic!("expected ToolUse"),
    }
}

#[test]
fn parse_error() {
    let line = json!({
        "type": "error", "sessionID": "s-1",
        "error": {"name": "InvalidModel", "data": {"message": "not found"}}
    })
    .to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::Error { ref message } if message == "not found"));
}

#[test]
fn parse_empty_returns_none() {
    assert!(parse_line("").is_none());
}

#[test]
fn parse_step_finish_with_tokens() {
    let line = json!({
        "type": "step_finish",
        "sessionID": "s-1",
        "part": {
            "tokens": {
                "total": 77244,
                "input": 315,
                "output": 45,
                "reasoning": 7,
                "cache": {"read": 76884, "write": 12}
            },
            "cost": 0.01
        }
    })
    .to_string();

    let msg = parse_line(&line).unwrap();
    let ParsedMessage::StepFinish { usage, reason: _ } = msg else {
        panic!("expected StepFinish");
    };
    assert_eq!(usage.input_tokens, Some(315));
    assert_eq!(usage.output_tokens, Some(52));
    assert_eq!(usage.cache_read_tokens, Some(76_884));
    assert_eq!(usage.cache_creation_tokens, Some(12));
    assert_eq!(usage.used_percent, None);
}

#[test]
fn parse_step_finish_without_tokens_returns_none() {
    let line = json!({"type": "step_finish", "sessionID": "s-1", "part": {}}).to_string();
    assert!(parse_line(&line).is_none());
}
