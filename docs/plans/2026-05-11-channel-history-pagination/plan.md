# Channel History Pagination — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复"频道里向上翻历史到一定程度就到顶"的 bug,同时清理 Read 协议里 `since + limit` 当前被末尾切覆盖、事实上失效的语义。daemon 端把 `limit` 切法在 `since` 在/不在场两个分支上分化(头切 vs 末尾切);前端利用 `line_number` 的协议连续性算 `since = oldest_in_screen - limit - 1`,实现向旧翻页;message-list 监听 scrollTop 到顶触发 + prepend 后保持滚动锚点。

**Architecture:** 协议字段保持不变。daemon 的 `handlers/read.rs::handle_read` 和 `thread_io.rs::read_thread_entries` 两份独立的 since+limit 实现各自切法分支化;前端 `daemon-web/handlers.ts::read` 同步对齐(local 模式);`use-chat-store` 加 `prependMessages` action + `hasMoreHistory` flag;`chat-layout` 加 `loadOlder` 回调;`message-list` 通过 `onScrolledNearTop` prop 把"触顶"信号交给上层;prepend 后在 `message-list` 内通过 scrollHeight delta 保持视觉锚点。

**Tech Stack:** Rust(daemon,thread_io / handlers / api),React 19 + TypeScript + Zustand(frontend),Vitest + Testing Library(frontend tests),`cargo test -p gitim-daemon`(daemon tests)。

**Design doc:** [`design.md`](./design.md)

**Convention:** 每个 task 描述文件路径、变更目标、测试覆盖与验收。不内联代码;实现细节执行阶段由具体编辑者按上下文写。每个任务改完立刻 commit。

**TDD discipline:** 改 daemon 切法、改前端 store、改 daemon-web handler 都必须**先写失败测试 → 验证失败 → 实现 → 验证通过 → commit**。`message-list` 的 onScroll 监听和锚点保持这种 DOM-side effect 写组件测试(Testing Library + jsdom);手工 QA 是补充不是替代。

**Legacy test 检查:** 改 daemon since 行为时,先 grep 现有测试中 `since:` 关键字,凡是断言"末尾切"行为(等价于 `since=None`)的旧测试,要么删除(只是 cover 旧无效语义)、要么改写成新语义的断言。**不要保留任何只为兼容旧行为存在的测试或 helper**。

---

## Phase 0 · Baseline

### Task 0:跑一次全量 baseline,排除祖传红测干扰判断

**Files:** 不改动。

- [ ] **Step 1:** worktree 根跑 `cargo test --workspace --no-fail-fast`。期望 PASS,若有失败先记录,distinguish "相关 (gitim-daemon::read / thread_io / card_handlers)" vs "祖传红测"。
- [ ] **Step 2:** `cd products/gitim/frontend && npm test`(vitest 全量)。期望 PASS。
- [ ] **Step 3:** 不 commit。结果仅用于建 baseline。

**Acceptance:** baseline 记录在脑里(或 todowrite),后续 Phase 出现的红测能区分是不是本次引入的。

---

## Phase 1 · Daemon 协议对齐

### Task 1:`thread_io::read_thread_entries` 切法分支化(TDD)

**Files:**
- Modify: [`crates/gitim-daemon/src/thread_io.rs:37-57`](../../../crates/gitim-daemon/src/thread_io.rs:37) — `read_thread_entries` 的 limit 切法
- Create / Modify: 同 crate `tests/` 目录或文件末尾 `#[cfg(test)] mod tests`(若现有 thread_io 已有 test 文件,沿用;否则新建 inline test 模块)

**变更目标:**
- `read_thread_entries` 内,当 `since.is_some()` 时,把 `limit` 从末尾切(`entries[start..]` with `start = len - lim`)改为头切(`entries.truncate(lim)`);`since.is_none()` 时保持末尾切。
- 不改函数签名,不动 since 的 retain 逻辑(`line > since` 不变)。

