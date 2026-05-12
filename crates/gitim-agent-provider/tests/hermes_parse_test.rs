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
fn build_prompt_payload_does_not_inject_system_prompt() {
    // The runtime's system prompt must NOT enter hermes' conversation
    // history — it lives in SOUL.md (managed by hermes_profile) so that
    // hermes' frozen system-prompt slot owns it and the in-loop compressor
    // cannot summarise it away. Earlier versions of this function prepended
    // the runtime system prompt to the first user payload; that is exactly
    // the regression this test guards against.
    let payload = build_prompt_payload("events");

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
fn parse_usage_update_extracts_camelcase_tokens() {
    // Hermes' own mid-stream push uses camelCase; the parse_acp_usage helper
    // accepts both naming conventions so the same code path also handles
    // ACP-spec snake_case on the prompt response.
    let params = json!({
        "sessionId": "s-1",
        "update": {
            "sessionUpdate": "usage_update",
            "usage": {
                "inputTokens": 1250,
                "outputTokens": 340,
                "cacheReadInputTokens": 95000,
                "cacheCreationInputTokens": 200
            }
        }
    });
    let msg = parse_notification(&params).unwrap();
    let ParsedNotification::Usage(u) = msg else {
        panic!("expected Usage variant, got {msg:?}");
    };
    assert_eq!(u.input_tokens, Some(1_250));
    assert_eq!(u.output_tokens, Some(340));
    assert_eq!(u.cache_read_tokens, Some(95_000));
    assert_eq!(u.cache_creation_tokens, Some(200));
    assert!(u.used_percent.is_none());
}

#[test]
fn parse_usage_update_accepts_acp_snake_case() {
    // Defensive: if a future hermes build (or a different ACP server)
    // uses spec-compliant snake_case on the same notification path,
    // the parser should still pick it up.
    let params = json!({
        "sessionId": "s-1",
        "update": {
            "sessionUpdate": "usage_update",
            "usage": {"input_tokens": 50, "output_tokens": 10}
        }
    });
    let msg = parse_notification(&params).unwrap();
    let ParsedNotification::Usage(u) = msg else {
        panic!("expected Usage variant");
    };
    assert_eq!(u.input_tokens, Some(50));
    assert_eq!(u.output_tokens, Some(10));
}

#[test]
fn parse_usage_update_with_empty_usage_returns_none() {
    let params = json!({
        "sessionId": "s-1",
        "update": {"sessionUpdate": "usage_update", "usage": {}}
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
