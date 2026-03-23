use crate::types::handler::Handler;

/// A link extracted from a message body.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub kind: LinkKind,
    pub raw: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LinkKind {
    Channel { name: String },
    Message { channel: String, line_number: u64 },
    UserProfile { handler: Handler },
    Softlink { url: String, title: Option<String> },
}
