#![deny(warnings)]

const RELEASES_REPO: &str = "CiferaTeam/gitim-releases";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const BINARIES: &[&str] = &["gitim", "gitim-daemon", "gitim-runtime"];

fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.strip_prefix('v').unwrap_or(s);
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    Some((major, minor, patch))
}

fn is_newer(current: &str, remote: &str) -> bool {
    match (parse_version(current), parse_version(remote)) {
        (Some(c), Some(r)) => r > c,
        _ => false,
    }
}

fn detect_platform() -> Result<String, String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let arch_name = match arch {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        other => return Err(format!("unsupported architecture: {other}")),
    };
    let os_name = match os {
        "macos" => "darwin",
        "linux" => "linux",
        other => return Err(format!("unsupported OS: {other}")),
    };
    Ok(format!("{os_name}-{arch_name}"))
}

fn download_url(tag: &str, platform: &str) -> String {
    format!(
        "https://github.com/{RELEASES_REPO}/releases/download/{tag}/gitim-{tag}-{platform}.tar.gz"
    )
}

fn latest_release_api_url() -> String {
    format!("https://api.github.com/repos/{RELEASES_REPO}/releases/latest")
}

fn confirm(prompt: &str) -> bool {
    use std::io::{self, Write};
    print!("{prompt} [y/N] ");
    let _ = io::stdout().flush();
    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}

async fn fetch_latest_tag() -> Result<String, Box<dyn std::error::Error>> {
    let client = reqwest::Client::builder()
        .user_agent("gitim-updater")
        .build()?;
    let resp: serde_json::Value = client
        .get(latest_release_api_url())
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    resp["tag_name"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "no tag_name in release response".into())
}

async fn download_and_extract(
    url: &str,
    dest: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::builder()
        .user_agent("gitim-updater")
        .build()?;
    let bytes = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let decoder = flate2::read::GzDecoder::new(&bytes[..]);
    let mut archive = tar::Archive::new(decoder);
    archive.unpack(dest)?;
    Ok(())
}

fn replace_binaries(
    extracted_dir: &std::path::Path,
    install_dir: &std::path::Path,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut installed = Vec::new();
    for bin_name in BINARIES {
        let src = find_binary(extracted_dir, bin_name);
        let Some(src) = src else {
            eprintln!("Warning: {bin_name} not found in archive, skipping");
            continue;
        };
        let dest = install_dir.join(bin_name);
        // Rename-then-copy: if copy fails, the backup is still usable
        let backup = dest.with_extension("old");
        if dest.exists() {
            std::fs::rename(&dest, &backup)?;
        }
        std::fs::copy(&src, &dest)?;
        let _ = std::fs::remove_file(&backup);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))?;
        }
        installed.push(bin_name.to_string());
    }
    Ok(installed)
}

fn find_binary(dir: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
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

fn walkdir(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
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

pub async fn cmd_update(version: Option<&str>, yes: bool) {
    // 1. Detect platform
    let platform = match detect_platform() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {e}");
            eprintln!("You can build from source instead: ./install-from-source.sh");
            std::process::exit(1);
        }
    };

    // 2. Resolve target version
    eprintln!("Checking for updates...");
    let tag = match version {
        Some(v) => {
            let v = v.strip_prefix('v').unwrap_or(v);
            format!("v{v}")
        }
        None => match fetch_latest_tag().await {
            Ok(t) => t,
            Err(e) => {
                eprintln!("Error: failed to fetch latest release: {e}");
                std::process::exit(1);
            }
        },
    };

    // 3. Compare versions (skip if requesting explicit version)
    let tag_version = tag.strip_prefix('v').unwrap_or(&tag);
    if version.is_none() && !is_newer(CURRENT_VERSION, tag_version) {
        eprintln!("Already up to date (v{CURRENT_VERSION}).");
        return;
    }
    eprintln!("Current: v{CURRENT_VERSION} -> Target: {tag}");

    // 4. Detect install directory
    let exe_path = std::env::current_exe().unwrap_or_else(|e| {
        eprintln!("Error: cannot determine binary location: {e}");
        std::process::exit(1);
    });
    let install_dir = exe_path.parent().unwrap_or_else(|| {
        eprintln!("Error: cannot determine install directory");
        std::process::exit(1);
    });

    // 5. Check for running daemon, prompt to stop
    let cwd = std::env::current_dir().ok();
    let repo_root = cwd.as_ref().and_then(|d| gitim_client::find_repo_root(d));
    if let Some(ref root) = repo_root {
        if gitim_client::is_daemon_running(root) {
            if !yes {
                eprintln!("A GitIM daemon is running for this repo.");
                if !confirm("Stop it before updating?") {
                    eprintln!("Aborted.");
                    std::process::exit(1);
                }
            }
            let client = gitim_client::GitimClient::new(root);
            let _ = client.stop().await;
            eprintln!("Daemon stopped.");
        }
    }

    // 6. Download
    let url = download_url(&tag, &platform);
    eprintln!("Downloading {tag} ({platform})...");
    let tmp = tempfile::tempdir().unwrap_or_else(|e| {
        eprintln!("Error: cannot create temp directory: {e}");
        std::process::exit(1);
    });
    if let Err(e) = download_and_extract(&url, tmp.path()).await {
        eprintln!("Error: download failed: {e}");
        std::process::exit(1);
    }

    // 7. Replace binaries
    eprintln!("Installing to {}...", install_dir.display());
    match replace_binaries(tmp.path(), install_dir) {
        Ok(installed) => {
            for name in &installed {
                eprintln!("  {name} -> {}", install_dir.join(name).display());
            }
            eprintln!("Updated to {tag}.");
        }
        Err(e) => {
            eprintln!("Error: failed to install binaries: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version() {
        assert_eq!(parse_version("0.3.1"), Some((0, 3, 1)));
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("v0.10.0"), Some((0, 10, 0)));
        assert_eq!(parse_version("bad"), None);
        assert_eq!(parse_version("1.2"), None);
    }

    #[test]
    fn test_is_newer() {
        assert!(is_newer("0.3.1", "0.4.0"));
        assert!(is_newer("0.3.1", "0.3.2"));
        assert!(is_newer("0.3.1", "1.0.0"));
        assert!(!is_newer("0.3.1", "0.3.1"));
        assert!(!is_newer("0.3.1", "0.3.0"));
        assert!(!is_newer("0.3.1", "0.2.9"));
        assert!(is_newer("0.3.1", "v0.4.0"));
    }

    #[test]
    fn test_detect_platform() {
        let platform = detect_platform();
        assert!(platform.is_ok());
        let p = platform.unwrap();
        assert!(p.contains('-'));
    }

    #[test]
    fn test_download_url() {
        let url = download_url("v0.3.1", "darwin-arm64");
        assert_eq!(
            url,
            "https://github.com/CiferaTeam/gitim-releases/releases/download/v0.3.1/gitim-v0.3.1-darwin-arm64.tar.gz"
        );
    }

    #[test]
    fn test_latest_release_api_url() {
        assert_eq!(
            latest_release_api_url(),
            "https://api.github.com/repos/CiferaTeam/gitim-releases/releases/latest"
        );
    }
}
