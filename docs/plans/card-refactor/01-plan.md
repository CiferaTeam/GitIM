# Card Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把后端"board + card"二层模型改造为"card 挂 channel"单层模型。删除 `BoardMeta`、`boards/*` 目录、`board_handlers.rs`、board CLI、board IPC 方法；引入 `CardMeta`（带 `channel`/`labels`/`status` enum）、`channels/<ch>/cards/<id>/` 布局、新 card handlers、新 CLI、runtime HTTP 端点、index `channel_type=card` 支持。

**Architecture:** 顺序推进六个 phase：core types → daemon handler → client → CLI → runtime HTTP → index。每 phase 末尾 `cargo test` 验证，完整闭环后端到端跑通。

**Tech Stack:** Rust（`gitim-core`/`gitim-daemon`/`gitim-client`/`gitim-cli`/`gitim-runtime`/`gitim-index`），`serde_yaml` 存储，`tokio::broadcast` 事件，`axum` HTTP，`rusqlite` FTS5 索引。

---

## 文件结构总览

### Create

- `crates/gitim-core/src/types/card.rs` — `CardMeta` + `CardStatus` enum
- `crates/gitim-daemon/src/card_handlers.rs` — 新 card handlers
- `crates/gitim-daemon/tests/card_test.rs` — 新测试

### Modify

- `crates/gitim-core/src/types/mod.rs` — 导出 card 模块
- `crates/gitim-daemon/src/api.rs` — Request enum / Event enum
- `crates/gitim-daemon/src/handlers.rs` — dispatcher + guest-mode guard
- `crates/gitim-daemon/src/lib.rs` — 模块声明
- `crates/gitim-client/src/client.rs` — 方法签名
- `crates/gitim-cli/src/main.rs` — clap 命令定义
- `crates/gitim-cli/src/commands/card.rs` — CLI 实现
- `crates/gitim-cli/src/commands/mod.rs` — 模块声明
- `crates/gitim-runtime/src/http.rs` — 新增 5 个 `/im/cards/...` 端点
- `crates/gitim-index/src/lib.rs` — channel_type=card 支持、parse_diff_path、rebuild 扫描、search include_cards

### Delete

- `crates/gitim-core/src/types/board.rs`
- `crates/gitim-daemon/src/board_handlers.rs`
- `crates/gitim-daemon/tests/board_test.rs`
- `crates/gitim-cli/src/commands/board.rs`

---

## Phase 1: Core Types

### Task 1.1: 新建 `CardStatus` + `CardMeta` in `types/card.rs`

**Files:**
- Create: `crates/gitim-core/src/types/card.rs`

- [ ] **Step 1**: 写失败测试（内联 `#[cfg(test)]`）

```rust
// crates/gitim-core/src/types/card.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CardStatus {
    Todo,
    Doing,
    Done,
}

impl CardStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            CardStatus::Todo => "todo",
            CardStatus::Doing => "doing",
            CardStatus::Done => "done",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "todo" => Ok(CardStatus::Todo),
            "doing" => Ok(CardStatus::Doing),
            "done" => Ok(CardStatus::Done),
            other => Err(format!("invalid status '{}', allowed: todo/doing/done", other)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CardMeta {
    pub title: String,
    pub channel: String,
    pub status: CardStatus,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub assignee: Option<String>,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
}

pub const MAX_LABELS: usize = 10;
pub const MAX_LABEL_LEN: usize = 32;

pub fn validate_label(label: &str) -> Result<(), String> {
    if label.is_empty() || label.len() > MAX_LABEL_LEN {
        return Err(format!("label length out of range (1..={})", MAX_LABEL_LEN));
    }
    for ch in label.chars() {
        if !matches!(ch, 'a'..='z' | '0'..='9' | '-' | '_') {
            return Err(format!("invalid char '{}' in label (allowed: a-z 0-9 - _)", ch));
        }
    }
    Ok(())
}

pub fn validate_labels(labels: &[String]) -> Result<(), String> {
    if labels.len() > MAX_LABELS {
        return Err(format!("too many labels (max {})", MAX_LABELS));
    }
    for l in labels {
        validate_label(l)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_parse_roundtrip() {
        assert_eq!(CardStatus::parse("todo").unwrap(), CardStatus::Todo);
        assert_eq!(CardStatus::parse("doing").unwrap(), CardStatus::Doing);
        assert_eq!(CardStatus::parse("done").unwrap(), CardStatus::Done);
        assert!(CardStatus::parse("backlog").is_err());
    }

    #[test]
    fn card_meta_yaml_roundtrip() {
        let meta = CardMeta {
            title: "Refactor cards".to_string(),
            channel: "backend".to_string(),
            status: CardStatus::Doing,
            labels: vec!["v2".to_string(), "agent-task".to_string()],
            assignee: Some("claude".to_string()),
            created_by: "lewis".to_string(),
            created_at: "20260417T120000Z".to_string(),
            updated_at: "20260417T120000Z".to_string(),
        };
        let yaml = serde_yaml::to_string(&meta).unwrap();
        let parsed: CardMeta = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(meta, parsed);
        assert!(yaml.contains("status: doing"));
    }

    #[test]
    fn card_meta_no_labels_no_assignee() {
        let yaml = "title: T\nchannel: c\nstatus: todo\ncreated_by: a\ncreated_at: '20260417T120000Z'\nupdated_at: '20260417T120000Z'\n";
        let parsed: CardMeta = serde_yaml::from_str(yaml).unwrap();
        assert!(parsed.labels.is_empty());
        assert!(parsed.assignee.is_none());
    }

    #[test]
    fn validate_label_ok() {
        assert!(validate_label("v2").is_ok());
        assert!(validate_label("agent-task").is_ok());
        assert!(validate_label("sprint_2").is_ok());
    }

    #[test]
    fn validate_label_rejects_uppercase() {
        assert!(validate_label("V2").is_err());
    }

    #[test]
    fn validate_label_rejects_too_long() {
        let too_long = "a".repeat(33);
        assert!(validate_label(&too_long).is_err());
    }

    #[test]
    fn validate_labels_rejects_too_many() {
        let many: Vec<String> = (0..11).map(|i| format!("l{}", i)).collect();
        assert!(validate_labels(&many).is_err());
    }
}
```

- [ ] **Step 2**: 跑测试

Run: `cargo test -p gitim-core --lib types::card`
Expected: 6 passing

- [ ] **Step 3**: Commit

```bash
git add crates/gitim-core/src/types/card.rs
git commit -m "feat(core): 新增 CardMeta 和 CardStatus 类型"
```

---

### Task 1.2: 在 `types/mod.rs` 导出 card，移除 board

**Files:**
- Modify: `crates/gitim-core/src/types/mod.rs`
- Delete: `crates/gitim-core/src/types/board.rs`

- [ ] **Step 1**: 改 `types/mod.rs`

完整新内容：

```rust
pub mod card;
pub mod channel;
pub mod handler;
pub mod link;
pub mod message;
pub mod meta;
pub mod config;

pub use card::{CardMeta, CardStatus, MAX_LABELS, MAX_LABEL_LEN, validate_label, validate_labels};
pub use channel::ChannelName;
pub use handler::Handler;
pub use link::{Link, LinkKind};
pub use message::{Message, ChannelEvent, ThreadEntry, ThreadLine, ThreadFile};
pub use meta::{UserMeta, ChannelMeta};
pub use config::Config;
```

- [ ] **Step 2**: 删除 `types/board.rs` 文件

Run: `rm crates/gitim-core/src/types/board.rs`

- [ ] **Step 3**: 跑测试验证 core 编译干净

Run: `cargo build -p gitim-core`
Expected: 0 errors, 0 warnings

Run: `cargo test -p gitim-core`
Expected: 所有既有测试通过（`board` 模块移除后无人再引用）

- [ ] **Step 4**: Commit

```bash
git add crates/gitim-core/src/types/mod.rs
git rm crates/gitim-core/src/types/board.rs
git commit -m "refactor(core): 删除 board 类型，types/mod 改导出 card"
```

---

## Phase 2: Daemon — API & Handlers

### Task 2.1: 改 `api.rs` — Request enum 删 board、改 card 方法签名；Event enum 重做

**Files:**
- Modify: `crates/gitim-daemon/src/api.rs`

- [ ] **Step 1**: 替换 Event enum 中的 Card 相关 variants，新增 `CardMessageAppended`

在 `#[serde(tag = "event")] pub enum Event` 中，替换既有的 `CardCreated` 和 `CardStatusChanged` 为：

