#![deny(warnings)]

//! gitim-updater: shared core for GitIM self-update.
//!
//! Pure helpers (version parsing, platform detection, URL formatting) sit
//! alongside async IO helpers (`fetch_latest_tag`, `download_and_extract`) and
//! a sync atomic-replace helper (`replace_binaries`). The CLI and runtime both
//! drive the flow through these — no direct reqwest / tar calls at callsites.

use std::path::{Path, PathBuf};
use thiserror::Error;

/// GitHub repo that hosts the release tarballs.
pub const RELEASES_REPO: &str = "CiferaTeam/gitim-releases";

/// Binaries shipped in every release tarball, in install order.
pub const BINARIES: &[&str] = &["gitim", "gitim-daemon", "gitim-runtime"];

/// Errors the updater can surface to callers.
///
/// Variants marked with `#[from]` chain the underlying error so loggers and
/// callers can keep context (the wrapped error's `Display` is flattened by
/// `thiserror`). Pure helpers in this module only ever return
/// `UnsupportedPlatform`; the rest are reserved for upcoming IO functions.
#[derive(Debug, Error)]
pub enum UpdateError {
    #[error("unsupported platform: os={os}, arch={arch}")]
    UnsupportedPlatform { os: String, arch: String },

    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("HTTP status {0}")]
    HttpStatus(u16),

    #[error("extract failed: {0}")]
    Extract(String),

    #[error("missing binary in archive: {0}")]
    MissingBinary(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Parse a `major.minor.patch` version string, tolerating an optional leading `v`.
///
/// Returns `None` for malformed input (missing component, non-numeric, pre-release
/// suffix, etc.). Empty string -> `None`. Four-segment inputs like `"1.2.3.4"` and
/// pre-release suffixes are rejected — this is **stricter than** the original CLI
/// helper (`crates/gitim-cli/src/commands/update.rs`), which silently discarded
/// trailing segments. The tighter contract is intentional: fail-closed on anything
/// we don't fully understand.
pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.strip_prefix('v').unwrap_or(s);
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    // Reject trailing segments (e.g. "1.2.3.4"). Tightens CLI semantics — the
    // original silently accepted the first three parts and dropped the rest.
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// True iff `remote` is strictly newer than `current`. Malformed inputs -> false
/// (fail closed: never offer to "update" when we cannot compare).
pub fn is_newer(current: &str, remote: &str) -> bool {
    match (parse_version(current), parse_version(remote)) {
        (Some(c), Some(r)) => r > c,
        _ => false,
    }
}

/// Canonical platform slug used in release tarball names.
///
/// Returns one of `darwin-arm64`, `darwin-x86_64`, `linux-arm64`, `linux-x86_64`.
/// Anything else is `UnsupportedPlatform`.
pub fn detect_platform() -> Result<String, UpdateError> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let arch_name = match arch {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        _ => {
            return Err(UpdateError::UnsupportedPlatform {
                os: os.to_string(),
                arch: arch.to_string(),
            });
        }
    };
    let os_name = match os {
        "macos" => "darwin",
        "linux" => "linux",
        _ => {
            return Err(UpdateError::UnsupportedPlatform {
                os: os.to_string(),
                arch: arch.to_string(),
            });
        }
    };
    Ok(format!("{os_name}-{arch_name}"))
}

/// GitHub release asset URL for the given tag + platform.
pub fn download_url(tag: &str, platform: &str) -> String {
    format!(
        "https://github.com/{RELEASES_REPO}/releases/download/{tag}/gitim-{tag}-{platform}.tar.gz"
    )
}

/// GitHub "latest release" API URL.
pub fn latest_release_api_url() -> String {
    format!("https://api.github.com/repos/{RELEASES_REPO}/releases/latest")
}

// -- IO helpers -------------------------------------------------------------
//
// All three helpers funnel errors through `UpdateError`. Network-layer failures
// surface through `#[from] reqwest::Error`; archive / JSON-shape failures use
// `Extract(String)`; disk failures use `#[from] io::Error` or bubble up with
// extra context via `Extract`. `HttpStatus(u16)` is raised eagerly on any
// non-2xx response so callers can distinguish "couldn't reach GitHub" from
// "GitHub said 404 for that tag".

const USER_AGENT: &str = "gitim-updater";

/// Fetch the `tag_name` of the latest release from the GitHub releases API.
pub async fn fetch_latest_tag() -> Result<String, UpdateError> {
    let client = reqwest::Client::builder().user_agent(USER_AGENT).build()?;
    let resp = client.get(latest_release_api_url()).send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(UpdateError::HttpStatus(status.as_u16()));
    }
    let body: serde_json::Value = resp.json().await?;
    body["tag_name"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| UpdateError::Extract("no tag_name in release response".to_string()))
}

