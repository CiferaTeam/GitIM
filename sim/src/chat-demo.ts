import Anthropic from "@anthropic-ai/sdk";
import { gitimTools, executeTool } from "./tools.js";

// ── 三人聊天 Demo ───────────────────────────────────────────
//
//  3 个 agent 在 #discussion 频道讨论一个话题。
//  每个 agent 有不同的性格和立场。
//
//  流程:
//    1. 注册 3 个用户
//    2. 每个 agent 轮流发言 (observe → think → act)
//    3. 每轮: read 新消息 → LLM 生成回应 → send
//
//  环境变量:
//    LLM_API_KEY     — LLM API key (必须)
//    LLM_BASE_URL    — API endpoint (可选, 默认 Anthropic 官方)
//    LLM_MODEL       — 模型名 (可选, 默认 claude-sonnet-4-20250514)
//    GITIM_DAEMON_URL — mock daemon 地址 (可选, 默认 http://localhost:3000)
//
//  用法:
//    LLM_API_KEY=sk-... LLM_BASE_URL=https://api.minimaxi.com/anthropic LLM_MODEL=MiniMax-M2.7 \
//      tsx src/chat-demo.ts [--rounds 5] [--topic "话题"]

const DAEMON_URL = process.env.GITIM_DAEMON_URL ?? "http://localhost:3000";
const LLM_MODEL = process.env.LLM_MODEL ?? "claude-sonnet-4-20250514";
const CHANNEL = "discussion";

// ── Agent Definitions ───────────────────────────────────────

interface AgentDef {
  handler: string;
  displayName: string;
  personality: string;
}

const AGENTS: AgentDef[] = [
  {
    handler: "alice",
    displayName: "Alice",
    personality:
      "你是 Alice，一个乐观的技术创业者。你相信 AI 会让世界变得更好，喜欢引用具体的技术案例来支持你的观点。说话简洁有力，偶尔用类比。",
  },
  {
    handler: "bob",
    displayName: "Bob",
    personality:
      "你是 Bob，一个务实的工程师。你重视实际可行性，对过度乐观持怀疑态度。喜欢追问细节和边界情况。说话直接，有时带点冷幽默。",
  },
  {
    handler: "carol",
    displayName: "Carol",
    personality:
      "你是 Carol，一个哲学系出身的产品经理。你关注技术对人和社会的影响，喜欢提出被忽略的视角。说话温和但切中要害，善于总结和提炼。",
  },
];

// ── Daemon Client ───────────────────────────────────────────

async function callDaemon(payload: Record<string, unknown>): Promise<{ ok: boolean; data?: any; error?: string }> {
  const res = await fetch(`${DAEMON_URL}/api`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });
  return res.json();
}

// ── Agent Loop ──────────────────────────────────────────────

async function registerAgents(): Promise<void> {
  for (const agent of AGENTS) {
    const res = await callDaemon({
      method: "register_user",
      handler: agent.handler,
      display_name: agent.displayName,
    });
    if (!res.ok) throw new Error(`register ${agent.handler} failed: ${res.error}`);
    console.log(`[setup] registered @${agent.handler} (exists: ${res.data?.exists})`);
  }
}

