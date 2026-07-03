import type { Message } from "@/lib/types";

export function getCardPreviewReadQuery(line?: number): { since?: number; limit: number } {
  if (line == null) return { limit: 8 };
  return { since: Math.max(0, line - 1), limit: 1 };
}

export function selectCardPreviewMessages(messages: Message[], line?: number): Message[] {
  const realMessages = messages.filter((m) => m.type !== "event");
  if (line == null) return realMessages.slice(-8);
  return realMessages.filter((msg) => msg.line_number === line).slice(0, 1);
}
