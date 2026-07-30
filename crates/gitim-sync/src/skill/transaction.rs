use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use gitim_core::epoch::{EpochFile, EpochStatus};
use gitim_core::skill::{
    plan_skill_mutation, validate_package_entries, validate_skill_commit, PackageEntry,
    PackageEntryKind, ProposalId, RequestId, RevisionId, SkillCommitEvidence, SkillError,
    SkillMeta, SkillMutationContext, SkillMutationPlan, SkillMutationRequest, SkillMutationResult,
    SkillObjectSnapshot, SkillProposalMeta, SkillProposalSnapshot, SkillPublicationMeta,
    SkillReceipt, SkillRepairAcceptedState, SkillRepairScope, SkillRepositorySnapshot,
    SkillRevisionMeta, SkillRevisionSnapshot, SkillSlug, SkillTreeEdit, ValidatedPackage,
    WorkspaceSkillMeta,
};
use serde::{Deserialize, Serialize};

use super::checkpoint::{
    validate_incoming_skill_history, AcceptedSkillState, AcceptedTree, SkillCheckpointStore,
    SkillSyncError, SkillValidationCheckpoint,
};
use super::guard::SkillSyncGuard;
use crate::git::{classify_remote_error, GitError, GitStorage, GIT_HTTP_TIMEOUT_ARGS};

pub const SKILL_TRANSACTION_TIMEOUT: Duration = Duration::from_secs(180);
pub const SKILL_GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
pub const SKILL_GIT_MAX_CONCURRENCY: usize = 4;

const JOURNAL_SCHEMA_VERSION: u32 = 1;
const MAX_EPOCH_DEPTH: usize = 32;
const RECEIPT_ROOT: &str = "skills/receipts";

static SKILL_TRANSPORT_FAILURES: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillTransactionPhase {
    Prepared,
    Built,
    Pushed,
    Completed,
}

