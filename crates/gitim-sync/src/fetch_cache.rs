//! Shared pull-fetch publication order:
//!
//! ```text
//! private remote snapshot
//!         |
//!         v
//! immutable generation refs in bare cache
//!         |
//!         v
//! validate published manifest and commit tips
//!         |
//!         v
//! atomic state replacement selects generation
//!         |
//!         v
//! followers import selected generation
//!         |
//!         v
//! inactive generations cleaned best effort
//! ```

#![allow(dead_code)]

use crate::git::{GitError, GitObjectFormat, GitStorage};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(test)]
thread_local! {
    static FETCH_FOR_PULL_ENTRY_COUNT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_fetch_for_pull_entry_count() {
    FETCH_FOR_PULL_ENTRY_COUNT.set(0);
}

#[cfg(test)]
pub(crate) fn fetch_for_pull_entry_count() -> u64 {
    FETCH_FOR_PULL_ENTRY_COUNT.get()
}

const CACHE_SCHEMA_VERSION: u32 = 1;
const CACHE_LOCK_FILE: &str = "fetch-cache.lock";
const CACHE_STATE_FILE: &str = "fetch-cache-state.json";
const CACHE_REPOSITORY_DIR: &str = "fetch-cache.git";
const CACHE_SHADOW_HEADS_PREFIX: &str = "refs/gitim-fetch-cache/remote/heads/";
const LOCK_WAIT_TIMEOUT: Duration = Duration::from_secs(1);
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(25);
const MAX_CONTENTION_SHORT_RETRIES: u8 = 3;
const RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(120);
const MIN_TRANSIENT_COOLDOWN: Duration = Duration::from_secs(3);
const STANDARD_FETCH_REFSPEC: &str = "+refs/heads/*:refs/remotes/origin/*";
const AUTH_FAILURE_COOLDOWN: Duration = Duration::from_secs(300);

#[derive(Debug, Clone)]
pub(crate) struct SyncCacheProgress {
    interval: Duration,
    applied_generation: Option<u64>,
    disabled: bool,
    contention_short_retries: u8,
}

impl SyncCacheProgress {
    pub(crate) fn new(interval_secs: u32) -> Self {
        Self {
            interval: Duration::from_secs(u64::from(interval_secs)),
            applied_generation: None,
            disabled: false,
            contention_short_retries: 0,
        }
    }

    fn contention_neutral_hint(&mut self) -> CacheNeutralHint {
        if self.contention_short_retries < MAX_CONTENTION_SHORT_RETRIES {
            self.contention_short_retries = self.contention_short_retries.saturating_add(1);
            CacheNeutralHint::RetryContention
        } else {
            self.contention_short_retries = 0;
            CacheNeutralHint::PreserveSchedule
        }
    }

    pub(crate) fn reset_contention_retries(&mut self) {
        self.contention_short_retries = 0;
    }
}

#[derive(Debug)]
pub(crate) enum PullFetchResult {
    Ready,
    NeutralSkip(CacheNeutralHint),
    RemoteError(GitError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheNeutralHint {
    PreserveSchedule,
    RetryContention,
}

#[derive(Debug, Clone)]
struct CacheContext {
    workspace: PathBuf,
    runtime_dir: PathBuf,
    cache_repository: PathBuf,
    state_file: PathBuf,
    lock_file: PathBuf,
    remote_identity: String,
    config_revision: ConfigRevision,
    object_format: GitObjectFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ConfigRevision {
    modified_unix_nanos: u64,
    length: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AttemptClass {
    Success,
    AuthFailed,
    RateLimited,
    TransientFailure,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CacheState {
    schema_version: u32,
    remote_identity: String,
    config_revision: ConfigRevision,
    generation: u64,
    manifest: BTreeMap<String, String>,
    completed_at_unix_ms: u64,
    attempt: AttemptClass,
    retry_after_unix_ms: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
enum CacheInfraError {
    #[error("cache state I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("cache state JSON failed: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
#[derive(Clone, Copy, Default)]
struct PublicationHooks {
    before_state_replace: Option<fn() -> Result<(), CacheInfraError>>,
}

#[cfg(not(test))]
#[derive(Clone, Copy, Default)]
struct PublicationHooks {
    _private: (),
}

enum LockAttempt {
    Acquired(CacheLock),
    Contended,
    Failed(CacheInfraError),
}

struct CacheLock {
    file: std::fs::File,
}

impl Drop for CacheLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[derive(Deserialize)]
struct WorkspaceConfigView {
    workspace: PathBuf,
    git: WorkspaceGitView,
}

#[derive(Deserialize)]
struct WorkspaceGitView {
    provider: String,
    remote_url: Option<String>,
    token: Option<String>,
}

#[derive(Deserialize)]
struct MeView {
    handler: String,
}

fn discover(repo: &GitStorage) -> Option<CacheContext> {
    let clone_root = repo.root().canonicalize().ok()?;
    let workspace = find_workspace(&clone_root)?.canonicalize().ok()?;
    let runtime_dir = workspace.join(".gitim-runtime");
    if !std::fs::symlink_metadata(&runtime_dir)
        .ok()?
        .file_type()
        .is_dir()
        || runtime_dir.canonicalize().ok()? != runtime_dir
    {
        return None;
    }
    validate_managed_layout(&workspace, &clone_root)?;

    let config_path = runtime_dir.join("config.json");
    let config: WorkspaceConfigView =
        serde_json::from_slice(&std::fs::read(&config_path).ok()?).ok()?;
    if config.workspace.canonicalize().ok()? != workspace || config.git.provider != "github" {
        return None;
    }

    let configured_remote = config.git.remote_url?.trim().to_string();
    let configured_token = config.git.token?;
    if configured_remote.is_empty() || configured_token.is_empty() {
        return None;
    }
    let remote_identity = normalize_config_remote(&configured_remote)?;
    let raw_origin = repo.raw_origin_url().ok()?;
    let (origin_identity, origin_token) = parse_credentialed_origin(&raw_origin)?;
    if origin_identity != remote_identity || origin_token != configured_token {
        return None;
    }

    let fetch_refspecs = repo.origin_fetch_refspecs().ok()?;
    if fetch_refspecs.as_slice() != [STANDARD_FETCH_REFSPEC] {
        return None;
    }
    let object_format = repo.object_format().ok()?;

    let revision = config_revision(&config_path)?;
    Some(CacheContext {
        workspace,
        cache_repository: runtime_dir.join(CACHE_REPOSITORY_DIR),
        state_file: runtime_dir.join(CACHE_STATE_FILE),
        lock_file: runtime_dir.join(CACHE_LOCK_FILE),
        runtime_dir,
        remote_identity,
        config_revision: revision,
        object_format,
    })
}

fn find_workspace(clone_root: &Path) -> Option<PathBuf> {
    clone_root
        .ancestors()
        .find(|ancestor| ancestor.join(".gitim-runtime/config.json").is_file())
        .map(Path::to_path_buf)
}

fn validate_managed_layout(workspace: &Path, clone_root: &Path) -> Option<()> {
    let human = workspace.join(".gitim-runtime/human").canonicalize().ok();
    if human.as_deref() == Some(clone_root) {
        return Some(());
    }

    if clone_root.parent()? != workspace {
        return None;
    }
    let handler = clone_root.file_name()?.to_str()?;
    let gitim_dir = clone_root.join(".gitim");
    if !gitim_dir.join("config.yaml").is_file() {
        return None;
    }
    let me: MeView =
        serde_json::from_slice(&std::fs::read(gitim_dir.join("me.json")).ok()?).ok()?;
    (me.handler == handler).then_some(())
}

fn normalize_config_remote(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.contains(['?', '#', '@']) {
        return None;
    }
    let remainder = strip_https_scheme(raw)?;
    normalize_github_repository(remainder)
}

fn parse_credentialed_origin(raw: &str) -> Option<(String, String)> {
    let raw = raw.trim();
    if raw.contains(['?', '#']) {
        return None;
    }
    let remainder = strip_https_scheme(raw)?;
    let (credentials, repository) = remainder.rsplit_once('@')?;
    if credentials.contains('/') {
        return None;
    }
    let (username, token) = credentials.split_once(':')?;
    if username.is_empty() || token.is_empty() {
        return None;
    }
    Some((normalize_github_repository(repository)?, token.to_string()))
}

fn strip_https_scheme(raw: &str) -> Option<&str> {
    let (scheme, remainder) = raw.split_at_checked(8)?;
    scheme.eq_ignore_ascii_case("https://").then_some(remainder)
}

fn normalize_github_repository(raw: &str) -> Option<String> {
    let mut segments = raw.split('/');
    let host = segments.next()?;
    let owner = segments.next()?;
    let repository = segments.next()?;
    if segments.next().is_some()
        || !host.eq_ignore_ascii_case("github.com")
        || owner.is_empty()
        || repository.is_empty()
    {
        return None;
    }
    let repository = repository.strip_suffix(".git").unwrap_or(repository);
    if repository.is_empty() {
        return None;
    }
    Some(format!(
        "github.com/{}/{}",
        owner.to_lowercase(),
        repository.to_lowercase()
    ))
}

fn config_revision(path: &Path) -> Option<ConfigRevision> {
    let metadata = path.metadata().ok()?;
    let modified = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some(ConfigRevision {
        modified_unix_nanos: u64::try_from(modified.as_nanos()).unwrap_or(u64::MAX),
        length: metadata.len(),
    })
}

fn unix_ms(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

fn success_is_fresh(state: &CacheState, now_ms: u64, interval: Duration) -> bool {
    state.attempt == AttemptClass::Success
        && state.completed_at_unix_ms <= now_ms
        && now_ms.saturating_sub(state.completed_at_unix_ms)
            < u64::try_from(interval.as_millis()).unwrap_or(u64::MAX)
}

fn failure_cooldown_active(state: &CacheState, now_ms: u64, revision: &ConfigRevision) -> bool {
    state.attempt != AttemptClass::Success
        && &state.config_revision == revision
        && state.completed_at_unix_ms <= now_ms
        && state
            .retry_after_unix_ms
            .is_some_and(|retry_after| retry_after > now_ms)
}

fn state_is_semantically_valid(state: &CacheState) -> bool {
    let retry_after_is_valid = match state.attempt {
        AttemptClass::Success => state.retry_after_unix_ms.is_none(),
        _ => state.retry_after_unix_ms.is_some(),
    };
    retry_after_is_valid
        && (state.generation != 0 || state.manifest.is_empty())
        && state.manifest.iter().all(|(ref_name, object_id)| {
            ref_name
                .strip_prefix(CACHE_SHADOW_HEADS_PREFIX)
                .is_some_and(|branch| !branch.is_empty())
                && matches!(object_id.len(), 40 | 64)
                && object_id.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}

fn retry_after(error: &GitError, now_ms: u64, interval: Duration) -> u64 {
    let cooldown = match error {
        GitError::AuthFailed(_) => AUTH_FAILURE_COOLDOWN,
        GitError::RateLimited => RATE_LIMIT_COOLDOWN,
        _ => interval.max(MIN_TRANSIENT_COOLDOWN),
    };
    now_ms.saturating_add(u64::try_from(cooldown.as_millis()).unwrap_or(u64::MAX))
}

fn read_state(path: &Path) -> Result<Option<CacheState>, CacheInfraError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn write_state_atomic(path: &Path, state: &CacheState) -> Result<(), CacheInfraError> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "cache state path has no parent",
        )
    })?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    serde_json::to_writer(&mut temporary, state)?;
    temporary.flush()?;
    temporary
        .persist(path)
        .map_err(|error| CacheInfraError::Io(error.error))?;
    Ok(())
}

fn acquire_lock(path: &Path) -> LockAttempt {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) => return LockAttempt::Failed(error.into()),
    };
    let started = Instant::now();
    loop {
        if started.elapsed() >= LOCK_WAIT_TIMEOUT {
            return LockAttempt::Contended;
        }
        match file.try_lock_exclusive() {
            Ok(()) => return LockAttempt::Acquired(CacheLock { file }),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                let elapsed = started.elapsed();
                if elapsed >= LOCK_WAIT_TIMEOUT {
                    return LockAttempt::Contended;
                }
                std::thread::sleep(LOCK_POLL_INTERVAL.min(LOCK_WAIT_TIMEOUT - elapsed));
            }
            Err(error) => return LockAttempt::Failed(error.into()),
        }
    }
}

fn log_cache_outcome(context: &CacheContext, outcome: &'static str, generation: Option<u64>) {
    tracing::debug!(
        workspace = %context.workspace.display(),
        outcome,
        generation = ?generation,
        "shared pull-fetch cache"
    );
}

fn cleanup_inactive_generations(context: &CacheContext, generation: u64) {
    if !GitStorage::cache_repository_is_valid(&context.cache_repository, context.object_format) {
        log_cache_outcome(context, "cleanup_invalid_cache", Some(generation));
        return;
    }
    if let Err(error) = GitStorage::cleanup_cache_generations(&context.cache_repository, generation)
    {
        tracing::debug!(
            workspace = %context.workspace.display(),
            outcome = "cleanup_failed",
            generation,
            error = %error,
            "shared pull-fetch cache"
        );
    }
}

fn validate_active_generation(context: &CacheContext, state: &CacheState) -> Result<(), GitError> {
    if !GitStorage::cache_repository_is_valid(&context.cache_repository, context.object_format) {
        return Err(GitError::CommandFailed(
            "fetch-cache repository validation failed".to_string(),
        ));
    }
    let manifest =
        GitStorage::cache_generation_manifest(&context.cache_repository, state.generation)?;
    if manifest != state.manifest {
        return Err(GitError::CommandFailed(
            "fetch-cache active generation manifest mismatch".to_string(),
        ));
    }
    GitStorage::cache_commit_tips_are_readable(&context.cache_repository, &manifest)?;
    Ok(())
}

pub(crate) fn fetch_for_pull(
    repo: &GitStorage,
    progress: &mut SyncCacheProgress,
) -> PullFetchResult {
    #[cfg(test)]
    FETCH_FOR_PULL_ENTRY_COUNT.with(|count| count.set(count.get().saturating_add(1)));

    let mut clock = SystemTime::now;
    fetch_for_pull_with_clock(repo, progress, &mut clock, PublicationHooks::default())
}

fn fetch_for_pull_at(
    repo: &GitStorage,
    progress: &mut SyncCacheProgress,
    now: SystemTime,
    hooks: PublicationHooks,
) -> PullFetchResult {
    let mut clock = || now;
    fetch_for_pull_with_clock(repo, progress, &mut clock, hooks)
}

fn fetch_for_pull_with_clock<F>(
    repo: &GitStorage,
    progress: &mut SyncCacheProgress,
    clock: &mut F,
    hooks: PublicationHooks,
) -> PullFetchResult
where
    F: FnMut() -> SystemTime,
{
    if progress.disabled {
        return direct_fallback(repo, progress, None);
    }
    let Some(context) = discover(repo) else {
        return direct_fallback(repo, progress, None);
    };
    let _lock = match acquire_lock(&context.lock_file) {
        LockAttempt::Acquired(cache_lock) => cache_lock,
        LockAttempt::Contended => {
            log_cache_outcome(&context, "lock_contended", None);
            return PullFetchResult::NeutralSkip(progress.contention_neutral_hint());
        }
        LockAttempt::Failed(error) => {
            tracing::debug!(
                workspace = %context.workspace.display(),
                outcome = "fallback",
                reason = "lock_failed",
                error = %error,
                "shared pull-fetch cache"
            );
            return direct_fallback(repo, progress, None);
        }
    };
    progress.reset_contention_retries();
    let decision_at = clock();
    match read_state(&context.state_file) {
        Ok(None) => {
            refresh_as_leader_with_hooks(repo, &context, None, progress, clock, hooks, false)
        }
        Ok(Some(state))
            if state.schema_version == CACHE_SCHEMA_VERSION
                && state.remote_identity == context.remote_identity
                && state_is_semantically_valid(&state) =>
        {
            let now_ms = unix_ms(decision_at);
            if failure_cooldown_active(&state, now_ms, &context.config_revision) {
                log_cache_outcome(&context, "cooldown_reuse", Some(state.generation));
                progress.reset_contention_retries();
                PullFetchResult::NeutralSkip(CacheNeutralHint::PreserveSchedule)
            } else if success_is_fresh(&state, now_ms, progress.interval) {
                match progress.applied_generation {
                    Some(applied_generation) if state.generation < applied_generation => {
                        tracing::debug!(
                            workspace = %context.workspace.display(),
                            outcome = "older_generation_ignored",
                            generation = state.generation,
                            applied_generation,
                            "shared pull-fetch cache"
                        );
                    }
                    Some(applied_generation) if state.generation == applied_generation => {
                        log_cache_outcome(&context, "fresh_reuse", Some(state.generation));
                    }
                    _ => {
                        if state.generation > 0 {
                            if let Err(error) = validate_active_generation(&context, &state) {
                                tracing::debug!(
                                    workspace = %context.workspace.display(),
                                    outcome = "repair",
                                    reason = "active_generation_invalid",
                                    generation = state.generation,
                                    error = %error,
                                    "shared pull-fetch cache"
                                );
                                return refresh_as_leader_with_hooks(
                                    repo,
                                    &context,
                                    Some(&state),
                                    progress,
                                    clock,
                                    hooks,
                                    false,
                                );
                            }
                        }
                        if !state.manifest.is_empty() {
                            match repo.import_cache_generation(
                                &context.cache_repository,
                                state.generation,
                            ) {
                                Ok(()) => {
                                    log_cache_outcome(
                                        &context,
                                        "follower_import",
                                        Some(state.generation),
                                    );
                                    cleanup_inactive_generations(&context, state.generation);
                                }
                                Err(error) => {
                                    tracing::debug!(
                                        workspace = %context.workspace.display(),
                                        outcome = "fallback",
                                        reason = "import_failed",
                                        generation = state.generation,
                                        error = %error,
                                        "shared pull-fetch cache"
                                    );
                                    return refresh_as_leader_with_hooks(
                                        repo,
                                        &context,
                                        Some(&state),
                                        progress,
                                        clock,
                                        hooks,
                                        true,
                                    );
                                }
                            }
                        }
                        progress.applied_generation = Some(state.generation);
                    }
                }
                PullFetchResult::Ready
            } else {
                refresh_as_leader_with_hooks(
                    repo,
                    &context,
                    Some(&state),
                    progress,
                    clock,
                    hooks,
                    false,
                )
            }
        }
        Ok(Some(_)) => {
            log_cache_outcome(&context, "fallback_invalid_state", None);
            direct_fallback(repo, progress, None)
        }
        Err(error) => {
            tracing::debug!(
                workspace = %context.workspace.display(),
                outcome = "fallback",
                reason = "state_read_failed",
                error = %error,
                "shared pull-fetch cache"
            );
            direct_fallback(repo, progress, None)
        }
    }
}

fn direct_fallback(
    repo: &GitStorage,
    progress: &mut SyncCacheProgress,
    trustworthy_generation: Option<u64>,
) -> PullFetchResult {
    progress.reset_contention_retries();
    match repo.fetch() {
        Ok(()) => {
            if let Some(generation) = trustworthy_generation {
                progress.applied_generation = Some(generation);
            } else {
                progress.disabled = true;
            }
            PullFetchResult::Ready
        }
        Err(GitError::RemoteSlotBusy) => {
            PullFetchResult::NeutralSkip(CacheNeutralHint::PreserveSchedule)
        }
        Err(error) => PullFetchResult::RemoteError(error),
    }
}

fn refresh_as_leader_with_hooks<F>(
    repo: &GitStorage,
    context: &CacheContext,
    previous: Option<&CacheState>,
    progress: &mut SyncCacheProgress,
    clock: &mut F,
    hooks: PublicationHooks,
    force_repair: bool,
) -> PullFetchResult
where
    F: FnMut() -> SystemTime,
{
    log_cache_outcome(
        context,
        "leader_refresh",
        previous.map(|state| state.generation),
    );
    let active_generation_invalid = previous.is_some_and(|state| {
        state.generation > 0 && validate_active_generation(context, state).is_err()
    });
    let repair_incomplete_cache = active_generation_invalid || force_repair;
    let trustworthy_previous_generation = previous
        .filter(|_| !repair_incomplete_cache)
        .map(|state| state.generation);
    if let Err(error) = repo.fetch_cache_shadow() {
        if matches!(error, GitError::RemoteSlotBusy) {
            log_cache_outcome(
                context,
                "remote_slot_busy",
                previous.map(|state| state.generation),
            );
            progress.reset_contention_retries();
            return PullFetchResult::NeutralSkip(CacheNeutralHint::PreserveSchedule);
        }
        let completed_at = clock();
        if let Err(persist_error) = publish_failure(
            context,
            previous,
            &error,
            progress.interval,
            unix_ms(completed_at),
        ) {
            tracing::debug!(
                workspace = %context.workspace.display(),
                outcome = "cooldown_persist_failed",
                error = %persist_error,
                "shared pull-fetch cache"
            );
        }
        tracing::debug!(
            workspace = %context.workspace.display(),
            outcome = "leader_remote_error",
            generation = ?previous.map(|state| state.generation),
            error = %error,
            "shared pull-fetch cache"
        );
        return PullFetchResult::RemoteError(error);
    }
    let manifest = match repo.cache_shadow_manifest() {
        Ok(manifest) => manifest,
        Err(error) => {
            tracing::debug!(
                workspace = %context.workspace.display(),
                outcome = "fallback",
                reason = "manifest_failed",
                generation = ?previous.map(|state| state.generation),
                error = %error,
                "shared pull-fetch cache"
            );
            return direct_fallback(repo, progress, trustworthy_previous_generation);
        }
    };
    let manifest_changed = previous.is_none_or(|state| state.manifest != manifest);
    let needs_publication = manifest_changed || repair_incomplete_cache;
    let generation = match previous {
        None if manifest.is_empty() => 0,
        None => 1,
        Some(state) if !needs_publication => state.generation,
        Some(state) => {
            let Some(generation) = state.generation.checked_add(1) else {
                log_cache_outcome(
                    context,
                    "fallback_generation_overflow",
                    Some(state.generation),
                );
                return direct_fallback(repo, progress, trustworthy_previous_generation);
            };
            generation
        }
    };
    if needs_publication {
        let cache_result = if repair_incomplete_cache {
            GitStorage::rebuild_bare_cache(&context.cache_repository, context.object_format)
        } else {
            GitStorage::ensure_bare_cache(&context.cache_repository, context.object_format)
        };
        if let Err(error) = cache_result {
            tracing::debug!(
                workspace = %context.workspace.display(),
                outcome = "fallback",
                reason = "cache_init_failed",
                generation,
                error = %error,
                "shared pull-fetch cache"
            );
            return direct_fallback(repo, progress, trustworthy_previous_generation);
        }
        if !manifest.is_empty() {
            if let Err(error) = repo.publish_cache_generation(&context.cache_repository, generation)
            {
                tracing::debug!(
                    workspace = %context.workspace.display(),
                    outcome = "fallback",
                    reason = "publication_failed",
                    generation,
                    error = %error,
                    "shared pull-fetch cache"
                );
                return direct_fallback(repo, progress, trustworthy_previous_generation);
            }
        }
    }
    let completed_at = clock();
    let state = CacheState {
        schema_version: CACHE_SCHEMA_VERSION,
        remote_identity: context.remote_identity.clone(),
        config_revision: context.config_revision.clone(),
        generation,
        manifest,
        completed_at_unix_ms: unix_ms(completed_at),
        attempt: AttemptClass::Success,
        retry_after_unix_ms: None,
    };
    #[cfg(test)]
    if let Some(before_state_replace) = hooks.before_state_replace {
        if let Err(error) = before_state_replace() {
            tracing::debug!(
                workspace = %context.workspace.display(),
                outcome = "fallback",
                reason = "state_replace_hook_failed",
                generation,
                error = %error,
                "shared pull-fetch cache"
            );
            return direct_fallback(repo, progress, trustworthy_previous_generation);
        }
    }
    let _ = hooks;
    if needs_publication && state.generation > 0 {
        if let Err(error) = validate_active_generation(context, &state) {
            tracing::debug!(
                workspace = %context.workspace.display(),
                outcome = "fallback",
                reason = "published_generation_invalid",
                generation,
                error = %error,
                "shared pull-fetch cache"
            );
            return direct_fallback(repo, progress, trustworthy_previous_generation);
        }
    }
    if let Err(error) = write_state_atomic(&context.state_file, &state) {
        tracing::debug!(
            workspace = %context.workspace.display(),
            outcome = "fallback",
            reason = "state_replace_failed",
            generation,
            error = %error,
            "shared pull-fetch cache"
        );
        return direct_fallback(repo, progress, trustworthy_previous_generation);
    }
    log_cache_outcome(
        context,
        if needs_publication {
            "leader_published"
        } else {
            "leader_unchanged"
        },
        Some(generation),
    );
    if !state.manifest.is_empty() && progress.applied_generation != Some(generation) {
        if let Err(error) = validate_active_generation(context, &state) {
            tracing::debug!(
                workspace = %context.workspace.display(),
                outcome = "fallback",
                reason = "active_generation_invalid",
                generation,
                error = %error,
                "shared pull-fetch cache"
            );
            return direct_fallback(repo, progress, None);
        }
        if let Err(error) = repo.import_cache_generation(&context.cache_repository, generation) {
            tracing::debug!(
                workspace = %context.workspace.display(),
                outcome = "fallback",
                reason = "leader_import_failed",
                generation,
                error = %error,
                "shared pull-fetch cache"
            );
            return direct_fallback(repo, progress, None);
        }
        log_cache_outcome(context, "leader_import", Some(generation));
        progress.applied_generation = Some(generation);
    } else {
        progress.applied_generation = Some(generation);
    }
    if generation > 0 {
        cleanup_inactive_generations(context, generation);
    }
    PullFetchResult::Ready
}

fn publish_failure(
    context: &CacheContext,
    previous: Option<&CacheState>,
    error: &GitError,
    interval: Duration,
    now_ms: u64,
) -> Result<(), CacheInfraError> {
    let attempt = match error {
        GitError::AuthFailed(_) => AttemptClass::AuthFailed,
        GitError::RateLimited => AttemptClass::RateLimited,
        _ => AttemptClass::TransientFailure,
    };
    let state = CacheState {
        schema_version: CACHE_SCHEMA_VERSION,
        remote_identity: context.remote_identity.clone(),
        config_revision: context.config_revision.clone(),
        generation: previous.map_or(0, |state| state.generation),
        manifest: previous.map_or_else(BTreeMap::new, |state| state.manifest.clone()),
        completed_at_unix_ms: now_ms,
        attempt,
        retry_after_unix_ms: Some(retry_after(error, now_ms, interval)),
    };
    write_state_atomic(&context.state_file, &state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{GitError, GitStorage};
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    const CONFIG_REMOTE: &str = "https://github.com/CiferaTeam/GitIM";
    const RAW_ORIGIN: &str = "https://x-access-token:test-pat-123@github.com/CiferaTeam/GitIM.git";
    const TEST_TOKEN: &str = "test-pat-123";

    struct WorkspaceFixture {
        root: tempfile::TempDir,
        workspace: PathBuf,
        clone_root: PathBuf,
        token: String,
    }

    impl WorkspaceFixture {
        fn human() -> Self {
            let root = tempfile::tempdir().expect("create temp directory");
            let workspace = root.path().join("workspace");
            let clone_root = workspace.join(".gitim-runtime/human");
            std::fs::create_dir_all(&clone_root).expect("create human clone directory");
            init_repository(&clone_root);

            let fixture = Self {
                root,
                workspace,
                clone_root,
                token: TEST_TOKEN.to_string(),
            };
            fixture.write_config("github", CONFIG_REMOTE, TEST_TOKEN);
            fixture.set_origin(RAW_ORIGIN);
            fixture
        }

        fn agent(handler: &str) -> Self {
            let root = tempfile::tempdir().expect("create temp directory");
            let workspace = root.path().join("workspace");
            let clone_root = workspace.join(handler);
            std::fs::create_dir_all(clone_root.join(".gitim"))
                .expect("create agent metadata directory");
            init_repository(&clone_root);
            std::fs::write(clone_root.join(".gitim/config.yaml"), "version: 1\n")
                .expect("write agent config");
            std::fs::write(
                clone_root.join(".gitim/me.json"),
                serde_json::to_vec(&json!({ "handler": handler })).expect("serialize me.json"),
            )
            .expect("write me.json");

            let fixture = Self {
                root,
                workspace,
                clone_root,
                token: TEST_TOKEN.to_string(),
            };
            fixture.write_config("github", CONFIG_REMOTE, TEST_TOKEN);
            fixture.set_origin(RAW_ORIGIN);
            fixture
        }

        fn write_config(&self, provider: &str, remote_url: &str, token: &str) {
            let runtime_dir = self.workspace.join(".gitim-runtime");
            std::fs::create_dir_all(&runtime_dir).expect("create runtime directory");
            let config = json!({
                "workspace": self.workspace,
                "created_at": "2026-07-31T00:00:00Z",
                "git": {
                    "provider": provider,
                    "remote_url": remote_url,
                    "token": token,
                    "github_email": "owner@example.com"
                },
                "future_field": true
            });
            std::fs::write(
                runtime_dir.join("config.json"),
                serde_json::to_vec_pretty(&config).expect("serialize workspace config"),
            )
            .expect("write workspace config");
        }

        fn set_origin(&self, raw_url: &str) {
            git_config(&self.clone_root, &["remote.origin.url", raw_url]);
            git_config(
                &self.clone_root,
                &["remote.origin.fetch", STANDARD_FETCH_REFSPEC],
            );
        }

        fn add_fetch_refspec(&self, refspec: &str) {
            git_config(&self.clone_root, &["--add", "remote.origin.fetch", refspec]);
        }

        fn storage(&self) -> GitStorage {
            GitStorage::new(&self.clone_root)
        }
    }

    fn init_repository(path: &Path) {
        let output = Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(path)
            .output()
            .expect("run git init");
        assert!(output.status.success(), "git init failed");
    }

    fn git_config(path: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("config")
            .args(args)
            .current_dir(path)
            .output()
            .expect("run git config");
        assert!(output.status.success(), "git config failed");
    }

    fn state(
        attempt: AttemptClass,
        revision: ConfigRevision,
        completed_at_unix_ms: u64,
        retry_after_unix_ms: Option<u64>,
    ) -> CacheState {
        CacheState {
            schema_version: CACHE_SCHEMA_VERSION,
            remote_identity: "github.com/ciferateam/gitim".to_string(),
            config_revision: revision,
            generation: 7,
            manifest: BTreeMap::from([(
                format!("{CACHE_SHADOW_HEADS_PREFIX}main"),
                "0123456789012345678901234567890123456789".to_string(),
            )]),
            completed_at_unix_ms,
            attempt,
            retry_after_unix_ms,
        }
    }

    #[test]
    fn discovery_accepts_exact_human_layout() {
        let fixture = WorkspaceFixture::human();
        let context = discover(&fixture.storage()).expect("human clone should be eligible");

        assert_eq!(
            context.workspace,
            fixture
                .workspace
                .canonicalize()
                .expect("canonical workspace")
        );
        assert_eq!(
            context.cache_repository,
            context
                .workspace
                .join(".gitim-runtime")
                .join(CACHE_REPOSITORY_DIR)
        );
        assert_eq!(context.remote_identity, "github.com/ciferateam/gitim");
        assert_eq!(fixture.token, TEST_TOKEN);
        assert!(fixture.root.path().exists());
    }

    #[test]
    fn discovery_accepts_exact_agent_layout_with_matching_handler() {
        let fixture = WorkspaceFixture::agent("alice");
        let context = discover(&fixture.storage()).expect("agent clone should be eligible");

        assert_eq!(
            context.runtime_dir,
            context.workspace.join(".gitim-runtime")
        );
        assert_eq!(
            context.state_file,
            context.runtime_dir.join(CACHE_STATE_FILE)
        );
        assert_eq!(context.lock_file, context.runtime_dir.join(CACHE_LOCK_FILE));
    }

    #[test]
    fn discovery_falls_back_for_local_provider() {
        let fixture = WorkspaceFixture::human();
        fixture.write_config("local", CONFIG_REMOTE, TEST_TOKEN);

        assert!(discover(&fixture.storage()).is_none());
    }

    #[test]
    fn discovery_falls_back_for_canonical_workspace_mismatch() {
        let fixture = WorkspaceFixture::human();
        let other_workspace = fixture.root.path().join("other-workspace");
        std::fs::create_dir_all(&other_workspace).expect("create other workspace");
        let config = json!({
            "workspace": other_workspace,
            "git": {
                "provider": "github",
                "remote_url": CONFIG_REMOTE,
                "token": TEST_TOKEN
            }
        });
        std::fs::write(
            fixture.workspace.join(".gitim-runtime/config.json"),
            serde_json::to_vec(&config).expect("serialize config"),
        )
        .expect("write config");

        assert!(discover(&fixture.storage()).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn discovery_rejects_runtime_directory_symlink() {
        use std::os::unix::fs::symlink;

        let fixture = WorkspaceFixture::agent("alice");
        let runtime_dir = fixture.workspace.join(".gitim-runtime");
        let external_runtime = fixture.root.path().join("external-runtime");
        std::fs::rename(&runtime_dir, &external_runtime).expect("move runtime directory");
        symlink(&external_runtime, &runtime_dir).expect("link external runtime directory");

        assert!(discover(&fixture.storage()).is_none());
    }

    #[test]
    fn discovery_falls_back_for_missing_or_malformed_config() {
        let fixture = WorkspaceFixture::human();
        let config_path = fixture.workspace.join(".gitim-runtime/config.json");
        std::fs::remove_file(&config_path).expect("remove config");
        assert!(discover(&fixture.storage()).is_none());

        std::fs::write(config_path, b"{malformed").expect("write malformed config");
        assert!(discover(&fixture.storage()).is_none());
    }

    #[test]
    fn discovery_falls_back_for_nested_clone() {
        let fixture = WorkspaceFixture::human();
        let nested = fixture.workspace.join("nested/repository");
        std::fs::create_dir_all(&nested).expect("create nested repository");
        init_repository(&nested);
        git_config(&nested, &["remote.origin.url", RAW_ORIGIN]);
        git_config(&nested, &["remote.origin.fetch", STANDARD_FETCH_REFSPEC]);

        assert!(discover(&GitStorage::new(&nested)).is_none());
    }

    #[test]
    fn discovery_falls_back_for_agent_handler_or_config_mismatch() {
        let fixture = WorkspaceFixture::agent("alice");
        std::fs::write(
            fixture.clone_root.join(".gitim/me.json"),
            br#"{"handler":"bob"}"#,
        )
        .expect("write mismatched me.json");
        assert!(discover(&fixture.storage()).is_none());

        std::fs::write(
            fixture.clone_root.join(".gitim/me.json"),
            br#"{"handler":"alice"}"#,
        )
        .expect("restore me.json");
        std::fs::remove_file(fixture.clone_root.join(".gitim/config.yaml"))
            .expect("remove agent config");
        assert!(discover(&fixture.storage()).is_none());
    }

    #[test]
    fn discovery_falls_back_for_origin_repository_identity_mismatch() {
        let fixture = WorkspaceFixture::human();
        fixture.set_origin("https://x-access-token:test-pat-123@github.com/CiferaTeam/Other.git");

        assert!(discover(&fixture.storage()).is_none());
    }

    #[test]
    fn discovery_falls_back_for_origin_token_mismatch() {
        let fixture = WorkspaceFixture::human();
        fixture.set_origin("https://x-access-token:different-pat@github.com/CiferaTeam/GitIM.git");

        assert!(discover(&fixture.storage()).is_none());
    }

    #[test]
    fn discovery_rejects_multiple_origin_urls_in_any_order() {
        let mismatch = "https://x-access-token:test-pat-123@github.com/CiferaTeam/Other.git";

        let mismatch_then_match = WorkspaceFixture::human();
        git_config(
            &mismatch_then_match.clone_root,
            &["--unset-all", "remote.origin.url"],
        );
        git_config(
            &mismatch_then_match.clone_root,
            &["--add", "remote.origin.url", mismatch],
        );
        git_config(
            &mismatch_then_match.clone_root,
            &["--add", "remote.origin.url", RAW_ORIGIN],
        );
        assert!(discover(&mismatch_then_match.storage()).is_none());

        let match_then_mismatch = WorkspaceFixture::human();
        git_config(
            &match_then_mismatch.clone_root,
            &["--add", "remote.origin.url", mismatch],
        );
        assert!(discover(&match_then_mismatch.storage()).is_none());

        let duplicate = WorkspaceFixture::human();
        git_config(
            &duplicate.clone_root,
            &["--add", "remote.origin.url", RAW_ORIGIN],
        );
        assert!(discover(&duplicate.storage()).is_none());
    }

    #[test]
    fn discovery_rejects_empty_or_whitespace_padded_origin_url() {
        let empty = WorkspaceFixture::human();
        empty.set_origin("");
        assert!(discover(&empty.storage()).is_none());

        let padded = WorkspaceFixture::human();
        padded.set_origin(&format!(" {RAW_ORIGIN} "));

        assert!(discover(&padded.storage()).is_none());
    }

    #[test]
    fn discovery_requires_one_standard_fetch_refspec() {
        let fixture = WorkspaceFixture::human();
        git_config(&fixture.clone_root, &["--unset-all", "remote.origin.fetch"]);
        assert!(discover(&fixture.storage()).is_none());

        git_config(
            &fixture.clone_root,
            &[
                "remote.origin.fetch",
                "+refs/heads/main:refs/remotes/origin/main",
            ],
        );
        assert!(discover(&fixture.storage()).is_none());

        git_config(
            &fixture.clone_root,
            &["remote.origin.fetch", STANDARD_FETCH_REFSPEC],
        );
        fixture.add_fetch_refspec("+refs/heads/main:refs/remotes/upstream/main");
        assert!(discover(&fixture.storage()).is_none());
    }

    #[test]
    fn discovery_raw_origin_url_ignores_url_rewrites() {
        let fixture = WorkspaceFixture::human();
        git_config(
            &fixture.clone_root,
            &[
                "url.https://example.invalid/rewritten/.insteadOf",
                "https://x-access-token:test-pat-123@github.com/",
            ],
        );

        assert_eq!(
            fixture.storage().raw_origin_url().expect("read raw URL"),
            RAW_ORIGIN
        );
    }

    #[test]
    fn discovery_origin_fetch_refspecs_reject_multiple_values() {
        let fixture = WorkspaceFixture::human();
        fixture.add_fetch_refspec("+refs/pull/*/head:refs/remotes/origin/pull/*");

        assert!(fixture.storage().origin_fetch_refspecs().is_err());
        assert!(discover(&fixture.storage()).is_none());
    }

    #[test]
    fn discovery_rejects_an_explicit_empty_fetch_refspec() {
        let fixture = WorkspaceFixture::human();
        fixture.add_fetch_refspec("");

        assert!(fixture.storage().origin_fetch_refspecs().is_err());
        assert!(discover(&fixture.storage()).is_none());
    }

    #[test]
    fn discovery_rejects_whitespace_padded_fetch_refspec() {
        let fixture = WorkspaceFixture::human();
        git_config(
            &fixture.clone_root,
            &[
                "remote.origin.fetch",
                &format!(" {STANDARD_FETCH_REFSPEC} "),
            ],
        );

        assert!(fixture.storage().origin_fetch_refspecs().is_err());
        assert!(discover(&fixture.storage()).is_none());
    }

    #[test]
    fn state_future_success_timestamp_is_stale() {
        let revision = ConfigRevision {
            modified_unix_nanos: 11,
            length: 22,
        };
        let now_ms = unix_ms(UNIX_EPOCH + Duration::from_secs(100));
        let future = state(AttemptClass::Success, revision, now_ms + 1, None);

        assert!(!success_is_fresh(&future, now_ms, Duration::from_secs(3)));
    }

    #[test]
    fn state_failure_cooldown_requires_same_config_revision() {
        let revision = ConfigRevision {
            modified_unix_nanos: 11,
            length: 22,
        };
        let changed_revision = ConfigRevision {
            modified_unix_nanos: 12,
            length: 22,
        };
        let failed = state(
            AttemptClass::AuthFailed,
            revision.clone(),
            1_000,
            Some(301_000),
        );

        assert!(failure_cooldown_active(&failed, 2_000, &revision));
        assert!(!failure_cooldown_active(&failed, 2_000, &changed_revision));

        let future_failure = state(
            AttemptClass::TransientFailure,
            revision.clone(),
            2_001,
            Some(5_000),
        );
        assert!(!failure_cooldown_active(&future_failure, 2_000, &revision));
    }

    #[test]
    fn state_semantics_require_retry_after_only_for_failures() {
        let revision = ConfigRevision {
            modified_unix_nanos: 11,
            length: 22,
        };
        let success_with_retry = state(AttemptClass::Success, revision.clone(), 1_000, Some(2_000));
        let failure_without_retry = state(AttemptClass::AuthFailed, revision, 1_000, None);

        assert!(!state_is_semantically_valid(&success_with_retry));
        assert!(!state_is_semantically_valid(&failure_without_retry));
    }

    #[test]
    fn state_semantics_require_shadow_refs_and_full_git_object_ids() {
        let revision = ConfigRevision {
            modified_unix_nanos: 11,
            length: 22,
        };
        let mut invalid_ref = state(AttemptClass::Success, revision.clone(), 1_000, None);
        invalid_ref.manifest = BTreeMap::from([(
            "refs/heads/main".to_string(),
            "0123456789012345678901234567890123456789".to_string(),
        )]);
        let mut invalid_object = state(AttemptClass::Success, revision.clone(), 1_000, None);
        invalid_object.manifest = BTreeMap::from([(
            "refs/gitim-fetch-cache/remote/heads/main".to_string(),
            "not-a-git-object".to_string(),
        )]);
        let mut sha256_object = state(AttemptClass::Success, revision, 1_000, None);
        sha256_object.manifest = BTreeMap::from([(
            "refs/gitim-fetch-cache/remote/heads/main".to_string(),
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
        )]);

        assert!(!state_is_semantically_valid(&invalid_ref));
        assert!(!state_is_semantically_valid(&invalid_object));
        assert!(state_is_semantically_valid(&sha256_object));
    }

    #[test]
    fn state_json_never_contains_credentials() {
        let fixture = WorkspaceFixture::human();
        let context = discover(&fixture.storage()).expect("discover cache context");
        let mut state = state(
            AttemptClass::Success,
            context.config_revision.clone(),
            5_000,
            None,
        );
        state.remote_identity.clone_from(&context.remote_identity);

        write_state_atomic(&context.state_file, &state).expect("write cache state");
        let json = std::fs::read_to_string(&context.state_file).expect("read cache state");
        assert!(!json.contains(TEST_TOKEN));
        assert!(!json.contains(RAW_ORIGIN));
        assert_eq!(
            read_state(&context.state_file).expect("parse state"),
            Some(state)
        );
    }

    #[test]
    fn state_retry_after_matches_failure_class() {
        let now_ms = 10_000;
        assert_eq!(
            retry_after(
                &GitError::AuthFailed("redacted".to_string()),
                now_ms,
                Duration::from_secs(1)
            ),
            now_ms + 300_000
        );
        assert_eq!(
            retry_after(&GitError::RateLimited, now_ms, Duration::from_secs(1)),
            now_ms + RATE_LIMIT_COOLDOWN.as_millis() as u64
        );
        assert_eq!(
            retry_after(
                &GitError::CommandFailed("offline".to_string()),
                now_ms,
                Duration::from_secs(1)
            ),
            now_ms + MIN_TRANSIENT_COOLDOWN.as_millis() as u64
        );
        assert_eq!(
            retry_after(
                &GitError::Timeout(Duration::from_secs(120)),
                now_ms,
                Duration::from_secs(9)
            ),
            now_ms + 9_000
        );
    }

    #[test]
    fn state_sync_progress_starts_enabled_without_generation() {
        let progress = SyncCacheProgress::new(3);

        assert_eq!(progress.interval, Duration::from_secs(3));
        assert_eq!(progress.applied_generation, None);
        assert!(!progress.disabled);
    }

    #[test]
    fn state_contention_retry_hint_is_bounded() {
        let mut progress = SyncCacheProgress::new(3);

        for _ in 0..MAX_CONTENTION_SHORT_RETRIES {
            assert_eq!(
                progress.contention_neutral_hint(),
                CacheNeutralHint::RetryContention
            );
        }
        assert_eq!(
            progress.contention_neutral_hint(),
            CacheNeutralHint::PreserveSchedule
        );
        assert_eq!(
            progress.contention_neutral_hint(),
            CacheNeutralHint::RetryContention
        );

        progress.reset_contention_retries();
        for _ in 0..MAX_CONTENTION_SHORT_RETRIES {
            assert_eq!(
                progress.contention_neutral_hint(),
                CacheNeutralHint::RetryContention
            );
        }
        assert_eq!(
            progress.contention_neutral_hint(),
            CacheNeutralHint::PreserveSchedule
        );
    }

    #[test]
    fn state_read_missing_file_is_empty() {
        let temp = tempfile::tempdir().expect("create temp directory");

        assert_eq!(
            read_state(&temp.path().join(CACHE_STATE_FILE)).expect("read missing state"),
            None
        );
        assert_eq!(
            unix_ms(SystemTime::UNIX_EPOCH - Duration::from_millis(1)),
            0
        );
    }

    mod orchestration {
        use super::*;
        use fs2::FileExt;
        use std::cell::RefCell;
        use std::time::Instant;

        struct BoundaryExpectation {
            state_file: PathBuf,
            cache_repository: PathBuf,
            old_generation: u64,
            new_generation: u64,
            expected_main: String,
        }

        thread_local! {
            static BOUNDARY_EXPECTATION: RefCell<Option<BoundaryExpectation>> =
                const { RefCell::new(None) };
            static STATE_REPLACE_TAMPER: RefCell<Option<(PathBuf, u64)>> =
                const { RefCell::new(None) };
        }

        struct OrchestrationFixture {
            workspace: WorkspaceFixture,
            origin: PathBuf,
            seed: PathBuf,
        }

        impl OrchestrationFixture {
            fn new() -> Self {
                let workspace = WorkspaceFixture::human();
                let origin = workspace.root.path().join("origin.git");
                git_ok(
                    workspace.root.path(),
                    &["init", "--bare", "-b", "main", path_arg(&origin)],
                );
                let seed = workspace.root.path().join("seed");
                git_ok(
                    workspace.root.path(),
                    &["clone", path_arg(&origin), path_arg(&seed)],
                );
                git_ok(&seed, &["config", "user.name", "Cache Test"]);
                git_ok(&seed, &["config", "user.email", "cache@test.invalid"]);
                std::fs::write(seed.join("version.txt"), "one\n").expect("write seed file");
                git_ok(&seed, &["add", "version.txt"]);
                git_ok(&seed, &["commit", "-m", "seed remote"]);
                git_ok(&seed, &["push", "-u", "origin", "main"]);

                let rewrite_key = format!("url.file://{}.insteadOf", origin.display());
                git_config(&workspace.clone_root, &[rewrite_key.as_str(), RAW_ORIGIN]);

                Self {
                    workspace,
                    origin,
                    seed,
                }
            }

            fn storage(&self) -> GitStorage {
                self.workspace.storage()
            }

            fn context(&self) -> CacheContext {
                discover(&self.storage()).expect("discover orchestration fixture")
            }

            fn advance_remote(&self, contents: &str) {
                std::fs::write(self.seed.join("version.txt"), contents)
                    .expect("update remote fixture");
                git_ok(&self.seed, &["add", "version.txt"]);
                git_ok(&self.seed, &["commit", "-m", "advance remote"]);
                git_ok(&self.seed, &["push", "origin", "main"]);
            }

            fn add_agent(&self, handler: &str) -> GitStorage {
                let clone_root = self.workspace.workspace.join(handler);
                git_ok(
                    self.workspace.root.path(),
                    &["clone", path_arg(&self.origin), path_arg(&clone_root)],
                );
                let gitim_dir = clone_root.join(".gitim");
                std::fs::create_dir_all(&gitim_dir).expect("create agent metadata");
                std::fs::write(gitim_dir.join("config.yaml"), "version: 1\n")
                    .expect("write agent config");
                std::fs::write(
                    gitim_dir.join("me.json"),
                    serde_json::to_vec(&json!({ "handler": handler }))
                        .expect("serialize agent identity"),
                )
                .expect("write agent identity");
                git_config(&clone_root, &["remote.origin.url", RAW_ORIGIN]);
                let rewrite_key = format!("url.file://{}.insteadOf", self.origin.display());
                git_config(&clone_root, &[rewrite_key.as_str(), RAW_ORIGIN]);
                GitStorage::new(&clone_root)
            }

            fn add_empty_agent(&self, handler: &str) -> GitStorage {
                let clone_root = self.workspace.workspace.join(handler);
                std::fs::create_dir_all(&clone_root).expect("create empty agent clone");
                git_ok(&clone_root, &["init", "-b", "main"]);
                let gitim_dir = clone_root.join(".gitim");
                std::fs::create_dir_all(&gitim_dir).expect("create agent metadata");
                std::fs::write(gitim_dir.join("config.yaml"), "version: 1\n")
                    .expect("write agent config");
                std::fs::write(
                    gitim_dir.join("me.json"),
                    serde_json::to_vec(&json!({ "handler": handler }))
                        .expect("serialize agent identity"),
                )
                .expect("write agent identity");
                git_config(&clone_root, &["remote.origin.url", RAW_ORIGIN]);
                git_config(
                    &clone_root,
                    &["remote.origin.fetch", STANDARD_FETCH_REFSPEC],
                );
                let rewrite_key = format!("url.file://{}.insteadOf", self.origin.display());
                git_config(&clone_root, &[rewrite_key.as_str(), RAW_ORIGIN]);
                GitStorage::new(&clone_root)
            }

            fn fail_remote(&self, stderr: &str) {
                let script = self.workspace.root.path().join("failing-upload-pack.sh");
                std::fs::write(
                    &script,
                    format!("#!/bin/sh\nprintf '%s\\n' '{stderr}' >&2\nexit 1\n"),
                )
                .expect("write failing upload-pack");
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
                        .expect("make upload-pack executable");
                }
                git_config(
                    &self.workspace.clone_root,
                    &["remote.origin.uploadpack", path_arg(&script)],
                );
            }

            fn restore_remote(&self) {
                git_ok(
                    &self.workspace.clone_root,
                    &["config", "--unset-all", "remote.origin.uploadpack"],
                );
            }

            fn change_config_revision(&self) {
                let config_path = self.workspace.workspace.join(".gitim-runtime/config.json");
                let mut config: serde_json::Value = serde_json::from_slice(
                    &std::fs::read(&config_path).expect("read workspace config"),
                )
                .expect("parse workspace config");
                config["revision_nonce"] = json!("changed");
                std::fs::write(
                    config_path,
                    serde_json::to_vec_pretty(&config).expect("serialize changed config"),
                )
                .expect("write changed config");
            }
        }

        fn path_arg(path: &Path) -> &str {
            path.to_str().expect("test path is UTF-8")
        }

        fn git_ok(current_dir: &Path, args: &[&str]) {
            let output = Command::new("git")
                .args(args)
                .current_dir(current_dir)
                .output()
                .expect("run git command");
            assert!(
                output.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        fn cache_refs(path: &Path, prefix: &str) -> BTreeMap<String, String> {
            let output = Command::new("git")
                .args([
                    "for-each-ref",
                    "--format=%(refname)%00%(objectname)",
                    prefix,
                ])
                .current_dir(path)
                .output()
                .expect("list cache refs");
            assert!(
                output.status.success(),
                "list cache refs failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout)
                .expect("cache refs are UTF-8")
                .lines()
                .map(|line| {
                    let (name, object) = line.split_once('\0').expect("cache ref record");
                    (name.to_string(), object.to_string())
                })
                .collect()
        }

        fn git_stdout(current_dir: &Path, args: &[&str]) -> String {
            let output = Command::new("git")
                .args(args)
                .current_dir(current_dir)
                .output()
                .expect("run git command");
            assert!(
                output.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout)
                .expect("git stdout is UTF-8")
                .trim()
                .to_string()
        }

        fn clear_cache_object_database(cache_repository: &Path) {
            let objects = cache_repository.join("objects");
            std::fs::remove_dir_all(&objects).expect("remove cache object database");
            std::fs::create_dir_all(objects.join("info")).expect("recreate object info directory");
            std::fs::create_dir_all(objects.join("pack")).expect("recreate object pack directory");
        }

        fn preserve_only_cache_commit(cache_repository: &Path, object_id: &str) {
            use std::io::Write as _;
            use std::process::Stdio;

            let commit = Command::new("git")
                .args(["cat-file", "commit", object_id])
                .current_dir(cache_repository)
                .output()
                .expect("read cache commit object");
            assert!(
                commit.status.success(),
                "read cache commit failed: {}",
                String::from_utf8_lossy(&commit.stderr)
            );
            clear_cache_object_database(cache_repository);

            let mut child = Command::new("git")
                .args(["hash-object", "-t", "commit", "-w", "--stdin"])
                .current_dir(cache_repository)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("start cache commit restore");
            child
                .stdin
                .take()
                .expect("cache commit restore stdin")
                .write_all(&commit.stdout)
                .expect("write cache commit");
            let restored = child
                .wait_with_output()
                .expect("finish cache commit restore");
            assert!(
                restored.status.success(),
                "restore cache commit failed: {}",
                String::from_utf8_lossy(&restored.stderr)
            );
            assert_eq!(
                String::from_utf8(restored.stdout)
                    .expect("restored object ID is UTF-8")
                    .trim(),
                object_id
            );
        }

        #[cfg(unix)]
        struct UploadPackControl {
            counter_dir: PathBuf,
            started: std::fs::File,
            release: std::fs::File,
        }

        #[cfg(unix)]
        impl UploadPackControl {
            fn wait_for_remote_attempt(&mut self) {
                use std::io::Read as _;

                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    let mut byte = [0_u8; 1];
                    match self.started.read(&mut byte) {
                        Ok(1) if byte[0] == b'\n' => return,
                        Ok(0 | 1) => {}
                        Ok(_) => unreachable!("single-byte FIFO read"),
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(error) => panic!("read upload-pack start gate: {error}"),
                    }
                    assert!(
                        Instant::now() < deadline,
                        "timed out waiting for upload-pack start gate"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
            }

            fn release_remote_attempt(&mut self) {
                self.release
                    .write_all(b"go\n")
                    .expect("release upload-pack gate");
                self.release.flush().expect("flush upload-pack gate");
            }
        }

        #[cfg(unix)]
        fn create_fifo(path: &Path) {
            use std::ffi::CString;
            use std::os::unix::ffi::OsStrExt as _;

            let path = CString::new(path.as_os_str().as_bytes()).expect("FIFO path has no NUL");
            // SAFETY: `path` is a live, NUL-terminated C string and the mode is valid.
            let result = unsafe { libc::mkfifo(path.as_ptr(), 0o600) };
            assert_eq!(
                result,
                0,
                "create FIFO failed: {}",
                std::io::Error::last_os_error()
            );
        }

        #[cfg(unix)]
        fn configure_upload_pack_control(
            fixture: &OrchestrationFixture,
            repos: &[GitStorage],
        ) -> UploadPackControl {
            use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

            let counter_dir = fixture.workspace.root.path().join("upload-pack-requests");
            std::fs::create_dir_all(&counter_dir).expect("create upload-pack counter");
            let started_fifo = fixture
                .workspace
                .root
                .path()
                .join("upload-pack-started.fifo");
            let release_fifo = fixture
                .workspace
                .root
                .path()
                .join("upload-pack-release.fifo");
            create_fifo(&started_fifo);
            create_fifo(&release_fifo);
            let exec_path = git_stdout(fixture.workspace.root.path(), &["--exec-path"]);
            let upload_pack = PathBuf::from(exec_path).join("git-upload-pack");
            assert!(upload_pack.is_file(), "git-upload-pack must exist");

            let script = fixture.workspace.root.path().join("gated-upload-pack.sh");
            std::fs::write(
                &script,
                format!(
                    "#!/bin/sh\n\
                     set -eu\n\
                     suffix=0\n\
                     while ! mkdir {}/request-$$-$suffix 2>/dev/null; do\n\
                       suffix=$((suffix + 1))\n\
                     done\n\
                     printf 'started\\n' > {}\n\
                     IFS= read -r _ < {}\n\
                     exec {} \"$@\"\n",
                    shell_quote(&counter_dir),
                    shell_quote(&started_fifo),
                    shell_quote(&release_fifo),
                    shell_quote(&upload_pack)
                ),
            )
            .expect("write gated upload-pack");
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
                .expect("make gated upload-pack executable");

            for repo in repos {
                git_config(
                    repo.root(),
                    &["remote.origin.uploadpack", path_arg(&script)],
                );
            }

            let open_fifo = |path: &Path| {
                std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(path)
                    .expect("open upload-pack FIFO")
            };
            UploadPackControl {
                counter_dir,
                started: open_fifo(&started_fifo),
                release: open_fifo(&release_fifo),
            }
        }

        #[cfg(unix)]
        fn shell_quote(path: &Path) -> String {
            format!("'{}'", path_arg(path).replace('\'', "'\"'\"'"))
        }

        #[cfg(unix)]
        fn request_count(counter_dir: &Path) -> usize {
            std::fs::read_dir(counter_dir)
                .expect("read upload-pack counter")
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| name.starts_with("request-"))
                        && entry.file_type().is_ok_and(|file_type| file_type.is_dir())
                })
                .count()
        }

        #[cfg(unix)]
        fn run_concurrent_fetches(
            repos: &[GitStorage],
            progresses: &mut [SyncCacheProgress],
            now: SystemTime,
        ) -> Vec<(usize, PullFetchResult)> {
            assert_eq!(repos.len(), progresses.len());
            let indices = (0..repos.len()).collect::<Vec<_>>();
            let scheduled = indices
                .into_iter()
                .map(|index| (index, now))
                .collect::<Vec<_>>();
            run_scheduled_fetches(repos, progresses, &scheduled)
        }

        #[cfg(unix)]
        fn run_scheduled_fetches(
            repos: &[GitStorage],
            progresses: &mut [SyncCacheProgress],
            scheduled: &[(usize, SystemTime)],
        ) -> Vec<(usize, PullFetchResult)> {
            assert!(!scheduled.is_empty());
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(scheduled.len()));
            let handles = scheduled
                .iter()
                .map(|&(index, now)| {
                    let repo = repos[index].clone();
                    let mut progress = progresses[index].clone();
                    let barrier = std::sync::Arc::clone(&barrier);
                    std::thread::spawn(move || {
                        barrier.wait();
                        let result = fetch_for_pull_at(
                            &repo,
                            &mut progress,
                            now,
                            PublicationHooks::default(),
                        );
                        (index, progress, result)
                    })
                })
                .collect::<Vec<_>>();

            let mut results = Vec::with_capacity(handles.len());
            for handle in handles {
                let (index, updated, result) = handle.join().expect("join fetch-cache caller");
                progresses[index] = updated;
                results.push((index, result));
            }
            results
        }

        #[cfg(unix)]
        fn run_gated_contention_boundary(
            repos: &[GitStorage],
            progresses: &mut [SyncCacheProgress],
            control: &mut UploadPackControl,
            boundary_at: SystemTime,
            expected_requests: usize,
        ) {
            use crate::sync_loop::schedule_after_cache_neutral;

            let leader_repo = repos[0].clone();
            let mut leader_progress = progresses[0].clone();
            let leader = std::thread::spawn(move || {
                let result = fetch_for_pull_at(
                    &leader_repo,
                    &mut leader_progress,
                    boundary_at,
                    PublicationHooks::default(),
                );
                (leader_progress, result)
            });
            control.wait_for_remote_attempt();

            let followers = (1..repos.len())
                .map(|index| (index, boundary_at))
                .collect::<Vec<_>>();
            let follower_results = run_scheduled_fetches(repos, progresses, &followers);
            let mut pending = follower_results
                .into_iter()
                .map(|(index, result)| match result {
                    PullFetchResult::NeutralSkip(CacheNeutralHint::RetryContention) => {
                        (index, CacheNeutralHint::RetryContention, boundary_at)
                    }
                    other => panic!("follower {index} bypassed forced contention: {other:?}"),
                })
                .collect::<Vec<_>>();
            assert_eq!(pending.len(), repos.len() - 1);
            assert_eq!(request_count(&control.counter_dir), expected_requests);

            control.release_remote_attempt();
            let (updated_leader, leader_result) = leader.join().expect("join fetch-cache leader");
            assert_ready(leader_result);
            progresses[0] = updated_leader;

            for _ in 0..MAX_CONTENTION_SHORT_RETRIES {
                if pending.is_empty() {
                    break;
                }
                let scheduled = pending
                    .iter()
                    .map(|&(index, hint, previous_at)| {
                        let (retry_delay, rate_limits, rebase_failures) =
                            schedule_after_cache_neutral(Some(hint), 7, 11)
                                .expect("neutral hint schedules another cycle");
                        assert_eq!((rate_limits, rebase_failures), (7, 11));
                        let delay = retry_delay
                            .expect("contention retry budget must not reach regular cadence");
                        (index, previous_at + delay)
                    })
                    .collect::<Vec<_>>();
                let results = run_scheduled_fetches(repos, progresses, &scheduled);
                pending = results
                    .into_iter()
                    .filter_map(|(index, result)| match result {
                        PullFetchResult::Ready => None,
                        PullFetchResult::NeutralSkip(hint) => {
                            let scheduled_at = scheduled
                                .iter()
                                .find_map(|&(scheduled_index, scheduled_at)| {
                                    (scheduled_index == index).then_some(scheduled_at)
                                })
                                .expect("scheduled follower time");
                            Some((index, hint, scheduled_at))
                        }
                        PullFetchResult::RemoteError(error) => {
                            panic!("follower {index} unexpectedly fetched remote: {error}")
                        }
                    })
                    .collect();
                assert_eq!(request_count(&control.counter_dir), expected_requests);
            }

            assert!(
                pending.is_empty(),
                "followers did not converge through bounded production retry hints"
            );
        }

        fn assert_tree_excludes(path: &Path, forbidden: &[&[u8]]) {
            let metadata = std::fs::symlink_metadata(path)
                .unwrap_or_else(|error| panic!("inspect {}: {error}", path.display()));
            if metadata.is_dir() {
                for entry in std::fs::read_dir(path)
                    .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
                {
                    let entry = entry.expect("read cache artifact entry");
                    assert_tree_excludes(&entry.path(), forbidden);
                }
                return;
            }
            if metadata.is_file() {
                let bytes = std::fs::read(path)
                    .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
                for needle in forbidden {
                    assert!(
                        !bytes.windows(needle.len()).any(|window| window == *needle),
                        "{} contains credential material",
                        path.display()
                    );
                }
            }
        }

        fn assert_ready(result: PullFetchResult) {
            assert!(
                matches!(result, PullFetchResult::Ready),
                "expected ready result, got {result:?}"
            );
        }

        fn reject_state_replacement() -> Result<(), CacheInfraError> {
            Err(CacheInfraError::Io(std::io::Error::other(
                "injected state replacement failure",
            )))
        }

        fn verify_complete_generation_before_state_replacement() -> Result<(), CacheInfraError> {
            BOUNDARY_EXPECTATION.with(|slot| {
                let expectation = slot.borrow_mut().take().ok_or_else(|| {
                    CacheInfraError::Io(std::io::Error::other(
                        "missing publication boundary expectation",
                    ))
                })?;
                let selected = read_state(&expectation.state_file)?.ok_or_else(|| {
                    CacheInfraError::Io(std::io::Error::other("missing selected cache state"))
                })?;
                if selected.generation != expectation.old_generation {
                    return Err(CacheInfraError::Io(std::io::Error::other(
                        "state selected new generation before publication completed",
                    )));
                }
                let prefix = format!(
                    "refs/gitim-fetch-cache/generations/{}/",
                    expectation.new_generation
                );
                let refs = cache_refs(&expectation.cache_repository, &prefix);
                let main_ref = format!(
                    "refs/gitim-fetch-cache/generations/{}/heads/main",
                    expectation.new_generation
                );
                if refs.len() != 1 || refs.get(&main_ref) != Some(&expectation.expected_main) {
                    return Err(CacheInfraError::Io(std::io::Error::other(
                        "new generation was incomplete at publication boundary",
                    )));
                }
                Ok(())
            })
        }

        fn remove_generation_tip_before_state_replacement() -> Result<(), CacheInfraError> {
            STATE_REPLACE_TAMPER.with(|slot| {
                let (cache_repository, generation) = slot.borrow_mut().take().ok_or_else(|| {
                    CacheInfraError::Io(std::io::Error::other(
                        "missing state replacement tamper target",
                    ))
                })?;
                let generation_main =
                    format!("refs/gitim-fetch-cache/generations/{generation}/heads/main");
                git_ok(&cache_repository, &["update-ref", "-d", &generation_main]);
                Ok(())
            })
        }

        #[test]
        fn orchestration_first_eligible_caller_publishes_generation_one() {
            let fixture = OrchestrationFixture::new();
            let repo = fixture.storage();
            let context = fixture.context();
            let mut progress = SyncCacheProgress::new(30);
            let now = UNIX_EPOCH + Duration::from_secs(10_000);

            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                now,
                PublicationHooks::default(),
            ));

            let published = read_state(&context.state_file)
                .expect("read cache state")
                .expect("published cache state");
            assert_eq!(published.generation, 1);
            assert_eq!(published.attempt, AttemptClass::Success);
            assert_eq!(published.completed_at_unix_ms, unix_ms(now));
            assert_eq!(progress.applied_generation, Some(1));
            assert_eq!(
                cache_refs(
                    &context.cache_repository,
                    "refs/gitim-fetch-cache/generations/1/"
                )
                .len(),
                1
            );
            assert!(fixture.origin.is_dir());
            assert!(fixture.seed.is_dir());
        }

        #[test]
        fn orchestration_clock_is_sampled_while_cache_lock_is_held() {
            let fixture = OrchestrationFixture::new();
            let repo = fixture.storage();
            let context = fixture.context();
            let mut progress = SyncCacheProgress::new(30);
            let mut clock = || {
                let competing_lock = std::fs::OpenOptions::new()
                    .create(true)
                    .read(true)
                    .write(true)
                    .truncate(false)
                    .open(&context.lock_file)
                    .expect("open competing cache lock");
                let error = competing_lock
                    .try_lock_exclusive()
                    .expect_err("clock must run while cache lock is held");
                assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
                UNIX_EPOCH + Duration::from_secs(10_000)
            };

            assert_ready(fetch_for_pull_with_clock(
                &repo,
                &mut progress,
                &mut clock,
                PublicationHooks::default(),
            ));
        }

        #[test]
        fn orchestration_success_completion_uses_post_publication_clock() {
            let fixture = OrchestrationFixture::new();
            let repo = fixture.storage();
            let context = fixture.context();
            let mut progress = SyncCacheProgress::new(30);
            let decision_at = UNIX_EPOCH + Duration::from_secs(10_000);
            let completed_at = decision_at + Duration::from_secs(42);
            let times = std::cell::RefCell::new(std::collections::VecDeque::from([
                decision_at,
                completed_at,
            ]));
            let mut clock = || times.borrow_mut().pop_front().expect("clock sample");

            assert_ready(fetch_for_pull_with_clock(
                &repo,
                &mut progress,
                &mut clock,
                PublicationHooks::default(),
            ));

            let published = read_state(&context.state_file)
                .expect("read cache state")
                .expect("published cache state");
            assert_eq!(published.completed_at_unix_ms, unix_ms(completed_at));
            assert!(times.borrow().is_empty());
        }

        #[test]
        fn orchestration_failure_cooldown_uses_post_attempt_clock() {
            let fixture = OrchestrationFixture::new();
            fixture.fail_remote("HTTP 401 invalid username or token");
            let repo = fixture.storage();
            let context = fixture.context();
            let mut progress = SyncCacheProgress::new(30);
            let decision_at = UNIX_EPOCH + Duration::from_secs(10_000);
            let completed_at = decision_at + Duration::from_secs(50);
            let times = std::cell::RefCell::new(std::collections::VecDeque::from([
                decision_at,
                completed_at,
            ]));
            let mut clock = || times.borrow_mut().pop_front().expect("clock sample");

            assert!(matches!(
                fetch_for_pull_with_clock(
                    &repo,
                    &mut progress,
                    &mut clock,
                    PublicationHooks::default(),
                ),
                PullFetchResult::RemoteError(GitError::AuthFailed(_))
            ));

            let published = read_state(&context.state_file)
                .expect("read cache state")
                .expect("published failure state");
            assert_eq!(published.completed_at_unix_ms, unix_ms(completed_at));
            assert_eq!(
                published.retry_after_unix_ms,
                Some(unix_ms(completed_at + AUTH_FAILURE_COOLDOWN))
            );
            assert!(times.borrow().is_empty());
        }

        #[test]
        fn orchestration_fresh_success_reuses_generation_without_remote_fetch() {
            let fixture = OrchestrationFixture::new();
            let repo = fixture.storage();
            let context = fixture.context();
            let mut progress = SyncCacheProgress::new(30);
            let first = UNIX_EPOCH + Duration::from_secs(10_000);
            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                first,
                PublicationHooks::default(),
            ));
            let first_state = read_state(&context.state_file)
                .expect("read first state")
                .expect("first state");
            std::fs::rename(
                &fixture.origin,
                fixture.workspace.root.path().join("origin-offline.git"),
            )
            .expect("make remote transport unavailable");

            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                first + Duration::from_secs(1),
                PublicationHooks::default(),
            ));

            assert_eq!(
                read_state(&context.state_file).expect("read reused state"),
                Some(first_state)
            );
            assert_eq!(progress.applied_generation, Some(1));
        }

        #[test]
        fn orchestration_unchanged_manifest_refreshes_timestamp_without_incrementing() {
            let fixture = OrchestrationFixture::new();
            let repo = fixture.storage();
            let context = fixture.context();
            let mut progress = SyncCacheProgress::new(30);
            let first = UNIX_EPOCH + Duration::from_secs(10_000);
            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                first,
                PublicationHooks::default(),
            ));
            let first_state = read_state(&context.state_file)
                .expect("read first state")
                .expect("first state");
            let refreshed_at = first + Duration::from_secs(30);

            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                refreshed_at,
                PublicationHooks::default(),
            ));

            let refreshed = read_state(&context.state_file)
                .expect("read refreshed state")
                .expect("refreshed state");
            assert_eq!(refreshed.generation, 1);
            assert_eq!(refreshed.manifest, first_state.manifest);
            assert_eq!(refreshed.completed_at_unix_ms, unix_ms(refreshed_at));
            assert!(cache_refs(
                &context.cache_repository,
                "refs/gitim-fetch-cache/generations/2/"
            )
            .is_empty());
        }

        #[test]
        fn orchestration_changed_manifest_publishes_generation_two() {
            let fixture = OrchestrationFixture::new();
            let repo = fixture.storage();
            let context = fixture.context();
            let mut progress = SyncCacheProgress::new(30);
            let first = UNIX_EPOCH + Duration::from_secs(10_000);
            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                first,
                PublicationHooks::default(),
            ));
            fixture.advance_remote("two\n");
            let changed_at = first + Duration::from_secs(30);

            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                changed_at,
                PublicationHooks::default(),
            ));

            let changed = read_state(&context.state_file)
                .expect("read changed state")
                .expect("changed state");
            assert_eq!(changed.generation, 2);
            assert_eq!(changed.completed_at_unix_ms, unix_ms(changed_at));
            assert_eq!(progress.applied_generation, Some(2));
            assert_eq!(
                git_stdout(&fixture.seed, &["rev-parse", "refs/heads/main"]),
                git_stdout(repo.root(), &["rev-parse", "refs/remotes/origin/main"])
            );
            assert_eq!(
                cache_refs(
                    &context.cache_repository,
                    "refs/gitim-fetch-cache/generations/2/"
                )
                .len(),
                1
            );
        }

        #[test]
        fn orchestration_older_state_cannot_rewind_applied_generation() {
            let fixture = OrchestrationFixture::new();
            let repo = fixture.storage();
            let context = fixture.context();
            let now = UNIX_EPOCH + Duration::from_secs(10_000);
            let mut progress = SyncCacheProgress::new(30);
            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                now,
                PublicationHooks::default(),
            ));
            let mut generation_one = read_state(&context.state_file)
                .expect("read generation one state")
                .expect("generation one state");
            let generation_one_main =
                generation_one.manifest[&format!("{CACHE_SHADOW_HEADS_PREFIX}main")].clone();

            fixture.advance_remote("two\n");
            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                now + Duration::from_secs(30),
                PublicationHooks::default(),
            ));
            let generation_two_main = git_stdout(&fixture.seed, &["rev-parse", "HEAD"]);
            assert_eq!(progress.applied_generation, Some(2));
            assert_eq!(
                git_stdout(repo.root(), &["rev-parse", "refs/remotes/origin/main"]),
                generation_two_main
            );

            git_ok(
                &context.cache_repository,
                &[
                    "update-ref",
                    "refs/gitim-fetch-cache/generations/1/heads/main",
                    &generation_one_main,
                ],
            );
            generation_one.completed_at_unix_ms = unix_ms(now + Duration::from_secs(31));
            write_state_atomic(&context.state_file, &generation_one)
                .expect("restore older cache state");

            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                now + Duration::from_secs(31),
                PublicationHooks::default(),
            ));

            assert_eq!(progress.applied_generation, Some(2));
            assert_eq!(
                git_stdout(repo.root(), &["rev-parse", "refs/remotes/origin/main"]),
                generation_two_main
            );
        }

        #[test]
        fn orchestration_empty_snapshot_keeps_generations_monotonic_for_missed_follower() {
            let fixture = OrchestrationFixture::new();
            let leader = fixture.storage();
            let follower = fixture.add_agent("alice");
            let context = fixture.context();
            let now = UNIX_EPOCH + Duration::from_secs(10_000);
            let mut leader_progress = SyncCacheProgress::new(30);
            assert_ready(fetch_for_pull_at(
                &leader,
                &mut leader_progress,
                now,
                PublicationHooks::default(),
            ));
            let mut follower_progress = SyncCacheProgress::new(30);
            assert_ready(fetch_for_pull_at(
                &follower,
                &mut follower_progress,
                now + Duration::from_secs(1),
                PublicationHooks::default(),
            ));
            assert_eq!(follower_progress.applied_generation, Some(1));

            git_ok(
                &fixture.origin,
                &["config", "receive.denyDeleteCurrent", "ignore"],
            );
            git_ok(&fixture.seed, &["push", "origin", "--delete", "main"]);
            assert_ready(fetch_for_pull_at(
                &leader,
                &mut leader_progress,
                now + Duration::from_secs(30),
                PublicationHooks::default(),
            ));
            let empty_state = read_state(&context.state_file)
                .expect("read empty state")
                .expect("empty state");
            assert_eq!(empty_state.generation, 2);
            assert!(empty_state.manifest.is_empty());
            assert_eq!(leader_progress.applied_generation, Some(2));
            assert!(cache_refs(
                &context.cache_repository,
                "refs/gitim-fetch-cache/generations/1/"
            )
            .is_empty());

            let offline_origin = fixture.workspace.root.path().join("origin-offline.git");
            std::fs::rename(&fixture.origin, &offline_origin)
                .expect("make remote unavailable during empty reuse");
            let mut empty_observer_progress = SyncCacheProgress::new(30);
            empty_observer_progress.applied_generation = Some(1);
            assert_ready(fetch_for_pull_at(
                &follower,
                &mut empty_observer_progress,
                now + Duration::from_secs(31),
                PublicationHooks::default(),
            ));
            assert_eq!(empty_observer_progress.applied_generation, Some(2));
            std::fs::rename(&offline_origin, &fixture.origin)
                .expect("restore remote after empty reuse");

            fixture.advance_remote("repopulated\n");
            assert_ready(fetch_for_pull_at(
                &leader,
                &mut leader_progress,
                now + Duration::from_secs(60),
                PublicationHooks::default(),
            ));
            let repopulated_state = read_state(&context.state_file)
                .expect("read repopulated state")
                .expect("repopulated state");
            assert_eq!(repopulated_state.generation, 3);
            assert_eq!(leader_progress.applied_generation, Some(3));

            assert_ready(fetch_for_pull_at(
                &follower,
                &mut follower_progress,
                now + Duration::from_secs(61),
                PublicationHooks::default(),
            ));
            assert_eq!(follower_progress.applied_generation, Some(3));
            assert_eq!(
                git_stdout(&fixture.seed, &["rev-parse", "refs/heads/main"]),
                git_stdout(follower.root(), &["rev-parse", "refs/remotes/origin/main"])
            );
        }

        #[cfg(unix)]
        #[test]
        fn orchestration_empty_snapshot_replaces_cache_symlink_without_touching_target() {
            use std::os::unix::fs::symlink;

            let fixture = OrchestrationFixture::new();
            let leader = fixture.storage();
            let context = fixture.context();
            let now = UNIX_EPOCH + Duration::from_secs(10_000);
            let mut progress = SyncCacheProgress::new(30);
            assert_ready(fetch_for_pull_at(
                &leader,
                &mut progress,
                now,
                PublicationHooks::default(),
            ));

            let external = fixture.workspace.root.path().join("external-cache.git");
            git_ok(
                fixture.workspace.root.path(),
                &["init", "--bare", path_arg(&external)],
            );
            git_ok(
                &external,
                &[
                    "config",
                    "--local",
                    "remote.origin.url",
                    "https://external.invalid/cache.git",
                ],
            );
            git_ok(
                &external,
                &["config", "--local", "cache.external-marker", "preserve"],
            );
            git_ok(
                &external,
                &[
                    "fetch",
                    path_arg(&fixture.seed),
                    "+refs/heads/main:refs/gitim-fetch-cache/generations/7/heads/main",
                ],
            );
            let external_refs_before = cache_refs(&external, "refs/gitim-fetch-cache/generations/");
            let external_config_before = git_stdout(&external, &["config", "--local", "--list"]);

            std::fs::rename(
                &context.cache_repository,
                context.runtime_dir.join("fetch-cache-owned-backup.git"),
            )
            .expect("move owned cache");
            symlink(&external, &context.cache_repository).expect("link external cache");
            git_ok(
                &fixture.origin,
                &["config", "receive.denyDeleteCurrent", "ignore"],
            );
            git_ok(&fixture.seed, &["push", "origin", "--delete", "main"]);

            assert_ready(fetch_for_pull_at(
                &leader,
                &mut progress,
                now + Duration::from_secs(30),
                PublicationHooks::default(),
            ));

            let empty = read_state(&context.state_file)
                .expect("read empty state")
                .expect("empty state");
            assert_eq!(empty.generation, 2);
            assert!(empty.manifest.is_empty());
            assert!(std::fs::symlink_metadata(&context.cache_repository)
                .expect("inspect rebuilt cache")
                .file_type()
                .is_dir());
            assert!(GitStorage::cache_repository_is_valid(
                &context.cache_repository,
                context.object_format
            ));
            assert_eq!(
                cache_refs(&external, "refs/gitim-fetch-cache/generations/",),
                external_refs_before
            );
            assert_eq!(
                git_stdout(&external, &["config", "--local", "--list"]),
                external_config_before
            );
        }

        #[cfg(unix)]
        #[test]
        fn orchestration_cleanup_rejects_invalid_cache_symlink() {
            use std::os::unix::fs::symlink;

            let fixture = OrchestrationFixture::new();
            let leader = fixture.storage();
            let context = fixture.context();
            let now = UNIX_EPOCH + Duration::from_secs(10_000);
            let mut progress = SyncCacheProgress::new(30);
            assert_ready(fetch_for_pull_at(
                &leader,
                &mut progress,
                now,
                PublicationHooks::default(),
            ));

            let external = fixture.workspace.root.path().join("cleanup-external.git");
            git_ok(
                fixture.workspace.root.path(),
                &["init", "--bare", path_arg(&external)],
            );
            git_ok(
                &external,
                &["config", "--local", "cache.external-marker", "preserve"],
            );
            git_ok(
                &external,
                &[
                    "fetch",
                    path_arg(&fixture.seed),
                    "+refs/heads/main:refs/gitim-fetch-cache/generations/7/heads/main",
                ],
            );
            let external_refs_before = cache_refs(&external, "refs/gitim-fetch-cache/generations/");
            let external_config_before = git_stdout(&external, &["config", "--local", "--list"]);
            std::fs::rename(
                &context.cache_repository,
                context.runtime_dir.join("fetch-cache-cleanup-backup.git"),
            )
            .expect("move owned cache");
            symlink(&external, &context.cache_repository).expect("link external cache");

            cleanup_inactive_generations(&context, 1);

            assert_eq!(
                cache_refs(&external, "refs/gitim-fetch-cache/generations/",),
                external_refs_before
            );
            assert_eq!(
                git_stdout(&external, &["config", "--local", "--list"]),
                external_config_before
            );
        }

        #[cfg(unix)]
        #[test]
        fn orchestration_stale_empty_generation_rebuilds_cache_symlink() {
            use std::os::unix::fs::symlink;

            let fixture = OrchestrationFixture::new();
            let leader = fixture.storage();
            let context = fixture.context();
            let now = UNIX_EPOCH + Duration::from_secs(10_000);
            let mut progress = SyncCacheProgress::new(30);
            assert_ready(fetch_for_pull_at(
                &leader,
                &mut progress,
                now,
                PublicationHooks::default(),
            ));
            git_ok(
                &fixture.origin,
                &["config", "receive.denyDeleteCurrent", "ignore"],
            );
            git_ok(&fixture.seed, &["push", "origin", "--delete", "main"]);
            assert_ready(fetch_for_pull_at(
                &leader,
                &mut progress,
                now + Duration::from_secs(30),
                PublicationHooks::default(),
            ));
            let empty = read_state(&context.state_file)
                .expect("read empty state")
                .expect("empty state");
            assert_eq!(empty.generation, 2);
            assert!(empty.manifest.is_empty());
            assert_eq!(progress.applied_generation, Some(2));

            let external = fixture
                .workspace
                .root
                .path()
                .join("stale-empty-external.git");
            git_ok(
                fixture.workspace.root.path(),
                &["init", "--bare", path_arg(&external)],
            );
            git_ok(
                &external,
                &["config", "--local", "cache.external-marker", "preserve"],
            );
            let external_config_before = git_stdout(&external, &["config", "--local", "--list"]);
            std::fs::rename(
                &context.cache_repository,
                context.runtime_dir.join("fetch-cache-empty-backup.git"),
            )
            .expect("move owned empty cache");
            symlink(&external, &context.cache_repository).expect("link external empty cache");

            assert_ready(fetch_for_pull_at(
                &leader,
                &mut progress,
                now + Duration::from_secs(60),
                PublicationHooks::default(),
            ));

            let repaired = read_state(&context.state_file)
                .expect("read repaired empty state")
                .expect("repaired empty state");
            assert_eq!(repaired.generation, 3);
            assert!(repaired.manifest.is_empty());
            assert_eq!(progress.applied_generation, Some(3));
            assert!(GitStorage::cache_repository_is_valid(
                &context.cache_repository,
                context.object_format
            ));
            assert_eq!(
                git_stdout(&external, &["config", "--local", "--list"]),
                external_config_before
            );
        }

        #[test]
        fn orchestration_new_daemon_imports_once_then_skips_applied_generation() {
            let fixture = OrchestrationFixture::new();
            let leader = fixture.storage();
            let follower = fixture.add_agent("alice");
            let context = fixture.context();
            let now = UNIX_EPOCH + Duration::from_secs(10_000);
            let mut leader_progress = SyncCacheProgress::new(30);
            assert_ready(fetch_for_pull_at(
                &leader,
                &mut leader_progress,
                now,
                PublicationHooks::default(),
            ));
            git_ok(
                follower.root(),
                &["update-ref", "-d", "refs/remotes/origin/main"],
            );
            std::fs::rename(
                &fixture.origin,
                fixture.workspace.root.path().join("origin-offline.git"),
            )
            .expect("make remote transport unavailable");
            let mut follower_progress = SyncCacheProgress::new(30);

            assert_ready(fetch_for_pull_at(
                &follower,
                &mut follower_progress,
                now + Duration::from_secs(1),
                PublicationHooks::default(),
            ));
            assert_eq!(
                git_stdout(&fixture.seed, &["rev-parse", "refs/heads/main"]),
                git_stdout(follower.root(), &["rev-parse", "refs/remotes/origin/main"])
            );
            assert_eq!(follower_progress.applied_generation, Some(1));

            std::fs::rename(
                &context.cache_repository,
                context.runtime_dir.join("fetch-cache-offline.git"),
            )
            .expect("make cache repository unavailable");
            assert_ready(fetch_for_pull_at(
                &follower,
                &mut follower_progress,
                now + Duration::from_secs(2),
                PublicationHooks::default(),
            ));
        }

        #[test]
        fn orchestration_restart_with_unknown_generation_imports_once() {
            let fixture = OrchestrationFixture::new();
            let repo = fixture.storage();
            let now = UNIX_EPOCH + Duration::from_secs(10_000);
            let mut first_process = SyncCacheProgress::new(30);
            assert_ready(fetch_for_pull_at(
                &repo,
                &mut first_process,
                now,
                PublicationHooks::default(),
            ));
            git_ok(
                repo.root(),
                &["update-ref", "-d", "refs/remotes/origin/main"],
            );
            std::fs::rename(
                &fixture.origin,
                fixture.workspace.root.path().join("origin-offline.git"),
            )
            .expect("make remote transport unavailable");
            let mut restarted_process = SyncCacheProgress::new(30);

            assert_ready(fetch_for_pull_at(
                &repo,
                &mut restarted_process,
                now + Duration::from_secs(1),
                PublicationHooks::default(),
            ));

            assert_eq!(restarted_process.applied_generation, Some(1));
            assert_eq!(
                git_stdout(&fixture.seed, &["rev-parse", "refs/heads/main"]),
                git_stdout(repo.root(), &["rev-parse", "refs/remotes/origin/main"])
            );
        }

        #[test]
        fn orchestration_lock_contention_is_bounded_and_neutral() {
            let fixture = OrchestrationFixture::new();
            let repo = fixture.storage();
            let context = fixture.context();
            let held_lock = std::fs::OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .truncate(false)
                .open(&context.lock_file)
                .expect("open held lock");
            held_lock.lock_exclusive().expect("hold cache lock");
            std::fs::rename(
                &fixture.origin,
                fixture.workspace.root.path().join("origin-offline.git"),
            )
            .expect("make remote transport unavailable");
            let mut progress = SyncCacheProgress::new(30);
            let started = Instant::now();

            assert!(matches!(
                fetch_for_pull_at(
                    &repo,
                    &mut progress,
                    UNIX_EPOCH + Duration::from_secs(10_000),
                    PublicationHooks::default(),
                ),
                PullFetchResult::NeutralSkip(CacheNeutralHint::RetryContention)
            ));

            let waited = started.elapsed();
            assert!(
                waited >= Duration::from_millis(900),
                "contention returned before the bounded wait: {waited:?}"
            );
            assert!(
                waited < Duration::from_secs(3),
                "contention exceeded the bounded wait: {waited:?}"
            );
            assert_eq!(progress.applied_generation, None);
            assert!(!progress.disabled);
        }

        #[test]
        fn orchestration_remote_slot_busy_is_neutral_and_keeps_cache_enabled() {
            let fixture = OrchestrationFixture::new();
            let repo = fixture.storage();
            let context = fixture.context();
            let lock_path = context.runtime_dir.join("remote-git.lock");
            let held_lock = std::fs::OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .truncate(false)
                .open(&lock_path)
                .expect("open remote git lock");
            held_lock.lock_exclusive().expect("hold remote git lock");
            std::fs::rename(
                &fixture.origin,
                fixture.workspace.root.path().join("origin-offline.git"),
            )
            .expect("make remote transport unavailable");
            let mut progress = SyncCacheProgress::new(30);
            let started = Instant::now();

            assert!(matches!(
                fetch_for_pull_at(
                    &repo,
                    &mut progress,
                    UNIX_EPOCH + Duration::from_secs(10_000),
                    PublicationHooks::default(),
                ),
                PullFetchResult::NeutralSkip(CacheNeutralHint::PreserveSchedule)
            ));

            let waited = started.elapsed();
            assert!(
                waited >= Duration::from_millis(900),
                "busy return before the bounded wait: {waited:?}"
            );
            assert!(
                waited < Duration::from_secs(3),
                "busy return exceeded the bounded wait: {waited:?}"
            );
            assert_eq!(progress.applied_generation, None);
            assert!(!progress.disabled);
        }

        #[test]
        fn orchestration_acquired_lock_resets_contention_retry_budget() {
            let fixture = OrchestrationFixture::new();
            let repo = fixture.storage();
            let mut progress = SyncCacheProgress::new(30);
            assert_eq!(
                progress.contention_neutral_hint(),
                CacheNeutralHint::RetryContention
            );
            assert_eq!(
                progress.contention_neutral_hint(),
                CacheNeutralHint::RetryContention
            );

            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                UNIX_EPOCH + Duration::from_secs(10_000),
                PublicationHooks::default(),
            ));

            assert_eq!(progress.contention_short_retries, 0);
        }

        #[test]
        fn orchestration_lock_open_failure_uses_direct_fetch() {
            let fixture = OrchestrationFixture::new();
            let repo = fixture.storage();
            let context = fixture.context();
            std::fs::create_dir(&context.lock_file).expect("replace lock file with directory");
            let mut progress = SyncCacheProgress::new(30);

            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                UNIX_EPOCH + Duration::from_secs(10_000),
                PublicationHooks::default(),
            ));

            assert!(progress.disabled);
            assert_eq!(
                git_stdout(&fixture.seed, &["rev-parse", "refs/heads/main"]),
                git_stdout(repo.root(), &["rev-parse", "refs/remotes/origin/main"])
            );
            assert!(read_state(&context.state_file)
                .expect("read absent state")
                .is_none());
        }

        #[test]
        fn orchestration_auth_failure_publishes_five_minute_cooldown() {
            let fixture = OrchestrationFixture::new();
            fixture.fail_remote(
                "HTTP 401 https://x-access-token:test-pat-123@github.com/CiferaTeam/GitIM.git",
            );
            let repo = fixture.storage();
            let context = fixture.context();
            let mut progress = SyncCacheProgress::new(30);
            let now = UNIX_EPOCH + Duration::from_secs(10_000);

            let result = fetch_for_pull_at(&repo, &mut progress, now, PublicationHooks::default());

            assert!(matches!(
                result,
                PullFetchResult::RemoteError(GitError::AuthFailed(_))
            ));
            let failed = read_state(&context.state_file)
                .expect("read failure state")
                .expect("failure state");
            assert_eq!(failed.attempt, AttemptClass::AuthFailed);
            assert_eq!(failed.generation, 0);
            assert!(failed.manifest.is_empty());
            assert_eq!(failed.completed_at_unix_ms, unix_ms(now));
            assert_eq!(
                failed.retry_after_unix_ms,
                Some(unix_ms(now) + AUTH_FAILURE_COOLDOWN.as_millis() as u64)
            );
        }

        #[test]
        fn orchestration_rate_limit_publishes_two_minute_cooldown() {
            let fixture = OrchestrationFixture::new();
            fixture.fail_remote("HTTP 429 too many requests");
            let repo = fixture.storage();
            let context = fixture.context();
            let mut progress = SyncCacheProgress::new(30);
            let now = UNIX_EPOCH + Duration::from_secs(10_000);

            let result = fetch_for_pull_at(&repo, &mut progress, now, PublicationHooks::default());

            assert!(matches!(
                result,
                PullFetchResult::RemoteError(GitError::RateLimited)
            ));
            let failed = read_state(&context.state_file)
                .expect("read failure state")
                .expect("failure state");
            assert_eq!(failed.attempt, AttemptClass::RateLimited);
            assert_eq!(
                failed.retry_after_unix_ms,
                Some(unix_ms(now) + RATE_LIMIT_COOLDOWN.as_millis() as u64)
            );
        }

        #[test]
        fn orchestration_transient_failure_uses_interval_with_three_second_floor() {
            for (interval_secs, expected_cooldown_secs) in [(1, 3), (9, 9)] {
                let fixture = OrchestrationFixture::new();
                fixture.fail_remote("temporary network failure");
                let repo = fixture.storage();
                let context = fixture.context();
                let mut progress = SyncCacheProgress::new(interval_secs);
                let now = UNIX_EPOCH + Duration::from_secs(10_000);

                let result =
                    fetch_for_pull_at(&repo, &mut progress, now, PublicationHooks::default());

                assert!(matches!(
                    result,
                    PullFetchResult::RemoteError(GitError::CommandFailed(_))
                ));
                let failed = read_state(&context.state_file)
                    .expect("read failure state")
                    .expect("failure state");
                assert_eq!(failed.attempt, AttemptClass::TransientFailure);
                assert_eq!(
                    failed.retry_after_unix_ms,
                    Some(unix_ms(now) + expected_cooldown_secs * 1_000)
                );
            }
        }

        #[test]
        fn orchestration_follower_inside_failure_cooldown_is_neutral() {
            let fixture = OrchestrationFixture::new();
            fixture.fail_remote("HTTP 401 invalid username or token");
            let repo = fixture.storage();
            let context = fixture.context();
            let now = UNIX_EPOCH + Duration::from_secs(10_000);
            let mut leader_progress = SyncCacheProgress::new(30);
            assert!(matches!(
                fetch_for_pull_at(
                    &repo,
                    &mut leader_progress,
                    now,
                    PublicationHooks::default(),
                ),
                PullFetchResult::RemoteError(GitError::AuthFailed(_))
            ));
            let failure_state = read_state(&context.state_file)
                .expect("read failure state")
                .expect("failure state");
            let mut follower_progress = SyncCacheProgress::new(30);

            assert!(matches!(
                fetch_for_pull_at(
                    &repo,
                    &mut follower_progress,
                    now + Duration::from_secs(1),
                    PublicationHooks::default(),
                ),
                PullFetchResult::NeutralSkip(CacheNeutralHint::PreserveSchedule)
            ));

            assert_eq!(
                read_state(&context.state_file).expect("read unchanged failure state"),
                Some(failure_state)
            );
            assert_eq!(follower_progress.applied_generation, None);
            assert!(!follower_progress.disabled);
        }

        #[test]
        fn orchestration_config_revision_change_invalidates_failure_cooldown() {
            let fixture = OrchestrationFixture::new();
            fixture.fail_remote("HTTP 401 invalid username or token");
            let repo = fixture.storage();
            let context = fixture.context();
            let now = UNIX_EPOCH + Duration::from_secs(10_000);
            let mut progress = SyncCacheProgress::new(30);
            assert!(matches!(
                fetch_for_pull_at(&repo, &mut progress, now, PublicationHooks::default(),),
                PullFetchResult::RemoteError(GitError::AuthFailed(_))
            ));
            let failed_revision = read_state(&context.state_file)
                .expect("read failure state")
                .expect("failure state")
                .config_revision;
            fixture.restore_remote();
            fixture.change_config_revision();
            let refreshed_context = fixture.context();
            assert_ne!(refreshed_context.config_revision, failed_revision);

            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                now + Duration::from_secs(1),
                PublicationHooks::default(),
            ));

            let recovered = read_state(&context.state_file)
                .expect("read recovered state")
                .expect("recovered state");
            assert_eq!(recovered.attempt, AttemptClass::Success);
            assert_eq!(recovered.generation, 1);
            assert_eq!(recovered.config_revision, refreshed_context.config_revision);
        }

        #[test]
        fn orchestration_corrupt_state_falls_back_and_disables_cache_for_process() {
            let fixture = OrchestrationFixture::new();
            let repo = fixture.storage();
            let context = fixture.context();
            let now = UNIX_EPOCH + Duration::from_secs(10_000);
            let mut leader_progress = SyncCacheProgress::new(30);
            assert_ready(fetch_for_pull_at(
                &repo,
                &mut leader_progress,
                now,
                PublicationHooks::default(),
            ));
            let valid_state = std::fs::read(&context.state_file).expect("save valid cache state");
            std::fs::write(&context.state_file, b"{corrupt").expect("corrupt cache state");
            let mut progress = SyncCacheProgress::new(30);

            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                now + Duration::from_secs(1),
                PublicationHooks::default(),
            ));
            assert!(progress.disabled);

            std::fs::write(&context.state_file, valid_state).expect("restore valid cache state");
            std::fs::rename(
                &fixture.origin,
                fixture.workspace.root.path().join("origin-offline.git"),
            )
            .expect("make remote transport unavailable");
            assert!(matches!(
                fetch_for_pull_at(
                    &repo,
                    &mut progress,
                    now + Duration::from_secs(2),
                    PublicationHooks::default(),
                ),
                PullFetchResult::RemoteError(GitError::CommandFailed(_))
            ));
            assert!(progress.disabled);
        }

        #[test]
        fn orchestration_generation_zero_with_manifest_has_no_trustworthy_state() {
            let fixture = OrchestrationFixture::new();
            let repo = fixture.storage();
            let context = fixture.context();
            let now = UNIX_EPOCH + Duration::from_secs(10_000);
            let mut invalid = state(
                AttemptClass::Success,
                context.config_revision.clone(),
                unix_ms(now),
                None,
            );
            invalid.remote_identity.clone_from(&context.remote_identity);
            invalid.generation = 0;
            write_state_atomic(&context.state_file, &invalid)
                .expect("write parseable invalid state");
            let mut progress = SyncCacheProgress::new(30);

            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                now + Duration::from_secs(1),
                PublicationHooks::default(),
            ));

            assert_eq!(progress.applied_generation, None);
            assert!(progress.disabled);
        }

        #[test]
        fn orchestration_corrupt_bare_repairs_with_new_generation() {
            let fixture = OrchestrationFixture::new();
            let repo = fixture.storage();
            let context = fixture.context();
            let now = UNIX_EPOCH + Duration::from_secs(10_000);
            let mut leader_progress = SyncCacheProgress::new(30);
            assert_ready(fetch_for_pull_at(
                &repo,
                &mut leader_progress,
                now,
                PublicationHooks::default(),
            ));
            git_ok(
                repo.root(),
                &["update-ref", "-d", "refs/remotes/origin/main"],
            );
            std::fs::rename(
                &context.cache_repository,
                context.runtime_dir.join("fetch-cache-valid-backup.git"),
            )
            .expect("move valid bare cache");
            std::fs::create_dir(&context.cache_repository).expect("create corrupt bare cache");
            let mut progress = SyncCacheProgress::new(30);

            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                now + Duration::from_secs(1),
                PublicationHooks::default(),
            ));
            assert_eq!(progress.applied_generation, Some(2));
            assert!(!progress.disabled);
            assert_eq!(
                git_stdout(&fixture.seed, &["rev-parse", "refs/heads/main"]),
                git_stdout(repo.root(), &["rev-parse", "refs/remotes/origin/main"])
            );

            std::fs::rename(
                &fixture.origin,
                fixture.workspace.root.path().join("origin-offline.git"),
            )
            .expect("make remote transport unavailable");
            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                now + Duration::from_secs(2),
                PublicationHooks::default(),
            ));
        }

        #[test]
        fn orchestration_fresh_follower_repairs_invalid_active_generation() {
            for tamper in ["missing", "altered"] {
                let fixture = OrchestrationFixture::new();
                let leader = fixture.storage();
                let follower = fixture.add_agent("alice");
                let later_follower = fixture.add_agent("bob");
                let context = fixture.context();
                let now = UNIX_EPOCH + Duration::from_secs(10_000);
                let mut leader_progress = SyncCacheProgress::new(30);
                assert_ready(fetch_for_pull_at(
                    &leader,
                    &mut leader_progress,
                    now,
                    PublicationHooks::default(),
                ));
                let selected = read_state(&context.state_file)
                    .expect("read selected state")
                    .expect("selected state");
                let expected_main =
                    selected.manifest[&format!("{CACHE_SHADOW_HEADS_PREFIX}main")].clone();
                git_ok(
                    follower.root(),
                    &["update-ref", "-d", "refs/remotes/origin/main"],
                );
                git_ok(
                    later_follower.root(),
                    &["update-ref", "-d", "refs/remotes/origin/main"],
                );
                let generation_main = "refs/gitim-fetch-cache/generations/1/heads/main";

                match tamper {
                    "missing" => {
                        git_ok(
                            &context.cache_repository,
                            &["update-ref", "-d", generation_main],
                        );
                        git_ok(
                            &context.cache_repository,
                            &[
                                "update-ref",
                                "refs/gitim-fetch-cache/generations/1/heads/extra",
                                &expected_main,
                            ],
                        );
                    }
                    "altered" => {
                        std::fs::write(fixture.seed.join("version.txt"), "tampered\n")
                            .expect("write unpushed tamper");
                        git_ok(&fixture.seed, &["add", "version.txt"]);
                        git_ok(
                            &fixture.seed,
                            &["commit", "-m", "create unpushed tamper object"],
                        );
                        let tampered_object = git_stdout(&fixture.seed, &["rev-parse", "HEAD"]);
                        let source_refspec =
                            "+refs/heads/main:refs/gitim-fetch-cache/tamper-source";
                        git_ok(
                            &context.cache_repository,
                            &["fetch", path_arg(&fixture.seed), source_refspec],
                        );
                        git_ok(
                            &context.cache_repository,
                            &["update-ref", generation_main, &tampered_object],
                        );
                    }
                    _ => unreachable!("test tamper is known"),
                }

                let mut progress = SyncCacheProgress::new(30);
                assert_ready(fetch_for_pull_at(
                    &follower,
                    &mut progress,
                    now + Duration::from_secs(1),
                    PublicationHooks::default(),
                ));

                let repaired = read_state(&context.state_file)
                    .expect("read repaired state")
                    .expect("repaired state");
                assert_eq!(repaired.generation, 2, "{tamper}");
                assert_eq!(repaired.manifest, selected.manifest, "{tamper}");
                validate_active_generation(&context, &repaired)
                    .expect("repaired active generation matches manifest");
                assert_eq!(progress.applied_generation, Some(2), "{tamper}");
                assert!(!progress.disabled, "{tamper}");
                assert_eq!(
                    git_stdout(follower.root(), &["rev-parse", "refs/remotes/origin/main"]),
                    expected_main,
                    "{tamper}"
                );

                std::fs::rename(
                    &fixture.origin,
                    fixture.workspace.root.path().join("origin-offline.git"),
                )
                .expect("make remote transport unavailable");
                let mut later_progress = SyncCacheProgress::new(30);
                assert_ready(fetch_for_pull_at(
                    &later_follower,
                    &mut later_progress,
                    now + Duration::from_secs(2),
                    PublicationHooks::default(),
                ));
                assert_eq!(later_progress.applied_generation, Some(2), "{tamper}");
                assert_eq!(
                    git_stdout(
                        later_follower.root(),
                        &["rev-parse", "refs/remotes/origin/main"]
                    ),
                    expected_main,
                    "{tamper}"
                );
            }
        }

        #[test]
        fn orchestration_stale_leader_repairs_invalid_applied_generation() {
            let fixture = OrchestrationFixture::new();
            let repo = fixture.storage();
            let context = fixture.context();
            let now = UNIX_EPOCH + Duration::from_secs(10_000);
            let mut progress = SyncCacheProgress::new(30);
            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                now,
                PublicationHooks::default(),
            ));
            let selected = read_state(&context.state_file)
                .expect("read selected state")
                .expect("selected state");
            let expected_main =
                selected.manifest[&format!("{CACHE_SHADOW_HEADS_PREFIX}main")].clone();
            git_ok(
                &context.cache_repository,
                &[
                    "update-ref",
                    "-d",
                    "refs/gitim-fetch-cache/generations/1/heads/main",
                ],
            );

            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                now + Duration::from_secs(30),
                PublicationHooks::default(),
            ));

            let repaired = read_state(&context.state_file)
                .expect("read repaired state")
                .expect("repaired state");
            assert_eq!(repaired.generation, 2);
            assert_eq!(repaired.manifest, selected.manifest);
            validate_active_generation(&context, &repaired)
                .expect("repaired active generation matches manifest");
            assert_eq!(progress.applied_generation, Some(2));
            assert_eq!(
                git_stdout(repo.root(), &["rev-parse", "refs/remotes/origin/main"]),
                expected_main
            );
        }

        #[test]
        fn orchestration_active_repair_preserves_remote_error_classification() {
            let fixture = OrchestrationFixture::new();
            let repo = fixture.storage();
            let context = fixture.context();
            let now = UNIX_EPOCH + Duration::from_secs(10_000);
            let mut progress = SyncCacheProgress::new(30);
            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                now,
                PublicationHooks::default(),
            ));
            let selected = read_state(&context.state_file)
                .expect("read selected state")
                .expect("selected state");
            git_ok(
                &context.cache_repository,
                &[
                    "update-ref",
                    "-d",
                    "refs/gitim-fetch-cache/generations/1/heads/main",
                ],
            );
            fixture.fail_remote("HTTP 401 invalid username or token");

            assert!(matches!(
                fetch_for_pull_at(
                    &repo,
                    &mut progress,
                    now + Duration::from_secs(30),
                    PublicationHooks::default(),
                ),
                PullFetchResult::RemoteError(GitError::AuthFailed(_))
            ));

            let failed = read_state(&context.state_file)
                .expect("read failed repair state")
                .expect("failed repair state");
            assert_eq!(failed.attempt, AttemptClass::AuthFailed);
            assert_eq!(failed.generation, 1);
            assert_eq!(failed.manifest, selected.manifest);
        }

        #[test]
        fn orchestration_fresh_follower_repairs_missing_active_tip_objects() {
            let fixture = OrchestrationFixture::new();
            let leader = fixture.storage();
            let follower = fixture.add_agent("alice");
            let later_follower = fixture.add_agent("bob");
            let context = fixture.context();
            let now = UNIX_EPOCH + Duration::from_secs(10_000);
            let mut leader_progress = SyncCacheProgress::new(30);
            assert_ready(fetch_for_pull_at(
                &leader,
                &mut leader_progress,
                now,
                PublicationHooks::default(),
            ));
            let selected = read_state(&context.state_file)
                .expect("read selected state")
                .expect("selected state");
            let expected_main =
                selected.manifest[&format!("{CACHE_SHADOW_HEADS_PREFIX}main")].clone();
            git_ok(
                follower.root(),
                &["update-ref", "-d", "refs/remotes/origin/main"],
            );
            git_ok(
                later_follower.root(),
                &["update-ref", "-d", "refs/remotes/origin/main"],
            );
            clear_cache_object_database(&context.cache_repository);
            assert_eq!(
                cache_refs(
                    &context.cache_repository,
                    "refs/gitim-fetch-cache/generations/1/"
                )
                .len(),
                1
            );

            let mut follower_progress = SyncCacheProgress::new(30);
            assert_ready(fetch_for_pull_at(
                &follower,
                &mut follower_progress,
                now + Duration::from_secs(1),
                PublicationHooks::default(),
            ));

            let repaired = read_state(&context.state_file)
                .expect("read repaired state")
                .expect("repaired state");
            assert_eq!(repaired.generation, 2);
            assert_eq!(repaired.manifest, selected.manifest);
            assert_eq!(follower_progress.applied_generation, Some(2));
            assert_eq!(
                git_stdout(follower.root(), &["rev-parse", "refs/remotes/origin/main"]),
                expected_main
            );

            std::fs::rename(
                &fixture.origin,
                fixture.workspace.root.path().join("origin-offline.git"),
            )
            .expect("make remote transport unavailable");
            let mut later_progress = SyncCacheProgress::new(30);
            assert_ready(fetch_for_pull_at(
                &later_follower,
                &mut later_progress,
                now + Duration::from_secs(2),
                PublicationHooks::default(),
            ));
            assert_eq!(later_progress.applied_generation, Some(2));
            assert_eq!(
                git_stdout(
                    later_follower.root(),
                    &["rev-parse", "refs/remotes/origin/main"]
                ),
                expected_main
            );
        }

        #[test]
        fn orchestration_stale_leader_repairs_missing_applied_tip_objects() {
            let fixture = OrchestrationFixture::new();
            let repo = fixture.storage();
            let context = fixture.context();
            let now = UNIX_EPOCH + Duration::from_secs(10_000);
            let mut progress = SyncCacheProgress::new(30);
            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                now,
                PublicationHooks::default(),
            ));
            let selected = read_state(&context.state_file)
                .expect("read selected state")
                .expect("selected state");
            clear_cache_object_database(&context.cache_repository);

            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                now + Duration::from_secs(30),
                PublicationHooks::default(),
            ));

            let repaired = read_state(&context.state_file)
                .expect("read repaired state")
                .expect("repaired state");
            assert_eq!(repaired.generation, 2);
            assert_eq!(repaired.manifest, selected.manifest);
            assert_eq!(progress.applied_generation, Some(2));
        }

        #[test]
        fn orchestration_import_failure_forces_single_repair_generation() {
            let fixture = OrchestrationFixture::new();
            let leader = fixture.storage();
            let follower = fixture.add_empty_agent("alice");
            let context = fixture.context();
            let now = UNIX_EPOCH + Duration::from_secs(10_000);
            let mut leader_progress = SyncCacheProgress::new(30);
            assert_ready(fetch_for_pull_at(
                &leader,
                &mut leader_progress,
                now,
                PublicationHooks::default(),
            ));
            let selected = read_state(&context.state_file)
                .expect("read selected state")
                .expect("selected state");
            let expected_main =
                selected.manifest[&format!("{CACHE_SHADOW_HEADS_PREFIX}main")].clone();
            preserve_only_cache_commit(&context.cache_repository, &expected_main);
            let mut progress = SyncCacheProgress::new(30);
            assert_ready(fetch_for_pull_at(
                &follower,
                &mut progress,
                now + Duration::from_secs(1),
                PublicationHooks::default(),
            ));

            let repaired = read_state(&context.state_file)
                .expect("read repaired state")
                .expect("repaired state");
            assert_eq!(repaired.generation, 2);
            assert_eq!(repaired.manifest, selected.manifest);
            assert_eq!(progress.applied_generation, Some(2));
            assert_eq!(
                git_stdout(follower.root(), &["rev-parse", "refs/remotes/origin/main"]),
                expected_main
            );
        }

        #[test]
        fn orchestration_forced_repair_preserves_remote_error_classification() {
            let fixture = OrchestrationFixture::new();
            let leader = fixture.storage();
            let repo = fixture.add_empty_agent("alice");
            let context = fixture.context();
            let now = UNIX_EPOCH + Duration::from_secs(10_000);
            let mut leader_progress = SyncCacheProgress::new(30);
            assert_ready(fetch_for_pull_at(
                &leader,
                &mut leader_progress,
                now,
                PublicationHooks::default(),
            ));
            let selected = read_state(&context.state_file)
                .expect("read selected state")
                .expect("selected state");
            let expected_main =
                selected.manifest[&format!("{CACHE_SHADOW_HEADS_PREFIX}main")].clone();
            preserve_only_cache_commit(&context.cache_repository, &expected_main);
            fixture.fail_remote("HTTP 401 invalid username or token");
            git_config(
                repo.root(),
                &[
                    "remote.origin.uploadpack",
                    path_arg(&fixture.workspace.root.path().join("failing-upload-pack.sh")),
                ],
            );

            let mut progress = SyncCacheProgress::new(30);
            assert!(matches!(
                fetch_for_pull_at(
                    &repo,
                    &mut progress,
                    now + Duration::from_secs(1),
                    PublicationHooks::default(),
                ),
                PullFetchResult::RemoteError(GitError::AuthFailed(_))
            ));

            let failed = read_state(&context.state_file)
                .expect("read failed forced repair state")
                .expect("failed forced repair state");
            assert_eq!(failed.attempt, AttemptClass::AuthFailed);
            assert_eq!(failed.generation, 1);
            assert_eq!(failed.manifest, selected.manifest);
        }

        #[test]
        fn orchestration_publication_failure_keeps_old_generation_active() {
            let fixture = OrchestrationFixture::new();
            let repo = fixture.storage();
            let context = fixture.context();
            let now = UNIX_EPOCH + Duration::from_secs(10_000);
            let mut progress = SyncCacheProgress::new(30);
            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                now,
                PublicationHooks::default(),
            ));
            let first_state = read_state(&context.state_file)
                .expect("read first state")
                .expect("first state");
            fixture.advance_remote("two\n");

            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                now + Duration::from_secs(30),
                PublicationHooks {
                    before_state_replace: Some(reject_state_replacement),
                },
            ));

            assert_eq!(
                read_state(&context.state_file).expect("read active state"),
                Some(first_state)
            );
            assert_eq!(progress.applied_generation, Some(1));
            assert_eq!(
                cache_refs(
                    &context.cache_repository,
                    "refs/gitim-fetch-cache/generations/2/"
                )
                .len(),
                1
            );

            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                now + Duration::from_secs(31),
                PublicationHooks::default(),
            ));
            assert_eq!(
                read_state(&context.state_file)
                    .expect("read retried state")
                    .expect("retried state")
                    .generation,
                2
            );
        }

        #[test]
        fn orchestration_state_replacement_selects_only_complete_generation() {
            let fixture = OrchestrationFixture::new();
            let repo = fixture.storage();
            let context = fixture.context();
            let now = UNIX_EPOCH + Duration::from_secs(10_000);
            let mut progress = SyncCacheProgress::new(30);
            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                now,
                PublicationHooks::default(),
            ));
            fixture.advance_remote("two\n");
            let expected_main = git_stdout(&fixture.seed, &["rev-parse", "refs/heads/main"]);
            BOUNDARY_EXPECTATION.with(|slot| {
                *slot.borrow_mut() = Some(BoundaryExpectation {
                    state_file: context.state_file.clone(),
                    cache_repository: context.cache_repository.clone(),
                    old_generation: 1,
                    new_generation: 2,
                    expected_main,
                });
            });

            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                now + Duration::from_secs(30),
                PublicationHooks {
                    before_state_replace: Some(verify_complete_generation_before_state_replacement),
                },
            ));

            assert_eq!(
                read_state(&context.state_file)
                    .expect("read selected state")
                    .expect("selected state")
                    .generation,
                2
            );
            BOUNDARY_EXPECTATION.with(|slot| {
                assert!(
                    slot.borrow().is_none(),
                    "publication boundary hook was not called"
                );
            });
        }

        #[test]
        fn orchestration_invalid_generation_is_rejected_before_state_replacement() {
            let fixture = OrchestrationFixture::new();
            let repo = fixture.storage();
            let context = fixture.context();
            let now = UNIX_EPOCH + Duration::from_secs(10_000);
            let mut progress = SyncCacheProgress::new(30);
            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                now,
                PublicationHooks::default(),
            ));
            let first_state = read_state(&context.state_file)
                .expect("read first state")
                .expect("first state");
            fixture.advance_remote("two\n");
            let expected_main = git_stdout(&fixture.seed, &["rev-parse", "HEAD"]);
            STATE_REPLACE_TAMPER.with(|slot| {
                *slot.borrow_mut() = Some((context.cache_repository.clone(), 2));
            });

            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                now + Duration::from_secs(30),
                PublicationHooks {
                    before_state_replace: Some(remove_generation_tip_before_state_replacement),
                },
            ));

            assert_eq!(
                read_state(&context.state_file).expect("read selected state"),
                Some(first_state)
            );
            assert_eq!(progress.applied_generation, Some(1));
            assert!(!progress.disabled);
            assert_eq!(
                git_stdout(repo.root(), &["rev-parse", "refs/remotes/origin/main"]),
                expected_main
            );
            STATE_REPLACE_TAMPER.with(|slot| {
                assert!(
                    slot.borrow().is_none(),
                    "state replacement tamper hook was not called"
                );
            });
        }

        #[test]
        fn orchestration_cache_artifacts_exclude_credentials() {
            let fixture = OrchestrationFixture::new();
            let repo = fixture.storage();
            let context = fixture.context();
            let mut progress = SyncCacheProgress::new(30);
            assert_ready(fetch_for_pull_at(
                &repo,
                &mut progress,
                UNIX_EPOCH + Duration::from_secs(10_000),
                PublicationHooks::default(),
            ));

            let forbidden = [TEST_TOKEN.as_bytes(), RAW_ORIGIN.as_bytes()];
            assert_tree_excludes(&context.lock_file, &forbidden);
            assert_tree_excludes(&context.state_file, &forbidden);
            assert_tree_excludes(&context.cache_repository, &forbidden);
        }

        #[cfg(unix)]
        pub(super) fn run_ten_daemon_acceptance() {
            let fixture = OrchestrationFixture::new();
            let repos = (0..10)
                .map(|index| fixture.add_agent(&format!("agent-{index}")))
                .collect::<Vec<_>>();
            let mut control = configure_upload_pack_control(&fixture, &repos);
            let mut progresses = vec![SyncCacheProgress::new(30); repos.len()];
            let first_refresh = UNIX_EPOCH + Duration::from_secs(10_000);
            fixture.advance_remote("one-plus\n");
            let origin_main = git_stdout(&fixture.origin, &["rev-parse", "refs/heads/main"]);

            run_gated_contention_boundary(&repos, &mut progresses, &mut control, first_refresh, 1);
            assert_eq!(request_count(&control.counter_dir), 1);
            for repo in &repos {
                assert_eq!(
                    git_stdout(repo.root(), &["rev-parse", "refs/remotes/origin/main"]),
                    origin_main
                );
            }

            let fresh_results = run_concurrent_fetches(
                &repos,
                &mut progresses,
                first_refresh + Duration::from_secs(1),
            );
            assert!(
                fresh_results
                    .iter()
                    .all(|(_, result)| matches!(result, PullFetchResult::Ready)),
                "fresh-window cycles must reuse the published generation"
            );
            assert_eq!(request_count(&control.counter_dir), 1);

            fixture.advance_remote("two\n");
            let updated_origin_main =
                git_stdout(&fixture.origin, &["rev-parse", "refs/heads/main"]);
            run_gated_contention_boundary(
                &repos,
                &mut progresses,
                &mut control,
                first_refresh + Duration::from_secs(31),
                2,
            );

            assert_eq!(request_count(&control.counter_dir), 2);
            for repo in &repos {
                assert_eq!(
                    git_stdout(repo.root(), &["rev-parse", "refs/remotes/origin/main"]),
                    updated_origin_main
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn ten_daemons_coalesce_remote_fetches_and_converge() {
        orchestration::run_ten_daemon_acceptance();
    }
}
