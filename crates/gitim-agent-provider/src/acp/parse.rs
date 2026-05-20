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
        input: Option<Value>,
        args_text: String,
    },
    /// Tool invocation progress / completion.
    ToolCallUpdate {
        tool: String,
        call_id: String,
        status: String,
        input: Option<Value>,
        output: Option<String>,
        args_text: String,
    },
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
            let tool = extract_tool_name(update);
            let input = extract_tool_input(update);
            let args_text = extract_tool_content_text(update);
            Some(ParsedNotification::ToolCall {
                tool,
                call_id,
                input,
                args_text,
            })
        }
        "tool_call_update" => {
            let status = update.get("status")?.as_str()?;
            let call_id = update.get("toolCallId")?.as_str()?.to_string();
            let tool = extract_tool_name(update);
            let input = extract_tool_input(update);
            let output = extract_tool_output(update);
            let args_text = extract_tool_content_text(update);
            Some(ParsedNotification::ToolCallUpdate {
                tool,
                call_id,
                status: status.to_string(),
                input,
                output,
                args_text,
            })
        }
        "usage_update" => parse_acp_usage(update.get("usage")?).map(ParsedNotification::Usage),
        _ => None,
    }
}

fn extract_tool_name(update: &Value) -> String {
    let raw = update
        .get("title")
        .or_else(|| update.get("name"))
        .or_else(|| update.get("kind"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    raw.split(':')
        .next()
        .unwrap_or("unknown")
        .trim()
        .to_string()
}

fn extract_tool_input(update: &Value) -> Option<Value> {
    ["rawInput", "input", "parameters"]
        .iter()
        .find_map(|key| update.get(*key).cloned())
}

fn extract_tool_output(update: &Value) -> Option<String> {
    update
        .get("rawOutput")
        .or_else(|| update.get("output"))
        .map(|v| match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
}

fn extract_tool_content_text(update: &Value) -> String {
    let Some(blocks) = update.get("content").and_then(|v| v.as_array()) else {
        return String::new();
    };

    let mut pieces = Vec::new();
    for block in blocks {
        let kind = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match kind {
            "content" => {
                if let Some(text) = block
                    .get("content")
                    .and_then(|v| v.get("text"))
                    .and_then(|v| v.as_str())
                {
                    if !text.is_empty() {
                        pieces.push(text.to_string());
                    }
                }
            }
            "text" => {
                if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                    if !text.is_empty() {
                        pieces.push(text.to_string());
                    }
                }
            }
            "diff" => {
                let path = block.get("path").and_then(|v| v.as_str()).unwrap_or("");
                if path.is_empty() {
                    continue;
                }
                let old_len = block
                    .get("oldText")
                    .and_then(|v| v.as_str())
                    .map(str::len)
                    .unwrap_or(0);
                let new_len = block
                    .get("newText")
                    .and_then(|v| v.as_str())
                    .map(str::len)
                    .unwrap_or(0);
                let summary = if old_len == 0 {
                    format!("--- {path}\n+++ {path}\n(new file, {new_len} bytes)")
                } else {
                    format!("--- {path}\n+++ {path}\n(edited: {old_len} -> {new_len} bytes)")
                };
                pieces.push(summary);
            }
            _ => {}
        }
    }
    pieces.join("\n")
}

/// Map an ACP `usage` object to the provider-agnostic [`ProviderUsage`].
///
/// ACP surfaces usage in a few shapes:
/// 1. `session/prompt` response — ACP schema aliases (`inputTokens`,
///    `cachedReadTokens`, `thoughtTokens`, …)
/// 2. older prompt/notification shapes — snake_case (`input_tokens`, …)
/// 3. older Hermes `usage_update` notifications — camelCase
///    (`cacheReadInputTokens`, …)
///
/// Returns `None` when none of the billing counts are present, so an empty
/// `usage: {}` does not fabricate a 0% snapshot.
pub fn parse_acp_usage(v: &Value) -> Option<ProviderUsage> {
    let obj = v.as_object()?;
    let pick = |keys: &[&str]| -> Option<u64> {
        keys.iter()
            .find_map(|key| obj.get(*key).and_then(Value::as_u64))
    };
    let input = pick(&["input_tokens", "inputTokens"]);
    let output = add_optional_tokens(
        pick(&["output_tokens", "outputTokens"]),
        pick(&["thought_tokens", "thoughtTokens"]),
    );
    let cache_read = pick(&[
        "cache_read_input_tokens",
        "cacheReadInputTokens",
        "cached_read_tokens",
        "cachedReadTokens",
    ]);
    let cache_creation = pick(&[
        "cache_creation_input_tokens",
        "cacheCreationInputTokens",
        "cached_write_tokens",
        "cachedWriteTokens",
        "cacheWriteTokens",
    ]);
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

fn add_optional_tokens(current: Option<u64>, next: Option<u64>) -> Option<u64> {
    if current.is_none() && next.is_none() {
        return None;
    }
    Some(current.unwrap_or(0).saturating_add(next.unwrap_or(0)))
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
            ParsedNotification::ToolCall {
                tool,
                call_id,
                input,
                ..
            } => {
                assert_eq!(tool, "terminal");
                assert_eq!(call_id, "tc-1");
                assert_eq!(input, Some(json!({"command": "ls -la"})));
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
            ParsedNotification::ToolCallUpdate {
                call_id,
                output,
                status,
                ..
            } => {
                assert_eq!(call_id, "tc-1");
                assert_eq!(status, "completed");
                assert_eq!(output.as_deref(), Some("file1.rs\nfile2.rs"));
            }
            _ => panic!("expected ToolCallUpdate"),
        }
    }

    #[test]
    fn parse_tool_call_update_pending_is_kept_for_streamed_args() {
        let params = json!({
            "sessionId": "s-1",
            "update": {
                "sessionUpdate": "tool_call_update",
                "toolCallId": "tc-1",
                "status": "pending",
                "content": [{"type": "content", "content": {"type": "text", "text": "{\"command\":\"echo hi\"}"}}]
            }
        });
        let msg = parse_notification(&params).unwrap();
        match msg {
            ParsedNotification::ToolCallUpdate {
                call_id,
                status,
                args_text,
                ..
            } => {
                assert_eq!(call_id, "tc-1");
                assert_eq!(status, "pending");
                assert_eq!(args_text, "{\"command\":\"echo hi\"}");
            }
            _ => panic!("expected ToolCallUpdate"),
        }
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
    fn prompt_response_usage_accepts_acp_schema_aliases() {
        let usage = parse_acp_usage(&json!({
            "inputTokens": 999,
            "outputTokens": 42,
            "totalTokens": 12_345,
            "cachedReadTokens": 11_000,
            "cachedWriteTokens": 100,
            "thoughtTokens": 7,
        }))
        .expect("prompt response usage parses");
        assert_eq!(usage.input_tokens, Some(999));
        assert_eq!(usage.output_tokens, Some(49));
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
