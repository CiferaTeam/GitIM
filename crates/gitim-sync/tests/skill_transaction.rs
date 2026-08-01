#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use fs2::FileExt;
use gitim_core::skill::{
    validate_package_entries, PackageEntry, RequestId, SkillCreateRequest, SkillMutationRequest,
    SkillOperation, SkillProposeRequest, SkillRepairRequest, SkillRepairScope,
    SkillRoleUpdateRequest, SkillSlug, SkillWorkspaceBootstrapRequest,
};
use gitim_core::types::Handler;
use gitim_sync::git::GitStorage;
use gitim_sync::skill::checkpoint::{
    validate_incoming_skill_history, SkillCheckpointStore, SkillConflict,
};
use gitim_sync::skill::guard::SkillSyncGuard;
use gitim_sync::skill::transaction::{
    execute_remote_skill_transaction, execute_remote_skill_transaction_with_test_config,
    recover_remote_skill_transactions, recover_remote_skill_transactions_with_test_config,
    skill_transaction_error_is_retryable, skill_transport_failure_count,
    RemoteSkillTransactionRequest, SkillLocalState, SkillTransactionCrashPoint,
    SkillTransactionTestConfig, SKILL_GIT_COMMAND_TIMEOUT, SKILL_GIT_MAX_CONCURRENCY,
    SKILL_TRANSACTION_TIMEOUT,
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

struct Fixture {
    _test_lock: MutexGuard<'static, ()>,
    _remote: TempDir,
    first: TempDir,
    second: TempDir,
}

impl Fixture {
    fn new() -> Self {
        // These fixtures spawn real Git processes and assert wall-clock deadlines.
        // Serialize fixture setup and execution so tests cannot consume each other's budgets.
        static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let test_lock = TEST_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        fs::write(seed.path().join("README.md"), "workspace\n").unwrap();
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

        let first = TempDir::new().unwrap();
        git(
            first.path(),
            [
                OsStr::new("clone"),
                remote.path().as_os_str(),
                OsStr::new("."),
            ],
        );
        configure(first.path());
        let second = TempDir::new().unwrap();
        git(
            second.path(),
            [
                OsStr::new("clone"),
                remote.path().as_os_str(),
                OsStr::new("."),
            ],
        );
        configure(second.path());

        Self {
            _test_lock: test_lock,
            _remote: remote,
            first,
            second,
        }
    }

    fn transaction(
        &self,
        root: &Path,
        request: SkillMutationRequest,
        package: Option<gitim_core::skill::ValidatedPackage>,
    ) -> Result<
        gitim_sync::skill::transaction::RemoteSkillTransactionResult,
        gitim_sync::skill::checkpoint::SkillSyncError,
    > {
        let repo = GitStorage::new(root);
        let guard = SkillSyncGuard::new(root).unwrap();
        execute_remote_skill_transaction(
            &repo,
            &guard,
            RemoteSkillTransactionRequest {
                request,
                actor: "alice".to_owned(),
                author_email: "alice@example.com".to_owned(),
                now: "2026-07-31T00:00:00Z".to_owned(),
                package,
            },
        )
    }

    fn bootstrap_and_create(&self, slug: &SkillSlug) -> String {
        self.transaction(
            self.first.path(),
            SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
                request_id: RequestId::generate(),
            }),
            None,
        )
        .unwrap();
        let created = self
            .transaction(
                self.first.path(),
                SkillMutationRequest::Create(SkillCreateRequest {
                    request_id: RequestId::generate(),
                    slug: slug.clone(),
                    display_name: "Race skill".to_owned(),
                    description: "Transaction race fixture".to_owned(),
                    source_directory: self.first.path().join("unused"),
                }),
                Some(package(slug, "initial")),
            )
            .unwrap();
        created.result.current_revision.unwrap().as_str().to_owned()
    }

    fn clone_remote(&self) -> TempDir {
        let clone = TempDir::new().unwrap();
        git(
            clone.path(),
            [
                OsStr::new("clone"),
                self._remote.path().as_os_str(),
                OsStr::new("."),
            ],
        );
        configure(clone.path());
        clone
    }
}

fn package(slug: &SkillSlug, body: &str) -> gitim_core::skill::ValidatedPackage {
    validate_package_entries(
        slug,
        vec![PackageEntry::new(
            "SKILL.md",
            format!(
                "---\nname: {}\ndescription: Test skill\n---\n\n{body}\n",
                slug.as_str()
            )
            .into_bytes(),
        )],
    )
    .unwrap()
}

#[test]
fn exports_transaction_resource_limits() {
    assert_eq!(SKILL_TRANSACTION_TIMEOUT.as_secs(), 180);
    assert_eq!(SKILL_GIT_COMMAND_TIMEOUT.as_secs(), 60);
    assert_eq!(SKILL_GIT_MAX_CONCURRENCY, 4);
}

#[test]
fn published_receipt_is_globally_idempotent_and_mismatched_reuse_is_rejected() {
    let fixture = Fixture::new();
    let request_id = RequestId::generate();
    let bootstrap = SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
        request_id: request_id.clone(),
    });

    let published = fixture
        .transaction(fixture.first.path(), bootstrap.clone(), None)
        .unwrap();
    assert_eq!(published.result.control_revision, Some(1));
    assert_eq!(published.local_state, SkillLocalState::PendingSync);

    let replayed = fixture
        .transaction(fixture.second.path(), bootstrap, None)
        .unwrap();
    assert_eq!(replayed.commit_id, published.commit_id);
    assert_eq!(replayed.result, published.result);

    let slug = SkillSlug::new("same-request").unwrap();
    let conflicting = SkillMutationRequest::Create(SkillCreateRequest {
        request_id,
        slug: slug.clone(),
        display_name: "Same request".to_owned(),
        description: "Conflicting reuse".to_owned(),
        source_directory: fixture.second.path().join("unused"),
    });
    let error = fixture
        .transaction(
            fixture.second.path(),
            conflicting,
            Some(package(&slug, "different mutation")),
        )
        .unwrap_err();
    assert_eq!(error.code(), "request_id_conflict");
}

#[test]
fn concurrent_identical_requests_serialize_one_checkout_journal_and_publication() {
    let fixture = Fixture::new();
    let request_id = RequestId::generate();
    let request = SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
        request_id: request_id.clone(),
    });
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let (first_ready_sender, first_ready_receiver) = std::sync::mpsc::sync_channel(1);
    let first_root = fixture.first.path().to_path_buf();
    let first_request = request.clone();
    let first_gate = Arc::clone(&gate);
    let first = std::thread::spawn(move || {
        let repo = GitStorage::new(&first_root);
        let guard = SkillSyncGuard::new(&first_root).unwrap();
        execute_remote_skill_transaction_with_test_config(
            &repo,
            &guard,
            transaction_request(first_request, None),
            SkillTransactionTestConfig {
                after_built: Some(Arc::new(move || {
                    first_ready_sender.send(()).unwrap();
                    let (lock, changed) = &*first_gate;
                    let mut released = lock.lock().unwrap();
                    while !*released {
                        released = changed.wait(released).unwrap();
                    }
                })),
                ..SkillTransactionTestConfig::default()
            },
        )
    });
    first_ready_receiver
        .recv_timeout(Duration::from_secs(10))
        .unwrap();

    let (second_progress_sender, second_progress_receiver) = std::sync::mpsc::sync_channel(1);
    let second_root = fixture.first.path().to_path_buf();
    let second = std::thread::spawn(move || {
        let repo = GitStorage::new(&second_root);
        let guard = SkillSyncGuard::new(&second_root).unwrap();
        execute_remote_skill_transaction_with_test_config(
            &repo,
            &guard,
            transaction_request(request, None),
            SkillTransactionTestConfig {
                after_built: Some(Arc::new(move || {
                    let _ = second_progress_sender.try_send(());
                })),
                ..SkillTransactionTestConfig::default()
            },
        )
    });
    let second_progressed_while_first_owned_journal = second_progress_receiver
        .recv_timeout(Duration::from_secs(1))
        .is_ok();

    let (lock, changed) = &*gate;
    *lock.lock().unwrap() = true;
    changed.notify_all();
    let first_result = first.join().unwrap();
    let second_result = second.join().unwrap();

    assert!(
        !second_progressed_while_first_owned_journal,
        "the second request mutated the shared journal before the first completed"
    );
    let first_result = first_result.unwrap();
    let second_result = second_result.unwrap();
    assert_eq!(second_result, first_result);
    git(fixture.first.path(), ["fetch", "origin"]);
    let receipt_path = format!("skills/receipts/{}.meta.yaml", request_id.as_str());
    let log = git(
        fixture.first.path(),
        ["log", "--format=%H", "origin/main", "--", &receipt_path],
    );
    assert_eq!(String::from_utf8(log.stdout).unwrap().lines().count(), 1);
    assert!(!fixture
        .first
        .path()
        .join(".gitim/skill-transactions")
        .join(request_id.as_str())
        .exists());
}

#[test]
fn concurrent_distinct_bootstraps_return_the_winning_workspace_state() {
    let fixture = Fixture::new();
    fs::write(
        fixture.first.path().join("users/bob.meta.yaml"),
        "display_name: Bob\nrole: human\nintroduction: Administrator candidate\n",
    )
    .unwrap();
    git(fixture.first.path(), ["add", "users/bob.meta.yaml"]);
    git(
        fixture.first.path(),
        ["commit", "-m", "add second bootstrap candidate"],
    );
    git(fixture.first.path(), ["push", "origin", "main"]);
    git(fixture.second.path(), ["pull", "--ff-only"]);
    let alice_request_id = RequestId::generate();
    let bob_request_id = RequestId::generate();
    let barrier = Arc::new(Barrier::new(2));

    let alice_root = fixture.first.path().to_path_buf();
    let alice_barrier = Arc::clone(&barrier);
    let alice_request_id_for_thread = alice_request_id.clone();
    let alice = std::thread::spawn(move || {
        let repo = GitStorage::new(&alice_root);
        let guard = SkillSyncGuard::new(&alice_root).unwrap();
        execute_remote_skill_transaction_with_test_config(
            &repo,
            &guard,
            RemoteSkillTransactionRequest {
                request: SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
                    request_id: alice_request_id_for_thread,
                }),
                actor: "alice".to_owned(),
                author_email: "alice@example.com".to_owned(),
                now: "2026-07-31T00:00:00Z".to_owned(),
                package: None,
            },
            SkillTransactionTestConfig {
                after_built: Some(Arc::new(move || {
                    alice_barrier.wait();
                })),
                ..SkillTransactionTestConfig::default()
            },
        )
    });

    let bob_root = fixture.second.path().to_path_buf();
    let bob_barrier = Arc::clone(&barrier);
    let bob_request_id_for_thread = bob_request_id.clone();
    let bob = std::thread::spawn(move || {
        let repo = GitStorage::new(&bob_root);
        let guard = SkillSyncGuard::new(&bob_root).unwrap();
        execute_remote_skill_transaction_with_test_config(
            &repo,
            &guard,
            RemoteSkillTransactionRequest {
                request: SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
                    request_id: bob_request_id_for_thread,
                }),
                actor: "bob".to_owned(),
                author_email: "bob@example.com".to_owned(),
                now: "2026-07-31T00:00:01Z".to_owned(),
                package: None,
            },
            SkillTransactionTestConfig {
                after_built: Some(Arc::new(move || {
                    bob_barrier.wait();
                })),
                ..SkillTransactionTestConfig::default()
            },
        )
    });

    let alice_result = alice.join().unwrap().unwrap();
    let bob_result = bob.join().unwrap().unwrap();
    assert_eq!(alice_result.commit_id, bob_result.commit_id);
    assert_eq!(alice_result.result.control_revision, Some(1));
    assert_eq!(bob_result.result.control_revision, Some(1));

    git(fixture.first.path(), ["fetch", "origin"]);
    let workspace = git(
        fixture.first.path(),
        ["show", "origin/main:skills/workspace.meta.yaml"],
    );
    let workspace: gitim_core::skill::WorkspaceSkillMeta =
        serde_yaml::from_slice(&workspace.stdout).unwrap();
    assert_eq!(workspace.administrators.len(), 1);
    assert!(
        workspace.administrators[0].as_str() == "alice"
            || workspace.administrators[0].as_str() == "bob"
    );

    let winning_request = if workspace.administrators[0].as_str() == "alice" {
        &alice_request_id
    } else {
        &bob_request_id
    };
    let losing_request = if workspace.administrators[0].as_str() == "alice" {
        &bob_request_id
    } else {
        &alice_request_id
    };
    let winning_receipt = git(
        fixture.first.path(),
        [
            "show",
            &format!(
                "origin/main:skills/receipts/{}.meta.yaml",
                winning_request.as_str()
            ),
        ],
    );
    assert!(!winning_receipt.stdout.is_empty());
    let losing_receipt = Command::new("git")
        .args([
            "show",
            &format!(
                "origin/main:skills/receipts/{}.meta.yaml",
                losing_request.as_str()
            ),
        ])
        .current_dir(fixture.first.path())
        .output()
        .unwrap();
    assert!(!losing_receipt.status.success());
}

