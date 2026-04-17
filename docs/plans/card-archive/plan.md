# Card 归档功能实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: 用 `superpowers:subagent-driven-development` 或 `superpowers:executing-plans` 按任务逐步执行。步骤用 checkbox(`- [ ]`)跟踪。
>
> **Plan 编写约定**: 本文档只写分工(改什么文件、做什么行为、怎么测),**不贴具体 Rust 代码**。实现时由 engineer 基于代码库既有 pattern 自行编写。

---

## Goal

给看板 card 增加"归档/取消归档"能力,归档后从活跃列表消失、只读、支持恢复。

## Architecture

**路径移位式归档**(与 `archive/channels/` 同一根部,但 card 以**整目录** `git mv`):

- 活跃: `channels/<ch>/cards/<card_id>/`
- 归档: `archive/channels/<ch>/cards/<card_id>/`

`ListCards` 扫描 root 不含 `archive/`,**零改动**即可正确忽略归档 card。

**核心不变量**:
- 归档是生命周期维度,`CardMeta.status`(todo/doing/done)是工作进度维度,**正交**,不耦合
- 归档态 card **只读**:拒绝 `UpdateCard` / `SendCardMessage`
- 支持 `UnarchiveCard` 反向恢复(误操作/重新打开场景)
- 权限:`card.created_by == author` OR `card.assignee == Some(author)`(`creator OR assignee`)
- Meta schema **不加** `archived_at/archived_by` 字段,归档信息由**路径**表达,审计靠 git log

## Tech Stack

- Rust workspace(`gitim-core` / `gitim-daemon` / `gitim-cli`)
- 既有 idioms: `GitStorage::mv`、`add_and_commit_as`、`push_with_retry`、`Event::*`、serde + clap
- 测试:`cargo test -p gitim-daemon --lib`(daemon 集成测试),`cargo test -p gitim-core`(类型)

## Scope

### In-Scope (v1)

- 后端 API: `ArchiveCard` / `UnarchiveCard` / `ListArchivedCards` / `ReadCard` auto-fallback
- `locate_card` 辅助函数(统一路径查找)
- 生命周期约束: `SendCardMessage` / `UpdateCard` 拒绝归档 card
- 新 Event variants: `CardArchived` / `CardUnarchived`
- CLI: `gitim card archive` / `unarchive` / `archived`
- 单元 + 集成测试覆盖 happy path + 拒绝 path

### Out-of-Scope (follow-up)

- **WebUI**: `webui-v2/` 目前无看板 UI,无落点;等看板功能落地后 v2 补
- **`gitim-runtime/src/http.rs` 对 archive/unarchive/list_archived 的 HTTP endpoint 暴露**:现状 runtime http 未暴露 `archive_channel`(channel archive 也只走 CLI/IPC),本次保持对齐。未来 WebUI 看板落地时,与 channel archive 一起补 HTTP
- **`gitim-index` 归档搜索**: 归档 card 的 `discussion.thread` 不在 index 扫描 root 内,v1 接受"归档 card 消息搜不到";未来对齐 `include_cards` flag 加 `include_archived`
- **Channel 归档级联 cards**: 归档 channel 时 `channels/<ch>/cards/` 留为孤儿(既有遗留 bug),与本次无关,独立 ticket

### Known Smells(记录不修)

- `archive/channels/` 下既有平铺文件(`<ch>.{thread,meta.yaml}` from channel archive)也会有子目录(`<ch>/cards/<id>/` from card archive),依赖文件系统 namespace 区分。未来 channel 归档级联 cards 时可统一
- `locate_card` 若活跃和归档**都存在**同一 card_id(手工 git 污染),优先活跃并 log warning,不硬 fail
- `handle_archive_card` / `handle_unarchive_card` 代码高度对称(~99%),未抽共享辅助。Follow-up 可考虑 `move_card(direction: CardMoveDirection)` 统一。本 v1 接受并靠测试覆盖控制 drift 风险。

