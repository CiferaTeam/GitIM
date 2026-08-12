# Agents Human + Outside Roster — 需求共识

> Phase 2 grill-me + Phase 3 plan-eng-review 输出。仅 design / requirements，不含逐步实现指令。
> 下一阶段 (writing-plans) 产 `01-plan.md`。

Status: APPROVED (eng-review locks applied)
Date: 2026-08-12
Branch: `feat/agents-human-external-nodes`
Review trail:
- 2026-08-12 grill-me: 区分共享库真人 vs agent；布局 Humans → Local/Fleet → Outside
- 2026-08-12 plan-eng-review: 见文末 `## GSTACK REVIEW REPORT`；逐项 AskUserQuestion 按用户要求压缩为自动锁定推荐项

---

## 背景 / 问题

Agents 页今天按 **本机 runtime + 已订阅 fleet 节点** 展示 live agents。共享 GitHub 协作空间里常见：

- 5 个真人各自 onboard
- 每人塞 ~10 个 agent
- 合计 ~55 个 `users/<handler>.meta.yaml`

**硬伤：** human 与 agent 的 UserMeta 同形（双方 onboard 都写 `role: member`），`me.json.provider` 不进 git。只靠现有 git 数据 **分不出 5 个真人 vs 50 个 agent**。

另：别人机器上的 agent 若不在本机 fleet 订阅里，本机拿不到 live 状态，需要底部汇总「名册上有、live 视图没有」的账号。

---

## 目标

1. 协议层：`users/<h>.meta.yaml` 带可同步的 `kind`，权威区分 human / agent。
2. Runtime WebUI Agents 页：
   - **顶：** Humans（`kind=human`，含自己 + `you` badge）
   - **中：** 现有 Local + Fleet（live agents，不变）
   - **底：** Outside（默认折叠；`kind=agent|unknown` 且未出现在 Local/Fleet）

## 非目标 (v1)

- 旧数据回填 / migrate / set-kind API（缺字段 = `unknown`；真要改用手改 yaml + git commit）
- Browser 模式 Agents 页（导航 `requiresRuntime`，本无此页）
- 自动发现未订阅的远程「机器节点」
- WebUI 编辑 kind / labels 当 human 标记
- Coordinator prompt / routing 消费 kind（可后续）

---

## 协议