**测试覆盖(TDD,在 Step 2 之前先写好):**
- `limit only, no since` — 末尾 N 条回归测试(验证旧行为不破)
- `since only` — 取 since 之后全部,无 limit
- `since + limit (新语义)` — 自 since 起的前 N 条,断言返回的 line_number 是 `[since+1, since+2, ..., since+lim]`
- `since 超出最大 line` — 返回空
- `since=0 + limit=N` — 等价 `limit=N` 取末尾(其实不,since=0 时 retain `line > 0` 留下全部,然后头切前 N → 拿到最早的 N 条而非末尾;断言这个新语义)
- `limit=0` — 返回空
- 空 thread 文件 — 返回空

**步骤:**
- [ ] **Step 1:** 在 `thread_io.rs` 测试模块(或对应 `tests/`)写上述 7 个 case 的 `#[test]`。每个 case 构造一个临时 thread 文件(`tempfile::NamedTempFile`)写入若干已知 line_number 的消息,调 `read_thread_entries`,assert 返回的 entries 数量和 line_number 序列。
- [ ] **Step 2:** 跑 `cargo test -p gitim-daemon thread_io -- --nocapture`。期望"since + limit 新语义" / "since=0" 这些 case FAIL(因为切法还是末尾)。
- [ ] **Step 3:** 改 `read_thread_entries`:把 `if let Some(lim) = limit { ... }` 块分支化 ——`since.is_some()` 走 `entries.truncate(lim)`,`since.is_none()` 走原 saturating_sub 末尾切。
- [ ] **Step 4:** 再跑 `cargo test -p gitim-daemon thread_io`。期望 PASS。
- [ ] **Step 5:** 同 crate 跑 `cargo test -p gitim-daemon card_handlers`(`handle_read_card` 依赖此函数,确认 card read 路径没有被本次改动破坏 —— 应该 OK,因为 card 测试不传 since)。
- [ ] **Step 6:** Commit:`feat(daemon): align thread_io read_thread_entries since+limit semantics`

**Acceptance:** thread_io 新测试全 PASS;card_handlers 既有测试不退;`since + limit` 在场时切法变成头切。

---

### Task 2:`handlers/read.rs::handle_read` 切法分支化(TDD)

**Files:**
- Modify: [`crates/gitim-daemon/src/handlers/read.rs:75-84`](../../../crates/gitim-daemon/src/handlers/read.rs:75) — handle_read 的 limit 切法
- Modify / Create: `crates/gitim-daemon/src/handlers/read.rs` 末尾 `#[cfg(test)] mod tests`(若无则新建),或对应 `tests/` 集成测试

**变更目标:**
- 跟 Task 1 同样分支化:`since.is_some()` → `entries.truncate(lim)`,否则末尾切。
- `read.rs` 没用 `thread_io::read_thread_entries`(它自己写了 retain + slice 逻辑),所以这是另一份独立改动。

**测试覆盖:**
- 跟 Task 1 同样 7 个 case,但走 `handle_read` 完整路径(`SharedState` mock + tempdir + 写入 channel meta + thread 文件 + 调 `handle_read` + 解析 `Response::data` 里 JSON entries)。
- 至少一个测试覆盖 channel membership check 不被改动影响(传一个不在 members 里的当前 user,断言 `Response::error("not_member")` —— 这是 read.rs 原有行为,不能因为切法改动而破)。

**步骤:**
- [ ] **Step 1:** Grep `crates/gitim-daemon/src/handlers/read.rs` 现有测试,确认 since 相关旧测试都期望什么行为。如果有"末尾切被覆盖"的隐式假设,记下来。
- [ ] **Step 2:** 写新测试 case(see "测试覆盖")。`cargo test -p gitim-daemon handle_read --no-run` 应编译通过,跑应 FAIL(切法还没改)。
- [ ] **Step 3:** 改 `read.rs:81-84`,跟 Task 1 同样的分支化。
- [ ] **Step 4:** 跑 `cargo test -p gitim-daemon handle_read`,期望 PASS。
- [ ] **Step 5:** 如果 grep 出来旧测试在 cover 旧无效行为(`since + limit` 期望末尾切),**改写或删除**这些测试,让它们要么 cover 新语义、要么不再存在。不要保留"对旧行为兼容"的死代码。
- [ ] **Step 6:** `cargo test -p gitim-daemon` 全量 PASS。
- [ ] **Step 7:** Commit:`feat(daemon): align handle_read since+limit semantics`