#[test]
fn mismatched_request_reuse_waits_for_the_request_owner_then_rejects() {
    let fixture = Fixture::new();
    let request_id = RequestId::generate();
    let first_request = SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
        request_id: request_id.clone(),
    });
    let slug = SkillSlug::new("mismatched-contention").unwrap();
    let second_request = SkillMutationRequest::Create(SkillCreateRequest {
        request_id: request_id.clone(),
        slug: slug.clone(),
        display_name: "Mismatched contention".to_owned(),
        description: "Must retain request identity".to_owned(),
        source_directory: fixture.first.path().join("unused"),
    });
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let (first_ready_sender, first_ready_receiver) = std::sync::mpsc::sync_channel(1);
    let first_root = fixture.first.path().to_path_buf();
    let first_gate = Arc::clone(&gate);
    let first = std::thread::spawn(move || {
        let repo = GitStorage::new(&first_root);
        let guard = SkillSyncGuard::new(&first_root).unwrap();
        execute_remote_skill_transaction_with_test_config(
            &repo,
            &guard,
            transaction_request(first_request, None),
            SkillTransactionTestConfig {
                after_built: Some(Arc::new(move || {
                    first_ready_sender.send(()).unwrap();
                    let (lock, changed) = &*first_gate;
                    let mut released = lock.lock().unwrap();
                    while !*released {
                        released = changed.wait(released).unwrap();
                    }
                })),
                ..SkillTransactionTestConfig::default()
            },
        )
    });
    first_ready_receiver
        .recv_timeout(Duration::from_secs(10))
        .unwrap();

    let (second_result_sender, second_result_receiver) = std::sync::mpsc::sync_channel(1);
    let second_root = fixture.first.path().to_path_buf();
    let second = std::thread::spawn(move || {
        let repo = GitStorage::new(&second_root);
        let guard = SkillSyncGuard::new(&second_root).unwrap();
        let result = execute_remote_skill_transaction(
            &repo,
            &guard,
            transaction_request(second_request, Some(package(&slug, "mismatch"))),
        );
        second_result_sender.send(result).unwrap();
    });
    let second_returned_while_first_owned_journal = second_result_receiver
        .recv_timeout(Duration::from_secs(1))
        .ok();

    let (lock, changed) = &*gate;
    *lock.lock().unwrap() = true;
    changed.notify_all();
    assert!(first.join().unwrap().is_ok());
    let returned_while_first_owned_journal = second_returned_while_first_owned_journal.is_some();
    let second_result = match second_returned_while_first_owned_journal {
        Some(result) => result,
        None => second_result_receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap(),
    };
    second.join().unwrap();

    assert!(
        !returned_while_first_owned_journal,
        "the mismatched request inspected an in-flight journal"
    );
    assert_eq!(second_result.unwrap_err().code(), "request_id_conflict");
    assert!(!fixture
        .first
        .path()
        .join(".gitim/skill-transactions")
        .join(request_id.as_str())
        .exists());
}

#[test]
fn request_lock_contention_times_out_without_mutating_the_owner_journal() {
    let fixture = Fixture::new();
    let request_id = RequestId::generate();
    let request = SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
        request_id: request_id.clone(),
    });
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let (first_ready_sender, first_ready_receiver) = std::sync::mpsc::sync_channel(1);
    let first_root = fixture.first.path().to_path_buf();
    let first_request = request.clone();
    let first_gate = Arc::clone(&gate);
    let first = std::thread::spawn(move || {
        let repo = GitStorage::new(&first_root);
        let guard = SkillSyncGuard::new(&first_root).unwrap();
        execute_remote_skill_transaction_with_test_config(
            &repo,
            &guard,
            transaction_request(first_request, None),
            SkillTransactionTestConfig {
                after_built: Some(Arc::new(move || {
                    first_ready_sender.send(()).unwrap();
                    let (lock, changed) = &*first_gate;
                    let mut released = lock.lock().unwrap();
                    while !*released {
                        released = changed.wait(released).unwrap();
                    }
                })),
                ..SkillTransactionTestConfig::default()
            },
        )
    });
    first_ready_receiver
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    let journal_path = fixture
        .first
        .path()
        .join(".gitim/skill-transactions")
        .join(request_id.as_str())
        .join("transaction.yaml");
    let journal_before = fs::read(&journal_path).unwrap();

    let failures_before = skill_transport_failure_count();
    let repo = GitStorage::new(fixture.first.path());
    let guard = SkillSyncGuard::new(fixture.first.path()).unwrap();
    let second_result = execute_remote_skill_transaction_with_test_config(
        &repo,
        &guard,
        transaction_request(request, None),
        SkillTransactionTestConfig {
            transaction_timeout: Duration::from_millis(300),
            ..SkillTransactionTestConfig::default()
        },
    );
    let journal_after = fs::read(&journal_path);

    let (lock, changed) = &*gate;
    *lock.lock().unwrap() = true;
    changed.notify_all();
    let first_result = first.join().unwrap();

    let error = second_result.unwrap_err();
    assert!(skill_transaction_error_is_retryable(&error));
    assert!(skill_transport_failure_count() > failures_before);
    assert_eq!(journal_after.unwrap(), journal_before);
    assert!(first_result.is_ok());
}

fn transaction_request(
    request: SkillMutationRequest,
    package: Option<gitim_core::skill::ValidatedPackage>,
) -> RemoteSkillTransactionRequest {
    RemoteSkillTransactionRequest {
        request,
        actor: "alice".to_owned(),
        author_email: "alice@example.com".to_owned(),
        now: "2026-07-31T00:00:00Z".to_owned(),
        package,
    }
}

fn add_remote_user(root: &Path, handler: &str) {
    git(root, ["fetch", "origin"]);
    git(root, ["reset", "--hard", "origin/main"]);
    fs::write(
        root.join(format!("users/{handler}.meta.yaml")),
        format!("display_name: {handler}\nrole: agent\nintroduction: Collaborator\n"),
    )
    .unwrap();
    git(root, ["add", "users"]);
    git(root, ["commit", "-m", "add collaborator"]);
    git(root, ["push", "origin", "main"]);
}

fn remote_tip(root: &Path) -> String {
    git(root, ["fetch", "origin"]);
    String::from_utf8(git(root, ["rev-parse", "origin/main"]).stdout)
        .unwrap()
        .trim()
        .to_owned()
}

fn remote_tree_paths(root: &Path) -> String {
    String::from_utf8(git(root, ["ls-tree", "-r", "--name-only", "origin/main"]).stdout).unwrap()
}

#[test]
fn role_update_rejects_a_target_archived_after_the_clone_snapshot() {
    let fixture = Fixture::new();
    add_remote_user(fixture.second.path(), "bob");
    let slug = SkillSlug::new("remote-user-authority").unwrap();
    fixture.bootstrap_and_create(&slug);
    let checkpoint_before = SkillCheckpointStore::new(fixture.first.path())
        .unwrap()
        .load()
        .unwrap()
        .unwrap();

    git(fixture.second.path(), ["fetch", "origin"]);
    git(fixture.second.path(), ["reset", "--hard", "origin/main"]);
    fs::create_dir_all(fixture.second.path().join("archive/users")).unwrap();
    git(
        fixture.second.path(),
        ["mv", "users/bob.meta.yaml", "archive/users/bob.meta.yaml"],
    );
    git(
        fixture.second.path(),
        ["commit", "-m", "archive collaborator"],
    );
    git(fixture.second.path(), ["push", "origin", "main"]);
    let archived_tip = remote_tip(fixture.first.path());
    let request_id = RequestId::generate();

    let repo = GitStorage::new(fixture.first.path());
    let guard = SkillSyncGuard::new(fixture.first.path()).unwrap();
    let error = execute_remote_skill_transaction(
        &repo,
        &guard,
        transaction_request(
            SkillMutationRequest::RoleUpdate(SkillRoleUpdateRequest {
                request_id: request_id.clone(),
                slug,
                operation: SkillOperation::MaintainerAdd,
                target: Handler::new("bob").unwrap(),
                remove_maintainer: false,
                expected_control_revision: 1,
            }),
            None,
        ),
    )
    .unwrap_err();

    assert_eq!(error.code(), "skill_role_target_inactive");
    assert_eq!(remote_tip(fixture.first.path()), archived_tip);
    let receipt_path = format!("skills/receipts/{}.meta.yaml", request_id.as_str());
    assert!(!remote_tree_paths(fixture.first.path())
        .lines()
        .any(|path| path == receipt_path));
    let checkpoint = SkillCheckpointStore::new(fixture.first.path())
        .unwrap()
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint, checkpoint_before);
}

#[test]
fn mutation_rejects_an_unaccepted_remote_skill_commit_without_publication() {
    let fixture = Fixture::new();
    add_remote_user(fixture.second.path(), "bob");
    let slug = SkillSlug::new("remote-skill-authority").unwrap();
    fixture.bootstrap_and_create(&slug);
    let checkpoint_before = SkillCheckpointStore::new(fixture.first.path())
        .unwrap()
        .load()
        .unwrap()
        .unwrap();
    let accepted_workspace = checkpoint_before.workspace_tree.clone().unwrap();

    git(fixture.second.path(), ["fetch", "origin"]);
    git(fixture.second.path(), ["reset", "--hard", "origin/main"]);
    let workspace_path = fixture.second.path().join("skills/workspace.meta.yaml");
    let workspace = fs::read_to_string(&workspace_path).unwrap();
    fs::write(
        &workspace_path,
        workspace.replace(
            "administrators:\n- alice",
            "administrators:\n- alice\n- bob",
        ),
    )
    .unwrap();
    git(fixture.second.path(), ["add", "skills/workspace.meta.yaml"]);
    git(
        fixture.second.path(),
        ["commit", "-m", "inject unauthorized administrator"],
    );
    git(fixture.second.path(), ["push", "origin", "main"]);
    let rejected_tip = remote_tip(fixture.first.path());
    let request_id = RequestId::generate();

    let repo = GitStorage::new(fixture.first.path());
    let guard = SkillSyncGuard::new(fixture.first.path()).unwrap();
    let error = execute_remote_skill_transaction(
        &repo,
        &guard,
        transaction_request(
            SkillMutationRequest::RoleUpdate(SkillRoleUpdateRequest {
                request_id: request_id.clone(),
                slug,
                operation: SkillOperation::MaintainerAdd,
                target: Handler::new("bob").unwrap(),
                remove_maintainer: false,
                expected_control_revision: 1,
            }),
            None,
        ),
    )
    .unwrap_err();

    assert_eq!(error.code(), "skill_sync_conflict");
    assert_eq!(remote_tip(fixture.first.path()), rejected_tip);
    let receipt_path = format!("skills/receipts/{}.meta.yaml", request_id.as_str());
    assert!(!remote_tree_paths(fixture.first.path())
        .lines()
        .any(|path| path == receipt_path));
    let checkpoint = SkillCheckpointStore::new(fixture.first.path())
        .unwrap()
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint, checkpoint_before);
    assert_eq!(checkpoint.workspace_tree.unwrap(), accepted_workspace);
}

