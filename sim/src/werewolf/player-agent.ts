#!/usr/bin/env tsx
/**
 * player-agent.ts — 狼人杀玩家进程
 *
 * 每个玩家是独立进程，由 runner.ts 启动。
 * 通过 GitIM daemon HTTP API 通信，不直接读取频道，
 * 而是由 Context Manager 注入消息。
 *
 * 用法:
 *   tsx src/werewolf/player-agent.ts \
 *     --handler alice --role seer --display-name Alice \
 *     --personality "你很谨慎..." --wolves "dave,eve"
 */

import Anthropic from "@anthropic-ai/sdk";
import { Role, getVisibleChannels } from "./types.js";
import { formatInjection, pollVisibleChannels, maxLineFromMessages } from "./context-manager.js";
import { makePlayerPrompt } from "./prompts.js";
import { gitimTools, executeTool } from "../tools.js";

// ── CLI Args ──────────────────────────────────────────────

function parseArgs(argv: string[]): {
  handler: string;
  role: Role;
  displayName: string;
  personality: string;
  daemonUrl: string;
  wolfPartners: string[];
  wolves: string[];
} {
  const args = argv.slice(2);
  const get = (flag: string): string | undefined => {
    const idx = args.indexOf(flag);
    return idx >= 0 ? args[idx + 1] : undefined;
  };

  const handler = get("--handler");
  const roleStr = get("--role");
  const displayName = get("--display-name");
  const personality = get("--personality");
  const daemonUrl = get("--daemon-url") ?? process.env.GITIM_DAEMON_URL ?? "http://localhost:3000";
  const wolfPartnersStr = get("--wolf-partners") ?? "";
  const wolvesStr = get("--wolves");

  if (!handler || !roleStr || !displayName || !personality) {
    console.error("用法: --handler <h> --role <r> --display-name <n> --personality <p> --wolves <w>");
    process.exit(1);
  }
  if (!wolvesStr) {
    console.error("--wolves 参数必须提供（所有狼人的 handler 列表）");
    process.exit(1);
  }

  const role = roleStr as Role;
  if (!Object.values(Role).includes(role)) {
    console.error(`未知角色: ${roleStr}`);
    process.exit(1);
  }

  const wolfPartners = wolfPartnersStr ? wolfPartnersStr.split(",").filter(Boolean) : [];
  const wolves = wolvesStr.split(",").filter(Boolean);

  return { handler, role, displayName, personality, daemonUrl, wolfPartners, wolves };
}

// ── Player Tools (subset) ─────────────────────────────────

const PLAYER_TOOL_NAMES = new Set(["send_message", "list_users"]);
const playerTools = gitimTools.filter((t) => PLAYER_TOOL_NAMES.has(t.name));

// ── DM Channel Helper ─────────────────────────────────────

function dmChannel(a: string, b: string): string {
  return a <= b ? `dm:${a},${b}` : `dm:${b},${a}`;
}

// ── Detect Relevance ──────────────────────────────────────

function hasRelevantActivity(
  handler: string,
  role: Role,
  channelMessages: Record<string, { author: string; body: string; line_number: number; timestamp: string }[]>
): boolean {
  // Check if God @mentions my handler in any channel
  for (const msgs of Object.values(channelMessages)) {
    for (const m of msgs) {
      if (m.body.includes(`@${handler}`)) return true;
    }
  }

  // Check God DM channel
  const godDm = dmChannel(handler, "god");
  if (channelMessages[godDm]?.length) return true;

  // Wolf channel activity for wolves
  if (role === Role.Wolf && channelMessages["wolves"]?.length) return true;

  // General channel has new messages (participate in discussion)
  if (channelMessages["general"]?.length) return true;

  return false;
}

// ── Determine Task ────────────────────────────────────────

