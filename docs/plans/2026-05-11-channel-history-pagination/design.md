# 频道消息历史翻页(Read 协议 since + limit 语义对齐)

> Design doc。实施任务清单由 `superpowers:writing-plans` 后续生成,见同目录 plan 文件。

## Goal

修复"频道里向上翻消息历史翻到一定程度就到顶,但消息明显还有更多"的 bug,同时把 Read 协议里被 limit 末尾切覆盖、当前事实上失效的 `since` 参数语义清理对齐。

## Background

WebUI 频道视图打开时调 `client.read(slug, channel, 50)`,daemon 返回末尾 50 条;前端 `setMessages` 全量替换,store 没有 prepend 路径,`message-list` 也没监听 scrollTop 到顶。用户看到的最早一条 = 全频道倒数第 50 条;再往上翻就到顶。

调查路径中发现 Read 协议存在更深的语义瑕疵:`since + limit` 组合下,`since` 实际被后续的"末尾切 limit 条"完全覆盖,等价于 `since=None`(只要频道行数 > limit)。换言之,`gitim read --since N --limit M` 的 CLI 公开参数在常见场景下行为与单传 `--limit M` 完全一致 —— `since` 是装饰字段。

## Decision

**不引入新协议字段**(不加 `before`)。利用 `line_number` 在协议层的连续性保证(由 `gitim-sync/src/renumber.rs:21` 的批次重编保证 thread 文件内 line_number 永远是密集整数序列),让 `since + limit` 同时承担"增量取新"和"翻历史"两个语义需求 —— 客户端按需算 `since = oldest_in_screen.line_number - limit - 1`。

为此需要把 `limit` 的切法在 `since` 在场/不在场两个分支上分化:
- `since` 不在场 → 末尾切(打开频道默认行为不变)
- `since` 在场 → 头切(让 `since` 真正生效,语义对齐"自 since 起向新方向取 N 条")

## Three-Mode Semantics

| 调用 | daemon 行为 | 用途 |
|---|---|---|
| `read(limit=N)` | 末尾 N 条 | 打开频道初次加载 |
| `read(since=K, limit=N)` | line > K 中的前 N 条 | 增量 poll(K = 已知最新);翻历史(K = oldest - limit - 1) |
| `read(since=K)` | line > K 全部 | 增量全拉(无截断,适用断网恢复场景) |

## Protocol & Daemon

**协议字段不变**。`gitim-daemon/src/api.rs::Request::Read`、`gitim-runtime/src/http.rs::ReadRequest` 保持现状。

**`crates/gitim-daemon/src/handlers/read.rs:75-84` 切法分支化:**

```rust
if let Some(since_line) = since {
    entries.retain(|e| e.line_number() > since_line);
}

if let Some(lim) = limit {
    if since.is_some() {
        entries.truncate(lim);                          // 头切
    } else {
        let start = entries.len().saturating_sub(lim);
        entries = entries[start..].to_vec();            // 末尾切
    }
}
```

**`crates/gitim-daemon/src/thread_io.rs:37-56::read_thread_entries` 同步对齐**。该函数被 `card_handlers.rs::handle_read_card` 复用(同样的 since + limit 模式),协议清理在 daemon 层一并落地,避免两套切法在不同 handler 间漂移。

## Frontend

**`products/gitim/frontend/`:**

- `lib/client.ts::read` 已支持 `limit` 入参;无需新增字段,调用方传 since 时复用现有 IPC 形参。
  - `daemon-web/handlers.ts::read` ([line 432-440](products/gitim/frontend/src/daemon-web/handlers.ts:432)) 同步对齐切法分支(local 模式 = remote 模式)。
  - `lib/backend.ts::read`(remote backend)只透传,无需改。

- `components/chat/use-chat-store`(或就近 store):新增 `prependMessages(msgs: Message[])` action,按 `line_number` dedup 后头部插入。

- `components/chat/chat-layout.tsx`:新增 `loadOlder()` 回调 —
  - 算 `since = oldestLine - limit - 1`,`since < 0` 时直接 return(到顶)
  - 调 `client.read(slug, channel, limit, since)`
  - 返回 0 条 → 标记 `hasMoreHistory = false`
  - 返回非零 → prepend,保持滚动锚点

- `components/chat/message-list.tsx`:加 `onScroll` 监听,scrollTop ≤ 50px 阈值触发 `loadOlder`,防抖避免并发请求;prepend 后用 `scrollTop = scrollTop + (newScrollHeight - prevScrollHeight)` 维持视觉位置不变。区分 append(新消息从底来)和 prepend(旧消息从顶来)以避免现有"消息变多就滚底"逻辑把用户拉回底部。

- Page size:**50**(与初次加载一致)。

## Compatibility

- **CLI `--since`**:行为变化 —— 从"末尾切被覆盖、实际无效"变为"自 since 起向新方向取 N 条"。当前是 latent bug,没有客户端在依赖该行为(WebUI 不传,CLI 命令行用户传了也无效)。`--help` 文案需对齐。
- **`Request::Poll` 的 `since`**:不动。Poll 的 since 是 commit-hash 字符串(`gitim-daemon/src/handlers/poll.rs:11`),与 Read 的 line-number since 是同名不同义,本次协议清理不波及。
- **`Request::ReadCard`**:经 `thread_io::read_thread_entries` 共享逻辑,自动获得对齐后的语义。card 翻页 UI 是 non-goal。
- **`since=None, limit=N`**:行为完全不变。

## Tests

**Daemon (`crates/gitim-daemon/src/handlers/read.rs` 测试模块):**
- `limit only` 末尾切回归
- `since only` 取 since 之后全部
- `since + limit` 头切 — 覆盖"增量 poll"和"翻历史"两种调用模式
- 边界:`since` 超过 max line(返回空)、`since=0`(等价无过滤)、`limit=0`(返回空)、空频道
- 与 `thread_io::read_thread_entries` 共享测试覆盖

**Frontend (`products/gitim/frontend/src/components/chat/`):**
- store `prependMessages` 的 dedup 行为
- `daemon-web/handlers.ts::read` 在 since+limit 模式下的头切语义(单元测试,无需启 daemon)
- `loadOlder` 在返回 0 条时设置 `hasMoreHistory = false`
- 滚动锚点保持(可走组件测试或 e2e)

**手工 QA:**
- 一个 ≥ 200 条消息的频道,从底部一路滚到顶,验证消息无跳跃、无重复、最终到达 line_number = 1
- 切换频道再切回,scroll position 恢复行为不被打破([chat-layout.tsx:325-354](products/gitim/frontend/src/components/chat/chat-layout.tsx:325) `handleNavBack`)

## Non-Goals

- 加新协议字段(`before` / `after` 等)
- 重命名 `since`
- card 翻页 UI / 看板内消息翻页
- `Request::Poll` 的 since 命名清理
- 服务端 streaming pagination(SSE-based history scroll)
- 长 thread 的 daemon 端 parse 性能优化(每次 read 仍全量 parse;非本任务范围)
- 跨频道历史搜索结果跳转(走 `/im/thread` 路径,与 history scroll 解耦)
