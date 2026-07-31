use std::fs::{self, File, OpenOptions};
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
    snapshot_skill_directory_with_observer(source, request_dir, |_| {})
}

fn snapshot_skill_directory_with_observer(
    source: &Path,
    request_dir: &Path,
    observer: impl Fn(&Path),
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
    #[cfg(unix)]
    {
        let source_directory = open_directory(&source)?;
        let opened_metadata = source_directory
            .metadata()
            .map_err(|_| SkillError::InvalidPackage)?;
        if !same_file_identity(&source_metadata, &opened_metadata) {
            return Err(SkillError::InvalidPackage);
        }
        collect_directory_unix(
            &source_directory,
            Path::new(""),
            staging.path(),
            &mut entries,
            &mut total_bytes,
            &observer,
        )?;
        let source_after = fs::symlink_metadata(source).map_err(|_| SkillError::InvalidPackage)?;
        if source_after.file_type().is_symlink()
            || !same_file_identity(&opened_metadata, &source_after)
        {
            return Err(SkillError::InvalidPackage);
        }
    }
    #[cfg(not(unix))]
    collect_directory_fallback(
        &source,
        &source,
        staging.path(),
        &mut entries,
        &mut total_bytes,
        &observer,
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

#[cfg(unix)]
fn collect_directory_unix(
    directory: &File,
    relative_directory: &Path,
    destination_root: &Path,
    entries: &mut Vec<PackageEntry>,
    total_bytes: &mut usize,
    observer: &impl Fn(&Path),
) -> Result<(), SkillError> {
    let mut children = read_directory_names(directory)?;
    children.sort();

    for name in children {
        validate_relative_path(Path::new(&name))?;
        let relative = relative_directory.join(&name);
        validate_relative_path(&relative)?;
        let child = open_entry_at(directory, &name)?;
        let opened = child.metadata().map_err(|_| SkillError::InvalidPackage)?;

        if opened.is_dir() {
            observer(&relative);
            collect_directory_unix(
                &child,
                &relative,
                destination_root,
                entries,
                total_bytes,
                observer,
            )?;
            let current = open_entry_at(directory, &name)?;
            let current_metadata = current.metadata().map_err(|_| SkillError::InvalidPackage)?;
            if !current_metadata.is_dir() || !same_file_identity(&opened, &current_metadata) {
                return Err(SkillError::InvalidPackage);
            }
        } else if opened.is_file() {
            collect_regular_file_unix(
                child,
                &relative,
                &opened,
                destination_root,
                entries,
                total_bytes,
            )?;
            let current = open_entry_at(directory, &name)?;
            let current_metadata = current.metadata().map_err(|_| SkillError::InvalidPackage)?;
            if !current_metadata.is_file() || !same_file_identity(&opened, &current_metadata) {
                return Err(SkillError::InvalidPackage);
            }
        } else {
            return Err(SkillError::InvalidPackage);
        }
    }
    Ok(())
}

#[cfg(unix)]
fn collect_regular_file_unix(
    mut file: File,
    relative: &Path,
    before: &fs::Metadata,
    destination_root: &Path,
    entries: &mut Vec<PackageEntry>,
    total_bytes: &mut usize,
) -> Result<(), SkillError> {
    if entries.len() >= MAX_PACKAGE_FILES {
        return Err(SkillError::PackageTooLarge);
    }
    let length = usize::try_from(before.len()).map_err(|_| SkillError::PackageTooLarge)?;
    if length > MAX_PACKAGE_FILE_BYTES {
        return Err(SkillError::PackageTooLarge);
    }

    let mut bytes = Vec::with_capacity(length);
    (&mut file)
        .take(u64::try_from(MAX_PACKAGE_FILE_BYTES).unwrap_or(u64::MAX) + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| SkillError::InvalidPackage)?;
    if bytes.len() > MAX_PACKAGE_FILE_BYTES {
        return Err(SkillError::PackageTooLarge);
    }
    let after = file.metadata().map_err(|_| SkillError::InvalidPackage)?;
    if bytes.len() != length || !same_file_identity(before, &after) {
        return Err(SkillError::InvalidPackage);
    }

    *total_bytes = total_bytes
        .checked_add(bytes.len())
        .ok_or(SkillError::PackageTooLarge)?;
    if *total_bytes > MAX_PACKAGE_BYTES {
        return Err(SkillError::PackageTooLarge);
    }
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
    Ok(())
}

#[cfg(unix)]
fn open_directory(path: &Path) -> Result<File, SkillError> {
    let mut options = OpenOptions::new();
    use std::os::unix::fs::OpenOptionsExt;
    options
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| SkillError::InvalidPackage)
}

#[cfg(unix)]
fn open_entry_at(directory: &File, name: &std::ffi::OsStr) -> Result<File, SkillError> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let name = std::ffi::CString::new(name.as_bytes()).map_err(|_| SkillError::InvalidPackage)?;
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
        )
    };
    if descriptor < 0 {
        return Err(SkillError::InvalidPackage);
    }
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

