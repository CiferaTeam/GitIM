import type { Message } from "@/lib/types";

/** How many card-discussion messages to show in the hover preview. */
export const CARD_PREVIEW_LIMIT = 12;
/** Upper bound for loading a merged reply range into the lightweight preview. */
const CARD_REPLY_PREVIEW_FETCH_LIMIT = 1000;
/** Lines before the target to include when previewing a specific line. */
const CARD_PREVIEW_BEFORE = 5;
/** Total window size around a target line (before + target + after). */
const CARD_PREVIEW_WINDOW = 11;

export function getCardPreviewReadQuery(line?: number): { since?: number; limit: number } {
  if (line == null) return { limit: CARD_PREVIEW_LIMIT };
  return {
    since: Math.max(0, line - CARD_PREVIEW_BEFORE - 1),
    limit: CARD_PREVIEW_WINDOW,
  };
}

export function getCardReplyPreviewReadQuery(
  firstLine: number,
  replyCount: number,
): { since: number; limit: number } {
  const normalizedCount = Number.isFinite(replyCount)
    ? Math.max(CARD_PREVIEW_LIMIT, Math.floor(replyCount))
    : CARD_PREVIEW_LIMIT;
  return {
    since: Math.max(0, firstLine - 1),
    limit: Math.min(normalizedCount, CARD_REPLY_PREVIEW_FETCH_LIMIT),
  };
}

export function selectCardPreviewMessages(messages: Message[], line?: number): Message[] {
  const realMessages = messages.filter((m) => m.type !== "event");
  if (line == null) return realMessages.slice(-CARD_PREVIEW_LIMIT);
  const idx = realMessages.findIndex((m) => m.line_number === line);
  if (idx === -1) return realMessages.slice(-CARD_PREVIEW_LIMIT);
  return realMessages.slice(
    Math.max(0, idx - CARD_PREVIEW_BEFORE),
    idx - CARD_PREVIEW_BEFORE + CARD_PREVIEW_WINDOW,
  );
}
