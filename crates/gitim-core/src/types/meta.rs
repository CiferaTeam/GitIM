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
}