/// Download a tarball from `url` and unpack it into `dest`.
///
/// Small (5-20MB) tarballs are read fully into memory — streaming to disk adds
/// complexity the current sizes don't warrant.
pub async fn download_and_extract(url: &str, dest: &Path) -> Result<(), UpdateError> {
    let client = reqwest::Client::builder().user_agent(USER_AGENT).build()?;
    let resp = client.get(url).send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(UpdateError::HttpStatus(status.as_u16()));
    }
    let bytes = resp.bytes().await?;
    let decoder = flate2::read::GzDecoder::new(&bytes[..]);
    let mut archive = tar::Archive::new(decoder);
    // Archive corruption / malformed tar entries -> Extract (semantic mismatch
    // with the archive contract), not raw Io.
    archive
        .unpack(dest)
        .map_err(|e| UpdateError::Extract(format!("tar unpack failed: {e}")))?;
    Ok(())
}

/// Recursively find a file named `name` under `dir`. Directories are skipped.
pub fn find_binary(dir: &Path, name: &str) -> Option<PathBuf> {
    for entry in walkdir(dir) {
        let matches = entry
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n == name);
        if matches && entry.is_file() {
            return Some(entry);
        }
    }
    None
}

fn walkdir(dir: &Path) -> Vec<PathBuf> {
    let mut results = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                results.extend(walkdir(&path));
            } else {
                results.push(path);
            }
        }
    }
    results
}

/// Literal `.old` backup path — appends to the full file name, not via
/// `Path::with_extension` (which *replaces* extensions). On Unix our binaries
/// have no extension so either approach works; on Windows `.exe` would be
/// preserved as `gitim.exe.old` rather than collapsed to `gitim.old`.
fn backup_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_os_string();
    s.push(".old");
    PathBuf::from(s)
}

#[cfg(unix)]
fn set_exec_perms(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn set_exec_perms(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Atomically replace every binary in [`BINARIES`] that exists under `src_dir`.
///
/// For each binary:
/// 1. Locate it under `src_dir` via `find_binary`; missing -> warn + skip.
/// 2. If the destination exists, rename it to `<dest>.old` (tracked for rollback).
/// 3. Copy the new file into place and chmod 0o755.
///
/// If any step fails, every rename performed so far is reversed (in reverse
/// order) and the original error is returned. On full success, `.old` backups
/// are deleted unless `keep_backup` is true.
///
/// Returns the list of binary names that were successfully replaced.
pub fn replace_binaries(
    src_dir: &Path,
    install_dir: &Path,
    keep_backup: bool,
) -> Result<Vec<String>, UpdateError> {
    // (dest, backup) pairs for rollback. Each entry means "we renamed `dest`
    // away to `backup`" — rollback restores by renaming `backup` back to `dest`.
    let mut renames: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut installed: Vec<String> = Vec::new();

    for bin_name in BINARIES {
        let Some(src) = find_binary(src_dir, bin_name) else {
            eprintln!("Warning: {bin_name} not found in archive, skipping");
            continue;
        };
        let dest = install_dir.join(bin_name);
        let backup = backup_path(&dest);

        // Step 1: move any existing binary out of the way.
        if dest.exists() {
            if let Err(e) = std::fs::rename(&dest, &backup) {
                rollback(&renames);
                return Err(UpdateError::Io(e));
            }
            renames.push((dest.clone(), backup.clone()));
        }

        // Step 2: copy the new binary into place.
        if let Err(e) = std::fs::copy(&src, &dest) {
            // Best-effort: remove any partial dest the failed copy may have left.
            let _ = std::fs::remove_file(&dest);
            rollback(&renames);
            return Err(UpdateError::Io(e));
        }

        // Step 3: set executable perms.
        if let Err(e) = set_exec_perms(&dest) {
            let _ = std::fs::remove_file(&dest);
            rollback(&renames);
            return Err(UpdateError::Io(e));
        }

        installed.push(bin_name.to_string());
    }

    // Success: optionally drop backups.
    if !keep_backup {
        for (_, backup) in &renames {
            let _ = std::fs::remove_file(backup);
        }
    }

    Ok(installed)
}

/// Undo renames in reverse order. Best-effort — rollback failures are logged
/// but not propagated; the caller already has a primary error to report.
fn rollback(renames: &[(PathBuf, PathBuf)]) {
    for (dest, backup) in renames.iter().rev() {
        // Clear any partial file sitting at `dest` so the rename can land.
        if dest.exists() {
            let _ = std::fs::remove_file(dest);
        }
        if let Err(e) = std::fs::rename(backup, dest) {
            eprintln!(
                "Warning: rollback failed to restore {} from {}: {}",
                dest.display(),
                backup.display(),
                e
            );
        }
    }
}
