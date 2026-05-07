#![deny(warnings)]

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection};
use thiserror::Error;
use tracing::warn;

use gitim_core::dm::parse_dm_filename;
use gitim_core::parser::parse_thread;
use gitim_core::types::Message;

#[derive(Error, Debug)]
pub enum IndexError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("index is currently rebuilding")]
    Rebuilding,
    #[error("search requires at least one of: query, author")]
    EmptySearch,
}

/// 索引状态
enum IndexState {
    Ready(Connection),
    Rebuilding,
}

pub struct Index {
    state: Mutex<IndexState>,
    db_path: std::path::PathBuf,
}

/// 搜索参数
pub struct SearchParams {
    pub query: Option<String>,
    pub author: Option<String>,
    pub channel: Option<String>,
    pub channel_type: Option<String>,
    pub current_user: Option<String>,
    pub limit: usize,
    pub offset: usize,
    pub include_cards: bool,
}

/// 搜索结果中的单条消息
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub channel: String,
    pub channel_type: String,
    pub line_number: u64,
    pub parent_line: u64,
    pub author: String,
    pub timestamp: String,
    pub body: String,
}

/// 搜索结果（含总数）
#[derive(Debug)]
pub struct SearchResponse {
    pub messages: Vec<SearchResult>,
    pub total: usize,
}

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS messages (
    channel TEXT NOT NULL,
    channel_type TEXT NOT NULL,
    line_number INTEGER NOT NULL,
    parent_line INTEGER NOT NULL,
    author TEXT NOT NULL,
    timestamp TEXT NOT NULL,
    body TEXT NOT NULL,
    PRIMARY KEY (channel, line_number)
);

CREATE INDEX IF NOT EXISTS idx_author ON messages(author);
CREATE INDEX IF NOT EXISTS idx_channel_type ON messages(channel_type);
CREATE INDEX IF NOT EXISTS idx_timestamp ON messages(timestamp);

CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
    body,
    content='messages',
    content_rowid='rowid',
    tokenize='trigram'
);

-- Triggers: 自动同步 FTS 表
CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts(rowid, body) VALUES (new.rowid, new.body);
END;

CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, body) VALUES('delete', old.rowid, old.body);
END;

CREATE TABLE IF NOT EXISTS sync_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    commit_id TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
"#;

