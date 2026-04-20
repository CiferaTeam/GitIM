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

/// All failure modes across fetch / download / extract / install.
///
/// Variants with `#[from]` chain the underlying error for context.
/// `MissingBinary` is constructed by callers (not by this crate yet) —
/// it's the variant the Task 6 runtime endpoint returns when an extracted
/// archive is missing one of `BINARIES`, before the async install phase
/// starts.
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

    #[error("sha256 mismatch: expected {expected}, actual {actual}")]
    Sha256Mismatch { expected: String, actual: String },

    #[error("sha256 line not found in SHA256SUMS for {0}")]
    Sha256LineMissing(String),

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

/// Verify that `bytes` hash to `expected_hex` under SHA-256.
///
/// `expected_hex` is a 64-char lowercase hex string (uppercase tolerated) —
/// the canonical SHA-256 output format from `shasum -a 256` / `sha256sum`.
/// Anything shorter, longer, or non-hex is rejected as
/// `UpdateError::Sha256Mismatch` (fail closed — we never silently accept a
/// malformed expectation).
pub fn verify_sha256(bytes: &[u8], expected_hex: &str) -> Result<(), UpdateError> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let actual_bytes = hasher.finalize();
    let actual_hex = hex::encode(actual_bytes);

    // Case-insensitive compare — sha256sum on BSD/mac emits lowercase, GNU
    // coreutils also lowercase, but older shasum(1) on macOS emits uppercase
    // for some flags. Normalize both.
    let expected_norm = expected_hex.trim().to_lowercase();

    // Length guard: SHA-256 is always 32 bytes = 64 hex chars. Anything else
    // is malformed upstream data — treat as mismatch rather than a separate
    // error variant (callers only care "verify failed").
    if expected_norm.len() != 64 || !expected_norm.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(UpdateError::Sha256Mismatch {
            expected: expected_hex.to_string(),
            actual: actual_hex,
        });
    }

    if expected_norm != actual_hex {
        return Err(UpdateError::Sha256Mismatch {
            expected: expected_norm,
            actual: actual_hex,
        });
    }
    Ok(())
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

// Base-URL overrides used by the runtime E2E test. Unset in production; when
// absent the defaults produce the same strings the original hard-coded URLs
// did, so the existing URL-contract tests (`download_url_contract`,
// `latest_release_api_url_contract`) continue to pass untouched. The
// indirection is a **test-only seam** — callers never pass URLs explicitly.
//
// Deliberately read inside the helper (not at `static` init) so a test setting
// the env var takes effect in the current process without reloading.
fn releases_api_base() -> String {
    std::env::var("GITIM_RELEASES_API_URL")
        .unwrap_or_else(|_| "https://api.github.com".to_string())
}

fn releases_download_base() -> String {
    std::env::var("GITIM_RELEASES_DOWNLOAD_BASE")
        .unwrap_or_else(|_| "https://github.com".to_string())
}

/// GitHub release asset URL for the given tag + platform.
pub fn download_url(tag: &str, platform: &str) -> String {
    format!(
        "{base}/{RELEASES_REPO}/releases/download/{tag}/gitim-{tag}-{platform}.tar.gz",
        base = releases_download_base(),
    )
}

/// GitHub "latest release" API URL.
pub fn latest_release_api_url() -> String {
    format!(
        "{base}/repos/{RELEASES_REPO}/releases/latest",
        base = releases_api_base(),
    )
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

/// Fetch the full body of `url` into memory. Used for both small SHA256SUMS
/// text files and the release tarball (10-20 MB at current binary sizes —
/// well within RAM, streaming to disk not worth the complexity).
///
/// Non-2xx -> `UpdateError::HttpStatus(code)`.
pub async fn download_bytes(url: &str) -> Result<Vec<u8>, UpdateError> {
    let client = reqwest::Client::builder().user_agent(USER_AGENT).build()?;
    let resp = client.get(url).send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(UpdateError::HttpStatus(status.as_u16()));
    }
    let bytes = resp.bytes().await?;
    Ok(bytes.to_vec())
}

