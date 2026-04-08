use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BoardMeta {
    pub name: String,
    pub display_name: String,
    pub created_by: String,
    pub created_at: String,
    #[serde(default = "default_statuses")]
    pub statuses: Vec<String>,
}

fn default_statuses() -> Vec<String> {
    vec![
        "todo".to_string(),
        "in-progress".to_string(),
        "done".to_string(),
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CardMeta {
    pub title: String,
    pub status: String,
    #[serde(default)]
    pub assignee: Option<String>,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_board_meta_default_statuses() {
        let yaml = "name: sprint-1\ndisplay_name: Sprint 1\ncreated_by: lewis\ncreated_at: '20260403T140000Z'\n";
        let meta: BoardMeta = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(meta.statuses, vec!["todo", "in-progress", "done"]);
    }

    #[test]
    fn test_board_meta_custom_statuses() {
        let yaml = "name: sprint-1\ndisplay_name: Sprint 1\ncreated_by: lewis\ncreated_at: '20260403T140000Z'\nstatuses:\n  - backlog\n  - doing\n  - review\n  - done\n";
        let meta: BoardMeta = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(meta.statuses.len(), 4);
        assert_eq!(meta.statuses[0], "backlog");
    }

    #[test]
    fn test_card_meta_roundtrip() {
        let card = CardMeta {
            title: "Test card".to_string(),
            status: "todo".to_string(),
            assignee: Some("lewis".to_string()),
            created_by: "lewis".to_string(),
            created_at: "20260403T140000Z".to_string(),
            updated_at: "20260403T140000Z".to_string(),
        };
        let yaml = serde_yaml::to_string(&card).unwrap();
        let parsed: CardMeta = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(card, parsed);
    }

    #[test]
    fn test_card_meta_no_assignee() {
        let yaml = "title: Test\nstatus: todo\ncreated_by: lewis\ncreated_at: '20260403T140000Z'\nupdated_at: '20260403T140000Z'\n";
        let meta: CardMeta = serde_yaml::from_str(yaml).unwrap();
        assert!(meta.assignee.is_none());
    }
}
