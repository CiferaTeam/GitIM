# Shared Fetch Cache Implementation Plan

> **Required execution skill:** Use `superpowers:subagent-driven-development` to implement this plan task by task, with `superpowers:test-driven-development` governing every behavior change.

**Goal:** Coalesce pull-only GitHub fetches from Runtime-managed clones into one
credential-free workspace-local fetch lane without changing direct push,
conflict, epoch, protocol, or frontend behavior.

**Architecture:** A private `fetch_cache` module discovers eligible Runtime
clones, serializes one leader with an advisory file lock, fetches remote branch
heads into a private namespace in the leader clone, publishes changed snapshots
as immutable generations in a local bare repository, and imports the active
generation into follower `origin/*` refs. `start_sync_loop` owns the per-daemon
applied generation. Every cache infrastructure failure falls back to the
existing direct `GitStorage::fetch`; contention and shared failure cooldowns
neutrally skip the cycle.

**Tech Stack:** Rust stable, `gitim-sync`, Git CLI 2.30+, `fs2` advisory locks,
`serde_json` state, `tempfile` atomic replacement, existing `tracing` and
`GitError` classification.

**Global Constraints:**

- Work only in `/Users/lewisliu/ateam/GitIM/.kimi/change-remote-url`.
- Keep `GitStorage::fetch()` direct and preserve all push/recovery/epoch paths.
- Compile cache code only for non-WASM targets.
- Never persist or log PATs or credential-bearing origin URLs.
- Keep workspace configuration schema and every public protocol unchanged.
- Do not prune normal follower `refs/remotes/origin/*`.
- Use scoped verification; do not run workspace-wide `cargo test`.

## File map

| File | Change |
|---|---|
| `crates/gitim-sync/Cargo.toml` | Add native runtime dependencies for advisory locking, JSON state, and atomic temp files. |
| `crates/gitim-sync/src/lib.rs` | Register the private non-WASM `fetch_cache` module. |
| `crates/gitim-sync/src/git.rs` | Add credential-safe raw config reads and branch-snapshot Git primitives. |
| `crates/gitim-sync/src/fetch_cache.rs` | Add eligibility, state, lock, cooldown, immutable generation, fallback, and focused tests. |
| `crates/gitim-sync/src/sync_loop.rs` | Own `SyncCacheProgress` and route only pull-only cycles through the cache. |
| `docs/plans/shared-fetch-cache/00-requirements.md` | Mark implemented tasks and record final verification commands. |

No Runtime, daemon, IPC, HTTP, frontend, workspace-config, or WASM source file
changes are expected.

## Task 1: Establish typed cache discovery and state

**Files:**

- Modify: `crates/gitim-sync/Cargo.toml`
- Modify: `crates/gitim-sync/src/lib.rs`
- Modify: `crates/gitim-sync/src/git.rs`
- Create: `crates/gitim-sync/src/fetch_cache.rs`

### Step 1: Add failing discovery and state tests

Create the private native module and begin with tests for:

1. exact human layout eligibility;
2. exact agent layout eligibility with matching `me.json` handler;
3. local provider fallback;
4. canonical workspace mismatch fallback;
5. malformed or missing config fallback;
6. nested clone fallback;
7. handler or `.gitim/config.yaml` mismatch fallback;
8. origin repository identity mismatch;
9. origin token mismatch;
10. missing, custom, or multiple `remote.origin.fetch` refspecs;
11. future success timestamp is stale;
12. failure cooldown is reusable only for the same config revision;
13. state JSON never contains the token or credential-bearing origin URL.
14. raw `remote.origin.url` is read without applying URL rewrites;
15. all `remote.origin.fetch` values are returned in configuration order.

Use a focused fixture that writes the production workspace layout beneath a
`tempfile::TempDir`, initializes a clone, and configures a raw GitHub-looking
origin without making a network request:

```rust
#[cfg(test)]
struct WorkspaceFixture {
    root: tempfile::TempDir,
    workspace: PathBuf,
    clone_root: PathBuf,
    token: String,
}

#[cfg(test)]
impl WorkspaceFixture {
    fn human() -> Self;
    fn agent(handler: &str) -> Self;
    fn write_config(&self, provider: &str, remote_url: &str, token: &str);
    fn set_origin(&self, raw_url: &str);
    fn add_fetch_refspec(&self, refspec: &str);
}
```

The canonical valid fixture must use:

```text
config remote_url: https://github.com/CiferaTeam/GitIM
raw origin URL:    https://x-access-token:test-pat-123@github.com/CiferaTeam/GitIM.git
fetch refspec:     +refs/heads/*:refs/remotes/origin/*
```

Run:

```bash
cargo test -p gitim-sync fetch_cache::tests::discovery -- --nocapture
cargo test -p gitim-sync fetch_cache::tests::state -- --nocapture
```

Expected: compilation or assertion failure because the cache module and types
do not yet exist.

### Step 2: Add native dependencies and the private module

Move `serde_json` and `tempfile` from dev-only use into the non-WASM dependency
set, and add `fs2`:

```toml
[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
tokio.workspace = true
notify = "7"
rand = "0.9"
libc = "0.2"
fs2 = "0.4"
serde_json.workspace = true
tempfile = "3"

[dev-dependencies]
serde_yaml.workspace = true
```

Register the module in `lib.rs`:

```rust
#[cfg(not(target_arch = "wasm32"))]
mod fetch_cache;
```

### Step 3: Implement the typed data model

Use these constants and private types:

```rust
const CACHE_SCHEMA_VERSION: u32 = 1;
const CACHE_LOCK_FILE: &str = "fetch-cache.lock";
const CACHE_STATE_FILE: &str = "fetch-cache-state.json";
const CACHE_REPOSITORY_DIR: &str = "fetch-cache.git";
const LOCK_WAIT_TIMEOUT: Duration = Duration::from_secs(1);
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(25);
const RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(120);
const MIN_TRANSIENT_COOLDOWN: Duration = Duration::from_secs(3);
const STANDARD_FETCH_REFSPEC: &str =
    "+refs/heads/*:refs/remotes/origin/*";

#[derive(Debug, Clone)]
pub(crate) struct SyncCacheProgress {
    interval: Duration,
    applied_generation: Option<u64>,
    disabled: bool,
    contention_short_retries: u8,
}

impl SyncCacheProgress {
    pub(crate) fn new(interval_secs: u32) -> Self;
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
```

`SyncCacheProgress` is moved through blocking cycles but never serialized.
`CacheState` contains only credential-free identity, ref names/object IDs,
timestamps, and the config file revision.

### Step 4: Implement fail-closed discovery

First add the two credential-safe config reads required by discovery:

```rust
impl GitStorage {
    pub(crate) fn raw_origin_url(&self) -> Result<String, GitError>;
    pub(crate) fn origin_fetch_refspecs(&self) -> Result<Vec<String>, GitError>;
}
```

Both use exact `git config --get remote.origin.url` and
`git config --get-all remote.origin.fetch` invocations, trim output, and
return only generic, credential-free error messages when a command fails.
Never feed config command stderr containing an origin URL into a returned
error.

Add a private tolerant config view:

```rust
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
```

Implement:

```rust
fn discover(repo: &GitStorage) -> Option<CacheContext>;
fn find_workspace(clone_root: &Path) -> Option<PathBuf>;
fn validate_managed_layout(
    workspace: &Path,
    clone_root: &Path,
) -> Option<()>;
fn normalize_config_remote(raw: &str) -> Option<String>;
fn parse_credentialed_origin(raw: &str) -> Option<(String, String)>;
fn config_revision(path: &Path) -> Option<ConfigRevision>;
```

Rules:

- canonicalize both the declared workspace and discovered ancestor;
- accept exactly `.gitim-runtime/human` or `<workspace>/<handler>`;
- for agents, require `.gitim/config.yaml` and a matching `.gitim/me.json`;
- read the raw origin through the Git primitive added in Task 2;
- compare lowercased `github.com/<owner>/<repo>` identities and exact tokens;
- require exactly one standard origin fetch refspec;
- discard the raw origin and token before returning `CacheContext`;
- map every discovery error to `None`, allowing direct fetch.