#[derive(Clone, Debug)]
pub struct RemoteSkillTransactionRequest {
    pub request: SkillMutationRequest,
    pub actor: String,
    pub author_email: String,
    pub now: String,
    pub package: Option<ValidatedPackage>,
    pub active_users: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteSkillTransactionResult {
    pub commit_id: String,
    pub result: SkillMutationResult,
    pub local_state: SkillLocalState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillLocalState {
    Current,
    PendingSync,
}

#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkillTransactionCrashPoint {
    AfterPrepared,
    AfterBuilt,
    AfterPushed,
}

#[doc(hidden)]
#[derive(Clone)]
pub struct SkillTransactionTestConfig {
    pub transaction_timeout: Duration,
    pub git_command_timeout: Duration,
    pub max_concurrency: usize,
    pub git_program: Option<PathBuf>,
    pub crash_after: Option<SkillTransactionCrashPoint>,
    pub after_built: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Default for SkillTransactionTestConfig {
    fn default() -> Self {
        Self {
            transaction_timeout: SKILL_TRANSACTION_TIMEOUT,
            git_command_timeout: SKILL_GIT_COMMAND_TIMEOUT,
            max_concurrency: SKILL_GIT_MAX_CONCURRENCY,
            git_program: None,
            crash_after: None,
            after_built: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TransactionJournal {
    schema_version: u32,
    phase: SkillTransactionPhase,
    request_fingerprint: String,
    request: SkillMutationRequest,
    actor: String,
    author_email: String,
    now: String,
    package_sha256: Option<String>,
    active_users: BTreeSet<String>,
    receipt_path: String,
    source_directory: PathBuf,
    private_index: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    remote_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    remote_tip: Option<String>,
    #[serde(default)]
    semantic_oids: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    candidate_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result: Option<SkillMutationResult>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceManifest {
    paths: Vec<String>,
}

struct WorkspaceSemaphore {
    available: Mutex<usize>,
    changed: Condvar,
}

type WorkspaceSemaphoreKey = (PathBuf, usize);
type WorkspaceSemaphores = Mutex<HashMap<WorkspaceSemaphoreKey, Weak<WorkspaceSemaphore>>>;

struct WorkspacePermit {
    semaphore: Arc<WorkspaceSemaphore>,
}

impl Drop for WorkspacePermit {
    fn drop(&mut self) {
        let mut available = self
            .semaphore
            .available
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *available += 1;
        self.semaphore.changed.notify_one();
    }
}

struct TransactionContext {
    deadline: Instant,
    git_timeout: Duration,
    max_concurrency: usize,
    git_program: PathBuf,
    crash_after: Option<SkillTransactionCrashPoint>,
    after_built: Option<Arc<dyn Fn() + Send + Sync>>,
}

#[derive(Default)]
struct TreeMaterial {
    files: BTreeMap<String, Vec<u8>>,
    modes: BTreeMap<String, String>,
}

#[derive(Clone)]
struct TreeEntry {
    mode: String,
    object_type: String,
    oid: String,
    path: String,
}

pub fn execute_remote_skill_transaction(
    repo: &GitStorage,
    guard: &SkillSyncGuard,
    request: RemoteSkillTransactionRequest,
) -> Result<RemoteSkillTransactionResult, SkillSyncError> {
    execute_remote_skill_transaction_with_config(
        repo,
        guard,
        request,
        SkillTransactionTestConfig::default(),
    )
}

#[doc(hidden)]
pub fn execute_remote_skill_transaction_with_test_config(
    repo: &GitStorage,
    guard: &SkillSyncGuard,
    request: RemoteSkillTransactionRequest,
    config: SkillTransactionTestConfig,
) -> Result<RemoteSkillTransactionResult, SkillSyncError> {
    execute_remote_skill_transaction_with_config(repo, guard, request, config)
}

#[doc(hidden)]
pub fn skill_transport_failure_count() -> u64 {
    SKILL_TRANSPORT_FAILURES.load(Ordering::Relaxed)
}

fn execute_remote_skill_transaction_with_config(
    repo: &GitStorage,
    guard: &SkillSyncGuard,
    request: RemoteSkillTransactionRequest,
    config: SkillTransactionTestConfig,
) -> Result<RemoteSkillTransactionResult, SkillSyncError> {
    if config.max_concurrency == 0 {
        return Err(checkpoint_error(
            "transaction concurrency must be greater than zero",
        ));
    }
    let deadline = Instant::now()
        .checked_add(config.transaction_timeout)
        .ok_or_else(|| checkpoint_error("transaction deadline overflow"))?;
    let context = TransactionContext {
        deadline,
        git_timeout: config.git_command_timeout,
        max_concurrency: config.max_concurrency,
        git_program: config.git_program.unwrap_or_else(|| PathBuf::from("git")),
        crash_after: config.crash_after,
        after_built: config.after_built,
    };
    let result = execute_transaction(repo, guard, request, &context);
    if matches!(result, Err(SkillSyncError::Git(GitError::Timeout(_)))) {
        SKILL_TRANSPORT_FAILURES.fetch_add(1, Ordering::Relaxed);
    }
    result
}

fn execute_transaction(
    repo: &GitStorage,
    guard: &SkillSyncGuard,
    request: RemoteSkillTransactionRequest,
    context: &TransactionContext,
) -> Result<RemoteSkillTransactionResult, SkillSyncError> {
    if !repo.has_remote() {
        return Err(SkillError::RemoteRequired.into());
    }
    guard.quarantine_resolved()?;
    let _permit = acquire_workspace_permit(repo.root(), context.deadline, context.max_concurrency)?;
    if let Some(recovered) = recover_current_transaction(repo, &request, context)? {
        return Ok(recovered);
    }
    let mut journal = prepare_journal(repo, request)?;
    let package = load_snapshotted_package(&journal)?;
    maybe_crash(context, SkillTransactionCrashPoint::AfterPrepared)?;

    for attempt in 0..3 {
        ensure_before_deadline(context)?;
        fetch(repo, context)?;
        let start_branch = current_branch(repo, context)?;
        let (remote_branch, remote_tip) = resolve_active_remote(repo, &start_branch, context)?;
        let mut snapshot = load_snapshot_for_request(
            repo,
            &remote_tip,
            &journal.active_users,
            &journal.request,
            context,
        )?;
        attach_repair_checkpoint(repo, &mut snapshot, &journal.request, &remote_tip, context)?;

        let mutation_context = SkillMutationContext {
            actor: journal.actor.clone(),
            now: journal.now.clone(),
            package: package.clone(),
        };
        let plan = plan_skill_mutation(&snapshot, &mutation_context, &journal.request)?;
        if plan.edits.is_empty() {
            let commit_id = find_receipt_commit(repo, &remote_tip, &journal.receipt_path, context)?;
            let local_state =
                record_accepted_view(repo, &remote_branch, &remote_tip, &remote_tip, context)
                    .unwrap_or(SkillLocalState::PendingSync);
            complete_journal(&mut journal)?;
            return Ok(RemoteSkillTransactionResult {
                commit_id,
                result: plan.result,
                local_state,
            });
        }

        let mut captured_semantic_oids =
            semantic_oids(repo, &remote_tip, &journal.request, &snapshot, context)?;
        if let Some(fingerprint) = repair_checkpoint_fingerprint(repo, &journal.request)? {
            captured_semantic_oids.insert("$checkpoint".to_owned(), fingerprint);
        }
        journal.remote_branch = Some(remote_branch.clone());
        journal.remote_tip = Some(remote_tip.clone());
        journal.semantic_oids = captured_semantic_oids.clone();
        journal.phase = SkillTransactionPhase::Prepared;
        save_journal(&journal)?;

        let candidate = build_candidate(repo, &journal, &plan, context)?;
        validate_candidate(
            repo,
            &candidate,
            &snapshot,
            &plan,
            &journal.active_users,
            context,
        )?;
        journal.candidate_commit = Some(candidate.clone());
        journal.result = Some(plan.result.clone());
        journal.phase = SkillTransactionPhase::Built;
        save_journal(&journal)?;
        if let Some(after_built) = &context.after_built {
            after_built();
        }
        maybe_crash(context, SkillTransactionCrashPoint::AfterBuilt)?;
        ensure_repair_checkpoint_unchanged(repo, &journal.request, &captured_semantic_oids)?;

        match push_candidate(repo, &candidate, &remote_branch, context) {
            Ok(()) => {
                journal.phase = SkillTransactionPhase::Pushed;
                save_journal(&journal)?;
                maybe_crash(context, SkillTransactionCrashPoint::AfterPushed)?;
                let local_state =
                    record_accepted_view(repo, &remote_branch, &remote_tip, &candidate, context)
                        .unwrap_or(SkillLocalState::PendingSync);
                complete_journal(&mut journal)?;
                return Ok(RemoteSkillTransactionResult {
                    commit_id: candidate,
                    result: plan.result,
                    local_state,
                });
            }
            Err(SkillSyncError::Git(GitError::PushConflict)) if attempt < 2 => {
                fetch(repo, context)?;
                let (next_branch, next_tip) = resolve_active_remote(repo, &start_branch, context)?;
                let mut next_snapshot = load_snapshot_for_request(
                    repo,
                    &next_tip,
                    &journal.active_users,
                    &journal.request,
                    context,
                )?;
                attach_repair_checkpoint(
                    repo,
                    &mut next_snapshot,
                    &journal.request,
                    &next_tip,
                    context,
                )?;
                let duplicate =
                    plan_skill_mutation(&next_snapshot, &mutation_context, &journal.request);
                match duplicate {
                    Ok(duplicate) if duplicate.edits.is_empty() => {
                        let commit_id =
                            find_receipt_commit(repo, &next_tip, &journal.receipt_path, context)?;
                        let local_state =
                            record_accepted_view(repo, &next_branch, &next_tip, &next_tip, context)
                                .unwrap_or(SkillLocalState::PendingSync);
                        complete_journal(&mut journal)?;
                        return Ok(RemoteSkillTransactionResult {
                            commit_id,
                            result: duplicate.result,
                            local_state,
                        });
                    }
                    Err(SkillError::RequestIdConflict) => {
                        return Err(SkillError::RequestIdConflict.into());
                    }
                    _ => {}
                }
                let mut next_semantic =
                    semantic_oids(repo, &next_tip, &journal.request, &next_snapshot, context)?;
                if let Some(fingerprint) = repair_checkpoint_fingerprint(repo, &journal.request)? {
                    next_semantic.insert("$checkpoint".to_owned(), fingerprint);
                }
                if captured_semantic_oids != next_semantic {
                    return Err(SkillError::SyncConflict.into());
                }
            }
            Err(error) => return Err(error),
        }
    }

    Err(GitError::PushConflict.into())
}

fn recover_current_transaction(
    repo: &GitStorage,
    request: &RemoteSkillTransactionRequest,
    context: &TransactionContext,
) -> Result<Option<RemoteSkillTransactionResult>, SkillSyncError> {
    let root = transaction_root(repo.root(), request.request.request_id())?;
    let journal_path = root.join("transaction.yaml");
    if !journal_path.exists() {
        return Ok(None);
    }
    let mut journal = load_journal(&journal_path)?;
    let fingerprint =
        request_fingerprint(&request.request, &request.actor, request.package.as_ref())?;
    if journal.request_fingerprint != fingerprint {
        return Err(SkillError::RequestIdConflict.into());
    }
    if journal.phase == SkillTransactionPhase::Completed {
        fs::remove_dir_all(&root)
            .map_err(|error| checkpoint_io("remove completed transaction", error))?;
        return Ok(None);
    }

    fetch(repo, context)?;
    let start_branch = current_branch(repo, context)?;
    let (remote_branch, remote_tip) = resolve_active_remote(repo, &start_branch, context)?;
    if read_optional_blob(repo, &remote_tip, &journal.receipt_path, context)?.is_some() {
        let package = load_snapshotted_package(&journal)?;
        let snapshot = load_snapshot(repo, &remote_tip, &journal.active_users, context)?;
        let duplicate = plan_skill_mutation(
            &snapshot,
            &SkillMutationContext {
                actor: journal.actor.clone(),
                now: journal.now.clone(),
                package,
            },
            &journal.request,
        )?;
        if !duplicate.edits.is_empty() {
            return Err(SkillError::RequestIdConflict.into());
        }
        let commit_id = find_receipt_commit(repo, &remote_tip, &journal.receipt_path, context)?;
        if journal
            .candidate_commit
            .as_ref()
            .is_some_and(|candidate| candidate != &commit_id)
        {
            return Err(checkpoint_error(
                "published receipt does not match the journal candidate",
            ));
        }
        let local_state =
            record_accepted_view(repo, &remote_branch, &remote_tip, &remote_tip, context)
                .unwrap_or(SkillLocalState::PendingSync);
        complete_journal(&mut journal)?;
        return Ok(Some(RemoteSkillTransactionResult {
            commit_id,
            result: duplicate.result,
            local_state,
        }));
    }

    if journal.phase == SkillTransactionPhase::Pushed {
        return Err(checkpoint_error(
            "pushed transaction has no authoritative receipt",
        ));
    }
    remove_recorded_scratch(&journal)?;
    fs::remove_dir_all(&root)
        .map_err(|error| checkpoint_io("remove unpublished transaction", error))?;
    Ok(None)
}

fn prepare_journal(
    repo: &GitStorage,
    request: RemoteSkillTransactionRequest,
) -> Result<TransactionJournal, SkillSyncError> {
    let request_id = request.request.request_id().clone();
    let root = transaction_root(repo.root(), &request_id)?;
    let journal_path = root.join("transaction.yaml");
    let fingerprint =
        request_fingerprint(&request.request, &request.actor, request.package.as_ref())?;
    if journal_path.exists() {
        let existing = load_journal(&journal_path)?;
        if existing.request_fingerprint != fingerprint {
            return Err(SkillError::RequestIdConflict.into());
        }
        match existing.phase {
            SkillTransactionPhase::Prepared | SkillTransactionPhase::Built => {
                remove_recorded_scratch(&existing)?;
                fs::remove_dir_all(&root)
                    .map_err(|error| checkpoint_io("remove incomplete transaction", error))?;
            }
            SkillTransactionPhase::Pushed => return Ok(existing),
            SkillTransactionPhase::Completed => {
                fs::remove_dir_all(&root)
                    .map_err(|error| checkpoint_io("remove completed transaction", error))?;
            }
        }
    }

    create_real_directory(&root)?;
    let source_directory = root.join("source");
    snapshot_package(&source_directory, request.package.as_ref())?;
    let private_index = root.join("private-index");
    let journal = TransactionJournal {
        schema_version: JOURNAL_SCHEMA_VERSION,
        phase: SkillTransactionPhase::Prepared,
        request_fingerprint: fingerprint,
        request: request.request,
        actor: request.actor,
        author_email: request.author_email,
        now: request.now,
        package_sha256: request
            .package
            .as_ref()
            .map(|value| value.content_sha256.clone()),
        active_users: request.active_users,
        receipt_path: receipt_path(&request_id),
        source_directory,
        private_index,
        remote_branch: None,
        remote_tip: None,
        semantic_oids: BTreeMap::new(),
        candidate_commit: None,
        result: None,
    };
    save_journal(&journal)?;
    Ok(journal)
}

fn request_fingerprint(
    request: &SkillMutationRequest,
    actor: &str,
    package: Option<&ValidatedPackage>,
) -> Result<String, SkillSyncError> {
    let mut value = serde_json::to_value(request)
        .map_err(|error| checkpoint_error(format!("serialize request fingerprint: {error}")))?;
    clear_source_directory(&mut value);
    serde_json::to_string(&(actor, value, package.map(|value| &value.content_sha256)))
        .map_err(|error| checkpoint_error(format!("encode request fingerprint: {error}")))
}

fn clear_source_directory(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(values) => {
            values.remove("source_directory");
            for value in values.values_mut() {
                clear_source_directory(value);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                clear_source_directory(value);
            }
        }
        _ => {}
    }
}

fn transaction_root(root: &Path, request_id: &RequestId) -> Result<PathBuf, SkillSyncError> {
    let root = root
        .canonicalize()
        .map_err(|error| checkpoint_io("canonicalize repository", error))?;
    let transactions = root.join(".gitim").join("skill-transactions");
    create_real_directory(&transactions)?;
    Ok(transactions.join(request_id.as_str()))
}

fn create_real_directory(path: &Path) -> Result<(), SkillSyncError> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(checkpoint_error(format!(
                "{} must be a real directory",
                path.display()
            )));
        }
        return Ok(());
    }
    fs::create_dir_all(path).map_err(|error| checkpoint_io("create transaction directory", error))
}

fn snapshot_package(
    source: &Path,
    package: Option<&ValidatedPackage>,
) -> Result<(), SkillSyncError> {
    create_real_directory(source)?;
    let mut paths = Vec::new();
    if let Some(package) = package {
        for entry in &package.entries {
            validate_relative_path(&entry.path)?;
            let destination = source.join(&entry.path);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| checkpoint_io("create package snapshot directory", error))?;
            }
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options
                .open(&destination)
                .map_err(|error| checkpoint_io("create package snapshot file", error))?;
            file.write_all(&entry.bytes)
                .and_then(|()| file.sync_all())
                .map_err(|error| checkpoint_io("write package snapshot", error))?;
            paths.push(entry.path.clone());
        }
    }
    let manifest = source
        .parent()
        .ok_or_else(|| checkpoint_error("source snapshot has no parent"))?
        .join("source-manifest.json");
    write_atomic_json(&manifest, &SourceManifest { paths })
}