impl Index {
    /// 打开或创建索引。如果 db 不存在或 schema 损坏，则全量重建。
    pub fn open(db_path: &Path) -> Result<Self, IndexError> {
        let conn = Connection::open(db_path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        conn.execute_batch(SCHEMA_SQL)?;
        Ok(Self {
            state: Mutex::new(IndexState::Ready(conn)),
            db_path: db_path.to_path_buf(),
        })
    }

    /// 创建内存索引，用于测试。
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, IndexError> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA_SQL)?;
        Ok(Self {
            state: Mutex::new(IndexState::Ready(conn)),
            db_path: std::path::PathBuf::from(":memory:"),
        })
    }

    /// 获取当前索引对应的 commit_id。
    pub fn get_commit_id(&self) -> Result<Option<String>, IndexError> {
        let guard = self.state.lock().unwrap();
        let conn = match &*guard {
            IndexState::Ready(c) => c,
            IndexState::Rebuilding => return Err(IndexError::Rebuilding),
        };
        let mut stmt = conn.prepare("SELECT commit_id FROM sync_state WHERE id = 1")?;
        let result = stmt.query_row([], |row| row.get::<_, String>(0)).ok();
        Ok(result)
    }

    /// 设置 commit_id。
    fn set_commit_id(conn: &Connection, commit_id: &str) -> Result<(), IndexError> {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT OR REPLACE INTO sync_state (id, commit_id, updated_at) VALUES (1, ?1, ?2)",
            params![commit_id, now],
        )?;
        Ok(())
    }

    /// 批量插入消息（在事务内）。
    fn insert_messages(
        conn: &Connection,
        channel: &str,
        channel_type: &str,
        messages: &[&Message],
    ) -> Result<(), IndexError> {
        let mut stmt = conn.prepare_cached(
            "INSERT OR IGNORE INTO messages (channel, channel_type, line_number, parent_line, author, timestamp, body)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
        )?;
        for msg in messages {
            stmt.execute(params![
                channel,
                channel_type,
                msg.line_number as i64,
                msg.point_to as i64,
                msg.author.as_str(),
                msg.timestamp,
                msg.body,
            ])?;
        }
        Ok(())
    }

    /// 从 .thread 文件内容索引消息。channel_name 是纯名称（如 "general" 或 "alice--bob"）。
    pub fn index_thread_content(
        &self,
        channel_name: &str,
        content: &str,
    ) -> Result<usize, IndexError> {
        let guard = self.state.lock().unwrap();
        let conn = match &*guard {
            IndexState::Ready(c) => c,
            IndexState::Rebuilding => return Err(IndexError::Rebuilding),
        };

        let channel_type = if parse_dm_filename(channel_name).is_some() {
            "dm"
        } else {
            "channel"
        };
        let parsed = match parse_thread(content) {
            Ok(f) => f,
            Err(e) => {
                warn!("index: failed to parse thread '{}': {}", channel_name, e);
                return Ok(0);
            }
        };

        let count = parsed.messages().len();
        Self::insert_messages(conn, channel_name, channel_type, &parsed.messages())?;
        Ok(count)
    }

    /// 增量更新：从 git diff 结果中索引新消息，并更新 commit_id。
    /// diff_results: HashMap<文件路径字符串, 新增行内容>
    pub fn append_from_diff(
        &self,
        diff_results: &std::collections::HashMap<String, String>,
        new_commit_id: &str,
    ) -> Result<usize, IndexError> {
        let guard = self.state.lock().unwrap();
        let conn = match &*guard {
            IndexState::Ready(c) => c,
            IndexState::Rebuilding => return Err(IndexError::Rebuilding),
        };

        let tx = conn.unchecked_transaction()?;
        let mut total = 0;

        for (path_str, added_content) in diff_results {
            let (channel_name, channel_type) = match parse_diff_path(path_str) {
                Some(v) => v,
                None => continue,
            };

            let parsed = match parse_thread(added_content) {
                Ok(f) => f,
                Err(e) => {
                    warn!("index: failed to parse diff for '{}': {}", path_str, e);
                    continue;
                }
            };

            Self::insert_messages(&tx, &channel_name, channel_type, &parsed.messages())?;
            total += parsed.messages().len();
        }

        Self::set_commit_id(&tx, new_commit_id)?;
        tx.commit()?;
        Ok(total)
    }

    /// 全量重建：扫描 repo_root 下所有 .thread 文件。
    pub fn rebuild(&self, repo_root: &Path, commit_id: &str) -> Result<usize, IndexError> {
        let guard = self.state.lock().unwrap();
        let conn = match &*guard {
            IndexState::Ready(c) => c,
            IndexState::Rebuilding => return Err(IndexError::Rebuilding),
        };

        let tx = conn.unchecked_transaction()?;

        // 清空现有数据
        tx.execute_batch("DELETE FROM messages; DELETE FROM sync_state;")?;

        let mut total = 0;

        // 扫描 channels/
        let channels_dir = repo_root.join("channels");
        if channels_dir.exists() {
            for entry in std::fs::read_dir(&channels_dir).into_iter().flatten() {
                if let Ok(entry) = entry {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.ends_with(".thread") {
                        let channel_name = name.trim_end_matches(".thread");
                        match std::fs::read_to_string(entry.path()) {
                            Ok(content) => {
                                let parsed = match parse_thread(&content) {
                                    Ok(f) => f,
                                    Err(e) => {
                                        warn!("index rebuild: skip {}: {}", name, e);
                                        continue;
                                    }
                                };
                                Self::insert_messages(
                                    &tx,
                                    channel_name,
                                    "channel",
                                    &parsed.messages(),
                                )?;
                                total += parsed.messages().len();
                            }
                            Err(e) => warn!("index rebuild: skip {}: {}", name, e),
                        }
                    }
                }
            }
        }

        // 扫描 dm/
        let dm_dir = repo_root.join("dm");
        if dm_dir.exists() {
            for entry in std::fs::read_dir(&dm_dir).into_iter().flatten() {
                if let Ok(entry) = entry {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.ends_with(".thread") {
                        let channel_name = name.trim_end_matches(".thread");
                        match std::fs::read_to_string(entry.path()) {
                            Ok(content) => {
                                let parsed = match parse_thread(&content) {
                                    Ok(f) => f,
                                    Err(e) => {
                                        warn!("index rebuild: skip {}: {}", name, e);
                                        continue;
                                    }
                                };
                                Self::insert_messages(&tx, channel_name, "dm", &parsed.messages())?;
                                total += parsed.messages().len();
                            }
                            Err(e) => warn!("index rebuild: skip {}: {}", name, e),
                        }
                    }
                }
            }
        }

        // 扫描 channels/<ch>/cards/<id>/discussion.thread
        if channels_dir.exists() {
            for ch_entry in std::fs::read_dir(&channels_dir)
                .into_iter()
                .flatten()
                .flatten()
            {
                if !ch_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }
                let cards_dir = ch_entry.path().join("cards");
                if !cards_dir.exists() {
                    continue;
                }
                let channel_name = ch_entry.file_name().to_string_lossy().to_string();
                for card_entry in std::fs::read_dir(&cards_dir)
                    .into_iter()
                    .flatten()
                    .flatten()
                {
                    if !card_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        continue;
                    }
                    let card_id = card_entry.file_name().to_string_lossy().to_string();
                    let thread_path = card_entry.path().join("discussion.thread");
                    if !thread_path.exists() {
                        continue;
                    }
                    let content = match std::fs::read_to_string(&thread_path) {
                        Ok(c) => c,
                        Err(e) => {
                            warn!(
                                "index rebuild: skip card {}/{}: {}",
                                channel_name, card_id, e
                            );
                            continue;
                        }
                    };
                    let parsed = match parse_thread(&content) {
                        Ok(f) => f,
                        Err(e) => {
                            warn!(
                                "index rebuild: skip card {}/{}: {}",
                                channel_name, card_id, e
                            );
                            continue;
                        }
                    };
                    let ident = format!("channels/{}/cards/{}", channel_name, card_id);
                    Self::insert_messages(&tx, &ident, "card", &parsed.messages())?;
                    total += parsed.messages().len();
                }
            }
        }

        Self::set_commit_id(&tx, commit_id)?;
        tx.commit()?;
        Ok(total)
    }

    /// 全量重建（reindex 命令用）：关闭连接 → 删 db → 重建 → 重开。
    /// Ready 仅在重建成功后设置；失败时尝试恢复为空 db 以避免永久 Rebuilding。
    pub fn reindex(&self, repo_root: &Path, commit_id: &str) -> Result<usize, IndexError> {
        // 1. Mark as rebuilding
        {
            let mut guard = self.state.lock().unwrap();
            *guard = IndexState::Rebuilding;
        }

        // 2. 删除旧 db 文件（及 WAL/SHM）
        let _ = std::fs::remove_file(&self.db_path);
        let _ = std::fs::remove_file(self.db_path.with_extension("db-wal"));
        let _ = std::fs::remove_file(self.db_path.with_extension("db-shm"));

        // 3. Try to open, rebuild, and only then set Ready
        match self.try_open_and_rebuild(repo_root, commit_id) {
            Ok((conn, count)) => {
                let mut guard = self.state.lock().unwrap();
                *guard = IndexState::Ready(conn);
                Ok(count)
            }
            Err(e) => {
                // Recovery: try to open an empty db so we are not stuck in Rebuilding
                match Connection::open(&self.db_path) {
                    Ok(conn) => {
                        let _ = conn
                            .execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;");
                        let _ = conn.execute_batch(SCHEMA_SQL);
                        let mut guard = self.state.lock().unwrap();
                        *guard = IndexState::Ready(conn);
                    }
                    Err(open_err) => {
                        warn!(
                            "reindex recovery failed, index stuck in Rebuilding: {}",
                            open_err
                        );
                    }
                }
                Err(e)
            }
        }
    }

    /// Open a fresh db, create schema, run full rebuild, return connection + count.
    fn try_open_and_rebuild(
        &self,
        repo_root: &Path,
        commit_id: &str,
    ) -> Result<(Connection, usize), IndexError> {
        let conn = Connection::open(&self.db_path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        conn.execute_batch(SCHEMA_SQL)?;

        let tx = conn.unchecked_transaction()?;
        let mut total = 0;

        // 扫描 channels/
        let channels_dir = repo_root.join("channels");
        if channels_dir.exists() {
            for entry in std::fs::read_dir(&channels_dir).into_iter().flatten() {
                if let Ok(entry) = entry {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.ends_with(".thread") {
                        let channel_name = name.trim_end_matches(".thread");
                        match std::fs::read_to_string(entry.path()) {
                            Ok(content) => {
                                let parsed = match parse_thread(&content) {
                                    Ok(f) => f,
                                    Err(e) => {
                                        warn!("index reindex: skip {}: {}", name, e);
                                        continue;
                                    }
                                };
                                Self::insert_messages(
                                    &tx,
                                    channel_name,
                                    "channel",
                                    &parsed.messages(),
                                )?;
                                total += parsed.messages().len();
                            }
                            Err(e) => warn!("index reindex: skip {}: {}", name, e),
                        }
                    }
                }
            }
        }

        // 扫描 dm/
        let dm_dir = repo_root.join("dm");
        if dm_dir.exists() {
            for entry in std::fs::read_dir(&dm_dir).into_iter().flatten() {
                if let Ok(entry) = entry {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.ends_with(".thread") {
                        let channel_name = name.trim_end_matches(".thread");
                        match std::fs::read_to_string(entry.path()) {
                            Ok(content) => {
                                let parsed = match parse_thread(&content) {
                                    Ok(f) => f,
                                    Err(e) => {
                                        warn!("index reindex: skip {}: {}", name, e);
                                        continue;
                                    }
                                };
                                Self::insert_messages(&tx, channel_name, "dm", &parsed.messages())?;
                                total += parsed.messages().len();
                            }
                            Err(e) => warn!("index reindex: skip {}: {}", name, e),
                        }
                    }
                }
            }
        }

        // 扫描 channels/<ch>/cards/<id>/discussion.thread
        if channels_dir.exists() {
            for ch_entry in std::fs::read_dir(&channels_dir)
                .into_iter()
                .flatten()
                .flatten()
            {
                if !ch_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }
                let cards_dir = ch_entry.path().join("cards");
                if !cards_dir.exists() {
                    continue;
                }
                let channel_name = ch_entry.file_name().to_string_lossy().to_string();
                for card_entry in std::fs::read_dir(&cards_dir)
                    .into_iter()
                    .flatten()
                    .flatten()
                {
                    if !card_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        continue;
                    }
                    let card_id = card_entry.file_name().to_string_lossy().to_string();
                    let thread_path = card_entry.path().join("discussion.thread");
                    if !thread_path.exists() {
                        continue;
                    }
                    let content = match std::fs::read_to_string(&thread_path) {
                        Ok(c) => c,
                        Err(e) => {
                            warn!(
                                "index reindex: skip card {}/{}: {}",
                                channel_name, card_id, e
                            );
                            continue;
                        }
                    };
                    let parsed = match parse_thread(&content) {
                        Ok(f) => f,
                        Err(e) => {
                            warn!(
                                "index reindex: skip card {}/{}: {}",
                                channel_name, card_id, e
                            );
                            continue;
                        }
                    };
                    let ident = format!("channels/{}/cards/{}", channel_name, card_id);
                    Self::insert_messages(&tx, &ident, "card", &parsed.messages())?;
                    total += parsed.messages().len();
                }
            }
        }

        Self::set_commit_id(&tx, commit_id)?;
        tx.commit()?;
        Ok((conn, total))
    }

    /// 搜索。
    pub fn search(&self, params: SearchParams) -> Result<SearchResponse, IndexError> {
        if params.query.is_none() && params.author.is_none() {
            return Err(IndexError::EmptySearch);
        }

        let guard = self.state.lock().unwrap();
        let conn = match &*guard {
            IndexState::Ready(c) => c,
            IndexState::Rebuilding => return Err(IndexError::Rebuilding),
        };

        let mut conditions: Vec<String> = Vec::new();
        let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        // FTS 全文搜索
        if let Some(ref query) = params.query {
            let escaped = escape_fts_query(query);
            let idx = bind_values.len() + 1;
            conditions.push(format!(
                "m.rowid IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?{})",
                idx
            ));
            bind_values.push(Box::new(escaped));
        }

        // 作者过滤
        if let Some(ref author) = params.author {
            let idx = bind_values.len() + 1;
            conditions.push(format!("m.author = ?{}", idx));
            bind_values.push(Box::new(author.clone()));
        }

        // 频道过滤
        if let Some(ref channel) = params.channel {
            let idx = bind_values.len() + 1;
            conditions.push(format!("m.channel = ?{}", idx));
            bind_values.push(Box::new(channel.clone()));
        }

        // 频道类型过滤
        if let Some(ref channel_type) = params.channel_type {
            let idx = bind_values.len() + 1;
            conditions.push(format!("m.channel_type = ?{}", idx));
            bind_values.push(Box::new(channel_type.clone()));
        }

        // Cards 默认过滤：除非显式 include_cards=true 或指定了 channel_type
        if !params.include_cards && params.channel_type.is_none() {
            conditions.push("m.channel_type != 'card'".to_string());
        }

        // DM 可见性过滤 (不适用于 card：卡片通过所属 channel 管理访问权限)
        let skip_dm_filter = params.channel_type.as_deref() == Some("card");
        if let Some(ref current_user) = params.current_user {
            if !skip_dm_filter {
                let idx1 = bind_values.len() + 1;
                let card_clause = if params.include_cards {
                    " OR m.channel_type = 'card'"
                } else {
                    ""
                };
                conditions.push(format!(
                    "(m.channel_type = 'channel'{} OR (m.channel LIKE '%' || ?{} || '%'))",
                    card_clause, idx1
                ));
                bind_values.push(Box::new(current_user.clone()));
            }
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        // 查询总数
        let count_sql = format!("SELECT COUNT(*) FROM messages m {}", where_clause);
        let refs: Vec<&dyn rusqlite::types::ToSql> =
            bind_values.iter().map(|b| b.as_ref()).collect();
        let total: usize = conn.query_row(&count_sql, refs.as_slice(), |row| row.get(0))?;

        // 查询结果
        let query_sql = format!(
            "SELECT m.channel, m.channel_type, m.line_number, m.parent_line, m.author, m.timestamp, m.body
             FROM messages m {}
             ORDER BY m.timestamp DESC, m.line_number DESC
             LIMIT ?{} OFFSET ?{}",
            where_clause,
            bind_values.len() + 1,
            bind_values.len() + 2,
        );

        let mut all_binds: Vec<Box<dyn rusqlite::types::ToSql>> = bind_values;
        all_binds.push(Box::new(params.limit as i64));
        all_binds.push(Box::new(params.offset as i64));
        let refs: Vec<&dyn rusqlite::types::ToSql> = all_binds.iter().map(|b| b.as_ref()).collect();

        let mut stmt = conn.prepare(&query_sql)?;
        let messages: Vec<SearchResult> = stmt
            .query_map(refs.as_slice(), |row| {
                Ok(SearchResult {
                    channel: row.get(0)?,
                    channel_type: row.get(1)?,
                    line_number: row.get::<_, i64>(2)? as u64,
                    parent_line: row.get::<_, i64>(3)? as u64,
                    author: row.get(4)?,
                    timestamp: row.get(5)?,
                    body: row.get(6)?,
                })
            })?
            .filter_map(|r| r.ok())
            .filter(|msg| {
                // 精确 DM 可见性过滤
                if msg.channel_type == "dm" {
                    if let Some(ref current_user) = params.current_user {
                        if let Some((a, b)) = parse_dm_filename(&msg.channel) {
                            return a == current_user || b == current_user;
                        }
                        return false;
                    }
                    return false; // 无身份不看 DM
                }
                true
            })
            .collect();

        Ok(SearchResponse { messages, total })
    }
}