```rust
#[serde(rename = "card_created")]
CardCreated {
    channel: String,
    card_id: String,
},

#[serde(rename = "card_status_changed")]
CardStatusChanged {
    channel: String,
    card_id: String,
    old_status: String,
    new_status: String,
    author: String,
},

#[serde(rename = "card_message_appended")]
CardMessageAppended {
    channel: String,
    card_id: String,
    line_numbers: Vec<u64>,
},
```

（注：事件里 `old_status`/`new_status` 仍用 `String` 即可——既有前端和 SSE 使用者更易消费，`CardStatus` enum 通过 serde 序列化也是小写字符串，语义一致。）

- [ ] **Step 2**: 替换 Request enum 的 card/board 方法

删除：`CreateBoard { .. }` 和 `ListBoards`。

用以下定义替换既有的 `CreateCard` / `ListCards` / `ReadCard` / `SendCardMessage` / `UpdateCard`：

```rust
#[serde(rename = "create_card")]
CreateCard {
    channel: String,
    title: String,
    #[serde(default)]
    labels: Option<Vec<String>>,
    #[serde(default)]
    assignee: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    author: Option<String>,
},
#[serde(rename = "list_cards")]
ListCards {
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    labels: Option<Vec<String>>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    assignee: Option<String>,
},
#[serde(rename = "read_card")]
ReadCard {
    channel: String,
    card_id: String,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    since: Option<u64>,
},
#[serde(rename = "send_card_message")]
SendCardMessage {
    channel: String,
    card_id: String,
    body: String,
    #[serde(default)]
    reply_to: Option<u64>,
    #[serde(default)]
    author: Option<String>,
},
#[serde(rename = "update_card")]
UpdateCard {
    channel: String,
    card_id: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    labels: Option<Vec<String>>,
    #[serde(default)]
    assignee: Option<String>,
    #[serde(default)]
    author: Option<String>,
},
```

- [ ] **Step 3**: 验证编译（api 本身无下游，编译通过 = Request/Event 语法正确）

Run: `cargo build -p gitim-daemon 2>&1 | head -40`
Expected: 会有大量编译错误（handlers/dispatcher 还在引用旧签名和 BoardMeta）。这是预期的——下一个 task 会修。**Do not commit yet.**

---

### Task 2.2: 新建 `card_handlers.rs` 实现所有 card handlers

**Files:**
- Create: `crates/gitim-daemon/src/card_handlers.rs`

- [ ] **Step 1**: 创建完整 `card_handlers.rs` 内容

