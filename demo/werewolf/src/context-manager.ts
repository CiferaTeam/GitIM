// ── Poll Response Types (match daemon poll API) ──────────

export interface PollEntry {
  type: "message" | "event";
  line_number: number;
  author: string;
  timestamp: string;
  body?: string;
  point_to?: number;
  mentions?: string[];
  event_type?: string;
  meta?: Record<string, unknown>;
}

export interface PollChange {
  channel: string;
  kind: string;
  entries: PollEntry[];
}

export interface PollResult {
  commit_id: string;
  changes: PollChange[];
}

// ── Format Poll Changes for LLM Injection ────────────────

export function formatPollChanges(
  changes: PollChange[],
  task: string,
  recentThinking?: string[]
): string {
  const sections: string[] = [];

  for (const change of changes) {
    const messages = change.entries.filter((e) => e.type === "message" && e.body);
    if (messages.length === 0) continue;

    const label = change.channel.startsWith("dm:")
      ? `DM(${change.channel.slice(3)})`
      : `#${change.channel}`;
    sections.push(`=== ${label} 新消息 (${messages.length}条) ===`);
    for (const m of messages) {
      sections.push(
        `[L${String(m.line_number).padStart(6, "0")}][@${m.author}][${m.timestamp}] ${m.body}`
      );
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
