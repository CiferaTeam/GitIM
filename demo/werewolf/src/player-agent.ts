#!/usr/bin/env tsx
/**
 * player-agent.ts — 狼人杀通用玩家进程
 *
 * 启动时不知道角色，等 God 通过 DM 通知。
 * 每个玩家有自己的 daemon 和 socket，poll 拉取消息。
 */

import Anthropic from "@anthropic-ai/sdk";
import { dmChannel } from "./types.js";
import { formatPollChanges, type PollChange, type PollResult } from "./context-manager.js";
import { makePlayerPrompt } from "./prompts.js";
import { gitimTools, executeTool, callDaemon } from "./tools.js";

// ── CLI Args ──────────────────────────────────────────────

function parseArgs(argv: string[]) {
  const args = argv.slice(2);
  const get = (flag: string): string | undefined => {
    const idx = args.indexOf(flag);
    return idx >= 0 ? args[idx + 1] : undefined;
  };

  const handler = get("--handler");
  const displayName = get("--display-name");
  const personality = get("--personality");
  const socketPath = get("--socket-path");

  if (!handler || !displayName || !personality || !socketPath) {
    console.error("用法: --handler <h> --display-name <n> --personality <p> --socket-path <s>");
    process.exit(1);
  }

  return { handler, displayName, personality, socketPath };
}

// ── Player Tools (subset) ─────────────────────────────────

const PLAYER_TOOL_NAMES = new Set(["send_message", "list_users"]);
const playerTools = gitimTools.filter((t) => PLAYER_TOOL_NAMES.has(t.name));

// ── Determine Task from Poll Changes ─────────────────────

function determineTask(handler: string, changes: PollChange[]): string {
  const godDm = dmChannel(handler, "god");

  for (const ch of changes) {
    const msgs = ch.entries.filter((e) => e.type === "message" && e.body);
    if (msgs.length === 0) continue;

    // God DM takes highest priority — must reply via DM
    if (ch.channel === godDm) {
      const last = msgs[msgs.length - 1];
      return `上帝通过私信联系了你："${last.body}"。你必须调用 send_message 工具，channel 填 "${godDm}" 来回复上帝。`;
    }
    // Wolf channel: werewolf-wolves-N
    if (ch.channel.startsWith("werewolf-wolves")) {
      return `狼人频道 #${ch.channel} 有新消息，请调用 send_message 工具在 #${ch.channel} 与同伴讨论。`;
    }
    // Game channel: werewolf-N (but not werewolf-wolves-N)
    if (ch.channel.startsWith("werewolf-")) {
      for (const m of msgs) {
        if (m.body?.includes(`@${handler}`)) {
          return `你在 #${ch.channel} 被 @mention 了："${m.body}"。请调用 send_message 工具在 #${ch.channel} 发言。`;
        }
      }
      return `游戏频道 #${ch.channel} 有新消息，请根据讨论内容调用 send_message 工具在 #${ch.channel} 发言或行动。`;
    }
    // General channel
    if (ch.channel === "general") {
      for (const m of msgs) {
        if (m.body?.includes(`@${handler}`)) {
          return `你在 #general 被 @mention 了："${m.body}"。请调用 send_message 工具在 #general 发言。`;
        }
      }
      return "请根据 #general 频道的讨论内容思考并行动。";
    }
  }

  return "请等待上帝的指示。";
}

// ── Main Loop ─────────────────────────────────────────────

