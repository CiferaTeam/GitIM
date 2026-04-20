#![deny(warnings)]

//! `gitim update` CLI orchestrator.
//!
//! All download / extract / replace logic lives in `gitim-updater`. This file
//! owns only the user-facing interaction: progress messages on stderr, the
//! confirm prompt when a daemon is running, and exit-code semantics.

use gitim_updater::{
    detect_platform, fetch_latest_tag, install_update, is_newer, replace_binaries, RELEASES_REPO,
};

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

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
    eprintln!("Downloading {tag} ({platform})...");
    let tmp = tempfile::tempdir().unwrap_or_else(|e| {
        eprintln!("Error: cannot create temp directory: {e}");
        std::process::exit(1);
    });
    let base = format!("https://github.com/{RELEASES_REPO}/releases/download");
    if let Err(e) = install_update(&base, &tag, &platform, tmp.path()).await {
        eprintln!("Error: download failed: {e}");
        std::process::exit(1);
    }

    // 7. Replace binaries (CLI doesn't retain backups — they're a debug aid
    // only the runtime's update flow needs).
    eprintln!("Installing to {}...", install_dir.display());
    match replace_binaries(tmp.path(), install_dir, false) {
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