---

## File Structure Map

| 文件 | 动作 | 责任 |
|---|---|---|
| `crates/gitim-daemon/src/api.rs` | 修改 | `Request` enum 加 3 个 variant(Archive/Unarchive/ListArchived Cards);`Event` enum 加 2 个 variant(CardArchived/CardUnarchived) |
| `crates/gitim-daemon/src/card_handlers.rs` | 修改 | 新增 `locate_card` 辅助;新增 3 个 handler(archive/unarchive/list_archived);修改 `handle_read_card` 用 locate_card + 返回 `archived` 字段;修改 `handle_send_card_message` / `handle_update_card` 拒绝归档 |
| `crates/gitim-daemon/src/handlers.rs` | 修改 | 在主 dispatch match 里加 3 个新 Request variant 路由 |
| `crates/gitim-daemon/tests/` 或 inline `#[cfg(test)]` | 新增 | 各 handler 的单元测试 + 集成测试 |
| `crates/gitim-client/src/lib.rs` | 修改 | 封装 3 个新 IPC 调用方法(archive_card / unarchive_card / list_archived_cards) |
| `crates/gitim-cli/src/commands/card.rs` | 修改 | 加 3 个 clap 子命令 + 对应命令处理函数 |
| `crates/gitim-cli/src/main.rs` 或 cli 路由 | 修改(如需) | 注册新子命令 |

---

## Task 1 — API schema 扩展 + Event variants

**Files:**
- Modify: `crates/gitim-daemon/src/api.rs`
- Test: `crates/gitim-daemon/src/api.rs`(inline `#[cfg(test)]`)或新建 tests fixture

**参考位置:** `api.rs:153-160`(现有 `ArchiveChannel` / `ListArchivedChannels`)、`api.rs:31-40`(现有 `CardCreated` / `CardStatusChanged`)

### Steps

- [ ] **Step 1.1 — 写失败的 serde 测试**

  在 `api.rs` 的 test module 增加 3 个 roundtrip 测试:
  - `test_archive_card_request_roundtrip`: 构造 `Request::ArchiveCard { channel, card_id, author }`,序列化为 JSON,验证 `"type": "archive_card"`,反序列化回等价结构
  - `test_unarchive_card_request_roundtrip`: 类似,`type = "unarchive_card"`
  - `test_list_archived_cards_request_roundtrip`: `type = "list_archived_cards"`,`channel: Option<String>` 可省略(None)
  - `test_card_archived_event_roundtrip`: `Event::CardArchived { channel, card_id, author }`,`type = "card_archived"`
  - `test_card_unarchived_event_roundtrip`: 类似

- [ ] **Step 1.2 — 跑测试,确认都 FAIL**

  `cargo test -p gitim-daemon --lib api::tests -- --nocapture`
  预期:全部 FAIL(variant 不存在)

- [ ] **Step 1.3 — 扩展 `Request` enum**

  在 `Request` 定义里(参照 `ArchiveChannel` 的写法,`api.rs:153`),加 3 个 variant:
  - `ArchiveCard { channel: String, card_id: String, author: String }`,`#[serde(rename = "archive_card")]`
  - `UnarchiveCard { channel: String, card_id: String, author: String }`,`#[serde(rename = "unarchive_card")]`
  - `ListArchivedCards { #[serde(default)] channel: Option<String> }`,`#[serde(rename = "list_archived_cards")]`

- [ ] **Step 1.4 — 扩展 `Event` enum**

  参照 `CardCreated`(`api.rs:31-34`),加:
  - `CardArchived { channel: String, card_id: String, author: String }`,`#[serde(rename = "card_archived")]`
  - `CardUnarchived { channel: String, card_id: String, author: String }`,`#[serde(rename = "card_unarchived")]`

- [ ] **Step 1.5 — 跑测试,确认 PASS**

  `cargo test -p gitim-daemon --lib api::tests`
  预期:全部 PASS

