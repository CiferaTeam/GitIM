# SQLite 本地索引 Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 GitIM daemon 添加 SQLite 本地索引，支持全文搜索、按作者过滤、按频道类型筛选。

**Architecture:** 新建 `gitim-index` crate 封装所有 SQLite 逻辑。daemon 通过 `Mutex<Connection>` + `spawn_blocking` 访问索引。sync_loop 新增 `on_synced` 回调驱动增量更新，索引与 git commit_id 保持一致性。

**Tech Stack:** Rust, rusqlite (bundled feature, 含 FTS5), SQLite trigram tokenizer

---

## 文件地图

```
新建:
  crates/gitim-index/Cargo.toml          — crate 配置
  crates/gitim-index/src/lib.rs          — 公共 API (Index struct)
  crates/gitim-index/tests/index_test.rs — 单元测试 (:memory:)
  cli/src/commands/search.ts             — search CLI 命令
  cli/src/commands/reindex.ts            — reindex CLI 命令

修改:
  Cargo.toml:2                           — workspace members 加入 gitim-index
  Cargo.toml:10-19                       — workspace.dependencies 加入 rusqlite
  crates/gitim-daemon/Cargo.toml:7       — 加入 gitim-index 依赖
  crates/gitim-sync/src/sync_loop.rs:14-21 — start_sync_loop 加第三个回调 on_synced
  crates/gitim-sync/src/lib.rs           — (不变，sync_loop 已 pub)
  crates/gitim-daemon/src/state.rs:49-102  — spawn_sync_loop 添加 on_synced 闭包
  crates/gitim-daemon/src/state.rs:18-28   — AppState 加 index 字段
  crates/gitim-daemon/src/api.rs:16-68     — Request enum 加 Search + Reindex
  crates/gitim-daemon/src/handlers.rs:1-50 — dispatch 加 Search + Reindex
  crates/gitim-daemon/src/handlers.rs      — 新增 handle_search + handle_reindex
  crates/gitim-daemon/src/main.rs:74-106   — 启动时初始化索引
  cli/src/client.ts:84-87                  — 加 search() + reindex()
  cli/src/index.ts                         — 注册 search + reindex 命令
```

---

## Chunk 1: gitim-index crate 核心

### Task 1: 创建 crate 骨架 + schema

**Files:**
- Create: `crates/gitim-index/Cargo.toml`
- Create: `crates/gitim-index/src/lib.rs`
- Modify: `Cargo.toml` (root workspace)

- [ ] **Step 1: 创建 crate 目录和 Cargo.toml**

```bash
mkdir -p crates/gitim-index/src
```

写入 `crates/gitim-index/Cargo.toml`:

```toml
[package]
name = "gitim-index"
version.workspace = true
edition.workspace = true

[dependencies]
gitim-core = { path = "../gitim-core" }
rusqlite = { version = "0.34", features = ["bundled", "modern_sqlite"] }
tracing.workspace = true
thiserror.workspace = true

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: 注册到 workspace**

修改 `Cargo.toml:2`:

```toml
members = ["crates/gitim-core", "crates/gitim-daemon", "crates/gitim-sync", "crates/gitim-index"]
```

- [ ] **Step 3: 写 lib.rs — schema 定义 + Index struct**

写入 `crates/gitim-index/src/lib.rs`:

```rust
#![deny(warnings)]

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, params};
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
        let result = stmt
            .query_row([], |row| row.get::<_, String>(0))
            .ok();
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
    fn insert_messages(conn: &Connection, channel: &str, channel_type: &str, messages: &[Message]) -> Result<(), IndexError> {
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
    pub fn index_thread_content(&self, channel_name: &str, content: &str) -> Result<usize, IndexError> {
        let guard = self.state.lock().unwrap();
        let conn = match &*guard {
            IndexState::Ready(c) => c,
            IndexState::Rebuilding => return Err(IndexError::Rebuilding),
        };

        let channel_type = if parse_dm_filename(channel_name).is_some() { "dm" } else { "channel" };
        let parsed = match parse_thread(content) {
            Ok(f) => f,
            Err(e) => {
                warn!("index: failed to parse thread '{}': {}", channel_name, e);
                return Ok(0);
            }
        };

        let count = parsed.messages.len();
        Self::insert_messages(conn, channel_name, channel_type, &parsed.messages)?;
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

            Self::insert_messages(&tx, &channel_name, channel_type, &parsed.messages)?;
            total += parsed.messages.len();
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
                                Self::insert_messages(&tx, channel_name, "channel", &parsed.messages)?;
                                total += parsed.messages.len();
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
                                Self::insert_messages(&tx, channel_name, "dm", &parsed.messages)?;
                                total += parsed.messages.len();
                            }
                            Err(e) => warn!("index rebuild: skip {}: {}", name, e),
                        }
                    }
                }
            }
        }

        Self::set_commit_id(&tx, commit_id)?;
        tx.commit()?;
        Ok(total)
    }

    /// 全量重建（reindex 命令用）：关闭连接 → 删 db → 重建 → 重开。
    pub fn reindex(&self, repo_root: &Path, commit_id: &str) -> Result<usize, IndexError> {
        {
            let mut guard = self.state.lock().unwrap();
            *guard = IndexState::Rebuilding;
        }

        // 删除旧 db 文件（及 WAL/SHM）
        let _ = std::fs::remove_file(&self.db_path);
        let _ = std::fs::remove_file(self.db_path.with_extension("db-wal"));
        let _ = std::fs::remove_file(self.db_path.with_extension("db-shm"));

        // 重新打开
        let conn = Connection::open(&self.db_path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        conn.execute_batch(SCHEMA_SQL)?;

        {
            let mut guard = self.state.lock().unwrap();
            *guard = IndexState::Ready(conn);
        }

        self.rebuild(repo_root, commit_id)
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
            conditions.push("m.rowid IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?{})".replace("{}", &(bind_values.len() + 1).to_string()));
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

        // DM 可见性过滤
        if let Some(ref current_user) = params.current_user {
            let idx1 = bind_values.len() + 1;
            let idx2 = bind_values.len() + 2;
            conditions.push(format!(
                "(m.channel_type = 'channel' OR (m.channel LIKE '%' || ?{} || '%'))",
                idx1
            ));
            // 更精确的 DM 过滤需要解析 channel name，这里用 LIKE 做近似匹配
            // 后续在结果集上再精确过滤
            bind_values.push(Box::new(current_user.clone()));
            let _ = idx2; // 预留
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        // 查询总数
        let count_sql = format!("SELECT COUNT(*) FROM messages m {}", where_clause);
        let refs: Vec<&dyn rusqlite::types::ToSql> = bind_values.iter().map(|b| b.as_ref()).collect();
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

/// 从 diff 文件路径解析出 channel_name 和 channel_type。
fn parse_diff_path(path_str: &str) -> Option<(String, &'static str)> {
    if let Some(name) = path_str.strip_prefix("channels/") {
        let name = name.strip_suffix(".thread")?;
        Some((name.to_string(), "channel"))
    } else if let Some(name) = path_str.strip_prefix("dm/") {
        let name = name.strip_suffix(".thread")?;
        Some((name.to_string(), "dm"))
    } else {
        None
    }
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

        let result = idx.search(SearchParams {
            query: None,
            author: Some("alice".to_string()),
            channel: None,
            channel_type: None,
            current_user: Some("alice".to_string()),
            limit: 50,
            offset: 0,
        }).unwrap();

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

        let result = idx.search(SearchParams {
            query: Some("deploy".to_string()),
            author: None,
            channel: None,
            channel_type: None,
            current_user: Some("alice".to_string()),
            limit: 50,
            offset: 0,
        }).unwrap();

        assert_eq!(result.messages.len(), 2);
        assert!(result.messages.iter().all(|m| m.body.contains("deploy")));
    }

    #[test]
    fn test_search_by_channel_type() {
        let idx = Index::open_in_memory().unwrap();
        idx.index_thread_content("general", &make_thread_content(&[
            ("alice", 1, "20260323T100000Z", "channel msg"),
        ])).unwrap();
        idx.index_thread_content("alice--bob", &make_thread_content(&[
            ("alice", 1, "20260323T100000Z", "dm msg"),
        ])).unwrap();

        let result = idx.search(SearchParams {
            query: Some("msg".to_string()),
            author: None,
            channel: None,
            channel_type: Some("dm".to_string()),
            current_user: Some("alice".to_string()),
            limit: 50,
            offset: 0,
        }).unwrap();

        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].channel_type, "dm");
    }

    #[test]
    fn test_dm_visibility_filter() {
        let idx = Index::open_in_memory().unwrap();
        idx.index_thread_content("alice--bob", &make_thread_content(&[
            ("alice", 1, "20260323T100000Z", "secret to bob"),
        ])).unwrap();
        idx.index_thread_content("alice--charlie", &make_thread_content(&[
            ("alice", 1, "20260323T100000Z", "secret to charlie"),
        ])).unwrap();

        // bob 只能看到 alice--bob 的 DM
        let result = idx.search(SearchParams {
            query: Some("secret".to_string()),
            author: None,
            channel: None,
            channel_type: None,
            current_user: Some("bob".to_string()),
            limit: 50,
            offset: 0,
        }).unwrap();

        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].channel, "alice--bob");
    }

    #[test]
    fn test_fts_escape_special_chars() {
        let idx = Index::open_in_memory().unwrap();
        let content = make_thread_content(&[
            ("alice", 1, "20260323T100000Z", "hello OR NOT world"),
        ]);
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
    fn test_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        let channels_dir = dir.path().join("channels");
        std::fs::create_dir_all(&channels_dir).unwrap();
        std::fs::write(
            channels_dir.join("general.thread"),
            make_thread_content(&[
                ("alice", 1, "20260323T100000Z", "hello"),
                ("bob", 2, "20260323T100001Z", "world"),
            ]),
        ).unwrap();

        let idx = Index::open_in_memory().unwrap();
        let count = idx.rebuild(dir.path(), "def456").unwrap();
        assert_eq!(count, 2);
        assert_eq!(idx.get_commit_id().unwrap().unwrap(), "def456");
    }

    #[test]
    fn test_corrupted_thread_skipped() {
        let idx = Index::open_in_memory().unwrap();
        // 损坏的内容
        let count = idx.index_thread_content("broken", "this is not valid thread format").unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_combined_search() {
        let idx = Index::open_in_memory().unwrap();
        idx.index_thread_content("general", &make_thread_content(&[
            ("alice", 1, "20260323T100000Z", "deploy to staging"),
            ("bob", 2, "20260323T100001Z", "deploy to production"),
            ("alice", 3, "20260323T100002Z", "checking logs"),
        ])).unwrap();

        let result = idx.search(SearchParams {
            query: Some("deploy".to_string()),
            author: Some("alice".to_string()),
            channel: None,
            channel_type: None,
            current_user: Some("alice".to_string()),
            limit: 50,
            offset: 0,
        }).unwrap();

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
        assert_eq!(parse_diff_path("users/alice.meta.json"), None);
    }

    #[test]
    fn test_escape_fts_query() {
        assert_eq!(escape_fts_query("hello"), "\"hello\"");
        assert_eq!(escape_fts_query("hello \"world\""), "\"hello \"\"world\"\"\"");
    }
}
```

- [ ] **Step 4: 编译验证**

Run: `cargo build -p gitim-index`
Expected: 成功

- [ ] **Step 5: 运行测试**

Run: `cargo test -p gitim-index`
Expected: 全部通过

- [ ] **Step 6: Commit**

```bash
git add crates/gitim-index/ Cargo.toml
git commit -m "feat(index): add gitim-index crate with SQLite FTS5 schema, search, rebuild"
```

---

## Chunk 2: sync_loop 加 on_synced 回调

### Task 2: 给 sync_loop 加第三个回调

**Files:**
- Modify: `crates/gitim-sync/src/sync_loop.rs:14-46`

- [ ] **Step 1: 修改 start_sync_loop 签名，加 F3 on_synced**

修改 `crates/gitim-sync/src/sync_loop.rs`。将 `start_sync_loop` 函数签名改为：

```rust
/// Start the sync loop with push-first strategy.
///
/// - `on_pushed`: called after a successful push (all pending messages are now remote)
/// - `on_renumbered`: called for each message that was renumbered during conflict resolution
///   (file, old_line, new_line)
/// - `on_synced`: called after every sync cycle completes, with the current HEAD commit hash.
///   The index layer uses this to decide whether incremental updates are needed.
pub async fn start_sync_loop<F1, F2, F3>(
    repo_root: &Path,
    interval_secs: u32,
    on_pushed: F1,
    on_renumbered: F2,
    on_synced: F3,
) where
    F1: Fn() + Send + 'static,
    F2: Fn(PathBuf, u64, u64) + Send + 'static,
    F3: Fn(String) + Send + 'static,
