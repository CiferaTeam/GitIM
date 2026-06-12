use crate::types::config::Config;
use crate::types::meta::{ChannelMeta, UserMeta};
use crate::types::ProjectMeta;
use thiserror::Error;

pub mod compliance;
pub mod im_rules;
pub mod read_check;

#[derive(Error, Debug)]
pub enum ValidationError {
    #[error("YAML parse error: {0}")]
    YamlParse(#[from] serde_yaml::Error),
    #[error("field '{field}' {reason}")]
    FieldConstraint { field: String, reason: String },
    #[error("invalid channel name: {0}")]
    InvalidChannelName(String),
    #[error("invalid config: {0}")]
    InvalidConfig(String),
}

pub fn validate_user_meta(yaml: &str) -> Result<UserMeta, ValidationError> {
    let meta: UserMeta = serde_yaml::from_str(yaml)?;
    if meta.display_name.is_empty() || meta.display_name.len() > 64 {
        return Err(ValidationError::FieldConstraint {
            field: "display_name".into(),
            reason: "must be 1-64 characters".into(),
        });
    }
    if meta.role.is_empty() || meta.role.len() > 32 {
        return Err(ValidationError::FieldConstraint {
            field: "role".into(),
            reason: "must be 1-32 characters".into(),
        });
    }
    if meta.introduction.is_empty() || meta.introduction.len() > 500 {
        return Err(ValidationError::FieldConstraint {
            field: "introduction".into(),
            reason: "must be 1-500 characters".into(),
        });
    }
    Ok(meta)
}

pub fn validate_channel_meta(yaml: &str) -> Result<ChannelMeta, ValidationError> {
    let meta: ChannelMeta = serde_yaml::from_str(yaml)?;
    if meta.display_name.is_empty() || meta.display_name.len() > 64 {
        return Err(ValidationError::FieldConstraint {
            field: "display_name".into(),
            reason: "must be 1-64 characters".into(),
        });
    }
    if meta.introduction.is_empty() || meta.introduction.len() > 500 {
        return Err(ValidationError::FieldConstraint {
            field: "introduction".into(),
            reason: "must be 1-500 characters".into(),
        });
    }
    use crate::types::Handler;
    Handler::new(&meta.created_by).map_err(|_| ValidationError::FieldConstraint {
        field: "created_by".into(),
        reason: "must be a valid handler".into(),
    })?;
    let ts_re = crate::preconditions::regex_literal(r"^\d{8}T\d{6}Z$");
    if !ts_re.is_match(&meta.created_at) {
        return Err(ValidationError::FieldConstraint {
            field: "created_at".into(),
            reason: "must match YYYYMMDDTHHmmssZ format".into(),
        });
    }
    Ok(meta)
}

pub fn validate_project_meta(yaml: &str) -> Result<ProjectMeta, ValidationError> {
    let meta: ProjectMeta = serde_yaml::from_str(yaml)?;
    if meta.display_name.is_empty() || meta.display_name.len() > 64 {
        return Err(ValidationError::FieldConstraint {
            field: "display_name".into(),
            reason: "must be 1-64 characters".into(),
        });
    }
    if meta.introduction.is_empty() || meta.introduction.len() > 500 {
        return Err(ValidationError::FieldConstraint {
            field: "introduction".into(),
            reason: "must be 1-500 characters".into(),
        });
    }
    use crate::types::Handler;
    Handler::new(&meta.created_by).map_err(|_| ValidationError::FieldConstraint {
        field: "created_by".into(),
        reason: "must be a valid handler".into(),
    })?;
    Ok(meta)
}

pub fn validate_channel_name(name: &str) -> Result<(), ValidationError> {
    if name.is_empty() || name.len() > 32 {
        return Err(ValidationError::InvalidChannelName(
            "must be 1-32 characters".into(),
        ));
    }
    if !name
        .chars()
        .all(|c| matches!(c, 'a'..='z' | '0'..='9' | '-'))
    {
        return Err(ValidationError::InvalidChannelName(
            "invalid characters".into(),
        ));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(ValidationError::InvalidChannelName(
            "must not start or end with hyphen".into(),
        ));
    }
    if name.contains("--") {
        return Err(ValidationError::InvalidChannelName(
            "must not contain consecutive hyphens".into(),
        ));
    }
    Ok(())
}

pub fn validate_config(yaml: &str) -> Result<Config, ValidationError> {
    let config: Config = serde_yaml::from_str(yaml)?;
    if config.version != 1 {
        return Err(ValidationError::InvalidConfig(format!(
            "unsupported version: {}, expected 1",
            config.version
        )));
    }
    Ok(config)
}

#[cfg(test)]
mod project_meta_validator_tests {
    use super::*;

    #[test]
    fn valid_yaml() {
        let yaml = r#"
display_name: Design Sprint
created_by: lewisliu
created_at: "2026-05-21T08:00:00Z"
introduction: All UX work for v2
"#;
        let meta = validate_project_meta(yaml).expect("ok");
        assert_eq!(meta.display_name, "Design Sprint");
    }

    #[test]
    fn empty_display_name_rejected() {
        let yaml = r#"
display_name: ""
created_by: lewisliu
created_at: "2026-05-21T08:00:00Z"
introduction: hi
"#;
        assert!(validate_project_meta(yaml).is_err());
    }

    #[test]
    fn too_long_introduction_rejected() {
        let intro = "x".repeat(501);
        let yaml = format!(
            "display_name: a\ncreated_by: lewisliu\ncreated_at: \"2026-05-21T08:00:00Z\"\nintroduction: {}\n",
            intro
        );
        assert!(validate_project_meta(&yaml).is_err());
    }
}
