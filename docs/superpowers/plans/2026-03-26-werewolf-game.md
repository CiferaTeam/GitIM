# Werewolf Game — Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 5+ AI agents 通过真实 GitIM daemon 完成一局完整的狼人杀游戏，每个 agent 独立进程，只通过 IM 消息通信。

**Architecture:** 多进程架构——Game Runner spawn God + 5 Player 进程，所有通信走 GitIM daemon HTTP API。God 是纯 LLM agent（规则在 system prompt），状态即消息（无单独 state store）。每个 player 有 thinking channel（DM(self)）记录推理过程。Context Manager 做 channel 可见性过滤 + 消息格式化注入。

**Tech Stack:** TypeScript, Anthropic SDK (`@anthropic-ai/sdk`), GitIM daemon HTTP API, `tsx` runner

**Worktree:** `/Users/lewisliu/ateam/GitIM/.worktrees/werewolf/`

**Eng Review Key Decisions:**
- 多进程（每 agent 一个进程），非单进程
- 状态即消息（God 用 send_message 宣布状态），砍掉 update/read_game_state tools
- 不新增 tool，复用现有 5 个（send_message, read_messages, list_channels, list_users, get_thread）
- Player 不给 read_messages tool（Context Manager 注入消息，player 不主动读）
- 顺序发言 + "结束"信号 / 60s timeout
- 死亡 = 最后反思 → 停止 loop
- 投票纯 LLM
- DM(self) 是 blocker，必须先验证

---

## File Structure

```
sim/src/
├── tools.ts                      (MODIFY: export callDaemon)
├── chat-demo.ts                  (UNTOUCHED)
├── werewolf/
│   ├── types.ts                  (NEW: Role, GameConfig, AgentConfig, visibility map)
│   ├── context-manager.ts        (NEW: channel visibility filter + message formatting)
│   ├── prompts.ts                (NEW: God + Player system prompts)
│   ├── player-agent.ts           (NEW: player process entry point)
│   ├── god-agent.ts              (NEW: god process entry point)
│   └── runner.ts                 (NEW: spawn + lifecycle monitor)
├── werewolf/__tests__/
│   ├── context-manager.test.ts   (NEW: unit tests)
│   ├── types.test.ts             (NEW: visibility matrix tests)
│   └── e2e-mock.test.ts          (NEW: mock daemon E2E)
└── package.json                  (MODIFY: add scripts + vitest)
```

---

## Chunk 1: Foundation — Types + Context Manager

### Task 1: Validate DM(self) with real daemon

**Files:**
- None (manual validation against running daemon)

- [ ] **Step 1: Start daemon in HTTP debug mode**

确保 `.gitim/config.yaml` 里有 `debug_http: true`，然后启动 daemon：
```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/werewolf
cargo run --bin gitim-daemon -- --repo-root /tmp/test-werewolf-dm
```

- [ ] **Step 2: Test DM(self) — register user + send + read**

```bash
DAEMON=http://localhost:3000
# Register user
curl -s $DAEMON/api -d '{"method":"register_user","handler":"alice","display_name":"Alice"}' | jq .
# Send DM to self
curl -s $DAEMON/api -d '{"method":"send","channel":"dm:alice,alice","body":"thinking out loud","author":"alice"}' | jq .
# Read DM(self)
curl -s $DAEMON/api -d '{"method":"read","channel":"dm:alice,alice"}' | jq .
```

Expected: all three return `{"ok": true, ...}`. Read should return the message.

- [ ] **Step 3: Document result**

If DM(self) works: proceed. If blocked: fallback to `dm:god,{player}` for thinking (上帝看得到所有人的思考，可接受)。

---

### Task 2: Add vitest + export callDaemon

**Files:**
- Modify: `sim/package.json`
- Modify: `sim/src/tools.ts:12` (export callDaemon)

- [ ] **Step 1: Add vitest to devDependencies**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/werewolf/sim
npm install -D vitest
```

- [ ] **Step 2: Add test script to package.json**

在 `scripts` 里添加：
```json
"test": "vitest run",
"test:watch": "vitest",
"werewolf": "tsx src/werewolf/runner.ts"
```

- [ ] **Step 3: Export callDaemon from tools.ts**

在 `sim/src/tools.ts` 第 12 行，把 `async function callDaemon` 改为 `export async function callDaemon`。

- [ ] **Step 4: Commit**

```bash
git add sim/package.json sim/package-lock.json sim/src/tools.ts
git commit -m "chore(sim): add vitest + export callDaemon for werewolf reuse"
```

---

### Task 3: Create types.ts — Roles, Config, Visibility

**Files:**
- Create: `sim/src/werewolf/types.ts`
- Create: `sim/src/werewolf/__tests__/types.test.ts`

- [ ] **Step 1: Write the test**

```typescript
// sim/src/werewolf/__tests__/types.test.ts
import { describe, it, expect } from "vitest";
import { getVisibleChannels, Role } from "../types.js";