```

- [ ] **Step 2: 在 run_sync_cycle 末尾调用 on_synced**

修改 `run_sync_cycle` 签名加上 `on_synced: &F3`，在函数末尾（两个分支都执行完后）加：

```rust
fn run_sync_cycle<F1, F2, F3>(repo: &GitStorage, on_pushed: &F1, on_renumbered: &F2, on_synced: &F3)
where
    F1: Fn(),
    F2: Fn(PathBuf, u64, u64),
    F3: Fn(String),
{
    // ... 现有逻辑 ...

    // Cycle 结束后，通知 on_synced（即使没有变更）
    match repo.rev_parse("HEAD") {
        Ok(head) => on_synced(head),
        Err(e) => warn!("sync: failed to get HEAD for on_synced: {}", e),
    }
}
```

同时更新 loop 中的调用：

```rust
run_sync_cycle(&repo, &on_pushed, &on_renumbered, &on_synced);
```

同样更新 `sync_with_push` 签名不变（它不需要 on_synced，因为 run_sync_cycle 统一调用）。

- [ ] **Step 3: 编译并确认现有测试仍通过**

Run: `cargo build -p gitim-sync && cargo test -p gitim-sync`
Expected: 编译成功。如果有调用 start_sync_loop 的地方需要补 on_synced 参数（state.rs 那边会在 Task 3 修复）。此时只确保 sync crate 自身通过。

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-sync/src/sync_loop.rs
git commit -m "feat(sync): add on_synced callback to sync_loop for index integration"
```

