# Daemon Singleton Requirements

## Goal

Guarantee one live `gitim-daemon` process per repository clone, including concurrent runtime recovery and CLI startup.

## Runtime invariants

- `.gitim/run/gitim.lock` is a stable lock inode and remains present across daemon restarts.
- A daemon holds an exclusive lock for its full process lifetime before publishing its PID or socket.
- The lock owner prepares stale PID, socket, and port artifacts before writing its PID.
- Cleanup removes PID, socket, and port only when the PID file identifies the calling process.
- Runtime process management does not delete per-clone run artifacts after sending signals; the next lifecycle lock owner owns stale-artifact preparation.
- Concurrent clients may spawn competing candidates; exactly one candidate becomes the daemon and every client converges on its socket.
- Clients treat a connectable socket as readiness, use the stable lock to distinguish an owner handoff from a free repository, and use the PID file only for diagnostics.
- A client reaps a lock-losing candidate and retries when the prior owner releases its lease without publishing a replacement socket.
- Candidate exits without an active owner use bounded exponential backoff; candidates still running when startup returns are handed to one shared polling reaper.
- Waiting behind another owner preserves each caller's existing retry schedule, preventing concurrent callers from resetting into a fork herd.
- Directly starting a second daemon exits without changing the active daemon's PID or socket.

## Verification

- Unit tests cover exclusive ownership, lock handoff, owner-scoped cleanup, and stable lock-file lifetime.
- `gitim-client` and `gitim-daemon` scoped test suites pass.
- A process-level smoke test starts concurrent daemon candidates against one temporary initialized Git clone and observes one surviving owner.
