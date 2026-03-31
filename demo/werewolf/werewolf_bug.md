# Werewolf Demo Bug Report (2026-03-30 首轮测试)

模型：Minimax-M2.7-highspeed via api.minimaxi.com/anthropic
配置：5 人局（alice/dave=狼人, bob=预言家, charlie=女巫, eve=村民）

## 系统/架构类

### ✅ P1. 频道无读权限隔离（根本性）

- **现象**：charlie（女巫）和 eve（村民）的 clone 里都有完整的 `channels/wolves.thread`
- **根因**：git sync 拉取所有文件到所有 clone，无文件级 ACL
- **影响**：charlie 实际看到狼人私聊内容后在 general 不断泄露
- **分类**：设计如此，非 bug
- **分析**：物理文件在 clone 里是 git 固有限制。但 daemon API 层面已有完整隔离：
  - `poll`：已实现 membership 过滤（非成员频道变更不推送）
  - `send`：已加 membership 校验（非成员不能发消息）
  - `read_messages` / `list_channels`：无过滤，但 player agent 工具集不含这两个 API（仅 send_message + list_users）
  - Player 获取内容仅通过 poll（已过滤），不直接读文件

### ✅ P2. general.meta.json members 不完整

- **现象**：members 只有 `["alice", "god"]`，缺少 bob/charlie/dave/eve
- **证据**：git log 只有 2 个 commit 碰了 meta（初始化 + alice join），其他 4 人有 thread 里的 `[E:join]` 事件但 meta 未更新
- **根因**：auto_join_general 确实更新了 meta members，但并发 onboard 导致 meta 文件 push 冲突，sync_loop 的冲突解决只保留 .thread 新增行，meta.json 变更被 `discard_unpushed()` 静默丢弃
- **分类**：sync bug
- **修复**：meta 文件从 JSON 迁移为 YAML（减少冲突），sync_loop 扩展冲突解决：捕获本地 meta → discard → 与远端合并（members 取并集）→ 写回提交（fix/werewolf-bugs 分支）

### P3. wolves.meta.json 不存在

- **现象**：`wolves.thread` 有 22 行消息，但 `wolves.meta.json` 不存在
- **根因**：`send_message` 自动创建 thread 文件时不生成 meta.json
- **分类**：daemon bug — 频道自动创建应同时生成 meta
- **关联**：P1 — 即使有 meta 也拦不住 git sync 的读取

### ✅ P4. 缺少 join_channel 工具

- **现象**：God 系统提示词写了"用 join_channel 工具逐个拉狼人成员入群"，但 tools.ts 只有 5 个工具（send_message, read_messages, list_channels, list_users, get_thread）
- **根因**：工具集不完整，God 被指示使用不存在的工具
- **分类**：demo 代码 bug
- **修复**：tools.ts 添加 join_channel 工具定义 + executeTool 处理分支（fix/werewolf-bugs 分支）

### ⚠️ P5. `dm:god` 格式消息静默丢失

- **现象**：bob/dave/eve 发到 `dm:god`（缺少自己的 handler），tool 调用返回成功但消息未落盘
- **证据**：dm/ 目录无 `god.thread`，bob 的"收到"确认从未到达 God
- **根因**：daemon 的 send handler 对无效 DM channel 格式未做校验/报错
- **分类**：daemon bug — 应校验 `dm:a,b` 格式并返回错误
- **关联**：P8 — 模型写错格式 + 系统不报错 = 消息黑洞
- **部分修复**：main 分支已加 membership 校验（handle_send 检查 allowed_senders），`dm:god` 格式下 bob 不在 `["god"]` 中会被拒绝返回错误，不再静默丢失。但仍缺少 DM 格式的显式校验（如必须包含 2 个 handler、必须按字典序等）

### ✅ P6. 淘汰玩家仍可发言

- **现象**：charlie 在 L035 被投票出局后继续发了 6 条消息（L037-L042）
- **根因**：God prompt 没有明确要求忽略死亡玩家消息，Player prompt 没有说明死亡后应停止发言
- **分类**：提示词问题
- **修复**：God prompt 加"死亡玩家管理"章节（维护存活列表、提醒死者退出），Player prompt 加"被淘汰后立即停止一切游戏行为"

## 提示词/LLM 行为类

### ✅ P7. 玩家公开暴露角色

- **现象**：charlie L018 说"我是女巫"，bob L019 说"收到预言家身份"——均在 general
- **根因**：player prompt 未强调角色保密；Minimax 模型指令遵循精度不够
- **分类**：prompt 问题
- **修复**：Player prompt 加"角色保密"专属章节，强调"绝对不要在公开频道透露角色"；God prompt 分配角色时告知保密要求；确认环节改为"只回复'收到'二字"

### ✅ P8. 玩家 DM 格式写错

- **现象**：bob/dave/eve 发到 `dm:god` 而非 `dm:bob,god`
- **根因**：player prompt 对 DM 格式说明不够清晰，或模型不遵循
- **分类**：prompt 问题
- **关联**：P5 — 与静默丢失联动，多个玩家确认消息成为黑洞
- **修复**：Player prompt 加详细 DM 格式说明（含字母序排列逻辑和错误示范）；God prompt 也明确 DM 格式规范

### ✅ P9. God 以 1/5 票淘汰玩家

- **现象**：投票结果只有 alice 1 票投 charlie，其余 4 人弃票，God 直接判出局
- **根因**：God 提示词对投票规则（需多数票/平票处理）表述不够严格
- **分类**：prompt 问题
- **修复**：God prompt 投票规则改为"必须至少获得 2 票才能被淘汰，1 票视为流票，平票无人出局"

### ✅ P10. God 未回复预言家查验结果

- **现象**：bob 在 DM 里发了 3 次"查验charlie"，God 从未回复
- **根因**：bob 回复晚了（142618Z），God 在 142530Z 已宣布天亮。时序问题 + God 可能没 poll 到 DM 变更
- **分类**：prompt + 系统时序问题
- **修复**：God prompt 每个夜晚行动步骤加"必须等待对方通过 DM 回复后再继续下一步，不要跳过任何人的行动"

## 按根因分类

| 根因 | 涉及问题 | 修复层 | 状态 |
|------|---------|--------|------|
| sync_loop 冲突解决不处理 meta | P2 | sync_loop + meta YAML 迁移 | ✅ 已修复 |
| git 全量同步无 ACL | P1 | 设计如此（poll/send 已过滤，player 工具集已限制）| ✅ 非 bug |
| send_message 不生成 meta | P3 | daemon send | 待修 |
| 工具集不完整 | P4 | demo tools.ts | ✅ 已修复（join_channel 已加）|
| daemon 对无效输入静默 | P5 | daemon send 校验 | ⚠️ 部分（membership 校验已加，DM 格式校验待补）|
| 提示词缺少死亡管理 | P6 | prompts.ts | ✅ 已修复 |
| prompt + 模型质量 | P7, P8, P9, P10 | prompts.ts | ✅ 已修复 |
