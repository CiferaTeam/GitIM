use crate::types::asset::AssetRef;
use crate::types::handler::Handler;
use serde::{Deserialize, Serialize};

/// A link extracted from a message body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Link {
    pub kind: LinkKind,
    pub raw: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LinkKind {
    Channel {
        name: String,
    },
    Message {
        channel: String,
        line_number: u64,
    },
    Card {
        channel: String,
        card_id: String,
        line_number: Option<u64>,
        label: Option<String>,
    },
    QuickSession {
        session_id: String,
        line_number: Option<u64>,
    },
    UserProfile {
        handler: Handler,
    },
    Softlink {
        url: String,
        title: Option<String>,
    },
    Asset {
        asset: AssetRef,
    },
}
