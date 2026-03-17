use crate::types::handler::Handler;

/// A parsed message from a .thread file.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub line_number: u64,
    pub point_to: u64,
    pub author: Handler,
    pub timestamp: String,
    pub body: String,
    pub mentions: Vec<Handler>,
}

/// A line in a .thread file — either a message start or a continuation.
#[derive(Debug, Clone, PartialEq)]
pub enum ThreadLine {
    MessageStart(Message),
    Continuation(String),
}

/// Result of parsing a .thread file.
#[derive(Debug, Clone)]
pub struct ThreadFile {
    pub messages: Vec<Message>,
}