fn load_snapshotted_package(
    journal: &TransactionJournal,
) -> Result<Option<ValidatedPackage>, SkillSyncError> {
    let Some(expected_hash) = journal.package_sha256.as_deref() else {
        return Ok(None);
    };
    let manifest = journal
        .source_directory
        .parent()
        .ok_or_else(|| checkpoint_error("source snapshot has no parent"))?
        .join("source-manifest.json");
    let bytes = fs::read(manifest).map_err(|error| checkpoint_io("read source manifest", error))?;
    let manifest: SourceManifest = serde_json::from_slice(&bytes)
        .map_err(|error| checkpoint_error(format!("parse source manifest: {error}")))?;
    let slug = request_slug(&journal.request).ok_or(SkillError::InvalidPackage)?;
    let mut entries = Vec::new();
    for path in manifest.paths {
        validate_relative_path(&path)?;
        let bytes = fs::read(journal.source_directory.join(&path))
            .map_err(|error| checkpoint_io("read package snapshot", error))?;
        entries.push(PackageEntry::new(path, bytes));
    }
    let package = validate_package_entries(slug, entries)?;
    if package.content_sha256 != expected_hash {
        return Err(SkillError::InvalidPackage.into());
    }
    Ok(Some(package))
}

fn request_slug(request: &SkillMutationRequest) -> Option<&SkillSlug> {
    match request {
        SkillMutationRequest::Create(request) => Some(&request.slug),
        SkillMutationRequest::Propose(request) => Some(&request.slug),
        SkillMutationRequest::Repair(request) => match &request.scope {
            SkillRepairScope::Workspace => None,
            SkillRepairScope::Skill(slug) => Some(slug),
        },
        SkillMutationRequest::WorkspaceBootstrap(_)
        | SkillMutationRequest::ProposalTransition(_) => None,
    }
}

fn save_journal(journal: &TransactionJournal) -> Result<(), SkillSyncError> {
    let root = journal
        .source_directory
        .parent()
        .ok_or_else(|| checkpoint_error("transaction source has no parent"))?;
    let path = root.join("transaction.yaml");
    let bytes = serde_yaml::to_string(journal)
        .map_err(|error| checkpoint_error(format!("serialize transaction journal: {error}")))?;
    write_atomic(&path, bytes.as_bytes())
}

fn load_journal(path: &Path) -> Result<TransactionJournal, SkillSyncError> {
    let bytes = fs::read(path).map_err(|error| checkpoint_io("read transaction journal", error))?;
    let journal: TransactionJournal = serde_yaml::from_slice(&bytes)
        .map_err(|error| checkpoint_error(format!("parse transaction journal: {error}")))?;
    if journal.schema_version != JOURNAL_SCHEMA_VERSION {
        return Err(checkpoint_error("unsupported transaction journal schema"));
    }
    Ok(journal)
}

fn complete_journal(journal: &mut TransactionJournal) -> Result<(), SkillSyncError> {
    journal.phase = SkillTransactionPhase::Completed;
    save_journal(journal)?;
    let root = journal
        .source_directory
        .parent()
        .ok_or_else(|| checkpoint_error("transaction source has no parent"))?;
    fs::remove_dir_all(root).map_err(|error| checkpoint_io("remove transaction journal", error))
}

fn remove_recorded_scratch(journal: &TransactionJournal) -> Result<(), SkillSyncError> {
    if journal.private_index.exists() {
        fs::remove_file(&journal.private_index)
            .map_err(|error| checkpoint_io("remove private index", error))?;
    }
    Ok(())
}

