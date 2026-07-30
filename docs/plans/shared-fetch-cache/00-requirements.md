# Shared Fetch Cache — Requirements

Status: IMPLEMENTED
Date: 2026-07-31

## Problem

A Runtime workspace owns one human clone and zero or more agent clones. Each clone
runs an identity-bound daemon, and every daemon performs a pull-only
`git fetch origin` on its sync cadence even when the remote has not changed.

With ten active agents and the default three-second cadence, the node can start
roughly 150–200 remote fetch commands per minute while idle. The commands usually
transfer few objects, but still consume Git subprocesses, remote ref negotiation,
connections, and authentication checks.

## Goal

Coalesce pull-only remote fetches from Runtime-managed clones into one
workspace-local fetch lane while preserving each daemon's clone, identity,
permission checks, push path, and conflict handling.

For `N` idle daemons in one workspace, each freshness boundary should perform
at most one coalesced remote fetch. Followers update their `origin/*` tracking
refs from a disposable local cache after they acquire the shared lock and the
remote-tracking branch set has changed.

## Current Constraints

- A daemon owns one `GitStorage`, one `current_user`, and one working clone.
- Sync uses `origin/*` tracking refs for divergence checks, rebases, and epoch
  rotation.
- Workspace GitHub credentials remain sourced from
  `.gitim-runtime/config.json`; clone `remote.origin.url` values are derived
  credential-bearing URLs.
- Runtime-managed clone layouts are:
  - human: `<workspace>/.gitim-runtime/human`
  - agent: `<workspace>/<handler>`
- Standalone daemon and CLI repositories do not necessarily have a Runtime
  workspace ancestor.

## Requirements

### R1 — Pull-only integration

The shared lane is used by the pull-only path when the clone has no unpushed
commits.

The following operations retain fresh, direct access to `origin`:

- daemon startup epoch recovery;
- push;
- non-fast-forward recovery;
- conflict resolution and rebase retries;
- epoch rotation and redirect fencing.

`GitStorage::fetch()` keeps its current direct-fetch semantics. The pull-only
path calls a separate cache-aware operation.

A successful push does not acquire the cache lock or invalidate shared cache
state. Other pull-only daemons may observe that remote update up to one
freshness window plus the existing sync-loop scheduling jitter later. A daemon
with unpushed commits and all epoch or conflict safety paths still access
`origin` directly.

### R2 — Workspace-local discovery

A clone is eligible when an ancestor contains a readable
`.gitim-runtime/config.json` whose declared `workspace` canonicalizes to that
ancestor, whose `git.provider` is `github`, and whose credential-free
`git.remote_url` and token are non-empty.

The clone root must canonicalize to one of the Runtime-managed layouts:

- `<workspace>/.gitim-runtime/human`; or
- `<workspace>/<handler>`, with `.gitim/config.yaml` present and the directory
  name equal to the handler in `.gitim/me.json`.

Its `remote.origin.fetch` configuration must contain exactly the standard
Runtime clone refspec:

```text
+refs/heads/*:refs/remotes/origin/*
```

Repositories merely nested somewhere below a workspace, single-branch clones,
and clones with custom or multiple fetch refspecs are not eligible.

The raw `remote.origin.url` is read without URL rewriting and normalized
in-memory to a lowercased `github.com/<owner>/<repo>` identity plus its
credential token. Both the repository identity and token must match the
workspace configuration before any cache read or write. Raw credential-bearing
URLs and tokens are never included in errors or logs.

The cache is stored under that workspace's `.gitim-runtime/` directory. Local
mode workspaces and standalone repositories continue through the direct-fetch
path.

Discovery uses a private tolerant JSON view of the workspace path, provider,
remote URL, and token, and ignores unrelated configuration fields. Missing,
malformed, mismatched, or inaccessible workspace state must preserve the
current direct-fetch behavior.

### R3 — One fetch leader per freshness window