/// 从 git diff 的文件路径解析 (channel_identifier, channel_type)。
/// - "channels/<name>.thread" → (name, "channel")
/// - "dm/<h1>--<h2>.thread" → ("<h1>--<h2>", "dm")
/// - "channels/<ch>/cards/<id>/discussion.thread" → ("channels/<ch>/cards/<id>", "card")
fn parse_diff_path(path_str: &str) -> Option<(String, &'static str)> {
    if let Some(rest) = path_str.strip_prefix("channels/") {
        // Plain channel: "channels/<name>.thread" with no nested slashes
        if let Some(name) = rest.strip_suffix(".thread") {
            if !name.contains('/') {
                return Some((name.to_string(), "channel"));
            }
        }
        // Card discussion: "channels/<ch>/cards/<id>/discussion.thread"
        if let Some(card_rel) = rest.strip_suffix("/discussion.thread") {
            let parts: Vec<&str> = card_rel.split('/').collect();
            if parts.len() == 3 && parts[1] == "cards" {
                let ident = format!("channels/{}/cards/{}", parts[0], parts[2]);
                return Some((ident, "card"));
            }
        }
    }
    if let Some(rest) = path_str.strip_prefix("dm/") {
        if let Some(name) = rest.strip_suffix(".thread") {
            return Some((name.to_string(), "dm"));
        }
    }
    None
}

