/**
 * God Agent — LLM-driven game master process for werewolf.
 *
 * 独立进程，有自己的 daemon 和 socket。
 * 负责游戏全流程：设置（创建频道、分配角色、确认）→ 游戏循环。
 */

import Anthropic from "@anthropic-ai/sdk";
import { parseArgs } from "node:util";
import { Role } from "./types.js";
import { GOD_SYSTEM_PROMPT } from "./prompts.js";
import { formatPollChanges, type PollResult } from "./context-manager.js";
import { gitimTools, executeTool, callDaemon } from "./tools.js";

// ── CLI args ────────────────────────────────────────────────

const { values } = parseArgs({
  options: {
    players: { type: "string" },
    "socket-path": { type: "string" },
  },
  strict: true,
});

if (!values.players || !values["socket-path"]) {
  console.error("Usage: god-agent --players 'alice:seer,...' --socket-path <path>");
  process.exit(1);
}

const socketPath = values["socket-path"]!;

// Parse "alice:seer,bob:villager,..."
const playerRoles = new Map<string, Role>();
for (const entry of values.players.split(",")) {
  const [handler, roleStr] = entry.trim().split(":");
  if (!handler || !roleStr || !(Object.values(Role) as string[]).includes(roleStr)) {
    console.error(`Invalid player entry: "${entry}". Expected "handler:role".`);
    process.exit(1);
  }
  playerRoles.set(handler, roleStr as Role);
}

// ── Environment ─────────────────────────────────────────────

const apiKey = process.env.LLM_API_KEY;
if (!apiKey) {
  console.error("LLM_API_KEY environment variable is required");
  process.exit(1);
}

const baseURL = process.env.LLM_BASE_URL;
const model = process.env.LLM_MODEL ?? "claude-sonnet-4-20250514";

const client = new Anthropic({
  apiKey,
  ...(baseURL ? { baseURL, defaultHeaders: { Authorization: `Bearer ${apiKey}` } } : {}),
});

// ── Build kick-off message ──────────────────────────────────

function buildKickoff(): string {
  const handlers = [...playerRoles.keys()];
  const wolfHandlers = handlers.filter((h) => playerRoles.get(h) === Role.Wolf);

  const lines = [
    `以下 ${playerRoles.size} 名玩家已在 GitIM 上就绪，可以通信：`,
    "",
  ];
  for (const [handler, role] of playerRoles) {
    lines.push(`- @${handler} → ${role}`);
  }
  lines.push("");
  lines.push("注意事项：");
  lines.push(`- 狼人同伴关系：${wolfHandlers.join("、")} 互为同伴，请在角色 DM 中告知。`);
  lines.push("- DM 格式：两个 handler 按字母序排列，用 dm:handler1,handler2 格式。例如给 alice 发 DM 用 dm:alice,god。");
  lines.push("- 创建 #wolves 频道：先发一条消息到 channel \"wolves\"（自动创建），然后用 join_channel 工具逐个拉狼人成员入群。");
  lines.push("");
  lines.push("请按照你的系统提示中的「第一阶段：游戏设置」步骤开始。");

  return lines.join("\n");
}

// ── Helpers ──────────────────────────────────────────────────

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

// ── Main loop ───────────────────────────────────────────────

async function main() {
  const messages: Anthropic.Messages.MessageParam[] = [
    { role: "user", content: buildKickoff() },
  ];

  let pollCursor: string | null = null;
  let gameOver = false;

  try {
    const init = (await callDaemon(socketPath, { method: "poll", since: null })) as PollResult;
    pollCursor = init.commit_id;
  } catch (err) {
    console.error("[god] 无法获取初始 poll cursor:", err);
    process.exit(1);
  }

  while (!gameOver) {
    let response: Anthropic.Messages.Message;
    try {
      response = await client.messages.create({
        model,
        max_tokens: 4096,
        system: GOD_SYSTEM_PROMPT,
        tools: gitimTools,
        messages,
      });
    } catch (err) {
      console.error("[god] LLM error:", err);
      messages.push({ role: "user", content: "发生了错误，请继续推进游戏。" });
      continue;
    }

    messages.push({ role: "assistant", content: response.content });

    for (const block of response.content) {
      if (block.type === "text" && block.text.includes("【游戏结束】")) {
        gameOver = true;
      }
    }
    if (gameOver) break;

    // Handle tool_use
    if (response.stop_reason === "tool_use") {
      const toolResults: Anthropic.Messages.ToolResultBlockParam[] = [];
      for (const block of response.content) {
        if (block.type !== "tool_use") continue;
        try {
          const result = await executeTool(socketPath, block.name, block.input as Record<string, unknown>, "god");
          toolResults.push({ type: "tool_result", tool_use_id: block.id, content: result });
        } catch (err) {
          toolResults.push({
            type: "tool_result",
            tool_use_id: block.id,
            content: `Error: ${err instanceof Error ? err.message : String(err)}`,
            is_error: true,
          });
        }
      }
      messages.push({ role: "user", content: toolResults });
      continue;
    }

    // end_turn: poll for player messages with timeout
    const POLL_INTERVAL = 5000;
    const POLL_TIMEOUT = 120_000;
    const pollDeadline = Date.now() + POLL_TIMEOUT;
    let allChanges: PollResult["changes"] = [];

    await sleep(POLL_INTERVAL);
    while (Date.now() < pollDeadline) {
      try {
        const pollResult = (await callDaemon(socketPath, { method: "poll", since: pollCursor })) as PollResult;
        pollCursor = pollResult.commit_id;

        const newMessages = pollResult.changes.filter((ch) =>
          ch.entries.some((e) => e.type === "message" && e.body)
        );
        if (newMessages.length > 0) {
          allChanges.push(...pollResult.changes);
          break;
        }
      } catch { /* ignore, retry */ }
      await sleep(POLL_INTERVAL);
    }

    if (allChanges.length > 0) {
      messages.push({ role: "user", content: formatPollChanges(allChanges, "请根据以上新消息继续推进游戏。") });
    } else {
      messages.push({ role: "user", content: "已等待 120 秒，没有收到新的玩家回复。请继续推进游戏流程（可能有玩家离线）。" });
    }
  }

  console.log("[god] 游戏结束，进程退出。");
  process.exit(0);
}

main().catch((err) => {
  console.error("[god] Fatal error:", err);
  process.exit(1);
});