- [ ] **Step 1.6 — 确认其余测试不回归**

  `cargo check --workspace`(因为 `handlers.rs` 里对 `Request` 的 match 现在不全了,应该**故意不改 match**让编译器报 non-exhaustive,提醒 Task 7 要修)
  实际操作:运行 `cargo check -p gitim-daemon`,**预期编译错误** `non-exhaustive patterns`。这是期望的 ——Task 7 会补全 match。

  **决策:允许 Task 1 结束后 workspace 不编译**(因为 Task 7 要修 match)。若想保持每步可编译,在 Step 1.4 之后立刻跳到 Task 7 的 Step 7.1(加占位 `_ =>` arm),再回来做 Task 2/3/4/5/6。**推荐做法:不加占位 arm,让编译错误做"你欠我一个 handler"的自然提醒**。

- [ ] **Step 1.7 — commit**

  ```bash
  git add crates/gitim-daemon/src/api.rs
  git commit -m "feat(api): add ArchiveCard/UnarchiveCard/ListArchivedCards requests and Card{Un,}Archived events"
  ```

---

## Task 2 — `locate_card` 辅助 + `ReadCard` auto-fallback

**Files:**
- Modify: `crates/gitim-daemon/src/card_handlers.rs`
- Test: 同文件 inline tests(对齐既有惯例)

**参考位置:**
- `card_handlers.rs:286` (`handle_read_card`)
- `card_handlers.rs:12-22` (`validate_card_id`,作为辅助函数的邻居)

### Steps

- [ ] **Step 2.1 — 写 `locate_card` 的单元测试**

  加 test:
  - `test_locate_card_finds_active_path`:fixture 建 `channels/foo/cards/abc/`,调用 `locate_card("foo", "abc")`,期望返回 `{rel_path: "channels/foo/cards/abc", is_archived: false}`
  - `test_locate_card_finds_archived_path`:fixture 建 `archive/channels/foo/cards/abc/`,期望返回 `{rel_path: "archive/channels/foo/cards/abc", is_archived: true}`
  - `test_locate_card_prefers_active_when_both_exist`:两个路径都 setup,期望返回活跃 + `is_archived: false`(异常态不硬 fail)
  - `test_locate_card_not_found`:两个都不存在,期望返回 `Err(CardNotFound)` 或等价

- [ ] **Step 2.2 — 跑测试确认 FAIL**

- [ ] **Step 2.3 — 实现 `locate_card`**

  在 `card_handlers.rs` 的辅助函数区(`validate_card_id` 下方附近)加:
  - 定义 `LocatedCard` struct(字段:`rel_path: String`, `is_archived: bool`)
  - 定义 `fn locate_card(state: &SharedState, channel: &ChannelName, card_id: &str) -> Option<LocatedCard>`
  - 逻辑:先检查 `channels/<ch>/cards/<id>/card.meta.yaml` 存在性;存在 → 返回活跃。否则检查 `archive/channels/<ch>/cards/<id>/card.meta.yaml`;存在 → 返回归档 + log warning if 活跃也存在。否则 `None`

- [ ] **Step 2.4 — 跑测试 PASS**

- [ ] **Step 2.5 — 写 `handle_read_card` 的 archived 字段测试**

  - `test_read_card_active_returns_archived_false`: fixture 活跃 card + 发一条消息,ReadCard,期望响应 JSON 包含 `"archived": false`
  - `test_read_card_archived_returns_archived_true_and_messages`: fixture 归档 card(手工搬到 archive/ 目录),ReadCard,期望 `"archived": true` 且消息数组非空(归档后仍可读)
  - `test_read_card_not_found_returns_error`: 不存在的 card_id,期望 error response

- [ ] **Step 2.6 — 跑测试确认 FAIL**

