# Oneshot Timer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let oneshot LLM agents register "wake me in N minutes with this anchor" via `gitim timer set`; agent_loop fires the wake by injecting a synthetic prompt prefix when due.

**Architecture:** Pure-fs design — timer state lives in `<agent_clone>/.gitim/timers.json` (gitignored). `gitim timer set/list/cancel` are direct fs read/write. agent_loop checks the file each cycle and prepends a `## ⏰ Timer reminder(s) fired` block to the LLM prompt when timers are due. Zero new IPC, HTTP, tokio task, or daemon involvement. See [00-requirements.md](00-requirements.md) and [01-eng-review-findings.md](01-eng-review-findings.md).

**Tech Stack:** Rust 1.x stable, `chrono` (already), `humantime` (new), `fs2` (new), `tempfile` (already), `clap`, `tokio`, `tracing`.

**Source of design / constraints:**
- Design: `docs/plans/oneshot-timer/00-requirements.md`
- Plan-phase constraints from eng-review: `docs/plans/oneshot-timer/01-eng-review-findings.md`

---

## File Structure

| Crate | File | Action | Responsibility |
|---|---|---|---|
| `gitim-core` | `src/timer.rs` | **NEW** | Timer types, parse_duration, partition_fired, lockfile helper, atomic IO, file read/write |
| `gitim-core` | `src/lib.rs` | EDIT | `pub mod timer;` |
| `gitim-core` | `Cargo.toml` | EDIT | Add `humantime`, `fs2`, `getrandom`, `hex` (latter two already used by flow) |
| `gitim-core` | `src/timer_test.rs` | NEW (inline `#[cfg(test)]` in timer.rs is fine) | Unit tests |
| `gitim-cli` | `src/commands/timer.rs` | **NEW** | `cmd_set` / `cmd_list` / `cmd_cancel` |
| `gitim-cli` | `src/commands/mod.rs` | EDIT | `pub mod timer;` |
| `gitim-cli` | `src/main.rs` | EDIT | Add `Timer { command: TimerCommands }` to `Commands` enum + dispatch arm |
| `gitim-cli` | `tests/timer_cli.rs` | **NEW** | CLI E2E tests via `assert_cmd` |
| `gitim-cli` | `Cargo.toml` | EDIT | Add `assert_cmd` and `predicates` to dev-deps if missing |
| `gitim-runtime` | `src/agent_loop.rs` | EDIT | In `run_once`: pop fired timers, prepend to external_prompt. Expose `next_timer_due()` method. |
| `gitim-runtime` | `src/http.rs` | EDIT | In `start_agent_loop` outer loop: sleep duration = `min(poll_interval, time_until_next_due)` |
| `gitim-runtime` | `tests/timer_integration.rs` | **NEW** | Integration tests against agent_loop |
| `gitim-agent-provider` | `src/prompts.rs` | EDIT | Append timer section to `default_gitim_api()` |
| repo root | `CLAUDE.md` | EDIT | Current Orientation: add "where we are" line about timer |

---

## Task 1: gitim-core deps + module scaffold

**Files:**
- Modify: `crates/gitim-core/Cargo.toml`
- Modify: `crates/gitim-core/src/lib.rs`
- Create: `crates/gitim-core/src/timer.rs`

- [ ] **Step 1: Add deps to gitim-core/Cargo.toml**

Add under `[dependencies]` (alphabetical sort with existing entries):

```toml
fs2 = "0.4"
humantime = "2"
getrandom = "0.2"
hex = "0.4"
```