An advisory file lock on a stable, separate lock file serializes cache use
across daemon processes. Each eligible daemon polls `try_lock_exclusive` for at
most one second. Ordinary contention or a wait timeout returns a neutral cache
skip with a contention retry hint and never triggers a direct fetch. The async
loop schedules a 100-millisecond retry without changing its regular cadence,
auth circuit, rate-limit counter, or rebase-failure counter. A daemon performs
at most three consecutive short retries before returning to its regular
schedule. Acquiring the lock or taking any non-contention path resets that
budget.

The lock holder keeps the lock through the freshness decision, leader remote
fetch, cache publication and state write, and any follower import into its
clone. The first eligible daemon after the freshness deadline becomes the
leader and runs `git fetch origin` in its own clone.

For a successful attempt, shared state records only the completion timestamp,
sampled after the immutable generation has been published, not a
leader-specific freshness duration. Failed-attempt completion and retry times
are sampled after the remote attempt. Freshness is sampled only after the
cache lock is acquired. Each lock holder compares the recorded completion with
its own configured sync interval. The shortest interval among active daemons
therefore bounds the workspace's healthy remote refresh cadence while slower
daemons reuse the same result.

State is atomically replaced only after the refresh attempt completes, so a
crashed leader releases the OS lock without publishing a partial success.

The state records the most recent attempt class and an explicit `retry_after`
for failures:

- authentication failure: five minutes, matching the existing auth probe
  interval;
- rate limit: two minutes, matching the existing maximum backoff;
- transient remote failure: the greater of the leader's sync interval and
  three seconds.

Followers before `retry_after` return a neutral skip that preserves the regular
loop schedule without starting another remote fetch or mutating their own auth
circuit or rate-limit backoff. A workspace-config file revision change
invalidates a stored failure cooldown so token rotation can probe immediately.
A state timestamp from the future is treated as stale rather than suppressing
fetch indefinitely.

When many daemons contend in one scheduler tick, the one-second bound may cause
some followers to neutrally skip before importing the selected generation.
When the leader completes within a follower's short-retry budget, that follower
reuses the fresh generation locally and converges inside the same freshness
window. If contention persists through the budget, the follower returns to its
regular cadence. That cadence may cross the next freshness boundary and start
one new coalesced remote fetch.

### R4 — Credential-free cache

The leader uses its verified clone `origin` for remote access. The remote fetch
writes only to a private shadow namespace in that clone. The shared bare
repository imports the shadow refs and objects through a local filesystem path.

Every published generation stores the credential-free `git.remote_url` from
the workspace configuration as its remote identity. The lock holder validates
that identity before any cache import or publication. PAT values and
credential-bearing URLs must not appear in:

- the bare cache's Git config;
- cache metadata;
- lock files;
- logs or returned errors.

The clone's credential-bearing `origin` URL is used only by the Git subprocess
and the in-memory eligibility comparison. Token rotation preserves the
configured remote identity. An
identity mismatch disables the shared lane without reading, publishing, or
reinitializing the cache; that cycle and subsequent cycles use direct fetch.
Remote URL migration remains outside this feature and requires the workspace's
existing rebuild flow.

### R5 — Ref publication and follower import

The leader runs one remote operation:

```text
git fetch --atomic --no-tags --prune origin \
  +refs/heads/*:refs/gitim-fetch-cache/remote/heads/*
```

The private shadow namespace is therefore an exact snapshot of currently
advertised remote branches without changing the clone's normal
`refs/remotes/origin/*`. Tags are not part of the shared cache.

After a successful remote fetch, the leader snapshots the private branch
namespace, then compares it with the last published manifest.

- Changed manifest: publish the fetched refs and objects to a new immutable
  generation namespace in the bare cache with a second `git fetch --atomic`
  over a local filesystem path, then atomically replace state with the
  incremented generation.
- Unchanged manifest: advance the freshness timestamp without incrementing the
  generation.

Published refs live under
`refs/gitim-fetch-cache/generations/<generation>/heads/*`. State is the commit
marker naming the active immutable generation. The lock is held across both Git
transactions, state replacement, follower import, and best-effort removal of
inactive generation refs.

A crash before state replacement leaves the previous generation fully usable;
a crash after state replacement leaves the new generation fully usable.
Orphan generation refs and unreferenced objects are harmless and are cleaned
under a later lock. No follower reads refs that do not match the generation
named by state.

