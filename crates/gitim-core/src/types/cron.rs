//! `CronSpec` — schedule + target + prompt for a cron trigger.
//!
//! Each `crons/<name>/spec.yaml` parses into one of these. Validation is
//! a separate pass (see `validate`) so callers can synthesize a spec in
//! memory and assert it passes the same checks the YAML loader applies.

use std::collections::BTreeMap;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use croner::Cron;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::handler::Handler;

/// Hard cap on prompt size — 8 KiB measured in **bytes**, not characters.
/// Prompts get committed to git on every fire; an oversized prompt would
/// spam the audit log and stress agent context windows. The limit is
/// generous enough for a paragraph-length instruction with a few example
/// links.
///
/// Byte length matters for multi-byte content: a CJK character takes 3
/// bytes in UTF-8 and most emoji take 4, so an 8 KiB Chinese prompt is
/// roughly ~2,700 characters and an emoji-heavy prompt is even less. If
/// you're hitting the cap with what looks like a short prompt, that's
/// why — it's the byte budget, not the character count.
pub const MAX_PROMPT_BYTES: usize = 8 * 1024;

/// Current schema version. Stored in spec.yaml so older daemons can
/// reject specs they don't understand instead of silently misinterpreting.
pub const CURRENT_VERSION: u32 = 1;

#[derive(Error, Debug)]
pub enum CronSpecError {
    #[error("unsupported spec version {0}, expected {CURRENT_VERSION}")]
    InvalidVersion(u32),
    #[error("invalid cron schedule: {0}")]
    InvalidSchedule(String),
    #[error("invalid IANA timezone: {0}")]
    InvalidTimezone(String),
    #[error("prompt cannot be empty")]
    EmptyPrompt,
    #[error("prompt exceeds {} bytes (got {len})", MAX_PROMPT_BYTES)]
    OversizedPrompt { len: usize },
    #[error("invalid created_at timestamp: {0}")]
    InvalidCreatedAt(String),
    #[error("created_at must be UTC (end with 'Z'), got: {0}")]
    CreatedAtNotUtc(String),
    #[error("YAML parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),
}

/// Reject names that would alias the archive convention or shadow the
/// `crons/` root. `archive` and `crons` are the two top-level neighbors a
/// stem could collide with after `git mv`; `.`-prefixed names would clash
/// with hidden-file discipline (`.gitim/`, `.git/`).
const RESERVED_CRON_NAMES: &[&str] = &["archive", "crons"];

/// Why a candidate cron name was rejected. Stable variants — runtime and
/// daemon both map these onto the wire `error_code: "invalid_name"`, so
/// reordering or splitting variants is a wire-visible change.
#[derive(Error, Debug, PartialEq, Eq)]
pub enum CronNameError {
    #[error("cron name cannot be empty")]
    Empty,
    #[error("cron name exceeds 63 characters (got {0})")]
    TooLong(usize),
    #[error("cron name '{0}' cannot start with '.'")]
    DotPrefix(String),
    #[error("cron name '{0}' is reserved")]
    Reserved(String),
    #[error("cron name '{0}' must start with a lowercase letter or digit")]
    BadFirstChar(String),
    #[error("cron name '{name}' contains invalid character '{ch}' (allowed: a-z 0-9 -)")]
    BadChar { name: String, ch: char },
}

/// Validate `<name>` against `^[a-z0-9][a-z0-9-]{0,62}$` plus the reserved
/// list. Lifted from the daemon so the runtime HTTP layer (which constructs
/// disk paths from the URL `<name>` segment in `crons/<name>/<ts>` reads)
/// can enforce the same rule before any path join — preventing path
/// traversal via `..`, percent-encoded slashes, or any other shape the
/// regex would reject.
///
/// Same shape as channel names (`ChannelName::new`) but kept separate
/// because cron names live in their own namespace and we don't want a
/// future channel-name policy change to silently re-shape cron rules.
pub fn validate_cron_name(name: &str) -> Result<(), CronNameError> {
    if name.is_empty() {
        return Err(CronNameError::Empty);
    }
    if name.len() > 63 {
        return Err(CronNameError::TooLong(name.len()));
    }
    if name.starts_with('.') {
        return Err(CronNameError::DotPrefix(name.to_string()));
    }
    if RESERVED_CRON_NAMES.contains(&name) {
        return Err(CronNameError::Reserved(name.to_string()));
    }
    let mut chars = name.chars();
    let first = chars.next().expect("non-empty checked above");
    if !matches!(first, 'a'..='z' | '0'..='9') {
        return Err(CronNameError::BadFirstChar(name.to_string()));
    }
    for ch in std::iter::once(first).chain(chars) {
        if !matches!(ch, 'a'..='z' | '0'..='9' | '-') {
            return Err(CronNameError::BadChar {
                name: name.to_string(),
                ch,
            });
        }
    }
    Ok(())
}