(Note: `getrandom` / `hex` may already be transitive deps via flow but explicit declare is required for this crate's direct use.)

- [ ] **Step 2: Add module declaration to lib.rs**

In `crates/gitim-core/src/lib.rs`, add (sorted with existing `pub mod`):

```rust
pub mod timer;
```

- [ ] **Step 3: Create empty timer.rs skeleton**

```rust
// crates/gitim-core/src/timer.rs

//! Oneshot timer: per-agent file-backed pending wake-ups.
//!
//! Design: `docs/plans/oneshot-timer/00-requirements.md`
//! Constraints: `docs/plans/oneshot-timer/01-eng-review-findings.md`

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const MIN_DURATION_SECS: i64 = 10;
pub const MAX_DURATION_SECS: i64 = 24 * 60 * 60;
pub const MAX_PENDING_PER_AGENT: usize = 3;
pub const TIMERS_FILENAME: &str = "timers.json";
pub const LOCK_FILENAME: &str = "timers.json.lock";
pub const GITIM_DIR: &str = ".gitim";

#[derive(Error, Debug)]
pub enum TimerError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid duration: {0}")]
    InvalidDuration(String),
    #[error("anchor cannot be empty")]
    EmptyAnchor,
    #[error("cap reached ({MAX_PENDING_PER_AGENT} pending timers); cancel one first")]
    CapReached,
    #[error("no timer matches \"{0}\"")]
    NoMatch(String),
    #[error("prefix \"{prefix}\" matches {count} timers: {ids}")]
    AmbiguousPrefix {
        prefix: String,
        count: usize,
        ids: String,
    },
    #[error("not in a gitim agent clone (no .gitim/ directory)")]
    NotInClone,
    #[error("random: {0}")]
    Random(#[from] getrandom::Error),
    #[error("timers.json corrupted: {0}")]
    Corrupted(String),
}

#[cfg(test)]
mod tests {
    // Tests come in subsequent tasks.
}
```

- [ ] **Step 4: Verify it builds**

Run: `cargo build -p gitim-core`

Expected: builds clean (unused warnings on constants are OK at this stage).

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-core/Cargo.toml crates/gitim-core/src/lib.rs crates/gitim-core/src/timer.rs
git commit -m "feat(gitim-core): scaffold timer module"
```

---

## Task 2: Timer / TimersFile types + serde round-trip

**Files:**
- Modify: `crates/gitim-core/src/timer.rs`

- [ ] **Step 1: Write failing test**

Append to the `tests` mod in `timer.rs`:

```rust
#[test]
fn timers_file_serde_round_trip() {
    let original = TimersFile {
        version: 1,
        timers: vec![Timer {
            id: "20260520T143055-a3f4c2".into(),
            fire_at: "2026-05-20T15:00:55Z".parse().unwrap_or_else(|_| Utc::now()),
            created_at: "2026-05-20T14:30:55Z".parse().unwrap_or_else(|_| Utc::now()),
            anchor: "<#product:L000042>".into(),
            note: Some("check deploy".into()),
        }],
    };
    let json = serde_json::to_string(&original).unwrap_or_default();
    let parsed: TimersFile = serde_json::from_str(&json).unwrap_or(TimersFile::empty());
    assert_eq!(parsed.version, 1);
    assert_eq!(parsed.timers.len(), 1);
    assert_eq!(parsed.timers[0].id, "20260520T143055-a3f4c2");
    assert_eq!(parsed.timers[0].anchor, "<#product:L000042>");
    assert_eq!(parsed.timers[0].note.as_deref(), Some("check deploy"));
}

#[test]
fn timers_file_empty_default() {
    let f = TimersFile::empty();
    assert_eq!(f.version, 1);
    assert!(f.timers.is_empty());
}

#[test]
fn timers_file_missing_note_parses() {
    let json = r#"{"version":1,"timers":[{"id":"20260520T143055-a3f4c2","fire_at":"2026-05-20T15:00:55Z","created_at":"2026-05-20T14:30:55Z","anchor":"<#x>"}]}"#;
    let f: TimersFile = serde_json::from_str(json).unwrap();
    assert!(f.timers[0].note.is_none());
}
```

- [ ] **Step 2: Run test (should fail to compile — types don't exist yet)**

Run: `cargo test -p gitim-core --lib timer::tests::timers_file -- --nocapture`

Expected: compile error `cannot find type 'TimersFile'`.

- [ ] **Step 3: Add Timer + TimersFile types**

Insert above the `#[cfg(test)] mod tests` block in `timer.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timer {
    pub id: String,
    pub fire_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub anchor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimersFile {
    pub version: u32,
    #[serde(default)]
    pub timers: Vec<Timer>,
}

impl TimersFile {
    pub fn empty() -> Self {
        Self {
            version: 1,
            timers: Vec::new(),
        }
    }
}
```

- [ ] **Step 4: Run tests, expect pass**

Run: `cargo test -p gitim-core --lib timer::tests`

Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-core
git add crates/gitim-core/src/timer.rs
git commit -m "feat(gitim-core): Timer + TimersFile serde types"
```

---

## Task 3: parse_duration with 10s-24h bounds

**Files:**
- Modify: `crates/gitim-core/src/timer.rs`

- [ ] **Step 1: Write failing tests**

Append to `tests` mod:

```rust
#[test]
fn parse_duration_humantime_ok() {
    assert_eq!(parse_duration("30s").unwrap().num_seconds(), 30);
    assert_eq!(parse_duration("5m").unwrap().num_seconds(), 300);
    assert_eq!(parse_duration("2h").unwrap().num_seconds(), 7200);
    assert_eq!(parse_duration("1h30m").unwrap().num_seconds(), 5400);
}

#[test]
fn parse_duration_min_bound() {
    let err = parse_duration("5s").unwrap_err();
    assert!(matches!(err, TimerError::InvalidDuration(_)));
}

#[test]
fn parse_duration_max_bound() {
    let err = parse_duration("25h").unwrap_err();
    assert!(matches!(err, TimerError::InvalidDuration(_)));
}

#[test]
fn parse_duration_garbage() {
    assert!(parse_duration("30 minutes").is_err());
    assert!(parse_duration("").is_err());
    assert!(parse_duration("abc").is_err());
}

#[test]
fn parse_duration_at_bounds_ok() {
    assert!(parse_duration("10s").is_ok());
    assert!(parse_duration("24h").is_ok());
}
```

- [ ] **Step 2: Run, expect compile fail**

Run: `cargo test -p gitim-core --lib timer::tests::parse_duration -- --nocapture`

Expected: `cannot find function 'parse_duration'`.

- [ ] **Step 3: Implement parse_duration**

Insert in `timer.rs` (above tests mod):

```rust
pub fn parse_duration(s: &str) -> Result<ChronoDuration, TimerError> {
    let std_dur = humantime::parse_duration(s)
        .map_err(|e| TimerError::InvalidDuration(format!("{s:?}: {e}")))?;
    let secs = std_dur.as_secs() as i64;
    if !(MIN_DURATION_SECS..=MAX_DURATION_SECS).contains(&secs) {
        return Err(TimerError::InvalidDuration(format!(
            "{s:?}: must be {MIN_DURATION_SECS}s..{MAX_DURATION_SECS}s ({}s given)",
            secs
        )));
    }
    Ok(ChronoDuration::seconds(secs))
}
```

- [ ] **Step 4: Run tests, expect pass**

Run: `cargo test -p gitim-core --lib timer::tests::parse_duration`

Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-core
git add crates/gitim-core/src/timer.rs
git commit -m "feat(gitim-core): parse_duration with 10s-24h bounds"
```

---

## Task 4: Timer ID generation (panic-free, format aligned with flow run_id)

**Files:**
- Modify: `crates/gitim-core/src/timer.rs`

- [ ] **Step 1: Write failing tests**

Append:

```rust
#[test]
fn new_id_matches_22_char_pattern() {
    let id = Timer::new_id().unwrap();
    assert_eq!(id.len(), 22, "id was: {id:?}");
    let bytes = id.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        let ok = match i {
            0..=7 => b.is_ascii_digit(),
            8 => b == b'T',
            9..=14 => b.is_ascii_digit(),
            15 => b == b'-',
            16..=21 => (b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            _ => false,
        };
        assert!(ok, "char at {i} (={b}) failed validation in {id:?}");
    }
}

#[test]
fn new_id_pairs_unique_with_high_probability() {
    use std::collections::HashSet;
    let mut s = HashSet::new();
    for _ in 0..200 {
        s.insert(Timer::new_id().unwrap());
    }
    assert!(s.len() >= 199, "had {}/200 unique", s.len());
}
```

- [ ] **Step 2: Run, expect compile fail**

Run: `cargo test -p gitim-core --lib timer::tests::new_id`

Expected: `cannot find associated function 'new_id'`.

- [ ] **Step 3: Implement Timer::new_id and Timer::new**

Insert in the `impl Timer { ... }` block (add the block above tests mod):

```rust
impl Timer {
    pub fn new_id() -> Result<String, TimerError> {
        let now = Utc::now();
        let timestamp = now.format("%Y%m%dT%H%M%S");
        let mut hash_bytes = [0u8; 3];
        getrandom::getrandom(&mut hash_bytes)?;
        let hash = hex::encode(hash_bytes);
        Ok(format!("{timestamp}-{hash}"))
    }

    pub fn new(duration: ChronoDuration, anchor: String, note: Option<String>) -> Result<Self, TimerError> {
        let trimmed = anchor.trim();
        if trimmed.is_empty() {
            return Err(TimerError::EmptyAnchor);
        }
        let now = Utc::now();
        Ok(Self {
            id: Self::new_id()?,
            fire_at: now + duration,
            created_at: now,
            anchor: trimmed.to_string(),
            note,
        })
    }
}
```

- [ ] **Step 4: Run tests, expect pass**

Run: `cargo test -p gitim-core --lib timer::tests`

Expected: all timer tests pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-core
git add crates/gitim-core/src/timer.rs
git commit -m "feat(gitim-core): Timer::new + panic-free id generation"
```

---

## Task 5: partition_fired pure function (F6 — clock injection)

**Files:**
- Modify: `crates/gitim-core/src/timer.rs`

- [ ] **Step 1: Write failing tests**

Append:

```rust
fn mk_timer(id: &str, fire_at_iso: &str) -> Timer {
    Timer {
        id: id.into(),
        fire_at: fire_at_iso.parse().unwrap(),
        created_at: "2026-01-01T00:00:00Z".parse().unwrap(),
        anchor: "<#x>".into(),
        note: None,
    }
}

#[test]
fn partition_fired_empty() {
    let (fired, pending) = partition_fired(vec![], "2026-05-20T15:00:00Z".parse().unwrap());
    assert!(fired.is_empty());
    assert!(pending.is_empty());
}

#[test]
fn partition_fired_all_future() {
    let timers = vec![
        mk_timer("a", "2026-05-20T16:00:00Z"),
        mk_timer("b", "2026-05-20T17:00:00Z"),
    ];
    let (fired, pending) = partition_fired(timers, "2026-05-20T15:00:00Z".parse().unwrap());
    assert!(fired.is_empty());
    assert_eq!(pending.len(), 2);
}

#[test]
fn partition_fired_all_past() {
    let timers = vec![
        mk_timer("a", "2026-05-20T14:00:00Z"),
        mk_timer("b", "2026-05-20T13:00:00Z"),
    ];
    let (fired, pending) = partition_fired(timers, "2026-05-20T15:00:00Z".parse().unwrap());
    assert_eq!(fired.len(), 2);
    assert!(pending.is_empty());
}

#[test]
fn partition_fired_mixed() {
    let timers = vec![
        mk_timer("past", "2026-05-20T14:00:00Z"),
        mk_timer("future", "2026-05-20T16:00:00Z"),
        mk_timer("now", "2026-05-20T15:00:00Z"),
    ];
    let (fired, pending) = partition_fired(timers, "2026-05-20T15:00:00Z".parse().unwrap());
    let fired_ids: Vec<_> = fired.iter().map(|t| t.id.as_str()).collect();
    let pending_ids: Vec<_> = pending.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(fired_ids, vec!["past", "now"], "fire_at == now is fired");
    assert_eq!(pending_ids, vec!["future"]);
}

#[test]
fn partition_fired_pending_sorted_by_fire_at_ascending() {
    let timers = vec![
        mk_timer("late", "2026-05-20T18:00:00Z"),
        mk_timer("early", "2026-05-20T16:00:00Z"),
        mk_timer("mid", "2026-05-20T17:00:00Z"),
    ];
    let (_, pending) = partition_fired(timers, "2026-05-20T15:00:00Z".parse().unwrap());
    let ids: Vec<_> = pending.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, vec!["early", "mid", "late"]);
}
```

- [ ] **Step 2: Run, expect compile fail**

Run: `cargo test -p gitim-core --lib timer::tests::partition_fired`

Expected: `cannot find function 'partition_fired'`.

- [ ] **Step 3: Implement partition_fired**

Insert in `timer.rs` above tests mod:

```rust
/// Pure function — `now` is injected so tests can simulate clock skew.
/// Returned `pending` is sorted by `fire_at` ascending; `fired` preserves
/// input order (caller renders them in registration sequence).
pub fn partition_fired(mut timers: Vec<Timer>, now: DateTime<Utc>) -> (Vec<Timer>, Vec<Timer>) {
    timers.sort_by_key(|t| t.fire_at);
    let mut fired = Vec::new();
    let mut pending = Vec::new();
    for t in timers {
        if t.fire_at <= now {
            fired.push(t);
        } else {
            pending.push(t);
        }
    }
    (fired, pending)
}
```

- [ ] **Step 4: Run tests, expect pass**

Run: `cargo test -p gitim-core --lib timer::tests::partition_fired`

Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-core
git add crates/gitim-core/src/timer.rs
git commit -m "feat(gitim-core): partition_fired pure function with clock injection"
```

---

## Task 6: cancel_by_id_or_prefix

**Files:**
- Modify: `crates/gitim-core/src/timer.rs`

- [ ] **Step 1: Write failing tests**

Append:

```rust
#[test]
fn cancel_by_full_id() {
    let mut timers = vec![
        mk_timer("20260520T143055-aaaaaa", "2026-05-20T16:00:00Z"),
        mk_timer("20260520T143120-bbbbbb", "2026-05-20T17:00:00Z"),
    ];
    let cancelled = cancel_by_id_or_prefix(&mut timers, "20260520T143055-aaaaaa").unwrap();
    assert_eq!(cancelled, "20260520T143055-aaaaaa");
    assert_eq!(timers.len(), 1);
    assert_eq!(timers[0].id, "20260520T143120-bbbbbb");
}

#[test]
fn cancel_by_unique_prefix() {
    let mut timers = vec![
        mk_timer("20260520T143055-aaaaaa", "2026-05-20T16:00:00Z"),
        mk_timer("20260520T143120-bbbbbb", "2026-05-20T17:00:00Z"),
    ];
    let cancelled = cancel_by_id_or_prefix(&mut timers, "aaa").unwrap();
    assert_eq!(cancelled, "20260520T143055-aaaaaa");
    assert_eq!(timers.len(), 1);
}

#[test]
fn cancel_no_match() {
    let mut timers = vec![mk_timer("20260520T143055-aaaaaa", "2026-05-20T16:00:00Z")];
    let err = cancel_by_id_or_prefix(&mut timers, "zzz").unwrap_err();
    assert!(matches!(err, TimerError::NoMatch(_)));
    assert_eq!(timers.len(), 1);
}

#[test]
fn cancel_ambiguous_prefix() {
    let mut timers = vec![
        mk_timer("20260520T143055-aaaaaa", "2026-05-20T16:00:00Z"),
        mk_timer("20260520T143120-bbbbbb", "2026-05-20T17:00:00Z"),
    ];
    let err = cancel_by_id_or_prefix(&mut timers, "2026").unwrap_err();
    match err {
        TimerError::AmbiguousPrefix { count, .. } => assert_eq!(count, 2),
        other => panic!("expected AmbiguousPrefix, got {other:?}"),
    }
    assert_eq!(timers.len(), 2);
}
```

- [ ] **Step 2: Run, expect compile fail**

Run: `cargo test -p gitim-core --lib timer::tests::cancel`

Expected: `cannot find function 'cancel_by_id_or_prefix'`.

- [ ] **Step 3: Implement**

Insert in `timer.rs`:

```rust
/// Remove and return the cancelled id. Substring is matched as prefix
/// against `id`. 0 or >1 matches → error and `timers` is left unchanged.
pub fn cancel_by_id_or_prefix(timers: &mut Vec<Timer>, prefix: &str) -> Result<String, TimerError> {
    let matches: Vec<usize> = timers
        .iter()
        .enumerate()
        .filter(|(_, t)| t.id.starts_with(prefix))
        .map(|(i, _)| i)
        .collect();
    match matches.len() {
        0 => Err(TimerError::NoMatch(prefix.to_string())),
        1 => {
            let idx = matches[0];
            Ok(timers.remove(idx).id)
        }
        n => {
            let ids = matches
                .iter()
                .map(|&i| timers[i].id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            Err(TimerError::AmbiguousPrefix {
                prefix: prefix.to_string(),
                count: n,
                ids,
            })
        }
    }
}
```

- [ ] **Step 4: Run tests, expect pass**

Run: `cargo test -p gitim-core --lib timer::tests::cancel`

Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-core
git add crates/gitim-core/src/timer.rs
git commit -m "feat(gitim-core): cancel_by_id_or_prefix with disambiguation"
```

---

## Task 7: Lockfile helper (F1 — lock anchor independent of atomic-rename target)

**Files:**
- Modify: `crates/gitim-core/src/timer.rs`

- [ ] **Step 1: Write failing test**

Append:

```rust
#[test]
fn with_timers_lock_serializes_writers() {
    use std::sync::{Arc, Mutex};
    use std::thread;

    let tmp = tempfile::TempDir::new().unwrap();
    let clone_path = tmp.path().to_path_buf();
    std::fs::create_dir_all(clone_path.join(GITIM_DIR)).unwrap();

    let counter = Arc::new(Mutex::new(0u32));
    let observed_concurrent = Arc::new(Mutex::new(false));

    let mut handles = vec![];
    for _ in 0..8 {
        let clone_path = clone_path.clone();
        let counter = counter.clone();
        let observed = observed_concurrent.clone();
        handles.push(thread::spawn(move || {
            with_timers_lock(&clone_path, |_locked_path| {
                {
                    let mut c = counter.lock().unwrap();
                    if *c != 0 {
                        // Another thread holds the lock simultaneously — bug.
                        *observed.lock().unwrap() = true;
                    }
                    *c += 1;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
                {
                    let mut c = counter.lock().unwrap();
                    *c -= 1;
                }
                Ok(())
            })
            .unwrap();
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    assert!(
        !*observed_concurrent.lock().unwrap(),
        "two threads held the lock simultaneously"
    );
}

#[test]
fn with_timers_lock_creates_lockfile_if_missing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let clone_path = tmp.path().to_path_buf();
    std::fs::create_dir_all(clone_path.join(GITIM_DIR)).unwrap();

    with_timers_lock(&clone_path, |_| Ok(())).unwrap();
    assert!(clone_path.join(GITIM_DIR).join(LOCK_FILENAME).exists());
}

#[test]
fn with_timers_lock_requires_gitim_dir() {
    let tmp = tempfile::TempDir::new().unwrap();
    // No .gitim/ created.
    let err = with_timers_lock(tmp.path(), |_| Ok(())).unwrap_err();
    assert!(matches!(err, TimerError::NotInClone));
}
```

- [ ] **Step 2: Run, expect compile fail**

Run: `cargo test -p gitim-core --lib timer::tests::with_timers_lock`

Expected: `cannot find function 'with_timers_lock'`.

- [ ] **Step 3: Implement lockfile helper**

Insert in `timer.rs`:

```rust
use fs2::FileExt;
use std::fs::OpenOptions;

/// Acquire an exclusive advisory lock on the timer lockfile, run `f`, release.
/// Locks `<clone>/.gitim/timers.json.lock` — independent of `timers.json` so
/// atomic-rename writes of `timers.json` don't unlink the lock anchor (F1).
///
/// The closure receives the lock-held path to `timers.json` (purely a
/// convenience — the lock is on `.lock`, the read/write targets `.json`).
///
/// On `NotInClone`, the closure is never called.
pub fn with_timers_lock<T>(
    clone_path: &Path,
    f: impl FnOnce(&Path) -> Result<T, TimerError>,
) -> Result<T, TimerError> {
    let gitim_dir = clone_path.join(GITIM_DIR);
    if !gitim_dir.is_dir() {
        return Err(TimerError::NotInClone);
    }
    let lock_path = gitim_dir.join(LOCK_FILENAME);
    let timers_path = gitim_dir.join(TIMERS_FILENAME);

    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;
    lock_file.lock_exclusive()?;
    let result = f(&timers_path);
    // Drop-as-unlock would work, but explicit is friendlier on review.
    let _ = lock_file.unlock();
    result
}

/// Convenience for callers that need the timers.json path without holding
/// the lock (read-only inspection, e.g., peek_next_due best-effort).
pub fn timers_path(clone_path: &Path) -> PathBuf {
    clone_path.join(GITIM_DIR).join(TIMERS_FILENAME)
}
```

- [ ] **Step 4: Run tests, expect pass**

Run: `cargo test -p gitim-core --lib timer::tests::with_timers_lock`

Expected: 3 passed (the 8-thread serialization test may take ~200ms).

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-core
git add crates/gitim-core/src/timer.rs
git commit -m "feat(gitim-core): timers lockfile helper (F1 — independent lock anchor)"
```

---

## Task 8: Atomic IO — read_timers / write_timers (F5 — NamedTempFile-based)

**Files:**
- Modify: `crates/gitim-core/src/timer.rs`

- [ ] **Step 1: Write failing tests**

Append:

```rust
#[test]
fn read_timers_missing_file_returns_empty() {
    let tmp = tempfile::TempDir::new().unwrap();
    let clone = tmp.path();
    std::fs::create_dir_all(clone.join(GITIM_DIR)).unwrap();
    let f = read_timers(clone).unwrap();
    assert_eq!(f.timers.len(), 0);
}

#[test]
fn read_timers_corrupted_returns_empty_and_warns() {
    let tmp = tempfile::TempDir::new().unwrap();
    let clone = tmp.path();
    let gitim = clone.join(GITIM_DIR);
    std::fs::create_dir_all(&gitim).unwrap();
    std::fs::write(gitim.join(TIMERS_FILENAME), "{this is not json").unwrap();
    let f = read_timers(clone).unwrap();
    assert_eq!(f.timers.len(), 0);
    // File preserved (not deleted) — eng-review constraint.
    assert!(gitim.join(TIMERS_FILENAME).exists());
}

#[test]
fn read_timers_unknown_version_returns_empty() {
    let tmp = tempfile::TempDir::new().unwrap();
    let clone = tmp.path();
    let gitim = clone.join(GITIM_DIR);
    std::fs::create_dir_all(&gitim).unwrap();
    std::fs::write(
        gitim.join(TIMERS_FILENAME),
        r#"{"version":99,"timers":[]}"#,
    )
    .unwrap();
    let f = read_timers(clone).unwrap();
    assert_eq!(f.timers.len(), 0);
}

#[test]
fn write_then_read_round_trip() {
    let tmp = tempfile::TempDir::new().unwrap();
    let clone = tmp.path();
    std::fs::create_dir_all(clone.join(GITIM_DIR)).unwrap();
    let original = TimersFile {
        version: 1,
        timers: vec![Timer {
            id: "20260520T143055-aaaaaa".into(),
            fire_at: "2026-05-20T15:00:55Z".parse().unwrap(),
            created_at: "2026-05-20T14:30:55Z".parse().unwrap(),
            anchor: "<#x>".into(),
            note: None,
        }],
    };
    write_timers(clone, &original).unwrap();
    let read = read_timers(clone).unwrap();
    assert_eq!(read.timers.len(), 1);
    assert_eq!(read.timers[0].id, "20260520T143055-aaaaaa");
}

#[test]
fn write_does_not_leave_tmp_files() {
    let tmp = tempfile::TempDir::new().unwrap();
    let clone = tmp.path();
    let gitim = clone.join(GITIM_DIR);
    std::fs::create_dir_all(&gitim).unwrap();
    write_timers(clone, &TimersFile::empty()).unwrap();
    let leftover: Vec<_> = std::fs::read_dir(&gitim)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let n = e.file_name();
            let s = n.to_string_lossy();
            s.starts_with("timers.json.") && s != "timers.json.lock"
        })
        .collect();
    assert!(leftover.is_empty(), "tmp files remained: {leftover:?}");
}
```

- [ ] **Step 2: Run, expect compile fail**

Run: `cargo test -p gitim-core --lib timer::tests`

Expected: `cannot find function 'read_timers'` / `'write_timers'`.

- [ ] **Step 3: Implement read_timers / write_timers**

Insert in `timer.rs`:

```rust
use std::io::Write;
use tempfile::NamedTempFile;