#[test]
fn same_skill_concurrent_proposals_do_not_semantically_retry() {
    let fixture = Fixture::new();
    let slug = SkillSlug::new("proposal-race").unwrap();
    let base = fixture.bootstrap_and_create(&slug);
    let barrier = Arc::new(Barrier::new(2));

    let run = |root: &Path, marker: &'static str| {
        let root = root.to_path_buf();
        let slug = slug.clone();
        let base = base.clone();
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            let repo = GitStorage::new(&root);
            let guard = SkillSyncGuard::new(&root).unwrap();
            execute_remote_skill_transaction_with_test_config(
                &repo,
                &guard,
                transaction_request(
                    SkillMutationRequest::Propose(SkillProposeRequest {
                        request_id: RequestId::generate(),
                        slug: slug.clone(),
                        base_revision: gitim_core::skill::RevisionId::new(&base).unwrap(),
                        summary: marker.to_owned(),
                        source_directory: root.join("unused"),
                    }),
                    Some(package(&slug, marker)),
                ),
                SkillTransactionTestConfig {
                    after_built: Some(Arc::new(move || {
                        barrier.wait();
                    })),
                    ..SkillTransactionTestConfig::default()
                },
            )
        })
    };

    let first = run(fixture.first.path(), "first proposal");
    let second = run(fixture.second.path(), "second proposal");
    let outcomes = [first.join().unwrap(), second.join().unwrap()];
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|result| result
                .as_ref()
                .is_err_and(|error| error.code() == "skill_sync_conflict"))
            .count(),
        1
    );
}

#[test]
fn unrelated_remote_change_rebuilds_the_same_proposal_on_the_new_tip() {
    let fixture = Fixture::new();
    let slug = SkillSlug::new("unrelated-retry").unwrap();
    let base = fixture.bootstrap_and_create(&slug);
    let request_id = RequestId::generate();
    let injected = Arc::new(AtomicBool::new(false));
    let writer = fixture.second.path().to_path_buf();
    let callback_flag = Arc::clone(&injected);
    let repo = GitStorage::new(fixture.first.path());
    let guard = SkillSyncGuard::new(fixture.first.path()).unwrap();
    let result = execute_remote_skill_transaction_with_test_config(
        &repo,
        &guard,
        transaction_request(
            SkillMutationRequest::Propose(SkillProposeRequest {
                request_id: request_id.clone(),
                slug: slug.clone(),
                base_revision: gitim_core::skill::RevisionId::new(&base).unwrap(),
                summary: "retry unchanged semantics".to_owned(),
                source_directory: fixture.first.path().join("unused"),
            }),
            Some(package(&slug, "candidate")),
        ),
        SkillTransactionTestConfig {
            after_built: Some(Arc::new(move || {
                if callback_flag.swap(true, Ordering::SeqCst) {
                    return;
                }
                git(&writer, ["fetch", "origin"]);
                git(&writer, ["reset", "--hard", "origin/main"]);
                fs::write(writer.join("README.md"), "unrelated message\n").unwrap();
                git(&writer, ["add", "README.md"]);
                git(&writer, ["commit", "-m", "chore: unrelated message"]);
                git(&writer, ["push", "origin", "main"]);
            })),
            ..SkillTransactionTestConfig::default()
        },
    )
    .unwrap();

    assert!(injected.load(Ordering::SeqCst));
    assert_eq!(result.result.event_revision, Some(2));
    let suffix = &request_id.as_str()[2..];
    git(fixture.first.path(), ["fetch", "origin"]);
    let proposal_path = format!("skills/{}/proposals/p-{suffix}", slug.as_str());
    let revision_path = format!("skills/{}/revisions/r-{suffix}", slug.as_str());
    let tree = String::from_utf8(
        git(
            fixture.first.path(),
            ["ls-tree", "-r", "--name-only", "origin/main"],
        )
        .stdout,
    )
    .unwrap();
    assert_eq!(
        tree.lines()
            .filter(|path| path.starts_with(&proposal_path))
            .count(),
        2
    );
    assert!(tree
        .lines()
        .any(|path| path == format!("{revision_path}/revision.meta.yaml")));
}

#[test]
fn epoch_rotation_between_attempts_retargets_the_active_branch() {
    let fixture = Fixture::new();
    let slug = SkillSlug::new("epoch-retry").unwrap();
    fixture.bootstrap_and_create(&slug);
    let rotated = Arc::new(AtomicBool::new(false));
    let writer = fixture.second.path().to_path_buf();
    let callback_flag = Arc::clone(&rotated);
    let archive = TempDir::new().unwrap();
    let archive_path = archive.path().to_path_buf();
    let request = SkillMutationRequest::Create(SkillCreateRequest {
        request_id: RequestId::generate(),
        slug: SkillSlug::new("after-rotation").unwrap(),
        display_name: "After rotation".to_owned(),
        description: "Targets the re-resolved epoch".to_owned(),
        source_directory: fixture.first.path().join("unused"),
    });
    let package_slug = SkillSlug::new("after-rotation").unwrap();
    let repo = GitStorage::new(fixture.first.path());
    let guard = SkillSyncGuard::new(fixture.first.path()).unwrap();
    let result = execute_remote_skill_transaction_with_test_config(
        &repo,
        &guard,
        transaction_request(request, Some(package(&package_slug, "rotated"))),
        SkillTransactionTestConfig {
            after_built: Some(Arc::new(move || {
                if callback_flag.swap(true, Ordering::SeqCst) {
                    return;
                }
                git(&writer, ["fetch", "origin"]);
                git(&writer, ["reset", "--hard", "origin/main"]);
                let storage = GitStorage::new(&writer);
                let outcome = gitim_sync::rotate::try_fire_rotation(
                    &storage,
                    &Mutex::new(()),
                    "main",
                    1,
                    &archive_path,
                    ("system", "system@gitim"),
                    "2026-07-31T00:00:01Z",
                )
                .unwrap();
                assert!(matches!(
                    outcome,
                    gitim_sync::rotate::RotationOutcome::Won { .. }
                ));
            })),
            ..SkillTransactionTestConfig::default()
        },
    )
    .unwrap();

    assert!(rotated.load(Ordering::SeqCst));
    assert!(result.result.current_revision.is_some());
    git(fixture.first.path(), ["fetch", "origin"]);
    let active = String::from_utf8(
        git(
            fixture.first.path(),
            ["show", "origin/main:gitim.epoch.yaml"],
        )
        .stdout,
    )
    .unwrap();
    assert!(active.contains("target_branch: main-epoch-2"));
    let receipt = format!(
        "origin/main-epoch-2:skills/receipts/{}.meta.yaml",
        result
            .result
            .current_revision
            .as_ref()
            .unwrap()
            .as_str()
            .replacen("r-", "q-", 1)
    );
    git(fixture.first.path(), ["show", &receipt]);
}

fn install_conflict(
    fixture: &Fixture,
    changed_path: &str,
    rewrite: impl FnOnce(String) -> String,
) -> (SkillCheckpointStore, SkillConflict) {
    git(fixture.second.path(), ["fetch", "origin"]);
    git(fixture.second.path(), ["reset", "--hard", "origin/main"]);
    let path = fixture.second.path().join(changed_path);
    let original = fs::read_to_string(&path).unwrap();
    fs::write(&path, rewrite(original)).unwrap();
    git(fixture.second.path(), ["add", "--", changed_path]);
    git(
        fixture.second.path(),
        ["commit", "-m", "inject invalid Skill state"],
    );
    git(fixture.second.path(), ["push", "origin", "main"]);

    git(fixture.first.path(), ["fetch", "origin"]);
    let repo = GitStorage::new(fixture.first.path());
    let store = SkillCheckpointStore::new(fixture.first.path()).unwrap();
    let previous = store.load().unwrap().unwrap();
    let tip = String::from_utf8(git(fixture.first.path(), ["rev-parse", "origin/main"]).stdout)
        .unwrap()
        .trim()
        .to_owned();
    let validation = validate_incoming_skill_history(&repo, &previous, &tip).unwrap();
    store.save(&validation.checkpoint).unwrap();
    let key = if changed_path == "skills/workspace.meta.yaml" {
        "$workspace"
    } else {
        changed_path.split('/').nth(1).unwrap()
    };
    (store, validation.checkpoint.conflicts[key].clone())
}

fn install_remote_conflict_checkpoint(root: &Path) -> SkillCheckpointStore {
    git(root, ["fetch", "origin"]);
    git(root, ["reset", "--hard", "origin/main"]);
    let repo = GitStorage::new(root);
    let store = SkillCheckpointStore::new(root).unwrap();
    let tip = String::from_utf8(git(root, ["rev-parse", "origin/main"]).stdout)
        .unwrap()
        .trim()
        .to_owned();
    let validation = validate_incoming_skill_history(
        &repo,
        &gitim_sync::skill::checkpoint::SkillValidationCheckpoint::empty("main"),
        &tip,
    )
    .unwrap();
    store.save(&validation.checkpoint).unwrap();
    store
}

fn assert_identical_repair_converges_from_built_journal(
    fixture: &Fixture,
    request: SkillMutationRequest,
    checkpoint_key: &str,
) {
    let request_id = request.request_id().clone();
    let first_store = SkillCheckpointStore::new(fixture.first.path()).unwrap();
    let second_store = install_remote_conflict_checkpoint(fixture.second.path());
    assert!(first_store
        .load()
        .unwrap()
        .unwrap()
        .conflicts
        .contains_key(checkpoint_key));
    assert!(second_store
        .load()
        .unwrap()
        .unwrap()
        .conflicts
        .contains_key(checkpoint_key));

    let winner_result = Arc::new(Mutex::new(None));
    let winner_result_for_hook = Arc::clone(&winner_result);
    let winner_root = fixture.second.path().to_path_buf();
    let winner_request = request.clone();
    let launched = Arc::new(AtomicBool::new(false));
    let launched_for_hook = Arc::clone(&launched);
    let loser_repo = GitStorage::new(fixture.first.path());
    let loser_guard = SkillSyncGuard::new(fixture.first.path()).unwrap();
    let loser = execute_remote_skill_transaction_with_test_config(
        &loser_repo,
        &loser_guard,
        transaction_request(request, None),
        SkillTransactionTestConfig {
            after_built: Some(Arc::new(move || {
                if launched_for_hook.swap(true, Ordering::SeqCst) {
                    return;
                }
                let winner_repo = GitStorage::new(&winner_root);
                let winner_guard = SkillSyncGuard::new(&winner_root).unwrap();
                let published = execute_remote_skill_transaction(
                    &winner_repo,
                    &winner_guard,
                    transaction_request(winner_request.clone(), None),
                )
                .unwrap();
                *winner_result_for_hook.lock().unwrap() = Some(published);
            })),
            ..SkillTransactionTestConfig::default()
        },
    )
    .unwrap();
    let winner = winner_result.lock().unwrap().take().unwrap();

    assert!(launched.load(Ordering::SeqCst));
    assert_eq!(loser.commit_id, winner.commit_id);
    assert_eq!(loser.result, winner.result);
    assert!(!fixture
        .first
        .path()
        .join(".gitim/skill-transactions")
        .join(request_id.as_str())
        .exists());
    git(fixture.first.path(), ["fetch", "origin"]);
    let receipt_path = format!("skills/receipts/{}.meta.yaml", request_id.as_str());
    let receipt_commits = String::from_utf8(
        git(
            fixture.first.path(),
            ["log", "--format=%H", "origin/main", "--", &receipt_path],
        )
        .stdout,
    )
    .unwrap();
    assert_eq!(receipt_commits.lines().count(), 1);
    let loser_checkpoint = first_store.load().unwrap().unwrap();
    assert_eq!(loser_checkpoint.last_scanned_tip, winner.commit_id);
    assert!(!loser_checkpoint.conflicts.contains_key(checkpoint_key));
}