**Acceptance:** `cargo test -p gitim-daemon` 全绿;现有 read.rs since 测试要么转为新语义、要么删除;新增 case 覆盖三种调用模式。

---

## Phase 2 · 前端 daemon-web local backend

### Task 3:`daemon-web/handlers.ts::read` 切法分支化(TDD)

**Files:**
- Modify: [`products/gitim/frontend/src/daemon-web/handlers.ts:430-440`](../../../products/gitim/frontend/src/daemon-web/handlers.ts:430) — `read` 函数 limit 切法
- Modify: [`products/gitim/frontend/src/daemon-web/handlers.test.ts`](../../../products/gitim/frontend/src/daemon-web/handlers.test.ts) — 新增 read 测试 case

**变更目标:**
- 跟 daemon 端语义对齐:`since` 在场时 `entries.slice(0, limit)`(头切),否则 `entries.slice(-limit)`(末尾切)。
- `daemon-web/handlers.ts::read` 当前签名 `read(channel: string, limit?: number)` 没有 `since` 入参!这是因为 daemon-web v1 没暴露 since。需要新增 `since?: number` 第三参数。
- 同步更新 `daemon-web/handlers.ts:83` 类型定义(若有)和 `lib/backend.ts::LocalBackend::read` 调用点透传(若需要)。

**测试覆盖:**
- `limit only` — 末尾 N 条(回归)
- `since only` — 自 since 之后全部
- `since + limit` — 自 since 起前 N 条
- `since` 超出最大 line — 返回空
- `since` + `limit=0` — 返回空

**步骤:**
- [ ] **Step 1:** Grep `daemon-web/handlers.test.ts` 现有 read 相关测试。补 5 个 case 的 `it(...)` 断言。
- [ ] **Step 2:** `npm test -- handlers` FAIL(since 入参不存在;或切法不对)。
- [ ] **Step 3:** 改 `handlers.ts::read` 签名加 `since?: number`,加 retain `line > since`,加切法分支(同 daemon 语义)。
- [ ] **Step 4:** 改 [`backend.ts:185-192`](../../../products/gitim/frontend/src/lib/backend.ts:185) 的 `LocalBackend::read`(若签名需要变),让 `since` 能从上层传下来。检查 [`lib/backend.ts:59`](../../../products/gitim/frontend/src/lib/backend.ts:59) 的接口定义同步加 `since?` 字段。
- [ ] **Step 5:** 改 [`lib/client.ts:559-573`](../../../products/gitim/frontend/src/lib/client.ts:559) 的 `read` 接受 `since?: number` 第四参数,local 路径透传给 `activeBackend.read(channel, limit, since)`,remote 路径放进 POST body `{ channel, limit, since }`(remote daemon 已经接受 since,本次只是前端 expose 出来)。
- [ ] **Step 6:** `npm test` PASS。
- [ ] **Step 7:** Commit:`feat(daemon-web): align read since+limit semantics and expose since`

**Acceptance:** daemon-web 单元测试覆盖三种调用模式;`client.read` 签名加 `since?` 参数;remote 模式 POST body 多个 `since` 字段(daemon 已接受,无需 daemon 改)。

---

## Phase 3 · 前端 store

### Task 4:`use-chat-store` 加 `prependMessages` action 和 `hasMoreHistory` flag(TDD)

**Files:**
- Modify: [`products/gitim/frontend/src/hooks/use-chat-store.ts`](../../../products/gitim/frontend/src/hooks/use-chat-store.ts)
- Modify: [`products/gitim/frontend/src/hooks/use-chat-store.test.ts`](../../../products/gitim/frontend/src/hooks/use-chat-store.test.ts)

