import net from "node:net";
import readline from "node:readline";
import type Anthropic from "@anthropic-ai/sdk";

// ── Socket Client ─────────────────────────────────────────
//
//  Line-delimited JSON over Unix socket.
//  Protocol 同 cli/src/client.ts.

export async function callDaemon(
  socketPath: string,
  payload: Record<string, unknown>
): Promise<unknown> {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection(socketPath);
    const message = JSON.stringify(payload) + "\n";

    socket.on("connect", () => socket.write(message));

    const rl = readline.createInterface({ input: socket });
    rl.on("error", () => {});
    rl.on("line", (line: string) => {
      try {
        const json = JSON.parse(line);
        if (!json.ok) reject(new Error(json.error ?? "daemon error"));
        else resolve(json.data);
      } catch {
        reject(new Error(`Invalid response: ${line}`));
      }
      socket.end();
    });

    socket.on("error", (err: Error) => {
      reject(new Error(`Cannot connect to daemon: ${err.message}`));
    });
  });
}

// ── Tool Definitions (Claude tool_use format) ─────────────

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
          description: "频道名称，如 'general'。DM 格式: 'dm:handler1,handler2'（两个 handler 必须按字母序排列，例如给 alice 发私信用 'dm:alice,god'，不是 'dm:god,alice'）",
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
  {
    name: "create_channel",
    description:
      "创建一个新频道。重要：创建后频道没有成员，必须紧接着调用 join_channel 将用户拉入，否则没人能看到频道消息。",
    input_schema: {
      type: "object" as const,
      properties: {
        name: { type: "string", description: "频道名称（小写字母、数字、连字符）" },
        introduction: { type: "string", description: "频道简介" },
      },
      required: ["name", "introduction"],
    },
  },
  {
    name: "join_channel",
    description:
      "将用户加入指定频道。targets 为空时表示自己加入，非空时表示拉其他用户入群。",
    input_schema: {
      type: "object" as const,
      properties: {
        channel: { type: "string", description: "频道名称" },
        targets: {
          type: "array",
          items: { type: "string" },
          description: "要拉入的用户 handler 列表（可选，为空则自己加入）",
        },
      },
      required: ["channel"],
    },
  },
];

// ── Tool Executor ─────────────────────────────────────────

export async function executeTool(
  socketPath: string,
  toolName: string,
  input: Record<string, unknown>,
  author: string
): Promise<string> {
  switch (toolName) {
    case "send_message": {
      const data = await callDaemon(socketPath, {
        method: "send",
        channel: input.channel,
        body: input.body,
        reply_to: input.reply_to ?? 0,
        author,
      });
      return JSON.stringify(data);
    }
    case "read_messages": {
      const data = await callDaemon(socketPath, {
        method: "read",
        channel: input.channel,
        limit: input.limit ?? null,
        since: input.since ?? null,
      });
      return JSON.stringify(data);
    }
    case "list_channels": {
      const data = await callDaemon(socketPath, { method: "channels" });
      return JSON.stringify(data);
    }
    case "list_users": {
      const data = await callDaemon(socketPath, { method: "users" });
      return JSON.stringify(data);
    }
    case "get_thread": {
      const data = await callDaemon(socketPath, {
        method: "thread",
        channel: input.channel,
        line_number: input.line_number,
      });
      return JSON.stringify(data);
    }
    case "create_channel": {
      const data = await callDaemon(socketPath, {
        method: "create_channel",
        name: input.name,
        introduction: input.introduction ?? "",
        author,
      });
      return JSON.stringify(data);
    }
    case "join_channel": {
      const data = await callDaemon(socketPath, {
        method: "join_channel",
        channel: input.channel,
        targets: input.targets ?? [],
        author,
      });
      return JSON.stringify(data);
    }
    default:
      throw new Error(`unknown tool: ${toolName}`);
  }
}
