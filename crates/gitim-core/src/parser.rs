use regex::Regex;
use std::sync::LazyLock;
use thiserror::Error;
use crate::link::extract_links;
use crate::mention::extract_mentions;
use crate::types::{Handler, Message, ThreadFile};

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

static MSG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\[L(\d{6,})\]\[P(\d{6,})\]\[@([a-z0-9-]+)\]\[(\d{8}T\d{6}Z)\] (.+)$").unwrap()
});

pub fn parse_thread(input: &str) -> Result<ThreadFile, ParseError> {
    let input = &input.replace("\r\n", "\n");
    if input.is_empty() {
        return Ok(ThreadFile { messages: vec![] });
    }

    let mut messages: Vec<Message> = Vec::new();
    let mut current_body: Option<String> = None;
    let mut first_content_line = true;

    for (file_line_idx, line) in input.lines().enumerate() {
        if let Some(caps) = MSG_RE.captures(line) {
            if let (Some(body), Some(msg)) = (current_body.take(), messages.last_mut()) {
                msg.body = body;
                msg.mentions = extract_mentions(&msg.body);
                msg.links = extract_links(&msg.body);
            }

            let line_number: u64 = caps[1].parse().unwrap();
            let point_to: u64 = caps[2].parse().unwrap();
            let author = Handler::new(&caps[3]).map_err(|e| ParseError::InvalidHandler {
                line: file_line_idx + 1,
                source: e,
            })?;
            let timestamp = caps[4].to_string();
            let body_first_line = caps[5].to_string();

            messages.push(Message {
                line_number,
                point_to,
                author,
                timestamp,
                body: String::new(),
                mentions: Vec::new(),
                links: Vec::new(),
            });
            current_body = Some(body_first_line);
            first_content_line = false;
        } else {
            if first_content_line {
                return Err(ParseError::FirstLineNotMessage(file_line_idx + 1));
            }
            if let Some(ref mut body) = current_body {
                // Strip leading space escape if the remainder starts with [L (spec 5.3 rule 5)
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

    if let (Some(body), Some(msg)) = (current_body, messages.last_mut()) {
        msg.body = body;
        msg.mentions = extract_mentions(&msg.body);
        msg.links = extract_links(&msg.body);
    }

    Ok(ThreadFile { messages })
}