---

## Chunk 3: daemon 集成

### Task 3: AppState 集成索引 + spawn_sync_loop 适配

**Files:**
- Modify: `crates/gitim-daemon/Cargo.toml:7`
- Modify: `crates/gitim-daemon/src/state.rs:18-102`
- Modify: `crates/gitim-daemon/src/main.rs:74-106`

- [ ] **Step 1: 给 daemon 加 gitim-index 依赖**

在 `crates/gitim-daemon/Cargo.toml` 的 `[dependencies]` 中添加：

```toml
gitim-index = { path = "../gitim-index" }
```

- [ ] **Step 2: AppState 添加 index 字段**

修改 `crates/gitim-daemon/src/state.rs`：

在 `AppState` struct 中添加 index 字段：

```rust
pub struct AppState {
    pub repo_root: PathBuf,
    pub config: Config,
    pub git_storage: GitStorage,
    pub thread_cache: RwLock<HashMap<String, ThreadFile>>,
    pub users: RwLock<Vec<String>>,
    pub event_tx: broadcast::Sender<Event>,
    pub current_user: RwLock<Option<String>>,
    pub pending_push: std::sync::RwLock<Vec<PendingMessage>>,
    pub sync_started: AtomicBool,
    pub index: Option<std::sync::Arc<gitim_index::Index>>,
}
```

更新 `new()` 构造函数：`index: None`

- [ ] **Step 3: 添加 initialize_index 方法**

在 `impl AppState` 中添加：

```rust
/// 初始化索引。检查现有 db 状态，决定增量更新还是全量重建。
pub fn initialize_index(state: &SharedState) -> Result<(), Box<dyn std::error::Error>> {
    let db_path = state.repo_root.join(".gitim").join("index.db");
    let index = std::sync::Arc::new(gitim_index::Index::open(&db_path)?);

    let current_head = state.git_storage.rev_parse("HEAD").unwrap_or_default();

    match index.get_commit_id()? {
        Some(stored_commit) if stored_commit == current_head => {
            tracing::info!("index up to date at {}", &current_head[..8]);
        }
        Some(stored_commit) => {
            // 检查是否是祖先
            let is_ancestor = std::process::Command::new("git")
                .args(["merge-base", "--is-ancestor", &stored_commit, &current_head])
                .current_dir(&state.repo_root)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);

            if is_ancestor {
                tracing::info!("index behind, incremental update from {}..{}", &stored_commit[..8], &current_head[..8]);
                let diff = state.git_storage.diff_range(&stored_commit, &current_head)?;
                let diff_strings: std::collections::HashMap<String, String> = diff
                    .into_iter()
                    .map(|(k, v)| (k.to_string_lossy().to_string(), v))
                    .collect();
                let count = index.append_from_diff(&diff_strings, &current_head)?;
                tracing::info!("index: {} messages added", count);
            } else {
                tracing::warn!("index commit not ancestor of HEAD, full rebuild");
                let count = index.rebuild(&state.repo_root, &current_head)?;
                tracing::info!("index rebuilt: {} messages indexed", count);
            }
        }
        None => {
            tracing::info!("index empty, full rebuild");
            let count = index.rebuild(&state.repo_root, &current_head)?;
            tracing::info!("index rebuilt: {} messages indexed", count);
        }
    }

    // 写入 state — 这里需要 unsafe 因为 AppState 已经创建
    // 更好的方式是在构造时就传入，但为了最小改动，用 Option
    unsafe {
        let state_ptr = std::sync::Arc::as_ptr(state) as *mut AppState;
        (*state_ptr).index = Some(index);
    }

    Ok(())
}
```

