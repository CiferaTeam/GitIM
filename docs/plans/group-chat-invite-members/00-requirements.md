# 群聊邀请成员 — 需求共识

## 背景
WebUI 的创建频道对话框只有 name/display/intro，频道 Header DropdownMenu 只能看现有成员；看上去"没法拉人"。

**但实际上 daemon + CLI 早已支持邀请语义**：
- `handle_join_channel(author, channel, targets: Vec<String>)` 复用一个接口：`targets` 空 = author 自己加入；`targets` 非空 = author 以现有成员身份把 targets 加到 `ChannelMeta.members`（`handlers.rs:1035-1051, 1414-1421`）
- CLI `gitim join-channel <channel> -t h1 h2` 早已存在（`main.rs:72-77, 395-397`，doc comment 明写 "Join a channel or invite users"）

**断点在 HTTP + WebUI**：
- `/im/join` 把 targets 写死 `&[]`（`gitim-runtime/src/http.rs:384`）
- `/im/create-channel` 无 `invitees` 字段
- `webui-v2/src/lib/client.ts` `joinChannel(channel)` 只传 channel
- 创建对话框 / Header 没有任何邀请入口

## 功能范围（用户确认）

1. **创建时可选人**
2. **创建后可加人**（任意群内成员都能再拉人，不止创建者）
3. **邀请语义** — 直接把对方加入 `ChannelMeta.members`（被动入群，无对方确认流程）
4. **Thread 不留痕** — 本期不写 "system: X invited Y"
5. **权限** — caller 必须是 channel 的现有 member（复用 `handle_join_channel` 既有校验）

## 技术方案

### daemon 层
- **扩展** `handle_create_channel`：接受 `invitees: Vec<String>` 参数，创建时 `members = [author] ∪ invitees`（去重，顺序：author 优先）
- 复用 `handle_join_channel` 的 targets 机制 —— **不新增 command**
- 相关 Command / API types 同步扩展 `CreateChannel { ..., invitees }`

### gitim-runtime HTTP
- `JoinRequest` 新增 `#[serde(default)] targets: Vec<String>`，透传给 `client.join_channel(&req.channel, &req.targets)`
- `CreateChannelRequest` 新增 `#[serde(default)] invitees: Vec<String>`，透传给 `client.create_channel(..., invitees)`

### gitim-client Rust
- `create_channel(name, display_name, introduction, invitees: &[String])` 签名扩展，构造 Command 时带上 invitees

### gitim-cli
- **不动**。现有 `gitim join-channel <channel> -t h1 h2` 已够用
- `gitim create-channel` 是否需要 `-t` 选人？本期不做（用户未要求；有需要可 follow-up）

### webui-v2
- `lib/client.ts`
  - `joinChannel(channel, targets?: string[])` 参数扩展
  - `createChannel(name, displayName?, intro?, invitees?: string[])` 参数扩展
- 新组件 `components/chat/member-picker.tsx`
  - 搜索框 + checkbox/chip 多选
  - 数据源 `useChatStore((s) => s.users)`（已有，`app.tsx:152` 启动加载）
  - 排除自己（`me.handler`）；可选 prop `excludeHandlers: string[]` 用于隐藏已在群者
- `components/chat/sidebar.tsx` 创建 Dialog 嵌入 `<MemberPicker>`，提交时附 `invitees`
- `components/chat/header.tsx` DropdownMenu 新增"邀请成员"条目 → 打开 `components/chat/invite-dialog.tsx`（内嵌 `<MemberPicker>`，调用 `joinChannel(channel, targets)`）；成功后刷新 channel members

### 用户校验
- daemon 现有 handlers 各自 `state.users` 检查即可，不抽通用 helper（本期 scope 不扩）
- 校验时机：创建 handler 校验 invitees 都已注册；join handler 校验 targets 都已注册

## 五问决议

