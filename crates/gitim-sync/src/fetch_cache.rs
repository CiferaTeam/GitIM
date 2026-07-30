#![allow(dead_code)]

use crate::git::{GitError, GitStorage};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CACHE_SCHEMA_VERSION: u32 = 1;
const CACHE_LOCK_FILE: &str = "fetch-cache.lock";
const CACHE_STATE_FILE: &str = "fetch-cache-state.json";
const CACHE_REPOSITORY_DIR: &str = "fetch-cache.git";
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
                "refs/heads/main".to_string(),
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
}