#[cfg(unix)]
fn read_directory_names(directory: &File) -> Result<Vec<std::ffi::OsString>, SkillError> {
    use std::ffi::CStr;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStringExt;

    let duplicate = unsafe { libc::dup(directory.as_raw_fd()) };
    if duplicate < 0 {
        return Err(SkillError::InvalidPackage);
    }
    let stream = unsafe { libc::fdopendir(duplicate) };
    if stream.is_null() {
        unsafe {
            libc::close(duplicate);
        }
        return Err(SkillError::InvalidPackage);
    }

    let mut names = Vec::new();
    loop {
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name != b"." && name != b".." {
            names.push(std::ffi::OsString::from_vec(name.to_vec()));
        }
    }
    if unsafe { libc::closedir(stream) } != 0 {
        return Err(SkillError::InvalidPackage);
    }
    Ok(names)
}

#[cfg(not(unix))]
fn collect_directory_fallback(
    source_root: &Path,
    directory: &Path,
    destination_root: &Path,
    entries: &mut Vec<PackageEntry>,
    total_bytes: &mut usize,
    observer: &impl Fn(&Path),
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
            observer(relative);
            collect_directory_fallback(
                source_root,
                &path,
                destination_root,
                entries,
                total_bytes,
                observer,
            )?;
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

#[cfg(not(unix))]
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

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::Path;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use super::snapshot_skill_directory_with_observer;

    #[test]
    fn directory_swap_never_imports_outside_bytes_and_leaves_no_request() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let references = source.join("references");
        fs::create_dir_all(&references).unwrap();
        fs::write(
            source.join("SKILL.md"),
            "---\nname: safe-skill\ndescription: safe\n---\n",
        )
        .unwrap();
        fs::write(references.join("safe.txt"), "safe").unwrap();

        let outside = root.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "outside-secret").unwrap();

        let parked = source.join("references-original");
        let source_for_swap = source.clone();
        let outside_for_swap = outside.clone();
        let (swap_tx, swap_rx) = mpsc::sync_channel(0);
        let (done_tx, done_rx) = mpsc::sync_channel(0);
        let swapper = thread::spawn(move || {
            swap_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            fs::rename(source_for_swap.join("references"), &parked).unwrap();
            symlink(&outside_for_swap, source_for_swap.join("references")).unwrap();
            done_tx.send(()).unwrap();
        });

        let request = root.path().join("request");
        let result =
            snapshot_skill_directory_with_observer(&source, &request, |relative: &Path| {
                if relative == Path::new("references") {
                    swap_tx.send(()).unwrap();
                    done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                }
            });
        swapper.join().unwrap();

        assert!(result.is_err());
        assert!(!request.exists());
        assert!(!root.path().join("request/references/secret.txt").exists());
    }
}