#[test]
fn concurrent_identical_workspace_repairs_resolve_the_global_receipt_first() {
    let fixture = Fixture::new();
    fixture.bootstrap_and_create(&SkillSlug::new("workspace-duplicate-repair").unwrap());
    let (_store, conflict) = install_conflict(&fixture, "skills/workspace.meta.yaml", |value| {
        value.replace("administrators:\n- alice", "administrators: []")
    });
    let request = SkillMutationRequest::Repair(SkillRepairRequest {
        request_id: RequestId::generate(),
        scope: SkillRepairScope::Workspace,
        conflict_tip: conflict.rejected_commit,
        accepted_tree: conflict.accepted_tree_oid.unwrap(),
    });

    assert_identical_repair_converges_from_built_journal(&fixture, request, "$workspace");
}

#[test]
fn concurrent_identical_skill_repairs_resolve_the_global_receipt_first() {
    let fixture = Fixture::new();
    let slug = SkillSlug::new("skill-duplicate-repair").unwrap();
    fixture.bootstrap_and_create(&slug);
    let changed_path = format!("skills/{}/skill.meta.yaml", slug.as_str());
    let (_store, conflict) = install_conflict(&fixture, &changed_path, |value| {
        value.replace("display_name: Race skill", "display_name: ''")
    });
    let request = SkillMutationRequest::Repair(SkillRepairRequest {
        request_id: RequestId::generate(),
        scope: SkillRepairScope::Skill(slug.clone()),
        conflict_tip: conflict.rejected_commit,
        accepted_tree: conflict.accepted_tree_oid.unwrap(),
    });

    assert_identical_repair_converges_from_built_journal(&fixture, request, slug.as_str());
}

#[test]
fn skill_and_workspace_repairs_restore_the_checkpoint_accepted_tree() {
    for workspace_scope in [false, true] {
        let fixture = Fixture::new();
        let slug = SkillSlug::new("repair-scope").unwrap();
        fixture.bootstrap_and_create(&slug);
        let (changed_path, scope, rewrite): (String, SkillRepairScope, fn(String) -> String) =
            if workspace_scope {
                (
                    "skills/workspace.meta.yaml".to_owned(),
                    SkillRepairScope::Workspace,
                    |value| value.replace("administrators:\n- alice", "administrators: []"),
                )
            } else {
                (
                    format!("skills/{}/skill.meta.yaml", slug.as_str()),
                    SkillRepairScope::Skill(slug.clone()),
                    |_| "not: [valid\n".to_owned(),
                )
            };
        let (_store, conflict) = install_conflict(&fixture, &changed_path, rewrite);
        let rejected_commit = conflict.rejected_commit.clone();
        let accepted_tree = conflict.accepted_tree_oid.clone().unwrap();
        let repair_id = RequestId::generate();
        let repaired = fixture
            .transaction(
                fixture.first.path(),
                SkillMutationRequest::Repair(SkillRepairRequest {
                    request_id: repair_id.clone(),
                    scope,
                    conflict_tip: conflict.rejected_commit,
                    accepted_tree: accepted_tree.clone(),
                }),
                None,
            )
            .unwrap_or_else(|error| {
                let journal = fixture
                    .first
                    .path()
                    .join(".gitim/skill-transactions")
                    .join(repair_id.as_str())
                    .join("transaction.yaml");
                panic!(
                    "workspace_scope={workspace_scope}: {error:?}; journal={}",
                    fs::read_to_string(journal).unwrap_or_default()
                )
            });
        assert!(!repaired.commit_id.is_empty());
        git(fixture.first.path(), ["fetch", "origin"]);
        let repaired_scope = if workspace_scope {
            changed_path.clone()
        } else {
            format!("skills/{}", slug.as_str())
        };
        let repaired_tree = String::from_utf8(
            git(
                fixture.first.path(),
                ["rev-parse", &format!("origin/main:{repaired_scope}")],
            )
            .stdout,
        )
        .unwrap()
        .trim()
        .to_owned();
        assert_eq!(repaired_tree, accepted_tree);
        let receipt = format!(
            "origin/main:skills/receipts/{}.meta.yaml",
            repair_id.as_str()
        );
        git(fixture.first.path(), ["show", &receipt]);
        git(
            fixture.first.path(),
            [
                "merge-base",
                "--is-ancestor",
                &rejected_commit,
                "origin/main",
            ],
        );
        let checkpoint = SkillCheckpointStore::new(fixture.first.path())
            .unwrap()
            .load()
            .unwrap()
            .unwrap();
        let checkpoint_key = if workspace_scope {
            "$workspace"
        } else {
            slug.as_str()
        };
        assert!(!checkpoint.conflicts.contains_key(checkpoint_key));
        assert_eq!(checkpoint.last_scanned_tip, repaired.commit_id);
        let resumed = validate_incoming_skill_history(
            &GitStorage::new(fixture.first.path()),
            &checkpoint,
            "origin/main",
        )
        .unwrap();
        assert!(resumed.checkpoint.conflicts.is_empty());
    }
}

#[test]
fn repair_rejects_non_admin_absent_conflict_and_checkpoint_mismatch_without_receipts() {
    let fixture = Fixture::new();
    add_remote_user(fixture.second.path(), "bob");
    fixture.bootstrap_and_create(&SkillSlug::new("repair-admission").unwrap());
    let store = SkillCheckpointStore::new(fixture.first.path()).unwrap();
    let accepted = store.load().unwrap().unwrap().workspace_tree.unwrap();
    let repo = GitStorage::new(fixture.first.path());
    let guard = SkillSyncGuard::new(fixture.first.path()).unwrap();

    let absent_id = RequestId::generate();
    let absent = execute_remote_skill_transaction(
        &repo,
        &guard,
        transaction_request(
            SkillMutationRequest::Repair(SkillRepairRequest {
                request_id: absent_id.clone(),
                scope: SkillRepairScope::Workspace,
                conflict_tip: accepted.commit_oid.clone(),
                accepted_tree: accepted.tree_oid.clone(),
            }),
            None,
        ),
    )
    .unwrap_err();
    assert_eq!(absent.code(), "skill_sync_conflict");

    let (_store, conflict) = install_conflict(&fixture, "skills/workspace.meta.yaml", |value| {
        value.replace("administrators:\n- alice", "administrators: []")
    });
    let mismatch_id = RequestId::generate();
    let mismatch = execute_remote_skill_transaction(
        &repo,
        &guard,
        transaction_request(
            SkillMutationRequest::Repair(SkillRepairRequest {
                request_id: mismatch_id.clone(),
                scope: SkillRepairScope::Workspace,
                conflict_tip: accepted.commit_oid,
                accepted_tree: conflict.accepted_tree_oid.clone().unwrap(),
            }),
            None,
        ),
    )
    .unwrap_err();
    assert_eq!(mismatch.code(), "skill_sync_conflict");

    let non_admin_id = RequestId::generate();
    let non_admin = execute_remote_skill_transaction(
        &repo,
        &guard,
        RemoteSkillTransactionRequest {
            request: SkillMutationRequest::Repair(SkillRepairRequest {
                request_id: non_admin_id.clone(),
                scope: SkillRepairScope::Workspace,
                conflict_tip: conflict.rejected_commit,
                accepted_tree: conflict.accepted_tree_oid.unwrap(),
            }),
            actor: "bob".to_owned(),
            author_email: "bob@example.com".to_owned(),
            now: "2026-07-31T00:00:00Z".to_owned(),
            package: None,
        },
    )
    .unwrap_err();
    assert_eq!(non_admin.code(), "skill_admin_required");

    git(fixture.first.path(), ["fetch", "origin"]);
    for request_id in [absent_id, mismatch_id, non_admin_id] {
        let receipt = format!(
            "origin/main:skills/receipts/{}.meta.yaml",
            request_id.as_str()
        );
        let output = Command::new("git")
            .args(["show", &receipt])
            .current_dir(fixture.first.path())
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "unexpected receipt for {}",
            request_id.as_str()
        );
    }
}

