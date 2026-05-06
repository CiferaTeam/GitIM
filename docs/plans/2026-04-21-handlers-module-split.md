# handlers.rs 模块拆分 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 `handlers.rs`（3480 行）拆分为 `handlers/` 子模块目录，迁移内联测试到 `tests/`，消除 `entry_to_json` 重复。

**Architecture:** 纯结构重组，零业务逻辑变更。将单文件拆为 8 个职责单一的子模块文件，1567 行内联测试迁移到 `tests/handlers_test.rs`，统一 `entry_to_json` 到 `handlers/serde.rs` 并让 `thread_io.rs` 复用。

**Tech Stack:** Rust 2021 edition, cargo test, git

---

## 文件结构

```
crates/gitim-daemon/src/
├── handlers/              # 新目录，替代 handlers.rs
│   ├── mod.rs             # handle_request 路由 + resolve_author + resolve_thread_path
│   ├── serde.rs           # link_to_json + entry_to_json (pub(crate))
│   ├── send.rs            # handle_send
│   ├── read.rs            # handle_read + handle_get_thread + handle_list_channels
│   │                      #   + handle_list_archived_channels + handle_list_users
│   ├── poll.rs            # handle_poll
│   ├── channel.rs         # handle_create_channel + handle_archive_channel
│   │                      #   + handle_unarchive_channel + write_channel_event
│   │                      #   + handle_join_channel + handle_leave_channel
│   ├── user.rs            # handle_register_user
│   └── search.rs          # handle_search + handle_reindex
├── card_handlers.rs       # 不动
├── thread_io.rs           # 删除重复 entry_to_json，改用 crate::handlers::serde::entry_to_json
└── lib.rs                 # pub mod handlers 不变（目录模块自动识别）

crates/gitim-daemon/tests/
├── handlers_test.rs       # 从 handlers.rs 内联测试迁移（35 个测试）
└── ... (其他 8 个测试文件不变)
```

---

### Task 1: 创建 handlers/ 目录骨架 + serde.rs

**Files:**
- Create: `crates/gitim-daemon/src/handlers/mod.rs`
- Create: `crates/gitim-daemon/src/handlers/serde.rs`
- Delete: `crates/gitim-daemon/src/handlers.rs`

- [ ] **Step 1: 创建 handlers/ 目录和 serde.rs**

`serde.rs` 提取 `link_to_json` 和 `entry_to_json`，标记为 `pub(crate)` 以便 `thread_io.rs` 复用。

```rust
// crates/gitim-daemon/src/handlers/serde.rs

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
```

- [ ] **Step 2: 创建 mod.rs 骨架（先只含路由 + 共享辅助函数）**

