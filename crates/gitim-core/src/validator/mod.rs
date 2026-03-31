use thiserror::Error;
use crate::types::meta::{UserMeta, ChannelMeta};
use crate::types::config::Config;

pub mod compliance;
pub mod im_rules;
pub mod read_check;

#[derive(Error, Debug)]
pub enum ValidationError {
    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),
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
    let ts_re = regex::Regex::new(r"^\d{8}T\d{6}Z$").unwrap();
    if !ts_re.is_match(&meta.created_at) {
        return Err(ValidationError::FieldConstraint {
            field: "created_at".into(),
            reason: "must match YYYYMMDDTHHmmssZ format".into(),
        });
    }
    Ok(meta)
}

pub fn validate_channel_name(name: &str) -> Result<(), ValidationError> {
    if name.is_empty() || name.len() > 32 {
        return Err(ValidationError::InvalidChannelName("must be 1-32 characters".into()));
    }
    if !name.chars().all(|c| matches!(c, 'a'..='z' | '0'..='9' | '-')) {
        return Err(ValidationError::InvalidChannelName("invalid characters".into()));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(ValidationError::InvalidChannelName("must not start or end with hyphen".into()));
    }
    if name.contains("--") {
        return Err(ValidationError::InvalidChannelName("must not contain consecutive hyphens".into()));
    }
    Ok(())
}

pub fn validate_config(yaml: &str) -> Result<Config, ValidationError> {
    let config: Config = serde_yaml::from_str(yaml)?;
    if config.version != 1 {
        return Err(ValidationError::InvalidConfig(
            format!("unsupported version: {}, expected 1", config.version),
        ));
    }
    Ok(config)
}