#[test]
fn workspace_repair_rejects_a_checkpoint_changed_after_candidate_build() {
    let fixture = Fixture::new();
    let slug = SkillSlug::new("repair-race").unwrap();
    fixture.bootstrap_and_create(&slug);
    let changed_path = "skills/workspace.meta.yaml";
    let (store, conflict) = install_conflict(&fixture, changed_path, |value| {
        value.replace("administrators:\n- alice", "administrators: []")
    });
    let request_id = RequestId::generate();
    let request = SkillMutationRequest::Repair(SkillRepairRequest {
        request_id: request_id.clone(),
        scope: SkillRepairScope::Workspace,
        conflict_tip: conflict.rejected_commit,
        accepted_tree: conflict.accepted_tree_oid.unwrap(),
    });
    let repo = GitStorage::new(fixture.first.path());
    let guard = SkillSyncGuard::new(fixture.first.path()).unwrap();
    let changed = Arc::new(AtomicBool::new(false));
    let changed_flag = Arc::clone(&changed);
    let error = execute_remote_skill_transaction_with_test_config(
        &repo,
        &guard,
        transaction_request(request, None),
        SkillTransactionTestConfig {
            after_built: Some(Arc::new(move || {
                if changed_flag.swap(true, Ordering::SeqCst) {
                    return;
                }
                let mut checkpoint = store.load().unwrap().unwrap();
                let replacement = checkpoint
                    .workspace_tree
                    .as_ref()
                    .unwrap()
                    .commit_oid
                    .clone();
                checkpoint
                    .conflicts
                    .get_mut("$workspace")
                    .unwrap()
                    .rejected_commit = replacement;
                store.save(&checkpoint).unwrap();
            })),
            ..SkillTransactionTestConfig::default()
        },
    )
    .unwrap_err();
    assert!(changed.load(Ordering::SeqCst));
    assert_eq!(error.code(), "skill_sync_conflict");
    git(fixture.first.path(), ["fetch", "origin"]);
    let receipt = format!(
        "origin/main:skills/receipts/{}.meta.yaml",
        request_id.as_str()
    );
    let output = Command::new("git")
        .args(["show", &receipt])
        .current_dir(fixture.first.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[test]
fn workspace_repair_rejects_a_checkpoint_cleared_after_candidate_build() {
    let fixture = Fixture::new();
    let slug = SkillSlug::new("repair-clear-race").unwrap();
    fixture.bootstrap_and_create(&slug);
    let (store, conflict) = install_conflict(&fixture, "skills/workspace.meta.yaml", |value| {
        value.replace("administrators:\n- alice", "administrators: []")
    });
    let request_id = RequestId::generate();
    let request = SkillMutationRequest::Repair(SkillRepairRequest {
        request_id: request_id.clone(),
        scope: SkillRepairScope::Workspace,
        conflict_tip: conflict.rejected_commit,
        accepted_tree: conflict.accepted_tree_oid.unwrap(),
    });
    let repo = GitStorage::new(fixture.first.path());
    let guard = SkillSyncGuard::new(fixture.first.path()).unwrap();
    let cleared = Arc::new(AtomicBool::new(false));
    let cleared_flag = Arc::clone(&cleared);
    let error = execute_remote_skill_transaction_with_test_config(
        &repo,
        &guard,
        transaction_request(request, None),
        SkillTransactionTestConfig {
            after_built: Some(Arc::new(move || {
                if !cleared_flag.swap(true, Ordering::SeqCst) {
                    fs::remove_file(&store.path).unwrap();
                }
            })),
            ..SkillTransactionTestConfig::default()
        },
    )
    .unwrap_err();
    assert!(cleared.load(Ordering::SeqCst));
    assert_eq!(error.code(), "skill_sync_conflict");

    git(fixture.first.path(), ["fetch", "origin"]);
    let receipt = format!(
        "origin/main:skills/receipts/{}.meta.yaml",
        request_id.as_str()
    );
    let output = Command::new("git")
        .args(["show", &receipt])
        .current_dir(fixture.first.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[test]
fn workspace_repair_uses_one_checkpoint_snapshot_per_attempt() {
    let fixture = Fixture::new();
    let slug = SkillSlug::new("repair-snapshot-race").unwrap();
    fixture.bootstrap_and_create(&slug);
    let (store, conflict) = install_conflict(&fixture, "skills/workspace.meta.yaml", |value| {
        value.replace("administrators:\n- alice", "administrators: []")
    });
    let request_id = RequestId::generate();
    let request = SkillMutationRequest::Repair(SkillRepairRequest {
        request_id: request_id.clone(),
        scope: SkillRepairScope::Workspace,
        conflict_tip: conflict.rejected_commit,
        accepted_tree: conflict.accepted_tree_oid.unwrap(),
    });
    let repo = GitStorage::new(fixture.first.path());
    let guard = SkillSyncGuard::new(fixture.first.path()).unwrap();
    let changed = Arc::new(AtomicBool::new(false));
    let changed_flag = Arc::clone(&changed);
    let error = execute_remote_skill_transaction_with_test_config(
        &repo,
        &guard,
        transaction_request(request, None),
        SkillTransactionTestConfig {
            after_repair_snapshot: Some(Arc::new(move || {
                if changed_flag.swap(true, Ordering::SeqCst) {
                    return;
                }
                let mut checkpoint = store.load().unwrap().unwrap();
                checkpoint
                    .conflicts
                    .get_mut("$workspace")
                    .unwrap()
                    .rejected_commit = checkpoint
                    .workspace_tree
                    .as_ref()
                    .unwrap()
                    .commit_oid
                    .clone();
                store.save(&checkpoint).unwrap();
            })),
            ..SkillTransactionTestConfig::default()
        },
    )
    .unwrap_err();
    assert!(changed.load(Ordering::SeqCst));
    assert_eq!(error.code(), "skill_sync_conflict");
    git(fixture.first.path(), ["fetch", "origin"]);
    let receipt = format!(
        "origin/main:skills/receipts/{}.meta.yaml",
        request_id.as_str()
    );
    let output = Command::new("git")
        .args(["show", &receipt])
        .current_dir(fixture.first.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[test]
fn concurrent_repairs_serialize_final_checkpoint_authority_through_publication() {
    let fixture = Fixture::new();
    let slug = SkillSlug::new("repair-final-cas").unwrap();
    fixture.bootstrap_and_create(&slug);
    let (_store, conflict) = install_conflict(&fixture, "skills/workspace.meta.yaml", |value| {
        value.replace("administrators:\n- alice", "administrators: []")
    });
    let first_id = RequestId::generate();
    let second_id = RequestId::generate();
    let first_request = SkillMutationRequest::Repair(SkillRepairRequest {
        request_id: first_id.clone(),
        scope: SkillRepairScope::Workspace,
        conflict_tip: conflict.rejected_commit.clone(),
        accepted_tree: conflict.accepted_tree_oid.clone().unwrap(),
    });
    let second_request = SkillMutationRequest::Repair(SkillRepairRequest {
        request_id: second_id.clone(),
        scope: SkillRepairScope::Workspace,
        conflict_tip: conflict.rejected_commit,
        accepted_tree: conflict.accepted_tree_oid.unwrap(),
    });
    let (waiting_sender, waiting_receiver) = std::sync::mpsc::sync_channel(1);
    let waiting_receiver = Arc::new(Mutex::new(waiting_receiver));
    let second_progressed_in_pause = Arc::new(AtomicBool::new(false));
    let second_handle = Arc::new(Mutex::new(None));
    let launched = Arc::new(AtomicBool::new(false));

    let second_root = fixture.first.path().to_path_buf();
    let second_progressed_in_pause_for_hook = Arc::clone(&second_progressed_in_pause);
    let second_handle_for_hook = Arc::clone(&second_handle);
    let waiting_receiver_for_hook = Arc::clone(&waiting_receiver);
    let launched_for_hook = Arc::clone(&launched);
    let first_repo = GitStorage::new(fixture.first.path());
    let first_guard = SkillSyncGuard::new(fixture.first.path()).unwrap();
    let first_result = execute_remote_skill_transaction_with_test_config(
        &first_repo,
        &first_guard,
        transaction_request(first_request, None),
        SkillTransactionTestConfig {
            after_repair_compare: Some(Arc::new(move || {
                if launched_for_hook.swap(true, Ordering::SeqCst) {
                    return;
                }
                let root = second_root.clone();
                let request = second_request.clone();
                let ready = waiting_sender.clone();
                let handle = std::thread::spawn(move || {
                    let repo = GitStorage::new(&root);
                    let guard = SkillSyncGuard::new(&root).unwrap();
                    execute_remote_skill_transaction_with_test_config(
                        &repo,
                        &guard,
                        transaction_request(request, None),
                        SkillTransactionTestConfig {
                            before_repair_checkpoint_load: Some(Arc::new(move || {
                                let _ = ready.try_send(());
                            })),
                            ..SkillTransactionTestConfig::default()
                        },
                    )
                });
                *second_handle_for_hook.lock().unwrap() = Some(handle);
                second_progressed_in_pause_for_hook.store(
                    waiting_receiver_for_hook
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_millis(250))
                        .is_ok(),
                    Ordering::SeqCst,
                );
            })),
            ..SkillTransactionTestConfig::default()
        },
    )
    .unwrap();
    let second_result = second_handle
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .join()
        .unwrap()
        .unwrap_err();

    assert!(!second_progressed_in_pause.load(Ordering::SeqCst));
    assert_eq!(second_result.code(), "skill_sync_conflict");
    git(fixture.first.path(), ["fetch", "origin"]);
    let first_receipt = format!(
        "origin/main:skills/receipts/{}.meta.yaml",
        first_id.as_str()
    );
    assert!(Command::new("git")
        .args(["show", &first_receipt])
        .current_dir(fixture.first.path())
        .output()
        .unwrap()
        .status
        .success());
    let second_receipt = format!(
        "origin/main:skills/receipts/{}.meta.yaml",
        second_id.as_str()
    );
    assert!(!Command::new("git")
        .args(["show", &second_receipt])
        .current_dir(fixture.first.path())
        .output()
        .unwrap()
        .status
        .success());
    let checkpoint = SkillCheckpointStore::new(fixture.first.path())
        .unwrap()
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.last_scanned_tip, first_result.commit_id);
    assert!(checkpoint.conflicts.is_empty());
}

#[test]
fn skill_repair_rejects_when_another_clone_restores_the_remote_tree_first() {
    let fixture = Fixture::new();
    let slug = SkillSlug::new("remote-repair-race").unwrap();
    fixture.bootstrap_and_create(&slug);
    let changed_path = format!("skills/{}/skill.meta.yaml", slug.as_str());
    let (_first_store, conflict) = install_conflict(&fixture, &changed_path, |value| {
        value.replace("display_name: Race skill", "display_name: ''")
    });

    let second_repo = GitStorage::new(fixture.second.path());
    let second_store = SkillCheckpointStore::new(fixture.second.path()).unwrap();
    let tip = String::from_utf8(git(fixture.second.path(), ["rev-parse", "origin/main"]).stdout)
        .unwrap()
        .trim()
        .to_owned();
    let second_validation = validate_incoming_skill_history(
        &second_repo,
        &gitim_sync::skill::checkpoint::SkillValidationCheckpoint::empty("main"),
        &tip,
    )
    .unwrap();
    second_store.save(&second_validation.checkpoint).unwrap();

    let first_request = SkillMutationRequest::Repair(SkillRepairRequest {
        request_id: RequestId::generate(),
        scope: SkillRepairScope::Skill(slug.clone()),
        conflict_tip: conflict.rejected_commit.clone(),
        accepted_tree: conflict.accepted_tree_oid.clone().unwrap(),
    });
    let second_request = SkillMutationRequest::Repair(SkillRepairRequest {
        request_id: RequestId::generate(),
        scope: SkillRepairScope::Skill(slug),
        conflict_tip: conflict.rejected_commit,
        accepted_tree: conflict.accepted_tree_oid.unwrap(),
    });
    let second_root = fixture.second.path().to_path_buf();
    let ran = Arc::new(AtomicBool::new(false));
    let ran_flag = Arc::clone(&ran);
    let first_repo = GitStorage::new(fixture.first.path());
    let first_guard = SkillSyncGuard::new(fixture.first.path()).unwrap();
    let error = execute_remote_skill_transaction_with_test_config(
        &first_repo,
        &first_guard,
        transaction_request(first_request, None),
        SkillTransactionTestConfig {
            after_built: Some(Arc::new(move || {
                if ran_flag.swap(true, Ordering::SeqCst) {
                    return;
                }
                let repo = GitStorage::new(&second_root);
                let guard = SkillSyncGuard::new(&second_root).unwrap();
                execute_remote_skill_transaction(
                    &repo,
                    &guard,
                    transaction_request(second_request.clone(), None),
                )
                .unwrap();
            })),
            ..SkillTransactionTestConfig::default()
        },
    )
    .unwrap_err();
    assert!(ran.load(Ordering::SeqCst));
    assert_eq!(error.code(), "skill_sync_conflict");
}

#[test]
fn crash_journal_recovers_prepared_built_and_pushed_transactions() {
    for phase in [
        SkillTransactionCrashPoint::AfterPrepared,
        SkillTransactionCrashPoint::AfterBuilt,
        SkillTransactionCrashPoint::AfterPushed,
    ] {
        let fixture = Fixture::new();
        let request_id = RequestId::generate();
        let request = SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
            request_id: request_id.clone(),
        });
        let repo = GitStorage::new(fixture.first.path());
        let guard = SkillSyncGuard::new(fixture.first.path()).unwrap();
        let crashed = execute_remote_skill_transaction_with_test_config(
            &repo,
            &guard,
            transaction_request(request.clone(), None),
            SkillTransactionTestConfig {
                crash_after: Some(phase),
                ..SkillTransactionTestConfig::default()
            },
        );
        assert!(crashed
            .unwrap_err()
            .to_string()
            .contains("injected transaction crash"));
        let journal = fixture
            .first
            .path()
            .join(".gitim/skill-transactions")
            .join(request_id.as_str())
            .join("transaction.yaml");
        assert!(journal.exists());

        let recovered = fixture
            .transaction(fixture.first.path(), request, None)
            .unwrap();
        assert_eq!(recovered.result.control_revision, Some(1));
        assert!(!journal.exists());
    }
}

#[test]
fn built_journal_accepts_an_authoritative_identical_duplicate_from_another_parent() {
    let fixture = Fixture::new();
    let request_id = RequestId::generate();
    let request = SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
        request_id: request_id.clone(),
    });
    let first_repo = GitStorage::new(fixture.first.path());
    let first_guard = SkillSyncGuard::new(fixture.first.path()).unwrap();
    execute_remote_skill_transaction_with_test_config(
        &first_repo,
        &first_guard,
        transaction_request(request.clone(), None),
        SkillTransactionTestConfig {
            crash_after: Some(SkillTransactionCrashPoint::AfterBuilt),
            ..SkillTransactionTestConfig::default()
        },
    )
    .unwrap_err();

    fs::write(
        fixture.second.path().join("unrelated.txt"),
        "different parent\n",
    )
    .unwrap();
    git(fixture.second.path(), ["add", "unrelated.txt"]);
    git(
        fixture.second.path(),
        ["commit", "-m", "test: advance transaction parent"],
    );
    git(fixture.second.path(), ["push", "origin", "main"]);
    let published = fixture
        .transaction(fixture.second.path(), request, None)
        .unwrap();

    let journal_root = fixture
        .first
        .path()
        .join(".gitim/skill-transactions")
        .join(request_id.as_str());
    let journal = fs::read_to_string(journal_root.join("transaction.yaml")).unwrap();
    assert!(
        !journal.contains(&published.commit_id),
        "fixture did not produce a distinct local candidate"
    );

    let recovered = recover_remote_skill_transactions(&first_repo, &first_guard).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0], published);
    assert!(!journal_root.exists());
    let checkpoint = SkillCheckpointStore::new(fixture.first.path())
        .unwrap()
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.last_scanned_tip, published.commit_id);
}