fn write_atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<(), SkillSyncError> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| checkpoint_error(format!("serialize JSON: {error}")))?;
    write_atomic(path, &bytes)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), SkillSyncError> {
    let parent = path
        .parent()
        .ok_or_else(|| checkpoint_error("transaction file has no parent"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| checkpoint_io("create transaction temporary file", error))?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.flush())
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| checkpoint_io("write transaction file", error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| checkpoint_io("chmod transaction file", error))?;
    }
    temporary
        .persist(path)
        .map_err(|error| checkpoint_io("persist transaction file", error.error))?;
    Ok(())
}

fn load_snapshot(
    repo: &GitStorage,
    commit: &str,
    active_users: &BTreeSet<String>,
    context: &TransactionContext,
) -> Result<SkillRepositorySnapshot, SkillSyncError> {
    let material = load_tree_material(repo, commit, context)?;
    parse_snapshot(material, active_users.clone()).map_err(Into::into)
}

fn load_snapshot_for_request(
    repo: &GitStorage,
    commit: &str,
    active_users: &BTreeSet<String>,
    request: &SkillMutationRequest,
    context: &TransactionContext,
) -> Result<SkillRepositorySnapshot, SkillSyncError> {
    let SkillMutationRequest::Repair(repair) = request else {
        return load_snapshot(repo, commit, active_users, context);
    };
    let checkpoint = SkillCheckpointStore::new(repo.root())?
        .load()?
        .ok_or(SkillError::SyncConflict)?;
    if checkpoint.last_scanned_tip != commit {
        return Err(SkillError::SyncConflict.into());
    }
    let (key, slug) = match &repair.scope {
        SkillRepairScope::Workspace => ("$workspace", None),
        SkillRepairScope::Skill(slug) => (slug.as_str(), Some(slug)),
    };
    let conflict = checkpoint
        .conflicts
        .get(key)
        .ok_or(SkillError::SyncConflict)?;
    let actual = load_tree_material(repo, commit, context)?;
    let actual_scope_files: BTreeMap<_, _> = actual
        .files
        .iter()
        .filter(|(path, _)| scope_contains(path, slug))
        .map(|(path, bytes)| (path.clone(), bytes.clone()))
        .collect();
    let actual_rejected_receipts: BTreeMap<_, _> = conflict
        .rejected_receipt_paths
        .iter()
        .filter_map(|path| {
            actual
                .files
                .get(path)
                .map(|bytes| (path.clone(), bytes.clone()))
        })
        .collect();
    let mut parseable = actual;
    parseable.files.retain(|path, _| {
        !scope_contains(path, slug) && !conflict.rejected_receipt_paths.contains(path)
    });
    parseable.modes.retain(|path, _| {
        !scope_contains(path, slug) && !conflict.rejected_receipt_paths.contains(path)
    });
    if let Some((accepted_commit, _)) = accepted_scope_location(&checkpoint, slug) {
        let accepted = load_tree_material(repo, accepted_commit, context)?;
        parseable.files.extend(
            accepted
                .files
                .into_iter()
                .filter(|(path, _)| scope_contains(path, slug)),
        );
        parseable.modes.extend(
            accepted
                .modes
                .into_iter()
                .filter(|(path, _)| scope_contains(path, slug)),
        );
    }
    let mut snapshot = parse_snapshot(parseable, active_users.clone())?;
    snapshot
        .repository_files
        .retain(|path, _| !scope_contains(path, slug));
    snapshot.repository_files.extend(actual_scope_files);
    for (path, bytes) in actual_rejected_receipts {
        let request_id = receipt_id_from_path(&path).ok_or(SkillError::SyncConflict)?;
        let receipt: SkillReceipt =
            serde_yaml::from_slice(&bytes).map_err(|_| SkillError::SyncConflict)?;
        if receipt.id != request_id {
            return Err(SkillError::SyncConflict.into());
        }
        snapshot.repository_files.insert(path, bytes);
        snapshot.receipts.insert(request_id, receipt);
    }
    Ok(snapshot)
}

fn load_tree_material(
    repo: &GitStorage,
    commit: &str,
    context: &TransactionContext,
) -> Result<TreeMaterial, SkillSyncError> {
    validate_revision(commit)?;
    let output = run_git(
        repo,
        &[
            "ls-tree",
            "-r",
            "-z",
            "--full-tree",
            commit,
            "--",
            "skills",
            "archive/skills",
        ],
        &[],
        context,
    )?;
    let mut material = TreeMaterial::default();
    for entry in parse_tree_entries(&output.stdout)? {
        validate_relative_path(&entry.path)?;
        if entry.object_type != "blob" {
            return Err(SkillError::SyncConflict.into());
        }
        material.modes.insert(entry.path.clone(), entry.mode);
        let blob = run_git(repo, &["cat-file", "blob", &entry.oid], &[], context)?;
        material.files.insert(entry.path, blob.stdout);
    }
    Ok(material)
}

fn parse_snapshot(
    material: TreeMaterial,
    active_users: BTreeSet<String>,
) -> Result<SkillRepositorySnapshot, SkillError> {
    if material.modes.values().any(|mode| mode != "100644") {
        return Err(SkillError::SyncConflict);
    }
    let workspace = material
        .files
        .get("skills/workspace.meta.yaml")
        .map(|bytes| serde_yaml::from_slice::<WorkspaceSkillMeta>(bytes))
        .transpose()
        .map_err(|_| SkillError::SyncConflict)?;
    let mut receipts = BTreeMap::new();
    let mut active_slugs = BTreeSet::new();
    let mut archived_slugs = BTreeSet::new();
    for (path, bytes) in &material.files {
        if let Some(id) = receipt_id_from_path(path) {
            let receipt: SkillReceipt =
                serde_yaml::from_slice(bytes).map_err(|_| SkillError::SyncConflict)?;
            receipts.insert(id, receipt);
            continue;
        }
        let components: Vec<_> = path.split('/').collect();
        match components.as_slice() {
            ["skills", slug, ..] if *slug != "workspace.meta.yaml" && *slug != "receipts" => {
                active_slugs.insert(SkillSlug::new(slug).map_err(|_| SkillError::SyncConflict)?);
            }
            ["archive", "skills", slug, ..] => {
                archived_slugs.insert(SkillSlug::new(slug).map_err(|_| SkillError::SyncConflict)?);
            }
            _ => {}
        }
    }
    let active_skills = active_slugs
        .into_iter()
        .map(|slug| parse_skill_object(&material, &slug, false).map(|skill| (slug, skill)))
        .collect::<Result<_, _>>()?;
    let archived_skills = archived_slugs
        .into_iter()
        .map(|slug| parse_skill_object(&material, &slug, true).map(|skill| (slug, skill)))
        .collect::<Result<_, _>>()?;
    Ok(SkillRepositorySnapshot {
        workspace,
        active_skills,
        archived_skills,
        receipts,
        active_users,
        conflict_checkpoint: None,
        repository_files: material.files,
    })
}