| # | 问题 | 决定 |
|---|---|---|
| 1 | MemberPicker 数据源 | `useChatStore.users` + `/im/users`（均已有） |
| 2 | CLI 本期做不做 | 不做，已存在 |
| 3 | 邀请对话框容器 | 新 Dialog（DropdownMenu `w-48` 太窄） |
| 4 | daemon 用户校验 | 现有 state.users check，不抽 helper |
| 5 | meta.yaml 并发合并 | **本期不处理**，follow-up |

## 非目标（本期不做）
- 邀请需对方接受的流程
- 移除/踢出成员
- 邀请事件 `.thread` 留痕
- 邀请权限分级
- `gitim create-channel -t` CLI 参数
- `channels/*.meta.yaml` 并发 merge driver（现 `gitim-sync/conflict.rs` 只处理 `.thread` 行号，同时邀请不同人会触发 git 冲突 marker，记 follow-up）

## 测试要点
- daemon：`handle_create_channel` with invitees → 验证 meta.members 集合正确；invitees 含未注册 handle → 报错
- runtime HTTP：`/im/join` with targets → 底层 client 收到正确 targets；`/im/create-channel` with invitees → 透传正确
- webui：MemberPicker 搜索过滤；创建/邀请对话框提交后 members 刷新

## 关键文件映射

| 职责 | 文件 |
|---|---|
| create channel handler | `crates/gitim-daemon/src/handlers.rs:1055` |
| join channel handler | `crates/gitim-daemon/src/handlers.rs:1035` |
| Command API types | `crates/gitim-daemon/src/api.rs:99-124` |
| Rust client | `crates/gitim-client/src/client.rs:165` |
| HTTP routes | `crates/gitim-runtime/src/http.rs:345, 371` |
| webui client | `webui-v2/src/lib/client.ts:72` |
| sidebar create dialog | `webui-v2/src/components/chat/sidebar.tsx:71-226` |
| header dropdown | `webui-v2/src/components/chat/header.tsx:61-92` |
| chat store | `webui-v2/src/hooks/use-chat-store.ts:13, 51, 65` |

## Phase 3 Eng Review 决议

### Step 0 scope ✓
- 文件数 ~10，无冗余；非目标都在 follow-up 列表。

### Architecture
- **A1 serde 兼容性**：`#[serde(default)]` 保证新增 `invitees` 向后兼容
- **A2 签名 fan-out**：`client::create_channel` 加 `invitees: &[String]`，`gitim-cli/commands/channels.rs:31` 和 `gitim-runtime/http.rs:362` 同步调整（CLI 传 `&[]`）
- **A3 invitee 未注册** → **daemon 整体 reject**，返回明确错误；前端 Dialog inline 显示不关闭
- **A4 members 刷新** → 复用现有 `chat-layout.tsx:87` 的 `/im/channels` refetch 模式，不引入新同步机制

### Code Quality
- **CQ1 daemon dedup**：`members = [author] ∪ invitees` 保序去重 + author 首位
- **CQ2 错误展示**：Dialog 内 inline 显示错误，提交失败不关闭 Dialog
- **CQ3 MemberPicker contract**：props `{ allUsers, excludeHandlers, value, onChange }`；组件不直接依赖 `useChatStore`，调用方取好传入

### Tests
- daemon `create_channel`：happy / dedup self / dedup duplicates / unregistered error / empty（regression）
- runtime HTTP：`/im/create-channel` with invitees / `/im/join` with targets / backward compat
- CLI：无新测试
- webui：MemberPicker 前端测试先占位（前端测试基建记 follow-up）

### Performance
- 邀请后全量 refetch channels（MVP 可接受，SSE 精细化推送记 follow-up）
- daemon 端单次 git commit（方案 b 的主要优势）

### Follow-ups（已登记 `TODOS.md`）
- `channels/*.meta.yaml` 并发 set-union 合并
- SSE 推送频道 members 变更
- webui-v2 前端测试基建

---

**决策记录**：Q1=A（整体 reject）、Q2=i（inline Dialog）、Q3=p（props 注入）、Q4=写入 TODOS.md。
