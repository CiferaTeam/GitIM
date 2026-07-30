use serde::{Deserialize, Serialize};

use super::{RevisionId, SkillError, SkillSlug};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillReference {
    pub slug: SkillSlug,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<RevisionId>,
}

pub fn parse_skill_reference(value: &str) -> Result<SkillReference, SkillError> {
    let body = value
        .strip_prefix("skill:")
        .ok_or(SkillError::InvalidPackage)?;
    let (slug, revision) = match body.split_once('@') {
        Some((slug, revision)) if !revision.contains('@') => {
            (SkillSlug::new(slug)?, Some(RevisionId::new(revision)?))
        }
        Some(_) => return Err(SkillError::InvalidPackage),
        None => (SkillSlug::new(body)?, None),
    };
    Ok(SkillReference { slug, revision })
}

pub fn scan_skill_references(markdown: &str) -> Vec<SkillReference> {
    let mut references = Vec::new();
    let mut index = 0;
    let mut inline_code_delimiter = None;
    let mut fenced_code_delimiter = None;
    let mut link_destination_depth = 0_usize;
    let bytes = markdown.as_bytes();

    while index < bytes.len() {
        let delimiter = bytes[index];
        let run_length = bytes[index..]
            .iter()
            .take_while(|byte| **byte == delimiter)
            .count();

        if let Some((fence, minimum_length)) = fenced_code_delimiter {
            if delimiter == fence
                && run_length >= minimum_length
                && is_fence_closer(markdown, index, run_length)
            {
                fenced_code_delimiter = None;
                index += run_length;
            } else {
                index += markdown[index..].chars().next().map_or(1, char::len_utf8);
            }
            continue;
        }

        if let Some(inline_length) = inline_code_delimiter {
            if delimiter == b'`' && run_length == inline_length {
                inline_code_delimiter = None;
                index += run_length;
            } else if delimiter == b'`' {
                index += run_length;
            } else {
                index += markdown[index..].chars().next().map_or(1, char::len_utf8);
            }
            continue;
        }

        if link_destination_depth > 0 {
            match bytes[index] {
                b'(' => link_destination_depth += 1,
                b')' => link_destination_depth -= 1,
                _ => {}
            }
            index += markdown[index..].chars().next().map_or(1, char::len_utf8);
            continue;
        }

        if bytes[index] == b'\\' {
            index += '\\'.len_utf8();
            index += markdown[index..].chars().next().map_or(0, char::len_utf8);
            continue;
        }

        if matches!(delimiter, b'`' | b'~')
            && run_length >= 3
            && is_fence_opener(markdown, index, delimiter, run_length)
        {
            fenced_code_delimiter = Some((delimiter, run_length));
            index += run_length;
            continue;
        }

        if delimiter == b'`' && run_length > 0 {
            inline_code_delimiter = Some(run_length);
            index += run_length;
            continue;
        }

        if bytes[index] == b'(' && index > 0 && bytes[index - 1] == b']' {
            link_destination_depth = 1;
            index += 1;
            continue;
        }

        if markdown[index..].starts_with("skill:") && has_valid_left_boundary(markdown, index) {
            if let Some((reference, end)) = parse_reference_at(markdown, index) {
                if has_valid_right_boundary(markdown, end) {
                    references.push(reference);
                    index = end;
                    continue;
                }
            }
        }

        index += markdown[index..].chars().next().map_or(1, char::len_utf8);
    }

    references
}

fn parse_reference_at(value: &str, start: usize) -> Option<(SkillReference, usize)> {
    let mut end = start + "skill:".len();
    while let Some(character) = value[end..].chars().next() {
        if matches!(character, 'a'..='z' | '0'..='9' | '-') {
            end += character.len_utf8();
        } else {
            break;
        }
    }

    if end == start + "skill:".len() {
        return None;
    }

    if value[end..].starts_with('@') {
        let revision_end = end + 1 + 28;
        let revision = value.get(end + 1..revision_end)?;
        let reference =
            parse_skill_reference(&format!("{}@{revision}", &value[start..end])).ok()?;
        return Some((reference, revision_end));
    }

    let reference = parse_skill_reference(&value[start..end]).ok()?;
    Some((reference, end))
}

fn has_valid_left_boundary(value: &str, start: usize) -> bool {
    value[..start]
        .chars()
        .next_back()
        .is_none_or(|character| !is_reference_boundary_character(character))
}

fn has_valid_right_boundary(value: &str, end: usize) -> bool {
    value[end..]
        .chars()
        .next()
        .is_none_or(|character| !is_reference_boundary_character(character))
}

fn is_reference_boundary_character(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | '/' | '\\' | '-')
}

fn is_fence_opener(markdown: &str, index: usize, delimiter: u8, run_length: usize) -> bool {
    if !is_fence_line_start(markdown, index) {
        return false;
    }

    delimiter != b'`' || !line_remainder(markdown, index + run_length).contains('`')
}

fn is_fence_closer(markdown: &str, index: usize, run_length: usize) -> bool {
    is_fence_line_start(markdown, index)
        && line_remainder(markdown, index + run_length)
            .chars()
            .all(|character| matches!(character, ' ' | '\t'))
}

fn is_fence_line_start(markdown: &str, index: usize) -> bool {
    let line_start = markdown[..index]
        .rfind('\n')
        .map_or(0, |newline| newline + 1);
    let indentation = &markdown[line_start..index];
    indentation.len() <= 3 && indentation.bytes().all(|byte| byte == b' ')
}

fn line_remainder(markdown: &str, index: usize) -> &str {
    let line_end = markdown[index..]
        .find('\n')
        .map_or(markdown.len(), |offset| index + offset);
    markdown[index..line_end]
        .strip_suffix('\r')
        .unwrap_or(&markdown[index..line_end])
}