The parser accepts only HTTPS GitHub URLs, strips one optional `.git` suffix,
and rejects query strings, fragments, missing owner/repository, or extra path
segments. Use `rsplit_once('@')` for the credential boundary so parsing never
includes the raw URL in an error.

### Step 5: Implement timestamp and state helpers

Implement injected-clock helpers so tests do not sleep:

```rust
fn unix_ms(now: SystemTime) -> u64;
fn success_is_fresh(
    state: &CacheState,
    now_ms: u64,
    interval: Duration,
) -> bool;
fn failure_cooldown_active(
    state: &CacheState,
    now_ms: u64,
    revision: &ConfigRevision,
) -> bool;
fn retry_after(
    error: &GitError,
    now_ms: u64,
    interval: Duration,
) -> u64;
fn read_state(path: &Path) -> Result<Option<CacheState>, CacheInfraError>;
fn write_state_atomic(
    path: &Path,
    state: &CacheState,
) -> Result<(), CacheInfraError>;
```

Treat `completed_at_unix_ms > now_ms` and `retry_after_unix_ms > now_ms` with a
future completion timestamp as stale. `write_state_atomic` creates its temp
file in `.gitim-runtime/`, sets mode `0600` on Unix, writes and flushes JSON,
then persists it over the state path.

### Step 6: Run focused tests and commit

Run:

```bash
cargo test -p gitim-sync fetch_cache::tests::discovery -- --nocapture
cargo test -p gitim-sync fetch_cache::tests::state -- --nocapture
cargo fmt --all -- --check
git diff --check
```

Expected: all focused tests pass and diff checks are clean.

Commit:

```text
feat(sync): add shared fetch cache discovery

Test: cargo test -p gitim-sync fetch_cache::tests
Co-authored-by: Codex <codex@openai.com>
```

## Task 2: Add exact branch-snapshot Git primitives

**Files:**

- Modify: `crates/gitim-sync/src/git.rs`
- Modify: `crates/gitim-sync/src/fetch_cache.rs`

### Step 1: Add failing Git primitive tests

Add focused tests with a local bare origin that prove:

- shadow fetch creates exactly
  `refs/gitim-fetch-cache/remote/heads/*`;
- a remote force-push replaces the shadow object ID;
- a deleted remote branch is pruned from the shadow;
- manifest output is sorted and contains only ref name to object ID mappings;
- publishing a generation copies the complete shadow into an immutable bare
  namespace;
- importing a generation force-updates `refs/remotes/origin/*`;
- import does not prune an existing follower-only tracking ref;
- import does not alter symbolic `refs/remotes/origin/HEAD`;
- inactive-generation cleanup leaves the active namespace intact.

Use Git's `url.<file-url>.insteadOf` only to route the GitHub-looking origin to
the local bare fixture. The raw-config assertion must still return the
credential-bearing configured value.

Run:

```bash
cargo test -p gitim-sync git::tests::cache_ -- --nocapture
```

Expected: compilation failure because the new methods do not exist.

### Step 2: Add shadow fetch and manifest methods

Define in `git.rs`:

```rust
const CACHE_REMOTE_HEADS: &str =
    "refs/gitim-fetch-cache/remote/heads";
const CACHE_GENERATIONS: &str =
    "refs/gitim-fetch-cache/generations";

impl GitStorage {
    pub(crate) fn fetch_cache_shadow(&self) -> Result<(), GitError>;
    pub(crate) fn cache_shadow_manifest(
        &self,
    ) -> Result<BTreeMap<String, String>, GitError>;
}
```

`fetch_cache_shadow` runs the existing timeout-aware command path with:

```text
git -c http.lowSpeedLimit=1000 -c http.lowSpeedTime=10 \
  fetch --atomic --no-tags --prune origin \
  +refs/heads/*:refs/gitim-fetch-cache/remote/heads/*
```