fn parse_skill_object(
    material: &TreeMaterial,
    slug: &SkillSlug,
    archived: bool,
) -> Result<SkillObjectSnapshot, SkillError> {
    let root = if archived {
        format!("archive/skills/{}", slug.as_str())
    } else {
        format!("skills/{}", slug.as_str())
    };
    let meta: SkillMeta = parse_yaml(material, &format!("{root}/skill.meta.yaml"))?;
    let history = String::from_utf8(
        material
            .files
            .get(&format!("{root}/history.thread"))
            .cloned()
            .ok_or(SkillError::SyncConflict)?,
    )
    .map_err(|_| SkillError::SyncConflict)?;
    let prefix = format!("{root}/");
    let mut revision_ids = BTreeSet::new();
    let mut publication_ids = BTreeSet::new();
    let mut proposal_ids = BTreeSet::new();
    for path in material
        .files
        .keys()
        .filter(|path| path.starts_with(&prefix))
    {
        let parts: Vec<_> = path[prefix.len()..].split('/').collect();
        match parts.as_slice() {
            ["revisions", id, ..] => {
                revision_ids.insert(RevisionId::new(id).map_err(|_| SkillError::SyncConflict)?);
            }
            ["publications", file] => {
                let id = file
                    .strip_suffix(".meta.yaml")
                    .ok_or(SkillError::SyncConflict)?;
                publication_ids.insert(RevisionId::new(id).map_err(|_| SkillError::SyncConflict)?);
            }
            ["proposals", id, ..] => {
                proposal_ids.insert(ProposalId::new(id).map_err(|_| SkillError::SyncConflict)?);
            }
            _ => {}
        }
    }
    let revisions = revision_ids
        .into_iter()
        .map(|id| {
            let revision_root = format!("{root}/revisions/{}", id.as_str());
            let meta: SkillRevisionMeta =
                parse_yaml(material, &format!("{revision_root}/revision.meta.yaml"))?;
            let package_prefix = format!("{revision_root}/package/");
            let entries = material
                .files
                .iter()
                .filter_map(|(path, bytes)| {
                    path.strip_prefix(&package_prefix).map(|relative| {
                        let kind = match material.modes.get(path).map(String::as_str) {
                            Some("100644") => PackageEntryKind::Regular,
                            Some("120000") => PackageEntryKind::Symlink,
                            _ => PackageEntryKind::BlockDevice,
                        };
                        PackageEntry::with_kind(relative, bytes.clone(), kind)
                    })
                })
                .collect();
            let package = validate_package_entries(slug, entries)?;
            Ok((id, SkillRevisionSnapshot { meta, package }))
        })
        .collect::<Result<_, SkillError>>()?;
    let publications = publication_ids
        .into_iter()
        .map(|id| {
            let value = parse_yaml(
                material,
                &format!("{root}/publications/{}.meta.yaml", id.as_str()),
            )?;
            Ok((id, value))
        })
        .collect::<Result<BTreeMap<_, SkillPublicationMeta>, SkillError>>()?;
    let proposals = proposal_ids
        .into_iter()
        .map(|id| {
            let proposal_root = format!("{root}/proposals/{}", id.as_str());
            let meta: SkillProposalMeta =
                parse_yaml(material, &format!("{proposal_root}/proposal.meta.yaml"))?;
            let discussion = String::from_utf8(
                material
                    .files
                    .get(&format!("{proposal_root}/discussion.thread"))
                    .cloned()
                    .ok_or(SkillError::SyncConflict)?,
            )
            .map_err(|_| SkillError::SyncConflict)?;
            Ok((id, SkillProposalSnapshot { meta, discussion }))
        })
        .collect::<Result<_, SkillError>>()?;
    Ok(SkillObjectSnapshot {
        meta,
        revisions,
        publications,
        proposals,
        history,
    })
}

fn parse_yaml<T: serde::de::DeserializeOwned>(
    material: &TreeMaterial,
    path: &str,
) -> Result<T, SkillError> {
    serde_yaml::from_slice(material.files.get(path).ok_or(SkillError::SyncConflict)?)
        .map_err(|_| SkillError::SyncConflict)
}

fn semantic_oids(
    repo: &GitStorage,
    commit: &str,
    request: &SkillMutationRequest,
    snapshot: &SkillRepositorySnapshot,
    context: &TransactionContext,
) -> Result<BTreeMap<String, String>, SkillSyncError> {
    let mut paths = BTreeSet::from([
        receipt_path(request.request_id()),
        "skills/workspace.meta.yaml".to_owned(),
        "users".to_owned(),
    ]);
    match request {
        SkillMutationRequest::Create(value) => add_skill_semantic_paths(&mut paths, &value.slug),
        SkillMutationRequest::Propose(value) => add_skill_semantic_paths(&mut paths, &value.slug),
        SkillMutationRequest::ProposalTransition(value) => {
            let slug = snapshot
                .active_skills
                .iter()
                .chain(snapshot.archived_skills.iter())
                .find(|(_, skill)| skill.proposals.contains_key(&value.proposal_id))
                .map(|(slug, _)| slug)
                .ok_or(SkillError::ProposalNotFound)?;
            add_skill_semantic_paths(&mut paths, slug);
        }
        SkillMutationRequest::Repair(value) => {
            if let SkillRepairScope::Skill(slug) = &value.scope {
                add_skill_semantic_paths(&mut paths, slug);
            }
        }
        SkillMutationRequest::WorkspaceBootstrap(_) => {
            paths.insert("skills".to_owned());
            paths.insert("archive/skills".to_owned());
        }
    }
    paths
        .into_iter()
        .map(|path| object_oid_at(repo, commit, &path, context).map(|oid| (path, oid)))
        .collect()
}

fn add_skill_semantic_paths(paths: &mut BTreeSet<String>, slug: &SkillSlug) {
    paths.insert(format!("skills/{}", slug.as_str()));
    paths.insert(format!("archive/skills/{}", slug.as_str()));
}

fn object_oid_at(
    repo: &GitStorage,
    commit: &str,
    path: &str,
    context: &TransactionContext,
) -> Result<String, SkillSyncError> {
    validate_relative_path(path)?;
    let literal = format!(":(literal){path}");
    let output = run_git(
        repo,
        &["ls-tree", "-z", "--full-tree", commit, "--", &literal],
        &[],
        context,
    )?;
    let entries = parse_tree_entries(&output.stdout)?;
    match entries.as_slice() {
        [] => Ok("absent".to_owned()),
        [entry] => Ok(format!(
            "{}:{}:{}",
            entry.mode, entry.object_type, entry.oid
        )),
        _ => Err(checkpoint_error("tree lookup returned multiple entries")),
    }
}

fn build_candidate(
    repo: &GitStorage,
    journal: &TransactionJournal,
    plan: &SkillMutationPlan,
    context: &TransactionContext,
) -> Result<String, SkillSyncError> {
    let base = journal
        .remote_tip
        .as_deref()
        .ok_or_else(|| checkpoint_error("prepared journal is missing remote tip"))?;
    let private_index = journal
        .private_index
        .to_str()
        .ok_or_else(|| checkpoint_error("private index path is not UTF-8"))?;
    let index_env = [("GIT_INDEX_FILE", private_index)];
    run_git(repo, &["read-tree", "--reset", base], &index_env, context)?;
    let root = journal
        .private_index
        .parent()
        .ok_or_else(|| checkpoint_error("private index has no parent"))?;
    for edit in &plan.edits {
        match edit {
            SkillTreeEdit::Upsert { path, bytes } => {
                validate_relative_path(path)?;
                let mut blob = tempfile::NamedTempFile::new_in(root)
                    .map_err(|error| checkpoint_io("create blob file", error))?;
                blob.write_all(bytes)
                    .and_then(|()| blob.flush())
                    .map_err(|error| checkpoint_io("write blob file", error))?;
                let blob_path = blob
                    .path()
                    .to_str()
                    .ok_or_else(|| checkpoint_error("blob path is not UTF-8"))?;
                let output = run_git(repo, &["hash-object", "-w", "--", blob_path], &[], context)?;
                let oid = parse_oid(&output.stdout)?;
                let cache = format!("100644,{oid},{path}");
                run_git(
                    repo,
                    &["update-index", "--add", "--cacheinfo", &cache],
                    &index_env,
                    context,
                )?;
            }
            SkillTreeEdit::Delete { path } => {
                validate_relative_path(path)?;
                run_git(
                    repo,
                    &["update-index", "--force-remove", "--", path],
                    &index_env,
                    context,
                )?;
            }
        }
    }
    let tree = parse_oid(&run_git(repo, &["write-tree"], &index_env, context)?.stdout)?;
    validate_identity(&journal.actor)?;
    validate_identity(&journal.author_email)?;
    let message = canonical_commit_message(&plan.commit_message, journal.request.request_id());
    let identity_env = [
        ("GIT_AUTHOR_NAME", journal.actor.as_str()),
        ("GIT_AUTHOR_EMAIL", journal.author_email.as_str()),
        ("GIT_COMMITTER_NAME", journal.actor.as_str()),
        ("GIT_COMMITTER_EMAIL", journal.author_email.as_str()),
    ];
    let output = run_git(
        repo,
        &["commit-tree", &tree, "-p", base, "-m", &message],
        &identity_env,
        context,
    )?;
    parse_oid(&output.stdout).map_err(Into::into)
}

