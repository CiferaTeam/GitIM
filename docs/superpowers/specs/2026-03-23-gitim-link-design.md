# GitIM Link 协议扩展设计

> **协议级链接功能：in-system link + softlink**
> 版本：1.0-draft | 作者：Lewis

---

## 1. 概述

为 GitIM 消息格式增加链接能力，支持两类链接：

- **in-system link**：指向 GitIM 内部实体（频道、消息、用户资料），纯导航，不触发通知。
- **softlink**：指向外部 URL 的跳转链接。

**与 mention 的区别：**

- `<@handler>` mention — 有通知语义，写入时验证 handler 存在性。
- `<~handler>` 用户资料链接 — 纯导航，不验证，不触发通知。

**本次范围：**

- 5 种链接语法定义
- `Link`、`LinkKind` 类型定义
- `Message` 结构体扩展
- 解析逻辑（从 body 提取 links）

**不在范围：**

- 写入验证（link 不验证目标存在性）
- 读取检测（link 目标可能被 archive 或删除，不告警）
- 前端渲染与交互

---

## 2. 语法

### 2.1 完整语法表

| 类型 | 前缀符号 | 语法 | 示例 |
|------|---------|------|------|
| 频道链接 | `#` | `<#channel>` | `<#general>` |
| 消息链接 | `#` | `<#channel:LNNNNNN>` | `<#general:L000042>` |
| 用户资料链接 | `~` | `<~handler>` | `<~bob>` |
| 外部裸链接 | `!` | `<!url>` | `<!https://example.com>` |
| 外部带标题链接 | `!` | `<!url\|显示文本>` | `<!https://x.com\|点击查看>` |

### 2.2 符号体系总览

与现有 mention 共享 `<符号...>` 体系：

| 符号 | 类型 | 语义 |
|------|------|------|
| `@` | mention | 通知 + 验证（已有） |
| `#` | 频道/消息链接 | 纯导航 |
| `~` | 用户资料链接 | 纯导航 |
| `!` | 外部链接 | 纯导航 |

### 2.3 语法规则

| 属性 | 值 |
|------|------|
| 格式 | `<` + 符号 + 内容 + `>` |
| 符号 | `#`、`~`、`!` |
| 位置 | MAY 出现在 body 的任意位置（首行、续行、行首、行中、行尾） |
| 数量 | 单条消息 MAY 包含零到多个链接 |
| 重复 | 不去重，按出现顺序保留所有实例 |

### 2.4 各类型内容规则

**频道链接 `<#channel>`：**
- channel 遵循 handler 字符集：`[a-z0-9]([a-z0-9-]*[a-z0-9])?`

**消息链接 `<#channel:LNNNNNN>`：**
- channel 同上
- `:L` 为固定分隔符
- 行号为 6 位及以上数字

**用户资料链接 `<~handler>`：**
- handler 遵循现有 Handler 规则：`[a-z0-9]([a-z0-9-]*[a-z0-9])?`

**外部链接 `<!url>` / `<!url|文本>`：**
- URL 必须是 RFC 3986 合法编码
- URL 中的 `|` 必须编码为 `%7C`，`>` 必须编码为 `%3E`
- `|` 为 URL 与显示文本的分隔符，取第一个裸 `|`
- 显示文本为 `|` 之后、`>` 之前的任意 UTF-8 文本

---

## 3. 数据结构

### 3.1 Link 类型

```rust
pub struct Link {
    pub kind: LinkKind,
    pub raw: String,
}

pub enum LinkKind {
    Channel { name: String },
    Message { channel: String, line_number: u64 },
    UserProfile { handler: Handler },
    Softlink { url: String, title: Option<String> },
}
```

### 3.2 Message 扩展

`Message` 新增 `links: Vec<Link>` 字段：

```rust
pub struct Message {
    pub line_number: u64,
    pub point_to: u64,
    pub author: Handler,
    pub timestamp: String,
    pub body: String,
    pub mentions: Vec<Handler>,
    pub links: Vec<Link>,       // 新增
}
```

---

## 4. 解析

### 4.1 提取正则

