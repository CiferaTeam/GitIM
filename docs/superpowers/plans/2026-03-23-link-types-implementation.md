# Link Types Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 GitIM 消息格式增加 in-system link（频道、消息、用户资料）和 softlink（外部 URL）的解析与序列化能力。

**Architecture:** 新增 `link.rs` 模块（提取函数）和 `types/link.rs`（类型定义），与现有 `mention.rs` / `types/message.rs` 模式一致。parser 在构建 Message 时同时调用 `extract_links`。handlers.rs 提取 `message_to_json()` 辅助函数统一序列化 mentions + links。

**Tech Stack:** Rust (regex, serde_json), shell (E2E tests)

**Spec:** `docs/superpowers/specs/2026-03-23-gitim-link-design.md`

**Worktree:** `/Users/lewisliu/ateam/GitIM/.worktrees/link-types` (branch: `feature/link-types`)

---

## Chunk 1: Core Types + Extraction

### Task 1: Link/LinkKind 类型定义

**Files:**
- Create: `crates/gitim-core/src/types/link.rs`
- Modify: `crates/gitim-core/src/types/mod.rs:1-9`

- [ ] **Step 1: 创建 types/link.rs**

```rust
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
```

- [ ] **Step 2: 在 types/mod.rs 中注册并导出**

在 `crates/gitim-core/src/types/mod.rs` 中加入：

```rust
pub mod link;

pub use link::{Link, LinkKind};
```

- [ ] **Step 3: 编译验证**

Run: `cargo check -p gitim-core`
Expected: 编译通过（Link 尚未被使用，但类型定义正确）

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-core/src/types/link.rs crates/gitim-core/src/types/mod.rs
git commit -m "feat(core): add Link and LinkKind type definitions"
```

---

### Task 2: extract_links() 提取函数 + 单元测试

**Files:**
- Create: `crates/gitim-core/src/link.rs`
- Modify: `crates/gitim-core/src/lib.rs:1-8`

**依赖:** Task 1（Link/LinkKind 类型）

- [ ] **Step 1: 写失败测试 — 6 种语法 + 边界 case**

在 `crates/gitim-core/src/link.rs` 底部写测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::LinkKind;

    #[test]
    fn test_channel_link() {
        let links = extract_links("see <#general> for info");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].raw, "<#general>");
        match &links[0].kind {
            LinkKind::Channel { name } => assert_eq!(name, "general"),
            _ => panic!("expected Channel"),
        }
    }

    #[test]
    fn test_message_link() {
        let links = extract_links("check <#general:L000042>");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].raw, "<#general:L000042>");
        match &links[0].kind {
            LinkKind::Message { channel, line_number } => {
                assert_eq!(channel, "general");
                assert_eq!(*line_number, 42);
            }
            _ => panic!("expected Message"),
        }
    }

    #[test]
    fn test_user_profile_link() {
        let links = extract_links("contact <~bob>");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].raw, "<~bob>");
        match &links[0].kind {
            LinkKind::UserProfile { handler } => assert_eq!(handler.as_str(), "bob"),
            _ => panic!("expected UserProfile"),
        }
    }

    #[test]
    fn test_softlink_bare() {
        let links = extract_links("visit <!https://example.com>");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].raw, "<!https://example.com>");
        match &links[0].kind {
            LinkKind::Softlink { url, title } => {
                assert_eq!(url, "https://example.com");
                assert_eq!(*title, None);
            }
            _ => panic!("expected Softlink"),
        }
    }

    #[test]
    fn test_softlink_with_title() {
        let links = extract_links("see <!https://example.com|Example Site>");
        assert_eq!(links.len(), 1);
        match &links[0].kind {
            LinkKind::Softlink { url, title } => {
                assert_eq!(url, "https://example.com");
                assert_eq!(title.as_deref(), Some("Example Site"));
            }
            _ => panic!("expected Softlink"),
        }
    }

    #[test]
    fn test_softlink_empty_title() {
        let links = extract_links("<!https://example.com|>");
        assert_eq!(links.len(), 1);
        match &links[0].kind {
            LinkKind::Softlink { url, title } => {
                assert_eq!(url, "https://example.com");
                assert_eq!(title.as_deref(), Some(""));
            }
            _ => panic!("expected Softlink"),
        }
    }

    #[test]
    fn test_multiple_links() {
        let links = extract_links("<#general> and <~bob> and <!https://x.com>");
        assert_eq!(links.len(), 3);
    }

    #[test]
    fn test_duplicate_links_not_deduped() {
        let links = extract_links("<#general> <#general>");
        assert_eq!(links.len(), 2);
    }

    #[test]
    fn test_no_links() {
        let links = extract_links("plain text no links");
        assert!(links.is_empty());
    }

    #[test]
    fn test_mention_not_captured() {
        let links = extract_links("<@alice> is not a link");
        assert!(links.is_empty());
    }

    // --- 边界 case ---

    #[test]
    fn test_empty_markers_not_matched() {
        // <#> <~> <!> — regex requires [^>]+ (at least 1 char)
        let links = extract_links("<#> <~> <!>");
        assert!(links.is_empty());
    }

    #[test]
    fn test_uppercase_channel_ignored() {
        let links = extract_links("<#GENERAL>");
        assert!(links.is_empty());
    }

    #[test]
    fn test_consecutive_hyphen_channel_ignored() {
        let links = extract_links("<#a--b>");
        assert!(links.is_empty());
    }

    #[test]
    fn test_short_line_number_ignored() {
        // L00042 is only 5 digits, need at least 6
        let links = extract_links("<#general:L00042>");
        assert!(links.is_empty());
    }

    #[test]
    fn test_unclosed_marker() {
        let links = extract_links("<#general is not closed");
        assert!(links.is_empty());
    }

    #[test]
    fn test_system_handler_ignored() {
        let links = extract_links("<~system>");
        assert!(links.is_empty());
    }

    #[test]
    fn test_softlink_url_with_encoded_pipe() {
        let links = extract_links("<!https://example.com/path%7Cother>");
        assert_eq!(links.len(), 1);
        match &links[0].kind {
            LinkKind::Softlink { url, title } => {
                assert_eq!(url, "https://example.com/path%7Cother");
                assert_eq!(*title, None);
            }
            _ => panic!("expected Softlink"),
        }
    }

    #[test]
    fn test_message_link_long_line_number() {
        let links = extract_links("<#dev:L1234567>");
        assert_eq!(links.len(), 1);
        match &links[0].kind {
            LinkKind::Message { channel, line_number } => {
                assert_eq!(channel, "dev");
                assert_eq!(*line_number, 1234567);
            }
            _ => panic!("expected Message"),
        }
    }

    #[test]
    fn test_mention_and_link_coexist() {
        // extract_links 只管 link，不管 mention
        let links = extract_links("<@bob> <~bob>");
        assert_eq!(links.len(), 1);
        match &links[0].kind {
            LinkKind::UserProfile { handler } => assert_eq!(handler.as_str(), "bob"),
            _ => panic!("expected UserProfile"),
        }
    }
}
```