#[test]
fn built_journal_rejects_an_authoritative_mismatched_request_reuse() {
    let fixture = Fixture::new();
    fixture
        .transaction(
            fixture.first.path(),
            SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
                request_id: RequestId::generate(),
            }),
            None,
        )
        .unwrap();
    git(fixture.second.path(), ["fetch", "origin"]);
    git(fixture.second.path(), ["reset", "--hard", "origin/main"]);

    let request_id = RequestId::generate();
    let local_slug = SkillSlug::new("local-built").unwrap();
    let local_request = SkillMutationRequest::Create(SkillCreateRequest {
        request_id: request_id.clone(),
        slug: local_slug.clone(),
        display_name: "Local built".to_owned(),
        description: "Local recovery identity".to_owned(),
        source_directory: fixture.first.path().join("unused"),
    });
    let first_repo = GitStorage::new(fixture.first.path());
    let first_guard = SkillSyncGuard::new(fixture.first.path()).unwrap();
    execute_remote_skill_transaction_with_test_config(
        &first_repo,
        &first_guard,
        transaction_request(local_request, Some(package(&local_slug, "local candidate"))),
        SkillTransactionTestConfig {
            crash_after: Some(SkillTransactionCrashPoint::AfterBuilt),
            ..SkillTransactionTestConfig::default()
        },
    )
    .unwrap_err();

    let remote_slug = SkillSlug::new("remote-published").unwrap();
    fixture
        .transaction(
            fixture.second.path(),
            SkillMutationRequest::Create(SkillCreateRequest {
                request_id: request_id.clone(),
                slug: remote_slug.clone(),
                display_name: "Remote published".to_owned(),
                description: "Remote recovery identity".to_owned(),
                source_directory: fixture.second.path().join("unused"),
            }),
            Some(package(&remote_slug, "remote candidate")),
        )
        .unwrap();

    let error = recover_remote_skill_transactions(&first_repo, &first_guard).unwrap_err();
    assert_eq!(error.code(), "request_id_conflict");
    assert!(fixture
        .first
        .path()
        .join(".gitim/skill-transactions")
        .join(request_id.as_str())
        .join("transaction.yaml")
        .exists());
}

#[test]
fn startup_recovery_enumerates_published_and_unpublished_journals() {
    let fixture = Fixture::new();
    let repo = GitStorage::new(fixture.first.path());
    let guard = SkillSyncGuard::new(fixture.first.path()).unwrap();
    let published_id = RequestId::generate();
    let published = SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
        request_id: published_id.clone(),
    });
    let error = execute_remote_skill_transaction_with_test_config(
        &repo,
        &guard,
        transaction_request(published, None),
        SkillTransactionTestConfig {
            crash_after: Some(SkillTransactionCrashPoint::AfterPushed),
            ..SkillTransactionTestConfig::default()
        },
    )
    .unwrap_err();
    assert!(matches!(
        error,
        gitim_sync::skill::checkpoint::SkillSyncError::Checkpoint(_)
    ));

    let slug = SkillSlug::new("startup-recovery").unwrap();
    let unpublished_id = RequestId::generate();
    let unpublished = SkillMutationRequest::Create(SkillCreateRequest {
        request_id: unpublished_id.clone(),
        slug: slug.clone(),
        display_name: "Startup recovery".to_owned(),
        description: "Prepared candidate cleanup".to_owned(),
        source_directory: fixture.first.path().join("unused"),
    });
    let error = execute_remote_skill_transaction_with_test_config(
        &repo,
        &guard,
        transaction_request(unpublished, Some(package(&slug, "candidate"))),
        SkillTransactionTestConfig {
            crash_after: Some(SkillTransactionCrashPoint::AfterBuilt),
            ..SkillTransactionTestConfig::default()
        },
    )
    .unwrap_err();
    assert!(matches!(
        error,
        gitim_sync::skill::checkpoint::SkillSyncError::Checkpoint(_)
    ));

    let transaction_root = fixture.first.path().join(".gitim/skill-transactions");
    assert!(transaction_root.join(published_id.as_str()).exists());
    assert!(transaction_root.join(unpublished_id.as_str()).exists());
    assert!(!fixture
        .first
        .path()
        .join(".gitim/skill-validation.json")
        .exists());

    let recovered = recover_remote_skill_transactions(&repo, &guard).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].result.control_revision, Some(1));
    assert!(!transaction_root.join(published_id.as_str()).exists());
    assert!(!transaction_root.join(unpublished_id.as_str()).exists());
    let checkpoint = SkillCheckpointStore::new(fixture.first.path())
        .unwrap()
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.last_scanned_tip, recovered[0].commit_id);
    git(fixture.first.path(), ["fetch", "origin"]);
    let tree = String::from_utf8(
        git(
            fixture.first.path(),
            ["ls-tree", "-r", "--name-only", "origin/main"],
        )
        .stdout,
    )
    .unwrap();
    assert!(!tree
        .lines()
        .any(|path| path.starts_with(&format!("skills/{}/", slug.as_str()))));
}

#[test]
fn startup_recovery_discards_completed_journal_with_missing_source_residue() {
    let fixture = Fixture::new();
    let repo = GitStorage::new(fixture.first.path());
    let guard = SkillSyncGuard::new(fixture.first.path()).unwrap();
    execute_remote_skill_transaction(
        &repo,
        &guard,
        transaction_request(
            SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
                request_id: RequestId::generate(),
            }),
            None,
        ),
    )
    .unwrap();
    let request_id = RequestId::generate();
    let slug = SkillSlug::new("completed-residue").unwrap();
    let request = SkillMutationRequest::Create(SkillCreateRequest {
        request_id: request_id.clone(),
        slug: slug.clone(),
        display_name: "Completed residue".to_owned(),
        description: "Startup cleanup".to_owned(),
        source_directory: fixture.first.path().join("unused"),
    });
    execute_remote_skill_transaction_with_test_config(
        &repo,
        &guard,
        transaction_request(request.clone(), Some(package(&slug, "completed"))),
        SkillTransactionTestConfig {
            crash_after: Some(SkillTransactionCrashPoint::AfterPushed),
            ..SkillTransactionTestConfig::default()
        },
    )
    .unwrap_err();
    let transaction_root = fixture
        .first
        .path()
        .join(".gitim/skill-transactions")
        .join(request_id.as_str());
    let pushed_journal = fs::read_to_string(transaction_root.join("transaction.yaml")).unwrap();
    execute_remote_skill_transaction(
        &repo,
        &guard,
        transaction_request(request, Some(package(&slug, "completed"))),
    )
    .unwrap();
    assert!(!transaction_root.exists());
    assert!(fixture
        .first
        .path()
        .join(".gitim/skill-validation.json")
        .exists());

    fs::create_dir_all(&transaction_root).unwrap();
    fs::write(
        transaction_root.join("transaction.yaml"),
        pushed_journal.replacen("phase: pushed", "phase: completed", 1),
    )
    .unwrap();

    let recovered = recover_remote_skill_transactions(&repo, &guard).unwrap();
    assert!(recovered.is_empty());
    assert!(!transaction_root.exists());
}

#[test]
fn startup_recovery_fails_closed_when_checkpoint_lock_exceeds_its_deadline() {
    let fixture = Fixture::new();
    let repo = GitStorage::new(fixture.first.path());
    let guard = SkillSyncGuard::new(fixture.first.path()).unwrap();
    let request_id = RequestId::generate();
    let request = SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
        request_id: request_id.clone(),
    });
    execute_remote_skill_transaction_with_test_config(
        &repo,
        &guard,
        transaction_request(request, None),
        SkillTransactionTestConfig {
            crash_after: Some(SkillTransactionCrashPoint::AfterPushed),
            ..SkillTransactionTestConfig::default()
        },
    )
    .unwrap_err();
    let checkpoint = SkillCheckpointStore::new(fixture.first.path()).unwrap();
    let checkpoint_lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&checkpoint.lock_path)
        .unwrap();
    checkpoint_lock.lock_exclusive().unwrap();

    let failures_before = skill_transport_failure_count();
    let error = recover_remote_skill_transactions_with_test_config(
        &repo,
        &guard,
        SkillTransactionTestConfig {
            transaction_timeout: Duration::from_millis(300),
            ..SkillTransactionTestConfig::default()
        },
    )
    .unwrap_err();
    FileExt::unlock(&checkpoint_lock).unwrap();

    assert!(skill_transaction_error_is_retryable(&error));
    assert!(skill_transport_failure_count() > failures_before);
    assert!(!checkpoint.path.exists());
    assert!(fixture
        .first
        .path()
        .join(".gitim/skill-transactions")
        .join(request_id.as_str())
        .join("transaction.yaml")
        .exists());
}

#[cfg(unix)]
#[test]
fn hanging_git_child_with_group_kill_failure_respects_deadline_and_releases_permit() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    let real_git = String::from_utf8(
        Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let wrapper = fixture.first.path().join("hanging-git");
    let marker = fixture.first.path().join("hanging-git-fired");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nif [ ! -e '{}' ] && [ \"$1\" = \"-c\" ]; then\n  for arg in \"$@\"; do\n    if [ \"$arg\" = \"fetch\" ]; then touch '{}'; sleep 10; fi\n  done\nfi\nexec '{}' \"$@\"\n",
            marker.display(),
            marker.display(),
            real_git.trim(),
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();

    let request_id = RequestId::generate();
    let request = SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
        request_id: request_id.clone(),
    });
    let repo = GitStorage::new(fixture.first.path());
    let guard = SkillSyncGuard::new(fixture.first.path()).unwrap();
    let config = SkillTransactionTestConfig {
        transaction_timeout: Duration::from_secs(20),
        git_command_timeout: Duration::from_secs(8),
        git_program: Some(wrapper),
        max_concurrency: 1,
        simulate_process_group_kill_failure: true,
        ..SkillTransactionTestConfig::default()
    };
    let failures_before = skill_transport_failure_count();
    let started = std::time::Instant::now();
    let error = execute_remote_skill_transaction_with_test_config(
        &repo,
        &guard,
        transaction_request(request.clone(), None),
        config.clone(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        gitim_sync::skill::checkpoint::SkillSyncError::Git(gitim_sync::git::GitError::Timeout(_))
    ));
    assert!(skill_transaction_error_is_retryable(&error));
    assert!(skill_transport_failure_count() > failures_before);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "timeout cleanup exceeded its bounded deadline"
    );
    assert!(fixture
        .first
        .path()
        .join(".gitim/skill-transactions")
        .join(request_id.as_str())
        .join("transaction.yaml")
        .exists());
    assert!(!fixture
        .first
        .path()
        .join(".gitim/skill-validation.json")
        .exists());

    let recovered = execute_remote_skill_transaction_with_test_config(
        &repo,
        &guard,
        transaction_request(request, None),
        config,
    )
    .unwrap();
    assert_eq!(recovered.result.control_revision, Some(1));
}