- [ ] **Step 2.7 — 改造 `handle_read_card`**

  位置 `card_handlers.rs:286`。改造点:
  - 原有直接拼 `repo_root/channels/<ch>/cards/<id>/card.meta.yaml` 的逻辑替换为调 `locate_card`
  - 根据返回的 `LocatedCard.rel_path` 读 meta 和 `discussion.thread`
  - 响应 JSON 增加 `"archived": <LocatedCard.is_archived>` 字段
  - `None` 返回情况保持既有 error 文案

- [ ] **Step 2.8 — 跑测试 PASS**

- [ ] **Step 2.9 — commit**

  ```bash
  git add crates/gitim-daemon/src/card_handlers.rs
  git commit -m "feat(daemon): locate_card helper + ReadCard auto-fallback to archive with archived flag"
  ```

---

## Task 3 — `SendCardMessage` / `UpdateCard` 拒绝归档 card

**Files:**
- Modify: `crates/gitim-daemon/src/card_handlers.rs`(`handle_send_card_message` at `:340`,`handle_update_card` at `:476`)
- Test: 同文件

### Steps

- [ ] **Step 3.1 — 写测试**

  - `test_send_card_message_rejects_archived`:fixture 归档 card,调用 `handle_send_card_message`,期望 error 响应包含 "archived" 关键词
  - `test_update_card_rejects_archived`:同上,对 `handle_update_card`

- [ ] **Step 3.2 — 跑测试确认 FAIL**(当前两 handler 直接拼活跃路径,归档 card 会导致 "not found" error,但错误文案不匹配 "archived")

- [ ] **Step 3.3 — 改造 `handle_send_card_message`**

  位置 `card_handlers.rs:340`。将现有"直接拼活跃路径读 meta"改为:
  - 调 `locate_card`,若 `None` → 返回 "card not found"
  - 若 `is_archived == true` → 返回 `"cannot send to archived card"`
  - 否则沿用既有逻辑

- [ ] **Step 3.4 — 改造 `handle_update_card`**

  位置 `card_handlers.rs:476`。同样策略:
  - 调 `locate_card`,若 `None` → 返回 "card not found"
  - 若 `is_archived == true` → 返回 `"cannot update archived card"`
  - 否则沿用既有逻辑(注意现有逻辑里读 meta 的路径拼接也要换成 `LocatedCard.rel_path`)

- [ ] **Step 3.5 — 跑测试 PASS + 回归既有活跃路径测试**

  `cargo test -p gitim-daemon --lib card_handlers`
  预期:新测试 PASS,既有 send/update 活跃 card 的测试继续 PASS

- [ ] **Step 3.6 — commit**

  ```bash
  git add crates/gitim-daemon/src/card_handlers.rs
  git commit -m "feat(daemon): SendCardMessage and UpdateCard reject archived cards"
  ```

---

## Task 4 — `handle_archive_card` 实现

**Files:**
- Modify: `crates/gitim-daemon/src/card_handlers.rs`(新增 handler,紧邻 `handle_update_card` 下方)
- Test: 同文件 inline tests

**参考模式:** `handlers.rs:1185-1297`(`handle_archive_channel`)

### Steps

- [ ] **Step 4.1 — 写测试矩阵**

  - `test_archive_card_by_creator_success`:fixture 活跃 card,creator 归档,期望:响应 success、card 目录移到 `archive/channels/<ch>/cards/<id>/`、git log 显示 archive commit、`Event::CardArchived` 被 emit(可通过 event_tx 接收端断言)
  - `test_archive_card_by_assignee_success`:card 的 assignee 归档,期望同上
  - `test_archive_card_rejects_non_creator_non_assignee`:路人(既非 creator 也非 assignee)归档,期望 error "only creator or assignee can archive"
  - `test_archive_card_rejects_already_archived`:card 已归档,再次归档,期望 error "card already archived"
  - `test_archive_card_rejects_unknown_card`:card 不存在,期望 "card not found"
  - `test_archive_card_rejects_unknown_author`:author 未注册,期望 "unknown user"
  - `test_archive_card_preserves_status_field`:card 原本 `status: doing`,归档后 meta.yaml 里 `status` 仍是 `doing`(不被强制改为 done)

