use serde::{Deserialize, Serialize};

/// Hard ceiling on a user-supplied introduction blurb. The field is
/// human-display only (not fed to the LLM) so the limit is about UI density —
/// a single long-tweet-sized line that fits in the agent card without
/// truncation. Enforced at the daemon RPC boundary so every writer (CLI,
/// runtime, future clients) gets the same answer.
pub const MAX_INTRODUCTION_LEN: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserMeta {
    pub display_name: String,
    pub role: String,
    pub introduction: String,
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
