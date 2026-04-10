use serde::Deserialize;
use serde_json::Value;

use crate::Event;

/// Parsed result from a single line of Claude stream-json output.
#[derive(Debug)]
pub enum ParsedMessage {
    /// System init message with session ID.
    System { session_id: String },
    /// One or more events extracted from an assistant/user message.
    Events(Vec<Event>),
    /// Final result.
    Result {
        session_id: String,
        output: String,
        is_error: bool,
    },
    /// Permission control request requiring a response on stdin.
    ControlRequest { request_id: String, input: Value },
}

/// Parse a single line of Claude stream-json output.
/// Returns None for empty lines, malformed JSON, or unrecognized message types.
pub fn parse_line(line: &str) -> Option<ParsedMessage> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }

    let raw: RawMessage = serde_json::from_str(line).ok()?;

    match raw.r#type.as_str() {
        "system" => Some(ParsedMessage::System {
            session_id: raw.session_id.unwrap_or_default(),
        }),
        "assistant" | "user" => {
            let content: MessageContent = serde_json::from_value(raw.message?).ok()?;
            let events = parse_content_blocks(&content);
            if events.is_empty() {
                None
            } else {
                Some(ParsedMessage::Events(events))
            }
        }
        "result" => Some(ParsedMessage::Result {
            session_id: raw.session_id.unwrap_or_default(),
            output: raw.result.unwrap_or_default(),
            is_error: raw.is_error.unwrap_or(false),
        }),
        "log" => {
            let log = raw.log?;
            Some(ParsedMessage::Events(vec![Event::Log {
                level: log.level,
                content: log.message,
            }]))
        }
        "control_request" => {
            let request: ControlRequestPayload = serde_json::from_value(raw.request?).ok()?;
            let input = request.input.unwrap_or(Value::Object(Default::default()));
            Some(ParsedMessage::ControlRequest {
                request_id: raw.request_id?,
                input,
            })
        }
        _ => None,
    }
}

fn parse_content_blocks(content: &MessageContent) -> Vec<Event> {
    let mut events = Vec::new();
    for block in &content.content {
        match block.r#type.as_str() {
            "text" => {
                if let Some(text) = &block.text {
                    if !text.is_empty() {
                        events.push(Event::Text {
                            content: text.clone(),
                        });
                    }
                }
            }
            "thinking" => {
                if let Some(text) = &block.text {
                    if !text.is_empty() {
                        events.push(Event::Thinking {
                            content: text.clone(),
                        });
                    }
                }
            }
            "tool_use" => {
                let input = block
                    .input
                    .clone()
                    .unwrap_or(Value::Object(Default::default()));
                events.push(Event::ToolUse {
                    tool: block.name.clone().unwrap_or_default(),
                    call_id: block.id.clone().unwrap_or_default(),
                    input,
                });
            }
            "tool_result" => {
                let output = block
                    .tool_result_content
                    .as_ref()
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default();
                events.push(Event::ToolResult {
                    call_id: block.tool_use_id.clone().unwrap_or_default(),
                    output,
                });
            }
            _ => {}
        }
    }
    events
}

// ── Claude SDK JSON types (internal) ──

#[derive(Deserialize)]
struct RawMessage {
    r#type: String,
    #[serde(default)]
    message: Option<Value>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    is_error: Option<bool>,
    #[serde(default)]
    log: Option<LogEntry>,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    request: Option<Value>,
}

#[derive(Deserialize)]
struct LogEntry {
    level: String,
    message: String,
}

#[derive(Deserialize)]
struct MessageContent {
    #[serde(default)]
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    r#type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    input: Option<Value>,
    #[serde(default)]
    tool_use_id: Option<String>,
    /// The "content" field in tool_result blocks.
    /// Renamed to avoid conflict with the struct field name in ContentBlock list.
    #[serde(default, rename = "content")]
    tool_result_content: Option<Value>,
}

#[derive(Deserialize)]
struct ControlRequestPayload {
    #[serde(default)]
    input: Option<Value>,
}
