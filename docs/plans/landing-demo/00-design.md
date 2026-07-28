# Landing Demo v2 — 设计稿

状态：**分镜 v2 已出，待 owner 批改**。分镜锁定后启动 Phase A 实现。

## 决策记录

| 日期 | 决策 | 结论 |
|------|------|------|
| 2026-07-28 | 演示形态路线 | **路线 A 锁定**：继续卡通 stage（接受与真产品的偏差，当前产品形态已稳定；产品大改时 demo 再大改一次） |
| 2026-07-28 | 语音解说 | **采用 MiMo TTS 离线预生成**，已实测通过（见 §3） |
| 2026-07-28 | 流程对齐方式 | 以真实产品事件链为骨架，逐帧分镜表与 owner 逐点确认后再写代码 |
| 2026-07-28 | 展示范围（owner） | **收窄为两类**：聊天 + 聊天引起的成员列表/卡片变化。取消 Files tab 与审计幕；舞台改**单场景无 tab**（左 chat 主视图 / 右 Members+Cards+紧凑 Git 树）；每句对话后必有箭头扇出到变化位置；语境改为真实的线上事故响应 |

## 0. 背景与诊断

现状（`feat/homepage-demo-ppt`）：landing 内嵌 DemoStage，15 帧剧本，单点 spotlight
（`opacity-40` 全局 dim + 底部中央 caption pill）。

基于截图证据的问题：

| # | 问题 | 证据 |
|---|------|------|
| P1 | 全局 dim 粗暴：聚光时其余全部 opacity-40，播放中整屏灰蒙蒙 | `ppt-02-demo-t1/t2.png` |
| P2 | caption pill 浮在舞台底部中央，遮挡 chat 输入框 / commit log | `ppt-02-demo-t4.png` |
| P3 | 结束帧仍停在 dim + pill 状态，没有"成果全览"时刻 | `ppt-03-demo-final.png` |
| P4 | 初始帧是大空盒（空 chat、空 receipt） | `ppt-01-landing-full.png` |
| P5 | 只有 6–7 个语义步，因果跳跃大：消息发出后文件怎么变的全靠观众脑补 | scenario.ts 15 帧 |
| P6 | 高光只指"看哪里"，从不指"什么导致了什么" | 单点 spotlight 模型 |
| P7 | 帧固定 600–1200ms 定时器驱动，播得太快，观众来不及读 | 实测播放 |
| P8 | 舞台是手绘迷你 UI，与真实产品观感差距大（已决策：接受，见决策记录） | — |

## 1. 核心概念：因果联动高光（cause → effect）

设计转向：**从"单点聚光"到"事件驱动的高光网络"**。每个 frame 不只说"看这里"，
而是同步表达"这个动作 → 引起了那些变化"。

用户原话的场景即为一等公民：

> 输入文案发出去的时候，同步出现高亮箭头，指示这个地方的文件发生了变化。

### 1.1 锚点系统

舞台内所有可被指示的元素注册稳定锚点 id（`data-anchor` 属性）：

- `tab-chat|tab-agents|tab-cards|tab-files`
- `chat-input`、`chat-msg-<lineNumber>`
- `agent-<handler>`、`card-<cardId>`
- `file-node-<path>`（文件树节点）、`file-content`、`commit-log`、`commit-<id>`
- `receipt-panel`、`receipt-<id>`

Overlay 层（绝对定位 SVG，盖在 stage 上，`pointer-events: none`）在每帧切换时
用 `getBoundingClientRect()` 解算锚点坐标。窗口 resize / tab 切换时重算。

### 1.2 三种高光元素

1. **流动箭头（arrow）**：from 锚点边缘 → to 锚点边缘的 SVG 三次贝塞尔虚线，
   `stroke-dashoffset` 缓慢流动表现方向，终点小三角箭头 + 可选 label
   （如 `"1 file changed"`）。150ms 淡入，驻留期间流动，下一帧 150ms 淡出。
   用 accent 色 `#60a5fa`，线宽 1.5px —— 克制，符合 DESIGN.md minimal-functional。
