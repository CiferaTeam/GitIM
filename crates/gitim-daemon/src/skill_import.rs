use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::{Component, Path};

use gitim_core::skill::{
    validate_package_entries, PackageEntry, SkillError, SkillSlug, ValidatedPackage,
    MAX_PACKAGE_BYTES, MAX_PACKAGE_FILES, MAX_PACKAGE_FILE_BYTES,
};

pub fn snapshot_skill_directory(
    source: &Path,
    request_dir: &Path,
) -> Result<ValidatedPackage, SkillError> {
    let source_metadata = fs::symlink_metadata(source).map_err(|_| SkillError::InvalidPackage)?;
    if source_metadata.file_type().is_symlink() || !source_metadata.is_dir() {
        return Err(SkillError::InvalidPackage);
    }
    let source = source
        .canonicalize()
        .map_err(|_| SkillError::InvalidPackage)?;
    let request_parent = request_dir.parent().ok_or(SkillError::InvalidPackage)?;
    fs::create_dir_all(request_parent).map_err(|_| SkillError::InvalidPackage)?;
    let request_parent = request_parent
        .canonicalize()
        .map_err(|_| SkillError::InvalidPackage)?;
    if request_dir.exists() {
        return Err(SkillError::InvalidPackage);
    }

    let staging = tempfile::Builder::new()
        .prefix(".skill-import-")
        .tempdir_in(&request_parent)
        .map_err(|_| SkillError::InvalidPackage)?;
    let mut entries = Vec::new();
    let mut total_bytes = 0_usize;
    collect_directory(
        &source,
        &source,
        staging.path(),
        &mut entries,
        &mut total_bytes,
    )?;
    let slug = package_slug(&entries)?;
    let package = validate_package_entries(&slug, entries)?;
    let staging_path = staging.keep();
    if fs::rename(&staging_path, request_dir).is_err() {
        let _ = fs::remove_dir_all(&staging_path);
        return Err(SkillError::InvalidPackage);
    }
    Ok(package)
}

fn collect_directory(
    source_root: &Path,
    directory: &Path,
    destination_root: &Path,
    entries: &mut Vec<PackageEntry>,
    total_bytes: &mut usize,
) -> Result<(), SkillError> {
    let before = fs::symlink_metadata(directory).map_err(|_| SkillError::InvalidPackage)?;
    if before.file_type().is_symlink() || !before.is_dir() {
        return Err(SkillError::InvalidPackage);
    }
    let canonical = directory
        .canonicalize()
        .map_err(|_| SkillError::InvalidPackage)?;
    if !canonical.starts_with(source_root) {
        return Err(SkillError::InvalidPackage);
    }

    let mut children = fs::read_dir(directory)
        .map_err(|_| SkillError::InvalidPackage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| SkillError::InvalidPackage)?;
    children.sort_by_key(std::fs::DirEntry::file_name);
    for child in children {
        let path = child.path();
        let metadata = fs::symlink_metadata(&path).map_err(|_| SkillError::InvalidPackage)?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            return Err(SkillError::InvalidPackage);
        }
        let relative = path
            .strip_prefix(source_root)
            .map_err(|_| SkillError::InvalidPackage)?;
        validate_relative_path(relative)?;
        if file_type.is_dir() {
            collect_directory(source_root, &path, destination_root, entries, total_bytes)?;
        } else if file_type.is_file() {
            if entries.len() >= MAX_PACKAGE_FILES {
                return Err(SkillError::PackageTooLarge);
            }
            let length =
                usize::try_from(metadata.len()).map_err(|_| SkillError::PackageTooLarge)?;
            if length > MAX_PACKAGE_FILE_BYTES {
                return Err(SkillError::PackageTooLarge);
            }
            *total_bytes = total_bytes
                .checked_add(length)
                .ok_or(SkillError::PackageTooLarge)?;
            if *total_bytes > MAX_PACKAGE_BYTES {
                return Err(SkillError::PackageTooLarge);
            }
            let bytes = read_regular_file(&path, &metadata)?;
            let portable = relative
                .components()
                .map(|component| component.as_os_str().to_str())
                .collect::<Option<Vec<_>>>()
                .ok_or(SkillError::InvalidPackage)?
                .join("/");
            let destination = destination_root.join(relative);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(|_| SkillError::InvalidPackage)?;
            }
            write_new_file(&destination, &bytes)?;
            entries.push(PackageEntry::new(portable, bytes));
        } else {
            return Err(SkillError::InvalidPackage);
        }
    }

    let after = fs::symlink_metadata(directory).map_err(|_| SkillError::InvalidPackage)?;
    if after.file_type().is_symlink() || !same_file_identity(&before, &after) {
        return Err(SkillError::InvalidPackage);
    }
    Ok(())
}

fn read_regular_file(path: &Path, before: &fs::Metadata) -> Result<Vec<u8>, SkillError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path).map_err(|_| SkillError::InvalidPackage)?;
    let opened = file.metadata().map_err(|_| SkillError::InvalidPackage)?;
    if !opened.is_file() || !same_file_identity(before, &opened) {
        return Err(SkillError::InvalidPackage);
    }
    let capacity = usize::try_from(opened.len()).map_err(|_| SkillError::PackageTooLarge)?;
    let mut bytes = Vec::with_capacity(capacity);
    file.read_to_end(&mut bytes)
        .map_err(|_| SkillError::InvalidPackage)?;
    let after = file.metadata().map_err(|_| SkillError::InvalidPackage)?;
    let path_after = fs::symlink_metadata(path).map_err(|_| SkillError::InvalidPackage)?;
    if bytes.len() != capacity
        || !same_file_identity(&opened, &after)
        || !same_file_identity(&opened, &path_after)
        || path_after.file_type().is_symlink()
    {
        return Err(SkillError::InvalidPackage);
    }
    Ok(bytes)
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), SkillError> {
    use std::io::Write;

    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|_| SkillError::InvalidPackage)?;
    file.write_all(bytes)
        .map_err(|_| SkillError::InvalidPackage)?;
    file.sync_all().map_err(|_| SkillError::InvalidPackage)
}

fn validate_relative_path(path: &Path) -> Result<(), SkillError> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(SkillError::InvalidPackage);
    }
    Ok(())
}

fn package_slug(entries: &[PackageEntry]) -> Result<SkillSlug, SkillError> {
    let markdown = entries
        .iter()
        .find(|entry| entry.path == "SKILL.md")
        .ok_or(SkillError::InvalidPackage)?;
    let text = std::str::from_utf8(&markdown.bytes).map_err(|_| SkillError::InvalidPackage)?;
    let body = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
        .ok_or(SkillError::InvalidPackage)?;
    let mut frontmatter = String::new();
    let mut closed = false;
    for line in body.lines() {
        if line == "---" {
            closed = true;
            break;
        }
        frontmatter.push_str(line);
        frontmatter.push('\n');
    }
    if !closed {
        return Err(SkillError::InvalidPackage);
    }
    let value: serde_yaml::Value =
        serde_yaml::from_str(&frontmatter).map_err(|_| SkillError::InvalidPackage)?;
    let name = value
        .get("name")
        .and_then(serde_yaml::Value::as_str)
        .ok_or(SkillError::InvalidPackage)?;
    SkillSlug::new(name)
}

#[cfg(unix)]
fn same_file_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
}

#[cfg(not(unix))]
fn same_file_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.len() == right.len()
        && left.modified().ok() == right.modified().ok()
        && left.file_type() == right.file_type()
}
