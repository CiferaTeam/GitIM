#!/usr/bin/env tsx
/**
 * god-agent.ts — 狼人杀上帝进程（claude -p 薄壳）
 *
 * 壳的职责：poll GitIM → 格式化消息 → claude -p --resume
 * Claude 通过 Bash 调用 gitim CLI 完成所有游戏操作。
 *
 * God 的特殊之处：首次调用包含 kickoff 消息（玩家列表和角色分配），
 * 后续通过 poll 推送玩家回复。
 */

import { execSync } from "node:child_process";
import { parseArgs } from "node:util";
import { Role } from "./types.js";
import { makeGodSystemPrompt } from "./prompts.js";
import { formatPollChanges, type PollResult } from "./context-manager.js";
import { callDaemon } from "./tools.js";

// ── CLI args ────────────────────────────────────────────────

const { values } = parseArgs({
  options: {
    players: { type: "string" },
    "socket-path": { type: "string" },
    "repo-dir": { type: "string" },
    "game-id": { type: "string" },
  },
  strict: true,
});

if (!values.players || !values["socket-path"] || !values["repo-dir"]) {
  console.error("Usage: god-agent --players 'alice:seer,...' --socket-path <path> --repo-dir <dir> [--game-id N]");
  process.exit(1);
}

const socketPath = values["socket-path"]!;
const repoDir = values["repo-dir"]!;
const gameId = parseInt(values["game-id"] ?? "1", 10);

// Parse "alice:seer,bob:villager,..."
const playerRoles = new Map<string, Role>();
for (const entry of values.players!.split(",")) {
  const [handler, roleStr] = entry.trim().split(":");
  if (!handler || !roleStr || !(Object.values(Role) as string[]).includes(roleStr)) {
    console.error(`Invalid player entry: "${entry}". Expected "handler:role".`);
    process.exit(1);
  }
  playerRoles.set(handler, roleStr as Role);
}

// ── Claude -p Wrapper ────────────────────────────────────

interface ClaudeResult {
  result: string;
  session_id: string;
  is_error?: boolean;
}

function callClaude(prompt: string, systemPrompt: string, sessionId?: string): ClaudeResult {
  const args = [
    "claude", "-p", JSON.stringify(prompt),
    "--bare",
    "--system-prompt", JSON.stringify(systemPrompt),
    "--output-format", "json",
    "--allowedTools", "Bash(gitim *)",
    "--max-turns", "1000",
  ];
  if (sessionId) {
    args.push("--resume", sessionId);
  }

  const result = execSync(args.join(" "), {
    cwd: repoDir,
    encoding: "utf-8",
    timeout: 600_000, // 10 min — God turns can be long
    stdio: ["pipe", "pipe", "pipe"],
    env: { ...process.env, CLAUDE_CODE_MAX_OUTPUT_TOKENS: "8192" },
  });

  return JSON.parse(result.trim());
}

// ── Build kickoff message ──────────────────────────────────

function buildKickoff(): string {
  const handlers = [...playerRoles.keys()];
  const wolfHandlers = handlers.filter((h) => playerRoles.get(h) === Role.Wolf);

  const lines = [
    `这是第 ${gameId} 局游戏。`,
    `游戏频道名：werewolf-${gameId}，狼人频道名：werewolf-wolves-${gameId}。`,
    "",
    `以下 ${playerRoles.size} 名玩家已在 GitIM 上就绪，可以通信：`,
    "",
  ];
  for (const [handler, role] of playerRoles) {
    lines.push(`- @${handler} -> ${role}`);
  }
  lines.push("");
  lines.push(`狼人同伴关系：${wolfHandlers.join("、")} 互为同伴，请在角色 DM 中告知。`);
  lines.push("");
  lines.push("请按照游戏规则的「设置阶段」开始。先创建游戏频道和狼人频道，然后逐一 DM 分配角色。");

  return lines.join("\n");
}

// ── Main loop ───────────────────────────────────────────────

async function main() {
  const systemPrompt = makeGodSystemPrompt(gameId);

  let pollCursor: string | null = null;
  try {
    const init = (await callDaemon(socketPath, { method: "poll", since: null })) as PollResult;
    pollCursor = init.commit_id;
  } catch (err) {
    console.error("[god] 无法获取初始 poll cursor:", err);
    process.exit(1);
  }

  // First call: kickoff
  console.log("[god] 发送 kickoff，启动游戏设置...");
  let sessionId: string | undefined;
  try {
    const result = callClaude(buildKickoff(), systemPrompt);
    sessionId = result.session_id;
    console.log(`[god] kickoff 完成 (session: ${sessionId})`);

    if (result.result.includes("【游戏结束】")) {
      console.log("[god] 游戏结束。");
      process.exit(0);
    }
  } catch (err) {
    console.error("[god] kickoff 失败:", err instanceof Error ? err.message : err);
    process.exit(1);
  }

  // Poll loop
  while (true) {
    // Wait for player responses
    const POLL_INTERVAL = 5000;
    const POLL_TIMEOUT = 180_000; // 3 min
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
          allChanges.push(...newMessages);
          // Wait a bit more to batch concurrent replies
          await sleep(3000);
          // Grab any additional messages that came in
          const extra = (await callDaemon(socketPath, { method: "poll", since: pollCursor })) as PollResult;
          pollCursor = extra.commit_id;
          const extraMsgs = extra.changes.filter((ch) =>
            ch.entries.some((e) => e.type === "message" && e.body)
          );
          if (extraMsgs.length > 0) allChanges.push(...extraMsgs);
          break;
        }
      } catch { /* ignore, retry */ }
      await sleep(POLL_INTERVAL);
    }

    let prompt: string;
    if (allChanges.length > 0) {
      prompt = formatPollChanges(allChanges, "以上是新收到的玩家消息，请根据游戏进度继续推进。");
    } else {
      prompt = "已等待 3 分钟，没有收到新的玩家回复。请继续推进游戏流程（可能有玩家反应较慢，可以发消息提醒一次）。";
    }

    console.log(`[god] 推送消息给 claude -p (${allChanges.length} 个频道有更新)...`);

    try {
      const result = callClaude(prompt, systemPrompt, sessionId);
      sessionId = result.session_id;

      if (result.is_error) {
        console.error(`[god] claude -p 错误: ${result.result}`);
      } else {
        console.log(`[god] claude -p 完成`);
      }

      if (result.result.includes("【游戏结束】")) {
        console.log("[god] 游戏结束。");
        process.exit(0);
      }
    } catch (err) {
      console.error("[god] claude -p 调用失败:", err instanceof Error ? err.message : err);
      sessionId = undefined;
    }
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

main().catch((err) => {
  console.error("[god] Fatal error:", err);
  process.exit(1);
});