- [ ] **Step 2: 运行测试确认全部失败**

Run: `cargo test -p gitim-core -- link::tests --no-run 2>&1 | head -20`
Expected: 编译失败（`extract_links` 不存在）

- [ ] **Step 3: 实现 extract_links()**

在 `crates/gitim-core/src/link.rs` 顶部写实现：

```rust
use regex::Regex;
use std::sync::LazyLock;
use crate::types::{Handler, Link, LinkKind};
use crate::validator::validate_channel_name;

static LINK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<([#~!])([^>]+)>").unwrap()
});

static MSG_LINK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(.+):L(\d{6,})$").unwrap()
});

/// 从消息 body 中提取所有协议级链接，按出现顺序返回，不去重。
pub fn extract_links(body: &str) -> Vec<Link> {
    let mut result = Vec::new();
    for caps in LINK_RE.captures_iter(body) {
        let prefix = &caps[1];
        let content = &caps[2];
        let raw = caps[0].to_string();

        let kind = match prefix {
            "#" => parse_channel_or_message(content),
            "~" => parse_user_profile(content),
            "!" => parse_softlink(content),
            _ => None,
        };

        if let Some(kind) = kind {
            result.push(Link { kind, raw });
        }
    }
    result
}

fn parse_channel_or_message(content: &str) -> Option<LinkKind> {
    if let Some(caps) = MSG_LINK_RE.captures(content) {
        let channel = &caps[1];
        let line_number: u64 = caps[2].parse().ok()?;
        validate_channel_name(channel).ok()?;
        Some(LinkKind::Message {
            channel: channel.to_string(),
            line_number,
        })
    } else {
        validate_channel_name(content).ok()?;
        Some(LinkKind::Channel {
            name: content.to_string(),
        })
    }
}

fn parse_user_profile(content: &str) -> Option<LinkKind> {
    let handler = Handler::new(content).ok()?;
    Some(LinkKind::UserProfile { handler })
}

fn parse_softlink(content: &str) -> Option<LinkKind> {
    if let Some(pos) = content.find('|') {
        let url = &content[..pos];
        let title = &content[pos + 1..];
        Some(LinkKind::Softlink {
            url: url.to_string(),
            title: Some(title.to_string()),
        })
    } else {
        Some(LinkKind::Softlink {
            url: content.to_string(),
            title: None,
        })
    }
}
```

