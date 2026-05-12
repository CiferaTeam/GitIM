//! Hermes drives `latest_usage` from two sources: the id=3 `session/prompt`
//! response and mid-stream `usage_update` notifications. The runtime token
//! accumulator depends on `ExecResult.usage` carrying a stable per-turn
//! delta, so mid-stream pushes must not feed it.
//!
//! These tests exercise `parse_notification` directly to lock the wire-shape
//! contract; the integration path (drive_session) is verified at runtime via
//! `agent_loop` integration coverage.

use gitim_agent_provider::hermes::{
    parse_acp_usage_for_test, parse_notification, ParsedNotification,
};
use serde_json::json;

#[test]
fn usage_update_is_parsed_but_runtime_drops_it() {
    // The parser still surfaces ParsedNotification::Usage for mid-stream
    // events — handlers may want progress signals — but the hermes session
    // driver is required to drop these without overwriting latest_usage.
    // This test only proves the parser still recognizes the shape; the
    // drop-on-floor invariant is upheld by code review of drive_session.
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
    // ACP id=3 prompt response. parse_acp_usage_for_test exercises the same
    // helper that drive_session calls when it sees a prompt response.
    let usage = parse_acp_usage_for_test(&json!({
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
    assert!(parse_acp_usage_for_test(&json!({})).is_none());
    let params = json!({"update": {"sessionUpdate": "usage_update", "usage": {}}});
    assert!(parse_notification(&params).is_none());
}
