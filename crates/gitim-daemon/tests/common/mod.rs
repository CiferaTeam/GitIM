#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(dead_code)]
//! Shared test fixtures for `gitim-daemon` integration tests.
//!
//! ## Design
//!
//! Three layers of helpers:
//!
//! 1. **Primitives** — `run_git`, `make_config`, `write_user`, `write_channel_meta`:
//!    low-level building blocks used by other helpers and by tests with unusual
//!    setups.
//!
//! 2. **State factory** — `make_state(root, current_user, users)`: creates an
//!    `Arc<AppState>` and seeds the in-memory user list. Call after all files
//!    are in place.
//!
//! 3. **Composite setups** — `setup_repo_alice`, `setup_repo_alice_bob`,
//!    `setup_repo_with_users`, `setup_repo_with_channel`: one-call fixtures
//!    that cover ≥80 % of test files.
//!
//! ## Seed invariant
//!
//! User YAML written by `write_user_default` is:
//! ```text
//! display_name: Alice\nrole: dev\nintroduction: hi\n
//! ```
//! (capitalised display_name, `role: dev`, `introduction: hi`).
//!
//! Bob's introduction is `hello` to match the existing literals in
//! archive_channel / board_test / card_test / unarchive_channel.
//!
//! Any test that needs different field values should call `write_user`
//! directly with explicit arguments.

use std::path::Path;
use std::sync::Arc;

use gitim_core::types::Config;
use gitim_daemon::api::Event;
use gitim_daemon::state::AppState;
use tempfile::TempDir;
use tokio::sync::broadcast;

// ─── Primitives ──────────────────────────────────────────────────────────────

