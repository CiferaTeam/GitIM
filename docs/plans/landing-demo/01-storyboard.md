# Landing Demo v2 — 逐帧分镜表 v2（待 owner 批改）

v2 变更（2026-07-28，owner 反馈）：

1. **真实语境**：从虚构的 "release-142 加 reviewer" 改为**线上事故响应**——
   发布前夜 prod webhook 重复投递，owner 在频道里拉起一个临时事故小组。
   对话带具体技术细节（重复发票、30s 重试窗口、canary 观察），不再是 mock 腔。
2. **每句对话后必有箭头**：结构固定为「消息出现 → 箭头扇出到所有变化位置」。
3. **展示范围收窄为两类**：聊天 + 聊天引起的成员列表/卡片变化。
   **取消 Files tab 和审计幕**；舞台改为**单场景、无 tab**：
   左侧永远是 chat 主视图（channel 或某张卡的 discussion），
   右侧栏常驻 Members / Cards / 紧凑 Git 树（箭头的落点）。

用法：逐帧看，直接在文件上批注。锁定后 = Phase A/B 实现 spec + TTS 输入文本。

## 约定

- **时长**：帧时长 = 解说音频时长 + 0.5s；标 `（无解说）` 为纯视觉节拍，约 2s。
- **高光记号**：`→` 流动箭头（from → to）；`◎` 脉冲 ring；`🏷` 变化徽章。
- **协议保真**：commit message 与源码一致（`msg: @A -> T L%06`、`user: register @h`、
  `card: create/update <id> in <ch> by @h`）；卡状态 `todo / doing / done`。
- **命令展示**：coordinator 的 CLI 调用以**行内命令 chip** 挂在它的消息下方
  （等效真实产品里 agent 的 tool-call 展示），不再有独立 receipt 面板。

## 舞台布局 v2（单场景）

```
┌────────────────────────────────────────────────────┐
│ ▰▰▱ 事故 │ ▱ 组队派活 │ ▱ 交付             12 / 26 │ ← 章节进度条
├────────────────────────────────────────────────────┤
│ 解说栏（= 字幕，固定 40px）                           │
├───────────────────────────────┬────────────────────┤
│ #release-v2-4 或 card 讨论视图  │ MEMBERS            │
│                               │  👤 lewis (owner)  │
│  chat 消息流（唯一主视图）       │  🤖 coordinator    │
│                               │  🤖 investigator ⚡ │
│        ╭── 箭头 overlay ──────┤  🤖 fixer          │
│                               ├────────────────────┤
│  [输入框]                      │ CARDS              │
│                               │  wh-3a91 doing     │
│                               │  wh-3a92 todo      │
│                               ├────────────────────┤
│                               │ GIT                │
│                               │  紧凑文件树          │
│                               │  ⎿ 最新 commit 行   │
├───────────────────────────────┴────────────────────┤
│  ◀ ▶  Play(with audio/muted)  ↻  🔇                 │
└────────────────────────────────────────────────────┘
```

## 初始态（预填，消灭空盒）

- 频道 `#release-v2-4`，历史消息 2 条：
  1. lewis: `v2.4 cuts tomorrow 10:00. Freeze starts tonight.`
  2. coordinator: `Noted — I'll keep this channel as the release log.`
- Members：lewis（owner）、coordinator（active）
- Cards：空；Git 树：`users/`×2、`channels/release-v2-4.*`；commits：2 条历史
- 虚构时间戳 `20260713T214x00Z`（发布前夜 21:4x，界面显示 `21:4x`）

## 章 1 · 事故（5 帧）

| # | 画面 | 高光 | 解说词（EN，即 TTS 文本） | 事件 / CLI | Git 痕迹 |
|---|------|------|--------------------------|-----------|---------|
| 1.1 | 预填态全亮开场 | 无 | "A GitIM workspace, the night before a release. One human, one coordinator." | — | — |
| 1.2 | 输入框逐字打字 | ◎`chat-input` | "Then production breaks. You describe what you need — in plain language." | 键入 `<@coordinator> prod is double-firing webhook retries — customers are getting duplicate invoices. v2.4 can't ship like this. Build me an incident team.` | — |
| 1.3 | 消息出现在 chat（L000003） | ◎`chat-msg-3` | "Hit send." | send | — |
| 1.4 | 右侧 git 树 thread 文件亮 + 最新 commit 行 | →`chat-msg-3`→`git:release-v2-4.thread` 🏷`+1 line`；→`chat-msg-3`→`git:latest-commit` 🏷`new commit` | "Every word lands in a file — and a commit — the second it's sent." | daemon 写入 + auto-commit | `msg: @lewis -> release-v2-4 L000003` |
| 1.5 | Members 里 coordinator → working | ◎`member-coordinator` 🏷`working` | "The mention wakes the coordinator. Nobody else." | routing 命中 | — |

