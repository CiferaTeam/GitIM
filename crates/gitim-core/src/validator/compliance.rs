use crate::parser::parse_thread;
use crate::types::ThreadEntry;
use thiserror::Error;
use std::collections::HashSet;

#[derive(Error, Debug)]
pub enum ComplianceError {
    #[error("parse error: {0}")]
    Parse(#[from] crate::parser::ParseError),
    #[error("line number not continuous: expected L{expected:06}, got L{got:06}")]
    LineNumberGap { expected: u64, got: u64 },
    #[error("unknown author '@{0}' not in users/")]
    UnknownAuthor(String),
    #[error("invalid P reference: P{0:06} does not exist")]
    InvalidPointTo(u64),
    #[error("message L{0:06} has empty body")]
    EmptyBody(u64),
    #[error("unknown mention '<@{handler}>' in message L{line_number:06}")]
    UnknownMention { handler: String, line_number: u64 },
}

#[derive(Debug)]
pub struct AppendValidation;

pub fn validate_append(
    existing: &str,
    new_lines: &str,
    registered_users: &[&str],
) -> Result<AppendValidation, ComplianceError> {
    let existing_file = parse_thread(existing)?;
    let new_file = parse_thread(new_lines)?;

    let max_existing = existing_file.last_line_number();

    let mut known_lines: HashSet<u64> = existing_file
        .entries
        .iter()
        .map(|e| e.line_number())
        .collect();

    let user_set: HashSet<&str> = registered_users.iter().copied().collect();

    let mut expected_next = max_existing + 1;

    for entry in &new_file.entries {
        let ln = entry.line_number();
        if ln != expected_next {
            return Err(ComplianceError::LineNumberGap {
                expected: expected_next,
                got: ln,
            });
        }

        if !user_set.contains(entry.author().as_str()) {
            return Err(ComplianceError::UnknownAuthor(entry.author().to_string()));
        }

        match entry {
            ThreadEntry::Message(msg) => {
                if msg.point_to != 0 && !known_lines.contains(&msg.point_to) {
                    return Err(ComplianceError::InvalidPointTo(msg.point_to));
                }

                for mention in &msg.mentions {
                    if !user_set.contains(mention.as_str()) {
                        return Err(ComplianceError::UnknownMention {
                            handler: mention.to_string(),
                            line_number: ln,
                        });
                    }
                }

                if msg.body.trim().is_empty() {
                    return Err(ComplianceError::EmptyBody(ln));
                }
            }
            ThreadEntry::Event(_ev) => {
                // Events: line number continuity and author checks already done above.
                // Full event validation will be added in Task 6.
            }
        }

        known_lines.insert(ln);
        expected_next = ln + 1;
    }

    Ok(AppendValidation)
}