```rust
// crates/gitim-daemon/src/handlers/mod.rs

mod channel;
mod poll;
mod read;
mod search;
mod serde;
mod send;
mod user;

pub use channel::*;
pub use poll::*;
pub use read::*;
pub use search::*;
pub use send::*;
pub use user::*;

use crate::api::{Event, Request, Response};
use crate::state::SharedState;

pub(crate) use serde::{entry_to_json, link_to_json};

/// Resolve author from explicit param or daemon identity.
async fn resolve_author(author: Option<String>, state: &SharedState) -> Result<String, Response> {
    match author {
        Some(a) if !a.is_empty() => Ok(a),
        _ => {
            let current = state.current_user.read().await;
            match current.clone() {
                Some(u) => Ok(u),
                None => Err(Response::error(
                    "no author specified and no identity configured",
                )),
            }
        }
    }
}

/// Resolve a channel string to a filesystem path and a cache key.
/// Channels: "channels/{name}.thread", DMs: "dm:{h1},{h2}" -> "dm/{h1}--{h2}.thread"
fn resolve_thread_path(
    state: &SharedState,
    channel: &str,
) -> Result<(std::path::PathBuf, String), Response> {
    use gitim_core::dm::dm_filename;
    use gitim_core::types::{ChannelName, Handler};

    if channel.starts_with("dm:") {
        let parts: Vec<&str> = channel[3..].split(',').collect();
        if parts.len() != 2 {
            return Err(Response::error("DM format must be dm:handler1,handler2"));
        }
        let h1 = Handler::new(parts[0])
            .map_err(|e| Response::error(format!("invalid DM handler: {}", e)))?;
        let h2 = Handler::new(parts[1])
            .map_err(|e| Response::error(format!("invalid DM handler: {}", e)))?;
        let name = dm_filename(&h1, &h2);
        let path = state.repo_root.join("dm").join(format!("{}.thread", name));
        Ok((path, name))
    } else {
        let name = ChannelName::new(channel)
            .map_err(|e| Response::error(format!("invalid channel name: {}", e)))?;
        let path = state
            .repo_root
            .join("channels")
            .join(format!("{}.thread", name));
        Ok((path, name.to_string()))
    }
}

pub async fn handle_request(req: Request, state: SharedState) -> Response {
    // Guest mode guard: reject all write operations
    if state.is_guest.load(std::sync::atomic::Ordering::SeqCst) {
        let is_write = matches!(
            req,
            Request::Send { .. }
                | Request::RegisterUser { .. }
                | Request::JoinChannel { .. }
                | Request::LeaveChannel { .. }
                | Request::CreateChannel { .. }
                | Request::ArchiveChannel { .. }
                | Request::UnarchiveChannel { .. }
                | Request::CreateCard { .. }
                | Request::SendCardMessage { .. }
                | Request::UpdateCard { .. }
                | Request::ArchiveCard { .. }
                | Request::UnarchiveCard { .. }
        );
        if is_write {
            return Response::error("guest mode: write operations are not allowed");
        }
    }

    match req {
        Request::Status => {
            let is_guest = state.is_guest.load(std::sync::atomic::Ordering::SeqCst);
            Response::success(serde_json::json!({
                "version": "0.1.0",
                "status": "running",
                "guest": is_guest,
            }))
        }
        Request::Send {
            channel,
            body,
            reply_to,
            author,
        } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            handle_send(state, channel, body, reply_to, resolved_author).await
        }
        Request::Read {
            channel,
            limit,
            since,
        } => handle_read(state, channel, limit, since).await,
        Request::ListChannels => handle_list_channels(state).await,
        Request::ListUsers => handle_list_users(state).await,
        Request::GetThread {
            channel,
            line_number,
        } => handle_get_thread(state, channel, line_number).await,
        Request::Subscribe => Response::success(serde_json::json!({"subscribed": true})),
        Request::RegisterUser {
            handler,
            display_name,
            role,
            introduction,
        } => handle_register_user(state, handler, display_name, role, introduction).await,
        Request::Poll { since } => handle_poll(state, since).await,
        Request::Stop => handle_stop(state).await,
        Request::Onboard {
            git_server,
            auth,
            admin,
            guest,
        } => crate::onboard::handle_onboard(state, git_server, auth, admin, guest).await,
        Request::JoinChannel {
            channel,
            targets,
            author,
        } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            handle_join_channel(state, channel, targets, resolved_author).await
        }
        Request::LeaveChannel {
            channel,
            targets,
            author,
        } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            handle_leave_channel(state, channel, targets, resolved_author).await
        }
        Request::CreateChannel {
            name,
            display_name,
            introduction,
            author,
            invitees,
        } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            handle_create_channel(state, name, display_name, introduction, resolved_author, invitees).await
        }
        Request::Search {
            query,
            author,
            channel,
            channel_type,
            limit,
            offset,
            include_cards,
        } => handle_search(state, query, author, channel, channel_type, limit, offset, include_cards).await,
        Request::Reindex => handle_reindex(state).await,
        Request::ArchiveChannel { channel, author } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            handle_archive_channel(state, channel, resolved_author).await
        }
        Request::UnarchiveChannel { channel, author } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            handle_unarchive_channel(state, channel, resolved_author).await
        }
        Request::ListArchivedChannels => handle_list_archived_channels(state).await,
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
        Request::ArchiveCard { channel, card_id, author } => {
            crate::card_handlers::handle_archive_card(state, channel, card_id, author).await
        }
        Request::UnarchiveCard { channel, card_id, author } => {
            crate::card_handlers::handle_unarchive_card(state, channel, card_id, author).await
        }
        Request::ListArchivedCards { channel } => {
            crate::card_handlers::handle_list_archived_cards(state, channel).await
        }
    }
}
```

- [ ] **Step 3: 创建空的子模块文件（占位，确保编译通过）**

每个文件只含必要的 `use` 和一个占位的 `pub async fn`。后续 Task 逐个填充。

```rust
// crates/gitim-daemon/src/handlers/send.rs
use crate::api::Response;
use crate::state::SharedState;

pub async fn handle_send(
    _state: SharedState,
    _channel: String,
    _body: String,
    _reply_to: Option<u64>,
    _author: String,
) -> Response {
    unimplemented!("filled in Task 2")
}
```

