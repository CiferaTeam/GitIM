use crate::parser::parse_thread;
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
}

pub struct AppendValidation;

pub fn validate_append(
    existing: &str,
    new_lines: &str,
    registered_users: &[&str],
) -> Result<AppendValidation, ComplianceError> {
    let existing_file = parse_thread(existing)?;
    let new_file = parse_thread(new_lines)?;

    let max_existing = existing_file
        .messages
        .last()
        .map(|m| m.line_number)
        .unwrap_or(0);

    let mut known_lines: HashSet<u64> = existing_file
        .messages
        .iter()
        .map(|m| m.line_number)
        .collect();

    let user_set: HashSet<&str> = registered_users.iter().copied().collect();

    let mut expected_next = max_existing + 1;

    for msg in &new_file.messages {
        if msg.line_number != expected_next {
            return Err(ComplianceError::LineNumberGap {
                expected: expected_next,
                got: msg.line_number,
            });
        }

        if !user_set.contains(msg.author.as_str()) {
            return Err(ComplianceError::UnknownAuthor(msg.author.to_string()));
        }

        if msg.point_to != 0 && !known_lines.contains(&msg.point_to) {
            return Err(ComplianceError::InvalidPointTo(msg.point_to));
        }

        if msg.body.trim().is_empty() {
            return Err(ComplianceError::EmptyBody(msg.line_number));
        }

        known_lines.insert(msg.line_number);
        expected_next = msg.line_number + 1;
    }

    Ok(AppendValidation)
}