**变更目标:**
- 加 state `hasMoreHistory: boolean`(初始 `true`)。
- 加 action `prependMessages(msgs: Message[])` —— 按 `line_number` 去重(已有的 line_number 跳过),仅 prepend 新的;同时按 line_number 升序合并(防御性:即使入参顺序不保证)。
- 加 action `setHasMoreHistory(v: boolean)`。
- `setMessages([])`(切换 channel 时)要同时 reset `hasMoreHistory = true`(回到"未知是否到顶"状态)。

**测试覆盖:**
- `prependMessages` 在空 messages 上 → 等价 setMessages
- `prependMessages` 在已有 messages 上 → 前部插入,顺序 line_number 升序
- `prependMessages` 包含与现有重复 line_number → 重复项被跳过,不复制
- `prependMessages` 空数组 → 无副作用
- 切 channel(`setMessages([])`)后 `hasMoreHistory` 回到 `true`

**步骤:**
- [ ] **Step 1:** 在 `use-chat-store.test.ts` 加上面 5 个 case 的 `it(...)`。FAIL(action 不存在 / `hasMoreHistory` 没字段)。
- [ ] **Step 2:** `npm test -- use-chat-store` 验证 FAIL。
- [ ] **Step 3:** 在 `use-chat-store.ts` 的 state 接口加 `hasMoreHistory`,实现两个 action。在 `setMessages` 内 reset `hasMoreHistory`。
- [ ] **Step 4:** `npm test -- use-chat-store` PASS。
- [ ] **Step 5:** Commit:`feat(frontend): add prependMessages action and hasMoreHistory flag`

**Acceptance:** store 测试新增 case 全过;现有测试不退。

---

## Phase 4 · 前端 UI 翻页

### Task 5:`chat-layout` 加 `loadOlder` 回调

**Files:**
- Modify: [`products/gitim/frontend/src/components/chat/chat-layout.tsx`](../../../products/gitim/frontend/src/components/chat/chat-layout.tsx)
- Modify: [`products/gitim/frontend/src/components/chat/chat-layout.test.tsx`](../../../products/gitim/frontend/src/components/chat/chat-layout.test.tsx)(若存在;无则新建简单的 component test)

**变更目标:**
- 提取常量 `MESSAGES_PAGE_SIZE = 50` 放在文件顶部(三处现存 `client.read(..., 50)` 调用都改用此常量,保持单一来源)。
- 新增 `handleLoadOlder` `useCallback`:
  - 从 store 读当前 `messages`,取 `oldestLine = messages[0]?.line_number`(空时直接 return)。
  - 读 store `hasMoreHistory`,false 直接 return。
  - 读一个本地 `loadingOlderRef = useRef(false)`,fetch 中拒绝重入。
  - 计算 `since = oldestLine - MESSAGES_PAGE_SIZE - 1`,`since < 0` 则视为已到顶,设 `setHasMoreHistory(false)` 后 return。
  - `await client.read(slug, channel, MESSAGES_PAGE_SIZE, since)`,如果 workspace 已切走(`isCurrentWorkspaceRequest` 检查),丢弃。
  - 返回 `entries.length < MESSAGES_PAGE_SIZE` → `setHasMoreHistory(false)`(包括 0 条)。
  - 返回非空 → `prependMessages(entries)`。
  - 失败(`res.ok === false`)→ console.warn,不 toast;不更新 `hasMoreHistory`(下次重试)。
- 把 `handleLoadOlder` 作为 prop 传给 `MessageList`(Task 6 接它)。

**测试覆盖:**
- 给 `handleLoadOlder` 写单测较繁琐(依赖整 chat-layout 上下文);用 component test 模拟 store 状态 + mock `client.read`,断言:
  - 空 messages → 不调 client.read
  - oldestLine - PAGE_SIZE - 1 < 0 → 设 hasMoreHistory=false
  - 正常翻页 → client.read 被调用,since 算式正确
  - 返回少于 PAGE_SIZE → hasMoreHistory 设 false
  - 返回 fail → hasMoreHistory 不变,console.warn 被调
  - 二次触发在第一次未返时 → 第二次被防抖丢弃