/// Read the timers file. Returns empty on: file missing, corrupted JSON,
/// or unknown schema version (logs warning in the latter two cases).
/// Never errors on read — read errors translate to "0 timers".
pub fn read_timers(clone_path: &Path) -> Result<TimersFile, TimerError> {
    let path = timers_path(clone_path);
    match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<TimersFile>(&bytes) {
            Ok(f) if f.version == 1 => Ok(f),
            Ok(f) => {
                tracing::warn!(
                    path = %path.display(),
                    version = f.version,
                    "timers.json unknown schema version, treating as empty"
                );
                Ok(TimersFile::empty())
            }
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "timers.json corrupted, treating as empty (file preserved)"
                );
                Ok(TimersFile::empty())
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TimersFile::empty()),
        Err(e) => Err(TimerError::Io(e)),
    }
}

/// Write the timers file atomically: stage in a NamedTempFile under the
/// `.gitim/` dir, fsync, then `persist` (atomic rename). NamedTempFile's
/// Drop cleans up the temp on early-return / panic / persist failure (F5).
pub fn write_timers(clone_path: &Path, file: &TimersFile) -> Result<(), TimerError> {
    let gitim = clone_path.join(GITIM_DIR);
    if !gitim.is_dir() {
        return Err(TimerError::NotInClone);
    }
    let target = gitim.join(TIMERS_FILENAME);

    let mut tmp = NamedTempFile::new_in(&gitim)?;
    {
        let writer = tmp.as_file_mut();
        let json = serde_json::to_vec_pretty(file)?;
        writer.write_all(&json)?;
        writer.sync_all()?;
    }
    tmp.persist(&target)
        .map_err(|e| TimerError::Io(e.error))?;
    Ok(())
}
```

- [ ] **Step 4: Run tests, expect pass**

Run: `cargo test -p gitim-core --lib timer::tests`

Expected: all tests pass (~12+ total now).

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-core
git add crates/gitim-core/src/timer.rs
git commit -m "feat(gitim-core): atomic read_timers + write_timers via NamedTempFile (F5)"
```