Non-zero output must pass through `classify_remote_error`, preserving auth,
rate-limit, timeout, disk-full, and sanitized generic classifications.

The manifest command is:

```text
git for-each-ref --format=%(refname)%00%(objectname) \
  refs/gitim-fetch-cache/remote/heads/
```

Parse each line at its first NUL and reject malformed records with a generic
`CommandFailed("invalid fetch-cache ref manifest")`.

### Step 3: Add immutable generation operations

Add:

```rust
impl GitStorage {
    pub(crate) fn ensure_bare_cache(path: &Path) -> Result<(), GitError>;
    pub(crate) fn publish_cache_generation(
        &self,
        cache_path: &Path,
        generation: u64,
    ) -> Result<(), GitError>;
    pub(crate) fn import_cache_generation(
        &self,
        cache_path: &Path,
        generation: u64,
    ) -> Result<(), GitError>;
    pub(crate) fn cleanup_cache_generations(
        cache_path: &Path,
        active_generation: u64,
    ) -> Result<(), GitError>;
}
```

`ensure_bare_cache` accepts an existing valid bare repository or atomically
recreates a missing/corrupt disposable directory while the caller holds the
workspace lock. It must never write a remote URL into the bare config.

Publication runs from the bare cache:

```text
git fetch --atomic --no-tags <leader-clone-path> \
  +refs/gitim-fetch-cache/remote/heads/*:refs/gitim-fetch-cache/generations/<N>/heads/*
```

Import runs from the follower clone:

```text
git fetch --atomic --no-tags <cache-path> \
  +refs/gitim-fetch-cache/generations/<N>/heads/*:refs/remotes/origin/*
```

Do not pass `--prune` to follower import. Validate that `generation > 0`.
Delete stale generation refs with `git update-ref --stdin` only after state
names the active generation. Cleanup is best effort at the orchestration layer.

### Step 4: Run focused tests and commit

Run:

```bash
cargo test -p gitim-sync git::tests::cache_ -- --nocapture
cargo fmt --all -- --check
git diff --check
```

Expected: all cache Git primitive tests pass.

Commit:

```text
feat(sync): add immutable fetch cache generations

Test: cargo test -p gitim-sync git::tests::cache_
Co-authored-by: Codex <codex@openai.com>
```

## Task 3: Implement locking, refresh, publication, and fallback

**Files:**

- Modify: `crates/gitim-sync/src/fetch_cache.rs`

### Step 1: Add failing orchestration tests

Add tests for:

- first eligible caller becomes leader and publishes generation 1;
- fresh success reuses generation 1 without a remote fetch;
- unchanged remote manifest refreshes the timestamp without incrementing;
- changed remote manifest publishes generation 2;
- a new daemon imports once and then skips import for the applied generation;
- restart with unknown applied generation imports once;
- lock contention waits no longer than one second and returns neutral skip;
- lock open/I/O failure performs direct fetch;
- auth failure writes a five-minute cooldown;
- rate limit writes a two-minute cooldown;
- transient failure writes `max(interval, 3s)` cooldown;
- followers inside a failure cooldown return neutral skip;
- config revision change invalidates the cooldown;
- corrupt state or bare repository takes the direct fallback;
- direct fallback success records a trustworthy state generation as applied;
- direct fallback without trustworthy generation disables cache for the
  process lifetime;
- publication failure before state replacement leaves the old generation
  active;
- state replacement exposes only a complete new generation;
- `.gitim-runtime/` cache artifacts contain neither the token nor the
  credential-bearing origin URL.

Use an injectable clock and a test hook at the state-replacement boundary:

```rust
#[cfg(test)]
#[derive(Clone, Copy, Default)]
struct PublicationHooks {
    before_state_replace: Option<fn() -> Result<(), CacheInfraError>>,
}
```

Production calls pass `PublicationHooks::default()` through a `#[cfg(test)]`
wrapper so no runtime fault-injection API is exposed.

Run:

```bash
cargo test -p gitim-sync fetch_cache::tests::orchestration -- --nocapture
```

Expected: assertion failures because orchestration is not implemented.

### Step 2: Implement bounded advisory locking

Use a stable separate lock file and distinguish contention from infrastructure
failure:

```rust
enum LockAttempt {
    Acquired(CacheLock),
    Contended,
    Failed(CacheInfraError),
}

struct CacheLock {
    file: std::fs::File,
}

fn acquire_lock(path: &Path) -> LockAttempt;
```

Open with create/read/write and mode `0600` on Unix. Poll
`fs2::FileExt::try_lock_exclusive` every 25ms. Return `Contended` after one
second for `WouldBlock`; return `Failed` for all other errors. `Drop` unlocks
best effort. The lock remains held through state decision, remote refresh,
generation publication, follower import, and cleanup.

### Step 3: Implement the cache-aware pull entry point

Add:

```rust
pub(crate) fn fetch_for_pull(
    repo: &GitStorage,
    progress: &mut SyncCacheProgress,
) -> PullFetchResult;
```

Execution order:

1. if `progress.disabled`, run direct fetch;
2. discover eligibility; on `None`, run direct fetch;
3. acquire lock;
4. contention returns `NeutralSkip(RetryContention)` while the bounded
   short-retry budget remains, then `NeutralSkip(PreserveSchedule)`;
5. lock error runs direct fallback;
6. read and validate state schema and remote identity;
7. active same-revision failure cooldown returns
   `NeutralSkip(PreserveSchedule)`;
8. fresh success imports its generation when not yet applied;
9. stale or missing state performs leader refresh;
10. import/publication/cache errors run direct fallback;
11. only an actual remote operation failure returns `RemoteError`.

Use one helper for the rewind guard:

```rust
fn direct_fallback(
    repo: &GitStorage,
    progress: &mut SyncCacheProgress,
    trustworthy_generation: Option<u64>,
) -> PullFetchResult;
```

On direct success, set `applied_generation` to the trustworthy generation. If
none exists, set `disabled = true`. On direct failure, return
`PullFetchResult::RemoteError(error)` so the current daemon's circuit and
backoff still see the failure.

### Step 4: Implement leader refresh and immutable publication

Add:

```rust
fn refresh_as_leader(
    repo: &GitStorage,
    context: &CacheContext,
    previous: Option<&CacheState>,
    progress: &mut SyncCacheProgress,
    now: SystemTime,
) -> PullFetchResult;

fn publish_failure(
    context: &CacheContext,
    previous: Option<&CacheState>,
    error: &GitError,
    interval: Duration,
    now_ms: u64,
) -> Result<(), CacheInfraError>;
```

Publication sequence:

```text
fetch origin -> private pruned shadow
compare sorted manifest
if changed:
    ensure bare cache
    publish generation N refs atomically
    atomically replace state naming N
else:
    atomically replace state with same generation and fresh timestamp
import N when this daemon has not applied N
best-effort delete inactive generation refs
```

Generation 0 is valid only for an initially empty history. An empty changed
snapshot after an existing state advances to the next generation but publishes
and imports no refs; this preserves generation identity for daemons that miss
the empty interval. A failure state preserves the previous active generation
and manifest while replacing `attempt`, completion time, `retry_after`, and
config revision. If failure-state persistence itself fails, return the original
classified remote error.

Keep this correctness diagram as the module comment:

```text
private remote snapshot
        |
        v
immutable generation refs in bare cache
        |
        v
atomic state replacement selects generation
        |
        v
followers import selected generation
        |
        v
inactive generations cleaned best effort
```

### Step 5: Add bounded debug logs

Log only credential-free fields:

- workspace path;
- leader/fresh reuse/unchanged/import/fallback outcome;
- generation number;
- sanitized `GitError`.

Do not log discovery inputs, config JSON, raw ref subprocess arguments, tokens,
or credential-bearing URLs.

### Step 6: Run focused tests and commit

Run:

