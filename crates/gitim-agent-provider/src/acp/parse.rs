//! ACP wire-level parsing — pure, stateless functions decoding the
//! JSON-RPC notification payloads `hermes` (and eventually `kimi`) emit
//! over their stdout stream.
//!
//! These helpers are deliberately decoupled from any provider-specific
//! state so they can be unit-tested directly against captured fixtures.
//! See `multica/server/pkg/agent/hermes.go` for the upstream Go decoder
//! this is translated from.

use serde_json::Value;

use crate::ProviderUsage;

/// Parsed result from a `session/update` notification's `params` object.
///
/// Variants closely follow the ACP `sessionUpdate` taxonomy that hermes
/// emits — `agent_message_chunk` / `agent_thought_chunk` / `tool_call` /
/// `tool_call_update` / `usage_update`. Anything unrecognized returns
/// `None` from [`parse_notification`].
#[derive(Debug)]
pub enum ParsedNotification {
    /// Text content from an agent message chunk.
    Text { content: String },
    /// Thinking / reasoning content.
    Thinking { content: String },
    /// Tool invocation.
    ToolCall {
        tool: String,
        call_id: String,
        input: Value,
    },
    /// Tool result (completed or failed).
    ToolResult { call_id: String, output: String },
    /// Mid-session token usage push. Hermes emits these as
    /// `sessionUpdate: "usage_update"` with camelCase fields, separately
    /// from the snake_case usage on the final session/prompt response.
    Usage(ProviderUsage),
}

/// Detect a hermes-internal API failure that has been streamed as plain
/// assistant text rather than a JSON-RPC error. Hermes catches LLM API
/// exceptions in its agent loop and turns them into a `final_response`
/// string, so the ACP `session/prompt` reply still looks successful
/// (`stop_reason=end_turn`, no `error` field) — but the agent never
/// actually runs any tools. We have to fall back to substring matching
/// against the stable error prefixes hermes emits, otherwise the
/// runtime reports "done" while the user sees no IM reply.
///
/// Returns the first line of the output (trimmed) when it starts with a
/// known failure prefix; `None` otherwise. Kept here (rather than inside
/// `hermes/`) because it is a pure parse-like helper; only hermes
/// currently calls it, kimi v1 deliberately does not.
pub fn detect_api_failure(output: &str) -> Option<String> {
    const KNOWN_PREFIXES: &[&str] = &[
        // Botocore retry wrapper around AWS Bedrock / Anthropic / OpenAI
        "API call failed after",
        // Botocore validation
        "Parameter validation failed",
    ];
    let trimmed = output.trim_start();
    for prefix in KNOWN_PREFIXES {
        if trimmed.starts_with(prefix) {
            let line = trimmed.lines().next()?.trim();
            return Some(line.to_string());
        }
    }
    None
}

/// Parse the `params` object from a `session/update` JSON-RPC notification.
/// Returns `None` for unrecognized or ignorable update types.
pub fn parse_notification(params: &Value) -> Option<ParsedNotification> {
    let update = params.get("update")?;
    let update_type = update.get("sessionUpdate")?.as_str()?;

    match update_type {
        "agent_message_chunk" => {
            let text = update.get("content")?.get("text")?.as_str()?;
            if text.is_empty() {
                return None;
            }
            Some(ParsedNotification::Text {
                content: text.to_string(),
            })
        }
        "agent_thought_chunk" => {
            let text = update.get("content")?.get("text")?.as_str()?;
            if text.is_empty() {
                return None;
            }
            Some(ParsedNotification::Thinking {
                content: text.to_string(),
            })
        }
        "tool_call" => {
            let call_id = update.get("toolCallId")?.as_str()?.to_string();
            let title = update.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let tool = title
                .split(':')
                .next()
                .unwrap_or("unknown")
                .trim()
                .to_string();
            let input = update
                .get("rawInput")
                .cloned()
                .unwrap_or(Value::Object(Default::default()));
            Some(ParsedNotification::ToolCall {
                tool,
                call_id,
                input,
            })
        }
        "tool_call_update" => {
            let status = update.get("status")?.as_str()?;
            if status != "completed" && status != "failed" {
                return None;
            }
            let call_id = update.get("toolCallId")?.as_str()?.to_string();
            let output = update
                .get("rawOutput")
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            Some(ParsedNotification::ToolResult { call_id, output })
        }
        "usage_update" => parse_acp_usage(update.get("usage")?).map(ParsedNotification::Usage),
        _ => None,
    }
}