#[cfg(unix)]
#[test]
fn post_push_timeout_is_retryable_and_keeps_the_pushed_journal() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    let real_git = String::from_utf8(
        Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let wrapper = fixture.first.path().join("post-push-timeout-git");
    let marker = fixture.first.path().join("post-push-timeout-fired");
    let armed = fixture.first.path().join("post-push-timeout-armed");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nif [ -e '{}' ] && [ ! -e '{}' ] && [ \"$1\" = \"update-ref\" ]; then\n  touch '{}'; sleep 10\nfi\nexec '{}' \"$@\"\n",
            armed.display(),
            marker.display(),
            marker.display(),
            real_git.trim(),
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();

    let request_id = RequestId::generate();
    let request = SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
        request_id: request_id.clone(),
    });
    let repo = GitStorage::new(fixture.first.path());
    let guard = SkillSyncGuard::new(fixture.first.path()).unwrap();
    let config = SkillTransactionTestConfig {
        transaction_timeout: Duration::from_secs(30),
        post_push_git_command_timeout: Some(Duration::from_secs(2)),
        git_program: Some(wrapper),
        max_concurrency: 1,
        after_pushed: Some(Arc::new(move || {
            fs::write(&armed, b"armed").unwrap();
        })),
        ..SkillTransactionTestConfig::default()
    };
    let failures_before = skill_transport_failure_count();
    let error = execute_remote_skill_transaction_with_test_config(
        &repo,
        &guard,
        transaction_request(request.clone(), None),
        config.clone(),
    )
    .unwrap_err();

    assert!(skill_transaction_error_is_retryable(&error));
    assert!(skill_transport_failure_count() > failures_before);
    let journal = fixture
        .first
        .path()
        .join(".gitim/skill-transactions")
        .join(request_id.as_str())
        .join("transaction.yaml");
    assert!(journal.exists());
    let journal_text = fs::read_to_string(&journal).unwrap();
    assert!(
        journal_text.contains("phase: pushed"),
        "unexpected journal after {error:?}; armed={}, fired={}: {journal_text}",
        fixture
            .first
            .path()
            .join("post-push-timeout-armed")
            .exists(),
        fixture
            .first
            .path()
            .join("post-push-timeout-fired")
            .exists(),
    );
    assert!(!fixture
        .first
        .path()
        .join(".gitim/skill-validation.json")
        .exists());
    git(fixture.first.path(), ["fetch", "origin"]);
    assert!(String::from_utf8(
        git(
            fixture.first.path(),
            ["show", "origin/main:skills/workspace.meta.yaml",],
        )
        .stdout,
    )
    .is_ok());

    let recovered = execute_remote_skill_transaction_with_test_config(
        &repo,
        &guard,
        transaction_request(request, None),
        config,
    )
    .unwrap();
    assert_eq!(recovered.result.control_revision, Some(1));
    assert!(!journal.exists());
    assert!(fixture
        .first
        .path()
        .join(".gitim/skill-validation.json")
        .exists());
}

#[cfg(unix)]
#[test]
fn post_validation_timeout_keeps_the_previous_checkpoint_and_pushed_journal() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    fixture.bootstrap_and_create(&SkillSlug::new("checkpoint-base").unwrap());
    let checkpoint_path = fixture.first.path().join(".gitim/skill-validation.json");
    let checkpoint_before = fs::read(&checkpoint_path).unwrap();
    let real_git = String::from_utf8(
        Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let wrapper = fixture.first.path().join("post-validation-timeout-git");
    let marker = fixture.first.path().join("post-validation-timeout-fired");
    let armed = fixture.first.path().join("post-validation-timeout-armed");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nif [ -e '{}' ] && [ ! -e '{}' ] && [ \"$1\" = \"rev-parse\" ]; then\n  touch '{}'; sleep 10\nfi\nexec '{}' \"$@\"\n",
            armed.display(),
            marker.display(),
            marker.display(),
            real_git.trim(),
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();

    let request_id = RequestId::generate();
    let slug = SkillSlug::new("checkpoint-timeout").unwrap();
    let request = SkillMutationRequest::Create(SkillCreateRequest {
        request_id: request_id.clone(),
        slug: slug.clone(),
        display_name: "Checkpoint timeout".to_owned(),
        description: "Checkpoint publication ordering".to_owned(),
        source_directory: fixture.first.path().join("unused"),
    });
    let repo = GitStorage::new(fixture.first.path());
    let guard = SkillSyncGuard::new(fixture.first.path()).unwrap();
    let config = SkillTransactionTestConfig {
        transaction_timeout: Duration::from_secs(30),
        post_push_git_command_timeout: Some(Duration::from_secs(2)),
        git_program: Some(wrapper),
        max_concurrency: 1,
        after_pushed: Some(Arc::new(move || {
            fs::write(&armed, b"armed").unwrap();
        })),
        ..SkillTransactionTestConfig::default()
    };
    let error = execute_remote_skill_transaction_with_test_config(
        &repo,
        &guard,
        transaction_request(request.clone(), Some(package(&slug, "candidate"))),
        config.clone(),
    )
    .unwrap_err();

    assert!(skill_transaction_error_is_retryable(&error));
    assert_eq!(fs::read(&checkpoint_path).unwrap(), checkpoint_before);
    let journal = fixture
        .first
        .path()
        .join(".gitim/skill-transactions")
        .join(request_id.as_str())
        .join("transaction.yaml");
    assert!(fs::read_to_string(&journal)
        .unwrap()
        .contains("phase: pushed"));

    let recovered = execute_remote_skill_transaction_with_test_config(
        &repo,
        &guard,
        transaction_request(request, Some(package(&slug, "candidate"))),
        config,
    )
    .unwrap();
    assert_eq!(recovered.result.control_revision, Some(1));
    assert_ne!(fs::read(&checkpoint_path).unwrap(), checkpoint_before);
    assert!(!journal.exists());
}

#[cfg(unix)]
#[test]
fn checkpoint_finalization_validation_obeys_the_overall_deadline() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    let real_git = String::from_utf8(
        Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let wrapper = fixture.first.path().join("finalization-deadline-git");
    let marker = fixture.first.path().join("finalization-validation-fired");
    let armed = fixture.first.path().join("finalization-validation-armed");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nif [ -e '{}' ] && [ ! -e '{}' ] && [ \"$1\" = \"rev-list\" ]; then\n  for arg in \"$@\"; do\n    if [ \"$arg\" = \"--reverse\" ]; then touch '{}'; sleep 10; fi\n  done\nfi\nexec '{}' \"$@\"\n",
            armed.display(),
            marker.display(),
            marker.display(),
            real_git.trim(),
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();

    let request_id = RequestId::generate();
    let request = SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
        request_id: request_id.clone(),
    });
    let repo = GitStorage::new(fixture.first.path());
    let guard = SkillSyncGuard::new(fixture.first.path()).unwrap();
    let started = std::time::Instant::now();
    let error = execute_remote_skill_transaction_with_test_config(
        &repo,
        &guard,
        transaction_request(request, None),
        SkillTransactionTestConfig {
            transaction_timeout: Duration::from_secs(8),
            git_command_timeout: Duration::from_secs(10),
            git_program: Some(wrapper),
            after_built: Some(Arc::new(move || {
                fs::write(&armed, b"armed").unwrap();
            })),
            ..SkillTransactionTestConfig::default()
        },
    )
    .unwrap_err();

    assert!(skill_transaction_error_is_retryable(&error));
    assert!(
        marker.exists(),
        "checkpoint validation bypassed the test Git runner"
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "checkpoint validation exceeded the transaction deadline"
    );
    assert!(!fixture
        .first
        .path()
        .join(".gitim/skill-validation.json")
        .exists());
    assert!(fs::read_to_string(
        fixture
            .first
            .path()
            .join(".gitim/skill-transactions")
            .join(request_id.as_str())
            .join("transaction.yaml")
    )
    .unwrap()
    .contains("phase: pushed"));
}

#[cfg(unix)]
#[test]
fn deadline_exhaustion_reaps_the_killed_git_child_before_returning() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    let wrapper = fixture.first.path().join("stubborn-git");
    let pid_path = fixture.first.path().join("stubborn-git.pid");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\necho $$ > '{}'\ntrap '' TERM INT\nexec sleep 10\n",
            pid_path.display(),
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();

    let repo = GitStorage::new(fixture.first.path());
    let guard = SkillSyncGuard::new(fixture.first.path()).unwrap();
    let error = execute_remote_skill_transaction_with_test_config(
        &repo,
        &guard,
        transaction_request(
            SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
                request_id: RequestId::generate(),
            }),
            None,
        ),
        SkillTransactionTestConfig {
            transaction_timeout: Duration::from_secs(8),
            git_command_timeout: Duration::from_secs(10),
            git_program: Some(wrapper),
            simulate_process_group_kill_failure: true,
            ..SkillTransactionTestConfig::default()
        },
    )
    .unwrap_err();

    assert!(skill_transaction_error_is_retryable(&error));
    let pid: libc::pid_t = fs::read_to_string(&pid_path)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let mut status = 0;
    // SAFETY: `pid` was written by the direct child spawned for this test.
    let wait_result = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
    assert_eq!(
        wait_result, -1,
        "the transaction returned before reaping Git child {pid}"
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
}

#[cfg(unix)]
#[test]
fn exhausted_overall_deadline_reaps_the_direct_child_and_releases_the_permit() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    let pid_directory = TempDir::new().unwrap();
    let wrapper = fixture.first.path().join("deadline-git");
    let pid_path = pid_directory.path().join("deadline-git.pid");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\necho $$ > '{}'\nexec sleep 10\n",
            pid_path.display(),
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();

    let repo = GitStorage::new(fixture.first.path());
    let guard = SkillSyncGuard::new(fixture.first.path()).unwrap();
    let error = execute_remote_skill_transaction_with_test_config(
        &repo,
        &guard,
        transaction_request(
            SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
                request_id: RequestId::generate(),
            }),
            None,
        ),
        SkillTransactionTestConfig {
            transaction_timeout: Duration::from_secs(8),
            git_command_timeout: Duration::from_secs(10),
            git_program: Some(wrapper),
            max_concurrency: 1,
            simulate_process_group_kill_failure: true,
            simulated_child_kill_failures: 2,
            ..SkillTransactionTestConfig::default()
        },
    )
    .unwrap_err();

    assert!(skill_transaction_error_is_retryable(&error));
    assert!(
        pid_path.exists(),
        "Git wrapper did not start before {error:?}"
    );
    let pid: libc::pid_t = fs::read_to_string(&pid_path)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let mut status = 0;
    // SAFETY: `pid` was written by the direct child spawned for this test.
    let wait_result = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
    assert_eq!(
        wait_result, -1,
        "the transaction returned before reaping Git child {pid}"
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );

    let recovered = execute_remote_skill_transaction_with_test_config(
        &repo,
        &guard,
        transaction_request(
            SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
                request_id: RequestId::generate(),
            }),
            None,
        ),
        SkillTransactionTestConfig {
            transaction_timeout: Duration::from_secs(10),
            max_concurrency: 1,
            ..SkillTransactionTestConfig::default()
        },
    )
    .unwrap();
    assert_eq!(recovered.result.control_revision, Some(1));
}

