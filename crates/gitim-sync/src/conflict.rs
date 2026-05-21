use std::collections::HashMap;
use std::path::PathBuf;

use gitim_core::parser::{parse_thread, ParseError};
use gitim_core::types::ChannelMeta;
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
        } else {
            if let Some(parent) = abs_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
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