注意：上面用了 unsafe 来修改 Arc 内部的 Option，这不理想。更好的方式是让 index 字段本身就是 `std::sync::RwLock<Option<Arc<Index>>>`，这样可以安全地设置。修改为：

```rust
pub index: std::sync::RwLock<Option<std::sync::Arc<gitim_index::Index>>>,
```

构造函数中：`index: std::sync::RwLock::new(None)`

initialize_index 最后改为：

```rust
*state.index.write().unwrap() = Some(index);
Ok(())
```

- [ ] **Step 4: spawn_sync_loop 添加 on_synced 闭包**

修改 `spawn_sync_loop`，在 `start_sync_loop` 调用时传入第三个闭包：

```rust
let synced_state = state.clone();

tokio::spawn(async move {
    gitim_sync::sync_loop::start_sync_loop(
        &sync_root,
        sync_interval,
        move || { /* on_pushed — 现有代码不变 */ },
        move |file, old_line, new_line| { /* on_renumbered — 现有代码不变 */ },
        move |head_commit| {
            // on_synced: 增量更新索引
            let idx_guard = synced_state.index.read().unwrap();
            if let Some(ref index) = *idx_guard {
                match index.get_commit_id() {
                    Ok(Some(stored)) if stored == head_commit => { /* 已是最新 */ }
                    Ok(Some(stored)) => {
                        let diff = synced_state.git_storage.diff_range(&stored, &head_commit);
                        match diff {
                            Ok(d) => {
                                let diff_strings: std::collections::HashMap<String, String> = d
                                    .into_iter()
                                    .map(|(k, v)| (k.to_string_lossy().to_string(), v))
                                    .collect();
                                match index.append_from_diff(&diff_strings, &head_commit) {
                                    Ok(n) if n > 0 => tracing::info!("index: +{} messages", n),
                                    Ok(_) => {}
                                    Err(e) => tracing::warn!("index append failed: {}", e),
                                }
                            }
                            Err(e) => tracing::warn!("index diff failed: {}", e),
                        }
                    }
                    Ok(None) => {
                        // 索引空，全量重建
                        match index.rebuild(&synced_state.repo_root, &head_commit) {
                            Ok(n) => tracing::info!("index rebuilt: {} messages", n),
                            Err(e) => tracing::warn!("index rebuild failed: {}", e),
                        }
                    }
                    Err(e) => tracing::warn!("index get_commit_id failed: {}", e),
                }
            }
        },
    )
    .await;
});
```

- [ ] **Step 5: main.rs 中初始化索引**

修改 `crates/gitim-daemon/src/main.rs`，在 `app_state` 创建后、sync_loop 启动前添加：

```rust
// Initialize search index
if let Err(e) = state::AppState::initialize_index(&app_state) {
    tracing::warn!("index initialization failed (search will be unavailable): {}", e);
}
```

- [ ] **Step 6: 编译验证**

Run: `cargo build`
Expected: 全部编译通过

- [ ] **Step 7: Commit**

```bash
git add crates/gitim-daemon/
git commit -m "feat(daemon): integrate sqlite index into AppState and sync_loop"
```

### Task 4: 添加 Search + Reindex API handler

**Files:**
- Modify: `crates/gitim-daemon/src/api.rs:16-68`
- Modify: `crates/gitim-daemon/src/handlers.rs`

- [ ] **Step 1: api.rs 添加 Search 和 Reindex 请求类型**

在 `Request` enum 中添加：

```rust
    #[serde(rename = "search")]
    Search {
        #[serde(default)]
        query: Option<String>,
        #[serde(default)]
        author: Option<String>,
        #[serde(default)]
        channel: Option<String>,
        #[serde(default)]
        channel_type: Option<String>,
        #[serde(default = "default_limit")]
        limit: usize,
        #[serde(default)]
        offset: usize,
    },
    #[serde(rename = "reindex")]
    Reindex,
```

添加默认函数：

```rust
fn default_limit() -> usize { 50 }
```