describe("getVisibleChannels", () => {
  const players = ["alice", "bob", "charlie", "dave", "eve"];
  const wolves = ["dave", "eve"];

  it("wolf sees general + wolves + dm(god) + dm(self)", () => {
    const channels = getVisibleChannels("dave", Role.Wolf, wolves);
    expect(channels).toContain("general");
    expect(channels).toContain("wolves");
    expect(channels).toContain("dm:dave,god");
    expect(channels).toContain("dm:dave,dave");
    expect(channels).not.toContain("dm:alice,god");
  });

  it("seer sees general + dm(god) + dm(self)", () => {
    const channels = getVisibleChannels("alice", Role.Seer, wolves);
    expect(channels).toContain("general");
    expect(channels).toContain("dm:alice,god");
    expect(channels).toContain("dm:alice,alice");
    expect(channels).not.toContain("wolves");
  });

  it("villager sees general + dm(self) only", () => {
    const channels = getVisibleChannels("bob", Role.Villager, wolves);
    expect(channels).toContain("general");
    expect(channels).toContain("dm:bob,bob");
    expect(channels).not.toContain("wolves");
    expect(channels).not.toContain("dm:bob,god");
  });

  it("witch sees general + dm(god) + dm(self)", () => {
    const channels = getVisibleChannels("charlie", Role.Witch, wolves);
    expect(channels).toContain("general");
    expect(channels).toContain("dm:charlie,god");
    expect(channels).toContain("dm:charlie,charlie");
    expect(channels).not.toContain("wolves");
  });

  it("god sees everything", () => {
    const channels = getVisibleChannels("god", Role.God, wolves);
    expect(channels).toContain("general");
    expect(channels).toContain("wolves");
    // God sees all DMs
    expect(channels.length).toBeGreaterThan(3);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/werewolf/sim
npx vitest run src/werewolf/__tests__/types.test.ts
```

Expected: FAIL — module not found.

- [ ] **Step 3: Implement types.ts**

```typescript
// sim/src/werewolf/types.ts

export enum Role {
  Wolf = "wolf",
  Seer = "seer",
  Witch = "witch",
  Hunter = "hunter",
  Villager = "villager",
  God = "god",
}

export interface AgentConfig {
  handler: string;
  displayName: string;
  role: Role;
  personality: string;
}

export interface GameConfig {
  players: AgentConfig[];
  daemonUrl: string;
  llmModel: string;
}

/** DM channel name: two handlers sorted lexicographically, joined by comma */
function dmChannel(a: string, b: string): string {
  return a <= b ? `dm:${a},${b}` : `dm:${b},${a}`;
}

/** Returns the list of channel identifiers an agent can see. */
export function getVisibleChannels(
  handler: string,
  role: Role,
  wolfHandlers: string[],
  allHandlers?: string[]
): string[] {
  const channels: string[] = ["general"];

  // Self DM (thinking channel)
  channels.push(dmChannel(handler, handler));

  if (role === Role.God) {
    // God sees everything
    channels.push("wolves");
    if (allHandlers) {
      for (const h of allHandlers) {
        channels.push(dmChannel("god", h));
        channels.push(dmChannel(h, h));
      }
    }
    return channels;
  }

  // Wolves see wolf channel
  if (role === Role.Wolf) {
    channels.push("wolves");
  }

  // Non-villager roles have DM with God
  if (role !== Role.Villager) {
    channels.push(dmChannel(handler, "god"));
  }

  return channels;
}
```

- [ ] **Step 4: Run test to verify it passes**

```bash
npx vitest run src/werewolf/__tests__/types.test.ts
```

Expected: all 5 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add sim/src/werewolf/types.ts sim/src/werewolf/__tests__/types.test.ts
git commit -m "feat(werewolf): add Role enum and visibility matrix with tests"
```

---

### Task 4: Create Context Manager

**Files:**
- Create: `sim/src/werewolf/context-manager.ts`
- Create: `sim/src/werewolf/__tests__/context-manager.test.ts`

- [ ] **Step 1: Write the test**

```typescript
// sim/src/werewolf/__tests__/context-manager.test.ts
import { describe, it, expect } from "vitest";
import { formatInjection, ChannelMessages } from "../context-manager.js";

describe("formatInjection", () => {
  const channelMessages: ChannelMessages = {
    general: [
      { author: "god", body: "天亮了。昨晚 frank 被杀害了。", line_number: 45, timestamp: "20260325T103000Z" },
      { author: "alice", body: "太可惜了", line_number: 46, timestamp: "20260325T103015Z" },
    ],
    wolves: [
      { author: "dave", body: "今晚杀谁？", line_number: 12, timestamp: "20260325T102500Z" },
    ],
  };

  it("formats messages grouped by channel", () => {
    const result = formatInjection(channelMessages, "现在是白天讨论阶段，请发言。");
    expect(result).toContain("=== #general 新消息");
    expect(result).toContain("[@god]");
    expect(result).toContain("天亮了");
    expect(result).toContain("=== #wolves 新消息");
    expect(result).toContain("今晚杀谁");
    expect(result).toContain("=== 当前任务 ===");
    expect(result).toContain("请发言");
  });

  it("skips channels with no messages", () => {
    const result = formatInjection({ general: [] }, "任务");
    expect(result).not.toContain("#general");
    expect(result).toContain("=== 当前任务 ===");
  });

  it("includes thinking section when provided", () => {
    const result = formatInjection(
      { general: channelMessages.general },
      "任务",
      ["我怀疑 bob", "charlie 可疑"]
    );
    expect(result).toContain("=== 你的近期思考");
    expect(result).toContain("我怀疑 bob");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

```bash
npx vitest run src/werewolf/__tests__/context-manager.test.ts
```

- [ ] **Step 3: Implement context-manager.ts**

```typescript
// sim/src/werewolf/context-manager.ts

export interface MessageEntry {
  author: string;
  body: string;
  line_number: number;
  timestamp: string;
}

export type ChannelMessages = Record<string, MessageEntry[]>;

/**
 * Format channel messages into structured text for LLM injection.
 * This is the user message appended to the agent's conversation each turn.
 */
export function formatInjection(
  channelMessages: ChannelMessages,
  task: string,
  recentThinking?: string[]
): string {
  const sections: string[] = [];

  // Channel messages grouped by channel
  for (const [channel, messages] of Object.entries(channelMessages)) {
    if (messages.length === 0) continue;
    const channelLabel = channel.startsWith("dm:") ? `DM(${channel.slice(3)})` : `#${channel}`;
    sections.push(`=== ${channelLabel} 新消息 (${messages.length}条) ===`);
    for (const m of messages) {
      sections.push(`[L${String(m.line_number).padStart(6, "0")}][@${m.author}][${m.timestamp}] ${m.body}`);
    }
    sections.push("");
  }

  // Recent thinking
  if (recentThinking && recentThinking.length > 0) {
    sections.push(`=== 你的近期思考 (最近${recentThinking.length}条) ===`);
    recentThinking.forEach((t, i) => sections.push(`[思考${i + 1}] ${t}`));
    sections.push("");
  }

  // Current task
  sections.push("=== 当前任务 ===");
  sections.push(task);

  return sections.join("\n");
}

/**
 * Fetch new messages from visible channels via daemon API.
 * Returns messages grouped by channel.
 */
export async function pollVisibleChannels(
  daemonUrl: string,
  channels: string[],
  sinceLines: Record<string, number>
): Promise<ChannelMessages> {
  const result: ChannelMessages = {};

  for (const ch of channels) {
    const channelName = ch.startsWith("dm:") ? ch : ch;
    const since = sinceLines[ch] ?? 0;
    try {
      const res = await fetch(`${daemonUrl}/api`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          method: "read",
          channel: channelName,
          since: since > 0 ? since : undefined,
        }),
      });
      const json = await res.json();
      if (json.ok && json.data?.messages) {
        result[ch] = json.data.messages.map((m: any) => ({
          author: m.author,
          body: m.body,
          line_number: m.line_number,
          timestamp: m.timestamp ?? "",
        }));
      } else {
        result[ch] = [];
      }
    } catch {
      result[ch] = [];
    }
  }

  return result;
}

/** Get the max line number across all channel messages for cursor tracking */
export function maxLineFromMessages(channelMessages: ChannelMessages): Record<string, number> {
  const result: Record<string, number> = {};
  for (const [ch, msgs] of Object.entries(channelMessages)) {
    result[ch] = msgs.reduce((max, m) => Math.max(max, m.line_number), 0);
  }
  return result;
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
npx vitest run src/werewolf/__tests__/context-manager.test.ts
```

Expected: all 3 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add sim/src/werewolf/context-manager.ts sim/src/werewolf/__tests__/context-manager.test.ts
git commit -m "feat(werewolf): add Context Manager with visibility-filtered message formatting"
```

---

## Chunk 2: Agent Processes — Prompts + Player + God

### Task 5: Create prompts.ts

**Files:**
- Create: `sim/src/werewolf/prompts.ts`

- [ ] **Step 1: Write God system prompt**

```typescript
// sim/src/werewolf/prompts.ts
import { Role } from "./types.js";

export const GOD_SYSTEM_PROMPT = `你是狼人杀游戏的上帝（主持人）。你负责驱动整局游戏的进程。

## 游戏规则

### 角色
- **狼人** (2人): 每晚可以杀一个人。在狼人频道(#wolves)讨论。
- **预言家** (1人): 每晚可以验一个人的身份。通过私聊告知结果。
- **女巫** (1人): 有一瓶解药和一瓶毒药，各只能用一次。通过私聊使用。
- **猎人** (1人): 被投票出局或被狼人杀死时可以带走一人。通过私聊确认。
- **村民** (剩余): 无特殊能力。

### 阶段流程
1. **夜晚阶段**:
   - 在 #general 宣布天黑
   - 在 #wolves 让狼人讨论击杀目标（等待狼人回复决定）
   - 私聊预言家让其验人（等待回复）
   - 私聊女巫告知被杀者，询问是否使用药（等待回复）
   - 结算夜晚结果
2. **白天阶段**:
   - 在 #general 宣布天亮 + 昨晚结果
   - 按座位顺序让每个存活玩家发言（每次 @一个人，等其发言完毕说"结束"或等 60 秒）
   - 发言结束后进入投票（让每人投一票）
   - 宣布投票结果，被投出的人出局
3. **重复**直到一方胜利

### 胜利条件
- **狼人胜**: 狼人数量 >= 好人数量
- **好人胜**: 所有狼人被淘汰

### 你的行为准则
- 你通过消息驱动游戏，宣布每个阶段的开始和结束
- 你记住所有角色分配（在你的上下文中）
- 你按顺序叫每个玩家发言，一次叫一个
- 夜晚行动通过私聊进行（send_message 到 dm:god,玩家）
- 狼人讨论在 #wolves 频道进行
- 投票和讨论在 #general 进行
- 你用有趣的叙事风格宣布结果
- 当胜利条件满足时，宣布游戏结束

### 重要
- 每次只叫一个人发言/行动，等待其回复后再叫下一个
- 如果一个玩家 60 秒没响应，跳过他
- 死亡的玩家不再参与发言和投票
- 游戏结束时在 #general 发送包含 "【游戏结束】" 的消息`;

export function makePlayerPrompt(config: {
  handler: string;
  role: Role;
  personality: string;
  wolfPartners?: string[];
}): string {
  const { handler, role, personality, wolfPartners } = config;

  const roleDescriptions: Record<Role, string> = {
    [Role.Wolf]: \`你是狼人。你的搭档是: \${wolfPartners?.join(", ") ?? "未知"}。
每天晚上你和搭档在 #wolves 频道讨论要杀谁。
白天你需要伪装成好人，不被发现。
投票时尽量把嫌疑引向无辜的人。\`,
    [Role.Seer]: \`你是预言家。每天晚上上帝会私聊你，你可以选一个人验身份。
上帝会告诉你那个人是好人还是狼人。
白天发言时要巧妙利用你的信息，但不要太早暴露自己。\`,
    [Role.Witch]: \`你是女巫。你有一瓶解药（救人）和一瓶毒药（杀人），各只能用一次。
每天晚上上帝会私聊你，告诉你谁被狼人杀了，你可以选择是否用药。
白天发言时可以暗示你的信息但要保护自己。\`,
    [Role.Hunter]: \`你是猎人。如果你被投票出局或被狼人杀死，你可以带走一个人。
上帝会私聊你确认是否开枪以及目标。
白天积极参与讨论和推理。\`,
    [Role.Villager]: \`你是村民。你没有特殊能力，但你的投票至关重要。
仔细观察每个人的发言，分析谁在说谎。
投票时跟随你的判断。\`,
    [Role.God]: "",
  };

  return \`\${personality}

## 你的身份
你是 \${handler}，角色是【\${role}】。

## 角色说明
\${roleDescriptions[role]}

## 行为准则
- 你在 GitIM 频道里和其他玩家交流
- 用 send_message 工具发送消息
- 发送私聊用 send_message，channel 格式为 "dm:你的handler,对方handler"
- 每次发言控制在 2-4 句话
- 发言结束后说"结束"
- 仔细观察其他人的发言模式，进行推理
- 你的文字回复会自动记录到你的思考频道，用于记录你的推理过程
- 投票时直接说出你要投的人的名字

## 重要
- 不要透露自己的角色（除非策略需要）
- 上帝叫你发言时才发言
- 用中文交流\`;
}
```

- [ ] **Step 2: Commit**

```bash
git add sim/src/werewolf/prompts.ts
git commit -m "feat(werewolf): add God and Player system prompts"
```

---

### Task 6: Create player-agent.ts

**Files:**
- Create: `sim/src/werewolf/player-agent.ts`

这是每个 player 进程的入口。接收命令行参数启动。

- [ ] **Step 1: Implement player-agent.ts**

```typescript
// sim/src/werewolf/player-agent.ts
//
// Usage: tsx src/werewolf/player-agent.ts \
//   --handler alice --role seer --display-name Alice \
//   --personality "..." --daemon-url http://localhost:3000 \
//   --wolf-partners "dave,eve"    (only for wolves)
//
// Each player is an independent process. Communication only via GitIM daemon.

import Anthropic from "@anthropic-ai/sdk";
import { Role, getVisibleChannels } from "./types.js";
import { formatInjection, pollVisibleChannels, maxLineFromMessages } from "./context-manager.js";
import { makePlayerPrompt } from "./prompts.js";
import { gitimTools, executeTool } from "../tools.js";

// ── Parse CLI args ──────────────────────────────────────────
function parseArgs(): {
  handler: string;
  role: Role;
  displayName: string;
  personality: string;
  daemonUrl: string;
  wolfPartners: string[];
  wolves: string[];
} {
  const args = process.argv.slice(2);
  const get = (flag: string): string => {
    const idx = args.indexOf(flag);
    if (idx < 0 || idx + 1 >= args.length) throw new Error(`missing ${flag}`);
    return args[idx + 1];
  };
  const getOpt = (flag: string, def: string): string => {
    const idx = args.indexOf(flag);
    return idx >= 0 && idx + 1 < args.length ? args[idx + 1] : def;
  };

  return {
    handler: get("--handler"),
    role: get("--role") as Role,
    displayName: get("--display-name"),
    personality: get("--personality"),
    daemonUrl: getOpt("--daemon-url", "http://localhost:3000"),
    wolfPartners: getOpt("--wolf-partners", "").split(",").filter(Boolean),
    wolves: getOpt("--wolves", "").split(",").filter(Boolean),
  };
}

// ── Player Tools (subset: no read_messages for players) ─────
const playerTools: Anthropic.Messages.Tool[] = gitimTools.filter(
  (t) => t.name === "send_message" || t.name === "list_users"
);

// ── Main Loop ───────────────────────────────────────────────
async function main(): Promise<void> {
  const config = parseArgs();
  const systemPrompt = makePlayerPrompt({
    handler: config.handler,
    role: config.role,
    personality: config.personality,
    wolfPartners: config.wolfPartners,
  });

  const client = new Anthropic({
    apiKey: process.env.LLM_API_KEY ?? process.env.ANTHROPIC_API_KEY,
    baseURL: process.env.LLM_BASE_URL || undefined,
  });
  const model = process.env.LLM_MODEL ?? "claude-sonnet-4-20250514";

  const visibleChannels = getVisibleChannels(
    config.handler,
    config.role,
    config.wolves
  );

  const sinceLines: Record<string, number> = {};
  const messages: Anthropic.Messages.MessageParam[] = [];
  const thinkingHistory: string[] = [];
  let lastActionTime = Date.now();

  console.log(`[${config.handler}] started (role=${config.role}, channels=${visibleChannels.join(",")})`);

  // Poll loop
  while (true) {
    const channelMessages = await pollVisibleChannels(
      config.daemonUrl,
      visibleChannels,
      sinceLines
    );

    // Update cursors
    const newMaxLines = maxLineFromMessages(channelMessages);
    for (const [ch, max] of Object.entries(newMaxLines)) {
      if (max > (sinceLines[ch] ?? 0)) sinceLines[ch] = max;
    }

    // Check for new messages
    const totalNew = Object.values(channelMessages).reduce((s, msgs) => s + msgs.length, 0);
    if (totalNew === 0) {
      await new Promise((r) => setTimeout(r, 2000));
      continue;
    }

    // Check for game over
    const generalMsgs = channelMessages["general"] ?? [];
    const gameOver = generalMsgs.some((m) => m.body.includes("【游戏结束】"));
    if (gameOver) {
      // Write final reflection
      const reflection = `游戏结束了。回顾这局游戏，我的角色是${config.role}。${thinkingHistory.slice(-3).join("。")}`;
      await executeTool("send_message", {
        channel: `dm:${config.handler},${config.handler}`,
        body: `[最终反思] ${reflection}`,
      }, config.handler);
      console.log(`[${config.handler}] game over, reflection written. exiting.`);
      process.exit(0);
    }

    // Check if God mentioned me (my turn to speak)
    const mentionedByGod = generalMsgs.some(
      (m) => m.author === "god" && m.body.includes(`@${config.handler}`)
    );
    // Check for DM from God (night action)
    const godDm = channelMessages[`dm:${config.handler <= "god" ? config.handler + ",god" : "god," + config.handler}`] ?? [];
    const hasGodDm = godDm.length > 0;
    // Check for wolf channel activity (for wolves)
    const wolfMsgs = channelMessages["wolves"] ?? [];
    const wolfActivity = wolfMsgs.length > 0 && config.role === Role.Wolf;

    // Only act if directly addressed or relevant activity
    if (!mentionedByGod && !hasGodDm && !wolfActivity) {
      // Got messages but not our turn — just observe
      await new Promise((r) => setTimeout(r, 2000));
      continue;
    }

    // Build injection and call LLM
    const task = mentionedByGod
      ? "上帝叫你发言了，请在 #general 发表你的看法，发言完毕后说'结束'。"
      : hasGodDm
        ? "上帝私聊了你，请通过私聊回复。"
        : "狼人频道有新消息，请在 #wolves 参与讨论。";

    const injection = formatInjection(channelMessages, task, thinkingHistory.slice(-5));
    messages.push({ role: "user", content: injection });

    try {
      const response = await client.messages.create({
        model,
        max_tokens: 1024,
        system: systemPrompt,
        tools: playerTools,
        messages,
      });

      // Process response
      const assistantContent: Anthropic.Messages.ContentBlock[] = response.content;
      messages.push({ role: "assistant", content: assistantContent });

      for (const block of assistantContent) {
        if (block.type === "tool_use") {
          const result = await executeTool(
            block.name,
            block.input as Record<string, unknown>,
            config.handler
          );
          console.log(`  [${config.handler}] ${block.name}(${JSON.stringify(block.input).slice(0, 100)})`);
          // Append tool result to conversation
          messages.push({
            role: "user",
            content: [{ type: "tool_result", tool_use_id: block.id, content: result }],
          });
        } else if (block.type === "text" && block.text.trim()) {
          // Text = thinking → write to thinking channel
          const thinking = block.text.trim();
          thinkingHistory.push(thinking);
          await executeTool("send_message", {
            channel: `dm:${config.handler},${config.handler}`,
            body: thinking,
          }, config.handler);
          console.log(`  [${config.handler}] 💭 ${thinking.slice(0, 80)}...`);
        }
      }

      lastActionTime = Date.now();
    } catch (err) {
      console.error(`[${config.handler}] LLM error:`, err);
      // Retry once after 5s
      await new Promise((r) => setTimeout(r, 5000));
    }

    await new Promise((r) => setTimeout(r, 1000));
  }
}

main().catch((err) => {
  console.error(`[player] Fatal:`, err);
  process.exit(1);
});
```

- [ ] **Step 2: Commit**

```bash
git add sim/src/werewolf/player-agent.ts
git commit -m "feat(werewolf): add player agent process with poll loop + thinking channel"
```

---

### Task 7: Create god-agent.ts

**Files:**
- Create: `sim/src/werewolf/god-agent.ts`

God 进程——驱动整局游戏。

- [ ] **Step 1: Implement god-agent.ts**

```typescript
// sim/src/werewolf/god-agent.ts
//
// Usage: tsx src/werewolf/god-agent.ts \
//   --daemon-url http://localhost:3000 \
//   --players "alice:seer,bob:villager,charlie:witch,dave:wolf,eve:wolf"

import Anthropic from "@anthropic-ai/sdk";
import { Role, getVisibleChannels } from "./types.js";
import { formatInjection, pollVisibleChannels, maxLineFromMessages } from "./context-manager.js";
import { GOD_SYSTEM_PROMPT } from "./prompts.js";
import { gitimTools, executeTool } from "../tools.js";

// ── Parse CLI args ──────────────────────────────────────────
function parseArgs(): {
  daemonUrl: string;
  players: Array<{ handler: string; role: Role }>;
} {
  const args = process.argv.slice(2);
  const get = (flag: string): string => {
    const idx = args.indexOf(flag);
    if (idx < 0 || idx + 1 >= args.length) throw new Error(`missing ${flag}`);
    return args[idx + 1];
  };
  const getOpt = (flag: string, def: string): string => {
    const idx = args.indexOf(flag);
    return idx >= 0 && idx + 1 < args.length ? args[idx + 1] : def;
  };

  const playersStr = get("--players");
  const players = playersStr.split(",").map((p) => {
    const [handler, role] = p.split(":");
    return { handler, role: role as Role };
  });

  return {
    daemonUrl: getOpt("--daemon-url", "http://localhost:3000"),
    players,
  };
}

// ── Main Loop ───────────────────────────────────────────────
async function main(): Promise<void> {
  const config = parseArgs();
  const wolves = config.players.filter((p) => p.role === Role.Wolf).map((p) => p.handler);
  const allHandlers = config.players.map((p) => p.handler);

  const client = new Anthropic({
    apiKey: process.env.LLM_API_KEY ?? process.env.ANTHROPIC_API_KEY,
    baseURL: process.env.LLM_BASE_URL || undefined,
  });
  const model = process.env.LLM_MODEL ?? "claude-sonnet-4-20250514";

  // God sees all channels
  const visibleChannels = getVisibleChannels("god", Role.God, wolves, allHandlers);
  const sinceLines: Record<string, number> = {};
  const messages: Anthropic.Messages.MessageParam[] = [];

  // Build role assignment summary for God's context
  const rolesSummary = config.players.map((p) => `${p.handler}: ${p.role}`).join(", ");

  console.log(`[god] started. players: ${rolesSummary}`);
  console.log(`[god] visible channels: ${visibleChannels.join(", ")}`);

  // Initial kick-off message
  const kickoff = `游戏开始！\n\n玩家角色分配（只有你知道）:\n${config.players.map((p) => `- ${p.handler}: ${p.role}`).join("\n")}\n\n请开始第一个夜晚阶段。先在 #general 宣布天黑，然后依次处理狼人、预言家、女巫的夜间行动。`;

  messages.push({ role: "user", content: kickoff });

  let gameOver = false;

  while (!gameOver) {
    try {
      const response = await client.messages.create({
        model,
        max_tokens: 2048,
        system: GOD_SYSTEM_PROMPT,
        tools: gitimTools,
        messages,
      });

      const assistantContent = response.content;
      messages.push({ role: "assistant", content: assistantContent });

      // Process tool calls
      const toolResults: Anthropic.Messages.ToolResultBlockParam[] = [];

      for (const block of assistantContent) {
        if (block.type === "tool_use") {
          try {
            const result = await executeTool(
              block.name,
              block.input as Record<string, unknown>,
              "god"
            );
            console.log(`  [god] ${block.name}(${JSON.stringify(block.input).slice(0, 120)})`);
            toolResults.push({ type: "tool_result", tool_use_id: block.id, content: result });
          } catch (err) {
            toolResults.push({
              type: "tool_result",
              tool_use_id: block.id,
              content: `Error: ${err}`,
              is_error: true,
            });
          }
        } else if (block.type === "text" && block.text.trim()) {
          console.log(`  [god] 📢 ${block.text.trim().slice(0, 100)}`);
          // Check for game over in God's text
          if (block.text.includes("【游戏结束】")) {
            gameOver = true;
          }
        }
      }

      // If there were tool calls, add results and let God continue
      if (toolResults.length > 0) {
        messages.push({ role: "user", content: toolResults });

        // After sending messages, wait for player responses
        if (response.stop_reason === "tool_use") {
          // Wait for responses (poll new messages)
          await new Promise((r) => setTimeout(r, 3000));

          const channelMessages = await pollVisibleChannels(
            config.daemonUrl,
            visibleChannels,
            sinceLines
          );

          const newMaxLines = maxLineFromMessages(channelMessages);
          for (const [ch, max] of Object.entries(newMaxLines)) {
            if (max > (sinceLines[ch] ?? 0)) sinceLines[ch] = max;
          }

          const totalNew = Object.values(channelMessages).reduce((s, msgs) => s + msgs.length, 0);
          if (totalNew > 0) {
            const injection = formatInjection(channelMessages, "请继续推进游戏进程。");
            messages.push({ role: "user", content: injection });
          } else {
            // Wait more and retry
            await new Promise((r) => setTimeout(r, 5000));
            const retry = await pollVisibleChannels(config.daemonUrl, visibleChannels, sinceLines);
            const retryMaxLines = maxLineFromMessages(retry);
            for (const [ch, max] of Object.entries(retryMaxLines)) {
              if (max > (sinceLines[ch] ?? 0)) sinceLines[ch] = max;
            }
            const retryTotal = Object.values(retry).reduce((s, msgs) => s + msgs.length, 0);
            if (retryTotal > 0) {
              const injection = formatInjection(retry, "请继续推进游戏进程。");
              messages.push({ role: "user", content: injection });
            } else {
              messages.push({ role: "user", content: "没有收到玩家回复（可能超时了），请跳过并继续推进游戏。" });
            }
          }
        }
      } else if (response.stop_reason === "end_turn") {
        // God finished a thought without tool calls — wait for context update
        await new Promise((r) => setTimeout(r, 3000));
        const channelMessages = await pollVisibleChannels(config.daemonUrl, visibleChannels, sinceLines);
        const newMaxLines = maxLineFromMessages(channelMessages);
        for (const [ch, max] of Object.entries(newMaxLines)) {
          if (max > (sinceLines[ch] ?? 0)) sinceLines[ch] = max;
        }
        const totalNew = Object.values(channelMessages).reduce((s, msgs) => s + msgs.length, 0);
        if (totalNew > 0) {
          const injection = formatInjection(channelMessages, "请继续推进游戏进程。");
          messages.push({ role: "user", content: injection });
        } else {
          messages.push({ role: "user", content: "请继续推进游戏进程。" });
        }
      }
    } catch (err) {
      console.error(`[god] LLM error:`, err);
      await new Promise((r) => setTimeout(r, 5000));
      messages.push({ role: "user", content: "发生了错误，请继续推进游戏。" });
    }
  }

  console.log(`[god] game over. exiting.`);
  process.exit(0);
}

main().catch((err) => {
  console.error(`[god] Fatal:`, err);
  process.exit(1);
});
```

- [ ] **Step 2: Commit**

```bash
git add sim/src/werewolf/god-agent.ts
git commit -m "feat(werewolf): add God agent process — LLM-driven game master"
```

---

## Chunk 3: Game Runner + E2E Mock Test

### Task 8: Create runner.ts

**Files:**
- Create: `sim/src/werewolf/runner.ts`

Runner = 启动器 + 生命周期监控。负责：注册用户、创建 channels、分配角色、spawn 所有进程。

- [ ] **Step 1: Implement runner.ts**

```typescript
// sim/src/werewolf/runner.ts
//
// Usage: tsx src/werewolf/runner.ts [--players 5] [--daemon-url http://localhost:3000]

import { spawn, ChildProcess } from "child_process";
import { Role, AgentConfig } from "./types.js";
import { callDaemon } from "../tools.js";

const DAEMON_URL = process.env.GITIM_DAEMON_URL ?? "http://localhost:3000";

// ── Personality Pool ────────────────────────────────────────
const PERSONALITIES = [
  "你性格果断，说话简洁有力，善于分析逻辑漏洞。",
  "你性格温和，善于观察细节，喜欢总结别人的发言找矛盾。",
  "你性格活泼，说话幽默，但推理时很认真。",
  "你性格沉稳，不轻易表态，但一旦发言就一针见血。",
  "你性格急躁，喜欢直接质疑别人，但有时会过于冲动。",
  "你性格谨慎，喜欢用排除法分析，很少被情绪带节奏。",
  "你性格热心，喜欢帮别人分析，但有时会暴露太多信息。",
];

// ── Role Assignment ─────────────────────────────────────────
function assignRoles(playerCount: number): Role[] {
  // Standard 5-player: 2 wolf, 1 seer, 1 witch, 1 villager
  // Standard 6-player: 2 wolf, 1 seer, 1 witch, 1 hunter, 1 villager
  // Standard 7+: 2 wolf, 1 seer, 1 witch, 1 hunter, rest villager
  const roles: Role[] = [Role.Wolf, Role.Wolf, Role.Seer, Role.Witch];
  if (playerCount >= 6) roles.push(Role.Hunter);
  while (roles.length < playerCount) roles.push(Role.Villager);

  // Shuffle
  for (let i = roles.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [roles[i], roles[j]] = [roles[j], roles[i]];
  }
  return roles;
}

// ── Player Names ────────────────────────────────────────────
const PLAYER_NAMES = [
  { handler: "alice", displayName: "Alice" },
  { handler: "bob", displayName: "Bob" },
  { handler: "charlie", displayName: "Charlie" },
  { handler: "dave", displayName: "Dave" },
  { handler: "eve", displayName: "Eve" },
  { handler: "frank", displayName: "Frank" },
  { handler: "grace", displayName: "Grace" },
];

// ── Main ────────────────────────────────────────────────────
async function main(): Promise<void> {
  const args = process.argv.slice(2);
  const playerCountIdx = args.indexOf("--players");
  const playerCount = playerCountIdx >= 0 ? parseInt(args[playerCountIdx + 1], 10) : 5;
  const daemonUrlIdx = args.indexOf("--daemon-url");
  const daemonUrl = daemonUrlIdx >= 0 ? args[daemonUrlIdx + 1] : DAEMON_URL;

  // Override DAEMON_URL for tools.ts
  process.env.GITIM_DAEMON_URL = daemonUrl;

  console.log(`\n=== 🐺 GitIM 狼人杀 ===`);
  console.log(`玩家数: ${playerCount}`);
  console.log(`Daemon: ${daemonUrl}\n`);

  // Check daemon
  try {
    const status = await callDaemon({ method: "status" });
    console.log(`[runner] daemon online`);
  } catch {
    console.error("[runner] Error: gitim-daemon not running. Start with debug_http: true");
    process.exit(1);
  }

  // Assign roles
  const roles = assignRoles(playerCount);
  const players: AgentConfig[] = PLAYER_NAMES.slice(0, playerCount).map((p, i) => ({
    handler: p.handler,
    displayName: p.displayName,
    role: roles[i],
    personality: PERSONALITIES[i % PERSONALITIES.length],
  }));

  const wolves = players.filter((p) => p.role === Role.Wolf).map((p) => p.handler);

  console.log(`[runner] Role assignment:`);
  for (const p of players) {
    console.log(`  ${p.handler}: ${p.role} ${p.role === Role.Wolf ? "🐺" : p.role === Role.Seer ? "🔮" : p.role === Role.Witch ? "🧪" : p.role === Role.Hunter ? "🔫" : "👤"}`);
  }

  // Register users (god + players)
  console.log(`\n[runner] Registering users...`);
  await callDaemon({ method: "register_user", handler: "god", display_name: "上帝" });
  for (const p of players) {
    await callDaemon({ method: "register_user", handler: p.handler, display_name: p.displayName });
  }

  // Create channels + add members
  console.log(`[runner] Creating channels...`);
  // #general: send a message to create it (auto-create on first message)
  await callDaemon({ method: "send", channel: "general", body: "欢迎来到狼人杀！游戏即将开始。", author: "god" });

  // #wolves: create by sending, then manage membership
  await callDaemon({ method: "send", channel: "wolves", body: "狼人密谋频道已创建。", author: "god" });
  // Join wolves + god to #wolves
  for (const w of wolves) {
    await callDaemon({ method: "join_channel", channel: "wolves", targets: [w], author: "god" });
  }

  // God DMs each player their role
  console.log(`[runner] Sending role assignments via DM...`);
  for (const p of players) {
    const roleMsg = p.role === Role.Wolf
      ? `你的角色是【狼人】🐺。你的搭档: ${wolves.filter((w) => w !== p.handler).join(", ")}。在 #wolves 频道和搭档商量策略。`
      : `你的角色是【${p.role}】。请在游戏中发挥你的能力。`;
    await callDaemon({
      method: "send",
      channel: `dm:${p.handler <= "god" ? p.handler + ",god" : "god," + p.handler}`,
      body: roleMsg,
      author: "god",
    });
  }

  // Spawn agent processes
  console.log(`\n[runner] Spawning agent processes...`);
  const children: ChildProcess[] = [];

  // Spawn God
  const playersArg = players.map((p) => `${p.handler}:${p.role}`).join(",");
  const godProc = spawn("tsx", [
    "src/werewolf/god-agent.ts",
    "--daemon-url", daemonUrl,
    "--players", playersArg,
  ], {
    cwd: process.cwd(),
    stdio: ["ignore", "inherit", "inherit"],
    env: { ...process.env, GITIM_DAEMON_URL: daemonUrl },
  });
  children.push(godProc);
  console.log(`  [runner] spawned god (pid=${godProc.pid})`);

  // Spawn Players
  for (const p of players) {
    const playerProc = spawn("tsx", [
      "src/werewolf/player-agent.ts",
      "--handler", p.handler,
      "--role", p.role,
      "--display-name", p.displayName,
      "--personality", p.personality,
      "--daemon-url", daemonUrl,
      "--wolf-partners", wolves.filter((w) => w !== p.handler).join(","),
      "--wolves", wolves.join(","),
    ], {
      cwd: process.cwd(),
      stdio: ["ignore", "inherit", "inherit"],
      env: { ...process.env, GITIM_DAEMON_URL: daemonUrl },
    });
    children.push(playerProc);
    console.log(`  [runner] spawned ${p.handler} (pid=${playerProc.pid})`);
  }

  // Monitor: wait for God to exit (game over), then kill remaining
  console.log(`\n[runner] Game in progress... (waiting for God to finish)\n`);

  godProc.on("exit", (code) => {
    console.log(`\n[runner] God exited (code=${code}). Cleaning up...`);
    for (const child of children) {
      if (child !== godProc && !child.killed) {
        child.kill("SIGTERM");
      }
    }
    setTimeout(() => {
      // Force kill any remaining
      for (const child of children) {
        if (!child.killed) child.kill("SIGKILL");
      }
      console.log(`[runner] All processes terminated. Game complete.\n`);
      process.exit(0);
    }, 3000);
  });

  // Handle ctrl+c
  process.on("SIGINT", () => {
    console.log(`\n[runner] Interrupted. Killing all processes...`);
    for (const child of children) {
      if (!child.killed) child.kill("SIGTERM");
    }
    process.exit(1);
  });
}

main().catch((err) => {
  console.error("[runner] Fatal:", err);
  process.exit(1);
});
```

- [ ] **Step 2: Commit**

```bash
git add sim/src/werewolf/runner.ts
git commit -m "feat(werewolf): add Game Runner — spawn + lifecycle management"
```

---

### Task 9: E2E Mock Test (no real LLM)

**Files:**
- Create: `sim/src/werewolf/__tests__/e2e-mock.test.ts`

用 mock 验证：types → context manager → formatInjection → tool executor 的完整路径。不调用真实 LLM。

- [ ] **Step 1: Write the test**

```typescript
// sim/src/werewolf/__tests__/e2e-mock.test.ts
import { describe, it, expect } from "vitest";
import { Role, getVisibleChannels } from "../types.js";
import { formatInjection, ChannelMessages, maxLineFromMessages } from "../context-manager.js";
import { makePlayerPrompt, GOD_SYSTEM_PROMPT } from "../prompts.js";

describe("E2E mock: full pipeline without LLM", () => {
  const wolves = ["dave", "eve"];

  describe("role assignment → visibility → context", () => {
    it("wolf gets correct visibility and context injection", () => {
      // Step 1: visibility
      const channels = getVisibleChannels("dave", Role.Wolf, wolves);
      expect(channels).toContain("wolves");
      expect(channels).toContain("general");

      // Step 2: mock messages
      const msgs: ChannelMessages = {
        general: [{ author: "god", body: "天黑了", line_number: 1, timestamp: "20260325T100000Z" }],
        wolves: [{ author: "eve", body: "杀 alice", line_number: 1, timestamp: "20260325T100100Z" }],
      };

      // Step 3: format injection
      const injection = formatInjection(msgs, "请在狼人频道讨论击杀目标。", ["alice 可能是预言家"]);
      expect(injection).toContain("#general");
      expect(injection).toContain("#wolves");
      expect(injection).toContain("杀 alice");
      expect(injection).toContain("alice 可能是预言家");
      expect(injection).toContain("请在狼人频道讨论");
    });

    it("villager cannot see wolf channel", () => {
      const channels = getVisibleChannels("bob", Role.Villager, wolves);
      expect(channels).not.toContain("wolves");

      // Even if wolf messages exist, villager's injection won't include them
      const msgs: ChannelMessages = {
        general: [{ author: "god", body: "天亮了", line_number: 2, timestamp: "20260325T100200Z" }],
      };
      const injection = formatInjection(msgs, "请发言。");
      expect(injection).not.toContain("wolves");
    });
  });

  describe("prompts", () => {
    it("God prompt contains game rules", () => {
      expect(GOD_SYSTEM_PROMPT).toContain("狼人杀");
      expect(GOD_SYSTEM_PROMPT).toContain("夜晚阶段");
      expect(GOD_SYSTEM_PROMPT).toContain("【游戏结束】");
    });

    it("wolf player prompt includes partner info", () => {
      const prompt = makePlayerPrompt({
        handler: "dave",
        role: Role.Wolf,
        personality: "你很狡猾。",
        wolfPartners: ["eve"],
      });
      expect(prompt).toContain("eve");
      expect(prompt).toContain("狼人");
      expect(prompt).toContain("伪装");
    });

    it("seer player prompt includes verify ability", () => {
      const prompt = makePlayerPrompt({
        handler: "alice",
        role: Role.Seer,
        personality: "你很聪明。",
      });
      expect(prompt).toContain("预言家");
      expect(prompt).toContain("验");
    });

    it("villager prompt has no special abilities", () => {
      const prompt = makePlayerPrompt({
        handler: "bob",
        role: Role.Villager,
        personality: "你很直率。",
      });
      expect(prompt).toContain("村民");
      expect(prompt).toContain("投票");
    });
  });

  describe("cursor tracking", () => {
    it("maxLineFromMessages returns correct cursors per channel", () => {
      const msgs: ChannelMessages = {
        general: [
          { author: "a", body: "hi", line_number: 3, timestamp: "" },
          { author: "b", body: "yo", line_number: 7, timestamp: "" },
        ],
        wolves: [
          { author: "c", body: "kill", line_number: 2, timestamp: "" },
        ],
      };
      const cursors = maxLineFromMessages(msgs);
      expect(cursors.general).toBe(7);
      expect(cursors.wolves).toBe(2);
    });
  });
});
```

- [ ] **Step 2: Run all tests**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/werewolf/sim
npx vitest run
```

Expected: all tests PASS.

- [ ] **Step 3: Commit**

```bash
git add sim/src/werewolf/__tests__/e2e-mock.test.ts
git commit -m "test(werewolf): add mock E2E tests for full pipeline without LLM"
```

---

### Task 10: Update package.json scripts

**Files:**
- Modify: `sim/package.json`

- [ ] **Step 1: Add werewolf script**

确保 `scripts` 包含：
```json
{
  "scripts": {
    "chat": "tsx src/chat-demo.ts",
    "werewolf": "tsx src/werewolf/runner.ts",
    "test": "vitest run",
    "test:watch": "vitest"
  }
}
```

- [ ] **Step 2: Final commit**

```bash
git add sim/package.json
git commit -m "chore(sim): add werewolf + test scripts"
```

---

## Verification Checklist

Run after all tasks complete:

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/werewolf/sim

# All unit + mock tests pass
npx vitest run

# Type check
npx tsc --noEmit

# Werewolf runner prints help without crashing
tsx src/werewolf/runner.ts --help 2>&1 || echo "runner loaded"
```

## NOT in scope (deferred)
- 真实 LLM 测试（用户明天亲自测）
- 上帝视角观察者 agent
- 人机混战
- 行为分析 / replay 系统
- 通用 scenario plugin 架构
- read API 成员权限修复