```bash
cargo test -p gitim-sync fetch_cache::tests -- --nocapture
cargo fmt --all -- --check
git diff --check
```

Expected: all discovery, state, locking, fallback, publication, and hygiene
tests pass.

Commit:

```text
feat(sync): coalesce workspace pull fetches

Test: cargo test -p gitim-sync fetch_cache::tests
Co-authored-by: Codex <codex@openai.com>
```

## Task 4: Integrate loop-owned progress without changing direct paths

**Files:**

- Modify: `crates/gitim-sync/src/sync_loop.rs`

### Step 1: Add failing sync-loop tests

Preserve the public deterministic test API:

```rust
pub fn run_sync_cycle(
    repo: &GitStorage,
    circuit: &mut AuthCircuit,
    commit_lock: &Mutex<()>,
    on_pushed: &dyn Fn(String, String),
    on_renumbered: &dyn Fn(PathBuf, u64, u64),
    on_synced: &dyn Fn(String),
    on_cycle_done: &dyn Fn(),
    rebase_author: Option<&(String, String)>,
) -> SyncOutcome;
```

Add tests proving:

- public `run_sync_cycle` retains direct pull fetch behavior;
- internal cache-aware cycle maps neutral skip to public
  `SyncOutcome::Normal` plus a private cache-neutral sideband;
- the private sideband preserves auth failure count, half-open probe
  eligibility, regular loop delay, and rate-limit/rebase failure counters;
- cache remote auth failure records exactly one failure;
- cache remote rate limit returns `SyncOutcome::RateLimited`;
- cache/direct success clears a half-open auth circuit;
- a pull-only imported generation still executes divergence and rebase logic;
- an unpushed cycle never touches cache state or waits on the cache lock.

Run:

```bash
cargo test -p gitim-sync sync_loop::tests::cache_ -- --nocapture
```

Expected: compilation failure because the internal cache-aware seam is absent.

### Step 2: Add a private cache-aware cycle seam

Refactor the public function into a compatibility wrapper:

```rust
pub fn run_sync_cycle(/* unchanged arguments */) -> SyncOutcome {
    run_sync_cycle_with_cache(
        /* unchanged arguments */,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_sync_cycle_with_cache(
    repo: &GitStorage,
    circuit: &mut AuthCircuit,
    commit_lock: &Mutex<()>,
    on_pushed: &dyn Fn(String, String),
    on_renumbered: &dyn Fn(PathBuf, u64, u64),
    on_synced: &dyn Fn(String),
    on_cycle_done: &dyn Fn(),
    rebase_author: Option<&(String, String)>,
    cache_progress: Option<&mut SyncCacheProgress>,
) -> CacheAwareCycleResult;
```

`CacheAwareCycleResult` carries the unchanged public `SyncOutcome` plus a
private `Option<CacheNeutralHint>`. Pass `cache_progress` only to the pull-only
branch. The push branch and every helper it reaches remain unchanged.

### Step 3: Map typed pull outcomes

Change only the fetch portion of `sync_pull_only`:

```rust
fn sync_pull_only(
    repo: &GitStorage,
    circuit: &mut AuthCircuit,
    commit_lock: &Mutex<()>,
    cache_progress: Option<&mut SyncCacheProgress>,
) -> CacheAwareCycleResult {
    let fetch_result = match cache_progress {
        Some(progress) => fetch_for_pull(repo, progress),
        None => match repo.fetch() {
            Ok(()) => PullFetchResult::Ready,
            Err(error) => PullFetchResult::RemoteError(error),
        },
    };

    match fetch_result {
        PullFetchResult::NeutralSkip(hint) => {
            return CacheAwareCycleResult::neutral(hint);
        }
        PullFetchResult::Ready => observe_auth(circuit, &Ok(())),
        PullFetchResult::RemoteError(error) => {
            let classified = Err(error);
            observe_auth(circuit, &classified);
            // Preserve existing AuthFailed, RateLimited, and generic mapping.
        }
    }

    // Existing divergence guard and rebase body remain byte-for-byte
    // behaviorally equivalent after successful readiness.
}
```