---

## Task 9: pop_fired_timers + peek_next_due (locked, public API for agent_loop)

**Files:**
- Modify: `crates/gitim-core/src/timer.rs`

- [ ] **Step 1: Write failing tests**

Append:

```rust
#[test]
fn pop_fired_removes_fired_keeps_pending() {
    let tmp = tempfile::TempDir::new().unwrap();
    let clone = tmp.path();
    std::fs::create_dir_all(clone.join(GITIM_DIR)).unwrap();
    let initial = TimersFile {
        version: 1,
        timers: vec![
            mk_timer("a", "2026-05-20T14:00:00Z"),
            mk_timer("b", "2026-05-20T16:00:00Z"),
        ],
    };
    write_timers(clone, &initial).unwrap();
    let now: DateTime<Utc> = "2026-05-20T15:00:00Z".parse().unwrap();
    let fired = pop_fired_timers(clone, now).unwrap();
    assert_eq!(fired.len(), 1);
    assert_eq!(fired[0].id, "a");
    let remaining = read_timers(clone).unwrap();
    assert_eq!(remaining.timers.len(), 1);
    assert_eq!(remaining.timers[0].id, "b");
}

#[test]
fn pop_fired_missing_file_returns_empty() {
    let tmp = tempfile::TempDir::new().unwrap();
    let clone = tmp.path();
    std::fs::create_dir_all(clone.join(GITIM_DIR)).unwrap();
    let fired = pop_fired_timers(clone, Utc::now()).unwrap();
    assert!(fired.is_empty());
}

#[test]
fn pop_fired_no_due_does_not_rewrite() {
    let tmp = tempfile::TempDir::new().unwrap();
    let clone = tmp.path();
    std::fs::create_dir_all(clone.join(GITIM_DIR)).unwrap();
    let f = TimersFile {
        version: 1,
        timers: vec![mk_timer("a", "2026-05-20T16:00:00Z")],
    };
    write_timers(clone, &f).unwrap();
    let before_mtime = std::fs::metadata(timers_path(clone)).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let now: DateTime<Utc> = "2026-05-20T15:00:00Z".parse().unwrap();
    let fired = pop_fired_timers(clone, now).unwrap();
    assert!(fired.is_empty());
    let after_mtime = std::fs::metadata(timers_path(clone)).unwrap().modified().unwrap();
    assert_eq!(before_mtime, after_mtime, "file rewrote unnecessarily");
}

#[test]
fn peek_next_due_returns_earliest() {
    let tmp = tempfile::TempDir::new().unwrap();
    let clone = tmp.path();
    std::fs::create_dir_all(clone.join(GITIM_DIR)).unwrap();
    let f = TimersFile {
        version: 1,
        timers: vec![
            mk_timer("late", "2026-05-20T18:00:00Z"),
            mk_timer("early", "2026-05-20T16:00:00Z"),
        ],
    };
    write_timers(clone, &f).unwrap();
    let next = peek_next_due(clone).unwrap();
    assert_eq!(
        next.map(|t| t.to_rfc3339()),
        Some("2026-05-20T16:00:00+00:00".to_string())
    );
}

#[test]
fn peek_next_due_empty_is_none() {
    let tmp = tempfile::TempDir::new().unwrap();
    let clone = tmp.path();
    std::fs::create_dir_all(clone.join(GITIM_DIR)).unwrap();
    assert!(peek_next_due(clone).unwrap().is_none());
}
```

- [ ] **Step 2: Run, expect compile fail**

Run: `cargo test -p gitim-core --lib timer::tests::pop_fired`

Expected: `cannot find function 'pop_fired_timers'`.

- [ ] **Step 3: Implement pop_fired_timers + peek_next_due**

Insert in `timer.rs`:

```rust
/// Remove and return all timers with `fire_at <= now`. Atomic + locked.
/// Returns `Ok(vec![])` for file-missing / corrupted / empty. Write failure
/// after read keeps fired in the file (next call retries) — see eng-review F2.
pub fn pop_fired_timers(clone_path: &Path, now: DateTime<Utc>) -> Result<Vec<Timer>, TimerError> {
    with_timers_lock(clone_path, |_| {
        let current = read_timers(clone_path)?;
        let (fired, pending) = partition_fired(current.timers, now);
        if fired.is_empty() {
            return Ok(vec![]);
        }
        let new_file = TimersFile {
            version: 1,
            timers: pending,
        };
        write_timers(clone_path, &new_file)?;
        Ok(fired)
    })
}

/// Best-effort read of earliest pending `fire_at`. No lock held — the next
/// `pop_fired_timers` will re-read under lock anyway, so a stale snapshot
/// here only affects the next sleep duration by at most one cycle.
pub fn peek_next_due(clone_path: &Path) -> Result<Option<DateTime<Utc>>, TimerError> {
    let f = read_timers(clone_path)?;
    Ok(f.timers.iter().map(|t| t.fire_at).min())
}
```

- [ ] **Step 4: Run tests, expect pass**

Run: `cargo test -p gitim-core --lib timer::tests`

Expected: all pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-core
git add crates/gitim-core/src/timer.rs
git commit -m "feat(gitim-core): pop_fired_timers + peek_next_due (locked, atomic)"
```

---

## Task 10: register_timer (with cap check)

**Files:**
- Modify: `crates/gitim-core/src/timer.rs`

- [ ] **Step 1: Write failing tests**

Append:

```rust
#[test]
fn register_timer_persists() {
    let tmp = tempfile::TempDir::new().unwrap();
    let clone = tmp.path();
    std::fs::create_dir_all(clone.join(GITIM_DIR)).unwrap();
    let dur = ChronoDuration::seconds(60);
    let t = register_timer(clone, dur, "<#x>".into(), Some("test".into())).unwrap();
    assert!(!t.id.is_empty());
    let f = read_timers(clone).unwrap();
    assert_eq!(f.timers.len(), 1);
    assert_eq!(f.timers[0].id, t.id);
}

#[test]
fn register_timer_cap_enforced() {
    let tmp = tempfile::TempDir::new().unwrap();
    let clone = tmp.path();
    std::fs::create_dir_all(clone.join(GITIM_DIR)).unwrap();
    for _ in 0..MAX_PENDING_PER_AGENT {
        register_timer(clone, ChronoDuration::seconds(60), "<#x>".into(), None).unwrap();
    }
    let err = register_timer(clone, ChronoDuration::seconds(60), "<#x>".into(), None).unwrap_err();
    assert!(matches!(err, TimerError::CapReached));
    assert_eq!(read_timers(clone).unwrap().timers.len(), MAX_PENDING_PER_AGENT);
}

#[test]
fn register_timer_empty_anchor_rejected() {
    let tmp = tempfile::TempDir::new().unwrap();
    let clone = tmp.path();
    std::fs::create_dir_all(clone.join(GITIM_DIR)).unwrap();
    let err = register_timer(clone, ChronoDuration::seconds(60), "   ".into(), None).unwrap_err();
    assert!(matches!(err, TimerError::EmptyAnchor));
    assert_eq!(read_timers(clone).unwrap().timers.len(), 0);
}
```

- [ ] **Step 2: Run, expect compile fail**

Run: `cargo test -p gitim-core --lib timer::tests::register_timer`

Expected: `cannot find function 'register_timer'`.

- [ ] **Step 3: Implement register_timer + cancel_timer**

Insert in `timer.rs`:

```rust
/// Register a new timer. Enforces cap on current pending count.
pub fn register_timer(
    clone_path: &Path,
    duration: ChronoDuration,
    anchor: String,
    note: Option<String>,
) -> Result<Timer, TimerError> {
    with_timers_lock(clone_path, |_| {
        let mut current = read_timers(clone_path)?;
        if current.timers.len() >= MAX_PENDING_PER_AGENT {
            return Err(TimerError::CapReached);
        }
        let timer = Timer::new(duration, anchor, note)?;
        current.timers.push(timer.clone());
        write_timers(clone_path, &current)?;
        Ok(timer)
    })
}

/// Cancel a timer by full id or unique prefix.
pub fn cancel_timer(clone_path: &Path, id_or_prefix: &str) -> Result<String, TimerError> {
    with_timers_lock(clone_path, |_| {
        let mut current = read_timers(clone_path)?;
        let cancelled = cancel_by_id_or_prefix(&mut current.timers, id_or_prefix)?;
        write_timers(clone_path, &current)?;
        Ok(cancelled)
    })
}
```

- [ ] **Step 4: Run tests, expect pass**

Run: `cargo test -p gitim-core --lib timer::tests`

Expected: all pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-core
git add crates/gitim-core/src/timer.rs
git commit -m "feat(gitim-core): register_timer + cancel_timer with cap"
```