fn canonical_commit_message(message: &str, request_id: &RequestId) -> String {
    let body = message
        .lines()
        .filter(|line| !line.starts_with("Gitim-Request-Id: "))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned();
    format!("{body}\n\nGitim-Request-Id: {}", request_id.as_str())
}

fn validate_candidate(
    repo: &GitStorage,
    candidate: &str,
    before: &SkillRepositorySnapshot,
    plan: &SkillMutationPlan,
    active_users: &BTreeSet<String>,
    context: &TransactionContext,
) -> Result<(), SkillSyncError> {
    let actual = load_snapshot(repo, candidate, active_users, context)?;
    if actual != plan.after {
        return Err(SkillError::SyncConflict.into());
    }
    let evidence = SkillCommitEvidence {
        commit_author: plan.commit_evidence.commit_author.clone(),
        request_trailer: plan.commit_evidence.request_trailer.clone(),
        parent_count: 1,
        receipt: plan.receipt.clone(),
        changed_paths: plan.changed_paths.clone(),
    };
    let parent = run_git(
        repo,
        &["show", "-s", "--format=%P", candidate],
        &[],
        context,
    )?;
    let parent = String::from_utf8(parent.stdout)
        .map_err(|_| checkpoint_error("candidate parent is not UTF-8"))?;
    parse_oid(parent.as_bytes())?;
    validate_skill_commit(before, &actual, &evidence)?;
    Ok(())
}

fn push_candidate(
    repo: &GitStorage,
    candidate: &str,
    branch: &str,
    context: &TransactionContext,
) -> Result<(), SkillSyncError> {
    validate_branch(branch)?;
    let refspec = format!("{candidate}:refs/heads/{branch}");
    let args = [
        GIT_HTTP_TIMEOUT_ARGS[0],
        GIT_HTTP_TIMEOUT_ARGS[1],
        GIT_HTTP_TIMEOUT_ARGS[2],
        GIT_HTTP_TIMEOUT_ARGS[3],
        "push",
        "origin",
        &refspec,
    ];
    let output = run_git_output(repo, &args, &[], context)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(classify_remote_error(&String::from_utf8_lossy(&output.stderr)).into())
    }
}

fn record_accepted_view(
    repo: &GitStorage,
    branch: &str,
    prior_tip: &str,
    commit: &str,
    context: &TransactionContext,
) -> Result<SkillLocalState, SkillSyncError> {
    let remote_ref = format!("refs/remotes/origin/{branch}");
    let updated = run_git_output(
        repo,
        &["update-ref", &remote_ref, commit, prior_tip],
        &[],
        context,
    )?;
    if !updated.status.success() {
        fetch(repo, context)?;
    }
    let store = SkillCheckpointStore::new(repo.root())?;
    let previous = store
        .load()?
        .unwrap_or_else(|| SkillValidationCheckpoint::empty(branch));
    let validation = validate_incoming_skill_history(repo, &previous, commit)?;
    store.save(&validation.checkpoint)?;
    let head = rev_parse(repo, "HEAD", context)?;
    let head_root = object_oid_at(repo, &head, "skills", context)?;
    let accepted_root = object_oid_at(repo, commit, "skills", context)?;
    Ok(if head_root == accepted_root {
        SkillLocalState::Current
    } else {
        SkillLocalState::PendingSync
    })
}

fn find_receipt_commit(
    repo: &GitStorage,
    tip: &str,
    receipt_path: &str,
    context: &TransactionContext,
) -> Result<String, SkillSyncError> {
    let mut search_tip = tip.to_owned();
    for _ in 0..MAX_EPOCH_DEPTH {
        let output = run_git(
            repo,
            &[
                "log",
                "-n",
                "1",
                "--format=%H",
                "--diff-filter=A",
                &search_tip,
                "--",
                receipt_path,
            ],
            &[],
            context,
        )?;
        let found = parse_oid(&output.stdout)?;
        let parents = run_git(repo, &["show", "-s", "--format=%P", &found], &[], context)?;
        if !parents.stdout.iter().all(u8::is_ascii_whitespace) {
            return Ok(found);
        }
        let Some(bytes) = read_optional_blob(repo, &found, "gitim.epoch.yaml", context)? else {
            return Ok(found);
        };
        let epoch: EpochFile = serde_yaml::from_slice(&bytes).map_err(|error| {
            SkillSyncError::EpochValidationBlocked(format!("parse epoch metadata: {error}"))
        })?;
        let Some(snapshot) = epoch.snapshot else {
            return Ok(found);
        };
        if read_optional_blob(repo, &snapshot.source_commit, receipt_path, context)?.is_none() {
            return Ok(found);
        }
        search_tip = snapshot.source_commit;
    }
    Err(SkillSyncError::EpochValidationBlocked(
        "receipt history exceeds maximum epoch depth".to_owned(),
    ))
}

fn attach_repair_checkpoint(
    repo: &GitStorage,
    snapshot: &mut SkillRepositorySnapshot,
    request: &SkillMutationRequest,
    remote_tip: &str,
    context: &TransactionContext,
) -> Result<(), SkillSyncError> {
    let SkillMutationRequest::Repair(request) = request else {
        return Ok(());
    };
    let checkpoint = SkillCheckpointStore::new(repo.root())?
        .load()?
        .ok_or(SkillError::SyncConflict)?;
    if checkpoint.last_scanned_tip != remote_tip {
        return Err(SkillError::SyncConflict.into());
    }
    let (key, slug) = match &request.scope {
        SkillRepairScope::Workspace => ("$workspace", None),
        SkillRepairScope::Skill(slug) => (slug.as_str(), Some(slug)),
    };
    let conflict = checkpoint
        .conflicts
        .get(key)
        .ok_or(SkillError::SyncConflict)?;
    if conflict.rejected_commit != request.conflict_tip
        || conflict.accepted_tree_oid.as_deref() != Some(request.accepted_tree.as_str())
    {
        return Err(SkillError::SyncConflict.into());
    }

    let (accepted_state, accepted_files, accepted_modes) = if let Some((commit, archived)) =
        accepted_scope_location(&checkpoint, slug)
    {
        let accepted_material = load_tree_material(repo, commit, context)?;
        let accepted_modes = accepted_material
            .modes
            .iter()
            .filter(|(path, _)| scope_contains(path, slug))
            .map(|(path, mode)| (path.clone(), mode.clone()))
            .collect();
        let accepted_snapshot = parse_snapshot(accepted_material, snapshot.active_users.clone())?;
        let accepted_state = if let Some(slug) = slug {
            let skill = if archived {
                accepted_snapshot
                    .archived_skills
                    .get(slug)
                    .cloned()
                    .ok_or(SkillError::SyncConflict)?
            } else {
                accepted_snapshot
                    .active_skills
                    .get(slug)
                    .cloned()
                    .ok_or(SkillError::SyncConflict)?
            };
            if archived {
                SkillRepairAcceptedState::ArchivedSkill {
                    slug: slug.clone(),
                    skill,
                }
            } else {
                SkillRepairAcceptedState::ActiveSkill {
                    slug: slug.clone(),
                    skill,
                }
            }
        } else {
            SkillRepairAcceptedState::Workspace(
                accepted_snapshot
                    .workspace
                    .clone()
                    .ok_or(SkillError::AdminUninitialized)?,
            )
        };
        let accepted_files = accepted_snapshot
            .repository_files
            .iter()
            .filter(|(path, _)| scope_contains(path, slug))
            .map(|(path, bytes)| (path.clone(), bytes.clone()))
            .collect();
        (accepted_state, accepted_files, accepted_modes)
    } else {
        let slug = slug.ok_or(SkillError::SyncConflict)?;
        (
            SkillRepairAcceptedState::AbsentSkill { slug: slug.clone() },
            BTreeMap::new(),
            BTreeMap::new(),
        )
    };

    let actual_material = load_tree_material(repo, remote_tip, context)?;
    let mut changed_paths: BTreeSet<String> = snapshot
        .repository_files
        .keys()
        .chain(accepted_files.keys())
        .filter(|path| scope_contains(path, slug))
        .filter(|path| snapshot.repository_files.get(*path) != accepted_files.get(*path))
        .cloned()
        .collect();
    let entry_changed_paths: BTreeSet<String> = actual_material
        .modes
        .keys()
        .chain(accepted_modes.keys())
        .filter(|path| scope_contains(path, slug))
        .filter(|path| actual_material.modes.get(*path) != accepted_modes.get(*path))
        .filter(|path| !changed_paths.contains(*path))
        .cloned()
        .collect();
    changed_paths.extend(entry_changed_paths.iter().cloned());
    changed_paths.extend(conflict.rejected_receipt_paths.iter().cloned());
    snapshot.conflict_checkpoint = Some(gitim_core::skill::SkillConflictCheckpoint {
        conflict_tip: conflict.rejected_commit.clone(),
        accepted_tree: request.accepted_tree.clone(),
        accepted_state,
        accepted_files,
        entry_changed_paths,
        rejected_receipt_paths: conflict.rejected_receipt_paths.clone(),
        changed_paths,
    });
    Ok(())
}