- [ ] **Step 4.2 — 跑测试确认 FAIL**(handler 未实现)

- [ ] **Step 4.3 — 实现 `handle_archive_card`**

  函数签名:`pub async fn handle_archive_card(state: SharedState, channel: String, card_id: String, author: String) -> Response`

  行为(逐点对齐 `handle_archive_channel` 的 idioms):
  1. `ensure_known_user(&state, &author)`
  2. `ChannelName::new(&channel)` 验证
  3. `validate_card_id(&card_id)`
  4. `locate_card` → 必须 `Some` 且 `is_archived == false`,否则分别返回 "card not found" / "card already archived"
  5. 读 `card.meta.yaml` → 解析 `CardMeta` → 拿 `created_by` + `assignee`
  6. **权限检查**:`author == meta.created_by || meta.assignee.as_deref() == Some(author.as_str())`,否则 error "only creator or assignee can archive"
  7. 确保目标父目录存在:`state.repo_root.join("archive/channels").join(<ch>).join("cards")` → `create_dir_all`
  8. `git mv`:
     - from = `channels/<ch>/cards/<id>`
     - to = `archive/channels/<ch>/cards/<id>`
     - 用 `state.git_storage.mv(from, to)`(git mv 支持目录,git 2.x 原子)
  9. commit:`state.git_storage.add_and_commit_as(&[<to>], &format!("card: archive {} in {} by @{}", card_id, channel, author), Some(&author))`
     - 注意:`add_and_commit_as` 第一个参数是要 add 的路径列表。整个目录 rename 后,git 已 stage 了 rename,但为了保险传目录 path 让 `git add` 幂等。确认 `add_and_commit_as` 对目录参数的行为(若不支持目录,传 `<to>/card.meta.yaml` 和 `<to>/discussion.thread` 两个具体文件)
  10. `push_with_retry(&state, "archive_card").await`
  11. `state.event_tx.send(Event::CardArchived { channel, card_id, author })`
  12. `info!("card '{}' archived in channel '{}' by @{}", card_id, channel, author)`
  13. 返回 `Response::success(json!({"channel": ..., "card_id": ..., "archived_by": author}))`

- [ ] **Step 4.4 — 跑测试 PASS**

  `cargo test -p gitim-daemon --lib archive_card`
  预期:7 个测试 PASS

- [ ] **Step 4.5 — commit**

  ```bash
  git add crates/gitim-daemon/src/card_handlers.rs
  git commit -m "feat(daemon): handle_archive_card with creator-or-assignee permission"
  ```

---

## Task 5 — `handle_unarchive_card` 实现

**Files:**
- Modify: `crates/gitim-daemon/src/card_handlers.rs`
- Test: 同文件

### Steps

- [ ] **Step 5.1 — 写测试**

  - `test_unarchive_card_by_creator_success`:fixture 归档 card,creator unarchive,期望目录搬回 `channels/<ch>/cards/<id>/`,git log 显示 unarchive commit,`Event::CardUnarchived` emit
  - `test_unarchive_card_by_assignee_success`:同上
  - `test_unarchive_card_rejects_non_creator_non_assignee`
  - `test_unarchive_card_rejects_not_archived`:card 本来就在活跃区,unarchive,期望 error "card not archived"
  - `test_unarchive_card_rejects_unknown_card`

- [ ] **Step 5.2 — 跑测试确认 FAIL**

- [ ] **Step 5.3 — 实现 `handle_unarchive_card`**

  签名:`pub async fn handle_unarchive_card(state: SharedState, channel: String, card_id: String, author: String) -> Response`

  行为(基本是 archive 的镜像):
  1. 用户/频道/card_id 校验
  2. `locate_card` → 必须 `Some` 且 `is_archived == true`,否则 "card not found" / "card not archived"
  3. 读 meta → 权限检查同 Task 4
  4. 确保 `channels/<ch>/cards/` 父目录存在(基本一直存在,但防御性 create_dir_all)
  5. `git mv archive/channels/<ch>/cards/<id> → channels/<ch>/cards/<id>`
  6. commit message: `card: unarchive {} in {} by @{}`
  7. push retry
  8. `Event::CardUnarchived`
  9. Response success

