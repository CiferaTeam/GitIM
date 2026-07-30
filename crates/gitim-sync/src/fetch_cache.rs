//! Shared pull-fetch publication order:
//!
//! ```text
//! private remote snapshot
//!         |
//!         v
//! immutable generation refs in bare cache
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

use crate::git::{GitError, GitStorage};
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
const RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(120);
const MIN_TRANSIENT_COOLDOWN: Duration = Duration::from_secs(3);
const STANDARD_FETCH_REFSPEC: &str = "+refs/heads/*:refs/remotes/origin/*";
const AUTH_FAILURE_COOLDOWN: Duration = Duration::from_secs(300);

#[derive(Debug, Clone)]
pub(crate) struct SyncCacheProgress {
    interval: Duration,
    applied_generation: Option<u64>,
    disabled: bool,
}

impl SyncCacheProgress {
    pub(crate) fn new(interval_secs: u32) -> Self {
        Self {
            interval: Duration::from_secs(u64::from(interval_secs)),
            applied_generation: None,
            disabled: false,
        }
    }
}

#[derive(Debug)]
pub(crate) enum PullFetchResult {
    Ready,
    NeutralSkip,
    RemoteError(GitError),
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
    validate_managed_layout(&workspace, &clone_root)?;

    let config_path = workspace.join(".gitim-runtime/config.json");
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