```rust
// crates/gitim-daemon/src/handlers/read.rs
use crate::api::Response;
use crate::state::SharedState;

pub async fn handle_read(
    _state: SharedState, _channel: String, _limit: Option<usize>, _since: Option<u64>,
) -> Response { unimplemented!() }

pub async fn handle_get_thread(
    _state: SharedState, _channel: String, _line_number: u64,
) -> Response { unimplemented!() }

pub async fn handle_list_channels(_state: SharedState) -> Response { unimplemented!() }

pub async fn handle_list_archived_channels(_state: SharedState) -> Response { unimplemented!() }

pub async fn handle_list_users(_state: SharedState) -> Response { unimplemented!() }

pub async fn handle_stop(_state: SharedState) -> Response { unimplemented!() }
```

```rust
// crates/gitim-daemon/src/handlers/poll.rs
use crate::api::Response;
use crate::state::SharedState;

pub async fn handle_poll(_state: SharedState, _since: Option<String>) -> Response {
    unimplemented!()
}
```

```rust
// crates/gitim-daemon/src/handlers/channel.rs
use crate::api::Response;
use crate::state::SharedState;

pub async fn handle_join_channel(
    _state: SharedState, _channel: String, _targets: Vec<String>, _author: String,
) -> Response { unimplemented!() }

pub async fn handle_leave_channel(
    _state: SharedState, _channel: String, _targets: Vec<String>, _author: String,
) -> Response { unimplemented!() }

pub async fn handle_create_channel(
    _state: SharedState, _name: String, _display_name: Option<String>,
    _introduction: Option<String>, _author: String, _invitees: Vec<String>,
) -> Response { unimplemented!() }

pub async fn handle_archive_channel(
    _state: SharedState, _channel: String, _author: String,
) -> Response { unimplemented!() }

pub async fn handle_unarchive_channel(
    _state: SharedState, _channel: String, _author: String,
) -> Response { unimplemented!() }
```

```rust
// crates/gitim-daemon/src/handlers/user.rs
use crate::api::Response;
use crate::state::SharedState;

pub async fn handle_register_user(
    _state: SharedState, _handler: String, _display_name: String,
    _role: String, _introduction: String,
) -> Response { unimplemented!() }
```

```rust
// crates/gitim-daemon/src/handlers/search.rs
use crate::api::Response;
use crate::state::SharedState;

pub async fn handle_search(
    _state: SharedState, _query: Option<String>, _author: Option<String>,
    _channel: Option<String>, _channel_type: Option<String>,
    _limit: usize, _offset: usize, _include_cards: bool,
) -> Response { unimplemented!() }

pub async fn handle_reindex(_state: SharedState) -> Response { unimplemented!() }
```

- [ ] **Step 4: 删除旧 handlers.rs，验证编译**

```bash
rm crates/gitim-daemon/src/handlers.rs
cargo check -p gitim-daemon 2>&1 | head -20
```

Expected: 编译通过（`unimplemented!` 不会在 check 阶段报错，只有链接时才会）

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-daemon/src/handlers/ crates/gitim-daemon/src/handlers.rs
git commit -m "refactor(daemon): scaffold handlers/ module directory"
```

---

### Task 2: 填充 send.rs

**Files:**
- Modify: `crates/gitim-daemon/src/handlers/send.rs`

- [ ] **Step 1: 从 handlers.rs 原文复制 `handle_send` 函数（行 316-549）到 send.rs**

在文件顶部添加必要的 `use` 声明，将 `super::resolve_thread_path`、`super::resolve_author`、`super::entry_to_json` 替换为直接使用（它们在 mod.rs 中可见）。

`send.rs` 的 `use` 块：

```rust
use crate::api::{Event, Response};
use crate::state::{PendingMessage, PushResult, SharedState};
use crate::handlers::{entry_to_json, resolve_thread_path};
use gitim_core::formatter::format_message;
use gitim_core::parser::parse_thread;
use gitim_core::types::{ChannelMeta, Handler};
use gitim_core::validator::compliance::validate_append;
use tracing::{info, warn};
```

函数体保持原样不变。注意 `resolve_thread_path` 和 `resolve_author` 在 `mod.rs` 中是模块级私有函数，需要改为 `pub(super)` 或在 `mod.rs` 中 `pub(crate)` 暴露。选择 `pub(super)` 最小暴露。

- [ ] **Step 2: 验证编译**

```bash
cargo check -p gitim-daemon 2>&1 | head -20
```

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-daemon/src/handlers/
git commit -m "refactor(daemon): extract handle_send to handlers/send.rs"
```