#[test]
fn post_push_checkpoint_failure_returns_error_and_retains_the_pushed_journal() {
    let fixture = Fixture::new();
    let request_id = RequestId::generate();
    let checkpoint_path = fixture.first.path().join(".gitim/skill-validation.json");
    let callback_path = checkpoint_path.clone();
    let repo = GitStorage::new(fixture.first.path());
    let guard = SkillSyncGuard::new(fixture.first.path()).unwrap();
    execute_remote_skill_transaction_with_test_config(
        &repo,
        &guard,
        transaction_request(
            SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
                request_id: request_id.clone(),
            }),
            None,
        ),
        SkillTransactionTestConfig {
            after_built: Some(Arc::new(move || {
                if callback_path.is_file() {
                    fs::remove_file(&callback_path).unwrap();
                }
                fs::create_dir(&callback_path).unwrap();
            })),
            ..SkillTransactionTestConfig::default()
        },
    )
    .unwrap_err();

    assert!(checkpoint_path.is_dir());
    let journal = fs::read_to_string(
        fixture
            .first
            .path()
            .join(".gitim/skill-transactions")
            .join(request_id.as_str())
            .join("transaction.yaml"),
    )
    .unwrap();
    assert!(journal.contains("phase: pushed"));
}

#[test]
fn saturated_workspace_semaphore_obeys_the_overall_deadline() {
    let fixture = Fixture::new();
    let root = fixture.first.path().to_path_buf();
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let ready = Arc::new(Barrier::new(2));
    let worker_gate = Arc::clone(&gate);
    let worker_ready = Arc::clone(&ready);
    let worker_root = root.clone();
    let first = std::thread::spawn(move || {
        let repo = GitStorage::new(&worker_root);
        let guard = SkillSyncGuard::new(&worker_root).unwrap();
        execute_remote_skill_transaction_with_test_config(
            &repo,
            &guard,
            transaction_request(
                SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
                    request_id: RequestId::generate(),
                }),
                None,
            ),
            SkillTransactionTestConfig {
                max_concurrency: 1,
                after_built: Some(Arc::new(move || {
                    worker_ready.wait();
                    let (lock, changed) = &*worker_gate;
                    let mut released = lock.lock().unwrap();
                    while !*released {
                        released = changed.wait(released).unwrap();
                    }
                })),
                ..SkillTransactionTestConfig::default()
            },
        )
    });
    ready.wait();

    let repo = GitStorage::new(&root);
    let guard = SkillSyncGuard::new(&root).unwrap();
    let blocked_id = RequestId::generate();
    let error = execute_remote_skill_transaction_with_test_config(
        &repo,
        &guard,
        transaction_request(
            SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
                request_id: blocked_id.clone(),
            }),
            None,
        ),
        SkillTransactionTestConfig {
            transaction_timeout: Duration::from_millis(100),
            max_concurrency: 1,
            ..SkillTransactionTestConfig::default()
        },
    )
    .unwrap_err();
    assert!(matches!(
        error,
        gitim_sync::skill::checkpoint::SkillSyncError::Git(gitim_sync::git::GitError::Timeout(_))
    ));
    assert!(!root.join(".gitim/skill-validation.json").exists());
    let blocked_transaction = root
        .join(".gitim/skill-transactions")
        .join(blocked_id.as_str());
    assert!(blocked_transaction.join("source").is_dir());
    assert!(
        fs::read_to_string(blocked_transaction.join("transaction.yaml"))
            .unwrap()
            .contains("phase: prepared")
    );

    let (lock, changed) = &*gate;
    *lock.lock().unwrap() = true;
    changed.notify_all();
    assert!(first.join().unwrap().is_ok());
}

#[test]
fn checkpoint_lock_contention_obeys_deadline_and_retains_pushed_journal() {
    let fixture = Fixture::new();
    let root = fixture.first.path().to_path_buf();
    let checkpoint = SkillCheckpointStore::new(&root).unwrap();

    let request_id = RequestId::generate();
    let request = SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
        request_id: request_id.clone(),
    });
    let (built_sender, built_receiver) = std::sync::mpsc::sync_channel(1);
    let (continue_sender, continue_receiver) = std::sync::mpsc::sync_channel(1);
    let continue_receiver = Arc::new(Mutex::new(continue_receiver));
    let (result_sender, result_receiver) = std::sync::mpsc::sync_channel(1);
    let worker_root = root.clone();
    let worker_request = request.clone();
    let failures_before = skill_transport_failure_count();
    let worker = std::thread::spawn(move || {
        let repo = GitStorage::new(&worker_root);
        let guard = SkillSyncGuard::new(&worker_root).unwrap();
        let result = execute_remote_skill_transaction_with_test_config(
            &repo,
            &guard,
            transaction_request(worker_request, None),
            SkillTransactionTestConfig {
                transaction_timeout: Duration::from_secs(2),
                max_concurrency: 1,
                after_built: Some(Arc::new(move || {
                    built_sender.send(()).unwrap();
                    continue_receiver.lock().unwrap().recv().unwrap();
                })),
                ..SkillTransactionTestConfig::default()
            },
        );
        result_sender.send(result).unwrap();
    });
    built_receiver
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    let checkpoint_lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&checkpoint.lock_path)
        .unwrap();
    checkpoint_lock.lock_exclusive().unwrap();
    continue_sender.send(()).unwrap();
    let result_while_lock_held = result_receiver.recv_timeout(Duration::from_secs(4));

    FileExt::unlock(&checkpoint_lock).unwrap();
    worker.join().unwrap();
    let error = result_while_lock_held
        .expect("checkpoint lock wait exceeded the transaction deadline")
        .unwrap_err();
    assert!(skill_transaction_error_is_retryable(&error));
    assert!(skill_transport_failure_count() > failures_before);
    assert!(!checkpoint.path.exists());
    let journal_path = root
        .join(".gitim/skill-transactions")
        .join(request_id.as_str())
        .join("transaction.yaml");
    let journal = fs::read_to_string(&journal_path).unwrap();
    assert!(journal.contains("phase: pushed"));

    let repo = GitStorage::new(&root);
    let guard = SkillSyncGuard::new(&root).unwrap();
    let recovered = execute_remote_skill_transaction_with_test_config(
        &repo,
        &guard,
        transaction_request(request, None),
        SkillTransactionTestConfig {
            max_concurrency: 1,
            ..SkillTransactionTestConfig::default()
        },
    )
    .unwrap();
    assert_eq!(recovered.result.control_revision, Some(1));
    assert!(!journal_path.exists());
}

#[test]
fn configured_one_permit_is_shared_by_clones_of_the_same_workspace() {
    let fixture = Fixture::new();
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let ready = Arc::new(Barrier::new(2));
    let worker_gate = Arc::clone(&gate);
    let worker_ready = Arc::clone(&ready);
    let first_root = fixture.first.path().to_path_buf();
    let first = std::thread::spawn(move || {
        let repo = GitStorage::new(&first_root);
        let guard = SkillSyncGuard::new(&first_root).unwrap();
        execute_remote_skill_transaction_with_test_config(
            &repo,
            &guard,
            transaction_request(
                SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
                    request_id: RequestId::generate(),
                }),
                None,
            ),
            SkillTransactionTestConfig {
                max_concurrency: 1,
                after_built: Some(Arc::new(move || {
                    worker_ready.wait();
                    let (lock, changed) = &*worker_gate;
                    let mut released = lock.lock().unwrap();
                    while !*released {
                        released = changed.wait(released).unwrap();
                    }
                })),
                ..SkillTransactionTestConfig::default()
            },
        )
    });
    ready.wait();

    let entered = Arc::new(AtomicBool::new(false));
    let callback_entered = Arc::clone(&entered);
    let repo = GitStorage::new(fixture.second.path());
    let guard = SkillSyncGuard::new(fixture.second.path()).unwrap();
    let blocked_id = RequestId::generate();
    let error = execute_remote_skill_transaction_with_test_config(
        &repo,
        &guard,
        transaction_request(
            SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
                request_id: blocked_id.clone(),
            }),
            None,
        ),
        SkillTransactionTestConfig {
            transaction_timeout: Duration::from_millis(500),
            max_concurrency: 1,
            after_built: Some(Arc::new(move || {
                callback_entered.store(true, Ordering::SeqCst);
            })),
            ..SkillTransactionTestConfig::default()
        },
    )
    .unwrap_err();

    assert!(skill_transaction_error_is_retryable(&error));
    assert!(!entered.load(Ordering::SeqCst));
    assert!(fixture
        .second
        .path()
        .join(".gitim/skill-transactions")
        .join(blocked_id.as_str())
        .join("transaction.yaml")
        .exists());

    let (lock, changed) = &*gate;
    *lock.lock().unwrap() = true;
    changed.notify_all();
    assert!(first.join().unwrap().is_ok());
}

#[test]
fn production_workspace_pool_allows_exactly_four_clone_transactions() {
    let fixture = Fixture::new();
    let clones: Vec<_> = (0..5).map(|_| fixture.clone_remote()).collect();
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let mut workers = Vec::new();

    for clone in clones.iter().take(SKILL_GIT_MAX_CONCURRENCY) {
        let root = clone.path().to_path_buf();
        let worker_gate = Arc::clone(&gate);
        let worker_ready = ready_tx.clone();
        workers.push(std::thread::spawn(move || {
            let repo = GitStorage::new(&root);
            let guard = SkillSyncGuard::new(&root).unwrap();
            execute_remote_skill_transaction_with_test_config(
                &repo,
                &guard,
                transaction_request(
                    SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
                        request_id: RequestId::generate(),
                    }),
                    None,
                ),
                SkillTransactionTestConfig {
                    after_built: Some(Arc::new(move || {
                        worker_ready.send(()).unwrap();
                        let (lock, changed) = &*worker_gate;
                        let mut released = lock.lock().unwrap();
                        while !*released {
                            released = changed.wait(released).unwrap();
                        }
                    })),
                    ..SkillTransactionTestConfig::default()
                },
            )
        }));
    }
    drop(ready_tx);
    for _ in 0..SKILL_GIT_MAX_CONCURRENCY {
        ready_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    }

    let entered = Arc::new(AtomicBool::new(false));
    let callback_entered = Arc::clone(&entered);
    let fifth_root = clones[SKILL_GIT_MAX_CONCURRENCY].path();
    let repo = GitStorage::new(fifth_root);
    let guard = SkillSyncGuard::new(fifth_root).unwrap();
    let error = execute_remote_skill_transaction_with_test_config(
        &repo,
        &guard,
        transaction_request(
            SkillMutationRequest::WorkspaceBootstrap(SkillWorkspaceBootstrapRequest {
                request_id: RequestId::generate(),
            }),
            None,
        ),
        SkillTransactionTestConfig {
            transaction_timeout: Duration::from_millis(500),
            after_built: Some(Arc::new(move || {
                callback_entered.store(true, Ordering::SeqCst);
            })),
            ..SkillTransactionTestConfig::default()
        },
    )
    .unwrap_err();

    assert!(skill_transaction_error_is_retryable(&error));
    assert!(!entered.load(Ordering::SeqCst));

    let (lock, changed) = &*gate;
    *lock.lock().unwrap() = true;
    changed.notify_all();
    for worker in workers {
        let _ = worker.join().unwrap();
    }
}