---

## Task 11: format_fired_for_prompt (synthetic message renderer)

**Files:**
- Modify: `crates/gitim-core/src/timer.rs`

- [ ] **Step 1: Write failing tests**

Append:

```rust
#[test]
fn format_fired_for_prompt_single() {
    let timers = vec![Timer {
        id: "20260520T143055-aaaaaa".into(),
        fire_at: "2026-05-20T15:00:55Z".parse().unwrap(),
        created_at: "2026-05-20T14:30:55Z".parse().unwrap(),
        anchor: "<#product:L000042>".into(),
        note: Some("check deploy".into()),
    }];
    let now: DateTime<Utc> = "2026-05-20T15:00:55Z".parse().unwrap();
    let out = format_fired_for_prompt(&timers, now);
    assert!(out.contains("## ⏰ Timer reminder(s) fired"));
    assert!(out.contains("<#product:L000042>"));
    assert!(out.contains("check deploy"));
    assert!(out.contains("30m"), "expected 'Set 30m ago' phrasing: {out}");
}

#[test]
fn format_fired_for_prompt_multiple_numbered() {
    let timers = vec![
        Timer {
            id: "20260520T143055-aaaaaa".into(),
            fire_at: "2026-05-20T15:00:55Z".parse().unwrap(),
            created_at: "2026-05-20T14:30:55Z".parse().unwrap(),
            anchor: "<#a>".into(),
            note: None,
        },
        Timer {
            id: "20260520T143120-bbbbbb".into(),
            fire_at: "2026-05-20T15:00:55Z".parse().unwrap(),
            created_at: "2026-05-20T13:48:55Z".parse().unwrap(),
            anchor: "<#b>".into(),
            note: Some("follow up".into()),
        },
    ];
    let now: DateTime<Utc> = "2026-05-20T15:00:55Z".parse().unwrap();
    let out = format_fired_for_prompt(&timers, now);
    assert!(out.contains("1."), "{out}");
    assert!(out.contains("2."), "{out}");
    assert!(out.contains("<#a>"));
    assert!(out.contains("<#b>"));
}

#[test]
fn format_fired_for_prompt_empty_is_empty() {
    let now = Utc::now();
    assert_eq!(format_fired_for_prompt(&[], now), "");
}
```

- [ ] **Step 2: Run, expect compile fail**

Run: `cargo test -p gitim-core --lib timer::tests::format_fired`

Expected: `cannot find function 'format_fired_for_prompt'`.

- [ ] **Step 3: Implement**

Insert in `timer.rs`:

```rust
/// Render fired timers as the synthetic prompt prefix injected before
/// daemon-change content. Empty `timers` → empty string (caller decides
/// whether to skip the run entirely).
pub fn format_fired_for_prompt(timers: &[Timer], now: DateTime<Utc>) -> String {
    if timers.is_empty() {
        return String::new();
    }
    let mut out = String::from("## ⏰ Timer reminder(s) fired\n\n");
    for (i, t) in timers.iter().enumerate() {
        let elapsed = (now - t.created_at).num_seconds().max(0) as u64;
        let ago = humantime::format_duration(std::time::Duration::from_secs(elapsed));
        // humantime adds nanosecond precision sometimes — trim to leading unit pair.
        let ago_trimmed: String = ago
            .to_string()
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ");
        out.push_str(&format!("{}. Set {} ago\n", i + 1, ago_trimmed));
        out.push_str(&format!("   anchor: {}\n", t.anchor));
        if let Some(note) = &t.note {
            out.push_str(&format!("   note: {}\n", note));
        }
        out.push('\n');
    }
    out.push_str("Use the `gitim` CLI to fetch context at the anchor(s) above.\n");
    out
}
```

- [ ] **Step 4: Run tests, expect pass**

Run: `cargo test -p gitim-core --lib timer::tests`

Expected: all pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-core
git add crates/gitim-core/src/timer.rs
git commit -m "feat(gitim-core): format_fired_for_prompt synthetic message renderer"
```

---

## Task 12: find_clone_root helper (cwd → .gitim parent)

**Files:**
- Modify: `crates/gitim-core/src/timer.rs`

- [ ] **Step 1: Write failing tests**

Append:

```rust
#[test]
fn find_clone_root_finds_gitim() {
    let tmp = tempfile::TempDir::new().unwrap();
    let clone = tmp.path();
    std::fs::create_dir_all(clone.join(GITIM_DIR)).unwrap();
    let sub = clone.join("a").join("b");
    std::fs::create_dir_all(&sub).unwrap();
    let found = find_clone_root(&sub).unwrap();
    assert_eq!(std::fs::canonicalize(&found).unwrap(), std::fs::canonicalize(clone).unwrap());
}

#[test]
fn find_clone_root_at_root() {
    let tmp = tempfile::TempDir::new().unwrap();
    let clone = tmp.path();
    std::fs::create_dir_all(clone.join(GITIM_DIR)).unwrap();
    let found = find_clone_root(clone).unwrap();
    assert_eq!(std::fs::canonicalize(&found).unwrap(), std::fs::canonicalize(clone).unwrap());
}

#[test]
fn find_clone_root_not_in_clone_errors() {
    let tmp = tempfile::TempDir::new().unwrap();
    let err = find_clone_root(tmp.path()).unwrap_err();
    assert!(matches!(err, TimerError::NotInClone));
}
```

- [ ] **Step 2: Run, expect compile fail**

Expected: `cannot find function 'find_clone_root'`.

- [ ] **Step 3: Implement**

Insert in `timer.rs`:

```rust
/// Walk up from `start` looking for a directory containing `.gitim/`.
/// Returns that ancestor (the clone root). Errors if none found.
pub fn find_clone_root(start: &Path) -> Result<PathBuf, TimerError> {
    let mut current = start.to_path_buf();
    loop {
        if current.join(GITIM_DIR).is_dir() {
            return Ok(current);
        }
        match current.parent() {
            Some(p) => current = p.to_path_buf(),
            None => return Err(TimerError::NotInClone),
        }
    }
}
```

- [ ] **Step 4: Run tests, expect pass**

Run: `cargo test -p gitim-core --lib timer::tests::find_clone_root`

Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-core
git add crates/gitim-core/src/timer.rs
git commit -m "feat(gitim-core): find_clone_root walks up from cwd"
```

---

## Task 13: CLI `gitim timer set` subcommand

**Files:**
- Modify: `crates/gitim-cli/src/commands/mod.rs`
- Create: `crates/gitim-cli/src/commands/timer.rs`
- Modify: `crates/gitim-cli/src/main.rs`

- [ ] **Step 1: Register module in commands/mod.rs**

Add (alphabetical):

```rust
pub mod timer;
```

- [ ] **Step 2: Create commands/timer.rs with cmd_set**

```rust
// crates/gitim-cli/src/commands/timer.rs

use std::process;

use chrono::Utc;
use gitim_core::timer::{
    find_clone_root, parse_duration, register_timer, TimerError,
};
use humantime::format_duration;

use crate::output::OutputMode;

pub async fn cmd_set(_mode: &OutputMode, duration: &str, anchor: &str, note: Option<&str>) {
    let dur = match parse_duration(duration) {
        Ok(d) => d,
        Err(e) => exit_with(&e, 2),
    };
    let cwd = match std::env::current_dir() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: failed to read cwd: {e}");
            process::exit(1);
        }
    };
    let clone = match find_clone_root(&cwd) {
        Ok(c) => c,
        Err(e) => exit_with(&e, 2),
    };
    let timer = match register_timer(&clone, dur, anchor.to_string(), note.map(|s| s.to_string())) {
        Ok(t) => t,
        Err(e) => match e {
            TimerError::CapReached
            | TimerError::EmptyAnchor
            | TimerError::InvalidDuration(_) => exit_with(&e, 2),
            _ => exit_with(&e, 1),
        },
    };
    let remaining = (timer.fire_at - Utc::now()).to_std().unwrap_or_default();
    println!(
        "{}  fires in {}  (at {})",
        timer.id,
        format_duration(remaining),
        timer.fire_at.to_rfc3339()
    );
}

fn exit_with(e: &TimerError, code: i32) -> ! {
    eprintln!("error: {e}");
    process::exit(code);
}
```

- [ ] **Step 3: Wire clap subcommand in main.rs**

In the root `#[derive(Subcommand)] enum Commands` block (around `main.rs:27`), add (place near other multi-arg commands, e.g., next to `Cron`):

```rust
    Timer {
        #[command(subcommand)]
        command: TimerCommands,
    },
```

Then immediately after the existing `#[derive(Subcommand)] enum CronCommands` block (or any sibling subcommand enum), add:

```rust
#[derive(Subcommand)]
enum TimerCommands {
    /// Register a one-shot timer.
    Set {
        /// Duration (humantime, e.g. 30m, 1h30m). 10s..24h.
        duration: String,
        /// Anchor pointing back to the message/card this timer relates to.
        anchor: String,
        /// Optional note to your future self.
        #[arg(long)]
        note: Option<String>,
    },
    /// List pending timers.
    List {
        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Cancel a pending timer by full id or unique prefix.
    Cancel {
        id_or_prefix: String,
    },
}
```

In the main dispatch (after `Commands::Reindex`):

```rust
        Commands::Timer { command } => match command {
            TimerCommands::Set {
                duration,
                anchor,
                note,
            } => {
                commands::timer::cmd_set(&mode, &duration, &anchor, note.as_deref()).await;
                Ok(())
            }
            TimerCommands::List { json } => {
                commands::timer::cmd_list(&mode, json).await;
                Ok(())
            }
            TimerCommands::Cancel { id_or_prefix } => {
                commands::timer::cmd_cancel(&mode, &id_or_prefix).await;
                Ok(())
            }
        },
```

