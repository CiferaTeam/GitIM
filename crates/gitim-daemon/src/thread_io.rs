use gitim_core::formatter::format_message;
use gitim_core::parser::parse_thread;
use gitim_core::types::{Handler, ThreadEntry};

use crate::handlers::serde::entry_to_json;

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

    // Three calling modes (see docs/plans/2026-05-11-channel-history-pagination/):
    //   limit only           → tail-cut, last N entries (channel open default)
    //   since only           → all entries after since (no truncation)
    //   since + limit        → head-cut, first N entries after since
    //                          (covers both incremental poll and history paging)
    if let Some(lim) = limit {
        if since.is_some() {
            entries.truncate(lim);
        } else {
            let drop_count = entries.len().saturating_sub(lim);
            entries.drain(..drop_count);
        }
    }

    Ok(entries.iter().map(|entry| entry_to_json(entry)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gitim_core::formatter::format_message;
    use gitim_core::types::Handler;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// Write a thread file with `count` simple messages at line_numbers 1..=count.
    fn make_thread_file(count: u64) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        let author = Handler::new("alice").unwrap();
        for i in 1..=count {
            let content =
                format_message(i, 0, &author, "20260511T120000Z", &format!("msg {}", i));
            f.write_all(content.as_bytes()).unwrap();
        }
        f
    }

    fn line_numbers(entries: &[serde_json::Value]) -> Vec<u64> {
        entries
            .iter()
            .map(|e| e["line_number"].as_u64().unwrap())
            .collect()
    }

    #[test]
    fn read_limit_only_returns_last_n() {
        let f = make_thread_file(100);
        let entries = read_thread_entries(f.path(), Some(50), None).unwrap();
        assert_eq!(line_numbers(&entries), (51..=100).collect::<Vec<_>>());
    }

    #[test]
    fn read_since_only_returns_all_after() {
        let f = make_thread_file(100);
        let entries = read_thread_entries(f.path(), None, Some(80)).unwrap();
        assert_eq!(line_numbers(&entries), (81..=100).collect::<Vec<_>>());
    }

    #[test]
    fn read_since_with_limit_paging_back() {
        // Translates "translate older messages": oldest in screen = 951,
        // caller passes since = oldest - limit - 1 = 900 to fetch [901..=950].
        let f = make_thread_file(1000);
        let entries = read_thread_entries(f.path(), Some(50), Some(900)).unwrap();
        assert_eq!(line_numbers(&entries), (901..=950).collect::<Vec<_>>());
    }

    #[test]
    fn read_since_with_limit_incremental_poll() {
        // Incremental: caller knows latest seen line = 50, asks for next 30.
        let f = make_thread_file(100);
        let entries = read_thread_entries(f.path(), Some(30), Some(50)).unwrap();
        assert_eq!(line_numbers(&entries), (51..=80).collect::<Vec<_>>());
    }

    #[test]
    fn read_since_beyond_max_returns_empty() {
        let f = make_thread_file(50);
        let entries = read_thread_entries(f.path(), Some(50), Some(100)).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn read_since_zero_with_limit_takes_first_n() {
        // since=0 retains all (line>0 is every entry); head-truncate yields the first N.
        let f = make_thread_file(100);
        let entries = read_thread_entries(f.path(), Some(10), Some(0)).unwrap();
        assert_eq!(line_numbers(&entries), (1..=10).collect::<Vec<_>>());
    }

    #[test]
    fn read_limit_zero_returns_empty() {
        let f = make_thread_file(50);
        let entries = read_thread_entries(f.path(), Some(0), None).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn read_empty_file_returns_empty() {
        let f = NamedTempFile::new().unwrap();
        let entries = read_thread_entries(f.path(), Some(50), None).unwrap();
        assert!(entries.is_empty());
    }
}