```rust
use crate::api::{Event, Response};
use crate::state::{PendingMessage, PushResult, SharedState};
use crate::thread_io;
use gitim_core::types::{
    validate_labels, CardMeta, CardStatus, ChannelName, Handler,
};
use gitim_sync::git::GitError;
use tracing::{info, warn};

const MAX_PUSH_RETRIES: u32 = 3;

fn validate_card_id(card_id: &str) -> Result<(), String> {
    if card_id.is_empty() || card_id.len() > 20 {
        return Err("card_id length out of range".into());
    }
    for ch in card_id.chars() {
        if !matches!(ch, '0'..='9' | 'a'..='f' | '-') {
            return Err(format!("invalid character in card_id: '{}'", ch));
        }
    }
    Ok(())
}

fn generate_card_id() -> String {
    let now = chrono::Utc::now();
    let ts = now.format("%Y%m%d-%H%M%S").to_string();
    let rand_hex = format!("{:03x}", rand::random::<u16>() & 0xFFF);
    format!("{}-{}", ts, rand_hex)
}

fn channel_thread_exists(state: &SharedState, channel: &ChannelName) -> bool {
    let p = state
        .repo_root
        .join("channels")
        .join(format!("{}.thread", channel));
    p.exists()
}

async fn ensure_known_user(state: &SharedState, handler: &str) -> Result<(), String> {
    let users = state.users.read().await;
    if !users.contains(&handler.to_string()) {
        return Err(format!("unknown user: {}", handler));
    }
    Ok(())
}

async fn push_with_retry(state: &SharedState, op: &str) -> Result<(), String> {
    if !state.git_storage.has_remote() {
        return Ok(());
    }
    for attempt in 1..=MAX_PUSH_RETRIES {
        match state.git_storage.push() {
            Ok(()) => return Ok(()),
            Err(GitError::PushConflict) => {
                warn!(
                    "{}: push conflict (attempt {}/{}), rebasing",
                    op, attempt, MAX_PUSH_RETRIES
                );
                state
                    .git_storage
                    .fetch()
                    .map_err(|e| format!("{} fetch failed: {}", op, e))?;
                state
                    .git_storage
                    .rebase_onto_origin()
                    .map_err(|e| format!("{} rebase failed: {}", op, e))?;
            }
            Err(e) => return Err(format!("{} push failed: {}", op, e)),
        }
    }
    Err(format!(
        "{}: push still conflicting after {} retries",
        op, MAX_PUSH_RETRIES
    ))
}

pub async fn handle_create_card(
    state: SharedState,
    channel: String,
    title: String,
    labels: Option<Vec<String>>,
    assignee: Option<String>,
    status: Option<String>,
    author: String,
) -> Response {
    let _h = match Handler::new(&author) {
        Ok(h) => h,
        Err(e) => return Response::error(format!("invalid author: {}", e)),
    };
    if let Err(e) = ensure_known_user(&state, &author).await {
        return Response::error(e);
    }

    let ch_name = match ChannelName::new(&channel) {
        Ok(n) => n,
        Err(e) => return Response::error(format!("invalid channel name: {}", e)),
    };
    if !channel_thread_exists(&state, &ch_name) {
        return Response::error(format!("channel '{}' does not exist", channel));
    }

    let labels_vec = labels.unwrap_or_default();
    if let Err(e) = validate_labels(&labels_vec) {
        return Response::error(format!("invalid labels: {}", e));
    }

    let status_parsed = match status.as_deref() {
        None => CardStatus::Todo,
        Some(s) => match CardStatus::parse(s) {
            Ok(v) => v,
            Err(e) => return Response::error(e),
        },
    };

    if let Some(ref a) = assignee {
        if let Err(e) = ensure_known_user(&state, a).await {
            return Response::error(format!("assignee invalid: {}", e));
        }
    }

    if title.trim().is_empty() {
        return Response::error("title cannot be empty");
    }

    let card_id = generate_card_id();
    let card_dir = state
        .repo_root
        .join("channels")
        .join(ch_name.to_string())
        .join("cards")
        .join(&card_id);
    if let Err(e) = std::fs::create_dir_all(&card_dir) {
        return Response::error(format!("failed to create card dir: {}", e));
    }

    let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let meta = CardMeta {
        title: title.clone(),
        channel: ch_name.to_string(),
        status: status_parsed,
        labels: labels_vec,
        assignee,
        created_by: author.clone(),
        created_at: now.clone(),
        updated_at: now,
    };
    let meta_str = serde_yaml::to_string(&meta).unwrap();
    let meta_rel = format!(
        "channels/{}/cards/{}/card.meta.yaml",
        ch_name, card_id
    );
    let thread_rel = format!(
        "channels/{}/cards/{}/discussion.thread",
        ch_name, card_id
    );
    if let Err(e) = std::fs::write(card_dir.join("card.meta.yaml"), &meta_str) {
        return Response::error(format!("failed to write card meta: {}", e));
    }
    if let Err(e) = std::fs::write(card_dir.join("discussion.thread"), "") {
        return Response::error(format!("failed to write card thread: {}", e));
    }

    let commit_msg = format!(
        "card: create {} in {} by @{}",
        card_id, channel, author
    );
    if let Err(e) = state
        .git_storage
        .add_and_commit_as(&[&meta_rel, &thread_rel], &commit_msg, Some(&author))
    {
        return Response::error(format!("create_card commit failed: {}", e));
    }

    if let Err(e) = push_with_retry(&state, "create_card").await {
        return Response::error(e);
    }

    let _ = state.event_tx.send(Event::CardCreated {
        channel: ch_name.to_string(),
        card_id: card_id.clone(),
    });

    info!("card '{}' created in channel '{}' by @{}", card_id, channel, author);

    Response::success(serde_json::json!({
        "channel": ch_name.to_string(),
        "card_id": card_id,
        "title": title,
    }))
}

pub async fn handle_list_cards(
    state: SharedState,
    channel: Option<String>,
    labels: Option<Vec<String>>,
    status: Option<String>,
    assignee: Option<String>,
) -> Response {
    let status_filter = match status {
        None => None,
        Some(s) => match CardStatus::parse(&s) {
            Ok(v) => Some(v),
            Err(e) => return Response::error(e),
        },
    };

    let label_filter = labels.unwrap_or_default();
    let channels_to_scan: Vec<String> = match channel {
        Some(c) => {
            let name = match ChannelName::new(&c) {
                Ok(n) => n,
                Err(e) => return Response::error(format!("invalid channel name: {}", e)),
            };
            vec![name.to_string()]
        }
        None => {
            let channels_dir = state.repo_root.join("channels");
            let mut names = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&channels_dir) {
                for entry in entries.flatten() {
                    if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        names.push(entry.file_name().to_string_lossy().to_string());
                    }
                }
            }
            names
        }
    };

    let mut cards: Vec<serde_json::Value> = Vec::new();
    for ch in &channels_to_scan {
        let cards_dir = state.repo_root.join("channels").join(ch).join("cards");
        if !cards_dir.exists() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&cards_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let meta_path = entry.path().join("card.meta.yaml");
            let Ok(content) = std::fs::read_to_string(&meta_path) else {
                continue;
            };
            let Ok(meta) = serde_yaml::from_str::<CardMeta>(&content) else {
                continue;
            };
            if let Some(ref s) = status_filter {
                if meta.status != *s {
                    continue;
                }
            }
            if !label_filter.is_empty() {
                let all_match = label_filter.iter().all(|l| meta.labels.contains(l));
                if !all_match {
                    continue;
                }
            }
            if let Some(ref a) = assignee {
                if meta.assignee.as_deref() != Some(a.as_str()) {
                    continue;
                }
            }
            let card_id = entry.file_name().to_string_lossy().to_string();
            cards.push(serde_json::json!({
                "card_id": card_id,
                "channel": meta.channel,
                "title": meta.title,
                "status": meta.status.as_str(),
                "labels": meta.labels,
                "assignee": meta.assignee,
                "created_by": meta.created_by,
                "created_at": meta.created_at,
                "updated_at": meta.updated_at,
            }));
        }
    }

    cards.sort_by(|a, b| {
        let ca = a["channel"].as_str().unwrap_or("");
        let cb = b["channel"].as_str().unwrap_or("");
        ca.cmp(cb)
            .then(a["card_id"].as_str().unwrap_or("").cmp(b["card_id"].as_str().unwrap_or("")))
    });
    Response::success(serde_json::json!({ "cards": cards }))
}

pub async fn handle_read_card(
    state: SharedState,
    channel: String,
    card_id: String,
    limit: Option<usize>,
    since: Option<u64>,
) -> Response {
    let ch_name = match ChannelName::new(&channel) {
        Ok(n) => n,
        Err(e) => return Response::error(format!("invalid channel name: {}", e)),
    };
    if let Err(e) = validate_card_id(&card_id) {
        return Response::error(format!("invalid card_id: {}", e));
    }
    let card_dir = state
        .repo_root
        .join("channels")
        .join(ch_name.to_string())
        .join("cards")
        .join(&card_id);
    let meta_path = card_dir.join("card.meta.yaml");
    let meta: CardMeta = match std::fs::read_to_string(&meta_path) {
        Ok(c) => match serde_yaml::from_str(&c) {
            Ok(m) => m,
            Err(e) => return Response::error(format!("failed to parse card meta: {}", e)),
        },
        Err(_) => {
            return Response::error(format!(
                "card '{}' not found in channel '{}'",
                card_id, channel
            ))
        }
    };
    let thread_path = card_dir.join("discussion.thread");
    let entries = match thread_io::read_thread_entries(&thread_path, limit, since) {
        Ok(e) => e,
        Err(e) => return Response::error(e),
    };
    Response::success(serde_json::json!({
        "channel": ch_name.to_string(),
        "card_id": card_id,
        "meta": {
            "title": meta.title,
            "status": meta.status.as_str(),
            "labels": meta.labels,
            "assignee": meta.assignee,
            "created_by": meta.created_by,
            "created_at": meta.created_at,
            "updated_at": meta.updated_at,
        },
        "entries": entries,
    }))
}

pub async fn handle_send_card_message(
    state: SharedState,
    channel: String,
    card_id: String,
    body: String,
    reply_to: Option<u64>,
    author: String,
) -> Response {
    let handler = match Handler::new(&author) {
        Ok(h) => h,
        Err(e) => return Response::error(format!("invalid author: {}", e)),
    };
    if let Err(e) = ensure_known_user(&state, &author).await {
        return Response::error(e);
    }
    let ch_name = match ChannelName::new(&channel) {
        Ok(n) => n,
        Err(e) => return Response::error(format!("invalid channel name: {}", e)),
    };
    if let Err(e) = validate_card_id(&card_id) {
        return Response::error(format!("invalid card_id: {}", e));
    }
    let card_dir = state
        .repo_root
        .join("channels")
        .join(ch_name.to_string())
        .join("cards")
        .join(&card_id);
    let meta_path = card_dir.join("card.meta.yaml");
    if !meta_path.exists() {
        return Response::error(format!(
            "card '{}' not found in channel '{}'",
            card_id, channel
        ));
    }
    let thread_path = card_dir.join("discussion.thread");
    let (next_line, _new_content) =
        match thread_io::append_message_to_thread(&thread_path, &handler, &body, reply_to) {
            Ok(v) => v,
            Err(e) => return Response::error(e),
        };

    let thread_rel = format!(
        "channels/{}/cards/{}/discussion.thread",
        ch_name, card_id
    );
    // Use card-thread-scoped channel identifier for pending_push key
    let channel_key = format!("channels/{}/cards/{}", ch_name, card_id);
    let commit_msg = format!(
        "card-msg: @{} -> {}/{} L{:06}",
        author, ch_name, card_id, next_line
    );
    let commit_status = match state
        .git_storage
        .add_and_commit_as(&[&thread_rel], &commit_msg, Some(&author))
    {
        Ok(()) => "committed",
        Err(e) => {
            warn!(
                "git commit failed for L{:06} in {}/{}: {}",
                next_line, ch_name, card_id, e
            );
            "written"
        }
    };

    let should_await_push =
        state.has_remote && state.sync_started.load(std::sync::atomic::Ordering::SeqCst);
    let push_rx = if should_await_push {
        let (tx, rx) = tokio::sync::oneshot::channel::<PushResult>();
        {
            let mut pending = state.pending_push.write().unwrap();
            pending.push(PendingMessage {
                channel: channel_key.clone(),
                line_number: next_line,
                result_tx: Some(tx),
            });
        }
        Some(rx)
    } else {
        {
            let mut pending = state.pending_push.write().unwrap();
            pending.push(PendingMessage {
                channel: channel_key.clone(),
                line_number: next_line,
                result_tx: None,
            });
        }
        None
    };

    let _ = state.event_tx.send(Event::CardMessageAppended {
        channel: ch_name.to_string(),
        card_id: card_id.clone(),
        line_numbers: vec![next_line],
    });

    info!(
        "card message sent to {}/{} by @{} at L{:06}",
        ch_name, card_id, author, next_line
    );

    if let Some(rx) = push_rx {
        state.push_notify.notify_one();
        match rx.await {
            Ok(PushResult::Pushed { commit_id }) => Response::success(serde_json::json!({
                "line_number": next_line,
                "channel": ch_name.to_string(),
                "card_id": card_id,
                "status": "pushed",
                "commit_id": commit_id,
            })),
            Ok(PushResult::Failed { reason }) => Response::success(serde_json::json!({
                "line_number": next_line,
                "channel": ch_name.to_string(),
                "card_id": card_id,
                "status": "commit_only",
                "error": reason,
            })),
            Err(_) => Response::success(serde_json::json!({
                "line_number": next_line,
                "channel": ch_name.to_string(),
                "card_id": card_id,
                "status": "commit_only",
                "error": "push result channel closed",
            })),
        }
    } else {
        Response::success(serde_json::json!({
            "line_number": next_line,
            "channel": ch_name.to_string(),
            "card_id": card_id,
            "status": commit_status,
        }))
    }
}

pub async fn handle_update_card(
    state: SharedState,
    channel: String,
    card_id: String,
    status: Option<String>,
    labels: Option<Vec<String>>,
    assignee: Option<String>,
    author: String,
) -> Response {
    let _h = match Handler::new(&author) {
        Ok(h) => h,
        Err(e) => return Response::error(format!("invalid author: {}", e)),
    };
    if let Err(e) = ensure_known_user(&state, &author).await {
        return Response::error(e);
    }
    let ch_name = match ChannelName::new(&channel) {
        Ok(n) => n,
        Err(e) => return Response::error(format!("invalid channel name: {}", e)),
    };
    if let Err(e) = validate_card_id(&card_id) {
        return Response::error(format!("invalid card_id: {}", e));
    }
    if status.is_none() && labels.is_none() && assignee.is_none() {
        return Response::error("must provide at least one field to update");
    }

    let card_dir = state
        .repo_root
        .join("channels")
        .join(ch_name.to_string())
        .join("cards")
        .join(&card_id);
    let meta_path = card_dir.join("card.meta.yaml");
    let mut meta: CardMeta = match std::fs::read_to_string(&meta_path) {
        Ok(c) => match serde_yaml::from_str(&c) {
            Ok(m) => m,
            Err(e) => return Response::error(format!("failed to parse card meta: {}", e)),
        },
        Err(_) => {
            return Response::error(format!(
                "card '{}' not found in channel '{}'",
                card_id, channel
            ))
        }
    };

    let old_status = meta.status.clone();
    if let Some(ref s) = status {
        match CardStatus::parse(s) {
            Ok(v) => meta.status = v,
            Err(e) => return Response::error(e),
        }
    }
    if let Some(ref new_labels) = labels {
        if let Err(e) = validate_labels(new_labels) {
            return Response::error(format!("invalid labels: {}", e));
        }
        meta.labels = new_labels.clone();
    }
    if let Some(ref a) = assignee {
        if let Err(e) = ensure_known_user(&state, a).await {
            return Response::error(format!("assignee invalid: {}", e));
        }
        meta.assignee = Some(a.clone());
    }

    meta.updated_at = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let meta_str = serde_yaml::to_string(&meta).unwrap();
    if let Err(e) = std::fs::write(&meta_path, &meta_str) {
        return Response::error(format!("failed to write card meta: {}", e));
    }

    let meta_rel = format!(
        "channels/{}/cards/{}/card.meta.yaml",
        ch_name, card_id
    );
    let commit_msg = format!(
        "card: update {} in {} by @{}",
        card_id, channel, author
    );
    if let Err(e) = state
        .git_storage
        .add_and_commit_as(&[&meta_rel], &commit_msg, Some(&author))
    {
        return Response::error(format!("update_card commit failed: {}", e));
    }

    if let Err(e) = push_with_retry(&state, "update_card").await {
        return Response::error(e);
    }

    if let Some(ref _s) = status {
        if old_status != meta.status {
            let _ = state.event_tx.send(Event::CardStatusChanged {
                channel: ch_name.to_string(),
                card_id: card_id.clone(),
                old_status: old_status.as_str().to_string(),
                new_status: meta.status.as_str().to_string(),
                author: author.clone(),
            });
        }
    }

    info!("card '{}' updated in channel '{}' by @{}", card_id, channel, author);

    Response::success(serde_json::json!({
        "channel": ch_name.to_string(),
        "card_id": card_id,
        "status": meta.status.as_str(),
        "labels": meta.labels,
        "assignee": meta.assignee,
    }))
}
```