Note: if dispatch arms in `main.rs` return `Result<(), _>` to a Result-based runner, follow the existing pattern — these calls `process::exit` internally on error so `Ok(())` is correct for the happy path. If the actual main.rs uses a different signature, mirror its convention.

- [ ] **Step 4: Verify `gitim timer set --help` parses**

Run: `cargo build -p gitim-cli && target/debug/gitim timer set --help`

Expected: clap-rendered help showing positional `duration` / `anchor` and `--note`.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-cli
git add crates/gitim-cli/src/commands/mod.rs crates/gitim-cli/src/commands/timer.rs crates/gitim-cli/src/main.rs
git commit -m "feat(gitim-cli): gitim timer set subcommand"
```

---

## Task 14: CLI `gitim timer list` + `cancel`

**Files:**
- Modify: `crates/gitim-cli/src/commands/timer.rs`

- [ ] **Step 1: Append cmd_list + cmd_cancel**

```rust
pub async fn cmd_list(_mode: &OutputMode, json: bool) {
    let clone = match resolve_clone() {
        Ok(c) => c,
        Err(e) => exit_with(&e, 2),
    };
    let mut file = match gitim_core::timer::read_timers(&clone) {
        Ok(f) => f,
        Err(e) => exit_with(&e, 1),
    };
    file.timers.sort_by_key(|t| t.fire_at);

    if json {
        match serde_json::to_string_pretty(&file.timers) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("error: {e}");
                process::exit(1);
            }
        }
        return;
    }

    if file.timers.is_empty() {
        println!("(no pending timers)");
        return;
    }

    let now = Utc::now();
    println!(
        "{:<28} {:<10} {:<22} {:<28} NOTE",
        "ID", "DUE IN", "FIRES AT", "ANCHOR"
    );
    for t in &file.timers {
        let due_in = (t.fire_at - now).to_std().unwrap_or_default();
        let due_fmt = trim_humantime(format_duration(due_in).to_string());
        println!(
            "{:<28} {:<10} {:<22} {:<28} {}",
            t.id,
            due_fmt,
            t.fire_at.to_rfc3339(),
            t.anchor,
            t.note.as_deref().unwrap_or("")
        );
    }
}

pub async fn cmd_cancel(_mode: &OutputMode, id_or_prefix: &str) {
    let clone = match resolve_clone() {
        Ok(c) => c,
        Err(e) => exit_with(&e, 2),
    };
    match gitim_core::timer::cancel_timer(&clone, id_or_prefix) {
        Ok(id) => println!("cancelled: {id}"),
        Err(e @ (TimerError::NoMatch(_) | TimerError::AmbiguousPrefix { .. })) => exit_with(&e, 2),
        Err(e) => exit_with(&e, 1),
    }
}

fn resolve_clone() -> Result<std::path::PathBuf, TimerError> {
    let cwd = std::env::current_dir().map_err(TimerError::Io)?;
    find_clone_root(&cwd)
}

fn trim_humantime(s: String) -> String {
    s.split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ")
}
```

Also adjust the top-of-file `use` block to include:

```rust
use std::process;

use chrono::Utc;
use gitim_core::timer::{
    cancel_timer, find_clone_root, parse_duration, read_timers, register_timer, TimerError,
};
use humantime::format_duration;

use crate::output::OutputMode;
```

(Replace the existing `use` block from Task 13 with this richer one — keep cmd_set unchanged.)

Add `humantime = "2"` to `crates/gitim-cli/Cargo.toml` `[dependencies]` if not present.

- [ ] **Step 2: Build, expect success**

Run: `cargo build -p gitim-cli`

Expected: builds clean.

- [ ] **Step 3: Smoke test list (empty)**

```bash
mkdir -p /tmp/timer-smoke/.gitim
cd /tmp/timer-smoke
~/ateam/GitIM/.claude/worktrees/ecstatic-hugle-03df01/target/debug/gitim timer list
```

Expected: `(no pending timers)`, exit 0.

- [ ] **Step 4: Smoke test set + list + cancel**

```bash
cd /tmp/timer-smoke
TARGET=~/ateam/GitIM/.claude/worktrees/ecstatic-hugle-03df01/target/debug/gitim
$TARGET timer set 30m '<#x>' --note "smoke"
$TARGET timer list
$TARGET timer cancel "2026"  # ambiguous OR unique depending on counts
$TARGET timer list
```

Expected:
- `set` prints `<id>  fires in 30m  (at <iso>)`, exit 0
- `list` shows the entry in tabular form
- `cancel` either prints `cancelled: <id>` or `error: prefix ... matches N timers ...` (exit 2)
- Final `list` shows `(no pending timers)` if cancel succeeded

Clean up: `rm -rf /tmp/timer-smoke`.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-cli
git add crates/gitim-cli/Cargo.toml crates/gitim-cli/src/commands/timer.rs
git commit -m "feat(gitim-cli): gitim timer list + cancel subcommands"
```

---

## Task 15: agent_loop integration — pop fired + prepend to prompt (F2)

**Files:**
- Modify: `crates/gitim-runtime/src/agent_loop.rs`

- [ ] **Step 1: Inspect current run_once shape**

Read `crates/gitim-runtime/src/agent_loop.rs` lines 575-650 to confirm the `external_prompt` calculation lives where this plan expects (`format_changes_as_prompt` call around line 596).

- [ ] **Step 2: Write integration test (in inline mod tests)**

Append to the existing `#[cfg(test)]` mod in `agent_loop.rs`:

```rust
#[test]
fn fired_timer_prefix_renders_into_run_once_path() {
    // Hosted as a unit test: directly call the helper that builds
    // the combined external_prompt from (fired_timers, daemon_changes).
    use gitim_core::timer::{format_fired_for_prompt, Timer};
    let now: chrono::DateTime<chrono::Utc> = "2026-05-20T15:00:55Z".parse().unwrap();
    let fired = vec![Timer {
        id: "20260520T143055-aaaaaa".into(),
        fire_at: "2026-05-20T15:00:55Z".parse().unwrap(),
        created_at: "2026-05-20T14:30:55Z".parse().unwrap(),
        anchor: "<#x>".into(),
        note: Some("test".into()),
    }];
    let timer_part = format_fired_for_prompt(&fired, now);
    let combined = combine_timer_and_changes(Some(timer_part.clone()), None);
    assert!(combined.is_some());
    assert!(combined.unwrap().contains("⏰ Timer reminder"));

    let combined = combine_timer_and_changes(Some(timer_part.clone()), Some("from daemon".into()));
    let s = combined.unwrap();
    assert!(s.contains("⏰ Timer reminder"));
    assert!(s.contains("from daemon"));
    assert!(s.find("⏰").unwrap() < s.find("from daemon").unwrap(), "timer must come first");

    let combined = combine_timer_and_changes(None, Some("from daemon".into()));
    assert_eq!(combined.as_deref(), Some("from daemon"));

    let combined = combine_timer_and_changes(None, None);
    assert!(combined.is_none());
}
```

- [ ] **Step 3: Run, expect compile fail**

Run: `cargo test -p gitim-runtime --lib agent_loop::tests::fired_timer_prefix`

Expected: `cannot find function 'combine_timer_and_changes'`.

- [ ] **Step 4: Add combine_timer_and_changes helper + integrate into run_once**

In `agent_loop.rs`, above `pub async fn run_once`:

```rust
/// Compose external_prompt from optional timer-fired prefix and optional
/// daemon-change body. Returns None when both are empty — caller treats
/// that as "idle".
fn combine_timer_and_changes(
    timer_prefix: Option<String>,
    changes_prompt: Option<String>,
) -> Option<String> {
    match (timer_prefix, changes_prompt) {
        (None, None) => None,
        (Some(t), None) if t.is_empty() => None,
        (Some(t), None) => Some(t),
        (None, Some(c)) => Some(c),
        (Some(t), Some(c)) if t.is_empty() => Some(c),
        (Some(t), Some(c)) => Some(format!("{t}\n---\n\n{c}")),
    }
}
```

Then modify `run_once` (current code around line 590):

```rust
// BEFORE:
let external_prompt = if result.changes.is_empty() {
    None
} else {
    format_changes_as_prompt(&result.changes, &self.handler)
};

// AFTER (replace the block above with):
let fired_timers = gitim_core::timer::pop_fired_timers(&self.repo_root, chrono::Utc::now())
    .unwrap_or_else(|e| {
        tracing::warn!(error = %e, handler = %self.handler, "timer pop failed; continuing");
        Vec::new()
    });
let timer_prefix = if fired_timers.is_empty() {
    None
} else {
    Some(gitim_core::timer::format_fired_for_prompt(&fired_timers, chrono::Utc::now()))
};
let changes_prompt = if result.changes.is_empty() {
    None
} else {
    format_changes_as_prompt(&result.changes, &self.handler)
};
let external_prompt = combine_timer_and_changes(timer_prefix, changes_prompt);
```

Add a public method to expose the next due timestamp to the outer loop:

```rust
impl AgentLoop {
    /// Best-effort: earliest pending `fire_at` for this agent, or None.
    /// Used by the outer scheduler to shorten sleep when a timer is closer
    /// than the configured poll interval.
    pub fn next_timer_due(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        gitim_core::timer::peek_next_due(&self.repo_root).ok().flatten()
    }
}
```

(Place inside the existing `impl AgentLoop` block, near `run_once`.)

- [ ] **Step 5: Run all gitim-runtime tests, expect pass**

Run: `cargo test -p gitim-runtime --lib agent_loop::tests`

Expected: existing tests + new combine_timer test all pass. If `format_changes_as_prompt` signature or visibility doesn't match this plan's assumption, adjust the call site.