fn default_version() -> u32 {
    CURRENT_VERSION
}

fn default_enabled() -> bool {
    true
}

/// Cron spec as serialized to `crons/<name>/spec.yaml`.
///
/// Unknown fields are captured into `extra` so older clients writing newer
/// specs back round-trip cleanly (forward-compat).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CronSpec {
    #[serde(default = "default_version")]
    pub version: u32,
    pub schedule: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    pub target: Handler,
    pub prompt: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub created_by: Handler,
    pub created_at: String,
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl CronSpec {
    /// Parse a spec.yaml string + run full validation. Caller does not need
    /// a separate `.validate()` step.
    pub fn from_yaml(s: &str) -> Result<Self, CronSpecError> {
        let spec: CronSpec = serde_yaml::from_str(s)?;
        spec.validate()?;
        Ok(spec)
    }

    /// Serialize back to YAML, preserving any `extra` fields. Does NOT
    /// re-validate — caller should typically have constructed via
    /// `from_yaml` or be the daemon writing a freshly-validated spec.
    pub fn to_yaml(&self) -> Result<String, CronSpecError> {
        Ok(serde_yaml::to_string(self)?)
    }

    /// Pure validation pass — no IO, no side effects. Run on every spec
    /// before it's persisted.
    pub fn validate(&self) -> Result<(), CronSpecError> {
        if self.version != CURRENT_VERSION {
            return Err(CronSpecError::InvalidVersion(self.version));
        }

        Cron::from_str(&self.schedule)
            .map_err(|e| CronSpecError::InvalidSchedule(format!("{e}")))?;

        if let Some(tz) = &self.timezone {
            tz.parse::<Tz>()
                .map_err(|e| CronSpecError::InvalidTimezone(format!("{tz}: {e}")))?;
        }

        // Handler types validate themselves via TryFrom<String> on deserialize,
        // so if `target: Handler` exists at all it's well-formed. Daemon-side
        // existence checks (does `users/<target>.meta.yaml` exist) live in the
        // handler, not the spec — that's a runtime fact, not a schema fact.

        if self.prompt.is_empty() {
            return Err(CronSpecError::EmptyPrompt);
        }
        if self.prompt.len() > MAX_PROMPT_BYTES {
            return Err(CronSpecError::OversizedPrompt {
                len: self.prompt.len(),
            });
        }

        DateTime::parse_from_rfc3339(&self.created_at)
            .map_err(|e| CronSpecError::InvalidCreatedAt(format!("{}: {e}", self.created_at)))?;

        // UTC-only invariant: Wave 2's engine derives `<theoretical_ts>.thread`
        // filenames directly from these timestamps for idempotency, so mixing
        // offsets here ("2026-05-09T10:00:00+08:00" vs "2026-05-09T02:00:00Z"
        // — same instant, different filenames) would let two clones fire the
        // same job twice. Reject anything that doesn't end with Z.
        if !self.created_at.ends_with('Z') {
            return Err(CronSpecError::CreatedAtNotUtc(self.created_at.clone()));
        }

        Ok(())
    }

    /// Whether this spec should be considered for firing. Single point so
    /// future archive-style flags can be added without touching every caller.
    pub fn is_active(&self) -> bool {
        self.enabled
    }
}