2. **脉冲 ring（pulse）**：目标元素外圈蓝色 ring 做 2 次呼吸（scale 1→1.02→1，
   250ms × 2），之后定格为 1px 细 ring 直到帧结束。替代现有的 opacity-40 dim。
3. **变化徽章（badge）**：文件树节点 / commit 条目旁滑入小徽章：
   `new file`、`+1 line`、`status → done`。

### 1.3 dim 策略废除

- 不再有任何全局降透明度。非相关区域**保持全亮**——观众余光仍能跟上上下文。
- 视线引导完全交给箭头 + 脉冲 ring。

### 1.4 caption 改为解说栏

- 移除浮动的中央 pill。舞台顶部固定一条解说栏（高度 40px）：
  左侧章节名（如「组建团队 · 3/9」），中间 frame 标题 + 一句说明，右侧步骤计数。
- 位置永远稳定，永不遮挡内容。解说栏文案同时充当**语音解说的字幕**（见 §3）。

## 2. 剧本：26 帧，三幕，单场景

> **逐帧分镜表见 [01-storyboard.md](01-storyboard.md) v2**（含每帧解说词 / CLI /
> Git 痕迹，已与 daemon、CLI 源码核对协议保真）。本节只保留骨架。

语境（owner 要求真实语境）：**线上事故响应**——发布前夜 prod webhook 重复投递
（客户收到重复发票），owner 在 `#release-v2-4` 里拉起临时事故小组。

```
事故消息 → coordinator 被 mention 唤醒
→ add-agent ×2（investigator/claude + fixer/codex，成员列表变化）
→ card create ×2（卡片变化）→ 分工 ping（mention routing）
→ card discussion 调查 → 状态 todo→doing→done
→ coordinator 回执 → 终帧全亮
```

章 1「事故」（5 帧）· 章 2「组队派活」（13 帧）· 章 3「交付」（8 帧）。

固定节拍：**每句对话出现后，箭头扇出到所有变化位置**（成员列表 / 卡片 /
git 树 / commit 行）——这是 owner 点名的核心机制。

终帧全亮收尾，解说："Two agents hired. Two cards closed. Sixteen commits.
You never left the chat."

### 节奏：音频驱动（替代定时器）

- **帧时长 = 该帧解说音频时长 + 0.5s 留白**（详见 §3）；无音频的帧回退到
  按 caption 字数估算的 2.5–4s。
- 打字动画帧 max(音频, 打字时长)。
- 可暂停、单步前进/后退、点击章节进度条跳幕。
- `prefers-reduced-motion`：纯步进 + 不自动播放音频（现有降级逻辑保留）。

## 3. 语音解说（MiMo TTS，已实测）

### 3.1 已验证事实（2026-07-28 实测）

- 端点：`https://token-plan-cn.xiaomimimo.com/v1`（OpenAI 兼容，owner 提供 key）
- 模型清单（`GET /v1/models` 实测）：`mimo-v2.5` / `mimo-v2.5-pro` / `mimo-v2.5-asr`
  / **`mimo-v2.5-tts`** / `mimo-v2.5-tts-voiceclone` / `mimo-v2.5-tts-voicedesign`
- 调用方式：**不是** `/audio/speech`，而是 `POST /v1/chat/completions`：
  `messages = [user: 风格描述, assistant: 待播报文本]` + `audio: { format, voice }`，
  返回 `choices[0].message.audio.data`（base64）
- 冒烟样例：两句英文（约 30 词）→ 24kHz mono wav，**8.96s**，音色 Chloe。
  即解说节奏约 4–4.5s/句 —— 36 帧 ≈ 2–3 分钟总长，符合"慢一点"的要求。
- key 管理：脚本运行时从 `~/ateam/llm_api.md` 读取，不落任何仓库文件。

