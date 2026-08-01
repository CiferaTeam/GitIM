#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use gitim_core::skill::{RequestId, SkillMutationRequest, SkillWorkspaceBootstrapRequest};
use gitim_daemon::startup::recover_skill_transactions_before_serving;
use gitim_sync::git::GitStorage;
use gitim_sync::skill::guard::SkillSyncGuard;
use gitim_sync::skill::transaction::{
    execute_remote_skill_transaction_with_test_config, RemoteSkillTransactionRequest,
    SkillTransactionCrashPoint, SkillTransactionTestConfig,
};
use tempfile::TempDir;

fn git<I, S>(root: &Path, args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("failed to run git");
    assert!(
        output.status.success(),
        "git failed in {}: {}",
        root.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn configure(root: &Path) {
    git(root, ["config", "user.name", "Fixture"]);
    git(root, ["config", "user.email", "fixture@example.com"]);
    git(root, ["config", "commit.gpgsign", "false"]);
}

fn remote_fixture() -> (TempDir, TempDir) {
    let remote = TempDir::new().unwrap();
    git(remote.path(), ["init", "--bare", "-b", "main"]);
    let seed = TempDir::new().unwrap();
    git(seed.path(), ["init", "-b", "main"]);
    configure(seed.path());
    fs::create_dir_all(seed.path().join("users")).unwrap();
    fs::write(
        seed.path().join("users/alice.meta.yaml"),
        "display_name: Alice\nrole: human\nintroduction: Owner\n",
    )
    .unwrap();
    git(seed.path(), ["add", "."]);
    git(seed.path(), ["commit", "-m", "initialize workspace"]);
    git(
        seed.path(),
        [
            OsStr::new("remote"),
            OsStr::new("add"),
            OsStr::new("origin"),
            remote.path().as_os_str(),
        ],
    );
    git(seed.path(), ["push", "-u", "origin", "main"]);

    let clone = TempDir::new().unwrap();
    git(
        clone.path(),
        [
            OsStr::new("clone"),
            remote.path().as_os_str(),
            OsStr::new("."),
        ],
    );
    configure(clone.path());
    (remote, clone)
}

#[tokio::test]
async fn production_startup_recovers_pushed_skill_transaction_before_serving() {
    let (_remote, clone) = remote_fixture();
    let request_id = RequestId::generate();
    let repo = GitStorage::new(clone.path());
    let guard = SkillSyncGuard::new(clone.path()).unwrap();
    let crashed = execute_remote_skill_transaction_with_test_config(
        &repo,
        &guard,
        RemoteSkillTransactionRequest {
            request: SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
                request_id: request_id.clone(),
            }),
            actor: "alice".to_owned(),
            author_email: "alice@example.com".to_owned(),
            now: "2026-07-31T00:00:00Z".to_owned(),
            package: None,
        },
        SkillTransactionTestConfig {
            crash_after: Some(SkillTransactionCrashPoint::AfterPushed),
            ..SkillTransactionTestConfig::default()
        },
    );
    assert!(crashed.is_err());

    let recovered = recover_skill_transactions_before_serving(clone.path().to_path_buf())
        .await
        .unwrap();
    assert_eq!(recovered.len(), 1);
    assert!(!clone
        .path()
        .join(".gitim/skill-transactions")
        .join(request_id.as_str())
        .exists());
    assert!(clone.path().join(".gitim/skill-validation.json").exists());
}

#[tokio::test]
async fn production_startup_refuses_to_serve_an_ambiguous_pushed_transaction() {
    let (_remote, clone) = remote_fixture();
    let prior_tip = String::from_utf8(git(clone.path(), ["rev-parse", "origin/main"]).stdout)
        .unwrap()
        .trim()
        .to_owned();
    let request_id = RequestId::generate();
    let repo = GitStorage::new(clone.path());
    let guard = SkillSyncGuard::new(clone.path()).unwrap();
    let crashed = execute_remote_skill_transaction_with_test_config(
        &repo,
        &guard,
        RemoteSkillTransactionRequest {
            request: SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
                request_id: request_id.clone(),
            }),
            actor: "alice".to_owned(),
            author_email: "alice@example.com".to_owned(),
            now: "2026-07-31T00:00:00Z".to_owned(),
            package: None,
        },
        SkillTransactionTestConfig {
            crash_after: Some(SkillTransactionCrashPoint::AfterPushed),
            ..SkillTransactionTestConfig::default()
        },
    );
    assert!(crashed.is_err());
    let refspec = format!("{prior_tip}:refs/heads/main");
    git(clone.path(), ["push", "--force", "origin", &refspec]);

    let error = recover_skill_transactions_before_serving(clone.path().to_path_buf())
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("pushed transaction has no authoritative receipt"),
        "unexpected startup recovery error: {error}"
    );
    assert!(clone
        .path()
        .join(".gitim/skill-transactions")
        .join(request_id.as_str())
        .exists());
}
