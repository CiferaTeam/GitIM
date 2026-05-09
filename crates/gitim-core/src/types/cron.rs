//! `CronSpec` — schedule + target + prompt for a cron trigger.
//!
//! Each `crons/<name>/spec.yaml` parses into one of these. Validation is
//! a separate pass (see `validate`) so callers can synthesize a spec in
//! memory and assert it passes the same checks the YAML loader applies.

use std::collections::BTreeMap;
use std::str::FromStr;

use chrono::DateTime;
use chrono_tz::Tz;
use croner::Cron;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::handler::Handler;

/// Hard cap on prompt size — 8 KiB. Prompts get committed to git on every
/// fire; an oversized prompt would spam the audit log and stress agent
/// context windows. The limit is generous enough for a paragraph-length
/// instruction with a few example links.
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
    #[error("invalid target handler: {0}")]
    InvalidTarget(String),
    #[error("prompt cannot be empty")]
    EmptyPrompt,
    #[error("prompt exceeds {} bytes (got {len})", MAX_PROMPT_BYTES)]
    OversizedPrompt { len: usize },
    #[error("invalid created_at timestamp: {0}")]
    InvalidCreatedAt(String),
    #[error("YAML parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),
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
        // but if a caller constructed a CronSpec programmatically with a string
        // that bypassed Handler::new, we'd never get here. Validation of target
        // is therefore implicit — if Handler exists, it's well-formed.
        // (We keep InvalidTarget in the error enum for the daemon-side check
        // that target exists in users/, which is not the spec's job.)

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

        Ok(())
    }

    /// Whether this spec should be considered for firing. Single point so
    /// future archive-style flags can be added without touching every caller.
    pub fn is_active(&self) -> bool {
        self.enabled
    }
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
}