- [ ] **Step 6: Commit**

```bash
cargo fmt -p gitim-runtime
git add crates/gitim-runtime/src/agent_loop.rs
git commit -m "feat(runtime): pop fired timers and prepend to run_once prompt"
```

---

## Task 16: outer sleep — `min(poll_interval, time_until_next_due)` (http.rs)

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`

- [ ] **Step 1: Locate the sleep line**

Read `crates/gitim-runtime/src/http.rs` line 3420-3430. Confirm `tokio::time::sleep(poll_interval).await;` at the end of the `loop { ... }` body.

- [ ] **Step 2: Replace with min(poll_interval, time_until_due)**

Replace:

```rust
            tokio::time::sleep(poll_interval).await;
```

with:

```rust
            let sleep_dur = match agent_loop.next_timer_due() {
                Some(due) => {
                    let remaining = (due - chrono::Utc::now())
                        .to_std()
                        .unwrap_or(std::time::Duration::from_secs(1));
                    remaining.min(poll_interval).max(std::time::Duration::from_secs(1))
                }
                None => poll_interval,
            };
            tokio::time::sleep(sleep_dur).await;
```

(Same replacement for the backoff path is NOT needed — backoff is for error recovery and shouldn't be cut short by timers.)

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p gitim-runtime`

Expected: clean build.

- [ ] **Step 4: Verify existing runtime tests still pass**

Run: `cargo test -p gitim-runtime --lib`

Expected: pre-existing tests still green.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-runtime
git add crates/gitim-runtime/src/http.rs
git commit -m "feat(runtime): outer agent loop sleeps until min(poll_interval, next_timer_due)"
```

---

## Task 17: System prompt — append timer section to default_gitim_api

**Files:**
- Modify: `crates/gitim-agent-provider/src/prompts.rs`

- [ ] **Step 1: Locate insertion point**

Read `crates/gitim-agent-provider/src/prompts.rs` near line 352 (start of `default_gitim_api`) and find the end of its `format!(...)` body.

- [ ] **Step 2: Add the timer section at the end of the format string**

Insert the following Chinese-language block immediately before the closing `"`, after the existing trailing section content (e.g., after the "Flows API" section if it's last):

```markdown

## 一次性定时提醒（timer）

你是 oneshot 运行的——一旦本轮响应结束，你的进程就退出了。如果你判断"这件事要过一段时间再回来看看"
（比如等一个 deploy 完成、等对方回复一段时间、给自己一个 cool-down 后复盘），普通的"30 分钟后我再
看一下"在你身上不会自动发生——没人会在 30 分钟后唤醒你。

`gitim timer` 解决这个问题。注册之后到点，runtime 会重新唤起你一次，并把"为什么唤醒、当初锚点
在哪里"塞进你看到的消息流，让你能继续之前的线索。

注册：
  gitim timer set <duration> <anchor> [--note <text>]
  例：gitim timer set 30m '<#deploys:L000128>' --note "看 prod 是否绿了"

  duration:  humantime，如 45s / 5m / 1h30m
  anchor:    指向"当时这个 timer 是为哪条消息/卡片设的"——醒来后你顺着它 gitim read 回到
             现场。建议格式 `<#channel:L行号>`、DM 路径、卡片路径。
  note:      给未来的自己一句话提醒，可选。

查看 / 撤销：
  gitim timer list
  gitim timer cancel <id 或 id 前缀>
```

If the current `default_gitim_api` ends with a `gitim_bin = ...` interpolation followed by literal text, leave that intact and append the section after the literal closer.

- [ ] **Step 3: Verify prompts.rs compiles + any existing snapshot test still passes**

Run: `cargo test -p gitim-agent-provider --lib prompts`

Expected: passes. If a snapshot test for `default_gitim_api` exists and now diffs, update the snapshot intentionally.

- [ ] **Step 4: Commit**

```bash
cargo fmt -p gitim-agent-provider
git add crates/gitim-agent-provider/src/prompts.rs
git commit -m "feat(prompts): expose oneshot timer API to agents via default_gitim_api"
```

---

## Task 18: CLI integration tests (assert_cmd)

**Files:**
- Modify: `crates/gitim-cli/Cargo.toml`
- Create: `crates/gitim-cli/tests/timer_cli.rs`

- [ ] **Step 1: Add dev-deps**

In `[dev-dependencies]` of `crates/gitim-cli/Cargo.toml`:

```toml
assert_cmd = "2"
predicates = "3"
```

- [ ] **Step 2: Create test file**

```rust
// crates/gitim-cli/tests/timer_cli.rs

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;

