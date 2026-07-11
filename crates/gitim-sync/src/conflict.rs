use std::collections::HashMap;
use std::path::PathBuf;

use gitim_core::parser::{parse_thread, ParseError};
use gitim_core::types::{
    truncate_quick_session_preview, validate_quick_session_meta, ChannelMeta, QuickSessionMeta,
    QuickSessionStatus, QuickSessionTitleSource, ThreadEntry,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::renumber::{renumber_batch, RenumberError};

#[derive(Error, Debug)]
pub enum ConflictError {
    #[error("renumber error: {0}")]
    Renumber(#[from] RenumberError),
    #[cfg(not(target_arch = "wasm32"))]
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse error: {0}")]
    Parse(#[from] ParseError),
    #[error("quick session conflict cannot be reconciled: {0}")]
    QuickSession(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenumberMapping {
    pub file: PathBuf,
    pub old_line: u64,
    pub new_line: u64,
}

/// Result of resolving conflicts for a single file.
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedFile {
    pub path: PathBuf,
    pub content: String,
}

/// Build commit message for rebased messages.
/// Format: `msg: @author -> channel L000011 L000012 L000013(rebased)`
pub fn build_rebase_commit_msg(
    mappings: &[RenumberMapping],
    local_additions: &HashMap<PathBuf, String>,
) -> String {
    let mut entries: Vec<(String, String, Vec<u64>)> = Vec::new();

    let mut by_file: HashMap<&PathBuf, Vec<u64>> = HashMap::new();
    for m in mappings {
        by_file.entry(&m.file).or_default().push(m.new_line);
    }

    let mut sorted_by_file: Vec<_> = by_file.into_iter().collect();
    sorted_by_file.sort_by(|a, b| a.0.cmp(b.0));
    for (file, new_lines) in &sorted_by_file {
        let channel = file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        if let Some(content) = local_additions.get(*file) {
            if let Ok(parsed) = parse_thread(content) {
                let mut by_author: HashMap<String, Vec<u64>> = HashMap::new();
                for (entry, new_ln) in parsed.entries.iter().zip(new_lines.iter()) {
                    by_author
                        .entry(entry.author().as_str().to_string())
                        .or_default()
                        .push(*new_ln);
                }
                let mut authors: Vec<_> = by_author.into_iter().collect();
                authors.sort_by(|a, b| a.0.cmp(&b.0));
                for (author, lines) in authors {
                    entries.push((author, channel.clone(), lines));
                }
            } else {
                entries.push(("unknown".to_string(), channel.clone(), new_lines.clone()));
            }
        }
    }

    if entries.is_empty() {
        return "msg: sync after rebase".to_string();
    }

    let parts: Vec<String> = entries
        .iter()
        .map(|(author, channel, lines)| {
            let line_parts: Vec<String> = lines.iter().map(|l| format!("L{:06}", l)).collect();
            format!(
                "msg: @{} -> {} {}(rebased)",
                author,
                channel,
                line_parts.join(" ")
            )
        })
        .collect();

    parts.join("\n")
}

/// Pure content transformation: renumber local additions to fit after remote content.
/// Takes already-read remote contents — no filesystem access.
pub fn resolve_content_pure(
    local_additions: &HashMap<PathBuf, String>,
    remote_contents: &HashMap<PathBuf, String>,
) -> Result<(Vec<ResolvedFile>, Vec<RenumberMapping>), ConflictError> {
    let mut all_mappings: Vec<RenumberMapping> = Vec::new();
    let mut resolved_files: Vec<ResolvedFile> = Vec::new();

    let mut sorted_files: Vec<_> = local_additions.keys().collect();
    sorted_files.sort();
    for rel_path in sorted_files {
        let local_content = &local_additions[rel_path];
        let remote_content = remote_contents
            .get(rel_path)
            .map(|s| s.as_str())
            .unwrap_or("");

        let max_line = if remote_content.is_empty() {
            0
        } else {
            let remote_file = parse_thread(remote_content)?;
            remote_file
                .entries
                .iter()
                .map(|e| e.line_number())
                .max()
                .unwrap_or(0)
        };

        let local_file = parse_thread(local_content)?;
        let old_line_numbers: Vec<u64> =
            local_file.entries.iter().map(|e| e.line_number()).collect();

        let renumbered = renumber_batch(local_content, max_line)?;

        let renumbered_file = parse_thread(&renumbered)?;
        let new_line_numbers: Vec<u64> = renumbered_file
            .entries
            .iter()
            .map(|e| e.line_number())
            .collect();

        for (old_ln, new_ln) in old_line_numbers.iter().zip(new_line_numbers.iter()) {
            all_mappings.push(RenumberMapping {
                file: rel_path.clone(),
                old_line: *old_ln,
                new_line: *new_ln,
            });
        }

        let mut final_content = remote_content.to_string();
        if !final_content.is_empty() && !final_content.ends_with('\n') {
            final_content.push('\n');
        }
        final_content.push_str(&renumbered);

        resolved_files.push(ResolvedFile {
            path: rel_path.clone(),
            content: final_content,
        });
    }

    Ok((resolved_files, all_mappings))
}

/// I/O wrapper: reads remote files from filesystem, then delegates to resolve_content_pure.
#[cfg(not(target_arch = "wasm32"))]
pub fn resolve_content(
    local_additions: &HashMap<PathBuf, String>,
    repo_root: &std::path::Path,
) -> Result<(Vec<ResolvedFile>, Vec<RenumberMapping>), ConflictError> {
    let mut remote_contents: HashMap<PathBuf, String> = HashMap::new();

    for rel_path in local_additions.keys() {
        let abs_path = repo_root.join(rel_path);
        if abs_path.exists() {
            remote_contents.insert(rel_path.clone(), std::fs::read_to_string(&abs_path)?);
        } else if let Some(parent) = abs_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
    }

    resolve_content_pure(local_additions, &remote_contents)
}

/// Merge two ChannelMeta: members 取并集（排序去重），标量字段取 remote。
pub fn merge_channel_meta(local: &ChannelMeta, remote: &ChannelMeta) -> ChannelMeta {
    let mut members: Vec<String> = remote.members.clone();
    for m in &local.members {
        if !members.contains(m) {
            members.push(m.clone());
        }
    }
    members.sort();

    ChannelMeta {
        display_name: remote.display_name.clone(),
        created_by: remote.created_by.clone(),
        created_at: remote.created_at.clone(),
        introduction: remote.introduction.clone(),
        members,
        project: remote.project.clone(),
    }
}

pub fn merge_quick_session_meta(
    local: &QuickSessionMeta,
    remote: &QuickSessionMeta,
    merged_thread: &str,
    mappings: &[RenumberMapping],
    thread_path: &std::path::Path,
) -> Result<QuickSessionMeta, ConflictError> {
    validate_quick_session_meta(local)
        .map_err(|error| ConflictError::QuickSession(error.to_string()))?;
    validate_quick_session_meta(remote)
        .map_err(|error| ConflictError::QuickSession(error.to_string()))?;
    if local.id != remote.id
        || local.agent_id != remote.agent_id
        || local.created_by != remote.created_by
        || local.created_at != remote.created_at
    {
        return Err(ConflictError::QuickSession(
            "immutable metadata differs".to_string(),
        ));
    }
    if local.status == QuickSessionStatus::Archived || remote.status == QuickSessionStatus::Archived
    {
        return Err(ConflictError::QuickSession(
            "archive transitions require manual resolution".to_string(),
        ));
    }

    let (title, title_source) = match (&local.title, &remote.title) {
        (Some(local), Some(remote)) if local != remote => {
            return Err(ConflictError::QuickSession(
                "concurrent titles differ".to_string(),
            ))
        }
        (Some(title), _) | (_, Some(title)) => {
            (Some(title.clone()), QuickSessionTitleSource::ApiSet)
        }
        (None, None) => (None, QuickSessionTitleSource::None),
    };
    let translate_local = |line: u64| {
        mappings
            .iter()
            .find(|mapping| mapping.file == thread_path && mapping.old_line == line)
            .map_or(line, |mapping| mapping.new_line)
    };
    let parsed = parse_thread(merged_thread)?;
    let newest_human_line = parsed
        .entries
        .iter()
        .filter_map(|entry| match entry {
            ThreadEntry::Message(message) if message.author.as_str() == local.created_by => {
                Some(message.line_number)
            }
            _ => None,
        })
        .max();
    let final_preview = parsed
        .entries
        .iter()
        .rev()
        .find_map(|entry| match entry {
            ThreadEntry::Message(message) => Some(truncate_quick_session_preview(&message.body)),
            ThreadEntry::Event(_) => None,
        })
        .unwrap_or_default();

    let local_human_line = local.last_human_line.map(translate_local);
    let remote_human_line = remote.last_human_line;
    let (last_human_request_id, last_human_updated_at) = match (local_human_line, remote_human_line)
    {
        (Some(local_line), Some(remote_line)) if local_line > remote_line => (
            local.last_human_request_id.clone(),
            Some(local.updated_at.as_str()),
        ),
        (Some(local_line), None) if Some(local_line) == newest_human_line => (
            local.last_human_request_id.clone(),
            Some(local.updated_at.as_str()),
        ),
        (_, Some(_)) => (
            remote.last_human_request_id.clone(),
            Some(remote.updated_at.as_str()),
        ),
        _ => (None, None),
    };

    let local_failure = match (local.last_failed_attempt_id.as_ref(), local.error.as_ref()) {
        (Some(attempt), Some(error)) => Some((
            attempt.clone(),
            error.clone(),
            local.updated_at.clone(),
            local_human_line,
        )),
        _ => None,
    };
    let remote_failure = match (
        remote.last_failed_attempt_id.as_ref(),
        remote.error.as_ref(),
    ) {
        (Some(attempt), Some(error)) => Some((
            attempt.clone(),
            error.clone(),
            remote.updated_at.clone(),
            remote_human_line,
        )),
        _ => None,
    };
    let failure = match (local_failure, remote_failure) {
        (Some(local), Some(remote)) => Some(if local.2 >= remote.2 { local } else { remote }),
        (Some(failure), None) | (None, Some(failure)) => Some(failure),
        (None, None) => None,
    }
    .filter(|failure| {
        last_human_updated_at.is_none_or(|human_updated_at| failure.2.as_str() >= human_updated_at)
    });

    let local_completion = match (
        local.last_completed_attempt_id.as_ref(),
        local.last_completed_input_line,
        local.last_completed_line,
    ) {
        (Some(attempt), Some(input), Some(output)) => Some((
            attempt.clone(),
            translate_local(input),
            translate_local(output),
        )),
        _ => None,
    };
    let remote_completion = match (
        remote.last_completed_attempt_id.as_ref(),
        remote.last_completed_input_line,
        remote.last_completed_line,
    ) {
        (Some(attempt), Some(input), Some(output)) => Some((attempt.clone(), input, output)),
        _ => None,
    };
    let completion = match (local_completion, remote_completion) {
        (Some(local), Some(remote)) if local.0 != remote.0 && local.1 == remote.1 => {
            return Err(ConflictError::QuickSession(
                "different attempts completed the same input".to_string(),
            ))
        }
        (Some(local), Some(remote)) => Some(if (local.1, local.2) >= (remote.1, remote.2) {
            local
        } else {
            remote
        }),
        (Some(completion), None) | (None, Some(completion)) => Some(completion),
        (None, None) => None,
    };
    if completion.as_ref().is_some_and(|completed| {
        failure
            .as_ref()
            .is_some_and(|failed| failed.0 == completed.0)
    }) {
        return Err(ConflictError::QuickSession(
            "the same attempt both completed and failed".to_string(),
        ));
    }

    let local_claim = if local.status == QuickSessionStatus::Running {
        Some((
            local.attempt_id.clone().ok_or_else(|| {
                ConflictError::QuickSession("local running claim is incomplete".to_string())
            })?,
            translate_local(local.processing_input_line.ok_or_else(|| {
                ConflictError::QuickSession("local running claim is incomplete".to_string())
            })?),
            local.processing_started_at.clone().ok_or_else(|| {
                ConflictError::QuickSession("local running claim is incomplete".to_string())
            })?,
        ))
    } else {
        None
    };
    let remote_claim = if remote.status == QuickSessionStatus::Running {
        Some((
            remote.attempt_id.clone().ok_or_else(|| {
                ConflictError::QuickSession("remote running claim is incomplete".to_string())
            })?,
            remote.processing_input_line.ok_or_else(|| {
                ConflictError::QuickSession("remote running claim is incomplete".to_string())
            })?,
            remote.processing_started_at.clone().ok_or_else(|| {
                ConflictError::QuickSession("remote running claim is incomplete".to_string())
            })?,
        ))
    } else {
        None
    };
    let claim_completed = |claim: &(String, u64, String)| {
        completion
            .as_ref()
            .is_some_and(|completed| completed.0 == claim.0 && completed.1 == claim.1)
            || failure.as_ref().is_some_and(|failed| failed.0 == claim.0)
    };
    let local_claim = local_claim.filter(|claim| !claim_completed(claim));
    let remote_claim = remote_claim.filter(|claim| !claim_completed(claim));
    let claim = match (local_claim, remote_claim) {
        (Some(local), Some(remote)) if local.0 != remote.0 || local.1 != remote.1 => {
            return Err(ConflictError::QuickSession(
                "concurrent running claims differ".to_string(),
            ))
        }
        (Some(local), Some(remote)) => Some(if local.2 >= remote.2 { local } else { remote }),
        (Some(claim), None) | (None, Some(claim)) => Some(claim),
        (None, None) => None,
    };

    let (summary, summary_updated_at) = match (
        local.summary_updated_at.as_deref(),
        remote.summary_updated_at.as_deref(),
    ) {
        (Some(local_time), Some(remote_time)) if local_time > remote_time => {
            (local.summary.clone(), local.summary_updated_at.clone())
        }
        (Some(_), None) => (local.summary.clone(), local.summary_updated_at.clone()),
        _ => (remote.summary.clone(), remote.summary_updated_at.clone()),
    };
    if claim
        .as_ref()
        .is_some_and(|claim| failure.as_ref().is_some_and(|failed| failed.0 != claim.0))
    {
        return Err(ConflictError::QuickSession(
            "a running claim conflicts with a failed attempt".to_string(),
        ));
    }
    let status = if claim.is_some() {
        QuickSessionStatus::Running
    } else if failure.as_ref().is_some_and(|failed| {
        newest_human_line.is_none_or(|newest| failed.3.is_none_or(|seen| newest <= seen))
    }) {
        QuickSessionStatus::Error
    } else if title.is_some() {
        QuickSessionStatus::Active
    } else {
        QuickSessionStatus::NeedsTitle
    };
    let (attempt_id, processing_input_line, processing_started_at) = match claim {
        Some((attempt, input, started)) => (Some(attempt), Some(input), Some(started)),
        None => (None, None, None),
    };
    let (last_completed_attempt_id, last_completed_input_line, last_completed_line) =
        match completion {
            Some((attempt, input, output)) => (Some(attempt), Some(input), Some(output)),
            None => (None, None, None),
        };
    let (error, last_failed_attempt_id) = match failure {
        Some((attempt, error, _, _)) => (Some(error), Some(attempt)),
        None => (None, None),
    };
    let merged = QuickSessionMeta {
        id: local.id.clone(),
        title,
        title_source,
        agent_id: local.agent_id.clone(),
        created_by: local.created_by.clone(),
        status,
        created_at: local.created_at.clone(),
        updated_at: local.updated_at.clone().max(remote.updated_at.clone()),
        archived_at: None,
        archived_from: None,
        summary,
        summary_updated_at,
        last_message_preview: final_preview,
        error,
        processing_input_line,
        processing_started_at,
        attempt_id,
        last_completed_attempt_id,
        last_completed_input_line,
        last_completed_line,
        last_failed_attempt_id,
        last_human_request_id,
        last_human_line: newest_human_line,
        revision: local
            .revision
            .max(remote.revision)
            .checked_add(1)
            .ok_or_else(|| ConflictError::QuickSession("revision overflow".to_string()))?,
    };
    validate_quick_session_meta(&merged)
        .map_err(|error| ConflictError::QuickSession(error.to_string()))?;
    Ok(merged)
}