从完整 body（含续行）中提取所有非 mention 的协议级标记：

```
<([#~!])([^>]+)>
```

mention `<@handler>` 由现有 `mention.rs` 负责，`link.rs` 不处理。

### 4.2 分发逻辑

按匹配的首字符（capture group 1）分发：

| 首字符 | 解析逻辑 |
|--------|---------|
| `#` | 若内容含 `:L\d{6,}` → `Message { channel, line_number }`，否则 → `Channel { name }` |
| `~` | → `UserProfile { handler }` |
| `!` | 若内容含裸 `\|` → `Softlink { url, title }`，否则 → `Softlink { url, title: None }` |

### 4.3 解析流程

1. 按现有逻辑解析消息起始行和续行，组装完整 body。
2. 对完整 body 应用提取正则，收集所有匹配。
3. 对每个匹配按首字符分发，构建 `Link` 实例。
4. 格式不合法的匹配（如 channel 名不合法）静默忽略，不中断解析。
5. 按出现顺序存入 `links` 字段，不去重。

---

## 5. 验证

### 5.1 写入验证

**不验证。** Link 目标不要求存在。频道可能被 archive，用户可能离开，外部 URL 可能失效。

### 5.2 读取检测

**不检测。** Link 不影响消息结构完整性，不输出告警。

---

## 6. API 序列化

### 6.1 JSON 格式

`links` 跟随 message 一起返回，每个 link 平铺序列化：

```json
{
  "line_number": 42,
  "body": "看看 <#general:L000010> 那条，资料在 <!https://example.com|参考文档>",
  "mentions": [],
  "links": [
    { "kind": "message", "channel": "general", "line_number": 10, "raw": "<#general:L000010>" },
    { "kind": "softlink", "url": "https://example.com", "title": "参考文档", "raw": "<!https://example.com|参考文档>" }
  ]
}
```

### 6.2 kind 值

| LinkKind | JSON kind 值 |
|----------|-------------|
| Channel | `"channel"` |
| Message | `"message"` |
| UserProfile | `"user_profile"` |
| Softlink | `"softlink"` |

### 6.3 send 接口

**不变。** 客户端在 body 中直接写入标记语法，daemon 原样追加。

---

## 7. formatter 无变动

`formatter.rs` 接收 body 字符串原样输出。链接标记是 body 的一部分，formatter 不需要感知链接语法。

---

## 8. 边界情况

| 条件 | 规则 |
|------|------|
| `<#>` 空频道名 | 正则匹配但分发时格式不合法，静默忽略 |
| `<~>` 空 handler | 同上 |
| `<!>` 空 URL | 同上 |
| `<#GENERAL>` 大写 | channel 名不合法，静默忽略 |
| 未闭合 `<#general` | 正则不匹配，视为普通文本 |
| softlink URL 含裸 `\|` | 协议要求编码为 `%7C`，裸 `\|` 按分隔符处理 |
| softlink URL 含裸 `>` | 协议要求编码为 `%3E`，裸 `>` 按闭合标记处理 |
| `<!url\|>` 空显示文本 | `title` 为空字符串（非 None） |
| 同一链接出现多次 | 不去重，全部保留 |
| 单条消息链接数量 | 无上限 |

---

## 9. 影响范围

### 9.1 需要改动的文件

| 文件 | 改动 |
|------|------|
| `crates/gitim-core/src/link.rs` | **新增** — `Link`、`LinkKind` 类型 + `extract_links()` |
| `crates/gitim-core/src/types/message.rs` | 加 `links: Vec<Link>` 字段 |
| `crates/gitim-core/src/parser.rs` | 构建 Message 时调 `extract_links` |
| `crates/gitim-core/src/lib.rs` | 加 `pub mod link;` |

### 9.2 不需要改动的文件

| 文件 | 原因 |
|------|------|
| `formatter.rs` | link 是 body 一部分，格式化不感知 |
| `validator/compliance.rs` | 不验证 link 目标 |
| `validator/read_check.rs` | 不检测 link 目标 |
| `handlers.rs` | send 接口不变 |
| `mention.rs` | 保持独立 |
