/**
 * God Agent — LLM-driven game master process for werewolf.
 *
 * Spawned by runner.ts as a standalone process.
 * Drives the entire game via Anthropic tool_use loop.
 */

import Anthropic from "@anthropic-ai/sdk";
import { parseArgs } from "node:util";
import { Role, getVisibleChannels } from "./types.js";
import { GOD_SYSTEM_PROMPT } from "./prompts.js";
import {
  pollVisibleChannels,
  maxLineFromMessages,
  formatInjection,
  type ChannelMessages,
} from "./context-manager.js";
import { gitimTools, executeTool } from "./tools.js";

// ── CLI args ────────────────────────────────────────────────

const { values } = parseArgs({
  options: {
    players: { type: "string" },
    "daemon-url": { type: "string", default: "http://localhost:3000" },
  },
  strict: true,
});

if (!values.players) {
  console.error("Usage: god-agent --players 'alice:seer,bob:villager,...'");
  process.exit(1);
}

const daemonUrl = values["daemon-url"] ?? process.env.GITIM_DAEMON_URL ?? "http://localhost:3000";

// Parse "alice:seer,bob:villager,..." into a map
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

const client = new Anthropic({ apiKey, ...(baseURL ? { baseURL } : {}) });

// ── Visibility ──────────────────────────────────────────────

const allHandlers = [...playerRoles.keys()];
const wolfHandlers = allHandlers.filter((h) => playerRoles.get(h) === Role.Wolf);
const visibleChannels = getVisibleChannels("god", Role.God, wolfHandlers, allHandlers);

// ── Build initial kick-off message ──────────────────────────

function buildKickoff(): string {
  const lines = ["游戏开始！", "", "玩家角色分配（只有你知道）:"];
  for (const [handler, role] of playerRoles) {
    lines.push(`- ${handler}: ${role}`);
  }
  lines.push("");
  lines.push("请开始第一个夜晚阶段。先在 #general 宣布天黑，然后依次处理狼人、预言家、女巫的夜间行动。");
  return lines.join("\n");
}

// ── Helpers ──────────────────────────────────────────────────

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

function hasNewMessages(channelMessages: ChannelMessages): boolean {
  return Object.values(channelMessages).some((msgs) => msgs.length > 0);
}

// ── Main loop ───────────────────────────────────────────────

async function main() {
  const messages: Anthropic.Messages.MessageParam[] = [
    { role: "user", content: buildKickoff() },
  ];

  // Track per-channel read cursors so we only inject new messages
  let sinceLines: Record<string, number> = {};
  let gameOver = false;

  while (!gameOver) {
    // ── Call LLM ──────────────────────────────────────────
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
      messages.push({
        role: "user",
        content: "发生了错误，请继续推进游戏。",
      });
      continue;
    }

    // ── Process response ─────────────────────────────────
    // Append assistant turn
    messages.push({ role: "assistant", content: response.content });

    // Check for text containing game-over marker
    for (const block of response.content) {
      if (block.type === "text" && block.text.includes("【游戏结束】")) {
        gameOver = true;
      }
    }
    if (gameOver) break;

    // ── Handle tool_use ──────────────────────────────────
    if (response.stop_reason === "tool_use") {
      const toolResults: Anthropic.Messages.ToolResultBlockParam[] = [];
      for (const block of response.content) {
        if (block.type !== "tool_use") continue;
        try {
          const result = await executeTool(
            block.name,
            block.input as Record<string, unknown>,
            "god"
          );
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
      // Continue the loop -- LLM wants to do more
      continue;
    }

    // ── stop_reason === "end_turn": poll for player messages ─
    // Wait 3s, poll, if nothing wait 5s more (8s total), then timeout
    await sleep(3000);
    let polled = await pollVisibleChannels(daemonUrl, visibleChannels, sinceLines);

    if (!hasNewMessages(polled)) {
      await sleep(5000);
      polled = await pollVisibleChannels(daemonUrl, visibleChannels, sinceLines);
    }

    // Update cursors
    const newCursors = maxLineFromMessages(polled);
    for (const [ch, line] of Object.entries(newCursors)) {
      if (line > (sinceLines[ch] ?? 0)) {
        sinceLines[ch] = line;
      }
    }

    if (hasNewMessages(polled)) {
      const injection = formatInjection(polled, "请根据以上新消息继续推进游戏。");
      messages.push({ role: "user", content: injection });
    } else {
      // Timeout -- no player responses
      messages.push({
        role: "user",
        content: "已等待超时，没有收到新的玩家回复。请继续推进游戏流程。",
      });
    }
  }

  console.log("[god] 游戏结束，进程退出。");
  process.exit(0);
}

main().catch((err) => {
  console.error("[god] Fatal error:", err);
  process.exit(1);
});