- [ ] **Step 2**: 在 `crates/gitim-daemon/src/lib.rs` 替换 `pub mod board_handlers;` 为 `pub mod card_handlers;`

- [ ] **Step 3**: 删除 `crates/gitim-daemon/src/board_handlers.rs`

Run: `rm crates/gitim-daemon/src/board_handlers.rs`

- [ ] **Step 4**: 编译暂不过（handlers.rs 还引用旧方法和 CreateBoard）。下一 task 修 dispatcher。

---

### Task 2.3: 改 `handlers.rs` dispatcher + guest-mode guard

**Files:**
- Modify: `crates/gitim-daemon/src/handlers.rs`

- [ ] **Step 1**: 替换 guest-mode guard 的 write 检测（line 51-63）

把 `Request::CreateBoard { .. } |` 整行删掉。其他 card 相关写操作保持。最终：

```rust
let is_write = matches!(
    req,
    Request::Send { .. }
        | Request::RegisterUser { .. }
        | Request::JoinChannel { .. }
        | Request::LeaveChannel { .. }
        | Request::CreateChannel { .. }
        | Request::ArchiveChannel { .. }
        | Request::CreateCard { .. }
        | Request::SendCardMessage { .. }
        | Request::UpdateCard { .. }
);
```

- [ ] **Step 2**: 替换 dispatcher 的 board/card 分支（当前在 line 168-259）

删除 `Request::CreateBoard { .. }` 和 `Request::ListBoards` 两个分支。

替换既有 `Request::CreateCard` / `Request::ListCards` / `Request::ReadCard` / `Request::SendCardMessage` / `Request::UpdateCard` 五个分支为：

```rust
Request::CreateCard {
    channel,
    title,
    labels,
    assignee,
    status,
    author,
} => {
    let resolved_author = match resolve_author(author, &state).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    crate::card_handlers::handle_create_card(
        state, channel, title, labels, assignee, status, resolved_author,
    )
    .await
}
Request::ListCards { channel, labels, status, assignee } => {
    crate::card_handlers::handle_list_cards(state, channel, labels, status, assignee).await
}
Request::ReadCard {
    channel,
    card_id,
    limit,
    since,
} => crate::card_handlers::handle_read_card(state, channel, card_id, limit, since).await,
Request::SendCardMessage {
    channel,
    card_id,
    body,
    reply_to,
    author,
} => {
    let resolved_author = match resolve_author(author, &state).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    crate::card_handlers::handle_send_card_message(
        state, channel, card_id, body, reply_to, resolved_author,
    )
    .await
}
Request::UpdateCard {
    channel,
    card_id,
    status,
    labels,
    assignee,
    author,
} => {
    let resolved_author = match resolve_author(author, &state).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    crate::card_handlers::handle_update_card(
        state, channel, card_id, status, labels, assignee, resolved_author,
    )
    .await
}
```

- [ ] **Step 3**: 跑编译检查

Run: `cargo build -p gitim-daemon 2>&1 | tail -20`
Expected: 0 errors（tests 还可能 fail，因为 board_test.rs 还在）

- [ ] **Step 4**: 删除旧 tests 文件

Run: `rm crates/gitim-daemon/tests/board_test.rs`

- [ ] **Step 5**: Commit（api + handlers + card_handlers + 删 board_handlers/board_test 的全套改动）

```bash
git add crates/gitim-daemon/src/api.rs \
        crates/gitim-daemon/src/handlers.rs \
        crates/gitim-daemon/src/lib.rs \
        crates/gitim-daemon/src/card_handlers.rs
git rm crates/gitim-daemon/src/board_handlers.rs \
       crates/gitim-daemon/tests/board_test.rs
git commit -m "refactor(daemon): card_handlers 替换 board_handlers，IPC 改 channel 归属"
```

---

### Task 2.4: 新写 `tests/card_test.rs`

**Files:**
- Create: `crates/gitim-daemon/tests/card_test.rs`

- [ ] **Step 1**: 写完整测试文件