---

### Task 3: 填充 read.rs

**Files:**
- Modify: `crates/gitim-daemon/src/handlers/read.rs`

- [ ] **Step 1: 从 handlers.rs 原文复制以下函数到 read.rs**

- `handle_read`（行 551-627）
- `handle_get_thread`（行 779-864）
- `handle_list_channels`（行 693-740）
- `handle_list_archived_channels`（行 742-770）
- `handle_list_users`（行 772-777）
- `handle_stop`（行 866-878）

`read.rs` 的 `use` 块：

```rust
use crate::api::{Event, Response};
use crate::state::SharedState;
use crate::handlers::{entry_to_json, resolve_thread_path};
use gitim_core::dm::parse_dm_filename;
use gitim_core::parser::parse_thread;
use gitim_core::types::{ChannelMeta, ChannelName};
use tracing::info;
```

- [ ] **Step 2: 验证编译**

```bash
cargo check -p gitim-daemon 2>&1 | head -20
```

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-daemon/src/handlers/read.rs
git commit -m "refactor(daemon): extract read/list/stop handlers to handlers/read.rs"
```

---

### Task 4: 填充 poll.rs

**Files:**
- Modify: `crates/gitim-daemon/src/handlers/poll.rs`

- [ ] **Step 1: 从 handlers.rs 原文复制 `handle_poll`（行 880-1154）到 poll.rs**

`poll.rs` 的 `use` 块：

```rust
use crate::api::Response;
use crate::state::SharedState;
use crate::handlers::entry_to_json;
use gitim_core::dm::parse_dm_filename;
use gitim_core::parser::parse_thread;
use gitim_core::types::ChannelMeta;
use std::collections::HashMap;
use tracing::warn;
```

- [ ] **Step 2: 验证编译**

```bash
cargo check -p gitim-daemon 2>&1 | head -20
```

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-daemon/src/handlers/poll.rs
git commit -m "refactor(daemon): extract handle_poll to handlers/poll.rs"
```

---

### Task 5: 填充 channel.rs

**Files:**
- Modify: `crates/gitim-daemon/src/handlers/channel.rs`

- [ ] **Step 1: 从 handlers.rs 原文复制以下函数到 channel.rs**

- `handle_join_channel`（行 1156-1163）
- `handle_leave_channel`（行 1165-1173）
- `handle_create_channel`（行 1176-1322）
- `handle_archive_channel`（行 1324-1438）
- `handle_unarchive_channel`（行 1440-1596）
- `write_channel_event`（行 1598-1827）

`channel.rs` 的 `use` 块：

```rust
use crate::api::{Event, Response};
use crate::state::{PendingMessage, PushResult, SharedState};
use crate::handlers::{entry_to_json, resolve_thread_path};
use gitim_core::formatter::{format_event, format_message};
use gitim_core::parser::parse_thread;
use gitim_core::types::{ChannelMeta, ChannelName, Handler};
use gitim_core::validator::compliance::validate_append;
use gitim_core::validator::im_rules;
use gitim_sync::git::GitError;
use tracing::{info, warn};
```

- [ ] **Step 2: 验证编译**

```bash
cargo check -p gitim-daemon 2>&1 | head -20
```

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-daemon/src/handlers/channel.rs
git commit -m "refactor(daemon): extract channel handlers to handlers/channel.rs"
```

---

### Task 6: 填充 user.rs + search.rs

**Files:**
- Modify: `crates/gitim-daemon/src/handlers/user.rs`
- Modify: `crates/gitim-daemon/src/handlers/search.rs`

- [ ] **Step 1: 从 handlers.rs 原文复制 `handle_register_user`（行 629-691）到 user.rs**

`user.rs` 的 `use` 块：

```rust
use crate::api::Response;
use crate::state::SharedState;
use gitim_core::types::{Handler, UserMeta};
use tracing::info;
```

- [ ] **Step 2: 从 handlers.rs 原文复制 `handle_search`（行 3396-3455）和 `handle_reindex`（行 3457-3480）到 search.rs**

`search.rs` 的 `use` 块：

```rust
use crate::api::Response;
use crate::state::SharedState;
```

- [ ] **Step 3: 验证编译**

```bash
cargo check -p gitim-daemon 2>&1 | head -20
```

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-daemon/src/handlers/user.rs crates/gitim-daemon/src/handlers/search.rs
git commit -m "refactor(daemon): extract user and search handlers"
```