    let revision = config_revision(&config_path)?;
    let runtime_dir = workspace.join(".gitim-runtime");
    Some(CacheContext {
        workspace,
        cache_repository: runtime_dir.join(CACHE_REPOSITORY_DIR),
        state_file: runtime_dir.join(CACHE_STATE_FILE),
        lock_file: runtime_dir.join(CACHE_LOCK_FILE),
        runtime_dir,
        remote_identity,
        config_revision: revision,
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
    let manifest =
        GitStorage::cache_generation_manifest(&context.cache_repository, state.generation)?;
    if manifest != state.manifest {
        return Err(GitError::CommandFailed(
            "fetch-cache active generation manifest mismatch".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn fetch_for_pull(
    repo: &GitStorage,
    progress: &mut SyncCacheProgress,
) -> PullFetchResult {
    #[cfg(test)]
    FETCH_FOR_PULL_ENTRY_COUNT.with(|count| count.set(count.get().saturating_add(1)));

    fetch_for_pull_at(
        repo,
        progress,
        SystemTime::now(),
        PublicationHooks::default(),
    )
}

fn fetch_for_pull_at(
    repo: &GitStorage,
    progress: &mut SyncCacheProgress,
    now: SystemTime,
    hooks: PublicationHooks,
) -> PullFetchResult {
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
            return PullFetchResult::NeutralSkip;
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
    match read_state(&context.state_file) {
        Ok(None) => refresh_as_leader_with_hooks(repo, &context, None, progress, now, hooks),
        Ok(Some(state))
            if state.schema_version == CACHE_SCHEMA_VERSION
                && state.remote_identity == context.remote_identity
                && state_is_semantically_valid(&state) =>
        {
            let now_ms = unix_ms(now);
            if failure_cooldown_active(&state, now_ms, &context.config_revision) {
                log_cache_outcome(&context, "cooldown_reuse", Some(state.generation));
                PullFetchResult::NeutralSkip
            } else if success_is_fresh(&state, now_ms, progress.interval) {
                if !state.manifest.is_empty()
                    && progress.applied_generation != Some(state.generation)
                {
                    if let Err(error) = validate_active_generation(&context, &state) {
                        tracing::debug!(
                            workspace = %context.workspace.display(),
                            outcome = "fallback",
                            reason = "active_generation_invalid",
                            generation = state.generation,
                            error = %error,
                            "shared pull-fetch cache"
                        );
                        return direct_fallback(repo, progress, Some(state.generation));
                    }
                    match repo.import_cache_generation(&context.cache_repository, state.generation)
                    {
                        Ok(()) => {
                            log_cache_outcome(&context, "follower_import", Some(state.generation));
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
                            return direct_fallback(repo, progress, Some(state.generation));
                        }
                    }
                } else {
                    log_cache_outcome(&context, "fresh_reuse", Some(state.generation));
                }
                progress.applied_generation = Some(state.generation);
                PullFetchResult::Ready
            } else {
                refresh_as_leader_with_hooks(repo, &context, Some(&state), progress, now, hooks)
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
    match repo.fetch() {
        Ok(()) => {
            if let Some(generation) = trustworthy_generation {
                progress.applied_generation = Some(generation);
            } else {
                progress.disabled = true;
            }
            PullFetchResult::Ready
        }
        Err(error) => PullFetchResult::RemoteError(error),
    }
}

fn refresh_as_leader(
    repo: &GitStorage,
    context: &CacheContext,
    previous: Option<&CacheState>,
    progress: &mut SyncCacheProgress,
    now: SystemTime,
) -> PullFetchResult {
    refresh_as_leader_with_hooks(
        repo,
        context,
        previous,
        progress,
        now,
        PublicationHooks::default(),
    )
}

fn refresh_as_leader_with_hooks(
    repo: &GitStorage,
    context: &CacheContext,
    previous: Option<&CacheState>,
    progress: &mut SyncCacheProgress,
    now: SystemTime,
    hooks: PublicationHooks,
) -> PullFetchResult {
    log_cache_outcome(
        context,
        "leader_refresh",
        previous.map(|state| state.generation),
    );
    if let Err(error) = repo.fetch_cache_shadow() {
        if let Err(persist_error) =
            publish_failure(context, previous, &error, progress.interval, unix_ms(now))
        {
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
            return direct_fallback(repo, progress, previous.map(|state| state.generation));
        }
    };
    let manifest_changed = previous.is_none_or(|state| state.manifest != manifest);
    let generation = match previous {
        None if manifest.is_empty() => 0,
        None => 1,
        Some(state) if !manifest_changed => state.generation,
        Some(state) => {
            let Some(generation) = state.generation.checked_add(1) else {
                log_cache_outcome(
                    context,
                    "fallback_generation_overflow",
                    Some(state.generation),
                );
                return direct_fallback(repo, progress, Some(state.generation));
            };
            generation
        }
    };
    if manifest_changed && !manifest.is_empty() {
        if let Err(error) = GitStorage::ensure_bare_cache(&context.cache_repository) {
            tracing::debug!(
                workspace = %context.workspace.display(),
                outcome = "fallback",
                reason = "cache_init_failed",
                generation,
                error = %error,
                "shared pull-fetch cache"
            );
            return direct_fallback(repo, progress, previous.map(|state| state.generation));
        }
        if let Err(error) = repo.publish_cache_generation(&context.cache_repository, generation) {
            tracing::debug!(
                workspace = %context.workspace.display(),
                outcome = "fallback",
                reason = "publication_failed",
                generation,
                error = %error,
                "shared pull-fetch cache"
            );
            return direct_fallback(repo, progress, previous.map(|state| state.generation));
        }
    }
    let state = CacheState {
        schema_version: CACHE_SCHEMA_VERSION,
        remote_identity: context.remote_identity.clone(),
        config_revision: context.config_revision.clone(),
        generation,
        manifest,
        completed_at_unix_ms: unix_ms(now),
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
            return direct_fallback(repo, progress, previous.map(|state| state.generation));
        }
    }
    let _ = hooks;
    if let Err(error) = write_state_atomic(&context.state_file, &state) {
        tracing::debug!(
            workspace = %context.workspace.display(),
            outcome = "fallback",
            reason = "state_replace_failed",
            generation,
            error = %error,
            "shared pull-fetch cache"
        );
        return direct_fallback(repo, progress, previous.map(|state| state.generation));
    }
    log_cache_outcome(
        context,
        if manifest_changed {
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
            return direct_fallback(repo, progress, Some(generation));
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
            return direct_fallback(repo, progress, Some(generation));
        }
        log_cache_outcome(context, "leader_import", Some(generation));
        progress.applied_generation = Some(generation);
        cleanup_inactive_generations(context, generation);
    } else {
        progress.applied_generation = Some(generation);
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
    fn discovery_origin_fetch_refspecs_preserve_configuration_order() {
        let fixture = WorkspaceFixture::human();
        fixture.add_fetch_refspec("+refs/pull/*/head:refs/remotes/origin/pull/*");

        assert_eq!(
            fixture
                .storage()
                .origin_fetch_refspecs()
                .expect("read fetch refspecs"),
            vec![
                STANDARD_FETCH_REFSPEC.to_string(),
                "+refs/pull/*/head:refs/remotes/origin/pull/*".to_string()
            ]
        );
    }

    #[test]
    fn discovery_rejects_an_explicit_empty_fetch_refspec() {
        let fixture = WorkspaceFixture::human();
        fixture.add_fetch_refspec("");

        assert_eq!(
            fixture
                .storage()
                .origin_fetch_refspecs()
                .expect("read fetch refspecs"),
            vec![STANDARD_FETCH_REFSPEC.to_string(), String::new()]
        );
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

        #[cfg(unix)]
        fn configure_upload_pack_counter(
            fixture: &OrchestrationFixture,
            repos: &[GitStorage],
        ) -> PathBuf {
            use std::os::unix::fs::PermissionsExt;

            let counter_dir = fixture.workspace.root.path().join("upload-pack-requests");
            std::fs::create_dir_all(&counter_dir).expect("create upload-pack counter");
            let exec_path = git_stdout(fixture.workspace.root.path(), &["--exec-path"]);
            let upload_pack = PathBuf::from(exec_path).join("git-upload-pack");
            assert!(upload_pack.is_file(), "git-upload-pack must exist");

            let script = fixture
                .workspace
                .root
                .path()
                .join("counting-upload-pack.sh");
            std::fs::write(
                &script,
                format!(
                    "#!/bin/sh\nset -eu\n: > {}/request-$$\nexec {} \"$@\"\n",
                    shell_quote(&counter_dir),
                    shell_quote(&upload_pack)
                ),
            )
            .expect("write counting upload-pack");
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
                .expect("make counting upload-pack executable");

            for repo in repos {
                git_config(
                    repo.root(),
                    &["remote.origin.uploadpack", path_arg(&script)],
                );
            }
            counter_dir
        }

        #[cfg(unix)]
        fn shell_quote(path: &Path) -> String {
            format!("'{}'", path_arg(path).replace('\'', "'\"'\"'"))
        }

        #[cfg(unix)]
        fn request_count(counter_dir: &Path) -> usize {
            std::fs::read_dir(counter_dir)
                .expect("read upload-pack counter")
                .count()
        }

        #[cfg(unix)]
        fn run_concurrent_fetches(
            repos: &[GitStorage],
            progresses: &mut [SyncCacheProgress],
            now: SystemTime,
        ) {
            assert_eq!(repos.len(), progresses.len());
            let indices = (0..repos.len()).collect::<Vec<_>>();
            run_concurrent_fetches_for(repos, progresses, &indices, now);
        }

        #[cfg(unix)]
        fn run_concurrent_fetches_for(
            repos: &[GitStorage],
            progresses: &mut [SyncCacheProgress],
            indices: &[usize],
            now: SystemTime,
        ) {
            assert!(!indices.is_empty());
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(indices.len()));
            let handles = indices
                .iter()
                .map(|&index| {
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

            for handle in handles {
                let (index, updated, result) = handle.join().expect("join fetch-cache caller");
                assert!(
                    matches!(
                        result,
                        PullFetchResult::Ready | PullFetchResult::NeutralSkip
                    ),
                    "expected ready or neutral result, got {result:?}"
                );
                progresses[index] = updated;
            }
        }

        #[cfg(unix)]
        fn converge_with_cache_only_cycles(
            repos: &[GitStorage],
            progresses: &mut [SyncCacheProgress],
            expected_main: &str,
            counter_dir: &Path,
            expected_requests: usize,
            now: SystemTime,
        ) {
            for _ in 0..repos.len() {
                let lagging = repos
                    .iter()
                    .enumerate()
                    .filter_map(|(index, repo)| {
                        (git_stdout(repo.root(), &["rev-parse", "refs/remotes/origin/main"])
                            != expected_main)
                            .then_some(index)
                    })
                    .collect::<Vec<_>>();
                if lagging.is_empty() {
                    return;
                }
                run_concurrent_fetches_for(repos, progresses, &lagging, now);
                assert_eq!(request_count(counter_dir), expected_requests);
            }

            assert!(
                repos.iter().all(|repo| {
                    git_stdout(repo.root(), &["rev-parse", "refs/remotes/origin/main"])
                        == expected_main
                }),
                "followers did not converge within bounded cache-only cycles"
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

        fn assert_neutral(result: PullFetchResult) {
            assert!(
                matches!(result, PullFetchResult::NeutralSkip),
                "expected neutral skip, got {result:?}"
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

            assert_neutral(fetch_for_pull_at(
                &repo,
                &mut progress,
                UNIX_EPOCH + Duration::from_secs(10_000),
                PublicationHooks::default(),
            ));

            let waited = started.elapsed();
            assert!(
                waited >= Duration::from_millis(900),
                "contention returned before the bounded wait: {waited:?}"
            );
            assert!(
                waited < Duration::from_millis(1_250),
                "contention exceeded the bounded wait: {waited:?}"
            );
            assert_eq!(progress.applied_generation, None);
            assert!(!progress.disabled);
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

            assert_neutral(fetch_for_pull_at(
                &repo,
                &mut follower_progress,
                now + Duration::from_secs(1),
                PublicationHooks::default(),
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
        fn orchestration_corrupt_bare_fallback_records_trustworthy_generation() {
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
            assert_eq!(progress.applied_generation, Some(1));
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
        fn orchestration_active_generation_manifest_mismatch_uses_direct_fetch() {
            for tamper in ["missing", "altered"] {
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
                let selected = read_state(&context.state_file)
                    .expect("read selected state")
                    .expect("selected state");
                let expected_main =
                    selected.manifest[&format!("{CACHE_SHADOW_HEADS_PREFIX}main")].clone();
                git_ok(
                    follower.root(),
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

                assert_eq!(progress.applied_generation, Some(1), "{tamper}");
                assert!(!progress.disabled, "{tamper}");
                assert_eq!(
                    git_stdout(follower.root(), &["rev-parse", "refs/remotes/origin/main"]),
                    expected_main,
                    "{tamper}"
                );
            }
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
            let counter_dir = configure_upload_pack_counter(&fixture, &repos);
            let mut progresses = vec![SyncCacheProgress::new(30); repos.len()];
            let first_refresh = UNIX_EPOCH + Duration::from_secs(10_000);
            fixture.advance_remote("one-plus\n");
            let origin_main = git_stdout(&fixture.origin, &["rev-parse", "refs/heads/main"]);

            run_concurrent_fetches(&repos, &mut progresses, first_refresh);

            assert_eq!(request_count(&counter_dir), 1);
            converge_with_cache_only_cycles(
                &repos,
                &mut progresses,
                &origin_main,
                &counter_dir,
                1,
                first_refresh,
            );
            for repo in &repos {
                assert_eq!(
                    git_stdout(repo.root(), &["rev-parse", "refs/remotes/origin/main"]),
                    origin_main
                );
            }

            run_concurrent_fetches(
                &repos,
                &mut progresses,
                first_refresh + Duration::from_secs(1),
            );
            assert_eq!(request_count(&counter_dir), 1);

            fixture.advance_remote("two\n");
            let updated_origin_main =
                git_stdout(&fixture.origin, &["rev-parse", "refs/heads/main"]);
            run_concurrent_fetches(
                &repos,
                &mut progresses,
                first_refresh + Duration::from_secs(31),
            );

            assert_eq!(request_count(&counter_dir), 2);
            converge_with_cache_only_cycles(
                &repos,
                &mut progresses,
                &updated_origin_main,
                &counter_dir,
                2,
                first_refresh + Duration::from_secs(31),
            );
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
