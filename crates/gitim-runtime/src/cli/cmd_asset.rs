//! Agent-facing asset upload and download commands.

use std::path::{Path, PathBuf};

use futures::StreamExt;
use gitim_core::types::{
    AssetRef, ASSET_COMPONENT_ENCODE_SET, ASSET_REF_VERSION, MAX_ASSETS_PER_MESSAGE,
    MAX_ASSET_BYTES, MAX_ASSET_REQUEST_BYTES,
};
use percent_encoding::utf8_percent_encode;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use super::http::{CliError, Client, UploadFile};
use super::workspace::resolve_workspace;

#[derive(Debug)]
pub struct PutArgs {
    pub workspace: Option<String>,
    pub files: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct GetArgs {
    pub workspace: Option<String>,
    pub asset_ref: String,
    pub output: Option<PathBuf>,
    pub force: bool,
}

pub async fn put(client: &Client, args: PutArgs) -> Result<i32, CliError> {
    let files = prepare_uploads(args.files).await?;
    let slug = resolve_workspace(client, args.workspace.as_deref()).await?;
    let response = client
        .post_files(&format!("/workspaces/{slug}/assets"), files)
        .await?;
    let output = serde_json::to_string(&response)
        .map_err(|error| CliError::Parse(format!("serialize asset upload response: {error}")))?;
    println!("{output}");
    Ok(0)
}

pub async fn get(client: &Client, args: GetArgs) -> Result<i32, CliError> {
    let asset: AssetRef = args.asset_ref.parse().map_err(|error| {
        CliError::InvalidConfig(format!("invalid canonical asset reference: {error}"))
    })?;
    let destination = args.output.unwrap_or_else(|| {
        let name = if matches!(asset.name.as_str(), "." | "..") {
            "attachment"
        } else {
            &asset.name
        };
        PathBuf::from(name)
    });
    validate_destination(&destination, args.force)?;

    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent_metadata = std::fs::metadata(parent).map_err(|error| {
        CliError::InvalidConfig(format!(
            "asset output directory '{}': {error}",
            parent.display()
        ))
    })?;
    if !parent_metadata.is_dir() {
        return Err(CliError::InvalidConfig(format!(
            "asset output parent '{}' is not a directory",
            parent.display()
        )));
    }

    let slug = resolve_workspace(client, args.workspace.as_deref()).await?;
    let encoded_name = utf8_percent_encode(&asset.name, ASSET_COMPONENT_ENCODE_SET);
    let path = format!(
        "/workspaces/{slug}/assets/resolve/{}/{}?name={encoded_name}&download=1",
        asset.origin_runtime_id, asset.sha256
    );
    let response = client.get_binary(&path).await?;
    validate_content_length(&response, asset.size)?;

    let temporary = tempfile::NamedTempFile::new_in(parent).map_err(|error| {
        CliError::InvalidConfig(format!(
            "create temporary asset in '{}': {error}",
            parent.display()
        ))
    })?;
    let std_file = temporary.as_file().try_clone().map_err(|error| {
        CliError::InvalidConfig(format!("clone temporary asset handle: {error}"))
    })?;
    let mut output = tokio::fs::File::from_std(std_file);
    let mut stream = response.bytes_stream();
    let mut hasher = Sha256::new();
    let mut received = 0_u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| CliError::Transport(error.to_string()))?;
        received = received
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| CliError::InvalidConfig("asset response size overflow".to_string()))?;
        if received > MAX_ASSET_BYTES || received > asset.size {
            return Err(CliError::InvalidConfig(format!(
                "asset response exceeds expected size {}",
                asset.size
            )));
        }
        hasher.update(&chunk);
        output
            .write_all(&chunk)
            .await
            .map_err(|error| CliError::InvalidConfig(format!("write temporary asset: {error}")))?;
    }
    if received != asset.size {
        return Err(CliError::InvalidConfig(format!(
            "asset response size mismatch: expected {}, received {received}",
            asset.size
        )));
    }
    let actual_hash = format!("{:x}", hasher.finalize());
    if actual_hash != asset.sha256 {
        return Err(CliError::InvalidConfig(format!(
            "asset response SHA-256 mismatch: expected {}, received {actual_hash}",
            asset.sha256
        )));
    }

    output
        .flush()
        .await
        .map_err(|error| CliError::InvalidConfig(format!("flush temporary asset: {error}")))?;
    output
        .sync_all()
        .await
        .map_err(|error| CliError::InvalidConfig(format!("sync temporary asset: {error}")))?;
    drop(output);

    let success = serde_json::to_string(&json!({
        "ok": true,
        "path": destination.to_string_lossy(),
        "sha256": asset.sha256,
        "size": received,
    }))
    .map_err(|error| CliError::Parse(format!("serialize asset download response: {error}")))?;

    if args.force {
        temporary.persist(&destination).map_err(|error| {
            CliError::InvalidConfig(format!(
                "persist asset to '{}': {}",
                destination.display(),
                error.error
            ))
        })?;
    } else {
        temporary.persist_noclobber(&destination).map_err(|error| {
            CliError::InvalidConfig(format!(
                "asset output '{}' already exists or cannot be created: {}",
                destination.display(),
                error.error
            ))
        })?;
    }
    println!("{success}");
    Ok(0)
}

