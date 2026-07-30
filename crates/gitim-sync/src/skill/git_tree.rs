use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use gitim_core::skill::{RequestId, SkillTreeEdit};

use crate::git::{
    classify_remote_error, run_git_command_with_env_and_timeout, run_git_with_env_and_timeout,
    GitError, GitStorage, GIT_HTTP_TIMEOUT_ARGS,
};

const SKILL_GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitTreeEntry {
    pub mode: String,
    pub object_type: String,
    pub oid: String,
    pub path: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrivateIndexCommitRequest {
    pub base_commit: String,
    pub private_index: PathBuf,
    pub edits: Vec<SkillTreeEdit>,
    pub message: String,
    pub author_name: String,
    pub author_email: String,
    pub request_id: RequestId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuiltPrivateCommit {
    pub commit_oid: String,
    pub tree_oid: String,
}

pub fn tree_oid_at(
    repo: &GitStorage,
    commit: &str,
    path: &str,
) -> Result<Option<String>, GitError> {
    let commit_oid = resolve_commit(repo, commit)?;
    if path.is_empty() {
        return resolve_object(repo, &format!("{commit_oid}^{{tree}}")).map(Some);
    }
    validate_tree_path(path)?;
    Ok(ls_tree_entry(repo, &commit_oid, path)?.map(|entry| entry.oid))
}

pub fn read_blob_at(
    repo: &GitStorage,
    commit: &str,
    path: &str,
) -> Result<Option<Vec<u8>>, GitError> {
    let commit_oid = resolve_commit(repo, commit)?;
    validate_tree_path(path)?;
    let Some(entry) = ls_tree_entry(repo, &commit_oid, path)? else {
        return Ok(None);
    };
    if entry.object_type != "blob" {
        return Err(invalid_input(format!(
            "path {path:?} is a {}, not a blob",
            entry.object_type
        )));
    }
    let output = run_skill_git(repo, &["cat-file", "blob", &entry.oid])?;
    Ok(Some(output.stdout))
}

pub fn list_tree_recursive(
    repo: &GitStorage,
    commit: &str,
    path: &str,
) -> Result<Vec<GitTreeEntry>, GitError> {
    let commit_oid = resolve_commit(repo, commit)?;
    let output = if path.is_empty() {
        run_skill_git(repo, &["ls-tree", "-r", "-z", "--full-tree", &commit_oid])?
    } else {
        validate_tree_path(path)?;
        let literal_pathspec = format!(":(literal){path}");
        run_skill_git(
            repo,
            &[
                "ls-tree",
                "-r",
                "-z",
                "--full-tree",
                &commit_oid,
                "--",
                &literal_pathspec,
            ],
        )?
    };
    parse_ls_tree(&output.stdout)
}

pub fn build_private_index_commit(
    repo: &GitStorage,
    request: &PrivateIndexCommitRequest,
) -> Result<BuiltPrivateCommit, GitError> {
    let private_index = normalize_private_index(repo, &request.private_index)?;
    let base_commit = resolve_commit(repo, &request.base_commit)?;
    validate_identity("author name", &request.author_name)?;
    validate_identity("author email", &request.author_email)?;
    validate_message(&request.message)?;

    let index = private_index
        .to_str()
        .ok_or_else(|| invalid_input("private index path is not UTF-8"))?;
    let index_env = [("GIT_INDEX_FILE", index)];
    run_skill_git_with_env(repo, &["read-tree", "--reset", &base_commit], &index_env)?;

    let temp_dir = private_index
        .parent()
        .ok_or_else(|| invalid_input("private index path has no parent"))?;
    for edit in &request.edits {
        match edit {
            SkillTreeEdit::Upsert { path, bytes } => {
                validate_tree_path(path)?;
                let mut blob_file = tempfile::NamedTempFile::new_in(temp_dir)?;
                blob_file.write_all(bytes)?;
                blob_file.flush()?;
                let blob_path = blob_file
                    .path()
                    .to_str()
                    .ok_or_else(|| invalid_input("temporary blob path is not UTF-8"))?;
                let output = run_skill_git(repo, &["hash-object", "-w", "--", blob_path])?;
                let blob_oid = parse_oid(&output.stdout)?;
                let cache_info = format!("100644,{blob_oid},{path}");
                run_skill_git_with_env(
                    repo,
                    &["update-index", "--add", "--cacheinfo", &cache_info],
                    &index_env,
                )?;
            }
            SkillTreeEdit::Delete { path } => {
                validate_tree_path(path)?;
                run_skill_git_with_env(
                    repo,
                    &["update-index", "--force-remove", "--", path],
                    &index_env,
                )?;
            }
        }
    }

    let tree_output = run_skill_git_with_env(repo, &["write-tree"], &index_env)?;
    let tree_oid = parse_oid(&tree_output.stdout)?;
    let commit_message = format!(
        "{}\n\nGitim-Request-Id: {}\n",
        request.message.trim_end_matches('\n'),
        request.request_id.as_str()
    );
    let identity_env = [
        ("GIT_AUTHOR_NAME", request.author_name.as_str()),
        ("GIT_AUTHOR_EMAIL", request.author_email.as_str()),
        ("GIT_COMMITTER_NAME", request.author_name.as_str()),
        ("GIT_COMMITTER_EMAIL", request.author_email.as_str()),
    ];
    let commit_output = run_skill_git_with_env(
        repo,
        &[
            "commit-tree",
            &tree_oid,
            "-p",
            &base_commit,
            "-m",
            &commit_message,
        ],
        &identity_env,
    )?;
    let commit_oid = parse_oid(&commit_output.stdout)?;

    Ok(BuiltPrivateCommit {
        commit_oid,
        tree_oid,
    })
}

pub fn push_commit_fast_forward(
    repo: &GitStorage,
    commit: &str,
    remote_branch: &str,
) -> Result<(), GitError> {
    let commit_oid = resolve_commit(repo, commit)?;
    validate_remote_branch(repo, remote_branch)?;
    let refspec = format!("{commit_oid}:refs/heads/{remote_branch}");
    let args = [
        GIT_HTTP_TIMEOUT_ARGS[0],
        GIT_HTTP_TIMEOUT_ARGS[1],
        GIT_HTTP_TIMEOUT_ARGS[2],
        GIT_HTTP_TIMEOUT_ARGS[3],
        "push",
        "origin",
        &refspec,
    ];
    let output = run_skill_git_command(repo, &args)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(classify_remote_error(&String::from_utf8_lossy(
            &output.stderr,
        )))
    }
}

fn resolve_commit(repo: &GitStorage, commit: &str) -> Result<String, GitError> {
    validate_revision_argument(commit)?;
    resolve_object(repo, &format!("{commit}^{{commit}}"))
}

fn resolve_object(repo: &GitStorage, object: &str) -> Result<String, GitError> {
    let output = run_skill_git(repo, &["rev-parse", "--verify", "--end-of-options", object])?;
    parse_oid(&output.stdout)
}

fn ls_tree_entry(
    repo: &GitStorage,
    commit_oid: &str,
    path: &str,
) -> Result<Option<GitTreeEntry>, GitError> {
    let literal_pathspec = format!(":(literal){path}");
    let output = run_skill_git(
        repo,
        &[
            "ls-tree",
            "-z",
            "--full-tree",
            commit_oid,
            "--",
            &literal_pathspec,
        ],
    )?;
    let mut entries = parse_ls_tree(&output.stdout)?;
    match entries.len() {
        0 => Ok(None),
        1 => Ok(entries.pop()),
        _ => Err(invalid_input(format!(
            "tree lookup for {path:?} returned multiple entries"
        ))),
    }
}

fn parse_ls_tree(bytes: &[u8]) -> Result<Vec<GitTreeEntry>, GitError> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(|record| {
            let tab = record
                .iter()
                .position(|byte| *byte == b'\t')
                .ok_or_else(|| invalid_input("malformed git ls-tree record"))?;
            let metadata = std::str::from_utf8(&record[..tab])
                .map_err(|error| invalid_input(format!("non-UTF-8 ls-tree metadata: {error}")))?;
            let path = std::str::from_utf8(&record[tab + 1..])
                .map_err(|error| invalid_input(format!("non-UTF-8 tree path: {error}")))?;
            let mut fields = metadata.split_ascii_whitespace();
            let mode = fields
                .next()
                .ok_or_else(|| invalid_input("ls-tree record is missing mode"))?;
            let object_type = fields
                .next()
                .ok_or_else(|| invalid_input("ls-tree record is missing object type"))?;
            let oid = fields
                .next()
                .ok_or_else(|| invalid_input("ls-tree record is missing oid"))?;
            if fields.next().is_some() {
                return Err(invalid_input("ls-tree record has extra metadata fields"));
            }
            validate_oid(oid)?;
            Ok(GitTreeEntry {
                mode: mode.to_owned(),
                object_type: object_type.to_owned(),
                oid: oid.to_owned(),
                path: path.to_owned(),
            })
        })
        .collect()
}

