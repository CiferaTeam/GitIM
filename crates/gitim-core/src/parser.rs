use crate::link::extract_links;
use crate::mention::extract_mentions;
use crate::types::{ChannelEvent, Handler, Message, ThreadEntry, ThreadFile};
use regex::Regex;
use std::sync::LazyLock;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ParseError {
    #[error("first non-empty line is not a message start (line {0})")]
    FirstLineNotMessage(usize),
    #[error("invalid handler in message at file line {line}: {source}")]
    InvalidHandler {
        line: usize,
        source: crate::types::handler::HandlerError,
    },
}

// Event tokens are kebab- or snake-case lowercase ASCII. Hyphens are
// part of the alphabet (e.g. `leave-workspace`) — the original `[a-z_]+`
// rejected anything compound and silently fell through the parser
// (offending lines were treated as continuation text).
static MSG_RE: LazyLock<Regex> = LazyLock::new(|| {
    crate::preconditions::regex_literal(
        r"^\[L(\d{6,})\]\[P(\d{6,})\]\[@([a-z0-9-]+)\]\[(\d{8}T\d{6}Z)\](?:\[E:([a-z][a-z0-9_-]*)\])? (.+)$",
    )
});

pub fn parse_thread(input: &str) -> Result<ThreadFile, ParseError> {
    let input = &input.replace("\r\n", "\n");
    if input.is_empty() {
        return Ok(ThreadFile { entries: vec![] });
    }

    let mut entries: Vec<ThreadEntry> = Vec::new();
    let mut current_body: Option<String> = None;
    let mut first_content_line = true;

    for (file_line_idx, line) in input.lines().enumerate() {
        if let Some(caps) = MSG_RE.captures(line) {
            finalize_entry(&mut entries, current_body.take());

            let line_number: u64 = crate::preconditions::parse_u64(&caps[1]);
            let point_to: u64 = crate::preconditions::parse_u64(&caps[2]);
            // `system` is rejected by `Handler::new` (it's reserved so no
            // user can claim it), but the daemon's cron engine emits
            // `[@system]` lines for cron fires. Reading those back has to
            // succeed — without this carve-out, parsing any thread file
            // containing a system message would fail. The factory
            // `Handler::system()` preserves the "no user-input path can
            // forge `system`" invariant; this parser just treats the
            // already-emitted token as the protocol-level constant.
            let raw_handler = &caps[3];
            let author = if raw_handler == "system" {
                Handler::system()
            } else {
                Handler::new(raw_handler).map_err(|e| ParseError::InvalidHandler {
                    line: file_line_idx + 1,
                    source: e,
                })?
            };
            let timestamp = caps[4].to_string();
            let event_type = caps.get(5).map(|m| m.as_str().to_string());
            let body_first_line = caps[6].to_string();

            if let Some(et) = event_type {
                entries.push(ThreadEntry::Event(ChannelEvent {
                    line_number,
                    point_to,
                    author,
                    timestamp,
                    event_type: et,
                    meta: serde_json::Value::Null,
                }));
            } else {
                entries.push(ThreadEntry::Message(Message {
                    line_number,
                    point_to,
                    author,
                    timestamp,
                    body: String::new(),
                    mentions: Vec::new(),
                    links: Vec::new(),
                }));
            }
            current_body = Some(body_first_line);
            first_content_line = false;
        } else {
            if first_content_line {
                return Err(ParseError::FirstLineNotMessage(file_line_idx + 1));
            }
            if let Some(ref mut body) = current_body {
                // Strip leading space escape if the remainder starts with [L (continuation rule)
                let content = if line.starts_with(" [L") {
                    &line[1..]
                } else {
                    line
                };
                body.push('\n');
                body.push_str(content);
            }
        }
    }

    finalize_entry(&mut entries, current_body.take());

    Ok(ThreadFile { entries })
}

fn finalize_entry(entries: &mut [ThreadEntry], body: Option<String>) {
    if let (Some(body), Some(entry)) = (body, entries.last_mut()) {
        match entry {
            ThreadEntry::Message(msg) => {
                msg.body = body;
                msg.mentions = extract_mentions(&msg.body);
                msg.links = extract_links(&msg.body);
            }
            ThreadEntry::Event(ev) => {
                ev.meta = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
            }
        }
    }
}
