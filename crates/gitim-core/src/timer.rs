// crates/gitim-core/src/timer.rs

//! Oneshot timer: per-agent file-backed pending wake-ups.
//!
//! Design: `docs/plans/oneshot-timer/00-requirements.md`
//! Constraints: `docs/plans/oneshot-timer/01-eng-review-findings.md`

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timer {
    pub id: String,
    pub fire_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub anchor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Timer {
    pub fn new_id() -> Result<String, TimerError> {
        let now = Utc::now();
        let timestamp = now.format("%Y%m%dT%H%M%S");
        let mut hash_bytes = [0u8; 3];
        getrandom::getrandom(&mut hash_bytes)?;
        let hash = hex::encode(hash_bytes);
        Ok(format!("{timestamp}-{hash}"))
    }

    pub fn new(
        duration: ChronoDuration,
        anchor: String,
        note: Option<String>,
    ) -> Result<Self, TimerError> {
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

/// Remove and return the cancelled id. The needle is matched as a substring
/// against `id` (lets users type either a full id or the random hash suffix
/// like `a3f`). 0 or >1 matches → error and `timers` is left unchanged.
pub fn cancel_by_id_or_prefix(timers: &mut Vec<Timer>, prefix: &str) -> Result<String, TimerError> {
    let matches: Vec<usize> = timers
        .iter()
        .enumerate()
        .filter(|(_, t)| t.id.contains(prefix))
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
    tmp.persist(&target).map_err(|e| TimerError::Io(e.error))?;
    Ok(())
}

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

pub fn parse_duration(s: &str) -> Result<ChronoDuration, TimerError> {
    // Reject whitespace-bearing strings ("30 minutes" etc) — humantime accepts
    // them but we want compact CLI-friendly forms only.
    if s.chars().any(char::is_whitespace) {
        return Err(TimerError::InvalidDuration(format!(
            "{s:?}: whitespace not allowed; use compact form like 30m or 1h30m"
        )));
    }
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn timers_file_serde_round_trip() {
        let original = TimersFile {
            version: 1,
            timers: vec![Timer {
                id: "20260520T143055-a3f4c2".into(),
                fire_at: "2026-05-20T15:00:55Z"
                    .parse()
                    .unwrap_or_else(|_| Utc::now()),
                created_at: "2026-05-20T14:30:55Z"
                    .parse()
                    .unwrap_or_else(|_| Utc::now()),
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
                16..=21 => b.is_ascii_digit() || (b'a'..=b'f').contains(&b),
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
        std::fs::write(gitim.join(TIMERS_FILENAME), r#"{"version":99,"timers":[]}"#).unwrap();
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
        let before_mtime = std::fs::metadata(timers_path(clone))
            .unwrap()
            .modified()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let now: DateTime<Utc> = "2026-05-20T15:00:00Z".parse().unwrap();
        let fired = pop_fired_timers(clone, now).unwrap();
        assert!(fired.is_empty());
        let after_mtime = std::fs::metadata(timers_path(clone))
            .unwrap()
            .modified()
            .unwrap();
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
}