**步骤:**
- [ ] **Step 1:** 把现有 3 处 `client.read(requestSlug, apiChannel, 50)` 改为 `client.read(requestSlug, apiChannel, MESSAGES_PAGE_SIZE)`(纯重构,跑现有测试应 PASS),Commit:`refactor(frontend): extract MESSAGES_PAGE_SIZE constant`。
- [ ] **Step 2:** 写 component test 上面 6 个 case(`handleLoadOlder` 通过 mock + 暴露 ref/prop 模拟触发)。FAIL(handler 未实现)。
- [ ] **Step 3:** `npm test -- chat-layout` 验证 FAIL。
- [ ] **Step 4:** 实现 `handleLoadOlder` 并通过 `onLoadOlder` prop 传给 `MessageList`(此时 `MessageList` 端还没接,prop 暂时未消费,Task 6 接)。
- [ ] **Step 5:** `npm test -- chat-layout` PASS。
- [ ] **Step 6:** Commit:`feat(frontend): add handleLoadOlder in chat-layout`

**Acceptance:** chat-layout 测试覆盖防抖、边界、to-top 判定;`MESSAGES_PAGE_SIZE` 单一来源。

---

### Task 6:`message-list` scrollTop 到顶检测 + prepend 后锚点保持

**Files:**
- Modify: [`products/gitim/frontend/src/components/chat/message-list.tsx`](../../../products/gitim/frontend/src/components/chat/message-list.tsx)
- Modify / Create: `products/gitim/frontend/src/components/chat/message-list.test.tsx`

**变更目标:**
- `MessageListProps` 加 `onLoadOlder?: () => void`(可选,缺省时不触发 — 兼容 card / thread 场景)。
- 在 `scrollRef` 上加 `onScroll` 监听:`scrollTop <= 50` 时调 `onLoadOlder`(由父端的防抖兜底重入)。
- prepend 锚点保持:用 `prevMessagesRef` 记录上一次 messages 的最早 line_number;当 effect 触发时,如果新 messages 的最早 line_number **小于** 旧的最早 line_number(即发生了 prepend)而非新消息从底部追加:
  - 在 effect 内记下 `prevScrollHeight`,DOM 更新后(用 `useLayoutEffect` 或 `requestAnimationFrame`)把 `scrollTop = scrollTop + (newScrollHeight - prevScrollHeight)`。
- 修当前 [`message-list.tsx:80-82`](../../../products/gitim/frontend/src/components/chat/message-list.tsx:80) 的"消息变多就滚底"逻辑:从 `messages.length > prev` 改为 `last line_number > prev last line_number`(append 才滚底,prepend 不滚底)。否则 prepend 会因为 length 增长而被错误地滚到底,毁掉锚点。

**测试覆盖:**
- scrollTop ≤ 50 时 `onLoadOlder` 被调
- scrollTop > 50 时 `onLoadOlder` 不被调
- 新增最新消息(append)→ `scrollTop = scrollHeight` (滚到底,验证回归不退)
- prepend(messages 头部插入更老消息)→ scrollTop 调整为 `scrollTop + (newScrollHeight - oldScrollHeight)`,视觉锚点不变
- `onLoadOlder` 未提供时,scrollTop=0 不报错

**步骤:**
- [ ] **Step 1:** 写 component test 上面 5 个 case。FAIL。
- [ ] **Step 2:** `npm test -- message-list` 验证 FAIL。
- [ ] **Step 3:** 改 `message-list.tsx`:加 prop、onScroll、`useLayoutEffect` 锚点保持、修 append 判定。
- [ ] **Step 4:** 改 `chat-layout.tsx` 在调用 `MessageList` 处把 Task 5 实现的 `handleLoadOlder` 作为 `onLoadOlder` prop 传入(把 Task 5 的 dangling prop 接上)。
- [ ] **Step 5:** `npm test -- message-list && npm test -- chat-layout` PASS。
- [ ] **Step 6:** Commit:`feat(frontend): infinite scroll up in channel history`

**Acceptance:** 5 个 message-list 测试全过;chat-layout 测试不退;新消息 append 仍滚到底,prepend 不滚动。

---