```rust
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::broadcast;

use gitim_core::types::Config;
use gitim_daemon::api::Request;
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::AppState;

fn make_config() -> Config {
    serde_yaml::from_str("version: 1").unwrap()
}

async fn setup_test_repo() -> (TempDir, Arc<AppState>) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    std::fs::create_dir_all(root.join("users")).unwrap();
    std::fs::create_dir_all(root.join("channels")).unwrap();
    std::fs::write(
        root.join("users/alice.meta.yaml"),
        "display_name: Alice\nrole: dev\nintroduction: hi\n",
    )
    .unwrap();
    std::fs::write(
        root.join("users/bob.meta.yaml"),
        "display_name: Bob\nrole: dev\nintroduction: hello\n",
    )
    .unwrap();
    // Pre-create a channel thread so card creation can pass the channel-exists check
    std::fs::write(root.join("channels/dev.thread"), "").unwrap();
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(&root)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(&root)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args([
            "-c", "user.name=Test", "-c", "user.email=test@test.com",
            "commit", "-m", "init",
        ])
        .current_dir(&root)
        .output()
        .unwrap();
    let (tx, _) = broadcast::channel(100);
    let state = Arc::new(AppState::new(
        root,
        make_config(),
        tx,
        Some("alice".to_string()),
    ));
    {
        let mut users = state.users.write().await;
        *users = vec!["alice".to_string(), "bob".to_string()];
    }
    (tmp, state)
}

async fn create_card(
    state: Arc<AppState>,
    channel: &str,
    title: &str,
) -> (gitim_daemon::api::Response, Option<String>) {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_card",
        "channel": channel,
        "title": title,
        "author": "alice",
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    let card_id = resp
        .data
        .as_ref()
        .and_then(|d| d["card_id"].as_str())
        .map(|s| s.to_string());
    (resp, card_id)
}

#[tokio::test]
async fn test_create_card_happy_path() {
    let (_t, state) = setup_test_repo().await;
    let (resp, card_id) = create_card(state.clone(), "dev", "Implement X").await;
    assert!(resp.ok, "create should succeed: {:?}", resp.error);
    let card_id = card_id.unwrap();
    let meta_path = state
        .repo_root
        .join("channels/dev/cards")
        .join(&card_id)
        .join("card.meta.yaml");
    assert!(meta_path.exists());
    let content = std::fs::read_to_string(&meta_path).unwrap();
    assert!(content.contains("status: todo"));
    assert!(content.contains("channel: dev"));
}

#[tokio::test]
async fn test_create_card_channel_missing() {
    let (_t, state) = setup_test_repo().await;
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_card",
        "channel": "ghost",
        "title": "T",
        "author": "alice",
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("does not exist"));
}

#[tokio::test]
async fn test_create_card_invalid_status() {
    let (_t, state) = setup_test_repo().await;
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_card",
        "channel": "dev",
        "title": "T",
        "status": "review",
        "author": "alice",
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("invalid status"));
}

#[tokio::test]
async fn test_create_card_with_labels() {
    let (_t, state) = setup_test_repo().await;
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_card",
        "channel": "dev",
        "title": "T",
        "labels": ["v2", "agent-task"],
        "author": "alice",
    }))
    .unwrap();
    let resp = handle_request(req, state.clone()).await;
    assert!(resp.ok);
    let card_id = resp.data.as_ref().unwrap()["card_id"].as_str().unwrap().to_string();
    let content = std::fs::read_to_string(
        state.repo_root.join("channels/dev/cards").join(&card_id).join("card.meta.yaml"),
    )
    .unwrap();
    assert!(content.contains("v2"));
    assert!(content.contains("agent-task"));
}

#[tokio::test]
async fn test_create_card_too_many_labels() {
    let (_t, state) = setup_test_repo().await;
    let labels: Vec<String> = (0..11).map(|i| format!("l{}", i)).collect();
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_card",
        "channel": "dev",
        "title": "T",
        "labels": labels,
        "author": "alice",
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("too many labels"));
}

#[tokio::test]
async fn test_list_cards_empty() {
    let (_t, state) = setup_test_repo().await;
    let req: Request = serde_json::from_value(serde_json::json!({"method": "list_cards"})).unwrap();
    let resp = handle_request(req, state).await;
    assert!(resp.ok);
    assert_eq!(resp.data.unwrap()["cards"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_list_cards_filter_by_channel() {
    let (_t, state) = setup_test_repo().await;
    // Create another channel
    std::fs::write(state.repo_root.join("channels/docs.thread"), "").unwrap();
    let (_, _) = create_card(state.clone(), "dev", "A").await;
    let (_, _) = create_card(state.clone(), "docs", "B").await;
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "list_cards",
        "channel": "dev",
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(resp.ok);
    let cards = resp.data.unwrap()["cards"].as_array().unwrap().clone();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0]["title"].as_str().unwrap(), "A");
}

#[tokio::test]
async fn test_list_cards_filter_by_status() {
    let (_t, state) = setup_test_repo().await;
    let (_, id_a) = create_card(state.clone(), "dev", "A").await;
    let (_, _) = create_card(state.clone(), "dev", "B").await;
    // Move A to doing
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "update_card",
        "channel": "dev",
        "card_id": id_a.unwrap(),
        "status": "doing",
        "author": "alice",
    }))
    .unwrap();
    let resp = handle_request(req, state.clone()).await;
    assert!(resp.ok);
    let req2: Request = serde_json::from_value(serde_json::json!({
        "method": "list_cards",
        "status": "doing",
    }))
    .unwrap();
    let resp2 = handle_request(req2, state).await;
    assert!(resp2.ok);
    let cards = resp2.data.unwrap()["cards"].as_array().unwrap().clone();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0]["title"].as_str().unwrap(), "A");
}

#[tokio::test]
async fn test_update_card_status_and_emit_event() {
    let (_t, state) = setup_test_repo().await;
    let mut rx = state.event_tx.subscribe();
    let (_, card_id) = create_card(state.clone(), "dev", "T").await;
    let id = card_id.unwrap();
    // Consume CardCreated
    let _ = rx.recv().await;

    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "update_card",
        "channel": "dev",
        "card_id": id,
        "status": "done",
        "author": "alice",
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(resp.ok);

    let ev = rx.recv().await.unwrap();
    match ev {
        gitim_daemon::api::Event::CardStatusChanged { old_status, new_status, .. } => {
            assert_eq!(old_status, "todo");
            assert_eq!(new_status, "done");
        }
        other => panic!("unexpected event: {:?}", other),
    }
}

#[tokio::test]
async fn test_send_card_message_emits_event() {
    let (_t, state) = setup_test_repo().await;
    let mut rx = state.event_tx.subscribe();
    let (_, card_id) = create_card(state.clone(), "dev", "T").await;
    let id = card_id.unwrap();
    let _ = rx.recv().await;

    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "send_card_message",
        "channel": "dev",
        "card_id": id,
        "body": "started work",
        "author": "alice",
    }))
    .unwrap();
    let resp = handle_request(req, state).await;
    assert!(resp.ok);

    let ev = rx.recv().await.unwrap();
    match ev {
        gitim_daemon::api::Event::CardMessageAppended { line_numbers, .. } => {
            assert_eq!(line_numbers, vec![1]);
        }
        other => panic!("unexpected event: {:?}", other),
    }
}

#[tokio::test]
async fn test_read_card_roundtrip() {
    let (_t, state) = setup_test_repo().await;
    let (_, card_id) = create_card(state.clone(), "dev", "T").await;
    let id = card_id.unwrap();
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "send_card_message",
        "channel": "dev",
        "card_id": id.clone(),
        "body": "progress line",
        "author": "bob",
    }))
    .unwrap();
    let _ = handle_request(req, state.clone()).await;

    let req2: Request = serde_json::from_value(serde_json::json!({
        "method": "read_card",
        "channel": "dev",
        "card_id": id,
    }))
    .unwrap();
    let resp = handle_request(req2, state).await;
    assert!(resp.ok);
    let data = resp.data.unwrap();
    assert_eq!(data["meta"]["title"].as_str().unwrap(), "T");
    let entries = data["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
}
```

- [ ] **Step 2**: 跑测试

Run: `cargo test -p gitim-daemon --test card_test`
Expected: 全部 11 个测试通过

- [ ] **Step 3**: 跑 daemon 全量测试确保没有回归

Run: `cargo test -p gitim-daemon`
Expected: 所有既有 daemon 测试 + 新 card_test 通过

- [ ] **Step 4**: Commit

```bash
git add crates/gitim-daemon/tests/card_test.rs
git commit -m "test(daemon): card_test 覆盖 CRUD + 事件 + 过滤"
```

---

## Phase 3: Client Library

### Task 3.1: 改 `gitim-client` 方法签名

**Files:**
- Modify: `crates/gitim-client/src/client.rs` (lines 255-366 — board/card methods block)

- [ ] **Step 1**: 删除 `create_board` 方法（255-270 行）和 `list_boards` 方法（291-293 行）

- [ ] **Step 2**: 替换其余 5 个方法

把 `create_card`/`list_cards`/`read_card`/`send_card_message`/`update_card` 这五个方法替换为：

```rust
pub async fn create_card(
    &self,
    channel: &str,
    title: &str,
    labels: Option<&[String]>,
    assignee: Option<&str>,
    status: Option<&str>,
) -> Result<ApiResponse, ClientError> {
    self.request(
        "create_card",
        json!({
            "channel": channel,
            "title": title,
            "labels": labels,
            "assignee": assignee,
            "status": status,
        }),
    )
    .await
}

pub async fn list_cards(
    &self,
    channel: Option<&str>,
    labels: Option<&[String]>,
    status: Option<&str>,
    assignee: Option<&str>,
) -> Result<ApiResponse, ClientError> {
    self.request(
        "list_cards",
        json!({
            "channel": channel,
            "labels": labels,
            "status": status,
            "assignee": assignee,
        }),
    )
    .await
}

pub async fn read_card(
    &self,
    channel: &str,
    card_id: &str,
    limit: Option<u64>,
    since: Option<u64>,
) -> Result<ApiResponse, ClientError> {
    self.request(
        "read_card",
        json!({
            "channel": channel,
            "card_id": card_id,
            "limit": limit,
            "since": since,
        }),
    )
    .await
}

pub async fn send_card_message(
    &self,
    channel: &str,
    card_id: &str,
    body: &str,
    reply_to: Option<u64>,
) -> Result<ApiResponse, ClientError> {
    self.request(
        "send_card_message",
        json!({
            "channel": channel,
            "card_id": card_id,
            "body": body,
            "reply_to": reply_to,
        }),
    )
    .await
}

pub async fn update_card(
    &self,
    channel: &str,
    card_id: &str,
    status: Option<&str>,
    labels: Option<&[String]>,
    assignee: Option<&str>,
) -> Result<ApiResponse, ClientError> {
    self.request(
        "update_card",
        json!({
            "channel": channel,
            "card_id": card_id,
            "status": status,
            "labels": labels,
            "assignee": assignee,
        }),
    )
    .await
}
```