`NeutralSkip` does not call `observe_auth`. The caller restores the pre-cycle
half-open probe timestamp when the private sideband is set. `RetryContention`
installs a one-cycle 100-millisecond delay override without changing the
regular loop delay or the rate-limit and rebase counters.
`PreserveSchedule` installs no override. Another daemon's shared result
therefore leaves this daemon's auth probe and backoff state unchanged, and a
shared failure cooldown never inherits a prior short-retry cadence.

### Step 4: Move progress through `spawn_blocking`

Initialize once beside `AuthCircuit`:

```rust
let mut cache_progress = SyncCacheProgress::new(interval_secs);
```

Move `cache_progress` into each blocking closure and return it:

```rust
let cache_progress_in = cache_progress;
let join_result = tokio::task::spawn_blocking(move || {
    let mut circuit_inner = circuit_in;
    let mut cache_progress_inner = cache_progress_in;
    let outcome = run_sync_cycle_with_cache(
        /* callbacks */,
        Some(&mut cache_progress_inner),
    );
    (outcome, circuit_inner, cache_progress_inner)
})
.await;
```

Copy the cycle result, circuit, and progress back on successful join. When the
result's private cache-neutral sideband is set, retain the existing regular
loop delay, rate-limit counter, and rebase-failure counter. Apply a one-cycle
short-delay override only for `RetryContention`. On the impossible panic path,
recreate only `SyncCacheProgress` with the current interval, which
conservatively causes one later import.

### Step 5: Run sync regression tests and commit

Run:

```bash
cargo test -p gitim-sync sync_loop::tests::cache_ -- --nocapture
cargo test -p gitim-sync --test sync_e2e_test
cargo test -p gitim-sync --test sync_auth_circuit
cargo test -p gitim-sync --test rotate_test
cargo fmt --all -- --check
git diff --check
```

Expected: cache-specific tests and unchanged push/auth/epoch regressions pass.

Commit:

```text
feat(sync): use shared cache for pull-only cycles

Test: cargo test -p gitim-sync sync_loop::tests::cache_
Test: cargo test -p gitim-sync --test sync_e2e_test
Test: cargo test -p gitim-sync --test sync_auth_circuit
Test: cargo test -p gitim-sync --test rotate_test
Co-authored-by: Codex <codex@openai.com>
```

## Task 5: Prove multi-daemon coalescing and compatibility

**Files:**

- Modify: `crates/gitim-sync/src/fetch_cache.rs`
- Modify: `crates/gitim-sync/src/git.rs`
- Modify: `docs/plans/shared-fetch-cache/00-requirements.md`
- Modify: `docs/plans/shared-fetch-cache/01-plan.md`

### Step 1: Add the failing concurrent acceptance test

Build ten eligible clones under one workspace. Configure each clone's
`remote.origin.uploadpack` to a fixture script that:

1. atomically creates a non-overwriting request directory using
   `mkdir request-$$-$suffix`;
2. signals a FIFO after entering upload-pack;
3. waits on a second FIFO before executing the real `git-upload-pack`.

Start the designated leader, wait for the FIFO signal while it holds the cache
lock, then start the other nine callers behind a barrier. Assert all nine
followers return `NeutralSkip(RetryContention)` and:

```rust
assert_eq!(request_count(&counter_dir), 1);
```

Release the leader only after all nine followers have hit the one-second
contention bound. Feed each concrete contention hint through the production
scheduling seam and run only the resulting bounded short-retry cadences at
advanced injected times. Assert every clone converges while the request count
stays one. Never schedule a `PreserveSchedule` result inside the acceptance
boundary.

Repeat all ten callers within the same freshness window and assert the count
remains one. Advance the injected clock beyond freshness, update the remote,
run all ten again, assert the count becomes two, then use the same bounded
cache-only convergence check and assert the count remains two.

The wrapper and assertions must be `#[cfg(unix)]`, matching the workspace's
v1 platform scope. It must not mutate process-global `PATH`.