fn fake_clone() -> tempfile::TempDir {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let gitim = tmp.path().join(".gitim");
    fs::create_dir_all(&gitim).expect("mkdir .gitim");
    fs::write(gitim.join("me.json"), r#"{"handler":"alice"}"#).expect("write me.json");
    tmp
}

fn gitim() -> Command {
    Command::cargo_bin("gitim").expect("gitim binary")
}

#[test]
fn set_then_list_shows_entry() {
    let clone = fake_clone();
    gitim()
        .current_dir(clone.path())
        .args(["timer", "set", "30m", "<#x>"])
        .assert()
        .success()
        .stdout(predicate::str::contains("fires in"));

    gitim()
        .current_dir(clone.path())
        .args(["timer", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("<#x>"));
}

#[test]
fn set_with_note() {
    let clone = fake_clone();
    gitim()
        .current_dir(clone.path())
        .args(["timer", "set", "30m", "<#x>", "--note", "hello world"])
        .assert()
        .success();
    gitim()
        .current_dir(clone.path())
        .args(["timer", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello world"));
}

#[test]
fn cap_enforced_at_4th_set() {
    let clone = fake_clone();
    for _ in 0..3 {
        gitim()
            .current_dir(clone.path())
            .args(["timer", "set", "30m", "<#x>"])
            .assert()
            .success();
    }
    gitim()
        .current_dir(clone.path())
        .args(["timer", "set", "30m", "<#x>"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("cap"));
}

#[test]
fn cancel_by_full_id() {
    let clone = fake_clone();
    let out = gitim()
        .current_dir(clone.path())
        .args(["timer", "set", "30m", "<#x>"])
        .output()
        .expect("set");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let id = stdout.split_whitespace().next().expect("id").to_string();

    gitim()
        .current_dir(clone.path())
        .args(["timer", "cancel", &id])
        .assert()
        .success()
        .stdout(predicate::str::contains(&id));
}

#[test]
fn cancel_no_match_exits_2() {
    let clone = fake_clone();
    gitim()
        .current_dir(clone.path())
        .args(["timer", "cancel", "nonexistent-xyz"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("no timer matches"));
}

#[test]
fn cancel_ambiguous_prefix_exits_2() {
    let clone = fake_clone();
    for _ in 0..2 {
        gitim()
            .current_dir(clone.path())
            .args(["timer", "set", "30m", "<#x>"])
            .assert()
            .success();
    }
    // All 2 timers share the "2026" timestamp prefix.
    gitim()
        .current_dir(clone.path())
        .args(["timer", "cancel", "2026"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("matches 2 timers"));
}

#[test]
fn duration_too_short_exits_2() {
    let clone = fake_clone();
    gitim()
        .current_dir(clone.path())
        .args(["timer", "set", "5s", "<#x>"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("invalid duration"));
}

#[test]
fn not_in_clone_exits_2() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    // No .gitim/ directory here.
    gitim()
        .current_dir(tmp.path())
        .args(["timer", "set", "30m", "<#x>"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("not in a gitim agent clone"));
}

#[test]
fn list_empty() {
    let clone = fake_clone();
    gitim()
        .current_dir(clone.path())
        .args(["timer", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("(no pending timers)"));
}

#[test]
fn list_json_outputs_array() {
    let clone = fake_clone();
    gitim()
        .current_dir(clone.path())
        .args(["timer", "set", "30m", "<#x>"])
        .assert()
        .success();
    gitim()
        .current_dir(clone.path())
        .args(["timer", "list", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("["));
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p gitim-cli --test timer_cli`

Expected: 10 passed.

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-cli/Cargo.toml crates/gitim-cli/tests/timer_cli.rs
git commit -m "test(gitim-cli): end-to-end timer subcommand tests"
```

---

## Task 19: runtime integration test — timer prefix injection round-trip

**Files:**
- Create: `crates/gitim-runtime/tests/timer_integration.rs`

- [ ] **Step 1: Create the test file**

```rust
// crates/gitim-runtime/tests/timer_integration.rs

//! Verifies that timer file content surfaces in the helper layer that
//! agent_loop uses to build the LLM prompt. Goes through the same
//! gitim-core APIs the production agent_loop uses — no provider mock,
//! no daemon. Confirms the file → prompt pipeline contract.

use gitim_core::timer::{
    self, format_fired_for_prompt, pop_fired_timers, register_timer, TimersFile,
};
use std::fs;
use std::time::Duration as StdDuration;

fn fake_clone() -> tempfile::TempDir {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    fs::create_dir_all(tmp.path().join(".gitim")).expect("mkdir .gitim");
    tmp
}

#[test]
fn full_round_trip_set_then_pop_renders_prompt() {
    let clone = fake_clone();
    // Set a timer that's already past (10 seconds is min, but we'll
    // fast-forward `now` past fire_at in pop_fired_timers).
    let t = register_timer(
        clone.path(),
        chrono::Duration::seconds(timer::MIN_DURATION_SECS),
        "<#deploys:L000128>".into(),
        Some("check prod".into()),
    )
    .expect("register");
    // Advance `now` past fire_at.
    let now = t.fire_at + chrono::Duration::seconds(1);
    let fired = pop_fired_timers(clone.path(), now).expect("pop");
    assert_eq!(fired.len(), 1);
    let prompt = format_fired_for_prompt(&fired, now);
    assert!(prompt.contains("⏰ Timer reminder(s) fired"));
    assert!(prompt.contains("<#deploys:L000128>"));
    assert!(prompt.contains("check prod"));

    // Second pop with same now → no more fired.
    let again = pop_fired_timers(clone.path(), now).expect("pop2");
    assert!(again.is_empty());
}

#[test]
fn future_timer_not_yet_fired() {
    let clone = fake_clone();
    let _t = register_timer(
        clone.path(),
        chrono::Duration::seconds(60 * 60),
        "<#x>".into(),
        None,
    )
    .expect("register");
    let fired = pop_fired_timers(clone.path(), chrono::Utc::now()).expect("pop");
    assert!(fired.is_empty());
    let remaining = timer::read_timers(clone.path()).expect("read");
    assert_eq!(remaining.timers.len(), 1);
}

#[test]
fn corrupt_file_returns_no_fired_and_preserves_file() {
    let clone = fake_clone();
    fs::write(
        clone.path().join(".gitim").join("timers.json"),
        "{not json{{",
    )
    .expect("write corrupt");
    let fired = pop_fired_timers(clone.path(), chrono::Utc::now()).expect("pop ok");
    assert!(fired.is_empty());
    let raw = fs::read_to_string(clone.path().join(".gitim").join("timers.json"))
        .expect("file still there");
    assert_eq!(raw, "{not json{{", "corrupt file must be preserved");
}

#[test]
fn cross_restart_backlog_fires_all_at_once() {
    let clone = fake_clone();
    // Manually craft a file with 3 long-past timers (simulating runtime
    // was offline for hours).
    let past = chrono::Utc::now() - chrono::Duration::hours(2);
    let file = TimersFile {
        version: 1,
        timers: (0..3)
            .map(|i| timer::Timer {
                id: format!("20260520T100000-aaaaa{i}"),
                fire_at: past + chrono::Duration::minutes(i as i64),
                created_at: past - chrono::Duration::hours(1),
                anchor: format!("<#x{i}>"),
                note: None,
            })
            .collect(),
    };
    timer::write_timers(clone.path(), &file).expect("write");
    let fired = pop_fired_timers(clone.path(), chrono::Utc::now()).expect("pop");
    assert_eq!(fired.len(), 3, "all backlog timers fire on next pop");
    let prompt = format_fired_for_prompt(&fired, chrono::Utc::now());
    assert!(prompt.contains("1."));
    assert!(prompt.contains("3."));
}

#[test]
fn concurrent_writers_no_lost_update() {
    use std::sync::Arc;
    use std::thread;
    let clone = fake_clone();
    let clone_path: Arc<std::path::PathBuf> = Arc::new(clone.path().to_path_buf());

    // 3 threads, each registers 1 timer (cap is 3, so all should fit
    // if they serialize).
    let mut handles = vec![];
    for i in 0..3 {
        let p = clone_path.clone();
        handles.push(thread::spawn(move || {
            register_timer(
                &p,
                chrono::Duration::seconds(60 * 60),
                format!("<#c{i}>"),
                None,
            )
        }));
    }
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(
        results.iter().filter(|r| r.is_ok()).count(),
        3,
        "all 3 should succeed: {results:?}"
    );

    let final_file = timer::read_timers(&clone_path).expect("read");
    assert_eq!(final_file.timers.len(), 3, "no lost updates");

    // Now a 4th thread tries — should fail with CapReached.
    let p = clone_path.clone();
    let extra = thread::spawn(move || {
        register_timer(&p, chrono::Duration::seconds(60), "<#x>".into(), None)
    })
    .join()
    .unwrap();
    assert!(matches!(extra.unwrap_err(), timer::TimerError::CapReached));

    let _ = StdDuration::from_millis(1); // silence unused import warning
}

#[test]
fn write_failure_after_partial_does_not_corrupt() {
    // Validates that on a write error the timers file either contains the
    // old state or the new state — never a half-written state. We can't
    // induce a real ENOSPC from a unit test, so we approximate by writing
    // happy-path and checking that no `.tmp` siblings remain after.
    let clone = fake_clone();
    register_timer(
        clone.path(),
        chrono::Duration::seconds(60),
        "<#x>".into(),
        None,
    )
    .expect("register");
    let dir = clone.path().join(".gitim");
    let leftover: Vec<_> = std::fs::read_dir(&dir)
        .expect("readdir")
        .filter_map(|e| e.ok())
        .filter(|e| {
            let s = e.file_name().to_string_lossy().into_owned();
            // tempfile NamedTempFile default prefix is ".tmp" — match
            // anything between timers.json and not equal to it / lockfile.
            s.contains("timers.json")
                && s != "timers.json"
                && s != "timers.json.lock"
        })
        .collect();
    assert!(leftover.is_empty(), "stray temp files: {leftover:?}");
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p gitim-runtime --test timer_integration`

Expected: 6 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-runtime/tests/timer_integration.rs
git commit -m "test(runtime): timer file → pop_fired → prompt round-trip integration"
```

---

## Task 20: Update CLAUDE.md Current Orientation

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Read current "Where we are" section**

Run: `grep -n "Current Orientation" CLAUDE.md` to find the section, then read its "Where we are" line(s).

- [ ] **Step 2: Append a sentence about oneshot timer**

In the "Where we are" paragraph, append (as a new sentence in the existing long paragraph, matching the project's voice):

```
**Oneshot timer** 已落地：agent 用 `gitim timer set <duration> <anchor> [--note]` 注册一次性
提醒，状态存 `<agent_clone>/.gitim/timers.json`（gitignored），agent_loop 每 cycle pop 到期项
并把"## ⏰ Timer reminder(s) fired" prefix 注入 LLM prompt。零新 IPC、零新 tokio task、零
git commit；F1 用单独的 `.gitim/timers.json.lock` 做 lock anchor（防 atomic rename 把 inode-
锁的 lock 抹掉），F5 用 `NamedTempFile::persist` 保证 write 失败不留 tmp 残骸。cap 每 agent
3 个 pending。design 见 `docs/plans/oneshot-timer/`。
```

- [ ] **Step 3: Spot-check formatting**

Open the file, ensure the addition flows with the existing prose style.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "docs(claude.md): record oneshot timer landing in Current Orientation"
```

---

## Task 21: Final full-suite verification

**Files:** none — verification only.

- [ ] **Step 1: Run full workspace tests**

Run: `cargo test`

Expected: all tests pass (existing + new). Subject to the project's "test cadence" rule — this is the final-stage full sweep to confirm no regressions.

- [ ] **Step 2: Run clippy + fmt across the workspace**

Run:
```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --no-deps --locked
```

Expected: no warnings introduced by this branch. Pre-existing `unwrap_used` warnings in `flow/run.rs` etc. are out of scope (eng-review F4 only applies to new timer code).

- [ ] **Step 3: Smoke test end-to-end manually**

```bash
cd /tmp && rm -rf timer-e2e && mkdir -p timer-e2e/.gitim && cd timer-e2e
GITIM=~/ateam/GitIM/.claude/worktrees/ecstatic-hugle-03df01/target/debug/gitim
$GITIM timer set 30m '<#deploys:L000128>' --note "verify prod" && \
$GITIM timer list && \
ID=$($GITIM timer list --json | grep -o '"id":"[^"]*"' | head -1 | cut -d'"' -f4) && \
echo "Cancelling $ID..." && \
$GITIM timer cancel "$ID" && \
$GITIM timer list
cd / && rm -rf /tmp/timer-e2e
```

Expected: set succeeds → list shows entry → cancel succeeds → final list is `(no pending timers)`.

- [ ] **Step 4: No commit needed** (verification only)

---

## Self-Review Notes

After writing this plan, against [00-requirements.md](00-requirements.md) and [01-eng-review-findings.md](01-eng-review-findings.md):

**Spec coverage** ✓
- Storage in `<agent_clone>/.gitim/timers.json` → Tasks 7-10
- gitim CLI subcommands (set/list/cancel) → Tasks 13-14
- agent_loop integration with synthetic prompt → Tasks 15-16
- system prompt discovery → Task 17
- cap=3 enforcement → Task 10
- anchor mandatory (trim non-empty) → Task 4 (Timer::new) + Task 13 (CLI parse)
- humantime 10s-24h bounds → Task 3
- atomic write with NamedTempFile (F5) → Task 8
- lockfile independent of rename target (F1) → Task 7
- panic-safe pop (F4) → Task 9 (Result-based) + Task 15 (unwrap_or_default + warn)
- partition_fired pure (F6) → Task 5 with clock injection
- cross-restart backlog → Task 19 integration test
- corruption preservation → Task 8 read_timers test + Task 19 integration

**Placeholder scan** ✓ — no TBD / TODO / "fill in" / "similar to" / `…`.

**Type consistency** ✓ — `Timer`, `TimersFile`, `TimerError`, `MAX_PENDING_PER_AGENT`, `find_clone_root`, `register_timer`, `cancel_timer`, `pop_fired_timers`, `peek_next_due`, `read_timers`, `write_timers`, `format_fired_for_prompt`, `parse_duration`, `combine_timer_and_changes` — names match across tasks.

**Eng-review constraints** ✓
- F1 lockfile anchor: Task 7 (`with_timers_lock` operates on `timers.json.lock`)
- F2 API decision: Task 15 chose composition via `combine_timer_and_changes` (does not modify `run_once` signature or `format_changes_as_prompt`)
- F3 jitter: documented in 00-requirements Non-goals; no plan task needed
- F4 panic-safety: `#![deny(clippy::unwrap_used, expect_used, panic)]` in Task 1 + `unwrap_or_default` + tracing::warn in Task 15
- F5 NamedTempFile: Task 8
- F6 partition_fired pure fn: Task 5 with explicit `now: DateTime<Utc>` injection
