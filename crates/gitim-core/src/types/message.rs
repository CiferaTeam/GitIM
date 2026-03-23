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

/// 频道事件（join/leave 等系统事件）
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelEvent {
    pub line_number: u64,
    pub point_to: u64,
    pub author: Handler,
    pub timestamp: String,
    pub event_type: String,
    pub meta: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ThreadEntry {
    Message(Message),
    Event(ChannelEvent),
}

impl ThreadEntry {
    pub fn line_number(&self) -> u64 {
        match self {
            ThreadEntry::Message(m) => m.line_number,
            ThreadEntry::Event(e) => e.line_number,
        }
    }
    pub fn author(&self) -> &Handler {
        match self {
            ThreadEntry::Message(m) => &m.author,
            ThreadEntry::Event(e) => &e.author,
        }
    }
    pub fn timestamp(&self) -> &str {
        match self {
            ThreadEntry::Message(m) => &m.timestamp,
            ThreadEntry::Event(e) => &e.timestamp,
        }
    }
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
    pub entries: Vec<ThreadEntry>,
}

impl ThreadFile {
    pub fn messages(&self) -> Vec<&Message> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                ThreadEntry::Message(m) => Some(m),
                _ => None,
            })
            .collect()
    }

    pub fn events(&self) -> Vec<&ChannelEvent> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                ThreadEntry::Event(ev) => Some(ev),
                _ => None,
            })
            .collect()
    }

    pub fn last_line_number(&self) -> u64 {
        self.entries.last().map(|e| e.line_number()).unwrap_or(0)
    }
}
