# GitIM Mention 协议扩展设计

> **协议级 @mention 功能**
> 版本：1.0-draft | 作者：Lewis

---

## 1. 概述

为 GitIM 消息格式增加 mention 能力，允许消息正文中通过特定语法引用已注册用户。

**核心区分：**

- **协议级 mention**：`<@handler>` — 经过写入验证，handler MUST 存在于 `users/`。未来版本将触发通知推送。
- **语义级 mention**：裸写 `@someone` — 纯文本，无任何协议行为，不验证，不索引。

**本次范围：**

- mention 语法定义
- `Message` 结构体扩展
- 解析逻辑（从 body 提取 mentions）
- 写入验证（handler 存在性检查）
- 读取检测（不存在时告警）

**不在范围：**

- daemon 内存索引与推送通知（SSE/长连接订阅）
- CLI 侧 mention 补全或高亮
- daemon 启动优化（tail 热点 + 异步全量加载）

---

## 2. 语法

### 2.1 协议级 mention

```
<@handler>
```

嵌入在消息 body 中（含首行和续行），示例：

```
[L000001][P000000][@nexus][20250316T120000Z] <@lewis> 请看一下部署配置
[L000002][P000001][@lewis][20250316T120500Z] 收到，<@coder> 你也帮忙看看
这里有个问题需要 <@cifera-nexus> 确认
```

### 2.2 语义级 mention

裸写 `@someone`，不带尖括号：

```
[L000003][P000000][@nexus][20250316T121000Z] 之前 @某位同事 提过这个方案
```

daemon 对此不做任何处理。

### 2.3 语法规则

| 属性 | 值 |
|------|------|
| 格式 | `<@` + handler + `>` |
| handler 字符集 | 与 3.2 Handler 规则一致：`^[a-z0-9]([a-z0-9-]*[a-z0-9])?$` |
| 边界 | `<@` 为起始标记，`>` 为终止标记，无歧义 |
| 位置 | MAY 出现在 body 的任意位置（首行、续行、行首、行中、行尾） |
| 数量 | 单条消息 MAY 包含零到多个 mention |
| 重复 | 同一 handler MAY 在同一条消息中被 mention 多次，解析结果去重 |

---

## 3. 解析

### 3.1 提取正则

从完整 body（含续行）中提取协议级 mention：

```
<@([a-z0-9]([a-z0-9-]*[a-z0-9])?)>
```

### 3.2 Message 结构体扩展

在 `Message` 结构体中增加字段：

```rust
pub struct Message {
    pub line_number: u64,
    pub point_to: u64,
    pub author: Handler,
    pub timestamp: String,
    pub body: String,
    pub mentions: Vec<Handler>,  // 新增：从 body 解析出的协议级 mention，去重
}
```

### 3.3 解析流程

1. 按现有逻辑解析消息起始行和续行，组装完整 body（含 CRLF 规范化、续行空格转义剥离）。
2. 对完整 body 应用提取正则，收集所有匹配的 handler 字符串。
3. 对每个匹配调用 `Handler::new()` 验证格式（正则不检查连续连字符等约束，需复用 Handler 验证逻辑）。
4. 去重后按首次出现顺序存入 `mentions` 字段。
5. 格式不合法的匹配（如果 `Handler::new()` 失败）静默忽略，不中断解析。

---

## 4. 写入验证

### 4.1 新增检查项

在 `validate_append` 的消息遍历循环中，新增：

| 检查项 | 规则 |
|--------|------|
| mention 存在性 | 消息 `mentions` 中的每个 handler MUST 存在于 `registered_users` 中 |

### 4.2 错误类型

新增 `ComplianceError` 变体：

```rust
#[error("unknown mention '<@{handler}>' in message L{line_number:06}")]
UnknownMention { handler: String, line_number: u64 },
```

### 4.3 行为

- mention 的 handler 不在 `registered_users` 中 → 拒绝整条消息写入。
- 用户若想表达"不存在的人"，应使用裸写 `@someone`（语义级），不会被拦截。

---

## 5. 读取检测

### 5.1 检测规则

在 `read_check` 中，对每条消息的 `mentions` 执行 handler 存在性检查：

- handler 不在已知用户列表中 → 输出 `warn!` 级别日志，包含文件路径、行号和无效 handler。
- 消息本身不标记为 `corrupted`（mention 不影响消息结构完整性）。
- 消息正常纳入索引，告警仅供排查。

### 5.2 场景

此检测捕获的典型场景：

- 用户被删除后，历史消息中仍有对其的 mention。
- 有人绕过 daemon 直接 git commit 了包含无效 mention 的消息。

---

## 6. formatter 无变动

`formatter.rs` 接收 body 字符串原样输出。`<@handler>` 是 body 的一部分，formatter 不需要感知 mention 语法。

---

## 7. 边界情况

| 条件 | 规则 |
|------|------|
| `<@>` 空 handler | 正则不匹配，视为普通文本 |
| `<@LEWIS>` 大写 | 正则不匹配（handler 限定小写），视为普通文本 |
| `<@system>` 保留字 | `Handler::new()` 拒绝，不纳入 mentions |
| `<@foo--bar>` 连续连字符 | 正则匹配，但 `Handler::new()` 拒绝；静默忽略 |
| 未闭合 `<@lewis` | 正则不匹配，视为普通文本 |
| 嵌套 `<@<@lewis>>` | 外层 `<@` 后遇到 `<`（不在 handler 字符集）匹配失败；引擎继续扫描，找到内层 `<@lewis>` 并提取 `lewis` 作为有效 mention，多余的尾部 `>` 被忽略 |
| 转义需求 | 无。如果用户不想触发协议级 mention，不加尖括号即可 |
| body 中出现 `<@handler>` 完全相同的文本多次 | 解析去重，mentions 列表中只出现一次 |
| 单条消息 mention 数量 | 无上限 |
