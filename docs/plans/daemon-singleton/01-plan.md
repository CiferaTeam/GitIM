# Daemon Singleton Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make daemon startup and shutdown preserve exactly one process owner per clone.

**Architecture:** `DaemonLifecycle::acquire` opens the persistent lock file, takes an exclusive OS lock, prepares stale run artifacts, and writes the owner PID before returning a lease retained by `main`. Cleanup checks PID ownership and preserves the lock inode. Client startup delegates stale-artifact preparation to the lock owner.

**Tech Stack:** Rust stable, `fs2` file locking, existing daemon/client unit tests.

---

### Task 1: Lock-backed lifecycle ownership

**Files:**
- Modify: `crates/gitim-daemon/Cargo.toml`
- Modify: `crates/gitim-daemon/src/error.rs`
- Modify: `crates/gitim-daemon/src/lifecycle.rs`

- [x] **Step 1: Write failing lifecycle tests**

Add tests that acquire one lease, reject a second acquisition, reacquire after lease drop, preserve `gitim.lock`, and restrict cleanup to the PID owner.

- [x] **Step 2: Verify RED**

Run: `cargo test -p gitim-daemon --lib lifecycle`

Expected: compilation fails because `DaemonLifecycle::acquire` and the lease type do not exist.

- [x] **Step 3: Implement the lifecycle lease**

Add `fs2 = "0.4"`. Implement `DaemonLifecycle::acquire() -> Result<DaemonLease, DaemonError>` using `OpenOptions`, `try_lock_exclusive`, stale-artifact preparation, and PID publication. Keep the locked file in `DaemonLease` until process exit.

- [x] **Step 4: Scope cleanup to the PID owner**

Make `cleanup` compare `.gitim/run/gitim.pid` with `std::process::id()` before removing PID, socket, and port. Preserve `gitim.lock`.

- [x] **Step 5: Verify GREEN**

Run: `cargo test -p gitim-daemon --lib lifecycle`

Expected: all lifecycle tests pass.

### Task 2: Wire daemon and client startup

**Files:**
- Modify: `crates/gitim-daemon/src/main.rs`
- Modify: `crates/gitim-client/src/daemon.rs`

- [x] **Step 1: Acquire before daemon initialization**

Replace the PID check/write window in `main` with one early `DaemonLifecycle::acquire` call and retain the returned lease for the lifetime of `main`.

- [x] **Step 2: Remove client-side artifact deletion**

Remove `STALE_FILES` and `clean_stale_files`; competing clients spawn candidates while the daemon lock chooses the single owner.

- [x] **Step 3: Run scoped suites**

Run: `cargo test -p gitim-client --lib` and `cargo test -p gitim-daemon --lib`.

Expected: both suites pass.

### Task 3: Process-level regression

**Files:**
- Test: `crates/gitim-daemon/tests/daemon_singleton.rs`

- [x] **Step 1: Write a concurrent-start regression test**

Create a temporary initialized clone, launch two daemon candidates concurrently, wait for the socket, and assert that the PID file names one live process while the competing process exits.

- [x] **Step 2: Verify RED/GREEN sensitivity**

Run the test against the fixed implementation, then temporarily restore the old check/write startup behavior to confirm that the regression fails before restoring the fix.

### Task 4: Recover clients across daemon owner handoff

**Files:**
- Modify: `crates/gitim-client/Cargo.toml`
- Modify: `crates/gitim-client/src/daemon.rs`
- Modify: `crates/gitim-runtime/src/workspace.rs`

- [x] **Step 1: Make socket and lock state authoritative**

Use a connectable Unix socket as the readiness signal. Probe the persistent lifecycle lock to wait for a running, starting, or stopping owner; do not gate startup on PID liveness.

- [x] **Step 2: Reap and retry competing candidates**

Retain each spawned child while waiting, reap a lock-losing candidate, and retry within one global startup deadline when the lock becomes free without a socket. Hand a still-running child to a shared polling reaper when startup returns, and apply bounded exponential backoff when candidates fail without another owner.

Preserve a caller's retry schedule while it waits behind an owner. Runtime shutdown sends signals but leaves PID/socket cleanup to the next lifecycle lock owner, so a concurrently started replacement keeps its published artifacts.

- [x] **Step 3: Verify client behavior**

Run: `cargo test -p gitim-client --lib` and `cargo clippy -p gitim-client --all-targets --no-deps --locked`.

Expected: all checks pass, including stale-live-PID and owner-handoff coverage.

### Task 5: Final verification

- [x] **Step 1: Run final checks**

Run: `cargo fmt --all -- --check`, `cargo test -p gitim-client --lib`, `cargo test -p gitim-daemon --test daemon_singleton`, `cargo test -p gitim-runtime --lib workspace::tests`, and `git diff --check`.

Expected: commands exit successfully with no failures.

## GSTACK REVIEW REPORT

| Review | Trigger | Why | Runs | Status | Findings |
|--------|---------|-----|------|--------|----------|
| CEO Review | `/plan-ceo-review` | Scope & strategy | 0 | N/A | Not required for this runtime correctness fix |
| Codex Review | `/codex review` | Independent 2nd opinion | 1 | CLEAR | 0 actionable findings |
| Eng Review | `/plan-eng-review` | Architecture & tests (required) | 7 | CLEAR | Singleton, handoff, reaping, backoff, and runtime cleanup paths cleared after fixes |
| Design Review | `/plan-design-review` | UI/UX gaps | 0 | N/A | No UI changes |
| DX Review | `/plan-devex-review` | Developer experience gaps | 0 | N/A | No developer workflow changes |

- **CODEX:** Confirmed the lifecycle lease, stable lock inode, socket readiness, and candidate reaping with no actionable correctness regressions.
- **UNRESOLVED:** 0
- **VERDICT:** CODEX + ENG CLEARED — ready to ship.