fn accepted_scope_location<'a>(
    checkpoint: &'a SkillValidationCheckpoint,
    slug: Option<&SkillSlug>,
) -> Option<(&'a str, bool)> {
    match slug {
        None => checkpoint
            .workspace_tree
            .as_ref()
            .map(|tree| (tree.commit_oid.as_str(), false)),
        Some(slug) => checkpoint
            .skills
            .get(slug.as_str())
            .map(|state| (state.tree.commit_oid.as_str(), state.archived)),
    }
}

fn scope_contains(path: &str, slug: Option<&SkillSlug>) -> bool {
    match slug {
        None => path == "skills/workspace.meta.yaml",
        Some(slug) => {
            let active = format!("skills/{}", slug.as_str());
            let archived = format!("archive/skills/{}", slug.as_str());
            path == active
                || path.starts_with(&format!("{active}/"))
                || path == archived
                || path.starts_with(&format!("{archived}/"))
        }
    }
}

fn repair_checkpoint_fingerprint(
    repo: &GitStorage,
    request: &SkillMutationRequest,
) -> Result<Option<String>, SkillSyncError> {
    let SkillMutationRequest::Repair(repair) = request else {
        return Ok(None);
    };
    let checkpoint = SkillCheckpointStore::new(repo.root())?
        .load()?
        .ok_or(SkillError::SyncConflict)?;
    let (key, accepted): (&str, Option<&AcceptedTree>) = match &repair.scope {
        SkillRepairScope::Workspace => ("$workspace", checkpoint.workspace_tree.as_ref()),
        SkillRepairScope::Skill(slug) => (
            slug.as_str(),
            checkpoint
                .skills
                .get(slug.as_str())
                .map(|state: &AcceptedSkillState| &state.tree),
        ),
    };
    let conflict = checkpoint
        .conflicts
        .get(key)
        .ok_or(SkillError::SyncConflict)?;
    serde_json::to_string(&(
        checkpoint.active_epoch,
        checkpoint.last_scanned_tip,
        conflict,
        accepted,
    ))
    .map(Some)
    .map_err(|error| checkpoint_error(format!("serialize repair checkpoint: {error}")))
}

fn ensure_repair_checkpoint_unchanged(
    repo: &GitStorage,
    request: &SkillMutationRequest,
    semantic_oids: &BTreeMap<String, String>,
) -> Result<(), SkillSyncError> {
    let Some(expected) = semantic_oids.get("$checkpoint") else {
        return Ok(());
    };
    if repair_checkpoint_fingerprint(repo, request)?.as_ref() != Some(expected) {
        return Err(SkillError::SyncConflict.into());
    }
    Ok(())
}

fn fetch(repo: &GitStorage, context: &TransactionContext) -> Result<(), SkillSyncError> {
    let args = [
        GIT_HTTP_TIMEOUT_ARGS[0],
        GIT_HTTP_TIMEOUT_ARGS[1],
        GIT_HTTP_TIMEOUT_ARGS[2],
        GIT_HTTP_TIMEOUT_ARGS[3],
        "fetch",
        "origin",
    ];
    let output = run_git_output(repo, &args, &[], context)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(classify_remote_error(&String::from_utf8_lossy(&output.stderr)).into())
    }
}

fn current_branch(
    repo: &GitStorage,
    context: &TransactionContext,
) -> Result<String, SkillSyncError> {
    let output = run_git(
        repo,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        &[],
        context,
    )?;
    let branch = String::from_utf8(output.stdout)
        .map_err(|_| checkpoint_error("current branch is not UTF-8"))?
        .trim()
        .to_owned();
    validate_branch(&branch)?;
    Ok(branch)
}

fn resolve_active_remote(
    repo: &GitStorage,
    start_branch: &str,
    context: &TransactionContext,
) -> Result<(String, String), SkillSyncError> {
    let mut branch = start_branch.to_owned();
    for _ in 0..MAX_EPOCH_DEPTH {
        validate_branch(&branch)?;
        let tip = rev_parse(repo, &format!("refs/remotes/origin/{branch}"), context)?;
        let epoch = read_optional_blob(repo, &tip, "gitim.epoch.yaml", context)?
            .map(|bytes| {
                let epoch: EpochFile = serde_yaml::from_slice(&bytes).map_err(|error| {
                    SkillSyncError::EpochValidationBlocked(format!("parse epoch metadata: {error}"))
                })?;
                epoch.validate().map_err(|error| {
                    SkillSyncError::EpochValidationBlocked(format!(
                        "validate epoch metadata: {error}"
                    ))
                })?;
                Ok::<_, SkillSyncError>(epoch)
            })
            .transpose()?;
        match epoch {
            Some(epoch) if epoch.status == EpochStatus::Redirected => {
                branch = epoch
                    .redirect
                    .ok_or_else(|| {
                        SkillSyncError::EpochValidationBlocked(
                            "redirected epoch lacks redirect".to_owned(),
                        )
                    })?
                    .target_branch;
            }
            _ => return Ok((branch, tip)),
        }
    }
    Err(SkillSyncError::EpochValidationBlocked(
        "epoch lineage exceeds maximum depth".to_owned(),
    ))
}

fn read_optional_blob(
    repo: &GitStorage,
    commit: &str,
    path: &str,
    context: &TransactionContext,
) -> Result<Option<Vec<u8>>, SkillSyncError> {
    let oid = object_oid_at(repo, commit, path, context)?;
    if oid == "absent" {
        return Ok(None);
    }
    let oid = oid
        .rsplit(':')
        .next()
        .ok_or_else(|| checkpoint_error("malformed tree identity"))?;
    Ok(Some(
        run_git(repo, &["cat-file", "blob", oid], &[], context)?.stdout,
    ))
}