- [ ] **Step 2: handlers.rs 添加 dispatch 和 handler**

在 `handle_request` 的 match 中添加：

```rust
        Request::Search { query, author, channel, channel_type, limit, offset } => {
            handle_search(state, query, author, channel, channel_type, limit, offset).await
        }
        Request::Reindex => handle_reindex(state).await,
```

添加 handler 函数：

```rust
async fn handle_search(
    state: SharedState,
    query: Option<String>,
    author: Option<String>,
    channel: Option<String>,
    channel_type: Option<String>,
    limit: usize,
    offset: usize,
) -> Response {
    let current_user = state.current_user.read().await.clone();
    let index_guard = state.index.read().unwrap();
    let index = match &*index_guard {
        Some(idx) => idx.clone(),
        None => return Response::error("search index not available"),
    };
    drop(index_guard);

    let params = gitim_index::SearchParams {
        query,
        author,
        channel,
        channel_type,
        current_user,
        limit,
        offset,
    };

    match tokio::task::spawn_blocking(move || index.search(params)).await {
        Ok(Ok(result)) => {
            let messages: Vec<serde_json::Value> = result.messages.iter().map(|m| {
                serde_json::json!({
                    "channel": m.channel,
                    "channel_type": m.channel_type,
                    "line_number": m.line_number,
                    "parent_line": m.parent_line,
                    "author": m.author,
                    "timestamp": m.timestamp,
                    "body": m.body,
                })
            }).collect();
            Response::success(serde_json::json!({
                "messages": messages,
                "total": result.total,
            }))
        }
        Ok(Err(gitim_index::IndexError::Rebuilding)) => {
            Response::error("indexing_in_progress")
        }
        Ok(Err(gitim_index::IndexError::EmptySearch)) => {
            Response::error("search requires at least one of: query, author")
        }
        Ok(Err(e)) => Response::error(format!("search failed: {}", e)),
        Err(e) => Response::error(format!("search task failed: {}", e)),
    }
}

async fn handle_reindex(state: SharedState) -> Response {
    let index_guard = state.index.read().unwrap();
    let index = match &*index_guard {
        Some(idx) => idx.clone(),
        None => return Response::error("search index not available"),
    };
    drop(index_guard);

    let repo_root = state.repo_root.clone();
    let head = match state.git_storage.rev_parse("HEAD") {
        Ok(h) => h,
        Err(e) => return Response::error(format!("failed to get HEAD: {}", e)),
    };

    match tokio::task::spawn_blocking(move || index.reindex(&repo_root, &head)).await {
        Ok(Ok(count)) => Response::success(serde_json::json!({
            "status": "complete",
            "messages_indexed": count,
        })),
        Ok(Err(e)) => Response::error(format!("reindex failed: {}", e)),
        Err(e) => Response::error(format!("reindex task failed: {}", e)),
    }
}
```

- [ ] **Step 3: 编译验证**

Run: `cargo build`
Expected: 成功

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-daemon/src/api.rs crates/gitim-daemon/src/handlers.rs
git commit -m "feat(daemon): add search and reindex API handlers"
```

---

## Chunk 4: CLI 集成

### Task 5: TypeScript CLI 添加 search 和 reindex 命令

**Files:**
- Modify: `cli/src/client.ts:84-87`
- Create: `cli/src/commands/search.ts`
- Create: `cli/src/commands/reindex.ts`
- Modify: `cli/src/index.ts`

- [ ] **Step 1: client.ts 添加 search 和 reindex 方法**

在 `GitimClient` 类中，`poll()` 方法后面添加：

```typescript
  async search(params: {
    query?: string;
    author?: string;
    channel?: string;
    channel_type?: string;
    limit?: number;
    offset?: number;
  }): Promise<ApiResponse> {
    return this.request('search', {
      query: params.query ?? null,
      author: params.author ?? null,
      channel: params.channel ?? null,
      channel_type: params.channel_type ?? null,
      limit: params.limit ?? 50,
      offset: params.offset ?? 0,
    });
  }

  async reindex(): Promise<ApiResponse> {
    return this.request('reindex');
  }
```

- [ ] **Step 2: 创建 search.ts 命令**

写入 `cli/src/commands/search.ts`:

```typescript
import { GitimClient } from '../client.js';
import { ensureDaemon, findRepoRoot } from '../daemon.js';

