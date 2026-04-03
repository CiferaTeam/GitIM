use gitim_core::formatter::format_message;
use gitim_core::parser::parse_thread;
use gitim_core::types::{Handler, ThreadEntry};

/// 向 thread 文件追加一条消息，返回 (行号, 新内容)
pub fn append_message_to_thread(
    thread_path: &std::path::Path,
    author: &Handler,
    body: &str,
    reply_to: Option<u64>,
) -> Result<(u64, String), String> {
    let existing = std::fs::read_to_string(thread_path).unwrap_or_default();
    let existing_file =
        parse_thread(&existing).map_err(|e| format!("failed to parse thread: {}", e))?;

    let next_line = existing_file.last_line_number() + 1;
    let point_to = reply_to.unwrap_or(0);

    let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let new_content = format_message(next_line, point_to, author, &now, body);

    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(thread_path)
        .map_err(|e| format!("open failed: {}", e))?;
    file.write_all(new_content.as_bytes())
        .map_err(|e| format!("write failed: {}", e))?;

    Ok((next_line, new_content))
}

/// 读取 thread 文件并返回 JSON entries
pub fn read_thread_entries(
    thread_path: &std::path::Path,
    limit: Option<usize>,
    since: Option<u64>,
) -> Result<Vec<serde_json::Value>, String> {
    let content = std::fs::read_to_string(thread_path).unwrap_or_default();
    let file = parse_thread(&content).map_err(|e| format!("parse error: {}", e))?;

    let mut entries: Vec<&ThreadEntry> = file.entries.iter().collect();

    if let Some(since_line) = since {
        entries.retain(|e| e.line_number() > since_line);
    }

    if let Some(lim) = limit {
        let start = entries.len().saturating_sub(lim);
        entries = entries[start..].to_vec();
    }

    Ok(entries.iter().map(|entry| entry_to_json(entry)).collect())
}

fn entry_to_json(entry: &ThreadEntry) -> serde_json::Value {
    match entry {
        ThreadEntry::Message(m) => {
            use gitim_core::link::extract_links;
            use gitim_core::types::LinkKind;
            let links: Vec<serde_json::Value> = extract_links(&m.body)
                .iter()
                .map(|link| match &link.kind {
                    LinkKind::Channel { name } => serde_json::json!({
                        "kind": "channel", "name": name, "raw": link.raw,
                    }),
                    LinkKind::Message {
                        channel,
                        line_number,
                    } => serde_json::json!({
                        "kind": "message", "channel": channel,
                        "line_number": line_number, "raw": link.raw,
                    }),
                    LinkKind::UserProfile { handler } => serde_json::json!({
                        "kind": "user_profile", "handler": handler.as_str(), "raw": link.raw,
                    }),
                    LinkKind::Softlink { url, title } => {
                        let mut v = serde_json::json!({
                            "kind": "softlink", "url": url, "raw": link.raw,
                        });
                        if let Some(t) = title {
                            v["title"] = serde_json::json!(t);
                        }
                        v
                    }
                })
                .collect();
            serde_json::json!({
                "type": "message",
                "line_number": m.line_number,
                "point_to": m.point_to,
                "author": m.author.as_str(),
                "timestamp": m.timestamp,
                "body": m.body,
                "mentions": m.mentions.iter().map(|h| h.as_str()).collect::<Vec<_>>(),
                "links": links,
            })
        }
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