- [ ] **Step 5.4 — 跑测试 PASS**

- [ ] **Step 5.5 — commit**

  ```bash
  git add crates/gitim-daemon/src/card_handlers.rs
  git commit -m "feat(daemon): handle_unarchive_card mirror of archive flow"
  ```

---

## Task 6 — `handle_list_archived_cards` 实现

**Files:**
- Modify: `crates/gitim-daemon/src/card_handlers.rs`
- Test: 同文件

**参考模式:** `handlers.rs:709-737`(`handle_list_archived_channels`),`card_handlers.rs:188`(`handle_list_cards` 扫描结构)

### Steps

- [ ] **Step 6.1 — 写测试**

  - `test_list_archived_cards_empty`:无归档 card,期望 response 返回空数组
  - `test_list_archived_cards_returns_all_when_no_channel_filter`:fixture 在 channel A 有 2 张归档 card,channel B 有 1 张归档,调用 `ListArchivedCards { channel: None }`,期望返回 3 张(按 channel+card_id 排序)
  - `test_list_archived_cards_filters_by_channel`:同上 fixture,`ListArchivedCards { channel: Some("A") }`,期望返回 2 张
  - `test_list_archived_cards_unknown_channel_returns_empty`:过滤一个不存在的 channel,期望空数组(不报错)
  - `test_list_archived_cards_ignores_active_cards`:fixture 同时有活跃和归档 card,期望响应只含归档

- [ ] **Step 6.2 — 跑测试确认 FAIL**

- [ ] **Step 6.3 — 实现 `handle_list_archived_cards`**

  签名:`pub async fn handle_list_archived_cards(state: SharedState, channel: Option<String>) -> Response`

  行为:
  1. 若 `channel` 为 `Some`,验证 channel name 格式,确定扫描 root 为 `archive/channels/<ch>/cards/`
  2. 若 `channel` 为 `None`,扫描 `archive/channels/*/cards/` 下所有
  3. 对每个 `<id>/card.meta.yaml`:读、解析 `CardMeta`,构造响应项(`channel`, `card_id`, `title`, `status`, `labels`, `assignee`, `created_by`, `created_at`, `updated_at`)
  4. 响应 JSON `{ "cards": [...] }`,按 `(channel, card_id)` 稳定排序

- [ ] **Step 6.4 — 跑测试 PASS**

- [ ] **Step 6.5 — commit**

  ```bash
  git add crates/gitim-daemon/src/card_handlers.rs
  git commit -m "feat(daemon): handle_list_archived_cards with optional channel filter"
  ```

---

## Task 7 — Handler dispatch 接线

**Files:**
- Modify: `crates/gitim-daemon/src/handlers.rs`(主 Request dispatch match,参考 `:185-193`)

### Steps

- [ ] **Step 7.1 — 跑 `cargo check -p gitim-daemon` 确认目前编译错误**

  预期:`non-exhaustive patterns: ArchiveCard, UnarchiveCard, ListArchivedCards not covered`(从 Task 1 遗留)

- [ ] **Step 7.2 — 在 dispatch match 里加 3 个 arm**

  紧接 `handlers.rs:185`(`Request::ListCards` / `Request::ReadCard` 附近),加:
  - `Request::ArchiveCard { channel, card_id, author }` → `crate::card_handlers::handle_archive_card(state, channel, card_id, author).await`
  - `Request::UnarchiveCard { channel, card_id, author }` → `crate::card_handlers::handle_unarchive_card(state, channel, card_id, author).await`
  - `Request::ListArchivedCards { channel }` → `crate::card_handlers::handle_list_archived_cards(state, channel).await`

