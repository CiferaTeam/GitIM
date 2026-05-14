#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Command;

use gitim_client::GitimClient;
use tempfile::{Builder, TempDir};

/// Ensure `gitim-daemon` binary is findable by adding target/debug to PATH,
/// and redirect daemon logs out of the developer's real `~/.gitim/logs/`.
///
/// Uses `Once` to avoid UB from concurrent set_var in multi-threaded test
/// runner — `call_once` serializes all env mutation through a single thread.
///
/// **Every test that spawns a daemon must call this first.** Skipping it
/// leaves `daemon_log::logs_dir()` falling back to `$HOME/.gitim/logs/`,
/// which would pollute the developer machine.
pub fn ensure_daemon_in_path() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let workspace_root = PathBuf::from(manifest_dir).join("../..");
        let target_debug = workspace_root.join("target/debug").canonicalize().unwrap();

        let current_path = std::env::var("PATH").unwrap_or_default();
        if !current_path.contains(target_debug.to_str().unwrap()) {
            std::env::set_var(
                "PATH",
                format!("{}:{}", target_debug.display(), current_path),
            );
        }

        // Redirect daemon logs from ~/.gitim/logs/ into a TempDir for this
        // test process. `daemon_log::logs_dir()` honors GITIM_LOG_DIR.
        // We `keep()` the TempDir intentionally — RAII drop would wipe the
        // dir while daemons may still be writing. OS temp reaper collects
        // it after the test binary exits.
        let log_tmp = TempDir::new().expect("tempdir for GITIM_LOG_DIR");
        std::env::set_var("GITIM_LOG_DIR", log_tmp.keep());
    });
}

/// Create a temp dir under /tmp to keep Unix socket paths under SUN_LEN (104 bytes on macOS).
pub fn short_tempdir() -> TempDir {
    Builder::new().prefix("gim").tempdir_in("/tmp").unwrap()
}

/// Temporarily point HOME at a fresh tempdir so tests that hit runtime routes
/// persisting `~/.gitim/runtime.json` cannot mutate the developer machine's
/// real GitIM config. Restores the prior HOME on drop.
///
/// Scope: this only covers `~/.gitim/runtime.json` and anything else keyed off
/// `dirs::home_dir()`. Daemon log pollution is handled separately by
/// `ensure_daemon_in_path` setting `GITIM_LOG_DIR`, so a test that only spawns
/// a daemon (without touching runtime.json) doesn't need a HomeGuard.
///
/// Note: `std::env::set_var("HOME", ...)` is process-global and not thread-safe.
/// Tests using this guard should be serialised with `#[serial(...)]` to avoid
/// races when running in cargo test's default multi-thread mode.
pub struct HomeGuard {
    original: Option<std::ffi::OsString>,
    tmp: TempDir,
}

impl HomeGuard {
    pub fn install() -> Self {
        let tmp = TempDir::new().expect("tempdir for HOME");
        let original = std::env::var_os("HOME");
        std::env::set_var("HOME", tmp.path());
        Self { original, tmp }
    }

    /// The tempdir HOME is currently pointing at. Useful for tests that need
    /// to write fixtures like `<home>/.gitim/runtime.json`.
    pub fn path(&self) -> &std::path::Path {
        self.tmp.path()
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match self.original.take() {
            Some(val) => std::env::set_var("HOME", val),
            None => std::env::remove_var("HOME"),
        }
    }
}

/// Create a bare git repo with an initial commit so clone works.
pub fn setup_bare_remote(tmp: &TempDir) -> PathBuf {
    let bare_path = tmp.path().join("remote.git");

    Command::new("git")
        .args(["init", "--bare", bare_path.to_str().unwrap()])
        .output()
        .unwrap();

    let init_clone = tmp.path().join("init-clone");
    Command::new("git")
        .args(["clone", bare_path.to_str().unwrap(), "init-clone"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(&init_clone)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&init_clone)
        .output()
        .unwrap();

    std::fs::write(init_clone.join(".gitkeep"), "").unwrap();
    Command::new("git")
        .args(["add", ".gitkeep"])
        .current_dir(&init_clone)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "initial"])
        .current_dir(&init_clone)
        .output()
        .unwrap();
    Command::new("git")
        .args(["push"])
        .current_dir(&init_clone)
        .output()
        .unwrap();

    bare_path
}

/// Stop daemon for a repo directory (best-effort cleanup).
pub async fn stop_daemon(repo_root: &std::path::Path) {
    let client = GitimClient::new(repo_root);
    let _ = client.stop().await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
}

pub fn fixture(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push("fixtures");
    path.push(name);
    path
}

pub fn resolve_stdbin(name: &str) -> String {
    let bin_path = format!("/bin/{name}");
    if std::path::Path::new(&bin_path).is_file() {
        bin_path
    } else {
        format!("/usr/bin/{name}")
    }
}
