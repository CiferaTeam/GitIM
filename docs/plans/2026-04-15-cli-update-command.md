# `gitim update` CLI Command Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `gitim update [VERSION]` command that self-updates all three binaries (gitim, gitim-daemon, gitim-runtime) from GitHub Releases.

**Architecture:** The update command is self-contained in `commands/update.rs`. It fetches release metadata from `CiferaTeam/GitIM` via GitHub API, downloads the platform-matching tarball, extracts it, and replaces binaries in-place. If a daemon is running, it prompts the user to stop it first. Version is embedded at compile time via `env!("CARGO_PKG_VERSION")`.

**Tech Stack:** reqwest (rustls-tls), flate2, tar, clap

---

## File Map

| File | Action | Responsibility |
|------|--------|---------------|
| `crates/gitim-cli/Cargo.toml` | Modify | Add reqwest, flate2, tar dependencies |
| `crates/gitim-cli/src/commands/update.rs` | Create | All update logic: version check, download, extract, replace |
| `crates/gitim-cli/src/commands/mod.rs` | Modify | Add `pub mod update;` |
| `crates/gitim-cli/src/main.rs` | Modify | Add `Update` variant, route before `init_client()` |

---

### Task 1: Add dependencies to gitim-cli

**Files:**
- Modify: `crates/gitim-cli/Cargo.toml`

- [ ] **Step 1: Add reqwest, flate2, tar to Cargo.toml**

```toml
[dependencies]
gitim-client = { path = "../gitim-client" }
clap = { version = "4", features = ["derive"] }
tokio.workspace = true
serde_json.workspace = true
regex.workspace = true
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json"] }
flate2 = "1"
tar = "0.4"
```

Notes:
- `rustls-tls` avoids OpenSSL system dep, keeps binary portable
- `default-features = false` drops `default-tls` (OpenSSL)
- `json` feature for deserializing GitHub API response

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p gitim-cli`
Expected: compiles without errors

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-cli/Cargo.toml Cargo.lock
git commit -m "feat(cli): add reqwest/flate2/tar deps for update command"
```

---

### Task 2: Pure helper functions + unit tests

**Files:**
- Create: `crates/gitim-cli/src/commands/update.rs`
- Modify: `crates/gitim-cli/src/commands/mod.rs`

- [ ] **Step 1: Write unit tests for version parsing and comparison**

In `crates/gitim-cli/src/commands/update.rs`:

```rust
#![deny(warnings)]

const RELEASES_REPO: &str = "CiferaTeam/GitIM";
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
        // handles v prefix
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
            "https://github.com/CiferaTeam/GitIM/releases/download/v0.3.1/gitim-v0.3.1-darwin-arm64.tar.gz"
        );
    }

    #[test]
    fn test_latest_release_api_url() {
        assert_eq!(
            latest_release_api_url(),
            "https://api.github.com/repos/CiferaTeam/GitIM/releases/latest"
        );
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/gitim-cli/src/commands/mod.rs`, add:

```rust
pub mod update;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p gitim-cli`
Expected: all 5 tests pass

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-cli/src/commands/update.rs crates/gitim-cli/src/commands/mod.rs
git commit -m "feat(cli): add update helpers with tests (version, platform, URLs)"
```

---

### Task 3: Wire Update command into CLI

**Files:**
- Modify: `crates/gitim-cli/src/main.rs`

- [ ] **Step 1: Add Update variant to Commands enum**

In `main.rs`, add to the `Commands` enum (after the `Stop` variant):

```rust
    /// Update GitIM to the latest version (or a specified version)
    Update {
        /// Target version (e.g. "0.4.0"). Defaults to latest release.
        version: Option<String>,
        /// Skip confirmation prompts
        #[arg(short, long)]
        yes: bool,
    },
```

- [ ] **Step 2: Route the Update command before init_client()**

In the `main()` function, add a handler block after the `Onboard` block and before `let client = init_client();`:

```rust
    if let Commands::Update { version, yes } = &cli.command {
        commands::update::cmd_update(version.as_deref(), *yes || cli.json).await;
        return;
    }