- [ ] **Step 7.3 — 跑编译 + 全量 daemon 测试**

  `cargo check -p gitim-daemon && cargo test -p gitim-daemon --lib`
  预期:编译通过,所有既有 + 新增测试 PASS

- [ ] **Step 7.4 — commit**

  ```bash
  git add crates/gitim-daemon/src/handlers.rs
  git commit -m "feat(daemon): route ArchiveCard/UnarchiveCard/ListArchivedCards in dispatch"
  ```

---

## Task 8 — `gitim-client` IPC 封装

**Files:**
- Modify: `crates/gitim-client/src/lib.rs`(或对应的 `GitimClient` 实现文件)

**参考模式:** 现有 `archive_channel` / `list_archived_channels` / `create_card` / `list_cards` 等方法

### Steps

- [ ] **Step 8.1 — 找到 `GitimClient` 的 card 方法区**

  搜索 `impl GitimClient` 内的 `create_card` / `list_cards`,在附近加新方法

- [ ] **Step 8.2 — 写集成测试(可选,取决于 client crate 是否有测试惯例)**

  若 `gitim-client` 有 mock IPC 的测试模式,按同样模式为 3 个新方法加 serde 测试;若没有,skip

- [ ] **Step 8.3 — 加 3 个方法**

  - `async fn archive_card(&self, channel: &str, card_id: &str, author: &str) -> Result<Value>`
  - `async fn unarchive_card(&self, channel: &str, card_id: &str, author: &str) -> Result<Value>`
  - `async fn list_archived_cards(&self, channel: Option<&str>) -> Result<Value>`

  每个方法的实现复用既有 `send_request` 或等价方法,构造对应 `Request::*` variant 并发送

- [ ] **Step 8.4 — 跑 `cargo check -p gitim-client && cargo test -p gitim-client`**

- [ ] **Step 8.5 — commit**

  ```bash
  git add crates/gitim-client/src/lib.rs
  git commit -m "feat(client): add archive_card/unarchive_card/list_archived_cards IPC methods"
  ```

---

## Task 9 — CLI 子命令

**Files:**
- Modify: `crates/gitim-cli/src/commands/card.rs`(新增 3 个子命令)
- Modify: `crates/gitim-cli/src/commands/card.rs` 或 `main.rs` clap enum(注册子命令)

**参考模式:** `crates/gitim-cli/src/commands/channels.rs:100-131`(`cmd_archive_channel` + `cmd_archived_channels`)

### Steps

- [ ] **Step 9.1 — 找到 card 子命令的 clap enum**

  打开 `commands/card.rs`,定位现有 `CardCmd` 或类似 enum

- [ ] **Step 9.2 — 加 3 个 variant**

  - `Archive { channel: String, card_id: String }`
  - `Unarchive { channel: String, card_id: String }`
  - `Archived { #[clap(long)] channel: Option<String> }`

  命令 help 文本:对齐 `channels archive` 的风格(简短、imperative)

- [ ] **Step 9.3 — 写 3 个 command handler 函数**

  - `pub async fn cmd_archive_card(client, channel, card_id) -> Result<()>`:调 `client.archive_card(...)`,打印 `"card {id} archived in #{channel} by @{author}"` 或等价
  - `pub async fn cmd_unarchive_card(...)`:类似
  - `pub async fn cmd_archived_cards(client, channel) -> Result<()>`:调 `client.list_archived_cards(channel)`,打印表格(对齐 `cmd_archived_channels` 的输出形式)

  `author` 从 client 本地身份(me.json)获取 —— 参考 create_card/update_card 怎么取 author

- [ ] **Step 9.4 — 在 match 里路由**

  现有 `CardCmd::Create { ... } => cmd_create_card(...)` 附近,加 3 个新 arm

- [ ] **Step 9.5 — 跑 `cargo build -p gitim-cli`**

  预期:编译通过

