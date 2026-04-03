#!/usr/bin/env tsx
/**
 * player-agent.ts — 狼人杀玩家进程（claude -p 薄壳）
 *
 * 壳的职责：poll GitIM → 格式化消息 → claude -p --resume
 * Claude 通过 Bash 调用 gitim CLI 完成所有游戏操作。
 */

import { spawnSync } from "node:child_process";
import { callDaemon } from "./tools.js";
import { formatPollChanges, type PollChange, type PollResult } from "./context-manager.js";
import { makePlayerSystemPrompt } from "./prompts.js";
import { dmChannel } from "./types.js";

// ── CLI Args ──────────────────────────────────────────────

function parsePlayerArgs(argv: string[]) {
  const args = argv.slice(2);
  const get = (flag: string): string | undefined => {
    const idx = args.indexOf(flag);
    return idx >= 0 ? args[idx + 1] : undefined;
  };

  const handler = get("--handler");
  const displayName = get("--display-name");
  const personality = get("--personality");
  const socketPath = get("--socket-path");
  const repoDir = get("--repo-dir");
  const gameId = parseInt(get("--game-id") ?? "1", 10);

  if (!handler || !displayName || !personality || !socketPath || !repoDir) {
    console.error("用法: --handler <h> --display-name <n> --personality <p> --socket-path <s> --repo-dir <d> [--game-id N]");
    process.exit(1);
  }

  return { handler, displayName, personality, socketPath, repoDir, gameId };
}

// ── Claude -p Wrapper ────────────────────────────────────

interface ClaudeResult {
  result: string;
  session_id: string;
  is_error?: boolean;
}

function callClaude(prompt: string, systemPrompt: string, repoDir: string, sessionId?: string): ClaudeResult {
  const args = [
    "-p", prompt,
    "--system-prompt", systemPrompt,
    "--output-format", "json",
    "--allowedTools", "Bash(gitim *)",
    "--max-turns", "500",
  ];
  if (sessionId) {
    args.push("--resume", sessionId);
  }

  const result = spawnSync("claude", args, {
    cwd: repoDir,
    encoding: "utf-8",
    timeout: 300_000, // 5 min
    stdio: ["pipe", "pipe", "pipe"],
    env: { ...process.env, CLAUDE_CODE_MAX_OUTPUT_TOKENS: "4096" },
  });

  if (result.error) throw result.error;
  if (result.status !== 0) {
    const detail = result.stderr || result.stdout || "(no output)";
    throw new Error(`claude exited with ${result.status}: ${detail.slice(0, 500)}`);
  }

  return parseClaudeOutput(result.stdout.trim());
}

function parseClaudeOutput(raw: string): ClaudeResult {
  const events = JSON.parse(raw) as Array<Record<string, unknown>>;
  const init = events.find((e) => e.type === "system" && e.subtype === "init");
  const resultEvent = events.findLast((e) => e.type === "result");
  return {
    session_id: (init?.session_id ?? resultEvent?.session_id ?? "") as string,
    result: (resultEvent?.result ?? "") as string,
    is_error: (resultEvent?.is_error ?? false) as boolean,
  };
}

// ── Determine Task from Poll Changes ─────────────────────

function describeChanges(handler: string, changes: PollChange[], gameId: number): string {
  const godDm = dmChannel(handler, "god");
  const gameChannel = `werewolf-${gameId}`;
  const wolvesChannel = `werewolf-wolves-${gameId}`;

  const parts: string[] = [];

  for (const ch of changes) {
    const msgs = ch.entries.filter((e) => e.type === "message" && e.body);
    if (msgs.length === 0) continue;

    if (ch.channel === godDm) {
      parts.push("上帝通过私信联系了你，请通过 DM 回复上帝。");
    } else if (ch.channel === wolvesChannel) {
      parts.push("狼人频道有新消息，请在狼人频道与同伴讨论。");
    } else if (ch.channel === gameChannel) {
      const mentioned = msgs.some((m) => m.body?.includes(`@${handler}`));
      if (mentioned) {
        parts.push("你在游戏频道被 @mention 了，请在游戏频道发言。");
      } else {
        parts.push("游戏频道有新消息，请根据讨论内容决定是否行动。");
      }
    }
  }

  return parts.length > 0 ? parts.join("\n") : "有新消息，请根据内容思考和行动。";
}

// ── Main Loop ─────────────────────────────────────────────

async function main(): Promise<void> {
  const { handler, displayName, personality, socketPath, repoDir, gameId } = parsePlayerArgs(process.argv);

  const tag = `[${handler}]`;
  console.log(`${tag} 启动 — 显示名: ${displayName}, repo: ${repoDir}`);

  const systemPrompt = makePlayerSystemPrompt({ handler, personality, gameId });
  const thinkingChannel = dmChannel(handler, handler);

  // Poll cursor
  let pollCursor: string | null = null;
  try {
    const init = (await callDaemon(socketPath, { method: "poll", since: null })) as PollResult;
    pollCursor = init.commit_id;
  } catch (err) {
    console.error(`${tag} 无法获取初始 poll cursor:`, err);
    process.exit(1);
  }

  let sessionId: string | undefined;

  while (true) {
    let pollResult: PollResult;
    try {
      pollResult = (await callDaemon(socketPath, { method: "poll", since: pollCursor })) as PollResult;
    } catch {
      await sleep(2000);
      continue;
    }

    pollCursor = pollResult.commit_id;

    // Filter relevant changes
    const gameChannel = `werewolf-${gameId}`;
    const wolvesChannel = `werewolf-wolves-${gameId}`;
    const relevant = pollResult.changes.filter((ch) => {
      if (ch.channel === thinkingChannel) return false;
      if (ch.channel.startsWith("werewolf-") && ch.channel !== gameChannel && ch.channel !== wolvesChannel) return false;
      return ch.entries.some((e) => e.type === "message" && e.body);
    });

    if (relevant.length === 0) {
      await sleep(2000);
      continue;
    }

    // Check for game end
    for (const ch of relevant) {
      for (const e of ch.entries) {
        if (e.body?.includes("【游戏结束】")) {
          console.log(`${tag} 检测到游戏结束，退出`);
          process.exit(0);
        }
      }
    }

    // Build prompt from poll changes
    const task = describeChanges(handler, relevant, gameId);
    const injection = formatPollChanges(relevant, task);

    console.log(`${tag} 收到新消息，调用 claude -p ...`);

    try {
      const result = callClaude(injection, systemPrompt, repoDir, sessionId);
      sessionId = result.session_id;

      if (result.is_error) {
        console.error(`${tag} claude -p 错误: ${result.result}`);
      } else {
        console.log(`${tag} claude -p 完成 (session: ${sessionId})`);
      }
    } catch (err) {
      console.error(`${tag} claude -p 调用失败:`, err instanceof Error ? err.message : err);
      // session 可能已损坏，重置
      sessionId = undefined;
    }

    await sleep(1000);
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

main().catch((err) => {
  console.error("Fatal:", err);
  process.exit(1);
});