## Phase 5 · CLI 文案对齐

### Task 7:`gitim read --since` `--help` 文案对齐新语义

**Files:**
- Modify: [`crates/gitim-cli/src/main.rs:262`](../../../crates/gitim-cli/src/main.rs:262) 和 [`crates/gitim-cli/src/main.rs:315`](../../../crates/gitim-cli/src/main.rs:315) — read / dm-read 子命令的 `--since` flag doc comment(clap derive)
- Modify: [`crates/gitim-cli/src/main.rs:58`](../../../crates/gitim-cli/src/main.rs:58) — read-card 子命令同样 flag

**变更目标:**
- `--since N` 的 doc string 改为类似 "Return messages with line_number > N, capped to --limit. Combine with --limit to page forward through history (recent direction); pair with --limit to fetch older messages by computing N = oldest_in_view - limit - 1." 中文 OK 但与现有 CLI 风格保持一致(看仓库其他 doc comment 风格)。
- 不改任何代码逻辑,仅 doc comment。

**步骤:**
- [ ] **Step 1:** Grep `main.rs` 现有的 `--since` doc string,看现有风格(中/英、长短)。
- [ ] **Step 2:** 改三处 doc string,描述新语义和典型 use case。
- [ ] **Step 3:** `cargo run -p gitim-cli -- read --help`、`gitim dm read --help`、`gitim card read --help` 三处人眼检查(或写一个 snapshot 测试,但 cli help snapshot 易脆,优先人眼)。
- [ ] **Step 4:** `cargo build -p gitim-cli` 通过。
- [ ] **Step 5:** Commit:`docs(cli): clarify --since semantics for read commands`

**Acceptance:** 三个 read 子命令的 `--help` 输出能让用户理解 since + limit 的"翻页"用法。

---

## Phase 6 · 收尾

### Task 8:全量回归 + 手工 QA

**Files:** 不改动。

- [ ] **Step 1:** 在 worktree 根跑 `cargo test --workspace --no-fail-fast`。期望全 PASS,且失败集合不大于 Phase 0 baseline。
- [ ] **Step 2:** `cd products/gitim/frontend && npm test`。期望 PASS。
- [ ] **Step 3:** `cd products/gitim/frontend && npm run build`。期望成功(tsc 全过 + vite build 无错)。
- [ ] **Step 4:** `cargo build --workspace` 跑通(catch 任何 daemon 那边的 lint / 警告)。
- [ ] **Step 5:** 手工 QA:
  - 准备一个 ≥ 200 条消息的 channel(可在 dev daemon 里跑 cli 灌一批消息脚本)
  - 启动 dev runtime + daemon-web 前端,打开该 channel
  - 从底部一路滚到顶,验证:消息流畅 prepend、无重复、最终到达 line_number = 1、`hasMoreHistory=false` 后不再触发 fetch
  - 切到另一个 channel 再切回,scroll position 恢复行为不被打破([`chat-layout.tsx:325-354`](../../../products/gitim/frontend/src/components/chat/chat-layout.tsx:325) handleNavBack)
  - 在小 channel(< PAGE_SIZE 条)打开,验证初次 fetch 后立即 `hasMoreHistory=false`,scrollTop=0 不报错也不发请求
- [ ] **Step 6:** 不 commit。手工 QA 结果回报给主线决定后续。

**Acceptance:** 全量 cargo test + frontend vitest + tsc 全过;手工 QA 五个场景行为符合预期。

---

## Done definition

- Bug 修复:用户在大 channel 里能从底滚到 line_number=1,无截断、无重复、无视觉跳跃
- 协议清理:`since + limit` 三种调用模式语义对齐(末尾切 / since 之后全部 / 自 since 起前 N 条)
- daemon `read.rs` + `thread_io.rs` 两份实现同步对齐
- daemon-web `handlers.ts::read` 暴露并对齐 `since` 参数
- 所有新行为有 TDD 测试覆盖,旧测试要么删除要么改写,**无僵尸测试**
- CLI `--since` doc string 反映新语义
- 全量回归绿,手工 QA 通过
