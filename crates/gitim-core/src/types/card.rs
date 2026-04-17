use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum CardError {
    #[error("invalid status '{0}', allowed: todo/doing/done")]
    InvalidStatus(String),
    #[error("label length out of range (1..={1}), got {0}")]
    LabelLengthOutOfRange(usize, usize),
    #[error("invalid char '{0}' in label (allowed: a-z 0-9 - _)")]
    InvalidLabelChar(char),
    #[error("too many labels (max {1}), got {0}")]
    TooManyLabels(usize, usize),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CardStatus {
    Todo,
    Doing,
    Done,
}

impl CardStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            CardStatus::Todo => "todo",
            CardStatus::Doing => "doing",
            CardStatus::Done => "done",
        }
    }

    pub fn parse(s: &str) -> Result<Self, CardError> {
        match s {
            "todo" => Ok(CardStatus::Todo),
            "doing" => Ok(CardStatus::Doing),
            "done" => Ok(CardStatus::Done),
            other => Err(CardError::InvalidStatus(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CardMeta {
    pub title: String,
    pub channel: String,
    pub status: CardStatus,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub assignee: Option<String>,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
}

pub const MAX_LABELS: usize = 10;
pub const MAX_LABEL_LEN: usize = 32;

pub fn validate_label(label: &str) -> Result<(), CardError> {
    if label.is_empty() || label.len() > MAX_LABEL_LEN {
        return Err(CardError::LabelLengthOutOfRange(label.len(), MAX_LABEL_LEN));
    }
    for ch in label.chars() {
        if !matches!(ch, 'a'..='z' | '0'..='9' | '-' | '_') {
            return Err(CardError::InvalidLabelChar(ch));
        }
    }
    Ok(())
}

pub fn validate_labels(labels: &[String]) -> Result<(), CardError> {
    if labels.len() > MAX_LABELS {
        return Err(CardError::TooManyLabels(labels.len(), MAX_LABELS));
    }
    for l in labels {
        validate_label(l)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_parse_roundtrip() {
        assert_eq!(CardStatus::parse("todo").unwrap(), CardStatus::Todo);
        assert_eq!(CardStatus::parse("doing").unwrap(), CardStatus::Doing);
        assert_eq!(CardStatus::parse("done").unwrap(), CardStatus::Done);
        assert!(CardStatus::parse("backlog").is_err());
    }

    #[test]
    fn card_meta_yaml_roundtrip() {
        let meta = CardMeta {
            title: "Refactor cards".to_string(),
            channel: "backend".to_string(),
            status: CardStatus::Doing,
            labels: vec!["v2".to_string(), "agent-task".to_string()],
            assignee: Some("claude".to_string()),
            created_by: "lewis".to_string(),
            created_at: "20260417T120000Z".to_string(),
            updated_at: "20260417T120000Z".to_string(),
        };
        let yaml = serde_yaml::to_string(&meta).unwrap();
        let parsed: CardMeta = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(meta, parsed);
        assert!(yaml.contains("status: doing"));
    }

    #[test]
    fn card_meta_no_labels_no_assignee() {
        let yaml = "title: T\nchannel: c\nstatus: todo\ncreated_by: a\ncreated_at: '20260417T120000Z'\nupdated_at: '20260417T120000Z'\n";
        let parsed: CardMeta = serde_yaml::from_str(yaml).unwrap();
        assert!(parsed.labels.is_empty());
        assert!(parsed.assignee.is_none());
    }

    #[test]
    fn validate_label_ok() {
        assert!(validate_label("v2").is_ok());
        assert!(validate_label("agent-task").is_ok());
        assert!(validate_label("sprint_2").is_ok());
    }

    #[test]
    fn validate_label_rejects_uppercase() {
        assert!(validate_label("V2").is_err());
    }

    #[test]
    fn validate_label_rejects_too_long() {
        let too_long = "a".repeat(33);
        assert!(validate_label(&too_long).is_err());
    }

    #[test]
    fn validate_labels_rejects_too_many() {
        let many: Vec<String> = (0..11).map(|i| format!("l{}", i)).collect();
        assert!(validate_labels(&many).is_err());
    }
}