async function runAgentTurn(
  client: Anthropic,
  agent: AgentDef,
  lastSeenLine: number
): Promise<number> {
  // 1. Observe: read new messages
  const readRes = await callDaemon({
    method: "read",
    channel: CHANNEL,
    since: lastSeenLine > 0 ? lastSeenLine : undefined,
  });

  const messages = readRes.data?.messages ?? [];
  const maxLine = messages.reduce(
    (max: number, m: { line_number: number }) => Math.max(max, m.line_number),
    lastSeenLine
  );

  // Format conversation history for the agent
  const chatHistory = messages
    .map((m: { author: string; body: string; line_number: number }) =>
      `[@${m.author}][L${m.line_number}] ${m.body}`
    )
    .join("\n");

  // 2. Think + Act: Claude decides what to say
  const systemPrompt = `${agent.personality}

你正在 GitIM 的 #${CHANNEL} 频道和其他人聊天。
你可以使用工具来发送消息和读取消息。
请用中文交流，每次只发一条消息，控制在 2-3 句话以内。
如果你想回复某条特定消息，使用 reply_to 参数指定行号。`;

  const userMessage = chatHistory
    ? `频道里的最新消息:\n\n${chatHistory}\n\n请发表你的看法。`
    : `你是第一个发言的人。请开始讨论。`;

  const response = await client.messages.create({
    model: LLM_MODEL,
    max_tokens: 1024,
    system: systemPrompt,
    tools: gitimTools,
    messages: [{ role: "user", content: userMessage }],
  });

  // Process tool calls
  for (const block of response.content) {
    if (block.type === "tool_use") {
      const result = await executeTool(
        block.name,
        block.input as Record<string, unknown>,
        agent.handler
      );
      console.log(`  [@${agent.handler}] ${block.name}(${JSON.stringify(block.input)})`);
      console.log(`  → ${result}`);
    } else if (block.type === "text" && block.text.trim()) {
      // If Claude responds with text but no tool call, send it as a message
      const sendRes = await callDaemon({
        method: "send",
        channel: CHANNEL,
        body: block.text.trim(),
        author: agent.handler,
      });
      if (sendRes.ok) {
        console.log(`  [@${agent.handler}] ${block.text.trim()}`);
      }
    }
  }

  // Return the latest line we've seen
  const finalRead = await callDaemon({ method: "read", channel: CHANNEL });
  const allMsgs = finalRead.data?.messages ?? [];
  return allMsgs.reduce(
    (max: number, m: { line_number: number }) => Math.max(max, m.line_number),
    maxLine
  );
}

// ── Main ────────────────────────────────────────────────────

async function main(): Promise<void> {
  const args = process.argv.slice(2);
  const roundsIdx = args.indexOf("--rounds");
  const topicIdx = args.indexOf("--topic");
  const rounds = roundsIdx >= 0 ? parseInt(args[roundsIdx + 1], 10) : 3;
  const topic = topicIdx >= 0 ? args[topicIdx + 1] : "AI agent 之间需要什么样的通信协议？";

  console.log(`\n=== GitIM 三人聊天 Demo ===`);
  console.log(`话题: ${topic}`);
  console.log(`轮数: ${rounds}`);
  console.log(`频道: #${CHANNEL}`);
  console.log(`参与者: ${AGENTS.map((a) => `@${a.handler}`).join(", ")}\n`);

  // Check daemon is running
  const statusRes = await callDaemon({ method: "status" }).catch(() => null);
  if (!statusRes?.ok) {
    console.error("错误: mock daemon 未运行。请先执行: npm run daemon");
    process.exit(1);
  }

  const llmApiKey = process.env.LLM_API_KEY ?? process.env.ANTHROPIC_API_KEY;
  const llmBaseURL = process.env.LLM_BASE_URL;
  const client = new Anthropic({
    apiKey: llmApiKey,
    baseURL: llmBaseURL || undefined,
    // Some Anthropic-compatible endpoints (e.g. MiniMax) expect Authorization: Bearer
    // instead of x-api-key. Send both to maximize compatibility.
    defaultHeaders: llmBaseURL
      ? { Authorization: `Bearer ${llmApiKey}` }
      : undefined,
  });
  console.log(`LLM: ${LLM_MODEL} @ ${process.env.LLM_BASE_URL ?? "api.anthropic.com"}\n`);

  // Setup
  await registerAgents();

  // Seed the topic as a system message (from first agent)
  await callDaemon({
    method: "send",
    channel: CHANNEL,
    body: `今天我们来讨论: ${topic}`,
    author: AGENTS[0].handler,
  });
  console.log(`\n[@${AGENTS[0].handler}] 今天我们来讨论: ${topic}\n`);

  // Run rounds
  const lastSeen: Record<string, number> = {};
  for (const a of AGENTS) lastSeen[a.handler] = 0;

  for (let round = 1; round <= rounds; round++) {
    console.log(`\n--- Round ${round} ---\n`);
    for (const agent of AGENTS) {
      lastSeen[agent.handler] = await runAgentTurn(
        client,
        agent,
        lastSeen[agent.handler]
      );
      // Small delay to avoid rate limiting
      await new Promise((r) => setTimeout(r, 500));
    }
  }

  // Final summary
  console.log(`\n=== 聊天结束 ===\n`);
  const finalRead = await callDaemon({ method: "read", channel: CHANNEL });
  const total = finalRead.data?.messages?.length ?? 0;
  console.log(`共 ${total} 条消息\n`);
}

main().catch((err) => {
  console.error("Fatal:", err);
  process.exit(1);
});