fn normalize_private_index(repo: &GitStorage, private_index: &Path) -> Result<PathBuf, GitError> {
    if !private_index.is_absolute() {
        return Err(invalid_input("private index path must be absolute"));
    }
    let parent = private_index
        .parent()
        .ok_or_else(|| invalid_input("private index path has no parent"))?;
    let parent_metadata = fs::metadata(parent)?;
    if !parent_metadata.is_dir() {
        return Err(invalid_input(
            "private index parent must be an existing directory",
        ));
    }
    let normalized_parent = parent.canonicalize()?;
    let private_name = private_index
        .file_name()
        .ok_or_else(|| invalid_input("private index path has no file name"))?;
    let normalized_private = normalized_parent.join(private_name);
    if normalized_private.to_str().is_none() {
        return Err(invalid_input("private index path is not UTF-8"));
    }

    let private_metadata = symlink_metadata_if_exists(&normalized_private)?;
    if private_metadata
        .as_ref()
        .is_some_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(invalid_input("private index must not be a symbolic link"));
    }
    if private_metadata
        .as_ref()
        .is_some_and(|metadata| !metadata.is_file())
    {
        return Err(invalid_input(
            "existing private index must be a regular file",
        ));
    }

    let normalized_active = active_index_path(repo)?;
    if normalized_private == normalized_active {
        return Err(invalid_input(
            "private index path resolves to the active Git index",
        ));
    }
    if let Some(private_metadata) = private_metadata {
        let active_metadata = metadata_if_exists(&normalized_active)?;
        if active_metadata
            .as_ref()
            .is_some_and(|active| same_file(&private_metadata, active))
        {
            return Err(invalid_input("private index aliases the active Git index"));
        }
    }
    Ok(normalized_private)
}

