export interface MessageEntry {
  author: string;
  body: string;
  line_number: number;
  timestamp: string;
}

export type ChannelMessages = Record<string, MessageEntry[]>;

export function formatInjection(
  channelMessages: ChannelMessages,
  task: string,
  recentThinking?: string[]
): string {
  const sections: string[] = [];

  for (const [channel, messages] of Object.entries(channelMessages)) {
    if (messages.length === 0) continue;
    const channelLabel = channel.startsWith("dm:") ? `DM(${channel.slice(3)})` : `#${channel}`;
    sections.push(`=== ${channelLabel} 新消息 (${messages.length}条) ===`);
    for (const m of messages) {
      sections.push(`[L${String(m.line_number).padStart(6, "0")}][@${m.author}][${m.timestamp}] ${m.body}`);
    }
    sections.push("");
  }

  if (recentThinking && recentThinking.length > 0) {
    sections.push(`=== 你的近期思考 (最近${recentThinking.length}条) ===`);
    recentThinking.forEach((t, i) => sections.push(`[思考${i + 1}] ${t}`));
    sections.push("");
  }

  sections.push("=== 当前任务 ===");
  sections.push(task);

  return sections.join("\n");
}

export async function pollVisibleChannels(
  daemonUrl: string,
  channels: string[],
  sinceLines: Record<string, number>
): Promise<ChannelMessages> {
  const result: ChannelMessages = {};

  for (const ch of channels) {
    const since = sinceLines[ch] ?? 0;
    try {
      const res = await fetch(`${daemonUrl}/api`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          method: "read",
          channel: ch,
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

export function maxLineFromMessages(channelMessages: ChannelMessages): Record<string, number> {
  const result: Record<string, number> = {};
  for (const [ch, msgs] of Object.entries(channelMessages)) {
    result[ch] = msgs.reduce((max, m) => Math.max(max, m.line_number), 0);
  }
  return result;
}