Each daemon's `start_sync_loop` owns a `SyncCacheProgress` value containing its
configured interval and an in-memory `applied_generation`. The value moves into
and back out of each blocking sync cycle alongside the existing auth circuit;
`GitStorage` remains a path-only Git operation wrapper.

- Generation differs from `applied_generation`: atomically import that
  generation's cached branches into `refs/remotes/origin/*`, then advance
  `applied_generation`.
- Generation equals `applied_generation`: skip cache import.

After either case, the pull-only path retains its existing divergence and
rebase checks. A daemon restart initializes `applied_generation` as unknown and
performs one import. A successful direct fallback records the state generation
as applied because direct fetch is at least as fresh; if no trustworthy
generation was readable, that daemon disables the cache for its remaining
process lifetime.

The leader shadow and each immutable cache generation contain the exact current
remote branch set, including `main-epoch-*`. Follower import intentionally does
not prune normal tracking refs, preserving current plain-fetch deletion
behavior. The generation namespace contains no `HEAD`, so the clone's
`refs/remotes/origin/HEAD` symbolic ref is untouched. Tag behavior remains
outside the shared lane.

### R6 — Disposable failure boundary

The cache is an optimization layer and is never a push target.

Failure to open or acquire the lock because of an I/O or operating-system
error, plus metadata, initialization, publication, or follower-import failure,
falls back to the direct `git fetch origin` path for that daemon. Contention
and the bounded wait timeout are neutral skips, not cache failures. A corrupted
cache is recreated under the workspace lock when possible.

The daemon that performs a failed remote fetch retains the existing `GitError`
classification so its auth circuit and rate-limit backoff behavior remains
intact. The failure class stored in shared state is only a request-suppression
signal and never changes another daemon's circuit state.

### R7 — Compatibility

The feature changes no:

- GitIM file or wire format;
- remote Git refs or commit history;
- daemon IPC;
- Runtime HTTP API;
- frontend behavior;
- workspace configuration schema.

Old and new daemons may operate against the same remote. Old daemons fetch
directly; new daemons share the local lane when they run on the same node.

Deleting the cache files causes the next eligible leader to recreate the
disposable cache. Direct-fetch compatibility remains available through every
eligibility and cache-failure fallback.

### R8 — Bounded observability

Debug-level logs distinguish:

- leader remote refresh;
- fresh-window reuse;
- unchanged generation;
- follower import;
- cache fallback.

Logs include the workspace path and generation where useful, and pass all
remote-derived text through credential redaction. No new health endpoint or
persistent metric is required.

## What already exists

- `GitStorage::fetch()` provides the unchanged direct-fetch fallback and the
  existing 120-second Git subprocess timeout and remote-error classification.
- `sync_loop` already separates pull-only and push paths, owns the auth circuit,
  and moves mutable per-loop state into and back out of `spawn_blocking`.
- Runtime `config.json` is the PAT source of truth, and token propagation keeps
  clone origins aligned during supported startup and provisioning flows.
- `fs2` advisory locks and atomic temporary-file replacement are established
  patterns in `gitim-core` and `gitim-runtime`.
- `gitim-sync` already has local bare-repository fixtures and end-to-end sync,
  auth-circuit, conflict, and epoch-rotation tests.

The feature reuses these paths rather than introducing a scheduler, daemon
coordinator, network service, or new configuration surface.

## Data flow

```text
run_sync_cycle
    |
    +-- has unpushed commits? -- yes --> existing direct push/fence/recovery
    |
    `-- no --> pull-only cache entry
                  |
                  +-- eligible layout/config/origin/token/refspec? -- no --> direct fetch
                  |
                  `-- yes --> try shared lock for <= 1s
                                |
                                +-- contended timeout --> bounded short-retry hint
                                +-- lock/cache I/O error --> direct fetch
                                |
                                `-- locked --> validate state identity
                                              |
                                              +-- identity mismatch --> direct fetch
                                              +-- failure retry_after active --> preserve cadence
                                              +-- fresh success --> active generation
                                              |
                                              `-- stale --> remote shadow fetch
                                                            |
                                                            +-- failure
                                                            |     `-- publish cooldown
                                                            |
                                                            `-- success
                                                                  |
                                                                  +-- manifest unchanged
                                                                  |     `-- refresh state
                                                                  |
                                                                  `-- manifest changed
                                                                        |
                                                                        +-- publish immutable refs
                                                                        `-- commit state generation

