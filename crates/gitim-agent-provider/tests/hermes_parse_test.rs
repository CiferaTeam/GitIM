use gitim_agent_provider::hermes::{
    build_prompt_payload, detect_api_failure, parse_notification, ParsedNotification,
};
use serde_json::json;

#[test]
fn detect_api_failure_botocore_retry() {
    let out = "API call failed after 3 retries: Parameter validation failed: \
        Invalid length for parameter modelId, value: 0, valid min length: 1";
    let err = detect_api_failure(out).expect("should detect");
    assert!(err.starts_with("API call failed after 3 retries"));
}

#[test]
fn detect_api_failure_parameter_validation() {
    let out = "Parameter validation failed: missing required key";
    assert!(detect_api_failure(out).is_some());
}

#[test]
fn detect_api_failure_with_leading_whitespace() {
    let out = "   \nAPI call failed after 1 retries: timeout";
    assert!(detect_api_failure(out).is_some());
}

#[test]
fn detect_api_failure_returns_none_for_normal_reply() {
    let out = "Sure, I can help with that. Let me check the file.";
    assert!(detect_api_failure(out).is_none());
}

#[test]
fn detect_api_failure_returns_none_when_phrase_is_quoted() {
    // The error phrase appears mid-sentence — agent is talking ABOUT errors,
    // not failing. Must not trigger.
    let out = "When the API call failed after 3 retries you should investigate.";
    assert!(detect_api_failure(out).is_none());
}

#[test]
fn detect_api_failure_returns_only_first_line() {
    let out = "API call failed after 3 retries: Invalid length\n\nstack trace here";
    let err = detect_api_failure(out).unwrap();
    assert!(!err.contains("stack trace"));
}

#[test]
fn build_prompt_payload_prepends_system_prompt_for_acp() {
    let payload = build_prompt_payload("events", Some("gitim system"));

    assert!(payload.starts_with("gitim system\n\n---\n\nevents"));
}

#[test]
fn build_prompt_payload_ignores_empty_system_prompt() {
    let payload = build_prompt_payload("events", Some(""));

    assert_eq!(payload, "events");
}

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