## 章 2 · 组队派活（13 帧）

| # | 画面 | 高光 | 解说词（EN） | 事件 / CLI | Git 痕迹 |
|---|------|------|-------------|-----------|---------|
| 2.1 | coordinator 回复出现（L000004）+ 命令 chip | ◎`chat-msg-4` | "The coordinator turns intent into CLI calls." | msg: `On it — spinning up two agents.` + chip `$ gitim-runtime add-agent --handler investigator --provider claude` | — |
| 2.2 | Members 新增 investigator | →`chat-msg-4`→`member-investigator` 🏷`new member · claude` | "A new teammate — running on Claude." | provision | — |
| 2.3 | git 树新增 yaml + commit | →`chat-msg-4`→`git:users/investigator.meta.yaml` 🏷`new file`；→→`git:latest-commit` 🏷 | "Its identity is a YAML file — written by the daemon, never by hand." | auto-commit | `user: register @investigator` |
| 2.4 | 第二条命令 chip 出现 | ◎`chat-msg-4` 内 chip2 | （无解说） | chip `$ gitim-runtime add-agent --handler fixer --provider codex` | — |
| 2.5 | Members 新增 fixer | →`chat-msg-4`→`member-fixer` 🏷`new member · codex` | "Different job, different model. Providers are per-agent." | provision | — |
| 2.6 | git 树新增 yaml + commit | →→`git:users/fixer.meta.yaml` 🏷`new file`；→→`git:latest-commit` 🏷 | （无解说） | auto-commit | `user: register @fixer` |
| 2.7 | coordinator 第二条消息（L000005）+ 建卡 chip ×1 | ◎`chat-msg-5`；→`chat-msg-5`→`git:release-v2-4.thread` 🏷`+1 line` | "Then the work becomes cards." | msg: `Cards up:` + chip `$ gitim card create release-v2-4 'Investigate duplicate webhook retries' --assignee investigator --label incident` | `msg: @coordinator -> release-v2-4 L000004`，`… L000005` |
| 2.8 | Cards 新增 wh-3a91 | →`chat-msg-5`→`card-wh-3a91` 🏷`new card · → investigator` | "Owned from the first second." | — | — |
| 2.9 | git 树新增 card 文件 + commit | →→`git:cards/wh-3a91/` 🏷`new file`×2；→→`git:latest-commit` 🏷 | "A card is two small files: metadata, and a discussion thread." | auto-commit | `card: create wh-3a91 in release-v2-4 by @coordinator` |
| 2.10 | 建卡 chip ×2 → Cards 新增 wh-3a92 | →`chat-msg-5`→`card-wh-3a92` 🏷`new card · → fixer` | （无解说） | chip `$ gitim card create release-v2-4 'Patch retry dedupe' --assignee fixer --label incident` | — |
| 2.11 | git 树 + commit | →→`git:cards/wh-3a92/` 🏷×2；→→`git:latest-commit` 🏷 | （无解说） | auto-commit | `card: create wh-3a92 in release-v2-4 by @coordinator` |
| 2.12 | coordinator 第三条消息（L000006）：分工 ping | ◎`chat-msg-6`；→`chat-msg-6`→`card-wh-3a91` ◎；→`chat-msg-6`→`card-wh-3a92` ◎ | "Mentions route the work — each agent wakes only for its own card." | msg: `<@investigator> you're on wh-3a91, <@fixer> on wh-3a92. Findings go in the card threads.` | `msg: … L000006` |
| 2.13 | Members：investigator + fixer → working | ◎`member-investigator` ◎`member-fixer` 🏷`working`×2 | （无解说） | routing 命中 | — |

## 章 3 · 交付（8 帧）