async fn prepare_uploads(paths: Vec<PathBuf>) -> Result<Vec<UploadFile>, CliError> {
    if paths.is_empty() {
        return Err(CliError::InvalidConfig(
            "asset put requires at least one --file".to_string(),
        ));
    }
    if paths.len() > MAX_ASSETS_PER_MESSAGE {
        return Err(CliError::InvalidConfig(format!(
            "asset put accepts at most {MAX_ASSETS_PER_MESSAGE} files"
        )));
    }

    let mut total = 0_u64;
    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let file = open_upload_file(&path).await.map_err(|error| {
            CliError::InvalidConfig(format!("open asset file '{}': {error}", path.display()))
        })?;
        let metadata = file.metadata().await.map_err(|error| {
            CliError::InvalidConfig(format!("stat asset file '{}': {error}", path.display()))
        })?;
        if !metadata.is_file() {
            return Err(CliError::InvalidConfig(format!(
                "asset path '{}' is not a regular file",
                path.display()
            )));
        }
        let length = metadata.len();
        if length > MAX_ASSET_BYTES {
            return Err(CliError::InvalidConfig(format!(
                "asset file '{}' exceeds the {MAX_ASSET_BYTES}-byte limit",
                path.display()
            )));
        }
        total = total
            .checked_add(length)
            .ok_or_else(|| CliError::InvalidConfig("asset request size overflow".to_string()))?;
        if total > MAX_ASSET_REQUEST_BYTES {
            return Err(CliError::InvalidConfig(format!(
                "asset upload exceeds the {MAX_ASSET_REQUEST_BYTES}-byte request limit"
            )));
        }
        let file_name = sanitized_file_name(&path);
        validate_upload_name(&file_name, length)?;
        files.push(UploadFile {
            file,
            file_name,
            length,
        });
    }
    Ok(files)
}

fn validate_content_length(response: &reqwest::Response, expected: u64) -> Result<(), CliError> {
    let mut values = response
        .headers()
        .get_all(reqwest::header::CONTENT_LENGTH)
        .iter();
    let value = values.next().ok_or_else(|| {
        CliError::InvalidConfig("asset response is missing Content-Length".to_string())
    })?;
    if values.next().is_some() {
        return Err(CliError::InvalidConfig(
            "asset response has multiple Content-Length headers".to_string(),
        ));
    }
    let raw = value.to_str().map_err(|_| {
        CliError::InvalidConfig("asset response has an invalid Content-Length".to_string())
    })?;
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(CliError::InvalidConfig(
            "asset response has an invalid Content-Length".to_string(),
        ));
    }
    let length = raw.parse::<u64>().map_err(|_| {
        CliError::InvalidConfig("asset response has an invalid Content-Length".to_string())
    })?;
    if length > MAX_ASSET_BYTES || length != expected {
        return Err(CliError::InvalidConfig(format!(
            "asset response size mismatch: expected {expected}, received {length}"
        )));
    }
    Ok(())
}

#[cfg(unix)]
async fn open_upload_file(path: &Path) -> std::io::Result<tokio::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    let path = path.to_path_buf();
    let file = tokio::task::spawn_blocking(move || {
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
    })
    .await
    .map_err(|error| std::io::Error::other(format!("asset open task failed: {error}")))??;
    Ok(tokio::fs::File::from_std(file))
}

#[cfg(not(unix))]
async fn open_upload_file(path: &Path) -> std::io::Result<tokio::fs::File> {
    if !tokio::fs::metadata(path).await?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "asset path is not a regular file",
        ));
    }
    tokio::fs::File::open(path).await
}

fn sanitized_file_name(path: &Path) -> String {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>();
    if name.is_empty() {
        "attachment".to_string()
    } else {
        name
    }
}

fn validate_upload_name(name: &str, size: u64) -> Result<(), CliError> {
    let candidate = AssetRef {
        version: ASSET_REF_VERSION,
        origin_runtime_id: "00000000-0000-4000-8000-000000000000".to_string(),
        sha256: "0".repeat(64),
        name: name.to_string(),
        media_type: "application/octet-stream".to_string(),
        size,
        width: None,
        height: None,
    };
    candidate.validate().map_err(|error| {
        CliError::InvalidConfig(format!("invalid asset filename '{name}': {error}"))
    })
}

fn validate_destination(destination: &Path, force: bool) -> Result<(), CliError> {
    if destination.file_name().is_none() {
        return Err(CliError::InvalidConfig(
            "asset output must name a file".to_string(),
        ));
    }
    if !force && destination.exists() {
        return Err(CliError::InvalidConfig(format!(
            "asset output '{}' already exists; pass --force to replace it",
            destination.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gitim_core::types::MAX_ASSET_FILENAME_BYTES;

    #[tokio::test]
    async fn per_file_size_boundary_accepts_exact_and_rejects_plus_one() {
        let directory = tempfile::tempdir().unwrap();
        let exact = directory.path().join("exact.bin");
        std::fs::File::create(&exact)
            .unwrap()
            .set_len(MAX_ASSET_BYTES)
            .unwrap();
        let prepared = prepare_uploads(vec![exact]).await.unwrap();
        assert_eq!(prepared[0].length, MAX_ASSET_BYTES);

        let oversized = directory.path().join("oversized.bin");
        std::fs::File::create(&oversized)
            .unwrap()
            .set_len(MAX_ASSET_BYTES + 1)
            .unwrap();
        let error = match prepare_uploads(vec![oversized]).await {
            Ok(_) => panic!("oversized file must fail"),
            Err(error) => error,
        };
        assert!(matches!(error, CliError::InvalidConfig(_)));
    }

    #[test]
    fn filename_boundary_accepts_exact_and_rejects_plus_one() {
        let exact = "a".repeat(MAX_ASSET_FILENAME_BYTES);
        validate_upload_name(&exact, 0).unwrap();
        let oversized = "a".repeat(MAX_ASSET_FILENAME_BYTES + 1);
        assert!(validate_upload_name(&oversized, 0).is_err());

        let encoded_max = "界".repeat(MAX_ASSET_FILENAME_BYTES / "界".len());
        validate_upload_name(&encoded_max, 0).unwrap();
    }
}