active generation
    |
    +-- already applied by this daemon --> skip cache import
    `-- new to this daemon -------------> atomic local import
                                               |
                                               `-- existing divergence + rebase checks
```

The immutable publication sequence is:

```text
private remote snapshot
        |
        v
bare refs/gitim-fetch-cache/generations/N/heads/*   (not visible yet)
        |
        v
atomic state.json replacement: active_generation = N
        |
        v
followers may import generation N
        |
        v
best-effort cleanup of inactive generation refs
```

`fetch_cache.rs` should retain this diagram as an inline module comment because
the state/ref publication order is a correctness invariant.

## Failure modes

| Code path | Production failure | Covered by test | Handling | User-visible effect |
|---|---|---:|---|---|
| Eligibility | Moved workspace, malformed config, wrong origin/token/handler/refspec | Yes | Direct fetch | Existing sync behavior |
| Lock | Another leader is slow | Yes | Neutral skip after one second | Update waits for a later cycle |
| Lock/cache I/O | Permission or filesystem error | Yes | Direct fetch; disable cache when generation is unknown | Existing sync behavior plus warning |
| Remote shadow fetch | Auth failure | Yes | Leader circuit observes error; shared five-minute cooldown | Existing auth log from leader |
| Remote shadow fetch | Rate limit | Yes | Leader backoff plus shared two-minute cooldown | Existing rate-limit log |
| Remote shadow fetch | Timeout/transient failure | Yes | Shared bounded transient cooldown | Existing warning from leader |
| State clock | Wall clock moves backward | Yes | Future timestamp treated as stale | One new leader probe |
| Publication | Process dies before state replacement | Yes | Prior immutable generation remains active | At most one freshness-window delay |
| Publication | Process dies after state replacement | Yes | New immutable generation is complete | No partial refs |
| Import | Clone ref update fails | Yes | Direct fetch; mark readable generation applied on success | Existing sync behavior plus warning |
| Integration | Rebase or divergence check fails | Existing regression suite | Existing retry/backoff behavior | Existing warning/backoff |
| Identity | Config remote changes while cache exists | Yes | Fail closed to direct fetch | Cache remains disabled |

No path lacks both error handling and implemented coverage; there are no silent
critical gaps.

## Test coverage

The reviewed 16 new-path groups have implemented coverage:

- Eligibility and state:
  `discovery_accepts_exact_human_layout`,
  `discovery_accepts_exact_agent_layout_with_matching_handler`,
  `discovery_requires_one_standard_fetch_refspec`,
  `discovery_falls_back_for_origin_repository_identity_mismatch`,
  `discovery_falls_back_for_origin_token_mismatch`,
  `state_future_success_timestamp_is_stale`, and
  `state_failure_cooldown_requires_same_config_revision`.
- Git parsing and snapshots:
  `cache_manifest_parser_rejects_malformed_records`,
  `cache_generation_manifest_parser_rejects_malformed_records`,
  `cache_shadow_fetch_force_updates_and_prunes_deleted_branches`, and
  `cache_generation_publish_and_import_preserve_follower_only_refs_and_head`.
- Locking, refresh, and cooldowns:
  `orchestration_lock_contention_is_bounded_and_neutral`,
  `orchestration_lock_open_failure_uses_direct_fetch`,
  `orchestration_fresh_success_reuses_generation_without_remote_fetch`,
  `orchestration_unchanged_manifest_refreshes_timestamp_without_incrementing`,
  `orchestration_changed_manifest_publishes_generation_two`,
  `orchestration_auth_failure_publishes_five_minute_cooldown`,
  `orchestration_rate_limit_publishes_two_minute_cooldown`, and
  `orchestration_transient_failure_uses_interval_with_three_second_floor`.
- Recovery and publication:
  `orchestration_corrupt_state_falls_back_and_disables_cache_for_process`,
  `orchestration_active_generation_manifest_mismatch_uses_direct_fetch`,
  `orchestration_publication_failure_keeps_old_generation_active`,
  `orchestration_state_replacement_selects_only_complete_generation`, and
  `orchestration_cache_artifacts_exclude_credentials`.
- Loop integration:
  `cache_neutral_skip_does_not_record_auth_observation`,
  `cache_half_open_neutral_skip_preserves_probe_eligibility`,
  `cache_neutral_schedule_retries_contention_and_preserves_backoff_state`,
  `cache_failure_cooldown_preserves_regular_schedule`,
  `cache_unpushed_cycle_resets_contention_retry_window`,
  `cache_imported_generation_still_rebases_pull_only_clone`, and
  `cache_unpushed_cycle_bypasses_cache_lock_and_state`.
- Unix multi-daemon acceptance:
  `ten_daemons_coalesce_remote_fetches_and_converge` proves one upload-pack
  request at the first boundary while a FIFO gate keeps the leader inside
  upload-pack until nine followers hit one real lock-contention result, then
  releases the leader and proves those followers converge through the
  production short-retry scheduler. It also proves no additional request in
  the fresh window and exactly one more request after the injected clock
  crosses freshness.

Remote request counting uses a per-clone `remote.origin.uploadpack` wrapper
that creates atomic non-overwriting request directories, synchronizes through
FIFO gates, and executes the absolute `git-upload-pack` path against a local
bare remote. The tests keep process-global `PATH` unchanged and use injected
clocks for freshness decisions and production retry hints for later cadence.

## Acceptance Criteria

1. Ten concurrent eligible pull-only cycles in one workspace cause exactly one
   remote fetch at a freshness boundary. When the leader completes within the
   bounded production contention-retry schedule, followers converge in the
   same freshness window without another remote fetch.
2. A remote branch update increments the cache generation and becomes visible
   when each follower next acquires the cache lock. A follower that exhausts
   its short-retry budget returns to regular cadence; crossing the next
   freshness boundary may start one new coalesced remote fetch.
3. An unchanged remote ref manifest keeps the generation stable, and followers
   that already applied it start no remote-fetch or cache-import subprocess.
4. A leader auth failure suppresses remote fetches for five minutes, a rate
   limit suppresses them for two minutes, and both leave follower circuit state
   unchanged.
5. Concurrent daemons either acquire the shared lock within one second or
   neutrally skip without starting direct fetches; an actual lock I/O,
   metadata, bare-repository, or import failure falls back to direct fetch.
6. A credential scan of `.gitim-runtime/` cache artifacts and test logs finds
   no PAT or credential-bearing URL added by this feature.
7. A workspace identity, origin identity, token, agent-handler, or fetch-refspec
   mismatch disables cache reads and writes.
8. Current remote branches and `main-epoch-*` refs have identical object IDs in
   the leader shadow and active generation; deleted branches are absent from
   both while existing follower tracking refs remain unpruned.
9. A custom fetch refspec disables the cache.
10. Existing `gitim-sync` push, conflict, auth-circuit, and epoch-rotation tests
    pass unchanged.
11. A successful push never waits for or mutates the cache; a pull-only
    follower observes it after the next cache refresh.
12. Process termination at each publication boundary leaves state pointing to
    a complete immutable generation.
13. A direct fallback never allows a later import of the same or an unknown
    older generation to rewind that clone's tracking refs.

## NOT in scope

This feature covers Runtime-managed GitHub remotes and pull-only remote fetch
coalescing. Each daemon still performs the existing local unpushed-commit,
divergence, rebase, and synced-HEAD checks required by its independent working
clone. Daemon lifecycle parking, adaptive sync cadence, shared identity
hosting, tag propagation, and remote migration are separate changes.

- Shared multi-identity daemon ownership — identity and working-clone
  boundaries stay unchanged.
- Adaptive or parked daemon cadence — local safety probes retain their current
  schedule.
- Push caching or cache invalidation — push and all safety recovery paths stay
  direct.
- Tag propagation — the cache snapshot contains branches only.
- Remote migration — cache identity mismatch fails closed to direct fetch.

## Parallelization

Sequential implementation, no parallelization opportunity. The cache module,
Git primitives, sync-loop outcome mapping, and tests share the same
`gitim-sync` state machine and must land in dependency order.

## Retrospective

Recent sync history contains multiple fixes for failed conflict replay, epoch
fencing, and concurrent convergence. This implementation therefore keeps every
push, conflict, and epoch path direct and limits shared behavior to the
pull-only fetch boundary.

## Implementation Tasks

Synthesized from this review's findings. Each task derives from a specific
finding above.

- [x] **T1 (P1, human: ~2h / CC: ~20min)** — cache core — Implement typed discovery, identity validation, lock, state, cooldown, and immutable-generation publication
  - Surfaced by: Architecture Review — cross-repository poisoning, stale ref union, failure herd, lock outage, and crash-boundary findings.
  - Files: `crates/gitim-sync/src/fetch_cache.rs`, `crates/gitim-sync/src/lib.rs`, `crates/gitim-sync/Cargo.toml`
  - Commits: `bf2f1b99`, `2ded4298`, `191392d9`, `4227f458`.
  - Verified by: focused `fetch_cache` unit tests.
- [x] **T2 (P1, human: ~1.5h / CC: ~15min)** — Git primitives — Add credential-safe raw config reads, private shadow fetch, ref manifest, generation publication, and follower import
  - Surfaced by: Architecture Review — exact branch snapshot and atomic publication requirements.
  - Files: `crates/gitim-sync/src/git.rs`
  - Commits: `25c4ba69`, `c002a4c6`; parser rejection coverage is included in
    `test(sync): verify shared fetch coalescing`.
  - Verified by: focused Git primitive tests with branch add, force-update,
    deletion, and malformed manifests.
- [x] **T3 (P1, human: ~1.5h / CC: ~15min)** — sync integration — Own `SyncCacheProgress` in `start_sync_loop` and map typed cache outcomes without changing direct safety paths
  - Surfaced by: Code Quality and Performance Reviews — explicit loop-owned state and preserved local integration.
  - Files: `crates/gitim-sync/src/sync_loop.rs`
  - Commits: `f574d1b2`, `544e116c`.
  - Verified by: pull-only cache outcome tests plus unchanged push, auth, and
    epoch tests.
- [x] **T4 (P1, human: ~2h / CC: ~20min)** — concurrency and failure tests — Prove request coalescing, cooldowns, immutable crash safety, fallbacks, and credential hygiene
  - Surfaced by: Test Review — 16 uncovered new branch groups.
  - File: `crates/gitim-sync/src/fetch_cache.rs`.
  - Commit: `675f35612983d85b56dca336d9eda30ed6cfca00`.
  - Verified by: `cargo test -p gitim-sync`.
- [x] **T5 (P2, human: ~30min / CC: ~5min)** — compatibility — Run formatting, scoped clippy/tests, diff hygiene, and update current-state documentation
  - Surfaced by: Compatibility Review — branch-only cache must not alter direct paths or public protocols.
  - Files: `crates/gitim-sync/src/fetch_cache.rs`,
    `crates/gitim-sync/src/git.rs`,
    `docs/plans/shared-fetch-cache/00-requirements.md`,
    `docs/plans/shared-fetch-cache/01-plan.md`.
  - Commits: `675f35612983d85b56dca336d9eda30ed6cfca00`,
    `d575fd9b321528874b561b6ec7e84cd75de1999d`;
    retry-budget reset fix titled `fix(sync): reset cache retry budget`.
  - Verified by: `cargo test -p gitim-sync`;
    `cargo fmt --all -- --check`;
    `cargo clippy -p gitim-sync --all-targets --no-deps --locked`;
    `git diff --check`.

## Final verification

- `cargo test -p gitim-sync` — pass, 204 tests across unit and integration
  targets.
- `cargo fmt --all -- --check` — pass.
- `cargo clippy -p gitim-sync --all-targets --no-deps --locked` — pass.
- `git diff --check` — pass.
- `git status --short` — only the intended `gitim-sync` tests and
  shared-fetch-cache plan documents are present.
