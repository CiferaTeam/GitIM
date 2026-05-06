use gitim_core::types::{Link, LinkKind, ThreadEntry};

pub(crate) fn link_to_json(link: &Link) -> serde_json::Value {
    match &link.kind {
        LinkKind::Channel { name } => serde_json::json!({
            "kind": "channel",
            "name": name,
            "raw": link.raw,
        }),
        LinkKind::Message {
            channel,
            line_number,
        } => serde_json::json!({
            "kind": "message",
            "channel": channel,
            "line_number": line_number,
            "raw": link.raw,
        }),
        LinkKind::UserProfile { handler } => serde_json::json!({
            "kind": "user_profile",
            "handler": handler.as_str(),
            "raw": link.raw,
        }),
        LinkKind::Softlink { url, title } => {
            let mut v = serde_json::json!({
                "kind": "softlink",
                "url": url,
                "raw": link.raw,
            });
            if let Some(t) = title {
                v["title"] = serde_json::json!(t);
            }
            v
        }
    }
}

pub(crate) fn entry_to_json(entry: &ThreadEntry) -> serde_json::Value {
    match entry {
        ThreadEntry::Message(m) => serde_json::json!({
            "type": "message",
            "line_number": m.line_number,
            "point_to": m.point_to,
            "author": m.author.as_str(),
            "timestamp": m.timestamp,
            "body": m.body,
            "mentions": m.mentions.iter().map(|h| h.as_str()).collect::<Vec<_>>(),
            "links": m.links.iter().map(link_to_json).collect::<Vec<_>>(),
        }),
        ThreadEntry::Event(ev) => serde_json::json!({
            "type": "event",
            "event_type": ev.event_type,
            "line_number": ev.line_number,
            "author": ev.author.as_str(),
            "timestamp": ev.timestamp,
            "meta": ev.meta,
        }),
    }
}
