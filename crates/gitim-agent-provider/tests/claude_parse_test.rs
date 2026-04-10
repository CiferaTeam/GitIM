use gitim_agent_provider::claude::{parse_line, ParsedMessage};
use gitim_agent_provider::Event;
use serde_json::json;

#[test]
fn parse_system_message_extracts_session_id() {
    let line = r#"{"type":"system","subtype":"init","session_id":"sess-123"}"#;
    let msg = parse_line(line).unwrap();
    assert!(
        matches!(msg, ParsedMessage::System { session_id } if session_id == "sess-123")
    );
}

#[test]
fn parse_assistant_text() {
    let line = json!({
        "type": "assistant",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": "Hello world"}]
        }
    })
    .to_string();
    let msg = parse_line(&line).unwrap();
    match msg {
        ParsedMessage::AssistantEvents(events) => {
            assert_eq!(events.len(), 1);
            assert!(
                matches!(&events[0], Event::Text { content } if content == "Hello world")
            );
        }
        _ => panic!("expected Events"),
    }
}

#[test]
fn parse_assistant_thinking() {
    let line = json!({
        "type": "assistant",
        "message": {
            "role": "assistant",
            "content": [{"type": "thinking", "text": "Let me think..."}]
        }
    })
    .to_string();
    let msg = parse_line(&line).unwrap();
    match msg {
        ParsedMessage::AssistantEvents(events) => {
            assert_eq!(events.len(), 1);
            assert!(
                matches!(&events[0], Event::Thinking { content } if content == "Let me think...")
            );
        }
        _ => panic!("expected Events"),
    }
}

#[test]
fn parse_assistant_tool_use() {
    let line = json!({
        "type": "assistant",
        "message": {
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "id": "call-1",
                "name": "Bash",
                "input": {"command": "ls"}
            }]
        }
    })
    .to_string();
    let msg = parse_line(&line).unwrap();
    match msg {
        ParsedMessage::AssistantEvents(events) => {
            assert_eq!(events.len(), 1);
            assert!(
                matches!(&events[0], Event::ToolUse { tool, call_id, .. } if tool == "Bash" && call_id == "call-1")
            );
        }
        _ => panic!("expected Events"),
    }
}

#[test]
fn parse_user_tool_result() {
    let line = json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": "call-1",
                "content": "file1.rs\nfile2.rs"
            }]
        }
    })
    .to_string();
    let msg = parse_line(&line).unwrap();
    match msg {
        ParsedMessage::UserEvents(events) => {
            assert_eq!(events.len(), 1);
            assert!(
                matches!(&events[0], Event::ToolResult { call_id, .. } if call_id == "call-1")
            );
        }
        _ => panic!("expected UserEvents"),
    }
}

#[test]
fn parse_result_completed() {
    let line = json!({
        "type": "result",
        "session_id": "sess-123",
        "result": "Done!",
        "is_error": false,
        "duration_ms": 5000.0,
        "num_turns": 3
    })
    .to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(
        msg,
        ParsedMessage::Result {
            session_id,
            output,
            is_error,
        } if session_id == "sess-123" && output == "Done!" && !is_error
    ));
}

#[test]
fn parse_result_error() {
    let line = json!({
        "type": "result",
        "session_id": "sess-123",
        "result": "Something went wrong",
        "is_error": true
    })
    .to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(
        msg,
        ParsedMessage::Result { is_error, .. } if is_error
    ));
}

#[test]
fn parse_control_request() {
    let line = json!({
        "type": "control_request",
        "request_id": "req-1",
        "request": {
            "subtype": "tool_use",
            "tool_name": "Bash",
            "input": {"command": "rm -rf /"}
        }
    })
    .to_string();
    let msg = parse_line(&line).unwrap();
    assert!(
        matches!(msg, ParsedMessage::ControlRequest { request_id, .. } if request_id == "req-1")
    );
}

#[test]
fn parse_log_message() {
    let line = json!({
        "type": "log",
        "log": {"level": "info", "message": "Starting execution"}
    })
    .to_string();
    let msg = parse_line(&line).unwrap();
    match msg {
        ParsedMessage::AssistantEvents(events) => {
            assert_eq!(events.len(), 1);
            assert!(matches!(
                &events[0],
                Event::Log { level, content } if level == "info" && content == "Starting execution"
            ));
        }
        _ => panic!("expected Events"),
    }
}

#[test]
fn parse_malformed_json_returns_none() {
    assert!(parse_line("not json at all").is_none());
}

#[test]
fn parse_empty_line_returns_none() {
    assert!(parse_line("").is_none());
}