- [ ] **Step 9.6 — 手工 smoke test(可选)**

  启动 daemon,创建一张 card,归档,list archived,unarchive,再次 list(空)。确认命令行流程顺畅

- [ ] **Step 9.7 — commit**

  ```bash
  git add crates/gitim-cli/src/commands/card.rs
  git commit -m "feat(cli): add card archive/unarchive/archived subcommands"
  ```

---

## Task 10 — 全量测试 + clippy + 清理

**Files:** 无(测试与静态检查)

### Steps

- [ ] **Step 10.1 — 全量 cargo test**

  `cargo test --workspace 2>&1 | tail -50`
  预期:所有测试 PASS,包括既有 ~270 + 新增的 ~20+ 测试

- [ ] **Step 10.2 — clippy 无警告**

  `cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -30`
  预期:无 error 无 warning。若有新警告,就地修

- [ ] **Step 10.3 — 检查是否有遗留 TODO/FIXME/println/dbg!**

  `grep -rn "TODO\|FIXME\|println!\|dbg!" crates/gitim-daemon/src/card_handlers.rs crates/gitim-cli/src/commands/card.rs crates/gitim-client/src/lib.rs`
  预期:只有必要的 follow-up TODO(如 index 集成),无调试残留

- [ ] **Step 10.4 — 更新 CLAUDE.md "Current Orientation"(可选)**

  本次功能加法,架构不变。CLAUDE.md 架构小节无需更新。可在 "Current Orientation → Where we are" 末尾追加一行:"Card 归档/取消归档已实现(后端 + CLI,WebUI 待看板 UI 落地后补)"。**如觉得不必要则 skip**。

- [ ] **Step 10.5 — commit(如有 Step 10.4 改动)**

  ```bash
  git add CLAUDE.md
  git commit -m "docs: note card archive feature in current orientation"
  ```

---

## Self-Review Checklist

在进入执行前,作者对照 spec 复核:

- [ ] **Spec 覆盖**:
  - `archive/channels/<ch>/cards/<id>/` 路径 → Task 4/5
  - `ListArchivedCards` 分开 API → Task 1/6
  - `ReadCard` auto-fallback + archived 字段 → Task 2
  - creator OR assignee 权限 → Task 4/5
  - 归档后只读(update/send 拒绝) → Task 3
  - Event variants → Task 1
  - CLI → Task 9
  - 不加 archived_at/archived_by 字段 → (设计约束,隐式体现在 Task 4 不动 meta.yaml)
  - status 不 coupling → Task 4(`test_archive_card_preserves_status_field`)
  - Index scope 外 → 本文档 Scope 章节明示,不产生 task

- [ ] **Placeholder 扫描**:无 TBD / TODO / "implement later";每步命令精确,预期明确

- [ ] **类型一致性**:
  - `LocatedCard` 在 Task 2 定义,Task 3/4/5 引用
  - `Event::CardArchived` / `CardUnarchived` 在 Task 1 定义,Task 4/5 emit
  - Request variant 命名:`ArchiveCard` / `UnarchiveCard` / `ListArchivedCards` 贯穿 Task 1/7/8/9

- [ ] **测试覆盖** happy + reject + 边界:
  - happy:各 handler 的 creator / assignee 成功
  - reject:权限、already archived、not archived、not found
  - 边界:both-exist(locate_card)、empty list、filter mismatch
  - 回归:现有 send/update 活跃 card 测试不破坏

---

## Execution Handoff

**推荐 Subagent-Driven Development**:

每个 Task 由一个 fresh subagent 执行,主 agent 做 two-stage review(spec reviewer + code quality reviewer)后再进下一个 Task。

worktree 路径:`/Users/lewisliu/ateam/GitIM/.worktrees/card-archive`。分派 subagent 时明确告知**工作目录就是该 worktree,所有文件操作在 worktree 下**,不要退回主仓库路径。