export async function searchCommand(query: string | undefined, options: {
  author?: string;
  channel?: string;
  type?: string;
  limit?: string;
  offset?: string;
}) {
  const repoRoot = findRepoRoot();
  if (!repoRoot) {
    console.error('Not in a GitIM repo');
    process.exit(1);
  }

  await ensureDaemon(repoRoot);
  const client = new GitimClient(repoRoot);

  const result = await client.search({
    query: query || undefined,
    author: options.author,
    channel: options.channel,
    channel_type: options.type,
    limit: options.limit ? parseInt(options.limit) : undefined,
    offset: options.offset ? parseInt(options.offset) : undefined,
  });

  if (!result.ok) {
    console.error(`Search failed: ${result.error}`);
    process.exit(1);
  }

  const { messages, total } = result.data;
  console.log(`Found ${total} results:\n`);

  for (const msg of messages) {
    const prefix = msg.channel_type === 'dm' ? `[DM:${msg.channel}]` : `[#${msg.channel}]`;
    console.log(`${prefix} @${msg.author} (L${msg.line_number}) ${msg.timestamp}`);
    console.log(`  ${msg.body}`);
    console.log();
  }
}
```

- [ ] **Step 3: 创建 reindex.ts 命令**

写入 `cli/src/commands/reindex.ts`:

```typescript
import { GitimClient } from '../client.js';
import { ensureDaemon, findRepoRoot } from '../daemon.js';

export async function reindexCommand() {
  const repoRoot = findRepoRoot();
  if (!repoRoot) {
    console.error('Not in a GitIM repo');
    process.exit(1);
  }

  await ensureDaemon(repoRoot);
  const client = new GitimClient(repoRoot);

  console.log('Rebuilding search index...');
  const result = await client.reindex();

  if (!result.ok) {
    console.error(`Reindex failed: ${result.error}`);
    process.exit(1);
  }

  console.log(`Done. ${result.data.messages_indexed} messages indexed.`);
}
```

- [ ] **Step 4: index.ts 注册新命令**

在 `cli/src/index.ts` 中添加 import：

```typescript
import { searchCommand } from './commands/search.js';
import { reindexCommand } from './commands/reindex.js';
```

在 `dm` 命令之前添加：

```typescript
program
  .command('search [query]')
  .description('搜索消息')
  .option('-a, --author <handler>', '按作者过滤')
  .option('-c, --channel <name>', '限定频道')
  .option('-t, --type <type>', '频道类型: channel | dm')
  .option('-l, --limit <n>', '结果数量限制', '50')
  .option('--offset <n>', '分页偏移', '0')
  .action((query, options) => searchCommand(query, options));

program
  .command('reindex')
  .description('重建搜索索引')
  .action(() => reindexCommand());
```

- [ ] **Step 5: Commit**

```bash
git add cli/src/client.ts cli/src/commands/search.ts cli/src/commands/reindex.ts cli/src/index.ts
git commit -m "feat(cli): add search and reindex commands"
```

---

## Chunk 5: 集成测试

### Task 6: 端到端集成测试

**Files:**
- Create: `crates/gitim-index/tests/index_test.rs` (如果 Task 1 中的 `#[cfg(test)]` 不够，补充文件级集成测试)

- [ ] **Step 1: 写集成测试 — 全量构建 + 增量 + 搜索**

写入 `crates/gitim-index/tests/integration_test.rs`:

```rust
use std::collections::HashMap;
use tempfile::tempdir;

fn make_msg(author: &str, line: u64, ts: &str, body: &str) -> String {
    format!("[L{:06}][P000000][@{}][{}] {}", line, author, ts, body)
}

#[test]
fn test_full_rebuild_then_incremental_append() {
    let dir = tempdir().unwrap();

    // 创建初始 .thread 文件
    let channels = dir.path().join("channels");
    std::fs::create_dir_all(&channels).unwrap();
    std::fs::write(
        channels.join("general.thread"),
        [
            make_msg("alice", 1, "20260323T100000Z", "hello everyone"),
            make_msg("bob", 2, "20260323T100001Z", "hi alice"),
        ].join("\n"),
    ).unwrap();

    let dm = dir.path().join("dm");
    std::fs::create_dir_all(&dm).unwrap();
    std::fs::write(
        dm.join("alice--bob.thread"),
        make_msg("alice", 1, "20260323T100000Z", "secret message"),
    ).unwrap();

    // 全量构建
    let index = gitim_index::Index::open_in_memory().unwrap();
    let count = index.rebuild(dir.path(), "commit_aaa").unwrap();
    assert_eq!(count, 3); // 2 channel + 1 dm

    // 验证搜索
    let result = index.search(gitim_index::SearchParams {
        query: Some("hello".to_string()),
        author: None,
        channel: None,
        channel_type: None,
        current_user: Some("alice".to_string()),
        limit: 50,
        offset: 0,
    }).unwrap();
    assert_eq!(result.messages.len(), 1);
    assert_eq!(result.messages[0].author, "alice");

    // 增量追加
    let mut diff = HashMap::new();
    diff.insert(
        "channels/general.thread".to_string(),
        make_msg("charlie", 3, "20260323T100002Z", "hello from charlie"),
    );
    let added = index.append_from_diff(&diff, "commit_bbb").unwrap();
    assert_eq!(added, 1);
    assert_eq!(index.get_commit_id().unwrap().unwrap(), "commit_bbb");

    // 验证增量后搜索
    let result = index.search(gitim_index::SearchParams {
        query: Some("hello".to_string()),
        author: None,
        channel: None,
        channel_type: None,
        current_user: Some("alice".to_string()),
        limit: 50,
        offset: 0,
    }).unwrap();
    assert_eq!(result.messages.len(), 2); // alice + charlie
}

#[test]
fn test_dm_visibility_across_channels() {
    let index = gitim_index::Index::open_in_memory().unwrap();

    // 索引多个 DM 和频道
    index.index_thread_content("general", &make_msg("alice", 1, "20260323T100000Z", "public hello")).unwrap();
    index.index_thread_content("alice--bob", &make_msg("alice", 1, "20260323T100000Z", "private to bob")).unwrap();
    index.index_thread_content("alice--charlie", &make_msg("charlie", 1, "20260323T100000Z", "private to alice")).unwrap();
    index.index_thread_content("bob--charlie", &make_msg("bob", 1, "20260323T100000Z", "private bob charlie")).unwrap();

    // bob 搜索 "private" — 应该只看到 alice--bob 和 bob--charlie
    let result = index.search(gitim_index::SearchParams {
        query: Some("private".to_string()),
        author: None,
        channel: None,
        channel_type: None,
        current_user: Some("bob".to_string()),
        limit: 50,
        offset: 0,
    }).unwrap();

    let channels: Vec<&str> = result.messages.iter().map(|m| m.channel.as_str()).collect();
    assert!(channels.contains(&"alice--bob"));
    assert!(channels.contains(&"bob--charlie"));
    assert!(!channels.contains(&"alice--charlie"));
}

#[test]
fn test_reindex_from_scratch() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test_index.db");

    let channels = dir.path().join("channels");
    std::fs::create_dir_all(&channels).unwrap();
    std::fs::write(
        channels.join("ops.thread"),
        make_msg("alice", 1, "20260323T100000Z", "deploy complete"),
    ).unwrap();

    // 创建并使用索引
    let index = gitim_index::Index::open(&db_path).unwrap();
    index.rebuild(dir.path(), "commit_111").unwrap();

    // reindex 全量重建
    let count = index.reindex(dir.path(), "commit_222").unwrap();
    assert_eq!(count, 1);
    assert_eq!(index.get_commit_id().unwrap().unwrap(), "commit_222");
}
```

- [ ] **Step 2: 运行所有测试**

Run: `cargo test -p gitim-index`
Expected: 全部通过

- [ ] **Step 3: 运行全项目测试**

Run: `cargo test`
Expected: 全部通过（确认没有破坏已有功能）

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-index/tests/
git commit -m "test(index): add integration tests for rebuild, incremental, dm visibility, reindex"
```

---

## 依赖关系

```
Task 1 (gitim-index crate)
    ↓
Task 2 (sync_loop on_synced) ← 可和 Task 1 并行
    ↓
Task 3 (daemon 集成) ← 依赖 Task 1 + Task 2
    ↓
Task 4 (API handlers) ← 依赖 Task 3
    ↓
Task 5 (CLI 集成) ← 依赖 Task 4
    ↓
Task 6 (集成测试) ← 依赖 Task 1，可和 Task 3-5 并行
```

**可并行的 subagent 分组：**
- **Group A**: Task 1 + Task 2（互相无依赖）
- **Group B**: Task 3 + Task 4（串行，依赖 Group A）
- **Group C**: Task 5（依赖 Group B）
- **Group D**: Task 6（依赖 Task 1，可和 Group B/C 并行写，最后统一运行）