- [ ] **Step 4: 在 lib.rs 中注册 link 模块**

在 `crates/gitim-core/src/lib.rs` 中加入：

```rust
pub mod link;
```

- [ ] **Step 5: 运行测试确认全部通过**

Run: `cargo test -p gitim-core -- link::tests -v`
Expected: 所有 18 个测试通过

- [ ] **Step 6: Commit**

```bash
git add crates/gitim-core/src/link.rs crates/gitim-core/src/lib.rs
git commit -m "feat(core): add extract_links() with unit tests"
```

---

### Task 3: 集成到 parser — Message.links 字段

**Files:**
- Modify: `crates/gitim-core/src/types/message.rs:1-25`
- Modify: `crates/gitim-core/src/parser.rs:4,36,48-55,75-78`

**依赖:** Task 2（extract_links 函数）

- [ ] **Step 1: 写失败的集成测试**

在 `crates/gitim-core/src/parser.rs` 底部已有测试区域，追加：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_message_with_links() {
        let input = "[L000001][P000000][@alice][20260323T120000Z] see <#general> and <!https://x.com>\n";
        let file = parse_thread(input).unwrap();
        assert_eq!(file.messages.len(), 1);
        let msg = &file.messages[0];
        assert_eq!(msg.links.len(), 2);
        assert_eq!(msg.links[0].raw, "<#general>");
        assert_eq!(msg.links[1].raw, "<!https://x.com>");
    }

    #[test]
    fn test_parse_multiline_message_links() {
        let input = "[L000001][P000000][@alice][20260323T120000Z] first line\ncontact <~bob> here\n";
        let file = parse_thread(input).unwrap();
        assert_eq!(file.messages.len(), 1);
        let msg = &file.messages[0];
        assert_eq!(msg.links.len(), 1);
        assert_eq!(msg.links[0].raw, "<~bob>");
    }

    #[test]
    fn test_parse_message_no_links() {
        let input = "[L000001][P000000][@alice][20260323T120000Z] no links here\n";
        let file = parse_thread(input).unwrap();
        assert_eq!(file.messages[0].links.len(), 0);
    }

    #[test]
    fn test_parse_mentions_and_links_independent() {
        let input = "[L000001][P000000][@alice][20260323T120000Z] <@bob> <~bob>\n";
        let file = parse_thread(input).unwrap();
        let msg = &file.messages[0];
        assert_eq!(msg.mentions.len(), 1);
        assert_eq!(msg.mentions[0].as_str(), "bob");
        assert_eq!(msg.links.len(), 1);
        assert_eq!(msg.links[0].raw, "<~bob>");
    }
}
```

- [ ] **Step 2: 运行测试确认编译失败**

Run: `cargo test -p gitim-core -- parser::tests --no-run 2>&1 | head -10`
Expected: 编译失败（Message 没有 `links` 字段）

- [ ] **Step 3: 给 Message 加 links 字段**

修改 `crates/gitim-core/src/types/message.rs`：

```rust
use crate::types::handler::Handler;
use crate::types::link::Link;