/// Extract a gzipped-tar byte slice into `dest` on disk.
///
/// Pure sync; no network. The `tar` 0.4 default `Archive::unpack` rejects
/// absolute paths and `..` traversal — we rely on that for defense in depth.
/// Do not call `archive.set_preserve_permissions(true)` or relax the path
/// checks without re-evaluating the trust model.
pub fn extract_tarball(bytes: &[u8], dest: &Path) -> Result<(), UpdateError> {
    let decoder = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    archive
        .unpack(dest)
        .map_err(|e| UpdateError::Extract(format!("tar unpack failed: {e}")))?;
    Ok(())
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
    //
    // `tar` 0.4's default `Archive::unpack` rejects absolute paths and `..`
    // traversal — we rely on that for defense in depth. Do not call
    // `archive.set_preserve_permissions(true)` or relax the path checks
    // without re-evaluating the trust model (release tarballs from the
    // official repo are trusted, but multiple consumers now call this).
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
            // `file_type()` does NOT follow symlinks; `path.is_dir()` does.
            // A self-referential symlink in a malformed tarball would otherwise
            // trigger infinite recursion here.
            let Ok(ft) = entry.file_type() else { continue };
            let path = entry.path();
            if ft.is_dir() {
                results.extend(walkdir(&path));
            } else if ft.is_file() {
                results.push(path);
            }
            // Symlinks are intentionally skipped — release tarballs from
            // CiferaTeam/gitim-releases do not contain symlinks, and this
            // guards against symlink loops if a malformed archive appears.
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

/// A rollback entry recorded during [`replace_binaries`]. Two shapes because
/// the two install scenarios call for different recovery actions:
///
/// - `Restore`: the destination existed, so we renamed it to `.old` before
///   copying. Rollback renames `.old` back to the destination.
/// - `Remove`: the destination did not previously exist, so the new copy is
///   the only file on disk. Rollback removes the stranded copy.
///
/// The previous implementation only tracked the `Restore` case, which meant a
/// partial-install failure on a fresh destination left the new binary
/// stranded after rollback — inconsistent with the "atomic or not at all"
/// contract.
enum RollbackAction {
    Restore { dest: PathBuf, backup: PathBuf },
    Remove { dest: PathBuf },
}

/// Atomically replace every binary in [`BINARIES`] that exists under `src_dir`.
///
/// For each binary:
/// 1. Locate it under `src_dir` via `find_binary`; missing -> warn + skip.
/// 2. If the destination exists, rename it to `<dest>.old` (tracked as
///    `Restore` for rollback). If it doesn't exist, record `Remove` so
///    rollback can delete the freshly-created copy.
/// 3. Copy the new file into place and chmod 0o755.
///
/// If any step fails, every action performed so far is reversed (in reverse
/// order) and the original error is returned. On full success, `.old` backups
/// from `Restore` entries are deleted unless `keep_backup` is true.
///
/// Returns the list of binary names that were successfully replaced.
pub fn replace_binaries(
    src_dir: &Path,
    install_dir: &Path,
    keep_backup: bool,
) -> Result<Vec<String>, UpdateError> {
    let mut actions: Vec<RollbackAction> = Vec::new();
    let mut installed: Vec<String> = Vec::new();

    for bin_name in BINARIES {
        let Some(src) = find_binary(src_dir, bin_name) else {
            tracing::warn!(binary = %bin_name, "not found in archive, skipping");
            continue;
        };
        let dest = install_dir.join(bin_name);
        let backup = backup_path(&dest);
        let had_existing = dest.exists();

        // Step 1: move any existing binary out of the way.
        if had_existing {
            if let Err(e) = std::fs::rename(&dest, &backup) {
                rollback(&actions);
                return Err(UpdateError::Io(e));
            }
            actions.push(RollbackAction::Restore {
                dest: dest.clone(),
                backup: backup.clone(),
            });
        } else {
            // Pre-register the remove action BEFORE the copy: if the copy
            // partially wrote to disk and then failed, the next iteration's
            // rollback still needs to clean it up.
            actions.push(RollbackAction::Remove {
                dest: dest.clone(),
            });
        }

        // Step 2: copy the new binary into place.
        if let Err(e) = std::fs::copy(&src, &dest) {
            // Best-effort: remove any partial dest the failed copy may have left.
            let _ = std::fs::remove_file(&dest);
            rollback(&actions);
            return Err(UpdateError::Io(e));
        }

        // Step 3: set executable perms.
        if let Err(e) = set_exec_perms(&dest) {
            let _ = std::fs::remove_file(&dest);
            rollback(&actions);
            return Err(UpdateError::Io(e));
        }

        installed.push(bin_name.to_string());
    }

    // Success: optionally drop backups. `Remove` actions have no backup.
    if !keep_backup {
        for action in &actions {
            if let RollbackAction::Restore { backup, .. } = action {
                let _ = std::fs::remove_file(backup);
            }
        }
    }

    Ok(installed)
}

/// Undo actions in reverse order. Best-effort — rollback failures are logged
/// but not propagated; the caller already has a primary error to report.
fn rollback(actions: &[RollbackAction]) {
    for action in actions.iter().rev() {
        match action {
            RollbackAction::Restore { dest, backup } => {
                // Clear any partial file sitting at `dest` so the rename can land.
                if dest.exists() {
                    if let Err(e) = std::fs::remove_file(dest) {
                        tracing::warn!(
                            path = %dest.display(),
                            error = %e,
                            "rollback: failed to remove partial copy"
                        );
                    }
                }
                if let Err(e) = std::fs::rename(backup, dest) {
                    tracing::warn!(
                        from = %backup.display(),
                        to = %dest.display(),
                        error = %e,
                        "rollback: failed to restore backup"
                    );
                }
            }
            RollbackAction::Remove { dest } => {
                // Only remove if it exists; a failed copy may never have
                // produced a file in the first place.
                if dest.exists() {
                    if let Err(e) = std::fs::remove_file(dest) {
                        tracing::warn!(
                            path = %dest.display(),
                            error = %e,
                            "rollback: failed to remove newly-created binary"
                        );
                    }
                }
            }
        }
    }
}