Run:

```bash
cargo test -p gitim-sync fetch_cache::tests::ten_daemons -- --nocapture
```

Expected: the acceptance test exercises one remote request per freshness
boundary and bounded local convergence.

### Step 2: Close acceptance gaps

Adjust only production behavior required by the failing acceptance test. Keep
the established invariants:

- one bounded lock lane;
- one remote shadow fetch;
- immutable generation publication;
- no direct fetch on contention;
- no follower pruning;
- no credential artifacts;
- direct fallback on cache infrastructure failures.

Run the test until it passes without freshness sleeps or narrow wall-clock
assertions. The upload-pack FIFO deterministically holds the leader through
the followers' real lock waits. Cache-only fan-out uses injected time, concrete
production retry hints, and the bounded contention short-retry budget.

### Step 3: Run the scoped final verification

Run:

```bash
cargo test -p gitim-sync
cargo fmt --all -- --check
cargo clippy -p gitim-sync --all-targets --no-deps --locked
git diff --check
git status --short
```

Expected:

- all `gitim-sync` tests pass;
- formatting and clippy are clean;
- no whitespace errors;
- status contains only intended source and plan changes.

Do not run workspace-wide `cargo test`; this feature is confined to
`gitim-sync` and changes no shared protocol or workspace dependency version.

### Step 4: Update current-state documentation

In `00-requirements.md`:

- mark T1–T5 complete;
- remove any duplicate observability bullet;
- replace planned coverage counts with actual test names and results;
- record the exact final verification commands.

In this plan, append a short `## Implementation result` section containing:

- commit SHAs;
- final scoped test result;
- any deliberate deviation from the file map.

Do not record rejected approaches or abandoned alternatives.

### Step 5: Commit the verified result

Commit:

```text
test(sync): verify shared fetch coalescing

Test: cargo test -p gitim-sync
Test: cargo fmt --all -- --check
Test: cargo clippy -p gitim-sync --all-targets --no-deps --locked
Test: git diff --check
Co-authored-by: Codex <codex@openai.com>
```

## Implementation result

- Task 1: `bf2f1b99680e3de1bfa397545f61ff4bd57ef84c`,
  `2ded42981bde6e0b8b82dcbcdd9f463294511056`.
- Task 2: `25c4ba69ad64ea33b281a0484932f5020b0ca69e`,
  `c002a4c6fb141075d0bdcc9146f3b57afa9d5e3a`.
- Task 3: `191392d9361494c60f0756d7eff02f8e2f490cc8`,
  `4227f4588a2932abbc0b54328eaf02083bdeeb75`.
- Task 4: `f574d1b2b1934e14190c86c8e78cb4a1d9015b6d`,
  `544e116c815923edf741bb725e8f47459ccac83a`.
- Task 5 acceptance and parser coverage:
  `675f35612983d85b56dca336d9eda30ed6cfca00`.
- Task 5 review fix: commit titled `fix(sync): retry cache lock contention`.
- Scoped verification passed:
  `cargo test -p gitim-sync` ran 203 tests,
  `cargo fmt --all -- --check`,
  `cargo clippy -p gitim-sync --all-targets --no-deps --locked`, and
  `git diff --check`.
- Task 5 includes `crates/gitim-sync/src/git.rs` for complete manifest-parser
  rejection coverage. The acceptance test and current-state documents use the
  remaining Task 5 file map.

## Plan self-review

- Every requirement R1–R8 maps to Tasks 1–5.
- Every new branch in the reviewed 16-group coverage map has a named test.
- Public `run_sync_cycle`, `GitStorage::fetch`, workspace config, and protocols
  remain compatible.
- Cache publication order, follower non-pruning, and the ref-rewind guard have
  explicit tests.
- Failure cooldowns preserve existing leader error classification and keep
  follower circuits untouched.
- Native-only dependencies cannot leak into the WASM build.
- No placeholder, deferred decision, new API surface, or unresolved type is
  required to begin implementation.

NO UNRESOLVED DECISIONS
