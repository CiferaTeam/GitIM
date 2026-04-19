#![deny(warnings)]

//! gitim-updater: shared core for GitIM self-update.
//!
//! Pure helpers live here (version parsing, platform detection, URL formatting).
//! IO functions (download / extract / replace binaries) will be added in a
//! follow-up task; the `UpdateError` variants they will use are declared now so
//! the public error surface stays stable across tasks.

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
/// suffix, etc.). Empty string -> `None`.
pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.strip_prefix('v').unwrap_or(s);
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    // Reject trailing segments (e.g. "1.2.3.4") to match existing CLI semantics.
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