### 3.2 生成管线（离线预生成，运行时零模型依赖）

```
scripts/generate-demo-narration.mjs   （构建期手动跑，不进 CI）
  ├─ 读 scenario.ts 每帧的 narration 文本
  ├─ 调 mimo-v2.5-tts（统一 style prompt + 预置音色）
  ├─ 优先请求 mp3 格式；不支持则拿 wav 用 ffmpeg 转 mp3（64kbps mono）
  └─ 写 products/gitim/frontend/public/demo-audio/<frame-id>.mp3 + manifest.json
     （manifest 含每帧 durationMs，供播放器排版帧时长）
```

- 产物提交进 git（36 段 × ~3–8s × 64kbps ≈ 1–2MB），保持 deterministic replay：
  任何环境打开 landing 都听到同一份解说。
- 语言：**英文**（跟随 landing 现有文案）；音色统一用一个预置音色（初样 Chloe，
  风格由 user message 的 style prompt 控制："calm, confident product-demo narrator"）。

### 3.3 播放与降级

- 进帧即播对应音频；**浏览器 autoplay 策略**：必须由用户手势解锁 ——
  demo 的 Play 按钮即解锁点。提供两个入口：`▶ Play with narration` / `Play muted`。
- 解说栏文案 = 字幕，永远可见，与音频同步。
- 降级链：音频文件缺失 / 解码失败 → 该帧回退定时器时长，不阻塞播放。
- 静音开关在控制栏（与 Replay 并排）。

## 4. 舞台 chrome 重构（v2：单场景无 tab）

```
┌────────────────────────────────────────────────────┐
│ ▰▰▱ 事故 │ ▱ 组队派活 │ ▱ 交付             12 / 26 │  ← 章节进度条（新）
├────────────────────────────────────────────────────┤
│ 解说栏（= 字幕，固定 40px）                           │  ← 解说栏（新）
├───────────────────────────────┬────────────────────┤
│ #release-v2-4 或 card 讨论视图  │ MEMBERS            │
│                               │  （状态徽章实时变）    │
│  chat 消息流（唯一主视图）       ├────────────────────┤
│  + 行内命令 chip               │ CARDS              │
│        ╭── 箭头 overlay ──────┤  （出现/指派/状态）   │
│                               ├────────────────────┤
│  [输入框]                      │ GIT                │
│                               │  紧凑树+最新 commit 行│
├───────────────────────────────┴────────────────────┤
│  ◀ ▶  Play(with audio/muted)  ↻  🔇                 │  ← 控制栏
└────────────────────────────────────────────────────┘
```

**布局要点（v2 范围收窄后）**：

- **无 tab、单场景**：主视图永远是 chat——channel 消息流，或某张卡的 discussion
  视图（章 3 切入/切出）。card discussion 本质上也是聊天，符合"只展示两类功能"。
- 右侧栏常驻三段：**MEMBERS**（状态徽章 active/working 实时变化）、
  **CARDS**（出现、指派、状态流转）、**GIT**（紧凑文件树 + 最新 commit 行）。
  三段都是箭头的落点——"每句对话之后哪些位置变了"一目了然。
- coordinator 的 CLI 调用以**行内命令 chip** 挂在其消息下方（等效真实产品 agent
  的 tool-call 展示），不再有独立 receipt 面板。
- 初始帧预填历史内容（2 条历史消息 + 已有文件），消灭空盒开场（P4）。
- 结束帧全部面板全亮、零聚光（P3）。
- 时间戳显示 `21:4x`（原始 ISO 放 title tooltip）。
- 命令 chip `truncate` + hover 展开。
- 移动端（<lg）：单列布局，右侧栏三段折叠为手风琴；箭头简化为短箭头 + 徽章。


## 5. 数据结构演进（`lib/demo-story/types.ts`）