/// Run a git command in `root`, asserting success. Env vars for author/committer
/// are set so commits don't need a configured git identity.
pub fn run_git(root: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@test.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@test.com")
        .output()
        .expect("git command failed");
    assert!(
        out.status.success(),
        "git {args:?} failed in {}: {}",
        root.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Parse the minimal config `"version: 1"` into a `Config`.
pub fn make_config() -> Config {
    serde_yaml::from_str("version: 1").unwrap()
}

/// Initialise a git repo at `root`: `git init → add . → commit "init"`.
/// All files already in `root` at call time are included in the initial commit.
pub fn init_repo(root: &Path) {
    run_git(root, &["init"]);
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-m", "init"]);
}

/// Write `users/<handler>.meta.yaml` with explicitly-supplied field values.
pub fn write_user(root: &Path, handler: &str, display_name: &str, role: &str, introduction: &str) {
    std::fs::create_dir_all(root.join("users")).unwrap();
    std::fs::write(
        root.join(format!("users/{handler}.meta.yaml")),
        format!("display_name: {display_name}\nrole: {role}\nintroduction: {introduction}\n"),
    )
    .unwrap();
}

/// Write `users/alice.meta.yaml` with the canonical test defaults:
/// `display_name: Alice`, `role: dev`, `introduction: hi`.
pub fn write_alice(root: &Path) {
    write_user(root, "alice", "Alice", "dev", "hi");
}

/// Write `users/bob.meta.yaml` with the canonical test defaults:
/// `display_name: Bob`, `role: dev`, `introduction: hello`.
///
/// Bob's introduction is `hello` (not `hi`) to match the seed strings in
/// archive_channel, board_test, card_test, and unarchive_channel.
pub fn write_bob(root: &Path) {
    write_user(root, "bob", "Bob", "dev", "hello");
}

/// Write a channel meta.yaml + empty .thread file.
///
/// The meta YAML format:
/// ```text
/// display_name: {name}\ncreated_by: {created_by}\ncreated_at: "20260323T000000Z"\nintroduction: general channel\nmembers: []\n
/// ```
pub fn write_channel_meta(root: &Path, name: &str, created_by: &str, members: &[&str]) {
    let ch_dir = root.join("channels");
    std::fs::create_dir_all(&ch_dir).unwrap();
    let members_yaml = if members.is_empty() {
        "[]".to_string()
    } else {
        format!(
            "\n{}",
            members
                .iter()
                .map(|m| format!("- {m}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    std::fs::write(
        ch_dir.join(format!("{name}.meta.yaml")),
        format!(
            "display_name: {name}\ncreated_by: {created_by}\ncreated_at: \"20260323T000000Z\"\nintroduction: general channel\nmembers: {members_yaml}\n"
        ),
    )
    .unwrap();
    std::fs::write(ch_dir.join(format!("{name}.thread")), "").unwrap();
}

// ─── State factory ────────────────────────────────────────────────────────────

/// Build an `Arc<AppState>` from an already-initialised git repo at `root`.
///
/// `current_user` maps to `AppState.current_user` (the daemon's identity).
/// `users` is the full in-memory user list written into `state.users`.
pub async fn make_state(
    root: std::path::PathBuf,
    current_user: Option<&str>,
    users: &[&str],
) -> Arc<AppState> {
    let (event_tx, _) = broadcast::channel::<Event>(256);
    let state = Arc::new(AppState::new(
        root,
        make_config(),
        event_tx,
        current_user.map(|s| s.to_string()),
    ));
    {
        let mut u = state.users.write().await;
        *u = users.iter().map(|s| s.to_string()).collect();
    }
    state
}

// ─── Composite setups ─────────────────────────────────────────────────────────

/// Minimal repo: alice registered, git init'd.
///
/// Used by: archive_channel, archive_dm_test, archive_user_test,
///          backward_compat, poll_archive_test, poll_cron_test, reconcile,
///          cron_fire_test, indexer_disabled_test, and similar.
pub async fn setup_repo_alice() -> (TempDir, Arc<AppState>) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    std::fs::create_dir_all(root.join("users")).unwrap();
    write_alice(&root);
    init_repo(&root);
    let state = make_state(root, Some("alice"), &["alice"]).await;
    (tmp, state)
}

/// Repo with alice + bob registered, git init'd.
///
/// Used by: archive_channel, archive_dm_test, archive_user_test, board_test,
///          card_test, cron_create_test, cron_departed_test, cron_lifecycle_test,
///          cron_read_test, unarchive_channel.
pub async fn setup_repo_alice_bob() -> (TempDir, Arc<AppState>) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    std::fs::create_dir_all(root.join("users")).unwrap();
    write_alice(&root);
    write_bob(&root);
    init_repo(&root);
    let state = make_state(root, Some("alice"), &["alice", "bob"]).await;
    (tmp, state)
}

/// Repo with an arbitrary set of handlers registered.
///
/// All users get the same defaults: `display_name = <handler>`, `role = dev`,
/// `introduction = hi`. Current user is `handlers[0]`.
///
/// Used by: concurrent_write_test, depart_user_test, depart_user_perf (partial),
///          poll_recipients_test, cross_clone_burn_test (partial).
pub async fn setup_repo_with_users(handlers: &[&str]) -> (TempDir, Arc<AppState>) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    std::fs::create_dir_all(root.join("users")).unwrap();
    for h in handlers {
        write_user(&root, h, h, "dev", "hi");
    }
    init_repo(&root);
    let state = make_state(root, handlers.first().copied(), handlers).await;
    (tmp, state)
}

/// Repo with alice + a pre-created "general" channel.
///
/// Channel meta YAML:
/// ```text
/// display_name: general\ncreated_by: alice\ncreated_at: "20260323T000000Z"\nintroduction: general channel\nmembers: []\n
/// ```
///
/// Used by: commit_test, thread_test, push_test (partial), push_confirm_test
///          (no-remote variant).
pub async fn setup_repo_with_channel(channel: &str) -> (TempDir, Arc<AppState>) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    std::fs::create_dir_all(root.join("channels")).unwrap();
    std::fs::create_dir_all(root.join("users")).unwrap();
    std::fs::create_dir_all(root.join(".gitim")).unwrap();
    std::fs::write(root.join(".gitim/config.yaml"), "version: 1").unwrap();
    write_alice(&root);
    write_channel_meta(&root, channel, "alice", &[]);
    init_repo(&root);
    let state = make_state(root, Some("alice"), &["alice"]).await;
    (tmp, state)
}

// ─── Bare+clone helpers ───────────────────────────────────────────────────────

/// Initialise a bare repo and clone it into `clone_path`, setting local git
/// identity for commits. `clone_path` must be an existing empty directory.
pub fn init_bare_and_clone(bare: &Path, clone: &Path) {
    run_git(bare, &["init", "--bare"]);
    run_git(
        clone.parent().unwrap(),
        &["clone", bare.to_str().unwrap(), clone.to_str().unwrap()],
    );
    run_git(clone, &["config", "user.email", "test@test.com"]);
    run_git(clone, &["config", "user.name", "test"]);
}