async function main(): Promise<void> {
  const { handler, displayName, personality, socketPath } = parseArgs(process.argv);

  const tag = `[${handler}]`;
  console.log(`${tag} 启动 — 显示名: ${displayName}`);

  // LLM client
  const llmApiKey = process.env.LLM_API_KEY ?? process.env.ANTHROPIC_API_KEY;
  if (!llmApiKey) {
    console.error(`${tag} 缺少 LLM_API_KEY 或 ANTHROPIC_API_KEY`);
    process.exit(1);
  }
  const llmBaseURL = process.env.LLM_BASE_URL;
  const llmModel = process.env.LLM_MODEL ?? "claude-sonnet-4-20250514";

  const client = new Anthropic({
    apiKey: llmApiKey,
    baseURL: llmBaseURL || undefined,
    defaultHeaders: llmBaseURL ? { Authorization: `Bearer ${llmApiKey}` } : undefined,
  });

  const systemPrompt = makePlayerPrompt({ handler, personality });
  const thinkingChannel = dmChannel(handler, handler);
  const messages: Anthropic.Messages.MessageParam[] = [];
  const thinkingHistory: string[] = [];

  // Poll cursor
  let pollCursor: string | null = null;
  try {
    const init = (await callDaemon(socketPath, { method: "poll", since: null })) as PollResult;
    pollCursor = init.commit_id;
  } catch (err) {
    console.error(`${tag} 无法获取初始 poll cursor:`, err);
    process.exit(1);
  }

  console.log(`${tag} 思考频道: ${thinkingChannel}`);

  while (true) {
    let pollResult: PollResult;
    try {
      pollResult = (await callDaemon(socketPath, { method: "poll", since: pollCursor })) as PollResult;
    } catch {
      await sleep(2000);
      continue;
    }

    pollCursor = pollResult.commit_id;

    // Filter: only changes with messages, exclude own thinking channel
    const relevant = pollResult.changes.filter((ch) => {
      if (ch.channel === thinkingChannel) return false;
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
          console.log(`${tag} 检测到游戏结束`);
          await executeTool(socketPath, "send_message", {
            channel: thinkingChannel,
            body: `游戏结束了。最终消息: ${e.body}`,
          }, handler);
          console.log(`${tag} 写入最终反思，退出`);
          process.exit(0);
        }
      }
    }

    // Build injection and call LLM
    const task = determineTask(handler, relevant);
    const injection = formatPollChanges(
      relevant,
      task,
      thinkingHistory.length > 0 ? thinkingHistory.slice(-5) : undefined
    );

    messages.push({ role: "user", content: injection });

    let response: Anthropic.Messages.Message;
    try {
      response = await callLLM(client, llmModel, systemPrompt, messages, playerTools);
    } catch (err) {
      console.error(`${tag} LLM 错误，5s 后重试:`, err);
      await sleep(5000);
      try {
        response = await callLLM(client, llmModel, systemPrompt, messages, playerTools);
      } catch {
        messages.pop();
        await sleep(2000);
        continue;
      }
    }

    messages.push({ role: "assistant", content: response.content });
    const toolResults: Anthropic.Messages.ToolResultBlockParam[] = [];

    for (const block of response.content) {
      if (block.type === "text" && block.text.trim()) {
        const thinking = block.text.trim();
        thinkingHistory.push(thinking);
        if (thinkingHistory.length > 20) thinkingHistory.shift();
        console.log(`${tag} thinking...`);
        await executeTool(socketPath, "send_message", { channel: thinkingChannel, body: thinking }, handler);
      } else if (block.type === "tool_use") {
        console.log(`${tag} ${block.name}(${JSON.stringify(block.input)})`);
        try {
          const result = await executeTool(socketPath, block.name, block.input as Record<string, unknown>, handler);
          toolResults.push({ type: "tool_result", tool_use_id: block.id, content: result });
        } catch (err) {
          const errorMsg = err instanceof Error ? err.message : String(err);
          toolResults.push({ type: "tool_result", tool_use_id: block.id, content: errorMsg, is_error: true });
        }
      }
    }

    if (toolResults.length > 0) {
      messages.push({ role: "user", content: toolResults });
    }

    if (response.stop_reason === "tool_use" && toolResults.length > 0) {
      try {
        const followUp = await callLLM(client, llmModel, systemPrompt, messages, playerTools);
        messages.push({ role: "assistant", content: followUp.content });
        for (const block of followUp.content) {
          if (block.type === "text" && block.text.trim()) {
            thinkingHistory.push(block.text.trim());
            if (thinkingHistory.length > 20) thinkingHistory.shift();
            await executeTool(socketPath, "send_message", { channel: thinkingChannel, body: block.text.trim() }, handler);
          } else if (block.type === "tool_use") {
            try {
              const result = await executeTool(socketPath, block.name, block.input as Record<string, unknown>, handler);
              messages.push({ role: "user", content: [{ type: "tool_result", tool_use_id: block.id, content: result }] });
            } catch (err) {
              messages.push({ role: "user", content: [{ type: "tool_result", tool_use_id: block.id, content: String(err), is_error: true }] });
            }
          }
        }
      } catch (err) {
        console.error(`${tag} follow-up LLM 错误:`, err);
      }
    }

    await sleep(1000);
  }
}

async function callLLM(
  client: Anthropic,
  model: string,
  system: string,
  messages: Anthropic.Messages.MessageParam[],
  tools: Anthropic.Messages.Tool[]
): Promise<Anthropic.Messages.Message> {
  const stream = client.messages.stream({ model, max_tokens: 1024, system, tools, messages });
  return stream.finalMessage();
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

main().catch((err) => {
  console.error("Fatal:", err);
  process.exit(1);
});
