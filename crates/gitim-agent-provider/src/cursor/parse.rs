//! Parse cursor-agent's stream-json envelopes into typed events the
//! provider driver can dispatch. The envelope shape is documented inline
//! in `CursorStreamEvent`. See `multica/server/pkg/agent/cursor.go:282+`
//! for the reference Go decoder this is translated from.

use serde::Deserialize;
use serde_json::Value;

/// One line off cursor-agent's stdout stream. `type` is the dispatch key;
/// the other fields are populated per-type (most are absent on any given
/// event — `serde(default)` keeps deserialization permissive).
#[derive(Debug, Deserialize, Default)]
pub struct CursorStreamEvent {
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub subtype: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// For `assistant` events: `{ content: [ { type, text|input|name|id } ] }`.
    #[serde(default)]
    pub message: Option<Value>,
    /// For `tool_use` standalone envelopes (NOT the assistant-embedded form).
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_id: Option<String>,
    #[serde(default)]
    pub parameters: Option<Value>,
    /// For `tool_result` standalone envelopes.
    #[serde(default)]
    pub output: Option<String>,
    /// For `result` envelopes.
    #[serde(default)]
    pub is_error: bool,
    #[serde(default, rename = "result")]
    pub result_text: Option<String>,
    #[serde(default)]
    pub usage: Option<CursorUsage>,
    /// For `text` and `step_finish` envelopes.
    #[serde(default)]
    pub part: Option<Value>,
    /// For `error` / `system{subtype:error}` envelopes — see
    /// `cursor_error_text` for the precedence (error > detail > result).
    #[serde(default, rename = "error")]
    pub error_msg: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct CursorUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    /// Note: multica uses `cached_input_tokens` here (cursor.go:325), not
    /// `cache_read_input_tokens`. Accept both names for resilience — the
    /// rename is documented in cursor.go but the field name may drift.
    #[serde(default, alias = "cached_input_tokens")]
    pub cache_read_input_tokens: u64,
}

pub fn parse_event(line: &str) -> Option<CursorStreamEvent> {
    serde_json::from_str::<CursorStreamEvent>(line.trim()).ok()
}

/// Pull the best human-readable error text out of an error/system-error
/// envelope. Precedence matches `multica/cursor.go:375-385`:
/// `error` → `detail` → `result`.
pub fn cursor_error_text(evt: &CursorStreamEvent) -> String {
    if let Some(s) = &evt.error_msg {
        if !s.is_empty() {
            return s.clone();
        }
    }
    if let Some(s) = &evt.detail {
        if !s.is_empty() {
            return s.clone();
        }
    }
    if let Some(s) = &evt.result_text {
        if !s.is_empty() {
            return s.clone();
        }
    }
    String::new()
}

/// Strip cursor-agent's optional `stdout:` / `stderr:` prefix that sometimes
/// fronts a stream-json line. Reference: `multica/cursor.go:361-373`.
pub fn normalize_stream_line(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // Detect prefix case-insensitively: `(stdout|stderr)` optionally followed
    // by whitespace, `:` or `=`, more whitespace, then the JSON.
    let lower = trimmed.to_ascii_lowercase();
    for prefix in ["stdout", "stderr"] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let mut idx = prefix.len();
            // Consume optional spaces, then optional `:` or `=`, then more spaces.
            let bytes = rest.as_bytes();
            let mut i = 0;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < bytes.len() && (bytes[i] == b':' || bytes[i] == b'=') {
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                idx += i;
                return trimmed[idx..].trim().to_string();
            }
        }
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_system_init() {
        let e = parse_event(r#"{"type":"system","subtype":"init","session_id":"s1"}"#).unwrap();
        assert_eq!(e.r#type, "system");
        assert_eq!(e.subtype.as_deref(), Some("init"));
        assert_eq!(e.session_id.as_deref(), Some("s1"));
    }

    #[test]
    fn parses_assistant_text_block() {
        let e = parse_event(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#,
        )
        .unwrap();
        let content = e.message.unwrap();
        let blocks = content.get("content").unwrap().as_array().unwrap();
        assert_eq!(blocks[0].get("type").unwrap(), "text");
        assert_eq!(blocks[0].get("text").unwrap(), "hi");
    }

    #[test]
    fn parses_tool_use_envelope() {
        let e = parse_event(
            r#"{"type":"tool_use","tool_name":"read_file","tool_id":"t1","parameters":{"path":"foo"}}"#,
        )
        .unwrap();
        assert_eq!(e.tool_name.as_deref(), Some("read_file"));
        assert_eq!(e.tool_id.as_deref(), Some("t1"));
        assert_eq!(e.parameters.unwrap().get("path").unwrap(), "foo");
    }

    #[test]
    fn parses_tool_result_envelope() {
        let e = parse_event(r#"{"type":"tool_result","tool_id":"t1","output":"file contents"}"#)
            .unwrap();
        assert_eq!(e.tool_id.as_deref(), Some("t1"));
        assert_eq!(e.output.as_deref(), Some("file contents"));
    }

    #[test]
    fn parses_result_with_usage() {
        let e = parse_event(
            r#"{"type":"result","session_id":"s1","usage":{"input_tokens":100,"output_tokens":50,"cached_input_tokens":20},"model":"claude-sonnet-4-6"}"#,
        )
        .unwrap();
        assert_eq!(e.session_id.as_deref(), Some("s1"));
        let u = e.usage.unwrap();
        assert_eq!(u.input_tokens, 100);
        assert_eq!(u.output_tokens, 50);
        assert_eq!(u.cache_read_input_tokens, 20);
    }

    #[test]
    fn returns_none_on_malformed_json() {
        assert!(parse_event("not json").is_none());
        assert!(parse_event("").is_none());
    }

    #[test]
    fn normalize_stream_line_strips_stdout_prefix() {
        assert_eq!(
            normalize_stream_line(r#"stdout: {"type":"result"}"#),
            r#"{"type":"result"}"#
        );
        assert_eq!(
            normalize_stream_line(r#"STDERR={"type":"error"}"#),
            r#"{"type":"error"}"#
        );
        assert_eq!(
            normalize_stream_line(r#"{"type":"system"}"#),
            r#"{"type":"system"}"#
        );
        assert_eq!(normalize_stream_line(""), "");
    }

    #[test]
    fn cursor_error_text_precedence() {
        let mut evt = CursorStreamEvent::default();
        evt.error_msg = Some("primary".to_string());
        evt.detail = Some("fallback".to_string());
        assert_eq!(cursor_error_text(&evt), "primary");

        let mut evt = CursorStreamEvent::default();
        evt.detail = Some("fallback".to_string());
        evt.result_text = Some("third".to_string());
        assert_eq!(cursor_error_text(&evt), "fallback");

        let mut evt = CursorStreamEvent::default();
        evt.result_text = Some("only".to_string());
        assert_eq!(cursor_error_text(&evt), "only");

        let evt = CursorStreamEvent::default();
        assert_eq!(cursor_error_text(&evt), "");
    }
}