### `UserKind`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserKind {
    #[default]
    Unknown,
    Human,
    Agent,
}
```

### `UserMeta.kind`

```rust
/// Account kind. Written once at RegisterUser / onboard. Immutable via API.
/// Missing field (old yaml) deserializes as Unknown.
#[serde(default, skip_serializing_if = "UserKind::is_unknown")]
pub kind: UserKind,
```

- **无 `deny_unknown_fields`**（现状已是如此）→ 新 daemon 写 `kind:`，老 daemon fetch 忽略未知字段，不炸。[P0 lock]
- `unknown` 不落盘（skip），与旧文件「缺字段」同形。
- `update_user` / labels RMW **不得**改 `kind`（读写时保留原值）。

### 写入路径

| Caller | kind |
|--------|------|
| `provision_human` → onboard | `human` |
| `provision_agent` → onboard | `agent` |
| CLI 人类 onboard | 默认 `human` |
| `RegisterUser` / `Onboard` 省略 `kind` | 落成 `Unknown`（不猜） |
| Guest onboard | 不写 `users/*`（现状），无 kind |

`register_user` 若 meta **已存在**：保持幂等 skip，**不升级** kind（无回填）。

### Wire

`ListUsersResponse.user_infos[]` / `ActiveUserEntry` **加性**字段：

```json
{ "handler": "alice", "display_name": "Alice", "kind": "human" }
```

- 缺省 / 省略 → 前端当 `unknown`
- `as_str()` 与 serde snake_case 一致（regression test，对齐 `NodeStatus`/`RunStatus` 惯例）

---

## UI（Runtime Agents 页）

```
┌─────────────────────────────────────┐
│ Humans                    (展开)     │  ← user_infos kind=human
│  [you] Alice · alice                 │
│  Bob · bob                           │
├─────────────────────────────────────┤
│ Local / Fleet nodes       (现有)     │  ← /agents + fleet snapshots
├─────────────────────────────────────┤
│ ▸ Outside · 42 agents · 8 unclassified│  ← 默认折叠
│    (展开后只读名单)                   │
└─────────────────────────────────────┘
```

### 分区规则（纯函数，必测）

输入：`userInfos`, `localAgents`, `fleetSnapshots`, `selfHandler`

1. **Humans** = `userInfos` where `kind == human`（字典序 handler）
2. **Live handlers** = local ∪ fleet 的 `agent.handler ?? agent.id`
3. **Outside** = `userInfos` where `kind ∈ {agent, unknown}` **且** handler ∉ live handlers
4. Humans **不**进 Outside；Local/Fleet 渲染逻辑保持现状

### 交互

- 搜索：过滤 Humans + Local/Fleet + Outside（展开前后 Outside 计数基于过滤后集合）
- status 滤镜（online/stopped/error）激活：隐藏 Humans + Outside（对齐 Archived）
- Outside 文案：`Outside · {n} agents · {m} unclassified`；为 0 的段省略；两段皆 0 则整段 Outside 不渲染
- `you` badge：`handler === chatStore.currentUser`（来自已有 `/im/me` 轮询）

### 卡片粒度

- Humans / Outside：只读紧凑行（display_name + handler + kind 小标）；不假装 live status / 不链 agent detail
- Local/Fleet：继续用 `AgentCard`

---

## 影响面（实现时必改）

| 层 | 文件（代表） |
|----|----------------|
| core | `types/meta.rs`（enum+字段+validate 无关）, `responses.rs` ActiveUserEntry |
| daemon | `api.rs` Onboard/RegisterUser, `onboard.rs::register_user`, `handlers/user.rs`, `handlers/read.rs` list_users |
| client | `client.rs` onboard / register_user 传 kind |
| runtime | `agent.rs` provision_human/agent |
| cli | onboard 默认 human |
| frontend | `types.ts` UserInfo.kind, `agent-list.tsx`, 新建 `lib/agents-roster.ts` 纯分区, tests |
| wasm | `UserMeta` 经 serde 自动带 kind；改 core 后跑 `npm run build:wasm` 并 commit `pkg/` |
| daemon-web | `state.ts` UserMeta 类型加 kind（读路径）；Agents 页仍 runtime-only |

---

## 测试策略（最小充分）

**Rust**
- UserMeta：缺字段 → Unknown；`human`/`agent` roundtrip；serialize 时 Unknown 省略 `kind`
- 老 yaml + 新字段共存（无 deny_unknown_fields regression）
- onboard provision 路径：human/agent 写入正确 kind；omit → Unknown；已存在 meta 不覆盖 kind
- `update_user` / labels add 后 kind 不变
- `ActiveUserEntry` wire 含 kind；`as_str` 锁 snake_case

**Frontend**
- `partitionAgentsRoster` 表驱动：5 human + live agents + outside agents + unknown；self you；status filter hide；search
- `agent-list` 组件：Outside 默认 collapsed；展开名单；空 Outside 不渲染

**不做：** 全量 `cargo test`；browser E2E（无 Agents 页）

---

## Edge cases

| Case | 行为 |
|------|------|
| 混版本 workspace | 新写 kind，老 daemon 忽略字段；老写无 kind，新 UI 显示 unknown |
| handler 同时在 live 与 user_infos(agent) | 只出现在 Local/Fleet，不进 Outside |
| kind=human 却误出现在 /agents | 仍在 Humans 展示；Local 若 runtime 真返回该卡则也显示（配置错误可见，不静默吞） |
| Outside 全 unknown | 文案只有 `unclassified` |
| user_infos 缺席（老 runtime） | Humans/Outside 空；Local/Fleet 照旧 |
| 手改 yaml kind | 允许；无 API；冲突靠 git |

---

## Step 0（eng-review）

1. **复用：** `user_infos` 轮询、`currentUser`、`AgentNodeSection`、fleet 分组 — 不新建 roster 服务。
2. **最小集：** kind 协议 + 分区纯函数 + agent-list 三段 UI。无回填、无 set-kind、无 browser。
3. **复杂度：** 触达 >8 文件，但是 **协议加性字段 + UI 分区**，无新服务/新实体；接受，不缩 scope。
4. **TODOS：** 无阻塞项；不捆绑 Social Cognition / labels WebUI。
5. **完整度：** 不做回填是产品决定（grill C），不是工程偷工；测试覆盖写入/wire/分区。

### Eng locks（自动采纳，压缩问答）

| ID | Lock |
|----|------|
| L1 | `UserKind` + `#[serde(default, skip_serializing_if = is_unknown)]`，无 Option 套娃 |
| L2 | 保持 UserMeta 无 `deny_unknown_fields`（混版本安全） |
| L3 | 分区逻辑抽 `lib/agents-roster.ts` 纯函数，UI 薄封装 |
| L4 | `you` = `currentUser`，不新开 me 轮询 |
| L5 | wasm `pkg/` 随 core 变更重建并提交 |
| L6 | `update_user`/labels RMW 保留 kind |
| L7 | Onboard/RegisterUser `kind` 可选；省略 → Unknown |

---

## ASCII — 数据流

```
provision_human ──onboard(kind=human)──┐
provision_agent ──onboard(kind=agent)──┼──► users/<h>.meta.yaml (git)
CLI human ────────onboard(kind=human)──┘           │
                                                   ▼
                                        list_users → user_infos[].kind
                                                   │
                     /agents + fleet SSE ──┐       │
                                           ▼       ▼
                                      partitionAgentsRoster
                                           │
                          ┌────────────────┼────────────────┐
                          ▼                ▼                ▼
                       Humans         Local/Fleet        Outside
```

---

## GSTACK REVIEW REPORT

| Runs | Status | Findings |
|------|--------|----------|
| plan-eng-review (claude, compressed Q&A per user) | PASS | L1–L7 locked; P0 serde compat verified (`UserMeta` has no `deny_unknown_fields`, meta.rs:21-31) |
| Outside voice / codex | SKIPPED this phase (user asked reduce questions; optional in Phase 6) | — |

**Architecture:** Additive `kind` on existing UserMeta; UI partitions over existing polls. No new IPC for mutation. Blast radius = new registers only + UI; old rows stay unknown by design.

**Code quality:** Extract pure roster partition (DRY + testable). Avoid stuffing branch logic into 500-line `agent-list.tsx`.

**Tests:** Core serde + onboard write + frontend partition table tests required before claim done.

**Performance:** `list_users` already reads all user yaml; adding one enum field is O(1) per user. Outside collapse avoids rendering large lists until expand.

**VERDICT:** READY for `01-plan.md` / writing-plans.

NO UNRESOLVED DECISIONS