/// 转义 FTS5 查询中的特殊字符。将用户输入包裹在双引号中。
fn escape_fts_query(input: &str) -> String {
    // 对 trigram tokenizer，最简单的方式是直接将输入作为子串匹配
    // FTS5 trigram 支持 LIKE 式的子串搜索，双引号包裹即可
    let escaped = input.replace('"', "\"\"");
    format!("\"{}\"", escaped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_thread_content(messages: &[(&str, u64, &str, &str)]) -> String {
        messages
            .iter()
            .map(|(author, line, ts, body)| {
                format!("[L{:06}][P000000][@{}][{}] {}", line, author, ts, body)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn test_schema_creation() {
        let idx = Index::open_in_memory().unwrap();
        assert!(idx.get_commit_id().unwrap().is_none());
    }

    #[test]
    fn test_index_thread_content() {
        let idx = Index::open_in_memory().unwrap();
        let content = make_thread_content(&[
            ("alice", 1, "20260323T100000Z", "hello world"),
            ("bob", 2, "20260323T100001Z", "hi alice"),
        ]);
        let count = idx.index_thread_content("general", &content).unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_search_by_author() {
        let idx = Index::open_in_memory().unwrap();
        let content = make_thread_content(&[
            ("alice", 1, "20260323T100000Z", "hello world"),
            ("bob", 2, "20260323T100001Z", "hi alice"),
            ("alice", 3, "20260323T100002Z", "how are you"),
        ]);
        idx.index_thread_content("general", &content).unwrap();

        let result = idx
            .search(SearchParams {
                query: None,
                author: Some("alice".to_string()),
                channel: None,
                channel_type: None,
                current_user: Some("alice".to_string()),
                limit: 50,
                offset: 0,
                include_cards: false,
            })
            .unwrap();

        assert_eq!(result.messages.len(), 2);
        assert!(result.messages.iter().all(|m| m.author == "alice"));
    }

    #[test]
    fn test_search_by_query() {
        let idx = Index::open_in_memory().unwrap();
        let content = make_thread_content(&[
            ("alice", 1, "20260323T100000Z", "deploy failed on staging"),
            ("bob", 2, "20260323T100001Z", "checking the logs now"),
            ("alice", 3, "20260323T100002Z", "deploy succeeded after fix"),
        ]);
        idx.index_thread_content("ops", &content).unwrap();

        let result = idx
            .search(SearchParams {
                query: Some("deploy".to_string()),
                author: None,
                channel: None,
                channel_type: None,
                current_user: Some("alice".to_string()),
                limit: 50,
                offset: 0,
                include_cards: false,
            })
            .unwrap();

        assert_eq!(result.messages.len(), 2);
        assert!(result.messages.iter().all(|m| m.body.contains("deploy")));
    }

    #[test]
    fn test_search_by_channel_type() {
        let idx = Index::open_in_memory().unwrap();
        idx.index_thread_content(
            "general",
            &make_thread_content(&[("alice", 1, "20260323T100000Z", "channel msg")]),
        )
        .unwrap();
        idx.index_thread_content(
            "alice--bob",
            &make_thread_content(&[("alice", 1, "20260323T100000Z", "dm msg")]),
        )
        .unwrap();

        let result = idx
            .search(SearchParams {
                query: Some("msg".to_string()),
                author: None,
                channel: None,
                channel_type: Some("dm".to_string()),
                current_user: Some("alice".to_string()),
                limit: 50,
                offset: 0,
                include_cards: false,
            })
            .unwrap();

        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].channel_type, "dm");
    }

    #[test]
    fn test_fts_escape_special_chars() {
        let idx = Index::open_in_memory().unwrap();
        let content =
            make_thread_content(&[("alice", 1, "20260323T100000Z", "hello OR NOT world")]);
        idx.index_thread_content("general", &content).unwrap();

        // 不应该 panic 或报错
        let result = idx.search(SearchParams {
            query: Some("OR NOT".to_string()),
            author: None,
            channel: None,
            channel_type: None,
            current_user: Some("alice".to_string()),
            limit: 50,
            offset: 0,
            include_cards: false,
        });
        assert!(result.is_ok());
    }

    #[test]
    fn test_empty_search_returns_error() {
        let idx = Index::open_in_memory().unwrap();
        let result = idx.search(SearchParams {
            query: None,
            author: None,
            channel: None,
            channel_type: None,
            current_user: Some("alice".to_string()),
            limit: 50,
            offset: 0,
            include_cards: false,
        });
        assert!(matches!(result, Err(IndexError::EmptySearch)));
    }

    #[test]
    fn test_append_from_diff() {
        let idx = Index::open_in_memory().unwrap();
        let mut diff = std::collections::HashMap::new();
        diff.insert(
            "channels/general.thread".to_string(),
            make_thread_content(&[("alice", 1, "20260323T100000Z", "first msg")]),
        );
        let count = idx.append_from_diff(&diff, "abc123").unwrap();
        assert_eq!(count, 1);
        assert_eq!(idx.get_commit_id().unwrap().unwrap(), "abc123");
    }

    #[test]
    fn test_corrupted_thread_skipped() {
        let idx = Index::open_in_memory().unwrap();
        // 损坏的内容
        let count = idx
            .index_thread_content("broken", "this is not valid thread format")
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_combined_search() {
        let idx = Index::open_in_memory().unwrap();
        idx.index_thread_content(
            "general",
            &make_thread_content(&[
                ("alice", 1, "20260323T100000Z", "deploy to staging"),
                ("bob", 2, "20260323T100001Z", "deploy to production"),
                ("alice", 3, "20260323T100002Z", "checking logs"),
            ]),
        )
        .unwrap();

        let result = idx
            .search(SearchParams {
                query: Some("deploy".to_string()),
                author: Some("alice".to_string()),
                channel: None,
                channel_type: None,
                current_user: Some("alice".to_string()),
                limit: 50,
                offset: 0,
                include_cards: false,
            })
            .unwrap();

        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].author, "alice");
        assert!(result.messages[0].body.contains("deploy"));
    }

    #[test]
    fn test_parse_diff_path() {
        assert_eq!(
            parse_diff_path("channels/general.thread"),
            Some(("general".to_string(), "channel"))
        );
        assert_eq!(
            parse_diff_path("dm/alice--bob.thread"),
            Some(("alice--bob".to_string(), "dm"))
        );
        assert_eq!(parse_diff_path("users/alice.meta.yaml"), None);
    }

    #[test]
    fn parse_diff_path_card() {
        let result =
            parse_diff_path("channels/backend/cards/20260417-120000-abc/discussion.thread");
        assert_eq!(
            result,
            Some((
                "channels/backend/cards/20260417-120000-abc".to_string(),
                "card"
            ))
        );
    }

    #[test]
    fn parse_diff_path_channel_still_works() {
        let result = parse_diff_path("channels/backend.thread");
        assert_eq!(result, Some(("backend".to_string(), "channel")));
    }

    #[test]
    fn parse_diff_path_dm_still_works() {
        let result = parse_diff_path("dm/alice--bob.thread");
        assert_eq!(result, Some(("alice--bob".to_string(), "dm")));
    }

    #[test]
    fn parse_diff_path_unknown() {
        let result = parse_diff_path("random/file.txt");
        assert_eq!(result, None);
    }

    #[test]
    fn test_escape_fts_query() {
        assert_eq!(escape_fts_query("hello"), "\"hello\"");
        assert_eq!(
            escape_fts_query("hello \"world\""),
            "\"hello \"\"world\"\"\""
        );
    }
}
