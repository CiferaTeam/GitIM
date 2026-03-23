use crate::parser::parse_thread;
use std::collections::HashSet;

#[derive(Debug)]
pub enum IntegrityIssue {
    LineNumberGap { expected: u64, got: u64 },
    UnknownAuthor(String),
    InvalidPointTo(u64),
    EmptyBody(u64),
    UnknownMention { handler: String, line_number: u64 },
    ParseError(String),
}

pub fn check_thread_integrity(input: &str, registered_users: &[&str]) -> Vec<IntegrityIssue> {
    let mut issues = Vec::new();

    let file = match parse_thread(input) {
        Ok(f) => f,
        Err(e) => {
            issues.push(IntegrityIssue::ParseError(e.to_string()));
            return issues;
        }
    };

    let user_set: HashSet<&str> = registered_users.iter().copied().collect();
    let mut known_lines: HashSet<u64> = HashSet::new();
    let mut expected_next: u64 = 1;

    for msg in file.messages() {
        if msg.line_number != expected_next {
            issues.push(IntegrityIssue::LineNumberGap {
                expected: expected_next,
                got: msg.line_number,
            });
        }

        if !user_set.contains(msg.author.as_str()) {
            issues.push(IntegrityIssue::UnknownAuthor(msg.author.to_string()));
        }

        if msg.point_to != 0 && !known_lines.contains(&msg.point_to) {
            issues.push(IntegrityIssue::InvalidPointTo(msg.point_to));
        }

        if msg.body.trim().is_empty() {
            issues.push(IntegrityIssue::EmptyBody(msg.line_number));
        }

        for mention in &msg.mentions {
            if !user_set.contains(mention.as_str()) {
                issues.push(IntegrityIssue::UnknownMention {
                    handler: mention.to_string(),
                    line_number: msg.line_number,
                });
            }
        }

        known_lines.insert(msg.line_number);
        expected_next = msg.line_number + 1;
    }

    issues
}
