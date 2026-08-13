# Remote Git Slot — Design

**Status**: Implemented
**Date**: 2026-08-13

## Goal

On one node, a workspace may run many **daemon** processes against the same
GitHub remote. Push/fetch already happen inside `gitim-daemon` (`GitStorage` in
`gitim-sync`), not in `gitim-runtime`. Shared fetch cache already coalesces idle
pull-only fetches across those daemons via a workspace file lock. Direct `origin`
operations (`push`, conflict `fetch`, cache-leader shadow fetch, epoch atomic
push) still run per clone and can overlap for the full 120s git timeout.

Cap those direct remote operations to **one in flight per workspace**, using
another cross-process file lock that each daemon acquires itself. A network stall
then cannot multiply into N hung git processes. Local commits, local rebase, and
fetch-cache follower import stay ungated. `gitim-runtime` does not hold or
arbitrate this lock.

## Scope

- Runtime-managed clones that can see a canonical `.gitim-runtime/` ancestor
  (human + agents under one workspace).
- Direct remote git: `GitStorage::push`, `fetch`, `fetch_cache_shadow`,
  `atomic_push_two_refs`.
- Existing shared fetch cache, send-local-then-push, auth circuit, and 120s
  process timeout stay as they are.

Standalone / non-Runtime repositories have no workspace ancestor and keep
today's ungated behavior.

## Mechanism

A second advisory file lock, separate from `fetch-cache.lock`, living in the
shared workspace directory so every daemon on this node can see it:

```text
<workspace>/.gitim-runtime/remote-git.lock
```

The path is under `.gitim-runtime/` only because that directory is already the
workspace-local coordination root (config, fetch-cache, usage). The lock is
taken inside `GitStorage` in the daemon that is about to shell out to git — the
same process that today takes `fetch-cache.lock`.

`GitStorage` discovers the workspace by walking ancestors for
`.gitim-runtime/config.json` (runtime dir is a real directory). If discovery
fails, the remote call runs immediately.

If discovery succeeds:

1. `try_lock_exclusive` with 25ms poll, **wait at most 1s** (same budget as
   fetch-cache lock).
2. On acquire: run the git remote command (still subject to the existing 120s
   kill timeout), then drop the lock.
3. On wait timeout or lock I/O failure: return `GitError::RemoteSlotBusy`
   without starting git.

Follower import from `fetch-cache.git` is a local filesystem fetch and does
not take this lock. Cache leader shadow fetch does.

Lock order is acyclic: cache path takes `fetch-cache.lock` first, then the
remote slot inside `fetch_cache_shadow`. Push / direct fetch / atomic push
take only the remote slot.

## Caller mapping

`RemoteSlotBusy` is a skip, not a protocol failure.

| Caller | Behavior |
| --- | --- |
| `sync_loop` push / pull-direct fetch | Skip this cycle (`SyncOutcome::Normal`). Next interval retries. |
| fetch-cache leader (`fetch_cache_shadow`) | `NeutralSkip(PreserveSchedule)`. Do not latch cache `disabled`. |
| fetch-cache `direct_fallback` | Same skip; do not latch cache `disabled`. |
| Handler `push_with_retry` | Existing "local commit durable, push failed" path. Client sees a transient push error; `sync_loop` retries. |
| Epoch `atomic_push_two_refs` / rotation fetch | Existing fail / `NotReady` paths; next fire retries. |

Do not classify `RemoteSlotBusy` as auth, rate-limit, or timeout.

## Invariants

1. At most one of `{push, fetch, fetch_cache_shadow, atomic_push_two_refs}`
   is in the 120s git wait on a given workspace at a time.
2. A daemon that does not get the slot within 1s does not spawn git.
3. Idle pull-only freshness still goes through shared fetch cache; this lock
   only covers the cache leader's remote shadow fetch plus every direct
   origin call.
4. `gitim send` / card writes still commit locally first. Slot contention
   cannot roll back a durable local commit.

## Tests

- Gate unit: held lock → second acquire returns `RemoteSlotBusy` within ~1s.
- Discovery miss → remote call is not wrapped.
- `sync_loop` / fetch-cache map `RemoteSlotBusy` to skip and leave cache
  enabled.
- Direct `fetch`/`push` still succeed when the lock is free (existing git
  tests keep covering the happy path).

## Out of scope

Handler requests that *do* acquire the slot still run the 120s git timeout
on that one daemon. Shortening that timeout is a separate change.