/// A parsed message from a .thread file.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub line_number: u64,
    pub point_to: u64,
    pub author: Handler,
    pub timestamp: String,
    pub body: String,
    pub mentions: Vec<Handler>,
    pub links: Vec<Link>,
}
```

- [ ] **Step 4: 更新 parser.rs 中构建 Message 的代码**

在 `crates/gitim-core/src/parser.rs` 中：

1. 加 import：`use crate::link::extract_links;`

2. 初始化 Message 时加 `links: Vec::new()`（约 L48-55）：

```rust
messages.push(Message {
    line_number,
    point_to,
    author,
    timestamp,
    body: String::new(),
    mentions: Vec::new(),
    links: Vec::new(),
});
```

3. body 完成时同时提取 links（两处：L34-37 和 L75-78）：

```rust
// L34-37 区域（循环内遇到新消息时结算前一条）
if let (Some(body), Some(msg)) = (current_body.take(), messages.last_mut()) {
    msg.body = body;
    msg.mentions = extract_mentions(&msg.body);
    msg.links = extract_links(&msg.body);
}
```

```rust
// L75-78 区域（循环结束后结算最后一条）
if let (Some(body), Some(msg)) = (current_body, messages.last_mut()) {
    msg.body = body;
    msg.mentions = extract_mentions(&msg.body);
    msg.links = extract_links(&msg.body);
}
```

- [ ] **Step 5: 运行所有测试**

Run: `cargo test -p gitim-core -v`
Expected: 所有旧测试 + 新增的 4 个 parser 集成测试通过

- [ ] **Step 6: Commit**

```bash
git add crates/gitim-core/src/types/message.rs crates/gitim-core/src/parser.rs
git commit -m "feat(core): integrate links extraction into parser"
```

---

## Chunk 2: API Serialization + E2E

### Task 4: handlers.rs — message_to_json() 辅助函数

**Files:**
- Modify: `crates/gitim-daemon/src/handlers.rs:1-8,228-239,355-361,491-503`

**依赖:** Task 3（Message.links 字段）

- [ ] **Step 1: 编译 daemon 确认 Message.links 变更不破坏现有代码**

Run: `cargo check -p gitim-daemon`
Expected: 编译通过（现有代码不读 links 字段）

- [ ] **Step 2: 加 import 并创建 message_to_json() 辅助函数**

在 `crates/gitim-daemon/src/handlers.rs` 顶部加 import：

```rust
use gitim_core::types::{Link, LinkKind};
```

在 `handle_request` 函数之前（约 L10 前）加入辅助函数：

```rust
fn link_to_json(link: &Link) -> serde_json::Value {
    match &link.kind {
        LinkKind::Channel { name } => serde_json::json!({
            "kind": "channel",
            "name": name,
            "raw": link.raw,
        }),
        LinkKind::Message { channel, line_number } => serde_json::json!({
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

fn message_to_json(m: &gitim_core::types::Message) -> serde_json::Value {
    serde_json::json!({
        "line_number": m.line_number,
        "point_to": m.point_to,
        "author": m.author.as_str(),
        "timestamp": m.timestamp,
        "body": m.body,
        "mentions": m.mentions.iter().map(|h| h.as_str()).collect::<Vec<_>>(),
        "links": m.links.iter().map(link_to_json).collect::<Vec<_>>(),
    })
}
```

- [ ] **Step 3: 替换 handle_read 中的手动序列化（L228-239）**

将：

```rust
let json_msgs: Vec<serde_json::Value> = messages
    .iter()
    .map(|m| {
        serde_json::json!({
            "line_number": m.line_number,
            "point_to": m.point_to,
            "author": m.author.as_str(),
            "timestamp": m.timestamp,
            "body": m.body,
        })
    })
    .collect();
```

替换为：

```rust
let json_msgs: Vec<serde_json::Value> = messages
    .iter()
    .map(|m| message_to_json(m))
    .collect();
```

- [ ] **Step 4: 替换 handle_get_thread 中的手动序列化（L355-361）**

将：

```rust
thread_msgs.push(serde_json::json!({
    "line_number": msg.line_number,
    "point_to": msg.point_to,
    "author": msg.author.as_str(),
    "timestamp": msg.timestamp,
    "body": msg.body,
}));
```

替换为：

```rust
thread_msgs.push(message_to_json(msg));
```

- [ ] **Step 5: 替换 handle_poll 中的手动序列化（L491-503）**

将：

```rust
let messages: Vec<serde_json::Value> = parsed
    .messages
    .iter()
    .map(|m| {
        serde_json::json!({
            "line": m.line_number,
            "author": m.author.as_str(),
            "timestamp": m.timestamp,
            "body": m.body,
            "reply_to": if m.point_to == 0 { None } else { Some(m.point_to) },
        })
    })
    .collect();
```

替换为：

```rust
let messages: Vec<serde_json::Value> = parsed
    .messages
    .iter()
    .map(|m| message_to_json(m))
    .collect();
```

**注意：** poll 之前用的字段名是 `line`/`reply_to`，统一到 `line_number`/`point_to` 后需要同步更新 CLI 的 poll 解析代码。检查 `cli/src/commands/poll.ts` 或 `cli/src/client.ts` 是否硬编码了旧字段名。

- [ ] **Step 6: 编译验证**

Run: `cargo check -p gitim-daemon`
Expected: 编译通过

- [ ] **Step 7: 运行全部 Rust 测试**

Run: `cargo test`
Expected: 全部通过

- [ ] **Step 8: Commit**

```bash
git add crates/gitim-daemon/src/handlers.rs
git commit -m "refactor(daemon): extract message_to_json() + add mentions/links to API"
```

---

### Task 5: E2E 测试 — 链接序列化验证

**Files:**
- Modify: `tests/e2e_test.sh:146` (在 poll 测试之后、stop 测试之前插入)

**依赖:** Task 4（handlers.rs 序列化）

- [ ] **Step 1: 在 e2e_test.sh 中加入 link 测试**

在 `# Test: stop` 之前（约 L148）插入：

```bash
# === Test: Links in messages ===
echo "=== Test: Links ==="

# Send message with links
RES=$(echo '{"method":"send","channel":"general","body":"see <#general:L000001> and <!https://example.com|docs> and <~tester>"}' | nc -U "$SOCK" -w 2)
echo "$RES" | grep -q '"ok":true' || { echo "FAIL: send with links ($RES)"; exit 1; }
echo "PASS: send with links"

# Read and verify links in response
RES=$(echo '{"method":"read","channel":"general","since":3}' | nc -U "$SOCK" -w 2)
echo "Read with links: $RES"

# Verify links array exists and has 3 entries
LINK_COUNT=$(echo "$RES" | jq '[.data.messages[] | select(.links | length > 0)] | .[0].links | length')
if [ "$LINK_COUNT" -eq 3 ]; then
  echo "PASS: message has 3 links"
else
  echo "FAIL: expected 3 links, got $LINK_COUNT"
  echo "Full response: $RES"
  exit 1
fi

# Verify mentions array also present
HAS_MENTIONS=$(echo "$RES" | jq '[.data.messages[] | select(has("mentions"))] | length')
if [ "$HAS_MENTIONS" -gt 0 ]; then
  echo "PASS: mentions field present in response"
else
  echo "FAIL: mentions field missing"
  exit 1
fi
```

- [ ] **Step 2: 运行 E2E 测试**

Run: `bash tests/e2e_test.sh`
Expected: 全部通过，包括新增的 links 测试

- [ ] **Step 3: Commit**

```bash
git add tests/e2e_test.sh
git commit -m "test: add E2E test for link serialization in messages"
```

---

## Chunk 3: CLI 兼容性检查

### Task 6: 检查并修复 CLI poll 字段名变更

**Files:**
- Check: `cli/src/client.ts`, `cli/src/commands/poll.ts`

**依赖:** Task 4（poll 响应字段名从 `line`/`reply_to` 变为 `line_number`/`point_to`）

- [ ] **Step 1: 检查 CLI 中 poll 响应解析代码**

搜索 CLI 代码中对 `line`、`reply_to` 字段的引用。如果 CLI 硬编码了旧字段名，需要更新为 `line_number`/`point_to`。

Run: `grep -rn '"line"' cli/src/ && grep -rn 'reply_to' cli/src/`

- [ ] **Step 2: 更新 CLI 代码（如需要）**

将 `.line` 改为 `.line_number`，`.reply_to` 改为 `.point_to`。

- [ ] **Step 3: 运行 E2E 测试确认不回归**

Run: `bash tests/e2e_test.sh`
Expected: 全部通过

- [ ] **Step 4: Commit（如有改动）**

```bash
git add cli/src/
git commit -m "fix(cli): update poll response field names to match unified format"
```

---

## Summary

| Task | 内容 | 文件数 | 测试 |
|------|------|--------|------|
| 1 | Link/LinkKind 类型 | 2 | 编译验证 |
| 2 | extract_links() | 2 | 18 个单元测试 |
| 3 | parser 集成 | 2 | 4 个集成测试 |
| 4 | handlers.rs 重构 | 1 | 编译 + 旧测试不回归 |
| 5 | E2E 测试 | 1 | E2E shell 测试 |
| 6 | CLI 兼容性 | 0-2 | E2E 不回归 |

**总计:** 7-9 个文件，22+ 个新测试