```

Note: `cli.json` implies non-interactive mode, so we skip confirmation.

- [ ] **Step 3: Add unreachable arm in the match**

In the `match cli.command` block, add:

```rust
        Commands::Update { .. } => unreachable!(),
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p gitim-cli`
Expected: compile error because `cmd_update` doesn't exist yet (that's fine, we'll add it in Task 4). Or add a stub:

Add to bottom of `update.rs` (temporarily):

```rust
pub async fn cmd_update(_version: Option<&str>, _yes: bool) {
    eprintln!("update: not yet implemented");
    std::process::exit(1);
}
```

Run: `cargo check -p gitim-cli`
Expected: compiles. Then run `cargo test -p gitim-cli` to make sure existing tests still pass.

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-cli/src/main.rs crates/gitim-cli/src/commands/update.rs
git commit -m "feat(cli): wire Update subcommand into CLI router"
```

---

### Task 4: Implement the full update flow

**Files:**
- Modify: `crates/gitim-cli/src/commands/update.rs`

Replace the stub `cmd_update` and add the supporting I/O functions. The full `update.rs` after this task:

- [ ] **Step 1: Add I/O helper — confirm prompt**

```rust
fn confirm(prompt: &str) -> bool {
    use std::io::{self, Write};
    print!("{prompt} [y/N] ");
    io::stdout().flush().unwrap();
    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}
```

- [ ] **Step 2: Add I/O helper — fetch latest tag**

```rust
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
```

- [ ] **Step 3: Add I/O helper — download and extract**

```rust
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
```

- [ ] **Step 4: Add I/O helper — replace binaries**

```rust
fn replace_binaries(
    extracted_dir: &std::path::Path,
    install_dir: &std::path::Path,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut installed = Vec::new();
    for bin_name in BINARIES {
        // The tarball extracts to a subdirectory like gitim-v0.3.1-darwin-arm64/
        // Walk the extracted dir to find each binary
        let src = find_binary(extracted_dir, bin_name);
        let Some(src) = src else {
            eprintln!("Warning: {bin_name} not found in archive, skipping");
            continue;
        };
        let dest = install_dir.join(bin_name);
        // On Unix: remove then copy (works even if the current binary is running,
        // because the OS keeps the old inode open via fd)
        if dest.exists() {
            std::fs::remove_file(&dest)?;
        }
        std::fs::copy(&src, &dest)?;
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
        if entry.file_name().to_str() == Some(name) && entry.is_file() {
            return Some(entry);
        }
    }
    None
}

/// Simple recursive directory walk (no extra dep needed)
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
```

- [ ] **Step 5: Implement cmd_update — the main orchestrator**

Replace the stub with:

```rust
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
```

- [ ] **Step 6: Add tempfile dep to Cargo.toml**

```toml
tempfile = "3"
```

- [ ] **Step 7: Verify compilation and tests**

Run: `cargo test -p gitim-cli`
Expected: all tests pass, no warnings

Run: `cargo check -p gitim-cli`
Expected: compiles

- [ ] **Step 8: Commit**

```bash
git add crates/gitim-cli/src/commands/update.rs crates/gitim-cli/Cargo.toml Cargo.lock
git commit -m "feat(cli): implement gitim update command (download, extract, replace)"
```

---

### Task 5: Manual verification

- [ ] **Step 1: Build and test --help**

Run: `cargo build -p gitim-cli && ./target/debug/gitim update --help`
Expected output should show:
```
Update GitIM to the latest version (or a specified version)

Usage: gitim update [OPTIONS] [VERSION]

Arguments:
  [VERSION]  Target version (e.g. "0.4.0"). Defaults to latest release.

Options:
  -y, --yes   Skip confirmation prompts
  -h, --help  Print help
```

- [ ] **Step 2: Test "already up to date" path**

Run: `cargo build -p gitim-cli && ./target/debug/gitim update`
Expected: either "Already up to date" or shows target version (depending on whether a newer release exists).

- [ ] **Step 3: Test with explicit version (current version)**

Run: `./target/debug/gitim update 0.3.1`
Expected: downloads and installs v0.3.1 (explicit version always proceeds, even if same).

- [ ] **Step 4: Commit any fixes**

If any adjustments were needed during manual testing, commit them.

```bash
git add -A
git commit -m "fix(cli): update command adjustments from manual testing"
```
