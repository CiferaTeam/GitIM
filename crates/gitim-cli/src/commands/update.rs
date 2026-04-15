const RELEASES_REPO: &str = "CiferaTeam/gitim-releases";
#[allow(dead_code)] // Used in Task 4 full update flow
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
#[allow(dead_code)] // Used in Task 4 full update flow
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

// Temporary stub -- will be replaced in Task 4
#[allow(dead_code)] // Wired in Task 3
pub async fn cmd_update(_version: Option<&str>, _yes: bool) {
    eprintln!("update: not yet implemented");
    std::process::exit(1);
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