- [ ] **Step 2**: 编译 + 测试

Run: `cargo build -p gitim-client`
Expected: 0 errors

Run: `cargo test -p gitim-client`
Expected: 所有既有测试通过（如有），否则空 OK

- [ ] **Step 3**: Commit

```bash
git add crates/gitim-client/src/client.rs
git commit -m "refactor(client): card 方法改 channel 归属 + labels 参数"
```

---

## Phase 4: CLI

### Task 4.1: 删 Board 子命令，改 Card 子命令

**Files:**
- Modify: `crates/gitim-cli/src/main.rs`

- [ ] **Step 1**: 删除 `BoardCommands` enum（218-233 行）

- [ ] **Step 2**: 从 `Commands` enum 删除 `Board { .. }` variant（169-173 行）

- [ ] **Step 3**: 替换 `CardCommands` enum（236-300 行）为新版本

```rust
#[derive(Subcommand)]
enum CardCommands {
    /// Create a new card in a channel
    Create {
        /// Channel name
        channel: String,
        /// Card title
        title: String,
        /// Labels (repeatable)
        #[arg(short, long)]
        label: Vec<String>,
        /// Assignee handler
        #[arg(long)]
        assignee: Option<String>,
        /// Initial status (todo/doing/done)
        #[arg(long)]
        status: Option<String>,
    },

    /// List cards with optional filters
    Ls {
        /// Filter by channel
        #[arg(short, long)]
        channel: Option<String>,
        /// Filter by label (repeatable; all must match)
        #[arg(short, long)]
        label: Vec<String>,
        /// Filter by status
        #[arg(long)]
        status: Option<String>,
        /// Filter by assignee handler
        #[arg(long)]
        assignee: Option<String>,
    },

    /// Read card discussion
    Read {
        /// Channel name
        channel: String,
        /// Card ID
        card_id: String,
        /// Maximum number of entries
        #[arg(short, long)]
        limit: Option<u64>,
        /// Only return entries after this line number
        #[arg(short, long)]
        since: Option<u64>,
    },

    /// Comment on a card
    Comment {
        /// Channel name
        channel: String,
        /// Card ID
        card_id: String,
        /// Message body
        body: String,
        /// Line number to reply to
        #[arg(short, long)]
        reply_to: Option<u64>,
    },

    /// Update card status / labels / assignee
    Update {
        /// Channel name
        channel: String,
        /// Card ID
        card_id: String,
        /// New status
        #[arg(long)]
        status: Option<String>,
        /// Replace labels (repeatable, pass none to clear — use `--label-clear`)
        #[arg(short, long)]
        label: Vec<String>,
        /// Clear labels (if set, ignore --label)
        #[arg(long)]
        label_clear: bool,
        /// New assignee handler
        #[arg(long)]
        assignee: Option<String>,
    },
}
```

- [ ] **Step 4**: 替换 main.rs 的 dispatch

删除 `Commands::Board { command }` 整块 match arm（461-479 行）。

替换 `Commands::Card { command }` 整块 match arm（480-540 行）为：

```rust
Commands::Card { command } => match command {
    CardCommands::Create {
        channel,
        title,
        label,
        assignee,
        status,
    } => {
        commands::card::cmd_create_card(
            &client, &mode, &channel, &title,
            if label.is_empty() { None } else { Some(&label) },
            assignee.as_deref(),
            status.as_deref(),
        )
        .await
    }
    CardCommands::Ls {
        channel,
        label,
        status,
        assignee,
    } => {
        commands::card::cmd_list_cards(
            &client, &mode,
            channel.as_deref(),
            if label.is_empty() { None } else { Some(&label) },
            status.as_deref(),
            assignee.as_deref(),
        )
        .await
    }
    CardCommands::Read {
        channel,
        card_id,
        limit,
        since,
    } => {
        commands::card::cmd_read_card(&client, &mode, &channel, &card_id, limit, since).await
    }
    CardCommands::Comment {
        channel,
        card_id,
        body,
        reply_to,
    } => {
        commands::card::cmd_send_card_message(
            &client, &mode, &channel, &card_id, &body, reply_to,
        )
        .await
    }
    CardCommands::Update {
        channel,
        card_id,
        status,
        label,
        label_clear,
        assignee,
    } => {
        let labels_param: Option<Vec<String>> = if label_clear {
            Some(Vec::new())
        } else if !label.is_empty() {
            Some(label)
        } else {
            None
        };
        commands::card::cmd_update_card(
            &client, &mode, &channel, &card_id,
            status.as_deref(),
            labels_param.as_deref(),
            assignee.as_deref(),
        )
        .await
    }
},
```

---

### Task 4.2: 改 `commands/card.rs` 实现

**Files:**
- Modify: `crates/gitim-cli/src/commands/card.rs`
- Delete: `crates/gitim-cli/src/commands/board.rs`
- Modify: `crates/gitim-cli/src/commands/mod.rs`

- [ ] **Step 1**: 删除 `commands/board.rs`

Run: `rm crates/gitim-cli/src/commands/board.rs`

- [ ] **Step 2**: 改 `commands/mod.rs` 删除 `pub mod board;` 行

- [ ] **Step 3**: 重写 `commands/card.rs`

```rust
#![deny(warnings)]

use std::process;

use gitim_client::GitimClient;

use crate::output::OutputMode;

fn print_or_exit(resp: gitim_client::ApiResponse, mode: &OutputMode, human_success: impl FnOnce(&serde_json::Value)) {
    if !resp.ok {
        eprintln!("Error: {}", resp.error.as_deref().unwrap_or("unknown"));
        process::exit(1);
    }
    match mode {
        OutputMode::Human => {
            if let Some(d) = &resp.data {
                human_success(d);
            }
        }
        OutputMode::Json => {
            let data = resp.data.unwrap_or(serde_json::Value::Null);
            match serde_json::to_string(&data) {
                Ok(s) => println!("{s}"),
                Err(e) => {
                    eprintln!("Error: failed to format output: {e}");
                    process::exit(1);
                }
            }
        }
    }
}

pub async fn cmd_create_card(
    client: &GitimClient,
    mode: &OutputMode,
    channel: &str,
    title: &str,
    labels: Option<&[String]>,
    assignee: Option<&str>,
    status: Option<&str>,
) {
    match client.create_card(channel, title, labels, assignee, status).await {
        Ok(resp) => print_or_exit(resp, mode, |d| {
            let id = d["card_id"].as_str().unwrap_or("?");
            let ch = d["channel"].as_str().unwrap_or("?");
            println!("创建卡片 #{}/{}", ch, id);
        }),
        Err(e) => {
            eprintln!("创建失败: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_list_cards(
    client: &GitimClient,
    mode: &OutputMode,
    channel: Option<&str>,
    labels: Option<&[String]>,
    status: Option<&str>,
    assignee: Option<&str>,
) {
    match client.list_cards(channel, labels, status, assignee).await {
        Ok(resp) => print_or_exit(resp, mode, |d| {
            let cards = d.get("cards").and_then(|v| v.as_array());
            match cards {
                Some(arr) if !arr.is_empty() => {
                    for c in arr {
                        let ch = c["channel"].as_str().unwrap_or("?");
                        let id = c["card_id"].as_str().unwrap_or("?");
                        let t = c["title"].as_str().unwrap_or("");
                        let s = c["status"].as_str().unwrap_or("");
                        let a = c["assignee"].as_str().unwrap_or("-");
                        let ls: Vec<&str> = c["labels"]
                            .as_array()
                            .map(|arr| arr.iter().filter_map(|l| l.as_str()).collect())
                            .unwrap_or_default();
                        println!(
                            "#{ch}/{id}  [{s}]  {t}  @{a}  {}",
                            if ls.is_empty() { String::new() } else { format!("[{}]", ls.join(", ")) }
                        );
                    }
                }
                _ => println!("没有卡片"),
            }
        }),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_read_card(
    client: &GitimClient,
    mode: &OutputMode,
    channel: &str,
    card_id: &str,
    limit: Option<u64>,
    since: Option<u64>,
) {
    match client.read_card(channel, card_id, limit, since).await {
        Ok(resp) => print_or_exit(resp, mode, |d| {
            let meta = &d["meta"];
            println!(
                "#{}/{}  [{}]  {}",
                d["channel"].as_str().unwrap_or("?"),
                d["card_id"].as_str().unwrap_or("?"),
                meta["status"].as_str().unwrap_or(""),
                meta["title"].as_str().unwrap_or(""),
            );
            if let Some(entries) = d["entries"].as_array() {
                for e in entries {
                    let ln = e["line_number"].as_u64().unwrap_or(0);
                    let author = e["author"].as_str().unwrap_or("?");
                    let body = e["body"].as_str().unwrap_or("");
                    println!("L{:06} @{}: {}", ln, author, body);
                }
            }
        }),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_send_card_message(
    client: &GitimClient,
    mode: &OutputMode,
    channel: &str,
    card_id: &str,
    body: &str,
    reply_to: Option<u64>,
) {
    match client.send_card_message(channel, card_id, body, reply_to).await {
        Ok(resp) => print_or_exit(resp, mode, |d| {
            println!(
                "L{:06} -> #{}/{}",
                d["line_number"].as_u64().unwrap_or(0),
                d["channel"].as_str().unwrap_or("?"),
                d["card_id"].as_str().unwrap_or("?"),
            );
        }),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_update_card(
    client: &GitimClient,
    mode: &OutputMode,
    channel: &str,
    card_id: &str,
    status: Option<&str>,
    labels: Option<&[String]>,
    assignee: Option<&str>,
) {
    match client.update_card(channel, card_id, status, labels, assignee).await {
        Ok(resp) => print_or_exit(resp, mode, |d| {
            println!(
                "更新 #{}/{}  status={}  assignee={}",
                d["channel"].as_str().unwrap_or("?"),
                d["card_id"].as_str().unwrap_or("?"),
                d["status"].as_str().unwrap_or(""),
                d["assignee"].as_str().unwrap_or("-"),
            );
        }),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}
```

