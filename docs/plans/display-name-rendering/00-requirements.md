# Display Name 前端渲染层 — 需求与设计

## 背景

`handler` 是 GitIM 唯一的协议标识符：消息 `author`、`<@mention>`、DM 文件名（`alice--bob`）、频道成员列表、git commit author name、agent system prompt（"你是 {handler}"）全部依赖它。`display_name` 在 onboard 时写入，但在聊天界面几乎不可见——人在 chat 里看到的是 `@handler`，而想要的是友好名字；@ 一个人去 wake up 时显示名和可输入名对不上号；同名 agent 的卡片无法区分。

本方案把 `display_name` 做成**纯前端渲染层**：人看到友好名字，@ / wake-up / routing 仍然 100% 走 `handler`。

## 已核实现状（代码 walkthrough 结论）

- `display_name` 存两份，onboard 时从同一个 `InferredIdentity` 写出：
  - `users/<handler>.meta.yaml`（`UserMeta.display_name: String`，必填，git 同步）
  - `.gitim/me.json`（`MeJson.display_name: Option<String>`，本地，不同步）
- `handler` 是唯一协议标识；LLM / agent prompt 永远只见 `handler`（`PromptContext` 无 display_name 字段）。
- Rust daemon 启动时**丢弃** display_name：`main.rs` 的 `read_identity_from_me` 只取 `(handler, guest, admin, email)`；`AppState` 无此字段。唯一从盘上读 display_name 的读路径是 `handle_list_archived_users`（best-effort）。
- `handle_list_users`（`read.rs:232`）返回**裸 handler 列表**（`Vec<String>`），无 display_name。daemon-web 的 `users()`（`handlers.ts:639`）同样只返回 handler keys，尽管 `state.ts` 内存里已持有 `UserMeta`（含 display_name）。
- agent 的 display_name 已经在 `/agents`（`AgentInfo.display_name`）和 `/im/me` 暴露；**缺的是真人用户**。
- 前端 `Agent` 类型无独立 `handler` 字段——handler 只 baked 进 `id = id ?? handler` 和 `name = display_name ?? handler`，因此两个同名 agent 的列表卡片视觉上不可区分（handler 仅在详情页 `agent.id` 可见）。
- onboard 后**无任何编辑路径**：`UpdateUser` 只携带 `{handler, introduction}`；`AgentUpdateRequest` 无 display_name 字段；前端编辑态无对应输入。

## 设计决策（已锁定）

1. **纯前端渲染层**——display_name 不碰协议 / `.thread` 文件 / daemon routing / agent prompt
2. **共享 display_name**（onboard 定义、所有人看到同一个），不是 per-viewer 私人备注
3. **消息只存 `<@handler>`**（与今天完全一致），渲染时查表贴 display_name
4. **固定格式 `display_name @handler` 一起显示**，靠一张 `handler → display_name` 表
5. **不做编辑**——display_name 仍只在 onboard 写一次；"改名"是独立的未来工作

## 设计

### 1. Directory（`handler → display_name` 表）

前端维护 `Map<handler, display_name>`。数据来源：

- agents → `/agents` 的 `AgentInfo.display_name`（已有）
- 自己 → `/im/me`（已有）
- 其他真人用户 → 扩展后的 `list_users`（见 §2）

查不到的 handler 一律回退显示**裸 handler**。表随 poll 刷新，因此 display_name 变化时会**自动回溯到所有历史消息**的渲染（消息正文不含 display_name，全靠渲染时查表）。

### 2. 后端改动（本方案唯一的非前端动作）

让 directory 拿得到真人用户的 display_name：

- **Rust daemon**：`handle_list_users`（`read.rs:232`）的返回项带上 display_name，参考 `handle_list_archived_users` 已有的 "per-user best-effort 读 meta.yaml" 模式；`ListUsersResponse.users` 的 wire 形状从 `Vec<String>` 升级为携带 display_name 的条目。
- **daemon-web**：`users()`（`handlers.ts:639`）从内存 `s.users`（已含 `UserMeta`）带出 display_name，与 Rust 端对齐。
- **向后兼容**：wire 形状走加性扩展（旧前端读新 daemon、新前端读旧 daemon 都不挂）；前端解析对缺失 display_name 回退 handler。

### 3. 渲染：所有 handler 露出点统一成 `display_name @handler`

受影响的前端渲染点（全部改为查 directory，查不到回退裸 handler）：

- 消息作者头 `message-item.tsx:219`、回复引用 `:242`、recipient 回执 `:285`
- inline mention `message-body.tsx:107`、user-profile `~handler` `:156`
- DM 标题 `dm-display-name.ts` / `header.tsx`
- 侧栏 DM 搜索 `sidebar.tsx:1131`
- agent 卡片 `agent-card.tsx`（今天只有 display_name → **补上 handler**，顺手解决同名卡片不可区分）

`@handler` 段做 muted / 小字（具体视觉走 `DESIGN.md`）。**因为永远并排 `@handler`，同名冲突自动消解，不需要任何冲突检测逻辑。**

### 4. Composer @ 弹窗

`mention-popup.tsx` 每项显示两段 `display_name` + `@handler`，过滤**同时匹配两段**（敲 "Ali" 或 "alice" 都能命中）；选中后插入 `<@handler>`（`input-area.tsx:209` 的插入逻辑不变，只改弹窗的数据源与展示）。人永远不用手敲 handler。

### 5. Hover 卡片

`user-card.tsx` 升级为 enrichment：鼠标悬停任意 mention / 作者名 → 弹 `display_name` + `@handler` + role / introduction / labels。因为人写的和 agent 写的 mention 在文件里**都是 `<@handler>`**，两者渲染与 hover 行为完全一致，不存在"agent 发的要特殊处理"的不对称。

### 完全不动

协议 / `.thread` 格式 / daemon recipients & wake-up / agent system prompt / 消息存储——一行都不改。

## 边界情况

- **directory 未加载完**：先回退裸 handler，加载后重渲染
- **查不到 handler**（departed / 历史用户）：显示裸 handler
- **display_name 缺失**（me.json `Option` 为 None / 旧数据）：回退 handler（沿用现有 `display_name ?? handler` 语义）
- **同名**：永远并排 `@handler`，天然区分

## Scope / Non-goals

- ❌ 编辑 display_name（独立未来工作，骑 `introduction` 已有的 `UpdateUser` 链路）
- ❌ 改协议 / 消息存储格式
- ❌ 让 agent / LLM 感知 display_name
- ❌ per-viewer 私人备注名

## Walkthrough 附带发现（本方案不处理，记录备查）

存在两个 `validate_user_meta`，规则不一致：

- `gitim-core/src/types/meta.rs:44`（struct 级）：introduction ≤ 256，**不校验 display_name**
- `gitim-core/src/validator/mod.rs:21`（yaml 级）：display_name 1–64 字符、introduction 1–500

两个 introduction 上限互相矛盾（256 vs 500），display_name 的长度约束只活在其中一个里。这属于**编辑 / 校验**范畴，本 render-only 方案不触及；若未来做 display_name 编辑，需先收口为单一 validator。