function determineTask(
  handler: string,
  role: Role,
  channelMessages: Record<string, { author: string; body: string }[]>
): string {
  const godDm = dmChannel(handler, "god");
  const godDmMsgs = channelMessages[godDm] ?? [];
  const generalMsgs = channelMessages["general"] ?? [];
  const wolfMsgs = channelMessages["wolves"] ?? [];

  // God DM takes priority
  if (godDmMsgs.length > 0) {
    const lastGodMsg = godDmMsgs[godDmMsgs.length - 1];
    return `上帝通过私信联系了你："${lastGodMsg.body}"。请通过 DM 回复上帝。`;
  }

  // Wolf channel activity
  if (role === Role.Wolf && wolfMsgs.length > 0) {
    return "狼人频道有新消息，请与同伴讨论击杀目标。";
  }

  // General channel @mention
  for (const m of generalMsgs) {
    if (m.body.includes(`@${handler}`)) {
      return `你在 #general 被 @mention 了："${m.body}"。请在 #general 发言。`;
    }
  }

  // General channel discussion
  if (generalMsgs.length > 0) {
    return "请根据 #general 频道的讨论内容思考并行动。";
  }

  return "请等待上帝的指示。";
}

// ── Main Loop ─────────────────────────────────────────────

async function main(): Promise<void> {
  const config = parseArgs(process.argv);
  const { handler, role, displayName, personality, daemonUrl, wolfPartners, wolves } = config;

  const tag = `[${handler}]`;
  console.log(`${tag} 启动 — 角色: ${role}, 显示名: ${displayName}`);

  // Override daemon URL for tools module
  process.env.GITIM_DAEMON_URL = daemonUrl;

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
    defaultHeaders: llmBaseURL
      ? { Authorization: `Bearer ${llmApiKey}` }
      : undefined,
  });

  // System prompt
  const systemPrompt = makePlayerPrompt({ handler, role, personality, wolfPartners });

  // Visible channels
  const visibleChannels = getVisibleChannels(handler, role, wolves);
  const thinkingChannel = dmChannel(handler, handler);

  // Persistent conversation
  const messages: Anthropic.Messages.MessageParam[] = [];
  const thinkingHistory: string[] = [];

  // Per-channel cursors (track last seen line number)
  const sinceLines: Record<string, number> = {};
  for (const ch of visibleChannels) sinceLines[ch] = 0;

  console.log(`${tag} 可见频道: ${visibleChannels.join(", ")}`);
  console.log(`${tag} 思考频道: ${thinkingChannel}`);

  // Poll loop
  while (true) {
    // 1. Poll visible channels for new messages
    const channelMessages = await pollVisibleChannels(daemonUrl, visibleChannels, sinceLines);

    // Update cursors
    const newCursors = maxLineFromMessages(channelMessages);
    for (const [ch, line] of Object.entries(newCursors)) {
      if (line > (sinceLines[ch] ?? 0)) sinceLines[ch] = line;
    }

    // Filter out own thinking channel messages (don't react to own thinking)
    const filteredMessages = { ...channelMessages };
    delete filteredMessages[thinkingChannel];

    // Count total new messages (excluding thinking channel)
    const totalNew = Object.values(filteredMessages).reduce((sum, msgs) => sum + msgs.length, 0);

    if (totalNew === 0) {
      await sleep(2000);
      continue;
    }

    // 2. Check for game end
    for (const msgs of Object.values(filteredMessages)) {
      for (const m of msgs) {
        if (m.body.includes("【游戏结束】")) {
          console.log(`${tag} 检测到游戏结束`);
          // Write final reflection to thinking channel
          const reflection = `游戏结束了。最终消息: ${m.body}`;
          await executeTool("send_message", { channel: thinkingChannel, body: reflection }, handler);
          console.log(`${tag} 写入最终反思，退出`);
          process.exit(0);
        }
      }
    }

    // 3. Check if messages are relevant to us
    if (!hasRelevantActivity(handler, role, filteredMessages)) {
      await sleep(2000);
      continue;
    }

    // 4. Build injection and call LLM
    const task = determineTask(handler, role, filteredMessages);
    const recentThinkingSlice = thinkingHistory.slice(-5);
    const injection = formatInjection(filteredMessages, task, recentThinkingSlice.length > 0 ? recentThinkingSlice : undefined);

    messages.push({ role: "user", content: injection });

    let response: Anthropic.Messages.Message;
    try {
      response = await callLLM(client, llmModel, systemPrompt, messages, playerTools);
    } catch (err) {
      console.error(`${tag} LLM 错误，5s 后重试:`, err);
      await sleep(5000);
      try {
        response = await callLLM(client, llmModel, systemPrompt, messages, playerTools);
      } catch (retryErr) {
        console.error(`${tag} LLM 重试失败，跳过本轮:`, retryErr);
        // Remove the injection we just pushed since we couldn't process it
        messages.pop();
        await sleep(2000);
        continue;
      }
    }

    // 5. Process response
    messages.push({ role: "assistant", content: response.content });

    const toolResults: Anthropic.Messages.ToolResultBlockParam[] = [];

    for (const block of response.content) {
      if (block.type === "text" && block.text.trim()) {
        // Text response → thinking channel
        const thinking = block.text.trim();
        thinkingHistory.push(thinking);
        if (thinkingHistory.length > 20) thinkingHistory.shift();

        console.log(`${tag} thinking...`);
        await executeTool("send_message", { channel: thinkingChannel, body: thinking }, handler);
      } else if (block.type === "tool_use") {
        console.log(`${tag} ${block.name}(${JSON.stringify(block.input)})`);
        try {
          const result = await executeTool(
            block.name,
            block.input as Record<string, unknown>,
            handler
          );
          toolResults.push({
            type: "tool_result",
            tool_use_id: block.id,
            content: result,
          });
        } catch (err) {
          const errorMsg = err instanceof Error ? err.message : String(err);
          console.error(`${tag} 工具执行失败: ${errorMsg}`);
          toolResults.push({
            type: "tool_result",
            tool_use_id: block.id,
            content: errorMsg,
            is_error: true,
          });
        }
      }
    }

    // Append tool results if any
    if (toolResults.length > 0) {
      messages.push({ role: "user", content: toolResults });
    }

    // If stop_reason is tool_use, continue the conversation to get final response
    if (response.stop_reason === "tool_use" && toolResults.length > 0) {
      try {
        const followUp = await callLLM(client, llmModel, systemPrompt, messages, playerTools);
        messages.push({ role: "assistant", content: followUp.content });

        for (const block of followUp.content) {
          if (block.type === "text" && block.text.trim()) {
            const thinking = block.text.trim();
            thinkingHistory.push(thinking);
            if (thinkingHistory.length > 20) thinkingHistory.shift();
            console.log(`${tag} thinking...`);
            await executeTool("send_message", { channel: thinkingChannel, body: thinking }, handler);
          } else if (block.type === "tool_use") {
            console.log(`${tag} ${block.name}(${JSON.stringify(block.input)})`);
            try {
              const result = await executeTool(
                block.name,
                block.input as Record<string, unknown>,
                handler
              );
              messages.push({
                role: "user",
                content: [{
                  type: "tool_result",
                  tool_use_id: block.id,
                  content: result,
                }],
              });
            } catch (err) {
              const errorMsg = err instanceof Error ? err.message : String(err);
              messages.push({
                role: "user",
                content: [{
                  type: "tool_result",
                  tool_use_id: block.id,
                  content: errorMsg,
                  is_error: true,
                }],
              });
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

// ── Helpers ───────────────────────────────────────────────

async function callLLM(
  client: Anthropic,
  model: string,
  system: string,
  messages: Anthropic.Messages.MessageParam[],
  tools: Anthropic.Messages.Tool[]
): Promise<Anthropic.Messages.Message> {
  return client.messages.create({
    model,
    max_tokens: 1024,
    system,
    tools,
    messages,
  });
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

// ── Entry Point ───────────────────────────────────────────

main().catch((err) => {
  console.error("Fatal:", err);
  process.exit(1);
});