fn rev_parse(
    repo: &GitStorage,
    revision: &str,
    context: &TransactionContext,
) -> Result<String, SkillSyncError> {
    validate_revision(revision)?;
    let object = format!("{revision}^{{commit}}");
    let output = run_git(
        repo,
        &["rev-parse", "--verify", "--end-of-options", &object],
        &[],
        context,
    )?;
    parse_oid(&output.stdout).map_err(Into::into)
}

fn acquire_workspace_permit(
    root: &Path,
    deadline: Instant,
    capacity: usize,
) -> Result<WorkspacePermit, SkillSyncError> {
    static SEMAPHORES: OnceLock<WorkspaceSemaphores> = OnceLock::new();
    let key = (
        root.canonicalize()
            .map_err(|error| checkpoint_io("canonicalize workspace", error))?,
        capacity,
    );
    let semaphore = {
        let mut semaphores = SEMAPHORES
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(semaphore) = semaphores.get(&key).and_then(Weak::upgrade) {
            semaphore
        } else {
            let semaphore = Arc::new(WorkspaceSemaphore {
                available: Mutex::new(capacity),
                changed: Condvar::new(),
            });
            semaphores.insert(key, Arc::downgrade(&semaphore));
            semaphore
        }
    };
    let mut available = semaphore
        .available
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    while *available == 0 {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(GitError::Timeout(SKILL_TRANSACTION_TIMEOUT))?;
        let (next, timeout) = semaphore
            .changed
            .wait_timeout(available, remaining)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        available = next;
        if timeout.timed_out() && *available == 0 {
            return Err(GitError::Timeout(SKILL_TRANSACTION_TIMEOUT).into());
        }
    }
    *available -= 1;
    drop(available);
    Ok(WorkspacePermit { semaphore })
}

fn run_git(
    repo: &GitStorage,
    args: &[&str],
    envs: &[(&str, &str)],
    context: &TransactionContext,
) -> Result<Output, SkillSyncError> {
    let output = run_git_output(repo, args, envs, context)?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(GitError::CommandFailed(String::from_utf8_lossy(&output.stderr).into_owned()).into())
    }
}

fn run_git_output(
    repo: &GitStorage,
    args: &[&str],
    envs: &[(&str, &str)],
    context: &TransactionContext,
) -> Result<Output, SkillSyncError> {
    let remaining = context
        .deadline
        .checked_duration_since(Instant::now())
        .ok_or(GitError::Timeout(SKILL_TRANSACTION_TIMEOUT))?;
    let timeout = context.git_timeout.min(remaining);
    let mut command = Command::new(&context.git_program);
    command
        .args(args)
        .current_dir(repo.root())
        .env("LC_ALL", "C")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in envs {
        command.env(key, value);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // The child leads a process group so timeout cleanup also terminates
        // credential helpers and transport subprocesses spawned by Git.
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let child = command.spawn().map_err(GitError::Io)?;
    let pid = child.id();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = sender.send(child.wait_with_output());
    });
    match receiver.recv_timeout(timeout) {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => Err(GitError::Io(error).into()),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err(GitError::CommandFailed("git wait thread disconnected".to_owned()).into())
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            #[cfg(unix)]
            {
                // SAFETY: `pid` names the process-group leader created above.
                unsafe {
                    libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
                }
            }
            #[cfg(not(unix))]
            {
                let _ = Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/T", "/F"])
                    .output();
            }
            let _ = receiver.recv();
            Err(GitError::Timeout(timeout).into())
        }
    }
}

fn parse_tree_entries(bytes: &[u8]) -> Result<Vec<TreeEntry>, SkillSyncError> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(|record| {
            let tab = record
                .iter()
                .position(|byte| *byte == b'\t')
                .ok_or_else(|| checkpoint_error("malformed tree entry"))?;
            let metadata = std::str::from_utf8(&record[..tab])
                .map_err(|_| checkpoint_error("tree metadata is not UTF-8"))?;
            let path = std::str::from_utf8(&record[tab + 1..])
                .map_err(|_| checkpoint_error("tree path is not UTF-8"))?;
            let mut fields = metadata.split_ascii_whitespace();
            let mode = fields
                .next()
                .ok_or_else(|| checkpoint_error("tree entry is missing mode"))?;
            let object_type = fields
                .next()
                .ok_or_else(|| checkpoint_error("tree entry is missing type"))?;
            let oid = fields
                .next()
                .ok_or_else(|| checkpoint_error("tree entry is missing oid"))?;
            if fields.next().is_some() {
                return Err(checkpoint_error("tree entry has extra fields"));
            }
            validate_oid(oid)?;
            Ok(TreeEntry {
                mode: mode.to_owned(),
                object_type: object_type.to_owned(),
                oid: oid.to_owned(),
                path: path.to_owned(),
            })
        })
        .collect()
}

fn parse_oid(bytes: &[u8]) -> Result<String, GitError> {
    let oid = std::str::from_utf8(bytes)
        .map_err(|_| GitError::CommandFailed("object ID is not UTF-8".to_owned()))?
        .trim();
    if !matches!(oid.len(), 40 | 64) || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(GitError::CommandFailed(format!(
            "invalid object ID {oid:?}"
        )));
    }
    Ok(oid.to_owned())
}

fn validate_oid(oid: &str) -> Result<(), SkillSyncError> {
    parse_oid(oid.as_bytes()).map(|_| ()).map_err(Into::into)
}

fn validate_revision(revision: &str) -> Result<(), SkillSyncError> {
    if revision.is_empty() || revision.starts_with('-') || revision.contains(['\0', '\n', '\r']) {
        return Err(checkpoint_error("invalid revision"));
    }
    Ok(())
}

fn validate_branch(branch: &str) -> Result<(), SkillSyncError> {
    if branch.is_empty()
        || branch.starts_with(['-', '/', '.'])
        || branch.ends_with(['/', '.'])
        || branch.ends_with(".lock")
        || branch.starts_with("refs/")
        || branch.contains("..")
        || branch.contains("//")
        || branch.contains("@{")
        || branch.contains(['\0', '\n', '\r', '\\', ' ', '~', '^', ':', '?', '*', '['])
    {
        return Err(checkpoint_error("invalid branch name"));
    }
    Ok(())
}

fn validate_identity(value: &str) -> Result<(), SkillSyncError> {
    if value.is_empty() || value.contains(['\0', '\n', '\r']) {
        return Err(checkpoint_error("invalid Git identity"));
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<(), SkillSyncError> {
    let candidate = Path::new(path);
    if path.is_empty()
        || path.contains('\0')
        || candidate.is_absolute()
        || candidate
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(checkpoint_error("invalid transaction path"));
    }
    Ok(())
}

fn receipt_path(id: &RequestId) -> String {
    format!("{RECEIPT_ROOT}/{}.meta.yaml", id.as_str())
}

fn receipt_id_from_path(path: &str) -> Option<RequestId> {
    let file = path.strip_prefix(&format!("{RECEIPT_ROOT}/"))?;
    if file.contains('/') {
        return None;
    }
    RequestId::new(file.strip_suffix(".meta.yaml")?).ok()
}

fn ensure_before_deadline(context: &TransactionContext) -> Result<(), SkillSyncError> {
    if Instant::now() >= context.deadline {
        return Err(GitError::Timeout(SKILL_TRANSACTION_TIMEOUT).into());
    }
    Ok(())
}

fn maybe_crash(
    context: &TransactionContext,
    point: SkillTransactionCrashPoint,
) -> Result<(), SkillSyncError> {
    if context.crash_after == Some(point) {
        return Err(checkpoint_error(format!(
            "injected transaction crash at {point:?}"
        )));
    }
    Ok(())
}

fn checkpoint_error(message: impl Into<String>) -> SkillSyncError {
    SkillSyncError::Checkpoint(message.into())
}

fn checkpoint_io(context: &str, error: std::io::Error) -> SkillSyncError {
    checkpoint_error(format!("{context}: {error}"))
}
