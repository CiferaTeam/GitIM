use gitim_agent_provider::hermes::{parse_notification, ParsedNotification};
use serde_json::json;

#[test]
fn parse_text_chunk() {
    let params = json!({
        "sessionId": "s-1",
        "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "Hello"}}
    });
    let msg = parse_notification(&params).unwrap();
    assert!(matches!(msg, ParsedNotification::Text { content } if content == "Hello"));
}

#[test]
fn parse_thinking_chunk() {
    let params = json!({
        "sessionId": "s-1",
        "update": {"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "Let me think..."}}
    });
    let msg = parse_notification(&params).unwrap();
    assert!(
        matches!(msg, ParsedNotification::Thinking { content } if content == "Let me think...")
    );
}

#[test]
fn parse_tool_call() {
    let params = json!({
        "sessionId": "s-1",
        "update": {
            "sessionUpdate": "tool_call", "toolCallId": "tc-1",
            "title": "terminal: ls -la", "kind": "execute",
            "status": "pending", "rawInput": {"command": "ls -la"}
        }
    });
    let msg = parse_notification(&params).unwrap();
    match msg {
        ParsedNotification::ToolCall { tool, call_id, .. } => {
            assert_eq!(tool, "terminal");
            assert_eq!(call_id, "tc-1");
        }
        _ => panic!("expected ToolCall"),
    }
}

#[test]
fn parse_tool_call_update_completed() {
    let params = json!({
        "sessionId": "s-1",
        "update": {
            "sessionUpdate": "tool_call_update", "toolCallId": "tc-1",
            "status": "completed", "rawOutput": "file1.rs\nfile2.rs"
        }
    });
    let msg = parse_notification(&params).unwrap();
    match msg {
        ParsedNotification::ToolResult { call_id, output } => {
            assert_eq!(call_id, "tc-1");
            assert_eq!(output, "file1.rs\nfile2.rs");
        }
        _ => panic!("expected ToolResult"),
    }
}

#[test]
fn parse_tool_call_update_pending_returns_none() {
    let params = json!({
        "sessionId": "s-1",
        "update": {"sessionUpdate": "tool_call_update", "toolCallId": "tc-1", "status": "pending"}
    });
    assert!(parse_notification(&params).is_none());
}

#[test]
fn parse_usage_update_returns_none() {
    let params = json!({
        "sessionId": "s-1",
        "update": {"sessionUpdate": "usage_update", "usage": {"inputTokens": 100}}
    });
    assert!(parse_notification(&params).is_none());
}

#[test]
fn parse_unknown_update_returns_none() {
    let params = json!({"sessionId": "s-1", "update": {"sessionUpdate": "something_new"}});
    assert!(parse_notification(&params).is_none());
}

#[test]
fn parse_tool_title_extracts_name() {
    let params = json!({
        "sessionId": "s-1",
        "update": {
            "sessionUpdate": "tool_call", "toolCallId": "tc-1",
            "title": "file_edit: path/to/file.rs", "kind": "edit",
            "status": "pending", "rawInput": {}
        }
    });
    let msg = parse_notification(&params).unwrap();
    assert!(matches!(msg, ParsedNotification::ToolCall { ref tool, .. } if tool == "file_edit"));
}