| # | 画面 | 高光 | 解说词（EN） | 事件 / CLI | Git 痕迹 |
|---|------|------|-------------|-----------|---------|
| 3.1 | 主视图切到 card wh-3a91 讨论；状态 todo→doing | →`member-investigator`→`card-wh-3a91` 🏷`status → doing`；→→`git:wh-3a91/card.meta.yaml` 🏷`~1 line` | "First move: claim the work. A status flip is just a field edit." | `gitim card update release-v2-4 wh-3a91 --status doing` | `card: update wh-3a91 … by @investigator` |
| 3.2 | investigator 的第 1 条讨论消息出现 | ◎`card-msg-1` | "The investigation happens in the open — inside the card's own thread." | `gitim card comment release-v2-4 wh-3a91 'Found it. We ack before the dedupe check — anything retried inside the 30s window gets processed twice. Under load, that is every retry.'` | — |
| 3.3 | 箭头落盘 + commit | →`card-msg-1`→`git:wh-3a91/discussion.thread` 🏷`+1 line`；→→`git:latest-commit` 🏷 | （无解说） | auto-commit | `msg: @investigator -> release-v2-4/wh-3a91 L000001` |
| 3.4 | investigator 第 2 条（移交）+ 卡 →done | ◎`card-msg-2`；→`card-msg-2`→`card-wh-3a91` 🏷`status → done` | "Findings, handoff, closure — all auditable." | `… 'Done on my side. <@fixer>: dedupe must run before ack, keyed on delivery id.'` + `card update --status done` | `msg: … L000002`；`card: update wh-3a91 … by @investigator` |
| 3.5 | 主视图切到 card wh-3a92 讨论；fixer 报告 + 状态 doing→done | ◎`card-msg-3`；🏷`status → done` | "The fix ships — and the card says why, not just that it did." | `gitim card comment release-v2-4 wh-3a92 'Patch in PR #417: dedupe moved ahead of ack, idempotency keyed on delivery id. Canary clean for 30 min.'` + update doing/done | `msg: @fixer -> … L000001`；`card: update wh-3a92 … by @fixer`×2 |
| 3.6 | 箭头落盘 + commits | →`card-msg-3`→`git:wh-3a92/discussion.thread` 🏷；→→`git:wh-3a92/card.meta.yaml` 🏷 | （无解说） | auto-commit ×3 | 同上 |
| 3.7 | 主视图切回 #release-v2-4；coordinator 回执（L000007） | ◎`chat-msg-7`；→`chat-msg-7`→`git:release-v2-4.thread` 🏷`+1 line`；→→`git:latest-commit` 🏷 | "The coordinator closes the loop — in plain language." | msg: `Both cards closed. Duplicate invoices at zero on canary — v2.4 is unblocked. Full trail is in Git.` | `msg: @coordinator -> release-v2-4 L000007` |
| 3.8 | **终帧：全部面板全亮，零聚光**；右侧 git 树完整可见 | 无 | "Two agents hired. Two cards closed. Sixteen commits. You never left the chat." | — | 累计 16 条 |

## 本次 demo 的 16 条 commit（时间序）

1. `msg: @lewis -> release-v2-4 L000003`
2. `msg: @coordinator -> release-v2-4 L000004`
3. `user: register @investigator`
4. `user: register @fixer`
5. `msg: @coordinator -> release-v2-4 L000005`
6. `card: create wh-3a91 in release-v2-4 by @coordinator`
7. `card: create wh-3a92 in release-v2-4 by @coordinator`
8. `msg: @coordinator -> release-v2-4 L000006`
9. `card: update wh-3a91 in release-v2-4 by @investigator`（doing）
10. `msg: @investigator -> release-v2-4/wh-3a91 L000001`
11. `msg: @investigator -> release-v2-4/wh-3a91 L000002`
12. `card: update wh-3a91 in release-v2-4 by @investigator`（done）
13. `card: update wh-3a92 in release-v2-4 by @fixer`（doing）
14. `msg: @fixer -> release-v2-4/wh-3a92 L000001`
15. `card: update wh-3a92 in release-v2-4 by @fixer`（done）
16. `msg: @coordinator -> release-v2-4 L000007`

触及文件（7 个）：`release-v2-4.thread`、`users/investigator.meta.yaml`、
`users/fixer.meta.yaml`、`cards/wh-3a91/{card.meta.yaml,discussion.thread}`、
`cards/wh-3a92/{card.meta.yaml,discussion.thread}`。

## 时长估算

- 有解说帧 17 个 × 平均 ~5.5s ≈ 94s；无解说节拍帧 9 个 × ~2s = 18s。
- **总时长 ≈ 2 分钟**。可暂停 / 步进 / 跳幕。

## 待 owner 决策点

1. **多 provider 卖点**：investigator=claude、fixer=codex（"providers are per-agent"）。
   保留 or 统一成一个 provider？
2. **虚构细节的度**：`duplicate invoices`、`30s window`、`PR #417`、`canary 30 min`
   都是编造但合理的技术细节（治 mock 感的药）。保留这个浓度 or 再降？
3. **解说语言**：现为英文（跟随 landing）。要不要中文字幕（音频仍英文）？
4. **终帧文案**：`"Two agents hired. Two cards closed. Sixteen commits.
   You never left the chat."` —— 主诉求落在"没离开过聊天"，符合收窄后的范围。
