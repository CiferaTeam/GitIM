use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::labels::{validate_labels, LabelError, USER_MAX_LABELS};

/// Hard ceiling on a user-supplied introduction blurb. The field is
/// human-display only (not fed to the LLM) so the limit is about UI density —
/// a single long-tweet-sized line that fits in the agent card without
/// truncation. Enforced at the daemon RPC boundary so every writer (CLI,
/// runtime, future clients) gets the same answer.
pub const MAX_INTRODUCTION_LEN: usize = 256;

#[derive(Error, Debug)]
pub enum UserMetaError {
    #[error("introduction too long ({0} > {1} bytes)")]
    IntroductionTooLong(usize, usize),
    #[error(transparent)]
    Label(#[from] LabelError),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserMeta {
    pub display_name: String,
    pub role: String,
    pub introduction: String,
    /// Capability labels claimed by this user/agent. Source of truth for
    /// `agents_with_labels` queries and `create_card` assignee suggestions.
    /// See `docs/plans/unified-labels/00-requirements.md` (P3, P4).
    #[serde(default)]
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelMeta {
    pub display_name: String,
    pub created_by: String,
    pub created_at: String,
    pub introduction: String,
    #[serde(default)]
    pub members: Vec<String>,
    /// 所属 project slug。None = 不在任何 project 下。
    /// 旧 channel meta 缺省 → None,backward-compat (review finding 3.B)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

#[cfg(test)]
mod channel_meta_project_tests {
    use super::*;

    #[test]
    fn old_yaml_without_project_field_parses_as_none() {
        // 老 channel.meta.yaml 无 project 字段
        let yaml = r#"
display_name: General
created_by: lewisliu
created_at: "2026-04-01T10:00:00Z"
introduction: General chat
members:
  - lewisliu
"#;
        let meta: ChannelMeta = serde_yaml::from_str(yaml).expect("parse");
        assert_eq!(meta.project, None);
    }

    #[test]
    fn none_project_skips_serialization() {
        let meta = ChannelMeta {
            display_name: "g".into(),
            created_by: "u".into(),
            created_at: "2026-04-01T10:00:00Z".into(),
            introduction: "x".into(),
            members: vec!["u".into()],
            project: None,
        };
        let yaml = serde_yaml::to_string(&meta).expect("ser");
        assert!(
            !yaml.contains("project"),
            "project field should be skipped when None; got:\n{yaml}"
        );
    }

    #[test]
    fn some_project_roundtrips() {
        let meta = ChannelMeta {
            display_name: "g".into(),
            created_by: "u".into(),
            created_at: "2026-04-01T10:00:00Z".into(),
            introduction: "x".into(),
            members: vec!["u".into()],
            project: Some("design".into()),
        };
        let yaml = serde_yaml::to_string(&meta).expect("ser");
        assert!(yaml.contains("project: design"));
        let back: ChannelMeta = serde_yaml::from_str(&yaml).expect("de");
        assert_eq!(meta, back);
    }

    #[test]
    fn new_yaml_with_extra_unknown_field_still_parses() {
        // 老 daemon 读新 yaml 的反向场景:新加字段不破 parse
        // (serde 默认 deny_unknown_fields = false)
        let yaml = r#"
display_name: g
created_by: u
created_at: "2026-04-01T10:00:00Z"
introduction: x
members:
  - u
project: design
future_field: foo
"#;
        let meta: ChannelMeta = serde_yaml::from_str(yaml).expect("parse");
        assert_eq!(meta.project, Some("design".to_string()));
    }
}

/// Validate a `UserMeta` against schema rules: introduction length + label set.
pub fn validate_user_meta(meta: &UserMeta) -> Result<(), UserMetaError> {
    if meta.introduction.len() > MAX_INTRODUCTION_LEN {
        return Err(UserMetaError::IntroductionTooLong(
            meta.introduction.len(),
            MAX_INTRODUCTION_LEN,
        ));
    }
    validate_labels(&meta.labels, USER_MAX_LABELS)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_yaml_without_labels_deserializes_as_empty() {
        let yaml = "display_name: Alice\nrole: backend\nintroduction: hello\n";
        let meta: UserMeta = serde_yaml::from_str(yaml).unwrap();
        assert!(meta.labels.is_empty());
        assert_eq!(meta.display_name, "Alice");
        assert_eq!(meta.role, "backend");
        assert_eq!(meta.introduction, "hello");
    }

    #[test]
    fn new_yaml_with_labels_roundtrip() {
        let meta = UserMeta {
            display_name: "Alice".into(),
            role: "backend".into(),
            introduction: "hello".into(),
            labels: vec!["rust".into(), "backend".into()],
        };
        let yaml = serde_yaml::to_string(&meta).unwrap();
        let parsed: UserMeta = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(meta, parsed);
    }

    #[test]
    fn add_labels_preserves_other_fields() {
        let yaml_before = "display_name: Alice\nrole: backend\nintroduction: hello\n";
        let mut meta: UserMeta = serde_yaml::from_str(yaml_before).unwrap();
        meta.labels.push("rust".into());
        let yaml_after = serde_yaml::to_string(&meta).unwrap();
        assert!(yaml_after.contains("display_name: Alice"));
        assert!(yaml_after.contains("role: backend"));
        assert!(yaml_after.contains("introduction: hello"));
        assert!(yaml_after.contains("- rust"));
    }

    #[test]
    fn validate_user_meta_accepts_empty_labels() {
        let meta = UserMeta {
            display_name: "A".into(),
            role: "r".into(),
            introduction: String::new(),
            labels: vec![],
        };
        assert!(validate_user_meta(&meta).is_ok());
    }

    #[test]
    fn validate_user_meta_accepts_valid_labels() {
        let meta = UserMeta {
            display_name: "A".into(),
            role: "r".into(),
            introduction: String::new(),
            labels: vec!["rust".into(), "backend".into(), "mobile_ios".into()],
        };
        assert!(validate_user_meta(&meta).is_ok());
    }

    #[test]
    fn validate_user_meta_rejects_too_many_labels() {
        let labels: Vec<String> = (0..21).map(|i| format!("l{}", i)).collect();
        let meta = UserMeta {
            display_name: "A".into(),
            role: "r".into(),
            introduction: String::new(),
            labels,
        };
        let err = validate_user_meta(&meta).unwrap_err();
        assert!(matches!(
            err,
            UserMetaError::Label(LabelError::TooMany(21, 20))
        ));
    }

    #[test]
    fn validate_user_meta_rejects_invalid_label_char() {
        let meta = UserMeta {
            display_name: "A".into(),
            role: "r".into(),
            introduction: String::new(),
            labels: vec!["Rust!".into()],
        };
        let err = validate_user_meta(&meta).unwrap_err();
        assert!(matches!(
            err,
            UserMetaError::Label(LabelError::InvalidChar('R'))
        ));
    }

    #[test]
    fn validate_user_meta_rejects_too_long_introduction() {
        let meta = UserMeta {
            display_name: "A".into(),
            role: "r".into(),
            introduction: "x".repeat(MAX_INTRODUCTION_LEN + 1),
            labels: vec![],
        };
        let err = validate_user_meta(&meta).unwrap_err();
        assert!(matches!(err, UserMetaError::IntroductionTooLong(_, _)));
    }
}
