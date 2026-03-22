import type Anthropic from "@anthropic-ai/sdk";

// ── GitIM Tool Schema ───────────────────────────────────────
//
//  Agent 通过 Claude tool use 调用这些工具和 gitim-daemon 通信。
//  和 gitim-daemon/src/api.rs 的 Request 对齐。
//
//  Agent → Claude tool use → tools → HTTP POST /api → gitim-daemon

const DAEMON_URL = process.env.GITIM_DAEMON_URL ?? "http://localhost:3000";

async function callDaemon(payload: Record<string, unknown>): Promise<unknown> {
  const res = await fetch(`${DAEMON_URL}/api`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });
  const json = await res.json();
  if (!json.ok) throw new Error(json.error ?? "daemon error");
  return json.data;
}

// ── Tool Definitions (Claude tool_use format) ───────────────

export const gitimTools: Anthropic.Messages.Tool[] = [
  {
    name: "send_message",
    description:
      "发送一条消息到指定频道。频道不存在会自动创建。可以通过 reply_to 回复特定消息。",
    input_schema: {
      type: "object" as const,
      properties: {
        channel: {
          type: "string",
          description: "频道名称，如 'general'。DM 格式: 'dm:handler1,handler2'",
        },
        body: { type: "string", description: "消息内容" },
        reply_to: {
          type: "number",
          description: "要回复的消息行号（可选，0 表示不回复）",
        },
      },
      required: ["channel", "body"],
    },
  },
  {
    name: "read_messages",
    description:
      "读取指定频道的消息。可以指定数量限制和起始行号过滤。",
    input_schema: {
      type: "object" as const,
      properties: {
        channel: { type: "string", description: "频道名称" },
        limit: { type: "number", description: "最多返回消息数（可选）" },
        since: {
          type: "number",
          description: "只返回行号大于此值的消息（可选）",
        },
      },
      required: ["channel"],
    },
  },
  {
    name: "list_channels",
    description: "列出所有频道。",
    input_schema: {
      type: "object" as const,
      properties: {},
      required: [],
    },
  },
  {
    name: "list_users",
    description: "列出所有已注册用户。",
    input_schema: {
      type: "object" as const,
      properties: {},
      required: [],
    },
  },
  {
    name: "get_thread",
    description:
      "获取指定消息的完整回复链（包括原始消息和所有回复）。",
    input_schema: {
      type: "object" as const,
      properties: {
        channel: { type: "string", description: "频道名称" },
        line_number: { type: "number", description: "起始消息行号" },
      },
      required: ["channel", "line_number"],
    },
  },
];

// ── Tool Executor ───────────────────────────────────────────

export async function executeTool(
  toolName: string,
  input: Record<string, unknown>,
  author: string
): Promise<string> {
  switch (toolName) {
    case "send_message": {
      const data = await callDaemon({
        method: "send",
        channel: input.channel,
        body: input.body,
        reply_to: input.reply_to ?? 0,
        author,
      });
      return JSON.stringify(data);
    }
    case "read_messages": {
      const data = await callDaemon({
        method: "read",
        channel: input.channel,
        limit: input.limit ?? null,
        since: input.since ?? null,
      });
      return JSON.stringify(data);
    }
    case "list_channels": {
      const data = await callDaemon({ method: "channels" });
      return JSON.stringify(data);
    }
    case "list_users": {
      const data = await callDaemon({ method: "users" });
      return JSON.stringify(data);
    }
    case "get_thread": {
      const data = await callDaemon({
        method: "thread",
        channel: input.channel,
        line_number: input.line_number,
      });
      return JSON.stringify(data);
    }
    default:
      throw new Error(`unknown tool: ${toolName}`);
  }
}