fn active_index_path(repo: &GitStorage) -> Result<PathBuf, GitError> {
    let output = run_skill_git(
        repo,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--absolute-git-dir",
            "--git-common-dir",
        ],
    )?;
    let paths = std::str::from_utf8(&output.stdout)
        .map_err(|error| invalid_input(format!("Git directory path is not UTF-8: {error}")))?;
    let mut paths = paths.lines();
    let git_dir = paths
        .next()
        .ok_or_else(|| invalid_input("Git did not return an active Git directory"))?;
    let common_dir = paths
        .next()
        .ok_or_else(|| invalid_input("Git did not return a common Git directory"))?;
    if paths.next().is_some() {
        return Err(invalid_input("Git returned unexpected directory paths"));
    }

    let normalized_git_dir = Path::new(git_dir).canonicalize()?;
    let normalized_common_dir = Path::new(common_dir).canonicalize()?;
    let active_admin_dir = if normalized_git_dir == normalized_common_dir {
        normalized_common_dir
    } else {
        normalized_git_dir
    };
    let active_index = active_admin_dir.join("index");
    if symlink_metadata_if_exists(&active_index)?.is_some() {
        active_index.canonicalize().map_err(GitError::from)
    } else {
        Ok(active_index)
    }
}

fn symlink_metadata_if_exists(path: &Path) -> Result<Option<fs::Metadata>, GitError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn metadata_if_exists(path: &Path) -> Result<Option<fs::Metadata>, GitError> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_file(_left: &fs::Metadata, _right: &fs::Metadata) -> bool {
    false
}