---

### Task 7: 迁移内联测试到 tests/handlers_test.rs

**Files:**
- Create: `crates/gitim-daemon/tests/handlers_test.rs`
- Modify: `crates/gitim-daemon/src/handlers/mod.rs`（删除 `#[cfg(test)] mod tests` 块）

- [ ] **Step 1: 从 handlers.rs 原文复制 `#[cfg(test)] mod tests` 块（行 1829-3395）到 tests/handlers_test.rs**

修改要点：
- 删除 `#[cfg(test)]` 和 `mod tests {` 包裹
- 将 `use super::*;` 替换为 `use gitim_daemon::handlers::handle_request;`
- 将 `use crate::state::AppState;` 替换为 `use gitim_daemon::state::AppState;`
- 将 `use gitim_core::types::config::Config;` 保持不变（外部 crate）
- 将 `use std::sync::Arc;` 和 `use tokio::sync::broadcast;` 保持不变
- 所有 `crate::` 引用改为 `gitim_daemon::`（如 `crate::api::Request` → `gitim_daemon::api::Request`）

- [ ] **Step 2: 从 mod.rs 删除测试块**

mod.rs 中不应有任何 `#[cfg(test)]` 代码。

- [ ] **Step 3: 运行测试验证**

```bash
cargo test -p gitim-daemon --test handlers_test 2>&1 | tail -20
```

Expected: 35 个测试全部通过

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-daemon/tests/handlers_test.rs crates/gitim-daemon/src/handlers/mod.rs
git commit -m "refactor(daemon): migrate inline tests to tests/handlers_test.rs"
```

---

### Task 8: 消除 thread_io.rs 中的 entry_to_json 重复

**Files:**
- Modify: `crates/gitim-daemon/src/thread_io.rs`

- [ ] **Step 1: 重写 thread_io.rs，删除本地 `entry_to_json`，改用 `crate::handlers::serde::entry_to_json`**

修改要点：
- 删除 `thread_io.rs` 中行 57-97 的 `fn entry_to_json` 函数
- 删除 `use gitim_core::link::extract_links;` 和 `use gitim_core::types::LinkKind;`（不再需要）
- 在 `read_thread_entries` 中将 `entry_to_json(entry)` 调用改为 `crate::handlers::serde::entry_to_json(entry)`
- 或者在文件顶部添加 `use crate::handlers::serde::entry_to_json;`

- [ ] **Step 2: 验证编译 + 测试**

```bash
cargo test -p gitim-daemon 2>&1 | tail -20
```

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-daemon/src/thread_io.rs
git commit -m "refactor(daemon): deduplicate entry_to_json, reuse handlers::serde"
```

---

### Task 9: 全量测试 + 最终验证

**Files:**
- 无新文件

- [ ] **Step 1: 运行全量测试**

```bash
cargo test -p gitim-daemon 2>&1 | grep -E "^test result:|FAILED"
```

Expected: 所有测试通过，0 FAILED

- [ ] **Step 2: 验证公开 API 不变**

```bash
# 确认 handle_request 仍然可以通过 gitim_daemon::handlers::handle_request 访问
grep -rn "handlers::handle_request" crates/gitim-daemon/tests/ crates/gitim-daemon/src/server.rs crates/gitim-daemon/src/http.rs crates/gitim-daemon/src/onboard.rs
```

Expected: 所有引用仍然有效

- [ ] **Step 3: 验证文件行数合理**

```bash
wc -l crates/gitim-daemon/src/handlers/*.rs
```

Expected: 每个文件 < 400 行（mod.rs ~200, send.rs ~240, poll.rs ~280, channel.rs ~500 需要进一步观察）

- [ ] **Step 4: Commit（如有最终调整）**

```bash
git add -A
git commit -m "refactor(daemon): handlers module split complete"
```

---

## 自检清单

- [x] Spec 覆盖：拆分、测试迁移、去重，每个需求都有对应 Task
- [x] 无占位符：每步都有具体代码或命令
- [x] 类型一致：所有函数签名与原文一致
- [x] 不变量：handle_request 公开接口不变，零业务逻辑变更
