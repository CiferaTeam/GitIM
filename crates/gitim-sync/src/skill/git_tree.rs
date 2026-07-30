use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use gitim_core::skill::{RequestId, SkillTreeEdit};

use crate::git::{
    classify_remote_error, run_git, run_git_command, run_git_with_env, GitError, GitStorage,
    GIT_HTTP_TIMEOUT_ARGS,
};

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
    let output = run_git(&["cat-file", "blob", &entry.oid], repo.root())?;
    Ok(Some(output.stdout))
}

pub fn list_tree_recursive(
    repo: &GitStorage,
    commit: &str,
    path: &str,
) -> Result<Vec<GitTreeEntry>, GitError> {
    let commit_oid = resolve_commit(repo, commit)?;
    let output = if path.is_empty() {
        run_git(
            &["ls-tree", "-r", "-z", "--full-tree", &commit_oid],
            repo.root(),
        )?
    } else {
        validate_tree_path(path)?;
        let literal_pathspec = format!(":(literal){path}");
        run_git(
            &[
                "ls-tree",
                "-r",
                "-z",
                "--full-tree",
                &commit_oid,
                "--",
                &literal_pathspec,
            ],
            repo.root(),
        )?
    };
    parse_ls_tree(&output.stdout)
}

pub fn build_private_index_commit(
    repo: &GitStorage,
    request: &PrivateIndexCommitRequest,
) -> Result<BuiltPrivateCommit, GitError> {
    let base_commit = resolve_commit(repo, &request.base_commit)?;
    validate_identity("author name", &request.author_name)?;
    validate_identity("author email", &request.author_email)?;
    validate_message(&request.message)?;
    prepare_private_index(repo, &request.private_index)?;

    let index = request
        .private_index
        .to_str()
        .ok_or_else(|| invalid_input("private index path is not UTF-8"))?;
    let index_env = [("GIT_INDEX_FILE", index)];
    run_git_with_env(
        &["read-tree", "--reset", &base_commit],
        repo.root(),
        &index_env,
    )?;

    let temp_dir = request
        .private_index
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
                let output = run_git(&["hash-object", "-w", "--", blob_path], repo.root())?;
                let blob_oid = parse_oid(&output.stdout)?;
                let cache_info = format!("100644,{blob_oid},{path}");
                run_git_with_env(
                    &["update-index", "--add", "--cacheinfo", &cache_info],
                    repo.root(),
                    &index_env,
                )?;
            }
            SkillTreeEdit::Delete { path } => {
                validate_tree_path(path)?;
                run_git_with_env(
                    &["update-index", "--force-remove", "--", path],
                    repo.root(),
                    &index_env,
                )?;
            }
        }
    }

    let tree_output = run_git_with_env(&["write-tree"], repo.root(), &index_env)?;
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
    let commit_output = run_git_with_env(
        &[
            "commit-tree",
            &tree_oid,
            "-p",
            &base_commit,
            "-m",
            &commit_message,
        ],
        repo.root(),
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
    let output = run_git_command(&args, repo.root())?;
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
    let output = run_git(
        &["rev-parse", "--verify", "--end-of-options", object],
        repo.root(),
    )?;
    parse_oid(&output.stdout)
}

fn ls_tree_entry(
    repo: &GitStorage,
    commit_oid: &str,
    path: &str,
) -> Result<Option<GitTreeEntry>, GitError> {
    let literal_pathspec = format!(":(literal){path}");
    let output = run_git(
        &[
            "ls-tree",
            "-z",
            "--full-tree",
            commit_oid,
            "--",
            &literal_pathspec,
        ],
        repo.root(),
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

fn prepare_private_index(repo: &GitStorage, private_index: &Path) -> Result<(), GitError> {
    let parent = private_index
        .parent()
        .ok_or_else(|| invalid_input("private index path has no parent"))?;
    fs::create_dir_all(parent)?;
    if fs::symlink_metadata(private_index).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(invalid_input("private index must not be a symbolic link"));
    }

    let private_parent = parent.canonicalize()?;
    let private_name = private_index
        .file_name()
        .ok_or_else(|| invalid_input("private index path has no file name"))?;
    let normalized_private = private_parent.join(private_name);

    let active_output = run_git(&["rev-parse", "--git-path", "index"], repo.root())?;
    let active_text = std::str::from_utf8(&active_output.stdout)
        .map_err(|error| invalid_input(format!("active index path is not UTF-8: {error}")))?;
    let active_path = PathBuf::from(active_text.trim());
    let active_path = if active_path.is_absolute() {
        active_path
    } else {
        repo.root().join(active_path)
    };
    let normalized_active = active_path.canonicalize()?;
    if normalized_private == normalized_active {
        return Err(invalid_input(
            "private index path resolves to the active Git index",
        ));
    }
    Ok(())
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
    run_git(&["check-ref-format", "--branch", branch], repo.root()).map(|_| ())
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
    if message
        .lines()
        .any(|line| line.starts_with("Gitim-Request-Id:"))
    {
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