- [ ] **Step 4**: 编译

Run: `cargo build -p gitim-cli`
Expected: 0 errors

- [ ] **Step 5**: 手动验证 help 打印

Run: `cargo run -p gitim-cli -- card --help`
Expected: 显示 5 个子命令 create/ls/read/comment/update

- [ ] **Step 6**: Commit

```bash
git add crates/gitim-cli/src/main.rs \
        crates/gitim-cli/src/commands/card.rs \
        crates/gitim-cli/src/commands/mod.rs
git rm crates/gitim-cli/src/commands/board.rs
git commit -m "refactor(cli): 删 board 子命令，card 子命令按 channel 归属重写"
```

---

## Phase 5: Runtime HTTP 暴露

### Task 5.1: 新增 5 个 `/im/cards/...` 端点

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`

- [ ] **Step 1**: 在文件中新增 5 个 handler 函数

在 `im_thread` 函数之后插入：

```rust
#[derive(Deserialize)]
struct CreateCardRequest {
    channel: String,
    title: String,
    #[serde(default)]
    labels: Option<Vec<String>>,
    #[serde(default)]
    assignee: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

async fn im_create_card(
    State(state): State<SharedRuntimeState>,
    Json(req): Json<CreateCardRequest>,
) -> Json<serde_json::Value> {
    let client = match human_client(&state) {
        Ok(c) => c,
        Err(j) => return j,
    };
    let labels_slice = req.labels.as_deref();
    api_response_to_json(
        client
            .create_card(
                &req.channel,
                &req.title,
                labels_slice,
                req.assignee.as_deref(),
                req.status.as_deref(),
            )
            .await,
    )
}

#[derive(Deserialize)]
struct ListCardsQuery {
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    label: Vec<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    assignee: Option<String>,
}

async fn im_list_cards(
    State(state): State<SharedRuntimeState>,
    axum::extract::Query(q): axum::extract::Query<ListCardsQuery>,
) -> Json<serde_json::Value> {
    let client = match human_client(&state) {
        Ok(c) => c,
        Err(j) => return j,
    };
    let labels_slice: Option<&[String]> = if q.label.is_empty() { None } else { Some(&q.label) };
    api_response_to_json(
        client
            .list_cards(q.channel.as_deref(), labels_slice, q.status.as_deref(), q.assignee.as_deref())
            .await,
    )
}

#[derive(Deserialize)]
struct ReadCardQuery {
    #[serde(default)]
    limit: Option<u64>,
    #[serde(default)]
    since: Option<u64>,
}

async fn im_read_card(
    State(state): State<SharedRuntimeState>,
    axum::extract::Path((channel, card_id)): axum::extract::Path<(String, String)>,
    axum::extract::Query(q): axum::extract::Query<ReadCardQuery>,
) -> Json<serde_json::Value> {
    let client = match human_client(&state) {
        Ok(c) => c,
        Err(j) => return j,
    };
    api_response_to_json(
        client.read_card(&channel, &card_id, q.limit, q.since).await,
    )
}

#[derive(Deserialize)]
struct SendCardMessageRequest {
    body: String,
    #[serde(default)]
    reply_to: Option<u64>,
}

async fn im_send_card_message(
    State(state): State<SharedRuntimeState>,
    axum::extract::Path((channel, card_id)): axum::extract::Path<(String, String)>,
    Json(req): Json<SendCardMessageRequest>,
) -> Json<serde_json::Value> {
    let client = match human_client(&state) {
        Ok(c) => c,
        Err(j) => return j,
    };
    api_response_to_json(
        client
            .send_card_message(&channel, &card_id, &req.body, req.reply_to)
            .await,
    )
}

#[derive(Deserialize)]
struct UpdateCardRequest {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    labels: Option<Vec<String>>,
    #[serde(default)]
    assignee: Option<String>,
}

async fn im_update_card(
    State(state): State<SharedRuntimeState>,
    axum::extract::Path((channel, card_id)): axum::extract::Path<(String, String)>,
    Json(req): Json<UpdateCardRequest>,
) -> Json<serde_json::Value> {
    let client = match human_client(&state) {
        Ok(c) => c,
        Err(j) => return j,
    };
    let labels_slice = req.labels.as_deref();
    api_response_to_json(
        client
            .update_card(
                &channel,
                &card_id,
                req.status.as_deref(),
                labels_slice,
                req.assignee.as_deref(),
            )
            .await,
    )
}
```

- [ ] **Step 2**: 在 `create_router` 里注册路由

在既有 `.route("/im/thread", post(im_thread))` 之后（约 line 1017）插入：

```rust
        .route("/im/cards", post(im_create_card))
        .route("/im/cards", get(im_list_cards))
        .route("/im/cards/{channel}/{card_id}", get(im_read_card))
        .route("/im/cards/{channel}/{card_id}/messages", post(im_send_card_message))
        .route("/im/cards/{channel}/{card_id}", axum::routing::patch(im_update_card))
```

- [ ] **Step 3**: 编译

Run: `cargo build -p gitim-runtime`
Expected: 0 errors

- [ ] **Step 4**: 跑 runtime tests

Run: `cargo test -p gitim-runtime --lib`
Expected: 既有单元测试通过

- [ ] **Step 5**: Commit

```bash
git add crates/gitim-runtime/src/http.rs
git commit -m "feat(runtime): HTTP 暴露 /im/cards 五个端点"
```

---

## Phase 6: Index（SQLite FTS5）

### Task 6.1: parse_diff_path 支持 card 路径 + insert 扫描 channels/<ch>/cards/

**Files:**
- Modify: `crates/gitim-index/src/lib.rs`

- [ ] **Step 1**: 找到 `parse_diff_path` 函数（在文件下方），扩展它识别 card 路径。

先 grep 看当前实现：

Run: `grep -n "parse_diff_path" crates/gitim-index/src/lib.rs`

然后替换函数为：

```rust
/// 从 git diff 的文件路径解析 (channel_identifier, channel_type)。
/// - "channels/<name>.thread" → (name, "channel")
/// - "dm/<h1>--<h2>.thread" → ("<h1>--<h2>", "dm")
/// - "channels/<ch>/cards/<id>/discussion.thread" → ("channels/<ch>/cards/<id>", "card")
fn parse_diff_path(path_str: &str) -> Option<(String, &'static str)> {
    if let Some(rest) = path_str.strip_prefix("channels/") {
        if let Some(name) = rest.strip_suffix(".thread") {
            if !name.contains('/') {
                return Some((name.to_string(), "channel"));
            }
        }
        // card path: <ch>/cards/<id>/discussion.thread
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
```

- [ ] **Step 2**: 在 `rebuild` 函数末尾（tx.commit 前）扫描 cards

在 `rebuild` 函数里，`dm_dir` 扫描块之后、`Self::set_commit_id` 之前，插入：

```rust
        // 扫描 channels/<ch>/cards/<id>/discussion.thread
        if channels_dir.exists() {
            for ch_entry in std::fs::read_dir(&channels_dir).into_iter().flatten().flatten() {
                if !ch_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }
                let cards_dir = ch_entry.path().join("cards");
                if !cards_dir.exists() {
                    continue;
                }
                let channel_name = ch_entry.file_name().to_string_lossy().to_string();
                for card_entry in std::fs::read_dir(&cards_dir).into_iter().flatten().flatten() {
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
                            warn!("index rebuild: skip card {}/{}: {}", channel_name, card_id, e);
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
```

- [ ] **Step 3**: 加测试（新增文件或扩展既有）

找一个既有测试位置，加：

```rust
#[test]
fn parse_diff_path_card() {
    let result = parse_diff_path("channels/backend/cards/20260417-120000-abc/discussion.thread");
    assert_eq!(
        result,
        Some(("channels/backend/cards/20260417-120000-abc".to_string(), "card"))
    );
}

#[test]
fn parse_diff_path_channel_still_works() {
    let result = parse_diff_path("channels/backend.thread");
    assert_eq!(result, Some(("backend".to_string(), "channel")));
}
```

- [ ] **Step 4**: 跑测试

Run: `cargo test -p gitim-index`
Expected: 既有测试通过 + 新增 2 个通过

- [ ] **Step 5**: Commit

```bash
git add crates/gitim-index/src/lib.rs
git commit -m "feat(index): 支持 channels/<ch>/cards/ 路径和 channel_type=card"
```

---

### Task 6.2: Search API 加 include_cards 参数

**Files:**
- Modify: `crates/gitim-index/src/lib.rs`
- Modify: `crates/gitim-daemon/src/api.rs`
- Modify: `crates/gitim-daemon/src/handlers.rs` (Request::Search dispatch)

- [ ] **Step 1**: 改 `SearchParams` 加字段

```rust
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
```

- [ ] **Step 2**: 在 `search` 函数中添加过滤

位置：`crates/gitim-index/src/lib.rs` 的 `pub fn search` 函数，在"频道类型过滤"块之后（约 line 455）、"DM 可见性过滤"块之前。

在该位置插入：

```rust
        // Cards 默认过滤：除非显式 include_cards=true 或指定了 channel_type
        if !params.include_cards && params.channel_type.is_none() {
            conditions.push("m.channel_type != 'card'".to_string());
        }
```

该位置紧接在：
```rust
        if let Some(ref channel_type) = params.channel_type {
            ...
            bind_values.push(Box::new(channel_type.clone()));
        }
```
之后。

- [ ] **Step 3**: 在 Request::Search 加 `include_cards` 字段

在 `api.rs` 的 Search variant 里：

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
    #[serde(default)]
    include_cards: bool,
},
```

- [ ] **Step 4**: handlers.rs 改 dispatch + `handle_search` 签名 + SearchParams 传参

`handlers.rs` 的 `Request::Search` dispatch arm（约 line 151-158）改为：
```rust
Request::Search {
    query,
    author,
    channel,
    channel_type,
    limit,
    offset,
    include_cards,
} => handle_search(state, query, author, channel, channel_type, limit, offset, include_cards).await,
```

`handle_search` 函数（约 line 2725）签名改为：
```rust
async fn handle_search(
    state: SharedState,
    query: Option<String>,
    author: Option<String>,
    channel: Option<String>,
    channel_type: Option<String>,
    limit: usize,
    offset: usize,
    include_cards: bool,
) -> Response {
```

在函数内构造 `SearchParams` 时加 `include_cards` 字段（定位：该函数内搜 `SearchParams {`，把 `include_cards` 加到 struct literal）。

`client.rs` search 方法：
```rust
pub async fn search(
    &self,
    query: Option<&str>,
    author: Option<&str>,
    channel: Option<&str>,
    channel_type: Option<&str>,
    limit: Option<u64>,
    offset: Option<u64>,
    include_cards: bool,
) -> Result<ApiResponse, ClientError> {
    self.request(
        "search",
        json!({
            "query": query,
            "author": author,
            "channel": channel,
            "channel_type": channel_type,
            "limit": limit.unwrap_or(50),
            "offset": offset.unwrap_or(0),
            "include_cards": include_cards,
        }),
    )
    .await
}
```

- [ ] **Step 5**: CLI search 加 `--include-cards` flag

`crates/gitim-cli/src/main.rs` 的 `Commands::Search` variant（约 line 102-120）加字段：

```rust
/// Include card discussion messages in results
#[arg(long)]
include_cards: bool,
```

同文件 Search 分支的 dispatch（约 line 405-424）改为在 `offset` 之后传入 `include_cards`：
```rust
Commands::Search {
    query,
    author,
    channel,
    channel_type,
    limit,
    offset,
    include_cards,
} => {
    commands::admin::cmd_search(
        &client, &mode,
        query.as_deref(),
        author.as_deref(),
        channel.as_deref(),
        channel_type.as_deref(),
        limit, offset, include_cards,
    )
    .await
}
```

`crates/gitim-cli/src/commands/admin.rs` 的 `cmd_search` 签名（约 line 66-75）尾部加一个参数：
```rust
pub async fn cmd_search(
    client: &GitimClient,
    mode: &OutputMode,
    query: Option<&str>,
    author: Option<&str>,
    channel: Option<&str>,
    channel_type: Option<&str>,
    limit: u64,
    offset: u64,
    include_cards: bool,
) {
```

函数体里 `client.search(...)` 的调用改为（约 line 77）：
```rust
    .search(query, author, channel, channel_type, Some(limit), Some(offset), include_cards)
```

- [ ] **Step 6**: 测试

Run: `cargo test -p gitim-index -p gitim-daemon -p gitim-client -p gitim-cli`
Expected: 全通过

- [ ] **Step 7**: Commit

```bash
git add crates/gitim-index/src/lib.rs \
        crates/gitim-daemon/src/api.rs \
        crates/gitim-daemon/src/handlers.rs \
        crates/gitim-client/src/client.rs \
        crates/gitim-cli/src/main.rs \
        crates/gitim-cli/src/commands/admin.rs
git commit -m "feat(search): include_cards 参数，默认不返回卡片消息"
```

---

## Phase 7: End-to-end

### Task 7.1: 全量测试 + 手动 smoke test

- [ ] **Step 1**: 跑全量测试

Run: `cargo test --workspace --exclude gitim-runtime` （排除 poller 集成测试，后续可单独跑）
Expected: 全绿

Run: `cargo test -p gitim-runtime --test poller`
Expected: 全绿（需要 daemon 二进制已编译，cargo 会自动处理）

- [ ] **Step 2**: 手动 smoke test（可选，确认 CLI 通）

```bash
# 在 .worktrees/card-refactor 下，模拟仓库
cd /tmp && mkdir -p smoke-test && cd smoke-test
git init
# 用本地 feature/card-refactor 构建的 gitim
/path/to/.worktrees/card-refactor/target/debug/gitim onboard --git-server git --handler alice --display-name Alice
/path/to/.worktrees/card-refactor/target/debug/gitim create-channel dev
/path/to/.worktrees/card-refactor/target/debug/gitim card create dev "First card" --label v2
/path/to/.worktrees/card-refactor/target/debug/gitim card ls
/path/to/.worktrees/card-refactor/target/debug/gitim card update dev <card_id> --status doing
/path/to/.worktrees/card-refactor/target/debug/gitim card comment dev <card_id> "progress note"
/path/to/.worktrees/card-refactor/target/debug/gitim card read dev <card_id>
```

Expected: 创建/列表/更新/评论/读取全部可用，文件出现在 `channels/dev/cards/<id>/`

- [ ] **Step 3**: Commit (如果有遗留改动)

```bash
git status  # 应该是 clean
```

---

## Self-Review Checklist

本 plan 自检（已在起草时完成，此 checklist 供执行者参考）：

1. **Spec 覆盖**：design §3 类型 ✓、§4 IPC ✓、§5 CLI ✓、§6 HTTP ✓、§7 Index ✓；§10 agent workflow 不在本 plan 范围（design §12 明确为独立工作流）
2. **Placeholder 扫描**：✓ 每步有完整代码
3. **类型一致**：`CardStatus` enum 仅用 `Todo/Doing/Done` ✓；CLI `--label` 参数一致 ✓；Event 字段命名统一 ✓

---

## Design spec 细化记录

实施过程中发现 design §7 的 "加 card_id TEXT NULL 字段"方案在 SQLite 复合 PK + NULL 上会有唯一性漏洞（`(ch, NULL, 1)` 和 `(ch, NULL, 1)` 被视为不同行）。本 plan 采取更简单方案：
- **不改 schema**
- 卡片消息的 `channel` 字段存路径 `channels/<ch>/cards/<id>`
- 既有 `channel_type` 字段加 `card` 值
- Search 默认过滤 `channel_type != 'card'`，`include_cards=true` 取消

实质等价，但更小侵入。Design doc §7 将同步 refine。
