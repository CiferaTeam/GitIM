use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::channel::ChannelName;
use super::handler::Handler;
use super::labels::{validate_labels, LabelError, CARD_MAX_LABELS};

#[derive(Error, Debug)]
pub enum CardError {
    #[error("invalid status '{0}', allowed: todo/doing/done")]
    InvalidStatus(String),
    #[error("card_id length out of range (1..={1}), got {0}")]
    CardIdLengthOutOfRange(usize, usize),
    #[error("invalid character in card_id: '{0}'")]
    InvalidCardIdChar(char),
    #[error("title cannot be empty")]
    EmptyTitle,
    #[error("invalid channel name: {0}")]
    InvalidChannel(String),
    #[error("invalid handler: {0}")]
    InvalidHandler(String),
    #[error("invalid timestamp '{0}'")]
    InvalidTimestamp(String),
    #[error(transparent)]
    Label(#[from] LabelError),
}

#[derive(Error, Debug)]
pub enum CardMetaYamlError {
    #[error("YAML parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error(transparent)]
    Card(#[from] CardError),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CardStatus {
    Todo,
    Doing,
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ArchivedVia {
    Channel,
    Manual,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_via: Option<ArchivedVia>,
}

pub(crate) const MAX_CARD_ID_LEN: usize = 20;

pub fn validate_card_id(card_id: &str) -> Result<(), CardError> {
    if card_id.is_empty() || card_id.len() > MAX_CARD_ID_LEN {
        return Err(CardError::CardIdLengthOutOfRange(
            card_id.len(),
            MAX_CARD_ID_LEN,
        ));
    }
    for ch in card_id.chars() {
        if !matches!(ch, '0'..='9' | 'a'..='f' | '-') {
            return Err(CardError::InvalidCardIdChar(ch));
        }
    }
    Ok(())
}

fn validate_timestamp(timestamp: &str) -> Result<(), CardError> {
    let bytes = timestamp.as_bytes();
    let valid = bytes.len() == 16
        && bytes[8] == b'T'
        && bytes[15] == b'Z'
        && bytes[..8].iter().all(u8::is_ascii_digit)
        && bytes[9..15].iter().all(u8::is_ascii_digit);
    if valid {
        Ok(())
    } else {
        Err(CardError::InvalidTimestamp(timestamp.to_string()))
    }
}

pub fn validate_card_meta(meta: &CardMeta) -> Result<(), CardError> {
    if meta.title.trim().is_empty() {
        return Err(CardError::EmptyTitle);
    }
    ChannelName::new(&meta.channel).map_err(|e| CardError::InvalidChannel(e.to_string()))?;
    Handler::new(&meta.created_by).map_err(|e| CardError::InvalidHandler(e.to_string()))?;
    if let Some(assignee) = &meta.assignee {
        Handler::new(assignee).map_err(|e| CardError::InvalidHandler(e.to_string()))?;
    }
    validate_labels(&meta.labels, CARD_MAX_LABELS)?;
    validate_timestamp(&meta.created_at)?;
    validate_timestamp(&meta.updated_at)?;
    Ok(())
}

pub fn parse_card_meta_yaml(yaml: &str) -> Result<CardMeta, CardMetaYamlError> {
    let meta: CardMeta = serde_yaml::from_str(yaml)?;
    validate_card_meta(&meta)?;
    Ok(meta)
}

pub fn stringify_card_meta_yaml(meta: &CardMeta) -> Result<String, CardMetaYamlError> {
    validate_card_meta(meta)?;
    Ok(serde_yaml::to_string(meta)?)
}

#[cfg(test)]
mod archived_via_tests {
    use super::*;

    #[test]
    fn archived_via_serializes_lowercase() {
        let yaml = serde_yaml::to_string(&ArchivedVia::Channel).unwrap();
        assert_eq!(yaml.trim(), "channel");
        let yaml = serde_yaml::to_string(&ArchivedVia::Manual).unwrap();
        assert_eq!(yaml.trim(), "manual");
    }

    #[test]
    fn card_meta_omits_archived_via_when_none() {
        let meta = CardMeta {
            title: "t".into(),
            channel: "c".into(),
            status: CardStatus::Todo,
            labels: vec![],
            assignee: None,
            created_by: "u".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            archived_via: None,
        };
        let yaml = serde_yaml::to_string(&meta).unwrap();
        assert!(
            !yaml.contains("archived_via"),
            "expected omitted field, got:\n{yaml}"
        );
    }

    #[test]
    fn card_meta_writes_archived_via_when_some() {
        let meta = CardMeta {
            title: "t".into(),
            channel: "c".into(),
            status: CardStatus::Todo,
            labels: vec![],
            assignee: None,
            created_by: "u".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            archived_via: Some(ArchivedVia::Channel),
        };
        let yaml = serde_yaml::to_string(&meta).unwrap();
        assert!(
            yaml.contains("archived_via: channel"),
            "expected field present, got:\n{yaml}"
        );
    }

    #[test]
    fn card_meta_reads_legacy_yaml_without_field() {
        let yaml = "title: t\nchannel: c\nstatus: todo\nlabels: []\nassignee: null\ncreated_by: u\ncreated_at: '2026-01-01T00:00:00Z'\nupdated_at: '2026-01-01T00:00:00Z'\n";
        let meta: CardMeta = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(meta.archived_via, None);
    }

    #[test]
    fn card_meta_reads_archived_via_channel() {
        let yaml = "title: t\nchannel: c\nstatus: todo\nlabels: []\nassignee: null\ncreated_by: u\ncreated_at: '2026-01-01T00:00:00Z'\nupdated_at: '2026-01-01T00:00:00Z'\narchived_via: channel\n";
        let meta: CardMeta = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(meta.archived_via, Some(ArchivedVia::Channel));
    }
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
            archived_via: None,
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
    fn validate_card_meta_rejects_too_many_labels() {
        // Use validate_card_meta to confirm CARD_MAX_LABELS is wired through
        let meta = CardMeta {
            title: "t".into(),
            channel: "c".into(),
            status: CardStatus::Todo,
            labels: (0..11).map(|i| format!("l{}", i)).collect(),
            assignee: None,
            created_by: "u".into(),
            created_at: "20260101T000000Z".into(),
            updated_at: "20260101T000000Z".into(),
            archived_via: None,
        };
        assert!(matches!(
            validate_card_meta(&meta),
            Err(CardError::Label(_))
        ));
    }

    #[test]
    fn validate_card_id_matches_runtime_shape() {
        assert!(validate_card_id("20260317-120000-abc").is_ok());
        assert!(validate_card_id("card-1").is_err());
        assert!(validate_card_id("").is_err());
    }

    #[test]
    fn parse_card_meta_yaml_validates_protocol_fields() {
        let yaml = "title: Browser card\nchannel: general\nstatus: todo\nlabels:\n  - mobile\nassignee: lewis\ncreated_by: lewis\ncreated_at: 20260317T120000Z\nupdated_at: 20260317T120000Z\n";
        let meta = parse_card_meta_yaml(yaml).unwrap();
        assert_eq!(meta.title, "Browser card");

        let invalid = "title: Browser card\nchannel: General\nstatus: todo\ncreated_by: lewis\ncreated_at: 20260317T120000Z\nupdated_at: 20260317T120000Z\n";
        assert!(parse_card_meta_yaml(invalid).is_err());
    }

    #[test]
    fn stringify_card_meta_yaml_validates_before_serializing() {
        let meta = CardMeta {
            title: "".to_string(),
            channel: "general".to_string(),
            status: CardStatus::Todo,
            labels: vec![],
            assignee: None,
            created_by: "lewis".to_string(),
            created_at: "20260317T120000Z".to_string(),
            updated_at: "20260317T120000Z".to_string(),
            archived_via: None,
        };
        assert!(stringify_card_meta_yaml(&meta).is_err());
    }
}