/// Compute the next time the spec should fire, strictly after `after`.
///
/// `after` is in UTC; the result is in UTC. Internally the spec's timezone
/// (default UTC) is used to interpret the cron expression so wall-clock
/// schedules like "9am Pacific" honor DST transitions.
pub fn next_fire_after(
    spec: &CronSpec,
    after: DateTime<Utc>,
) -> Result<DateTime<Utc>, CronSpecError> {
    let cron = Cron::from_str(&spec.schedule)
        .map_err(|e| CronSpecError::InvalidSchedule(format!("{e}")))?;

    let tz: Tz = match &spec.timezone {
        Some(s) => s
            .parse()
            .map_err(|e| CronSpecError::InvalidTimezone(format!("{s}: {e}")))?,
        None => Tz::UTC,
    };

    let after_in_tz = after.with_timezone(&tz);
    let next_in_tz = cron
        .find_next_occurrence(&after_in_tz, false)
        .map_err(|e| CronSpecError::InvalidSchedule(format!("{e}")))?;

    Ok(next_in_tz.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn minimal_yaml() -> String {
        r#"
schedule: "0 9 * * 1"
target: alice
prompt: "weekly checkin"
created_by: alice
created_at: "2026-05-09T10:00:00Z"
"#
        .to_string()
    }

    fn full_yaml() -> String {
        r#"
version: 1
schedule: "0 9 * * 1"
timezone: "America/Los_Angeles"
target: alice
prompt: "weekly checkin"
enabled: false
created_by: bob
created_at: "2026-05-09T10:00:00Z"
"#
        .to_string()
    }

    #[test]
    fn parse_minimal_yaml() {
        let spec = CronSpec::from_yaml(&minimal_yaml()).unwrap();
        assert_eq!(spec.version, 1);
        assert_eq!(spec.schedule, "0 9 * * 1");
        assert_eq!(spec.timezone, None);
        assert_eq!(spec.target.as_str(), "alice");
        assert_eq!(spec.prompt, "weekly checkin");
        assert!(spec.enabled);
        assert_eq!(spec.created_by.as_str(), "alice");
        assert_eq!(spec.created_at, "2026-05-09T10:00:00Z");
        assert!(spec.extra.is_empty());
    }

    #[test]
    fn parse_full_yaml() {
        let spec = CronSpec::from_yaml(&full_yaml()).unwrap();
        assert_eq!(spec.version, 1);
        assert_eq!(spec.schedule, "0 9 * * 1");
        assert_eq!(spec.timezone.as_deref(), Some("America/Los_Angeles"));
        assert_eq!(spec.target.as_str(), "alice");
        assert!(!spec.enabled);
        assert_eq!(spec.created_by.as_str(), "bob");
    }

    #[test]
    fn roundtrip_preserves_extra() {
        let yaml = r#"
schedule: "0 9 * * 1"
target: alice
prompt: "weekly"
created_by: alice
created_at: "2026-05-09T10:00:00Z"
future_flag_v2: "experimental"
priority: 7
"#;
        let spec = CronSpec::from_yaml(yaml).unwrap();
        assert_eq!(spec.extra.len(), 2);
        assert_eq!(
            spec.extra.get("future_flag_v2"),
            Some(&serde_json::json!("experimental"))
        );
        assert_eq!(spec.extra.get("priority"), Some(&serde_json::json!(7)));

        let dumped = spec.to_yaml().unwrap();
        let reparsed = CronSpec::from_yaml(&dumped).unwrap();
        assert_eq!(spec, reparsed);
    }

    #[test]
    fn reject_invalid_schedule() {
        let yaml = r#"
schedule: "not a cron"
target: alice
prompt: "x"
created_by: alice
created_at: "2026-05-09T10:00:00Z"
"#;
        let err = CronSpec::from_yaml(yaml).unwrap_err();
        assert!(matches!(err, CronSpecError::InvalidSchedule(_)));
    }

    #[test]
    fn reject_invalid_timezone() {
        let yaml = r#"
schedule: "0 9 * * 1"
timezone: "Mars/Olympus_Mons"
target: alice
prompt: "x"
created_by: alice
created_at: "2026-05-09T10:00:00Z"
"#;
        let err = CronSpec::from_yaml(yaml).unwrap_err();
        assert!(matches!(err, CronSpecError::InvalidTimezone(_)));
    }

    #[test]
    fn reject_invalid_target_handler() {
        // Capital letters are not allowed in handlers — Handler's TryFrom
        // surfaces this as a serde_yaml error before our validate() runs.
        let yaml = r#"
schedule: "0 9 * * 1"
target: ALICE
prompt: "x"
created_by: alice
created_at: "2026-05-09T10:00:00Z"
"#;
        let err = CronSpec::from_yaml(yaml).unwrap_err();
        assert!(matches!(err, CronSpecError::Yaml(_)));
    }

    #[test]
    fn reject_empty_prompt() {
        let yaml = r#"
schedule: "0 9 * * 1"
target: alice
prompt: ""
created_by: alice
created_at: "2026-05-09T10:00:00Z"
"#;
        let err = CronSpec::from_yaml(yaml).unwrap_err();
        assert!(matches!(err, CronSpecError::EmptyPrompt));
    }

    #[test]
    fn reject_oversized_prompt() {
        let huge = "a".repeat(MAX_PROMPT_BYTES + 1);
        let yaml = format!(
            r#"
schedule: "0 9 * * 1"
target: alice
prompt: "{huge}"
created_by: alice
created_at: "2026-05-09T10:00:00Z"
"#
        );
        let err = CronSpec::from_yaml(&yaml).unwrap_err();
        match err {
            CronSpecError::OversizedPrompt { len } => assert_eq!(len, MAX_PROMPT_BYTES + 1),
            other => panic!("expected OversizedPrompt, got {other:?}"),
        }
    }

    #[test]
    fn default_timezone_is_utc() {
        let spec = CronSpec::from_yaml(&minimal_yaml()).unwrap();
        assert_eq!(spec.timezone, None);
        // Confirm round-trip preserves "absent" rather than rewriting it.
        let dumped = spec.to_yaml().unwrap();
        assert!(!dumped.contains("timezone:"), "dumped:\n{dumped}");
    }

    #[test]
    fn default_enabled_is_true() {
        let spec = CronSpec::from_yaml(&minimal_yaml()).unwrap();
        assert!(spec.enabled);
        assert!(spec.is_active());
    }

    #[test]
    fn default_version_is_1() {
        let spec = CronSpec::from_yaml(&minimal_yaml()).unwrap();
        assert_eq!(spec.version, 1);
    }

    #[test]
    fn reject_version_2() {
        let yaml = r#"
version: 2
schedule: "0 9 * * 1"
target: alice
prompt: "x"
created_by: alice
created_at: "2026-05-09T10:00:00Z"
"#;
        let err = CronSpec::from_yaml(yaml).unwrap_err();
        assert!(matches!(err, CronSpecError::InvalidVersion(2)));
    }

    #[test]
    fn accept_created_at_utc_z() {
        // A trailing 'Z' is the only accepted form; this is the same
        // string the existing tests use, but called out explicitly.
        let spec = CronSpec::from_yaml(&minimal_yaml()).unwrap();
        assert!(spec.created_at.ends_with('Z'));
    }

    #[test]
    fn reject_created_at_non_utc_offset() {
        // Wave 2's engine derives thread filenames from these timestamps
        // for idempotency. Same instant under a different offset would
        // hash to a different filename and let the same fire happen
        // twice across clones — reject anything but UTC.
        let yaml = r#"
schedule: "0 9 * * 1"
target: alice
prompt: "x"
created_by: alice
created_at: "2026-05-09T10:00:00+08:00"
"#;
        let err = CronSpec::from_yaml(yaml).unwrap_err();
        match err {
            CronSpecError::CreatedAtNotUtc(s) => {
                assert!(s.contains("+08:00"), "echoed value: {s}");
            }
            other => panic!("expected CreatedAtNotUtc, got {other:?}"),
        }
    }

    fn spec_with(schedule: &str, timezone: Option<&str>) -> CronSpec {
        CronSpec {
            version: 1,
            schedule: schedule.to_string(),
            timezone: timezone.map(String::from),
            target: Handler::new("alice").unwrap(),
            prompt: "x".to_string(),
            enabled: true,
            created_by: Handler::new("alice").unwrap(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            extra: BTreeMap::new(),
        }
    }

    mod next_fire {
        use super::*;
        use chrono::TimeZone;
        use pretty_assertions::assert_eq;

        #[test]
        fn next_monday_9am_from_sunday() {
            // 2026-05-10 is a Sunday. Schedule fires Mondays at 9am UTC.
            let spec = spec_with("0 9 * * 1", None);
            let sunday = Utc.with_ymd_and_hms(2026, 5, 10, 12, 0, 0).unwrap();
            let next = next_fire_after(&spec, sunday).unwrap();
            assert_eq!(next, Utc.with_ymd_and_hms(2026, 5, 11, 9, 0, 0).unwrap());
        }

        #[test]
        fn alias_daily_from_arbitrary_time() {
            // @daily expands to "0 0 * * *". Next fire is the next 00:00 UTC.
            let spec = spec_with("@daily", None);
            let now = Utc.with_ymd_and_hms(2026, 5, 9, 14, 37, 12).unwrap();
            let next = next_fire_after(&spec, now).unwrap();
            assert_eq!(next, Utc.with_ymd_and_hms(2026, 5, 10, 0, 0, 0).unwrap());
        }

        #[test]
        fn tz_la_morning_from_utc() {
            // schedule "0 9 * * *" in America/Los_Angeles.
            // In May 2026 LA is on PDT (UTC-7), so LA 09:00 == UTC 16:00.
            let spec = spec_with("0 9 * * *", Some("America/Los_Angeles"));

            // From UTC 16:00 (= LA 09:00 same day) — already fired this minute,
            // expect next day's LA 09:00 since search is strictly-after.
            let from_morning = Utc.with_ymd_and_hms(2026, 5, 9, 16, 0, 0).unwrap();
            let next1 = next_fire_after(&spec, from_morning).unwrap();
            assert_eq!(next1, Utc.with_ymd_and_hms(2026, 5, 10, 16, 0, 0).unwrap());

            // From UTC 18:00 (= LA 11:00 same day) — also already fired,
            // expect next day's LA 09:00.
            let from_afternoon = Utc.with_ymd_and_hms(2026, 5, 9, 18, 0, 0).unwrap();
            let next2 = next_fire_after(&spec, from_afternoon).unwrap();
            assert_eq!(next2, Utc.with_ymd_and_hms(2026, 5, 10, 16, 0, 0).unwrap());
        }

        #[test]
        fn dst_forward_no_double_fire() {
            // US DST forward: 2026-03-08 02:00 LA → 03:00 LA.
            // Schedule "30 2 * * *" — 02:30 LA does not exist on 2026-03-08.
            //
            // croner's documented behavior for fixed-time jobs in a DST gap
            // (see croner src/lib.rs::test_dst_gap_fixed_time_job) is to SNAP
            // to the first valid wall-clock time after the gap, not skip the
            // day. So the "missed" 02:30 PST becomes 03:00 PDT = UTC 10:00
            // (LA was UTC-8 before the transition, becomes UTC-7 after).
            //
            // The plan anticipated "skip to next day" but croner snaps; we
            // assert what croner actually does. The critical invariant either
            // way is NO double-fire — verified below by checking that the
            // fire AFTER the snap lands on the FOLLOWING day, not somewhere
            // else on 2026-03-08.
            let spec = spec_with("30 2 * * *", Some("America/Los_Angeles"));

            // Just before the gap: 2026-03-08 01:59 LA PST = UTC 09:59.
            let before_gap = Utc.with_ymd_and_hms(2026, 3, 8, 9, 59, 0).unwrap();
            let next = next_fire_after(&spec, before_gap).unwrap();

            // Snap to first valid wall-clock time after the gap: 03:00 LA PDT
            // = UTC 10:00.
            let snap = Utc.with_ymd_and_hms(2026, 3, 8, 10, 0, 0).unwrap();
            assert_eq!(next, snap, "croner snaps fixed-time jobs out of DST gaps");

            // The next fire after the snap is 2026-03-09 02:30 LA PDT
            // = UTC 09:30. No second fire on 2026-03-08.
            let after = next_fire_after(&spec, next).unwrap();
            let next_day = Utc.with_ymd_and_hms(2026, 3, 9, 9, 30, 0).unwrap();
            assert_eq!(after, next_day, "no double-fire on DST forward day");
        }

        #[test]
        fn dst_backward_no_double_fire() {
            // US DST backward: 2026-11-01 02:00 LA → 01:00 LA.
            // 01:30 LA happens twice — once at PDT (UTC-7), once at PST
            // (UTC-8). Schedule "30 1 * * *" is a fixed-time job; croner
            // fires only at the FIRST occurrence (PDT). We assert exactly
            // one fire on this day, and the next fire is the following day.
            let spec = spec_with("30 1 * * *", Some("America/Los_Angeles"));

            // Midnight LA on 2026-11-01 (still PDT) = UTC 07:00.
            let midnight = Utc.with_ymd_and_hms(2026, 11, 1, 7, 0, 0).unwrap();
            let first = next_fire_after(&spec, midnight).unwrap();

            // 01:30 LA PDT = UTC 08:30.
            let pdt_fire = Utc.with_ymd_and_hms(2026, 11, 1, 8, 30, 0).unwrap();
            assert_eq!(first, pdt_fire, "first fire is at 01:30 LA PDT");

            // The next fire is 2026-11-02 01:30 LA PST = UTC 09:30, NOT a
            // second fire on 2026-11-01 at 01:30 PST (which is the same
            // wall-clock time but a different absolute instant).
            let second = next_fire_after(&spec, first).unwrap();
            let next_day = Utc.with_ymd_and_hms(2026, 11, 2, 9, 30, 0).unwrap();
            assert_eq!(second, next_day, "no second fire during fall-back overlap");
        }

        #[test]
        fn invalid_schedule_returns_error() {
            // Real specs go through validate() before fire. The function
            // should still surface an error rather than panic if a caller
            // hands in a bogus spec directly.
            let mut spec = spec_with("0 9 * * 1", None);
            spec.schedule = "totally bogus".to_string();
            let err = next_fire_after(&spec, Utc::now()).unwrap_err();
            assert!(matches!(err, CronSpecError::InvalidSchedule(_)));
        }
    }

    mod name_validation {
        //! Lock the wire-shape: every variant of `CronNameError` is reachable
        //! from a single inputs, and the daemon + runtime both rely on the
        //! `^[a-z0-9][a-z0-9-]{0,62}$` rule plus the reserved-word list. If
        //! any of these tests breaks, both consumers will surface a different
        //! `error_code` and the WebUI / CLI rendering will drift.
        //!
        //! `use super::*` is avoided here so the parent module's
        //! `pretty_assertions::assert_eq` re-export doesn't collide with
        //! the prelude version on every macro invocation. We import the
        //! handful of names we need explicitly.
        use super::super::{validate_cron_name, CronNameError};
        use pretty_assertions::assert_eq;

        #[test]
        fn rejects_empty() {
            assert!(matches!(
                validate_cron_name(""),
                Err(CronNameError::Empty)
            ));
        }

        #[test]
        fn rejects_too_long() {
            let n = "a".repeat(64);
            match validate_cron_name(&n) {
                Err(CronNameError::TooLong(64)) => {}
                other => panic!("expected TooLong(64), got {other:?}"),
            }
        }

        #[test]
        fn rejects_dot_prefix() {
            // `.hidden` would shadow `.gitim`/`.git` discipline.
            assert!(matches!(
                validate_cron_name(".hidden"),
                Err(CronNameError::DotPrefix(_))
            ));
        }

        #[test]
        fn rejects_reserved() {
            for n in ["archive", "crons"] {
                match validate_cron_name(n) {
                    Err(CronNameError::Reserved(s)) => assert_eq!(s, n),
                    other => panic!("expected Reserved({n}), got {other:?}"),
                }
            }
        }

        #[test]
        fn rejects_uppercase() {
            // Case sensitivity matters for fs-portability across case-
            // insensitive macOS volumes vs case-sensitive Linux.
            assert!(matches!(
                validate_cron_name("WeeklyReport"),
                Err(CronNameError::BadFirstChar(_)) | Err(CronNameError::BadChar { .. })
            ));
        }

        #[test]
        fn rejects_leading_hyphen() {
            assert!(matches!(
                validate_cron_name("-leading"),
                Err(CronNameError::BadFirstChar(_))
            ));
        }

        #[test]
        fn rejects_dotdot() {
            // The path-traversal canary. A runtime that joins the URL `<name>`
            // segment onto `crons/...` without this check would resolve `..`
            // to the parent of `crons/`, escaping the workspace's cron tree.
            assert!(matches!(
                validate_cron_name(".."),
                Err(CronNameError::DotPrefix(_))
            ));
        }

        #[test]
        fn rejects_slash() {
            assert!(matches!(
                validate_cron_name("a/b"),
                Err(CronNameError::BadChar { ch: '/', .. })
            ));
        }

        #[test]
        fn rejects_null_byte() {
            // NUL would terminate the C-style path mid-string and leak into
            // the parent directory. Don't trust the URL decoder upstream.
            assert!(matches!(
                validate_cron_name("a\0b"),
                Err(CronNameError::BadChar { ch: '\0', .. })
            ));
        }

        #[test]
        fn accepts_canonical_shapes() {
            for n in ["a", "weekly-report", "j0", "0-9", "abc-1-23"] {
                validate_cron_name(n).unwrap_or_else(|e| {
                    panic!("expected '{n}' to validate, got {e:?}")
                });
            }
        }

        #[test]
        fn boundary_63_chars_ok_64_chars_rejected() {
            let ok = "a".repeat(63);
            validate_cron_name(&ok).unwrap();
            let too_long = "a".repeat(64);
            assert!(matches!(
                validate_cron_name(&too_long),
                Err(CronNameError::TooLong(64))
            ));
        }
    }
}
