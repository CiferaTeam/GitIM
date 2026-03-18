use std::collections::HashMap;
use std::path::PathBuf;

use gitim_core::parser::{parse_thread, ParseError};
use thiserror::Error;

use crate::git::GitError;
use crate::renumber::{renumber_batch, RenumberError};

#[derive(Error, Debug)]
pub enum ConflictError {
    #[error("git error: {0}")]
    Git(#[from] GitError),
    #[error("renumber error: {0}")]
    Renumber(#[from] RenumberError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse error: {0}")]
    Parse(#[from] ParseError),
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenumberMapping {
    pub file: PathBuf,
    pub old_line: u64,
    pub new_line: u64,
}

/// Build commit message for rebased messages.
/// Format: `msg: @author -> channel L000011 L000012 L000013(rebased)`
/// Groups by (author, channel) for multi-file/multi-author scenarios.
fn build_rebase_commit_msg(
    mappings: &[RenumberMapping],
    local_additions: &HashMap<PathBuf, String>,
) -> String {
    // Parse each file's local additions to extract author info per new line
    let mut entries: Vec<(String, String, Vec<u64>)> = Vec::new(); // (author, channel, new_lines)

    // Group mappings by file
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

        // Parse local content to get authors
        if let Some(content) = local_additions.get(*file) {
            if let Ok(parsed) = parse_thread(content) {
                // Group by author within this file
                let mut by_author: HashMap<String, Vec<u64>> = HashMap::new();
                for (msg, new_ln) in parsed.messages.iter().zip(new_lines.iter()) {
                    by_author
                        .entry(msg.author.as_str().to_string())
                        .or_default()
                        .push(*new_ln);
                }
                let mut authors: Vec<_> = by_author.into_iter().collect();
                authors.sort_by(|a, b| a.0.cmp(&b.0));
                for (author, lines) in authors {
                    entries.push((author, channel.clone(), lines));
                }
            } else {
                // Fallback if parse fails
                entries.push(("unknown".to_string(), channel.clone(), new_lines.clone()));
            }
        }
    }

    if entries.is_empty() {
        return "msg: sync after rebase".to_string();
    }

    // Build message lines: msg: @author -> channel L000011 L000012(rebased)
    let parts: Vec<String> = entries
        .iter()
        .map(|(author, channel, lines)| {
            let line_parts: Vec<String> = lines.iter().map(|l| format!("L{:06}", l)).collect();
            format!("msg: @{} -> {} {}(rebased)", author, channel, line_parts.join(" "))
        })
        .collect();

    parts.join("\n")
}

pub fn resolve_thread_conflicts(
    repo: &crate::git::GitRepo,
    local_additions: &HashMap<PathBuf, String>,
) -> Result<Vec<RenumberMapping>, ConflictError> {
    // Step 1: Abort any in-progress rebase
    repo.rebase_abort()?;

    // Step 2: Reset to remote state
    repo.reset_hard_origin()?;

    let mut all_mappings: Vec<RenumberMapping> = Vec::new();
    let mut modified_paths: Vec<String> = Vec::new();

    // Step 3: For each file with local additions, renumber and append
    let mut sorted_files: Vec<_> = local_additions.keys().collect();
    sorted_files.sort();
    for rel_path in sorted_files {
        let local_content = &local_additions[rel_path];
        let abs_path = repo.root().join(rel_path);

        // Read the current (remote) file content
        let remote_content = if abs_path.exists() {
            std::fs::read_to_string(&abs_path)?
        } else {
            // File might not exist on remote yet (new channel/dm)
            if let Some(parent) = abs_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            String::new()
        };

        // Parse remote content to find max line number
        let max_line = if remote_content.is_empty() {
            0
        } else {
            let remote_file = parse_thread(&remote_content)?;
            remote_file
                .messages
                .iter()
                .map(|m| m.line_number)
                .max()
                .unwrap_or(0)
        };

        // Parse the local content to capture old line numbers
        let local_file = parse_thread(local_content)?;
        let old_line_numbers: Vec<u64> = local_file
            .messages
            .iter()
            .map(|m| m.line_number)
            .collect();

        // Renumber local additions starting after the max remote line
        let renumbered = renumber_batch(local_content, max_line)?;

        // Parse renumbered content to get new line numbers
        let renumbered_file = parse_thread(&renumbered)?;
        let new_line_numbers: Vec<u64> = renumbered_file
            .messages
            .iter()
            .map(|m| m.line_number)
            .collect();

        // Build mappings
        for (old_ln, new_ln) in old_line_numbers.iter().zip(new_line_numbers.iter()) {
            all_mappings.push(RenumberMapping {
                file: rel_path.clone(),
                old_line: *old_ln,
                new_line: *new_ln,
            });
        }

        // Append renumbered content to the file
        let mut final_content = remote_content;
        if !final_content.is_empty() && !final_content.ends_with('\n') {
            final_content.push('\n');
        }
        final_content.push_str(&renumbered);
        std::fs::write(&abs_path, &final_content)?;

        modified_paths.push(rel_path.to_str().unwrap_or("").to_string());
    }

    if modified_paths.is_empty() {
        return Ok(all_mappings);
    }

    // Step 4: Commit all changes with detailed message
    let path_refs: Vec<&str> = modified_paths.iter().map(|s| s.as_str()).collect();
    let commit_msg = build_rebase_commit_msg(&all_mappings, local_additions);
    repo.add_and_commit(&path_refs, &commit_msg)?;

    Ok(all_mappings)
}