fn validate_revision_argument(value: &str) -> Result<(), GitError> {
    if value.is_empty()
        || value.starts_with('-')
        || value.contains('\0')
        || value.contains('\n')
        || value.contains('\r')
    {
        return Err(invalid_input("invalid commit or revision argument"));
    }
    Ok(())
}

fn validate_remote_branch(repo: &GitStorage, branch: &str) -> Result<(), GitError> {
    if branch.is_empty()
        || branch.starts_with('-')
        || branch.starts_with("refs/")
        || branch.contains('\0')
        || branch.contains('\n')
        || branch.contains('\r')
    {
        return Err(invalid_input("invalid remote branch"));
    }
    run_skill_git(repo, &["check-ref-format", "--branch", branch]).map(|_| ())
}

fn validate_tree_path(path: &str) -> Result<(), GitError> {
    let candidate = Path::new(path);
    if path.is_empty()
        || path.contains('\0')
        || candidate.is_absolute()
        || candidate
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(invalid_input(format!(
            "invalid repository-relative path {path:?}"
        )));
    }
    Ok(())
}

fn validate_identity(label: &str, value: &str) -> Result<(), GitError> {
    if value.is_empty() || value.contains(['\0', '\n', '\r']) {
        return Err(invalid_input(format!("invalid {label}")));
    }
    Ok(())
}

fn validate_message(message: &str) -> Result<(), GitError> {
    if message.is_empty() || message.contains('\0') {
        return Err(invalid_input("invalid commit message"));
    }
    if message.lines().any(|line| {
        line.trim()
            .split_once(':')
            .is_some_and(|(token, _)| token.trim().eq_ignore_ascii_case("Gitim-Request-Id"))
    }) {
        return Err(invalid_input(
            "commit message already contains a Gitim-Request-Id trailer",
        ));
    }
    Ok(())
}

fn parse_oid(bytes: &[u8]) -> Result<String, GitError> {
    let oid = std::str::from_utf8(bytes)
        .map_err(|error| invalid_input(format!("Git returned a non-UTF-8 object ID: {error}")))?
        .trim();
    validate_oid(oid)?;
    Ok(oid.to_owned())
}

fn validate_oid(oid: &str) -> Result<(), GitError> {
    if !matches!(oid.len(), 40 | 64) || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid_input(format!("invalid Git object ID {oid:?}")));
    }
    Ok(())
}

fn invalid_input(message: impl Into<String>) -> GitError {
    GitError::CommandFailed(message.into())
}

fn run_skill_git(repo: &GitStorage, args: &[&str]) -> Result<Output, GitError> {
    run_skill_git_with_env(repo, args, &[])
}

fn run_skill_git_with_env(
    repo: &GitStorage,
    args: &[&str],
    envs: &[(&str, &str)],
) -> Result<Output, GitError> {
    run_git_with_env_and_timeout(args, repo.root(), envs, SKILL_GIT_COMMAND_TIMEOUT)
}

fn run_skill_git_command(repo: &GitStorage, args: &[&str]) -> Result<Output, GitError> {
    run_git_command_with_env_and_timeout(args, repo.root(), &[], SKILL_GIT_COMMAND_TIMEOUT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_git_command_timeout_is_60_seconds() {
        assert_eq!(
            SKILL_GIT_COMMAND_TIMEOUT,
            std::time::Duration::from_secs(60)
        );
    }
}