/// Map an ACP `usage` object to the provider-agnostic [`ProviderUsage`].
///
/// ACP surfaces usage in two shapes:
/// 1. `session/prompt` response — ACP snake_case (`input_tokens`, …)
/// 2. `session/update` `usage_update` — hermes' camelCase
///    (`inputTokens`, …)
///
/// Try snake_case first (ACP spec), camelCase as fallback. Returns
/// `None` when none of the four counts are present, so an empty
/// `usage: {}` does not fabricate a 0% snapshot.
pub fn parse_acp_usage(v: &Value) -> Option<ProviderUsage> {
    let obj = v.as_object()?;
    let pick = |snake: &str, camel: &str| -> Option<u64> {
        obj.get(snake)
            .or_else(|| obj.get(camel))
            .and_then(Value::as_u64)
    };
    let input = pick("input_tokens", "inputTokens");
    let output = pick("output_tokens", "outputTokens");
    let cache_read = pick("cache_read_input_tokens", "cacheReadInputTokens");
    let cache_creation = pick("cache_creation_input_tokens", "cacheCreationInputTokens");
    if input.is_none() && output.is_none() && cache_read.is_none() && cache_creation.is_none() {
        return None;
    }
    Some(ProviderUsage {
        input_tokens: input,
        output_tokens: output,
        used_percent: None,
        cache_read_tokens: cache_read,
        cache_creation_tokens: cache_creation,
        context_tokens: None,
        context_window_tokens: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── detect_api_failure ─────────────────────────────────────────

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

    // ── parse_notification ─────────────────────────────────────────

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
        assert!(
            matches!(msg, ParsedNotification::ToolCall { ref tool, .. } if tool == "file_edit")
        );
    }

    // ── parse_acp_usage shape contract ─────────────────────────────

    #[test]
    fn usage_update_is_parsed_but_runtime_drops_it() {
        // The parser still surfaces ParsedNotification::Usage for mid-stream
        // events — handlers may want progress signals — but the session
        // driver is required to drop these without overwriting latest_usage.
        // This test only proves the parser still recognizes the shape; the
        // drop-on-floor invariant is upheld by code review of the driver.
        let params = json!({
            "update": {
                "sessionUpdate": "usage_update",
                "usage": {
                    "inputTokens": 1234,
                    "outputTokens": 56,
                    "cacheReadInputTokens": 7890,
                    "cacheCreationInputTokens": 12,
                },
            },
        });
        let parsed = parse_notification(&params).expect("usage_update parses");
        match parsed {
            ParsedNotification::Usage(u) => {
                assert_eq!(u.input_tokens, Some(1234));
                assert_eq!(u.output_tokens, Some(56));
                assert_eq!(u.cache_read_tokens, Some(7890));
                assert_eq!(u.cache_creation_tokens, Some(12));
            }
            _ => panic!("expected Usage variant"),
        }
    }

    #[test]
    fn prompt_response_usage_uses_snake_case() {
        // Hermes wraps Claude today and relays Anthropic-style snake_case on the
        // ACP id=3 prompt response. parse_acp_usage exercises the same helper
        // the session driver calls when it sees a prompt response.
        let usage = parse_acp_usage(&json!({
            "input_tokens": 999,
            "output_tokens": 42,
            "cache_read_input_tokens": 11_000,
            "cache_creation_input_tokens": 100,
        }))
        .expect("prompt response usage parses");
        assert_eq!(usage.input_tokens, Some(999));
        assert_eq!(usage.output_tokens, Some(42));
        assert_eq!(usage.cache_read_tokens, Some(11_000));
        assert_eq!(usage.cache_creation_tokens, Some(100));
    }

    #[test]
    fn empty_usage_object_returns_none() {
        // Both the prompt-response parser and the usage_update parser must
        // return None on empty objects so the runtime never fabricates a 0-token
        // delta from a malformed event.
        assert!(parse_acp_usage(&json!({})).is_none());
        let params = json!({"update": {"sessionUpdate": "usage_update", "usage": {}}});
        assert!(parse_notification(&params).is_none());
    }
}
