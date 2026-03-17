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
    let mut total_messages: usize = 0;

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

        total_messages += renumbered_file.messages.len();

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

    // Step 4: Commit all changes
    let path_refs: Vec<&str> = modified_paths.iter().map(|s| s.as_str()).collect();
    let commit_msg = format!("msg: sync {} messages after rebase", total_messages);
    repo.add_and_commit(&path_refs, &commit_msg)?;

    Ok(all_mappings)
}
