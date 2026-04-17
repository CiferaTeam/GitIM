# 群聊邀请成员 — 需求共识

## 背景
当前创建频道对话框仅有 name/display_name/intro 字段，`CreateChannelRequest` 也无成员参数，频道建好后只有一个 `join_channel`（自我加入）动作，没有"把别人拉进群"的能力。`ChannelMeta.members: Vec<String>` 已存在，createChannel 时自动填入 `[author]`。ChannelHeader 右上角 DropdownMenu 已展示 `channel.members`，但无"邀请"入口。

## 功能范围（用户确认）

1. **创建时可选人** — 创建频道对话框增加"邀请成员"多选输入
2. **创建后可加人** — 已有 ChannelHeader DropdownMenu 里增加"邀请成员"入口
3. **邀请语义** — 直接把对方加入 `ChannelMeta.members`，对方不需确认（被动入群）
4. **Thread 不留痕** — 本期不在 `.thread` 文件写"system: X invited Y"事件
5. **权限** — 创建者 + 已在群里的任意成员均可邀请

## 数据模型
- 复用 `ChannelMeta.members: Vec<String>`（无需新增字段）
- 邀请 = 对 members 去重追加

## 后端改动
- `CreateChannelRequest` 增加 `invitees: Vec<String>`（可选），创建时 `members = [author] ∪ invitees`（去重）
- 新增 daemon command `InviteMembers { channel, targets: Vec<String> }`
  - HTTP: `POST /im/invite-members`
  - 权限校验：caller 必须是该 channel 的 member；否则 403
  - Handler：读 meta → 校验 → 合并 targets → 写回 meta → git commit
- 校验：targets 中每个 handle 必须是已注册用户（`users/<handle>.meta.yaml` 存在）；不存在则返回错误
- CLI：`gitim invite <channel> <handle>...`（本期做）

## 前端改动（webui-v2）
- **`components/chat/sidebar.tsx`**（创建对话框）：增加"邀请成员"多选字段
- **`components/chat/header.tsx`**（ChannelHeader DropdownMenu）：增加"邀请成员"按钮 → 打开邀请对话框
- 新增可复用 `MemberPicker` 组件（搜索 + 多选），供创建 & 邀请两处共用
- `lib/api.ts` / client：
  - `createChannel(..., invitees?)` 扩展参数
  - `inviteMembers(channel, targets)` 新方法
- Store 对应 action 更新

## 设计原则
- `MemberPicker` 数据源：已注册用户列表（从 daemon 取或复用已有 members store）
- 已在群里的 handle 在创建对话框默认禁用/过滤；邀请对话框过滤掉已在群成员
- 自己的 handle 不出现在可选列表

## 非目标（本期不做）
- 邀请需对方接受的流程
- 移除/踢出成员
- 邀请事件的 `.thread` 留痕
- 邀请权限分级（管理员/普通成员）