```ts
type AnchorId = string; // 约定命名空间，见 §1.1

type DemoEffect =
  | { kind: "arrow"; from: AnchorId; to: AnchorId; label?: string }
  | { kind: "pulse"; target: AnchorId }
  | { kind: "badge"; target: AnchorId; text: string };

interface DemoFrame {
  id: string;
  chapter: "incident" | "teamup" | "delivery"; // 新增（三幕）
  delayMs: number;       // 仅作无音频回退；有 narration 时以音频时长为准
  view: { kind: "channel" } | { kind: "card"; cardId: string }; // 主视图（新，替代 tab）
  title: string;          // 解说栏标题（新）
  caption?: string;       // 解说栏说明（新，兼字幕）
  narration?: string;     // 解说文本（新；音频生成管线的输入）
  effects?: DemoEffect[]; // 因果高光（新，替代旧 spotlight）
  typing?: { anchor: AnchorId; text: string; cps?: number }; // 打字动画（新）
  fileChanges: FileChange[];
  uiChanges: UiChange[];
  commit?: DemoCommit;
}
```

- 旧 `spotlight` 字段退役，相关渲染代码删除。
- `tab` 字段退役：`view` 描述主视图在 channel 与某张卡的 discussion 之间切换；
  Members / Cards / Git 右侧栏永远常驻，不在 view 语义内。
- `stateAtFrame` / `applyFrame` 纯函数逻辑不变，effects / narration 是渲染层概念，
  不进状态机。
- 协议保真：thread 行、card yaml、commit message 继续严格对齐 `gitim-core` 真实格式
  （现有 scenario 已做到，扩写时沿用 `formatThreadLine` 等 helper）。

## 6. 分期实施

| Phase | 内容 | 验收 |
|-------|------|------|
| A. 高光引擎 + 舞台重构 | 单场景布局（chat 主视图 + Members/Cards/Git 右侧栏）、锚点系统 + SVG 箭头 overlay + 脉冲 ring + 解说栏 + 章节进度条；现有 15 帧剧本先迁移到新模型冒烟 | playwright 截图逐帧核对；现有 vitest 全绿 |
| B. 剧本扩写 + 解说 | 15 → 26 帧、三幕、打字动画、行内命令 chip、预填初始态、全亮结束帧；**逐帧分镜表先与 owner 对齐**；对齐后写每帧 narration 文本并跑生成管线出音频 | 截图验收每幕；scenario.test.ts 扩写；音频 manifest 生成 |
| C. 打磨 | 时间戳格式化、命令 chip truncate、移动端箭头简化、音色/语速走查、最终视觉走查对照 DESIGN.md | 桌面 + 390px 截图；完整试听一遍；DESIGN.md 合规自查 |

依赖：B 依赖 A 的 effects 模型；C 依赖 B。

## 7. 测试策略

- `scenario.test.ts`：帧应用、章节边界、commit 序列（沿用现有纯函数测试模式）。
- `landing-page.test.tsx`：锚点存在性（每个 effects 引用的锚点都能在 DOM 找到——
  防"箭头指向不存在元素"回归）、解说栏文案、章节进度条状态。
- 新增契约测试：**剧本里每个 `effects.from/to` 锚点必须在对应帧的 view 下可见**，
  在 jsdom 里逐帧 mount 验证。
- 音频：`public/demo-audio/manifest.json` 与剧本帧 id 的一致性测试（每帧要么有
  音频条目要么显式标记无音频）。
- 视觉：每 Phase 结束跑 playwright 截图存档对比（不进 CI，人工验收）。

## 8. 非目标

- 不做多剧本选择器（仍单一 reviewer 故事，但剧本引擎支持未来扩展）。
- 不做真实后端联动（保持 deterministic replay 卖点）。
- 不动 landing hero 区文案（本轮专注 demo 舞台；hero 增强留待 demo 稳定后单独评审）。
- v1 不做 voice clone / voicedesign 定制音色（管线留口，先用预置音色）。
- 不做双语音轨（v1 仅英文；如需中文版再跑一遍生成管线即可，管线本身语言无关）。
